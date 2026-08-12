---
stepsCompleted: [1, 2, 3, 4, 5, 6]
status: complete
verdict: READY
documentsIncluded:
  prd: _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md
  prdAddendum: _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/addendum.md
  architecture: _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md
  epics: _bmad-output/planning-artifacts/epics.md
  ux: none (by design — CLI/engine product, PRD review confirmed no UX phase)
executionNote: "Headless-conservative run inside the parent orchestrator (Islam's standing 'proceed inline, non-blocking' direction; flapping platform classifier). Menus auto-continued with [C]; deviations logged here."
---

# Implementation Readiness Assessment Report

**Date:** 2026-07-02
**Project:** ktesio

## Document Inventory

Disk inventory verified via find (Bash available at this step):

**PRD (whole, run-foldered — no sharded duplicate):**
- `prds/prd-ktesio-2026-07-02/prd.md` (status: review) + `addendum.md` + `review-rubric.md` + `.memlog.md`

**Architecture (whole — no sharded duplicate):**
- `architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` (status: final) + `.memlog.md` + `reviews/` (lint+rubric, version-verification, adversarial-incompatibility)

**Epics & Stories (whole — no sharded duplicate):**
- `epics.md` (status: complete; 8 epics, 37 stories, stepsCompleted [1,2,3,4])

**UX Design:** none — intentional (CLI + embeddable library; no GUI surface). Not a gap.

**Upstream context (not assessed, referenced):** `briefs/brief-ktesio-2026-07-02/` (brief + addendum + memlog).

**Duplicates:** none. **Missing:** none required.

## PRD Analysis

*Source: prd.md (status: review), read in full. Extraction below states each requirement's capability; full text with testable consequences lives in the PRD §4–§5.*

### Functional Requirements

FR-1: Register an Agent Instance from an installed Adapter, unique Fleet name, Agent Home created, multi-instance per kind
FR-2: Isolated Agent Home per instance; no cross-instance leakage; concurrent same-kind instances
FR-3: Unregister/remove with retain-or-delete; stop-first or --force; no orphans
FR-4: Fleet visibility (name/kind/state/budgets/ledger totals; ≤2s freshness; --json)
FR-5: Start via Adapter with effective config, Skill Set, Memory Backing, budgets applied
FR-6: Stop graceful→forced (default 30s window); no surviving processes cross-platform
FR-7: Pause/resume with honest guaranteed/best-effort/unsupported semantics, surfaced not silent
FR-8: Defined state machine identical across Agents; uniform invalid-transition errors; every transition emits an event
FR-9: Crash detection + Restart Policy (never/on-failure/always, backoff, crash-loop bound)
FR-10: State persistence across Engine restarts/reboots; ledger loss ≤1s (superseded bound per spine AD-6)
FR-11: Layered config model with deterministic precedence (defaults<kind<instance<invocation)
FR-12: Adapter config mapping to native mechanisms; `agent.*` pass-through verbatim + unvalidated
FR-13: Effective-config inspection with per-value source-layer provenance
FR-14: Secrets never logged/echoed/unmasked; --reveal acknowledgment for machine output
FR-15: Attach/detach one Memory Backing, uniform commands, not while running
FR-16: v1 backings: `filesystem` (byte-durable managed dir) + `native` (explicit delegation)
FR-17: Memory portability boundary explicit (guarantees vs delegation; Agent Home export carries backing)
FR-18: Token Budgets per-run + cumulative; live-changeable
FR-19: Metering ingestion per declared source (self-reported | engine-observed); no-metering Adapters rejected at registration
FR-20: Rate supply (input/output $/1M); dollar derivation; no retroactive repricing; inert-with-notice without Rate
FR-21: Cost Cap enforcement via Breach Action (ratified default pause; per-instance pause/stop/warn) within latency bound
FR-22: Usage/cost visibility per instance + Fleet (tokens by scope, dollars, headroom, --json, totals = Ledger)
FR-23: Estimate honesty — every dollar labeled estimated|reconciled (type-enforced)
FR-24: Send text input uniformly; unsupported interaction fails fast quoting Capability Declaration
FR-25: Stream output + retained logs (rotated 10MB×3, timestamped, attributed)
FR-26: --json on every read command; stable documented exit codes; schema-compatibility tested
FR-27: Published versioned Adapter Contract + machine-readable per-OS Capability Declaration + conformance test-kit
FR-28: Hermes reference Adapter end-to-end (lifecycle/gateway, config, self-reported metering, memory, interaction)
FR-29: Second-agent paper validation before contract freeze — opencode (Islam-selected), characterization first
FR-30: Contract semver; incompatible-version rejection naming both versions
FR-31: Engine library exposes every capability through the Embedding Interface
FR-32: kt consumes only the public Embedding Interface (CI-enforced embeddability proof)
FR-33: Host event subscription (state/usage/breach/crash) with stable versioned payloads
FR-34: Engine embeds clean (no TTY, no prompts, no global-state collisions)
FR-35: Provision Skills to an instance (git/local, commit-locked, reproducible; adapter informed)
FR-36: Skill Set lifecycle per instance (list/upgrade/remove/integrity+remediation)
FR-37: Legacy commands functional with exactly one stderr deprecation notice per invocation
FR-38: Published deprecation lifecycle (announce → ≥90-day/one-minor window → removal at stated version)
FR-39: `kt` name + channels continuity (crates.io, Homebrew, install scripts)

