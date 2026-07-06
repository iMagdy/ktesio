//! The [`ProcessBackend`] port (hexagonal, spine AD-1/AD-4).
//!
//! All process control — spawning an agent, stopping it, checking whether it is
//! still alive — enters the engine through this ONE port. Its methods speak
//! DOMAIN terms ([`SpawnSpec`], [`StopOutcome`], [`ProcessStatus`]), never OS
//! syscalls. The per-OS implementations live behind it in
//! `crates/ktesio-engine/src/backends/{unix,windows}/` — the sole allowlisted
//! home for `#[cfg]` OS attributes (the OS-cfg CI gate). This file, and every
//! other engine module except `backends/`, is cfg-free.
//!
//! ## Sync trait, blocking pool (documented choice)
//!
//! The trait methods are **synchronous** thin syscall wrappers. The async
//! [`Engine`](crate::Engine) calls them from tokio's blocking pool
//! (`spawn_blocking`), so a blocking syscall (or the bounded graceful-stop wait)
//! never stalls an async worker. Keeping the trait sync keeps each backend a
//! minimal syscall shim and keeps the OS-specific surface as small as possible.
//!
//! ## Associated `Handle` (why not `Box<dyn>`)
//!
//! Each backend owns its running-process resources through an associated
//! [`ProcessBackend::Handle`] type (on Unix: the child + its process-group id;
//! on Windows: the child + its Job Object). Exactly one backend is compiled per
//! target (cfg-selected in `backends/mod.rs`), so the supervisor names the
//! concrete handle through a cfg-selected type alias — no trait objects, no
//! dynamic dispatch, and the port stays free of OS types.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// The resolved launch of an Agent Instance (built from a manifest `OpTemplate`
/// or a native adapter). Everything the backend needs to spawn the process, in
/// domain terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    /// The executable to run (a program name resolved on `PATH`, or a path).
    pub exec: String,
    /// Positional arguments.
    pub args: Vec<String>,
    /// Environment overrides applied on top of the inherited environment.
    pub env: BTreeMap<String, String>,
    /// The working directory for the child — the Agent Home.
    pub working_dir: PathBuf,
    /// A file the child's stdout/stderr are redirected to — the per-instance log
    /// seed (AD-12). `None` inherits the engine's streams (used only in tests
    /// that do not assert captured output).
    pub log_file: Option<PathBuf>,
}

/// The outcome of a [`ProcessBackend::stop`] call (AC3).
///
/// Records whether the graceful request sufficed or the backend had to escalate
/// to a forced kill after the window, so the supervisor can record the
/// escalation in the instance log and emit the right transition cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopOutcome {
    /// `true` if the graceful window elapsed and the process (group/job) was
    /// force-killed; `false` if it exited gracefully within the window.
    pub forced: bool,
}

/// Whether a supervised process is still alive (used for the `starting→running`
/// readiness check and `stopping→stopped` detection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    /// The process is still running.
    Alive,
    /// The process has exited; carries its exit code if one was reported.
    Exited {
        /// The exit code, if the OS reported one (`None` if killed by a signal
        /// with no code, etc.).
        code: Option<i32>,
    },
}

impl ProcessStatus {
    /// Whether the process has exited.
    pub fn is_exited(&self) -> bool {
        matches!(self, ProcessStatus::Exited { .. })
    }
}

/// A durable fingerprint of a supervised process (spine AD-5) — the PID-reuse
/// guard behind orphan adoption.
///
/// AD-5 requires the write-ahead spawn record to carry the process
/// `{ pid, start-time fingerprint }` so that, after an engine crash + restart,
/// a persisted record can be reconciled against live processes WITHOUT a false
/// adoption when the OS has recycled the PID for an unrelated new process. Two
/// different processes that happen to reuse a PID have DIFFERENT start-times, so
/// comparing the recorded `start_time` to a live PID's start-time is the guard:
/// a match adopts, a mismatch (or a gone PID) reconciles to `failed`.
///
/// `start_time` is an OPAQUE, monotonic-per-boot token whose UNIT is per-OS
/// (Linux: clock ticks since boot from `/proc/<pid>/stat` field 22; macOS: the
/// process start-time from `libproc` `proc_pidinfo(PROC_PIDTBSDINFO)`
/// (`pbi_start_tvsec`/`pbi_start_tvusec`) folded to microseconds; Windows: the
/// `GetProcessTimes` creation time in 100ns ticks). Its only contract is: STABLE for a given
/// process across reads, and DIFFERENT across a PID reuse. It is a domain value
/// (no OS type crosses the port); the per-OS sources live only in `backends/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessFingerprint {
    /// The OS process id.
    pub pid: u32,
    /// The opaque, per-OS process start-time token (see the type docs). Stable
    /// per process; differs across a PID reuse.
    pub start_time: u64,
}

impl ProcessFingerprint {
    /// Construct a fingerprint from a pid + start-time token.
    pub fn new(pid: u32, start_time: u64) -> Self {
        Self { pid, start_time }
    }

