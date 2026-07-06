//! Integration tests for `kt agent register | remove | list`.
//!
//! These drive the real `kt` binary (via `CARGO_BIN_EXE_kt`) with
//! `KTESIO_STATE_DIR` pinned to a `TempDir`, so no test ever touches the real
//! user data dir. They assert the CLI contract: exit codes, the Agent Home
//! path on stdout, and diagnostics on stderr.

mod helpers;

use std::path::Path;

use helpers::{run_kt_agent, TestContext};

/// Path to the SQLite state DB the engine creates under a state base.
fn state_db(state_dir: &Path) -> std::path::PathBuf {
    state_dir.join("state.db")
}

/// Seed a `running` state onto an already-registered instance by writing
/// directly into the engine's SQLite DB. This is how the AC5 running-guard is
/// exercised end-to-end before a real supervision core exists (story 1.4).
fn force_state_running(state_dir: &Path, name: &str) {
    let conn = rusqlite::Connection::open(state_db(state_dir)).expect("open state db");
    let affected = conn
        .execute(
            "UPDATE agent_instances SET state = 'running' WHERE name = ?1",
            [name],
        )
        .expect("update state");
    assert_eq!(affected, 1, "expected to update exactly one row");
}

/// Start an instance via a SEPARATE, leaked engine subprocess (crash semantics)
/// so the spawned `fake_agent` SURVIVES the command's exit and can be adopted by
/// the next `kt` invocation (story 1-6). A normal `kt agent start` cleanly drops
/// its engine, which kills the process (the single-lifetime `Drop`), so pause on
/// a later invocation would honestly reconcile the dead-process row to `failed`.
/// To exercise real pause-on-a-LIVE-adopted-instance the process must genuinely
/// outlive its starter — which is exactly the engine-crash case: this re-execs
/// the test binary into `agent_cli_start_helper_subprocess`, which opens an
/// engine, starts the instance, and `std::process::exit`s WITHOUT dropping the
/// engine (no handle Drop → the agent survives and re-parents to init).
fn start_via_surviving_engine(state_dir: &Path, name: &str) {
    let exe = std::env::current_exe().expect("test exe");
    let status = std::process::Command::new(exe)
        .args([
            "--exact",
            "agent_cli_start_helper_subprocess",
            "--nocapture",
        ])
        .env("KTESIO_CLI_START_HELPER", name)
        .env("KTESIO_STATE_DIR", state_dir)
        .status()
        .expect("run cli start helper subprocess");
    assert!(
        status.success(),
        "cli start helper subprocess failed: {status}"
    );
}

/// The re-exec entry for [`start_via_surviving_engine`]. When
/// `KTESIO_CLI_START_HELPER` is unset this is a trivial pass. When set, it opens
/// an engine over `KTESIO_STATE_DIR`, starts the named instance, and exits
/// WITHOUT dropping the engine — leaving a surviving, adoptable process.
#[test]
fn agent_cli_start_helper_subprocess() {
    let Ok(name) = std::env::var("KTESIO_CLI_START_HELPER") else {
        return;
    };
    let state = std::path::PathBuf::from(std::env::var("KTESIO_STATE_DIR").unwrap());
    let engine = ktesio_engine::Engine::open(Some(state)).expect("helper engine open");
    engine.blocking().start(&name).expect("helper start");
    // Exit WITHOUT dropping `engine` (crash semantics): the started process
    // survives and re-parents to init, ready for the next command to adopt.
    std::process::exit(0);
}

#[test]
fn register_prints_home_path_and_exits_zero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let run = run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(run.success, "register should exit 0; stderr={}", run.stderr);

    // The Agent Home path is printed to stdout and exists on disk.
    let home = state_dir.join("agents").join("demo");
    assert!(
        run.stdout.contains(&home.to_string_lossy().to_string()),
        "stdout should contain the home path; stdout={}",
        run.stdout
    );
    assert!(home.join("config.toml").is_file());
    // The success confirmation is a command result (stdout), consistent with
    // every other `kt` command's `ui::success` usage. AD-12 reserves stderr for
    // diagnostics/notices; a completed-successfully confirmation is not one.
    assert!(run.stdout.contains("Registered"));
    // AC1: the effective per-OS Capability Declaration is surfaced on stdout
    // (the mock declares `pause` and `interaction`).
    assert!(
        run.stdout.contains("Capabilities for demo"),
        "stdout should render capabilities; stdout={}",
        run.stdout
    );
    assert!(run.stdout.contains("pause"), "stdout={}", run.stdout);
    // The adapter snapshot is persisted in the Agent Home.
    assert!(home.join("adapter.json").is_file());
}

