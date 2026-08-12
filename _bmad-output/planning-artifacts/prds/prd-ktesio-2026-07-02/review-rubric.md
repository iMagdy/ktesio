# PRD Quality Review — Ktesio Unified Personal-Agent Runtime Engine

*Sequential rubric walk (subagents unavailable — classifier outage). Reviewer: parent LLM against `assets/prd-validation-checklist.md`, 2026-07-02.*

## Overall verdict

The PRD is architecture- and epics-ready: it has a real thesis (consistency is the product, enforced at the runner level), every FR carries testable consequences, and the hard trade-offs are surfaced as decisions rather than smoothed over. Two things put its promises at risk and should be watched: SM-1 (the ratified north-star) cannot be *fully* measured by the v1 scope as cut (§9.2 tension, explicitly flagged but unresolved), and the performance/latency numbers scattered through FRs and NFR-4 are invented placeholders that downstream stories could mistake for validated budgets.

## Decision-readiness — strong

§9.2's `[NOTE FOR PM]` names the single most consequential scope decision (Adapter #2 in or out of MVP) with both options and a working default. Q3/Q8 encode defaults-plus-ratification rather than hiding choices. Non-goals do real work (§8), and the licensing gate is honestly separated from the build (Q2, addendum §8).

### Findings
- **medium** Sequencing note edges toward authoring (§12) — the PRD proposes epic seams, which is downstream's job. It is flagged advisory; keep only if PM finds it useful. *Fix:* none required; delete at epics time if it constrains rather than helps.

## Substance over theater — strong

No persona theater (three UJs, each cited by FRs); no NFR boilerplate — every NFR is product-specific, and the honesty requirements (FR-7 pause semantics, FR-23 estimate labels) are genuinely unusual substance. Vision names a real reference agent and a falsifiable bet.

### Findings
- **high** Placeholder numbers could masquerade as validated budgets (NFR-4; FR-4 2s freshness; FR-6 30s window; FR-9 backoff; FR-10 ≤5s flush; FR-21 latency; FR-25 10MB; SM-3 15min; SM-5 1 day) — all tagged `[ASSUMPTION]`, but a story author skimming consequences may copy them as requirements. *Fix:* architecture phase must validate-or-replace every number; epics must not cite an untouched placeholder as an acceptance criterion.

## Strategic coherence — strong

Thesis stated and bet on; feature set follows it (contract + conformance kit before breadth); counter-metrics exist and target the real failure modes (catalog-breadth vanity, per-agent depth creep, precision theater). MVP scope kind is platform/capability and the scope logic matches.

### Findings
- **medium** SM-3 (≤15 min time-to-operate) has no baseline or measurement protocol. *Fix:* define the measurement (fresh machine? which OS? docs-only?) when test planning starts.

## Done-ness clarity — strong

Every FR has at least one testable consequence; error paths are first-class (FR-1 duplicate names, FR-7 unsupported-pause fail-fast, FR-9 crash-loop bound, FR-30 version mismatch). The conformance kit (FR-27) gives "identical controls" an operational meaning.

### Findings
- **medium** FR-24 fixes interaction to *text* input only; Hermes exposes multi-channel interaction (brief addendum §C). If the reference Agent's reality doesn't fit the minimal channel, the "consistent interaction" promise thins out at the flagship. *Fix:* architecture validates the interaction channel against real Hermes before contract freeze; widen or explicitly bound the FR then.
- **low** FR-17's export consequence depends on an export capability the MVP may demote (§9.2) — if demoted, FR-17 needs a rewritten consequence. *Fix:* reconcile at scope-cut time.

## Scope honesty — strong

21 assumptions inline *and* indexed with owners implied; 8 open questions each carry an owner and a "needed by." High assumption density is appropriate for a headless away-mode draft and every item is correction-ready. Deferred items carry reasons.

### Findings
- **low** Q5 (non-goals re-confirmation) and §8's "awaiting re-confirmation" repeat the same ask in two places — harmless, slightly noisy. *Fix:* collapse at next update.

## Downstream usability — strong

Glossary is load-bearing and used with discipline; IDs are contiguous (FR-1…39, UJ-1…3, SM-1…5 + C1…3, NFR-1…8, Q1…8); cross-references resolve; sections extract cleanly.

### Findings
- **low** FR actor is uniformly "The Operator" while Host parity is granted globally by FR-31 — correct but implicit at each FR. *Fix:* one preamble sentence in §4 ("every Operator capability is equally a Host capability via the Embedding Interface, per FR-31") would remove any doubt for story authors.

## Shape fit — strong

Developer-product capability spec with a light UJ layer is the right dial for a CLI/engine; chain-top traceability is present without over-formalization. No UX phase is needed downstream (no GUI surface), which the pipeline should note when routing.

## Mechanical notes

- Glossary drift: none found for defined terms; §1 uses "runner"/"runtime engine" rhetorically where Glossary says Engine — acceptable in vision prose, watch in FRs (FRs are clean).
- "CLI" vs "`kt`": generic "CLI" appears in §0/§1 prose; FRs use `kt` or Operator. Acceptable.
- Assumptions Index roundtrip: 21 index entries ↔ inline tags verified during walk; no orphans found in either direction.
- ID continuity: verified contiguous; no duplicates; every UJ referenced by ≥1 FR; every SM cites FRs.
- Required sections for stakes/type: present (spine + dev-product clusters + invented migration section).