    /// Whether this fingerprint identifies the SAME process as `other`: both the
    /// pid AND the start-time token must match. A pid match with a different
    /// start-time is a PID reuse (a DIFFERENT process), not the same one.
    pub fn matches(&self, other: &ProcessFingerprint) -> bool {
        self.pid == other.pid && self.start_time == other.start_time
    }
}

/// Why a process operation failed, in domain terms (never an OS error code).
///
/// The backends map their OS failures into these variants; the supervisor maps
/// them onward into an [`EngineError`](crate::domain::EngineError). `thiserror`
/// in the engine (no miette here — conventions).
#[derive(Debug, Error)]
pub enum BackendError {
    /// The process could not be spawned (exec not found, permission denied, a
    /// bad working dir, …). Carries the exec and the underlying detail so the
    /// diagnostic can be preserved verbatim (AC2).
    #[error("could not launch '{exec}': {detail}")]
    Spawn {
        /// The executable that failed to launch.
        exec: String,
        /// The underlying OS/spawn error detail (preserved for AC2).
        detail: String,
    },

    /// The process spawned but exited immediately during startup with a failure
    /// code — treated as a launch failure (AC2). Carries the code.
    #[error("'{exec}' exited immediately during startup with code {code}")]
    ImmediateExit {
        /// The executable.
        exec: String,
        /// The non-zero exit code observed at startup.
        code: i32,
    },

    /// A stop/kill/poll/pause/resume syscall failed unexpectedly. Carries the
    /// operation and detail. (A process that is already gone is NOT an error —
    /// that is the desired end state for stop; a SIGSTOP/SIGCONT to a gone group
    /// likewise resolves to success via `ESRCH`, see the Unix backend.)
    #[error("process control operation '{op}' failed: {detail}")]
    Control {
        /// The operation that failed (`"signal"`, `"wait"`, `"terminate"`,
        /// `"pause"`, `"resume"`, `"fingerprint"`, `"adopt"`, …).
        op: &'static str,
        /// The underlying detail.
        detail: String,
    },
}

/// The process-control port (spine AD-1 side port; AD-4 per-OS).
///
/// The domain/supervisor names this trait and gets a concrete implementation via
/// [`crate::backends::current`]; it never names a concrete backend or any OS
/// type. Implementors live only under `backends/{unix,windows}/`.
pub trait ProcessBackend {
    /// The backend-owned handle to a running process (the child + its
    /// group/job). Opaque to the supervisor, which stores it via a cfg-selected
    /// alias.
    type Handle: Send;

    /// Spawn the process described by `spec` into its own isolation group
    /// (a dedicated process group on Unix; a Job Object on Windows), so a later
    /// [`ProcessBackend::stop`] can terminate the WHOLE tree — catching any child
    /// processes the agent itself spawned (AC3 "no process of the instance
    /// survives"). Returns a handle on success; on a launch failure returns a
    /// [`BackendError`] and leaves NO zombie (AC2).
    fn spawn(&self, spec: &SpawnSpec) -> Result<Self::Handle, BackendError>;

    /// Request graceful shutdown of the process (group/job) and, if it has not
    /// exited within `graceful_window`, escalate to a forced kill of the whole
    /// group/job. Reaps the process so no zombie remains. Returns a
    /// [`StopOutcome`] recording whether escalation happened (AC3).
    fn stop(
        &self,
        handle: &mut Self::Handle,
        graceful_window: Duration,
    ) -> Result<StopOutcome, BackendError>;

    /// Non-blocking liveness check for the process, reaping it if it has exited
    /// (used for the `starting→running` readiness check and, later,
    /// crash detection). Never blocks.
    fn poll(&self, handle: &mut Self::Handle) -> Result<ProcessStatus, BackendError>;

    /// Suspend the process (group/job) — the GUARANTEED-path pause primitive
    /// (story 1-5, AC1). On Unix this delivers `SIGSTOP` to the whole process
    /// group, an uncatchable, verifiable suspension. On Windows there is no clean
    /// guaranteed whole-process suspend from `std` (AD-4), so the Windows body is
    /// the cooperative best-effort form (it succeeds without a hard suspension —
    /// the VISIBLE best-effort qualifier emitted by the supervisor/CLI, not the
    /// backend, is what keeps that honest).
    ///
    /// IMPORTANT — this method is only invoked by the supervisor on the
    /// GUARANTEED dispatch path (pause `SupportLevel::Guaranteed` on the current
    /// OS). The best-effort and unsupported levels never reach a backend call:
    /// the three-level DISPATCH is the supervisor's job, keyed on the declared
    /// per-OS `SupportLevel`, not the backend's. Sync (called via
    /// `spawn_blocking`, like `stop`/`spawn`/`poll`). Domain terms only.
    ///
    /// A process that has already exited is not an error (parity with `stop`):
    /// the desired suspended-or-gone end state already holds. Reports
    /// [`BackendError::Control`] (op `"pause"`/`"signal"`) only on an unexpected
    /// syscall failure.
    fn pause(&self, handle: &mut Self::Handle) -> Result<(), BackendError>;

