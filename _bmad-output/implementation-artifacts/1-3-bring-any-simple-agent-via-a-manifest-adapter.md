---
baseline_commit: fbfd90fd6eb7f2ea2788d19e067469479e95e06a
epic: 1
story: 3
story_key: 1-3-bring-any-simple-agent-via-a-manifest-adapter
github_issue: 65   # exists (OPEN, "[1-3] Story 1.3: Bring any simple agent via a manifest adapter"); orchestrator syncs — do NOT edit issue #65 from dev-story
---

# Story 1.3: Bring any simple agent via a manifest adapter

Status: done

<!-- Note: Validation is optional. Run validate-create-story for a quality check before dev-story. -->

## Story

As an Operator,
I want to register an agent described by a declarative `adapter.toml` supplied by path,
so that I can put my own agents under Ktesio without writing Rust. (FR-1 path registration; FR-27 seed)

## Acceptance Criteria

*(Verbatim from epics.md Story 1.3; AC numbering added for task traceability.)*

1. **Given** the `ktesio-adapter-api` crate defining the AgentAdapter trait, the manifest schema (types + validation owned by this crate under contract semver, AD-3), and a minimal per-OS Capability Declaration **when** I register an Agent Instance from a directory containing a valid `adapter.toml` (exec/args/env templates per lifecycle op, capability declaration, metering-source config, interaction wiring) **then** registration succeeds and the effective (current-OS) Capability Declaration is visible for the instance.
2. **And** an invalid manifest (missing mandatory section) is rejected with a diagnostic naming the section (FR-27 consequence).
3. **And** the `ktesio-conformance` crate ships a mock adapter + scripted fake agent used by this and all later lifecycle/governance tests.
4. **And** an adapter whose manifest declares no viable Metering Source is rejected at registration with a clear diagnostic (FR-19 hard line).

### Acceptance criteria — engineering interpretation (binding for dev)

