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
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ChildStdin;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use thiserror::Error;

use crate::domain::{LogLine, LogStream};
use crate::time::now_rfc3339;

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
    /// Where the ATTRIBUTED, rotated capture (`agent-out`/`agent-err`/`engine`
    /// lines) should be written — the CURRENT generation file (story 4-2,
    /// AD-12). `Some` in every PRODUCTION spawn, paired 1:1 with
    /// `log_file: Some(..)` (the supervisor computes both from the SAME
    /// `Registry` path authority in the same breath) — capture is
    /// UNCONDITIONAL and capability-independent (AC-E), never gated on
    /// `Capability::Interaction` the way [`SpawnSpec::pipe_stdin`] gates the
    /// stdin *write* direction. `None` only alongside `log_file: None`, the
    /// small set of unit tests that assert nothing about captured output —
    /// this pairing is a narrow test-fixture convenience, not a capability
    /// gate: reading FROM a process is never gated, only writing TO it is.
    pub attributed_log_path: Option<PathBuf>,
    /// The Agent Instance name, stamped on every captured [`LogLine`]
    /// (`LogLine.instance`) — paired with [`SpawnSpec::attributed_log_path`].
    pub instance_name: String,
    /// Whether to pipe the child's stdin (story 4.1 fix pass, HIGH finding —
    /// review of #79).
    ///
    /// `true` only when the instance's declared `Capability::Interaction`
    /// level on the CURRENT OS is `Guaranteed` or `BestEffort` — the caller
    /// (the supervisor, at spawn time) reads the effective Capability
    /// Declaration and sets this field; the backends themselves stay
    /// capability-agnostic (dumb process executors), mirroring how this
    /// codebase already gates BEHAVIOR (not just callability) on declared
    /// capabilities elsewhere (e.g. pause's SIGSTOP-vs-noop branching).
    ///
    /// `false` uses the pre-story-4.1 safe default (`Stdio::null()`): an
    /// adapter that declares no interaction support behaves EXACTLY as
    /// before spawn ever piped anything. This matters because the story's
    /// original implementation piped stdin UNCONDITIONALLY for every
    /// spawned process: a process that does a blocking read of stdin at
    /// startup (a common "sniff for piped input" real-CLI idiom) then hangs
    /// forever, because the engine holds the pipe's write end open for the
    /// process's entire supervised lifetime and nothing ever writes to it
    /// unless `send` is called — the child never sees EOF and never
    /// unblocks, yet is reported `running` (readiness here is just "the
    /// process didn't exit immediately"), a silent deadlock with no error
    /// signal anywhere.
    pub pipe_stdin: bool,
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

    /// A [`ProcessBackend::write_stdin`] call did not complete within
    /// [`STDIN_WRITE_TIMEOUT`] (story 4.1 fix pass, the CRITICAL finding —
    /// review of #79). See [`write_stdin_bounded`]'s docs for the full
    /// mechanism. The instance's interaction channel is now PERMANENTLY
    /// broken for the remainder of this engine session (a stop/start builds
    /// an entirely fresh handle/pipe).
    #[error(
        "stdin write did not complete within {timeout_secs}s — the agent may not be draining its input"
    )]
    StdinTimedOut {
        /// The bound (seconds) that elapsed before the write was abandoned.
        timeout_secs: u64,
    },
}

