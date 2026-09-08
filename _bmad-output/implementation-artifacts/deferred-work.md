# Deferred Work

<!-- Entry convention (epic-6 retro, item 8): `summary:` states the gap; `evidence:` says why it is
     real; `resolved: <what landed> <date>` is the LAST key of the entry, appended only when the fix
     ships (with the PR/commit ref). An entry WITHOUT a `resolved:` line is open — the file is swept
     at each retrospective. -->

Findings surfaced incidentally during quick-dev reviews that are out of scope for the triggering change. Collected for later focused attention.

## Resolution convention (adopted 2026-09-05, epic-6 retro action item 8 / issue #167)

When a deferred entry is fixed, its bullet gains a trailing marker line — `resolved: <ref> <date>` — naming the PR, commit, or change that resolved it and the date. Resolved entries STAY listed (the record of what was once deferred is part of the audit trail); a sweep greps for entries without a `resolved:` marker. Un-resolved entries have no marker, so "still open" is greppable as the absence of `resolved:` within the entry.

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
  resolved: PR #152 (docs/editorial) 2026-09-04 — the hermes paragraph now names `model` as a deliberately unmapped key.

## Deferred from: code review of spec-6-3-govern-and-interact-with-hermes-end-to-end-uj-1-for-real (2026-08-31)

- source_spec: `_bmad-output/implementation-artifacts/spec-6-3-govern-and-interact-with-hermes-end-to-end-uj-1-for-real.md`
  summary: architecture.md:68 breach-record sentence's "(…; tokens only)" parenthetical is stale since story 3-3 — `BudgetBreachEvent` also carries `dimension` plus `dollar_limit`/`dollar_observed`/`estimate_label` on dollar breaches (event.rs:507-525).
  evidence: The rewritten Budget-enforcement paragraph kept the pre-existing parenthetical; the dollar fields shipped in story 3-3 and the sentence was out of this story's minimal-edit scope.
  resolved: epic-6-retro remediation PR (docs hygiene batch, #165/D2) 2026-09-05 — the sentence now names `dimension` and the additive dollar fields.

- source_spec: `_bmad-output/implementation-artifacts/spec-6-4-prove-any-adapter-with-the-conformance-test-kit.md`
  summary: TCK polling helpers hard-code engine storage internals (usage_events/agent_instances SQLite schema, agents/<name>/logs/agent.log path, adapter.json snapshot layout) instead of public engine seams.
  evidence: Story 6-4 adversarial review (blind-hunter); coupling is intra-workspace and CI-tested today, but breaks silently if engine storage/layout changes. Switch to public engine APIs (e.g. read_agent_log) when available.
- source_spec: `_bmad-output/implementation-artifacts/spec-6-4-prove-any-adapter-with-the-conformance-test-kit.md`
  summary: TCK usage/observed row-count helpers conflate DB open/query errors with zero rows, producing misleading "have 0" timeout reasons instead of the actual error.
  evidence: Story 6-4 adversarial review (blind-hunter + edge-case-hunter); diagnostic polish only — the section still fails on timeout. Propagate Result through the helpers when touched next.
- source_spec: `_bmad-output/implementation-artifacts/spec-6-4-prove-any-adapter-with-the-conformance-test-kit.md`
  summary: All post-registration pre-section failures (tempdir, Engine::open, snapshot read) are mislabeled "registration failed" across all 8 report sections.
  evidence: Story 6-4 adversarial review (blind-hunter + edge-case-hunter EC1); needs a stage-differentiated error type plus new covered arms — deferred to avoid thinning the 95% coverage margin in this story.
- source_spec: `_bmad-output/implementation-artifacts/spec-6-4-prove-any-adapter-with-the-conformance-test-kit.md`
  summary: TCK harness is non-configurable — private 30s section timeout, fixed temp state root, no per-section selection — limiting third-party CI environments.
  evidence: Story 6-4 adversarial review (blind-hunter); API-surface design beyond story 6-4's captured intent; candidate for a future ergonomics story alongside the 6-6 contract freeze.

- source_spec: `_bmad-output/implementation-artifacts/spec-epic-6-retro-remediation.md` (epic-6 retro #163/B11 residual)
  summary: The subject-declared `memory.dir` delivery proof covers only the hermes builtin (an engine test over the shim `--dump` seam); a THIRD-PARTY manifest adapter's own declared delivery is still proven only on the TCK probe twin's env var — the memory section has no subject-delivery leg for manifest subjects.
  evidence: Retro remediation review (blind-hunter finding on the remediation diff, 2026-09-05): closing it generally needs a subject `--dump` seam convention for manifest adapters, which is a contract/docs decision (document the `--dump` proof convention) rather than test-only work.

## Deferred from: implementation of spec-7-2-subscribe-to-engine-events-with-stable-schemas (review triage, 2026-09-09)

- source_spec: `_bmad-output/implementation-artifacts/spec-7-2-subscribe-to-engine-events-with-stable-schemas.md`
  summary: Bus delivery is at-most-once in the crash window — a process crash between a durable event append and its bus publish loses that ONE event from the stream (the durable logs stay complete). A resync/gap-detection helper (e.g. a per-instance commit cursor a host could use to detect a gap and re-read through the query APIs) is DEFERRED, not built in-story.
  evidence: EC1-3 crash-window findings on the 7-2 review triage. Append-then-publish is the story's ratified order ("never publish what did not commit" — pinned by the VG1 append-failure silence tests); closing the window would need either publish-before-append (violates the committed-truth guarantee) or a durable per-subscriber cursor (a new persistence surface, inside the spec's Never list). The recourse is documented on the bus module and in docs/architecture.md: the query APIs (`transition_events`, `budget_breach_events`, ledger reads) return the committed truth regardless of any crash.

## Deferred from: implementation of spec-7-5-benchmark-the-performance-budgets (review triage, 2026-09-07)

- source_spec: `_bmad-output/implementation-artifacts/spec-7-5-benchmark-the-performance-budgets.md`
  summary: Trend tracking against prior runs (baseline-file comparison) — the perf harness prints one self-contained, gated report per run but nothing compares runs over time (e.g. a committed baseline JSON the harness diffs against, with a bounded regression delta and a documented update procedure).
  evidence: Each run's record is preserved (the CI perf-budgets job log + the uploaded JSON artifact, and local stdout), so the raw material exists; but detecting a slow multi-week drift (e.g. reads creeping from 6 ms toward 100 ms) is manual. A baseline file adds storage + churn questions (which platform's baseline is canonical; how updates are ratified) that deserve a focused change rather than riding this story's gate work.
