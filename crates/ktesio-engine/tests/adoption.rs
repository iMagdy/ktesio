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

use ktesio_engine::{
    AdapterRef, Engine, FleetEntry, LifecycleState, RemoveDisposition, RestartPolicy,
};
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
contract_version = "1.0.0"

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
///
/// A ZOMBIE (defunct) child still answers `kill -0` — it holds its pid until
/// reaped — so without a reaping PID1 (e.g. a bare CI container) a just-killed
/// process reads as alive and the "process gone" assertions false-fail. After
/// `kill -0` succeeds, discount a process whose `/proc/<pid>/stat` state is `Z`.
/// Reading /proc needs no OS-cfg: the path is absent off Linux, so the check
/// no-ops there (those callers run under a reaping init/launchd), preserving the
/// plain `kill -0` semantics.
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
        _ => {
            let exists = Command::new("kill")
                .args(["-0", &pid.to_string()])
                // Silence the "No such process" stderr once the pid is gone — the
                // exit status is what we read.
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            exists && !proc_pid_is_zombie(pid)
        }
    }
}

/// Whether `/proc/<pid>/stat` reports process state `Z` (zombie). No OS-cfg (the
/// gate allowlists only `backends/`): the read simply fails on non-Linux, so
/// this returns `false` there and [`pid_alive`] keeps its `kill -0` semantics.
fn proc_pid_is_zombie(pid: u32) -> bool {
    // /proc/<pid>/stat is "pid (comm) state ...". `comm` may contain spaces or
    // ')', so the state code is the first token AFTER the final ')'.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.split_whitespace().next() == Some("Z"))
        })
        .unwrap_or(false)
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

