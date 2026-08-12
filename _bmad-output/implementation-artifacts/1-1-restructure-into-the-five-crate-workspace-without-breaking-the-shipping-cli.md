---
baseline_commit: fbfd90fd6eb7f2ea2788d19e067469479e95e06a
---

# Story 1.1: Restructure into the five-crate workspace without breaking the shipping CLI

Status: done

## Story

As the Ktesio maintainer,
I want the repo restructured into the spine's Cargo workspace (ktesio-engine, ktesio-adapter-api, ktesio-adapters-hermes, ktesio-conformance, kt) with the existing CLI code living in the kt crate,
so that every later story lands inside enforced boundaries while v0.5.0 behavior keeps shipping.

## Acceptance Criteria

1. **Given** the current single-crate repo, **when** the workspace restructure lands, **then** `cargo build --release` produces a `kt` binary whose existing commands (init/search/install/publish/upgrade/list/show/doctor/uninstall/self-update) behave exactly as v0.5.0 (integration suite green).
2. **And** the five workspace crates exist with `kt` depending only on `ktesio-engine`'s public API and `ktesio-adapter-api` types (AD-2).
3. **And** CI adds crate-visibility + semver-check jobs for ktesio-engine and ktesio-adapter-api and keeps `cargo fmt --check`, clippy `-D warnings`, `cargo test --all-targets`, and `cargo tarpaulin --fail-under 95` green (NFR-3).
4. **And** no NEW OS-conditional code exists outside `ktesio-engine::backends`, enforced by a CI grep gate; the two pre-existing v0.5.0 self-update files (`crates/kt/src/update_check.rs`, `crates/kt/src/cli/self_update.rs`) are explicitly grandfathered until epic 8 relocates/deprecates them (12 OS-conditional attributes as of 2026-07-02). (spine conventions; AC amended per code-review decision D3, ratified by Islam 2026-07-02)

## Tasks / Subtasks

- [x] Task 1: Create the workspace skeleton (AC: 2)
  - [x] Root `Cargo.toml` becomes `[workspace]` with `members = ["crates/kt", "crates/ktesio-engine", "crates/ktesio-adapter-api", "crates/ktesio-adapters-hermes", "crates/ktesio-conformance"]`, `resolver = "2"`
  - [x] Move `[lints.rust] unexpected_cfgs` (tarpaulin_include check-cfg) to `[workspace.lints.rust]`; each member sets `lints.workspace = true`
  - [x] Hoist shared dependency versions to `[workspace.dependencies]`; members reference with `workspace = true`
- [x] Task 2: Move the existing CLI crate wholesale (AC: 1)
  - [x] `git mv` `src/` → `crates/kt/src/` and `tests/` → `crates/kt/tests/` (preserve history)
  - [x] `crates/kt/Cargo.toml`: **package name stays `ktesio`** (crates.io continuity, FR-39), `[[bin]] name = "kt"`, version 0.5.0 carried, description/license/etc. carried from root
  - [x] No Rust source edits: module tree is self-contained (verified — `main.rs` declares all 12 modules; no external path deps)
- [x] Task 3: Create the four skeleton crates (AC: 2)
  - [x] `ktesio-engine` (lib): doc-comment-only `lib.rs` stating it is the Embedding Interface home; `kt` adds it as a dependency (unused-for-now import proves the edge compiles)
  - [x] `ktesio-adapter-api` (lib): doc-comment-only `lib.rs` (Adapter Contract home); `ktesio-engine` depends on it
  - [x] `ktesio-adapters-hermes`, `ktesio-conformance` (libs): doc-comment-only, depend on `ktesio-adapter-api`
  - [x] Keep skeletons free of executable lines so tarpaulin coverage is unaffected
- [x] Task 4: Update CI (AC: 3, 4)
  - [x] `ci.yml` test/coverage jobs: make workspace-explicit (`cargo test --workspace --all-targets`, `cargo tarpaulin --workspace --fail-under 95`) and verify fmt/clippy pick up all members
  - [x] Add boundary job: `cargo check -p ktesio` must compile with `ktesio-engine` as its ONLY internal dependency path (visibility boundary — enforced by crate graph, asserted by `cargo tree -p ktesio -e normal` grep: no `ktesio-adapters-*` edges)
  - [x] Add semver-check job for `ktesio-engine` + `ktesio-adapter-api` (cargo-semver-checks; NOTE: with no published baseline it passes trivially — wire it now so it bites at first publish; verify current action/install method at implementation time, WebSearch was unavailable at authoring)
  - [x] Add grep gate: `#[cfg(unix)]`/`#[cfg(windows)]`/`#[cfg(target_os` allowed only under `crates/ktesio-engine/src/backends/` (currently zero hits anywhere — gate starts green)
