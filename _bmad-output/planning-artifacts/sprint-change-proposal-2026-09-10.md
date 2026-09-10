# Sprint Change Proposal — Hardening & Consolidation (next sprint)

**Date:** 2026-09-10 · **Trigger:** sprint complete (all 9 epics, 35 stories, v0.7.0 released) · **Mode:** Batch (autonomous, per Islam's standing instruction)

## 1. Issue Summary

The sprint that carried Ktesio from the pivot through contract v1, the embedding surface, and the
v0.7.0 publication is complete: 35/35 stories, 9/9 epics, zero backlog. The queue of deferred items
accumulated across epics 6–7 (recorded in `deferred-work.md` and the retro issues) now constitutes
the natural next sprint — but it has never been organized into an epic. Left unorganized, these
items share the fate AI-66 documented: unowned deferrals never land.

Evidence: #164 (fixture census grew to 5+ builders + 2 drains across the epic-7 suites alone), the
two allowlisted AD-12 stderr diagnostics (7-3's unclosed findings, also missing from
deferred-work.md until the epic review added them), the event-bus crash-window at-most-once
limitation with its deferred resync helper (7-2), and the subscriber-active overhead measured but
never budgeted (7-5).

## 2. Impact Analysis

- **Epic impact:** all nine epics are complete and unaffected. A **new epic (10 — Hardening &
  Consolidation)** is added to `epics.md`; no existing epic changes.
- **Story impact:** three stories drafted (below) from the queued items; the TCK Http-interaction
  leg remains explicitly deferred (it needs engine interaction-over-HTTP support — a new
  capability, not hardening).
- **Artifact conflicts:** none. PRD goals are unaffected (everything proposed is additive
  hardening of shipped surfaces). Architecture gains one documented additive extension (a
  diagnostic sink port, 10-2). No UX impact (library surface only).
- **Technical impact:** test-suite restructure (10-1), one additive public API (10-2, within the
  frozen contract's additive rules — engine-only, not adapter-api), one additive facade helper +
  a ratified budget (10-3).

## 3. Recommended Approach

**Option 1 — Direct Adjustment:** add Epic 10 (three stories) to the existing plan. Effort:
Medium. Risk: Low (all additive, no behavior change to shipped surfaces, full battery after each
story). Rollback: not applicable (nothing to revert). MVP review: not applicable (MVP shipped and
published).

## 4. Epic 10 (draft) — Consolidate & Harden the Embedding Surface

**Story 10-1 — One test-support home for the embedding suites (#164).**
Consolidate the manifest builders (5+ near-identical: uj3::write_flow_manifest, three in
events_subscription.rs, perf-budgets write_heartbeat_manifest), the lag-accumulating drain
helpers (2 implementations), and the committed-log readers into a single parameterized
test-support module in `ktesio-conformance`. The embedding quickstart stays deliberately
standalone. AC: no duplicate builder/drain logic outside the support module (quickstart exempt);
all suites green; boundary graph unchanged.

**Story 10-2 — Host-provided diagnostic sink (additive public API).**
`Engine` gains an optional diagnostic sink: the two AD-12 diagnostics (DC-10 memory-delivery
notice, enforcement breadcrumb) route to the sink when a host provides one, defaulting to stderr
when not. Removes both embed_clean audit allowlist entries (the `eprintln!` sites become sink
calls). AC: sink receives both diagnostics; default stderr behavior unchanged; allowlist entries
removed; additive API documented in docs/embedding.md + testing.md.

**Story 10-3 — Event-bus resync helper + ratified subscriber-active budget.**
(a) A subscribe-with-backfill helper: on subscribe, backfill from the committed logs
(transitions, breaches, usage rows) before live events — turning the documented
crash-window-at-most-once limitation into a one-call remedy. (b) Measure subscriber-active
overhead in the perf harness and ratify a subscriber-active budget from the measurement (gate at
a generous multiple pending field data, documented like the 7-5 tolerance policy). AC: backfill
test (crash-window simulation: committed-but-unpublished events are delivered by the helper);
budget ratified and documented.

**Explicitly deferred:** TCK Http-interaction leg (needs engine interaction-over-HTTP — a new
capability story, proposed separately when a real Http-native agent demands it); semver-baseline
retire-or-keep revisit (stays with the second published release).

**Sprint pre-work (sequenced first):** [ER] retrospectives — epic-5 (owed; no artifact exists)
and epic-7 (fresh, recommended). Lessons feed the story specs above.

## 5. Implementation Handoff

**Scope: Minor-to-Moderate** — direct implementation by the Developer agent (bmad-build, per
story: 3 lenses + patches), with one backlog addition (Epic 10) coordinated through
sprint-planning regenerate. Success criteria: per-story batteries green, coverage ≥95%, boundary
graph unchanged, docs current; sprint closes with the deferred-work entries resolved and the
audit allowlists shrunk.

## Handoff routing

Developer agent (bmad-build per story) · Retrospective agent (pre-work) · Orchestrator
(backlog regenerate + per-epic PR).