#[test]
fn duplicate_registration_exits_nonzero_with_diagnostic() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let first = run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(first.success);

    // Re-register the same NAME (kind must resolve, so reuse `mock`).
    let second = run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!second.success, "duplicate should exit non-zero");
    // Diagnostic names the conflict and gives a remediation hint (to stderr).
    assert!(
        second.stderr.contains("already exists"),
        "stderr={}",
        second.stderr
    );
    assert!(second.stderr.contains("kt agent remove demo"));
}

#[test]
fn invalid_name_exits_nonzero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let run = run_kt_agent(
        &["agent", "register", "Bad_Name", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!run.success, "invalid name should exit non-zero");
    assert!(run.stderr.contains("Invalid Agent Instance name"));
}

#[test]
fn list_shows_registered_instances() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    // Empty Fleet first.
    let empty = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(empty.success);
    assert!(
        empty.stdout.contains("No Agent Instances") || empty.stderr.contains("No Agent Instances")
    );

    run_kt_agent(
        &["agent", "register", "alpha", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let listed = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(listed.success);
    assert!(listed.stdout.contains("alpha"), "stdout={}", listed.stdout);
    assert!(listed.stdout.contains("Fleet"));
}

#[test]
fn remove_delete_removes_home() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let home = state_dir.join("agents").join("demo");
    assert!(home.is_dir());

    let run = run_kt_agent(
        &["agent", "remove", "demo", "--delete"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        run.success,
        "remove --delete should exit 0; stderr={}",
        run.stderr
    );
    assert!(!home.exists(), "home should be gone after --delete");
}

#[test]
fn remove_retain_keeps_home() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let home = state_dir.join("agents").join("demo");

    // Default (no flag) retains; be explicit here to assert the flag path.
    let run = run_kt_agent(
        &["agent", "remove", "demo", "--retain"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        run.success,
        "remove --retain should exit 0; stderr={}",
        run.stderr
    );
    assert!(home.is_dir(), "home should remain after --retain");
}

#[test]
fn remove_running_without_force_exits_nonzero_and_with_force_succeeds() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    run_kt_agent(
        &["agent", "register", "live", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    // Seed the running state directly (no supervision core yet — AC5 boundary).
    force_state_running(state_dir, "live");

    // Without --force: refused, non-zero, diagnostic to stderr.
    let refused = run_kt_agent(
        &["agent", "remove", "live", "--delete"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !refused.success,
        "running remove without --force should fail"
    );
    assert!(
        refused.stderr.contains("running") && refused.stderr.contains("--force"),
        "stderr={}",
        refused.stderr
    );
    // Instance still present.
    let still = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(still.stdout.contains("live"));

    // With --force: succeeds.
    let forced = run_kt_agent(
        &["agent", "remove", "live", "--delete", "--force"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        forced.success,
        "running remove with --force should exit 0; stderr={}",
        forced.stderr
    );
    let home = state_dir.join("agents").join("live");
    assert!(!home.exists());
}

// ---- Story 1.3: manifest adapters + `kt agent show` ----

/// A complete valid `adapter.toml` for a manifest-adapter directory fixture.
const VALID_MANIFEST: &str = r#"
contract_version = "0.1.0"

[adapter]
kind = "demo-manifest"

[lifecycle.start]
exec = "demo-agent"

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[metering]
source = "self-reported"
"#;

/// Write an `adapter.toml` into a fresh subdirectory of `dir` and return it.
fn manifest_dir(dir: &Path, body: &str) -> std::path::PathBuf {
    let m = dir.join("manifest-adapter");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(m.join("adapter.toml"), body).unwrap();
    m
}

#[test]
fn register_manifest_exits_zero_and_shows_capabilities() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = manifest_dir(&ctx.project_dir, VALID_MANIFEST);

    let run = run_kt_agent(
        &["agent", "register", "m", "--manifest", m.to_str().unwrap()],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        run.success,
        "manifest register should exit 0; stderr={}",
        run.stderr
    );
    // The instance kind comes from the manifest's [adapter] kind.
    assert!(
        run.stdout.contains("demo-manifest"),
        "stdout={}",
        run.stdout
    );
    assert!(
        run.stdout.contains("Capabilities for m"),
        "stdout={}",
        run.stdout
    );
    // Home + adapter snapshot exist.
    let home = state_dir.join("agents").join("m");
    assert!(home.join("adapter.json").is_file());
}

#[test]
fn register_invalid_manifest_exits_nonzero_naming_section() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    // Drop [metering] → invalid, naming the section.
    let body = VALID_MANIFEST.replace("[metering]\nsource = \"self-reported\"\n", "");
    let m = manifest_dir(&ctx.project_dir, &body);

    let run = run_kt_agent(
        &["agent", "register", "m", "--manifest", m.to_str().unwrap()],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!run.success, "invalid manifest should exit non-zero");
    assert!(
        run.stderr.contains("[metering]"),
        "diagnostic should name the section; stderr={}",
        run.stderr
    );
    // No partial state.
    assert!(!state_dir.join("agents").join("m").exists());
}

#[test]
fn register_manifest_not_found_exits_nonzero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let missing = ctx.project_dir.join("no-such-dir");

    let run = run_kt_agent(
        &[
            "agent",
            "register",
            "m",
            "--manifest",
            missing.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!run.success, "missing manifest should exit non-zero");
    assert!(
        run.stderr.contains("adapter.toml") || run.stderr.contains("No adapter.toml"),
        "stderr={}",
        run.stderr
    );
}

#[test]
fn register_unknown_kind_exits_nonzero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let run = run_kt_agent(
        &["agent", "register", "x", "--kind", "no-such-kind"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!run.success, "unknown kind should exit non-zero");
    assert!(
        run.stderr.contains("Unknown adapter kind"),
        "stderr={}",
        run.stderr
    );
    assert!(!state_dir.join("agents").join("x").exists());
}

#[test]
fn register_requires_kind_or_manifest() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    // Neither --kind nor --manifest → clap rejects before the engine runs.
    let run = run_kt_agent(&["agent", "register", "x"], &ctx.project_dir, state_dir);
    assert!(!run.success, "register with no adapter flag should fail");
}

#[test]
fn show_renders_effective_capabilities() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let show = run_kt_agent(&["agent", "show", "demo"], &ctx.project_dir, state_dir);
    assert!(show.success, "show should exit 0; stderr={}", show.stderr);
    assert!(
        show.stdout.contains("Capabilities for demo"),
        "stdout={}",
        show.stdout
    );
    assert!(show.stdout.contains("pause"), "stdout={}", show.stdout);
}

