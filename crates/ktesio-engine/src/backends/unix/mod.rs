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

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::{setsid, Pid};

use crate::ports::{BackendError, ProcessBackend, ProcessStatus, SpawnSpec, StopOutcome};

/// How often the graceful-stop wait polls for the process to exit.
///
/// A short poll interval keeps a fast-exiting process from waiting the whole
/// window while bounding the busy-work. The wait runs on tokio's blocking pool
/// (the engine calls `stop` via `spawn_blocking`), so a bounded `sleep` here
/// does not stall an async worker.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A running process on Unix: the owned child + its process-group id.
///
/// The group id equals the child pid (the child is its own group leader via
/// `setsid`). Holding the [`Child`] lets us reap it (no zombie); the group id
/// lets us signal the whole tree.
#[derive(Debug)]
pub struct UnixProcess {
    /// The owned child handle (reaped on stop / drop).
    child: Child,
    /// The process-group id to signal (== child pid).
    pgid: Pid,
    /// The child pid, cached for diagnostics and the 1-6 adoption fingerprint.
    pid: u32,
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
        Ok(UnixProcess { child, pgid, pid })
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
        // Block until the child is reaped (it was just SIGKILLed, so this is
        // bounded). `wait` reaps the direct child; group members are killed by
        // the SIGKILL above and reaped by init.
        handle.child.wait().map_err(|e| BackendError::Control {
            op: "wait",
            detail: e.to_string(),
        })?;
        Ok(StopOutcome { forced: true })
    }

    fn poll(&self, handle: &mut Self::Handle) -> Result<ProcessStatus, BackendError> {
        handle.reap_if_exited()
    }

    fn pid(&self, handle: &Self::Handle) -> u32 {
        handle.pid
    }
}

impl Drop for UnixProcess {
    /// Kill the process group on drop so a dropped handle never leaks the agent
    /// (or any child it spawned). This is what keeps `kt agent start` — which
    /// spawns, records `running`, then EXITS — from orphaning the process: when
    /// the supervisor is torn down at engine shutdown, the handle drops and the
    /// whole group dies. Best-effort; a group already gone is fine.
    ///
    /// NOTE (single-lifetime boundary, AD-5 is story 1-6): because the handle
    /// lives only for one engine lifetime, a process started by one engine is
    /// cleaned up when THAT engine ends. Carrying a running agent across engine
    /// restarts (adopting an orphan by pid + fingerprint) is story 1-6.
    fn drop(&mut self) {
        // If already reaped/exited, nothing to do; otherwise SIGKILL the group
        // and reap the direct child so no zombie remains.
        if let Ok(ProcessStatus::Alive) = self.reap_if_exited() {
            let _ = killpg(self.pgid, Signal::SIGKILL);
            let _ = self.child.wait();
        }
    }
}

impl UnixProcess {
    /// Non-blocking: reap the child if it has exited, returning its status.
    fn reap_if_exited(&mut self) -> Result<ProcessStatus, BackendError> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(ProcessStatus::Exited {
                code: status.code(),
            }),
            Ok(None) => Ok(ProcessStatus::Alive),
            Err(e) => Err(BackendError::Control {
                op: "wait",
                detail: e.to_string(),
            }),
        }
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
