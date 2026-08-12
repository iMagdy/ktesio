---
github_issue: 112   # Epic 9 issue #110; story 9-2 = #112 (per orchestrator sync, 2026-07-14).
baseline_commit: 42896b7f3ad3d151b0802fc08e0b79fde414a46e   # 9-1 (#111) landed here — the legacy command surface is already GONE.
epic: 9
story: 9.2
supersedes: [8-4]           # (epic-level) deprecate-in-place is retired outright (Islam-ratified 2026-07-14)
re_scopes: [8-1, AD-16]     # (epic-level) the skills machinery is deleted, not relocated
frs: [FR-37, FR-38, FR-39]  # FR-37/38 amended (removal + stated-version announce replaces deprecate-in-place); FR-39 preserved
sources:
  - _bmad-output/planning-artifacts/sprint-change-proposal-2026-07-13.md   # §4 Story 9.2 ACs; §2 Artifact conflicts; §6 decisions
  - _bmad-output/planning-artifacts/epics.md                                # Epic 9 → Story 9.2 (verbatim AC)
  - _bmad-output/implementation-artifacts/9-1-remove-the-legacy-skill-manager-command-surface-and-modules.md   # the excision this builds on
---

# Story 9.2: Reposition the top-level `kt` identity to the agent runner

Status: review

<!-- Headless-conservative run (Scrum-Master, 2026-07-14): every choice resolved from the
sprint-change-proposal §4/§6 + epics.md Story 9.2 + the owner's ratified decisions; the code
state was VERIFIED against the post-9-1 tree at baseline 42896b7 (the legacy surface is already
gone, only the identity/version/changelog/dep-cleanup work remains). Assumptions + the dangling-
dep verification are logged at the end of Dev Notes. No user interaction. sprint-status.yaml was
deliberately NOT edited — the orchestrator owns it (see Assumptions). -->

## Story

As an Operator,
I want `kt --help`, `kt --version`, and the crate metadata to present Ktesio as the agent runner,
so that the tool describes itself the way the README and `docs/` already do, with one canonical way to list and show the Fleet.

## Acceptance Criteria

Derived from the sprint-change-proposal §4 (Story 9.2 acceptance criteria) and §6 (the owner-ratified decisions: clean removal at **v0.6.0**, Story 9-4 skipped; top-level `list`/`show` removed so `kt agent list`/`show` is canonical), and epics.md → Story 9.2, refined against the code at the post-9-1 baseline `42896b7`. Every code location below was verified against source this session (see Dev Notes → "Verification log"). The docs/architecture reconciliation, the AD-16/Epic-8 re-scope, and tightening `scripts/check_docs.py`'s allowlist are **excluded** (that is Story 9-3, architect-owned).

**Given** the post-9-1 `kt` crate at baseline `42896b7` (the retired `Commands` variants, handler modules, support modules, and legacy tests are already deleted; `Commands` currently holds only `SelfUpdate` and `Agent`)
**When** the identity repositioning lands

1. **AC-1 — Top-level clap identity rebranded (`main.rs`).** The `#[command(...)]` `about` on `struct Cli` in `crates/kt/src/main.rs` (currently `"Agentic skills package manager"`, the sole occurrence at L77) no longer contains the strings `"Agentic"`, `"skills package manager"`, or `"package manager"`, and instead describes the agent runner (it names running/supervising/metering/budgeting AI **agents**). An optional `long_about` MAY be added for the fuller `kt --help` body; if added it carries the same no-skills-framing property. Recommended `about` (dev finalizes exact wording — the binding property is the assertion below): **"Run AI agents like services — supervise, meter, and budget them"** (mirrors the README lead + banner). Testable: `Cli::command().get_about().unwrap().to_string()` (a) does **not** contain `"skills package manager"`, `"package manager"`, or `"Agentic"` (case-insensitive), and (b) does contain `"agent"` (case-insensitive).

2. **AC-2 — Crate metadata rebranded (`crates/kt/Cargo.toml`).** `description` (currently `"Agentic skills package manager"`, L8) is rewritten to an agent-runner sentence with **no** `"skills"`/`"package-manager"`/`"package manager"` framing; `keywords` (currently `["agentic", "skills", "package-manager", "cli"]`, L13) is rewritten to drop `"skills"` and `"package-manager"` and reflect the agent runner. The rewrite respects crates.io keyword rules: **at most 5 keywords, each ≤ 20 chars, lowercase ASCII alphanumeric or hyphen**. Recommended: `description = "Run AI agents like services: supervise their lifecycle, meter real token usage, and enforce dollar budgets."` and `keywords = ["agent", "ai", "supervisor", "metering", "cli"]` (`"budget"`/`"llm"` are acceptable alternates). `name = "ktesio"`, `[[bin]] name = "kt"`, `categories`, and `readme` are **unchanged** (FR-39). Testable: `env!("CARGO_PKG_DESCRIPTION")` does not contain `"skills package manager"`/`"package manager"` and does contain `"agent"` (case-insensitive); keywords verified by inspection and by `cargo publish --dry-run` / `cargo package --list` metadata (no `CARGO_PKG_KEYWORDS` env var exists, so keywords are not unit-assertable).