#[test]
fn show_unknown_instance_exits_nonzero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let show = run_kt_agent(&["agent", "show", "ghost"], &ctx.project_dir, state_dir);
    assert!(!show.success, "show of a missing instance should fail");
    assert!(
        show.stderr.contains("No Agent Instance named 'ghost'"),
        "stderr={}",
        show.stderr
    );
}

// ---- Story 1.4: `kt agent start` / `kt agent stop` ----

/// Locate the `fake_agent` helper binary: a sibling of the `kt` test binary in
/// the same `target/<profile>/` dir (both are workspace bins built by
/// `--all-targets`). Cross-crate `CARGO_BIN_EXE_*` is unavailable, so resolve by
/// sibling path from `CARGO_BIN_EXE_kt`. If it is not present (e.g. under
/// tarpaulin, which does not build sibling bins), build it on demand.
fn fake_agent_bin() -> std::path::PathBuf {
    let kt = std::path::PathBuf::from(env!("CARGO_BIN_EXE_kt"));
    let dir = kt.parent().expect("kt bin has a parent dir");
    let bin = dir.join(format!("fake_agent{}", std::env::consts::EXE_SUFFIX));
    if bin.exists() {
        return bin;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "ktesio-conformance", "--bin", "fake_agent"])
        .status();
    assert!(
        matches!(status, Ok(s) if s.success()) && bin.exists(),
        "fake_agent not found at {} and on-demand build failed ({status:?})",
        bin.display()
    );
    bin
}

