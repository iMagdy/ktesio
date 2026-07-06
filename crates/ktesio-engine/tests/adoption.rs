//! Integration tests for story-1.6 orphan adoption + the engine-kill guarantee
//! (AC-B / AC7 / AC8, NFR-1), plus the folded-in AI-7 (resume-from-paused
//! survives restart) and AI-8 (honest adoption) action items, driven through the
//! PUBLIC async [`Engine`] (spine AD-2/AD-13) with the REAL `fake_agent`.
//!
//! ## Faithfully simulating an ENGINE CRASH
//!
//! The load-bearing NFR-1 proof requires an agent process that OUTLIVES a crashed
//! engine. A `kill -9` of the engine runs no destructors, so its supervised
//! handles never fire their kill-on-drop and the agent (spawned into its own
//! session via the backend's `setsid`) keeps running. We model this precisely by
//! doing the "engine 1" work in a SEPARATE child process (a re-exec of this test
//! binary via the `adoption_helper_subprocess` entry) that starts the agent and
//! then `std::process::exit`s WITHOUT dropping the engine — so (a) no handle Drop
//! runs (the agent survives) and (b) the agent RE-PARENTS to init, which reaps it
//! when it eventually dies (no lingering zombie in the test process, so liveness
//! probes are accurate). The parent test then opens a NEW engine over the SAME
//! state dir and asserts adoption / honest reconcile.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ktesio_engine::{AdapterRef, Engine, LifecycleState, RestartPolicy};
use tempfile::TempDir;

/// Write a manifest whose `[lifecycle.start]` exec is `fake_agent` + `args`.
fn write_fake_manifest(dir: &Path, kind: &str, args: &[&str]) {
    let bin = ktesio_conformance::fake_agent_bin();
    let args_toml = args
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "{kind}"

[lifecycle.start]
exec = {exec:?}
args = [{args_toml}]

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
    );
    std::fs::write(dir.join("adapter.toml"), body).unwrap();
}

/// Whether a pid is alive. NO OS-cfg here (the gate allowlists only `backends/`);
/// branch on the runtime OS id and shell out (`kill -0` / `tasklist`).
fn pid_alive(pid: u32) -> bool {
    match ktesio_engine::OsId::current() {
        ktesio_engine::OsId::Windows => {
            let out = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output();
            match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                Err(_) => false,
            }
        }
        _ => Command::new("kill")
            .args(["-0", &pid.to_string()])
            // Silence the "No such process" stderr once the pid is gone — the
            // exit status is what we read.
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
    }
}