**Total FRs: 39**

### Non-Functional Requirements

NFR-1: Resilience — agent crashes never crash Engine/kt; orphan cleanup/adoption; graceful per-instance degradation
NFR-2: Cross-platform parity (Linux/macOS/Windows) with documented closest-equivalents, per-OS capability honesty
NFR-3: Test coverage ≥95% on src/, CI-enforced (tarpaulin) — non-negotiable
NFR-4: Performance budgets — reads <1s @25-instance Fleet; ≤2% CPU, ≤50MB RSS per running instance (testable targets, benchmarked in Epic 7)
NFR-5: Observability — structured, timestamped, attributed, rotation-bounded logs; stdout/stderr discipline
NFR-6: Security & privacy — secrets per FR-14; isolation is process/filesystem-level NOT a sandbox (stated boundary)
NFR-7: Documentation currency — same-change docs; contract + embedding docs version with code
NFR-8: Rust 2021+, lean deps; tokio + rusqlite architecture-justified (Islam sign-off pending, non-blocking)

**Total NFRs: 8**

### Additional Requirements

- §6 guardrails: local-only enforcement (no paid-service dependency), no silent in-flight-work loss, zero remote telemetry [ASSUMPTION-tagged]
- §7 three independently versioned public contracts (Adapter Contract, Embedding Interface, kt CLI surface) with deprecation policy
- §9 MVP boundary: opencode adapter code OUT (paper mapping IN); adapter registry, per-window budgets, service/IPC embedding, richer memory backings deferred
- §13 open questions (owned, non-blocking for stories): licensing/positioning (Q2, gates Host GTM only); engine-lib delivery surface v1.x (Q6); deprecation window + skills.sh search fate (Q7); metering-mandatory hard line (Q8, encoded as default-reject)
- Upstream flag from architecture: Glossary lacks "Run" — defined in spine AD-7, PRD memlog carries the addition request

### PRD Completeness Assessment

Strong: 39 FRs each with testable consequences; NFRs product-specific with numbers architecture has since validated/replaced; glossary-disciplined; assumptions indexed (21) with resolutions dated; three Islam rulings folded in with [FIXED] tags. Residual risk: status is `review` (Islam has not personally reviewed the full document — flagged, not a traceability gap); a handful of numeric bounds are architecture-ratified placeholders pending the Epic 7 benchmark story.

## Epic Coverage Validation

### Coverage Matrix

