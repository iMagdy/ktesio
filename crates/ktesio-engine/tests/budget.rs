//! Integration tests for story-3-2 Token-Budget ENFORCEMENT (AC-A / AC-B / AC-C),
//! driven end-to-end through the PUBLIC async [`Engine`] + its background reaper
//! cadence (spine AD-7) — spawning the REAL `fake_agent` with `--emit-usage` so
//! genuine self-reported usage crosses a configured budget and the supervisor
//! fires the configured Breach Action.
//!
//! ## Robust, cross-OS by construction (retro AI-35/37/38)
//!
//! Like `tests/metering.rs`, these keep a SINGLE in-process `Engine` alive for the
//! whole test — NO cross-lifetime process survival, NO `OsId`-gated skip anywhere
//! (the evaluator + config parse are pure `std`; `fake_agent --emit-usage` is pure
//! `std`). Determinism comes from asserting on COMMITTED LIFECYCLE STATE: the
//! `fake_agent` emits a KNOWN number of usage events with FIXED token sentinels
//! (10 in / 20 out = 30 tokens each), the evaluator runs SYNCHRONOUSLY inside the
//! ingestion path the instant a breaching event commits, so each test POLLS the
//! committed state (the store / the engine reads) until it reaches the expected
//! state — never a wall-clock sleep against a side file. The bulk of the
//! boundary/scope coverage lives in the PURE evaluator unit tests
//! (`domain::budget`); these integration tests prove the end-to-end WIRING.
//!
//! ## Windows posture
//!
//! Every test here runs identically on Linux, macOS, and Windows: the budget is a
//! config value, the evaluator is pure, the breach event is a durable JSON line,
//! and the `stop`/`warn` assertions are OS-agnostic. The DEFAULT `pause` action is
//! exercised with a manifest that declares pause `guaranteed` on all three OSes
//! (the mock's real cross-OS pause backend), and the assertion is on the COMMITTED
//! `paused` state — not on process suspension timing — so it is deterministic
//! everywhere.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ktesio_engine::{AdapterRef, BreachScope, Engine, LifecycleState, OsId};
use tempfile::TempDir;

/// The token sentinels the `fake_agent --emit-usage` emitter stamps on every
/// event (10 in / 20 out = 30 tokens/event), so a budget breach point is exact.
const TOKENS_PER_EVENT: u64 = 30;