/// Linux AND running under CI (GitHub sets `CI`). Used to skip the heavy
/// process-spawning adoption tests that deadlock on the x86 ubuntu runner (#109),
/// while still running them locally (Linux dev) and on macOS/Windows/arm64.
fn is_linux_ci() -> bool {
    ktesio_engine::OsId::current() == ktesio_engine::OsId::Linux && std::env::var_os("CI").is_some()
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
        // WHOLE-FLEET REBOOT setup (story 1-7, AC-B). Register several instances
        // in DIFFERENT states, then crash (exit without drop):
        //   * `keeper`  — registered, never started (no process, no record).
        //   * `napper`  — registered, policy set to `never` (proves policy +
        //                 count survive the reopen unchanged).
        //   * `worker`  — started + LEFT RUNNING (survives the crash; the PARENT
        //                 test then kills it to fabricate "every process gone").
        //   * `finished`— started then cleanly STOPPED (record cleared; must stay
        //                 `stopped`, not be resurrected as an orphan).
        "whole_fleet_reboot" => {
            facade.register("keeper", "mock").unwrap();
            facade.register("napper", "mock").unwrap();
            facade
                .set_restart_policy("napper", RestartPolicy::Never)
                .unwrap();

            let worker_manifest = manifest.join("worker");
            std::fs::create_dir_all(&worker_manifest).unwrap();
            write_fake_manifest(&worker_manifest, "workerkind", &["--linger-ms", "600000"]);
            facade
                .register_with_adapter("worker", &AdapterRef::Manifest(worker_manifest))
                .unwrap();
            facade.start("worker").unwrap();

            let finished_manifest = manifest.join("finished");
            std::fs::create_dir_all(&finished_manifest).unwrap();
            write_fake_manifest(&finished_manifest, "finkind", &["--linger-ms", "600000"]);
            facade
                .register_with_adapter("finished", &AdapterRef::Manifest(finished_manifest))
                .unwrap();
            facade.start("finished").unwrap();
            facade
                .stop("finished", Some(Duration::from_secs(5)))
                .unwrap();

            // Crash: exit WITHOUT dropping the engine. `worker` survives (the
            // parent kills it to model the reboot); `finished` already stopped.
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
    //
    // Runtime-skip on Windows: this asserts a child SURVIVES its parent engine's
    // death, which needs Unix re-parenting to init; on Windows KILL_ON_JOB_CLOSE
    // kills the child when the `run_engine1` helper exits. The gone-process →
    // reconcile-to-`failed` adoption path IS covered on Windows by the other
    // adoption tests. NO `#[cfg]` (this file is outside the backends allowlist).
    if ktesio_engine::OsId::current() == ktesio_engine::OsId::Windows {
        return;
    }
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
        return;
    }
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
fn whole_fleet_survives_a_reboot_and_reconciles_running_to_failed() {
    // THE reboot-durability proof (story 1-7, AC-B / AC7 / AC8). A machine reboot
    // is the degenerate "all processes gone" case of engine-crash recovery: every
    // agent PID AND the engine are gone, but the on-disk SQLite state + Agent
    // Homes survive. We SIMULATE it (a true reboot is CI-infeasible) with the 1-6
    // harness: an engine-1 subprocess registers several instances in different
    // states + starts `worker` (survives the crash), then the parent KILLS every
    // live agent process (fabricating "all processes gone" — their PIDs would not
    // survive a reboot) and opens a NEW engine over the SAME state dir. The test
    // asserts the reboot INVARIANTS, not a literal reboot.
    //
    // Runtime-skip on Windows: the harness relies on `worker` SURVIVING the
    // engine-1 crash (Unix re-parenting to init) before we fabricate the reboot;
    // on Windows KILL_ON_JOB_CLOSE kills it when the `run_engine1` helper exits.
    // The reboot INVARIANTS (running → reconcile-to-`failed` for gone processes)
    // are covered on Windows by the other adoption tests. NO `#[cfg]` (this file
    // is outside the backends allowlist).
    if ktesio_engine::OsId::current() == ktesio_engine::OsId::Windows {
        return;
    }
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
        return;
    }
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    // Engine 1 (crash semantics): register keeper/napper/worker/finished, start
    // worker (left running) + finished (cleanly stopped), then exit without drop.
    run_engine1("whole_fleet_reboot", state.path(), manifest.path());

    // `worker` survived the crash (re-parented to init); note its pid, then KILL
    // it to fabricate the reboot condition (every process gone). init reaps it.
    let worker_pid = wait_for_agent_pid(&agent_log_path(state.path(), "worker"));
    assert!(
        pid_alive(worker_pid),
        "worker must survive the engine crash"
    );
    kill_pid(worker_pid);
    wait_until_gone(worker_pid, "worker should be gone for the reboot condition");

    // "Reboot": open a NEW engine over the SAME state dir → adopt_orphans runs
    // with ZERO live matches (every process is gone).
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();

    // (a) EVERY registration is still present with name/kind/home intact — nothing
    // is lost across the reboot (durable state lives in SQLite, AD-6).
    let fleet = facade.fleet().unwrap();
    let mut names: Vec<&str> = fleet.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["finished", "keeper", "napper", "worker"],
        "every registration must survive the reboot"
    );
    for entry in &fleet {
        assert!(
            entry.agent_home.contains(entry.name.as_str()),
            "agent home must be intact for {}",
            entry.name.as_str()
        );
        assert!(!entry.kind.is_empty(), "kind must be intact");
    }

    // (b) the previously-RUNNING instance reconciles to `failed` (reboot =
    // orphan-not-found), NEVER left `running` and NEVER dropped (AI-8 honesty).
    let worker = facade.instance_status("worker").unwrap();
    assert_eq!(
        worker.instance.state,
        LifecycleState::Failed,
        "a previously-running instance must reconcile to failed after a reboot"
    );

    // (c) the cleanly-STOPPED instance stays `stopped` (its record was cleared on
    // clean stop; a reboot must not resurrect it as an orphan).
    let finished = facade.instance_status("finished").unwrap();
    assert_eq!(
        finished.instance.state,
        LifecycleState::Stopped,
        "a cleanly-stopped instance must stay stopped after a reboot"
    );

    // the never-started instance stays `registered` (no process, nothing to
    // reconcile).
    let keeper = facade.instance_status("keeper").unwrap();
    assert_eq!(keeper.instance.state, LifecycleState::Registered);

    // (d) the persisted Restart Policy + count are UNCHANGED across the reopen —
    // `napper`'s explicitly-set `never` survived byte-intact (AD-6).
    let napper = facade.instance_status("napper").unwrap();
    assert_eq!(
        napper.restart_policy,
        RestartPolicy::Never,
        "the per-instance restart policy must survive the reboot"
    );
    assert_eq!(napper.restart_count, 0, "restart count must survive intact");
    // And the default-policy instances still read the default (survived intact).
    assert_eq!(keeper.restart_policy, RestartPolicy::OnFailure);

    // (e) no orphan process remains (the killed worker stays gone).
    assert!(
        !pid_alive(worker_pid),
        "no orphan process may remain after the reboot"
    );
}