- [x] Task 5: Fix release automation for workspace reality (AC: 1 — regression guard)
  - [x] `release.yml` publish job: `cargo metadata --no-deps` + `packages[0]` **breaks in a workspace** (member order is unspecified) → select the package named `ktesio` explicitly; `cargo publish --locked` → `cargo publish --locked -p ktesio`
  - [x] Build job: `cargo build --release --target …` at workspace root builds all members — acceptable, or narrow with `-p ktesio`; binary path `target/<target>/release/kt` is unchanged by workspace move (shared target dir) — verify assumption holds
  - [x] Update `scripts/test_automation.py` workflow-expectation tests (`test_release_workflow_contains_expected_asset_and_release_steps`, `test_ci_runs_coverage_after_primary_gates`) to match the edited workflows — they assert on workflow file CONTENT and will fail otherwise
- [x] Task 6: Docs currency + verification (AC: 1, 3; NFR-7)
  - [x] Update `docs/architecture.md` + `docs/testing.md` + `CONTRIBUTING.md` if they reference `src/` paths or single-crate layout (docs currency gate)
  - [x] Run full local gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --all-targets`, `cargo tarpaulin --workspace --fail-under 95`, `python3 scripts/check_docs.py`, `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py`
  - [x] Manual smoke: `cargo run -p ktesio -- --help`, `kt list` in a fixture project

## Dev Notes

**Architecture bindings (spine, FINAL):**
- AD-2 fixes the crate set and the dependency law: `kt` → `ktesio-engine` public API only (+ `ktesio-adapter-api` types); engine → adapter-api; adapters/conformance → adapter-api. Never engine→kt, never engine→concrete adapter, never adapter→engine internals.
- AD-1 hexagonal: nothing in this story adds engine logic — skeletons stay empty. Do NOT pre-create `ports/`/`domain/` module trees; later stories create modules when they need them (entity-timing principle).
- Conventions: OS-conditional code only in `backends` (Task 4 grep gate); engine is sole path authority (no impact this story); errors stay `thiserror` in future engine code, `miette` remains kt-only — note `miette` therefore must NOT appear in skeleton crates' deps.

**Load-bearing repo facts (verified this session):**
- `src/main.rs` is a self-contained binary crate root: 12 `mod` declarations, no `lib.rs`, inline unit tests, `#[cfg(not(tarpaulin_include))]` on `main`/`run_cli`. Wholesale move is safe; zero import rewrites needed.
- `tests/helpers/mod.rs:107` invokes the binary via `env!("CARGO_BIN_EXE_kt")` — this env var is generated by Cargo for the crate that OWNS the bin target; tests must move WITH the crate (`crates/kt/tests/`), then it keeps working unchanged.
- Root `Cargo.toml` today: package `ktesio` v0.5.0, `[[bin]] name = "kt"`, `[lints.rust] unexpected_cfgs = { check-cfg = ['cfg(tarpaulin_include)'] }` — the lints table MUST survive (workspace.lints) or clippy `-D warnings` fails on every `tarpaulin_include` cfg.
- `release.yml:104-105` — the `packages[0]` single-package assumption (see Task 5); `release.yml:121` bare `cargo publish --locked`.
- `ci.yml` jobs: fmt/clippy/test/build/docs/coverage — coverage is `cargo tarpaulin --fail-under 95` (line 136); `docs` job runs `check_docs.py`, `generate_release_docs.py`, `test_automation.py`, docs-site build — none of these touch Rust layout except `test_automation.py`'s workflow-content assertions (Task 5).
- Existing labels/infra conventions: conventional commits enforced by changelog tooling — commit as `refactor:` or `chore:`; suggest `refactor: restructure into five-crate workspace (story 1-1)`.

**Publish-name decision (prevents an FR-39 disaster):** the crates.io package `ktesio` is the shipping artifact users `cargo install`. The bin crate keeps `name = "ktesio"` with bin `kt` in directory `crates/kt/`. The spine's crate list says "kt" — read that as the directory/colloquial name; package identity is governed by FR-39 continuity. The four new crates publish later (Story 7.4), so give them `publish = false` UNTIL that story to prevent accidental `cargo publish` from the workspace — EXCEPT note in each Cargo.toml a TODO referencing story 7-4.