/// Write a manifest whose `[lifecycle.start]` exec is `fake_agent` + `args`,
/// declaring pause `guaranteed` on all three OSes so the DEFAULT pause Breach
/// Action is a real (cross-OS) suspension and its committed `paused` state is
/// deterministic.
fn write_fake_manifest(dir: &Path, kind: &str, args: &[&str]) {
    let bin = ktesio_conformance::fake_agent_bin();
    // Append `--dump argv-dump.txt` (story 2-2's write_dump mechanism): the fake_agent
    // writes its RECEIVED argv + env into this RELATIVE path, which resolves against
    // the child's working_dir = the Agent Home (the backend sets `current_dir` to it),
    // i.e. `<state_dir>/agents/<name>/argv-dump.txt`. That file sits under the state dir
    // the timeout `dump_diagnostics` walks, so on a macOS/Windows failure the received
    // argv is surfaced — revealing whether `--emit-usage` reached the process at all.
    // It writes a SEPARATE file (never agent.log/stdout), so no test's committed-state /
    // breach / usage-row assertions are affected; on a passing runner it is inert.
    let args_toml = args
        .iter()
        .copied()
        .chain(["--dump", "argv-dump.txt"])
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

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[capabilities.pause]
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

fn open(base: &TempDir) -> Engine {
    Engine::open(Some(base.path().to_path_buf())).expect("open engine")
}

/// The committed Lifecycle State for `name`, read via a direct read-only
/// connection to the same state DB the engine commits to (deterministic committed
/// state — NOT a wall-clock guess).
fn committed_state(state_dir: &Path, name: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(state_dir.join("state.db")).ok()?;
    conn.query_row(
        "SELECT state FROM agent_instances WHERE name = ?1",
        [name],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Poll the committed Lifecycle State for `name` until it equals `want`, bounded.
/// The evaluator runs synchronously inside the ingestion path, so the transition
/// is committed as soon as the breaching event is ingested by the reaper — this
/// waits for the DETERMINISTIC committed state, not a duration.
fn wait_for_state(state_dir: &Path, name: &str, want: LifecycleState, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let state = committed_state(state_dir, name);
        if state.as_deref() == Some(want.as_str()) {
            return;
        }
        // On timeout, dump a rich diagnostic block to stderr (visible in CI) BEFORE
        // the assert fires — so the next macOS/Windows CI run reveals the cause. The
        // assert itself is unchanged (same message, same 30s deadline).
        if Instant::now() >= deadline {
            dump_diagnostics(state_dir, name);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for '{name}' to reach {} (committed state: {state:?})",
            want.as_str()
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// The number of `usage_events` rows for `name` (committed ledger state).
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

/// Poll the committed usage-row count until it reaches `expected`, bounded.
fn wait_for_usage_rows(state_dir: &Path, name: &str, expected: u64, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        if usage_row_count(state_dir, name) >= expected {
            return;
        }
        // On timeout, dump diagnostics to stderr (see wait_for_state) BEFORE the
        // assert fires. The assert (message + 30s deadline) is unchanged.
        if Instant::now() >= deadline {
            dump_diagnostics(state_dir, name);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} committed usage rows for '{name}' (have {})",
            usage_row_count(state_dir, name)
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// On a timeout, dump a rich diagnostic block to STDERR (visible in CI logs) BEFORE
/// the caller panics — so the NEXT macOS/Windows CI run reveals WHY usage never
/// drained there (this suite passes locally + on Ubuntu, so the dump only ever fires
/// on the failing runner). It answers the key trichotomy: is the agent-output capture
/// `agents/<name>/logs/agent.log` (a) MISSING, (b) present but EMPTY, or (c) present
/// WITH `KTESIO_USAGE` lines that were emitted but never drained into the ledger?
/// Self-contained + best-effort: a read hiccup is reported inline, never a panic that
/// would mask the real timeout the caller is about to report.
fn dump_diagnostics(state_dir: &Path, name: &str) {
    eprintln!("\n===== KTESIO budget.rs TIMEOUT DIAGNOSTICS (instance '{name}') =====");

    // (1) The committed DB view the assertions read (a fresh read-only connection).
    eprintln!("committed usage rows: {}", usage_row_count(state_dir, name));
    eprintln!(
        "committed lifecycle state: {:?}",
        committed_state(state_dir, name)
    );

    // (2) The resolved fake_agent helper binary. A stale/missing binary that does not
    // understand `--emit-usage` would still reach `running` yet emit ZERO usage — so
    // its path + presence is load-bearing for the "have 0 rows" trichotomy.
    let bin = ktesio_conformance::fake_agent_bin();
    eprintln!(
        "fake_agent_bin(): {} (exists={}, is_file={})",
        bin.display(),
        bin.exists(),
        bin.is_file()
    );

    // (3) TARGETED trichotomy on the agent-output capture — the exact file the reaper
    // drains (`<state_dir>/agents/<name>/logs/agent.log`).
    let agent_log = state_dir
        .join("agents")
        .join(name)
        .join("logs")
        .join("agent.log");
    match std::fs::read_to_string(&agent_log) {
        Ok(text) if text.is_empty() => {
            eprintln!("agent.log: PRESENT BUT EMPTY at {}", agent_log.display());
        }
        Ok(text) => {
            let total = text.lines().count();
            let usage = text.lines().filter(|l| l.contains("KTESIO_USAGE")).count();
            let verdict = if usage > 0 {
                "emitted but NOT drained into the ledger"
            } else {
                "agent emitted NO KTESIO_USAGE lines"
            };
            eprintln!(
                "agent.log: PRESENT at {} — {total} line(s), {usage} KTESIO_USAGE line(s) ({verdict})",
                agent_log.display()
            );
        }
        Err(e) => {
            eprintln!(
                "agent.log: MISSING/UNREADABLE at {} ({e})",
                agent_log.display()
            );
        }
    }

    // (4) The argv the fake_agent actually RECEIVED (story 2-2's `--dump` mechanism:
    // `write_fake_manifest` appends `--dump argv-dump.txt`, which the child writes into
    // its working_dir = the Agent Home). Print ONLY the `arg=` lines — NEVER the `env=`
    // lines, which could carry runner secrets. This is the decisive datum:
    //   * file ABSENT     → args were dropped WHOLESALE (`--dump` never arrived either);
    //   * present, NO `--emit-usage` → the flag was dropped before/at spawn;
    //   * present, WITH `--emit-usage` → the flag arrived (a parser-side issue).
    let argv_dump = state_dir.join("agents").join(name).join("argv-dump.txt");
    match std::fs::read_to_string(&argv_dump) {
        Ok(text) => {
            eprintln!("received argv (from {}):", argv_dump.display());
            for line in text.lines().filter(|l| l.starts_with("arg=")) {
                eprintln!("    {line}");
            }
        }
        Err(e) => {
            eprintln!(
                "argv dump ABSENT/UNREADABLE at {} ({e}) — the args were likely dropped \
                 wholesale (`--dump` did not reach the process either)",
                argv_dump.display()
            );
        }
    }

    // (5) Full recursive walk of the state dir (which contains the agents/ tree, each
    // Agent Home, and every per-instance log): each file's path + byte size, plus the
    // full contents of small `*.log`/`*.toml`/`*.json` text files. The `argv-dump.txt`
    // is `.txt`, so the walk lists it by SIZE only — its `env=` lines are never spewed.
    eprintln!(
        "--- recursive walk of state dir {} ---",
        state_dir.display()
    );
    dump_tree(state_dir);
    eprintln!("===== end diagnostics =====\n");
}

/// Recursively print every file under `dir`: path + byte size, and (for small
/// `*.log`/`*.toml`/`*.json` text files) the full contents. Entries are sorted for
/// stable CI output. Best-effort — a read error is printed inline, never a panic.
fn dump_tree(dir: &Path) {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(e) => {
            eprintln!("  <read_dir {} failed: {e}>", dir.display());
            return;
        }
    };
    entries.sort();
    for path in entries {
        if path.is_dir() {
            dump_tree(&path);
        } else {
            dump_file(&path);
        }
    }
}

/// Print one file's path + byte size, plus the full contents of a small text log/
/// config file — so the agent-output `agent.log` shows whether it is missing, empty,
/// or holds undrained `KTESIO_USAGE` lines. Binary files (`state.db` + its
/// `-wal`/`-shm` siblings) are listed by size only.
fn dump_file(path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let bytes = meta.len();
            eprintln!("  FILE {} ({bytes} bytes)", path.display());
            let dump_text = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "log" | "toml" | "json"));
            if dump_text && bytes <= 256 * 1024 {
                match std::fs::read_to_string(path) {
                    Ok(text) if text.is_empty() => eprintln!("    <present but empty>"),
                    Ok(text) => {
                        for line in text.lines() {
                            eprintln!("    | {line}");
                        }
                    }
                    Err(e) => eprintln!("    <unreadable as utf-8: {e}>"),
                }
            }
        }
        Err(e) => eprintln!("  FILE {} <metadata failed: {e}>", path.display()),
    }
}

#[test]
fn a_cumulative_budget_breach_pauses_by_default_and_records_the_breach() {
    // AC-A: a Token Budget on a running instance, when metered consumption REACHES
    // it, drives the DEFAULT Breach Action (pause) via the Epic-1 lifecycle, and a
    // breach event is ALWAYS recorded. 5 events × 30 tokens = 150; a cumulative
    // budget of 90 breaches on the 3rd event (90 >= 90). The instance reaches
    // `paused` (asserted on committed STATE), a BudgetBreachEvent is recorded, and
    // the `running → paused` transition carries the BudgetExceeded cause.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "cap",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("cap", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // Budget BEFORE start: a cumulative ceiling of 90 tokens (breaches at event 3).
    // Default action (pause) — not set explicitly, proving the ratified default.
    facade
        .set_config("cap", "budget.tokens.cumulative", "90")
        .unwrap();

    facade.start("cap").unwrap();

    // The instance reaches `paused` — the breach fired the default Breach Action.
    wait_for_state(
        state.path(),
        "cap",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    // A breach event is ALWAYS recorded (AC-A / AC7), and EXACTLY ONE for the single
    // crossing (the idempotence latch, story 3-2): even though several events were
    // emitted over the ceiling, the breach fires at most once per scope per Run.
    let breaches = facade.budget_breach_events("cap").unwrap();
    assert_eq!(
        breaches.len(),
        1,
        "exactly one breach event for a single crossing (not one per event); got {}: {breaches:?}",
        breaches.len()
    );
    let b = &breaches[0];
    assert_eq!(b.scope, BreachScope::Cumulative);
    assert_eq!(b.limit, 90);
    assert!(
        b.observed >= 90,
        "observed {} must be >= the limit",
        b.observed
    );
    assert_eq!(b.action.as_str(), "pause");
    assert_eq!(b.metering_source, "self-reported");

    // The `running → paused` transition carries the BudgetExceeded cause (AC7): the
    // lifecycle log itself explains WHY.
    let events = facade.transition_events("cap").unwrap();
    let paused = events
        .iter()
        .find(|e| e.new_state == LifecycleState::Paused)
        .expect("a running → paused transition was recorded");
    assert!(
        matches!(
            paused.cause,
            ktesio_engine::TransitionCause::BudgetExceeded { .. }
        ),
        "the paused transition must carry BudgetExceeded, got {:?}",
        paused.cause
    );

    // The Fleet-detail budget surfaces the ceiling + (zero/near-zero) remaining +
    // the action (AC9) — tokens only.
    let fleet = facade.fleet().unwrap();
    let entry = fleet.iter().find(|e| e.name.as_str() == "cap").unwrap();
    let view = entry.budget.as_ref().expect("a budgeted instance");
    assert_eq!(view.cumulative_limit, Some(90));
    assert_eq!(view.breach_action.as_str(), "pause");

    let _ = facade.stop("cap", Some(Duration::from_secs(5)));
}

#[test]
fn the_ge_boundary_a_total_exactly_at_the_budget_breaches() {
    // AC-A threshold: reaches = `>=`. A single event of 30 tokens with a cumulative
    // budget of EXACTLY 30 breaches (30 >= 30) — the guardrail fires AT the ceiling,
    // not one token past it. Proven on the committed pause.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "edge",
        &["--emit-usage", "3", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("edge", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // Exactly one event's worth of tokens.
    facade
        .set_config(
            "edge",
            "budget.tokens.cumulative",
            &TOKENS_PER_EVENT.to_string(),
        )
        .unwrap();

    facade.start("edge").unwrap();
    wait_for_state(
        state.path(),
        "edge",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    let breaches = facade.budget_breach_events("edge").unwrap();
    let b = &breaches[0];
    assert_eq!(b.limit, TOKENS_PER_EVENT);
    assert_eq!(
        b.observed, TOKENS_PER_EVENT,
        "the FIRST event to reach the ceiling exactly is the breach"
    );

    let _ = facade.stop("edge", Some(Duration::from_secs(5)));
}

#[test]
fn breach_action_stop_drives_the_instance_to_stopped() {
    // AC-C: breach_action = stop drives the instance to a terminal `stopped`. A
    // cumulative budget of 30 with 5 emitted events breaches on event 1.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "hardstop",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "hardstop",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    facade
        .set_config("hardstop", "budget.tokens.cumulative", "30")
        .unwrap();
    facade
        .set_config("hardstop", "budget.breach_action", "stop")
        .unwrap();

    facade.start("hardstop").unwrap();

    // The instance reaches the terminal `stopped` state (the stop Breach Action).
    wait_for_state(
        state.path(),
        "hardstop",
        LifecycleState::Stopped,
        Duration::from_secs(30),
    );

    // The breach is recorded with the stop action.
    let breaches = facade.budget_breach_events("hardstop").unwrap();
    assert!(!breaches.is_empty());
    assert_eq!(breaches[0].action.as_str(), "stop");
}

#[test]
fn breach_action_warn_records_exactly_one_breach_across_many_post_breach_events() {
    // AC-C + the idempotence latch (story 3-2 fix): breach_action = warn performs NO
    // lifecycle transition, so the agent keeps running and EVERY subsequent usage
    // event re-runs enforcement while STILL over the ceiling. A breach must fire AT
    // MOST ONCE per scope per Run — so a single logical crossing records EXACTLY ONE
    // breach event, NOT one per post-crossing event.
    //
    // Budget 30, 5 emitted events × 30 tokens: event 1 crosses cumulative=30 and
    // events 2–5 stay over it. WITHOUT the latch this records ~5 duplicate breach
    // events (this assertion FAILS: 5 != 1). WITH the latch: exactly 1. We wait for
    // ALL 5 usage rows to commit (so every post-breach event has run enforcement),
    // then assert the breach COUNT is exactly 1.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "watchonly",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "watchonly",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    facade
        .set_config("watchonly", "budget.tokens.cumulative", "30")
        .unwrap();
    facade
        .set_config("watchonly", "budget.breach_action", "warn")
        .unwrap();

    facade.start("watchonly").unwrap();

    // Wait until ALL 5 usage events have committed — i.e. every post-breach event has
    // passed through the enforcement stage. Only then is the "exactly one" claim a
    // real test of the latch (all 4 post-crossing events had their chance to re-record).
    wait_for_usage_rows(state.path(), "watchonly", 5, Duration::from_secs(30));

    // EXACTLY ONE breach event for the single logical crossing (the latch fix). A
    // small settle re-poll guards against a late 6th ingestion pass sneaking a
    // duplicate in after the count check.
    let breaches = facade.budget_breach_events("watchonly").unwrap();
    assert_eq!(
        breaches.len(),
        1,
        "warn must record EXACTLY ONE breach for a single crossing across 5 events \
         (a per-event re-record is the bug); got {}: {breaches:?}",
        breaches.len()
    );
    assert_eq!(breaches[0].action.as_str(), "warn");
    assert_eq!(breaches[0].scope, BreachScope::Cumulative);

    // The instance is STILL running (warn does NOT transition).
    assert_eq!(
        committed_state(state.path(), "watchonly").as_deref(),
        Some("running"),
        "warn must not transition the instance"
    );
    // No pause/stop transition was recorded (only the breach event).
    let events = facade.transition_events("watchonly").unwrap();
    assert!(
        !events.iter().any(|e| matches!(
            e.new_state,
            LifecycleState::Paused | LifecycleState::Stopping
        )),
        "warn must record NO pause/stop transition"
    );

    let _ = facade.stop("watchonly", Some(Duration::from_secs(5)));
}

#[test]
fn breach_action_pause_on_an_unsupported_adapter_stays_running_and_still_records() {
    // AC6 (honest posture): breach_action = pause on an adapter whose CURRENT-OS
    // pause level projects to `unsupported` must FAIL FAST honestly — NO fake pause,
    // NO silent escalation to stop. The instance STAYS `running` (no
    // paused/stopping/stopped transition), and the breach event is STILL recorded
    // (FR-21 "always recorded regardless of action" — the record captures the fact
    // even though the action could not be honored). Cross-OS by construction: the
    // manifest declares pause ONLY for a NON-current OS (so the current-OS projection
    // is the Unsupported default), the budget/evaluator/breach-log are pure, and we
    // assert on COMMITTED STATE — NO OsId gate on the assertions.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    // Declare pause for an OS OTHER than the current one so the current-OS projection
    // is Unsupported (the 1-5 unsupported-pause construction). `interaction` stays
    // guaranteed everywhere so registration passes; metering is self-reported.
    let other_os = match OsId::current() {
        OsId::Linux => "windows",
        OsId::Macos => "windows",
        OsId::Windows => "linux",
        OsId::Other => "linux",
    };
    let bin = ktesio_conformance::fake_agent_bin();
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "nopause"

[lifecycle.start]
exec = {exec:?}
args = ["--emit-usage", "5", "--linger-ms", "600000"]

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
        other = other_os,
    );
    std::fs::write(manifest.path().join("adapter.toml"), body).unwrap();

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "nopause",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    // A breaching cumulative budget of 30 (event 1 crosses it) with the DEFAULT-ish
    // explicit pause action.
    facade
        .set_config("nopause", "budget.tokens.cumulative", "30")
        .unwrap();
    facade
        .set_config("nopause", "budget.breach_action", "pause")
        .unwrap();

    facade.start("nopause").unwrap();

    // The breach IS recorded even though pause is unsupported (FR-21). Poll the
    // committed breach log.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let breaches = facade.budget_breach_events("nopause").unwrap();
        if !breaches.is_empty() {
            assert_eq!(breaches[0].action.as_str(), "pause");
            assert_eq!(breaches[0].scope, BreachScope::Cumulative);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the breach must be recorded even when pause is unsupported"
        );
        std::thread::sleep(Duration::from_millis(40));
    }

    // HONEST posture: the instance STAYS running — no fake pause, no silent escalation
    // to stop. Assert on committed state after the breach was recorded.
    assert_eq!(
        committed_state(state.path(), "nopause").as_deref(),
        Some("running"),
        "an unsupported pause on breach must NOT transition the instance"
    );
    // No paused/stopping/stopped transition was ever recorded (no fake pause, no
    // escalation) — only the breach event captured the fact.
    let events = facade.transition_events("nopause").unwrap();
    assert!(
        !events.iter().any(|e| matches!(
            e.new_state,
            LifecycleState::Paused | LifecycleState::Stopping | LifecycleState::Stopped
        )),
        "unsupported pause must record NO pause/stop/escalation transition; events={events:?}"
    );

    let _ = facade.stop("nopause", Some(Duration::from_secs(5)));
}

#[test]
fn a_per_run_budget_breaches_within_the_run() {
    // AC (per-run scope): a per-run ceiling bounds a single Run's span. 5 events ×
    // 30 = 150; a per-run budget of 60 breaches on event 2 (60 >= 60). Prove the
    // pause + the reported scope is PerRun.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "perrun",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "perrun",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    facade
        .set_config("perrun", "budget.tokens.per_run", "60")
        .unwrap();

    facade.start("perrun").unwrap();
    wait_for_state(
        state.path(),
        "perrun",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    // Exactly one per-run breach for the single crossing (the idempotence latch):
    // 5 events cross the per-run ceiling of 60, but the per-run scope latches once.
    let breaches = facade.budget_breach_events("perrun").unwrap();
    assert_eq!(
        breaches.len(),
        1,
        "exactly one per-run breach for a single crossing; got {}: {breaches:?}",
        breaches.len()
    );
    let b = &breaches[0];
    assert_eq!(b.scope, BreachScope::PerRun, "the per-run scope tripped");
    assert_eq!(b.limit, 60);

    let _ = facade.stop("perrun", Some(Duration::from_secs(5)));
}

#[test]
fn a_budget_changed_while_running_applies_immediately() {
    // AC-B: budgets are changeable while `running`, applying immediately. Start
    // under a HIGH budget (no breach), confirm the instance is running past some
    // usage, then LOWER the budget below the already-committed total — the very
    // next event breaches (a live read on each ingestion, no restart/re-arm).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    // Many events, slowly, so there is time to lower the budget mid-run.
    write_fake_manifest(
        manifest.path(),
        "live",
        &["--emit-usage", "20", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("live", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // Start with a HIGH cumulative budget — no breach initially.
    facade
        .set_config("live", "budget.tokens.cumulative", "1000000")
        .unwrap();

    facade.start("live").unwrap();

    // Let at least 3 events commit under the high budget (no breach — still running).
    wait_for_usage_rows(state.path(), "live", 3, Duration::from_secs(30));
    assert_eq!(
        committed_state(state.path(), "live").as_deref(),
        Some("running"),
        "no breach under the high budget"
    );

    // LOWER the budget below the already-committed total (3 events = 90 tokens) —
    // apply immediately on the next event (AC-B). A budget of 30 is already crossed.
    facade
        .set_config("live", "budget.tokens.cumulative", "30")
        .unwrap();

    // The next ingested event re-reads the CURRENT budget and breaches → pause.
    wait_for_state(
        state.path(),
        "live",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );
    let breaches = facade.budget_breach_events("live").unwrap();
    assert!(!breaches.is_empty(), "the lowered budget must breach live");
    assert_eq!(breaches[0].limit, 30);

    let _ = facade.stop("live", Some(Duration::from_secs(5)));
}

#[test]
fn an_unbudgeted_instance_never_breaches() {
    // The negative control: an instance with NO budget configured runs its whole
    // emission without a breach — the common (un-budgeted) path is unaffected.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "free",
        &["--emit-usage", "4", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("free", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // NO budget config set.
    facade.start("free").unwrap();

    // All 4 events commit...
    wait_for_usage_rows(state.path(), "free", 4, Duration::from_secs(30));
    // ...and the instance is STILL running (no breach, no pause/stop).
    assert_eq!(
        committed_state(state.path(), "free").as_deref(),
        Some("running")
    );
    // No breach recorded, and the Fleet budget is the honest absence.
    assert!(facade.budget_breach_events("free").unwrap().is_empty());
    let fleet = facade.fleet().unwrap();
    let entry = fleet.iter().find(|e| e.name.as_str() == "free").unwrap();
    assert!(
        entry.budget.is_none(),
        "an un-budgeted instance shows no budget"
    );

    let _ = facade.stop("free", Some(Duration::from_secs(5)));
}

#[test]
fn only_the_ingestion_choke_point_enforces_budgets() {
    // AD-7 companion invariant (the single-evaluator guard, mirroring 3-1's
    // single-writer audit): NO code path other than the supervisor's ONE
    // ingestion→commit choke point may EVALUATE a budget / fire a Breach Action. A
    // grep-style source scan over the engine `src/` proves every CALL to
    // `BudgetEvaluator::evaluate(` lives ONLY in `domain/supervisor.rs`. If a future
    // change scatters a second enforcement site, this fails. Pure source scan (no
    // OS cfg); runs on every OS.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut eval_files = std::collections::BTreeSet::new();
    visit_rs(&src, &mut |path, text| {
        for line in text.lines() {
            let trimmed = line.trim_start();
            // Skip the definition + doc comments; only actual invocations count.
            if trimmed.starts_with("//")
                || trimmed.starts_with("///")
                || trimmed.contains("fn evaluate(")
            {
                continue;
            }
            if line.contains("BudgetEvaluator::evaluate(") {
                eval_files.insert(
                    path.strip_prefix(&src)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    });
    // The ONLY files that may CALL the evaluator: the supervisor choke point (the
    // sole PRODUCTION enforcement site) and `domain/budget.rs` (the evaluator's own
    // home — its pure unit tests exercise it directly; that is not a second
    // enforcement path).
    let allowed: std::collections::BTreeSet<String> = ["domain/supervisor.rs", "domain/budget.rs"]
        .into_iter()
        .map(String::from)
        .collect();
    let violations: Vec<&String> = eval_files.difference(&allowed).collect();
    assert!(
        violations.is_empty(),
        "BudgetEvaluator::evaluate is called outside the ingestion choke point: {violations:?}"
    );
    assert!(
        eval_files.contains("domain/supervisor.rs"),
        "the enforcement choke point must call BudgetEvaluator::evaluate; callers: {eval_files:?}"
    );
}

/// Recursively visit every `.rs` file under `dir`, calling `f(path, contents)`.
fn visit_rs(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rs(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                f(&path, &text);
            }
        }
    }
}
