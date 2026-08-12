# Spine Review — adversarial incompatibility lens

*Configured finalize reviewer #2: "construct two units one level down that each obey every AD to the letter yet still build incompatibly." Sequential fallback (subagents gated). 2026-07-02.*

**Verdict: three real incompatibility constructions found; two close via the rubric's fixes, one needs its own tightening. After the three fixes, no further construction survived the attempts below.**

## Constructions that succeeded (holes)

1. **The path-construction fork (closes with rubric fix #1).** Unit A (lifecycle epic) has the engine hand out Agent Home paths from `StateStore`; Unit B (skills epic) computes `~/.ktesio/agents/{name}/skills/` directly — both obey every AD as written. They diverge the moment the state dir is configurable or platform-conventional (XDG vs macOS vs Windows). *Close:* engine = sole path authority convention.
2. **The "run" delimiter fork (closes with rubric fix #2).** Unit A (budgets) treats a Run as start→terminal-state; Unit B (metering ingestion) allocates a new run id per interaction session. Both satisfy AD-7's pipeline rule; per-run budgets now mean different things in the ledger vs the evaluator. *Close:* Run defined in AD-7 + UsageEvent minimum shape fixed.
3. **The manifest-schema ownership fork (needs its own fix).** AD-3 says manifests are "declarative TOML" whose executor lives in the engine, and AD-2 puts "manifest schema" in `ktesio-adapter-api`. An epic implementing the executor (engine) and an epic implementing manifest validation/authoring docs (adapter-api) could each evolve the schema — two writers, one format, no stated owner or versioning rule for *the manifest schema itself* (the Adapter Contract semver in AD-2/PRD §7 covers the *trait*, arguably not the TOML). *Close:* one sentence in AD-3: the manifest schema is part of the Adapter Contract, defined (types + validation) ONLY in `ktesio-adapter-api`, versioned under the same contract semver; the engine executor consumes that crate's parsed form and never defines its own.

## Constructions attempted that the spine already blocks

- **Double breach enforcement** (host reacts to breach events by also stopping the instance → races supervisor): AD-7 names the supervisor as the only Breach Action executor; host subscriptions are observational. Blocked.
- **Two currency formatters** (CLI renders its own dollars): AD-8 single-module rule + EstimateLabel type. Blocked.
- **CLI-private engine calls** (kt using internals for speed): AD-2 visibility + semver CI. Blocked.
- **Divergent event dialects** (kt --json enriching payloads): AD-14 same-serde-structs rule. Blocked.
- **OS-conditional code sprawl** (a `#[cfg(windows)]` in the skills module): conventions restrict platform code to `backends::*`. Blocked.
- **Direct ledger writes** (skills or adapter code appending usage): AD-7 "no other code path may mutate the Usage Ledger." Blocked.
- **Secret leakage via events** (host event carrying resolved config): AD-10 SecretString serialization masking. Blocked.
