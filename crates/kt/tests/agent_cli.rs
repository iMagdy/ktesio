//! Integration tests for `kt agent register | remove | list`.
//!
//! These drive the real `kt` binary (via `CARGO_BIN_EXE_kt`) with
//! `KTESIO_STATE_DIR` pinned to a `TempDir`, so no test ever touches the real
//! user data dir. They assert the CLI contract: exit codes, the Agent Home
//! path on stdout, and diagnostics on stderr.

mod helpers;

use std::path::Path;

use helpers::{run_kt_agent, run_kt_agent_with_env, TestContext};

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
    // Runtime-skip on Windows (data-driven OS id, NO `#[cfg]` — this file is
    // outside the backends allowlist). This test drives the story-1-6 cross-
    // process adoption harness (`start_via_surviving_engine`): a subprocess
    // starts the agent and exits WITHOUT a graceful stop so the child re-parents
    // and survives, then a separate `kt` command adopts it live. That survival
    // relies on Unix re-parenting to init; on Windows JOB_OBJECT_LIMIT_KILL_ON_
    // JOB_CLOSE kills the child when the helper exits, so the next `Engine::open`
    // adoption reconciles the row to `failed` and pause can't run. Cross-lifetime
    // survival genuinely can't be simulated on Windows (consistent with the
    // engine's documented single-lifetime behavior); the pause/resume SEMANTICS
    // are fully covered on Windows by `crates/ktesio-engine/tests/pause.rs`.
    if ktesio_engine::OsId::current() == ktesio_engine::OsId::Windows {
        return;
    }
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
    // Runtime-skip on Windows (data-driven OS id, NO `#[cfg]` — this file is
    // outside the backends allowlist). Like the best-effort case above, this test
    // relies on the story-1-6 cross-process adoption harness
    // (`start_via_surviving_engine`) to make the instance genuinely `running`
    // before pause runs. On Windows JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE kills the
    // survivor when the helper subprocess exits, so the next `Engine::open`
    // reconciles the row to `failed` and pause fails with a reconciled-to-failed
    // error instead of the intended UNSUPPORTED diagnostic. Cross-lifetime
    // survival can't be simulated on Windows; the pause semantics (including the
    // unsupported projection) are covered by `crates/ktesio-engine/tests/pause.rs`.
    if ktesio_engine::OsId::current() == ktesio_engine::OsId::Windows {
        return;
    }
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

#[test]
fn list_json_emits_a_parseable_document_with_budget_seed_and_real_usage() {
    // Story 1-7 (AC5/AC9) + story 3-1 (AC-C/AC11): `kt agent list --json` writes ONE
    // parseable JSON document to stdout (and NOTHING non-JSON there), carrying a
    // top-level schema_version + per-instance objects whose `budget` is the honest
    // JSON `null` seed (budgets are 3-2) while `usage` is a REAL token-totals object
    // (zeros for a never-metered instance, not null) and `metering_source` is
    // surfaced. The metering note is on stderr.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "alpha", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let run = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(
        run.success,
        "list --json should exit 0; stderr={}",
        run.stderr
    );

    // stdout is PURE JSON and re-parses (nothing else on stdout, AC9/AD-12).
    let doc: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{}", run.stdout));
    // Story 3-5 bumped the Fleet document version 1 → 2 (additive: `totals` gained).
    assert_eq!(doc["schema_version"], serde_json::json!(2), "{doc}");
    let instances = doc["instances"].as_array().expect("instances array");
    assert_eq!(instances.len(), 1);
    // Story 3-5: a top-level `totals` object aggregates the rows. A never-metered
    // Fleet → zero tokens, dollars honestly absent (never a fabricated $0), not partial.
    assert!(doc["totals"].is_object(), "totals present: {doc}");
    assert_eq!(doc["totals"]["total_input_tokens"], serde_json::json!(0));
    assert_eq!(doc["totals"]["total_output_tokens"], serde_json::json!(0));
    assert!(
        doc["totals"].get("total_dollars").is_none(),
        "no Rate anywhere ⇒ no dollar total: {doc}"
    );
    assert_eq!(doc["totals"]["dollars_partial"], serde_json::json!(false));
    let entry = &instances[0];
    assert_eq!(entry["name"], serde_json::json!("alpha"));
    assert_eq!(entry["kind"], serde_json::json!("mock"));
    assert_eq!(entry["state"], serde_json::json!("registered"));
    assert_eq!(entry["restart_count"], serde_json::json!(0));
    // budget: the honest JSON null seed, NOT 0, NOT a fabricated number.
    assert_eq!(entry["budget"], serde_json::Value::Null, "{entry}");
    assert_ne!(entry["budget"], serde_json::json!(0));
    // usage: a real object with zero token totals (story 3-1) — NOT null.
    assert!(
        entry["usage"].is_object(),
        "usage must be an object: {entry}"
    );
    assert_eq!(
        entry["usage"]["cumulative_input_tokens"],
        serde_json::json!(0)
    );
    assert_eq!(
        entry["usage"]["cumulative_output_tokens"],
        serde_json::json!(0)
    );
    // The active Metering Source is surfaced (AC-C).
    assert_eq!(entry["metering_source"], serde_json::json!("self-reported"));
    // No dollar figure anywhere (tokens only — AD-8).
    assert!(entry["usage"].get("cost").is_none(), "{entry}");
    assert!(entry.get("agent_home").is_some());

    // The metering note rides on stderr (never stdout), so stdout stays valid JSON.
    assert!(
        run.stderr.contains("Usage Ledger"),
        "the metering note must be on stderr; stderr={}",
        run.stderr
    );
    assert!(
        !run.stdout.contains("Usage Ledger"),
        "stdout must be pure JSON (no note); stdout={}",
        run.stdout
    );
}

#[test]
fn list_json_on_empty_fleet_is_a_valid_empty_document() {
    // AC9: an empty Fleet with --json is still valid JSON — an empty `instances`
    // array — and the "no instances" guidance goes to STDERR so stdout parses.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let run = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(
        run.success,
        "empty list --json should exit 0; stderr={}",
        run.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{}", run.stdout));
    assert_eq!(doc["instances"], serde_json::json!([]), "{doc}");
    assert_eq!(doc["schema_version"], serde_json::json!(2));
    // Story 3-5: an empty Fleet still carries a valid all-zero / absent-dollars totals.
    assert_eq!(doc["totals"]["total_input_tokens"], serde_json::json!(0));
    assert!(doc["totals"].get("total_dollars").is_none(), "{doc}");
    // The "no instances" guidance is on stderr (so stdout is pure JSON).
    assert!(
        run.stderr.contains("No Agent Instances"),
        "empty --json guidance must be on stderr; stderr={}",
        run.stderr
    );
}

#[test]
fn human_list_shows_the_budget_column_and_real_usage_columns() {
    // Story 1-7 (AC4) + story 3-1/3-2 (AC-C/AC9): the human `list` renders a Budget
    // (tokens) column — the honest `—` for an UN-budgeted instance — AND a real
    // Usage (tokens) column; the metering note is on stderr.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "alpha", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let run = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(run.success, "list should exit 0; stderr={}", run.stderr);
    // The columns are present (headers): a Budget column (story 3-3 renamed it from
    // "Budget (tokens)" to "Budget (tok, est. $)") + Usage (real). The header may
    // truncate on a narrow terminal, so match a stable prefix.
    assert!(run.stdout.contains("Budget"), "stdout={}", run.stdout);
    assert!(run.stdout.contains("Usage"), "stdout={}", run.stdout);
    // An un-budgeted instance's budget cell renders the honest `—` absence.
    assert!(
        run.stdout.contains('—'),
        "the un-budgeted cell must render the em dash; stdout={}",
        run.stdout
    );
    // The metering note is on stderr (AD-12), never stdout.
    assert!(run.stderr.contains("Usage Ledger"), "stderr={}", run.stderr);
}

