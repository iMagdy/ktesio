---
baseline_commit: 0752d30a2f278c485d7a2f158a0f7c644b9f8251
baseline_ref: origin/main (PR #120 merged — "ci(coverage): fix the real root cause, a stale cached fake_agent (AI-67)")
---

# Story 5.1: Attach a managed filesystem Memory Backing

Status: done

<!-- Context engineered by create-story (headless BMAD run, 2026-07-30). Ground truth verified against `origin/main` @ 0752d30. -->

<!-- LINE-NUMBER CAVEAT: every `crates/**` line number below is an `origin/main` @ 0752d30 number.
     At authoring time the working tree sat on `fix/ai-63-drain-usage-incremental` (HEAD 67bd4a8),
     which adds ~+430 lines to `domain/supervisor.rs` (an AI-63(a) fix: bounded usage-tail read +
     a post-kill rescue drain) and ~+97 to `tests/lifecycle.rs`. If that branch is merged before
     dev starts, `supervisor.rs` numbers shift by roughly +112 from line 181 onward (e.g.
     `start_inner` 409 → 521). Trust the SYMBOL NAMES, not the numbers; re-grep before editing. -->

## Story

As an Operator,
I want to attach an engine-managed `filesystem` Memory Backing to an Agent Instance with one command,
so that agent memory persists under my control regardless of the agent's native memory story. (FR-15, FR-16 filesystem half)

## Acceptance Criteria

Verbatim from `_bmad-output/planning-artifacts/epics.md` lines 493–508 (Story 5.1); GitHub issue #82, epic #59.

**AC1 — attach creates the managed directory under path authority and the descriptor reaches the adapter at start**
**Given** a registered, not-running Agent Instance
**When** I attach a `filesystem` Memory Backing
**Then** the engine creates the managed directory inside the Agent Home (path authority) and hands the backing descriptor to the adapter at next start (AD-11)

**AC2 — adapter-kind parity**
**And** the same attach command sequence works identically on the mock adapter and a manifest adapter

**AC3 — no hot-swap**
**And** attach/detach on a `running` instance is rejected

**AC4 — byte-identical survival across stop/start AND engine restarts**
**Given** stop/start cycles and engine restarts
**When** the instance runs again
**Then** the managed directory's contents survive byte-identically

### Derived / consequence criteria (testable — from FR-15/FR-16, AD-11, and the current code state)

- **DC-1 (path authority, AD-2 + conventions row).** The managed directory path is computed ONLY inside `crates/ktesio-engine/src/paths.rs` (a new `MEMORY_DIR` const + an `EnginePaths` accessor). `kt` never constructs it and never joins `"memory"` itself; it receives the path from the engine's public API or prints nothing. No literal `"memory"` path segment appears in `crates/kt/**`.
- **DC-2 (persistence, AD-6).** The attachment is persisted in the ONE SQLite state DB as typed columns (never a serde-JSON blob), keyed by `instance_id` with `ON DELETE CASCADE`, behind an additive `SCHEMA_V5` migration and a `SCHEMA_VERSION` bump 4 → 5. An older DB opens and migrates in place; a newer DB still refuses via the existing `StoreError::SchemaTooNew` guard.
- **DC-3 (state guard is pure state-machine validation).** The attach/detach guard is decided from the persisted `LifecycleState` alone — no live process required — so it is deterministically testable via `Registry::seed_instance` exactly like `remove_running_without_force_is_rejected` (`registry.rs:1383`). There is **no `--force` escape** (unlike `remove`): AD-11 forbids hot-swap outright.
- **DC-4 (frozen exit-code contract, story 4-3 / PRD §7).** Every new failure mode maps into the **existing frozen 0–6 table** — no new numbers. Required mapping: rejected-because-running → **4** (invalid state, joining `AgentNotRunning`/`AgentRunningRequiresForce`/`AgentInvalidTransition`/`AgentStopUnconfirmed`); unknown/unsupported `--kind` token and malformed instance name → **2** (usage); unregistered instance → **3**; filesystem/DB failure → **1**. Each new `crate::error` diagnostic gets a `classify()` arm plus an assertion in `exit_code.rs`'s unit tests, and each new `RegistryError`/`EngineError` variant gets an arm in `every_engine_error_mapper_arm_preserves_its_documented_exit_code` / `registry_error_mapper_arms_preserve_their_documented_exit_codes` (`crates/kt/src/cli/agent.rs:2198` / `:2298`) — those two tests FAIL if a variant is added without a documented code.
- **DC-5 (no adapter-contract change).** `CONTRACT_VERSION` stays `"0.4.0"`; `ktesio-adapter-api::Manifest` gains no field and no `[memory]` section (see "Descriptor delivery" — decided mechanism is the existing AD-9 config seam). `Manifest` is `#[serde(deny_unknown_fields)]`, so adding a section would ALSO break forward-compat for older engines; that cost belongs to Epic 6, not here.
- **DC-6 (no frozen wire shape edited).** No `--json` document or content type frozen by story 4-3 (`FleetListing`, `FleetEntry`, `FleetTotals`, `UsageView`, `BudgetView`, `ShowDocument`, `UsageDocument`, `FleetUsageDocument`, `ConfigDocument`, `ConfigLeaf`, `LogLine`) changes in this story. The Memory Backing enters `--json` in story 5-2, together with `native`, as ONE intentional, announced key-set edit.
- **DC-7 (engine does not copy, seed, or migrate directory CONTENTS).** Byte-identical survival (AC4) is achieved by the engine never touching the contents — not by copy/restore logic. Creation is `create_dir_all` on one directory; there is no recursive walk, copy, or checksum of the tree anywhere on the start path. (See "The global-lock question" — this is a deliberate lock-safety constraint, not just simplicity.)
- **DC-8 (OS-cfg gate).** No `#[cfg(unix|windows|target_os|target_family)]` is added outside `crates/**/backends/`. Directory creation uses path-agnostic `std::fs`; tests branch on runtime `OsId::current()`, never on `cfg!`. Unix-style permission hardening (a 0600/0700-equivalent check, as the secrets file does) is **out of scope** — the mode READ lives in `backends/` by rule (`ports/secret_resolver.rs:31`), and no AC requires it.
- **DC-10 (delivery honesty — from the Q-1 ruling; spine AD-11 Delivery clause).** Delivery is *offered, not imposed*: the engine injects the path at the reserved key, and the adapter's declared `[config]` mapping decides whether the agent receives it. Because that is the adapter's choice, the engine must never be silent about it. At start, when a `filesystem` backing is attached and the resolved `ConfigMapping` declares **no** target for the reserved key, the engine emits one diagnostic notice (stderr per AD-12) naming the instance, the managed path, and the fact that the adapter declares no mapping for it — and the public backing read (Task 4.5) reports the same fact. The start still **succeeds**: the directory guarantee holds regardless, and refusing to start an otherwise-healthy agent because its adapter maps no memory key would be a regression (it is also exactly the guarantee level `native` legitimately ships). The check costs nothing: `mapping` is already resolved at `supervisor.rs:491` and `ConfigMapping::target(&str) -> Option<&ConfigTarget>` is a `BTreeMap` lookup — no new FS work, no new lock exposure (AD-17). Do **not** generalize this to other unmapped keys: story 2-2 Decision 6 ("an unmapped key is a silent no-op") stands; memory is special only because the operator took an explicit `attach` action and is owed the truth about its effect.
- **DC-9 (determinism).** No test sleeps to await state. Poll committed, observable state: the on-disk file/dir, the instance log, the DB via a public read, or `fake_agent --dump`'s written key/value lines. (Epic 2 retro AI-35/38; restated 4-1:156, 4-3.)

## Tasks / Subtasks

Dependency-ordered. Each task names its AC/DC. **Read "Exact code seams" and "Testing Notes" in Dev Notes before writing any code.**

- [x] **Task 1 — Path authority: the managed directory's one true path (AC1, DC-1, DC-8)**
  - [x] 1.1 In `crates/ktesio-engine/src/paths.rs`, add `pub const MEMORY_DIR: &str = "memory";` beside `EFFECTIVE_CONFIG_SNAPSHOT_FILE` (`:70`) and `pub fn agent_memory_dir(&self, name: &InstanceName) -> PathBuf` → `<agent_home>/memory`, mirroring `effective_config_snapshot` (`:177`) exactly. Do **not** hard-code `"memory"` anywhere else — note that `registry.rs:971`'s `instance_log_dir` inlines `join("logs")`; that is the inconsistency to NOT copy.
  - [x] 1.2 Update the (currently stale) Agent Home layout doc comment at `paths.rs:26-32` — it still lists only `state.db` + `agents/<name>/config.toml`. Record the real layout: `config.toml`, `adapter.json`, `effective-config.json`, rendered native config files, `logs/` (`instance.log`, `agent.log`, `agent-stderr.log`, `output.log[.1|.2]`, `breaches.log`), and the new `memory/`. (Epic-4 retro §hand-off explicitly asked Epic 5 to state the current layout rather than assume the pre-4-2 shape.)
  - [x] 1.3 Expose the path on the engine's public API only if a caller needs it (`kt`'s confirmation line does — see Task 5). Follow `Engine::agent_home` (`engine.rs:252-256`) as the shape; add the mirrored method to the `Blocking<'_>` facade (`engine.rs:948+`) in the same commit — a public engine method missing from the facade is an incomplete surface.

