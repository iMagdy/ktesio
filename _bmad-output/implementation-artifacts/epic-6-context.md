# Epic 6 Context: First-Class Hermes and a Frozen Public Adapter Contract

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

An Operator runs the real NousResearch Hermes Agent end-to-end under Ktesio (UJ-1 for real): register, start, stop, pause per its declaration, meter, budget, cap, memory-wire, and interact — all through the standard commands. Adapter authors get a published, versioned Adapter Contract with per-OS Capability Declarations and a conformance test-kit (TCK). The contract freezes only after the opencode paper validation proves it is not Hermes-shaped.

## Stories

- Story 6.1: Verify Hermes Agent surfaces from primary sources
- Story 6.2: Run the real Hermes Agent under Ktesio lifecycle
- Story 6.3: Govern and interact with Hermes end-to-end (UJ-1 for real)
- Story 6.4: Prove any adapter with the conformance test-kit
- Story 6.5: Validate the contract against opencode on paper
- Story 6.6: Freeze and publish the Adapter Contract v1

## Requirements & Constraints

- **Verification before adapter code (6.1):** the brief addendum's Hermes analysis (§C) rests on search-result excerpts, not fetched pages (§H caveat). A written verification note must confirm or correct — each cited to primary docs or the Hermes repo — the gateway/process model, lifecycle verbs, config mechanism, usage/analytics surface, and interaction channels BEFORE Story 6.2 writes adapter code. Any contract-impacting surprise is fed back as an Adapter Contract change proposal.
- **Real lifecycle parity (6.2):** the native `ktesio-adapters-hermes` adapter carries a per-OS Capability Declaration; registration/start/stop/(pause if declared) follow the standard state machine; stop terminates the full process tree on all three OSes; every Epic 1 lifecycle AC passes against the Hermes adapter with best-effort capabilities explicitly surfaced. Network-bound integration tests run sandboxed/recorded with the isolation strategy documented in-module.
- **Full governance journey (6.3):** self-reported usage lands in the Usage Ledger with idempotent batch reconciliation (no double-count on replays), Breach Action pauses past-cap instances, `kt` reports tokens + estimated dollars with honest labels; memory attach (both kinds) and interaction work through the standard commands; UJ-1 runs end-to-end as an integration test using only documented commands.
- **Conformance kit (6.4):** `ktesio-conformance` TCK exercises every contract section against the mock adapter — lifecycle transitions incl. crash, config mapping, both Metering Sources, memory attachment, interaction, Capability Declaration edge cases (`pause: unsupported`) — reporting per-capability compliance; Hermes passes all sections applicable to its declaration; the TCK is a cargo test harness any third-party adapter crate can invoke.
- **Second-agent proof (6.5):** opencode (opencode.ai, Islam's ruling) characterized on paper from primary sources; a conformance mapping maps EVERY contract section to opencode; unresolved structural axes are listed explicitly as contract-freeze risks, never silently dropped.
- **Freeze only after feedback (6.6):** Stories 6.4 + 6.5 feedback applied first; then `ktesio-adapter-api` tags contract v1; incompatible-version loading fails naming BOTH versions plus the compatibility rule; docs publish with the crate (NFR-7); semver-check CI guards against unannounced breakage.

## Technical Decisions

- **Contract seed already exists:** `ktesio-adapter-api` v0.4.0 is the minimal seed (`CONTRACT_VERSION` const, stored-not-negotiated). Epic 6 grows it to full capability set + freeze/negotiation, then tags v1 (FR-27/FR-30).
- **Two adapter kinds, one trait (AD-3):** native Rust impls compiled into the workspace (`hermes`, conformance `mock`) or manifest adapters (`adapter.toml`). The manifest schema is part of the Adapter Contract, defined only in `ktesio-adapter-api`, versioned under the same semver. No dynamic library loading.
- **Workspace boundary (AD-2):** `ktesio-adapters-hermes` depends on `ktesio-adapter-api`; the engine never reaches into adapter internals and vice versa. Semver-check CI applies to `ktesio-engine` AND `ktesio-adapter-api`.
- **Hermes specifics carried from brief §C:** single gateway process supervising multi-channel connectivity (Telegram/Discord/Slack/WhatsApp/Signal/CLI); model-agnostic via `hermes model`; existing surfaces `hermes gateway run/start/setup/enroll`, s6-overlay/Docker supervision, `/usage` + analytics for token/cost breakdown, loop/iteration guardrails; NO native stop/status or $-denominated cost cap found (unverified — 6.1 confirms/corrects).
- **Pause honesty (brief §G):** guaranteed-vs-best-effort semantics distinguished via per-OS Capability Declarations; Hermes' gateway persistence likely means best-effort pause — the declaration says so explicitly rather than over-promising.
- **Metering source of truth:** self-reported (Hermes `/usage` + analytics) arrives in batches → enforcement latency bounded by report cadence; reconciliation MUST be idempotent. The engine-observed loopback alternative exists but Hermes' native analytics makes self-reported the natural primary.
- **Inherited obligation from Epic 5 (story 5-2 ratified Q-1/Q-2, 2026-08-24):** Epic 6 owns the deferred `--json memory` wire surface — `GuaranteeLevel::as_str` is reserved for exactly this wire form; freezing the wire surface belongs with the contract-freeze story so both freeze together. The "ONE intentional announced key-set edit" obligation from story 5-1 transfers WITH it.
- **Memory delivery stays at the AD-9 config seam:** no SpawnSpec/StartLaunch field, no contract token minted outside the contract crate; Epic 6 consumes Epic 5's surface end-to-end unchanged.

## Cross-Story Dependencies

- Consumes Epic 1 (lifecycle/state machine/process backends), Epic 2 (config mapping), Epic 3 (metering/budget/cap), Epic 4 (interaction channels), Epic 5 (memory attach, both kinds) — UJ-1 is the integration of all five through one real agent.
- 6.1 gates 6.2 (no adapter code before verified facts); 6.2 gates 6.3 (governance needs a running real agent); 6.4 can start alongside 6.2/6.3 (mock-driven) but its Hermes-applicability pass needs 6.2/6.3 landed; 6.5 is independent research/paper work but its change proposals must land before 6.6; 6.6 is last and irreversible-ish (semver-checked).
- Epic 7 (embedding hosts) builds on the frozen contract + engine public API this epic stabilizes.

## Standing Gates (apply to every story)

- CLI-first: everything reachable via `kt`; stdout/stdout diagnostics split preserved.
- Test coverage ≥ 95% (`cargo tarpaulin --workspace --fail-under 95`).
- Docs currency: `docs/` + README updated in the same change.
- Cross-platform Linux/macOS/Windows; path-agnostic std APIs.
- Graceful degradation: partial failures name reason + remediation.