#[test]
fn show_json_surfaces_the_same_entry_shape_with_budget_seed_and_real_usage() {
    // Story 1-7 (AC5) + story 3-1: `kt agent show <name> --json` writes ONE JSON
    // document to stdout: { schema_version, instance: <the same FleetEntry shape> },
    // `budget` the honest null seed and `usage` a real token-totals object. The
    // metering note is on stderr.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "alpha", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let run = run_kt_agent(
        &["agent", "show", "alpha", "--json"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        run.success,
        "show --json should exit 0; stderr={}",
        run.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{}", run.stdout));
    // The show document shares the bumped Fleet schema version (2) but — the recorded
    // asymmetry — does NOT gain a Fleet `totals` (a single instance has no Fleet total;
    // its own `usage` IS its total).
    assert_eq!(doc["schema_version"], serde_json::json!(2), "{doc}");
    assert!(
        doc.get("totals").is_none(),
        "show --json must NOT carry a Fleet totals object: {doc}"
    );
    let entry = &doc["instance"];
    assert_eq!(entry["name"], serde_json::json!("alpha"));
    assert_eq!(entry["budget"], serde_json::Value::Null, "{entry}");
    assert!(
        entry["usage"].is_object(),
        "usage must be an object: {entry}"
    );
    assert_eq!(
        entry["usage"]["cumulative_input_tokens"],
        serde_json::json!(0)
    );
    assert_eq!(entry["metering_source"], serde_json::json!("self-reported"));
    // stdout is pure JSON; the note is on stderr.
    assert!(
        !run.stdout.contains("Usage Ledger"),
        "stdout={}",
        run.stdout
    );
    assert!(run.stderr.contains("Usage Ledger"), "stderr={}", run.stderr);
}

#[test]
fn human_show_surfaces_the_budget_row_and_real_usage_rows() {
    // Story 1-7 (AC4) + story 3-1/3-2 (AC-C/AC9): human `show` renders a Budget
    // (tokens) row (the honest `—` for an un-budgeted instance) plus REAL Usage
    // (tokens) + Metering source rows in the runtime-status block, with the
    // metering note on stderr.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "alpha", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let run = run_kt_agent(&["agent", "show", "alpha"], &ctx.project_dir, state_dir);
    assert!(run.success, "show should exit 0; stderr={}", run.stderr);
    assert!(run.stdout.contains("Budget"), "stdout={}", run.stdout);
    assert!(run.stdout.contains("Usage"), "stdout={}", run.stdout);
    // The Metering source row + the self-reported value are surfaced (AC-C).
    assert!(
        run.stdout.contains("Metering source"),
        "stdout={}",
        run.stdout
    );
    assert!(
        run.stdout.contains("self-reported"),
        "stdout={}",
        run.stdout
    );
    // The un-budgeted budget row renders the honest `—` absence.
    assert!(run.stdout.contains('—'), "stdout={}", run.stdout);
    assert!(run.stderr.contains("Usage Ledger"), "stderr={}", run.stderr);
}

// ---- Story 3-3: dollar cost + Cost Cap rendering (AC-B/AC10 — AD-8) ----

#[test]
fn config_set_rejects_a_malformed_rate_value_with_a_diagnostic() {
    // AC11: a malformed Rate dollar value is rejected at WRITE time (never silently
    // defaulted; a bad value must not crash). The write fails with a diagnostic and
    // the config is unchanged.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    let bad = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "cost.rate.input",
            "three-dollars",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !bad.success,
        "a malformed Rate must be rejected; stdout={}",
        bad.stdout
    );
    assert!(
        bad.stderr.contains("cost.rate.input"),
        "the diagnostic must name the key; stderr={}",
        bad.stderr
    );
    // Sub-micro precision is likewise rejected.
    let submicro = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "budget.dollars.cumulative",
            "5.0000001",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !submicro.success,
        "sub-micro precision must be rejected; stdout={}",
        submicro.stdout
    );
}

#[test]
fn rate_and_cap_render_labeled_dollars_in_list_json_and_human() {
    // AC10/AC-B: a Rate'd + capped instance surfaces the DOLLAR cost + cap in
    // `list --json` as INTEGER MICROS + the estimate label (NO `$` string on the
    // wire — AD-14), and in the human table THROUGH the single currency module
    // (a `$X.XX (estimated)` cell — AD-8). Even with zero usage, a Rate'd instance
    // shows `$0.00 (estimated)` (an honest labeled zero, distinct from the no-Rate
    // inert absence).
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    // Both directions → a supplied Rate; a cumulative dollar cap.
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "cost.rate.input",
            "3.00",
        ],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "cost.rate.output",
            "15.00",
        ],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "budget.dollars.cumulative",
            "5.00",
        ],
        &ctx.project_dir,
        state_dir,
    );

    // --json: integer micros + label, NO `$` string.
    let json = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(
        json.success,
        "list --json should exit 0; stderr={}",
        json.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&json.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{}", json.stdout));
    let entry = &doc["instances"][0];
    // The cumulative cost cap is $5.00 = 5_000_000 micros (an integer, not a string).
    assert_eq!(
        entry["budget"]["cumulative_cost_cap"],
        serde_json::json!(5_000_000),
        "the cap must be integer micros: {entry}"
    );
    // A Rate'd instance surfaces the derived cost ($0.00 at zero usage) + the label.
    assert_eq!(
        entry["usage"]["cumulative_dollars"],
        serde_json::json!(0),
        "{entry}"
    );
    assert_eq!(
        entry["usage"]["estimate_label"],
        serde_json::json!("estimated"),
        "{entry}"
    );
    // NO pre-formatted `$` string anywhere in the JSON document (AD-14).
    assert!(
        !json.stdout.contains('$'),
        "no `$` string on the wire; stdout={}",
        json.stdout
    );

    // Human table: the dollar cap cell rendered THROUGH the currency module shows a
    // `$` figure. (The narrow `list` Budget column may TRUNCATE the trailing
    // `(estimated)` label; the untruncated `show` surface asserts the label — see
    // `show_of_a_rated_instance_surfaces_a_labeled_cost_row`.)
    let human = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(human.success, "list should exit 0; stderr={}", human.stderr);
    assert!(
        human.stdout.contains('$'),
        "the human dollar cell must render a `$` figure; stdout={}",
        human.stdout
    );
}

