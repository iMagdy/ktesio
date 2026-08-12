---
baseline_commit: fbfd90fd6eb7f2ea2788d19e067469479e95e06a
epic: 1
story: 2
story_key: 1-2-register-and-remove-agent-instances-with-isolated-agent-homes
github_issue: 64   # exists; orchestrator syncs — do NOT edit from dev-story
---

# Story 1.2: Register and remove Agent Instances with isolated Agent Homes

Status: done

<!-- Note: Validation is optional. Run validate-create-story for a quality check before dev-story. -->

## Story

As an Operator,
I want to register an Agent Instance under a unique name and remove it cleanly,
so that each agent I manage has its own isolated Agent Home and the Fleet stays consistent. (FR-1, FR-2, FR-3)

## Acceptance Criteria

*(Verbatim from epics.md Story 1.2; AC numbering added for task traceability.)*

1. **Given** a fresh engine state (SQLite store introduced here per AD-6: rusqlite bundled, WAL; exact version pinned and recorded per the spine's verification note) **when** I register an Agent Instance with a unique name **then** an Agent Home is created with instance config and an empty Usage Ledger, the instance enters Lifecycle State `registered`, and all paths are computed only by the engine (path-authority convention).
2. **And** registering a duplicate name fails with a diagnostic naming the conflict and a remediation hint.
3. **Given** two Agent Instances of the same Agent kind **when** both are registered **then** their Agent Homes are disjoint and independently configured.
4. **Given** a registered (not running) Agent Instance **when** I remove it choosing retain or delete **then** retain leaves the Agent Home intact on disk, delete removes it, and every other Agent Home is byte-identical afterward (FR-2 isolation).
5. **And** removing a `running` instance requires stop-first or an explicit `--force` acknowledgment.

### Acceptance criteria — engineering interpretation (binding for dev)

- **AC1 fresh state:** "fresh engine state" = no SQLite DB file yet. First engine call must create the state dir, create + migrate the DB (schema v1), and be idempotent on a second run (existing DB opens cleanly, no re-migration side effects).
- **AC1 empty Usage Ledger:** the `usage_events` table exists and returns zero rows for the new instance. The Usage Ledger is **table rows scoped by `instance_id`**, not a file — "empty ledger" is a query result, not a created file (AD-6: ledger is DB-resident; only bulky artifacts are files).
- **AC1 path authority:** `kt` passes only a `name` (and later, adapter identity) to the engine. `kt` MUST NOT compute, join, or pass any Agent Home / state-dir path. The engine returns the created Agent Home path for display. Grep-auditable: no `agent_home`/`state_dir` path construction in `crates/kt/`.
- **AC2 duplicate:** attempted duplicate registration performs **no partial writes** — no DB row, no Agent Home directory left behind (transactional; see Dev Notes "Atomicity"). Diagnostic names the conflicting instance name and gives a remediation hint (e.g. "choose a different name or remove the existing instance with `kt agent remove <name>`").
- **AC3 disjoint homes:** two instances of the same kind get **distinct** Agent Home directories (keyed by instance name, which is unique per Fleet). Writing instance config into one home leaves the other's config file byte-unchanged.
- **AC4 retain/delete + isolation:** the retain-or-delete choice is an explicit caller decision surfaced through both the engine API and `kt`. Removal always deletes the DB row; `delete` additionally removes the Agent Home directory tree, `retain` leaves it on disk. **Isolation proof:** capture a checksum of a second instance's Agent Home before removing the first; assert byte-identical after.
- **AC5 running guard — SCOPE BOUNDARY:** nothing can actually `start`/run until Story 1.4 (tokio supervision core). Therefore this story implements the running-instance guard as **state-machine validation only**: `remove` on an instance whose Lifecycle State is `running` returns a distinct, typed error unless `--force` is set. Because a real `running` instance cannot be produced yet, this path is proven by **directly seeding an instance row in `running` state via the StateStore in tests** and asserting the guard fires (and that `--force` bypasses it). Real running-instance teardown (actually stopping the process before removal) lands in Story 1.4/1.6. State this boundary in the code comment on the guard.

## Tasks / Subtasks

- [x] **Task 1 — Adopt rusqlite + a cross-platform dirs crate in the workspace (AC: 1)**
  - [x] Add to root `[workspace.dependencies]`: `rusqlite = { version = "0.40.1", features = ["bundled"] }` and `directories = "6"` (see Dev Notes "Stack pins" for the exact verified versions and the cfg-free rationale). Record actual resolved pins in the Dev Agent Record (spine stack-verification note).
  - [x] Reference both from `crates/ktesio-engine/Cargo.toml` with `workspace = true`. Do NOT add them to `crates/kt/Cargo.toml` — `kt` never touches SQLite or path resolution (path-authority + AD-2).
  - [x] Do NOT add `tokio` (AD-13 is Story 1.4's job — see "Sync-vs-async decision"). Do NOT add `miette` to the engine (conventions: `thiserror` only in the lib).
  - [x] `cargo build -p ktesio-engine` compiles the bundled SQLite (first build is slow — this is expected; note it in the Debug Log).
- [x] **Task 2 — Domain types in `ktesio-engine::domain` (AC: 1, 3, 5)**
  - [x] Create `crates/ktesio-engine/src/domain/mod.rs` and submodules. This is the FIRST real engine code — create modules only as needed (entity-timing principle; 1-1 deliberately left the tree empty).
  - [x] `LifecycleState` enum with the ratified variants `registered starting running paused stopping stopped failed` (spine AD-15 / PRD Glossary). Only `registered` is *reachable* this story; the others exist as data so the transition table and the `remove` guard can name them. Derive serde + `Display` (snake_case wire form). Do NOT build the full transition table here — that's Story 1.4 (AD-15). A minimal "is this state removable without --force?" predicate is in scope.
  - [x] `AgentInstance` domain struct: at minimum `{ name: InstanceName, kind: String, state: LifecycleState, created_at: <RFC3339 UTC> }`. Use the exact Glossary term `AgentInstance` in code (conventions).
  - [x] `InstanceName` newtype validating `^[a-z0-9][a-z0-9_-]*$` (spine "IDs & time"). Invalid names are rejected at construction with a typed error naming the rule.
  - [x] `thiserror` error enum for the domain/registry (e.g. `RegistryError`) with variants: `DuplicateName { name }`, `InvalidName { name, .. }`, `NotFound { name }`, `RunningRequiresForce { name }`, `Store(#[from] StoreError)`, `Io(..)`. Each variant carries enough for `kt` to render a remediation hint. NO `miette` here.
- [x] **Task 3 — `StateStore` port + path authority (AC: 1, 3)**
  - [x] Define the `StateStore` **port** trait in `crates/ktesio-engine/src/ports/mod.rs` (`ports/state_store.rs`). Keep it a hexagonal port (AD-1): trait methods speak in domain types, not SQL. Minimum surface for this story: `create_instance(&AgentInstance) -> Result<(), StoreError>` (fails on duplicate name), `get_instance(&InstanceName) -> Result<Option<AgentInstance>, _>`, `list_instances() -> Result<Vec<AgentInstance>, _>`, `delete_instance(&InstanceName) -> Result<(), _>`, and a ledger read `count_usage_events(&InstanceName) -> Result<u64, _>` (used to prove "empty ledger"). See "Sync-vs-async decision" — these are **synchronous** this story.
  - [x] Path authority lives in the engine and ONLY the engine (spine "Filesystem layout" convention). Add a path helper (e.g. `ktesio-engine::paths`) that computes: the **state dir** (holds the SQLite DB) and each **Agent Home** (`<state_dir>/agents/<instance_name>/` — `[ASSUMPTION]`, tag it). Resolve the base **cfg-free** via the `directories` crate (`ProjectDirs::from("", "", "ktesio")` → its data dir). Expose an **injectable base dir** override (constructor param / builder) that defaults to the `directories` result — tests pass a `TempDir`; production passes `None`→default. See "Path authority — the cfg-free rule" in Dev Notes: the OS-cfg CI gate WILL fail the build if you hand-roll `#[cfg(unix/windows/target_os)]` here.
- [x] **Task 4 — SQLite `StateStore` impl + schema/migrations in `ktesio-engine::store` (AC: 1, 2, 3)**
  - [x] Create `crates/ktesio-engine/src/store/mod.rs` (`store/sqlite.rs`) implementing `StateStore` over rusqlite. Open with the AD-6 pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`. Bundled SQLite (Task 1) means no system SQLite dependency.
  - [x] Schema v1 (see "SQLite schema (this story)" in Dev Notes for the exact DDL). Two tables: `agent_instances` (registry + lifecycle) and `usage_events` (append-only ledger, created empty, shape frozen per AD-6/AD-7 so Epic 3 extends without a breaking migration). A `schema_version` mechanism (either `PRAGMA user_version` or a `_meta` table — pick one, document it) drives idempotent migration on open.
  - [x] `create_instance` inserts the row inside a transaction; the `UNIQUE` constraint on `agent_instances.name` is what enforces duplicate rejection — map the SQLite constraint violation to `StoreError::DuplicateName` (do not pre-check-then-insert; that races). Registration as a whole is atomic across DB + filesystem (see "Atomicity").
  - [x] Unit-test the store directly against a temp-dir DB: create/get/list/delete, duplicate rejection, empty-ledger count == 0, WAL pragma actually set, migration idempotent on reopen.
- [x] **Task 5 — Registry service (register / remove) wiring domain + store + Agent Home (AC: 1, 2, 3, 4, 5)**
  - [x] A registry service in `ktesio-engine::domain` (e.g. `domain::registry`) exposes `register(name, kind) -> Result<AgentInstance, RegistryError>` and `remove(name, RemoveDisposition, force: bool) -> Result<(), RegistryError>`. This is engine public API (re-exported at crate root) — it IS part of the Embedding Interface (AD-2).
  - [x] `register`: validate name → create Agent Home dir (engine-computed path) → write the initial instance config file into the home → insert the DB row → return the instance. On ANY failure, roll back cleanly (no half-created home, no orphan row — "Atomicity").
  - [x] Instance config file: write a minimal TOML `config.toml` into the Agent Home (AD-9: TOML at every layer; instance layer). Full layered resolution is Epic 2 — here just persist the instance-level file so "created with instance config" (FR-1/AC1) holds. `[ASSUMPTION]` the filename `config.toml`; tag it.
  - [x] `RemoveDisposition` enum `{ Retain, Delete }` (name it from the AC's "retain or delete"). `remove`: look up instance → if state is `running` and `!force` → `RunningRequiresForce` → else delete DB row (transaction) and, if `Delete`, remove the Agent Home tree. `Retain` leaves the home. Comment the running-guard SCOPE BOUNDARY (AC5 interpretation).
  - [x] The engine's public surface for this story = the registry service + the returned domain types + the path it reports. Everything else (store, ports, paths) stays `pub(crate)` or crate-internal per AD-1/AD-2. Verify `kt` can drive register/remove using ONLY re-exported public items.
- [x] **Task 6 — `kt agent register | remove | list` CLI commands (AC: 1, 2, 4, 5; CLI-first gate)**
  - [x] `[ASSUMPTION]` CLI verbs: `kt agent register <name> --kind <kind>`, `kt agent remove <name> [--delete|--retain] [--force]`, `kt agent list`. The spine only *suggests* `kt agent register|remove|list` (Capability→Architecture map, FR-1..4); tag the exact flag shape as an assumption. CLI-first gate: every capability MUST be reachable via `kt` (register, remove-with-disposition, running-guard/`--force`, and a list to observe results).
  - [x] Add an `Agent` subcommand group in `crates/kt/src/main.rs` (clap `Subcommand`, mirrors the existing nested `Publish` pattern) dispatching to a new `crates/kt/src/cli/agent.rs` module (add `pub mod agent;` to `crates/kt/src/cli/mod.rs`).
  - [x] `agent.rs` calls the engine's **synchronous** registry API directly (no blocking facade yet — see "Sync-vs-async decision"). It translates `RegistryError` → `miette` diagnostics with remediation (miette lives in `kt` ONLY — conventions; extend `crates/kt/src/error.rs` with new `#[derive(Error, Diagnostic)]` structs following the existing `ManifestDuplicateName` pattern, codes like `ktesio::agent::duplicate_name`, `ktesio::agent::running_requires_force`).
  - [x] `--delete`/`--retain`: make the disposition explicit. `[ASSUMPTION]` if neither is given, either require the choice (error asking for one) or default to `retain` (safer — never destroys data silently). Recommend **default retain**; tag it. `--force` only relevant with a `running` instance.
  - [x] Output discipline (AD-12): command results (created home path, the Fleet list) to **stdout**; diagnostics/notices to **stderr**. Reuse `crates/kt/src/ui.rs` patterns for rendering. A `--json` flag on `agent list` is **out of scope** here (Fleet visibility with `--json` is FR-4 / Story 1.7) — a plain human table suffices; note the deferral.
- [x] **Task 7 — Tests: engine unit + integration, kt integration (AC: all; coverage ≥95%)**
  - [x] Engine unit tests beside modules (conventions "Unit tests beside modules") covering: name validation (valid/invalid boundary cases), `LifecycleState` serde round-trip, store CRUD + duplicate + empty-ledger + WAL + migration idempotency, path helper disjointness (two names → two distinct homes; injected base dir honored), register happy path (home created + config file written + row present + ledger empty), register duplicate leaves no partial state, remove retain vs delete (home present/absent), remove isolation (other home byte-identical), remove running-without-force rejected + with-force accepted (seed `running` via store).
  - [x] Engine integration test in `crates/ktesio-engine/tests/` (conventions "integration tests per crate") driving register→list→remove through the PUBLIC API only against a temp base dir — this doubles as the AD-2 "public API is sufficient" proof for these capabilities.
  - [x] `kt` integration test in `crates/kt/tests/` using the `CARGO_BIN_EXE_kt` + `TempDir` helper pattern (`crates/kt/tests/helpers/mod.rs`). MUST set the state-dir base to the temp dir so tests never touch the real user data dir — see "Test isolation of the state dir" (add an env override like `KTESIO_STATE_DIR`, honored by the engine's path helper, mirroring the existing `KTESIO_NO_UPDATE_CHECK`/`XDG_CACHE_HOME` overrides). Cover: register prints the home path + exits 0; duplicate exits non-zero with the diagnostic; remove --delete removes the home; remove of a (seeded) running instance without --force exits non-zero, with --force exits 0.
  - [x] **Coverage reasoning (NFR-3, non-negotiable):** the workspace gate is `cargo tarpaulin --workspace --fail-under 95`. Story 1-1 measured 95.93% with the engine contributing ZERO coverable lines. This story adds the engine's first substantial code, so every engine branch (error variants, both remove dispositions, the running guard both ways, migration-on-reopen, name-validation failure paths) must be exercised or the workspace number drops below 95. Budget unit tests to hit each `RegistryError`/`StoreError` variant. Do NOT rely on `kt` integration tests for engine coverage — tarpaulin attributes them unevenly; cover engine logic with engine unit tests.
- [x] **Task 8 — Local gates + docs currency (AC: all; NFR-3, NFR-7)**
  - [x] Run the full local gate set (same set Story 1-1 used): `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --all-targets`; `cargo tarpaulin --workspace --fail-under 95`; `python3 scripts/check_docs.py`; `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py`.
  - [x] Confirm the CI **OS-cfg gate** stays green: `grep -rn --include='*.rs' -E 'cfg[!(]?.*(unix|windows|target_os|target_family)' crates/` must show ONLY the pre-existing allowlisted hits (backends dir — empty here — and the two grandfathered kt self-update files). If your path code triggers it, you used `#[cfg]` where you must instead use the `directories` crate. Confirm the **boundary gate** stays green: `cargo tree -p ktesio -e normal,build --all-features` shows only `ktesio-engine`/`ktesio-adapter-api` internal edges (do not add an engine→adapter edge use here; adapters arrive 1.3).
  - [x] Docs currency (NFR-7 / `check_docs.py`): if `docs/architecture.md` or `docs/testing.md` describe engine module layout, add the new `domain`/`ports`/`store`/`paths` modules and the SQLite state store. Do not touch AI-1/AI-2 items (below).
  - [x] Manual smoke: `cargo run -p ktesio -- agent register demo --kind mock` (prints a home path under a temp/overridden dir), `kt agent list`, `kt agent remove demo --delete`.

## Dev Notes

**This file is the dev agent's ONLY guide. The sections below fix the decisions the AC leave open. Where a choice is genuinely open it is tagged `[ASSUMPTION]` (conservative default chosen) or `[OPEN QUESTION]` (surface to Islam; do not block).**

### Architecture bindings (spine, FINAL, binding)

- **AD-6 (activated here) — SQLite is the one state store.** rusqlite, **bundled** SQLite, WAL, `synchronous=NORMAL`. All registry + lifecycle state in ONE DB under the engine state dir. Usage Ledger = append-only `usage_events` + rollups; bulky artifacts (logs, memory, skills, effective-config snapshots) are FILES in the Agent Home, never DB blobs. Durability bound ratified ≤1s (one txn per usage event) — no usage events are written this story, but the table shape must be right so Epic 3 (Story 3.1 owns `usage_events` population) extends it without a breaking migration. [Source: ARCHITECTURE-SPINE.md#AD-6; .memlog.md AD-6 decision line: "bundled sqlcipher OFF, bundled sqlite ON … WAL … synchronous=NORMAL … usage_events append-only + aggregates".]
- **AD-15 — lifecycle state machine is data.** The state set is ratified: `registered starting running paused stopping stopped failed`. Only `registered` is reachable this story; do NOT implement the full transition table (Story 1.4). Define the enum as data so `remove`'s running-guard can name `running`. [Source: ARCHITECTURE-SPINE.md#AD-15.]
- **AD-1 — hexagonal core.** `ktesio-engine::domain` has NO dependency on adapters, OS-conditional code, or terminal/UX crates. `StateStore` is a PORT (`ports::state_store`); the SQLite impl (`store::sqlite`) sits behind it. Domain code speaks domain types; SQL stays in `store`. [Source: ARCHITECTURE-SPINE.md#AD-1, #Structural Seed.]
- **AD-2 — crate law + public API.** Implementation lands in `ktesio-engine`; `kt` gets thin commands that call the engine's PUBLIC API only (+ `ktesio-adapter-api` types, unused this story). The registry service + returned domain types are the public surface; ports/store/paths stay internal. Never engine→kt, never engine→concrete adapter. [Source: ARCHITECTURE-SPINE.md#AD-2.]
- **Path authority convention.** "The engine is the sole path authority: state-dir location and Agent Home layout are computed only inside the engine; `kt`, adapters, and Hosts receive paths from the API and never construct them." [Source: ARCHITECTURE-SPINE.md#Consistency Conventions → Filesystem layout.]
- **Naming / IDs / errors conventions.** Glossary terms verbatim in code (`AgentInstance`, `AgentHome`, `UsageLedger`, `Fleet`, `LifecycleState`); instance names `^[a-z0-9][a-z0-9_-]*$`, unique per Fleet; timestamps RFC 3339 UTC; `thiserror` in engine, `miette` wraps in `kt` with remediation; every partial failure names instance + reason + remediation (NFR-1). [Source: ARCHITECTURE-SPINE.md#Consistency Conventions.]

### Sync-vs-async decision (CRITICAL — read before coding) `[ASSUMPTION]`

**This story's engine public API is SYNCHRONOUS. Do NOT pull in tokio.**

- AD-13 mandates an async-first tokio engine with a `blocking()` facade for `kt` — but **AD-13 adoption is Story 1.4's explicit job** (epics: "tokio 1.4"; sprint-status story `1-4-...`). Registration/removal are filesystem + SQLite operations with no supervision, no process I/O, no concurrency need. Introducing tokio now would be speculative and violates the entity-timing principle Story 1-1 established.
- **Decision:** expose the registry service and `StateStore` port as plain synchronous methods. `kt` calls them directly (no facade yet).
- **Forward contract for Story 1.4 (state this so 1-4 does not miss it):** when tokio lands, Story 1.4 will make the engine internals async and add the `blocking()` facade. The sync registry methods created here should then be reachable through that facade (either kept sync behind it, or wrapped). Design the registry API so it is *facade-friendly*: no hidden global state, no thread-locals, takes its base dir/handle explicitly (this also satisfies FR-34 "no global-state collisions"). Do NOT design an API shape that assumes a running tokio runtime.
- rusqlite itself is a synchronous C-binding library — it does NOT require an async runtime, which is further reason sync is the correct altitude for the state store this story. When the engine goes async (1.4), blocking DB calls belong on a blocking pool; that is 1.4's concern, not yours.

### Path authority — the cfg-free rule (CRITICAL) `[ASSUMPTION on layout]`

The OS-cfg CI gate (`crates/kt` + `crates/ktesio-engine` scanned; regex `cfg[!(]?.*(unix|windows|target_os|target_family)`; allowed ONLY under `crates/ktesio-engine/src/backends/` plus two grandfathered legacy files) **will fail the build** if you resolve platform data dirs with hand-rolled `#[cfg(...)]` branches (the way the legacy `crates/kt/src/update_check.rs::user_cache_dir` does — that file is grandfathered; your new code is not).

- **Route (chosen):** use the `directories` crate (v6, verified). `ProjectDirs::from("", "", "ktesio")` yields platform-correct dirs (Linux XDG, macOS `~/Library/Application Support`, Windows `%APPDATA%`) with all `cfg` hidden INSIDE the crate — your engine code stays cfg-free and the gate stays green. Use its `data_dir()` (or `data_local_dir()`) as the default state-dir base. `[ASSUMPTION: data_dir() over data_local_dir(); either is defensible — data_dir (roaming on Windows) chosen for portability. Tag and move on.]`
- **Injectable base dir:** the path helper/registry constructor takes an optional base dir. `Some(path)` → use it (tests pass a `TempDir`); `None` → default via `directories`. This keeps tests hermetic AND satisfies path authority (engine still computes the full Agent Home layout from the base).
- **Agent Home layout `[ASSUMPTION]`:** `<state_base>/agents/<instance_name>/` with `config.toml` inside; the SQLite DB at `<state_base>/state.db` (or `ktesio.db` — pick one, document). These exact names are not spine-fixed; choose sensible ones and tag them. The invariant that IS fixed: only the engine constructs them.
- `backends/` is NOT needed this story (pure paths + SQLite, no process/OS syscalls). Do not create a `backends` module. If you ever feel the urge to `#[cfg]`, stop — reach for `directories` instead.

### SQLite schema (this story) — schema v1

Create exactly two tables. `agent_instances` is the registry+lifecycle store this story populates; `usage_events` is created **empty** with its shape frozen per AD-6/AD-7 so Story 3.1 (which owns writing usage events) extends it additively. Reference DDL (adjust types/naming to taste but keep the columns and constraints):

```sql
-- Registry + lifecycle (this story writes/reads this)
CREATE TABLE agent_instances (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,          -- Fleet-unique; ^[a-z0-9][a-z0-9_-]*$ (enforced in domain)
    kind         TEXT NOT NULL,                 -- Agent kind (adapter identity comes in 1.3)
    state        TEXT NOT NULL,                 -- LifecycleState wire form; 'registered' this story
    agent_home   TEXT NOT NULL,                 -- engine-computed absolute path (path authority)
    created_at   TEXT NOT NULL,                 -- RFC 3339 UTC
    updated_at   TEXT NOT NULL                  -- RFC 3339 UTC
);

-- Usage Ledger: append-only, created EMPTY this story; populated in Epic 3 (Story 3.1).
-- Column shape per AD-7 minimum UsageEvent fields {instance id, run id, input tokens,
-- output tokens, metering source, timestamp} so 3.1 needs no breaking migration.
CREATE TABLE usage_events (
    id             INTEGER PRIMARY KEY,
    instance_id    INTEGER NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
    run_id         TEXT NOT NULL,               -- Run = starting→terminal span (AD-7); unused until lifecycle exists
    input_tokens   INTEGER NOT NULL,
    output_tokens  INTEGER NOT NULL,
    metering_source TEXT NOT NULL,              -- self-reported | engine-observed
    occurred_at    TEXT NOT NULL                -- RFC 3339 UTC
);
CREATE INDEX idx_usage_events_instance ON usage_events(instance_id);
```

- **`ON DELETE CASCADE` + `PRAGMA foreign_keys=ON`:** removing an instance cleans up any ledger rows (none this story, but it makes remove correct forever). Enable `foreign_keys` on every connection open (it is OFF by default in SQLite).
- **Migrations approach `[ASSUMPTION]`:** single embedded schema-v1 applied when the DB is new; gate with `PRAGMA user_version` (0 → apply v1 → set to 1). A future story adds v2 by checking `user_version` and stepping. Keep it dead-simple (no migration framework dependency this story). Document whichever mechanism you pick. Idempotency on reopen is an AC-tested requirement.
- **"Rollup aggregates" (AD-6):** NOT needed this story — no usage to roll up. Do not create aggregate tables speculatively; Story 3.1 introduces them when it needs them (entity-timing).

### Atomicity — registration & removal must not leave partial state (AC2, AC4)

Registration touches two media: the filesystem (Agent Home dir + config file) and the DB (the row). A crash or duplicate-name collision must not leave a half-created instance.

- **Recommended order + rollback:** (1) validate name; (2) `INSERT` the row in a DB transaction but do NOT commit yet — if the `UNIQUE` constraint fires, you learn about the duplicate before creating any files → return `DuplicateName`, nothing on disk; (3) create the Agent Home dir + write `config.toml`; (4) commit. If step 3 fails, the uncommitted transaction is dropped (no row) and you remove any partially created dir. This ordering makes duplicate detection filesystem-side-effect-free (satisfies AC2's "no partial writes"). `[ASSUMPTION: this ordering; an alternate "files first, row second, unlink files on row failure" also works — pick one, comment it, test the failure path.]`
- **Removal:** delete the row (transaction) first; then, for `Delete`, remove the dir tree. If dir removal fails after the row is gone, report a partial-failure diagnostic naming the instance + the leftover path + remediation (NFR-1) rather than silently succeeding.
- Test both rollback paths explicitly (they are prime coverage-holding branches, Task 7).

### Previous-story intelligence (Story 1.1 — `done`, commit `b76d9af` per task brief; baseline tag `fbfd90f`)

Carried facts you MUST respect (from `1-1-…md` Dev Agent Record + File List):

- **Workspace is live, 5 crates.** `ktesio-engine` etc. are currently **doc-only libs with `publish = false`** and a `TODO(story 7-4)` marker. This story adds the engine's FIRST real code. Keep `publish = false` (do not publish anything; that's Story 7.4).
- **`workspace.dependencies` hoisting pattern** (root `Cargo.toml`): add rusqlite + directories there once, reference with `{ workspace = true }` from the engine. Follow the existing block exactly (see the external-deps list).
- **`[workspace.lints]` + `lints.workspace = true`** on every member — the engine already sets it; new modules inherit `-D warnings` clippy. The `tarpaulin_include` check-cfg lint lives in `[workspace.lints.rust]`; do not re-add it.
- **CI gates now armed (Story 1.1):** boundary (allowlist: only `ktesio-engine`/`ktesio-adapter-api` internal edges), semver (dormant until publish), and the OS-cfg gate (exact allowlist above). **This story should need ZERO OS-conditional code** — keep it that way (use `directories`). Coverage gate is `cargo tarpaulin --workspace --fail-under 95`.
- **Error/UX conventions proven in 1.1:** `miette` is kt-only (it is NOT in any skeleton crate's deps — do not add it to the engine); `crates/kt/src/error.rs` holds `#[derive(Error, Diagnostic)]` structs with `#[diagnostic(code(ktesio::<area>::<name>))]` — copy that shape for the new agent errors. `crates/kt/src/ui.rs` holds terminal rendering; stdout = command output, stderr = diagnostics (AD-12).
- **Integration-test harness:** `crates/kt/tests/helpers/mod.rs` runs the bin via `env!("CARGO_BIN_EXE_kt")` inside a `TempDir`, sets `KTESIO_NO_UPDATE_CHECK=1`. Reuse it; add a state-dir env override so agent tests are hermetic.
- **Two OPEN sprint action items exist — DO NOT touch them here:** AI-1 (`ci.yml` semver-job cache path) and AI-2 (`scripts/test_automation.py` assertion tightening). They are folded into future workflow/automation-touching stories, not this one. Mentioned only so you don't "helpfully" fix them and expand scope.
- **Publish/release note:** `cargo package -p ktesio` is BLOCKED until 7-4 (engine unpublished). Irrelevant to dev-story but do not attempt any publish/release step.

### Stack pins (verified 2026-07-03 via crates.io API — spine stack-verification note)

| Crate | Pin | Notes / verification |
| --- | --- | --- |
| `rusqlite` | `0.40.1`, `features = ["bundled"]` | crates.io `max_stable_version = 0.40.1` (published 2026-06-06, not yanked, edition 2021). Bundles SQLite 3.53.2 via `libsqlite3-sys 0.38.1`. `bundled` compiles SQLite from source (first build slow; no system SQLite needed — good for Windows CI). AD-6: `sqlcipher` features OFF. |
| `directories` | `6` (`6.0.0`) | crates.io `max_stable_version = 6.0.0`. Cross-platform data-dir resolution with all `cfg` internal → keeps engine code cfg-free (OS-cfg gate). Alternatives verified same day: `dirs 6.0.0`, `etcetera 0.11.0` — `directories` chosen for its `ProjectDirs` app-scoped API. `[ASSUMPTION: directories over dirs/etcetera — swap is trivial if Islam prefers a lighter dep.]` |
| `tokio` | **NOT added this story** | AD-13 adoption is Story 1.4. |

Record the ACTUAL resolved versions from `Cargo.lock` in the Dev Agent Record after `cargo build` (spine: "record actual pins in that story").

### Project Structure Notes

New engine modules created this story (fills part of the spine Structural Seed that 1-1 intentionally left empty):

```text
crates/ktesio-engine/src/
  lib.rs            # re-export the registry service + public domain types (Embedding Interface surface)
  domain/           # AD-1 core: LifecycleState, AgentInstance, InstanceName, RegistryError, registry service
  ports/            # StateStore port trait (hexagonal)
  store/            # SQLite StateStore impl + schema v1 + migrations (AD-6)
  paths.rs          # engine-only path authority (state dir + Agent Home), cfg-free via `directories`
crates/ktesio-engine/tests/   # integration test: register→list→remove via public API
crates/kt/src/cli/agent.rs    # kt agent register|remove|list (thin; calls engine sync API; miette wrapping)
crates/kt/tests/              # kt integration test for agent commands (hermetic state dir)
```

- Do NOT create `backends/`, `metering/`, `skills/`, `events.rs` — those belong to later stories (entity-timing; matches how 1-1 deferred all modules).
- Variance from spine seed: spine shows `src/store/` for the SQLite impl and `src/ports/` for `StateStore` — this story realizes exactly those two, plus a `paths.rs` helper (the spine names path authority as a convention, not a fixed module; `paths.rs` is an `[ASSUMPTION]` on placement).

### Test isolation of the state dir (do this or tests pollute the real user data dir)

- The engine path helper must accept an override. For `kt` integration tests (which run the real binary), plumb an env var — recommend `KTESIO_STATE_DIR` — that the engine's path helper reads BEFORE falling back to `directories`. This mirrors the existing precedent (`update_check.rs` honors `XDG_CACHE_HOME`; the harness sets `KTESIO_NO_UPDATE_CHECK`). Without this, `cargo test` would create real `~/Library/Application Support/ktesio` / `%APPDATA%\ktesio` dirs on the dev's machine and in CI. `[ASSUMPTION: env var name KTESIO_STATE_DIR; tag it.]`
- Engine unit/integration tests take the base dir as a function/constructor arg (no env needed there) — cleaner and parallel-safe.

### Testing requirements (NFR-3 — coverage ≥95%, CI-enforced, non-negotiable)

- Layout (spine conventions): **unit tests beside modules** in the engine; **integration tests per crate** (`crates/ktesio-engine/tests/`, `crates/kt/tests/`).
- **Why coverage is at risk and how to hold it:** Story 1-1 sat at 95.93% with the engine at ZERO coverable lines. Adding engine logic without covering every branch will pull the workspace under 95. Enumerate and test every `RegistryError`/`StoreError` variant, both `RemoveDisposition` arms, the running-guard both ways (`--force` present/absent), migration-on-reopen, and each name-validation rejection. The duplicate-registration rollback path and the remove-dir-failure path are easy to leave uncovered — hit them.
- Cover engine logic with **engine** unit tests (tarpaulin attributes cross-crate integration coverage unevenly — do not lean on `kt` tests to cover engine branches).
- `kt` integration tests assert the CLI contract (exit codes, stdout home path, stderr diagnostics) via the `CARGO_BIN_EXE_kt` harness with `KTESIO_STATE_DIR` pointed at a `TempDir`.
- Determinism: RFC 3339 timestamps make row contents time-varying — assert on structure/among-fields, not exact timestamps, or inject a clock. `[ASSUMPTION: a fixed/injectable clock is nice-to-have, not required this story.]`

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.2] (ACs verbatim) and #Epic 1 context (FR-1..10 scope, dependency order 1→2→…).
- [Source: _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md#AD-6 (SQLite/WAL/usage_events), #AD-15 (state set), #AD-1 (hexagonal/StateStore port), #AD-2 (crate law/public API), #AD-13 (async is 1.4 — NOT now), #Consistency Conventions (path authority, naming, errors, IDs, testing, platform code), #Structural Seed, #Stack.]
- [Source: _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/.memlog.md — AD-6 decision line (bundled sqlite ON / sqlcipher OFF, WAL, synchronous=NORMAL, usage_events append-only, ≤1s durability).]
- [Source: _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md#3 Glossary (Agent Instance, Agent Home, Lifecycle State, Usage Ledger, Fleet, Adapter — exact terms), #FR-1, #FR-2, #FR-3.]
- [Source: _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/addendum.md#3 State store options (SQLite chosen), #5 Engine/CLI crate shape.]
- [Source: _bmad-output/implementation-artifacts/1-1-restructure-into-the-five-crate-workspace-without-breaking-the-shipping-cli.md — Dev Agent Record, Completion Notes, File List, Change Log (workspace facts, CI gates, error/UX conventions, harness, open AI-1/AI-2).]
- [Source: crates/ktesio-engine/{Cargo.toml,src/lib.rs} (doc-only skeleton), crates/kt/{Cargo.toml, src/main.rs, src/cli/mod.rs, src/error.rs, src/update_check.rs (grandfathered cfg example), tests/helpers/mod.rs}, root Cargo.toml (workspace.dependencies), .github/workflows/ci.yml:162-197 (OS-cfg gate exact pattern).]
- [Source: crates.io API 2026-07-03 — rusqlite 0.40.1, directories/dirs 6.0.0, etcetera 0.11.0, libsqlite3-sys 0.38.1.]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (Claude Opus 4.8), via Claude Code dev-story workflow.

### Debug Log References

- **Toolchain finding (important):** `rusqlite 0.40.1` pulls `libsqlite3-sys 0.38.1`, whose build script uses the `cfg_select!` macro. That macro is **unstable in Rust 1.94.1** (the machine's default toolchain) — `cargo build -p ktesio-engine` fails there with `error[E0658]: use of unstable library feature 'cfg_select'`. It compiles cleanly on **stable 1.96.1** (released 2026-06-26), which is what CI installs (`rustup toolchain install stable`). All local gates were therefore run with `cargo +stable ...`. No code change was needed — this is purely a minimum-Rust-version fact. First bundled-SQLite build compiles SQLite from source (~expected slow first build; cached after).
- **Bundled SQLite build:** `cargo +stable build -p ktesio-engine` compiled `libsqlite3-sys 0.38.1` (SQLite 3.53.x) from source, plus `directories 6.0.0` / `dirs-sys 0.5.0`. No system SQLite needed.
- **RFC3339 without a date crate:** no time crate is approved for this story, so `time.rs` formats "now" as RFC 3339 UTC via Howard Hinnant's `civil_from_days` algorithm over the Unix timestamp. Unit-tested against known epoch seconds (epoch, 1e9, a leap day, 2026-07-03).
- **OS-cfg gate near-miss (caught + fixed):** an early `paths.rs` doc comment literally contained the string `` `#[cfg(unix/windows/target_os)]` `` while *explaining* the cfg-free rule. The OS-cfg CI gate is a text grep (`cfg[!(]?.*(unix|windows|target_os|target_family)`) and does not exempt comments, so it flagged the line. Reworded the doc comment to avoid the token; re-simulated the gate → GREEN. No `#[cfg]` OS attributes exist anywhere in the new code (path resolution goes through `directories`).
- **Gate simulations:** locally simulated both the boundary gate (`cargo tree -p ktesio -e normal,build` → internal edges only `ktesio-engine`, `ktesio-adapter-api`) and the OS-cfg gate (only allowlisted hits) — both GREEN. `rusqlite` added to `kt` is a **dev-dependency only** (to seed a `running` row for the AC5 CLI test); `-e normal,build` excludes it, so the shipping `kt` crate still never depends on SQLite.

### Completion Notes List

**All 5 ACs implemented; all 8 tasks complete; all 6 local gates GREEN.**

- **AC1 (register + fresh state + empty ledger + path authority):** `Registry::open` creates the state dir and creates+migrates the SQLite DB (schema v1 via `PRAGMA user_version`; idempotent on reopen — tested). `register` yields an Agent Home with a `config.toml`, `LifecycleState::Registered`, and zero `usage_events` rows. `kt` passes only `name`+`kind`; the engine computes and returns the Agent Home path (grep-audited: no `agent_home`/`state_dir` construction in `crates/kt/`).
- **AC2 (duplicate → diagnostic + no partial writes):** the `UNIQUE` constraint on `agent_instances.name` detects duplicates atomically at INSERT, *before* any file is created (row-first ordering) → no partial writes. `kt` renders a miette diagnostic naming the conflict + remediation (`kt agent remove <name>`).
- **AC3 (disjoint homes):** homes are keyed by unique name (`<base>/agents/<name>/`); two same-kind instances get distinct, independently-configured homes — tested that writing into one leaves the other's config byte-unchanged.
- **AC4 (retain/delete + isolation):** `RemoveDisposition::{Retain,Delete}` surfaced through the engine API and `kt` (`--retain`/`--delete`, default retain). Row always deleted; `Delete` also removes the home tree. Isolation proven by asserting a second home is byte-identical after removing the first.
- **AC5 (running guard, SCOPE BOUNDARY):** implemented as state-machine validation only (`LifecycleState::is_removable_without_force`). Because no real `running` instance can exist until story 1.4, tests seed a `running` row directly — via the store in engine unit tests, and via a direct SQLite write in the `kt` integration test — and assert the guard fires without `--force` and is bypassed with it. Boundary documented in the `remove` doc comment.

**Gate results (run with `cargo +stable`, matching CI's stable toolchain):**
- `cargo fmt --all --check` → PASS (0 diffs).
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings).
- `cargo test --workspace --all-targets` → PASS, **423 tests, 0 failures** (engine lib 41, engine integration 3, kt lib 349, kt agent_cli 7, plus existing install/publish/adoption suites).
- `cargo tarpaulin --workspace --fail-under 95` → PASS, **95.98%** (2651/2762 lines), +0.43% vs story 1-1's 95.93%.
- `python3 scripts/check_docs.py` → PASS (23 markdown files validated).
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` → PASS (18 tests).
- Boundary CI gate + OS-cfg CI gate → simulated locally, both GREEN.

**Deviations from the story plan:**
1. **Atomicity ordering:** chose the "row first, files second, roll the row back on filesystem failure" variant over the story's primary "uncommitted-INSERT then files then commit" ordering. Reason: the `StateStore` port intentionally hides transactions (methods speak domain types, not SQL), so exposing uncommitted-transaction control through the port would leak SQL semantics and violate AD-1. The chosen variant is explicitly permitted by the story's Atomicity note ("an alternate 'files first/row second' also works — pick one, comment it, test the failure path") and keeps duplicate detection filesystem-side-effect-free (satisfying AC2). Commented in `register`'s doc; both rollback paths tested (dir-create failure and config-write failure).
2. **`serde_json` + `rusqlite` added to engine/kt as dev-dependencies** (not runtime): engine gets `serde_json` (dev) to prove the `LifecycleState` serde wire form matches the stored string; `kt` gets `rusqlite` (dev) to seed a `running` row for the AC5 CLI test. Neither affects the shipping crates or the boundary gate.
3. **Success confirmation to stdout, not stderr:** `kt agent register/remove` print their success confirmation via `ui::success` (stdout), consistent with every other `kt` command. AD-12's stderr is reserved for diagnostics/notices; a completed-successfully line is neither. The machine-parseable command result (the Agent Home path) is a separate stdout line.
4. **Removed the vestigial `use ktesio_engine as _;` line** from `crates/kt/src/main.rs` — real engine API usage now exists via `cli::agent`, so the anonymous import that formerly proved the edge is redundant.

**Assumptions tagged (from Dev Notes, all conservative defaults):**
- State-dir layout: DB at `<base>/state.db`; Agent Home at `<base>/agents/<name>/`; instance config `config.toml`. (Names not spine-fixed; the fixed invariant — only the engine constructs them — holds.)
- `KTESIO_STATE_DIR` env var name for test hermeticity (mirrors `KTESIO_NO_UPDATE_CHECK`).
- `directories::ProjectDirs::from("", "", "ktesio").data_dir()` for the default base (roaming on Windows).
- `PRAGMA user_version` for schema versioning (over a `_meta` table).
- CLI verbs `kt agent register <name> --kind <kind>` / `remove <name> [--delete|--retain] [--force]` / `list`; default disposition = **retain** (safer).
- `--json` on `agent list` deferred to FR-4 / story 1.7 (plain human table this story).
- Injectable clock deferred (RFC3339 asserts on structure, not exact timestamps).

**Open questions for Islam (non-blocking):**
1. **Minimum Rust version:** `rusqlite 0.40.1` → `libsqlite3-sys 0.38.1` requires a Rust with `cfg_select!` stabilized (builds on 1.96.1, fails on 1.94.1). CI's `stable` covers this today, but if a pinned/MSRV toolchain is ever introduced it must be ≥ the `cfg_select` stabilization release. Worth pinning a `rust-toolchain.toml` (or documenting MSRV) so the bundled-SQLite build can't silently break on an older toolchain. **RESOLVED in the fix pass below (MSRV rider): `rust-version = "1.96.1"` pinned at workspace level + a CI `msrv` job.**
2. **`data_dir()` vs `data_local_dir()`** for the Windows base — chose `data_dir()` (roaming) for portability; trivial to switch if you prefer non-roaming.
3. Dep swap latitude: `directories` vs the lighter `dirs`/`etcetera` — chose `directories` for its `ProjectDirs` app-scoped API; swap is trivial if you'd rather a leaner dep.

### Review Fix Pass (2026-07-03) — code-review remediation, Status stays `review`

Applied the approved code-review patch set (8 story fixes + MSRV rider F3 + AI-1). Scope strictly limited to the approved list; F4/F11/F12/AI-2 deliberately untouched. All fixes shipped with tests; the workspace coverage gate **rose to 96.24%** (from 95.98% at first review).

- **F1 (HIGH) — migration downgrade guard** (`store/sqlite.rs::migrate`): a DB whose `PRAGMA user_version` is ahead of `SCHEMA_VERSION` is now refused with the new `StoreError::SchemaTooNew { found, supported }` instead of skipping the DDL and stamping the version back down. The `user_version` bump moved inside the `version < SCHEMA_VERSION` up-migration path only. New test `newer_schema_db_is_refused_not_downgraded` seeds `user_version=2` and asserts the clean refusal + that the on-disk version is not downgraded. (Also satisfies the reviewer's schema-forward-compat request.)
- **F2 (HIGH) — register rollback captures the compensating delete** (`domain/registry.rs::register`): the rollback no longer discards the row-delete result. If `materialize_home` fails AND the compensating `delete_instance` also fails, it returns the new `RegistryError::RegisterOrphanRow { name, home_error, rollback_error }` naming the orphaned row + `--force` remediation (mirrors `RemoveLeftoverHome`). New test `register_orphan_row_when_rollback_delete_also_fails` injects the compound failure via a `#[cfg(test)]` `SqliteStore::break_deletes_for_test` (a `BEFORE DELETE` RAISE(ABORT) trigger) plus a file-at-agents-dir. `kt`'s `map_error` renders the new variant.
- **F5 (MED) — constraint classification** (`store/sqlite.rs::classify_insert`): now matches the *extended* result code (`SQLITE_CONSTRAINT_UNIQUE` / `SQLITE_CONSTRAINT_PRIMARYKEY`) before concluding `DuplicateName`; any other constraint violation falls through to `Backend`. New test `non_unique_constraint_maps_to_backend_not_duplicate` drives a `CHECK` violation and asserts `Backend`.
- **F6 (MED) — remove --delete TOCTOU** (`domain/registry.rs::remove`): dropped the `home.exists()` pre-check and now treats `io::ErrorKind::NotFound` from `remove_dir_all` as success (desired end-state already holds); only real failures surface as `RemoveLeftoverHome`. New test `remove_delete_succeeds_when_home_already_gone`.
- **F7 (LOW) — relative state dir** (`paths.rs::EnginePaths::new`): a non-absolute `KTESIO_STATE_DIR` is now rejected with the new `PathError::RelativeStateDir { value }` (the explicit `Some(base)` override remains trusted verbatim). New tests `relative_env_base_is_rejected` + `explicit_relative_override_is_trusted`.
- **F8 (LOW) — blank diagnostic path** (`domain/registry.rs::open`): the `EnginePaths::new` error map no longer hardcodes `path: String::new()`; it names the offending base (the supplied override, or a `<default via KTESIO_STATE_DIR>` marker). New test `open_maps_path_resolution_failure_and_names_the_base`.
- **F9 (LOW) — name length bound** (`domain/name.rs`): added `NameError::TooLong` with a documented `MAX_NAME_LEN = 128` cap, validated first in the constructor so an over-long name fails legibly here rather than opaquely at `create_dir_all`. New tests `accepts_name_at_max_length_and_rejects_over` + `too_long_is_checked_before_char_rules`.
- **F10 (LOW) — disposition fail-closed** (`kt/src/cli/agent.rs::DispositionArg::from_flags`): the `(true, true)` tie now fails **closed** to `Retain` (was `Delete`), so an ambiguous both-set input never silently destroys data (defense-in-depth; clap `conflicts_with` still blocks it at the CLI). Test split into `..._resolves_each_combination` + `..._fails_closed_to_retain_when_both_set`.
- **F3 (MSRV rider) — approved for this commit:** pinned `rust-version = "1.96.1"` in root `[workspace.package]`; every member inherits via `rust-version.workspace = true`. Added a CI `msrv` job (`rustup toolchain install 1.96.1 --profile minimal` → `cargo +1.96.1 check --workspace`) as a standalone gate (NOT added to `coverage.needs`, so the `test_automation` `needs:` assertion was left unchanged). Added `test_automation.py::test_ci_enforces_msrv_floor` locking the job + the `rust-version` pin. **Floor rationale:** empirically the machine's default 1.94.1 fails the bundled-SQLite build (`libsqlite3-sys 0.38.1` uses `cfg_select!`); stable 1.96.1 builds cleanly. 1.96.1 is the proven floor.
- **AI-1 (bundled) — semver-job binary cache** (`.github/workflows/ci.yml`): added a `cache` step keyed `${{ runner.os }}-cargo-semver-checks-bin` persisting `~/.cargo/bin`, so the armed gate stops source-installing `cargo-semver-checks` (~10 min) on every fresh runner. Marked AI-1 `done` in `sprint-status.yaml` action_items with a note. AI-2 left `open`.

**MSRV toolchain verifications (both required, both pass):**
- `cargo +1.94.1 check -p ktesio-engine` → **fails legibly** at resolution: `error: rustc 1.94.1 is not supported by the following packages: ktesio-adapter-api@0.1.0 requires rustc 1.96.1 / ktesio-engine@0.1.0 requires rustc 1.96.1` (the clean rust-version message, NOT the cryptic E0658 `cfg_select` build error).
- `cargo +1.96.1 check --workspace` → **passes** (all 5 crates check clean).

**Fix-pass gate results (run with `cargo +stable` = 1.96.1, matching CI):**
- `cargo fmt --all --check` → PASS (0 diffs).
- `cargo clippy --workspace --all-targets -- -D warnings` → PASS (0 warnings).
- `cargo test --workspace --all-targets` → PASS, **433 tests, 0 failures** (was 423; engine lib 41→50, kt lib 349→350; engine integration 3, kt agent_cli 7 unchanged).
- `cargo tarpaulin --workspace --fail-under 95` → PASS, **96.24%** (2685/2790 lines), +0.26% vs the 95.98% at first review.
- `python3 scripts/check_docs.py` → PASS (23 markdown files).
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` → PASS, **19 tests** (was 18; +`test_ci_enforces_msrv_floor`).
- Boundary CI gate + OS-cfg CI gate → re-simulated locally, both GREEN (internal edges exactly `ktesio-engine`/`ktesio-adapter-api`; OS-cfg hits only in the two grandfathered legacy files — the new fixes added zero `#[cfg]` and zero internal crate edges).

**Deliberately NOT touched (out of approved scope):** F4 (busy_timeout/concurrency — deferred to story 1-4; confirmed absent), F11 (list fail-closed — deliberate; `registry.list()` still surfaces store errors via `?`), F12 (pre-1970 epoch clamp — acceptable; `time.rs` unchanged), AI-2 (`test_automation.py` `-p ktesio` pin tightening — the two `assertIn` pins are unchanged). GitHub issue #64 not edited. Nothing committed.

### File List

**New — engine (`crates/ktesio-engine/`):**
- `src/domain/mod.rs` — domain module root; re-exports the public domain surface.
- `src/domain/lifecycle.rs` — `LifecycleState` enum (ratified set as data) + `is_removable_without_force` predicate + wire form.
- `src/domain/name.rs` — `InstanceName` newtype (`^[a-z0-9][a-z0-9_-]*$`) + `NameError`.
- `src/domain/instance.rs` — `AgentInstance` entity.
- `src/domain/error.rs` — `RegistryError` (thiserror; every variant carries remediation context).
- `src/domain/registry.rs` — `Registry` service (`open`/`register`/`remove`/`list`) + `RemoveDisposition`; atomicity/rollback + running-guard.
- `src/ports/mod.rs` — ports root + `StoreError`.
- `src/ports/state_store.rs` — `StateStore` port trait (synchronous).
- `src/store/mod.rs` — store root (internal).
- `src/store/sqlite.rs` — SQLite `StateStore` impl, schema v1 DDL, `PRAGMA user_version` migration, WAL/synchronous/foreign_keys pragmas.
- `src/paths.rs` — `EnginePaths` path authority (cfg-free via `directories`; injectable base + `KTESIO_STATE_DIR`).
- `src/time.rs` — RFC 3339 UTC formatting (no date-crate dependency).
- `tests/registration.rs` — integration test: register→list→remove via the public API only (AD-2 sufficiency proof).

**New — kt (`crates/kt/`):**
- `src/cli/agent.rs` — `kt agent register|remove|list` (thin; sync engine API; `RegistryError`→miette).
- `tests/agent_cli.rs` — kt integration tests (hermetic `KTESIO_STATE_DIR`; seeds a `running` row for the AC5 guard).

**Modified — engine:**
- `Cargo.toml` — added `rusqlite`/`directories`/`thiserror`/`serde` deps (workspace); `serde_json`+`tempfile` dev-deps.
- `src/lib.rs` — declared `domain`/`ports`/`store`(private)/`paths`/`time` modules; re-exported the registration public surface (Embedding Interface).

**Modified — kt:**
- `Cargo.toml` — added `rusqlite` **dev**-dependency (test-only running-row seeding).
- `src/main.rs` — `Agent` subcommand group + `AgentCommands` + dispatch + CLI tests; removed vestigial anonymous engine import.
- `src/cli/mod.rs` — `pub mod agent;`.
- `src/error.rs` — new `#[derive(Error, Diagnostic)]` structs: `AgentDuplicateName`, `AgentInvalidName`, `AgentNotFound`, `AgentRunningRequiresForce`, `AgentIo`, `AgentStore`.
- `tests/helpers/mod.rs` — added `run_kt_agent`/`KtRun` (state-dir-pinned runner returning exit status); `#[allow(dead_code)]` on the two pre-existing runner fns (each test binary compiles the helper independently).

**Modified — workspace / docs:**
- `Cargo.toml` (root) — hoisted `rusqlite = { version = "0.40.1", features = ["bundled"] }` and `directories = "6"` into `[workspace.dependencies]`.
- `Cargo.lock` — resolved: rusqlite 0.40.1, libsqlite3-sys 0.38.1, directories 6.0.0, dirs-sys 0.5.0, thiserror 2.0.18, serde 1.0.228 (+ transitive: hashlink 0.12.0, fallible-iterator 0.3.0, fallible-streaming-iterator 0.1.9, option-ext 0.2.0).
- `docs/architecture.md` — replaced the "empty skeleton" engine description with the live engine module layout + SQLite state store note (NFR-7 currency).

**New dependencies + resolved versions (approved defaults):**
| Crate | Requested | Resolved (Cargo.lock) |
| --- | --- | --- |
| `rusqlite` (features=`bundled`) | `0.40.1` | `0.40.1` |
| `libsqlite3-sys` (transitive) | — | `0.38.1` |
| `directories` | `6` | `6.0.0` |
| `dirs-sys` (transitive) | — | `0.5.0` |

**File List delta — Review Fix Pass (2026-07-03):**

*Modified — engine:*
- `src/ports/mod.rs` — added `StoreError::SchemaTooNew` (F1).
- `src/store/sqlite.rs` — `migrate()` downgrade guard (F1); `classify_insert()` extended-code matching (F5); `#[cfg(test)] break_deletes_for_test` fault injector; +2 store tests (F1, F5).
- `src/domain/error.rs` — added `RegistryError::RegisterOrphanRow` (F2).
- `src/domain/registry.rs` — `register()` rollback captures the compensating delete (F2); `remove()` NotFound-as-success TOCTOU fix (F6); `open()` populates the diagnostic path (F8); +4 registry tests (F2, F6, F8-open, F6-already-gone).
- `src/domain/name.rs` — `NameError::TooLong` + `MAX_NAME_LEN` cap + constructor check (F9); +2 name tests; extended `error_rule_text_is_specific`.
- `src/paths.rs` — `PathError::RelativeStateDir` + reject non-absolute env base (F7); +2 paths tests.

*Modified — kt:*
- `src/cli/agent.rs` — `DispositionArg::from_flags` fails closed to Retain on the both-set tie (F10); `map_error` renders `RegisterOrphanRow` (F2); split/extended disposition + map_error tests.

*Modified — workspace / CI / docs-automation:*
- `Cargo.toml` (root) — `rust-version = "1.96.1"` in `[workspace.package]` (F3/MSRV).
- `crates/{kt,ktesio-engine,ktesio-adapter-api,ktesio-adapters-hermes,ktesio-conformance}/Cargo.toml` — `rust-version.workspace = true` (F3/MSRV).
- `.github/workflows/ci.yml` — new `msrv` job (F3/MSRV); semver-job `~/.cargo/bin` binary cache (AI-1).
- `scripts/test_automation.py` — new `test_ci_enforces_msrv_floor` + one AI-1 cache-presence assertion.

*Modified — planning artifacts (`_bmad-output/`):*
- `implementation-artifacts/sprint-status.yaml` — AI-1 `open` → `done` (+note); AI-2 left `open`.
- `implementation-artifacts/1-2-…-isolated-agent-homes.md` — this fix-pass Dev Agent Record + File List delta + Change Log.

### Change Log

| Date | Change |
| --- | --- |
| 2026-07-03 | Implemented story 1.2: first real `ktesio-engine` code — SQLite `StateStore` (AD-6), hexagonal domain core + `StateStore` port (AD-1), engine path authority (cfg-free via `directories`), `Registry` service (register/remove/list) with atomic rollback and the AC5 running-guard, and thin `kt agent register|remove|list` commands (AD-2). Added rusqlite 0.40.1 (bundled) + directories 6 to the workspace. All 5 ACs met; all 6 local gates green (423 tests, 95.98% coverage). Status → review. |
| 2026-07-03 | Applied code-review fix pass (8 story fixes F1/F2/F5/F6/F7/F8/F9/F10 + MSRV rider F3 + AI-1). New errors: `StoreError::SchemaTooNew`, `RegistryError::RegisterOrphanRow`, `PathError::RelativeStateDir`, `NameError::TooLong`. Migration downgrade guard, extended-code constraint classification, remove-TOCTOU fix, disposition fail-closed-to-retain, blank-diagnostic-path fix, 128-char name cap. Pinned `rust-version = "1.96.1"` (workspace) + CI `msrv` job; added `~/.cargo/bin` cache to the semver job (AI-1 → done). +10 tests (433 total, 0 failures); coverage 95.98% → **96.24%**; all gates green; both MSRV toolchain checks verified (1.94.1 fails legibly, 1.96.1 passes). F4/F11/F12/AI-2 untouched. Status stays `review`. |