**Version-verification gaps (classifier outage at authoring):** cargo-semver-checks current install/action and tarpaulin `--workspace` flag behavior were not web-verified — verify both at implementation (both are durable, widely-used; treat as low risk).

**What NOT to do (scope discipline):** no tokio, no rusqlite, no engine code, no adapter traits (those arrive 1.2–1.4); no README feature changes (docs currency = path/layout accuracy only); do not rename the GitHub repo, binary, or package.

### Project Structure Notes

Target layout after this story (structural seed, spine):

```text
ktesio/
  Cargo.toml            # [workspace] members, workspace.lints, workspace.dependencies
  crates/
    kt/                 # package "ktesio", [[bin]] kt — ALL existing src/ + tests/ moved here
    ktesio-engine/      # skeleton lib, doc-only
    ktesio-adapter-api/ # skeleton lib, doc-only
    ktesio-adapters-hermes/   # skeleton lib, doc-only
    ktesio-conformance/ # skeleton lib, doc-only
  scripts/ docs/ .github/     # unchanged locations
```

Variance from spine seed: spine shows `src/domain/ ports/ backends/ store/ metering/ skills/ events.rs` inside ktesio-engine — those are FUTURE modules (stories 1.2+); this story ships the empty lib intentionally.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.1] (ACs verbatim)
- [Source: _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md#AD-2, #Consistency Conventions, #Structural Seed]
- [Source: _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md#FR-32, #FR-39, #NFR-3, #NFR-7]
- [Source: src/main.rs (read in full), tests/helpers/mod.rs:107, Cargo.toml, .github/workflows/ci.yml:136, .github/workflows/release.yml:104-121, scripts/test_automation.py]

## Dev Agent Record

### Agent Model Used

claude-fable-5 (Claude Code dev-story run, 2026-07-02)

### Debug Log References

- Baseline (pre-restructure): `cargo test --all-targets` — 368 tests green (345 unit + 23 integration in 4 suites).
- Post-restructure gates (all at final state):
  - `cargo fmt --check` — PASS
  - `cargo clippy --workspace --all-targets -- -D warnings` — PASS (proves `workspace.lints` carries the `tarpaulin_include` check-cfg)
  - `cargo test --workspace --all-targets` — PASS: 368/368, 9 suites (4 skeleton suites empty by design)
  - `cargo tarpaulin --workspace --fail-under 95` — PASS: **95.93%** (2331/2430 lines); skeleton crates contribute zero coverable lines; `--workspace` flag verified working (tarpaulin 0.35.4)
  - `python3 scripts/check_docs.py` — PASS (23 Markdown files)
  - `python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test` — PASS
  - `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` — PASS: 18/18 (17 existing + 1 new)
- Manual smoke: `cargo run -p ktesio -- --help` (all 10 commands listed, v0.5.0 footer), `target/release/kt --version` → `kt 0.5.0`, `kt list` in the repo fixture → renders installed skill table.
- Boundary evidence: `cargo tree -p ktesio -e normal` → only internal edges are `ktesio-engine → ktesio-adapter-api`; no adapters/conformance edges. Release binary lands at `target/release/kt` (shared workspace target dir — release.yml path assumption verified).
- Review fix pass (2026-07-03, patches P1–P6 + D3):
  - `cargo fmt --all --check` — PASS
  - `cargo clippy --workspace --all-targets -- -D warnings` — PASS
  - `cargo test --workspace --all-targets` — PASS: 368/368 (345 unit + 23 integration), 9 suites
  - `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` — PASS: 18/18 (boundary/semver assertions updated to the P2/P3/P4-edited ci.yml)
  - `python3 scripts/check_docs.py` — PASS (23 Markdown files)
  - kt-release dry-run (P1 verify): hermetic sim (working-tree clone on `main`, local bare origin, v0.5.0 tag + 1 unreleased fix commit) → exit 0, inferred patch → v0.5.1. On the live feature branch the script now correctly refuses at the branch guard instead of crashing in `cargo_package`. `replace_cargo_version` unit-checked: rewrites exactly `[workspace.package] version`, leaves `[workspace.dependencies]` inline versions untouched, raises without `[workspace.package]`.
  - CI gate sims (exact step bodies from edited ci.yml): boundary gate green (internal edges exactly ktesio-engine/ktesio-adapter-api under `-e normal,build --all-features`); OS-cfg gate green with exactly the 12 grandfathered hits (update_check.rs ×7: lines 124,131,141,517,521,523,528; self_update.rs ×5: lines 401,417,536,548,1064). Negative tests: gate exits 1 on a probe `#[cfg(windows)]` outside backends; boundary filter flags a fabricated `ktesio-adapters-hermes` edge; ci.yml YAML parse OK.
  - Tarpaulin skipped by contract: fix pass touches zero `.rs` files (audited via `git status -- '*.rs'`).

### Completion Notes List

- **Workspace**: root `Cargo.toml` is now a virtual `[workspace]` manifest (resolver 2, five members), with `[workspace.package]` (version 0.5.0, edition, license, homepage, repository), `[workspace.lints.rust]` (the `tarpaulin_include` check-cfg moved here), and `[workspace.dependencies]` hoisting all shared versions. All members set `lints.workspace = true` and reference deps with `workspace = true`.
- **CLI move**: `src/` and `tests/` moved wholesale to `crates/kt/` via `git mv` (git reports R/rename status — history preserved). Package name stays `ktesio` (FR-39), `[[bin]] name = "kt"`, version 0.5.0. `readme = "../../README.md"` keeps README in the published crate (verified via `cargo package --list`). The only Rust source edit anywhere: `use ktesio_engine as _;` added to `main.rs` to prove the AD-2 edge compiles (non-executable, warning-free).
- **Skeletons**: four `publish = false` doc-comment-only lib crates at version 0.1.0, each with a `TODO(story 7-4)` note; edges per AD-2 (engine → adapter-api; hermes/conformance → adapter-api); no `miette`, no executable lines.
- **CI (ci.yml)**: test/coverage/clippy made workspace-explicit; fmt made `--all`-explicit; new `boundary` job (`cargo check -p ktesio`, `cargo tree` grep forbidding `ktesio-(adapters-hermes|conformance)` edges, OS-conditional-cfg grep gate); new `semver` job (cargo-semver-checks via `cargo install --locked`, guarded by a crates.io existence check per crate); `coverage.needs` extended with `boundary, semver`.
- **Release (release.yml)**: publish job selects package `ktesio` explicitly (the `packages[0]` workspace trap is gone) and publishes with `cargo publish --locked -p ktesio`. Build job intentionally left building the whole workspace (story-blessed; also compiles skeletons on all 4 release targets).
- **Automation tests**: `test_release_workflow_contains_expected_asset_and_release_steps` and `test_ci_runs_coverage_after_primary_gates` updated to the edited workflow content; new `test_ci_enforces_workspace_boundary_and_semver_gates` locks the new CI jobs in place.
- **Docs currency**: `docs/architecture.md` (workspace layout section + module tree now rooted at `crates/kt/src/`, plus two previously missing modules `install_channel.rs`/`update_check.rs` documented), `docs/contributing.md`, `docs/testing.md`, `CONTRIBUTING.md`, `AGENTS.md` all updated to workspace-explicit commands/paths.

**Deviations from story plan (with reasons):**

1. **Semver job design** — story note said "with no published baseline it passes trivially". Web-verified at implementation (story asked for this): cargo-semver-checks in fact FAILS without a baseline (unpublished crate + default crates.io baseline → error, not trivial pass). Implemented instead as a guarded job: curl the crates.io API per crate (same pattern release.yml already uses); 404 → skip with `::notice` (arms automatically at first publish, story 7-4); 200 → run `cargo semver-checks check-release -p <crate>`. Tool installed via `cargo install cargo-semver-checks --locked` (repo convention, mirrors tarpaulin) instead of the third-party `obi1kenobi/cargo-semver-checks-action@v2` — this repo SHA-pins all actions and a new unverifiable SHA pin was the worse trade.
2. **OS-cfg grep gate needed an allowlist** — story claimed "currently zero hits anywhere — gate starts green". FALSE: 7 pre-existing hits in `crates/kt/src/update_check.rs` (platform cache dirs) and `crates/kt/src/cli/self_update.rs` (Windows PATHEXT resolution) — shipping v0.5.0 code this story must not rewrite (AC-1 "exactly as v0.5.0"). Gate implemented strict for all NEW code with an explicit, annotated two-file allowlist referencing the epic-8 relocation/deprecation. ASSUMPTION: grandfathering beats rewriting shipping self-update code in a restructure story; flag for review.
3. **`docs/contributing.md` and `AGENTS.md` also updated** — not named in Task 6, but they carry the same stale single-crate commands/paths (docs-currency gate class).
4. **Boundary grep also forbids `ktesio-conformance`** — story text said "no ktesio-adapters-* edges"; AD-2's law equally forbids a kt→conformance edge, so the gate covers both.

**Assumptions (tagged per continuous-loop mode):**

- Skeleton crates start at version **0.1.0** (not the workspace 0.5.0): they are unpublished pre-stability crates and `ktesio-adapter-api` is independently semver'd per the spine; story 7-4 assigns publish versions. `workspace.package.version = 0.5.0` is inherited only by the `ktesio` (kt) package.
- `coverage.needs` extended to include the new `boundary` and `semver` jobs (coverage stays the last gate; test_automation assertion updated to match).
- Legacy OS-cfg allowlist entries live in the CI grep gate (not code changes), keeping the shipping binary byte-equivalent in behavior.

**Open questions for Islam (non-blocking, flagged):**

1. **crates.io publish of `ktesio` is BLOCKED until story 7-4** — hard-verified: `cargo package -p ktesio` fails with "no matching package named `ktesio-engine` found (crates.io index)". The story-mandated kt→engine dependency edge means any release tag pushed before engine/adapter-api publish will fail the release.yml publish job (loudly, at the publish step; GitHub release assets are built in a separate job and unaffected up to that point). If a CLI release must ship before 7-4: either publish `ktesio-engine`+`ktesio-adapter-api` early or temporarily drop the edge in that release PR. Needs a release-planning decision.
2. **LICENSE text no longer embedded in the published .crate** (auto-inclusion only applied while the package root was the repo root; `license = "PolyForm-Noncommercial-1.0.0"` SPDX metadata is unchanged). Cosmetic for crates.io; fix at 7-4 via `include`/`license-file` if wanted.
3. Ratify the two grandfathered OS-cfg legacy files (deviation 2), or schedule their relocation earlier than epic 8.

**Review fix pass (2026-07-03) — approved patches P1–P6 + ratified AC amendment D3, nothing else:**

- **P1 (HIGH)**: `.agents/skills/kt-release/scripts/prepare_kt_release.py` fixed for the virtual workspace manifest — `cargo_package()` now takes the repo root, reads the package name from `crates/kt/Cargo.toml`, and resolves `version.workspace = true` through root `[workspace.package]`; `replace_cargo_version()` rewrites `[workspace.package] version` in the root manifest; `run_checks()` made workspace-explicit; SKILL.md helper-script docs updated to match. Verified via hermetic dry-run sim (exit 0).
- **P2 (HIGH)**: CI OS-cfg gate pattern broadened to the class pattern `cfg[!(]?.*(unix|windows|target_os|target_family)` (catches `all(unix, …)`, `not(windows)`, `any(…)`, `cfg!(…)`, `cfg_attr` forms); fail-open `|| true` replaced with explicit grep exit-status discrimination (1 = no match = OK, ≥2 = grep error = job fails).
- **P3 (MEDIUM)**: boundary gate flipped blocklist → allowlist — any `ktesio-*` token in the `cargo tree -p ktesio` output other than exactly `ktesio-engine`/`ktesio-adapter-api` fails, so future internal crates are caught automatically; widened to `-e normal,build --all-features` (verified green locally).
- **P4 (MEDIUM)**: semver job — `cargo install cargo-semver-checks --locked` moved inside the armed (200) branch behind a `command -v` guard (skips the ~10-min install while the gate is dormant); curl hardened with `--max-time 30 --retry 3` and `|| status=000`; transient statuses (000, 429, 5xx) skip with `::notice`; 404 → notice and other 4xx → error unchanged.
- **P5 (LOW)**: workspace-explicit commands in `.github/pull_request_template.md` and kt-release `SKILL.md`; `cargo fmt --check` → `cargo fmt --all --check` in `AGENTS.md`, `CONTRIBUTING.md`, `docs/testing.md`; `.github/CODEOWNERS` dead `/src/` + `/tests/` rules replaced with `/crates/`. `docs/github-repository-audit-checklist.md` deliberately untouched (historical record).
- **P6 (LOW)**: `.vscode/` added to `.gitignore` alongside `.idea/` (directory contents NOT added to git).
- **D3 (AC amendment, ratified by Islam 2026-07-02 in code review)**: AC-4 amended in this story file AND `epics.md` Story 1.1 — now requires no NEW OS-conditional code outside `ktesio-engine::backends` (CI grep gate), with the two pre-existing v0.5.0 self-update files (`crates/kt/src/update_check.rs`, `crates/kt/src/cli/self_update.rs`) explicitly grandfathered until epic 8 relocates/deprecates them (12 OS-conditional attributes as of 2026-07-02). This resolves open question 3 above — the grandfather is ratified.
- `scripts/test_automation.py` boundary/semver assertions updated in the same pass to lock the edited ci.yml (allowlist regex, class pattern, lazy install, transient-status handling).

### File List

Moved (28, `git mv`, history preserved — R status):

- `src/main.rs` → `crates/kt/src/main.rs` (also modified: added `use ktesio_engine as _;` edge proof)
- `src/cli/{mod,doctor,init,install,list,publish,search,self_update,show,uninstall,upgrade}.rs` → `crates/kt/src/cli/…` (11 files)
- `src/{discovery,error,git,install_channel,install_target,lockfile,manifest,skill,skills_sh,ui,update_check}.rs` → `crates/kt/src/…` (11 files)
- `tests/{adoption_cli,install_default,install_fallback,publish}.rs`, `tests/helpers/mod.rs` → `crates/kt/tests/…` (5 files)

Created (9):

- `crates/kt/Cargo.toml`
- `crates/ktesio-engine/Cargo.toml`, `crates/ktesio-engine/src/lib.rs`
- `crates/ktesio-adapter-api/Cargo.toml`, `crates/ktesio-adapter-api/src/lib.rs`
- `crates/ktesio-adapters-hermes/Cargo.toml`, `crates/ktesio-adapters-hermes/src/lib.rs`
- `crates/ktesio-conformance/Cargo.toml`, `crates/ktesio-conformance/src/lib.rs`

Modified (10):

- `Cargo.toml` (single-crate package → virtual workspace manifest)
- `Cargo.lock` (regenerated for workspace members)
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `scripts/test_automation.py`
- `docs/architecture.md`
- `docs/contributing.md`
- `docs/testing.md`
- `CONTRIBUTING.md`
- `AGENTS.md`

Modified in review fix pass (2026-07-03, P1–P6 + D3):

- `.agents/skills/kt-release/scripts/prepare_kt_release.py` (P1)
- `.agents/skills/kt-release/SKILL.md` (P1/P5)
- `.github/workflows/ci.yml` (P2/P3/P4)
- `scripts/test_automation.py` (assertions locked to edited ci.yml)
- `.github/pull_request_template.md` (P5)
- `.github/CODEOWNERS` (P5)
- `AGENTS.md`, `CONTRIBUTING.md`, `docs/testing.md` (P5 fmt-command alignment)
- `.gitignore` (P6)
- `_bmad-output/planning-artifacts/epics.md` and this story file (D3 AC-4 amendment; gitignored planning artifacts, not part of the repo diff)

## Change Log

- 2026-07-02: Story 1.1 implemented (workspace restructure, five crates, CI boundary/semver/os-cfg gates, release automation workspace fixes, docs currency). All gates green: 368/368 tests, clippy `-D warnings` clean, tarpaulin 95.93% (≥95), check_docs 23 files, test_automation 18/18. Status → review. (claude-fable-5)
- 2026-07-03: Review fix pass — applied approved code-review patches P1–P6 (kt-release script fixed for the virtual workspace manifest + dry-run verified; OS-cfg CI gate broadened to the compound-cfg class pattern with fail-closed grep exit handling; boundary gate blocklist→allowlist incl. build edges and all-features; semver job lazy tool install + curl resilience + transient-status skip; workspace-explicit command cleanup in PR template/SKILL.md/AGENTS.md/CONTRIBUTING.md/docs-testing/CODEOWNERS; `.vscode/` gitignored) and ratified AC amendment D3 (AC-4 now grandfathers the two v0.5.0 self-update files until epic 8 — ratified by Islam 2026-07-02; amended here and in epics.md). Gates re-run green: fmt/clippy clean, 368/368 tests, test_automation 18/18, check_docs 23 files, both CI gate sims green with exactly the 12 grandfathered OS-cfg hits, kt-release dry-run sim exit 0. Zero Rust source changes (tarpaulin skip per contract; 95.93% stands from 2026-07-02). Status stays review. (claude-fable-5)
