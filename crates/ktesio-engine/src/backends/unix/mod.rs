//! The Unix [`ProcessBackend`] (spine AD-4) — process groups + signals.
//!
//! Spawns each Agent Instance into its OWN process group (a fresh session via
//! `setsid`, so the child is the group leader and `pgid == pid`). Stopping then
//! signals the WHOLE group with `killpg`, catching any child processes the agent
//! itself spawned — the load-bearing mechanism behind AC3 "no process of the
//! instance survives". Graceful stop is `SIGTERM` to the group; after the window
//! elapses it escalates to `SIGKILL` to the group.
//!
//! This module is the allowlisted home for OS-conditional code (it is
//! `#[cfg(unix)]`-gated at its `mod` declaration in `backends/mod.rs`). It uses
//! `nix` for `setsid`/`kill`/`killpg` and `std::os::unix` for the `pre_exec`
//! child hook.
//!
//! ## Start-time fingerprint + orphan adoption (story 1-6, spine AD-5)
//!
//! [`UnixBackend::fingerprint`] reads a process's start-time — a per-boot,
//! per-process token that differs across a PID reuse — so the write-ahead spawn
//! record can carry `{ pid, start-time }` (AD-5). The SOURCE is per-OS and lives
//! ONLY here (the OS-cfg allowlist): Linux reads `/proc/<pid>/stat` field 22
//! (`starttime`, clock ticks since boot); macOS/BSD reads the process
//! `p_starttime` (a `timeval`, folded to microseconds) via `sysctl`
//! `KERN_PROC_PID`. Both are documented, stable APIs — no undocumented calls.
//! [`UnixBackend::adopt`] re-acquires a live PID whose current start-time equals
//! the recorded fingerprint (the PID-reuse guard), rebuilding a HANDLE that can
//! still signal/stop the group via `killpg` even though this process is not the
//! child's parent (so it holds no reap-able [`Child`]; liveness is `kill(pid, 0)`
//! and the OS/init reaps the non-child on exit). An adopted process is supervised
//! for the new engine's lifetime exactly like a freshly spawned one.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::{setsid, Pid};

use crate::ports::{
    BackendError, ProcessBackend, ProcessFingerprint, ProcessStatus, SpawnSpec, StopOutcome,
};

/// How often the graceful-stop wait polls for the process to exit.
///
/// A short poll interval keeps a fast-exiting process from waiting the whole
/// window while bounding the busy-work. The wait runs on tokio's blocking pool
/// (the engine calls `stop` via `spawn_blocking`), so a bounded `sleep` here
/// does not stall an async worker.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A running process on Unix: the (optionally owned) child + its process-group
/// id.
///
/// The group id equals the child pid (the child is its own group leader via
/// `setsid`). For a FRESHLY SPAWNED process, `child` is `Some` — holding the
/// [`Child`] lets us reap it (no zombie). For an ADOPTED process (story 1-6:
/// re-acquired on engine start), `child` is `None` — this engine is not the
/// process's parent, so it cannot `wait()`/reap it; liveness is probed with
/// `kill(pid, 0)` and the OS/init reaps the non-child when it exits. In BOTH
/// cases the group id lets us signal the whole tree (`killpg`), so stop / pause /
/// resume work identically.
///
/// ## Steady-state PID-reuse guard for an ADOPTED handle (AI-10)
///
/// An adopted handle carries the LIVE `start_time` verified at adoption so its
/// liveness poll can RE-check the start-time fingerprint, not just the bare PID.
/// A spawned handle records its start-time too (for symmetry / diagnostics), but
/// it reaps through its owned [`Child`] so it is already immune to PID reuse; the
/// re-check matters only on the adopted (`child: None`) path.
#[derive(Debug)]
pub struct UnixProcess {
    /// The owned child handle, if THIS engine spawned the process (reaped on
    /// stop / drop). `None` for an adopted process (not our child).
    child: Option<Child>,
    /// The process-group id to signal (== child pid).
    pgid: Pid,
    /// The child pid, cached for diagnostics and the 1-6 adoption fingerprint.
    pid: u32,
    /// The recorded process start-time token (spine AD-5) — the PID-reuse guard
    /// carried on the handle itself. `0` means "no start-time source" (a degraded
    /// but honest fingerprint on a host with no `process_start_time` source, or a
    /// read that failed at spawn). For an ADOPTED process (`child: None`) this is
    /// the LIVE start-time verified at adoption (always non-zero on the supported
    /// Linux/macOS hosts, since `adopt` returns `None` when it cannot read one), so
    /// [`UnixProcess::reap_if_exited`] can RE-verify it on every steady-state poll:
    /// a bare `kill(pid, 0)` alone cannot tell an adopted agent's crash from the OS
    /// recycling its PID to an unrelated process within a reaper interval (AI-10).
    start_time: u64,
}

/// The Unix process backend (AD-4).
///
/// Stateless — each running process is owned by its [`UnixProcess`] handle, held
/// by the supervisor. Constructed via [`crate::backends::current`].
#[derive(Clone, Copy, Debug, Default)]
pub struct UnixBackend;

impl UnixBackend {
    /// Construct the backend.
    pub fn new() -> Self {
        UnixBackend
    }
}

impl ProcessBackend for UnixBackend {
    type Handle = UnixProcess;

