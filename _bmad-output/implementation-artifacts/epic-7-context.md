# Epic 7 Context: Embed the Engine (Hosts)

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

A Host embeds the Ktesio engine as a Rust library: drives every capability without a CLI or TTY, subscribes to state/usage/breach/crash events with stable schemas, and depends on crates.io-published `ktesio-engine` + `ktesio-adapter-api`. `kt` consuming only the public API is proven in CI, and the performance budgets are benchmarked. UJ-3 — a hosting platform driving hosted personal agents through the library — lands here.

## Stories

- Story 7.1: Drive every capability through the library alone
- Story 7.2: Subscribe to engine events with stable schemas
- Story 7.3: Embed clean — no TTY, no prompts, blocking facade
- Story 7.4: Prove the boundary and publish the crates
- Story 7.5: Benchmark the performance budgets

## Requirements & Constraints

- **Full capability through the library (7.1):** an integration test host linking `ktesio-engine` only drives the full register→configure→cap→start→breach→pause→stop flow purely through the library. Every capability the flow uses (registration/fleet reads, unified lifecycle, unified configuration, token/dollar governance) is reachable and behaviorally identical to the `kt` path, with assertions SHARED between the host test and the CLI test suite; any capability found unreachable is closed in-story.
- **Stable event subscription (7.2):** per-instance state/usage/breach/crash events arrive in order, payload `schema_version`-stamped and schema-validated; slow subscribers cannot stall supervision (bounded channel policy documented and tested).
- **Embeds clean (7.3):** the full 7.1 flow runs headless with zero interactive prompts and no global process state that could collide with a host's runtime.
- **Boundary + publish (7.4):** a build-level check proves `kt` uses only public engine API; `cargo-semver-checks` guards both crates; `ktesio-engine` + `ktesio-adapter-api` publish to crates.io with an embedding quickstart whose host example compiles in CI. **As-built (2026-09-09):** publication is PREPARED-AND-HELD — Islam's standing no-deployment instruction holds; the machinery is complete and the exact ordered actions live in the runbook (`docs/release-process.md`, steps 0-7), each marked `HOLD — requires Islam's explicit go`. The in-repo freeze-baseline guard is KEPT (retire-or-keep deferred to the second published release) and a SECOND one is armed for `ktesio-engine` against the embedding-surface freeze at `8a8b328`.
- **Performance (7.5):** the NFR-4 budgets are benchmarked, not assumed.

## Technical Decisions

- **Async-first core, blocking facade (AD-13):** the engine is tokio-based; sync consumers (kt, hosts) use the `blocking()` facade, which is the sanctioned embedding surface and covers the full async API. The engine never touches a TTY.
- **One event schema, two consumers (AD-14):** event payloads are serde structs, snake_case, every payload carrying `schema_version`; the CLI's `--json` documents and the future host subscription share the same shapes. Event struct families each pin a `*_SCHEMA_VERSION` const; wire changes are announced, never silent.
- **Dual delivery:** `kt` is built exclusively on the engine's public surface (FR-32) — the CLI is the standing embeddability proof, and the CI boundary gate (`cargo tree` allowlist over kt's normal/build graph) keeps the dependency shape honest.
- **Frozen contract context:** the Adapter Contract is v1 (same-major negotiation at registration and manifest re-reads); the adapter-api Rust surface is CI-guarded against the in-repo freeze baseline. Engine and adapter-api publication is prepared-and-held at 7.4 (as above) — `license-file` packaging metadata and the publish=false toggles are already in place for that flip.
- **Shared test assertions live in `ktesio-conformance`** (the established dev-dependency of both the engine and kt test suites — dev-deps never cross the shipping boundary gate): flow-level expectations are pinned once and consumed by both the library host test and the CLI test.

## Cross-Story Dependencies

- 7.1 establishes the library-driven flow that 7.3 re-runs headless and that the 7.2 event work observes; 7.2's subscription surface builds on the AD-14 schemas the flow already emits.
- 7.4 is last and changes distribution: publish toggles, quickstart, and the semver gate's crates.io baseline (which retires-or-keeps the in-repo freeze-baseline guard). As-built, the publication itself is held (see the 7.4 requirement line); the in-repo guard stays armed. Epic 8 (skills provisioning) and the §4.9 capability are a separate epic — 7.1's reachability inventory, as built, covers §4.1–4.7 (§4.8 is discharged by the host test itself, which IS the §4.8/FR-31 proof; §4.4/§4.6 are cited facade-proven from the Epic-6 hermes e2e; §4.9 is owned by epic 8; §4.10 is out of scope).
- The engine→`ktesio-adapters-hermes` normal edge (hermes builtin compiled into the engine) is the declared hexagonal exception; hosts linking the engine get the builtin adapters without extra dependencies.
