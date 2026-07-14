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
//!
//! ## Cooperative best-effort pause/resume (documented `[ASSUMPTION]`, AD-4)
//!
//! Windows has NO clean GUARANTEED whole-process suspend from `std`: the closest
//! primitives (`NtSuspendProcess`, or per-thread `SuspendThread` enumeration) are
//! undocumented / brittle, and `std::process::Command` does not even expose the
//! child's thread handles. Per AD-4 the honest Windows pause is therefore
//! **adapter-cooperative only** — never an undocumented suspend API. So
//! [`WindowsBackend::pause`] / [`WindowsBackend::resume`] succeed WITHOUT a hard
//! suspension: they are no-ops that report success. This is honest because the
//! engine only ever calls the backend pause/resume on the GUARANTEED dispatch
//! path; on Windows the mock/manifest declare pause `best-effort`, which the
//! SUPERVISOR handles by transitioning state AND emitting a VISIBLE best-effort
//! qualifier (a `pause-best-effort` transition cause + a CLI stderr note) — the
//! qualifier, not a silent fake in the backend, is what makes it "surfaced not
//! silent". These methods are BEHAVIOR-verified only on the `windows-latest` CI
//! matrix; on Unix hosts they are compile-checked only.
//!
//! ## Start-time fingerprint + orphan adoption (story 1-6, spine AD-5)
//!
//! [`WindowsBackend::fingerprint`] reads the process CREATION TIME via the
//! documented `GetProcessTimes` (a `FILETIME`, folded to a u64 of 100ns ticks) —
//! stable per process, different across a PID reuse. [`WindowsBackend::adopt`]
//! re-opens a live pid with `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION |
//! PROCESS_TERMINATE)` and compares its creation time to the recorded
//! fingerprint (the PID-reuse guard); a match yields an ADOPTED handle that holds
//! the process HANDLE (no Job — the process is already running and may already be
//! in one), so a subsequent `stop` uses `TerminateProcess` on that handle. No
//! undocumented API. Behavior-verified on the `windows-latest` CI leg;
//! compile-checked on Unix.

use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess, WaitForSingleObject,
    CREATE_NEW_PROCESS_GROUP, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

use crate::ports::{
    BackendError, ProcessBackend, ProcessFingerprint, ProcessStatus, SecretError, SpawnSpec,
    StopOutcome,
};

/// `STILL_ACTIVE` (259): the exit code a process reports while still running.
const STILL_ACTIVE: u32 = 259;

/// How often the graceful-stop wait polls for the process to exit.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A running process on Windows.
///
/// For a FRESHLY SPAWNED process, `child` is `Some` and `job` owns the process
/// tree (kill-on-close). For an ADOPTED process (story 1-6, re-acquired on engine
/// start), `child` is `None`, `job` is null, and `adopted` holds a process HANDLE
/// opened via `OpenProcess` (for liveness + `TerminateProcess`) — this engine is
/// not the parent, so it holds no reap-able [`Child`] and did not create a job.
/// Dropping either form releases its OS handles; a spawned handle also kills the
/// tree via the job's kill-on-close.
pub struct WindowsProcess {
    /// The owned child handle if THIS engine spawned the process (drives
    /// waits/exit-code). `None` for an adopted process (not our child).
    child: Option<Child>,
    /// The Job Object handle (owns the process tree; kill-on-close configured)
    /// for a spawned process; null for an adopted one.
    job: HANDLE,
    /// The opened process HANDLE for an ADOPTED process (liveness +
    /// TerminateProcess); null for a spawned one (which uses its Child/job).
    adopted: HANDLE,
    /// The child pid, cached for diagnostics and the 1-6 adoption fingerprint.
    pid: u32,
    /// The child's stdin pipe (story 4.1, spine AD-12), captured at spawn time
    /// for a FRESHLY SPAWNED process. `None` for an ADOPTED process — a pipe
    /// handle cannot be recovered from a bare PID (no undocumented API; parity
    /// with the Unix backend and this module's own pause `[ASSUMPTION]`
    /// precedent); `send_input` on an adopted instance must therefore fail
    /// honestly (`EngineError::InteractionUnavailable`), never silently
    /// succeed.
    stdin: Option<ChildStdin>,
}

// The raw Job / process HANDLEs are owned OS resources this struct is solely
// responsible for; it is safe to move across threads (tokio's blocking pool).
unsafe impl Send for WindowsProcess {}

