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
