//! The Windows [`ProcessBackend`] (spine AD-4) — one Job Object per instance.
//!
//! Each Agent Instance is spawned and then assigned to its OWN Job Object
//! configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Stopping terminates the
//! whole job with `TerminateJobObject`, which kills EVERY process in the job —
//! the parent agent and any child processes it spawned — the Windows equivalent
//! of the Unix process-group kill behind AC3 "no process of the instance
//! survives". Closing the job handle (on drop) also kills the tree, so a dropped
//! handle never leaks processes.
//!
//! This module is the allowlisted home for OS-conditional code (it is
//! `#[cfg(windows)]`-gated at its `mod` declaration in `backends/mod.rs`). It
//! uses raw `windows-sys` Job-Object FFI.
//!
//! ## Assign-after-spawn race (documented `[ASSUMPTION]`)
//!
//! `std::process::Command` does not expose the child's main-thread handle, so a
//! `CREATE_SUSPENDED` + resume dance is not possible with std alone. We instead
//! spawn the child and assign it to the job IMMEDIATELY (before it does
//! meaningful work). Any descendant the child spawns AFTER assignment is in the
//! job and dies with it; the sub-millisecond window before assignment is
//! acceptable for the runner's supervised agents (and the test agent sleeps
//! before spawning children, so assignment always wins). `TerminateJobObject`
//! plus kill-on-close guarantee the parent + all post-assignment descendants
//! die. This mirrors how established Job-Object supervisors handle the std
//! limitation.
//!
//! ## Graceful shutdown on Windows (documented `[ASSUMPTION]`)
//!
//! A console agent has no portable "please shut down" signal equivalent to
//! SIGTERM that std can deliver to an arbitrary child. This backend therefore
//! implements the graceful step as "give the process the window to exit on its
//! own, then terminate the job": it waits up to `graceful_window` for the
//! process to exit, and if it has not, escalates to `TerminateJobObject`
//! (`forced == true`). If the process exits within the window, the stop is
//! graceful (`forced == false`). Richer graceful mechanisms (a
//! `CTRL_BREAK_EVENT` to the process group, or an adapter-specific shutdown
//! request) are a later refinement; the no-survivor guarantee is unchanged.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, WaitForSingleObject, CREATE_NEW_PROCESS_GROUP,
};

use crate::ports::{BackendError, ProcessBackend, ProcessStatus, SpawnSpec, StopOutcome};

/// `STILL_ACTIVE` (259): the exit code a process reports while still running.
const STILL_ACTIVE: u32 = 259;

/// How often the graceful-stop wait polls for the process to exit.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A running process on Windows: the owned child + its Job Object handle.
///
/// The Job holds the process (and its descendants). Dropping the handle closes
/// the job, which — with kill-on-close set — terminates every process in it, so
/// a leaked handle never leaves survivors.
pub struct WindowsProcess {
    /// The owned child handle (its process handle drives waits/exit-code).
    child: Child,
    /// The Job Object handle (owns the process tree; kill-on-close configured).
    job: HANDLE,
    /// The child pid, cached for diagnostics and the 1-6 adoption fingerprint.
    pid: u32,
}

// The raw Job HANDLE is an owned OS resource this struct is solely responsible
// for; it is safe to move across threads (tokio's blocking pool).
unsafe impl Send for WindowsProcess {}

impl Drop for WindowsProcess {
    fn drop(&mut self) {
        // Closing the job handle kills the tree (kill-on-close), then we release
        // the handle. Best-effort — nothing to do if it fails during teardown.
        if !self.job.is_null() {
            unsafe {
                CloseHandle(self.job);
            }
            self.job = std::ptr::null_mut();
        }
    }
}

/// The Windows process backend (AD-4).
///
/// Stateless — each running process is owned by its [`WindowsProcess`] handle.
/// Constructed via [`crate::backends::current`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsBackend;

impl WindowsBackend {
    /// Construct the backend.
    pub fn new() -> Self {
        WindowsBackend
    }
}

impl ProcessBackend for WindowsBackend {
    type Handle = WindowsProcess;

