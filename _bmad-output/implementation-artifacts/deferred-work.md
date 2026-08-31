# Deferred Work

Findings surfaced incidentally during quick-dev reviews that are out of scope for the triggering change. Collected for later focused attention.

## From AI-17 (pin workspace toolchain to 1.96.1) — review, 2026-07-06

- **Contributor docs still tell contributors to run bare `cargo` (fmt/clippy/test).** With the new `rust-toolchain.toml`, bare `cargo` resolves to the MSRV (1.96.1) locally for contributors without a `RUSTUP_TOOLCHAIN` override, while CI's fmt/clippy/test jobs now gate on latest `stable` (explicit `+stable`). This local-vs-CI toolchain skew is intentional but is not documented in the other contributor-facing files. Consider a one-line note (or a `+stable` reproduction hint) in: `CONTRIBUTING.md` (~L89-91), `docs/contributing.md` (~L15-24), `AGENTS.md` (~L14-16), `.github/pull_request_template.md` (~L7-9), `docs/github-repository-audit-checklist.md` (~L167-169), `.agents/skills/kt-release/SKILL.md` (~L58), and `scripts/prepare_kt_release.py` (~L244-246). `docs/testing.md` already documents the split; the rest do not. Low severity (surfaces as an occasional new-stable clippy/rustfmt CI nit, not a shipped bug).

- **Coverage CI job rebuilds `cargo-tarpaulin` on every fresh runner (no binary cache).** Pre-existing (predates AI-17): the `coverage` job in `.github/workflows/ci.yml` runs an unguarded `cargo install cargo-tarpaulin` with no `~/.cargo/bin` cache, so it recompiles tarpaulin (~several minutes) every run. The `semver` job already added a `${{ runner.os }}-cargo-semver-checks-bin` cache + `command -v` guard (AI-1); the coverage job could adopt the same pattern for symmetry and CI speed.

## From Story 5-1 (managed filesystem Memory Backing) — three-layer review, 2026-08-23

- source_spec: `5-1-attach-a-managed-filesystem-memory-backing`
  summary: Attach/detach vs start TOCTOU — the backing row read/write and the supervisor's start-path snapshot are not mutually atomic (attach landing between a start's backing read and spawn; detach clearing the row after the read but before launch), and the terminal-state guard's check is separate from the row write.
  evidence: Real windows under AD-17's ADOPTED coarse two-mutex model (registry-lock-only attach was the ratified Task 4.4 design); consequences are bounded (a started agent with an injected dir whose row then vanishes, self-correcting at the next stop/start) and single-operator CLI usage makes them theoretical today. Belongs to AI-63(b)/AD-17's replacement locking-model decision due before Epic 7, not to this story.

- source_spec: `5-1-attach-a-managed-filesystem-memory-backing`
  summary: SQLite migration steps are not crash-atomic — each SCHEMA_Vn batch runs before its `PRAGMA user_version` stamp, so a crash between them re-runs the batch on reopen and dies on "table already exists".
  evidence: Pre-existing pattern for V1→V4 (this story only followed it for V5); never observed in the wild because the batch+stamp window is milliseconds and desktop state DBs are small. Proper fix = wrap each step in BEGIN IMMEDIATE…COMMIT across ALL versions, one focused migration-hardening change.

- source_spec: `5-1-attach-a-managed-filesystem-memory-backing`
  summary: Semantic split between store and registry — `StateStore::upsert_memory_backing` documents REPLACE-on-re-attach (kind + timestamp overwritten) while `Registry::attach_memory` promises idempotent re-attach keeps the original timestamp and never changes kind; any future caller bypassing the registry guard can violate the A-6 invariant through sanctioned store behavior.
  evidence: Both behaviors are individually documented and tested; the invariant currently holds only because every caller goes through the registry. Hardening option: make the store reject kind-changes on an existing row (UNIQUE conflict → typed error) so the invariant lives below the registry too.

