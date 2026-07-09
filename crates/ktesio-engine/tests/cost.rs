//! Integration tests for story-3-3 dollar Cost-Cap ENFORCEMENT (AC-A / AC-B /
//! AC-C), driven end-to-end through the PUBLIC async [`Engine`] + its background
//! reaper (spine AD-7) — spawning the REAL `fake_agent` with `--emit-usage` so
//! genuine self-reported tokens, priced at a KNOWN Rate, cross a KNOWN dollar cap
//! and the supervisor fires the configured Breach Action.
//!
//! ## Robust, cross-OS by construction (retro AI-35/37/38)
//!
//! Like `tests/budget.rs`, these keep a SINGLE in-process `Engine` alive for the
//! whole test — NO cross-lifetime process survival, NO `OsId`-gated skip anywhere
//! (the money math + the CostEvaluator + the config parse are pure `std`;
//! `fake_agent --emit-usage` is pure `std`). Determinism comes from asserting on
//! COMMITTED LIFECYCLE STATE: the `fake_agent` emits a KNOWN number of usage events
//! with FIXED token sentinels, the derived cost = tokens × the KNOWN Rate is EXACT,
//! and the dollar evaluator runs SYNCHRONOUSLY inside the ingestion path the instant
//! a breaching event commits — so each test POLLS the committed state until it
//! reaches the expected state, never a wall-clock sleep. The bulk of the
//! boundary/precision/overflow coverage lives in the PURE unit tests
//! (`domain::cost`); these prove the end-to-end WIRING + the no-retro-repricing.
//!
//! ## The known-cost arithmetic (determinism lever)
//!
//! The default `fake_agent --emit-usage` sentinels are 10 input + 20 output tokens
//! per event. With a Rate of `$1.00/1M` on BOTH directions (1 micro per token), the
//! cost per event is exactly `10 + 20 = 30` micro-dollars. So N events cost `30 × N`
//! micros. A cumulative Cost Cap expressed as a dollar string picks the breach
//! event exactly: `$0.00009` = 90 micros breaches on event 3 (`>= 90`).
//!
//! ## Windows posture
//!
//! Every test here runs identically on Linux, macOS, and Windows: the Rate/cap are
//! config values, the derivation + evaluator are pure, the breach event is a durable
//! JSON line, and the assertions are on COMMITTED state (not process-suspension
//! timing). The DEFAULT `pause` action uses a manifest declaring pause `guaranteed`
//! on all three OSes.

use std::path::Path;
use std::time::{Duration, Instant};

use ktesio_engine::{AdapterRef, BreachDimension, BreachScope, Engine, LifecycleState};
use tempfile::TempDir;

/// Write a manifest whose `[lifecycle.start]` exec is `fake_agent` + `args`,
/// declaring pause `guaranteed` on all three OSes so the DEFAULT pause Breach
/// Action is a real (cross-OS) suspension with a deterministic committed state.
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
/// connection to the same state DB the engine commits to.
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
/// The dollar evaluator runs synchronously inside the ingestion path, so the
/// transition commits as soon as the breaching event is ingested — this waits for
/// the DETERMINISTIC committed state, not a duration.
fn wait_for_state(state_dir: &Path, name: &str, want: LifecycleState, within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        let state = committed_state(state_dir, name);
        if state.as_deref() == Some(want.as_str()) {
            return;
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
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} committed usage rows for '{name}' (have {})",
            usage_row_count(state_dir, name)
        );
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Configure a Rate of `$1.00/1M` on both directions (1 micro/token) so a known
/// token total yields a known micro-dollar cost.
fn set_unit_rate(facade: &ktesio_engine::Blocking, name: &str) {
    facade.set_config(name, "cost.rate.input", "1.00").unwrap();
    facade.set_config(name, "cost.rate.output", "1.00").unwrap();
}