    fn spawn(&self, spec: &SpawnSpec) -> Result<Self::Handle, BackendError> {
        // Create the Job Object first and configure kill-on-close so that even a
        // dropped handle (or a crash of the engine) tears down the tree.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(BackendError::Spawn {
                exec: spec.exec.clone(),
                detail: format!("CreateJobObjectW failed (os error {})", last_error()),
            });
        }
        // Wrap the job handle in a guard so any early return closes it.
        let job_guard = JobGuard { job };

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                core::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(BackendError::Spawn {
                exec: spec.exec.clone(),
                detail: format!("SetInformationJobObject failed (os error {})", last_error()),
            });
        }

        let mut command = Command::new(&spec.exec);
        command.args(&spec.args);
        command.current_dir(&spec.working_dir);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        match &spec.log_file {
            Some(path) => {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| BackendError::Spawn {
                        exec: spec.exec.clone(),
                        detail: format!("could not open log file {}: {e}", path.display()),
                    })?;
                let err_clone = file.try_clone().map_err(|e| BackendError::Spawn {
                    exec: spec.exec.clone(),
                    detail: format!("could not duplicate log handle: {e}"),
                })?;
                command.stdout(Stdio::from(file));
                command.stderr(Stdio::from(err_clone));
            }
            None => {
                command.stdout(Stdio::null());
                command.stderr(Stdio::null());
            }
        }
        command.stdin(Stdio::null());
        // A new process group isolates console signals (so a stray Ctrl-C to the
        // engine's console does not hit the agent). We do NOT create the child
        // suspended: std does not expose the main-thread handle needed to resume
        // it, so instead we spawn and assign to the job IMMEDIATELY (see the
        // module docs on the sub-millisecond assign-after-spawn window).
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);

        let child = command.spawn().map_err(|e| BackendError::Spawn {
            exec: spec.exec.clone(),
            detail: e.to_string(),
        })?;
        let pid = child.id();

        // Assign the child to the job immediately (before it does meaningful
        // work). From here, the child and every descendant it spawns are in the
        // job and die with a TerminateJobObject / job-handle close (AC3).
        let child_handle = child.as_raw_handle() as HANDLE;
        let assigned = unsafe { AssignProcessToJobObject(job, child_handle) };
        if assigned == 0 {
            let detail = format!(
                "AssignProcessToJobObject failed (os error {})",
                last_error()
            );
            // Kill the child we just created so nothing leaks, then fail.
            unsafe {
                TerminateJobObject(job, 1);
            }
            return Err(BackendError::Spawn {
                exec: spec.exec.clone(),
                detail,
            });
        }

        // Assignment succeeded — hand the job handle to the process struct.
        let job = job_guard.into_inner();
        Ok(WindowsProcess { child, job, pid })
    }

    fn stop(
        &self,
        handle: &mut Self::Handle,
        graceful_window: Duration,
    ) -> Result<StopOutcome, BackendError> {
        // Already exited? Reap and report a graceful stop.
        if handle.reap_if_exited()?.is_exited() {
            return Ok(StopOutcome { forced: false });
        }

        // Graceful step: give the process the window to exit on its own. (See
        // module docs on Windows graceful semantics.)
        let deadline = Instant::now() + graceful_window;
        loop {
            if handle.reap_if_exited()?.is_exited() {
                return Ok(StopOutcome { forced: false });
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(STOP_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }

        // Escalate: terminate the whole job (kills the parent + all descendants).
        let ok = unsafe { TerminateJobObject(handle.job, 1) };
        if ok == 0 {
            return Err(BackendError::Control {
                op: "terminate",
                detail: format!("TerminateJobObject failed (os error {})", last_error()),
            });
        }
        // Reap the child so its handle is released.
        let _ = handle.child.wait();
        Ok(StopOutcome { forced: true })
    }

    fn poll(&self, handle: &mut Self::Handle) -> Result<ProcessStatus, BackendError> {
        handle.reap_if_exited()
    }

    fn pid(&self, handle: &Self::Handle) -> u32 {
        handle.pid
    }
}

impl WindowsProcess {
    /// Non-blocking: reap the child if it has exited, returning its status.
    ///
    /// Uses `WaitForSingleObject(handle, 0)` for a zero-timeout liveness check,
    /// then `GetExitCodeProcess` for the code. Falls back to `Child::try_wait`
    /// to release the std child bookkeeping.
    fn reap_if_exited(&mut self) -> Result<ProcessStatus, BackendError> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(ProcessStatus::Exited {
                code: status.code(),
            }),
            Ok(None) => {
                // Double-check via the raw handle (defensive; try_wait is
                // authoritative but this keeps parity with the Unix poll).
                let h = self.child.as_raw_handle() as HANDLE;
                let waited = unsafe { WaitForSingleObject(h, 0) };
                if waited == WAIT_OBJECT_0 {
                    let mut code: u32 = 0;
                    let ok = unsafe { GetExitCodeProcess(h, &mut code) };
                    if ok != 0 && code != STILL_ACTIVE {
                        return Ok(ProcessStatus::Exited {
                            code: Some(code as i32),
                        });
                    }
                }
                Ok(ProcessStatus::Alive)
            }
            Err(e) => Err(BackendError::Control {
                op: "wait",
                detail: e.to_string(),
            }),
        }
    }
}

/// A guard that closes a Job handle unless [`JobGuard::into_inner`] is called.
///
/// Ensures an early return between `CreateJobObjectW` and handing the handle to
/// the [`WindowsProcess`] does not leak the job.
struct JobGuard {
    job: HANDLE,
}

impl JobGuard {
    /// Release the handle from the guard (the caller now owns it).
    fn into_inner(self) -> HANDLE {
        let job = self.job;
        std::mem::forget(self);
        job
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.job.is_null() {
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}

/// The last OS error code (`GetLastError`) as a `u32`, for diagnostics.
fn last_error() -> u32 {
    // SAFETY: GetLastError is always safe to call and has no preconditions.
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}