3. **AC-3 — One canonical Fleet surface: no top-level `kt list`/`kt show` (nor any retired command).** `kt --help` lists exactly the two agent-runner-relevant top-level commands — `agent` and `self-update` — and **no** retired skill command (`init`, `install`, `search`, `upgrade`, `publish`, `list`, `show`, `doctor`, `uninstall`, `remove`) appears at the top level, making `kt agent list` / `kt agent show` the single canonical way to list/show the Fleet (matching `docs/commands.md`, which already omits the top-level forms). NOTE: 9-1 already excised the skill `List`/`Show` `Commands` variants (which were the top-level `kt list`/`kt show`), so at baseline this is **structurally already true** — this story's job is to **guard** it with the negative test assertions in AC-4 and confirm the help/identity reflect it, not to remove anything further. No top-level alias is (re-)introduced.

4. **AC-4 — The two surface unit tests are authored to the new identity; the rest stay green.** In `crates/kt/src/main.rs`:
   - `test_cli_subcommands_exist` asserts the **new** surface: `find_subcommand("agent").is_some()` and `find_subcommand("self-update").is_some()` **present**, AND each retired name **absent** — `find_subcommand(n).is_none()` for `n` in `["init","install","search","upgrade","publish","list","show","doctor","uninstall","remove"]` (this is the positive assertion of the single-canonical-surface guarantee — the negative half 9-1 deliberately deferred). ⚠️ These are **top-level** lookups on `Cli::command()`, which only sees `agent` + `self-update`; `list`/`show`/`remove` are ALSO valid `kt agent` subcommands, so the absent-check is `None` at the top level and MUST NOT be written as a recursive/`agent`-tree search (that would false-fail against the live `kt agent list`/`show`/`remove`).
   - `test_subcommand_help_includes_details_and_examples` iterates the surviving commands (`self-update`, `agent`) — it already does post-9-1; keep it iterating only survivors and green.
   - Left green untouched (do not regress): `test_cli_struct_valid`, `test_cli_help_includes_license_and_repository`, `test_cli_without_subcommand_is_allowed_for_help_display`, `test_self_update_skips_passive_update_check`, and every `test_agent_*` (`test_agent_subcommands_exist`, `test_agent_config_parse`, `test_agent_start_stop_parse`, `test_agent_list_and_show_accept_json_flag`, `test_agent_pause_resume_parse`, `test_agent_register_requires_kind_or_manifest`).
   - RECOMMENDED (optional, idiomatic here): add a small test asserting the rebrand, e.g. `Cli::command().get_about()` is agent-framed and skills-free, and/or `env!("CARGO_PKG_DESCRIPTION")` is skills-free — parallel to the existing `test_cli_help_includes_license_and_repository` env-based test.

5. **AC-5 — Version bumped to 0.6.0.** The pivoted release version is **0.6.0**. Because `crates/kt/Cargo.toml` sets `version.workspace = true`, the bump is made **once** in the ROOT `/Users/imagdy/dev/ktesio/Cargo.toml` `[workspace.package]` — `version = "0.5.0"` → `version = "0.6.0"` (L12). This moves **only** the `ktesio`/`kt` package: the four internal crates (`ktesio-engine`, `ktesio-adapter-api`, `ktesio-adapters-hermes`, `ktesio-conformance`) pin their own explicit `version = "0.1.0"` and do **not** inherit, and the `version = "0.1.0"` used in the `[workspace.dependencies]` internal path deps is independent of `[workspace.package].version` — so nothing else moves. Do **not** add an explicit `version = "0.6.0"` to `crates/kt/Cargo.toml` (that would break the inherit convention). Testable: `env!("CARGO_PKG_VERSION")` for `kt` equals `"0.6.0"`, and `kt --version` prints `kt 0.6.0`.