- [x] **Task 2 — The port seam + the descriptor type (AC1, DC-5)**
  - [x] 2.1 Create `crates/ktesio-engine/src/ports/memory_backing.rs` — the AD-11 seam whose arrival `ports/mod.rs:14-15` has been waiting for. It holds: `MemoryBackingKind` (a closed enum with the FR-16 vocabulary + `as_str`/`from_wire`, copying `LifecycleState`'s wire-string discipline at `lifecycle.rs:48/64`) and the descriptor type handed to the adapter. **Do NOT invent a trait tree, resolver, or registry** — AD-11's "richer backings are Deferred behind this port" means the port is the extension SEAM, not a polymorphism exercise; the `filesystem` implementation is pure path authority inside the engine. Add a trait only if the `filesystem`/`native` split genuinely needs dispatch, and say so in the Dev Agent Record if you do.
  - [x] 2.2 Wire the module into `ports/mod.rs` (`mod memory_backing;` + a `pub use`) and **update the module doc at `ports/mod.rs:14-15`** to drop `MemoryBacking` from the "remaining port … arrives with the story that needs it" list — stories 2-4 (`:170`) and 3-1 (`:132/:171`) both set this precedent when their ports landed.
  - [x] 2.3 Scope decision to honor: this story implements **`filesystem` only**. `native` is story 5-2. Ship the kind vocabulary such that 5-2 adds behavior without a breaking enum edit, and have the CLI's `--kind` value parser accept only what 5-1 implements (an unrecognized token → the `AgentUnknownKind`-shaped diagnostic → exit **2**, naming the accepted value). If a reserved-but-unimplemented variant trips `dead_code`, follow the documented `#[allow(dead_code)]` precedent of `ExitCode::Success` (`crates/kt/src/exit_code.rs`) — a one-line justification comment, not a silencing.

- [x] **Task 3 — Persist the attachment (AC1, AC3, AC4, DC-2)**
  - [x] 3.1 `crates/ktesio-engine/src/store/sqlite.rs`: add `const SCHEMA_V5: &str` beside `SCHEMA_V4` (`:124`) creating one table (working name `agent_memory_backing`) with `id INTEGER PRIMARY KEY`, `instance_id INTEGER NOT NULL UNIQUE REFERENCES agent_instances(id) ON DELETE CASCADE`, `kind TEXT NOT NULL`, `attached_at TEXT NOT NULL` (RFC-3339 UTC — conventions row). `agent_runtime` (`:81`) is the exact structural precedent (one row per instance, UNIQUE FK, cascade). Doc-comment it in the house style: name the story and assert additivity.
  - [x] 3.2 Bump `SCHEMA_VERSION` 4 → 5 (`:41`, extend its doc comment) and add `if version < 5 { conn.execute_batch(SCHEMA_V5)…}` to `migrate` (`:232`, after `:258`). Keep the step-up-one-at-a-time shape so a reopen is idempotent and a crash-interrupted migration re-runs only what remains. `PRAGMA foreign_keys=ON` is already set per-connection in `configure` (`:164`) — that is what makes the cascade real; assert the cascade in a test rather than assuming it.
  - [x] 3.3 Add the read/write/clear methods to the `StateStore` port (`ports/state_store.rs`) and `impl StateStore for SqliteStore`, then thin `pub(crate)` pass-throughs on `Registry` — copy `write_spawn_record` (`registry.rs:666`) / `clear_spawn_record` (`:673`) / `spawn_record` (`:679`) verbatim in shape. Use the existing `SqliteStore::instance_id` (`:186`) resolution and the existing `backend`/`classify_insert` error mappers (`:271`/`:298`) — do not add new SQLite error plumbing.
  - [x] 3.4 Migration test: open a state dir, seed it at v4 shape, reopen, assert `PRAGMA user_version == 5` and that pre-existing instances/usage rows survive. Also assert `SchemaTooNew` still triggers for a future version (the guard at `:237-242` must not regress).

- [x] **Task 4 — Engine API: `attach` / `detach` / read, with the running-guard (AC1, AC3, DC-1, DC-3, DC-4, DC-7)**
  - [x] 4.1 Add the attach operation to `Registry` (it owns both path authority and the DB — `registry.rs:253`). Ordering is non-negotiable and copies `remove`'s discipline (`registry.rs:528-547`): validate the name → `lookup` (→ `NotFound`) → **state guard** → only then any side effect. Side effects, in order: `ensure_dir(&self.paths.agent_memory_dir(name), name.as_str())` (reuse the existing helper at `registry.rs:1131`, which already maps to `RegistryError::Io { name, path, source }`) → persist the row. If the row write fails after the directory exists, leave the directory (it is inert and idempotent) and return the store error — do **not** invent a rollback that deletes operator data.
  - [x] 4.2 Add the detach operation: same validate → lookup → guard order; then clear the row **only**. **Detach does NOT delete the directory** (DC-7 + "Detach semantics" in Dev Notes). Detaching when nothing is attached is a successful no-op; re-attaching the same kind is an idempotent success. Attaching a *different* kind while one is attached is rejected (detach first) → exit **4**.
  - [x] 4.3 New error variant for the guard. Reuse nothing that lies: `RegistryError::RunningRequiresForce` (`error.rs:45`) says "or pass `--force`", which is FALSE here. Add a variant whose message names the instance, its current state, and the remediation ("stop it first — a Memory Backing cannot be hot-swapped"), following the `EngineError::NotRunning` (`error.rs:325`) doc pattern that explains *why* a non-transition op gets a dedicated pre-flight check instead of `InvalidTransition`. Note both `Registered` and `Stopped`/`Failed` are permitted states; only `Running` (and, decide explicitly, `Starting`/`Stopping`/`Paused` — recommend rejecting every non-terminal state, i.e. permit only `Registered`/`Stopped`/`Failed`, and say so in the message) is refused.
  - [x] 4.4 Add the public `Engine` methods (async) + their `Blocking<'_>` mirrors. `Engine::set_config` (`engine.rs:844-848`, registry-lock only) is the closest shape — attach/detach need the **registry lock only**, not the supervisor lock. Do not take the supervisor lock; taking it would widen the fleet-wide stall for no reason (see "The global-lock question").
  - [x] 4.5 Add a public read so the attachment is observable through the public API (AD-2: `crates/ktesio-engine/tests/*` may use nothing else — `tests/registration.rs:1-7`). Keep it minimal: the kind + the managed path, or `None` — **plus the DC-10 delivery fact** (whether the adapter's declared mapping targets the reserved key), since the Q-1 ruling makes "offered vs delivered" part of what an operator must be able to learn. This is also what story 5-2's status/effective-config surface will consume, so shape it for reuse, not for one call site.

- [x] **Task 5 — Hand the descriptor to the adapter at start (AC1, AC2, DC-5, DC-7)**
  - [x] 5.1 Add the reserved engine-namespace unified-config key (working name `memory.dir`) to `crates/ktesio-engine/src/domain/config.rs`: a `pub const` doc-commented in the exact style of `METERING_BASE_URL_KEY` (`:126`) — engine-computed, engine-injected, operator-does-NOT-set, known so a mapping can target it, **and explicitly "does not touch the Adapter Contract surface (no `CONTRACT_VERSION` bump)"** — plus an entry in `KNOWN_KEYS` (`:565-581`) with the same style of comment. This is the decided delivery mechanism; see "Descriptor delivery" in Dev Notes for why, and A-1 for what Islam still owns.
  - [x] 5.2 In `Supervisor::start_inner`, build the invocation-override `ConfigLayer` from the attached backing and fold it into the **`mapping_effective`** resolution — copy `base_url_override` (`supervisor.rs:2752-2763`; the call site is `:513-519`) exactly, including its nested-dotted-table construction. **Critical: follow the precedent's SPLIT.** The override goes only into the value handed to `resolve_secrets` (`:530`) and `apply_config_mapping` (`:532`); `write_effective_config_snapshot` (`:551`) keeps taking the **plain** `effective`. Read the CORRECTION block under "Descriptor delivery" before writing this — the story originally claimed the opposite and it is wrong. Place the read of the attached backing in the **pre-transition block** (before the persisted `→ starting` transition at `supervisor.rs:579`), preserving the documented invariant at `supervisor.rs:399-408`: every fallible step happens before any state change, so a failure rejects the start with no state change. The adapter's existing `[config]` mapping (story 2-2) then delivers the value into env/flag/file — zero new delivery code.
  - [x] 5.2a **DC-10 delivery notice.** In the same pre-transition block, when a `filesystem` backing is attached, check `mapping.target(MEMORY_DIR_KEY)`; on `None` emit one diagnostic notice (stderr, AD-12) naming the instance, the managed path, and that the adapter declares no mapping for the key. The start still succeeds. `mapping` is already in hand at `:491` — do not re-resolve it, and do not add any filesystem work here (AD-17).
  - [x] 5.3 Defensive directory presence at start: if a `filesystem` backing is attached, `create_dir_all` the managed dir (idempotent, one directory, no recursion) so a home whose `memory/` was manually deleted still starts. Mirror `ensure_log_dir` (`supervisor.rs:2110`), reuse its error shape, and keep it in the pre-transition block. **Nothing else** — no listing, no copying, no size accounting (DC-7).
  - [x] 5.4 Mock parity: add the `memory.dir` → env mapping to **BOTH** mocks in lockstep — `native_config_mapping` in `crates/ktesio-engine/src/adapter/builtin.rs:50` and `MockAdapter` in `crates/ktesio-conformance/src/lib.rs:52`. `conformance_mock_fixture_matches_builtin_shape` (`crates/ktesio-engine/tests/registration.rs:182-232`) asserts the two declare identical config mappings and FAILS if only one is updated. Follow `MOCK_MODEL_ENV_VAR` (`builtin.rs:31` / `conformance/src/lib.rs:38`) for the shared-constant pattern.
  - [x] 5.5 Manifest parity (AC2): the test fixture manifest declares `[config."memory.dir"] env = "<VAR>"` — **no `ktesio-adapter-api` change is required for this**, because `ConfigMapping` keys are arbitrary dotted strings today. Verify no manifest-schema edit sneaks in (DC-5).

- [x] **Task 6 — CLI surface: `kt agent memory attach|detach` (AC1, AC2, AC3, DC-1, DC-4, DC-6)**
  - [x] 6.1 Add a nested `Memory { #[command(subcommand)] command: MemoryCommands }` variant to `AgentCommands` (`crates/kt/src/main.rs:107+`), modeled exactly on `Config { … ConfigCommands }` (`:220` / `:229`-`:239`) — the repo's one precedent for a two-level `kt agent` group. `MemoryCommands::Attach { name, kind }` and `Detach { name }`. Wire both dispatch arms (`main.rs:312`).
  - [x] 6.2 Command bodies in `crates/kt/src/cli/agent.rs`, next to `config_set` (`:1399`) / `config_get` (`:1444`). Human output only in this story: a confirmation naming the instance, the kind, and the managed path (received from the engine — DC-1). AD-12: result on stdout, notes/diagnostics on stderr. **No `--json` flag in this story** (DC-6, A-3) — adding one would freeze a new document into the v1 surface before 5-2 decides the memory wire shape.
  - [x] 6.3 Validate the instance name FIRST via the existing `validate_instance_name` helper (`agent.rs`, added by 4-3's M2 fix) so a malformed name exits **2** uniformly, not **3**. This is a shipped, test-pinned convention — `a_malformed_instance_name_exits_with_the_usage_code_on_every_read_command` exists precisely because it was violated once.
  - [x] 6.4 New diagnostics in `crates/kt/src/error.rs` + arms in `map_error` (`agent.rs:1763`) / `map_engine_error` (`:1884`), + `classify()` arms in `crates/kt/src/exit_code.rs`, + its unit-test assertions, + the two mapper tests (`agent.rs:2198`/`:2298`). Update `exit_code.rs`'s module-doc table (it is the human-readable contract) — the numbers do not change, only the diagnostic lists.
  - [ ] 6.5 Optional-but-recommended observability: add a Memory Backing row to the **human** `kt agent show` table only. The human table and the `--json` document are rendered separately, so this costs no frozen key-set edit (DC-6). If you do it, say so explicitly in the Dev Agent Record; if the reviewer prefers strict minimalism, dropping it is acceptable.

- [x] **Task 7 — Tests (all ACs, DC-3, DC-9) — read Testing Notes first**
  - [x] 7.1 `crates/ktesio-engine/tests/` — a new `memory.rs` integration file (the repo has one file per capability: `registration.rs`, `lifecycle.rs`, `pause.rs`, `interaction.rs`, `logs.rs`, `metering.rs`, `budget.rs`, `cost.rs`…). Public API only; reuse the `fn open(base: &TempDir) -> Registry` helper shape (`tests/registration.rs:18`).
  - [x] 7.2 **AC1:** attach on a `registered` instance ⇒ the directory exists at the engine-reported path *inside* the Agent Home, and the public read reports the kind.
  - [x] 7.3 **AC3:** attach AND detach on a `Running` instance are both rejected, with **no** side effect (assert the directory/row state is unchanged afterwards — a guard that rejects *after* mutating is the bug worth catching). Use `Registry::seed_instance` (`registry.rs:1118`) to seed `Running` with no live process, exactly as `remove_running_without_force_is_rejected` (`:1383`) does. Also cover the other non-terminal states you decided to refuse in 4.3.
  - [x] 7.4 **AC4 — the headline test.** Attach → start → write a known byte payload into the managed dir (include a nested subdirectory and a non-UTF-8 byte so "byte-identical" is real, not "text survived") → stop → start → **drop the `Engine` and `Engine::open` the same state dir again** (that is the "engine restart" half; do not simulate it) → assert every byte equal. Poll committed state for the stop/start transitions (the instance log / a public status read) — never sleep (DC-9).
  - [x] 7.5 **AC2 — kind parity.** The SAME attach→start sequence against (a) `--kind mock` and (b) a fixture manifest adapter, asserting the descriptor actually reached the child both times. Deterministic observation vehicle: `fake_agent --dump <path>` writes `env=KEY=VALUE` lines (`crates/ktesio-conformance/src/bin/fake_agent.rs:709-725`, flags at `:325-334`) — assert the mapped variable is present with the engine-computed path. Table-drive the two kinds so the test literally *is* "the same command sequence".
  - [x] 7.5a **DC-10 (delivery honesty).** Two cases, table-driven against the same attach→start sequence: an adapter whose mapping DOES declare the reserved key (assert the child receives it via `fake_agent --dump`, and the public read reports it delivered) and one whose mapping does NOT (assert the start still SUCCEEDS, the notice is emitted, and the public read reports it undelivered). The second case is the one that would otherwise ship silent — do not skip it because it looks like a no-op. Also assert the reserved key does **NOT** appear in `effective-config.json` (the CORRECTION's property; a regression here would silently break story 3-4's honest-provenance rule).
  - [x] 7.6 CLI tests in `crates/kt/tests/agent_cli.rs`: attach/detach happy paths + `code == Some(N)` for every failure mode in DC-4. Parse tests inline in `main.rs` (mirror `test_agent_config_*`), and add `memory` to `test_agent_subcommands_exist`'s positive list.
  - [x] 7.7 Coverage: every new branch needs a test — including each new error arm and each `from_wire` rejection. **The 95% gate is functional again as of PR #120** (see "Coverage is real now" in Dev Notes); budget for a real tarpaulin run.
  - [x] 7.8 **Scoped mutation check (Q-5 ruling — NOT a full AI-64 pass).** Self-administered, two mutations, minutes not a session; record both in the Dev Agent Record. (a) Break one new exit-code mapper arm (point a new diagnostic at the wrong code) and confirm `agent.rs`'s mapper test FAILS; restore. (b) Delete the `if version < 5` step from `migrate` and confirm the Task 3.4 migration test FAILS; restore. If either mutation passes undetected, the guard is theater — fix the test, then re-apply the mutation to prove the fix catches it (AI-64 clause (b)). Nothing else in this story needs a mutation pass: it freezes no wire shape, no `schema_version`, no exit-code *number*, and no contract surface.

- [x] **Task 8 — Docs and gates (AC1, AC3, DC-4, DC-5)**
  - [x] 8.1 `docs/commands.md`: a `## kt agent memory attach <name> --kind filesystem` / `detach` section with bash-fence examples, placed near the `config` sections. State plainly what Ktesio guarantees (the managed directory persists under the Agent Home and travels with it) and that attach/detach require the instance stopped. Add **one** sentence for DC-10 — the engine hands the path to the adapter at the reserved config key, and whether the agent uses it depends on the adapter declaring a mapping for that key — so the docs are not silently wrong about delivery. The **full** three-level guarantee-vs-delegation statement (NFR-7) is **story 5-2's** deliverable per the Q-1 ruling — do not pre-empt it, and do not contradict it.
  - [x] 8.2 `scripts/check_docs.py`: add `"memory"` to `AGENT_COMMANDS` **and** add a nested set for its subcommands, mirroring `CONFIG_COMMANDS = {"get", "set"}` (`~:37-53`). Without both, the new bash fences either fail the gate or are silently skipped — 4-3 proved the skip case by mutation.
  - [x] 8.3 `README.md` command table gains the new verb (AI-68/M3 lesson: Epic 4 shipped three commands and forgot the table twice).
  - [x] 8.4 Run every gate under the pinned toolchain (see "Gate commands"). Confirm `Cargo.lock` unchanged (no new dependency) and `CONTRACT_VERSION` still `"0.4.0"` (DC-5).

## Dev Notes

### CRITICAL SCOPING — what this story is and is NOT

**Greenfield, verified.** `MemoryBacking` exists nowhere in code today. Exhaustively confirmed absent: no `MemoryBacking` trait/enum/module/field, no `memory` entry in `KNOWN_KEYS`, no `[memory]` manifest section, no `MEMORY_DIR` path const, no memory table in `sqlite.rs`, no memory field on `AgentInstance`/`FleetEntry`/`SpawnRecord`, no `kt agent memory` subcommand, nothing in `docs/`. The single artifact is a forward-looking comment at `crates/ktesio-engine/src/ports/mod.rs:14-15`: *"The remaining port (`MemoryBacking`) arrives with the story that needs it (entity-timing) — no speculative port trees."* Story 1-4 (`:39`) recorded the deliberate decision **not** to stub it. So: nothing to refactor, nothing to reuse — but everything to align with.

**In scope:** the `filesystem` kind end-to-end — path authority, an additive DB migration, attach/detach with a running-guard, descriptor delivery at start through the existing AD-9 config seam, kind parity (mock + manifest), byte-identical survival across stop/start and engine restart, the CLI verbs, docs.

**Explicitly OUT of scope** (do not do these here):
- The `native` kind and the guarantee/delegation *statement* — **story 5-2** (`5-2-delegate-to-native-memory-with-an-explicit-boundary`), which owns FR-16's native half + FR-17 + NFR-7's docs/command-output wording.
- Any `--json` surface for memory, and any edit to a frozen `--json` key-set. 5-2 lands both memory fields in ONE announced edit (DC-6, A-3).
- Any `ktesio-adapter-api` change: no `Manifest` field, no `[memory]` section, no `CONTRACT_VERSION` bump, no `SpawnSpec`/`StartLaunch` field (DC-5, and see "Descriptor delivery" for the three rejected mechanisms).
- Agent Home export/import tooling (FR-17's portability *procedure* is 5-2; a real export command is not in either story).
- Permission hardening on the managed directory (DC-8).
- Solving AI-63(b) — the global-lock architecture decision. This story must **not** make it worse and must state where it touches it; it does not fix it. **Settled 2026-07-30 (Q-3): Epic 5 is NOT blocked on AI-63(b);** the lock model is now recorded as **AD-17** with a bounded-work rule this story already satisfies, and AI-63(b)'s replacement-model decision is due before Epic 7's daemon work.

### Binding architecture decisions (quote these; they are the guardrails)

- **AD-11 — `MemoryBacking` port with two v1 impls** (`ARCHITECTURE-SPINE.md:102-106`). Rule, verbatim: *"`filesystem` (engine-managed directory inside the Agent Home; survives restarts byte-identically) and `native` (delegation marker; engine guarantees only Agent Home persistence). Attach/detach permitted only while the Agent Instance is not `running`. The backing descriptor is handed to the adapter at start; richer backings are Deferred behind this port."* **Prevents:** *"memory wiring becoming per-agent bespoke glue"* — which is exactly why the descriptor must ride a general mechanism, not an agent-specific one. **AD-11 now also carries a "Delivery clause" (added 2026-07-30 by the Q-1 ruling)** that fixes the delivery mechanism, the offered-vs-delivered distinction, the three-level guarantee statement, the no-snapshot rule, and terminal-state-only attach/detach. Read it — it is binding on this story and on 5-2.
- **AD-2 — public-API boundary** (`:56-59`). `kt` depends only on `ktesio-engine`'s public API + `ktesio-adapter-api` types. CI enforces it (`.github/workflows/ci.yml:247-269`). Consequences here: `kt` never joins the memory path (DC-1); every engine integration test uses the public API only (`tests/registration.rs:1-7`: *"If this file needs a `pub(crate)` item, that is a signal the public surface is insufficient"*); `ktesio-conformance` stays a dev-dependency only.
- **AD-6 — one SQLite state store; Agent Homes hold bulky artifacts** (`:77-80`). Rule names this story's shape outright: *"Logs, memory dirs, Skill Sets, and effective-config snapshots are files inside the Agent Home — never blobs in the DB."* So: the *directory* is in the Agent Home; only the *attachment metadata* is in SQLite, as typed columns (DC-2).
- **AD-9 — layered TOML config with persisted provenance** (`:92-95`). Precedence: engine-defaults < adapter defaults < instance config < **invocation overrides** (strongest). The descriptor rides the invocation layer at start, so a hand-set lower-layer value cannot win — the same property that makes `metering.base_url` safe.
- **AD-17 — coarse global locking, recorded with a bounded-work rule** (NEW, added 2026-07-30 by the Q-3 ruling; it is what AI-63 asked for). The two-mutex model is ADOPTED for v1, and **no operation whose duration scales with external state may be added under the locks without an explicit bound**; the ~17 inventoried unbounded sites are debt, not precedent. This story already complies by design (DC-7 refuses both unbounded ops it would otherwise invite) — cite AD-17, not the existing sites, if a reviewer asks why the FS work here is acceptable.
- **AD-15 / transition table** (`:123-129`). Attach/detach are **not** lifecycle commands and must not be added to `next_state` (`transition.rs:114`). They are non-transition operations with a pre-flight state check — the `EngineError::NotRunning` doctrine (`error.rs:315-323`), inverted.
- **Conventions rows** (`:158-173`): errors are `thiserror` in the engine and `miette` only in `kt`; timestamps RFC-3339 UTC; **`:172` — "the engine is the sole path authority: state-dir location and Agent Home layout are computed only inside the engine; `kt`, adapters, and Hosts receive paths from the API and never construct them"**, now extended (Q-4) with *"`paths.rs` also OWNS the Agent Home layout documentation: every story that adds an entry to the home records it there in the same commit"*; **`:162` (Naming, extended by Q-2) — the CLI noun-group shape for new operator capabilities**; OS-conditional code only inside `backends/`; coverage ≥95%.
- **PRD FR-15/FR-16/FR-17** (`prd.md:181-201`), incl. FR-15's consequence: *"Attach before start → the Adapter receives the Memory Backing descriptor at launch; detach requires the Agent Instance not `running`. `[ASSUMPTION: no hot-swap in v1.]`"*
- **PRD §7** (`:362-370`), contract 3: command names, flags, exit codes and `--json` schemas are a v1 compatibility surface. `kt agent memory …` becomes part of it the moment it ships — which is why A-3 keeps the new surface as small as the ACs allow. Note §7's deprecation *mechanics* are still `[ASSUMPTION: pending Islam]` (AI-68).

### Descriptor delivery — the decided mechanism, and the three alternatives rejected

AD-11 says the descriptor "is handed to the adapter at start". The adapter-facing reality on `main`:

- `AgentAdapter::start()` (`crates/ktesio-adapter-api/src/adapter.rs:96`) takes **no parameters** and is **never called by the engine** — every lifecycle method is still the inert story-1-3 seed returning `AdapterError::Unavailable`. There is no "start parameter struct" on the trait to extend.
- The real start path is `Supervisor::start_inner` → `StartLaunch` (`adapter/mod.rs:199-207`) → `SpawnSpec` (`ports/process_backend.rs:45-135`) → `ProcessBackend::spawn`. The single production `SpawnSpec` literal is `supervisor.rs:587-604`.

**DECIDED (assumption A-1, pending Islam's ratification): deliver via a reserved engine-namespace unified-config key injected as an invocation override at start.** This is not invention — it is story 3-4's proven shape for the identical problem (engine computes a value only known at spawn; the adapter must receive it): `METERING_BASE_URL_KEY` (`config.rs:126`) + `base_url_override` (`supervisor.rs:2921-2932`), whose own doc says *"Reusing the existing config-mapping means NO new contract surface (no `CONTRACT_VERSION` bump)."* Benefits: zero adapter-api change, zero semver exposure, works identically for native and manifest adapters (AC2), and delivery mechanism (env / flag / file) stays the adapter's declared choice — which is precisely AD-11's "not per-agent bespoke glue".

**CORRECTION (architect, 2026-07-30) — the "free side effect" this section originally claimed does NOT exist; do not build on it.** The original text said the engine persists the override-bearing config to `effective-config.json`, making the managed path visible via `kt agent config get`, and offered that snapshot as a ready-made observation point for AC1. **That is false against `origin/main` @ 0752d30.** Story 3-4 deliberately splits the two resolutions: `start_inner` builds a SEPARATE `mapping_effective` carrying the override (`supervisor.rs:513-519`), uses it ONLY for `resolve_secrets` (`:530`) and `apply_config_mapping` (`:532`), and then writes the snapshot from the **plain** `effective` (`:551` — `write_effective_config_snapshot(&name, &effective)`). Its own comment states the intent: *"The SNAPSHOT (2c) below stays on the plain `effective` (the operator config), so the ephemeral loopback URL is NOT persisted as 'what applied' — honest provenance."*

**Binding consequences for this story:**
1. **Follow the precedent exactly: do NOT put the memory override in the snapshot.** Build a `mapping_effective`-shaped value, use it for the mapping application only, and leave `write_effective_config_snapshot`'s argument untouched. Ratified into the spine (AD-11 Delivery clause): the injected path is a *delivery mechanism, not operator configuration*. `memory.dir` therefore does **not** appear in `effective-config.json` or in `kt agent config get`, and nothing in `config get`'s output changes in this story.
2. **AC1's observation vehicle is NOT the snapshot.** Use the two that actually exist: the public engine read (Task 4.5) for the persisted attachment, and `fake_agent --dump`'s `env=KEY=VALUE` lines for the descriptor actually reaching the child (Task 7.5's vehicle — use it for AC1 too). A test asserting the path appears in the snapshot would fail; a "fix" that makes it pass would break 3-4's honest-provenance property.
3. If a started instance's resolved memory path must be operator-visible, that is a **memory** surface (the Task 4.5 read, the Task 6.5 `show` row, and story 5-2's status surface) — not a config surface. Config answers "what config applied"; the operator never set this key.

Rejected, with reasons the dev should not relitigate silently:
1. **A field on `SpawnSpec`/`StartLaunch`.** Both are `pub`-field structs with **no `#[non_exhaustive]`** (there are ZERO `non_exhaustive` attributes in the whole repo), so a field-add is a breaking change to `ktesio-engine`'s public API. Worse, it does not actually solve delivery: something still has to name the env var the agent reads. Cost with no benefit.
2. **A `[memory]` manifest section.** `Manifest` (`manifest.rs:42-62`) is `#[serde(deny_unknown_fields)]`, so this is a `CONTRACT_VERSION` 0.4.0 → 0.5.0 bump *and* a forward-compat break (an older engine hard-rejects a newer manifest). Story 6-6 freezes the contract; do not spend contract surface here.
3. **An unconditional engine-chosen env var** injected into `SpawnSpec.env` regardless of declaration. Tempting as a "guarantee floor", but it makes the engine invent a de-facto contract token outside the contract crate — the worst of both worlds. **This is the live tension in A-1: see it flagged for Islam.**

### The global-lock question (AI-63) — where this story touches a KNOWN live risk

Not hypothetical, and not closed. AI-63(a)'s sweep (`_bmad-output/implementation-artifacts/ai-63-lock-sweep-2026-07-21.md`) enumerated **~17 genuinely unbounded filesystem operation sites** already performed while the engine holds its **two** coarse mutexes (`EngineInner { registry: Mutex<Registry>, supervisor: Mutex<Supervisor> }`, `engine.rs:130-133`, acquired registry-first and held for the whole synchronous operation, for ALL instances). Its own §"Epic-5 (filesystem) exposure" names this epic: *"anything Epic 5 routes through a lifecycle op or the registry under the lock is unbounded BY DEFAULT — the codebase has no 'do FS off the lock' convention except the output-log tailer."* **AI-63(b) — the architectural decision between per-call bounds, per-instance locks, or an actor/dedicated-writer model — remains OPEN and is Islam's + the architect's call.**

State of play at authoring time: AI-63(a) is concluded; a *partial* (a)-driven fix was in flight on `fix/ai-63-drain-usage-incremental` (67bd4a8) bounding the reaper's whole-file usage drain — i.e. the sweep's most-severe item is being closed, and the class is not.

**Exactly where this story adds FS work under the lock, stated so the reviewer can weigh it:**

| New FS work | Where | Lock(s) held | Bound | Verdict |
|---|---|---|---|---|
| `create_dir_all(<home>/memory)` at **attach** | new registry op (Task 4.1) | registry only (do NOT take the supervisor lock — Task 4.4) | one directory, no recursion | Acceptable: operator-initiated one-shot, same shape as `register`'s `ensure_dir` (sweep site 16) |
| `create_dir_all(<home>/memory)` at **start** (defensive) | `start_inner` pre-transition block (Task 5.3) | **both** | one directory, no recursion | Acceptable: identical cost to the existing `ensure_log_dir` (sweep site 7), which already runs on every start/stop/poll |
| one extra DB read (the attached backing) at start | `start_inner` | **both** | joins the "rusqlite class" | Acceptable: the start path already does ~8 such reads |
| — | — | — | — | — |
| **Contents copy/seed/restore at start** | **NOT DONE** (DC-7) | would be **both** | **O(tree size), unbounded** | **Refused by design.** This is the trap the sweep warned about; AC4 needs no copying, only non-interference. |
| **`remove_dir_all` on detach** | **NOT DONE** (see below) | would be **both** | **O(tree size), unbounded** | **Refused by design** — sweep site 17 is already the worst registry offender. |

**Second-order exposure this story creates and cannot avoid:** `Engine::remove --delete` calls `std::fs::remove_dir_all(home)` (`registry.rs:369/376/558`) under **both** locks — sweep site 17, O(tree size). Every byte an operator puts in `memory/` lengthens that under-lock delete. This is inherent to "memory lives in the Agent Home" (AD-6/AD-11) and is **not** fixable in this story; record it in the Dev Agent Record as an AI-63(b) input rather than papering over it. The mitigation that does NOT belong here: making `remove --delete` bounded/async is exactly AI-63(b)'s decision.

**Architect's ruling on that exposure (Q-3, 2026-07-30) — ACCEPTED, and here is the reasoning the reviewer should apply:** site 17 is **already** unbounded before Epic 5. The sweep's own finding 5 records that `agent.log` is unrotated and grows without bound *inside the same Agent Home tree* that `remove --delete` walks. So `memory/` changes site 17's **constant factor, not its complexity class** — it adds a second unbounded contributor to an already-unbounded delete. Epic 5 therefore does not create this risk and cannot be the thing that gates its fix; the existing contributor necessitates AI-63(b) independently. Recorded as debt under **AD-17**, which also forbids citing site 17 as precedent for new unbounded work.

### Detach semantics — deliberate, and worth the reviewer's attention

Detach clears the attachment row and **leaves the directory and its contents on disk**. Rationale: (a) it is operator data — silent deletion is the kind of surprise a supervisor must never spring; (b) it avoids adding a second unbounded `remove_dir_all` under both locks (above); (c) it matches the shipped default of `kt agent remove`, where **`--retain` is the default** and deletion is opt-in (`main.rs` `Remove { delete, retain, force }`). Consequence to document: re-attaching later re-adopts the existing contents. A future `detach --delete` is a deliberate deferral, not an oversight (A-4).

### Coverage is real now — plan for it

Epic 4's coverage gate was measuring against a **stale cached `fake_agent` binary** for weeks; PR #120 (`0752d30`, merged 2026-07-21) fixed the actual root cause. Both CI jobs now carry the guard (`.github/workflows/ci.yml:152-153` for `test`, `:590-591` for `coverage`):

```
rm -f target/debug/fake_agent target/debug/fake_agent.exe
cargo +stable build -p ktesio-conformance --bin fake_agent
```

and `scripts/test_automation.py:142-158` asserts both jobs still carry those exact two lines. So: **coverage feedback is genuine from this story onward, and the ≥95% gate will actually bite** — four of five epics merged over red coverage (AI-67), and that excuse is gone. Two operational consequences: (1) budget a real local `cargo tarpaulin` run — 4-3 skipped it and shipped an unverified claim; (2) `fake_agent_bin()`'s doc (`crates/ktesio-conformance/src/lib.rs:204-231`) says it plainly — *"EXISTENCE IS NOT FRESHNESS"*, and it has bitten this repo twice. If you add a `fake_agent` flag for this story's tests, `rm -f` the cached binary before running anything locally, or you will debug a phantom.

### Test blind-spot patterns to avoid (AI-64 / AI-66 — five named faces, all found in Epic 4)

1. **Fixture monoculture** — exercise the non-default path too. Here: a *nested subdirectory* and a *non-UTF-8 byte* in the AC4 payload (a text-only assertion does not prove "byte-identical"), and a manifest adapter, not just the mock.
2. **Silent OS self-skip** — a runtime `if windows { return; }` reports PASSED. If a test cannot run on Windows, give it the `_unix` suffix (repo convention) and make sure it is never the SOLE guard for a contract.
3. **Unpinned link** — both ends unit-tested, the wiring between them unasserted. Here the link at risk is *"the persisted attachment actually becomes the value in the child's environment"* — assert it end-to-end via `fake_agent --dump`, not by testing the override builder and the mapping separately.
4. **Untested mode combination** — e.g. attach → start → stop → detach → start (does a detached instance start cleanly with the directory still present?).
5. **Tautological assertion** — pin literals, not the constant a value was stamped from (e.g. assert `PRAGMA user_version == 5`, not `== SCHEMA_VERSION`).

### Project Structure Notes

- Path authority: `crates/ktesio-engine/src/paths.rs` (**new** `MEMORY_DIR` + accessor).
- New port module: `crates/ktesio-engine/src/ports/memory_backing.rs` + `ports/mod.rs` wiring **and doc update**.
- Persistence: `crates/ktesio-engine/src/store/sqlite.rs` (`SCHEMA_V5`, `SCHEMA_VERSION` 5, `migrate`), `crates/ktesio-engine/src/ports/state_store.rs`.
- Domain ops + errors: `crates/ktesio-engine/src/domain/registry.rs`, `domain/error.rs`, `domain/config.rs` (reserved key + `KNOWN_KEYS`), `domain/supervisor.rs` (`start_inner` pre-transition block + the override builder).
- Public API: `crates/ktesio-engine/src/engine.rs` — async methods **and** the `Blocking<'_>` mirrors; re-exports via `domain/mod.rs` → `lib.rs`.
- Adapters/mocks (lockstep): `crates/ktesio-engine/src/adapter/builtin.rs`, `crates/ktesio-conformance/src/lib.rs`.
- CLI: `crates/kt/src/main.rs` (clap tree + dispatch + parse tests), `crates/kt/src/cli/agent.rs` (bodies + mappers), `crates/kt/src/error.rs`, `crates/kt/src/exit_code.rs`.
- Tests: `crates/ktesio-engine/tests/memory.rs` (**new**), `crates/kt/tests/agent_cli.rs`, harness `crates/kt/tests/helpers/mod.rs`.
- Docs/gates: `docs/commands.md`, `README.md`, `scripts/check_docs.py`, `scripts/test_automation.py`.
- **No new crate. No new dependency** (`Cargo.lock` must not change). **No OS-`cfg`.** **No `ktesio-adapter-api` edit.**

### Testing Notes

- Read first: `crates/ktesio-engine/tests/registration.rs:1-61` (AD-2 rule, the `open()` helper, and the existing byte-identical Agent-Home isolation assertion at `:53-61` — the direct template for AC4), `crates/ktesio-engine/tests/lifecycle.rs` (start/stop conventions), `crates/kt/tests/agent_cli.rs:1-60` (harness; `start_via_surviving_engine` and the `_unix` convention).
- **Engine restart** means drop the `Engine` and `Engine::open` the same state dir. Do not simulate it with a fresh `Registry` only — `Engine::open` also runs orphan adoption (AD-5), which is exactly the path that must not disturb `memory/`.
- **Determinism (DC-9):** never `sleep` to await state. Poll the committed artifact — the instance log, a public status read, or the on-disk file. The coverage/ubuntu runners are contention-sensitive; a sleep-shaped test is a future flake and this suite forbids them.
- **Observation vehicles (race-free):** `fake_agent --dump <path>` writes `arg=<token>` and `env=<KEY>=<VALUE>` lines (`crates/ktesio-conformance/src/bin/fake_agent.rs:709-725`); `--marker <path>` proves reach. For a `file`-target mapping the rendered file in the Agent Home is itself the observation.
- **`fake_agent` staleness:** `parse()` ignores unknown args (`fake_agent.rs:336`), so a stale binary silently does *less* and your test fails as a timing flake. `rm -f target/debug/fake_agent*` then rebuild before trusting a local failure.
- Test naming: long behavioral snake_case sentences, e.g. `attaching_a_filesystem_backing_creates_the_managed_directory_inside_the_agent_home`, `attach_on_a_running_instance_is_rejected_and_changes_nothing`, `managed_memory_contents_survive_stop_start_and_an_engine_restart_byte_identically`, `the_same_attach_sequence_works_on_the_mock_and_a_manifest_adapter`.

#### Gate commands (pinned toolchain — bare `cargo` resolves to 1.96.1 via `rust-toolchain.toml`; use `cargo +1.96.1` if a version manager overrides it)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `rm -f target/debug/fake_agent target/debug/fake_agent.exe && cargo build -p ktesio-conformance --bin fake_agent`
4. `cargo nextest run --workspace --all-targets` (nextest, not `cargo test` — retries absorb known Win/macOS real-IO flakes)
5. `cargo test --workspace --doc`
6. `cargo tarpaulin --engine llvm --workspace --fail-under 95` — **run it this time** (NFR-3; the gate is functional again)
7. `python3 scripts/check_docs.py`
8. `python3 scripts/test_automation.py`
9. OS-`cfg` grep gate (no `cfg(unix|windows|target_os|target_family)` outside `crates/**/backends/`)
10. AD-2 boundary gate (`kt` → only `ktesio-engine` + `ktesio-adapter-api`; `ktesio-conformance` dev-dep only)
11. semver-check (dormant until story 7-4; stays green while `CONTRACT_VERSION` is untouched)
12. MSRV `--locked` (`git diff --quiet Cargo.lock`)
13. Currency grep-lint — not triggered (this story renders no currency)

### Exact code seams (READ these; extend, do not reinvent)

*(`origin/main` @ 0752d30 line numbers — see the caveat at the top of this file.)*

- **Path authority** — `paths.rs`: `EFFECTIVE_CONFIG_SNAPSHOT_FILE:70`, `EnginePaths:79`, `agent_home:164`, `instance_config:169`, `effective_config_snapshot:177` (**the accessor to mirror**), stale layout doc `:26-32`.
- **Directory creation + registry ordering** — `registry.rs`: `ensure_dir:1131` (**reuse**), `materialize_home:445-496` (registration-time creation precedent), `remove:521` with the running-guard at `:542-547` (**the ordering discipline to copy**), `seed_instance:1118` (`#[cfg(test)]`, seeds a `Running` row with no process), `write_spawn_record:666`/`clear_spawn_record:673`/`spawn_record:679` (**the pass-through shape**), `instance_log_dir:971` (the inlined-`"logs"` inconsistency **not** to copy).
- **Persistence** — `store/sqlite.rs`: `SCHEMA_VERSION:41`, `SCHEMA_V4:124` (**add `V5` beside it**), `agent_runtime` DDL `:81` (**structural precedent**), `migrate:232` (`SchemaTooNew` guard `:237-242`, step-up `:248-259`, stamp `:261-266`), `configure:164` (`foreign_keys=ON`), `instance_id:186`, `backend:271`, `classify_insert:298`.
- **State machine** — `lifecycle.rs`: `LifecycleState:26`, `as_str:48`, `from_wire:64`, `is_removable_without_force:84`. `transition.rs`: `next_state:114` (**do not add attach/detach here**).
- **Errors** — `error.rs`: `RegistryError:16`, `NotFound:38`, `RunningRequiresForce:45` (**message is wrong for this story — new variant needed**), `Io:56` (**reuse for FS failures**), `Store:170`; `EngineError:180`, `Log:296`, `NotRunning:315-325` (**the doctrine comment to copy**), `Store:426`. Mappers: `supervisor.rs:2914` `registry_to_engine`, `:2948` `config_to_engine`; `engine.rs:85` `stop_error_to_registry` (whose doc says it maps to `RegistryError::Io` *"rather than inventing a new public variant"* — the restraint to emulate).
- **Locks + public API** — `engine.rs`: `EngineInner:130-133`, `agent_home:252-256`, `register:269`, `remove:339-369`, `start:597-599`, `set_config:844-848` (**registry-lock-only shape to copy**), `run_blocking:932`, `Blocking:948+` (`start:1003`, `set_config:1081`).
- **Start path** — `supervisor.rs`: `start:384`, `start_inner:409`, **ordering invariant doc `:399-408`**, `agent_home:487`, `effective_config:488`, `resolve_config_mapping:491`, `resolve_secrets:529`, `apply_config_mapping:532`, `write_effective_config_snapshot:550`, `ensure_log_dir` call `:566` / def `:2110`, persisted `→ starting` transition `:579` (**everything fallible goes BEFORE this**), `SpawnSpec` literal `:587-604`, `spawn:608`, `base_url_override:2752-2763` (**the override builder to copy** — the story's earlier `:2921-2932` cite was wrong; verified against `origin/main` @ 0752d30), its call site + the `mapping_effective`/`effective` SPLIT `:513-519`, `resolve_secrets:530`, `apply_config_mapping:532`, `write_effective_config_snapshot(&name, &effective):551` (**takes the PLAIN effective — do not change this**), `ConfigMapping::target` in `crates/ktesio-adapter-api/src/config.rs` (**the free DC-10 lookup**; its doc records story 2-2 Decision 6 — an unmapped key is a silent no-op).
- **Config** — `config.rs`: `METERING_UPSTREAM_BASE_URL_KEY:110`, `METERING_BASE_URL_KEY:126` (**the doc style to copy verbatim in spirit**), `PASS_THROUGH_PREFIX`, `KNOWN_KEYS:565-581`, `is_pass_through`, `ConfigError:1032`.
- **Adapter surface (read-only reference)** — `crates/ktesio-adapter-api/src/lib.rs:76` (`CONTRACT_VERSION`, + its `contract_version_parses_as_semver` test `:92-101`), `adapter.rs:60/96`, `manifest.rs:42-62` (`deny_unknown_fields`), `config.rs:174` (`ConfigMapping`, arbitrary dotted keys). `crates/ktesio-engine/src/adapter/mod.rs`: `MANIFEST_FILE:50`, `StartLaunch:199-207`, `start_launch_from_manifest:310`, `apply_config_mapping` / `ConfigApplyError:332`.
- **Mocks (lockstep)** — `adapter/builtin.rs`: `native:37`, `native_config_mapping:50`, `MOCK_MODEL_ENV_VAR:31`, `BuiltinMock:61`. `crates/ktesio-conformance/src/lib.rs`: `MOCK_KIND:32`, `MOCK_MODEL_ENV_VAR:38`, `MockAdapter:52`, `fake_agent_bin:232-257` (staleness doc `:204-231`). Drift guard: `crates/ktesio-engine/tests/registration.rs:182-232`.
- **CLI** — `crates/kt/src/main.rs`: `Commands:85`, `AgentCommands:107`, `Config:220` + `ConfigCommands:229/239` (**the nested-group precedent**), dispatch `:312`, inline parse tests. `crates/kt/src/cli/agent.rs`: `register:126`, `remove:483`, `config_set:1399`, `config_get:1444`, `map_config_error:1676`, `map_error:1763`, `map_engine_error:1884`, mapper tests `:2198`/`:2298`, `validate_instance_name` (4-3 M2). `crates/kt/src/exit_code.rs` (frozen table + `classify`), `crates/kt/src/error.rs` (22 diagnostics), `crates/kt/src/ui.rs` (stdout vs stderr, AD-12).
- **CI/gates** — `.github/workflows/ci.yml`: `fake_agent` guard `:152-153` and `:590-591`, boundary `:247-269`, semver `:354-406`, currency lint `:223`. `scripts/check_docs.py` (`AGENT_COMMANDS` ~`:37-53`, `CONFIG_COMMANDS`), `scripts/test_automation.py:142-158`.

### References

- Epic / story source: `_bmad-output/planning-artifacts/epics.md:493-508` (Story 5.1), `:489` (Epic 5 header), `:510` (Story 5.2 — the scope boundary), `:35-36` (FR-15/FR-16), `:86` (the AD-11 port line); GitHub issue **#82**, epic **#59**.
- Architecture spine: `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` — **line numbers below are POST-ruling (spine `updated: 2026-07-30`); the pre-ruling cites in the earlier draft were ~1-7 lines lower from AD-12 onward.** AD-11 `:102-106` (**the Rule at `:105`, the new Delivery clause at `:106`**), AD-2 `:56-59`, AD-5 `:72-75`, AD-6 `:77-80`, AD-9 `:92-95`, AD-10 `:97-100`, AD-12 `:108-111`, AD-15 `:123-129`, **AD-17 `:153-156` (NEW — the lock model + bounded-work rule)**, conventions `:158-173` (path authority + layout-doc ownership `:172`; CLI noun-group shape in the Naming row `:162`), structural seed `:199` (the `src/ports/` slot naming `MemoryBacking`), capability map `:246` (*"Memory wiring (FR-15..17) | `ports::MemoryBacking` + impls | AD-11"*) + `:254` (the new concurrency row), Deferred `:265` (richer backings).
- PRD: `_bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md` — §4.4 `:181` + FR-15 `:185` / FR-16 `:191` / FR-17 `:197`, glossary `Agent Home` `:64` / `Memory Backing` `:67`, §7 `:362-370`, §9.1 `:390`, NFR-7.
- Prior stories (conventions to mirror): `2-1-…md` (config layers, `KNOWN_KEYS` additivity), `2-2-…md` (`[config]` mapping delivery — the seam this story rides), `2-4-…md` (`:170`, new-port arrival + the `ports/mod.rs` note update), `3-1-…md` (`:132/:171`, same), `3-4-…md` (**the engine-injects-a-start-value precedent**), `4-3-…md` (the frozen 0–6 exit codes, wire-freeze discipline, mutation verification), `1-2-…md` (Agent Home isolation), `1-4-…md` (`:39`, "do NOT stub memory speculatively").
- Retro / audit inputs: `_bmad-output/implementation-artifacts/ai-63-lock-sweep-2026-07-21.md` (**the ~17 unbounded-FS-under-lock inventory + its explicit Epic-5 exposure section**), `epic-4-retro-2026-07-21.md` (AI-63..AI-69; §9 open questions; the "state the current Agent Home layout" hand-off), `ai-67-coverage-clustering-2026-07-21.md`, `epic-2-retro-2026-07-08.md` (AI-35/38 determinism).
- Toolchain/coverage context: user memory `ktesio-gate-toolchain`, `ktesio-coverage-ci-oom`, `ktesio-engine-tests-parallel-oversubscribe`.

### Assumptions & open items

**Recorded assumptions — ALL RATIFIED by the architect 2026-07-30 (A-1 ratified *and amended*; A-2/A-4/A-6/A-7/A-8 stand as written; A-3 and A-5 ratified verbatim). No assumption in this list is still "pending Islam".**

- **A-1 — RATIFIED AND AMENDED. Descriptor delivery = reserved `memory.dir` unified key + invocation-override at start** (story 3-4's `metering.base_url` precedent; no contract change — DC-5). The original wording left the delivery gap as a live tension for Islam; **Q-1 closes it**: the mapping-declared mechanism is ratified as the *only* delivery mechanism (no unconditional env var, ever), and the gap is closed by **honesty, not a second mechanism** — the engine detects an undeclared mapping at start and says so (new **DC-10**). Two amendments the dev must apply: (i) delivery is **offered, not imposed** — the three-level guarantee statement in Q-1 is the binding wording, and "handed to the adapter at start" means *injected at the reserved key*, not *received by the agent*; (ii) the injected override is **NOT** written into `effective-config.json` — see the CORRECTION block under "Descriptor delivery", which overrides this story's original claim that it would be.
- **A-2 — creation at attach; verify-only at start.** Attach does the `create_dir_all`; start does an idempotent single-directory `create_dir_all` as a self-heal and nothing more. No copy/seed/restore of contents, ever (DC-7) — deliberate lock-safety.
- **A-3 — RATIFIED verbatim (Q-2). CLI surface kept minimal: `kt agent memory attach|detach`, human output only, no `--json`.** New CLI surface freezes into the v1 compatibility contract (PRD §7) *at v1 ship*, and 5-2 explicitly owns "inspect effective config or backing status", so memory's read/JSON surface lands there once — with `native` — as one announced key-set edit. Observability in 5-1 comes from the human `show` row (Task 6.5, still optional), the public engine read (Task 4.5), and the new start-time delivery notice (DC-10).
- **A-4 — detach is metadata-only; the directory and contents remain.** A `detach --delete` is deferred, not forgotten.
- **A-5 — RATIFIED verbatim and promoted into the spine (AD-11 Delivery clause): attach/detach are permitted only in a TERMINAL persisted state** (`Registered`/`Stopped`/`Failed`) and rejected in every non-terminal one (`Running`, `Starting`, `Stopping`, `Paused`), with no `--force` escape. The ACs name only `running`; refusing the whole non-terminal set is the correct reading of "no hot-swap" and the message names the actual state either way.
- **A-6 — re-attaching the same kind is an idempotent success; attaching a different kind over an existing one is rejected (detach first) → exit 4.**
- **A-7 — `filesystem` only.** The kind vocabulary is shipped so 5-2 adds `native` behavior without a breaking enum edit; 5-1's `--kind` parser accepts only what 5-1 implements.
- **A-8 — no `MemoryBacking` trait unless it earns its keep.** The port module is the seam (AD-11's deferral point); a trait with one real impl and one marker would be the "speculative port tree" `ports/mod.rs:14` explicitly warns against.

**Q-1..Q-5 — ALL RESOLVED (architect Winston, 2026-07-30; Islam delegated these decisions and directed that the rulings be applied). Dev implements the rulings below; they override any conflicting assumption above.**

- **Q-1 (AD-11 descriptor delivery) — RESOLVED: reading (a), config-mapping only, PLUS a mandatory delivery-visibility obligation.** No unconditional env var. *Rationale:* the adapter's declared `[config]` mapping **is** part of the Adapter Contract, so delivering through it *is* delivering through the port — whereas injecting a reserved env var into `SpawnSpec.env` reaches past the port into the process, which is the "per-agent bespoke glue" AD-11 exists to prevent, and it buys a guarantee nothing consumes (no real agent reads a `KTESIO_*` variable; only the adapter knows the name its agent actually reads), at the price of the engine minting a contract token outside `ktesio-adapter-api` immediately before 6-6 freezes that contract. The genuine defect in (a) is not weak delivery but **silence** — so the ruling closes it with honesty rather than a second mechanism: at start, when a `filesystem` backing is attached and the resolved `ConfigMapping` declares **no** target for the reserved key, the engine emits a diagnostic notice (stderr, AD-12) and the public backing read reports the undelivered state. The check is free — `mapping` is already in hand at `supervisor.rs:491` and `ConfigMapping::target(key) -> Option<&ConfigTarget>` is a plain lookup. Recorded in the spine as **AD-11's Delivery clause**. See new **DC-10**.
  - **Consequence for Story 5-2's FR-17/NFR-7 wording (this settles it):** the guarantee/delegation boundary is **three** levels, never two, and never collapsed — (1) *guaranteed*: the managed directory exists inside the Agent Home, survives stop/start and engine restarts byte-identically, and travels with the home; (2) *offered*: the engine injects the path at the reserved key at every start; (3) *delegated*: whether the agent receives it (the adapter must declare a mapping — an unmapped key is a silent no-op per story 2-2 Decision 6) and what the agent writes there. 5-2 must render (2) and (3) honestly in docs **and** command output — that is exactly what NFR-7 asks for. Note this also means `filesystem` and `native` differ in the **directory** guarantee, not in the delivery mechanism; do not write 5-2's boundary as "filesystem is guaranteed, native is delegated".
- **Q-2 (new CLI surface freeze) — RESOLVED: ratify `kt agent memory attach|detach` now, in this story; A-3 upheld (human output only, no `--json`).** *Rationale:* PRD §7 makes the CLI a compatibility surface **"once v1 ships"** — the workspace is at 0.6.0 and v1 has not shipped, so the pre-v1 window is precisely when new verbs should land, and blocking the operator half on an unrelated policy ratification would leave engine capability with no way to exercise the ACs (AC2 is literally *"the same attach command sequence"*) while Epic 5's entire product claim is that the interface **is** the command. Deferring the verb to 5-2 was rejected: 5-1 already carries the exit-code mapping discipline (DC-4, plus the two mapper tests that fail on an undocumented variant), which is the right place for a verb to be born. The nested-group shape (mirroring `config`) is ratified and generalized into a spine convention (Naming row) so Epic 8's skills surface does not re-litigate it: noun group under `kt agent`, one nesting level max, no flags on `register`/`start`, no top-level `kt <noun>`, and every new verb maps into the frozen 0–6 table without adding a number. **§7's deprecation *mechanics* are NOT architecture's call — see "Still with Islam" below; they do not block this story.**
- **Q-3 (AI-63(b) sequencing) — RESOLVED: proceed; Epic 5 is NOT blocked on AI-63(b). Accept the site-17 growth as recorded debt under the new AD-17.** *Rationale:* site 17 (`remove --delete`'s `remove_dir_all` under both locks) is **already** unbounded before Epic 5 — the sweep's own finding 5 records that `agent.log` is unrotated and grows without bound inside the same Agent Home tree — so `memory/` changes site 17's *constant factor*, not its *complexity class*, and gating Epic 5 on a decision the existing unbounded contributor already necessitates independently would buy nothing. This story's own additions are bounded single-directory work (two `create_dir_all`s of one directory, one DB read), the severest live instance is fixed in PR #126, and the retro's own recommendation was (a) before Epic 5 (done) with (b) landing so that **Epic 7 inherits a decision**. What *was* architecture's to do now is the part AI-63 explicitly asked for — the lock model was an implementation fact, not a recorded decision — so it is now **AD-17**, which records the two-mutex model as ADOPTED for v1 and binds all new work to a bounded-work rule (no operation whose duration scales with external state may be added under the locks without an explicit bound; the ~17 inventoried sites are debt, not precedent) and requires AI-63(b) to be decided **before Epic 7's daemon work begins**. Story 5-1 already satisfies AD-17 by design (DC-7); no task changes.
- **Q-4 (Agent Home layout doc ownership) — RESOLVED: neither epic owns it; `paths.rs` owns it permanently and every story that adds an entry to the home updates it in the same commit.** *Rationale:* the layout doc is path authority, which the spine already assigns solely to the engine (conventions `:166`), so a cross-epic "consolidated layout doc" hand-off would create exactly the deferral that let the comment go stale in the first place; recorded as a one-clause extension to the Filesystem-layout conventions row. Task 1.2 stands unchanged (5-1 states the current layout *including* `memory/`); Epic 8 adds `skills.json`/`skills.lock` when it adds them. No cross-epic dependency.
- **Q-5 (does AI-64 bind here?) — RESOLVED: NO for the full independent adversarial mutation pass; YES for a scoped, self-administered mutation check on the two surfaces this story genuinely puts at risk.** *Rationale:* AI-64 as ratified binds stories that **freeze** a wire shape, `schema_version`, exit code, or the Adapter Contract; 5-1 freezes none — it *maps into* the already-frozen 0–6 table (DC-4) and adds no `--json` (DC-6) and no contract surface (DC-5), and 4-3 already built the structural guard (the two mapper tests fail if a variant is added without a documented code). Requiring a full independent pass for every new verb would dilute a mandate whose value is its scarcity. But the story does add a **persisted schema migration** (`SCHEMA_VERSION` 4 → 5), which is a genuine compatibility surface that AI-64's list does not name — so the proportionate obligation is the scoped check in new **Task 7.8** (minutes, not a session). *Recommendation to Islam, not a ruling:* amend AI-64's standing mandate to add "persisted schema version" to its four-item list — a bad migration is precisely the unannounced-change class the mandate exists for.

**Still with Islam — and NEITHER item blocks 5-1 dev (both can proceed in parallel):**

- **PRD §7 deprecation *mechanics* (AI-68) — NOT architecture's call; it is product/owner policy (Islam).** Architecture can rule on *shape* (what the surface looks like, how it is versioned, which crate owns it) but not on *how long a deprecation window is* or *what obligates an announcement* — those are commitments to users, and only the owner makes them. **Why it does not block this story:** PRD §7 makes the CLI a compatibility surface *"once v1 ships"*, and the workspace is at 0.6.0. `kt agent memory attach|detach` ships pre-v1, so it is changeable under ordinary pre-1.0 semver until v1 is cut. **Recommended default if Islam wants one now:** ratify §7's already-written policy as-is (announce in release notes → minimum one minor-version notice window → removal only at a major) and add the one missing mechanic — *the announcement obligation attaches at the release that ships the surface, not at the release that changes it* — which would have caught Epic 4's unannounced 0–6 table. **What is genuinely needed from Islam:** (i) ratify or amend that policy text so the `[ASSUMPTION: pending Islam]` marker can be struck; (ii) decide whether Epic 4's shipped surface gets a back-dated release-notes entry now or waits for the next cut. Both must land **before Story 6-6** freezes the Adapter Contract, where the stakes are far higher — that is the real deadline, not this story.
- **AI-64 amendment (from the Q-5 ruling).** Recommend adding "persisted schema version" to AI-64's four-item freeze list. It is Islam's standing mandate, so the amendment is his to make; 5-1 meanwhile discharges the substance via Task 7.8.

## Change Log

| Date | Version | Description | Author |
|---|---|---|---|
| 2026-07-30 | 0.1 | Initial story context created (headless BMAD create-story run) against `origin/main` @ 0752d30. Status → ready-for-dev. Five open questions raised for Islam (Q-1 AD-11 delegation boundary, Q-2 v1 CLI surface freeze, Q-3 AI-63(b) lock/FS, Q-4 Agent Home layout doc ownership, Q-5 AI-64 scope). | create-story (BMAD) |
| 2026-07-30 | 0.2 | **Q-1..Q-5 all RESOLVED by the architect** (Islam delegated; rulings applied, not merely recorded). Spine updated: **AD-11 gains a Delivery clause** (config-seam delivery; offered-vs-delivered; three-level guarantee statement binding on 5-2; no snapshot; terminal-state-only attach/detach), **new AD-17** records the two-mutex lock model with a bounded-work rule (AI-63's explicit ask), and two Consistency-Convention rows extended (CLI noun-group shape; `paths.rs` owns the Agent Home layout doc). Story: **new DC-10** (delivery honesty) + **new Tasks 5.2a, 7.5a, 7.8**; A-1 ratified-and-amended, A-3/A-5 ratified verbatim, no assumption left pending. **MATERIAL CORRECTION:** the "Descriptor delivery" section's claim that the injected override is persisted into `effective-config.json` and visible via `kt agent config get` is **false against `origin/main`** — 3-4 writes the snapshot from the plain `effective` (`supervisor.rs:551`) and deliberately keeps the override out; the AC1 observation vehicle that claim offered does not exist, and Tasks 5.2 / 7.5a now enforce the correct behavior. Also fixed a wrong seam cite (`base_url_override` is `:2752-2763`, not `:2921-2932`). | Winston (architect) |

## Dev Agent Record

### Agent Model Used

ox-alpha (opencode), resuming + completing an interrupted dev run on `feat/epic-5-memory`, 2026-08-23.

### Debug Log References

Resumed from an interrupted run: Tasks 1–5 and the 6.2–6.4 bodies existed but were UNVERIFIED (workspace did not compile — non-exhaustive `map_error` match on the two new `RegistryError` variants; Task 6.1's clap tree, Task 7's tests, and Task 8's docs were missing entirely). This session audited every existing hunk against the spec (kept all of it; it conformed), then finished: mapper arms + mapper-test pins, the full clap tree + dispatch + parse tests, the migration-test literal-pin fix (AI-66 #5 tautology: it compared `PRAGMA user_version` to `SCHEMA_VERSION` itself), `tests/memory.rs` (6 tests), 7 CLI tests, docs + gates.

Test-currency fixes found by running the suite (the interrupted run never had): (1) config.rs's pinned `KNOWN_KEYS` list needed the additive `"memory.dir"` entry (+ a new known-but-not-operator-set assertion test); (2) registry.rs's terminal-state unit test read `memory_status` on SEEDED rows — the delivery fact needs the adapter snapshot, which a seeded row's home deliberately lacks; the store-level row assert is what that test needs (real-instance delivery reads are covered in `register()`-based tests + integration).

Task 7.8 scoped mutation check (Q-5): BOTH mutations caught. (a) Pointed the `MemoryBackingHotSwap` mapper arm at `AgentIo` (code 1) → `registry_error_mapper_arms_preserve_their_documented_exit_codes` FAILED (`left: General, right: InvalidState`); restored, test green. (b) Deleted the `if version < 5` step from `migrate` → `migration_v4_db_upgrades_to_v5_preserving_rows` FAILED (`no such table: agent_memory_backing`); restored, test green.

Gates (all under `cargo +1.96.1`; bare cargo is mise-shimmed to 1.94.1 here and its rustfmt DISAGREES with 1.96.1's — format/check with one toolchain only): fmt ✓ · clippy `-D warnings` ✓ (fixed 3 `doc_lazy_continuation` doc-list errors) · fresh fake_agent build ✓ · workspace tests all-targets ✓ (~950 tests, 0 fail) · doc tests ✓ · check_docs.py ✓ (22 files) · test_automation.py ✓ (21 tests) · OS-cfg grep: 7 hits, ALL pre-existing in kt self_update/update_check (untouched) · AD-2 tree gate ✓ (`ktesio → ktesio-engine → ktesio-adapter-api`, no conformance) · Cargo.lock UNCHANGED ✓ · CONTRACT_VERSION still "0.4.0" ✓ · **tarpaulin --engine llvm --fail-under 95: 95.43%** (4614/4835; `ports/memory_backing.rs` 100%).

### Review Round (2026-08-23, three-layer BMAD review: blind-hunter + edge-case-hunter + verification-gap)

23 findings triaged: 10 patched, 4 deferred, 9 rejected (no intent_gap / no bad_spec ⇒ no loopback; code re-verified in place). PATCHES applied to the same tree:

1. **E5 symlink containment** — new `Registry::ensure_managed_memory_dir` refuses a pre-existing symlink at `<home>/memory` on attach/re-attach (deliberately NOT the shared `ensure_dir`: state-dir/home roots may legitimately be operator symlinks to another volume); the start-time self-heal gained the mirror refusal before its `create_dir_all`. Unix-gated integration test drives BOTH refusals through the public API and asserts nothing is written through the link.
2. **E3 strict UTF-8 delivery** — the start path now fails LOUD (typed `EngineError::Log`, pre-transition) if the managed path is not valid UTF-8, instead of lossy-coercing a mangled path into the reserved-key override.
3. **E4 spoofed reserved key** — `start_inner` strips any operator-supplied `memory.dir` from the resolved layers (mirroring the reserved-identity `name` drop), so without an attached backing no hand-set value can masquerade as engine-delivered memory; also keeps it out of the persisted snapshot.
4. **V1 self-heal proof** — new integration test: hand-delete `<home>/memory` while stopped ⇒ START recreates it; hand-delete again ⇒ same-kind RE-ATTACH recreates it.
5. **V2 native-suppression pin** — new integration test: `native` backing + manifest adapter that DOES declare the mapping ⇒ real child starts with NO injected env line and NO created directory (the `.filter(kind == Filesystem)` gate can no longer regress silently).
6. **B8 honesty rename** — `MemoryBackingStatus::delivered` → `declared` (it reports a DECLARED mapping target, not runtime receipt); field doc states exactly that.
7. **B5 state-aware remediation** — both HotSwap message copies now say "bring it to a terminal state first … stop from running or paused" instead of unconditional "stop it first" (which dead-ends from starting/stopping).
8. **B7 stdout contract documented** — commands.md now states the attach output shape (banner line + bare path on the final stdout line; diagnostics on stderr).
9. **B11 reserved key documented** — commands.md states `memory.dir` is engine-reserved, never operator-set, stripped at start, never in the snapshot.
10. **B12** — removed the vestigial `#[allow(dead_code)]` on `MemoryBackingKind::Native` (the variant is constructed by shipped paths).

Post-patch gates, all green: fmt ✓ · clippy `-D warnings` ✓ · full workspace tests all-targets ✓ (memory suite 9/9 incl. the 3 new tests; registry unit test added for the corrupt-manifest `memory_status` → typed Io path) · check_docs.py ✓ · test_automation.py 21/21 ✓ · **tarpaulin 95.45% ≥ 95** (+0.03pp vs pre-review).

Deferred (see `deferred-work.md`): attach↔start TOCTOU family (E1/E2/B4 — inherent to AD-17's adopted coarse-lock model, belongs to AI-63(b)), migration stamp atomicity (E6 — the V1→V5 pattern predates this story), store REPLACE-vs-registry-keep semantic split (B3), integration test-helper duplication (B15). Rejected as noise/spec-scoped: CLI status surface (Task 4.5 scopes the read to the engine API; 5-2 owns the surface), attached_at display, hot-swap wording, re-attach output distinction, CHANGELOG (release-flow concern), opaque timestamp decode.

### Completion Notes List

- **A-8 honored**: NO trait — the port module IS the seam; types only.
- **A-5/A-6 honored**: terminal-states-only guard (Registered/Stopped/Failed pass; Running/Starting/Stopping/Paused refuse), no force escape; same-kind re-attach idempotent (original timestamp stands); different-kind attach rejected with both kinds named.
- **CORRECTION honored**: the memory override rides a `mapping_effective`-shaped resolution used ONLY for secret-resolution + mapping application; `write_effective_config_snapshot` keeps the plain `effective`. Asserted by tests: neither `memory.dir` nor the path appears in `effective-config.json`.
- **DC-10 delivered twice over**: pure decision fn (`memory_delivery_notice`) + one stderr emission in the start pre-transition block; the public read carries `delivered`; CLI e2e asserts the notice text on stderr with exit 0 (mapped leg: dump proves receipt, notice silent).
- **Task 7.5 deviation, recorded honestly**: "the descriptor actually reached the child BOTH times" is unachievable verbatim — a native `--kind mock` has NO launch command (`NativeHasNoLaunch`), so it cannot have a child. The mock leg uses the SHIPPED story-2-2 Decision-8 inert-mock vehicle instead: resolve the builtin's code-declared mapping via the public API, fold the invocation override exactly as `start_inner` does, apply onto a launch, assert the declared env target carries the engine-computed path. The manifest leg observes a REAL child via `fake_agent --dump`. Both legs run ONE shared table-driven sequence.
- **Task 6.5 (optional human `show` Memory Backing row): deliberately NOT done**, per the task's own "strict minimalism is acceptable" allowance — observability ships via attach/detach confirmations (path from the engine), the DC-10 start notice, and the public read. Story 5-2 owns the fuller status surface.
- **AI-63(b) input (recorded per the spec's lock section)**: `Engine::remove --delete`'s `remove_dir_all(home)` under both locks now also walks whatever operators store in `memory/` — constant-factor growth of already-unbounded site 17, accepted debt under AD-17 (Q-3).
- **Second-order honesty note**: `Registry::memory_status` resolves the delivery fact from adapter facts; for an instance whose home lacks the snapshot (only reachable via test-seeded rows today) it errors like every other snapshot read rather than guessing.
- Windows note: nothing OS-specific was added; the new engine suite runs cross-OS (manifest legs spawn real processes through the standard harness). The `_unix` convention was not needed.

### File List

- crates/ktesio-engine/src/paths.rs (MEMORY_DIR const + accessor + layout doc)
- crates/ktesio-engine/src/ports/memory_backing.rs (NEW)
- crates/ktesio-engine/src/ports/mod.rs
- crates/ktesio-engine/src/ports/state_store.rs
- crates/ktesio-engine/src/store/sqlite.rs (SCHEMA_V5, bump, methods, tests)
- crates/ktesio-engine/src/domain/config.rs (MEMORY_DIR_KEY + KNOWN_KEYS + tests)
- crates/ktesio-engine/src/domain/error.rs (two RegistryError variants)
- crates/ktesio-engine/src/domain/mod.rs
- crates/ktesio-engine/src/domain/registry.rs (attach/detach/read + guard + tests)
- crates/ktesio-engine/src/domain/supervisor.rs (pre-transition wiring, invocation_overrides, DC-10 notice + tests)
- crates/ktesio-engine/src/engine.rs (async + Blocking methods)
- crates/ktesio-engine/src/lib.rs
- crates/ktesio-engine/src/adapter/builtin.rs (mock mapping + MOCK_MEMORY_ENV_VAR)
- crates/ktesio-conformance/src/lib.rs (fixture lockstep)
- crates/ktesio-engine/tests/memory.rs (NEW)
- crates/kt/src/main.rs (clap tree + dispatch + parse tests)
- crates/kt/src/cli/agent.rs (command bodies + map_error arms + mapper pins)
- crates/kt/src/error.rs (AgentMemoryHotSwap, AgentMemoryKindConflict)
- crates/kt/src/exit_code.rs (classify arms + module-doc table + tests)
- crates/kt/tests/agent_cli.rs (7 memory tests + seeding helper)
- docs/commands.md (memory sections + exit-code-4 causes)
- scripts/check_docs.py (memory + MEMORY_COMMANDS allowlist)
- README.md (command table rows)
- _bmad-output/implementation-artifacts/5-1-attach-a-managed-filesystem-memory-backing.md (this record)
- _bmad-output/implementation-artifacts/sprint-status.yaml (status note)

## Suggested Review Order

**The port seam (design intent)**

- Closed kind vocabulary + wire discipline — the AD-11 extension point, deliberately not a trait
  [`memory_backing.rs:43`](../../crates/ktesio-engine/src/ports/memory_backing.rs#L43)

- The public read: kind + path + the DC-10 `declared` fact (honesty about offered vs received)
  [`memory_backing.rs:100`](../../crates/ktesio-engine/src/ports/memory_backing.rs#L100)

**Path authority**

- The one true managed path: `<Agent Home>/memory`, computed only by the engine
  [`paths.rs:216`](../../crates/ktesio-engine/src/paths.rs#L216)

**Persistence (additive schema v4→v5)**

- One table, UNIQUE FK, cascade — structural copy of `agent_runtime`; no contract surface
  [`sqlite.rs:145`](../../crates/ktesio-engine/src/store/sqlite.rs#L145)

- Port method on `StateStore` (thin SQLite impl at :860; REPLACE documented, registry enforces A-6)
  [`state_store.rs:200`](../../crates/ktesio-engine/src/ports/state_store.rs#L200)

**Registry ops: guard ordering is the security model**

- attach: validate → lookup → terminal-state guard → symlink-hardened dir → row
  [`registry.rs:612`](../../crates/ktesio-engine/src/domain/registry.rs#L612)

- detach: metadata-only (operator data never deleted); nothing-attached is a success no-op
  [`registry.rs:677`](../../crates/ktesio-engine/src/domain/registry.rs#L677)

- The public read resolving the adapter's declared mapping into the delivery fact
  [`registry.rs:699`](../../crates/ktesio-engine/src/domain/registry.rs#L699)

- Review hardening: refuses to follow a planted symlink out of the Agent Home
  [`registry.rs:1330`](../../crates/ktesio-engine/src/domain/registry.rs#L1330)

**Start-path delivery + honesty**

- Pre-transition block: read backing, filesystem-only gate, strict UTF-8, symlink refusal, self-heal
  [`supervisor.rs:616`](../../crates/ktesio-engine/src/domain/supervisor.rs#L616)

- Reserved-key strip: operator hand-set `memory.dir` can never masquerade as engine delivery
  [`supervisor.rs:601`](../../crates/ktesio-engine/src/domain/supervisor.rs#L601)

- Invocation override builder — the descriptor rides the layered-config seam, not the contract
  [`supervisor.rs:3019`](../../crates/ktesio-engine/src/domain/supervisor.rs#L3019)

- DC-10 notice: attached-but-unmapped says so on stderr; start still succeeds
  [`supervisor.rs:3050`](../../crates/ktesio-engine/src/domain/supervisor.rs#L3050)

**Engine facade + adapter parity**

- Public async methods + Blocking mirrors (AD-2: tests may use nothing else)
  [`engine.rs:1174`](../../crates/ktesio-engine/src/engine.rs#L1174)

- Mock declares the reserved key in code, in lockstep with the conformance mock
  [`builtin.rs:113`](../../crates/ktesio-engine/src/adapter/builtin.rs#L113)

**CLI + error surface**

- `kt agent memory attach|detach`: name-first validation, exit codes per frozen table
  [`agent.rs:1684`](../../crates/kt/src/cli/agent.rs#L1684)

- Both guard errors map to exit 4 (invalid state) — no new exit-code number minted
  [`agent.rs:1941`](../../crates/kt/src/cli/agent.rs#L1941)

- Typed error with state-aware remediation text
  [`error.rs:64`](../../crates/ktesio-engine/src/domain/error.rs#L64)

**Tests**

- Integration suite: AC1–AC4 byte-identical survival, parity table, DC-10 both legs
  [`memory.rs:135`](../../crates/ktesio-engine/tests/memory.rs#L135)

- Self-heal proof: hand-deleted dir recreated by start AND re-attach
  [`memory.rs:560`](../../crates/ktesio-engine/tests/memory.rs#L560)

- Native-suppression pin: mapped manifest + native backing ⇒ no injection, no directory
  [`memory.rs:672`](../../crates/ktesio-engine/tests/memory.rs#L672)

- CLI exit-code matrix incl. kind-conflict via DB-seeded row and stderr-notice e2e
  [`agent_cli.rs:5409`](../../crates/kt/tests/agent_cli.rs#L5409)

**Docs & gates**

- Command docs: guarantee statement, stdout contract, reserved-key warning
  [`commands.md:208`](../../docs/commands.md#L208)

- check_docs allowlist for the nested `memory` verbs (the 4-3 mutation lesson)
  [`check_docs.py:50`](../../scripts/check_docs.py#L50)