/// How long a [`ProcessBackend::write_stdin`] call may block before the
/// instance's interaction channel is declared permanently broken for the rest
/// of this engine session (story 4.1 fix pass — the CRITICAL finding: the
/// ENTIRE engine shares ONE supervisor lock — `EngineInner::supervisor` in
/// `engine.rs` — so an unbounded blocking write to one instance's stdin pipe
/// could freeze every other instance's `start`/`stop`/`pause`/`send` AND the
/// crash-detection reaper forever, since none of them can acquire the lock
/// until the write returns; an adversarial audit reproduced this empirically
/// against the story's original unbounded `write_all`).
///
/// 5 seconds is a conservative, deliberately non-configurable bound (this is
/// an internal resilience bound, not a user-facing setting — no new CLI
/// flag/config surface is warranted, since neither `epics.md` nor the story
/// calls for one): a healthy agent draining the realistic operator payload
/// (`send`'s `text` argument — a line or a short paragraph of interactive
/// input) completes in low-single-digit milliseconds even on a loaded CI
/// runner, so 5s leaves generous headroom for scheduler jitter while still
/// keeping a genuinely stuck agent from wedging the engine for anything
/// beyond a human-noticeable pause. It intentionally mirrors the order of
/// magnitude of [`crate::domain::DEFAULT_STOP_WINDOW`]'s sibling bounded-wait
/// (30s) scaled down for a much smaller, interactive-sized payload rather
/// than a whole graceful-shutdown grace period.
pub const STDIN_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The per-generation byte cap for the attributed output capture (story 4-2,
/// AD-12, epics.md's literal "10MB × 3"). A FIXED, non-configurable engine
/// resilience bound (no unified-config key this story) — mirrors
/// [`STDIN_WRITE_TIMEOUT`]'s precedent of a hardcoded bound rather than a
/// user setting. Checked BEFORE each append ([`should_rotate`]), so a single
/// generation may end up a little over this exact byte count (the cost of a
/// cheap check-before-write rather than a mid-line truncation); this is an
/// accepted approximation, not an exact ceiling.
pub const LOG_ROTATE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// How many generations of the attributed capture are retained: the CURRENT
/// file plus this many minus one rotated predecessors (`.1`, `.2`, …) — AD-12's
/// "10MB × 3" (current + `.1` + `.2`). A read that spans a rotation boundary
/// honestly returns whatever these retained generations hold; `kt agent logs`
/// never errors due to rotation.
pub const LOG_ROTATE_GENERATIONS: u8 = 3;

/// Whether the attributed capture's CURRENT generation should be rotated
/// BEFORE the next append, given its current byte length (story 4-2, Task 1).
/// Pure, no I/O — unit-testable at the exact boundary. Uses `>=` (the `≥`
/// boundary convention this codebase uses everywhere else, e.g.
/// `BudgetEvaluator`): a generation that has JUST reached the cap rotates on
/// its NEXT append, rather than growing past it first.
fn should_rotate(current_len: u64) -> bool {
    current_len >= LOG_ROTATE_MAX_BYTES
}

/// The state of a spawned process's stdin channel (story 4.1 fix pass — the
/// CRITICAL and HIGH findings, review of #79).
///
/// Deliberately not a bare `Option<ChildStdin>` (as the story originally
/// shipped it): a bounded write timeout ([`STDIN_WRITE_TIMEOUT`]) introduces
/// a THIRD possibility beyond "has a pipe" / "never had one" — "had one, but
/// a write to it never came back in time, so it can never be safely touched
/// again". [`StdinState::TimedOut`] must stay clearly distinguishable from
/// both [`StdinState::NoPipe`] and [`StdinState::Live`].
#[derive(Debug)]
pub enum StdinState {
    /// No pipe is held for this handle. Covers BOTH: an ADOPTED process (no
    /// OS-portable, documented way to recover a pipe file descriptor from a
    /// bare PID; see [`ProcessBackend::adopt`]'s docs) and a FRESHLY SPAWNED
    /// process whose declared `Capability::Interaction` level was
    /// `Unsupported` on this OS, so [`SpawnSpec::pipe_stdin`] was `false` and
    /// the child's stdin was never piped at all (the HIGH finding's fix —
    /// `Stdio::null()`, the pre-story-4.1 safe default).
    NoPipe,
    /// A live pipe this handle can write to right now.
    Live(ChildStdin),
    /// A write to this pipe exceeded [`STDIN_WRITE_TIMEOUT`]. The spawned
    /// write thread (see [`write_stdin_bounded`]) may STILL be running,
    /// blocked on the OS write syscall — there is no safe, portable way to
    /// cancel a running [`std::thread`] or reclaim a `ChildStdin` it may
    /// still hold, so this state is PERMANENT for the remainder of this
    /// handle's life (until the instance is stopped and started again, which
    /// builds an entirely fresh handle/pipe). Every SUBSEQUENT `send` on this
    /// instance must fail fast on this state, never attempting another
    /// doomed write.
    TimedOut,
}

impl StdinState {
    /// Whether this state holds a live pipe that can be written to right now.
    pub fn is_live(&self) -> bool {
        matches!(self, StdinState::Live(_))
    }

    /// Whether a PRIOR write on this state exceeded [`STDIN_WRITE_TIMEOUT`]
    /// (see [`StdinState::TimedOut`]).
    pub fn is_timed_out(&self) -> bool {
        matches!(self, StdinState::TimedOut)
    }
}

/// Write `data` to `state`'s pipe and flush it, bounded to `timeout` (story
/// 4.1 fix pass — the CRITICAL finding, review of #79).
///
/// A portable, `std`-only bounded-write pattern that needs no raw
/// non-blocking I/O or fd manipulation (which would need different,
/// error-prone implementations per OS, unlike this function which is called
/// identically from both backends): TAKES OWNERSHIP of the `ChildStdin` out
/// of `state` (via [`std::mem::replace`], so nothing else can observe or
/// touch it while a write may be in-flight), moves it onto a spawned
/// [`std::thread`] that performs the actual `write_all` + `flush`, and blocks
/// the CALLER (only — never the child thread) up to `timeout` on an
/// [`mpsc::channel`]'s [`mpsc::Receiver::recv_timeout`].
///
/// Outcomes:
/// * **Success within `timeout`**: the spawned thread's `(ChildStdin,
///   io::Result<()>)` comes back on the channel. The `ChildStdin` is put BACK
///   into `state` (`Live`) so a LATER write on the same handle can reuse the
///   same pipe — safe because the thread has already finished with it (its
///   `write_all`/`flush` calls returned before sending, and ownership moved
///   back through the channel, so there is no concurrent access).
/// * **A genuine I/O failure within `timeout`** (e.g. `EPIPE` — the agent
///   exited between the caller's liveness check and this write): reported as
///   [`BackendError::Control`], exactly like every other backend op. The
///   `ChildStdin` is likewise put back (an I/O error means the thread
///   returned, not that it is stuck).
/// * **Timeout**: the thread may still be blocked on the write syscall.
///   Reclaiming the `ChildStdin` would race a write that could complete at
///   any moment on a handle nothing else should concurrently touch, so
///   `state` is set to [`StdinState::TimedOut`] and this returns
///   [`BackendError::StdinTimedOut`]. The thread is intentionally NEITHER
///   joined NOR aborted (there is no safe, portable way to cancel a blocked
///   `std::thread`); it is simply abandoned. If the write eventually
///   completes or fails, its result is sent to a receiver nobody is
///   listening to anymore (a harmless dropped `Err(SendError)`) and the
///   thread exits normally, dropping its `ChildStdin` (closing the pipe).
///   This leaks the thread until the write unblocks (the agent starts
///   draining, the pipe breaks, or the process exits) or the whole engine
///   process exits; acceptable because a stuck agent is already an
///   operator-actionable failure (the diagnostic says to restart the
///   instance), not a steady-state occurrence.
///
/// Called when `state` is NOT [`StdinState::Live`] (no live pipe: `NoPipe` or
/// already `TimedOut`) is a defensive misuse the caller must check for first
/// via [`StdinState::is_live`] / [`ProcessBackend::has_stdin`] — `state` is
/// left UNCHANGED (never fabricating a timeout on a handle that never had, or
/// already lost, a pipe) and this returns [`BackendError::Control`], mirroring
/// the pre-fix defensive-misuse contract.
pub(crate) fn write_stdin_bounded(
    state: &mut StdinState,
    data: &[u8],
    timeout: Duration,
) -> Result<(), BackendError> {
    let stdin = match std::mem::replace(state, StdinState::TimedOut) {
        StdinState::Live(stdin) => stdin,
        other => {
            // Defensive misuse: restore the ORIGINAL state unchanged.
            *state = other;
            return Err(BackendError::Control {
                op: "stdin",
                detail: "no stdin pipe held for this handle".to_string(),
            });
        }
    };
    // `state` is now provisionally `TimedOut` for the duration of the write —
    // the honest, conservative default if this function returns early (e.g. a
    // future refactor introduces an early return) without reaching the result
    // handling below: a handle must never look reusable when we are not sure
    // it is.
    let (tx, rx) = mpsc::channel();
    let payload = data.to_vec();
    thread::spawn(move || {
        let mut stdin = stdin;
        let result = stdin.write_all(&payload).and_then(|()| stdin.flush());
        // The receiver may already be gone (the caller timed out) — a send
        // error just means nobody is listening anymore; either way the
        // thread exits normally here, dropping `stdin` on the tuple's drop
        // (closing the pipe) if nobody claimed it.
        let _ = tx.send((stdin, result));
    });
    match rx.recv_timeout(timeout) {
        Ok((stdin, Ok(()))) => {
            *state = StdinState::Live(stdin);
            Ok(())
        }
        Ok((stdin, Err(e))) => {
            *state = StdinState::Live(stdin);
            Err(BackendError::Control {
                op: "stdin",
                detail: e.to_string(),
            })
        }
        Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Timeout, or the thread panicked before sending (defensive —
            // write_all/flush do not panic in practice). `state` is already
            // TimedOut (set above); the thread may still be running/blocked,
            // so the ChildStdin can never be safely reclaimed.
            Err(BackendError::StdinTimedOut {
                timeout_secs: timeout.as_secs(),
            })
        }
    }
}

// ---- Story 4-2 (AD-12): the shared output-capture reader/writer threads ----
//
// Three distinct sources can produce a `LogLine` for the same instance: the
// stdout reader thread, the stderr reader thread, and (Task 4) the
// supervisor's own transition-time `engine`-attributed sends — potentially
// from three different OS threads. Rather than a shared `Mutex` guarding a
// rotate-then-append sequence, ONE background writer thread owns the current-
// generation file handle and performs every rotation-check-then-append
// SERIALLY, fed by a single `mpsc::Sender<LogLine>` every source clones —
// eliminating the lock entirely (no two threads ever touch the file) and
// mirroring `write_stdin_bounded`'s established `std::thread` + `mpsc`
// continuous-I/O convention (the SAME pattern, not a new one). The writer
// thread exits naturally when every `Sender` clone is dropped (the channel
// closes, `recv()` returns `Err`) — no explicit shutdown signal needed.

/// Append ONE [`LogLine`] to the attributed capture at `path`, rotating FIRST
/// if the file's CURRENT size has already reached [`LOG_ROTATE_MAX_BYTES`]
/// ([`should_rotate`]) — so no line ever spans a rotation boundary. One
/// JSON-Lines record per call.
///
/// A free function (not a method) so it is directly unit-testable in a tight
/// loop with no thread/channel synchronization needed (the writer thread
/// below is a thin loop around this). Best-effort: there is no caller to
/// propagate a failure to (fed by a channel, from a background thread with no
/// return path) — a write hiccup here must never crash the engine over a
/// captured-log line, mirroring this codebase's existing best-effort
/// discipline for background capture (e.g. `drain_usage_for`'s read-failure
/// skip).
fn append_attributed_line(path: &Path, line: &LogLine) {
    let current_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if should_rotate(current_len) {
        rotate_generations(path, LOG_ROTATE_GENERATIONS);
    }
    let Ok(json) = serde_json::to_string(line) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{json}");
    }
}