#[test]
fn list_budget_dollar_label_lives_in_the_header_not_the_truncatable_cell() {
    // FR-23/AD-8 (primary review L1): the human `list` Budget column is NARROW and
    // truncates its cell with `…`. Rendering the estimate qualifier INLINE in the
    // cell (`… $0.50 (estimated) …`) would let truncation CHOP `(estimated)` and
    // leave a human staring at a bare, UNLABELED dollar. The fix moves the qualifier
    // OUT of the cell and into the COLUMN HEADER ("est. $"): the cell renders the
    // dollar value BARE (via render_dollars_bare), so there is no inline estimate
    // label in the cell to mangle, and the header carries the label instead.
    //
    // COLUMNS=110 is chosen so the Budget header + cell BOTH render in full while the
    // other columns (Usage / Agent Home) truncate — the truncation pressure is real,
    // yet the Budget column is the one under test and is fully observable.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    // A supplied Rate (both directions) + a cumulative dollar cap → the Budget cell
    // renders a `$` cost-cap figure.
    for (key, value) in [
        ("cost.rate.input", "3.00"),
        ("cost.rate.output", "15.00"),
        ("budget.dollars.cumulative", "5.00"),
    ] {
        run_kt_agent(
            &["agent", "config", "set", "priced", key, value],
            &ctx.project_dir,
            state_dir,
        );
    }

    let human = run_kt_agent_with_env(
        &["agent", "list"],
        &ctx.project_dir,
        state_dir,
        &[("COLUMNS", "110")],
    );
    assert!(human.success, "list should exit 0; stderr={}", human.stderr);

    // (1) The estimate qualifier lives in the HEADER ("est. $") — the stable home of
    // the dollar label on this truncatable surface (FR-23).
    assert!(
        human.stdout.contains("est. $"),
        "the Budget header must carry the estimate qualifier 'est. $'; stdout=\n{}",
        human.stdout
    );
    // (2) The header is NOT the stale "Budget (tokens)" mislabel — the column now
    // also renders an ESTIMATED dollar Cost Cap, so "(tokens)" would misdescribe it.
    assert!(
        !human.stdout.contains("Budget (tokens)"),
        "the header must not be the stale 'Budget (tokens)' mislabel; stdout=\n{}",
        human.stdout
    );
    // (3) The Budget cell renders BARE dollar figures whose only trailing
    // parenthetical is the Breach Action `(pause)` — NEVER an inline `(estimated)`
    // (which truncation could mangle into `(esti…` glued to a dollar). The exact
    // `cum $5.00/$5.00 (pause)` cell is proof: the `$` cap + remaining are bare, the
    // estimate label is not in the cell at all, so no mangled label can appear here.
    assert!(
        human.stdout.contains("cum $5.00/$5.00 (pause)"),
        "the Budget cell must render bare dollars + only the breach action (no inline \
         estimate label to truncate); stdout=\n{}",
        human.stdout
    );

    // (4) Belt-and-suspenders: the estimate qualifier is ALSO carried by the
    // always-present, never-truncated stderr metering note ("labeled estimates"),
    // which covers every rendered dollar on the surface regardless of terminal width.
    assert!(
        human.stderr.contains("labeled estimates"),
        "the stderr metering note must state dollar figures are labeled estimates; \
         stderr=\n{}",
        human.stderr
    );
}

#[test]
fn show_of_a_rated_instance_surfaces_a_labeled_cost_row() {
    // AC10: human `show` of a Rate'd instance renders a `Cost (estimated)` row with
    // a labeled dollar figure THROUGH the currency module.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "cost.rate.input",
            "3.00",
        ],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "cost.rate.output",
            "15.00",
        ],
        &ctx.project_dir,
        state_dir,
    );
    // A cumulative dollar cap too, so the Budget row renders a dollar Cost Cap pair.
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "priced",
            "budget.dollars.cumulative",
            "5.00",
        ],
        &ctx.project_dir,
        state_dir,
    );

    let show = run_kt_agent(&["agent", "show", "priced"], &ctx.project_dir, state_dir);
    assert!(show.success, "show should exit 0; stderr={}", show.stderr);
    assert!(
        show.stdout.contains("Cost (estimated)"),
        "show must render a Cost row; stdout={}",
        show.stdout
    );
    assert!(
        show.stdout.contains("$0.00 (estimated)"),
        "the Cost row must show a labeled dollar figure; stdout={}",
        show.stdout
    );
    // The `show` Value column is WIDE (no truncation), so its Budget row labels the
    // dollar Cost Cap INLINE (DollarLabel::Inline) — the full `remaining/cap (estimated)`
    // pair. At zero usage the remaining equals the $5.00 cap, so the pair is
    // `$5.00/$5.00 (estimated)`. This is the un-truncated companion to the `list`
    // truncation test, where the same label instead lives in the column header.
    assert!(
        show.stdout.contains("$5.00/$5.00 (estimated)"),
        "the `show` Budget row must render the inline-labeled dollar cap pair; \
         stdout={}",
        show.stdout
    );
}

#[test]
fn show_of_a_no_rate_instance_says_dollar_features_are_inert() {
    // AC-B: with NO Rate, dollar features are INERT and SAY SO — the human `show`
    // Cost row is an honest "no Rate configured — dollar features inert" note, never
    // a fabricated `$0.00`. Token features (the Budget/Usage rows) still render.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "norate", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let show = run_kt_agent(&["agent", "show", "norate"], &ctx.project_dir, state_dir);
    assert!(show.success, "show should exit 0; stderr={}", show.stderr);
    assert!(
        show.stdout.contains("dollar features inert"),
        "with no Rate, the Cost row must say dollar features are inert; stdout={}",
        show.stdout
    );
    // No fabricated dollar figure for a no-Rate instance.
    assert!(
        !show.stdout.contains("$0.00"),
        "a no-Rate instance must NOT fabricate a $0.00 cost; stdout={}",
        show.stdout
    );
    // Token features still work: the Usage row is present.
    assert!(show.stdout.contains("Usage"), "stdout={}", show.stdout);
}

// ---- Story 3-5: the Fleet-wide total (footer + `list --json` totals) ----

#[test]
fn list_json_carries_a_fleet_totals_object_bumped_to_schema_2() {
    // Story 3-5 (AC-A/AC9): `kt agent list --json` carries a top-level `totals` object
    // and the Fleet document version is bumped 1 → 2 (additive). With one Rate'd
    // instance at zero usage, the token totals are 0 and the dollar total is a labeled
    // $0 (a Rate exists ⇒ nothing partial). Integer micros + label on the wire; NO `$`.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    for (key, value) in [("cost.rate.input", "3.00"), ("cost.rate.output", "15.00")] {
        run_kt_agent(
            &["agent", "config", "set", "priced", key, value],
            &ctx.project_dir,
            state_dir,
        );
    }

    let run = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(
        run.success,
        "list --json should exit 0; stderr={}",
        run.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{}", run.stdout));
    assert_eq!(doc["schema_version"], serde_json::json!(2), "{doc}");
    let totals = &doc["totals"];
    assert!(totals.is_object(), "totals present: {doc}");
    assert_eq!(totals["total_input_tokens"], serde_json::json!(0));
    assert_eq!(totals["total_output_tokens"], serde_json::json!(0));
    // A Rate exists ⇒ a labeled $0 dollar total (integer micros), not partial, not absent.
    assert_eq!(totals["total_dollars"], serde_json::json!(0), "{doc}");
    assert_eq!(totals["estimate_label"], serde_json::json!("estimated"));
    assert_eq!(totals["dollars_partial"], serde_json::json!(false));
    // NO `$` string on the wire (AD-14).
    assert!(
        !run.stdout.contains('$'),
        "no `$` on the wire; stdout={}",
        run.stdout
    );
}

