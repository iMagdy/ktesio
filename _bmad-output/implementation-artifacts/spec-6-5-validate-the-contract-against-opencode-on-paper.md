---
title: 'Validate the contract against opencode on paper'
type: 'feature'
created: '2026-09-03'
status: 'done'
review_loop_iteration: 0
baseline_commit: e1e20047845896919400d244435c2dd870b0ed29
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Adapter Contract (0.4.0) grew up alongside one real agent — Hermes — so before Story 6-6 freezes it as v1 there is no proof it isn't Hermes-shaped. FR-29 (epics.md Story 6.5, Islam's ruling: opencode.ai) requires a second real agent characterized from primary sources and mapped section-by-section against the contract, with unresolved axes surfaced as freeze risks.

**Approach:** Produce two git-tracked paper artifacts in `_bmad-output/planning-artifacts/` (the Story 6-1 Hermes-verification precedent): an opencode characterization note pinned to opencode v1.18.27, and a conformance-mapping document that walks every contract surface (AgentAdapter trait, manifest sections, all 8 TCK section ids) to opencode, records `CP-6.5-<letter>` change proposals, and closes with an explicit contract-freeze-risk list.

## Boundaries & Constraints

**Always:**
- Every factual claim cites a primary source (opencode.ai docs page or anomalyco/opencode at the pinned version) with URL + access date; the canonical repo is **anomalyco/opencode** (`sst/opencode` 301-redirects there; never conflate with the archived `opencode-ai/opencode` project).
- The mapping covers EVERY contract section: the 8 TCK section ids (`capability_edges`, `lifecycle`, `pause`, `config_mapping`, `metering_self_reported`, `metering_engine_observed`, `memory`, `interaction`), the manifest sections (`[adapter]`/`[lifecycle]`/`[capabilities]`/`[metering]`/`[interaction]`/`[config]`), and the AgentAdapter trait surface — each gets a disposition: mapped, not_applicable (reason from the actual structure), or freeze risk. Nothing silently dropped.
- Unresolved axes land in an explicit **Contract-freeze risks** section, each naming the axis, the evidence gap, and the decision 6-6 must make. Change proposals use `CP-6.5-<letter>` ids in the CP-6.1 format (finding → proposed contract change → affected crate/types) and are proposals only.
- Per axis, name who covers it: opencode, Hermes, or mock/TCK-probe-only (the FR-29 structural-distance requirement).
- The characterization note mirrors the Hermes note's discipline: identity, process/lifecycle model, lifecycle verbs, config mechanism, usage/metering surface, interaction channels, memory/persistence, providers/auth, corrections-vs-expectation, residual risks — verdict line up front, per-claim citations.
- Pin to **opencode v1.18.27** (release 2026-09-02) and state the churn caveat (multiple releases weekly; re-validate per pinned version).

**Ask First:**
- Any CP that would add/rename ktesio-adapter-api types or touch memory wire vocabulary — `GuaranteeLevel`/the deferred `--json` memory surface is reserved for 6-6's freeze; word memory findings as freeze-risk input, not new vocabulary.
- Publishing either artifact as a public `docs/` page (Hermes precedent: git-tracked planning artifact only).
- If primary sources contradict the pinned characterization (breaking release, moved docs), HALT before re-pinning.

**Never:**
- No production-code changes: no `crates/`, `docs/`, `scripts/`, `CI` edits — the deliverable is the two documents plus sprint-status bookkeeping.
- No CP application, no CONTRACT_VERSION bump, no integration tests against a live opencode install (this is "on paper").
- No secondhand sources as primary evidence (blog recaps, tutorials); official docs/repo only.

## Code Map

**Contract side (what the mapping must walk):**
- `crates/ktesio-adapter-api/src/lib.rs:76` -- CONTRACT_VERSION 0.4.0 + bump history (docs L45-76).
- `crates/ktesio-adapter-api/src/adapter.rs:60` -- AgentAdapter trait: kind/capabilities/metering_source/config_mapping + default-Unavailable start/stop/pause/resume (:96-120).
- `crates/ktesio-adapter-api/src/capability.rs` -- Capability = {Pause, Interaction} (:65), SupportLevel 3 levels (:29), CapabilityDeclaration + effective(os) (:113/:186).
- `crates/ktesio-adapter-api/src/metering.rs:61` -- SelfReported (KTESIO_USAGE stdout sentinels, `sequence` replay-dedup) vs EngineObserved (loopback OpenAI-compatible).
- `crates/ktesio-adapter-api/src/manifest.rs` -- manifest sections (:44), InteractionChannelKind **Stdio-only** (:138), strict semver + deny_unknown_fields.
- `crates/ktesio-adapter-api/src/config.rs:54` -- ConfigTarget Env/Flag/File; reserved engine keys `memory.dir` (`ktesio-engine/src/domain/config.rs:148`) and `metering.base_url` (story 3-4).
- `crates/ktesio-engine/src/ports/memory_backing.rs:93` -- GuaranteeLevel is engine-API-only until 6-6 freezes its wire form.
- `crates/ktesio-conformance/src/tck.rs:295-311` -- the 8 `section_ids` the mapping must walk.

