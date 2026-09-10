# Sprint Change Proposal — Technical Debt & Process Cleanup (Epic 11)

**Date:** 2026-09-10 · **Trigger:** 65 open retro action items across epics 1–7, accumulated over the sprint · **Mode:** Batch (autonomous, per Islam's standing instruction)

## 1. Issue Summary

The sprint's retrospectives produced 65 open action items across 7 epics. Most are diagnostic polish, test-gate additions, or documentation fixes — individually small but collectively a maintainability tax. The epic-6 retro proved that unowned deferrals have a zero completion rate across three+ epics. This proposal organizes them into 6 thematic stories within one cleanup epic, each sized for a single focused session.

## 2. Already Resolved (mark done)

| Item | Resolved by |
|---|---|
| AI-3 (semver cache key) | `b590dc8` / PR #174 — version-keyed cache + verify-or-reinstall |
| AI-6 (contract_version strict parse) | `dc29e86` / PR #157 — `STRICT_SEMVER_REQUIREMENT` const |
| AI-64 (mutation pass mandate) | Applied in 6-6; standing rule; formally closed on #168 |
| #168 (bind-or-retire AI-63/65/66/69) | Closed with 4 ratified dispositions |
| epic-6-retro-item-8 (tracking hygiene) | Landing in the cleanup epic's own chore commits |

## 3. Epic 11 — Technical Debt & Process Cleanup

### Story 11-1: Engine robustness batch (12 items, med)

Engine correctness/polish from epics 1–3. Each is a small targeted fix + test.

| Item | Fix |
|---|---|
| AI-4 | RegistryError::Io names the offending relative path, not the env-var placeholder |
| AI-7 | resume() on an Unsupported-pause instance: fail-fast → honest diagnostic with remediation (don't strand) |
| AI-8 | guaranteed pause without in-memory handle: record `pause-unavailable` cause, don't silently return Ok |
| AI-9 | pause(): persist state=paused BEFORE signaling SIGSTOP (match stop()'s persist-first order) |
| AI-12 | poll_once: persistent poll errors surface as crash-detection input, not silent `None` |
| AI-13 | adopted process exit: record the exit-code-unavailable fact in the crash cause |
| AI-14 | fingerprint-read failure at spawn: don't write start_time=0 sentinel |
| AI-15 | `show --json`: single-instance lookup instead of O(N) fleet scan |
| AI-16 | `fleet_entry_for`: batch-read spawn records while holding the lock (kill the N+1) |
| AI-41 | ledger INSERT failure: retry/diagnose, don't silently drop the usage event + advance cursor |
| AI-44 | adopt_orphans: re-evaluate budgets after adoption (the crash-gap) |

**Owner:** first engine-touching story. **Acceptance:** each fix ships with a test; battery green.

### Story 11-2: Config & secrets batch (6 items, med)

| Item | Fix |
|---|---|
| AI-24 | set_config: temp-file + rename (atomic write) |
| AI-27 | env target shadowing: warn when a mapped env target overwrites a base-launch env var |
| AI-28 | config-file render + set_config: same atomic write pattern |
| AI-33/39 | secret:NAME → flag target: warn at config-set time (steer to env/file); runtime enforcement via the existing audit |
| AI-26 | `config set` leading-dash values: accept via `--` separator (clap standard) |

**Owner:** first config/IO-touching story.

### Story 11-3: Memory robustness batch (4 items, med)

| Item | Fix |
|---|---|
| epic-5-retro-item-1 | memory.dir strip gap: strip in the invocation-override branch too + test (A1) |
| epic-5-retro-item-3 | migration crash-atomicity: BEGIN IMMEDIATE or idempotent DDL + comment fix (B3) |
| epic-5-retro-item-4 | registry reverse-conflict test + deferred-work direction fix (A5) |
| AI-46 | engine-observed adoption: clear the stale base_url on adoption (the stranded-listener fix) |

**Owner:** first engine/memory-touching story.

### Story 11-4: Docs, docs-gate & process batch (9 items, low)

| Item | Fix |
|---|---|
| AI-18 | Document the honest-state/surfaced-not-silent pattern in AGENTS.md |
| AI-19 | Adopt the review rule: deferred-as-unreachable must be proven across every surface |
| AI-21 | Codify the away-mode/interruption recovery drill |
| AI-22 | Contributor docs: MSRV-pin vs CI-stable split + mise/RUSTUP_TOOLCHAIN note |
| AI-25 | `config get` table-prefix: name the child leaves that exist under it |
| AI-30 | config_json docstring: fix accessor names |
| AI-43 | Compact human `list`: surface the Metering Source (AC-C literal wording fix) |
| AI-45 | Fleet list dollar cell: same truncation-label fix as the Budget cell |
| AI-34 | Fix rustdoc --document-private-items failure on kt/src/main.rs |

**Owner:** tech-writer / docs-touching story.

### Story 11-5: Cross-platform & CI batch (6 items, med)

| Item | Fix |
|---|---|
| AI-29 | Unix-only survival tests: Windows equivalents or documented per-OS honesty |
| AI-35/38 | _live test cross-OS robustness (fake_agent .exe, readiness handshake) |
| AI-37 | Run the 3-OS matrix earlier (per-story or nightly on feature branches) |
| AI-54 | Codify full cross-OS + coverage CI on every feature-branch commit |
| epic-7-retro-item-2 | Live-docs probe: scheduled check that docs.ktesio.dev serves the newest page |
| AI-71 | Restore the workspace coverage gate to green (94.94% → ≥95%) |

**Owner:** first CI/infra-touching story.

### Story 11-6: Supervision & operations decisions (5 items, needs Islam)

| Item | Decision needed |
|---|---|
| AI-20 | PRODUCT DECISION: daemon/detach for durable cross-CLI supervision — build it or document the per-invocation model as intentional |
| AI-46 | (overlaps 11-3) engine-observed adoption stranding: clear stale base_url on adoption |
| AI-47 | HTTPS upstream support for engine-observed metering (v1 is HTTP-only) |
| AI-48 | Latent tracing exposure: confirmed safe (no subscriber in tree); document as a dependency-audit checkpoint |
| AI-36 | Dependabot vulnerabilities on main: review/triage |

**Owner:** Islam (product decisions) + dev (implementation where commissioned).

### Story 11-7: Orchestration & review process batch (7 items, low)

| Item | Fix |
|---|---|
| AI-17 | test_automation.py: tighten the assertIn pins + lazy-install placement |
| AI-19 | Adopt the review rule: deferred-as-unreachable proven across every surface |
| AI-55 | Extend the two-pass review default to public release-surface changes |
| AI-56 | Sync sprint-status 8-1 entry to the ratified rewrite |
| AI-57 | Codify the resumable-subagent recovery flow as a standing playbook pattern |
| AI-58 | Document the draft-then-ratify pattern for architecture-decision corrections |
| epic-6-retro-item-9 | Bind or retire: the carry-forward items that survived #168's ratification |

**Owner:** orchestrator / process.

## 4. Impact Analysis

- **PRD:** no conflicts — every item hardens or polishes an already-shipped capability.
- **Architecture:** no structural changes — the engine fixes are local (atomic writes, error ordering, poll-error surfacing); the sink from 10-2 already established the diagnostic routing pattern.
- **Boundary gate:** unchanged (all fixes are within existing crates).
- **Dependencies:** none added.

## 5. Recommended Approach

**Direct Adjustment** — add Epic 11 with 7 stories to the existing plan. Each story is a themed batch sized for one focused session; the fix-per-item is documented in the tables above. Execution order: 11-1 (engine) → 11-3 (memory) → 11-2 (config) → 11-5 (CI/docs) → 11-4 (docs) → 11-7 (process) → 11-6 (decisions).

## 6. Implementation Handoff

**Scope: Moderate** — thematic batching across 7 stories, each directly implementable by the Developer agent (bmad-build, 3-lens review per story). Story 11-6 requires Islam's product decisions before its implementation items can be commissioned.

## 7. Success Criteria

- All 65 items marked done or formally retired with rationale
- Battery green after every story (fmt/clippy/tests/tarpaulin/check_docs/test_automation)
- Zero new open action items created by the cleanup itself
- Per-epic PR (one PR covering all of Epic 11, per the standing flow)