    fn spawn(&self, spec: &SpawnSpec) -> Result<Self::Handle, BackendError> {
        let mut command = Command::new(&spec.exec);
        command.args(&spec.args);
        command.current_dir(&spec.working_dir);
        // Apply env overrides on top of the inherited environment.
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        // Redirect stdout+stderr to the per-instance log if one was given
        // (AD-12 seed); otherwise inherit.
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

        // Put the child in its OWN session+process group BEFORE exec, so a later
        // killpg reaches the whole tree. `setsid` fails only if the caller is
        // already a group leader — never true for a freshly forked child — so
        // this is safe. `pre_exec` runs in the forked child after fork, before
        // exec: it must be async-signal-safe, which a bare setsid syscall is.
        //
        // SAFETY: the closure performs only an async-signal-safe syscall
        // (`setsid`) and allocates nothing; it does not touch shared state of
        // the parent. This satisfies the `pre_exec` contract.
        unsafe {
            command.pre_exec(|| {
                setsid().map_err(io::Error::from)?;
                Ok(())
            });
        }

        let child = command.spawn().map_err(|e| BackendError::Spawn {
            exec: spec.exec.clone(),
            detail: e.to_string(),
        })?;
        let pid = child.id();
        // The child is its own group leader, so pgid == pid.
        let pgid = Pid::from_raw(pid as i32);
        // Record the start-time for symmetry with the adopted handle (a spawned
        // handle reaps via its owned Child, so it never relies on this for the
        // PID-reuse guard; a read failure degrades to 0, an honest pid-only form).
        let start_time = process_start_time(pid).unwrap_or(0);
        Ok(UnixProcess {
            child: Some(child),
            pgid,
            pid,
            start_time,
        })
    }

    fn stop(
        &self,
        handle: &mut Self::Handle,
        graceful_window: Duration,
    ) -> Result<StopOutcome, BackendError> {
        // Already gone? Reap and report a graceful (non-forced) stop.
        if handle.reap_if_exited()?.is_exited() {
            return Ok(StopOutcome { forced: false });
        }

        // (1) Graceful: SIGTERM to the whole group.
        signal_group(handle.pgid, Signal::SIGTERM)?;

        // (2) Wait up to the window for the group leader (our child) to exit.
        let deadline = Instant::now() + graceful_window;
        loop {
            if handle.reap_if_exited()?.is_exited() {
                // Exited gracefully within the window. Best-effort sweep of any
                // lingering group members the agent spawned, so none survive.
                //
                // [ASSUMPTION] pgid-reuse micro-window (documented, low severity;
                // parity with the Windows backend's assign-after-spawn honesty).
                // We reap the group LEADER first (the line just above), so by the
                // time this sweep runs the leader's pid — which equals the pgid —
                // has been released and the kernel could in principle recycle it
                // for an unrelated new group, which this SIGKILL would then hit.
                // The window is on the order of microseconds and bounded by
                // reaping the leader before sweeping; it is fully closed by the
                // pid + start-time fingerprint that story 1-6's orphan adoption
                // adds (a recycled group would fail the fingerprint match). No
                // behavior change here — this only records the known boundary.
                let _ = killpg(handle.pgid, Signal::SIGKILL);
                return Ok(StopOutcome { forced: false });
            }
            if Instant::now() >= deadline {
                break;
            }
            sleep(STOP_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }

        // (3) Escalate: SIGKILL to the whole group, then reap the child so no
        // zombie remains. SIGKILL cannot be caught/ignored, so the group dies.
        signal_group(handle.pgid, Signal::SIGKILL)?;
        match handle.child.as_mut() {
            // Spawned: block until the direct child is reaped (it was just
            // SIGKILLed, so this is bounded). Group members are killed by the
            // SIGKILL above and reaped by init.
            Some(child) => {
                child.wait().map_err(|e| BackendError::Control {
                    op: "wait",
                    detail: e.to_string(),
                })?;
            }
            // Adopted (not our child): we cannot `wait` it, but the SIGKILL to
            // the group is delivered and its real parent / init reaps it. Poll
            // liveness briefly so the desired end state ("no process survives")
            // is confirmed before returning.
            None => {
                let kill_deadline = Instant::now() + Duration::from_secs(5);
                while pid_is_alive(handle.pid) {
                    if Instant::now() >= kill_deadline {
                        break;
                    }
                    sleep(STOP_POLL_INTERVAL);
                }
            }
        }
        Ok(StopOutcome { forced: true })
    }

    fn poll(&self, handle: &mut Self::Handle) -> Result<ProcessStatus, BackendError> {
        handle.reap_if_exited()
    }

    fn pause(&self, handle: &mut Self::Handle) -> Result<(), BackendError> {
        // GUARANTEED pause on Unix (AC1): SIGSTOP the WHOLE process group. SIGSTOP
        // cannot be caught or ignored, so the suspension is real and verifiable
        // (the group freezes — a heartbeat stops growing). Guard for liveness
        // first (parity with `stop`): a process that already exited cannot be
        // paused, and that is a harmless no-op — the desired end state already
        // holds (and SIGSTOP to a gone group would resolve to ESRCH→Ok anyway).
        if handle.reap_if_exited()?.is_exited() {
            return Ok(());
        }
        signal_group(handle.pgid, Signal::SIGSTOP)
    }

    fn resume(&self, handle: &mut Self::Handle) -> Result<(), BackendError> {
        // GUARANTEED resume on Unix (AC1): SIGCONT the whole group, waking every
        // process the SIGSTOP suspended. Same already-exited tolerance as
        // `pause` — a resume of a gone process is a harmless no-op.
        if handle.reap_if_exited()?.is_exited() {
            return Ok(());
        }
        signal_group(handle.pgid, Signal::SIGCONT)
    }

    fn pid(&self, handle: &Self::Handle) -> u32 {
        handle.pid
    }

    fn fingerprint(&self, handle: &Self::Handle) -> ProcessFingerprint {
        // A read failure falls back to start_time 0 (a degraded but honest
        // fingerprint — the pid is still recorded) rather than erroring the
        // spawn; in normal operation reading the start-time of a process we hold
        // alive succeeds.
        let start_time = process_start_time(handle.pid).unwrap_or(0);
        ProcessFingerprint::new(handle.pid, start_time)
    }

    fn adopt(
        &self,
        fingerprint: &ProcessFingerprint,
    ) -> Result<Option<Self::Handle>, BackendError> {
        // The PID-reuse guard (AD-5): a live pid whose CURRENT start-time equals
        // the recorded one is the SAME process → adopt it. A gone pid, or one
        // whose start-time differs (reused for a new process), is NOT a match →
        // Ok(None), so the caller reconciles the record to `failed`.
        if !pid_is_alive(fingerprint.pid) {
            return Ok(None);
        }
        let live_start = match process_start_time(fingerprint.pid) {
            Some(t) => t,
            // The pid is alive but we cannot read its start-time (it may have
            // exited between the liveness probe and the read, or it is not
            // introspectable). Treat as no confident match — reconcile to failed.
            None => return Ok(None),
        };
        if live_start != fingerprint.start_time {
            // PID reused by a different process — do NOT adopt.
            return Ok(None);
        }
        // Same process. Rebuild an adopted handle: no owned Child (not our
        // child), but the group id (== pid, the setsid leader) lets us still
        // signal/stop the whole tree. Liveness is kill(pid, 0) RE-checked against
        // `live_start` on every poll (AI-10), so a later PID reuse cannot mask a
        // crash of this adopted process.
        Ok(Some(UnixProcess {
            child: None,
            pgid: Pid::from_raw(fingerprint.pid as i32),
            pid: fingerprint.pid,
            start_time: live_start,
        }))
    }
}

/// Read a process's start-time — the per-boot, per-process token that differs
/// across a PID reuse (spine AD-5). The SOURCE is per-OS; both are documented,
/// stable APIs (no undocumented calls). Returns `None` if the process is gone or
/// its start-time cannot be read.
#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    // Linux: /proc/<pid>/stat field 22 (`starttime`) — the time the process
    // started after boot, in clock ticks. The field is AFTER the `comm` field,
    // which is parenthesized and may itself contain spaces/parentheses, so we
    // split on the LAST ')' first, then take field 22 counting from there.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')').map(|(_, rest)| rest)?;
    // After ')' the fields are: state(3) ppid(4) ... starttime(22). `rest`
    // begins with a leading space then field 3, so index 22-3 = 19 in the
    // whitespace-split remainder.
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // field 3 is fields[0]; field 22 is fields[19].
    fields.get(19).and_then(|s| s.parse::<u64>().ok())
}