#[test]
fn list_json_totals_carry_the_partial_flag_field() {
    // AC5 (the honesty crux) SHAPE via the CLI: a Fleet mixing a Rate'd instance with a
    // no-Rate instance carries the `dollars_partial` field in `list --json`. With zero
    // usage NEITHER instance is metered yet, so the total is a complete labeled $0
    // (`dollars_partial == false`) — this test therefore asserts the field is PRESENT +
    // the document parses, NOT that it is `true`.
    //
    // The metered-partial arithmetic that flips the flag to `true` (and the "N unpriced"
    // count) is proven exactly in the pure unit tests (`fleet.rs`, `agent.rs` footer
    // tests) + the engine `totals == ledger` integration test — a rename here keeps this
    // test's name honest about what it checks rather than over-promising `true`.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "register", "free", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    for (key, value) in [("cost.rate.input", "3.00"), ("cost.rate.output", "15.00")] {
        run_kt_agent(
            &["agent", "config", "set", "priced", key, value],
            &ctx.project_dir,
            state_dir,
        );
    }

    let run = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(run.success, "stderr={}", run.stderr);
    let doc: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    // Two instances aggregated; the `priced` one has a Rate (dollar total present +
    // labeled), the `free` one does not — but with zero tokens neither is metered, so
    // the total is a complete labeled $0 (not partial). The shape is what we assert.
    assert_eq!(doc["instances"].as_array().unwrap().len(), 2);
    assert_eq!(doc["totals"]["total_dollars"], serde_json::json!(0));
    assert_eq!(
        doc["totals"]["estimate_label"],
        serde_json::json!("estimated")
    );
    assert!(doc["totals"].get("dollars_partial").is_some(), "{doc}");
}

#[test]
fn human_list_renders_the_fleet_total_footer() {
    // AC-A/AC-B: the human `kt agent list` renders a Fleet-wide total footer on stdout.
    // A Rate'd instance ⇒ the footer carries a labeled dollar total THROUGH the currency
    // module (a `$` figure + the estimate label); the footer names the token totals too.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "priced", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    for (key, value) in [("cost.rate.input", "3.00"), ("cost.rate.output", "15.00")] {
        run_kt_agent(
            &["agent", "config", "set", "priced", key, value],
            &ctx.project_dir,
            state_dir,
        );
    }

    let run = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(run.success, "list should exit 0; stderr={}", run.stderr);
    // The footer is on stdout (command output, AD-12).
    assert!(
        run.stdout.contains("Fleet total:"),
        "the human list must render a Fleet total footer; stdout=\n{}",
        run.stdout
    );
    // The dollar total rides through the currency module (a `$` figure) + is labeled.
    assert!(
        run.stdout.contains('$'),
        "labeled dollar total; stdout=\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("estimated"),
        "the Fleet total dollar figure must carry its estimate label; stdout=\n{}",
        run.stdout
    );
}

#[test]
fn human_list_footer_no_rate_shows_dash_not_zero_dollars() {
    // AC4/AC5: with NO instance Rate'd, the footer shows the token totals + an honest
    // `—` dollar marker, NEVER a fabricated `$0.00` (dollars are not derivable). The
    // per-instance rows stay honest too (tokens only).
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "norate", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let run = run_kt_agent(&["agent", "list"], &ctx.project_dir, state_dir);
    assert!(run.success, "list should exit 0; stderr={}", run.stderr);
    assert!(
        run.stdout.contains("Fleet total:"),
        "stdout=\n{}",
        run.stdout
    );
    // No fabricated $0.00 Fleet total (no Rate ⇒ dollars honestly absent).
    assert!(
        !run.stdout.contains("$0.00"),
        "a no-Rate Fleet must not fabricate a $0.00 total; stdout=\n{}",
        run.stdout
    );
    // The footer line itself carries the honest absent-dollars marker.
    assert!(
        run.stdout.contains("dollars not derived"),
        "the footer must say dollars are not derived (no Rate); stdout=\n{}",
        run.stdout
    );
}

// ---- Story 2-1: `kt agent config set` / `get` (AC10, AC-B, AC7, AD-12) ----