impl Drop for WindowsProcess {
    fn drop(&mut self) {
        // Spawned: closing the job handle kills the tree (kill-on-close), then
        // release it. Adopted: SIGKILL-equivalent is not applied on drop for a
        // process we merely re-opened (parity with Unix would kill it; but on
        // Windows an adopted process has no job, and the cross-lifetime handle is
        // dropped at engine shutdown — we terminate it in `stop`, and on drop we
        // only release the opened handle so we do not leak it). Best-effort.
        if !self.job.is_null() {
            unsafe {
                CloseHandle(self.job);
            }
            self.job = std::ptr::null_mut();
        }
        if !self.adopted.is_null() {
            unsafe {
                CloseHandle(self.adopted);
            }
            self.adopted = std::ptr::null_mut();
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
        // Piped UNCONDITIONALLY (story 4.1, spine AD-12): v1 pipes stdin for
        // every spawned process regardless of the declared Capability — the
        // Capability Declaration gates only whether `send_input` is *callable*,
        // not whether the pipe exists.
        command.stdin(Stdio::piped());
        // A new process group isolates console signals (so a stray Ctrl-C to the
        // engine's console does not hit the agent). We do NOT create the child
        // suspended: std does not expose the main-thread handle needed to resume
        // it, so instead we spawn and assign to the job IMMEDIATELY (see the
        // module docs on the sub-millisecond assign-after-spawn window).
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);

        let mut child = command.spawn().map_err(|e| BackendError::Spawn {
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
        // Capture the piped stdin now, for a FRESHLY SPAWNED handle only
        // (story 4.1) — an adopted handle never has one (see `adopt` below).
        let stdin = child.stdin.take();
        Ok(WindowsProcess {
            child: Some(child),
            job,
            adopted: std::ptr::null_mut(),
            pid,
            stdin,
        })
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

        // Escalate. Spawned: terminate the whole job (kills parent + descendants)
        // and reap the child. Adopted (no job, no child): terminate the opened
        // process HANDLE with TerminateProcess.
        if !handle.job.is_null() {
            let ok = unsafe { TerminateJobObject(handle.job, 1) };
            if ok == 0 {
                return Err(BackendError::Control {
                    op: "terminate",
                    detail: format!("TerminateJobObject failed (os error {})", last_error()),
                });
            }
        } else if !handle.adopted.is_null() {
            let ok = unsafe { TerminateProcess(handle.adopted, 1) };
            if ok == 0 {
                return Err(BackendError::Control {
                    op: "terminate",
                    detail: format!("TerminateProcess failed (os error {})", last_error()),
                });
            }
            // Wait briefly for the adopted process to actually exit.
            let h = handle.adopted;
            let _ = unsafe { WaitForSingleObject(h, 5000) };
        }
        // Reap the direct child if it is ours (adopted: OS handles it).
        if let Some(child) = handle.child.as_mut() {
            let _ = child.wait();
        }
        Ok(StopOutcome { forced: true })
    }

    fn poll(&self, handle: &mut Self::Handle) -> Result<ProcessStatus, BackendError> {
        handle.reap_if_exited()
    }

    fn pause(&self, handle: &mut Self::Handle) -> Result<(), BackendError> {
        // Cooperative best-effort pause on Windows (AD-4): NO guaranteed
        // whole-process suspend is available from std, and we do NOT reach for an
        // undocumented API (see the module `[ASSUMPTION]` block). Succeed without
        // a hard suspension — the VISIBLE best-effort qualifier the supervisor/CLI
        // emit (never a silent fake here) carries the honesty. We still touch the
        // liveness guard for parity with the Unix body and to reap a gone child.
        let _ = handle.reap_if_exited()?;
        Ok(())
    }

    fn resume(&self, handle: &mut Self::Handle) -> Result<(), BackendError> {
        // Cooperative best-effort resume on Windows — the counterpart of `pause`.
        let _ = handle.reap_if_exited()?;
        Ok(())
    }

    fn pid(&self, handle: &Self::Handle) -> u32 {
        handle.pid
    }

    fn fingerprint(&self, handle: &Self::Handle) -> ProcessFingerprint {
        // Creation time via GetProcessTimes; a read failure falls back to 0 (a
        // degraded but honest fingerprint — the pid is still recorded).
        let start_time = process_start_time(handle.pid).unwrap_or(0);
        ProcessFingerprint::new(handle.pid, start_time)
    }

    fn adopt(
        &self,
        fingerprint: &ProcessFingerprint,
    ) -> Result<Option<Self::Handle>, BackendError> {
        // Open the pid for query + terminate. A gone pid → OpenProcess fails →
        // Ok(None). Then compare the CURRENT creation time to the recorded one
        // (the PID-reuse guard, AD-5): a mismatch → a different process → Ok(None).
        let h = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                0,
                fingerprint.pid,
            )
        };
        if h.is_null() {
            return Ok(None);
        }
        let live_start = match process_start_time(fingerprint.pid) {
            Some(t) => t,
            None => {
                unsafe {
                    CloseHandle(h);
                }
                return Ok(None);
            }
        };
        if live_start != fingerprint.start_time {
            // PID reused by a different process — do NOT adopt.
            unsafe {
                CloseHandle(h);
            }
            return Ok(None);
        }
        // Same process. Hold the opened handle for liveness + TerminateProcess
        // (no Job — the process is already running and may be in one already).
        //
        // `stdin: None` (story 4.1): an adopted handle has no recoverable
        // pipe — there is no OS-portable, documented way to reopen a
        // `ChildStdin` from a bare pid. `send_input` against this handle
        // fails with `EngineError::InteractionUnavailable`, never silently
        // succeeding.
        Ok(Some(WindowsProcess {
            child: None,
            job: std::ptr::null_mut(),
            adopted: h,
            pid: fingerprint.pid,
            stdin: None,
        }))
    }