/// macOS: the process start-time (`pbi_start_tvsec`/`pbi_start_tvusec`) via the
/// documented `libproc` `proc_pidinfo(PROC_PIDTBSDINFO)` call, folded to
/// microseconds since the epoch — stable per process, different across a PID
/// reuse. Uses `nix::libc` (nix re-exports libc; no new dependency). No
/// undocumented API. Behavior-verified on the macOS CI leg.
#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<u64> {
    use nix::libc;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a zeroed proc_bsdinfo of exactly `size` bytes;
    // proc_pidinfo writes at most `size` bytes into it. The pointer is valid for
    // the call's duration. proc_pidinfo returns the number of bytes written.
    let written = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if written != size {
        // Process gone, insufficient permission, or a short read — no confident
        // start-time.
        return None;
    }
    // Fold seconds + microseconds into a single microsecond token — stable per
    // process, distinct across a PID reuse.
    let secs = info.pbi_start_tvsec as i128;
    let usec = info.pbi_start_tvusec as i128;
    let micros = secs.saturating_mul(1_000_000).saturating_add(usec);
    if micros <= 0 {
        return None;
    }
    Some(micros as u64)
}

/// Other Unix (neither Linux nor macOS — e.g. a BSD without `libproc`): no
/// supported start-time source. Returns `None`, yielding a degraded but honest
/// fingerprint (pid-only). Ktesio's supported targets are Linux / macOS /
/// Windows; this arm only keeps the backend COMPILING on other Unix.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

impl Drop for UnixProcess {
    /// Kill the process group on drop so a dropped handle never leaks the agent
    /// (or any child it spawned). This is what keeps `kt agent start` — which
    /// spawns, records `running`, then EXITS — from orphaning the process: when
    /// the supervisor is torn down at engine shutdown, the handle drops and the
    /// whole group dies. Best-effort; a group already gone is fine.
    ///
    /// NOTE (cross-lifetime, AD-5 is story 1-6): a handle for a SPAWNED process
    /// is cleaned up when this engine ends (kill-on-drop). An ADOPTED handle
    /// (re-acquired on engine start) likewise SIGKILLs its group on drop, but
    /// cannot `wait` a non-child (the OS/init reaps it); this keeps the
    /// no-survivor guarantee across engine restarts.
    fn drop(&mut self) {
        // If already reaped/exited, nothing to do; otherwise SIGKILL the group.
        if let Ok(ProcessStatus::Alive) = self.reap_if_exited() {
            let _ = killpg(self.pgid, Signal::SIGKILL);
            // Reap the direct child if it is ours (adopted: init reaps it).
            if let Some(child) = self.child.as_mut() {
                let _ = child.wait();
            }
        }
    }
}

