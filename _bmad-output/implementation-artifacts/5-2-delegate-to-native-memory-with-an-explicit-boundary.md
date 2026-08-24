---
baseline_commit: ff1f669552c21b3c45a3e59a2b7cdca388a39347
baseline_ref: origin/main (PR #138 merged — "feat(engine): attach a managed filesystem Memory Backing (story 5-1)")
---

# Story 5.2: Delegate to native memory with an explicit boundary

Status: review (implementation complete 2026-08-24, commit 67f3be9; awaiting BMAD review)

<!-- Context engineered 2026-08-24 (headless BMAD run). Ground truth verified against `origin/main` @ ff1f669, which already carries story 5-1's Memory Backing surface: the `native` vocabulary variant (`ports/memory_backing.rs`), the attach/detach/status surface, SCHEMA_V5 persistence, and the DC-10 honesty machinery. This story ADDS BEHAVIOR to an existing enum variant — no schema or enum-shape break is needed by design. -->

## Story

As an Operator,
I want a `native` backing that says plainly Ktesio only guarantees Agent Home persistence,
so that I always know what is guaranteed versus delegated. (FR-16 native half, FR-17)

## Acceptance Criteria

Verbatim from `_bmad-output/planning-artifacts/epics.md` lines 510–523 (Story 5.2); GitHub issue #83, epic #59.

**AC1 — delegation recorded and visible**
**Given** an instance with the `native` Memory Backing
**When** I inspect effective config or backing status
**Then** the delegation is recorded and visible: memory semantics belong to the agent; Ktesio guarantees only Agent Home persistence

**AC2 — portability + stated boundary**
**Given** an Agent Home copied to another machine (documented portability procedure)
**When** the instance runs there
**Then** a `filesystem` backing travels with it and the agent runs with memory intact, and the guarantees-vs-delegation boundary is stated in docs and command output (NFR-7)

### Derived / consequence criteria (testable — from FR-16/FR-17, AD-11, epic-5-context, and the code state @ ff1f669)

- **DC-1 (`native` behavior = metadata + honesty, NOT a directory).** Attaching `MemoryBackingKind::Native` must persist the row (the store path already accepts it end-to-end) but MUST NOT create `<home>/memory`. 5‑1 pinned this: `a_native_backing_never_injects_the_reserved_key_or_creates_the_directory_at_start` (`crates/ktesio-engine/tests/memory.rs`) asserts no reserved-key injection and no directory creation for a native backing at start; `Registry::attach_memory` materializes the dir only for the `Filesystem` kind (`domain/registry.rs`, `if kind == MemoryBackingKind::Filesystem`). The work here is everything AROUND that marker: visibility of the delegation fact.
- **DC-2 (status reports the guarantee level — extend, don't re-shape).** `MemoryBackingStatus { kind, dir, declared }` (`ports/memory_backing.rs`) already exists and 5‑1 shaped it "for reuse by story 5-2's status/effective-config surface". For `native`: `dir` stays the COMPUTED location only (doc says so verbatim today); `declared` is meaningless for delivery (nothing is injected) and must not read as a promise. Preferred shape: keep the struct additive — add the delegation STATEMENT as rendered text or a small typed field (e.g. `guarantee: GuaranteeLevel { HomePersistenceOnly, ManagedDirByteDurable }`) rather than breaking the existing fields; story 4-3 froze NO memory wire shape yet (5‑1 shipped human output only), so this is the LAST cheap moment to fix a shape that Epic 6 will freeze.
- **DC-3 (CLI surface completes the noun group).** `kt agent memory attach <name> --kind native` currently exits **2** ("This release accepts: --kind filesystem", `cli/agent.rs` `memory_attach`). Flip it to accept `native` (map to `MemoryBackingKind::Native`); detach/status wording gains the delegation sentence. Same guard set as filesystem applies unchanged: terminal-state-only, no `--force`, exit 4 on hot-swap/kind-conflict (both mappers already exist).
- **DC-4 (no contract/config change).** CONTRACT_VERSION stays `"0.4.0"`; no new unified-config key (a `native` backing injects NOTHING at start — the supervisor's `memory_dir` filter already keys off `Filesystem`); no `[memory]` manifest section (denied by `deny_unknown_fields`; belongs to Epic 6 if ever).
- **DC-5 (portability is documentation + proof-by-construction, not new machinery).** AC2's mechanism ALREADY holds: an Agent Home is a plain relative-to-`state_base` tree (`paths.rs` layout doc), the backing row rides inside `state.db`, and `filesystem` contents are untouched files under `<home>/memory`. The story delivers (a) a documented copy procedure (docs/ — include stop-first, copy-whole-state-dir, same-relative-layout, schema-version caveat via `StoreError::SchemaTooNew`), and (b) one integration test proving a copied home serves a byte-identical `memory/` tree (reuse `tests/memory.rs`'s `snapshot_tree`). No sync/copy feature code.
- **DC-6 (boundary stated in command output — NFR-7).** Attach confirmation for BOTH kinds names its guarantee level in one sentence: filesystem → managed-directory guarantee (exists, survives restarts byte-identically, travels with the home); native → "Ktesio guarantees only Agent Home persistence; memory semantics belong to the agent." Docs updated in the same change (`docs/commands.md` memory sections + README table row wording), per the standing docs-currency gate.
- **DC-7 (frozen contracts respected).** No new exit codes (attach-native success → 0; guards → existing 4; unknown kind → 2 — the classifier arms from 5‑1 cover both diagnostics already). No `#[cfg]` gates outside `backends/`. No test sleeps; poll committed state (existing conventions).

## Ratified decisions (Islam, 2026-08-24)

- **Q-1 → DEFER (option b).** Keep human output only in 5.2; no `--json` memory surface ships, and the wire-format freeze moves to Epic 6 where Hermes' real consumers force the shape. AC1 is satisfied by the human-visible delegation statement (DC-2/DC-6). The "ONE intentional announced key-set edit" from 5‑1's DC-6 is therefore ALSO deferred with it — recorded here so Epic 6 inherits the obligation.
- **Q-2 → TYPED FIELD.** `MemoryBackingStatus` gains a typed `guarantee: GuaranteeLevel` enum (`HomePersistenceOnly`, `ManagedDirByteDurable`) rather than a rendered string. It stays an engine-API type only for now (no serde/wire exposure per Q-1); Epic 6 freezes its wire form when JSON lands.

## Tasks / Subtasks (dependency-ordered; each names its AC/DC)

1. **Accept `--kind native` at the CLI + registry round-trip (AC1, DC-1, DC-3).**
   - `crates/kt/src/cli/agent.rs` `memory_attach`: add the `"native" => MemoryBackingKind::Native` arm; update the usage-error text to list both kinds; update the attach confirmation per DC-6.
   - Registry/store paths need NO code change (verify: seed → attach native → row persists with `kind="native"`; dir NOT created — pin with a registry test mirroring `attach_creates_the_managed_directory…`).
   - Exit-code mapper tests: extend the existing tables (unknown-kind text change is covered by current assertions).
2. **Make the delegation visible on reads (AC1, DC-2, DC-6).**
   - Extend `MemoryBackingStatus` per the ratified Q-2 shape; `Registry::memory_status` fills it without touching the adapter-mapping resolution for native (skip `memory_key_declared` — nothing is delivered; report the delegation instead).
   - Human output: wherever status renders today (detach message, future status verb), print the boundary sentence (NFR-7).
3. **Portability: document + prove (AC2, DC-5).**
   - New docs section (docs/commands.md or a dedicated docs/memory.md): the copy procedure + guarantee/delegation table.
   - Integration test in `tests/memory.rs`: build engine A with a populated `memory/` tree, copy the state dir to a second temp root, open engine B, assert byte-identical tree via `snapshot_tree` and a successful start.
4. **Docs + gates (DC-6, standing gates).**
   - `docs/commands.md` memory sections: native kind, boundary sentences, portability link; README command-table row update; `scripts/check_docs.py` needs no change (fences already validated).
   - Full gate suite: fmt, clippy `-D warnings`, workspace tests, check_docs, tarpaulin ≥ 95 (coverage bites — budget the real run).

## Dev Notes (ground truth @ ff1f669)

- The `native` variant ALREADY round-trips through the store (`from_wire("native")` → `as_str()` == `"native"`, tested in `ports/memory_backing.rs` tests) and through attach conflict logic (`AgentMemoryKindConflict` test uses attached=`filesystem`, requested=`native`). Only the CLI token gate and the visibility layer are missing.
- Start-path non-injection for native is already implemented AND tested (`supervisor.rs` filters on `Filesystem`; `tests/memory.rs::a_native_backing_never_injects…`). Do not regress it.
- Deferred-work items from 5‑1 (TOCTOU attach-vs-start, migration crash-atomicity, store-vs-registry idempotence split, shared test-support module) stay OUT of this story — they belong to AI-63(b)/focused follow-ups. Do not widen scope.
- Story 5-1 artifact (`5-1-attach-a-managed-filesystem-memory-backing.md`, Status: done) is the authoritative record of the shipped design decisions this story builds on (Q-1 ruling: mapping-declared-only delivery; A-8: the module IS the port).