#[test]
fn ai8_phantom_running_row_with_dead_process_reconciles_to_failed() {
    // AI-8 (honest adoption): a persisted `running` row whose process is gone at
    // open reconciles to `failed`, NOT a phantom `running`.
    //
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
        return;
    }
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
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
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
    //
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
        return;
    }
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
contract_version = "1.0.0"

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
fn fleet_composes_status_and_carries_the_metering_surface() {
    // Story 1-7 (Task 1, AC4/AC5) + story 3-1 (AC-C/AC11): Engine::fleet() composes
    // list() + the per-instance runtime status into FleetEntry rows, ordered by
    // name. `budget` stays the honest `None` seed (budgets are story 3-2), while
    // `usage` is now REAL — an all-zero UsageView for a never-metered instance (a
    // truthful zero, never null/fabricated) — and the active Metering Source is
    // surfaced.
    let state = TempDir::new().unwrap();
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade.register("beta", "mock").unwrap();
    facade.register("alpha", "mock").unwrap();

    let fleet = facade.fleet().unwrap();
    // Ordered by name (alpha before beta), one entry per registration.
    let names: Vec<&str> = fleet.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta"]);
    for entry in &fleet {
        // budget: the honest seed (JSON null), never 0 and never fabricated.
        assert!(entry.budget.is_none(), "budget must be the null seed");
        // usage: real, all-zero token totals for a never-metered instance.
        assert_eq!(entry.usage.cumulative_input_tokens, 0);
        assert_eq!(entry.usage.cumulative_output_tokens, 0);
        assert_eq!(entry.usage.current_run_input_tokens, 0);
        // The mock declares self-reported metering (surfaced — AC-C).
        assert_eq!(entry.metering_source, "self-reported");
        // Runtime fields match the per-instance status the CLI already surfaces.
        let status = facade.instance_status(entry.name.as_str()).unwrap();
        assert_eq!(entry.state, status.instance.state);
        assert_eq!(entry.restart_count, status.restart_count);
        assert_eq!(entry.restart_policy, status.restart_policy);
        assert_eq!(entry.kind, status.instance.kind);
        assert_eq!(entry.agent_home, status.instance.agent_home);
    }
    // The human budget-seed token is the em dash (consistent list + show).
    assert_eq!(FleetEntry::METERING_SEED_CELL, "—");
}

#[test]
fn fleet_entry_surfaces_the_failed_cause_for_a_failed_instance() {
    // Story 1-7 (Task 1): a `failed` instance's FleetEntry carries the last-known
    // failed cause (the same value `show` uses), while a healthy instance carries
    // none. This exercises fleet_entry_for's cause-resolution path.
    let state = TempDir::new().unwrap();
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade.register("boom", "mock").unwrap();
    facade.register("ok", "mock").unwrap();

    // Seed `boom` to `failed` with a write-ahead record carrying a cause (the
    // record-based cause path), directly in the store — no real crash needed.
    let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
    conn.execute(
        "UPDATE agent_instances SET state = 'failed' WHERE name = 'boom'",
        [],
    )
    .unwrap();
    let id: i64 = conn
        .query_row(
            "SELECT id FROM agent_instances WHERE name = 'boom'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO agent_runtime \
         (instance_id, pid, start_time, restart_policy, restart_count, last_known_cause) \
         VALUES (?1, 0, 0, 'on-failure', 2, 'crashed with code 1')",
        rusqlite::params![id],
    )
    .unwrap();
    drop(conn);

    let fleet = facade.fleet().unwrap();
    let boom = fleet.iter().find(|e| e.name.as_str() == "boom").unwrap();
    assert_eq!(boom.state, LifecycleState::Failed);
    assert_eq!(
        boom.restart_count, 2,
        "the seeded restart count is surfaced"
    );
    assert_eq!(
        boom.failed_cause.as_deref(),
        Some("crashed with code 1"),
        "a failed entry must surface its cause"
    );
    // A healthy instance carries no failed cause.
    let ok = fleet.iter().find(|e| e.name.as_str() == "ok").unwrap();
    assert!(ok.failed_cause.is_none(), "a healthy entry has no cause");
}