/// The rotated-generation sibling of `base` (story 4-2) — `base` with `.N`
/// appended to its raw [`std::ffi::OsStr`] (not a lossy `Display`
/// concatenation, so a non-UTF8 path stays exact). `n == 1` is the most-
/// recently-rotated generation; higher `n` is older. Mirrors
/// [`crate::domain::Registry::attributed_output_log_generation_path`]'s
/// naming exactly (`output.log.<n>`) so the read side and the write side
/// agree on every generation's path without either depending on the other.
fn generation_path(base: &Path, n: u8) -> PathBuf {
    let mut os = base.as_os_str().to_os_string();
    os.push(format!(".{n}"));
    PathBuf::from(os)
}

/// Rotate `base`'s generations (story 4-2, Task 2): discard the oldest
/// retained generation, shift every remaining generation up by one, then
/// rename `base` itself (the current generation) to `.1`. The caller opens a
/// FRESH `base` on its next append (this function never creates one) — an
/// open `File` handle held across a rename keeps writing to the RENAMED
/// inode, not the new path, which is exactly why [`append_attributed_line`]
/// reopens the path fresh on every call rather than holding a handle.
///
/// `generations` is [`LOG_ROTATE_GENERATIONS`] in production (current, `.1`,
/// and `.2`); a missing source file/generation is a silent no-op (`rename`
/// and `remove_file` errors are ignored) — rotation is best-effort
/// resilience, never a hard failure.
fn rotate_generations(base: &Path, generations: u8) {
    if generations <= 1 {
        // No history retained: defensive-only in production (the constant is
        // fixed at 3), but stay correct — just drop the current generation.
        let _ = std::fs::remove_file(base);
        return;
    }
    let oldest = generations - 1;
    let _ = std::fs::remove_file(generation_path(base, oldest));
    for gen in (1..oldest).rev() {
        let _ = std::fs::rename(generation_path(base, gen), generation_path(base, gen + 1));
    }
    let _ = std::fs::rename(base, generation_path(base, 1));
}

