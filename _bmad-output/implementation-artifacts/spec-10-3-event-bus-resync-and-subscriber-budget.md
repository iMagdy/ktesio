---
title: 'Event-bus resync helper + ratified subscriber-active budget'
type: 'feature'
created: '2026-09-10'
status: 'done'
review_loop_iteration: 0
baseline_commit: d07420a
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Two ratified deferrals from story 7-2/7-5 remain open. (a) The event bus delivers at-most-once in the crash window between durable append and publish — the documented recourse is "query APIs," but the host must hand-roll the backfill from three different log formats. (b) The perf budgets are measured with zero subscribers; a host using the epic's own subscription surface has no budget statement covering fan-out cost (measured-and-reported only, ~unquantified).

**Approach:** (a) An **additive resync helper** on the facade — `resync_events(name, after) -> Vec<EngineEvent>`-shaped (exact signature the implementer designs): reads the committed logs/ledger via the same committed-truth machinery the tests use, converts to `EngineEvent`s, letting a host heal the crash window in one call — the documented at-most-once caveat then reads "recoverable via resync_events." (b) A **subscriber-active budget**: the perf harness measures read latency and CPU/RSS with one active subscriber (already implemented as the reported addendum in 7-5's harness) and the measurement is **ratified into a gate** with a documented budget from the observed numbers, replacing the measured-and-reported-only policy for the subscriber addendum.

**Autopilot approval:** spec self-approved by the orchestrator 2026-09-10 under Islam's standing autonomous instruction (6-2 precedent).

## Boundaries & Constraints

**Always:**
- The resync helper reads COMMITTED truth only (the same readers the tests/perf use) and converts to `EngineEvent`s — no new event kinds, no schema changes, no bus changes.
- The helper is additive facade/Engine API, documented in docs/embedding.md (the host-facing page) + testing.md, with the crash-window semantics section updated to name it.
- The subscriber-active budget: gate added to the perf harness's CI mode with a documented tolerance (same policy shape as the CPU gate); the ratified number comes from the observed measurement recorded in docs/testing.md.
- Standings: coverage ≥95%, docs currency, boundary gate, fmt/clippy.

**Ask First:**
- Any bus/publish-path change (the resync is a READ-side helper; the bus's at-most-once window itself stays as documented).
- If the backfill ordering cannot be made consistent with live-event ordering (dedup markers needed), present the design choice before implementing.

**Never:**
- No change to the event bus, publish points, or existing subscribe semantics; no new dependencies.

## Code Map

- `crates/ktesio-engine/src/domain/supervisor.rs` — the committed append paths (instance.log transitions, breaches.log) whose exact formats the tests already parse.
- `crates/ktesio-engine/src/domain/bus.rs` — EngineEvent variants (transition/budget_breach/usage_update) the helper must produce.
- `crates/ktesio-engine/src/engine.rs` — facade surface for the new API; `crates/ktesio-conformance/src/uj3.rs` + `tests/events_subscription.rs` — the existing committed-log readers (replay/parsing logic to reuse, now shared since 10-1).
- `crates/ktesio-engine/examples/perf-budgets.rs` — the subscriber addendum to gate (7-5's harness).

## Tasks & Acceptance

**Execution:**
- [ ] `crates/ktesio-engine` — the resync/backfill helper (committed logs → EngineEvents, per-instance, ordered) + facade exposure + docs.
- [ ] `crates/ktesio-engine/tests/` — helper tests: backfill equals the committed logs exactly; combined with a live subscriber, no duplicates across (backfilled ∪ received) for the same window.
- [ ] `crates/ktesio-engine/examples/perf-budgets.rs` — subscriber-active gate with the ratified budget + docs/testing.md budget table update.
- [ ] `docs/embedding.md` — the resync helper documented as the crash-window remedy.

**Acceptance Criteria:**
- Given committed events a subscriber missed (crash window), when the host calls the resync helper, then it receives exactly those events as EngineEvents, in commit order.
- Given the perf harness, when run in CI mode, then the subscriber-active overhead is gated (not merely reported) at the documented budget.
- Given the workspace battery, then everything passes.

## Spec Change Log

## Design Notes

- Ordering consistency: the bus is publish-ordered (== commit order under the supervisor mutex); the resync replays commit order from disk. A host that backfills THEN subscribes live gets clean continuity; a host that subscribes first may see overlap — document "subscribe after backfill" as the contract, or add a sequence watermark if the implementer finds overlap unmanageable.
- The budget number: 7-5's harness already prints subscriber-active deltas — ratify from those observations with CI tolerance (documented, same shape as the CPU policy).

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. resync tests
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `cargo +1.96.1 tree -p ktesio -e normal,build` -- unchanged