#[test]
fn fleet_reflects_a_state_transition_on_the_next_read_freshness() {
    // Story 1-7 (Task 3, AC6 ≤2s freshness): the listing reads live persisted
    // state on every call — there is no cache — so a transition committed before
    // the read is ALWAYS reflected on the next fleet() (a single DB read, far
    // under 2s). We seed the transition directly (write the persisted state) and
    // assert the very next fleet() read reflects it — the reaper's 250ms poll only
    // makes long-lived embeddings fresh too; the read path itself is what carries
    // the guarantee. Seeding directly (rather than driving a real crash) keeps the
    // freshness assertion deterministic under coverage instrumentation.
    let state = TempDir::new().unwrap();
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade.register("svc", "mock").unwrap();

    // First read: freshly registered.
    let before = facade.fleet().unwrap();
    let entry = before.iter().find(|e| e.name.as_str() == "svc").unwrap();
    assert_eq!(entry.state, LifecycleState::Registered);

    // Commit a state transition out-of-band (a direct persisted write, standing in
    // for any committed transition), then read again WITHOUT reopening the engine.
    let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
    let affected = conn
        .execute(
            "UPDATE agent_instances SET state = 'stopped' WHERE name = ?1",
            ["svc"],
        )
        .unwrap();
    assert_eq!(affected, 1);

    // The next listing reflects the new state immediately (no stale cache).
    let after = facade.fleet().unwrap();
    let entry = after.iter().find(|e| e.name.as_str() == "svc").unwrap();
    assert_eq!(
        entry.state,
        LifecycleState::Stopped,
        "a committed transition must be reflected on the next listing (freshness)"
    );
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

/// Count write-ahead spawn records (`agent_runtime` rows) for `name` by reading
/// the engine's SQLite DB directly — used to prove `remove` clears the record so
/// no orphan can be adopted later (AI-11). Returns 0 if the row/instance is gone.
fn spawn_record_count(state: &Path, name: &str) -> i64 {
    let conn = rusqlite::Connection::open(state.join("state.db")).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM agent_runtime r \
         JOIN agent_instances i ON i.id = r.instance_id WHERE i.name = ?1",
        [name],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn ai11_remove_of_a_live_instance_terminates_it_and_leaves_no_orphan() {
    // AI-11: `remove` of a LIVE (running) instance must terminate its process
    // (leaving no unsupervised orphan) AND clear its write-ahead spawn record (so
    // a later engine crash cannot leave a TRUE orphan no future engine can adopt).
    // Drive the PUBLIC engine: start a long-lingering agent, capture its pid,
    // `remove --force`, then assert the process is gone and no spawn record / no
    // instance row remains.
    //
    // Temporary CI mitigation (#109): this test deadlocks uninterruptibly (D-state)
    // on the x86-64 ubuntu GitHub runner ONLY — it passes on macOS, Windows, arm64
    // Linux, and local Linux. Skip it on Linux-in-CI so #106's wins land + coverage
    // (#101) can run; #109 tracks the root-cause + un-skip. (See module docs: heavy
    // re-exec + surviving-orphan harness.)
    if is_linux_ci() {
        return;
    }
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(manifest.path(), "svc", &["--linger-ms", "600000"]);

    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "victim",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    let started = facade.start("victim").unwrap();
    assert_eq!(started.state, LifecycleState::Running);

    // The process is alive and its write-ahead spawn record was committed.
    let pid = wait_for_agent_pid(&agent_log_path(state.path(), "victim"));
    assert!(pid_alive(pid), "the agent must be running before remove");
    assert_eq!(
        spawn_record_count(state.path(), "victim"),
        1,
        "a running instance has a committed spawn record"
    );

    // remove --force: the running-guard is satisfied, and the live process is
    // torn down BEFORE the row is deleted (AI-11).
    facade
        .remove("victim", RemoveDisposition::Delete, true)
        .unwrap();

    // (a) the process is gone — no unsupervised orphan left behind.
    wait_until_gone(
        pid,
        "remove of a live instance must terminate its process (no orphan)",
    );
    // (b) the instance row is gone (removed from the Fleet).
    let fleet = facade.fleet().unwrap();
    assert!(
        !fleet.iter().any(|e| e.name.as_str() == "victim"),
        "the removed instance must be gone from the Fleet"
    );
    // (c) no write-ahead spawn record remains — a later engine crash cannot leave
    // a TRUE orphan (the record the stop path cleared is what a future engine
    // would have adopted from).
    assert_eq!(
        spawn_record_count(state.path(), "victim"),
        0,
        "remove must clear the write-ahead spawn record (no adoptable orphan)"
    );

    // A fresh engine over the same state dir finds nothing to adopt and no orphan
    // process — the NFR-1 invariant holds across a restart after remove.
    drop(engine);
    let engine2 = Engine::open(Some(state.path().to_path_buf())).unwrap();
    assert!(
        !engine2
            .blocking()
            .fleet()
            .unwrap()
            .iter()
            .any(|e| e.name.as_str() == "victim"),
        "a reopened engine must not resurrect a removed instance"
    );
    assert!(!pid_alive(pid), "no orphan process may remain after remove");
}
