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
    // standalone `kt agent start` kills the agent when the CLI exits (durable
    // supervision is story 1-6). That caveat is printed as a one-line NOTICE to
    // STDERR (AD-12: results → stdout, notices → stderr), and the stdout result
    // line (`running`) is UNCHANGED. This asserts both halves so a future change
    // that either drops the notice or leaks it onto stdout is caught.
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
    // The notice is on stderr, and names story 1-6 as the durable-supervision
    // follow-up.
    assert!(
        run.stderr
            .contains("supervised only for this engine session"),
        "single-lifetime notice must go to stderr; stderr={}",
        run.stderr
    );
    assert!(run.stderr.contains("1-6"), "stderr={}", run.stderr);
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