    /// Resume the process (group/job) — the GUARANTEED-path resume primitive
    /// (story 1-5, AC1). On Unix this delivers `SIGCONT` to the whole process
    /// group; on Windows it is the cooperative best-effort counterpart of
    /// [`ProcessBackend::pause`]. Same dispatch contract, sync semantics, and
    /// already-exited tolerance as [`ProcessBackend::pause`]. Reports
    /// [`BackendError::Control`] (op `"resume"`/`"signal"`) only on an unexpected
    /// syscall failure.
    fn resume(&self, handle: &mut Self::Handle) -> Result<(), BackendError>;

    /// The OS process id of the spawned child (for the [`ProcessFingerprint`] /
    /// diagnostics). A stable accessor so the supervisor can log the pid without
    /// naming an OS type.
    fn pid(&self, handle: &Self::Handle) -> u32;

    /// The durable [`ProcessFingerprint`] of a supervised process (spine AD-5) —
    /// its `{ pid, start-time }`. Written into the write-ahead spawn record
    /// BEFORE the instance is treated as supervised, so a later engine restart
    /// can adopt it back without a PID-reuse false match.
    ///
    /// The start-time source is per-OS and lives only in `backends/`: Linux reads
    /// `/proc/<pid>/stat` field 22; macOS reads the process start-time via
    /// `libproc` `proc_pidinfo(PROC_PIDTBSDINFO)`; Windows reads the
    /// `GetProcessTimes` creation time. Reading the start-time of a process THIS backend spawned and holds
    /// alive cannot fail in normal operation; a read failure falls back to a
    /// start-time of `0` (the record still carries the pid — a degraded but
    /// honest fingerprint) rather than erroring the spawn. Sync (called via
    /// `spawn_blocking`). Domain terms only.
    fn fingerprint(&self, handle: &Self::Handle) -> ProcessFingerprint;

    /// Try to RE-ACQUIRE a live process matching `fingerprint`, for orphan
    /// adoption on engine start (spine AD-5, AC-B). Returns:
    /// * `Ok(Some(handle))` — a live process with pid `fingerprint.pid` exists
    ///   AND its current start-time equals `fingerprint.start_time` (the SAME
    ///   process, re-held so `stop`/`pause`/`poll` work on it again); the state
    ///   stays as persisted (`running`/`paused`).
    /// * `Ok(None)` — no live process matches (the pid is gone, OR it was reused
    ///   by a DIFFERENT process whose start-time differs — the PID-reuse guard);
    ///   the caller reconciles the record to `failed`.
    ///
    /// Re-acquisition keeps the OS specifics (how to re-open a live PID, verify
    /// its start-time, and re-establish group/job control) inside `backends/`.
    /// An adopted handle must remain signal-able / stoppable as its original
    /// group/job (Unix: the pgid == pid is re-derived; Windows: re-open by pid).
    /// Sync (called via `spawn_blocking`). Reports [`BackendError::Control`]
    /// (op `"adopt"`) only on an unexpected syscall failure — a plain "no live
    /// match" is `Ok(None)`, not an error.
    fn adopt(&self, fingerprint: &ProcessFingerprint)
        -> Result<Option<Self::Handle>, BackendError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_status_is_exited_predicate() {
        assert!(!ProcessStatus::Alive.is_exited());
        assert!(ProcessStatus::Exited { code: Some(0) }.is_exited());
        assert!(ProcessStatus::Exited { code: None }.is_exited());
    }

    #[test]
    fn stop_outcome_records_forced_flag() {
        assert!(StopOutcome { forced: true }.forced);
        assert!(!StopOutcome { forced: false }.forced);
    }

    #[test]
    fn fingerprint_matches_requires_both_pid_and_start_time() {
        // AD-5 PID-reuse guard: the SAME process matches on both fields; a pid
        // match with a different start-time is a REUSE (different process).
        let a = ProcessFingerprint::new(1234, 999);
        assert!(
            a.matches(&ProcessFingerprint::new(1234, 999)),
            "same → match"
        );
        assert!(
            !a.matches(&ProcessFingerprint::new(1234, 1000)),
            "pid reuse (start-time differs) → no match"
        );
        assert!(
            !a.matches(&ProcessFingerprint::new(5678, 999)),
            "different pid → no match"
        );
    }

    #[test]
    fn backend_error_messages_name_the_exec_and_detail() {
        let e = BackendError::Spawn {
            exec: "no-such-bin".to_string(),
            detail: "No such file or directory (os error 2)".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("no-such-bin"), "{msg}");
        assert!(msg.contains("No such file"), "{msg}");

        let e2 = BackendError::ImmediateExit {
            exec: "flaky".to_string(),
            code: 7,
        };
        assert!(e2.to_string().contains("code 7"));
    }

    #[test]
    fn spawn_spec_equality_is_structural() {
        let a = SpawnSpec {
            exec: "x".to_string(),
            args: vec!["--serve".to_string()],
            env: BTreeMap::new(),
            working_dir: PathBuf::from("/home"),
            log_file: None,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