- source_spec: `5-1-attach-a-managed-filesystem-memory-backing`
  summary: Integration test helpers (fake-manifest writer, dump polling, tree snapshotting in tests/memory.rs) duplicate shapes already living in sibling integration files rather than a shared test-support utility.
  evidence: Same pattern grew per-file across registration/lifecycle/pause/interaction/logs/metering; each story copied the smallest shape it needed. Cost compounds across Epics 6–7 when manifest fixtures evolve (e.g. contract_version bumps touch N copies). Candidate: a `tests/support/` module (or `ktesio-conformance` test-fixture exports) once Epic 6's conformance kit forces the shape anyway.

## Deferred from: code review of 5-2-delegate-to-native-memory-with-an-explicit-boundary (2026-08-24)

- DC-3 detach/status wording not extended to name the delegation sentence — deferred to Epic 6's status surface: detach is kind-blind metadata removal and the story's ratified human surface is attach-only (NFR-7 sentences live in attach confirmations + docs).
- Reverse conflict direction (filesystem requested over an attached native backing) untested at both registry and CLI layers — one symmetric `!=` comparison; forward direction (native over filesystem) is covered at both. Candidate: a symmetry test with AI-63(b) work.

## Deferred from: one-shot blind-hunter pass on the round-1 triage fixes (2026-08-30)

- source_spec: `_bmad-output/implementation-artifacts/spec-6-2-round1-triage-fixes.md`
  summary: PATH save/restore in the hermes e2e test is not panic-safe — an assert between the shim install and teardown skips the restore.
  evidence: `crates/ktesio-engine/tests/hermes.rs` installs the PATH shim mid-test and restores it only as trailing teardown code; a RAII/Drop guard (like the existing `_shim` guard) would make the restore unconditional. The shim-install site predates this diff (blind-3 asked only for save/restore).
- source_spec: `_bmad-output/implementation-artifacts/spec-6-2-round1-triage-fixes.md`
  summary: hermes.rs module doc claims later tests start from a pristine PATH — true only on the happy path while the restore is not panic-safe.
  evidence: `crates/ktesio-engine/tests/hermes.rs:21-28` "restored at teardown so any test added to this binary later starts from a pristine environment" holds only if no test panics between install and restore; wording should soften or follow the RAII fix above.
- source_spec: `_bmad-output/implementation-artifacts/spec-6-2-round1-triage-fixes.md`
  summary: The engine-tests OS-cfg allowlist covers the whole `crates/ktesio-engine/tests/` directory while only `memory.rs:605` uses cfg — narrow the allowlist to the single file.
  evidence: `.github/workflows/ci.yml` allowlist entry whitelists the directory; the corrected comment (vg-o2) names memory.rs:605 as the sole user. Narrowing is a CI behavior change, out of the comment-only scope of vg-o2.
- source_spec: `_bmad-output/implementation-artifacts/spec-6-2-round1-triage-fixes.md`
  summary: No test pins that a manifest declaring kind `hermes` WITHOUT a `[lifecycle.start]` table yields NoStartTemplate with no builtin-table fallback.
  evidence: The new precedence test (blind-19) covers only manifest-with-start beating the builtin table; the no-start-table complement relies on pre-existing engine semantics in `resolve_start_launch` (`crates/ktesio-engine/src/adapter/mod.rs:275`).
- source_spec: `_bmad-output/implementation-artifacts/spec-6-2-round1-triage-fixes.md`
  summary: No user-facing doc that `model` is a silent no-op for the hermes kind (Decision 6) — discoverable only in code comments/tests.
  evidence: `docs/commands.md` hermes paragraph documents HERMES_HOME mapping but not the deliberately unmapped `model` key; pre-existing gap unrelated to the 13 triaged findings.

## Deferred from: code review of spec-6-3-govern-and-interact-with-hermes-end-to-end-uj-1-for-real (2026-08-31)

- source_spec: `_bmad-output/implementation-artifacts/spec-6-3-govern-and-interact-with-hermes-end-to-end-uj-1-for-real.md`
  summary: architecture.md:68 breach-record sentence's "(…; tokens only)" parenthetical is stale since story 3-3 — `BudgetBreachEvent` also carries `dimension` plus `dollar_limit`/`dollar_observed`/`estimate_label` on dollar breaches (event.rs:507-525).
  evidence: The rewritten Budget-enforcement paragraph kept the pre-existing parenthetical; the dollar fields shipped in story 3-3 and the sentence was out of this story's minimal-edit scope.
