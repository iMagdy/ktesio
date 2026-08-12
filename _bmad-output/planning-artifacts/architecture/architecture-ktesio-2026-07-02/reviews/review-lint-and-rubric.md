# Spine Review — lint (hand-run) + good-spine rubric

*Sequential fallback: lint_spine.py unrunnable (Bash gated by flapping classifier); its documented checks were executed by hand. Rubric walked per reviewer-gate.md. 2026-07-02.*

## Lint (deterministic checks, hand-run)

- Placeholders / template comments: **none** — all `<!-- -->` guidance stripped, no `{curly}` placeholders emitted.
- AD IDs: **AD-1..AD-16, unique, ascending, none reused.**
- Binds/Prevents/Rule present on all 16 ADs: **yes.**
- Mermaid validity: 3 diagrams (dependency-direction flowchart w/ subgraph+classDef; stateDiagram-v2; component flowchart) — syntax-walked, valid; no empty graphs.
- Stack pins: existing rows pinned from the real Cargo.toml. **FLAG (accepted):** `tokio*` and `rusqlite*` are caret ranges, not exact pins — author-flagged verification gap (classifier blocked WebSearch/cargo-search); resolution path stated in the table caption and memlog.

## Rubric verdict

**Adequate-to-strong: the spine fixes the real divergence points for epic-level builders and stays lean; two genuine gaps found (filesystem-layout authority; the "Run" scope concept) — fix before final.** Coverage of the altitude's dimensions is otherwise complete, including the operational envelope *except* distribution/release, which is ADOPTED reality but silent in the spine.

### Findings

- **high** No filesystem-layout authority (§Conventions / AD-6) — AD-6 fixes *what* is DB vs files but no rule says WHO constructs paths (engine state dir location, Agent Home layout). Two builders could each invent path construction (CLI computing an Agent Home path directly vs engine API). *Fix:* add a convention row: the engine is the sole path authority; consumers receive paths from the API, never construct them.
- **high** "Per-run" budget scope has no defined "Run" (AD-7 / PRD FR-18) — the PRD's Glossary never defines Run; a budget evaluator and a ledger writer could delimit runs differently (per start→stop span vs per interaction). *Fix:* define Run in AD-7's rule (span from `starting` to terminal `stopped`/`failed`) and fix the UsageEvent minimum shape (instance, run id, input/output tokens, source, timestamp). Also flag the Glossary gap back to the PRD as an open item.
- **medium** Distribution & release silent (§Conventions) — existing channels (crates.io, Homebrew, install scripts, release automation) and the new requirement that `ktesio-engine`/`ktesio-adapter-api` publish to crates.io for Hosts to embed are real operational facts the spine doesn't state. *Fix:* one ADOPTED convention row.
- **low** `binds:` frontmatter uses a range shorthand (`FR-1..FR-39`) rather than enumerating — acceptable at initiative altitude; downstream tooling that parses `binds` literally should expand it.
- **low** AD-8's grep-lint enforcement is named but not specified (which lint, where) — acceptable; lands naturally in the CI story.

## Dimension sweep (owned by this altitude)

Decided: paradigm, workspace/boundaries, adapter model, process supervision per OS, state store, metering/enforcement, config/secrets, memory, interaction/logs, events/API, state machine, skills reuse, migration shims. Deferred (named, with reasons): registry, IPC transport, provider schemas, keychain, windows/period budgets, reconciliation, richer memory, sandboxing, opencode code. Open: dep sign-offs, metering mechanism confirmation, opencode characterization scheduling. **No silent dimension after the distribution fix lands.**