*Validated story-level (not just the epics document's own coverage map — each FR traced to the story ACs that implement it).*

| FR | Epic coverage | Story-level trace | Status |
| --- | --- | --- | --- |
| FR-1 | Epic 1 | 1.2 (register) + 1.3 (path registration) | ✓ |
| FR-2 | Epic 1 | 1.2 | ✓ |
| FR-3 | Epic 1 | 1.2 | ✓ |
| FR-4 | Epic 1 | 1.7 | ✓ |
| FR-5 | Epic 1 | 1.4 | ✓ |
| FR-6 | Epic 1 | 1.4 | ✓ |
| FR-7 | Epic 1 | 1.5 | ✓ |
| FR-8 | Epic 1 | 1.4 | ✓ |
| FR-9 | Epic 1 | 1.6 | ✓ |
| FR-10 | Epic 1 | 1.7 (+1.6 spawn records) | ✓ |
| FR-11 | Epic 2 | 2.1 | ✓ |
| FR-12 | Epic 2 | 2.2 | ✓ |
| FR-13 | Epic 2 | 2.3 | ✓ |
| FR-14 | Epic 2 | 2.4 | ✓ |
| FR-15 | Epic 5 | 5.1 | ✓ |
| FR-16 | Epic 5 | 5.1 (filesystem) + 5.2 (native) | ✓ |
| FR-17 | Epic 5 | 5.2 | ✓ |
| FR-18 | Epic 3 | 3.2 | ✓ |
| FR-19 | Epic 3 | 3.1 (self-reported) + 3.4 (engine-observed) + 1.3 (registration rejection) | ✓ |
| FR-20 | Epic 3 | 3.3 | ✓ |
| FR-21 | Epic 3 | 3.2 (pipeline) + 3.3 (dollar cap e2e) | ✓ |
| FR-22 | Epic 3 | 3.5 | ✓ |
| FR-23 | Epic 3 | 3.5 | ✓ |
| FR-24 | Epic 4 | 4.1 | ✓ |
| FR-25 | Epic 4 | 4.2 | ✓ |
| FR-26 | Epic 4 | 4.3 | ✓ |
| FR-27 | Epic 6 | 1.3 (seed) + 6.4 (TCK completion) | ✓ |
| FR-28 | Epic 6 | 6.2 + 6.3 (6.1 de-risks) | ✓ |
| FR-29 | Epic 6 | 6.5 | ✓ |
| FR-30 | Epic 6 | 6.6 | ✓ |
| FR-31 | Epic 7 | 7.1 | ✓ |
| FR-32 | Epic 7 | 1.1 (seed: boundary CI) + 7.4 (proof + publish) | ✓ |
| FR-33 | Epic 7 | 7.2 | ✓ |
| FR-34 | Epic 7 | 7.3 | ✓ |
| FR-35 | Epic 8 | 8.2 (8.1 enabler) | ✓ |
| FR-36 | Epic 8 | 8.3 | ✓ |
| FR-37 | Epic 8 | 8.4 (8.1 shim) | ✓ |
| FR-38 | Epic 8 | 8.4 | ✓ |
| FR-39 | Epic 8 | 8.5 | ✓ |

### Missing Requirements

None. No FRs appear in the epics document that are absent from the PRD (reverse check clean).

### Coverage Statistics

- Total PRD FRs: 39
- FRs covered in epics/stories: 39
- Coverage percentage: **100%**
- NFR handling: NFR-1/2 embedded in Epic 1 story ACs; NFR-3/7 as every-story DoD gates; NFR-4 benchmarked in Story 7.5; NFR-5 in Story 4.2; NFR-6 in Story 2.4; NFR-8 adoption recorded in Stories 1.2/1.4

## UX Alignment Assessment

### UX Document Status

Not found — and **not implied**. The product is a CLI (`kt`) plus an embeddable Rust library; the PRD's non-goals explicitly exclude any web UI/hosted surface from Ktesio itself (§8), the PRD quality review confirmed no UX phase is required (shape-fit dimension), and hosts building UIs atop the engine own their own UX. Terminal-UX conventions (miette diagnostics, stdout/stderr discipline, ui.rs patterns) are carried as ADOPTED conventions in the architecture spine.

### Alignment Issues

None applicable.

### Warnings

None. The absence of UX documentation is a deliberate, documented decision — not a gap.

## Epic Quality Review

*Standards: create-epics-and-stories best practices, enforced adversarially. 8 epics / 37 stories reviewed individually.*

### 🔴 Critical Violations

None found. No technical-milestone epics, no forward dependencies between epics, no epic-sized stories that cannot complete, entities created only when first needed (SQLite in 1.2, tokio in 1.4, usage_events in 3.1 — verified).

### 🟠 Major Issues

1. **Story 1.4 is the largest story in the plan** (start + stop + data-driven state machine + tokio adoption + BOTH per-OS process backends). It is cohesive — start is meaningless unsupervised — but Windows Job Objects alone carries real effort, and NFR-2 forbids splitting by OS (parity per story). *Remediation:* keep as one story but have sprint planning treat it as the sizing outlier; if it must split during dev, split by capability (start/stop vs. state-machine formalization), never by OS. Not a structural defect; a sizing risk to manage.

### 🟡 Minor Concerns

1. **Three enabler/maintainer-framed stories** (1.1 workspace restructure, 6.1 Hermes primary-source verification, 8.1 machinery relocation) are technically-oriented rather than operator-value stories. Each is a justified brownfield/de-risk exception: 1.1 is the starter-template analog for a brownfield restructure (its user value is v0.5.0 behavioral continuity, asserted in its ACs), 6.1 prevents building the flagship adapter on unverified claims, 8.1 is the explicit brownfield integration story the standards call for. Accepted with rationale.
2. **Story 2.3 forward-reference wording** referenced Story 2.4 in an AC. FIXED during this review (reworded to a capability-boundary statement with no story reference).
3. **Story 5.2 "documented portability procedure"** is the softest AC phrase in the set — testable only via the doc's existence plus the memory-intact run that follows it. Acceptable; story author should keep the procedure executable.
4. **Dense ACs**: several stories pack 4–6 And-clauses; all individually testable, but story-context generation (bmad-create-story) should unpack them into discrete test cases.

### Best-Practices Checklist (all epics)

- Epics deliver user value: ✓ (with the three justified exceptions above)
- Epic independence (N never needs N+1): ✓ — declared order 1→2→3→{4,5}→6→7→8 verified story-by-story
- Stories independently completable, no forward deps: ✓ (post-fix)
- Entities created when needed: ✓
- Given/When/Then ACs, error paths included: ✓ (duplicate names, invalid manifests, failed launches, unsupported pause, crash-loops, unknown keys, no-Rate, replayed batches, loopback-only refusal all present)
- FR traceability maintained: ✓ (100% matrix above)
- Brownfield indicators present: ✓ (restructure story 1.1; migration/compatibility stories 8.4/8.5)

## Summary and Recommendations

### Overall Readiness Status

**READY** — proceed to sprint planning. (0 critical, 1 major sizing risk with managed remediation, 4 minor concerns of which 1 was fixed during review.)

### Critical Issues Requiring Immediate Action

None. The one major item — Story 1.4's size — needs *awareness at sprint planning*, not artifact rework: treat it as the sizing outlier and, if it must split in flight, split by capability, never by OS (NFR-2).

### Recommended Next Steps

1. **Run `bmad-sprint-planning`** to generate `sprint-status.yaml` from the validated epics (this readiness report finds no blocking precondition).
2. **Islam's standing opens** (none block stories; carry them into the sprint's decision log): licensing/positioning (gates Host go-to-market only); tokio+rusqlite sign-off + exact pins at first `cargo add`; PRD Glossary "Run" addition at next PRD touch; deprecation-window ratification before the Epic 8 release stories.
3. **At story-context time** (bmad-create-story): unpack dense multi-And ACs into discrete test cases, and keep Story 5.2's portability procedure executable, not descriptive.
4. **PRD status**: currently `review` — Islam should skim §9 (MVP boundary) and §13 (opens) at minimum before Epic 1 development starts; nothing in this assessment requires content changes first.

### Final Note

This assessment identified 5 issues across 2 categories (1 major / 4 minor), fixed 1 during review, and found zero traceability gaps: 39/39 FRs covered story-level, epic independence verified, no forward dependencies, honest brownfield handling. The artifact chain (PRD → spine → epics) is internally consistent — including the three Islam rulings and the architecture's Run definition — and is safe to hand to sprint planning as-is.

**Assessor:** BMAD implementation-readiness workflow, executed headless-conservative inline in the parent orchestrator (flapping classifier; menus auto-continued, deviations logged in frontmatter). **Date:** 2026-07-02.
