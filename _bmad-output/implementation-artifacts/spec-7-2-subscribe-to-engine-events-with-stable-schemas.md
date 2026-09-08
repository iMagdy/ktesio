---
title: 'Subscribe to engine events with stable schemas'
type: 'feature'
created: '2026-09-09'
status: 'done'
review_loop_iteration: 0
baseline_commit: 68f8294
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** A Host (UJ-3) can drive the engine but cannot OBSERVE it: state transitions, usage updates, breaches, and crash/restarts are only readable by polling committed logs. FR-33/AD-14 require a subscription surface with ordered, schema-stable payloads.

**Approach:** A subscription surface on the engine: `Engine::subscribe()` (facade-exposed) hands out receivers on a bounded broadcast channel fed at the SAME commit points where the event logs are appended — subscribers observe exactly the committed truth in publish order, per-instance FIFO. The payload is a wrapping `EngineEvent` enum over the EXISTING AD-14 structs verbatim (TransitionEvent, BudgetBreachEvent, UsageEvent, UsageUpdateEvent — no new schema families, all carrying their own `schema_version`). Slow subscribers are the documented policy: the channel is bounded, `send` never awaits subscriber capacity (supervision cannot stall), and a lagging receiver gets tokio's `Lagged` marker instead of the dropped events.

**Autopilot approval:** spec self-approved by the orchestrator 2026-09-09 under Islam's standing autonomous-sprint instruction (recorded per the 6-2 precedent); frozen-block semantics stand for the story's duration.

## Boundaries & Constraints

**Always:**
- Publish events at the log-append commit points (transition append, breach append, usage ingestion) — a subscriber NEVER sees an event that isn't committed, and per-instance ordering is FIFO in commit order.
- Payloads are the existing versioned structs verbatim; the wrapper carries no new wire vocabulary. Serde derives so a future wire surface (7-2's own AC says schema-validated) can round-trip.
- Bounded capacity as a named const with docs; `send` is non-blocking by construction (broadcast); a `Lagged` receiver keeps working (resyncs at current tail).
- Sync consumers (blocking facade) get a blocking recv; async consumers can use the raw receiver. Slow-subscriber behavior documented AND tested: a stalled receiver must not stall supervision (the instance keeps transitioning/ingesting) and must observe `Lagged`.
- Tests: (a) subscriber sees the UJ-3 flow's transitions + breach + usage in commit order with payloads validating against the structs; (b) crash/restart leg delivers the crashed/restarted transitions; (c) slow-subscriber no-stall + Lagged; (d) multiple subscribers fan out independently; (e) per-instance FIFO with several instances interleaving.
- Standings: coverage ≥95%, docs currency (testing.md + engine module docs), cross-platform, boundary gate untouched.

**Ask First:**
- Any change to the existing event structs or their schema versions (this story wraps, never amends).
- If the commit points cannot host a publish hook without restructuring the supervision loop, HALT with the finding.

**Never:**
- No wire/HTTP surface, no persistence changes, no new event kinds, no kt CLI surface.

## Code Map

- `crates/ktesio-engine/src/domain/event.rs` -- TransitionEvent (:435), BudgetBreachEvent (:498), the schema consts, TransitionCause variants incl. `crashed`/`restarted`.
- `crates/ktesio-engine/src/domain/usage.rs` -- UsageEvent (:107), UsageUpdateEvent (:202).
- `crates/ktesio-engine/src/domain/supervisor.rs:3115/3183` -- `append_event`/`append_breach_event`: the commit points where publishes hook in (usage ingestion site is the third).
- `crates/ktesio-engine/src/engine.rs:244` -- `blocking()`; the facade pattern for sync exposure.
- `crates/ktesio-conformance/src/uj3.rs` -- the shared-flow pattern the subscription test can reuse for driving a mini flow.
- Engine tokio dependency exists (AD-13); broadcast channels are `tokio::sync::broadcast`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-engine` -- `EngineEvent` wrapper + `subscribe()` surface + publishes at the three commit points + capacity const + blocking recv on the facade.
- [x] `crates/ktesio-engine/tests/` (new `events_subscription.rs` or in uj3-adjacent file) -- the five test families (flow-order+validation, crash leg, slow-subscriber no-stall+Lagged, fan-out, per-instance FIFO).
- [x] `docs/testing.md` + engine module docs -- the subscription contract, capacity/Lagged policy, commit-point guarantee.

**Acceptance Criteria:**
- Given a subscriber, when the UJ-3 mini-flow runs, then every transition, the breach, and usage updates arrive in publish order, per-instance FIFO, each payload deserializing into its versioned struct.
- Given a subscriber that stops receiving, when more than the channel capacity of events publishes, then supervision continues unaffected (the instance reaches its expected state) and the receiver observes `Lagged` then resyncs.
- Given two subscribers, when events publish, then both independently receive the full sequence (fan-out).
- Given the workspace battery (fmt, clippy, tests, tarpaulin ≥95%, check_docs, test_automation, boundary gate), then everything passes.

## Spec Change Log

## Design Notes

- Broadcast (not mpsc-per-subscriber) is the natural fit: bounded, non-blocking send, built-in Lagged semantics, Clone receivers. Capacity: pick a const that comfortably exceeds a single Run's event count in tests (e.g. 1024) and document the memory bound rationale.
- Publishing happens on the supervisor/ingestion side (possibly sync context): `broadcast::Sender::send` is sync and non-blocking — no await needed at commit points.
- The wrapper: `#[serde(tag = "kind", rename_all = "snake_case")]` enum with variant payloads as the verbatim structs; `schema_version` lives inside each payload already — do not duplicate at wrapper level.
- The blocked-facade recv: reuse whatever bridge the facade uses for its sync surface; document that a blocking recv across a runtime worker is the caller's error (same contract as the rest of the facade).

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. the five subscription test families
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `cargo +1.96.1 tree -p ktesio -e normal,build` -- unchanged
