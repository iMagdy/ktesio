//! Integration tests for story 6-2: run the REAL (native-adapter) Hermes agent
//! under the Ktesio lifecycle — every Epic 1 lifecycle AC exercised against
//! `--kind hermes` through the PUBLIC [`Engine`] API.
//!
//! ## Isolation strategy (the AC's documented-sandbox clause)
//!
//! **No network, anywhere.** The "real Hermes gateway" in these tests is a
//! PATH shim: the committed `hermes_shim` conformance launcher COPIED to
//! `<tmp>/hermes<EXE_SUFFIX>`, with its directory PREPENDED to `PATH` so the
//! adapter's code-declared launch (`exec = "hermes"`,
//! `args = ["gateway", "run", "--external-supervisor"]`) resolves to the shim.
//! That argv is CONTRACT — tests cannot add flags to it — so the shim forwards
//! the original argv FIRST and appends flags scripted via its `HERMES_SHIM_ARGS`
//! env var before re-exec'ing the real `fake_agent` fixture. A `--dump`
//! artifact then proves BOTH halves in one committed file: the fixed launch
//! arrived verbatim (`arg=gateway`, …), and which environment the
//! engine injected (`env=HERMES_HOME=…`). The agent's usage/insights surfaces
//! are likewise simulated by fake_agent's self-reported emission channel —
//! the same ingestion path a real gateway report takes.
//!
//! **PATH discipline:** mutating `PATH` is process-global (and `unsafe` under
//! edition 2024), so EVERY spawn-dependent phase lives inside ONE `#[test]`
//! function running sequentially over one instance in ONE engine, and the
//! mutation happens ONCE at that test's start (before any other thread this
//! test spawns). Out-of-band probes borrowed from adoption.rs (`kill -0`,
//! `tasklist`) resolve through the preserved remainder of PATH. Tests that
//! need NO process spawn (declaration surface, config composition) stay
//! independent `#[test]` fns and never touch the environment.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ktesio_engine::{
    ConfigLayer, Engine, LifecycleState, MemoryBackingKind, OsId, RestartPolicy, SourceLayer,
    TransitionCause,
};
use tempfile::TempDir;

fn open(base: &TempDir) -> Engine {
    Engine::open(Some(base.path().to_path_buf())).expect("open engine")
}