impl UnixProcess {
    /// Non-blocking: reap the child if it has exited, returning its status.
    ///
    /// For a SPAWNED process (`child: Some`) this is `try_wait` (which reaps on
    /// exit, no zombie). For an ADOPTED process (`child: None`, story 1-6) this
    /// engine is not the parent and cannot `wait`, so liveness is `kill(pid, 0)`:
    /// `ESRCH` means the process is gone (reaped by its real parent / init), any
    /// other result means it is still alive. An adopted process reports
    /// `Exited { code: None }` when gone — we cannot recover a non-child's exit
    /// code, which is honest (the code is unknown to us).
    ///
    /// ## Adopted PID-reuse guard (AI-10)
    ///
    /// A bare `kill(pid, 0)` cannot distinguish "the adopted process is still
    /// alive" from "the adopted process crashed and the OS recycled its PID for an
    /// unrelated new process within a reaper interval (~250ms)" — both read as
    /// alive, so the crash would be missed and the row left a phantom `running`.
    /// So when this handle carries a real recorded start-time (`start_time != 0`,
    /// always true for an adopted handle on the supported Linux/macOS hosts — see
    /// [`UnixBackend::adopt`]), we RE-read the live PID's start-time and treat a
    /// MISMATCH or a read failure as "the original process is gone" →
    /// `Exited { code: None }`. Only when we have NO recorded token (`start_time
    /// == 0`, a degraded host with no start-time source) do we fall back to the
    /// bare liveness probe — the best that host can honestly do. A spawned handle
    /// (`child: Some`) reaps via its owned `Child` and never reaches this path, so
    /// it was already immune to PID reuse.
    fn reap_if_exited(&mut self) -> Result<ProcessStatus, BackendError> {
        match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Ok(ProcessStatus::Exited {
                    code: status.code(),
                }),
                Ok(None) => Ok(ProcessStatus::Alive),
                Err(e) => Err(BackendError::Control {
                    op: "wait",
                    detail: e.to_string(),
                }),
            },
            // Adopted (not our child): probe liveness via kill(pid, 0), then
            // RE-verify the start-time fingerprint so a recycled PID cannot mask a
            // crash (AI-10).
            None => {
                if !pid_is_alive(self.pid) {
                    // The PID is gone → the process exited (reaped by init).
                    return Ok(ProcessStatus::Exited { code: None });
                }
                // The PID is alive. If we hold a real recorded start-time, confirm
                // it still matches the live PID.
                if self.start_time != 0 {
                    match process_start_time(self.pid) {
                        // Successful read that MATCHES → the same process, alive.
                        Some(live) if live == self.start_time => Ok(ProcessStatus::Alive),
                        // Successful read that MISMATCHES → the PID was recycled for
                        // a DIFFERENT process; the original adopted process is gone.
                        // This is THE AI-10 guard: a recycled PID always yields a
                        // successful read + a differing start-time, so this path is
                        // exactly the recycled-PID case.
                        Some(_) => Ok(ProcessStatus::Exited { code: None }),
                        // Read FAILED (a transient /proc or libproc hiccup) → we
                        // CANNOT confidently distinguish alive-vs-gone, so do NOT
                        // report a crash on an ambiguous read (that would spuriously
                        // kill+restart a genuinely-live agent). Treat as still-alive
                        // and let the reaper re-check next tick — a truly-dead
                        // process is still caught then (its PID goes away → Exited,
                        // or a reused PID reads a MISMATCHING start-time → Exited),
                        // so this does NOT reopen the AI-10 hole. This mirrors how
                        // `adopt`/`fingerprint` already treat a start-time read
                        // failure as "not confident", never as a positive signal.
                        None => Ok(ProcessStatus::Alive),
                    }
                } else {
                    // Degraded host with no start-time source: bare liveness is the
                    // honest best we can do (an adopted handle here is unusual —
                    // `adopt` needs a readable start-time to create one at all).
                    Ok(ProcessStatus::Alive)
                }
            }
        }
    }
}

/// Whether a pid is still alive: `kill(pid, 0)` succeeds (or fails with `EPERM`
/// — alive but not ours to signal) while it lives, and fails with `ESRCH` once
/// it is gone. Used for adopted-process liveness (no reap-able [`Child`]).
fn pid_is_alive(pid: u32) -> bool {
    match kill(Pid::from_raw(pid as i32), None) {
        Err(nix::errno::Errno::ESRCH) => false,
        // Ok (we can signal it) or EPERM (alive but not ours) → alive.
        _ => true,
    }
}