#[test]
fn config_set_then_get_shows_the_value_on_stdout() {
    // AC10/AC-A end-to-end: set a known key, then `config get <name> <key>`
    // prints the effective value to stdout (bare, no quotes) and exits 0.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let set = run_kt_agent(
        &["agent", "config", "set", "demo", "model", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(set.success, "set should exit 0; stderr={}", set.stderr);
    assert!(set.stdout.contains("Set"), "stdout={}", set.stdout);

    // Single-key get: the bare value on stdout.
    let get = run_kt_agent(
        &["agent", "config", "get", "demo", "model"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    assert!(
        get.stdout.lines().any(|l| l.trim() == "gpt-4"),
        "stdout should print the bare value; stdout={}",
        get.stdout
    );

    // Whole-config get: a Key/Value table on stdout, provenance note on stderr.
    let get_all = run_kt_agent(
        &["agent", "config", "get", "demo"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        get_all.success,
        "get-all should exit 0; stderr={}",
        get_all.stderr
    );
    assert!(
        get_all.stdout.contains("model"),
        "stdout should list the key; stdout={}",
        get_all.stdout
    );
    // Story 2-3: the stale "provenance arrives in Epic 2.3" deferral note is
    // RETIRED (the Source column now IS the provenance) — it must appear NOWHERE.
    assert!(
        !get_all.stderr.contains("Epic 2.3") && !get_all.stdout.contains("Epic 2.3"),
        "the stale Epic 2.3 deferral note must be gone; stdout={} stderr={}",
        get_all.stdout,
        get_all.stderr
    );
    // The provenance now rides on STDOUT as a "Source" column (result → stdout).
    assert!(
        get_all.stdout.contains("Source") && get_all.stdout.contains("instance"),
        "config get must show a Source column with the instance layer; stdout={}",
        get_all.stdout
    );
}

#[test]
fn config_get_effective_is_empty_before_any_set() {
    // AC-C / review decision #1: engine + kind defaults ship EMPTY in 2-1, so
    // before any `set` the effective config has no keys — `config get` exits 0
    // and prints the "no config keys set" info line (never a fabricated default).
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    assert!(
        get.stdout.contains("no config keys set"),
        "empty effective config should say so; stdout={}",
        get.stdout
    );
    // The seeded identity key `name` is NOT surfaced (patch #4).
    assert!(
        !get.stdout.contains("name"),
        "identity key must not appear; stdout={}",
        get.stdout
    );
}

#[test]
fn config_set_instance_value_is_read_back() {
    // AC-A/AC10: a key set at the INSTANCE layer is read back by `config get`.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "claude-opus"],
        &ctx.project_dir,
        state_dir,
    );
    let get = run_kt_agent(
        &["agent", "config", "get", "demo", "model"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        get.stdout.lines().any(|l| l.trim() == "claude-opus"),
        "instance value should read back; stdout={}",
        get.stdout
    );
}

#[test]
fn config_set_child_under_scalar_fails_closed() {
    // Review patch #3 end-to-end: nesting a child under an existing scalar is
    // rejected (non-zero exit) naming the conflict; nothing persisted.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "agent.a", "v1"],
        &ctx.project_dir,
        state_dir,
    );
    let config_path = state_dir.join("agents").join("demo").join("config.toml");
    let before = std::fs::read(&config_path).unwrap();

    let run = run_kt_agent(
        &["agent", "config", "set", "demo", "agent.a.b", "v2"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !run.success,
        "shape conflict must exit non-zero; stdout={}",
        run.stdout
    );
    assert!(
        run.stderr.contains("agent.a"),
        "stderr should name the conflicting ancestor; stderr={}",
        run.stderr
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        before,
        "failed write must leave config byte-unchanged"
    );
}

#[test]
fn config_set_unknown_key_is_rejected_with_suggestion_and_config_unchanged() {
    // AC-B: an unknown key outside `agent.*` is rejected (non-zero exit), the
    // suggestion is on stderr, and the on-disk config.toml is byte-unchanged.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let config_path = state_dir.join("agents").join("demo").join("config.toml");
    let before = std::fs::read(&config_path).expect("read config before");

    // `modle` is a near-miss for the known key `model`.
    let run = run_kt_agent(
        &["agent", "config", "set", "demo", "modle", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !run.success,
        "unknown key must exit non-zero; stdout={}",
        run.stdout
    );
    assert!(
        run.stderr.contains("modle"),
        "stderr should name the offending key; stderr={}",
        run.stderr
    );
    assert!(
        run.stderr.contains("model"),
        "stderr should suggest the nearest key; stderr={}",
        run.stderr
    );

    // AC-B atomicity: nothing persisted — config byte-unchanged.
    let after = std::fs::read(&config_path).expect("read config after");
    assert_eq!(before, after, "rejected write must not touch config.toml");
}

#[test]
fn config_set_agent_pass_through_key_round_trips_verbatim() {
    // AC7: an `agent.*` key writes successfully and round-trips verbatim.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let set = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "agent.custom_flag",
            "verbatim-value",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        set.success,
        "agent.* set should exit 0; stderr={}",
        set.stderr
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo", "agent.custom_flag"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        get.stdout.lines().any(|l| l.trim() == "verbatim-value"),
        "pass-through key must round-trip verbatim; stdout={}",
        get.stdout
    );
}

#[test]
fn config_get_unknown_instance_exits_nonzero() {
    // A `get` on an unregistered instance is the uniform not-found diagnostic on
    // stderr with a non-zero exit.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    let run = run_kt_agent(
        &["agent", "config", "get", "ghost"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !run.success,
        "get on a ghost must exit non-zero; stdout={}",
        run.stdout
    );
    assert!(
        run.stderr.contains("ghost"),
        "stderr should name the missing instance; stderr={}",
        run.stderr
    );
}

// ---- Story 2-2: the `agent.*`-unvalidated marker in `config get` (AC-B/AC7) ----

#[test]
fn config_get_marks_agent_pass_through_leaf_unvalidated_and_known_key_validated() {
    // AC-B / AC7 at the CLI: `config get <name>` renders a "Validated" marker per
    // row — an `agent.*` leaf is marked `unvalidated` (it bypassed known-key
    // validation), while a KNOWN key (`model`) is marked `validated`. The marker
    // rides on STDOUT (part of the result table, AD-12). Story 2-3 ADDS the
    // "Source" column beside it and RETIRES the stale Epic 2.3 note.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    // A known key (validated) and an agent.* pass-through key (unvalidated).
    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "agent.custom_flag", "on"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    // The Validated column header is present, and the unvalidated marker appears
    // for the agent.* leaf (result → stdout).
    assert!(
        get.stdout.contains("Validated"),
        "config get must have a Validated column; stdout={}",
        get.stdout
    );
    assert!(
        get.stdout.contains("unvalidated"),
        "the agent.* leaf must be marked unvalidated; stdout={}",
        get.stdout
    );
    // Both keys are listed.
    assert!(get.stdout.contains("model"), "stdout={}", get.stdout);
    assert!(
        get.stdout.contains("agent.custom_flag"),
        "stdout={}",
        get.stdout
    );
    // Story 2-3: the "Source" column now sits beside "Validated" on STDOUT, and
    // the stale Epic 2.3 deferral note is gone from both streams.
    assert!(
        get.stdout.contains("Source"),
        "config get must show a Source column beside Validated; stdout={}",
        get.stdout
    );
    assert!(
        !get.stderr.contains("Epic 2.3") && !get.stdout.contains("Epic 2.3"),
        "the stale Epic 2.3 deferral note must be gone; stdout={} stderr={}",
        get.stdout,
        get.stderr
    );
}

#[test]
fn config_get_known_key_only_shows_no_unvalidated_marker() {
    // AC-B companion: with ONLY a known key set (no agent.* leaf), `config get`
    // shows the value marked `validated` and NO `unvalidated` marker anywhere —
    // the marker is present only for pass-through leaves.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    // No `unvalidated` marker anywhere (the only key is a known one).
    assert!(
        !get.stdout.contains("unvalidated"),
        "a known-key-only config must show NO unvalidated marker; stdout={}",
        get.stdout
    );
    // The affirmative `validated` marker IS present — checked unambiguously (not
    // via a bare `contains("validated")`, which is a substring of "unvalidated").
    // Since no `unvalidated` occurs (asserted above), stripping it is a no-op here;
    // the check stays correct even if the two ever co-occur in some future row.
    assert!(
        get.stdout.replace("unvalidated", "").contains("validated"),
        "the known key must be marked validated (standalone token); stdout={}",
        get.stdout
    );
}

// ---- Story 2-3: per-value SOURCE-LAYER rendering (human + --json) (AC-A/AC3/AC4) ----

#[test]
fn config_get_human_shows_source_column_with_the_winning_layer() {
    // AC3 (the FR-13 heart): `config get <name>` gains a "Source" column naming
    // each leaf's winning layer. A key set at the instance layer shows source
    // `instance`; the column rides on STDOUT (result), and the stale Epic 2.3
    // deferral note is gone.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    // The Source column header + the `instance` label for the set key (STDOUT).
    assert!(
        get.stdout.contains("Source"),
        "config get must have a Source column; stdout={}",
        get.stdout
    );
    assert!(
        get.stdout.contains("instance"),
        "the instance-layer key must show source `instance`; stdout={}",
        get.stdout
    );
    // The stale deferral note is retired from both streams.
    assert!(
        !get.stdout.contains("Epic 2.3") && !get.stderr.contains("Epic 2.3"),
        "the stale Epic 2.3 note must be gone; stdout={} stderr={}",
        get.stdout,
        get.stderr
    );
}

#[test]
fn config_get_json_emits_source_per_leaf_on_stdout() {
    // AC4 + AD-12: `config get <name> --json` writes ONE parseable JSON document
    // to stdout — a versioned doc whose per-leaf objects carry { key, value,
    // source, unvalidated }. A known key set at the instance layer is
    // instance-sourced + validated; an agent.* leaf is unvalidated.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "gpt-4"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "agent.custom_flag", "on"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo", "--json"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        get.success,
        "config get --json should exit 0; stderr={}",
        get.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&get.stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON: {e}; stdout={}", get.stdout));
    assert_eq!(doc["schema_version"], serde_json::json!(1), "{doc}");
    let entries = doc["entries"].as_array().unwrap();

    let model = entries.iter().find(|e| e["key"] == "model").unwrap();
    assert_eq!(model["value"], serde_json::json!("gpt-4"));
    assert_eq!(model["source"], serde_json::json!("instance"));
    assert_eq!(model["unvalidated"], serde_json::json!(false));

    let flag = entries
        .iter()
        .find(|e| e["key"] == "agent.custom_flag")
        .unwrap();
    assert_eq!(flag["source"], serde_json::json!("instance"));
    assert_eq!(flag["unvalidated"], serde_json::json!(true));
    // Nothing but JSON on stdout (no note leaked there).
    assert!(
        !get.stdout.contains("Epic 2.3"),
        "stdout must stay pure JSON; stdout={}",
        get.stdout
    );
}

#[test]
fn config_get_single_key_json_emits_just_that_leaf() {
    // AC4: the single-key `config get <name> <key> --json` form emits exactly one
    // leaf with its value + source, matching the human single-key value.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "demo", "model", "claude-opus"],
        &ctx.project_dir,
        state_dir,
    );

    let get = run_kt_agent(
        &["agent", "config", "get", "demo", "model", "--json"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(get.success, "get should exit 0; stderr={}", get.stderr);
    let doc: serde_json::Value = serde_json::from_str(&get.stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON: {e}; stdout={}", get.stdout));
    let entries = doc["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "single-key --json emits one leaf; {doc}");
    assert_eq!(entries[0]["key"], serde_json::json!("model"));
    assert_eq!(entries[0]["value"], serde_json::json!("claude-opus"));
    assert_eq!(entries[0]["source"], serde_json::json!("instance"));
}

#[test]
fn config_get_persists_effective_config_snapshot_at_start() {
    // AC5 end-to-end through the CLI: starting an instance writes the
    // effective-config snapshot (effective-config.json) into the Agent Home,
    // carrying model=<v> tagged `instance`. Uses a live `fake_agent` manifest (the
    // builtin `mock` is inert — its start rejects before the snapshot write), and
    // reads the Agent Home path from `register`'s stdout (path authority — kt
    // never constructs it). The started process is cleaned up on this CLI exit
    // (kill-on-drop), so no separate stop is needed.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let m = fake_agent_manifest(&ctx.project_dir, &["--linger-ms", "600000"]);

    let reg = run_kt_agent(
        &[
            "agent",
            "register",
            "snapcli",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(reg.success, "register should exit 0; stderr={}", reg.stderr);
    // The registered Agent Home path is the stdout result line (an absolute path).
    let home = reg
        .stdout
        .lines()
        .find(|l| l.contains("agents") && l.contains("snapcli"))
        .unwrap_or_else(|| {
            panic!(
                "register stdout should name the home; stdout={}",
                reg.stdout
            )
        })
        .trim()
        .to_string();

    run_kt_agent(
        &["agent", "config", "set", "snapcli", "model", "gpt-4o"],
        &ctx.project_dir,
        state_dir,
    );
    let start = run_kt_agent(&["agent", "start", "snapcli"], &ctx.project_dir, state_dir);
    assert!(
        start.success,
        "start should exit 0; stderr={}",
        start.stderr
    );

    // The snapshot exists in the Agent Home and carries the resolved value +
    // provenance (written before the `starting` transition).
    let snapshot_path = std::path::Path::new(&home).join("effective-config.json");
    assert!(
        snapshot_path.is_file(),
        "the effective-config snapshot must exist at {snapshot_path:?}"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&snapshot_path).unwrap()).unwrap();
    assert_eq!(doc["schema_version"], serde_json::json!(1), "{doc}");
    let model = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "model")
        .unwrap();
    assert_eq!(model["value"], serde_json::json!("gpt-4o"));
    assert_eq!(model["source"], serde_json::json!("instance"));
}

// ============================================================================
// Story 2-4: SECRETS — the no-leak guarantee (FR-14, NFR-6, AD-10).
//
// The sentinel cleartext used across the matrix. If it ever appears in a
// no-leak surface (engine/instance log, event payload, config get --json, the
// snapshot), the guarantee is broken. It DOES appear in the adapter's native
// mechanism (the --dump file) and under `config get --reveal`.
// ============================================================================

const SECRET_SENTINEL: &str = "s3cr3t-sentinel-VALUE-xyz";

/// A `kt` run with ONE extra environment variable set on the child (so the env
/// SecretResolver can resolve `secret:NAME` at start). Mirrors `run_kt_agent` but
/// adds `env(key, value)`; the child `kt` process inherits it, and the engine it
/// spawns reads it via `std::env::var`. Local to the secret tests (the shared
/// helper does not need a general env knob).
fn run_kt_agent_env(
    args: &[&str],
    working_dir: &Path,
    state_dir: &Path,
    env_key: &str,
    env_val: &str,
) -> (bool, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kt"))
        .args(args)
        .current_dir(working_dir)
        .env("KTESIO_NO_UPDATE_CHECK", "1")
        .env("KTESIO_STATE_DIR", state_dir)
        .env(env_key, env_val)
        .output()
        .expect("Failed to execute kt");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Write a `fake_agent` manifest that (a) dumps its received argv + env to
/// `dump_path` at startup (`--dump`, the config-mapping observation point) and (b)
/// maps the unified `model` key into the native env var `MODEL` (`[config.model]
/// env = "MODEL"`). So a `model = "secret:NAME"` leaf, once resolved, lands in the
/// child's `MODEL` env — captured in the dump as `env=MODEL=<cleartext>`.
fn fake_agent_manifest_secret_env(dir: &Path, dump_path: &Path) -> std::path::PathBuf {
    let m = dir.join("fake-agent-secret-adapter");
    std::fs::create_dir_all(&m).unwrap();
    let bin = fake_agent_bin();
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "fake"

[lifecycle.start]
exec = {exec:?}
args = ["--linger-ms", "600000", "--dump", {dump:?}]

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"

[config.model]
env = "MODEL"
"#,
        exec = bin.to_string_lossy(),
        dump = dump_path.to_string_lossy(),
    );
    std::fs::write(m.join("adapter.toml"), body).unwrap();
    m
}

/// Recursively collect the text of every file under `dir` into one string (for the
/// no-leak sweep of an Agent Home / logs directory). Missing/binary files are
/// skipped. Also returns each scanned path so a failure names WHERE a leak was.
fn read_tree_text(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(read_tree_text(&path));
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            out.push((path, text));
        }
    }
    out
}

#[test]
fn secret_reaches_the_adapter_but_never_leaks_and_reveal_shows_it() {
    // THE no-leak matrix (AC-B, the security heart of FR-14/NFR-6), end-to-end
    // through the real `kt` binary:
    //   - a `secret:MODEL_KEY` leaf resolves (env resolver) to a sentinel cleartext;
    //   - the sentinel REACHES the adapter's native mechanism (env=MODEL=<sentinel>
    //     in the --dump file) — the value is USABLE (AC9 delivery);
    //   - the sentinel appears in NONE of: the persisted effective-config snapshot,
    //     `config get --json`, the human `config get`, or ANY file in the Agent Home
    //     (logs + event payloads included) — the MASK appears instead (AC-A/AC-B);
    //   - `config get --json --reveal` DOES carry the sentinel (AC-C, the sole
    //     un-mask), and the default `--json` carries the mask.
    //
    // Runtime-gate to Linux only (data-driven OS id, NO `#[cfg]` — this file is
    // outside the backends allowlist). The no-leak / masking logic this test
    // proves is OS-AGNOSTIC engine code: `ResolvedValue::display()` masking and
    // the snapshot / JSON serialization are identical on every OS. The
    // OS-SPECIFIC secret bit (the 0600 secrets-file permission check) already has
    // dedicated tests under `backends/{unix,windows}`. The reason this test can't
    // run everywhere is its POSITIVE-delivery half, which observes the sentinel
    // in the fake agent's `--dump`: that dump is written only AFTER the one-shot
    // `kt agent start` exits, and observing a one-shot-spawned agent is unreliable
    // on macOS + Windows CI. On macOS CI the agent never writes the dump at all
    // (the one-shot start leaves no observable running agent — regardless of
    // timeout); on Windows JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE kills the agent the
    // instant `kt` exits. The heavier `start_via_surviving_engine` harness isn't
    // warranted just to re-prove OS-agnostic masking. tarpaulin runs on Linux, so
    // the test still executes there and coverage is unchanged.
    if ktesio_engine::OsId::current() != ktesio_engine::OsId::Linux {
        return;
    }
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();

    // The dump file the agent writes its received env into (outside the state dir,
    // so the no-leak Agent-Home sweep does not scan the intended-cleartext dump).
    let dump = ctx.project_dir.join("agent-received.dump");
    let m = fake_agent_manifest_secret_env(&ctx.project_dir, &dump);

    // Register + set `model = secret:MODEL_KEY` (the reference is what is stored).
    let reg = run_kt_agent(
        &[
            "agent",
            "register",
            "sek",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(reg.success, "register failed; stderr={}", reg.stderr);
    let home = reg
        .stdout
        .lines()
        .find(|l| l.contains("agents") && l.contains("sek"))
        .expect("register stdout names the home")
        .trim()
        .to_string();
    let set = run_kt_agent(
        &["agent", "config", "set", "sek", "model", "secret:MODEL_KEY"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(set.success, "set failed; stderr={}", set.stderr);

    // Start with MODEL_KEY set in the environment → the env resolver resolves the
    // secret to the sentinel, which apply_config_mapping delivers into env MODEL.
    let (ok, out, err) = run_kt_agent_env(
        &["agent", "start", "sek"],
        &ctx.project_dir,
        state_dir,
        "MODEL_KEY",
        SECRET_SENTINEL,
    );
    assert!(ok, "start should succeed; stdout={out} stderr={err}");
    // Neither start's stdout nor stderr may carry the sentinel.
    assert!(
        !out.contains(SECRET_SENTINEL),
        "start stdout leaked the secret"
    );
    assert!(
        !err.contains(SECRET_SENTINEL),
        "start stderr leaked the secret"
    );

    // (POSITIVE) The sentinel REACHED the adapter's native env (the value is usable).
    let dump_text = {
        // The agent writes the dump at startup; poll briefly for it. This runs
        // Linux-only (see the gate above), where the one-shot-spawned agent
        // re-parents to init, survives `kt`'s exit, and reaches this within the
        // 5 s deadline. It is a wait for the write to APPEAR, not a fixed sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(t) = std::fs::read_to_string(&dump) {
                if t.contains("env=MODEL=") {
                    break t;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the agent never wrote its dump at {dump:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    assert!(
        dump_text.contains(&format!("env=MODEL={SECRET_SENTINEL}")),
        "the resolved cleartext must reach the adapter's native env; dump=\n{dump_text}"
    );

    // (NO-LEAK) The persisted snapshot masks the secret (never the sentinel).
    let snapshot_path = std::path::Path::new(&home).join("effective-config.json");
    let snapshot = std::fs::read_to_string(&snapshot_path).unwrap();
    assert!(
        !snapshot.contains(SECRET_SENTINEL),
        "the effective-config snapshot leaked the secret:\n{snapshot}"
    );
    assert!(
        snapshot.contains("secret:****"),
        "the snapshot must carry the mask; snapshot=\n{snapshot}"
    );

    // (NO-LEAK) Every file in the Agent Home — logs + event payloads included —
    // is free of the sentinel. This is the log + event-payload half of AC-B.
    for (path, text) in read_tree_text(std::path::Path::new(&home)) {
        assert!(
            !text.contains(SECRET_SENTINEL),
            "the secret leaked into an Agent Home file: {path:?}\n{text}"
        );
    }

    // (NO-LEAK) `config get --json` (default, no --reveal) masks the secret.
    let get_json = run_kt_agent(
        &["agent", "config", "get", "sek", "--json"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        get_json.success,
        "get --json failed; stderr={}",
        get_json.stderr
    );
    assert!(
        !get_json.stdout.contains(SECRET_SENTINEL),
        "config get --json leaked the secret by default:\n{}",
        get_json.stdout
    );
    assert!(
        get_json.stdout.contains("secret:****"),
        "config get --json must mask by default:\n{}",
        get_json.stdout
    );

    // (NO-LEAK) The human `config get` table masks too (same display path).
    let get_human = run_kt_agent(
        &["agent", "config", "get", "sek"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !get_human.stdout.contains(SECRET_SENTINEL),
        "the human config get leaked the secret:\n{}",
        get_human.stdout
    );

    // (REVEAL) `config get --json --reveal` re-resolves LIVE and DOES carry the
    // sentinel — the sole un-mask (AC-C). Needs MODEL_KEY in the env for the read.
    let (rok, rout, rerr) = run_kt_agent_env(
        &["agent", "config", "get", "sek", "--json", "--reveal"],
        &ctx.project_dir,
        state_dir,
        "MODEL_KEY",
        SECRET_SENTINEL,
    );
    assert!(rok, "get --reveal failed; stderr={rerr}");
    let doc: serde_json::Value = serde_json::from_str(&rout)
        .unwrap_or_else(|e| panic!("--reveal --json must be pure JSON: {e}; out={rout}"));
    let model = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "model")
        .expect("the model leaf is present");
    assert_eq!(
        model["value"],
        serde_json::json!(SECRET_SENTINEL),
        "--reveal must emit the unmasked cleartext; doc={doc}"
    );
}

#[test]
fn secret_single_key_reveal_shows_only_that_leaf_and_default_masks() {
    // AC-C single-key form: `config get <name> <key> --json [--reveal]`. Default
    // masks that one leaf; --reveal shows just its cleartext. A --reveal on a
    // NON-secret leaf is a harmless no-op (the plain value).
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let dump = ctx.project_dir.join("agent.dump");
    let m = fake_agent_manifest_secret_env(&ctx.project_dir, &dump);

    let reg = run_kt_agent(
        &[
            "agent",
            "register",
            "one",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(reg.success, "register failed; stderr={}", reg.stderr);
    // A secret leaf (agent.token) + a plain leaf (agent.mode).
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "one",
            "agent.token",
            "secret:TOKEN_KEY",
        ],
        &ctx.project_dir,
        state_dir,
    );
    run_kt_agent(
        &["agent", "config", "set", "one", "agent.mode", "fast"],
        &ctx.project_dir,
        state_dir,
    );

    // Default single-key --json masks the secret leaf.
    let masked = run_kt_agent(
        &["agent", "config", "get", "one", "agent.token", "--json"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(masked.success, "get failed; stderr={}", masked.stderr);
    let d: serde_json::Value = serde_json::from_str(&masked.stdout).unwrap();
    assert_eq!(d["entries"].as_array().unwrap().len(), 1);
    assert_eq!(d["entries"][0]["value"], serde_json::json!("secret:****"));

    // --reveal on the single secret leaf shows only its cleartext.
    let (rok, rout, _e) = run_kt_agent_env(
        &[
            "agent",
            "config",
            "get",
            "one",
            "agent.token",
            "--json",
            "--reveal",
        ],
        &ctx.project_dir,
        state_dir,
        "TOKEN_KEY",
        SECRET_SENTINEL,
    );
    assert!(rok, "reveal single-key failed");
    let d: serde_json::Value = serde_json::from_str(&rout).unwrap();
    assert_eq!(d["entries"].as_array().unwrap().len(), 1);
    assert_eq!(d["entries"][0]["value"], serde_json::json!(SECRET_SENTINEL));

    // --reveal on a NON-secret leaf is a harmless no-op: the plain value.
    let (pok, pout, _e) = run_kt_agent_env(
        &[
            "agent",
            "config",
            "get",
            "one",
            "agent.mode",
            "--json",
            "--reveal",
        ],
        &ctx.project_dir,
        state_dir,
        "TOKEN_KEY",
        SECRET_SENTINEL,
    );
    assert!(pok, "reveal on non-secret failed");
    let d: serde_json::Value = serde_json::from_str(&pout).unwrap();
    assert_eq!(d["entries"][0]["value"], serde_json::json!("fast"));
}

#[test]
fn unresolved_secret_rejects_the_start_with_a_diagnostic_and_no_state_change() {
    // AC5/AC9: a `secret:NAME` that resolves in NEITHER env nor the (absent)
    // secrets file REJECTS the start — non-zero exit, a stderr diagnostic naming the
    // NAME (never a value), and the instance stays in its PRIOR state (the snapshot
    // is not even written). The env var is deliberately NOT set.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    let dump = ctx.project_dir.join("agent.dump");
    let m = fake_agent_manifest_secret_env(&ctx.project_dir, &dump);

    let reg = run_kt_agent(
        &[
            "agent",
            "register",
            "noresolve",
            "--manifest",
            m.to_str().unwrap(),
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(reg.success, "register failed; stderr={}", reg.stderr);
    let home = reg
        .stdout
        .lines()
        .find(|l| l.contains("agents") && l.contains("noresolve"))
        .expect("register stdout names the home")
        .trim()
        .to_string();
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "noresolve",
            "model",
            "secret:DEFINITELY_UNSET_KEY_XYZ",
        ],
        &ctx.project_dir,
        state_dir,
    );

    // Start WITHOUT the env var → unresolved → non-zero, diagnostic names the NAME.
    let start = run_kt_agent(
        &["agent", "start", "noresolve"],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!start.success, "start must fail on an unresolved secret");
    assert!(
        start.stderr.contains("DEFINITELY_UNSET_KEY_XYZ"),
        "the diagnostic must name the secret NAME; stderr={}",
        start.stderr
    );
    // The instance stayed in its prior state: the snapshot was never written (the
    // resolution failure rejects before the snapshot write + the `starting`
    // transition).
    let snapshot_path = std::path::Path::new(&home).join("effective-config.json");
    assert!(
        !snapshot_path.exists(),
        "an unresolved secret must reject the start before the snapshot is written"
    );
    // And `agent list --json` still shows it as `registered` (never
    // `running`/`failed` from a half-launch). Assert on the JSON `state` field —
    // deterministic committed state, not the width-dependent human table.
    let list = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    let doc: serde_json::Value = serde_json::from_str(&list.stdout)
        .unwrap_or_else(|e| panic!("list --json not JSON: {e}\n{}", list.stdout));
    let state = doc["instances"][0]["state"].as_str().unwrap_or("");
    assert_eq!(
        state, "registered",
        "the instance must stay registered after a rejected start; list=\n{}",
        list.stdout
    );
}

// ---- Story 3-2: Token-Budget config + Fleet-detail budget surface (AC-C, AC9) ----

#[test]
fn budget_config_keys_set_and_surface_in_list_json() {
    // AC9: a budgeted instance surfaces the ceiling(s) + remaining + Breach Action
    // in `list --json` (tokens only, deterministic — no process spawn needed since
    // the budget is a config read). AC-C: the budget + action keys are settable.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );
    // Set a cumulative budget + a non-default action.
    let set = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "budget.tokens.cumulative",
            "5000",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        set.success,
        "set budget should exit 0; stderr={}",
        set.stderr
    );
    run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "budget.breach_action",
            "stop",
        ],
        &ctx.project_dir,
        state_dir,
    );

    let list = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    assert!(
        list.success,
        "list --json should exit 0; stderr={}",
        list.stderr
    );
    let doc: serde_json::Value = serde_json::from_str(&list.stdout)
        .unwrap_or_else(|e| panic!("list --json not JSON: {e}\n{}", list.stdout));
    let budget = &doc["instances"][0]["budget"];
    assert_eq!(
        budget["cumulative_limit"], 5000,
        "the cumulative ceiling surfaces in --json; doc={}",
        list.stdout
    );
    // Never metered → remaining equals the ceiling (ledger is zero).
    assert_eq!(budget["cumulative_remaining"], 5000);
    assert_eq!(budget["breach_action"], "stop");
    // Tokens only — no dollar cap/headroom.
    assert!(budget.get("cost_cap").is_none(), "no dollars: {budget}");

    // The human table shows the token budget cell (not `—`). The cell may truncate
    // on a narrow terminal, so match a stable prefix; the exact values are asserted
    // on --json above. `show` uses a wider Value column, so assert the full cell
    // there for the action + remaining.
    let show = run_kt_agent(&["agent", "show", "demo"], &ctx.project_dir, state_dir);
    assert!(
        show.stdout.contains("cum 5000/5000") && show.stdout.contains("stop"),
        "human show must render the full token budget cell; stdout=\n{}",
        show.stdout
    );
}

#[test]
fn an_unbudgeted_instance_shows_the_honest_absent_budget() {
    // AC9: an instance with NO budget configured shows an honest absent budget
    // (`null` in --json, `—` in the human table) — never a fabricated ceiling.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "bare", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let list = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    let doc: serde_json::Value = serde_json::from_str(&list.stdout)
        .unwrap_or_else(|e| panic!("list --json not JSON: {e}\n{}", list.stdout));
    assert_eq!(
        doc["instances"][0]["budget"],
        serde_json::Value::Null,
        "an un-budgeted instance's budget is null; doc={}",
        list.stdout
    );

    // The human show shows the `—` absence in the budget cell. (Story 3-3 renamed
    // the `show` row "Budget (tokens)" → "Budget" since it now covers tokens AND the
    // dollar Cost Cap.)
    let human = run_kt_agent(&["agent", "show", "bare"], &ctx.project_dir, state_dir);
    assert!(
        human.stdout.contains("Budget") && human.stdout.contains('—'),
        "human show must render the honest absent budget; stdout=\n{}",
        human.stdout
    );
}

#[test]
fn a_malformed_budget_value_is_rejected_at_write_time() {
    // AC-C: a malformed budget number / unknown Breach-Action string is rejected at
    // config-write time (non-zero exit, a clear diagnostic on stderr, nothing
    // persisted) — never silently defaulted.
    let ctx = TestContext::new();
    let state = TestContext::new();
    let state_dir = state.project_dir.as_path();
    run_kt_agent(
        &["agent", "register", "demo", "--kind", "mock"],
        &ctx.project_dir,
        state_dir,
    );

    let bad_num = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "budget.tokens.per_run",
            "lots",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(!bad_num.success, "a non-numeric budget must be rejected");
    assert!(
        bad_num.stderr.contains("budget.tokens.per_run"),
        "the diagnostic names the key; stderr={}",
        bad_num.stderr
    );

    let bad_action = run_kt_agent(
        &[
            "agent",
            "config",
            "set",
            "demo",
            "budget.breach_action",
            "throttle",
        ],
        &ctx.project_dir,
        state_dir,
    );
    assert!(
        !bad_action.success,
        "an unknown breach action must be rejected"
    );
    assert!(
        bad_action.stderr.contains("pause") && bad_action.stderr.contains("throttle"),
        "the diagnostic names the accepted set + the offending value; stderr={}",
        bad_action.stderr
    );

    // Nothing was persisted: the budget is still absent in --json.
    let list = run_kt_agent(&["agent", "list", "--json"], &ctx.project_dir, state_dir);
    let doc: serde_json::Value = serde_json::from_str(&list.stdout).unwrap();
    assert_eq!(
        doc["instances"][0]["budget"],
        serde_json::Value::Null,
        "a rejected write must persist nothing; doc={}",
        list.stdout
    );
}