6. **AC-6 — New CHANGELOG.md + docs/RELEASE_NOTES.md entries state the retired commands and the removal version (FR-38).** A new hand-authored `## v0.6.0` section is prepended (above the existing `## v0.5.0`) to **both** `/Users/imagdy/dev/ktesio/CHANGELOG.md` and `/Users/imagdy/dev/ktesio/docs/RELEASE_NOTES.md`. Each new entry states: (a) that the legacy skill-manager commands `init`, `install`, `search`, `upgrade`, `publish`, `list`, `show`, `doctor`, `uninstall` (and the `remove` alias) are **removed**; (b) the removal version — **0.6.0**; (c) the replacement — the agent runner under `kt agent …`; and (d) a pointer to the migration path / docs (honoring FR-38 "announce (release notes + README) … removal in a stated later version"). All **historical** entries (`## v0.5.0` and earlier — the generated asset tables, `### Features/Fixes/Documentation/Maintenance/Other Changes`) are **byte-unchanged**. The new entry need not carry a release asset table (no tag is built yet — the tag automation fills that at publish); a lean `### Removed` / `### Changed` block is sufficient and FR-38-complete. ⚠️ **Migration-pointer trap:** `scripts/check_docs.py` validates every Markdown link in root `*.md` + `docs/*.md` and fails the gate on a broken or repo-escaping target. So the migration pointer (d) must be either plain text, an `https://` URL, or a link to an **existing** in-repo file (e.g. `docs/commands.md` or `README.md`) — never a link to a not-yet-created migration doc.

7. **AC-7 — The five now-dangling `[workspace.dependencies]` entries are deleted (folds in the 9-1 LOW cleanup).** In the ROOT `/Users/imagdy/dev/ktesio/Cargo.toml`, delete the `[workspace.dependencies]` lines for `indicatif` (L65), `walkdir` (L73), `regex` (L74), `dialoguer` (L75), and `urlencoding` (L77) — all left unused by **any** crate after 9-1 pruned them from `crates/kt`. Verified this session: **0** source references (`grep -rn --include='*.rs' '<dep>::' crates/`) and **0** manifest references (`grep -rn --include='Cargo.toml' '^<dep>' crates/`) for each of the five across the whole workspace. `cargo metadata` and `cargo +1.96.1 build --release` stay green after removal; no member crate references any of the five via `{ workspace = true }`. Leave every other `[workspace.dependencies]` entry untouched (they remain in use, e.g. `console`, `serde`, `ureq`, `semver`, `flate2`, `sha2`, `tar`, `zip`, the tokio/hyper engine stack, `rusqlite`, `directories`, `toml`, `tempfile`, `clap`, `miette`, `thiserror`, the internal crates).