**Precedent & requirements:**
- `_bmad-output/planning-artifacts/hermes-agent-verification-2026-08-25.md` -- the 6-1 template (structure, citation discipline, CP format, residual-risks section).
- `_bmad-output/planning-artifacts/epics.md:584-596` -- Story 6.5 verbatim ACs; PRD FR-29 at `prds/prd-ktesio-2026-07-02/prd.md:277-281`.
- `sprint-status.yaml:89` -- CP-6.1-a…f and their resolutions (the tracking pattern to follow).

**opencode fact sheet (from 2026-09-03 primary-source research; re-verify any claim you cite):**
- One binary, TUI + server; "when you run opencode it starts a TUI and a server" — client/server by construction (opencode.ai/docs/server/). Headless: `opencode serve` (foreground HTTP server, default 127.0.0.1:4096, no daemonize flag) (opencode.ai/docs/cli/). `attach` joins a running backend.
- Sessions are server-side objects (GET/POST /session, DELETE /session/:id, POST /session/:id/abort) — one process hosts many; abort ≠ process stop. Exit codes/signal handling: undocumented → OPEN.
- **No pause/suspend surface** (only session abort, /revert); **no /health** (`/doc` OpenAPI + SSE `server.connected` are proxies) → OPEN.
- Config: `opencode.json` merged across scopes (global → OPENCODE_CONFIG → project → managed dirs); `OPENCODE_CONFIG`/`OPENCODE_CONFIG_CONTENT` env injection; `{env:VAR}` substitution; published JSON schema (opencode.ai/docs/config/). Flags: serve --port/--hostname, run --model/--agent.
- Metering: per-message `tokens`+`cost` in AssistantMessage types; `opencode stats`, `session list --format json`, SSE `message.updated`; Session object itself carries no cost fields. Cost rates from models.dev. Coverage-when-provider-omits-usage: undocumented → OPEN.
- Interaction: `opencode run "prompt"` one-shot (--format json), POST /session/:id/message, /tui/* endpoints drive the TUI via server, `opencode acp` = stdio JSON-RPC subprocess (editor plugins). SDK is JS/TS only.
- Memory/persistence: data under `~/.local/share/opencode/` (project storage per-project); sessions resumable (--continue/--session/--fork); rules = AGENTS.md/CLAUDE.md chain + `instructions` globs. XDG_DATA_HOME honor: undocumented → OPEN (isolation lever unclear).
- Providers: 75+ via AI SDK/models.dev; auth in `~/.local/share/opencode/auth.json`; custom provider via npm package + `options.baseURL` — the engine-observed loopback hook.
- Windows: installs but "best experience via WSL"; TUI terminal caveats. `autoupdate` defaults ON → supervisor must pin false.
- `sst/opencode` → `anomalyco/opencode` (301 verified 2026-09-03); archived `opencode-ai/opencode` is a DIFFERENT project (became Crush).

## Tasks & Acceptance

**Execution:**
- [x] `_bmad-output/planning-artifacts/opencode-characterization-2026-09-03.md` -- Write the characterization note mirroring the Hermes note's structure and citation discipline; pin v1.18.27; verdict line up front; OPEN axes explicit; corrections-vs-expectation table; residual risks. -- The epics AC's first artifact.
- [x] `_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md` -- Write the conformance mapping: per-section disposition table (all 8 TCK sections + trait surface + manifest sections → mapped / not_applicable+reason / freeze risk), the axis-coverage matrix (opencode vs Hermes vs mock-only), `CP-6.5-<letter>` proposals, closing Contract-freeze-risks section. -- The epics AC's second artifact.
- [x] `_bmad-output/implementation-artifacts/sprint-status.yaml` -- Record artifact paths + CP ids + freeze-risk count on the 6-5 entry at completion. -- Bookkeeping continuity (6-1 precedent).

**Acceptance Criteria:**
- Given the mapping document, when reading any of the 8 TCK section ids, then each carries an explicit opencode disposition and none is omitted.
- Given the characterization note, when checking any process-model claim, then it cites a primary-source URL at the pinned version with access date, and undetermined axes are listed OPEN rather than asserted.
- Given the freeze-risk list, when Story 6-6 planning reads it, then every unresolved axis names the decision required — none silently absorbed.
- Given any CP-6.5 entry, when read, then it follows the CP-6.1 format and proposes without applying.
- Given the repo after the story, when diffing the story commit against its baseline, then `crates/`, `docs/`, `scripts/`, `.github/` are untouched.

## Spec Change Log

## Design Notes

- Mapping hypotheses to verify and enrich while writing (do not treat as conclusions): lifecycle = manifest adapter on `opencode serve` (readiness = `/doc` reachable or SSE `server.connected`; stop = process kill since no daemon); pause = `unsupported` all OSes (honest, TCK not_applicable); interaction = HTTP-native sessions vs the contract's Stdio-only channel (likely CP + freeze risk; ACP stdio is a different process mode); metering = BOTH paths plausible — custom-provider `options.baseURL` → engine-observed loopback, and stats/SSE tailing → self-reported shim (structural distance from Hermes: self-reported only); memory = isolation lever unclear (`~/.local/share/opencode` fixed, XDG undocumented) → likely freeze risk on `memory.dir` applicability; config = rich Env/File mapping via `OPENCODE_CONFIG`/`OPENCODE_CONFIG_CONTENT`; autoupdate must be pinned false via supervisor-provided config.
- Keep both artifacts in `_bmad-output/planning-artifacts/` (git-tracked per `.gitignore:21`), NOT `docs/` — matching the Hermes precedent; `scripts/check_docs.py` does not see that directory.

## Verification

**Commands:**
- `python3 scripts/check_docs.py` -- still validates (no docs/ changes expected)
- `git diff --stat <baseline> -- crates/ docs/ scripts/ .github/` -- empty for the story's commits

**Manual checks:**
- Citation spot-check: at least one claim per characterization section re-fetchable at the pinned version.
- Completeness count: mapping covers 8 section ids + trait surface + manifest inventory; freeze-risk list present; CP ids sequential from `CP-6.5-a`.
## Suggested Review Order

**The decision-ready map (start here)**

- Verdict up front: repo identity, the pin (v1.18.27), and what the mapping claims to prove.
  [`conformance-mapping.md:1`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L1)

- Every TCK section dispositioned — mapped, not_applicable with reason, or freeze risk; nothing omitted.
  [`conformance-mapping.md:22`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L22)

**The 6-6 decision input**

- Freeze risks R1-R11 — the story's core deliverable: every unresolved axis with the decision required.
  [`conformance-mapping.md:136`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L136)

- CP-6.5-a…f change proposals in the CP-6.1 format — proposals only, nothing applied.
  [`conformance-mapping.md:101`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L101)

**Evidence the conclusions rest on**

- The axis-coverage matrix: 23 axes with per-axis prior coverage — the FR-29 structural-distance proof.
  [`conformance-mapping.md:69`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L69)

- The characterization note: verdict line, per-axis sections, every claim primary-cited at the pin.
  [`characterization.md:1`](../../_bmad-output/planning-artifacts/opencode-characterization-2026-09-03.md#L1)

- Process/lifecycle model — the structural heart: client/server, sessions-as-objects, no pause surface.
  [`characterization.md:21`](../../_bmad-output/planning-artifacts/opencode-characterization-2026-09-03.md#L21)

- Corrections vs the planning fact sheet — including the PRD's own "single-shot" falsification.
  [`characterization.md:89`](../../_bmad-output/planning-artifacts/opencode-characterization-2026-09-03.md#L89)

- OPEN axes, each mapped to its freeze-risk id — the none-silently-absorbed proof.
  [`characterization.md:102`](../../_bmad-output/planning-artifacts/opencode-characterization-2026-09-03.md#L102)

**Peripherals**

- Trait-surface and manifest-section dispositions backing the section table.
  [`conformance-mapping.md:39`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L39)

- The verification trail: how citations were checked at the pinned commit.
  [`conformance-mapping.md:200`](../../_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md#L200)
