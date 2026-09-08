---
title: 'Drive every capability through the library alone'
type: 'feature'
created: '2026-09-07'
status: 'done'
review_loop_iteration: 0
baseline_commit: 7b1a70c2777c689dc20dbba9392b7845ea208bfa
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** FR-31's promise — a host drives the full UJ-3 flow (register→configure→cap→start→breach→pause→stop) through the engine library with no CLI — is unproven. The Blocking facade exists and kt uses it, but no test demonstrates the whole journey library-only, and nothing pins the library path as *behaviorally identical* to the CLI path.

**Approach:** A **library host test** (`crates/ktesio-engine/tests/uj3_library_host.rs`) drives the full UJ-3 flow through the engine's public Blocking facade only — register a manifest adapter, set config, arm a token budget + dollar cap, start, emit usage past the ceiling, assert the breach pauses the instance with the recorded breach event, then stop — asserting fleet/usage/breach reads along the way. The **expected outcomes are pinned once** in a shared `uj3` module in `ktesio-conformance` (the established dev-dependency of both test suites) and consumed by BOTH the host test and a CLI journey test in `crates/kt/tests/agent_cli.rs`, so the two paths are provably behaviorally identical. Any capability the flow finds unreachable through the facade is closed in-story.

## Boundaries & Constraints

**Always:**
- The host test drives ONLY the engine's public API (`Engine::open` + `blocking()` facade) — no `kt`, no private items. `ktesio-conformance` may be used as TEST INFRASTRUCTURE (shared expectations/fixtures), never as a driving surface; this is documented in the module docs (dev-deps don't cross the shipping boundary gate).
- Shared expectations live in ONE place (`ktesio-conformance`'s `uj3` module): fixture manifest shape, config keys, budget/cap numbers, expected states, transition/breach-event expectations, and fleet/usage read expectations. Both suites call the same assertion helpers; neither re-states the expectations inline.
- The breach leg proves the real enforcement chain: usage event → BudgetEvaluator → pause with `BudgetExceeded` cause + a `budget_breach_events` record + honest dollar labeling (Rate configured) — polled against committed state, never wall-clock sleeps.
- The reachability inventory is explicit in the test's module docs: §4.1 (register/fleet/show reads), §4.2 (start/breach-pause/stop), §4.3 (set_config + effective read), §4.5 (budget/cap/rate) exercised by the flow; §4.4 (memory) and §4.6 (interaction) facade-reachability is already proven by the engine's hermes e2e and is cited, not re-tested; §4.7 is the manifest contract itself; §4.9 is out of scope (epic 8).
- Standings: coverage ≥95%, docs currency, cross-platform, graceful degradation; the boundary gate (`cargo tree -p ktesio` allowlist) must stay untouched.

**Ask First:**
- Any engine API change beyond what closing an unreachable capability requires (additions are additive, documented, and tested).
- If the flow finds a capability genuinely unreachable through the facade, the fix is in-story — but if closing it would change kt's CLI behavior or a frozen key-set, HALT.

**Never:**
- No kt CLI surface changes; no adapter-api changes; no event-schema changes (7-2 owns subscriptions).
- No new workspace dependencies for the host path.

## Code Map

- `crates/ktesio-engine/src/engine.rs:148/244/1030-1194` -- `Engine::open`, `blocking()`, and the facade surface the host drives (register/register_with_adapter, set_config, effective_config, start/stop/pause/resume, fleet, budget_breach_events, transition_events, instance_status).
- `crates/kt/tests/agent_cli.rs:2013` -- `uj1_governance_journey_through_documented_cli_commands`: the CLI journey whose behavioral expectations the shared module must mirror.
- `crates/ktesio-engine/tests/budget.rs` / `cost.rs` -- the budget/cap config keys, fake_agent `--emit-usage` traffic pattern, and the committed-state polling pattern to reuse.
- `crates/ktesio-conformance/src/lib.rs` -- the shared dev-dep home; the new `pub mod uj3` lands here (conformance is `publish = false`; no stability burden, but module docs state its test-infrastructure role).
- `crates/ktesio-conformance/src/tck.rs:830` (`write_probe_manifest`) and `crates/ktesio-engine/tests/*.rs` fixture builders -- the manifest fixture shape for the host's agent (NOTE: the fixture-consolidation deferral #164 stands — build the fixture in the uj3 module, do NOT refactor the other 16 builders).
- `.github/workflows/ci.yml` boundary job -- must stay green; the host adds no kt-graph edges.

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-conformance/src/lib.rs` (+ new `src/uj3.rs`) -- the shared UJ-3 module: fixture-manifest authoring, the flow's expected values (config keys/values, budget + rate + cap numbers, expected token/dollar totals, expected states and causes), and assertion helpers over observed reads -- expectations pinned once.
- [x] `crates/ktesio-engine/tests/uj3_library_host.rs` -- the host test: full flow via the Blocking facade over a hermetic temp root; consumes the uj3 module for every assertion; module docs carry the §4.1-4.7 reachability inventory.
- [x] `crates/kt/tests/agent_cli.rs` -- a CLI journey test driving the SAME flow through documented `kt` commands, consuming the SAME uj3 assertions -- the behavioral-identity half.
- [x] Any unreachable capability found -- close it (additive engine API + test) -- FR-31's closure clause.

**Acceptance Criteria:**
- Given the host test, when it runs, then the full flow completes library-only and every assertion comes from the shared uj3 module.
- Given the CLI journey test, when it runs, then the same flow through documented commands passes the same shared assertions.
- Given the flow's breach leg, when usage crosses the token ceiling (and separately the dollar cap is proven by the shared expectations already covered in cost.rs's matrix), then the instance pauses with `BudgetExceeded`, the breach event is recorded, and fleet/usage reads report honestly.
- Given the boundary gate, when CI runs, then kt's normal/build graph is unchanged.
- Given the full battery (fmt, clippy, tests, tarpaulin ≥95%, check_docs, test_automation), then everything passes.

## Spec Change Log

## Design Notes

- Budget/cap config keys and the emission pattern: copy the values used by budget.rs/cost.rs (proven paths) into the uj3 module rather than inventing new ones.
- The two tests drive DIFFERENT roots (the host uses its own temp state root; the CLI test uses the kt harness's isolated home) — the shared module asserts on OBSERVED READS (state, events, fleet totals), never on shared filesystem paths.
- fake_agent is built by CI before tests (existing contract); the uj3 fixture manifest follows the established `contract_version = "1.0.0"` + capabilities + metering shape.

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. the host + CLI journey tests
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `cargo +1.96.1 tree -p ktesio -e normal,build` -- unchanged allowlist