/// Signal a whole process group, treating "no such process" as success.
///
/// `killpg` returns `ESRCH` if the group is already gone — the desired end state
/// for a stop, so it is NOT an error. Any other failure is a real control error.
fn signal_group(pgid: Pid, signal: Signal) -> Result<(), BackendError> {
    match killpg(pgid, signal) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()), // group already gone
        Err(e) => Err(BackendError::Control {
            op: "signal",
            detail: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Resolve the `fake_agent` helper binary via the conformance dev-dependency
    /// (a dev-dep of the engine — off the shipping graph). Public within the test
    /// module so the child-survivor test can reach it.
    pub(super) fn fake_agent_path() -> std::path::PathBuf {
        ktesio_conformance::fake_agent_bin()
    }

    fn spec(exec: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            exec: exec.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            working_dir: std::env::temp_dir(),
            log_file: None,
        }
    }

    #[test]
    fn spawn_a_sleep_then_stop_kills_it() {
        // Spawn `sleep 60`, confirm it is alive, then stop with a short window.
        // It ignores nothing, so SIGTERM ends it gracefully (forced == false).
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("sleep", &["60"])).expect("spawn sleep");
        assert!(proc.pid > 0);
        assert_eq!(backend.poll(&mut proc).unwrap(), ProcessStatus::Alive);

        let outcome = backend
            .stop(&mut proc, Duration::from_secs(5))
            .expect("stop");
        assert!(!outcome.forced, "sleep exits on SIGTERM without escalation");
        assert!(backend.poll(&mut proc).unwrap().is_exited());
    }

    #[test]
    fn spawn_missing_exec_is_a_spawn_error_no_zombie() {
        // AC2: a non-existent exec fails at spawn with a preserved diagnostic and
        // leaves no child to zombie (nothing was spawned).
        let backend = UnixBackend::new();
        let err = backend
            .spawn(&spec("ktesio-no-such-binary-xyz", &[]))
            .unwrap_err();
        match err {
            BackendError::Spawn { exec, detail } => {
                assert_eq!(exec, "ktesio-no-such-binary-xyz");
                assert!(!detail.is_empty());
            }
            other => panic!("expected Spawn, got {other}"),
        }
    }

    #[test]
    fn stop_an_already_exited_process_is_graceful() {
        // `true` exits immediately. By the time we stop, it is already gone; the
        // backend reaps it and reports a non-forced stop (desired end state).
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("true", &[])).expect("spawn true");
        // Give it a moment to exit.
        sleep(Duration::from_millis(50));
        let outcome = backend
            .stop(&mut proc, Duration::from_secs(1))
            .expect("stop");
        assert!(!outcome.forced);
    }

    #[test]
    fn poll_reports_exit_of_a_short_lived_process() {
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("true", &[])).expect("spawn true");
        // Poll until it exits (bounded).
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if backend.poll(&mut proc).unwrap().is_exited() {
                break;
            }
            assert!(Instant::now() < deadline, "process did not exit in time");
            sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn log_file_captures_child_stdout() {
        // The spawned child's stdout is redirected to the per-instance log file.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("instance.log");
        let mut s = spec("echo", &["hello-from-child"]);
        s.log_file = Some(log.clone());
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&s).expect("spawn echo");
        // Wait for it to finish writing + exit.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.poll(&mut proc).unwrap().is_exited() {
            assert!(Instant::now() < deadline);
            sleep(Duration::from_millis(10));
        }
        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("hello-from-child"), "log={contents:?}");
    }

    #[test]
    fn pid_accessor_returns_the_child_pid() {
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("sleep", &["30"])).expect("spawn");
        assert_eq!(backend.pid(&proc), proc.pid);
        assert!(backend.pid(&proc) > 0);
        // Teardown.
        let _ = backend.stop(&mut proc, Duration::from_secs(2));
    }

    #[test]
    fn fingerprint_is_stable_across_reads_for_the_same_process() {
        // AD-5: the fingerprint must be STABLE for a live process (two reads
        // agree) so a write-ahead record can be reconciled later. On the
        // supported hosts (Linux/macOS) the start-time is a real, non-zero token.
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("sleep", &["30"])).expect("spawn");
        let fp1 = backend.fingerprint(&proc);
        let fp2 = backend.fingerprint(&proc);
        assert_eq!(fp1, fp2, "fingerprint must be stable across reads");
        assert_eq!(fp1.pid, proc.pid);
        // On the two supported Unix hosts the start-time source is real.
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(fp1.start_time > 0, "start-time token should be populated");
        }
        let _ = backend.stop(&mut proc, Duration::from_secs(2));
    }

    #[test]
    fn adopt_reacquires_a_live_matching_process_and_can_stop_it() {
        // AC7 (Unix): a live process whose start-time matches the recorded
        // fingerprint is ADOPTED — the re-held handle can still stop the group.
        // Spawn a long-lived child, capture its fingerprint, "adopt" it via a
        // second backend (simulating a new engine), and stop it through the
        // adopted handle. Leak-guard: keep the ORIGINAL handle so if adoption
        // fails the child is still reaped on drop.
        let backend = UnixBackend::new();
        let mut original = backend
            .spawn(&spec("sleep", &["600"]))
            .expect("spawn sleep");
        let fp = backend.fingerprint(&original);
        assert!(pid_alive(fp.pid));

        // Adopt via a fresh backend (the OS re-acquisition path).
        let adopter = UnixBackend::new();
        let adopted = adopter.adopt(&fp).expect("adopt call ok");
        let mut adopted = adopted.expect("a live matching process must be adopted");
        assert_eq!(adopter.pid(&adopted), fp.pid);
        assert_eq!(adopter.poll(&mut adopted).unwrap(), ProcessStatus::Alive);

        // Stopping through the ADOPTED handle terminates the process group.
        let _ = adopter.stop(&mut adopted, Duration::from_millis(500));
        // In the REAL orphan scenario the original engine has died, so the
        // process's parent is init, which auto-reaps it. Here the original
        // backend still owns the Child, so the killed process would linger as a
        // ZOMBIE until reaped; poll the original handle in the loop to reap it
        // (standing in for init) so `pid_alive` reflects the true end state.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // Reap via the original owner (init's role in production).
            let reaped = backend
                .poll(&mut original)
                .map(|s| s.is_exited())
                .unwrap_or(true);
            if reaped && !pid_alive(fp.pid) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "adopted process must be killable via the re-held handle"
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn adopt_returns_none_for_a_gone_pid() {
        // AC7: a fingerprint whose pid is gone yields Ok(None) → the caller
        // reconciles the record to `failed`. Spawn `true`, let it exit, then try
        // to adopt its (now-dead) fingerprint.
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("true", &[])).expect("spawn true");
        let fp = backend.fingerprint(&proc);
        // Wait for it to exit + reap.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.poll(&mut proc).unwrap().is_exited() {
            assert!(Instant::now() < deadline);
            sleep(Duration::from_millis(10));
        }
        // Give the OS a moment to fully release the pid.
        sleep(Duration::from_millis(50));
        let adopted = backend.adopt(&fp).expect("adopt call ok");
        assert!(adopted.is_none(), "a gone pid must not be adopted");
    }

    #[test]
    fn adopt_returns_none_on_start_time_mismatch_pid_reuse_guard() {
        // AD-5 PID-reuse guard: a live pid whose start-time DIFFERS from the
        // recorded one is a DIFFERENT process (the pid was recycled) and must NOT
        // be adopted. Spawn a live process, then craft a fingerprint with its pid
        // but a deliberately wrong start-time.
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("sleep", &["30"])).expect("spawn");
        let real = backend.fingerprint(&proc);
        // Only meaningful where a real start-time source exists.
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let forged = ProcessFingerprint::new(real.pid, real.start_time.wrapping_add(1));
            let adopted = backend.adopt(&forged).expect("adopt call ok");
            assert!(
                adopted.is_none(),
                "a start-time mismatch (PID reuse) must not be adopted"
            );
        }
        let _ = backend.stop(&mut proc, Duration::from_secs(2));
    }

    #[test]
    fn adopted_poll_reports_exited_when_start_time_no_longer_matches_ai10() {
        // AI-10 (steady-state PID-reuse guard): an ADOPTED handle whose live PID is
        // still alive but whose start-time NO LONGER matches the recorded token is
        // a DIFFERENT process (the original crashed and the OS recycled its PID) —
        // `poll` must report `Exited`, NOT `Alive`, so the reaper detects the crash
        // instead of trusting a bare PID match.
        //
        // We prove this deterministically WITHOUT waiting for a real PID recycle:
        // adopt a genuinely live process (so the handle is a real adopted handle),
        // then construct a sibling handle for the SAME live pid whose recorded
        // start-time is deliberately wrong. A bare `kill(pid,0)` would still say
        // "alive"; the start-time re-check must override that to `Exited`.
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            return; // no real start-time source on this host — guard is a no-op.
        }
        let backend = UnixBackend::new();
        // Keep the ORIGINAL handle so the live process is reaped on drop (leak
        // guard) regardless of what the crafted handle reports.
        let mut original = backend
            .spawn(&spec("sleep", &["600"]))
            .expect("spawn sleep");
        let fp = backend.fingerprint(&original);
        assert!(fp.start_time > 0, "supported host has a real start-time");

        // A genuine adopted handle (correct start-time) polls Alive.
        let adopter = UnixBackend::new();
        let mut adopted = adopter
            .adopt(&fp)
            .expect("adopt call ok")
            .expect("a live matching process must be adopted");
        assert_eq!(
            adopter.poll(&mut adopted).unwrap(),
            ProcessStatus::Alive,
            "a matching adopted process is alive"
        );

        // Now craft an adopted-shaped handle for the SAME live pid but with a
        // recorded start-time that does NOT match the live one (models a recycled
        // PID). The pid is provably still alive (the original sleep is running).
        assert!(pid_alive(fp.pid), "the live pid is still running");
        let mut recycled = UnixProcess {
            child: None,
            pgid: Pid::from_raw(fp.pid as i32),
            pid: fp.pid,
            start_time: fp.start_time.wrapping_add(1),
        };
        assert_eq!(
            backend.poll(&mut recycled).unwrap(),
            ProcessStatus::Exited { code: None },
            "a live pid whose start-time no longer matches must poll Exited (AI-10), \
             not Alive — so a recycled PID cannot mask an adopted process's crash"
        );

        // Teardown the real process via the original owning handle.
        let _ = backend.stop(&mut original, Duration::from_secs(2));
    }

    #[test]
    fn adopted_poll_with_no_recorded_start_time_falls_back_to_bare_liveness() {
        // AI-10 degraded fallback: an adopted-shaped handle that carries NO
        // recorded start-time (`start_time == 0` — a host with no start-time
        // source, where `adopt` would not normally build a handle) must fall back
        // to the bare `kill(pid, 0)` liveness probe, not spuriously report Exited.
        // Construct such a handle over a genuinely live pid and assert Alive.
        let backend = UnixBackend::new();
        let mut original = backend
            .spawn(&spec("sleep", &["600"]))
            .expect("spawn sleep");
        let pid = original.pid;
        assert!(pid_alive(pid));

        let mut degraded = UnixProcess {
            child: None,
            pgid: Pid::from_raw(pid as i32),
            pid,
            start_time: 0, // no recorded token → bare-liveness fallback
        };
        assert_eq!(
            backend.poll(&mut degraded).unwrap(),
            ProcessStatus::Alive,
            "a live adopted pid with no recorded start-time falls back to bare liveness"
        );

        // Teardown via the real owner.
        let _ = backend.stop(&mut original, Duration::from_secs(2));
    }

    #[test]
    fn spawn_with_unopenable_log_file_is_a_spawn_error() {
        // The log_file cannot be opened (its parent is a regular file, not a
        // dir) → a Spawn error whose detail names the log-open failure, and
        // nothing is left running.
        let dir = tempfile::tempdir().unwrap();
        // Make `blocked` a FILE, then ask to log into `blocked/inner.log`.
        let blocker = dir.path().join("blocked");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let mut s = spec("sleep", &["30"]);
        s.log_file = Some(blocker.join("inner.log"));
        let backend = UnixBackend::new();
        let err = backend.spawn(&s).unwrap_err();
        match err {
            BackendError::Spawn { detail, .. } => {
                assert!(detail.contains("log file"), "detail={detail}")
            }
            other => panic!("expected Spawn, got {other}"),
        }
    }

    #[test]
    fn signal_group_treats_missing_group_as_success() {
        // Signalling a group that does not exist returns Ok (ESRCH → success).
        // Use a pgid extremely unlikely to exist.
        let result = signal_group(Pid::from_raw(2_000_000_000), Signal::SIGTERM);
        assert!(result.is_ok(), "missing group must be Ok, got {result:?}");
    }

    /// Whether a pid is still alive (Unix): `kill(pid, 0)` succeeds while it
    /// lives, fails with ESRCH once it is gone. Test-only liveness probe.
    fn pid_alive(pid: u32) -> bool {
        use nix::sys::signal::kill;
        !matches!(
            kill(Pid::from_raw(pid as i32), None),
            Err(nix::errno::Errno::ESRCH)
        )
    }

    #[test]
    fn stop_kills_the_whole_group_no_child_survivor() {
        // THE load-bearing AC3 test (Unix): spawn `fake_agent --spawn-child`,
        // which forks a lingering CHILD in the same process group, then stop the
        // group and assert BOTH the parent AND the child are gone. A naive
        // "kill the parent PID" would miss the child; the process-group SIGKILL
        // catches it. The child pid is read from the redirected agent log.
        let dir = tempfile::tempdir().unwrap();
        let agent_log = dir.path().join("agent.log");
        let bin = fake_agent_path();
        let mut s = SpawnSpec {
            exec: bin.to_string_lossy().into_owned(),
            args: vec![
                "--spawn-child".to_string(),
                "--linger-ms".to_string(),
                "600000".to_string(),
            ],
            env: BTreeMap::new(),
            working_dir: dir.path().to_path_buf(),
            log_file: Some(agent_log.clone()),
        };
        s.env.clear();

        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&s).expect("spawn fake_agent --spawn-child");
        let parent_pid = proc.pid;

        // Wait for the child pid to be announced in the agent log.
        let child_pid = {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(contents) = std::fs::read_to_string(&agent_log) {
                    if let Some(line) = contents.lines().find(|l| l.starts_with("child-pid=")) {
                        break line["child-pid=".len()..].trim().parse::<u32>().unwrap();
                    }
                }
                assert!(Instant::now() < deadline, "child pid never announced");
                sleep(Duration::from_millis(20));
            }
        };
        assert!(pid_alive(parent_pid), "parent should be alive before stop");
        assert!(pid_alive(child_pid), "child should be alive before stop");

        // Stop with a short window; the group SIGKILL must catch both.
        let outcome = backend
            .stop(&mut proc, Duration::from_millis(200))
            .expect("stop");
        // fake_agent ignores nothing, so SIGTERM should end the parent within the
        // window (graceful); regardless, no process survives.
        let _ = outcome;

        // Give the OS a moment to tear down the group.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !pid_alive(parent_pid) && !pid_alive(child_pid) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "parent(alive={}) or child(alive={}) survived the group kill",
                pid_alive(parent_pid),
                pid_alive(child_pid)
            );
            sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn stop_escalates_to_forced_kill_when_graceful_window_elapses() {
        // AC3 escalation (Unix): a process that outlives a SHORT graceful window
        // is force-killed (SIGKILL), and the outcome records forced == true. We
        // force this WITHOUT an OS-cfg signal handler by giving the process a
        // long linger and the stop a tiny window — so the graceful SIGTERM does
        // not matter (fake_agent has no handler, so SIGTERM actually ends it
        // fast). To truly exercise escalation we need the process to survive
        // SIGTERM; `sh -c 'trap "" TERM; sleep 60'` ignores SIGTERM portably in
        // POSIX sh, forcing the window to elapse and SIGKILL to fire.
        let backend = UnixBackend::new();
        let mut proc = backend
            .spawn(&spec("sh", &["-c", "trap '' TERM; sleep 60"]))
            .expect("spawn sh trap");
        // Let the trap install.
        sleep(Duration::from_millis(100));
        let outcome = backend
            .stop(&mut proc, Duration::from_millis(200))
            .expect("stop");
        assert!(
            outcome.forced,
            "a SIGTERM-ignoring process must be force-killed (escalation)"
        );
        assert!(backend.poll(&mut proc).unwrap().is_exited());
    }

    #[test]
    fn pause_freezes_the_process_then_resume_wakes_it() {
        // AC1 (Unix guaranteed): spawn `fake_agent --heartbeat-ms 50`, which
        // prints an incrementing `heartbeat <n>` line to its log every ~50ms.
        // While SIGSTOP'd the process freezes, so the log's line count STOPS
        // growing; SIGCONT resumes it and the count grows again. This is the
        // cross-Unix-safe suspension proof (no /proc dependency).
        let dir = tempfile::tempdir().unwrap();
        let agent_log = dir.path().join("agent.log");
        let bin = fake_agent_path();
        let mut s = SpawnSpec {
            exec: bin.to_string_lossy().into_owned(),
            args: vec![
                "--heartbeat-ms".to_string(),
                "50".to_string(),
                "--linger-ms".to_string(),
                "600000".to_string(),
            ],
            env: BTreeMap::new(),
            working_dir: dir.path().to_path_buf(),
            log_file: Some(agent_log.clone()),
        };
        s.env.clear();

        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&s).expect("spawn fake_agent --heartbeat-ms");

        // Wait for the heartbeat to actually start ticking (a couple of lines).
        let line_count = |path: &std::path::Path| -> usize {
            std::fs::read_to_string(path)
                .map(|c| c.lines().filter(|l| l.starts_with("heartbeat ")).count())
                .unwrap_or(0)
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if line_count(&agent_log) >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "heartbeat never started");
            sleep(Duration::from_millis(20));
        }

        // Pause: SIGSTOP the group, then confirm the heartbeat is FROZEN.
        //
        // Robustness under scheduler jitter (LOW-3): rather than compare two
        // instantaneous samples (fragile when a loaded runner delays a read, or
        // when the child's in-flight 25ms poll lands one last line right after
        // pause returns), we settle briefly, snapshot a BASELINE, then watch
        // across a LONG window (1s ≫ many 50ms intervals) and require the count
        // NEVER exceeds baseline. A stuck-but-alive scheduler cannot make a
        // SUSPENDED process emit, so this tolerates jitter; yet it stays a
        // GENUINE proof — if the `pause()` (SIGSTOP) were removed, a live 50ms
        // heartbeat would emit ~20 lines here and exceed baseline on the first
        // poll, firing the assert. (Resume below further requires renewed growth.)
        backend.pause(&mut proc).expect("pause");
        sleep(Duration::from_millis(200)); // let SIGSTOP + any in-flight line settle
        let baseline = line_count(&agent_log);
        let watch_until = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < watch_until {
            let now = line_count(&agent_log);
            assert!(
                now <= baseline,
                "heartbeat must NOT grow while paused (SIGSTOP) — baseline {baseline}, saw {now}"
            );
            sleep(Duration::from_millis(50));
        }
        let paused_after = line_count(&agent_log);
        assert_eq!(
            paused_after, baseline,
            "heartbeat count must be unchanged across the paused window: {baseline} → {paused_after}"
        );

        // Resume: SIGCONT the group; the heartbeat must grow again.
        backend.resume(&mut proc).expect("resume");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if line_count(&agent_log) > paused_after {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "heartbeat must resume growing after SIGCONT (stuck at {paused_after})"
            );
            sleep(Duration::from_millis(20));
        }

        // Teardown.
        let _ = backend.stop(&mut proc, Duration::from_millis(200));
    }

    #[test]
    fn pause_and_resume_on_an_already_exited_process_are_harmless_no_ops() {
        // A dead process cannot be paused/resumed; both are harmless no-ops
        // (parity with stop-on-dead). `true` exits immediately.
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&spec("true", &[])).expect("spawn true");
        sleep(Duration::from_millis(50));
        // Reap it via poll so the handle knows it exited.
        assert!(backend.poll(&mut proc).unwrap().is_exited());
        backend.pause(&mut proc).expect("pause on dead is Ok");
        backend.resume(&mut proc).expect("resume on dead is Ok");
    }

    #[test]
    fn working_dir_and_env_are_applied() {
        // Prove the working dir + env override reach the child: run `sh -c` that
        // writes $PWD and $KT_TEST into the log, and assert both.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        let mut s = SpawnSpec {
            exec: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf 'pwd=%s env=%s' \"$PWD\" \"$KT_TEST\"".to_string(),
            ],
            env: BTreeMap::new(),
            working_dir: dir.path().to_path_buf(),
            log_file: Some(log.clone()),
        };
        s.env.insert("KT_TEST".to_string(), "applied".to_string());
        let backend = UnixBackend::new();
        let mut proc = backend.spawn(&s).expect("spawn sh");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !backend.poll(&mut proc).unwrap().is_exited() {
            assert!(Instant::now() < deadline);
            sleep(Duration::from_millis(10));
        }
        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("env=applied"), "log={contents:?}");
        // The working dir is the temp dir (canonicalize to dodge /var→/private/var).
        let want = std::fs::canonicalize(dir.path()).unwrap();
        assert!(
            contents.contains(&format!("pwd={}", want.display()))
                || contents.contains(&format!("pwd={}", dir.path().display())),
            "log={contents:?} want pwd={}",
            want.display()
        );
    }
}