    fn has_stdin(&self, handle: &Self::Handle) -> bool {
        handle.stdin.is_some()
    }

    fn write_stdin(&self, handle: &mut Self::Handle, data: &[u8]) -> Result<(), BackendError> {
        match handle.stdin.as_mut() {
            Some(stdin) => {
                stdin.write_all(data).map_err(|e| BackendError::Control {
                    op: "stdin",
                    detail: e.to_string(),
                })?;
                stdin.flush().map_err(|e| BackendError::Control {
                    op: "stdin",
                    detail: e.to_string(),
                })?;
                Ok(())
            }
            None => Err(BackendError::Control {
                op: "stdin",
                detail: "no stdin pipe held for this handle".to_string(),
            }),
        }
    }
}

impl WindowsProcess {
    /// Non-blocking: reap the child if it has exited, returning its status.
    ///
    /// SPAWNED (`child: Some`): `Child::try_wait` (authoritative), double-checked
    /// via the raw handle. ADOPTED (`child: None`, story 1-6): this engine is not
    /// the parent, so liveness is `WaitForSingleObject(adopted, 0)` +
    /// `GetExitCodeProcess` on the opened handle; a gone process reports its exit
    /// code if still readable, else `Exited { code: None }`.
    fn reap_if_exited(&mut self) -> Result<ProcessStatus, BackendError> {
        match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Ok(ProcessStatus::Exited {
                    code: status.code(),
                }),
                Ok(None) => {
                    // Double-check via the raw handle (defensive; try_wait is
                    // authoritative but this keeps parity with the Unix poll).
                    let h = child.as_raw_handle() as HANDLE;
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
            },
            // Adopted: liveness via the opened process handle.
            None => {
                if self.adopted.is_null() {
                    // No handle at all — treat as gone (defensive; not normally
                    // reachable, an adopted handle always opens a process handle).
                    return Ok(ProcessStatus::Exited { code: None });
                }
                let waited = unsafe { WaitForSingleObject(self.adopted, 0) };
                if waited == WAIT_OBJECT_0 {
                    let mut code: u32 = 0;
                    let ok = unsafe { GetExitCodeProcess(self.adopted, &mut code) };
                    if ok != 0 && code != STILL_ACTIVE {
                        return Ok(ProcessStatus::Exited {
                            code: Some(code as i32),
                        });
                    }
                    return Ok(ProcessStatus::Exited { code: None });
                }
                Ok(ProcessStatus::Alive)
            }
        }
    }
}

/// Read a process's creation time via `GetProcessTimes`, folded to a u64 of
/// 100ns ticks — stable per process, different across a PID reuse (spine AD-5).
/// Opens a short-lived query handle by pid. Returns `None` if the process cannot
/// be opened/queried (gone, or insufficient rights). No undocumented API.
fn process_start_time(pid: u32) -> Option<u64> {
    // PROCESS_QUERY_LIMITED_INFORMATION suffices for GetProcessTimes and is the
    // least-privileged right that works across integrity levels.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return None;
    }
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    let ok = unsafe { GetProcessTimes(h, &mut creation, &mut exit, &mut kernel, &mut user) };
    unsafe {
        CloseHandle(h);
    }
    if ok == 0 {
        return None;
    }
    let ticks = ((creation.dwHighDateTime as u64) << 32) | (creation.dwLowDateTime as u64);
    if ticks == 0 {
        return None;
    }
    Some(ticks)
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

/// Check the engine secrets file's permissions on Windows (story 2-4 AC6, spine
/// AD-10/AD-4) — the OS-specific INSPECTION confined to `backends/`.
///
/// DECISION (Assumption 7, option B — documented portable skip): Unix mode bits do
/// not exist on Windows, and a faithful DACL inspection (option A) needs `windows`
/// ACL FFI that is over-scope for v1's tiny-secrets budget. So this does NOT
/// attempt a Unix-style refusal: it returns `Ok(())` and relies on the DEFAULT
/// per-user profile ACLs — the state dir lives under the user's profile
/// (`%APPDATA%`/`%LOCALAPPDATA%` via the `directories` crate), which is
/// per-user-protected by Windows by default. This is an HONEST boundary (documented
/// in `docs/architecture.md`, NFR-6): it avoids a FALSE PASS masquerading as a
/// Unix-grade check AND avoids a hard failure that would make secrets UNUSABLE on
/// Windows. A future ACL-checking resolver can strengthen this behind the same
/// port without a schema/API change. The `_path` is accepted for signature
/// symmetry with the Unix backend.
pub fn check_secrets_file_permissions(_path: &std::path::Path) -> Result<(), SecretError> {
    // Portable skip (option B): Windows relies on default per-user profile ACLs.
    // Never a false pass framed as a Unix-grade check; never a hard failure.
    Ok(())
}