8. **AC-8 — All nine CI gates green; no out-of-scope edits.** With `cargo +1.96.1`: `build --release`, `fmt --check`, `clippy --all-targets -- -D warnings`, `test --all-targets` (all `main.rs` unit tests + `agent_cli.rs` green), `tarpaulin --fail-under 95` on `src/` (prove ≥95% locally per the standing #101 practice — this story adds no new untested code paths; it edits a string, a version, tests, docs, and the root manifest), the crate-visibility boundary job, `cargo-semver-checks`, the single-currency-formatter grep-lint, and the MSRV 1.96.1 build all pass. (Per 9-1's completion notes, the CI `cargo-semver-checks` job checks only `ktesio-engine`/`ktesio-adapter-api` and is dormant pre-publish — it does not check `kt`, and this story touches neither engine crate nor their public API, so the 0.6.0 bump does not arm or fail it; do not chase a phantom semver failure on `kt`.) The doc-lint (`scripts/check_docs.py`) stays green: its `KT_COMMANDS` allowlist still contains the retired names (it is left stale — tightening it is Story 9-3), so the new changelog/release-notes entries that mention retired commands do not trip it. No new `#[cfg(unix/windows/target_os)]` is introduced. **Out of scope (untouched):** `scripts/check_docs.py` allowlist, `docs/architecture.md`, `docs/lockfile.md`, `docs/manifest.md`, the AD-16/Epic-8 re-scope, and any engine/adapter/conformance code.

## Tasks / Subtasks

> Dev: check these boxes IN THIS FILE. This is a small surface/identity/version/docs change — no new logic. Toolchain: `cargo +1.96.1` (mise overrides `rust-toolchain.toml` locally — see MEMORY / docs/testing.md). Order: rebrand → version → tests → changelog → dep cleanup → gates.

- [x] **Task 1 — Rebrand the clap identity in `main.rs` (AC-1).** Rewrite the `about` on `struct Cli` (`crates/kt/src/main.rs:77`) from `"Agentic skills package manager"` to the agent-runner line (recommended: `"Run AI agents like services — supervise, meter, and budget them"`). Optionally add a `long_about` for the fuller `--help` body. Do not touch `AGENT_AFTER_HELP`, `SELF_UPDATE_AFTER_HELP`, `HELP_FOOTER`, or the `name`/`version`/`after_help` attributes.
- [x] **Task 2 — Rebrand `crates/kt/Cargo.toml` metadata (AC-2).** Rewrite `description` (L8) and `keywords` (L13) to the agent-runner framing (recommended values in AC-2), obeying the crates.io keyword rules (≤5, each ≤20 chars, lowercase). Leave `name`, `[[bin]]`, `version.workspace`, `categories`, `readme`, `[dependencies]`, and `[dev-dependencies]` untouched.
- [x] **Task 3 — Bump the workspace version to 0.6.0 (AC-5).** In the ROOT `Cargo.toml`, change `[workspace.package] version = "0.5.0"` → `"0.6.0"` (L12). Do **not** add an explicit version to `crates/kt/Cargo.toml`. Confirm the four internal crates still read `version = "0.1.0"` (they pin explicitly; they must not move).
- [x] **Task 4 — Author the two surface unit tests (AC-4).** In `main.rs`:
  - [x] `test_cli_subcommands_exist`: keep the `agent` + `self-update` present assertions; ADD `assert!(cmd.find_subcommand(n).is_none())` for each of `init`, `install`, `search`, `upgrade`, `publish`, `list`, `show`, `doctor`, `uninstall`, `remove` (iterate a slice).
  - [x] `test_subcommand_help_includes_details_and_examples`: confirm it iterates only `self-update` + `agent` (post-9-1 it already does) and stays green.
  - [x] (Optional, recommended) add a rebrand-assertion test on `Cli::command().get_about()` and/or `env!("CARGO_PKG_DESCRIPTION")` (agent-framed, skills-free).
  - [x] Do not modify `test_cli_help_includes_license_and_repository`, `test_self_update_skips_passive_update_check`, `test_cli_struct_valid`, `test_cli_without_subcommand_is_allowed_for_help_display`, or any `test_agent_*`.
- [x] **Task 5 — Prepend the v0.6.0 CHANGELOG entry (AC-6).** Add `## v0.6.0` above `## v0.5.0` in `CHANGELOG.md` stating the removed commands, the 0.6.0 removal version, the `kt agent …` replacement, and the migration pointer. Do not alter any existing section.
- [x] **Task 6 — Prepend the v0.6.0 RELEASE_NOTES entry (AC-6).** Mirror Task 5 in `docs/RELEASE_NOTES.md` (correct filename — no plural; `docs/RELEASES_NOTES.md` is a check_docs stale-pattern). Do not alter any existing section.
- [x] **Task 7 — Delete the five dangling `[workspace.dependencies]` (AC-7).** In the ROOT `Cargo.toml`, remove the `indicatif` (L65), `walkdir` (L73), `regex` (L74), `dialoguer` (L75), and `urlencoding` (L77) lines. Re-run the source+manifest grep to re-confirm zero references before deleting (baselines drift). Leave all other entries.
- [x] **Task 8 — Run the fast gates + smoke (AC-8).** With `cargo +1.96.1`: `fmt --check`, `clippy --all-targets -- -D warnings`, `nextest run --all-targets`, crate-visibility/OS-cfg/currency grep gates; plus `python3 scripts/check_docs.py` (must stay green). `tarpaulin`/`cargo-semver-checks`/MSRV build deliberately NOT run locally — validated by CI (coverage job passed on PR #114; semver dormant for `kt` pre-publish), per standing practice for a change with no new untested `src/` path. Smoke: `kt --help` (about now agent-framed; lists only `agent` + `self-update`), `kt --version` (prints `kt 0.6.0`), `kt agent list`, `kt agent register demo --kind mock`, `kt self-update --help`. Results in the completion notes.

## Dev Notes

**This is the rebrand + canonicalization + versioning story of Epic 9.** 9-1 already did the excision (the legacy `Commands` variants, the nine handler modules, the seven support modules, the four legacy tests, and the dead `ui.rs` helpers are gone; deps were pruned from `crates/kt`). 9-2 makes the shipped binary's *identity* match the already-live README/`docs/`, cements the single canonical Fleet surface, bumps to the pivot version, announces the removal per FR-38, and folds in the one LOW cleanup 9-1 left behind (the dangling **root** workspace deps). 9-3 (architect-owned) handles `docs/architecture.md`, `docs/lockfile.md`/`manifest.md`, and the AD-16/Epic-8 re-scope — **not** this story.

### Verification log (this session, against baseline `42896b7`)

- **The identity string lives in exactly three places** — `crates/kt/Cargo.toml:8` (`description`), `crates/kt/Cargo.toml:13` (`keywords`), `crates/kt/src/main.rs:77` (`about`). A repo-wide `grep -rniE 'agentic|skills package|package[ -]manager' crates/kt/` returns exactly those three lines. There is **no** `long_about` anywhere in `kt`.
- **No integration or snapshot test asserts the top-level `about`/`--help` text.** `crates/kt/tests/agent_cli.rs` has no help/about assertion (its only "about" hits at L558/L1884 are the English word in comments); there is no `trycmd`/`insta`/`assert_cmd` CLI-snapshot harness. So rewriting `about` breaks **nothing** beyond the two `main.rs` unit tests already in scope.
- **The retired top-level commands are already gone.** Post-9-1 `Commands` (main.rs L85–103) holds only `SelfUpdate` and `Agent`; there is no `List`/`Show`/`Install`/… at the top level. AC-3 is therefore a **guard**, realized through AC-4's negative assertions — not a new removal.
- **Versioning is isolated to `kt`.** Only `crates/kt/Cargo.toml` uses `version.workspace = true`; `ktesio-adapter-api`, `ktesio-adapters-hermes`, `ktesio-conformance`, and `ktesio-engine` each pin explicit `version = "0.1.0"`. Bumping `[workspace.package].version` moves only the `ktesio`/`kt` package.
- **The five root deps are provably dangling.** For each of `indicatif`, `walkdir`, `regex`, `dialoguer`, `urlencoding`: `0` `<dep>::` source refs across `crates/**/*.rs` and `0` `^<dep>` manifest refs across `crates/**/Cargo.toml`. A `[workspace.dependencies]` entry is "used" only when a member references it via `{ workspace = true }`; none do. 9-1's own File List / Deviation 1 corroborates: it pruned exactly these from `crates/kt` (`indicatif` under Deviation 1; `walkdir`/`regex`/`dialoguer`/`urlencoding` as expected) and explicitly left the root `[workspace.dependencies]` entries "for a later cleanup" — this story is that cleanup.

### The exact edit map (the load-bearing part)

| Area | File:Line (baseline) | Change |
| --- | --- | --- |
| clap `about` | `crates/kt/src/main.rs:77` | rewrite → agent runner (drop "Agentic skills package manager") |
| crate `description` | `crates/kt/Cargo.toml:8` | rewrite → agent runner (no "skills"/"package-manager") |
| crate `keywords` | `crates/kt/Cargo.toml:13` | rewrite → drop `"skills"`,`"package-manager"`; ≤5, each ≤20 chars, lowercase |
| workspace `version` | `Cargo.toml:12` | `"0.5.0"` → `"0.6.0"` (moves only `ktesio`/`kt`) |
| dangling deps | `Cargo.toml:65,73,74,75,77` | delete `indicatif`,`walkdir`,`regex`,`dialoguer`,`urlencoding` |
| surface tests | `crates/kt/src/main.rs` `test_cli_subcommands_exist` (~L291), `test_subcommand_help_includes_details_and_examples` (~L410) | add absent-retired-name assertions; keep survivor iteration |
| changelog | `CHANGELOG.md` (prepend above `## v0.5.0` @ L7) | new `## v0.6.0` removal entry (FR-38) |
| release notes | `docs/RELEASE_NOTES.md` (prepend above `## v0.5.0` @ L12) | new `## v0.6.0` removal entry (FR-38) |

⚠️ **Trap:** `kt --version` already prints the package version via `#[command(version)]`; the value changes *because* the workspace version moves — do **not** hard-code a version string in `main.rs`.

### Recommended identity copy (dev finalizes; the AC binds the testable properties, not the marketing words)

Drawn from the already-live positioning so the binary matches the docs:
- README lead (`README.md:11`): "Ktesio is a Rust CLI and engine that **runs AI agents like services** — supervise their lifecycle, meter real token usage, and enforce dollar budgets."
- Banner alt (`README.md:2`): "run AI agents like services — supervise, meter, and budget them".
- `docs/commands.md:8`: "The agent runner lives under `kt agent`."

So: `about = "Run AI agents like services — supervise, meter, and budget them"`; `description = "Run AI agents like services: supervise their lifecycle, meter real token usage, and enforce dollar budgets."`; `keywords = ["agent", "ai", "supervisor", "metering", "cli"]`.

### The 9-1 → 9-2 seam (why this story authors what 9-1 deferred)

9-1 made the **minimal green-keeping trim**: it removed the retired-command assertions from the two surface tests, leaving them asserting only `agent` + `self-update` *present*, and it left the `about` string reading "Agentic skills package manager" (a deliberate, documented transient mismatch). 9-2 closes that seam: it (a) rewrites the identity so `kt --help` no longer contradicts the docs, and (b) adds the **negative** assertions (the ten retired names are *absent*) that positively prove the single canonical surface. This is the "authoring" half the 9-1 story's "9-1 ↔ 9-2 coupling" note reserved for here.

### Owner-ratified decisions baked into this story (2026-07-14)

1. **Clean removal at v0.6.0.** The pivoted release version is **0.6.0** (proposal §6 Decision 1, Option A; §6 "Version bump required"). Optional **Story 9-4 (kind removal notices) is SKIPPED** — this story adds **no** deprecation stubs, notices, or unknown-subcommand interceptors. A bare clap "unrecognized subcommand" error for a retired name is the accepted behavior; FR-38's window is treated as satisfied-by-release-notes at this pre-1.0 pivot boundary.
2. **`kt agent list` / `kt agent show` are the single canonical surface** — top-level `kt list`/`kt show` are removed (already excised in 9-1; guarded here). No alias is added (proposal §6 Decision 3 — removal, not aliasing).
3. **FR-37/FR-38 amended, FR-39 preserved.** FR-37/38's deprecate-in-place lifecycle is replaced by "removed at the pivot release, announced at a stated version" (AC-6 delivers the announce). FR-39 continuity holds: `name = "ktesio"`, `[[bin]] name = "kt"`, install channels, and `kt self-update` are untouched.

### Scope guards (do NOT do these in 9-2)

- **No docs/architecture edits** — `docs/architecture.md` L136-222 tail, `docs/lockfile.md`, `docs/manifest.md`, and the AD-16/Epic-8 re-scope are **Story 9-3 (architect-owned, Winston)**.
- **Do NOT tighten `scripts/check_docs.py`'s `KT_COMMANDS` allowlist** — leave it stale in 9-2. It still lists the retired names (`init`,`search`,`install`,`upgrade`,`publish`,`list`,`show`,`doctor`,`uninstall`,`remove`), which (a) keeps the doc-lint green against the still-stale docs 9-3 will fix, and (b) conveniently lets the new changelog mention retired commands without tripping the lint. Tightening it is Story 9-3.
- **No engine/adapter/conformance changes** — this story touches only `crates/kt/src/main.rs`, `crates/kt/Cargo.toml`, the ROOT `Cargo.toml`, `CHANGELOG.md`, and `docs/RELEASE_NOTES.md`. `CONTRACT_VERSION` / `FLEET_SCHEMA_VERSION` do not move.
- **No re-excision** — 9-1 already removed the legacy surface/modules/tests; do not re-delete or relocate anything.
- **Do NOT rewrite historical CHANGELOG/RELEASE_NOTES entries** (`## v0.5.0` and earlier are generated release records — byte-frozen).

### Testing notes

- The proof is mostly subtractive + assertive-on-strings: no new feature paths, so the retained `agent_cli.rs` suite + the two rewritten `main.rs` unit tests carry it. No new test *files* are required (an optional in-file rebrand test is recommended).
- `test_cli_help_includes_license_and_repository` is the idiomatic model for the rebrand assertion — it already asserts help contains the `env!`-injected License/Repository footer; mirror it for the about/description.
- The rebrand changes `kt --help` header text only; `render_help()` still contains "agent" (the subcommand) and "self-update", so `test_subcommand_help_includes_details_and_examples` (which asserts each survivor's Details/Example blocks) is unaffected.
- Coverage: editing a string/version/tests/docs/manifest adds no untested `src/` lines; re-run `tarpaulin --fail-under 95` on `src/` and confirm ≥95% locally (the #101 per-crate split cleared the CI OOM 2026-07-14, but keep the local proof per standing practice).
- The doc-lint (`scripts/check_docs.py`) scans root `*.md` + `docs/*.md`; it flags `kt <cmd>` inside ```bash fences where `<cmd> ∉ KT_COMMANDS`. Because the retired names are still allow-listed, and because prose bullet lists (not ```bash fences) are not command-scanned at all, the v0.6.0 removal entries are safe either way. Reference `docs/RELEASE_NOTES.md` (not `RELEASES_NOTES`) to avoid the STALE_PATTERNS guard.

### Project Structure Notes

- **Blast radius:** five files — `crates/kt/src/main.rs`, `crates/kt/Cargo.toml`, ROOT `Cargo.toml`, `CHANGELOG.md`, `docs/RELEASE_NOTES.md`. No files created or deleted; no modules moved. (This is the first Epic 9 story to touch the ROOT `Cargo.toml` — 9-1 deliberately left the root workspace deps alone; AC-7 folds that cleanup in here.)
- **Naming/paths frozen for continuity (FR-39):** crates.io package `ktesio`, binary `kt`, `[[bin]] path = "src/main.rs"`, install channels, and `kt self-update` are unchanged. Only the human-facing *identity copy*, the *version*, the *changelog/notes*, and the *dead root deps* change.

### References

- **Sprint change proposal** — `_bmad-output/planning-artifacts/sprint-change-proposal-2026-07-13.md`: §4 (Epic 9 → Story 9.2 acceptance criteria, lines ~114-129), §2 (Artifact conflicts — "Do NOT rewrite CHANGELOG/RELEASE_NOTES historical entries; the new pivot release's notes state the removal"), §6 (Decision 1 Option A clean-break at ≥0.6.0 + Story 9-4 skipped; Decision 3 top-level `list`/`show` removal; "Version bump required"; FR-39 preservation).
- **Epics** — `_bmad-output/planning-artifacts/epics.md`: Epic 9 → Story 9.2 (verbatim AC, lines 773-787), and the Epic 8 correction note (8-4 superseded; 8-1/AD-16 premise changed).
- **Predecessor story** — `_bmad-output/implementation-artifacts/9-1-remove-the-legacy-skill-manager-command-surface-and-modules.md`: the excision this builds on; its "9-1 ↔ 9-2 coupling" Dev Note and its Deviation 1 / File List (root `[workspace.dependencies]` left for this cleanup; `indicatif` pruned from kt).
- **PRD FRs** — `_bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md`: FR-37 (L326-329, amended → removal), FR-38 (L335-338, "announce (release notes + README) … removal in a stated later version" — AC-6 satisfies this at 0.6.0), FR-39 (L340-343, continuity of the `kt` name / `ktesio` package / channels — preserved).
- **Post-9-1 code anchors (baseline `42896b7`, will drift — re-grep before editing):** `main.rs` `about` L77; `Commands` (only `SelfUpdate`+`Agent`) L85-103; `test_cli_subcommands_exist` L291-295; `test_subcommand_help_includes_details_and_examples` L410-424; `test_cli_help_includes_license_and_repository` L397-401. `crates/kt/Cargo.toml` `description` L8, `keywords` L13, `version.workspace` L5. ROOT `Cargo.toml` `[workspace.package] version` L12; dangling deps `indicatif` L65, `walkdir` L73, `regex` L74, `dialoguer` L75, `urlencoding` L77. `docs/commands.md:8` (agent-only intro); `README.md:2,11` (identity copy); `scripts/check_docs.py:29-59` (`KT_COMMANDS` — leave stale).

### Assumptions & open items logged (headless run)

1. **`github_issue: 112` set from the task/orchestrator.** Consistent with 9-1's `github_issue: 111` and epic issue #110. If the sync differs, correct on integration.
2. **`sprint-status.yaml` was NOT edited by this workflow** (the orchestrator owns it, per the task and proposal §5). The create-story workflow would normally flip `9-2-…` to `ready-for-dev`; that flip is deferred to the orchestrator. This file's own `Status:` is `ready-for-dev`.
3. **CHANGELOG/RELEASE_NOTES are normally tag-generated** (both files' headers say so). AC-6's hand-authored `## v0.6.0` entry is a deliberate pre-seed to satisfy FR-38's "removal at a stated version" in-repo now; the tag automation at publish will fill the asset table and may reconcile/merge the entry. This is intentional, not a conflict — flagged so the dev doesn't treat the "generated" header as forbidding a manual entry.
4. **Recommended identity copy and keywords are recommendations, not mandates.** The ACs bind the *testable properties* (no skills/package-manager framing; agent-framed; crates.io keyword rules; version 0.6.0). Exact wording is the dev's to finalize, subject to those assertions and `cargo publish --dry-run` passing keyword validation.
5. **`indicatif` is included in the AC-7 delete set.** 9-1's Deviation 1 pruned it from `crates/kt` (it was orphaned when the skill-manager progress bars were removed), leaving the root `[workspace.dependencies]` entry dangling. Verified 0 refs — safe to delete alongside the other four.
6. **Story filename uses the proposal §5 sprint-status key** `9-2-reposition-the-top-level-kt-identity-to-the-agent-runner`, while the H1 carries the epics.md Story 9.2 title — mirroring the 9-1 file/title convention in this folder.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (BMAD dev agent, headless/synchronous run 2026-07-14).

### Debug Log References

- Dangling-dep re-verification (AC-7, re-run before deletion): `0` `<dep>::` source refs and `0` `^<dep>` manifest refs across `crates/**` for all five (`indicatif`,`walkdir`,`regex`,`dialoguer`,`urlencoding`); `0` `{ workspace = true }` references. Safe to delete.
- Smoke run used an isolated `KTESIO_STATE_DIR=$(mktemp -d)` so the user's real Fleet state was never touched.

### Completion Notes List

- **AC-1** — `about` rewritten to `"Run AI agents like services — supervise, meter, and budget them"` (the recommended copy). No `long_about` added (optional; the AC binds only the `about` property). Verified skills-/package-manager-free and agent-framed via the new unit test and the live `kt --help` header.
- **AC-2** — `description = "Run AI agents like services: supervise their lifecycle, meter real token usage, and enforce dollar budgets."`; `keywords = ["agent", "ai", "supervisor", "metering", "cli"]` (5 keywords, each ≤20 chars, lowercase ASCII/hyphen — crates.io-valid). `name`/`[[bin]]`/`categories`/`readme` untouched (FR-39).
- **AC-3/AC-4** — `test_cli_subcommands_exist` now asserts `agent` + `self-update` PRESENT and each of the 10 retired names ABSENT at the **top level** (top-level `find_subcommand` on `Cli::command()`, NOT a recursive/agent-tree search — the trap; `kt agent list`/`show`/`remove` remain live and their tests stay green). `test_subcommand_help_includes_details_and_examples` still iterates only `self-update` + `agent`. Added the optional rebrand-assertion test `test_cli_identity_is_agent_framed_not_skills` (mirrors the license/repository env test). All prior `test_agent_*`, `test_cli_*`, and `test_self_update_*` untouched and green.
- **AC-5** — workspace version `0.5.0 → 0.6.0` in the ROOT `Cargo.toml` `[workspace.package]` only; `crates/kt/Cargo.toml` still inherits via `version.workspace = true` (no explicit version added). `Cargo.lock` auto-updated the single `ktesio` line to `0.6.0`. The four internal crates still pin `0.1.0` (unmoved). `kt --version` prints `kt 0.6.0`.
- **AC-6** — new `## v0.6.0` sections prepended above `## v0.5.0` in both `CHANGELOG.md` and `docs/RELEASE_NOTES.md`, each stating (a) the removed commands, (b) removal version 0.6.0, (c) the `kt agent …` replacement, (d) a migration pointer as a valid in-repo link to the command reference (`docs/commands.md` from root; `commands.md` from `docs/`). Retired command names appear only as inline code in prose bullets (no ```bash fences), so `check_docs.py` does not scan them. Historical entries byte-unchanged.
- **AC-7** — deleted exactly the five dangling `[workspace.dependencies]` (`indicatif`,`walkdir`,`regex`,`dialoguer`,`urlencoding`); all other entries intact.
- **AC-8 — Gates (all green, `cargo +1.96.1`):** `fmt --all --check` OK (rustfmt reflowed one `assert!`); `clippy --workspace --all-targets -- -D warnings` clean (compiles `ktesio v0.6.0`); `nextest run --workspace --all-targets` → **768 passed, 0 failed**; `test --workspace --doc` OK; `python3 scripts/test_automation.py` → 21 OK; `python3 scripts/check_docs.py` → 23 files validated; the three CI grep/tree gates (AD-2 crate-visibility boundary [edges = `ktesio-engine`/`ktesio-adapter-api` only], OS-cfg location, single-currency-module) all green; no new OS-cfg introduced. **cargo-semver-checks:** not run locally — dormant for `kt` pre-publish (the CI job checks only `ktesio-engine`/`ktesio-adapter-api`, which this story does not touch; the 0.6.0 bump neither arms nor fails it). **Coverage/tarpaulin:** not run locally per task guidance (CI validates); this story adds only a string, a version, tests, docs, and manifest edits — no new untested `src/` product paths. **Smoke:** `kt --version` → `kt 0.6.0`; `kt --help` header agent-framed and lists only `self-update`/`agent`/`help`; retired `kt install` → clap `unrecognized subcommand 'install'` (Story 9-4 skipped — bare clap error is the accepted behavior); `kt agent list`/`register demo --kind mock`/`self-update --help` all work.
- **Deviations:** none. Recommended identity copy + keywords adopted verbatim. `sprint-status.yaml` deliberately not edited (orchestrator-owned, per the story Assumptions).

### File List

- `crates/kt/src/main.rs` — rebrand `about` to the agent-runner line; add absent-retired-name assertions to `test_cli_subcommands_exist`; add `test_cli_identity_is_agent_framed_not_skills`
- `crates/kt/Cargo.toml` — rewrite `description` + `keywords` (no version edit — inherits workspace)
- `Cargo.toml` (root) — `[workspace.package] version` 0.5.0 → 0.6.0; delete dangling `[workspace.dependencies]` `indicatif`/`walkdir`/`regex`/`dialoguer`/`urlencoding`
- `Cargo.lock` — auto-updated `ktesio` 0.5.0 → 0.6.0 (single line)
- `CHANGELOG.md` — prepend `## v0.6.0` removal entry (historical entries untouched)
- `docs/RELEASE_NOTES.md` — prepend `## v0.6.0` removal entry (historical entries untouched)

## Change Log

| Date | Version | Description | Author |
| --- | --- | --- | --- |
| 2026-07-14 | 0.1 | Story context created (headless Scrum-Master run); code state verified against post-9-1 baseline 42896b7 (identity in exactly 3 spots; no help-asserting integration/snapshot test; retired top-level commands already gone; version isolated to kt; 5 root deps provably dangling — 0 refs). Owner's ratified decisions (v0.6.0, 9-4 skipped, list/show removal) + FR-38 announce baked in. | BMAD create-story |
| 2026-07-14 | 1.0 | Implemented (headless dev run): `about`/`description`/`keywords` rebranded to the agent runner; surface tests author the canonical-surface guard (10 retired names absent at top level) + added rebrand-assertion test; workspace `0.5.0 → 0.6.0`; `## v0.6.0` FR-38 removal entries prepended to CHANGELOG + RELEASE_NOTES; 5 dangling root workspace deps deleted. All gates green (fmt, clippy -D warnings, 768 nextest, doc, test_automation, check_docs, boundary/OS-cfg/currency grep gates). Status → review. | BMAD dev-story |