#[test]
fn a_dollar_cap_breach_pauses_by_default_and_records_a_dollar_breach() {
    // AC-A/AC-C: a Cost Cap on a running instance, when the DERIVED cost REACHES it,
    // drives the DEFAULT Breach Action (pause) and records a DOLLAR breach event.
    // 5 events × 30 micros = 150; a cumulative cap of $0.00009 (90 micros) breaches
    // on event 3 (90 >= 90). The instance reaches `paused` (committed STATE), a
    // dollar BudgetBreachEvent is recorded (dimension = dollars, labeled), and the
    // transition carries the dollar BudgetExceeded cause.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "capdollar",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "capdollar",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    set_unit_rate(&facade, "capdollar");
    // A cumulative dollar cap of 90 micros ($0.00009) — breaches at event 3.
    facade
        .set_config("capdollar", "budget.dollars.cumulative", "0.00009")
        .unwrap();

    facade.start("capdollar").unwrap();

    // The instance reaches `paused` — the dollar breach fired the default action.
    wait_for_state(
        state.path(),
        "capdollar",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    // Exactly one DOLLAR breach for the single crossing (the dimension-keyed latch).
    let breaches = facade.budget_breach_events("capdollar").unwrap();
    let dollar: Vec<_> = breaches
        .iter()
        .filter(|b| b.dimension == BreachDimension::Dollars)
        .collect();
    assert_eq!(
        dollar.len(),
        1,
        "exactly one dollar breach for a single crossing; got {}: {breaches:?}",
        dollar.len()
    );
    let b = dollar[0];
    assert_eq!(b.scope, BreachScope::Cumulative);
    assert_eq!(b.dollar_limit.map(|m| m.get()), Some(90));
    assert!(
        b.dollar_observed.map(|m| m.get()).unwrap_or(0) >= 90,
        "observed cost must be >= the cap: {:?}",
        b.dollar_observed
    );
    assert_eq!(b.estimate_label.map(|l| l.as_str()), Some("estimated"));
    assert_eq!(b.action.as_str(), "pause");

    // The `running → paused` transition carries the dollar BudgetExceeded cause.
    let events = facade.transition_events("capdollar").unwrap();
    let paused = events
        .iter()
        .find(|e| e.new_state == LifecycleState::Paused)
        .expect("a running → paused transition was recorded");
    assert!(
        matches!(
            &paused.cause,
            ktesio_engine::TransitionCause::BudgetExceeded {
                dimension: BreachDimension::Dollars,
                ..
            }
        ),
        "the paused transition must carry a DOLLAR BudgetExceeded, got {:?}",
        paused.cause
    );

    // The Fleet-detail cost surface (AC10): the labeled cost + cap + remaining.
    let fleet = facade.fleet().unwrap();
    let entry = fleet
        .iter()
        .find(|e| e.name.as_str() == "capdollar")
        .unwrap();
    assert!(
        entry.usage.cumulative_dollars.is_some(),
        "a Rate'd instance surfaces a derived dollar cost"
    );
    assert_eq!(
        entry.usage.estimate_label.map(|l| l.as_str()),
        Some("estimated")
    );
    let view = entry.budget.as_ref().expect("a capped instance");
    assert_eq!(view.cumulative_cost_cap.map(|m| m.get()), Some(90));
    // Remaining saturates at 0 once breached.
    assert_eq!(view.cumulative_dollars_remaining.map(|m| m.get()), Some(0));

    let _ = facade.stop("capdollar", Some(Duration::from_secs(5)));
}

#[test]
fn the_ge_boundary_a_cost_exactly_at_the_cap_breaches() {
    // AC-C threshold: reaches = `>=` in micros. A cap EXACTLY equal to the cost of
    // the first event breaches on event 1 (the guardrail fires AT the cap, not one
    // micro past it). One event = 30 micros; cap = $0.00003 = 30 micros.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "edgedollar",
        &["--emit-usage", "3", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "edgedollar",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    set_unit_rate(&facade, "edgedollar");
    facade
        .set_config("edgedollar", "budget.dollars.cumulative", "0.00003")
        .unwrap();

    facade.start("edgedollar").unwrap();
    wait_for_state(
        state.path(),
        "edgedollar",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    let breaches = facade.budget_breach_events("edgedollar").unwrap();
    let b = breaches
        .iter()
        .find(|b| b.dimension == BreachDimension::Dollars)
        .expect("a dollar breach");
    assert_eq!(b.dollar_limit.map(|m| m.get()), Some(30));
    assert_eq!(
        b.dollar_observed.map(|m| m.get()),
        Some(30),
        "the FIRST event to reach the cap exactly is the breach"
    );

    let _ = facade.stop("edgedollar", Some(Duration::from_secs(5)));
}

#[test]
fn breach_action_stop_drives_the_instance_to_stopped_on_a_dollar_cap() {
    // AC-C: breach_action = stop drives the instance to a terminal `stopped` on a
    // dollar cap. Cap $0.00003 (30 micros) with 5 events breaches on event 1.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "dollarstop",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "dollarstop",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    set_unit_rate(&facade, "dollarstop");
    facade
        .set_config("dollarstop", "budget.dollars.cumulative", "0.00003")
        .unwrap();
    facade
        .set_config("dollarstop", "budget.breach_action", "stop")
        .unwrap();

    facade.start("dollarstop").unwrap();
    wait_for_state(
        state.path(),
        "dollarstop",
        LifecycleState::Stopped,
        Duration::from_secs(30),
    );

    let breaches = facade.budget_breach_events("dollarstop").unwrap();
    let b = breaches
        .iter()
        .find(|b| b.dimension == BreachDimension::Dollars)
        .expect("a dollar breach");
    assert_eq!(b.action.as_str(), "stop");
}

#[test]
fn breach_action_warn_records_exactly_one_dollar_breach_across_many_events() {
    // AC-C + the dimension-keyed latch: breach_action = warn performs NO transition,
    // so the agent keeps running and EVERY subsequent event re-runs enforcement while
    // STILL over the cap. A dollar breach must fire AT MOST ONCE per scope per Run.
    // Cap $0.00003 (30 micros), 5 events: event 1 crosses, events 2–5 stay over.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "dollarwarn",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "dollarwarn",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    set_unit_rate(&facade, "dollarwarn");
    facade
        .set_config("dollarwarn", "budget.dollars.cumulative", "0.00003")
        .unwrap();
    facade
        .set_config("dollarwarn", "budget.breach_action", "warn")
        .unwrap();

    facade.start("dollarwarn").unwrap();

    // Wait until ALL 5 usage events have committed — every post-breach event ran
    // enforcement — then assert the dollar breach COUNT is exactly 1.
    wait_for_usage_rows(state.path(), "dollarwarn", 5, Duration::from_secs(30));

    let breaches = facade.budget_breach_events("dollarwarn").unwrap();
    let dollar: Vec<_> = breaches
        .iter()
        .filter(|b| b.dimension == BreachDimension::Dollars)
        .collect();
    assert_eq!(
        dollar.len(),
        1,
        "warn must record EXACTLY ONE dollar breach for a single crossing across 5 \
         events; got {}: {breaches:?}",
        dollar.len()
    );
    assert_eq!(dollar[0].action.as_str(), "warn");

    // The instance is STILL running (warn does NOT transition).
    assert_eq!(
        committed_state(state.path(), "dollarwarn").as_deref(),
        Some("running"),
        "warn must not transition the instance"
    );

    let _ = facade.stop("dollarwarn", Some(Duration::from_secs(5)));
}

#[test]
fn a_cost_cap_with_no_rate_is_inert_while_token_budgets_still_fire() {
    // AC-B (the honesty crux): a Cost Cap set WITHOUT a Rate is INERT — no dollar
    // enforcement — while TOKEN enforcement keeps working FULLY. We set a dollar cap
    // that WOULD breach (if a Rate existed) AND a token budget that WILL breach, with
    // NO Rate. The instance must pause on the TOKEN breach, and there must be NO
    // dollar breach event (the cap was inert, never a fabricated dollar breach).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "inert",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "inert",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    // A dollar cap but NO Rate → inert. A token budget that breaches at event 3
    // (90 tokens; 30 tokens/event).
    facade
        .set_config("inert", "budget.dollars.cumulative", "0.00001")
        .unwrap();
    facade
        .set_config("inert", "budget.tokens.cumulative", "90")
        .unwrap();

    facade.start("inert").unwrap();

    // The instance pauses — on the TOKEN breach (the dollar cap is inert).
    wait_for_state(
        state.path(),
        "inert",
        LifecycleState::Paused,
        Duration::from_secs(30),
    );

    let breaches = facade.budget_breach_events("inert").unwrap();
    // A token breach fired...
    assert!(
        breaches
            .iter()
            .any(|b| b.dimension == BreachDimension::Tokens),
        "the token budget must still fire (tokens work fully without a Rate)"
    );
    // ...and NO dollar breach (the cap with no Rate was inert — never fabricated).
    assert!(
        !breaches
            .iter()
            .any(|b| b.dimension == BreachDimension::Dollars),
        "a Cost Cap with no Rate must be inert — no dollar breach: {breaches:?}"
    );

    // The Fleet cost surface stays honestly ABSENT (no Rate ⇒ no dollar figure).
    let fleet = facade.fleet().unwrap();
    let entry = fleet.iter().find(|e| e.name.as_str() == "inert").unwrap();
    assert!(
        entry.usage.cumulative_dollars.is_none(),
        "no Rate ⇒ no derived dollar figure (honest inert view — AC-B)"
    );
    // The token budget view is present + real.
    let view = entry.budget.as_ref().expect("a token-budgeted instance");
    assert_eq!(view.cumulative_limit, Some(90));
    assert_eq!(view.cumulative_cost_cap, None, "the dollar cap stays inert");

    let _ = facade.stop("inert", Some(Duration::from_secs(5)));
}

#[test]
fn a_token_and_a_dollar_cap_each_fire_once_on_the_same_run() {
    // The dimension-keyed latch (Key design decision 3): a Run that trips BOTH its
    // token budget AND its dollar cap records ONE breach of EACH dimension (they
    // latch independently; the action is identical). Both cross on the same early
    // event; assert exactly one token breach + exactly one dollar breach.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_fake_manifest(
        manifest.path(),
        "both",
        &["--emit-usage", "5", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("both", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    set_unit_rate(&facade, "both");
    // Both breach on event 1: token cap 30 (30 tokens/event), dollar cap 30 micros
    // ($0.00003, 30 micros/event). Use warn so the instance keeps running and every
    // post-crossing event exercises the latch for BOTH dimensions.
    facade
        .set_config("both", "budget.tokens.cumulative", "30")
        .unwrap();
    facade
        .set_config("both", "budget.dollars.cumulative", "0.00003")
        .unwrap();
    facade
        .set_config("both", "budget.breach_action", "warn")
        .unwrap();

    facade.start("both").unwrap();
    wait_for_usage_rows(state.path(), "both", 5, Duration::from_secs(30));

    let breaches = facade.budget_breach_events("both").unwrap();
    let tokens = breaches
        .iter()
        .filter(|b| b.dimension == BreachDimension::Tokens)
        .count();
    let dollars = breaches
        .iter()
        .filter(|b| b.dimension == BreachDimension::Dollars)
        .count();
    assert_eq!(
        tokens, 1,
        "exactly one token breach across the run: {breaches:?}"
    );
    assert_eq!(
        dollars, 1,
        "exactly one dollar breach across the run: {breaches:?}"
    );

    let _ = facade.stop("both", Some(Duration::from_secs(5)));
}

#[test]
fn no_retroactive_repricing_a_rate_change_reprices_future_events_only() {
    // AC-A (the no-retro-repricing crux, end-to-end): start under a Rate, accrue some
    // cost, then RAISE the Rate; the already-metered events keep their ORIGINAL price
    // while only new events use the new Rate. We prove it by the derived cumulative
    // cost: with a high cap (no breach), emit under rate $1/1M, then raise to $9/1M,
    // and assert the cumulative cost is LESS than if the whole history had been
    // repriced at the new rate.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    // Many events so we can change the Rate mid-run and still have events left.
    write_fake_manifest(
        manifest.path(),
        "noretro",
        &["--emit-usage", "20", "--linger-ms", "600000"],
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "noretro",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    // Start under a $1/1M Rate on both directions (1 micro/token), a HIGH dollar cap
    // (no breach).
    set_unit_rate(&facade, "noretro");
    facade
        .set_config("noretro", "budget.dollars.cumulative", "1000.00")
        .unwrap();

    facade.start("noretro").unwrap();

    // Let at least 3 events commit under the ORIGINAL rate (cost 30 micros each).
    wait_for_usage_rows(state.path(), "noretro", 3, Duration::from_secs(30));
    let rows_at_change = usage_row_count(state.path(), "noretro");
    let cost_before = facade
        .fleet()
        .unwrap()
        .iter()
        .find(|e| e.name.as_str() == "noretro")
        .unwrap()
        .usage
        .cumulative_dollars
        .unwrap()
        .get();
    assert_eq!(
        cost_before,
        30 * rows_at_change as i64,
        "each early event is priced at $1/1M = 30 micros"
    );

    // RAISE the Rate to $9/1M on both directions (9 micros/token → 270 micros/event).
    facade
        .set_config("noretro", "cost.rate.input", "9.00")
        .unwrap();
    facade
        .set_config("noretro", "cost.rate.output", "9.00")
        .unwrap();

    // Let all 20 events commit (the later ones priced at the new rate).
    wait_for_usage_rows(state.path(), "noretro", 20, Duration::from_secs(30));

    let cost_after = facade
        .fleet()
        .unwrap()
        .iter()
        .find(|e| e.name.as_str() == "noretro")
        .unwrap()
        .usage
        .cumulative_dollars
        .unwrap()
        .get();

    // Had the WHOLE history been re-priced at $9/1M, all 20 events would cost
    // 270 × 20 = 5400 micros. With no retro-repricing, the early events kept their
    // 30-micro price, so the total is STRICTLY LESS than the fully-repriced figure.
    let fully_repriced = 270 * 20;
    assert!(
        cost_after < fully_repriced,
        "no retro-repricing: cumulative {cost_after} must be < the fully-repriced \
         {fully_repriced} (early events kept their original rate)"
    );
    // And it is at LEAST the early cost (30/event) for every event plus the uplift on
    // the later ones — strictly greater than a flat 30/event across all 20.
    assert!(
        cost_after > 30 * 20,
        "the later events DID reprice up: cumulative {cost_after} must exceed a flat \
         30-micro/event total (600)"
    );

    let _ = facade.stop("noretro", Some(Duration::from_secs(5)));
}

#[test]
fn only_the_ingestion_choke_point_evaluates_a_cost_cap() {
    // AD-7 companion invariant for DOLLARS (mirroring budget.rs's token audit): NO
    // code path other than the supervisor's ONE ingestion→commit choke point may
    // EVALUATE a Cost Cap. A source scan proves every CALL to
    // `CostEvaluator::evaluate(` lives ONLY in `domain/supervisor.rs` (+ its own home
    // `domain/cost.rs` unit tests). If a future change scatters a second enforcement
    // site, this fails. Pure source scan (no OS cfg); runs on every OS.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut eval_files = std::collections::BTreeSet::new();
    visit_rs(&src, &mut |path, text| {
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//")
                || trimmed.starts_with("///")
                || trimmed.contains("fn evaluate(")
            {
                continue;
            }
            if line.contains("CostEvaluator::evaluate(") {
                eval_files.insert(
                    path.strip_prefix(&src)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    });
    let allowed: std::collections::BTreeSet<String> = ["domain/supervisor.rs", "domain/cost.rs"]
        .into_iter()
        .map(String::from)
        .collect();
    let violations: Vec<&String> = eval_files.difference(&allowed).collect();
    assert!(
        violations.is_empty(),
        "CostEvaluator::evaluate is called outside the ingestion choke point: {violations:?}"
    );
    assert!(
        eval_files.contains("domain/supervisor.rs"),
        "the enforcement choke point must call CostEvaluator::evaluate; callers: {eval_files:?}"
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