fn wait_until_gone(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid} still alive)");
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Kill a process (and its session group) out-of-band, to fabricate a
/// "process gone" orphan. The agent is a session leader (setsid → pgid == pid),
/// and after the engine-1 subprocess exits it has re-parented to init, so a kill
/// → init reaps it (no lingering zombie). NO OS-cfg here (the gate allowlists
/// only `backends/`); branch on the runtime OS id and shell out.
fn kill_pid(pid: u32) {
    match ktesio_engine::OsId::current() {
        ktesio_engine::OsId::Windows => {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .status();
        }
        _ => {
            // Kill the whole process group, then the bare pid as a fallback.
            for target in [format!("-{pid}"), pid.to_string()] {
                let _ = Command::new("kill")
                    .args(["-KILL", &target])
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
}

/// The agent.log path inside an instance's Agent Home.
fn agent_log_path(base: &Path, name: &str) -> PathBuf {
    base.join("agents")
        .join(name)
        .join("logs")
        .join("agent.log")
}

/// Read the pid the fake_agent announced (`ready pid=<n>`) from its agent.log.
fn wait_for_agent_pid(agent_log: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(agent_log) {
            if let Some(line) = contents.lines().find(|l| l.contains("ready pid=")) {
                if let Some(idx) = line.find("pid=") {
                    if let Ok(pid) = line[idx + 4..].trim().parse::<u32>() {
                        return pid;
                    }
                }
            }
        }
        assert!(Instant::now() < deadline, "agent pid never announced");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run "engine 1" in a SEPARATE child process (a re-exec of this test binary via
/// the `adoption_helper_subprocess` entry) with the given mode + state/manifest
/// dirs. The child starts the agent(s) and `exit`s WITHOUT dropping the engine
/// (crash semantics). Blocks until the child exits, then returns.
fn run_engine1(mode: &str, state: &Path, manifest: &Path) {
    let exe = std::env::current_exe().expect("test exe");
    let status = Command::new(exe)
        .args(["--exact", "adoption_helper_subprocess", "--nocapture"])
        .env("KTESIO_ADOPTION_HELPER", mode)
        .env("KTESIO_ADOPTION_STATE", state)
        .env("KTESIO_ADOPTION_MANIFEST", manifest)
        .status()
        .expect("run engine-1 helper subprocess");
    assert!(
        status.success(),
        "engine-1 helper subprocess failed: {status}"
    );
}

/// The re-exec entry for the "engine 1" work (see module docs). When
/// `KTESIO_ADOPTION_HELPER` is unset this is a trivial pass (it runs as a normal
/// test in the parent binary too). When set, it performs the mode's engine-1 work
/// and `std::process::exit`s WITHOUT dropping the engine — modelling a crash.
#[test]
fn adoption_helper_subprocess() {
    let Ok(mode) = std::env::var("KTESIO_ADOPTION_HELPER") else {
        return; // normal in-process invocation: nothing to do.
    };
    let state = PathBuf::from(std::env::var("KTESIO_ADOPTION_STATE").unwrap());
    let manifest = PathBuf::from(std::env::var("KTESIO_ADOPTION_MANIFEST").unwrap());

    let engine = Engine::open(Some(state.clone())).expect("engine1 open");
    let facade = engine.blocking();

    match mode.as_str() {
        // Start `survivor` (stays alive) + `ghost` (we make it exit). The engine
        // then "crashes" (exit without drop): survivor's process survives &
        // re-parents to init; ghost's process is made to exit so the new engine
        // sees a `running` row whose process is GONE.
        "survivor_and_ghost" => {
            facade
                .register_with_adapter("survivor", &AdapterRef::Manifest(manifest.clone()))
                .unwrap();
            facade.start("survivor").unwrap();
            // `ghost`: an instance whose process exits shortly, leaving a stale
            // `running` row. It lingers LONG (like the survivor) so it reliably
            // survives start's readiness window + the crash; the PARENT test then
            // KILLS it (after it re-parents to init, so init reaps it — no
            // zombie), fabricating the "process gone, record present" orphan.
            let ghost_manifest = manifest.join("ghost");
            std::fs::create_dir_all(&ghost_manifest).unwrap();
            write_fake_manifest(&ghost_manifest, "ghostkind", &["--linger-ms", "600000"]);
            facade
                .register_with_adapter("ghost", &AdapterRef::Manifest(ghost_manifest))
                .unwrap();
            facade.start("ghost").unwrap();
            // Crash: exit without dropping the engine. Both survive; the parent
            // test kills `ghost` explicitly to make it a gone-process orphan.
            std::process::exit(0);
        }
        // Start `phantom` (long linger, reliably alive); after the crash the
        // PARENT test kills it, so the new engine finds a `running` row whose
        // process is gone (AI-8).
        "phantom" => {
            facade
                .register_with_adapter("phantom", &AdapterRef::Manifest(manifest.clone()))
                .unwrap();
            facade.start("phantom").unwrap();
            std::process::exit(0);
        }
        // Start `nap`, pause it, then crash (exit without drop). The paused
        // process survives (on Unix it is SIGSTOP'd — still a live, stopped
        // process; the new engine must adopt it and a resume must wake it).
        "paused_survivor" => {
            facade
                .register_with_adapter("nap", &AdapterRef::Manifest(manifest.clone()))
                .unwrap();
            facade.start("nap").unwrap();
            facade.pause("nap").unwrap();
            std::process::exit(0);
        }
        // Start `clean`, then STOP it cleanly (clears the record), then exit
        // normally — a later open must NOT resurrect it.
        "clean_stop" => {
            facade
                .register_with_adapter("clean", &AdapterRef::Manifest(manifest.clone()))
                .unwrap();
            facade.start("clean").unwrap();
            facade.stop("clean", Some(Duration::from_secs(5))).unwrap();
            std::process::exit(0);
        }
        other => panic!("unknown adoption helper mode: {other}"),
    }
}

#[test]
fn engine_kill_adopts_live_child_and_fails_gone_record() {
    // THE NFR-1 proof (AC8 / AC-B). Engine 1 (a subprocess) starts `survivor`
    // (survives) + `ghost` (self-exits at crash), then crashes. Engine 2 (this
    // process) opens the SAME state dir and must: ADOPT `survivor` (row `running`,
    // and a subsequent `stop` truly kills it — no orphan), and reconcile `ghost`
    // to `failed` (AI-8: no phantom `running`).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    // Engine 1 in a subprocess (crash semantics): start survivor + ghost, exit.
    run_engine1("survivor_and_ghost", state.path(), manifest.path());

    // The survivor process is alive (re-parented to init); note its pid.
    let survivor_pid = wait_for_agent_pid(&agent_log_path(state.path(), "survivor"));
    assert!(
        pid_alive(survivor_pid),
        "survivor must survive the engine crash"
    );
    // Make `ghost`'s process GONE (as if it had died around the crash), while
    // `survivor` keeps running. It re-parented to init after the subprocess
    // exited, so the kill is reaped by init (no zombie).
    let ghost_pid = wait_for_agent_pid(&agent_log_path(state.path(), "ghost"));
    kill_pid(ghost_pid);
    wait_until_gone(ghost_pid, "ghost should be killable after the crash");

    // Engine 2: open over the SAME state dir → adopt_orphans runs.
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();

    // `survivor` is ADOPTED: row still `running`, process alive.
    let survivor = facade.instance_status("survivor").unwrap();
    assert_eq!(
        survivor.instance.state,
        LifecycleState::Running,
        "a live orphan must be adopted as running"
    );
    assert!(pid_alive(survivor_pid), "adopted process alive after open");

    // `ghost` reconciled to `failed` (AI-8), not left a phantom `running`.
    let ghost = facade.instance_status("ghost").unwrap();
    assert_eq!(
        ghost.instance.state,
        LifecycleState::Failed,
        "a gone-process record must reconcile to failed"
    );

    // A subsequent `stop` on the ADOPTED instance TRULY terminates its process —
    // no orphan remains (the NFR-1 guarantee across an engine restart).
    let stopped = facade
        .stop("survivor", Some(Duration::from_secs(5)))
        .unwrap();
    assert_eq!(stopped.state, LifecycleState::Stopped);
    wait_until_gone(
        survivor_pid,
        "stop on the adopted instance must terminate its process (no orphan left)",
    );
}

#[test]
fn ai8_phantom_running_row_with_dead_process_reconciles_to_failed() {
    // AI-8 (honest adoption): a persisted `running` row whose process is gone at
    // open reconciles to `failed`, NOT a phantom `running`.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    // Long linger so the process reliably survives start's readiness window + the
    // crash; the parent kills it (init reaps it — no zombie) to make it gone.
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    run_engine1("phantom", state.path(), manifest.path());
    let pid = wait_for_agent_pid(&agent_log_path(state.path(), "phantom"));
    kill_pid(pid);
    wait_until_gone(pid, "phantom should be killable after the crash");

    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let status = engine.blocking().instance_status("phantom").unwrap();
    assert_eq!(
        status.instance.state,
        LifecycleState::Failed,
        "a phantom running row must reconcile to failed"
    );
    let events = engine.blocking().transition_events("phantom").unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.new_state, LifecycleState::Failed);
    let cause = serde_json::to_string(&last.cause).unwrap();
    assert!(cause.contains("crashed"), "cause={cause}");
}

#[test]
fn ai7_paused_live_process_is_adopted_and_resumable() {
    // AI-7 (resume-from-paused survives restart): a `paused` row whose process is
    // LIVE is adopted (handle re-held) so a later `resume` works. Runs on Unix
    // (guaranteed pause → SIGSTOP); skipped at RUNTIME on Windows where pause is
    // best-effort and a subprocess-suspended process model differs (the adoption
    // path itself is identical and covered by the survivor test).
    if ktesio_engine::OsId::current() == ktesio_engine::OsId::Windows {
        return;
    }
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    run_engine1("paused_survivor", state.path(), manifest.path());
    let pid = wait_for_agent_pid(&agent_log_path(state.path(), "nap"));
    // The process was SIGSTOP'd by the pause; it is still ALIVE (stopped). The new
    // engine must adopt it while the row stays `paused`.
    assert!(pid_alive(pid), "paused process must survive the crash");

    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    let status = facade.instance_status("nap").unwrap();
    assert_eq!(
        status.instance.state,
        LifecycleState::Paused,
        "a live paused process must be adopted while staying paused (AI-7)"
    );
    // A subsequent `resume` works (the handle was re-held) → running.
    let resumed = facade.resume("nap").unwrap();
    assert_eq!(resumed.state, LifecycleState::Running);
    assert!(pid_alive(pid));
    // Teardown: stop, confirm no orphan.
    facade.stop("nap", Some(Duration::from_secs(5))).unwrap();
    wait_until_gone(pid, "stop must terminate the resumed process");
}

#[test]
fn a_cleanly_stopped_instance_is_not_resurrected_on_reopen() {
    // The clear-on-clean-stop path: a cleanly-stopped instance cleared its
    // write-ahead record, so a later engine open does NOT adopt or fail it — it
    // stays `stopped` (no false orphan).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    run_engine1("clean_stop", state.path(), manifest.path());

    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let status = engine.blocking().instance_status("clean").unwrap();
    assert_eq!(
        status.instance.state,
        LifecycleState::Stopped,
        "a cleanly-stopped instance must stay stopped after a later open"
    );
}

#[test]
fn launch_failed_instance_surfaces_its_cause_via_instance_status() {
    // F-Med-3 (AC9): a launch-error `failed` instance has NO write-ahead spawn
    // record (the `starting → failed` launch error returns before the record is
    // written), yet `instance_status` must still surface the failed cause — by
    // falling back to the last transition-event-log cause (the preserved launch
    // diagnostic). Point a manifest at a non-existent exec, start it, and assert
    // the cause is surfaced.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    let body = r#"
contract_version = "0.1.0"

[adapter]
kind = "bad"

[lifecycle.start]
exec = "ktesio-no-such-binary-med3"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#;
    std::fs::write(manifest.path().join("adapter.toml"), body).unwrap();

    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade
        .register_with_adapter("bad", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // Start lands `failed` (launch error), returning an error, and writes NO
    // spawn record.
    let err = facade.start("bad").unwrap_err();
    assert!(err.to_string().contains("failed to launch"), "{err}");

    let status = facade.instance_status("bad").unwrap();
    assert_eq!(status.instance.state, LifecycleState::Failed);
    // The failed cause is surfaced from the event-log fallback (names the exec).
    let cause = status
        .failed_cause
        .expect("a launch-failed instance must surface a failed cause (AC9)");
    assert!(
        cause.contains("ktesio-no-such-binary-med3"),
        "failed cause should name the launch diagnostic; got: {cause}"
    );
}

#[test]
fn instance_status_and_set_policy_reject_an_invalid_name() {
    // The status read + policy set validate the name shape and reject a malformed
    // one with InvalidName before any lookup (the error paths). A missing but
    // well-formed name is NotFound for the status read.
    let state = TempDir::new().unwrap();
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    let status_err = facade.instance_status("Bad Name").unwrap_err();
    assert!(status_err.to_string().contains("invalid"), "{status_err}");
    let policy_err = facade
        .set_restart_policy("Bad Name", RestartPolicy::Never)
        .unwrap_err();
    assert!(policy_err.to_string().contains("invalid"), "{policy_err}");
    let missing = facade.instance_status("ghost").unwrap_err();
    assert!(missing.to_string().contains("ghost"), "{missing}");
}

#[test]
fn per_instance_restart_policy_defaults_to_on_failure_and_is_configurable() {
    // AC4: the effective per-instance policy defaults to `on-failure` (AD-15
    // default) and is per-instance configurable via the seed, surviving a reopen.
    let state = TempDir::new().unwrap();
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade.register("cfg", "mock").unwrap();
    assert_eq!(
        facade.instance_status("cfg").unwrap().restart_policy,
        RestartPolicy::OnFailure,
        "default policy is on-failure"
    );
    facade
        .set_restart_policy("cfg", RestartPolicy::Never)
        .unwrap();
    assert_eq!(
        facade.instance_status("cfg").unwrap().restart_policy,
        RestartPolicy::Never
    );
    drop(engine);
    // The policy seed survives a reopen; the instance stays `registered`.
    let engine2 = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let status = engine2.blocking().instance_status("cfg").unwrap();
    assert_eq!(
        status.restart_policy,
        RestartPolicy::Never,
        "seed survives reopen"
    );
    assert_eq!(status.instance.state, LifecycleState::Registered);
}