/// Write a manifest whose `[lifecycle.start]` exec points at `fake_agent`.
fn fake_agent_manifest(dir: &Path, args: &[&str]) -> std::path::PathBuf {
    let m = dir.join("fake-agent-adapter");
    std::fs::create_dir_all(&m).unwrap();
    let bin = fake_agent_bin();
    let args_toml = args
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "fake"

[lifecycle.start]
exec = {exec:?}
args = [{args_toml}]

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
    );
    std::fs::write(m.join("adapter.toml"), body).unwrap();
    m
}

#[test]
fn start_prints_running_state_and_exits_zero() {
    // AC1 at the CLI: register a manifest agent, start it → the new state
    // `running` is printed to stdout and the exit code is 0.
    //
    // NOTE (single-lifetime boundary): each `kt` invocation is its own engine
    // lifetime; the started process is cleaned up on exit (kill-on-drop). A
    // separate `kt agent stop` cannot re-attach to it (orphan adoption is story
    // 1-6). This test asserts the START contract; the full start→stop→no-survivor
    // proof lives in the engine's single-lifetime lifecycle integration test.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = fake_agent_manifest(&ctx.project_dir, &["--linger-ms", "600000"]);

    run_kt_agent(
        &[
            "agent",
            "register",
            "svc",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(&["agent", "start", "svc"], &ctx.project_dir, state_dir);
    assert!(run.success, "start should exit 0; stderr={}", run.stderr);
    assert!(run.stdout.contains("running"), "stdout={}", run.stdout);
    assert!(run.stdout.contains("Started"), "stdout={}", run.stdout);
}

#[test]
fn start_prints_single_lifetime_notice_to_stderr_only() {
    // LOW-1: the success path is honest about single-lifetime supervision — a
    // standalone `kt agent start` kills the agent when the CLI exits cleanly, and
    // durable supervision across SEPARATE CLI invocations is future work (story
    // 1-6 delivered crash recovery, NOT clean-exit cross-command survival). That
    // caveat is printed as a one-line NOTICE to STDERR (AD-12: results → stdout,
    // notices → stderr), and the stdout result line (`running`) is UNCHANGED.
    // This asserts both halves so a future change that either drops the notice or
    // leaks it onto stdout is caught.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = fake_agent_manifest(&ctx.project_dir, &["--linger-ms", "600000"]);

    run_kt_agent(
        &[
            "agent",
            "register",
            "svc",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(&["agent", "start", "svc"], &ctx.project_dir, state_dir);
    assert!(run.success, "start should exit 0; stderr={}", run.stderr);
    // stdout result line is unchanged (still shows `running`).
    assert!(run.stdout.contains("running"), "stdout={}", run.stdout);
    // The notice is on stderr and states the honest boundary (supervised only for
    // this engine session; cross-invocation durability is future work). It must
    // NOT promise cross-CLI durable supervision as delivered.
    assert!(
        run.stderr
            .contains("supervised only for this engine session"),
        "single-lifetime notice must go to stderr; stderr={}",
        run.stderr
    );
    assert!(
        run.stderr.contains("future work"),
        "notice must state durable cross-invocation supervision is future work; stderr={}",
        run.stderr
    );
    // It must NOT claim durable cross-CLI supervision arrives with 1-6 (that was
    // the false promise this fix removes).
    assert!(
        !run.stderr.contains("across CLI invocations arrives"),
        "notice must not promise cross-CLI durable supervision as delivered; stderr={}",
        run.stderr
    );
    // The notice must NOT leak onto stdout (AD-12: stdout is the result only).
    assert!(
        !run.stdout
            .contains("supervised only for this engine session"),
        "notice must not appear on stdout; stdout={}",
        run.stdout
    );
}

#[test]
fn start_missing_exec_lands_failed_with_preserved_diagnostic() {
    // AC2 at the CLI: a manifest whose start exec does not exist → non-zero
    // exit, the diagnostic is preserved on stderr, and the instance is `failed`.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = ctx.project_dir.join("bad-adapter");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(
        m.join("adapter.toml"),
        r#"
contract_version = "0.1.0"
[adapter]
kind = "bad"
[lifecycle.start]
exec = "ktesio-no-such-binary-cli-1-4"
[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"
[metering]
source = "self-reported"
"#,
    )
    .unwrap();

    run_kt_agent(
        &[
            "agent",
            "register",
            "bad",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(&["agent", "start", "bad"], &ctx.project_dir, state_dir);
    assert!(!run.success, "start of a bad exec should exit non-zero");
    assert!(
        run.stderr.contains("failed to launch"),
        "stderr={}",
        run.stderr
    );
    assert!(
        run.stderr.contains("ktesio-no-such-binary-cli-1-4"),
        "diagnostic preserved; stderr={}",
        run.stderr
    );
    // The instance is now `failed`.
    let list = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(list.stdout.contains("failed"), "stdout={}", list.stdout);
}

#[test]
fn stop_on_stopped_returns_uniform_invalid_transition() {
    // AC4 at the CLI: `stop` on a freshly-registered (registered) instance —
    // never started — returns the uniform invalid-transition diagnostic and a
    // non-zero exit, identical wording regardless of adapter kind.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    // Native builtin.
    run_kt_agent(
        &["agent", "register", "nat", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(&["agent", "stop", "nat"], &ctx.project_dir, state_dir);
    assert!(!run.success, "stop on registered should exit non-zero");
    assert!(
        run.stderr.contains("cannot stop"),
        "uniform invalid-transition; stderr={}",
        run.stderr
    );
}

#[test]
fn start_unknown_instance_exits_nonzero() {
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let run = run_kt_agent(&["agent", "start", "ghost"], &ctx.project_dir, state_dir);
    assert!(!run.success, "start of a missing instance should fail");
    assert!(run.stderr.contains("ghost"), "stderr={}", run.stderr);
}

#[test]
fn stop_accepts_timeout_flag() {
    // The `--timeout <secs>` flag parses and drives the graceful window. Stop on
    // a registered instance still rejects (AC4) but proves the flag is accepted.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "svc", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(
        &["agent", "stop", "svc", "--timeout", "5"],
        &ctx.project_dir,
        state_dir,
    );
    // Registered → stop is invalid, but the flag parsed (no clap error).
    assert!(run.stderr.contains("cannot stop"), "stderr={}", run.stderr);
}

// ---- Story 1.5: `kt agent pause` / `kt agent resume` (AC6) ----

/// The wire key for the current OS's `[capabilities.pause]` entry. Runtime data
/// (matches the engine's `OsId::current()` mapping), not conditional compilation.
fn current_os_pause_key() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        "windows" => "windows",
        _ => "other",
    }
}

/// Write a `fake_agent` manifest whose CURRENT-OS pause level is `pause_level`.
fn fake_agent_manifest_with_pause(
    dir: &Path,
    args: &[&str],
    pause_level: &str,
) -> std::path::PathBuf {
    let m = dir.join("pause-adapter");
    std::fs::create_dir_all(&m).unwrap();
    let bin = fake_agent_bin();
    let args_toml = args
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "fake"

[lifecycle.start]
exec = {exec:?}
args = [{args_toml}]

[capabilities.pause]
{os} = "{pause_level}"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
        os = current_os_pause_key(),
    );
    std::fs::write(m.join("adapter.toml"), body).unwrap();
    m
}

#[test]
fn pause_prints_paused_state_and_exits_zero_guaranteed_unix() {
    // AC6 + AC1 at the CLI (Unix guaranteed): `kt agent pause` on a genuinely
    // LIVE instance prints the new state `paused` to stdout with exit 0 and NO
    // best-effort qualifier. Runtime-skip on Windows (guaranteed pause is
    // Unix-only); NO cfg — data-driven skip.
    //
    // NOTE (single-lifetime CLI boundary, story 1-6): each `kt` command is a
    // short-lived engine whose handle Drop kills the process on the command's
    // clean exit (the story-1-4 single-lifetime safety net; durable
    // cross-invocation supervision remains future work — orphan ADOPTION here
    // covers the engine-CRASH case, proven in `tests/adoption.rs`). So this test
    // proves the pause command's CLI WIRING against a live adopted instance; it
    // does NOT chain a follow-up `kt agent resume` (the paused process does not
    // survive the pause command's clean drop). The pause/resume SEMANTICS —
    // including resume after a real SIGSTOP within one engine lifetime — are
    // covered by the engine integration tests in `tests/pause.rs`.
    if std::env::consts::OS == "windows" {
        return;
    }
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = fake_agent_manifest_with_pause(
        &ctx.project_dir,
        &["--heartbeat-ms", "50", "--linger-ms", "600000"],
        "guaranteed",
    );
    run_kt_agent(
        &[
            "agent",
            "register",
            "svc",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    // Start via a surviving (crashed-engine) subprocess so the process is
    // genuinely LIVE when the pause command adopts it (story 1-6). A plain
    // `kt agent start` would kill it on the command's clean engine drop.
    start_via_surviving_engine(state_dir, "svc");

    let paused = run_kt_agent(&["agent", "pause", "svc"], &ctx.project_dir, state_dir);
    assert!(
        paused.success,
        "guaranteed pause should exit 0; stderr={}",
        paused.stderr
    );
    assert!(paused.stdout.contains("paused"), "stdout={}", paused.stdout);
    assert!(paused.stdout.contains("Paused"), "stdout={}", paused.stdout);
    // A guaranteed pause emits NO best-effort qualifier on stderr.
    assert!(
        !paused.stderr.contains("best-effort"),
        "guaranteed pause must not print a best-effort note; stderr={}",
        paused.stderr
    );

    // Teardown: the pause command's clean drop already killed the SIGSTOP'd
    // process; a `stop` here settles the row (idempotent, no survivor).
    run_kt_agent(&["agent", "stop", "svc"], &ctx.project_dir, state_dir);
}

#[test]
fn pause_best_effort_prints_qualifier_note_to_stderr_only() {
    // AC2 + AC6 at the CLI: a best-effort pause prints the new state `paused` to
    // STDOUT and a VISIBLE qualifier NOTE to STDERR (never silent, never on
    // stdout). Mirrors the LOW-1 stdout/stderr-discipline assertion from 1-4.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m =
        fake_agent_manifest_with_pause(&ctx.project_dir, &["--linger-ms", "600000"], "best-effort");
    run_kt_agent(
        &["agent", "register", "be", "--manifest", m.to_str().unwrap()],
        &ctx.project_dir,
        state_dir,
    );
    // Start via a surviving (crashed-engine) subprocess so the process is live
    // when pause adopts it (story 1-6); a plain `kt agent start` kills it on exit.
    start_via_surviving_engine(state_dir, "be");

    let paused = run_kt_agent(&["agent", "pause", "be"], &ctx.project_dir, state_dir);
    assert!(
        paused.success,
        "best-effort pause should exit 0; stderr={}",
        paused.stderr
    );
    // Result line on stdout.
    assert!(paused.stdout.contains("paused"), "stdout={}", paused.stdout);
    // The qualifier note is on STDERR and names best-effort.
    assert!(
        paused.stderr.contains("best-effort"),
        "best-effort qualifier must be on stderr; stderr={}",
        paused.stderr
    );
    // The qualifier must NOT leak onto stdout (AD-12: stdout is the result only).
    assert!(
        !paused.stdout.contains("best-effort"),
        "qualifier must not appear on stdout; stdout={}",
        paused.stdout
    );

    // Teardown.
    run_kt_agent(&["agent", "stop", "be"], &ctx.project_dir, state_dir);
}

#[test]
fn pause_unsupported_exits_nonzero_quoting_the_declaration() {
    // AC3 + AC6 at the CLI: a pause that is `unsupported` on this OS fails fast
    // with a non-zero exit and a diagnostic (on STDERR) that QUOTES the
    // declaration (names pause, the OS, the level) and points at `kt agent show`.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    // Declare pause only for an OS that is NOT the current one → current-OS
    // projection is Unsupported.
    let other = if std::env::consts::OS == "windows" {
        "linux"
    } else {
        "windows"
    };
    let m = fake_agent_manifest_with_pause(&ctx.project_dir, &["--linger-ms", "600000"], "ignored");
    // Overwrite the manifest so pause is declared ONLY for the other OS.
    let bin = fake_agent_bin();
    let body = format!(
        r#"
contract_version = "0.1.0"
[adapter]
kind = "fake"
[lifecycle.start]
exec = {exec:?}
args = ["--linger-ms", "600000"]
[capabilities.pause]
{other} = "guaranteed"
[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"
[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
        other = other,
    );
    std::fs::write(m.join("adapter.toml"), body).unwrap();

    run_kt_agent(
        &["agent", "register", "un", "--manifest", m.to_str().unwrap()],
        &ctx.project_dir,
        state_dir,
    );
    // Start via a surviving (crashed-engine) subprocess so the instance is
    // genuinely `running` (adopted) when pause runs — so pause fails fast with
    // the UNSUPPORTED diagnostic, not a reconciled-to-failed transition error.
    start_via_surviving_engine(state_dir, "un");

    let paused = run_kt_agent(&["agent", "pause", "un"], &ctx.project_dir, state_dir);
    assert!(
        !paused.success,
        "unsupported pause must exit non-zero; stdout={}",
        paused.stdout
    );
    assert!(
        paused.stderr.contains("cannot pause"),
        "stderr must quote the declaration; stderr={}",
        paused.stderr
    );
    assert!(
        paused.stderr.contains("unsupported"),
        "stderr must name the level; stderr={}",
        paused.stderr
    );
    assert!(
        paused.stderr.contains("kt agent show un"),
        "stderr must point at kt agent show; stderr={}",
        paused.stderr
    );
    // Fail-fast made NO transition to `paused`: the pause command exited
    // non-zero WITHOUT persisting a pause. (We do not re-check the state via a
    // follow-up `kt` command here: each command's clean engine drop kills the
    // adopted process, and the next command's honest adoption would then
    // reconcile the gone process to `failed` — a single-lifetime CLI artifact,
    // NOT a pause side effect. The no-persist guarantee of the unsupported
    // fail-fast is proven at the engine level in `tests/pause.rs`.)

    // Teardown.
    run_kt_agent(&["agent", "stop", "un"], &ctx.project_dir, state_dir);
}

#[test]
fn pause_on_registered_returns_uniform_invalid_transition() {
    // AC4 at the CLI: pause on a registered (never started) instance returns the
    // uniform invalid-transition diagnostic and a non-zero exit.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "nat", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let run = run_kt_agent(&["agent", "pause", "nat"], &ctx.project_dir, state_dir);
    assert!(!run.success, "pause on registered should exit non-zero");
    assert!(
        run.stderr.contains("cannot pause"),
        "uniform invalid-transition; stderr={}",
        run.stderr
    );
}

// ---- Story 1-6: restart count / failed cause / policy surface (AC9) + restart ----

/// Seed a `failed` instance with an `agent_runtime` record carrying a Restart
/// Policy, a restart count, and a last-known (failed) cause — directly in the
/// engine's SQLite DB. The record's pid is 0 (a policy/status seed, NOT a live
/// process), so the engine's orphan adoption on open skips it and the row stays
/// `failed`. This is how AC9's CLI surface is exercised without a real crash.
fn seed_failed_with_record(state_dir: &Path, name: &str, policy: &str, count: u32, cause: &str) {
    let conn = rusqlite::Connection::open(state_db(state_dir)).expect("open state db");
    let affected = conn
        .execute(
            "UPDATE agent_instances SET state = 'failed' WHERE name = ?1",
            [name],
        )
        .expect("update state to failed");
    assert_eq!(affected, 1, "expected to update exactly one row");
    let id: i64 = conn
        .query_row(
            "SELECT id FROM agent_instances WHERE name = ?1",
            [name],
            |r| r.get(0),
        )
        .expect("instance id");
    conn.execute(
        "INSERT INTO agent_runtime \
         (instance_id, pid, start_time, restart_policy, restart_count, last_known_cause) \
         VALUES (?1, 0, 0, ?2, ?3, ?4)",
        rusqlite::params![id, policy, count as i64, cause],
    )
    .expect("insert agent_runtime record");
}

#[test]
fn list_surfaces_the_restart_count_column() {
    // AC9: `kt agent list` surfaces the per-instance restart count.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "svc", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    seed_failed_with_record(state_dir, "svc", "on-failure", 3, "crashed with code 1");

    let list = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(list.success, "list should exit 0; stderr={}", list.stderr);
    // The Restarts column header + the seeded count 3 are rendered (stdout).
    assert!(
        list.stdout.contains("Restarts"),
        "list must have a Restarts column; stdout={}",
        list.stdout
    );
    assert!(
        list.stdout.contains('3'),
        "list must show the restart count; stdout={}",
        list.stdout
    );
    // The failed state is shown too.
    assert!(list.stdout.contains("failed"), "stdout={}", list.stdout);
}

#[test]
fn show_surfaces_restart_count_policy_and_failed_cause() {
    // AC9: `kt agent show` on a failed instance surfaces the restart count, the
    // active Restart Policy, and the failed cause (result → stdout).
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "svc", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    seed_failed_with_record(
        state_dir,
        "svc",
        "on-failure",
        5,
        "crash-loop: 5 consecutive failures reached",
    );

    let show = run_kt_agent(&["agent", "show", "svc"], &ctx.project_dir, state_dir);
    assert!(show.success, "show should exit 0; stderr={}", show.stderr);
    // Runtime status block: state + policy + count.
    assert!(
        show.stdout.contains("Runtime status"),
        "show must render runtime status; stdout={}",
        show.stdout
    );
    assert!(
        show.stdout.contains("on-failure"),
        "policy; stdout={}",
        show.stdout
    );
    assert!(
        show.stdout.contains('5'),
        "restart count; stdout={}",
        show.stdout
    );
    // The failed cause (crash-loop reason) is surfaced.
    assert!(
        show.stdout.contains("crash-loop"),
        "show must surface the failed cause; stdout={}",
        show.stdout
    );
}

#[test]
fn show_surfaces_a_launch_error_failed_cause() {
    // F-Med-3 (AC9): a LAUNCH-ERROR `failed` instance has no write-ahead spawn
    // record (the `starting→failed` launch error returns before the record is
    // written), yet `kt agent show` must still surface the failed cause — via the
    // engine's event-log fallback. Register a manifest whose exec does not exist,
    // start it (fails to launch), then `show` must print the launch diagnostic.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = ctx.project_dir.join("bad-adapter");
    std::fs::create_dir_all(&m).unwrap();
    let body = r#"
contract_version = "0.1.0"

[adapter]
kind = "bad"

[lifecycle.start]
exec = "ktesio-no-such-binary-cli-med3"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#;
    std::fs::write(m.join("adapter.toml"), body).unwrap();
    run_kt_agent(
        &[
            "agent",
            "register",
            "bad",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    // Start fails to launch (exit non-zero) and lands the instance `failed`.
    let start = run_kt_agent(&["agent", "start", "bad"], &ctx.project_dir, state_dir);
    assert!(!start.success, "start of a bad exec should exit non-zero");

    let show = run_kt_agent(&["agent", "show", "bad"], &ctx.project_dir, state_dir);
    assert!(show.success, "show should exit 0; stderr={}", show.stderr);
    // The runtime status shows `failed`, and the failed cause (the preserved
    // launch diagnostic naming the missing exec) is surfaced on stdout.
    assert!(show.stdout.contains("failed"), "stdout={}", show.stdout);
    assert!(
        show.stdout.contains("ktesio-no-such-binary-cli-med3"),
        "show must surface the launch-error failed cause (AC9); stdout={}",
        show.stdout
    );
}

#[test]
fn start_restarts_a_failed_instance() {
    // AC3 at the CLI: `kt agent start` restarts a `failed` instance (the 1-6
    // transition row `failed → starting` permits it). Seed a `failed` instance
    // backed by a real `fake_agent` manifest, then start it → running.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    // Register a manifest instance whose exec is the real fake_agent (lingers).
    let manifest_dir = TestContext::new();
    let bin = ktesio_conformance::fake_agent_bin();
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "svc"

[lifecycle.start]
exec = {exec:?}
args = ["--linger-ms", "600000"]

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
    );
    std::fs::write(manifest_dir.project_dir.join("adapter.toml"), body).unwrap();
    let manifest_path = manifest_dir.project_dir.to_string_lossy().to_string();
    run_kt_agent(
        &["agent", "register", "svc", "--manifest", &manifest_path],
        &ctx.project_dir,
        state_dir,
    );
    // Force it to `failed` (no record needed for the transition; policy defaults).
    let conn = rusqlite::Connection::open(state_db(state_dir)).unwrap();
    conn.execute(
        "UPDATE agent_instances SET state = 'failed' WHERE name = 'svc'",
        [],
    )
    .unwrap();
    drop(conn);

    // `kt agent start` on a failed instance restarts it → running (exit 0).
    let start = run_kt_agent(&["agent", "start", "svc"], &ctx.project_dir, state_dir);
    assert!(
        start.success,
        "start on a failed instance should restart it (exit 0); stderr={}",
        start.stderr
    );
    assert!(start.stdout.contains("running"), "stdout={}", start.stdout);

    // Teardown: stop it so the process does not linger.
    run_kt_agent(&["agent", "stop", "svc"], &ctx.project_dir, state_dir);
}