/// Poll `instance_status` until `pred(state)` holds, bounded (crash.rs pattern).
fn wait_until_state(
    facade: &ktesio_engine::Blocking<'_>,
    name: &str,
    pred: impl Fn(LifecycleState) -> bool,
    within: Duration,
    what: &str,
) -> LifecycleState {
    let deadline = Instant::now() + within;
    loop {
        let state = facade
            .instance_status(name)
            .map(|s| s.instance.state)
            .unwrap_or(LifecycleState::Registered);
        if pred(state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} (last state: {state})"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether a pid is alive (adoption.rs pattern: tasklist on Windows, kill -0 +
/// a /proc zombie discount elsewhere).
fn pid_alive(pid: u32) -> bool {
    match OsId::current() {
        OsId::Windows => {
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
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            exists && !proc_pid_is_zombie(pid)
        }
    }
}

/// `/proc/<pid>/stat` state `Z` discount (Linux reaped-but-not-reaped children).
fn proc_pid_is_zombie(pid: u32) -> bool {
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

/// Poll until the dump file exists and contains an `env={var}=` line
/// (memory.rs pattern); returns the whole dump text.
fn poll_dump_for(dump: &Path, var: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(dump) {
            if text.contains(&format!("env={var}=")) {
                return text;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the agent never wrote its dump at {}: expected env={var}=…",
            dump.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The agent.log path inside an instance's Agent Home (interaction.rs shape).
fn agent_log_path(base: &Path, name: &str) -> PathBuf {
    base.join("agents")
        .join(name)
        .join("logs")
        .join("agent.log")
}

/// Committed usage-ledger row count for `name` (metering.rs read-only probe).
fn usage_row_count(state_dir: &Path, name: &str) -> u64 {
    let conn = rusqlite::Connection::open(state_dir.join("state.db")).expect("open state db");
    conn.query_row(
        "SELECT COUNT(*) FROM usage_events e \
         JOIN agent_instances i ON i.id = e.instance_id WHERE i.name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u64)
    .unwrap_or(0)
}

fn wait_for_usage_rows(state_dir: &Path, name: &str, expected: u64, within: Duration) -> u64 {
    let deadline = Instant::now() + within;
    loop {
        let count = usage_row_count(state_dir, name);
        if count >= expected {
            return count;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} committed usage rows for '{name}' (have {count})"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// fake_agent emits 10-in / 20-out per event (its own sentinels).
const USAGE_INPUT: u64 = 10;
const USAGE_OUTPUT: u64 = 20;

// ---------------------------------------------------------------------------
// No-spawn tests: declaration surface + composition honesty.
// ---------------------------------------------------------------------------

#[test]
fn hermes_registers_with_its_declared_per_os_shape_and_self_reported_metering() {
    // DC-2 / CP-a,d: registration surfaces the adapter's declared capabilities
    // for the CURRENT OS (pause best-effort — explicitly surfaced, never
    // silent) and the self-reported Metering Source in Fleet detail.
    let tmp = TempDir::new().unwrap();
    let engine = open(&tmp);
    let facade = engine.blocking();

    let registered = facade.register("flagship", "hermes").unwrap();
    assert_eq!(registered.kind, "hermes");
    assert_eq!(registered.state, LifecycleState::Registered);

    let caps = facade.effective_capabilities("flagship").unwrap();
    assert_eq!(caps.os, OsId::current());
    let pause = caps
        .entries
        .iter()
        .find(|(c, _)| *c == ktesio_engine::Capability::Pause)
        .expect("pause is declared");
    assert_eq!(
        pause.1,
        ktesio_engine::SupportLevel::BestEffort,
        "Hermes pause must be BEST-EFFORT on every OS (CP-a)"
    );
    let interaction = caps
        .entries
        .iter()
        .find(|(c, _)| *c == ktesio_engine::Capability::Interaction)
        .expect("interaction is declared");
    assert_eq!(
        interaction.1,
        ktesio_engine::SupportLevel::Guaranteed,
        "gateway stdin interaction is guaranteed"
    );

    let fleet = facade.fleet().unwrap();
    let entry = fleet
        .iter()
        .find(|e| e.name.as_str() == "flagship")
        .unwrap();
    assert_eq!(entry.metering_source, "self-reported");

    // The code-declared launch is public contract surface (DC-1).
    let launch = ktesio_engine::adapter::native_launch("hermes").expect("declared");
    assert_eq!(launch.exec, "hermes");
    assert_eq!(launch.args, vec!["gateway", "run", "--external-supervisor"]);
}

#[test]
fn hermes_memory_composition_maps_the_managed_dir_onto_hermes_home_exactly_as_start_would() {
    // DC-8 (the mock-leg shape from memory.rs): attach filesystem backing, fold
    // the invocation override into effective_config, resolve hermes' mapping,
    // apply it onto the DECLARED bare launch — HERMES_HOME must carry the
    // managed dir, and nothing else may be injected.
    let tmp = TempDir::new().unwrap();
    let engine = open(&tmp);
    let facade = engine.blocking();

    facade.register("svc", "hermes").unwrap();
    let dir = facade
        .attach_memory("svc", MemoryBackingKind::Filesystem)
        .unwrap();
    let status = facade.memory_status("svc").unwrap().expect("attached");
    assert!(status.declared, "hermes declares memory.dir delivery");

    let overrides = ConfigLayer::parse(
        SourceLayer::InvocationOverride,
        "<memory-dir invocation override>",
        &format!("[memory]\ndir = '{}'\n", dir.display()),
    )
    .expect("override layer parses (memory.dir is a KNOWN key)");
    let effective = facade.effective_config("svc", overrides).unwrap();
    let mapping = ktesio_engine::adapter::resolve_config_mapping("hermes", None).unwrap();
    let env_var = mapping
        .target("memory.dir")
        .and_then(|t| t.env_var())
        .expect("hermes declares an env target for memory.dir")
        .to_string();

    let mut launch = ktesio_engine::adapter::StartLaunch {
        exec: "hermes".to_string(),
        args: vec![
            "gateway".to_string(),
            "run".to_string(),
            "--external-supervisor".to_string(),
        ],
        env: BTreeMap::new(),
    };
    ktesio_engine::adapter::apply_config_mapping(
        &mut launch,
        &mapping,
        &effective,
        &BTreeMap::new(),
        Path::new(&facade.instance_status("svc").unwrap().instance.agent_home),
    )
    .unwrap_or_else(|e| panic!("apply failed: {e}"));
    assert_eq!(
        launch.env.get(&env_var),
        Some(&dir.to_string_lossy().into_owned()),
        "HERMES_HOME ({env_var}) must receive the managed Memory Backing dir"
    );
    assert_eq!(launch.env.len(), 1, "nothing else is injected");
}

#[test]
fn hermes_model_key_is_a_documented_noop_and_unbacked_gets_no_hermes_home() {
    // DC-4 (Decision 6): a DOCUMENTED key the adapter does not map is a no-op —
    // `model` is known engine-side but hermes declares NO rule for it (switching
    // happens via the agent's own `hermes model` CLI). And the DC-8 caveat half:
    // with NO filesystem backing, applying hermes' mapping injects NOTHING —
    // the agent falls back to its own default home chain (documented, not an
    // error).
    let tmp = TempDir::new().unwrap();
    let engine = open(&tmp);
    let facade = engine.blocking();

    let _ = facade.register("bare", "hermes").unwrap();

    // An operator-set `model` survives resolution… (a FLAT key — `kt agent
    // config set svc model gpt-4` writes `model = "…"`, so the override mirrors
    // that exact shape; KNOWN keys are flat dotted paths, not tables).
    let overrides = ConfigLayer::parse(
        SourceLayer::InvocationOverride,
        "<operator override>",
        "model = 'ktesio-sentinel-model'\n",
    )
    .expect("`model` is a KNOWN key");
    let effective = facade
        .effective_config("bare", overrides)
        .expect("a documented key resolves");
    assert_eq!(
        effective.value("model").and_then(|v| v.as_str()),
        Some("ktesio-sentinel-model")
    );

    // …but applying hermes' mapping delivers it NOWHERE:
    let mapping = ktesio_engine::adapter::resolve_config_mapping("hermes", None).unwrap();
    assert!(
        mapping.target("model").is_none(),
        "`model` must be unmapped"
    );
    let mut launch = ktesio_engine::adapter::StartLaunch {
        exec: "hermes".to_string(),
        args: vec![
            "gateway".to_string(),
            "run".to_string(),
            "--external-supervisor".to_string(),
        ],
        env: BTreeMap::new(),
    };
    ktesio_engine::adapter::apply_config_mapping(
        &mut launch,
        &mapping,
        &effective,
        &BTreeMap::new(),
        Path::new(&facade.instance_status("bare").unwrap().instance.agent_home),
    )
    .unwrap_or_else(|e| panic!("apply failed: {e}"));
    assert!(
        !launch.env.contains_key("HERMES_HOME"),
        "unbacked instances get NO HERMES_HOME (default-chain fallback)"
    );
}

// ---------------------------------------------------------------------------
// THE single PATH-dependent test: sequential lifecycle phases over one engine.
// ---------------------------------------------------------------------------

/// Copy the committed `hermes_shim` launcher onto PATH as `hermes<EXE_SUFFIX>`
/// and return the shim path (module doc documents why PATH is mutated here).
///
/// Also copies the fake_agent binary into the SAME directory: the shim resolves
/// its script target via [`ktesio_conformance::fake_agent_bin`], which anchors
/// at the RUNNING EXECUTABLE's directory (`current_exe()` of the shim process —
/// `<shim_dir>/hermes`), NOT the engine's cwd. Without this copy the shim
/// panics (exit 101) and every launch reports LaunchFailed.
fn install_shim(shim_dir: &TempDir) -> PathBuf {
    let exe = std::env::current_exe().expect("locate the running test executable");
    let mut dir = exe;
    dir.pop(); // drop the test-bin file name
    if dir.ends_with("deps") {
        dir.pop(); // drop `deps`
    }
    let candidate = dir.join(format!("hermes_shim{}", std::env::consts::EXE_SUFFIX));
    let source = if candidate.exists() {
        candidate
    } else {
        // Not built by this harness — build it on demand (tarpaulin parity with
        // fake_agent_bin's fallback).
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let status = Command::new(cargo)
            .args(["build", "-p", "ktesio-conformance", "--bin", "hermes_shim"])
            .env_remove("RUSTC_WRAPPER") // a shimmed PATH must not break the build
            .status()
            .expect("run cargo for hermes_shim");
        assert!(status.success(), "on-demand hermes_shim build failed");
        candidate
    };
    let shim = shim_dir
        .path()
        .join(format!("hermes{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&source, &shim).expect("copy hermes_shim onto PATH");
    // Copying preserves permission bits (both binaries are cargo-built
    // executables), so no chmod is needed on any platform.
    let agent = ktesio_conformance::fake_agent_bin();
    std::fs::copy(
        &agent,
        shim_dir
            .path()
            .join(format!("fake_agent{}", std::env::consts::EXE_SUFFIX)),
    )
    .expect("copy fake_agent beside the shim");
    shim
}

#[test]
fn hermes_lifecycle_end_to_end_under_a_path_shimmed_gateway() {
    // ---- Sandbox setup: the PATH shim (strategy documented in the module doc).
    let shim_dir = TempDir::new().unwrap();
    let _shim = install_shim(&shim_dir);

    /// Script the shim for the NEXT launch (side channel — see module doc).
    fn script(extra: &str) {
        unsafe {
            std::env::set_var("HERMES_SHIM_ARGS", extra);
        }
    }

    // SAFETY: this is the ONLY PATH mutation in this binary; it runs once, at
    // this single test's start, before any child is spawned by the engine
    // threads below (edition 2024 requires the unsafe block for set_var; the
    // harness runs each #[test] fn on its own thread and no other test in this
    // file reads or writes PATH concurrently — cargo also runs integration-test
    // BINARIES serially by default, one thread each, unless --test-threads
    // shares a binary, which is exactly why ALL spawn-dependent phases live
    // inside THIS one function).
    let joined = {
        let mut paths: Vec<PathBuf> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        paths.insert(0, shim_dir.path().to_path_buf());
        std::env::join_paths(paths).expect("join PATH")
    };
    unsafe {
        std::env::set_var("PATH", &joined);
    }

    let state = TempDir::new().unwrap();
    let engine = open(&state);
    let facade = engine.blocking();
    let agent_log = agent_log_path(state.path(), "gw");

    // ================================================================
    // PHASE A — FR-4 register + start → running (standard transitions).
    // ================================================================
    facade.register("gw", "hermes").unwrap();
    facade
        .set_restart_policy("gw", RestartPolicy::OnFailure)
        .unwrap();
    script("--echo-stdin --linger-ms 600000");
    facade.start("gw").unwrap();
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(10),
        "the shimmed gateway to reach running",
    );

    // ================================================================
    // PHASE B — DC-8 backed delivery + verbatim launch proof: stop, attach a
    // filesystem backing (attach is TERMINAL-STATE ONLY — story 5-1's A-5 guard
    // refuses it on `running`), re-script the shim with a --dump target and
    // start; one artifact proves both the fixed gateway argv AND HERMES_HOME
    // in the child env.
    // ================================================================
    facade.stop("gw", Some(Duration::from_secs(5))).unwrap();
    let dir = facade
        .attach_memory("gw", MemoryBackingKind::Filesystem)
        .unwrap();
    let dump = shim_dir.path().join("phase-b.dump");
    script(&format!(
        "--echo-stdin --dump {} --linger-ms 600000",
        dump.display()
    ));
    facade.start("gw").unwrap();
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(10),
        "the wrapped gateway to reach running",
    );
    let dump_text = poll_dump_for(&dump, "HERMES_HOME");
    // The FIXED launch reached the child verbatim. argv[0] is the RESOLVED
    // script target (the fake_agent binary copied beside the shim), not the
    // literal `hermes` name — assert the three declared args plus the resolved
    // interpreter line.
    let mut lines = dump_text.lines().filter(|l| l.starts_with("arg="));
    let argv0 = lines.next().expect("at least one arg= line (argv[0])");
    assert!(
        argv0.ends_with(&format!("fake_agent{}", std::env::consts::EXE_SUFFIX)),
        "argv[0] must be the resolved script target; got {argv0:?} in:\n{dump_text}"
    );
    for arg in ["arg=gateway", "arg=run", "arg=--external-supervisor"] {
        assert!(
            dump_text.contains(arg),
            "the declared gateway launch must arrive intact; want {arg:?} in:\n{dump_text}"
        );
    }
    // The shim's OWN env leaks into the child (it inherits everything), so the
    // dump's env section carries `HERMES_SHIM_ARGS=…` too — that is shim
    // plumbing, not engine delivery. Assert ONLY the engine-injected key:
    let expected_env = format!("env=HERMES_HOME={}", dir.display());
    assert!(
        dump_text.contains(&expected_env),
        "backed instance must receive HERMES_HOME={}; dump:\n{dump_text}",
        dir.display()
    );
    // Honest provenance: the injected value never reaches effective-config.json.
    let home = PathBuf::from(facade.instance_status("gw").unwrap().instance.agent_home);
    let snapshot =
        std::fs::read_to_string(home.join("effective-config.json")).expect("snapshot at start");
    assert!(
        !snapshot.contains(dir.to_str().unwrap()),
        "HERMES_HOME injection is delivery, not operator config; snapshot:\n{snapshot}"
    );

    // ================================================================
    // PHASE C — FR-8 send_input round-trip (Interaction Guaranteed ×3).
    // ================================================================
    facade.send_input("gw", "hello").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let echoed = std::fs::read_to_string(&agent_log)
            .map(|c| c.lines().any(|l| l == "stdin: hello"))
            .unwrap_or(false);
        if echoed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the gateway never echoed the sent line"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // ================================================================
    // PHASE D — FR-7 pause/resume best-effort, SURFACED (CP-a). Runs on all
    // three OSes — hermes is best-effort everywhere, so there is no skip.
    // ================================================================
    let paused = facade.pause("gw").unwrap();
    assert_eq!(paused.state, LifecycleState::Paused);
    let events = facade.transition_events("gw").unwrap();
    let pause_evt = events
        .iter()
        .rfind(|e| e.new_state == LifecycleState::Paused)
        .expect("a running→paused event exists");
    let cause = serde_json::to_string(&pause_evt.cause).unwrap();
    assert!(
        cause.contains("\"kind\":\"pause-best-effort\""),
        "best-effort pause must be EXPLICITLY surfaced, got {cause}"
    );
    let resumed = facade.resume("gw").unwrap();
    assert_eq!(resumed.state, LifecycleState::Running);
    let events = facade.transition_events("gw").unwrap();
    let resume_evt = events
        .iter()
        .rev()
        .find(|e| e.new_state == LifecycleState::Running && e.prior_state == LifecycleState::Paused)
        .expect("a paused→running event exists");
    let cause = serde_json::to_string(&resume_evt.cause).unwrap();
    assert!(
        cause.contains("\"kind\":\"resume-best-effort\""),
        "resume must symmetrically carry resume-best-effort, got {cause}"
    );

    // ================================================================
    // PHASE E — FR-22 self-reported usage lands in the ledger; fleet totals
    // equal the ledger exactly; metering_source visible in Fleet detail.
    // ================================================================
    facade.stop("gw", Some(Duration::from_secs(5))).unwrap();
    script("--emit-usage 2 --linger-ms 600000");
    facade.start("gw").unwrap();
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(10),
        "the metering-phase gateway to reach running",
    );
    wait_for_usage_rows(state.path(), "gw", 2, Duration::from_secs(10));
    let fleet = facade.fleet().unwrap();
    let entry = fleet.iter().find(|e| e.name.as_str() == "gw").unwrap();
    assert_eq!(entry.metering_source, "self-reported");
    assert_eq!(entry.usage.cumulative_input_tokens, 2 * USAGE_INPUT);
    assert_eq!(entry.usage.cumulative_output_tokens, 2 * USAGE_OUTPUT);

    // ================================================================
    // PHASE F — FR-5 stop terminates the FULL PROCESS TREE.
    // ================================================================
    // Re-script with --spawn-child (pids come back over the public agent.log),
    // restart, then stop and prove BOTH pids are gone.
    facade.stop("gw", Some(Duration::from_secs(5))).unwrap();
    script("--spawn-child --linger-ms 600000");
    facade.start("gw").unwrap();
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(10),
        "the tree-phase gateway to reach running",
    );
    // Parse THIS generation's pids. agent.log accumulates across launches
    // (append mode), and every fake_agent — including the spawned child —
    // announces `fake_agent ready pid=<n>`. The parent program-order-writes
    // its own ready line and then `child-pid=`; the child's ready line lands
    // ASYNCHRONOUSLY after that, so matching the ready line immediately
    // preceding the last `child-pid=` line is deterministic regardless of
    // whether the child's announcement has arrived yet (and never reaches
    // back into earlier generations' lines).
    let deadline = Instant::now() + Duration::from_secs(10);
    let (parent_pid, child_pid) = loop {
        if let Ok(contents) = std::fs::read_to_string(&agent_log) {
            let child = contents
                .lines()
                .rev()
                .find_map(|l| l.strip_prefix("child-pid=").map(String::from));
            if let Some(c) = child {
                // Parent = last ready line BEFORE the child-pid= line.
                let before = contents
                    .rsplit_once("child-pid=")
                    .map(|(head, _)| head)
                    .unwrap_or_default();
                if let Some(p) = before
                    .lines()
                    .rev()
                    .find_map(|l| l.strip_prefix("fake_agent ready pid="))
                {
                    break (
                        p.parse::<u32>().expect("parent pid"),
                        c.parse::<u32>().expect("child pid"),
                    );
                }
            }
        }
        assert!(Instant::now() < deadline, "never saw readiness pid lines");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(pid_alive(parent_pid), "parent alive before stop");
    assert!(pid_alive(child_pid), "child alive before stop");
    facade.stop("gw", Some(Duration::from_secs(5))).unwrap();
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Stopped,
        Duration::from_secs(10),
        "the final stop to land stopped",
    );
    wait_until_gone(parent_pid, "the gateway parent must die with the instance");
    wait_until_gone(child_pid, "the gateway CHILD must die too (tree kill)");

    // ================================================================
    // PHASE G — FR-6 exit 75 (the CP-b external-supervisor hand-off) is JUST a
    // crash: the reaper detects it and the on-failure policy relaunches with
    // the SAME persisted launch → Restarted{count==1, waited_ms>=1000}.
    // ================================================================
    let crash_count = shim_dir.path().join("crash-count");
    script(&format!(
        "--crash-with 75 --crash-after-ms 450 --crash-times 1 --crash-state {} --linger-ms 600000",
        crash_count.display()
    ));
    facade.start("gw").unwrap(); // stopped → starting → running again
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(10),
        "the crash-phase gateway to reach running first",
    );
    // The exit-75 crash crosses the readiness window → detected → restarted
    // after the production 1s base backoff (single crash keeps count stable).
    let deadline = Instant::now() + Duration::from_secs(30);
    let restart_evt = loop {
        let events = facade.transition_events("gw").unwrap();
        if let Some(e) = events
            .iter()
            .find(|e| matches!(e.cause, TransitionCause::Restarted { .. }))
        {
            break e.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the exit-75 hand-off was never relaunched (no Restarted event)"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    // Recorded DATA on the event, not wall-clock measurements.
    match &restart_evt.cause {
        TransitionCause::Restarted { count, waited_ms } => {
            assert_eq!(*count, 1, "exactly one supervisor hand-off relaunch");
            assert!(
                *waited_ms >= 1000,
                "production backoff honored: waited_ms={waited_ms}"
            );
        }
        other => panic!("expected Restarted, got {other:?}"),
    }
    wait_until_state(
        &facade,
        "gw",
        |s| s == LifecycleState::Running,
        Duration::from_secs(30),
        "the relaunched gateway to reach running",
    );
    // Teardown.
    let _ = facade.stop("gw", Some(Duration::from_secs(5)));
}