/// Spawn the SINGLE background writer thread for one instance's attributed
/// capture (story 4-2, Task 2), returning the `Sender` every capture source
/// (both reader threads, and the supervisor's own transition-time sends,
/// Task 4) clones. `pub(crate)` and separate from [`spawn_output_capture`] so
/// tests can drive + `join` it directly (send lines, drop every `Sender`
/// clone, join, THEN assert on disk — a deterministic synchronization, no
/// wall-clock polling) without needing a real spawned process.
pub(crate) fn spawn_log_writer(
    attributed_log_path: PathBuf,
) -> (mpsc::Sender<LogLine>, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<LogLine>();
    let handle = thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            append_attributed_line(&attributed_log_path, &line);
        }
    });
    (tx, handle)
}

/// Strip a trailing `\n` (and a preceding `\r`, if present) from a raw line
/// buffer, lossily decoding the remainder as UTF-8 (never a panic on
/// non-UTF8 agent output — the SAME defensive stance this codebase takes
/// everywhere text crosses a process boundary). Used only for the
/// ATTRIBUTED [`LogLine::text`] — the legacy `agent.log` write uses the raw
/// bytes verbatim, delimiter included, completely independently (CRITICAL
/// SCOPING #3).
fn strip_trailing_newline(buf: &[u8]) -> String {
    let mut bytes = buf;
    if bytes.last() == Some(&b'\n') {
        bytes = &bytes[..bytes.len() - 1];
        if bytes.last() == Some(&b'\r') {
            bytes = &bytes[..bytes.len() - 1];
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Spawn ONE reader thread that drains `pipe` (the child's stdout or stderr)
/// line-by-line via [`BufRead::read_until`] (story 4-2, Task 2) — NOT
/// `BufRead::lines()`, deliberately: `read_until(b'\n', ..)` returns the raw
/// bytes INCLUDING the delimiter when found (or the exact trailing bytes with
/// NO synthesized delimiter at EOF), which is what makes the legacy-log write
/// below byte-identical to today's kernel passthrough even for a final line
/// with no trailing newline (`lines()` strips the delimiter from every
/// yielded item, which would silently ADD a `\n` back on re-write that the
/// original raw stream never had).
///
/// For each line read, this thread does BOTH, independently:
/// 1. Appends the RAW bytes VERBATIM to `legacy_log_path` (CRITICAL SCOPING
///    #3 — same bytes, same file, same format as today; only the writer
///    changed from "the OS kernel" to this thread). Opening the file in
///    APPEND mode on every write mirrors the existing dual
///    `Stdio::from(file)`/`try_clone()` atomicity this replaces: an
///    append-mode write is atomically placed at the file's current end by
///    the OS, so two independently-opened handles (this reader + its stderr
///    sibling) can safely interleave without corrupting either line.
/// 2. Sends an attributed, timestamped, newline-stripped [`LogLine`]
///    (`stream`) to the writer thread via `sender`.
///
/// Exits naturally on EOF (the pipe closes when the child exits and its fd
/// closes) or a read error. Unlike the stdin *write* direction, a *read*
/// cannot hang indefinitely on backpressure — it only ever yields data, EOF,
/// or an error — so, deliberately, NO timeout is used here; a reviewer should
/// not expect a mechanism symmetrical to [`write_stdin_bounded`].
fn spawn_reader_thread<R>(
    pipe: R,
    stream: LogStream,
    legacy_log_path: PathBuf,
    instance: String,
    sender: mpsc::Sender<LogLine>,
) where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        loop {
            let mut buf: Vec<u8> = Vec::new();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // true EOF: nothing read.
                Ok(_) => {
                    // (1) Legacy capture: the raw bytes, unmodified.
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&legacy_log_path)
                    {
                        let _ = file.write_all(&buf);
                    }
                    // (2) Attributed capture: the same content, attributed +
                    // timestamped, sent to the writer thread. Best-effort —
                    // a closed receiver (the writer thread already gone)
                    // just means nobody is listening anymore; this thread
                    // must keep draining the pipe regardless (so the child
                    // never blocks on a full pipe buffer), not stop here.
                    let text = strip_trailing_newline(&buf);
                    let _ =
                        sender.send(LogLine::new(instance.clone(), stream, text, now_rfc3339()));
                }
                Err(_) => break,
            }
        }
    });
}