- **AC1 — adapter-api is the schema owner (AD-3).** `ktesio-adapter-api` gains its FIRST real code this story: (a) the `AgentAdapter` trait (declares the lifecycle ops so a native adapter *implements* them and a manifest adapter *carries templates* for them — but NOTHING is executed this story); (b) the `CapabilityDeclaration` type, keyed **per-OS** (capability × OS → guaranteed / best-effort / unsupported — as DATA, not `#[cfg]`); (c) the `MeteringSource` declaration (`self-reported` | `engine-observed`); (d) the `adapter.toml` manifest schema (serde types + a `validate()` that enforces mandatory sections) with a **contract-version constant** (`CONTRACT_VERSION` semver seed). The engine parses adapter.toml **only through this crate's types** and defines no schema of its own.
- **AC1 — "register from a directory containing a valid adapter.toml".** `kt agent register <name> --manifest <dir-or-file>` loads + validates the manifest **before** any filesystem side effect, resolves the current-OS Capability Declaration, persists it with the Agent Instance, and prints (a) the Agent Home path and (b) the effective per-OS Capability Declaration. Registration stays atomic (the AD-6 store + the F2 orphan-row lesson from 1-2): adapter validation is a pure, side-effect-free step that runs and must pass before the row insert / home creation.
- **AC1 — "effective (current-OS) Capability Declaration is visible".** "Effective" = the declaration projected onto the current OS via a **runtime OS identifier** (resolved from `std::env::consts::OS`, mapped to an `OsId` enum in adapter-api — NO `#[cfg]`). The engine persists this projected declaration with the instance and surfaces it: minimally via a new `kt agent show <name>` (or an addition to `kt agent list`) that renders each capability's current-OS support level. `[ASSUMPTION: surface via a new read command / column; tag the exact shape.]`
- **AC2 — missing-mandatory-section rejection names the section.** `Manifest::validate()` returns a typed error whose message names the missing/empty section (e.g. "adapter.toml is missing the required `[metering]` section" / "`[capabilities]` declares no capabilities"). The engine maps it to a `RegistryError` variant; `kt` renders a miette diagnostic quoting the section. Mandatory sections (this story's seed): the contract-version field, the adapter identity (kind/name), at least the lifecycle op templates the trait requires as data, a non-empty `[capabilities]` declaration, and a `[metering]` section with a viable source (AC4).
- **AC3 — the conformance MOCK adapter + scripted fake agent is a REUSABLE FIXTURE.** Ship a `mock` **native** `AgentAdapter` implementation in `ktesio-conformance` plus a scripted **fake agent** that stays **inert** this story (no real process spawn — 1-4 owns execution). "Scripted fake agent" = a described/inspectable stand-in (e.g. a struct describing a canned lifecycle-op script, or a tiny inert helper binary/asset that is NOT launched yet). This is the fixture "used by this and all later lifecycle/governance tests," so design it to be imported by later stories (1-4 start/stop, 3.x metering, 6.4 TCK).
- **AC4 — no viable Metering Source → REJECT at registration (FR-19 hard line).** An adapter (native or manifest) whose declared metering source is absent / `none` / not one of the two viable kinds is **rejected at registration** with a diagnostic naming the missing `[metering]` section (or the invalid value). This is a HARD line: there is no "register anyway" path. Prove it for both a manifest adapter (missing/invalid `[metering]`) and — as a unit — the validation predicate directly.

## Tasks / Subtasks

- [x] **Task 1 — Adopt a TOML deserializer in the workspace (AC: 1, 2) — DEP DECISION, see Dev Notes "TOML dependency"**
  - [x] `adapter.toml` parsing is a NEW capability. The repo currently has **no** TOML deserializer (skills `manifest.rs` uses `serde_json`; `toml`/`basic-toml` are absent from `Cargo.toml` and `Cargo.lock` — verified). Add ONE to `[workspace.dependencies]`. **Recommended conservative default: `toml = "1"`** (current stable `1.1.2`; serde-native, matches the existing `#[serde(deny_unknown_fields)]` manifest style; 1.x is now GA). **Lighter alternative to flag: `basic-toml = "0.1"`** (`0.1.10`; no `toml_edit` sub-tree, deserialize-only). This is an `[OPEN QUESTION]` for Islam — it needs his nod exactly like `rusqlite`/`directories` did (NFR-8 lean-deps). **Default to `toml` unless Islam says otherwise**; record the resolved pin in the Dev Agent Record.
  - [x] The TOML dep belongs to **`ktesio-adapter-api`** (it owns parsing per AD-3), referenced with `{ workspace = true }`. Do NOT add it to `ktesio-engine` or `kt` — they consume the *parsed* form via adapter-api's public API.
  - [x] `serde` (derive) is already in `[workspace.dependencies]`; add it to `ktesio-adapter-api` (the schema structs derive `Deserialize`).
- [x] **Task 2 — `ktesio-adapter-api`: the Adapter Contract types (AC: 1, 2, 4) — SCHEMA OWNER, AD-3**
  - [x] This crate is doc-only today (verified). Create its module tree (entity-timing — first real code). Suggested layout in Dev Notes "adapter-api module placement". Add a crate-level `CONTRACT_VERSION` semver constant (the FR-27/FR-30 seed; do NOT freeze — 6.6 owns v1 freeze).
  - [x] `OsId` enum (`Linux`, `Macos`, `Windows`, plus a catch-all) with `OsId::current()` resolving from `std::env::consts::OS` — **cfg-free** (this is the per-OS-as-data mechanism; the OS-cfg CI gate forbids `#[cfg]` here). Derive serde so declarations can be keyed by it in TOML.
  - [x] `SupportLevel` enum (`Guaranteed`, `BestEffort`, `Unsupported`) — the AD-4 support classification, as data.
  - [x] `CapabilityDeclaration` type: capability × OS → `SupportLevel` (per-OS keyed, AD-4). Provide `effective(os: OsId) -> EffectiveCapabilities` (or similar) projecting to a single OS. Capability keys this story (seed set): at minimum `pause` (the AD-4 exemplar) and `interaction` — enumerate the capability keys as data; keep the set small and documented (`[ASSUMPTION]` the exact key set; tag it — the full set freezes at 6.6).
  - [x] `MeteringSource` enum (`SelfReported`, `EngineObserved`) — the AD-7 declaration. Model "no viable source" as the **absence** of a valid `[metering]` section (AC4), not a `MeteringSource::None` variant that the engine would then have to special-case — a missing/invalid section is a validation error, so the type stays honest (an `AgentAdapter` that registers always has a real source).
  - [x] `AgentAdapter` trait: declares the lifecycle ops (align names with the ratified state machine verbs — `start`, `stop`, `pause`, `resume` — AD-15) as method signatures a native adapter implements, plus accessors for the adapter's `CapabilityDeclaration` and `MeteringSource`. **Nothing is executed this story** — the trait exists so native adapters can implement it (the mock does) and so its shape documents what manifest templates must cover. Keep method bodies unimplemented in the trait (it is an interface). `[ASSUMPTION]` the exact method set — keep it minimal and tag it; 1-4/6.4 widen it.
  - [x] Manifest schema: serde structs mirroring `adapter.toml` — `exec`/`args`/`env` templates per lifecycle op, `[capabilities]` (the per-OS declaration), `[metering]` (source config), `[interaction]` (channel wiring, e.g. stdin/stdout default per AD-12). Use `#[serde(deny_unknown_fields)]` (matches the repo's `manifest.rs` prior art and catches typos). Add `Manifest::from_toml_str(&str) -> Result<Manifest, ManifestError>` and `Manifest::validate(&self) -> Result<(), ManifestError>` (mandatory-section + viable-metering checks). `ManifestError` is `thiserror` (adapter-api uses `thiserror`, never `miette` — conventions). Message text NAMES the failing section (AC2).
  - [x] Provide the manifest→`CapabilityDeclaration` and manifest→`MeteringSource` extraction so the engine gets a uniform declaration whether the adapter is native or manifest (the engine treats both identically — AD-3 "two kinds, one trait").
- [x] **Task 3 — `ktesio-conformance`: the mock native adapter + scripted fake agent (AC: 3)**
  - [x] Implement `MockAdapter` as a **native** `AgentAdapter` (the `mock` kind the engine resolves — see 1-2 which registered `kind = "mock"` as a free string; this story makes `mock` resolve to this adapter). It declares a `CapabilityDeclaration` (with a per-OS example — e.g. `pause: guaranteed` on Unix, `best-effort` on Windows, to exercise AC1's per-OS projection) and a viable `MeteringSource` (`self-reported`) so it registers successfully.
  - [x] Ship the **scripted fake agent** as a reusable, **inert** fixture (no process spawn this story). It describes a canned lifecycle-op "script" that 1-4 will actually run. Expose it publicly from `ktesio-conformance` so later stories import it (state in a doc comment: "used by this and all later lifecycle/governance tests").
  - [x] `ktesio-conformance` already depends on `ktesio-adapter-api` (verified). Keep it a normal (non-dev) dependency so the mock is a real, importable adapter. Do NOT build the full TCK (that is 6.4) — just the mock + fake-agent fixture.
- [x] **Task 4 — `ktesio-engine`: manifest LOADER + VALIDATOR and adapter resolution (AC: 1, 2, 4) — engine consumes the parsed form only**
  - [x] New engine module (suggested `ktesio-engine::adapter` — see Dev Notes) that RESOLVES a `kind`/manifest to an `AgentAdapter`:
    - **native** builtin (e.g. `mock` → the conformance `MockAdapter`), and
    - **manifest** by path (load the directory's `adapter.toml`, parse+validate via `ktesio-adapter-api`, build a manifest-backed adapter view).
  - [x] The loader READS + PARSES + VALIDATES only. It stores templates + declarations. It **does NOT execute** any lifecycle op (1-4 owns the manifest executor / process launch). State this boundary in the module doc.
  - [x] Registration validation (runs BEFORE any filesystem side effect — atomicity): (1) resolve the adapter; (2) confirm a `CapabilityDeclaration` is present (non-empty); (3) confirm a viable `MeteringSource` is declared — else REJECT (FR-19). Only after all pass does registration proceed to the row insert + home creation.
  - [x] Persist the **effective (current-OS)** Capability Declaration with the Agent Instance. Choose the persistence site (Dev Notes "Persisting the effective declaration"): a new column on `agent_instances` **or** a file in the Agent Home. `[ASSUMPTION]` — recommend a `capabilities` snapshot **file in the Agent Home** (AD-6 says bulky/structured artifacts are files in the home; keep the DB lean), written atomically as part of `materialize_home`. Tag the choice; either is defensible.
  - [x] The engine consumes adapter-api's parsed `Manifest`/`CapabilityDeclaration`/`MeteringSource` types and defines NO schema of its own (AD-3). Add `ktesio-adapter-api` as a real (non-`_`) dependency use — 1-2's `use ktesio_adapter_api as _;` edge-proof in `lib.rs` becomes a real import this story.
- [x] **Task 5 — Integrate with the 1-2 registration path (AC: 1, 2, 4) — atomic, no orphan rows**
  - [x] Extend `Registry::register` (currently `register(name, kind)`) to also accept a manifest path (e.g. `register_with_adapter(name, AdapterRef)` where `AdapterRef` is `Native(kind)` or `Manifest(path)` — or add a new method and keep the old one for `--kind`). Preserve the F2 lesson: adapter resolution + validation happens FIRST, entirely side-effect-free; the DB row insert and `materialize_home` only run once validation passes, and the existing rollback (RegisterOrphanRow) path is retained.
  - [x] New `RegistryError` variants for adapter failures (each carries enough for a `kt` remediation hint — NFR-1): e.g. `ManifestNotFound { path }`, `ManifestInvalid { path, detail }` (detail names the section — AC2), `NoMeteringSource { adapter }` (AC4), `UnknownAdapterKind { kind }` (an unrecognized native kind). Keep them `thiserror`, no miette. **`kt`'s `map_error` is exhaustive over `RegistryError` — adding variants forces new arms (compile-time safety net); wire each to a miette diagnostic.**
  - [x] The `kind` column semantics are unchanged for native adapters; for a manifest adapter, decide what goes in `kind` (`[ASSUMPTION]` the manifest's declared kind/name, or a `manifest:` marker — tag it) and how the manifest path is recorded (it will be needed at start in 1-4 — persist it, likely in the Agent Home or a new column; tag the choice and note the 1-4 dependency).
- [x] **Task 6 — `kt agent register --manifest` + surface the effective Capability Declaration (AC: 1, 2, 4; CLI-first gate)**
  - [x] Extend the clap `AgentCommands::Register` (currently `{ name, kind }`) to accept `--manifest <path>` as an alternative to `--kind <kind>`. `[ASSUMPTION]` make `--kind` and `--manifest` mutually exclusive but at least one required (clap `required_unless_present` / `conflicts_with`); tag the exact shape. `main.rs` already dispatches `AgentCommands::Register { name, kind } => cli::agent::register(&name, &kind)` — update the signature + dispatch.
  - [x] `cli::agent::register` resolves the flags to the engine's `AdapterRef` and calls the extended registry API. On success, print the Agent Home path (as today) AND the effective per-OS Capability Declaration (a small rendered block/table — reuse `ui.rs`). On failure, map the new `RegistryError` variants to miette diagnostics (extend `crates/kt/src/error.rs` with new `#[derive(Error, Diagnostic)]` structs following the existing `Agent*` pattern; codes like `ktesio::agent::manifest_invalid`, `ktesio::agent::no_metering_source`, `ktesio::agent::manifest_not_found`, `ktesio::agent::unknown_kind`).
  - [x] Surface the effective Capability Declaration for an existing instance (AC1 "visible for the instance"): `[ASSUMPTION]` add `kt agent show <name>` OR extend `kt agent list` with a capabilities view. Recommend a dedicated `kt agent show <name>` (cleaner than overloading the list table); tag it. `--json` remains **out of scope** (FR-4 / story 1.7) — human-readable suffices; note the deferral.
  - [x] Output discipline (AD-12): command results (home path, capability rendering) to **stdout**; diagnostics/notices to **stderr**. Reuse `ui.rs`.
- [x] **Task 7 — Tests: adapter-api unit + engine unit/integration + conformance + kt integration (AC: all; coverage ≥95%)**
  - [x] **adapter-api unit tests** (beside modules): manifest parse **happy path** (a full valid `adapter.toml` → populated structs); each **rejection** — missing `[metering]`, `[metering]` with no viable source (AC4), missing/empty `[capabilities]`, missing contract-version, malformed TOML (syntax error) — each asserting the error **names the section**; `deny_unknown_fields` rejects an unknown key; `OsId::current()` returns a sane value and `CapabilityDeclaration::effective(os)` projects correctly for each OS (drive all three `OsId` values as DATA — no `#[cfg]`, so this runs on every CI OS); `SupportLevel`/`MeteringSource` serde round-trip; `CONTRACT_VERSION` parses as semver.
  - [x] **conformance unit tests**: `MockAdapter` exposes a non-empty `CapabilityDeclaration` and a viable `MeteringSource`; its per-OS declaration projects to the expected level for each `OsId` (proves the AC1 per-OS path via the mock); the scripted fake agent fixture is constructible and inert (does not spawn a process).
  - [x] **engine unit tests** (beside modules — tarpaulin attributes engine coverage best here, per 1-2's lesson): native `mock` resolution succeeds; unknown native kind → `UnknownAdapterKind`; manifest resolution from a temp dir with a valid `adapter.toml` succeeds and yields the effective declaration; manifest missing → `ManifestNotFound`; manifest invalid (missing section) → `ManifestInvalid` naming the section; manifest with no viable metering → `NoMeteringSource` (AC4); **registration with an invalid/no-metering adapter leaves NO partial state** (no row, no home — the atomicity/AC4 combo — mirror 1-2's `duplicate_registration_leaves_no_partial_state`); the effective declaration is persisted and re-readable.
  - [x] **engine integration test** (`crates/ktesio-engine/tests/`): register a `mock` instance AND a manifest instance (write a fixture `adapter.toml` into a `TempDir`) through the PUBLIC API only, then read back the effective declaration — doubles as the AD-2 "public API is sufficient" proof for these capabilities.
  - [x] **kt integration test** (`crates/kt/tests/`, `CARGO_BIN_EXE_kt` + `KTESIO_STATE_DIR` temp-dir harness): `kt agent register demo --kind mock` prints the home path + the effective capabilities + exits 0; `kt agent register m --manifest <tmp-with-valid-toml>` exits 0; `--manifest <tmp-with-invalid-toml>` exits non-zero with a diagnostic naming the section; a no-metering manifest exits non-zero (AC4); `kt agent show demo` renders the current-OS declaration. Reuse the `run_kt_agent`/`KtRun` helper from 1-2.
  - [x] **Coverage reasoning (NFR-3, non-negotiable):** the gate is `cargo tarpaulin --workspace --fail-under 95` (1-2 measured **96.24%**). Three crates gain real code (adapter-api, conformance, engine). adapter-api's parse/validate branches and conformance's mock are the risk — enumerate and hit every `ManifestError` variant, every `RegistryError` adapter variant, both adapter kinds' resolution, per-OS projection for all three `OsId`s, and the mock's declaration. Cover engine + adapter-api logic with **their own** unit tests (do NOT lean on `kt` integration tests — tarpaulin attributes cross-crate coverage unevenly, per 1-2).
- [x] **Task 8 — Local gates + docs currency + CI gates green (AC: all; NFR-3, NFR-7)**
  - [x] Run the full local gate set (same as 1-1/1-2, with `cargo +stable` = 1.96.1): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --all-targets`; `cargo tarpaulin --workspace --fail-under 95`; `python3 scripts/check_docs.py`; `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py`.
  - [x] **OS-cfg gate MUST stay green** (`grep -rn --include='*.rs' -E 'cfg[!(]?.*(unix|windows|target_os|target_family)' crates/` → only the pre-existing allowlisted hits: backends dir [empty] + the two grandfathered kt self-update files). Per-OS behavior is DATA via `OsId` — if you reach for `#[cfg]`, you did it wrong. Also beware doc-comment text containing the literal token `unix`/`windows` inside a `cfg(...)`-shaped string (1-2 hit this — the gate is a text grep and does not exempt comments).
  - [x] **Boundary gate MUST stay green** (`cargo tree -p ktesio -e normal,build --all-features` → only `ktesio-engine`/`ktesio-adapter-api` internal edges). The engine gains a real `ktesio-adapter-api` use — that edge is ALLOWED. Do NOT let `kt` gain an edge to `ktesio-conformance` (the mock lives behind the engine's resolution; `kt` names `--kind mock` as a string, it does not depend on the conformance crate). `[GUARD]` if `kt` or `ktesio-engine` needs the mock in *tests*, add `ktesio-conformance` as a **dev-dependency** only (`-e normal,build` excludes dev-deps, so the gate stays green) — mirrors how 1-2 added `rusqlite` to `kt` as a dev-dep.
  - [x] **MSRV gate** (`cargo +1.96.1 check --workspace`) stays green — verify the chosen TOML crate (and its transitives) build on 1.96.1. `toml` 1.x / `basic-toml` are pure-Rust with modest MSRVs; low risk, but confirm (mirrors 1-2's `libsqlite3-sys`/`cfg_select!` MSRV finding).
  - [x] Docs currency (NFR-7 / `check_docs.py`): update `docs/architecture.md` (engine gains an adapter-resolution module; adapter-api gains the contract types + manifest schema; the mock lands in conformance). If a manifest schema reference belongs in docs, seed it minimally (do NOT publish the full contract — 6.6). Do NOT touch open AI-2/AI-3/AI-4 items.
  - [x] Manual smoke: `cargo run -p ktesio -- agent register demo --kind mock` (prints home + capabilities under an overridden state dir); write a tiny valid `adapter.toml` in a temp dir and `kt agent register m --manifest <dir>`; `kt agent show demo`; a deliberately-broken `adapter.toml` prints a section-naming diagnostic.

## Dev Notes

**This file is the dev agent's ONLY guide. The sections below fix the decisions the AC leave open. Where a choice is genuinely open it is tagged `[ASSUMPTION]` (conservative default chosen) or `[OPEN QUESTION]` (surface to Islam; do not block — continuous-loop mode).**

### Scope discipline — this story SEEDS the contract; it does NOT freeze it, and it EXECUTES nothing

- **SEED, not freeze.** FR-27 (published, versioned Adapter Contract + machine-readable per-OS Capability Declaration + conformance test-kit) is **seeded here** and **completed/frozen at 6.4 (TCK) and 6.6 (contract v1 freeze + publish)**. Add a `CONTRACT_VERSION` constant and the minimal types; do NOT build the full contract, the full capability set, the versioning/negotiation logic (6.6), or the full TCK (6.4). Keep the type surface small and additively-extensible.
- **Runs NOTHING.** This story stores + validates templates and declarations. It does **not** start/stop/pause any agent, does **not** spawn a process, does **not** add tokio. Lifecycle EXECUTION is **story 1-4** (the manifest executor, the process launch, the `blocking()` facade). The scripted fake agent is **inert**. State this boundary in every module doc that could tempt a reader to "just wire up start".
- **OUT OF SCOPE (explicit):** actual lifecycle execution / start / stop (1-4); the full contract freeze + conformance TCK completion (6.4/6.6); the opencode paper mapping (6.5); tokio (1-4); `--json` on read commands (FR-4 / 1.7); config layering/mapping (Epic 2); metering *ingestion* (Epic 3 — this story only *declares* the source).

### Architecture bindings (spine, FINAL, binding)

- **AD-3 (activated here) — two adapter kinds, one trait; the manifest schema is OWNED BY adapter-api.** Every agent integrates as an `AgentAdapter` of exactly one kind: **native** (Rust impl compiled in — `hermes`, conformance `mock`) or **manifest** (a directory with `adapter.toml` declaring lifecycle exec/args/env templates, capability declaration, metering-source config, interaction wiring — loadable by path at registration). No dynamic library loading. **The generic manifest executor lives in the engine; manifests carry no code. The manifest schema's types and validation are defined ONLY in `ktesio-adapter-api` and versioned under the same contract semver — the engine executor consumes that crate's parsed form and never defines its own schema.** [Source: ARCHITECTURE-SPINE.md#AD-3.]
- **AD-4 — Capability Declarations are (capability × OS).** Keyed per OS; the engine surfaces the *effective* (current-OS) declaration everywhere capabilities are shown. **This is DATA keyed by a runtime OS id, NOT `#[cfg]`.** (Process control via `ProcessBackend` and the actual SIGSTOP/Job-Object mechanics are 1-4/1-5; this story only models + surfaces the declaration.) [Source: ARCHITECTURE-SPINE.md#AD-4.]
- **AD-7 — Metering Source is declared (`self-reported` | `engine-observed`).** This story only *declares* it in the adapter/manifest and validates its presence. The metering *pipeline* (UsageEvent → ledger → BudgetEvaluator, the loopback listener) is Epic 3. **FR-19 hard line: an adapter with NO viable metering source is REJECTED at registration** — enforce it here. [Source: ARCHITECTURE-SPINE.md#AD-7; PRD FR-19.]
- **AD-2 — crate law + public API.** New contract types land in `ktesio-adapter-api` (independently semver'd). The engine depends on adapter-api and consumes its parsed types; `kt` depends on the engine's public API (+ adapter-api types) only. The mock lives in `ktesio-conformance` (behind engine resolution). Never engine→kt, never engine→concrete adapter *at the type level for a specific agent* — the engine resolves the mock through a builtin-registry indirection, and if it needs the concrete mock in tests it uses a **dev-dependency** (boundary gate excludes dev-deps). [Source: ARCHITECTURE-SPINE.md#AD-2.]
- **AD-1 — hexagonal core.** `ktesio-adapter-api` depends on NOTHING internal (pure types + trait, + the TOML/serde deps). `ktesio-engine` depends on adapter-api. The `AgentAdapter` is a downward PORT (spine: "Downward port: `AgentAdapter` (the Adapter Contract)"). Domain code stays free of OS-conditional code. [Source: ARCHITECTURE-SPINE.md#Design Paradigm, #AD-1.]
- **AD-6 / F2 lesson — registration stays atomic.** The 1-2 store is the sole state store; the F2 orphan-row lesson (rollback the row if the home step fails) is live. Adapter validation is a pure step that must pass BEFORE any DB/filesystem side effect, so a rejected adapter leaves ZERO partial state. Persist the effective declaration inside the atomic path. [Source: ARCHITECTURE-SPINE.md#AD-6; 1-2 Dev Agent Record F2.]
- **Naming / errors conventions.** Glossary terms verbatim in code — **Adapter, Adapter Contract, Capability Declaration, Metering Source, Agent Instance, Agent Home** (exact PRD Glossary terms). `thiserror` in engine AND adapter-api; `miette` wraps in `kt` only. Every partial failure names the thing + reason + remediation (NFR-1). [Source: ARCHITECTURE-SPINE.md#Consistency Conventions; PRD §3 Glossary.]

### Per-OS Capability Declaration as DATA — the cfg-free rule (CRITICAL)

The OS-cfg CI gate (regex `cfg[!(]?.*(unix|windows|target_os|target_family)`, allowed ONLY under `crates/ktesio-engine/src/backends/` + two grandfathered legacy files) **will fail the build** if per-OS logic uses `#[cfg]`. Model the current OS as data:

- `OsId` enum in adapter-api, `OsId::current()` = `match std::env::consts::OS { "linux" => Linux, "macos" => Macos, "windows" => Windows, _ => Other }`. `std::env::consts::OS` is a runtime `&str` — no `#[cfg]`, gate stays green, and tests can drive **all three** `OsId` values on any CI OS (a real win: per-OS behavior is unit-testable everywhere, not just on the matching runner).
- The `CapabilityDeclaration` stores every (capability, OsId) → `SupportLevel`; `effective(OsId::current())` projects to the running OS. The engine persists the *projected* result.
- **Watch the grep on doc comments:** 1-2's near-miss was a doc comment literally containing `` `#[cfg(unix/windows/target_os)]` `` while *explaining* the rule — the text grep flagged it. When you document the cfg-free rationale, avoid writing the `cfg(...)`-shaped token with an OS word inside it.
- `backends/` is NOT created this story (no process/OS syscalls — pure declaration data). Do not create it.

### TOML dependency — NEW dep, needs Islam's nod `[OPEN QUESTION → default and proceed]`

`adapter.toml` requires a TOML deserializer. **Verified 2026-07-04:** the tree has NONE (`grep` over both `Cargo.toml`s and `Cargo.lock` — no `toml`, no `basic-toml`; skills `manifest.rs` uses `serde_json`). This is a genuine new dependency under NFR-8 (lean deps) and needs Islam's sign-off exactly like `rusqlite`/`directories` did.

| Candidate | Current version (crates.io, 2026-07-04) | Trade |
| --- | --- | --- |
| **`toml`** *(recommended default)* | `1.1.2` (1.x is now GA) | serde-native; the mainstream choice; matches the existing `#[serde(deny_unknown_fields)]` `manifest.rs` style; pulls `toml_edit`/`winnow` (format-preserving parser) transitively — a slightly heavier tree but battle-tested and 1.x-stable. |
| `basic-toml` | `0.1.10` | dtolnay's minimal deserialize-only parser; **leaner tree** (no `toml_edit`); no format preservation (fine — we only *read* adapter.toml this story). Good if Islam prioritizes a minimal dependency footprint. |

**Recommendation: `toml = "1"`** for stability + serde-native ergonomics + style consistency, **unless Islam prefers the leaner `basic-toml`**. Add to `[workspace.dependencies]`, reference from `ktesio-adapter-api` only. Record the resolved pin (from `Cargo.lock`) in the Dev Agent Record (spine stack-verification note). **Continuous-loop guidance: default to `toml`, tag it as an assumption pending Islam's nod, do not block.**

### adapter-api module placement `[ASSUMPTION on layout]`

First real code in this crate. Suggested tree (adjust to taste; the FIXED invariant is that the schema + capability + metering + trait types live ONLY here):

```text
crates/ktesio-adapter-api/src/
  lib.rs           # crate docs + CONTRACT_VERSION + re-exports of the public contract surface
  os.rs            # OsId (+ current()) — cfg-free runtime OS id
  capability.rs    # SupportLevel, CapabilityDeclaration (per-OS), effective(os) projection
  metering.rs      # MeteringSource enum
  adapter.rs       # AgentAdapter trait (lifecycle op signatures + declaration accessors; executes nothing)
  manifest.rs      # adapter.toml serde structs + from_toml_str + validate + ManifestError (thiserror)
```

- `CONTRACT_VERSION`: a `pub const CONTRACT_VERSION: &str = "0.1.0";` (or a `semver::Version` via the already-present `semver` workspace dep) — the FR-27/FR-30 seed. Do NOT implement negotiation (6.6).
- Keep the trait object-safe-ish and minimal; 1-4 adds execution methods, 6.4 adds whatever the TCK needs. Do not speculatively add methods.

### engine loader/validator boundary `[ASSUMPTION on module name]`

Suggested `crates/ktesio-engine/src/adapter/` (or `adapter.rs`): resolves `AdapterRef` → an `AgentAdapter`. It PARSES + VALIDATES + stores declarations; it EXECUTES nothing (1-4 owns the executor). The builtin-native registry (name → constructor) is a tiny map so the engine never hard-depends on a *concrete* adapter type in its shipping graph beyond the resolution indirection; if the concrete `MockAdapter` is needed to seed that map, weigh (a) a dev-dependency for tests vs (b) a minimal builtin table — **prefer keeping `mock` resolvable in the shipping engine** so `kt agent register --kind mock` works for a real operator, which means `ktesio-conformance` becomes a **normal** dependency of the engine *if and only if* that keeps the boundary gate green (it does — the gate only forbids a `kt`→conformance edge and a `kt`→non-engine edge; an `engine`→conformance edge is not in the gate's allowlist check because the gate inspects `cargo tree -p ktesio`, i.e. kt's graph, not the engine's). **`[VERIFY during dev]`**: confirm `cargo tree -p ktesio -e normal,build --all-features` still shows only `ktesio-engine`/`ktesio-adapter-api` after the engine takes a `ktesio-conformance` dep — if the transitive pulls conformance into kt's graph and trips the gate, fall back to a dev-dependency + a test-only builtin, and register `mock` in the shipping engine via a trait-object constructor that does not name the conformance type. This is the one real graph-shape risk in the story; resolve it explicitly and record the decision.
  - *(Rationale for flagging: the boundary gate is `-p ktesio`. An `engine → conformance` normal edge is transitive into `kt` and WOULD appear in `cargo tree -p ktesio`, tripping the allowlist. So the clean answer is almost certainly: the mock stays a **dev/test** fixture, and the shipping engine's builtin-native registry is either empty this story or holds only adapters that don't live in `conformance`. Simplest conservative path: **`mock` is resolvable only in tests** (engine + kt add `ktesio-conformance` as a dev-dependency), and the shipping `--kind mock` path is exercised via integration tests, not shipped to end operators until a non-conformance builtin exists. Tag this and pick the path that keeps ALL gates green; document which.)*

### Persisting the effective declaration `[ASSUMPTION]`

Recommend writing the projected current-OS `CapabilityDeclaration` as a small file in the Agent Home (e.g. `capabilities.toml`/`.json`) inside `materialize_home` (atomic with the rest of home creation; AD-6 keeps structured artifacts as files, DB lean). Alternative: a new `capabilities` TEXT column on `agent_instances` (a schema v2 migration — the 1-2 migration framework supports stepping via `PRAGMA user_version`). Either is fine; the file keeps the schema stable this story. Also persist the **manifest path** for a manifest adapter (1-4 needs it to launch) — likely alongside the capabilities file or in the instance config. Tag both choices; note the 1-4 dependency on the manifest path.

### Integrate with 1-2 registration (the exact seam)

- Today: `Registry::register(&self, name: &str, kind: &str) -> Result<AgentInstance, RegistryError>` (crates/ktesio-engine/src/domain/registry.rs). Order is: validate name → insert row → `materialize_home` (dir + `config.toml`) → rollback row on home failure (F2 `RegisterOrphanRow`).
- This story inserts adapter resolution + validation as a **pre-step** before the row insert, so a bad adapter never creates a row or a home. Keep the existing atomicity + rollback intact. The effective-declaration write joins `materialize_home` (so its failure is covered by the same rollback).
- `map_error` in `crates/kt/src/cli/agent.rs` is an **exhaustive `match` over `RegistryError`** — new variants are a compile error until you add arms. Add a `#[derive(Error, Diagnostic)]` struct per new variant in `crates/kt/src/error.rs` (follow the existing `AgentDuplicateName` … `AgentStore` block; codes `ktesio::agent::*`) and a matching arm.
- `kt/src/main.rs` already has the `Agent`/`AgentCommands` clap groups and dispatch (`AgentCommands::Register { name, kind }`). Extend `Register` with `--manifest`, update the dispatch signature, and add a `Show` subcommand if you go that route. The existing `test_agent_subcommands` and about-text tests will need updating to match.

### Previous-story intelligence (1-1 `done`, 1-2 `done`; baseline `fbfd90f`, carried facts at 1-2 completion)

From the 1-1 + 1-2 Dev Agent Records / File Lists — respect all of this:

- **Workspace is live, 5 crates.** `ktesio-adapter-api` + `ktesio-conformance` are currently **doc-only `publish = false` libs** (0.1.0, `TODO(story 7-4)`) — this story writes their FIRST real code. Keep `publish = false` (7-4 publishes).
- **Engine already has real code (1-2):** `domain` (Registry, InstanceName, LifecycleState [ratified set as data; only `registered` reachable], AgentInstance), `ports::StateStore` (sync port), `store::sqlite` (SQLite AD-6, WAL, `PRAGMA user_version` migration, schema v1), `paths` (cfg-free via `directories`, `KTESIO_STATE_DIR` override), `time` (RFC3339 without a date crate). The engine API is **synchronous** (tokio deferred to 1-4). The engine is the sole path authority.
- **`kt agent register|remove|list` exist** (1-2), thin over the sync registry API, miette-wrapped in `crates/kt/src/error.rs` (`Agent*` structs, `ktesio::agent::*` codes), rendered via `ui.rs`, hermetic tests via `CARGO_BIN_EXE_kt` + `KTESIO_STATE_DIR` (`run_kt_agent`/`KtRun` helper in `crates/kt/tests/helpers/mod.rs`).
- **Registration in 1-2 takes `name` + a free-string `kind`** — THIS story makes `kind` resolve to a real Adapter (native builtin like the conformance mock, or a manifest by path) and validates its Capability Declaration + Metering Source at registration.
- **CI gates armed:** boundary (allowlist: only `ktesio-engine`/`ktesio-adapter-api` internal edges in `cargo tree -p ktesio`), OS-cfg (exact allowlist above), semver (dormant until publish), **msrv (`1.96.1`)**, coverage (`tarpaulin --workspace --fail-under 95`; 1-2 hit 96.24%).
- **MSRV pinned `1.96.1`** (root `[workspace.package] rust-version`, members inherit). The new TOML dep must build on 1.96.1 (verify — mirrors 1-2's `cfg_select!`/`libsqlite3-sys` finding). Local gates run with `cargo +stable` (= 1.96.1 on the dev machine / CI).
- **Error/UX conventions:** `thiserror` in engine + adapter-api (NO miette in either lib — verified: neither has miette in deps), `miette` in `kt` only. `serde`/`serde_json` are in `[workspace.dependencies]`. `#[serde(deny_unknown_fields)]` is the repo's manifest style (skills `manifest.rs`).
- **Open sprint action items AI-2 / AI-3 / AI-4 exist — DO NOT touch them here** (they fold into future workflow/automation/paths-config stories). Mentioned only so you don't "helpfully" fix them and expand scope. **Do NOT edit GitHub issue #65** (the orchestrator syncs it).
- **rusqlite in `kt` is a dev-dependency only** (1-2, to seed a `running` row) — the shipping `kt` never depends on SQLite; the boundary gate (`-e normal,build`) excludes it. Reuse this pattern if `kt`/engine tests need the conformance mock.

### Stack pins (verify at `cargo add`)

| Crate | Pin | Notes |
| --- | --- | --- |
| `toml` *(recommended)* | `1` (`1.1.2`) | verified crates.io 2026-07-04, `max_stable_version = 1.1.2`; 1.x GA; serde-native; add to `[workspace.dependencies]`, reference from `ktesio-adapter-api` only. Record resolved `Cargo.lock` pin + transitives (`toml_edit`, `winnow`, `serde_spanned`, `toml_datetime`) in the Dev Agent Record. |
| `basic-toml` *(leaner alt)* | `0.1` (`0.1.10`) | verified crates.io 2026-07-04; deserialize-only, minimal tree. Swap target if Islam prefers minimal deps. |
| `serde` | `1` (workspace) | already present; add `{ workspace = true }` to adapter-api. |
| `semver` | `1` (workspace) | already present; optional, only if `CONTRACT_VERSION` is a `Version` rather than a `&str`. |
| `tokio` | **NOT this story** | AD-13 is 1-4. |

### Testing requirements (NFR-3 — coverage ≥95%, CI-enforced, non-negotiable)

- Layout: **unit tests beside modules** in adapter-api, conformance, and engine; **integration tests per crate** (`crates/ktesio-engine/tests/`, `crates/kt/tests/`).
- **Why coverage is at risk:** three crates gain real code. adapter-api's `validate()` has many rejection branches (each mandatory section, malformed TOML, unknown field, no-viable-metering) — enumerate and hit every one, each asserting the section name. conformance's mock adds coverable lines that must be exercised. The engine's adapter-resolution branches (native ok / unknown kind / manifest ok / not-found / invalid / no-metering) and the "rejected adapter leaves no partial state" path are prime coverage holders. Cover adapter-api and engine logic with **their own** unit tests (tarpaulin attributes cross-crate integration coverage unevenly — 1-2's explicit lesson).
- Per-OS projection is testable on **every** CI OS because it is data — drive all three `OsId` values in unit tests (do NOT gate them behind the running OS).
- `kt` integration tests assert the CLI contract (exit codes, stdout home path + capabilities, stderr diagnostics) via the `CARGO_BIN_EXE_kt` + `KTESIO_STATE_DIR` harness.
- Determinism: keep fixture `adapter.toml` content inline in tests (write to a `TempDir`), assert on structure. RFC3339 timestamps remain time-varying — assert on structure, not exact values (1-2 convention).

### Project Structure Notes

New code this story (fills more of the spine Structural Seed):

```text
crates/ktesio-adapter-api/src/     # FIRST real code: os, capability, metering, adapter (trait), manifest (schema+validate), CONTRACT_VERSION
crates/ktesio-conformance/src/     # FIRST real code: MockAdapter (native) + scripted fake agent fixture (INERT)
crates/ktesio-engine/src/adapter/  # adapter resolution + manifest loader/validator (parses via adapter-api; executes NOTHING)
crates/ktesio-engine/src/domain/registry.rs  # extended: adapter validation as a pre-step; new RegistryError variants; persist effective declaration
crates/ktesio-engine/tests/        # integration: register mock + manifest via public API; read effective declaration
crates/kt/src/cli/agent.rs         # --manifest flag; render effective declaration; new error mappings
crates/kt/src/error.rs             # new Agent* diagnostic structs (ktesio::agent::manifest_invalid, ::no_metering_source, ::manifest_not_found, ::unknown_kind)
crates/kt/src/main.rs              # Register gains --manifest; optional Show subcommand; update clap tests
crates/kt/tests/                   # kt integration: register --kind mock / --manifest valid|invalid|no-metering; show
```

- Do NOT create `backends/`, `metering/` (pipeline), `skills/`, `events.rs`, or any executor — later stories (1-4 executor + backends; Epic 3 metering pipeline; Epic 8 skills).
- Variance from spine seed: the spine lists `ktesio-adapter-api` as "Adapter Contract: traits, CapabilityDeclaration, manifest schema (AD-3)" and `ktesio-conformance` as "TCK + mock adapter" — this story realizes the contract types + manifest schema + the mock (NOT the TCK; 6.4). The engine `adapter` module is an `[ASSUMPTION]` on placement (the spine names the manifest executor as living in the engine but not the module name).

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.3] (ACs verbatim) + #Epic 1 (FR-1..10 scope; FR-27 minimal seed) + #Epic 6 (FR-27/FR-28/FR-29/FR-30 — where the contract completes/freezes; do NOT over-build here) + #FR Coverage Map (FR-27 "minimal declaration seeded in Epic 1; completed and frozen in Epic 6").
- [Source: _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md#AD-3 (two kinds/one trait; schema owned by adapter-api under contract semver; engine consumes parsed form), #AD-4 (per-OS Capability Declaration keyed capability×OS; effective current-OS surfaced), #AD-7 (Metering Source declared; FR-19 hard line), #AD-2 (crate law/public API/boundary), #AD-1 + #Design Paradigm (hexagonal; AgentAdapter downward port; adapter-api depends on nothing internal), #AD-6 (SQLite store; atomicity), #Consistency Conventions (naming/errors/OS-cfg/testing), #Structural Seed, #Stack.]
- [Source: PRD — FR-1 (path registration), FR-27 (published versioned Adapter Contract + machine-readable per-OS Capability Declaration + conformance test-kit — SEED only), FR-19 (metering mandatory; no-source adapters rejected at registration), FR-30 (contract semver — seed the version constant), §3 Glossary (Adapter, Adapter Contract, Capability Declaration, Metering Source, Agent Instance, Agent Home — exact terms).]
- [Source: _bmad-output/implementation-artifacts/1-1-*.md — workspace facts, CI gates (boundary/OS-cfg/semver), error/UX conventions, `publish=false` skeletons.]
- [Source: _bmad-output/implementation-artifacts/1-2-*.md — Dev Agent Record + File List: engine domain/ports/store/paths/time; `Registry::register(name, kind)` seam; F2 orphan-row atomicity lesson; `RegistryError` shape + exhaustive `map_error`; `kt agent` clap wiring; `CARGO_BIN_EXE_kt`+`KTESIO_STATE_DIR` harness; MSRV 1.96.1; rusqlite-as-dev-dep boundary pattern; coverage 96.24%.]
- [Source: crates/ktesio-adapter-api/{src/lib.rs,Cargo.toml} + crates/ktesio-conformance/{src/lib.rs,Cargo.toml} (doc-only skeletons this story fills); crates/ktesio-engine/src/{lib.rs (`use ktesio_adapter_api as _;` edge-proof to make real), domain/registry.rs, domain/error.rs, ports/{mod,state_store}.rs, store/sqlite.rs (schema v1 + user_version migration)}; crates/kt/src/{cli/agent.rs (exhaustive map_error), error.rs (Agent* diagnostic pattern), main.rs (Agent/AgentCommands clap), manifest.rs (serde `deny_unknown_fields` prior art)}; root Cargo.toml (`[workspace.dependencies]`); .github/workflows/ci.yml:112-196 (boundary + OS-cfg gate bodies).]
- [Source: crates.io API 2026-07-04 — `toml` max_stable 1.1.2; `basic-toml` max_stable 0.1.10.]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (Claude Opus 4.8), via the `bmad-dev-story` workflow.

### Debug Log References

- All gates run with `cargo +stable` (= 1.96.1 on this machine; the active default toolchain is 1.94.1, which is below MSRV and would fail the bundled-SQLite build — every gate explicitly pins `+stable` or `+1.96.1`).
- `toml` resolved to `1.1.2+spec-1.1.0` in `Cargo.lock`. Transitive tree: `serde_spanned 1.1.1`, `toml_datetime 1.1.1`, `toml_parser 1.1.2`, `toml_writer 1.1.1`, `winnow 1.0.3`. Note: `toml 1.x` does **not** pull `toml_edit` (the story anticipated it might); the tree is leaner than expected. Builds clean on MSRV 1.96.1.
- Party Mode: no BMAD menu offering Party Mode was presented during this headless dev-story run (dev-story is a linear workflow with no such menu), so there was no choice to make.
- **Review fix pass (2026-07-04, dev-story FIX mode):** applied the approved code-review fixes F1, F2, F3, F4, F5, F6, F8 + two spine edits. All gates re-run green with actual numbers (see the review-fix Completion Notes below). F7/F9 and AI-2/AI-3/AI-4 left untouched; GitHub issue #65 untouched; conformance still dev-only (boundary gate re-confirmed green). `semver` promoted from a dev-dependency to a normal dependency of `ktesio-adapter-api` (F5 uses it in `validate()`); it was already a workspace dependency, so this is not a NEW external dep.

### Completion Notes List

**Outcome:** COMPLETE. All 4 ACs satisfied; all 8 tasks / 46 subtasks checked; all gates green.

**What was built (by AC):**
- **AC1 (schema owner + effective per-OS declaration):** `ktesio-adapter-api` gained its first real code — `OsId` (cfg-free, resolved from `std::env::consts::OS`), `SupportLevel`, `Capability` (seed set: `pause`, `interaction`), `CapabilityDeclaration` (capability × OS → level) with `effective(os)` projection, `MeteringSource`, the `AgentAdapter` trait (lifecycle op signatures + declaration accessors; nothing executed), the `adapter.toml` serde schema with `Manifest::from_toml_str` + `validate()`, and `CONTRACT_VERSION = "0.1.0"`. `kt agent register --manifest <dir-or-file>` loads+validates before any side effect, persists the projected current-OS declaration as `adapter.json` in the Agent Home, and prints the home path + a rendered capability table. `kt agent show <name>` renders it too.
- **AC2 (missing-section rejection names the section):** `Manifest::validate()` returns section-naming errors (`` `contract_version` field ``, `` `[adapter]` section ``, `` `[lifecycle]` section ``, `` `[capabilities]` section ``, `` `[metering]` section ``). The engine maps them to `RegistryError::ManifestInvalid { detail }`; `kt` renders a miette diagnostic quoting the section.
- **AC3 (reusable mock + inert scripted fake agent):** `ktesio-conformance` ships `MockAdapter` (native `AgentAdapter`, per-OS declaration: `pause` guaranteed on Linux/macOS, best-effort on Windows) + `ScriptedFakeAgent` (inert — a canned `ScriptStep` script, spawns nothing; `is_inert()` guard). Publicly exported for 1-4/epic-3/6-4 reuse.
- **AC4 (no viable Metering Source → REJECT, hard line):** modeled as the absence of a valid `[metering]` section (no `MeteringSource::None` variant). Proven for a manifest (missing/invalid `[metering]`) and via the validation predicate directly; there is no "register anyway" path.

**Key design decisions / deviations (tagged):**
- **[DECISION — boundary-critical] The shipping `--kind mock` resolves to an engine-internal builtin (`engine::adapter::builtin::BuiltinMock`), NOT the conformance `MockAdapter`.** The story's Task 3 says "make `mock` resolve to *this* [conformance] adapter," but that would require a normal `engine → conformance` edge, which is transitive into `kt` (`kt → engine → conformance`) and trips the AD-2 boundary gate (`cargo tree -p ktesio`). Dev Notes line 150 explicitly anticipates this and prescribes the conservative path: the conformance mock stays a **dev-dependency** fixture, and the shipping engine resolves `mock` via its own builtin (identical declared shape). This keeps `kt agent register --kind mock` working for real operators AND the boundary gate green. Both mocks share the same per-OS shape; a test in `registration.rs` asserts they match. This is the one real graph-shape risk the story flagged — resolved and documented.
- **[ASSUMPTION] Effective declaration persistence:** written as `adapter.json` (JSON, not TOML) in the Agent Home inside `materialize_home` (atomic with home creation; covered by the same registration rollback). JSON keeps the engine free of a `toml` dependency (AD-3: only adapter-api owns TOML). The snapshot also records `manifest_path` (null for native) for 1-4 to launch a manifest adapter. `kt agent show` reads it back.
- **[ASSUMPTION] `kt agent show <name>`** is the read surface for the effective declaration (chosen over overloading `kt agent list`, per the story's recommendation). `--json` deferred (FR-4 / story 1.7).
- **[ASSUMPTION] clap shape:** `--kind` and `--manifest` are mutually exclusive (`conflicts_with`) and at least one is required (`required_unless_present`).
- **[ASSUMPTION] capability seed set:** `pause` + `interaction` only (the AD-4 exemplar + interaction wiring). Full set freezes at 6.6.
- **[ASSUMPTION] mandatory lifecycle template:** `[lifecycle]` must declare at least a `start` op template (the minimal viable requirement); `stop`/`pause`/`resume` templates are optional seeds. Documented in `manifest.rs`.
- **[ASSUMPTION] `AgentAdapter` lifecycle methods** (`start`/`stop`/`pause`/`resume`) have default bodies returning `AdapterError::Unavailable` ("not implemented until story 1-4") so an accidental early call is explicit. Nothing in the engine calls them; only the accessors are read.
- **Necessary test updates (driven by the design):** 1-2 tests that registered non-`mock` free-string kinds (`"other"`, `"different-kind"`) now use `"mock"`, because `kind` now *resolves* — a non-resolvable kind fails with `UnknownAdapterKind` before duplicate detection. The duplicate-name and reopen tests still prove the same behavior (name collision) with a resolvable kind. Touched: `registry.rs` unit test, `registration.rs` integration test, `agent_cli.rs` integration test.

**Open questions for Islam:**
1. **TOML dep (already approved: `toml = "1"`, resolved `1.1.2`).** For the record: the resolved tree is `toml_parser`/`toml_writer`/`toml_datetime`/`serde_spanned`/`winnow` — leaner than the `toml_edit` tree the story expected. No action needed unless you want the even-leaner `basic-toml`.
2. **`adapter.json` snapshot filename + JSON format** — chose JSON-in-home to avoid an engine `toml` dep. Confirm this is the persistence site you want (vs a DB column). 1-4 will read `manifest_path` from it.
3. **Builtin-`mock` vs conformance-`mock` duplication** — the shipping engine has a small `BuiltinMock` mirroring the conformance fixture (see the boundary DECISION above). If you'd prefer the shipping engine to NOT ship a `mock` kind at all (making `--kind mock` a test-only path), that's a one-line change to `builtin::native`, but it would break the operator-facing `kt agent register --kind mock` and the 1-2 integration tests. I kept it resolvable, per Dev Notes line 149's "prefer keeping mock resolvable in the shipping engine."

**Not done (correctly out of scope):** lifecycle execution / start / stop (1-4), the manifest executor / process launch, tokio, contract freeze + full TCK (6.4/6.6), opencode (6.5), `--json` on read commands, config layering (Epic 2), metering ingestion (Epic 3). AI-2/AI-3/AI-4 untouched. GitHub issue #65 untouched.

---

**Review fix pass (2026-07-04) — approved code-review fixes applied (Status stays `review`):**

- **F1 (MED) — empty/all-unsupported capability now rejected.** Added `CapabilityDeclaration::has_any_support()` (true iff ≥1 capability declares ≥1 non-`Unsupported` (OS→level) entry — rejects both an empty per-OS map AND an all-`Unsupported` declaration). `Manifest::validate()` now calls it for `[capabilities]`, and the engine's `enforce_registration_invariants` uses it so native/builtin adapters clear the identical bar. Tests: empty per-OS map rejected, all-`Unsupported` rejected, best-effort-only passes, normal passes (adapter-api `capability.rs` + `manifest.rs`); native all-`Unsupported` `ResolvedAdapter` → `NoCapabilities` (engine `adapter/mod.rs`).
- **F2 (MED) — real cross-boundary mock-drift guard.** `crates/ktesio-engine/tests/registration.rs::conformance_mock_fixture_matches_builtin_shape` now obtains the shipping builtin's declaration via the public `ktesio_engine::adapter::resolve(&AdapterRef::Native("mock"))` and asserts it EQUALS `ktesio_conformance::MockAdapter::new().capabilities()` directly (both derive `PartialEq`) AND cell-by-cell across `Capability::ALL × OsId::MODELED`. The tautological literal test in `builtin.rs` was reframed (renamed `builtin_mock_declares_the_ad4_exemplar_shape`) to an explicit intra-crate sanity check that points at the real cross-boundary guard.
- **F3 (MED, FIX-NOW) — snapshot no longer frozen to registering OS.** `AdapterSnapshot` now persists the FULL `CapabilityDeclaration` (dropped the register-time `os` + `effective` fields), keeping `manifest_path`. `effective_capabilities()` projects onto `OsId::current()` at READ time (new `read_adapter_snapshot` helper). Greenfield: no `adapter.json` exists in the wild; confirmed nothing else assumed the old single-OS snapshot shape. Tests: snapshot round-trips the full declaration; a hand-written snapshot with per-OS-distinct `pause` levels projects to the current OS at read time (proves read-time projection, drives all modeled OSes as data).
- **F4 (LOW) — unreadable manifest has its own error + message.** New `RegistryError::ManifestUnreadable { path, detail }` (engine) and `AgentManifestUnreadable` miette struct (`ktesio::agent::manifest_unreadable`); the `From<AdapterResolveError>` map now routes `ManifestUnreadable` to it instead of folding into `ManifestInvalid`. `kt` renders "Could not read the adapter manifest at '<path>': <io>. Check that it exists and is readable…" (no "fix the section"). Tests: the adapter-toml-is-a-directory registry test now expects `ManifestUnreadable`; the `kt` map_error test asserts the readability message and that it does NOT claim a section fix.
- **F5 (LOW) — contract_version parsed as semver.** `validate()` now rejects a present-but-non-semver `contract_version` via `semver::Version::parse` (new `ManifestError::InvalidField { field, detail }`). `semver` promoted to a normal `ktesio-adapter-api` dependency (already a workspace dep — not a new external dep). Test: `contract_version = "banana"` rejected naming the field.
- **F6 (LOW, load-bearing for 1-4) — adapter `kind` charset-validated.** `validate()` rejects a `[adapter] kind` that breaks the charset rule via `is_valid_adapter_kind` (same `InvalidField` variant). **Chosen rule: `^[a-z0-9][a-z0-9_-]*$`** (the `InstanceName` token rule — native builtin keys `mock`/`hermes` satisfy it). Tests: tab/space/newline/uppercase/leading-`_`/leading-`-`/dot kinds rejected naming the field; valid kinds pass. Spine Consistency Conventions row added (see spine edits).
- **F8 (LOW) — invalid metering value now names `[metering]`.** `from_toml_str` detects the `toml` unknown-variant error for the sole enum field (`[metering] source`) and attributes it to the section, so the diagnostic names `[metering]` like every other rejection. The overclaiming test (`… || contains("none")`) was tightened to assert the section IS named plus the offending value + `source`. The missing-`[metering]` path is unchanged.

**Spine edits (Islam-approved discovered-invariant refinements; no ADs renumbered):** in `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` — (F6) added a Consistency Conventions "Adapter kind charset" row; (F3) added an AD-4 read-time-projection clause (full declaration persisted; effective view projected onto the running OS at read time). A `refinement` entry recording both (F3, F6, ratified by Islam 2026-07-04) was appended to the spine's `.memlog.md` via `memlog.py` (entries → 29).

**Gate results (all green, actual numbers, 2026-07-04):**
- `cargo fmt --all --check`: clean (after `cargo fmt --all` reflowed the new test code).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean, 0 warnings.
- `cargo test --workspace --all-targets`: **525 passed**, 0 failed (was 513; +12 net new tests).
- `cargo tarpaulin --workspace --fail-under 95`: **96.32%** (3006/3121 lines), +0.10% vs the 96.22% baseline — the fix pass raised coverage. Changed-file deltas: `capability.rs` 40/42 (+0.64%), `manifest.rs` 54/54, `adapter/mod.rs` 59/60 (+3.33%), `registry.rs` 115/122. The 2 uncovered lines in `capability.rs` (`Display for SupportLevel`) and 1 in `adapter/mod.rs:224` (the defensive `NoMeteringSource` fallback, unreachable because `validate()` rejects a missing `[metering]` first) are pre-existing and unrelated to the fixes.
- `python3 scripts/check_docs.py`: 23 Markdown files validated.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py`: 19 tests OK.
- Boundary CI gate (replicated verbatim): GREEN — kt's `normal,build` graph edges are exactly `ktesio-engine` + `ktesio-adapter-api`; no new `kt → conformance` edge (conformance stays dev-only). No new normal `engine → conformance` edge.
- OS-cfg CI gate (replicated verbatim): GREEN — no new `#[cfg]`; `.rs` hits only in the allowlisted `update_check.rs` / `self_update.rs`.
- MSRV `cargo +1.96.1 check --workspace`: GREEN (semver + toml build on 1.96.1; `stable` == 1.96.1 on this machine).

**Untouched / out-of-scope (confirmed):** F7 (`--manifest` any filename — intended) and F9 (dead `NoMeteringSource` arm — accepted design) left as-is with unchanged behavior. AI-2/AI-3/AI-4 untouched. GitHub issue #65 untouched. No commit made; no `.github/workflows` modified.

### File List

**Created:**
- `crates/ktesio-adapter-api/src/os.rs` — `OsId` (cfg-free runtime OS id) — adapter-api
- `crates/ktesio-adapter-api/src/capability.rs` — `SupportLevel`, `Capability`, `CapabilityDeclaration`, `EffectiveCapabilities` — adapter-api
- `crates/ktesio-adapter-api/src/metering.rs` — `MeteringSource` — adapter-api
- `crates/ktesio-adapter-api/src/adapter.rs` — `AgentAdapter` trait + `AdapterError` — adapter-api
- `crates/ktesio-adapter-api/src/manifest.rs` — `adapter.toml` schema + `from_toml_str` + `validate` + `ManifestError` — adapter-api
- `crates/ktesio-engine/src/adapter/mod.rs` — `AdapterRef`, `ResolvedAdapter`, `resolve`, `AdapterResolveError`, `From<AdapterResolveError> for RegistryError` — engine
- `crates/ktesio-engine/src/adapter/builtin.rs` — engine-internal `BuiltinMock` + `native(kind)` table — engine

**Modified:**
- `Cargo.toml` — added `toml = "1"` to `[workspace.dependencies]` (resolved `1.1.2` in `Cargo.lock`) — workspace
- `Cargo.lock` — locked `toml 1.1.2+spec-1.1.0` + transitives (`serde_spanned`, `toml_datetime`, `toml_parser`, `toml_writer`, `winnow`) — workspace
- `crates/ktesio-adapter-api/Cargo.toml` — deps `serde`, `toml`, `thiserror`; dev-deps `semver`, `serde_json` — adapter-api
- `crates/ktesio-adapter-api/src/lib.rs` — module wiring, `CONTRACT_VERSION`, public re-exports, crate-level tests — adapter-api
- `crates/ktesio-conformance/Cargo.toml` — (unchanged deps; already had adapter-api) — conformance
- `crates/ktesio-conformance/src/lib.rs` — `MockAdapter`, `ScriptedFakeAgent`, `ScriptStep`, `MOCK_KIND`, `probe_inert_start` — conformance
- `crates/ktesio-engine/Cargo.toml` — normal dep `serde_json`; dev-dep `ktesio-conformance` — engine
- `crates/ktesio-engine/src/lib.rs` — `pub mod adapter`, real adapter-api re-exports, updated docs — engine
- `crates/ktesio-engine/src/domain/error.rs` — new `RegistryError` variants (`UnknownAdapterKind`, `ManifestNotFound`, `ManifestInvalid`, `NoMeteringSource`, `NoCapabilities`) — engine
- `crates/ktesio-engine/src/domain/registry.rs` — `register_with_adapter` (adapter validation pre-step), `effective_capabilities`, `AdapterSnapshot` persistence in `materialize_home`, story-1.3 tests — engine
- `crates/ktesio-engine/tests/registration.rs` — native+manifest public-API integration tests; conformance-fixture shape check; 1-2 kind fix — engine
- `crates/kt/src/error.rs` — new diagnostic structs (`AgentUnknownKind`, `AgentManifestNotFound`, `AgentManifestInvalid`, `AgentNoMeteringSource`, `AgentNoCapabilities`) — kt
- `crates/kt/src/cli/agent.rs` — `AdapterArg`, `register(name, &AdapterArg)`, `show`, `render_capabilities`, extended `map_error`, tests — kt
- `crates/kt/src/main.rs` — `Register` gains `--manifest` (mutually-exclusive-required with `--kind`); `Show` subcommand; dispatch; help text; clap tests — kt
- `crates/kt/tests/agent_cli.rs` — manifest valid/invalid/not-found, unknown-kind, `show`, capability-on-stdout tests; 1-2 kind fix — kt
- `docs/architecture.md` — workspace layout (adapter-api/conformance no longer skeletons), engine `adapter/` module, adapter-resolution + effective-declaration paragraph — docs

**Modified (review fix pass, 2026-07-04):**
- `crates/ktesio-adapter-api/Cargo.toml` — `semver` promoted from dev-dep to a normal dep (F5; already a workspace dep) — adapter-api
- `crates/ktesio-adapter-api/src/capability.rs` — added `CapabilityDeclaration::has_any_support()` + tests (F1) — adapter-api
- `crates/ktesio-adapter-api/src/manifest.rs` — `validate()`: `has_any_support` for `[capabilities]` (F1), semver `contract_version` (F5), `kind` charset via `is_valid_adapter_kind` (F6), new `ManifestError::InvalidField`; `from_toml_str` attributes the metering unknown-variant error to `[metering]` (F8); new/updated tests — adapter-api
- `crates/ktesio-engine/src/domain/error.rs` — new `RegistryError::ManifestUnreadable { path, detail }` (F4) — engine
- `crates/ktesio-engine/src/domain/registry.rs` — `AdapterSnapshot` persists the FULL declaration (F3); `effective_capabilities()` projects at read time via new `read_adapter_snapshot`; snapshot round-trip + read-time-projection tests; adapter-toml-is-a-directory test now expects `ManifestUnreadable` — engine
- `crates/ktesio-engine/src/adapter/mod.rs` — `enforce_registration_invariants` uses `has_any_support` (F1); `From<AdapterResolveError>` routes `ManifestUnreadable` to its own variant (F4); new invariant tests — engine
- `crates/ktesio-engine/src/adapter/builtin.rs` — reframed the literal shape test (renamed `builtin_mock_declares_the_ad4_exemplar_shape`), pointing at the real cross-boundary guard (F2) — engine
- `crates/ktesio-engine/tests/registration.rs` — `conformance_mock_fixture_matches_builtin_shape` now asserts builtin==conformance declaration equality across `Capability::ALL × OsId::MODELED` (F2) — engine
- `crates/kt/src/error.rs` — new `AgentManifestUnreadable` diagnostic (`ktesio::agent::manifest_unreadable`) (F4) — kt
- `crates/kt/src/cli/agent.rs` — `map_error` arm for `ManifestUnreadable` + import + test (F4) — kt
- `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` — Consistency Conventions "Adapter kind charset" row (F6); AD-4 read-time-projection clause (F3) — spine
- `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/.memlog.md` — appended a `refinement` entry recording the F3/F6 spine additions (ratified by Islam 2026-07-04) — spine memlog

### Change Log

- 2026-07-04 — Story 1.3 implemented (manifest adapter). Added the Adapter Contract types + `adapter.toml` schema/validation to `ktesio-adapter-api` (first real code; `CONTRACT_VERSION` seed); the conformance `MockAdapter` + inert `ScriptedFakeAgent` fixtures; engine adapter resolution (native builtin + manifest loader/validator, executes nothing) integrated into registration as an atomic pre-step; new `RegistryError` variants; `kt agent register --kind|--manifest` + `kt agent show`, rendering the effective per-OS Capability Declaration. New dep `toml 1.1.2` (adapter-api only). All local gates green: fmt, clippy (-D warnings), 513 tests, tarpaulin 96.22% (≥95), check_docs, test_automation; boundary + OS-cfg + MSRV gates green; conformance mock confirmed dev-only.
- 2026-07-04 — Addressed code review findings (Status stays `review`) — 7 items resolved: F1 empty/all-unsupported capability rejection (`has_any_support`, enforced in both manifest validate and engine invariants); F2 real cross-boundary BuiltinMock==MockAdapter declaration-equality guard; F3 adapter snapshot persists the full per-OS declaration and projects onto the running OS at read time (portable state dir); F4 `ManifestUnreadable` own error variant + `kt` message; F5 `contract_version` parsed as semver; F6 adapter `kind` charset-validated (`^[a-z0-9][a-z0-9_-]*$`); F8 invalid `[metering]` value now names the section. Plus two Islam-approved spine refinements (F6 conventions row, F3 AD-4 read-time-projection clause) + a spine memlog note. `semver` promoted to a normal adapter-api dep (already a workspace dep). Gates re-run green: fmt, clippy (-D warnings), 525 tests, tarpaulin 96.32% (≥95, +0.10%), check_docs (23 files), test_automation (19 tests); boundary + OS-cfg + MSRV (1.96.1) gates green; conformance still dev-only. F7/F9 + AI-2/AI-3/AI-4 untouched; issue #65 untouched; no commit.