/// Wire the shared output-capture primitive into a freshly spawned process
/// (story 4-2, Task 2/3): starts the ONE writer thread plus TWO reader
/// threads (stdout, stderr — separately, so attribution can tell them apart;
/// never a single interleaved reader), and returns the writer's `Sender` so
/// the caller (a backend's `spawn()`) can store it on the process handle —
/// the supervisor later clones it to send `Engine`-attributed lines through
/// the SAME channel (Task 4).
///
/// Called identically from both backends (mirrors `write_stdin_bounded`'s
/// "ONE shared implementation called identically from both `backends/unix`
/// and `backends/windows`" precedent). An ADOPTED handle never calls this —
/// there is no OS-portable way to recover a pipe fd from a bare PID, so it
/// gets no capture threads at all (mirrors `stdin`'s `None`-on-adoption
/// precedent); this is not a functional gap for reading/following (AC-H),
/// since reading only ever needs the FILE these threads keep writing to,
/// never a live handle.
pub(crate) fn spawn_output_capture(
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    legacy_log_path: PathBuf,
    attributed_log_path: PathBuf,
    instance: String,
) -> mpsc::Sender<LogLine> {
    let (tx, _writer_handle) = spawn_log_writer(attributed_log_path);
    // The writer thread is intentionally never joined here: it runs for the
    // process's whole supervised lifetime and exits naturally once every
    // `Sender` clone (this one, plus the two reader threads' clones below,
    // plus whatever the supervisor holds/clones — Task 3/4) is dropped.
    spawn_reader_thread(
        stdout,
        LogStream::AgentOut,
        legacy_log_path.clone(),
        instance.clone(),
        tx.clone(),
    );
    spawn_reader_thread(
        stderr,
        LogStream::AgentErr,
        legacy_log_path,
        instance,
        tx.clone(),
    );
    tx
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

    /// Whether this handle holds a live stdin pipe it can write to right now
    /// (story 4.1, AC-D). `true` only for a FRESHLY SPAWNED handle whose
    /// `ChildStdin` was captured at spawn time AND whose declared
    /// `Capability::Interaction` level was `Guaranteed`/`BestEffort` on this
    /// OS (see [`SpawnSpec::pipe_stdin`], story 4.1 fix pass HIGH finding);
    /// `false` for an ADOPTED handle (no OS-portable way to recover a pipe fd
    /// from a bare `{pid, start-time}` fingerprint — the same "no
    /// undocumented API" convention already established for Windows pause),
    /// a handle that was never piped (interaction `Unsupported`), OR a handle
    /// whose write previously timed out (see
    /// [`ProcessBackend::stdin_timed_out`] to distinguish that case). A
    /// cheap, no-I/O accessor, mirroring [`ProcessBackend::pid`]'s style.
    /// Callers MUST check this before calling [`ProcessBackend::write_stdin`].
    fn has_stdin(&self, handle: &Self::Handle) -> bool;

    /// Whether a PRIOR [`ProcessBackend::write_stdin`] call on this handle
    /// exceeded the bounded timeout (story 4.1 fix pass, the CRITICAL
    /// finding, review of #79) — see
    /// [`StdinState::TimedOut`](crate::ports::StdinState::TimedOut). Distinct
    /// from [`ProcessBackend::has_stdin`] returning `false` for an ADOPTED or
    /// never-piped handle (which never had a pipe at all): this means "had a
    /// live pipe, attempted a write, and it never came back in time". A
    /// cheap, no-I/O accessor so a caller can short-circuit a doomed repeat
    /// write without attempting it. Once `true`, stays `true` for the
    /// remainder of this handle's life (a stop/start builds an entirely
    /// fresh handle).
    fn stdin_timed_out(&self, handle: &Self::Handle) -> bool;

    /// Write `data` to the process's stdin pipe and flush it — the v1
    /// interaction channel (spine AD-12) — bounded to
    /// [`STDIN_WRITE_TIMEOUT`](crate::ports::STDIN_WRITE_TIMEOUT) (story 4.1
    /// fix pass, the CRITICAL finding, review of #79: the ORIGINAL
    /// unbounded `write_all` could freeze the entire engine forever, since
    /// every instance shares one supervisor lock — see
    /// [`write_stdin_bounded`](crate::ports::write_stdin_bounded)'s docs for
    /// the full mechanism and its timeout/success/error outcomes). Callers
    /// MUST check [`ProcessBackend::has_stdin`] first: called with no live
    /// pipe, this is a defensive [`BackendError::Control`] naming the
    /// situation, not a normal path — and MUST check
    /// [`ProcessBackend::stdin_timed_out`] first too, to avoid attempting
    /// another doomed write on an already-broken channel (this method itself
    /// also defensively refuses in that case, as a safety net). A genuine
    /// OS-level write failure within the bound (e.g. the agent exited
    /// between the caller's state check and this write — `EPIPE`) maps to
    /// [`BackendError::Control`], exactly like every other backend op; a
    /// write that does not return within the bound maps to
    /// [`BackendError::StdinTimedOut`]. Sync (called via `spawn_blocking`,
    /// like every other method on this trait) — the bounded wait itself
    /// happens on the calling (blocking-pool) thread, mirroring how
    /// [`ProcessBackend::stop`]'s graceful-window wait already blocks its
    /// caller for a bounded duration.
    fn write_stdin(&self, handle: &mut Self::Handle, data: &[u8]) -> Result<(), BackendError>;

    /// A clone of this handle's output-capture writer-thread `Sender`, if it
    /// has one (story 4-2, AD-12) — `Some` for a FRESHLY SPAWNED handle
    /// (capture is unconditional and capability-independent, AC-E: every
    /// spawn calls [`spawn_output_capture`] whenever it has somewhere to
    /// write, independent of any declared `Capability`); `None` for an
    /// ADOPTED handle (no capture threads were ever started for it — see
    /// [`ProcessBackend::adopt`]'s docs). A cheap accessor (an `mpsc::Sender`
    /// clone is a refcount bump, no I/O) mirroring [`ProcessBackend::has_stdin`]'s
    /// style; used by the supervisor to send `Engine`-attributed lines
    /// (Task 4) through the SAME channel the reader threads feed.
    fn log_sender(&self, handle: &Self::Handle) -> Option<mpsc::Sender<LogLine>>;
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
            attributed_log_path: None,
            instance_name: "x".to_string(),
            pipe_stdin: true,
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn stdin_timed_out_error_message_names_the_bound() {
        let e = BackendError::StdinTimedOut { timeout_secs: 5 };
        let msg = e.to_string();
        assert!(msg.contains('5'), "{msg}");
        assert!(msg.contains("draining"), "{msg}");
    }

    #[test]
    fn stdin_state_is_live_and_is_timed_out_agree_with_the_variant() {
        // A pure-logic sanity check on the three-state predicate pair (no
        // process needed) — the process-backed proofs of the ACTUAL bounded
        // write mechanism (write_stdin_bounded's timeout/success/misuse
        // outcomes) live in backends/unix/mod.rs's test module, which has the
        // real ChildStdin-spawning infrastructure this cfg-free module does
        // not.
        assert!(!StdinState::NoPipe.is_live());
        assert!(!StdinState::NoPipe.is_timed_out());
        assert!(!StdinState::TimedOut.is_live());
        assert!(StdinState::TimedOut.is_timed_out());
    }

    #[test]
    fn write_stdin_bounded_on_a_non_live_state_is_a_defensive_control_error_and_leaves_state_unchanged(
    ) {
        // Misuse guard: calling write_stdin_bounded on NoPipe/TimedOut must
        // NEVER fabricate a timeout — it must restore the ORIGINAL state
        // unchanged and return a defensive Control error. This is pure logic
        // (no real ChildStdin needed since the Live branch is never reached).
        let mut state = StdinState::NoPipe;
        let err = write_stdin_bounded(&mut state, b"x", Duration::from_millis(50)).unwrap_err();
        assert!(matches!(err, BackendError::Control { op: "stdin", .. }));
        assert!(
            matches!(state, StdinState::NoPipe),
            "state must be unchanged"
        );

        let mut state = StdinState::TimedOut;
        let err = write_stdin_bounded(&mut state, b"x", Duration::from_millis(50)).unwrap_err();
        assert!(matches!(err, BackendError::Control { op: "stdin", .. }));
        assert!(
            matches!(state, StdinState::TimedOut),
            "an already-TimedOut state must stay TimedOut, not be reinterpreted"
        );
    }

    // ---- Story 4-2: rotation-decision logic + the reader/writer threads ----

    #[test]
    fn should_rotate_at_the_exact_boundary() {
        // The >= convention this codebase uses everywhere else (BudgetEvaluator).
        assert!(
            !should_rotate(LOG_ROTATE_MAX_BYTES - 1),
            "one under: no rotate"
        );
        assert!(should_rotate(LOG_ROTATE_MAX_BYTES), "exactly at: rotate");
        assert!(should_rotate(LOG_ROTATE_MAX_BYTES + 1), "one over: rotate");
    }

    #[test]
    fn strip_trailing_newline_handles_lf_crlf_and_no_newline() {
        assert_eq!(strip_trailing_newline(b"hello\n"), "hello");
        assert_eq!(strip_trailing_newline(b"hello\r\n"), "hello");
        assert_eq!(strip_trailing_newline(b"hello"), "hello");
        assert_eq!(strip_trailing_newline(b""), "");
        // Lossy on non-UTF8 — never a panic.
        assert_eq!(strip_trailing_newline(&[0xff, 0xfe]), "\u{fffd}\u{fffd}");
    }

    #[test]
    fn generation_path_appends_the_generation_suffix() {
        let base = PathBuf::from("/tmp/agents/svc/logs/output.log");
        assert_eq!(
            generation_path(&base, 1),
            PathBuf::from("/tmp/agents/svc/logs/output.log.1")
        );
        assert_eq!(
            generation_path(&base, 2),
            PathBuf::from("/tmp/agents/svc/logs/output.log.2")
        );
    }

    fn line(text: &str) -> LogLine {
        LogLine::new("svc", LogStream::AgentOut, text, now_rfc3339())
    }

    #[test]
    fn append_attributed_line_rotates_at_the_byte_bound_keeping_exactly_3_generations() {
        // Task 2: a LogLine sequence through the writer-thread primitive
        // rotates at the REAL LOG_ROTATE_MAX_BYTES bound and produces exactly
        // 3 generations with the oldest discarded — driven directly (no real
        // process, no wall-clock dependency), per the Testing Notes'
        // guidance. should_rotate is checked on size BEFORE each append, so
        // writing three ~4MB lines crosses the 10MB bound on the THIRD
        // append's pre-check (~8MB < 10MB, so it lands, leaving the
        // generation ~12MB) — the FOURTH append then rotates. Six lines total
        // exercises exactly one rotation.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        let big_text = "x".repeat(4 * 1024 * 1024); // 4MB text per line
        for i in 0..6 {
            append_attributed_line(&base, &line(&format!("{big_text}-{i}")));
        }
        assert!(base.exists(), "current generation must exist");
        assert!(
            generation_path(&base, 1).exists(),
            "at least one rotation must have produced a .1 generation"
        );
        // Never more than LOG_ROTATE_GENERATIONS - 1 rotated predecessors.
        assert!(
            !generation_path(&base, LOG_ROTATE_GENERATIONS).exists(),
            "no generation beyond LOG_ROTATE_GENERATIONS - 1 may survive"
        );
    }

    #[test]
    fn rotate_generations_discards_the_oldest_and_shifts_the_rest() {
        // A focused, deterministic proof of the rotation SEQUENCE itself
        // (independent of should_rotate's byte-bound timing): current -> .1,
        // .1 -> .2, prior .2 discarded.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        std::fs::write(&base, "current").unwrap();
        std::fs::write(generation_path(&base, 1), "gen1").unwrap();
        std::fs::write(generation_path(&base, 2), "gen2-oldest").unwrap();

        rotate_generations(&base, LOG_ROTATE_GENERATIONS);

        assert!(!base.exists(), "current was renamed away");
        assert_eq!(
            std::fs::read_to_string(generation_path(&base, 1)).unwrap(),
            "current"
        );
        assert_eq!(
            std::fs::read_to_string(generation_path(&base, 2)).unwrap(),
            "gen1"
        );
        // The prior .2 ("gen2-oldest") is gone — discarded, not shifted to a
        // nonexistent .3.
        assert!(!generation_path(&base, 3).exists());
    }

    #[test]
    fn rotate_generations_on_missing_predecessors_is_a_harmless_no_op() {
        // The FIRST-ever rotation: no `.1`/`.2` exist yet. Must not error or
        // panic on the missing rename/remove sources.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        std::fs::write(&base, "current").unwrap();
        rotate_generations(&base, LOG_ROTATE_GENERATIONS);
        assert!(!base.exists());
        assert_eq!(
            std::fs::read_to_string(generation_path(&base, 1)).unwrap(),
            "current"
        );
    }

    #[test]
    fn rotate_generations_with_a_degenerate_generation_count_just_drops_current() {
        // Defensive-only in production (LOG_ROTATE_GENERATIONS is a fixed 3),
        // but `rotate_generations` is a plain, directly-callable pure-ish
        // function, so its `generations <= 1` branch is exercised directly
        // here rather than left as dead code.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        std::fs::write(&base, "current").unwrap();
        rotate_generations(&base, 1);
        assert!(!base.exists(), "the only generation is simply dropped");
        assert!(!generation_path(&base, 1).exists());

        let base2 = dir.path().join("output2.log");
        std::fs::write(&base2, "current2").unwrap();
        rotate_generations(&base2, 0);
        assert!(!base2.exists());
    }

    #[test]
    fn writer_thread_preserves_send_order_for_a_burst_of_same_second_lines() {
        // AC-G: multiple lines sent within one wall-clock second (the SAME
        // `at` value, since now_rfc3339 has whole-second resolution) must
        // land on disk in SEND order — never re-sorted. Deterministic: no
        // wall-clock dependency, driven entirely through the channel, then
        // synchronized via `join` (not a sleep-poll).
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        let (tx, handle) = spawn_log_writer(base.clone());
        let same_at = "2026-07-15T00:00:00Z";
        for i in 0..20 {
            tx.send(LogLine::new(
                "svc",
                LogStream::AgentOut,
                format!("line-{i}"),
                same_at,
            ))
            .unwrap();
        }
        drop(tx); // close the channel so the writer thread's recv() loop ends
        handle.join().expect("writer thread must not panic");

        let contents = std::fs::read_to_string(&base).unwrap();
        let texts: Vec<String> = contents
            .lines()
            .map(|l| {
                let parsed: LogLine = serde_json::from_str(l).unwrap();
                assert_eq!(parsed.at, same_at, "every line shares the same second");
                parsed.text
            })
            .collect();
        let want: Vec<String> = (0..20).map(|i| format!("line-{i}")).collect();
        assert_eq!(texts, want, "append order must equal send order");
    }

    #[test]
    fn spawn_log_writer_exits_naturally_once_every_sender_clone_is_dropped() {
        // No explicit shutdown signal is needed: the writer thread's recv()
        // loop ends (and the thread returns) once the channel closes.
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("output.log");
        let (tx, handle) = spawn_log_writer(base);
        let tx2 = tx.clone();
        drop(tx);
        drop(tx2);
        // The thread must terminate promptly; join() blocks until it does.
        handle.join().expect("writer thread must not panic");
    }
}
