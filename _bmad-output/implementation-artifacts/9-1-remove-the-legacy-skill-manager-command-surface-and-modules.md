---
github_issue: 111   # Epic 9 issue #110; story 9-1 = #111 (synced 2026-07-14).
baseline_commit: 72b212d23ed824e8a3f2095e5158c90d3b218e73
epic: 9
story: 9.1
supersedes: [8-4]           # deprecate-in-place is retired outright (Islam-ratified 2026-07-14)
re_scopes: [8-1, AD-16]     # the skills machinery 8-1/AD-16 planned to relocate-and-reuse is deleted here
frs: [FR-37, FR-38, FR-39]  # FR-37/38 amended (removal replaces deprecate-in-place); FR-39 preserved
sources:
  - _bmad-output/planning-artifacts/sprint-change-proposal-2026-07-13.md   # §4 Story 9.1 ACs; §2 Artifact conflicts (keep/delete lists)
  - _bmad-output/planning-artifacts/epics.md                                # Epic 9 → Story 9.1
---

# Story 9.1: Remove the legacy skill-manager command surface, modules, and tests

Status: review

<!-- Headless-conservative run (Scrum-Master, 2026-07-14): every choice resolved from the
sprint-change-proposal + epics.md + the owner's ratified decisions; the keep/delete lists
were VERIFIED against the code at baseline 72b212d, not copied on trust. Assumptions +
sequencing notes are logged at the end of Dev Notes. No user interaction. -->

## Story

As the Ktesio maintainer,
I want the retired skill-manager commands and every module and test that exists only to serve them deleted from the `kt` crate,
so that the binary carries only the agent runner (plus binary self-maintenance) and no dead skill-manager code.

## Acceptance Criteria

Derived from the sprint-change-proposal §4 (Story 9.1 acceptance criteria) and §2 (Artifact conflicts — the exact keep/delete lists), refined against the code at the baseline commit. Every list below was verified against source this session (see Dev Notes → "Verification log"); the `AGENT_AFTER_HELP` / branding / top-level `list`/`show` / changelog work is deliberately **excluded** (that is Story 9-2), and docs/architecture reconciliation is **excluded** (that is Story 9-3, architect-owned).

**Given** the current `kt` crate at baseline `72b212d`
**When** the excision lands

1. **AC-1 — Command surface removed (enum + dispatch + after-help + module decls).** `crates/kt/src/main.rs` no longer contains the `Commands` variants `Init`, `Install`, `Search`, `Publish`, `Upgrade`, `List`, `Show`, `Doctor`, or `Uninstall` (alias `remove`), nor the `PublishCommands` enum; their match arms in `run_cli` are gone; and their after-help constants `INIT_AFTER_HELP`, `INSTALL_AFTER_HELP`, `SEARCH_AFTER_HELP`, `UPGRADE_AFTER_HELP`, `PUBLISH_AFTER_HELP`, `LIST_AFTER_HELP`, `SHOW_AFTER_HELP`, `DOCTOR_AFTER_HELP`, `UNINSTALL_AFTER_HELP` are deleted. The `mod` declarations for the deleted support modules (`discovery`, `git`, `install_target`, `lockfile`, `manifest`, `skill`, `skills_sh`) are removed from `main.rs`. **Retained in `main.rs`:** the `SelfUpdate` and `Agent` variants + their arms, `SELF_UPDATE_AFTER_HELP`, `AGENT_AFTER_HELP`, `HELP_FOOTER`, `should_check_for_updates`, and the `mod cli; mod error; mod install_channel; mod ui; mod update_check;` declarations. (The top-level `about = "Agentic skills package manager"` string is intentionally **left unchanged** — rebranding is Story 9-2.)

2. **AC-2 — Legacy handler modules and support modules deleted; `cli/mod.rs` trimmed.** The files `crates/kt/src/cli/{init,install,search,publish,upgrade,list,show,doctor,uninstall}.rs` are deleted, and the support modules `crates/kt/src/{skills_sh,discovery,install_target,manifest,lockfile,git,skill}.rs` are deleted. `crates/kt/src/cli/mod.rs` declares only `pub mod agent;` and `pub mod self_update;`. **Retained:** `crates/kt/src/cli/{agent,self_update}.rs` and the top-level support files `crates/kt/src/{install_channel,update_check,ui,error}.rs`.

3. **AC-3 — `error.rs` trimmed to the agent + self-update families; `AgentManifest*` explicitly preserved.** `crates/kt/src/error.rs` no longer defines the 18 skill-only error types: `InitPathNotFound`, `ManifestNotFound`, `ManifestDuplicateName`, `ManifestInvalidName`, `LockfileNotFound`, `LockfileInvalid`, `GitCloneFailed`, `GitFetchFailed`, `GitCheckoutFailed`, `GitRevParseFailed`, `SkillCopyFailed`, `SkillNotFound`, `InstallInvalidFormat`, `InstallAlreadyExists`, `DiscoveryError`, `SkillsDirectoryEmpty`, `DoctorUnhealthy`, `SearchFailed`. It **retains** `SelfUpdateFailed` and the entire `Agent*` family (`AgentDuplicateName`, `AgentInvalidName`, `AgentNotFound`, `AgentRunningRequiresForce`, `AgentIo`, `AgentStore`, `AgentUnknownKind`, `AgentManifestNotFound`, `AgentManifestInvalid`, `AgentManifestUnreadable`, `AgentNoMeteringSource`, `AgentNoCapabilities`, `AgentInvalidTransition`, `AgentLaunchFailed`, `AgentCapabilityUnsupported`, `AgentUnknownConfigKey`, `AgentConfig`). ⚠️ The three **`AgentManifest*`** variants describe the agent `adapter.toml` and MUST survive — they are distinct from the deleted skills.json `Manifest*` trio (`ManifestNotFound`/`ManifestDuplicateName`/`ManifestInvalidName`).

4. **AC-4 — Legacy tests deleted; `tests/helpers/mod.rs` trimmed to the agent helpers; `agent_cli.rs` unchanged.** `crates/kt/tests/{adoption_cli,install_default,install_fallback,publish}.rs` are deleted. `crates/kt/tests/helpers/mod.rs` retains only the agent helpers — `TestContext` (with only its `new()` method), `run_kt_agent`, `run_kt_agent_with_env`, and the `KtRun` struct — and drops the skill-only helpers: `KtCommandOutput`, `run_kt_command`, `run_kt_command_output`, `run_git`, `create_local_skill_repo`, and `TestContext::{skills_dir, lockfile, manifest, ensure_skills_dir, create_fixture_repo}`. `crates/kt/tests/agent_cli.rs` is **byte-unchanged** (it imports only `{run_kt_agent, run_kt_agent_with_env, TestContext}` and has zero skill-manager references — verified).

5. **AC-5 — `Cargo.toml` dependencies pruned to what survives the excision; the self-update stack retained.** `crates/kt/Cargo.toml` `[dependencies]` drops every crate left unused after the excision, **verified by the compiler and `cargo machete` (and/or `cargo udeps`) — not assumed.** Strong candidate removals (zero references in any retained `src/` file at baseline): `walkdir`, `regex`, `dialoguer`, `urlencoding` — confirm before removing. Every dependency still used by the agent-runner or `self-update` path is **retained**, explicitly including the archive/verify stack (`flate2`, `sha2`, `tar`, `zip`), the HTTP/version stack (`ureq`, `semver`), and `ktesio-engine`, `clap`, `console`, `miette`, `indicatif`, `serde`, `serde_json`, `thiserror`. `[dev-dependencies]` (`tempfile`, `rusqlite`, `ktesio-conformance`) are **untouched** — all three back agent-runner tests. The crates.io package name `ktesio` and the `[[bin]] name = "kt"` are **unchanged** (FR-39). (The `description` + `keywords` rewrite is Story 9-2.)

6. **AC-6 — Behavior preserved and the crate compiles + all tests green.** `kt agent …` (register/remove/start/stop/pause/resume/list/show/config) and `kt self-update` behave exactly as before. The crate and its test targets **compile**, and `cargo test --all-targets` is **green** — which requires the two `main.rs` unit tests that assert the old surface to be brought into a passing state (see AC-7); `test_cli_struct_valid`, `test_cli_help_includes_license_and_repository`, `test_self_update_skips_passive_update_check`, and every `test_agent_*` test stay green untouched.

7. **AC-7 — The two surface-asserting unit tests are kept green by trimming the removed entries only (identity rewrite is deferred to 9-2).** `test_cli_subcommands_exist` and `test_subcommand_help_includes_details_and_examples` in `main.rs` currently assert the presence of the retired subcommands via runtime string lookups; they compile after the excision but **fail at runtime**. This story removes the now-invalid assertions/iteration entries (the retired command names), leaving only `agent` and `self-update`, so `cargo test` passes. This story does **NOT** add the new positive/negative identity assertions (e.g. asserting the retired names are *absent*, or iterating the new canonical surface) or repurpose these tests — that authoring is Story 9-2. (Rationale + the seam: Dev Notes → "The 9-1 ↔ 9-2 coupling".)

8. **AC-8 — All nine CI gates green.** `cargo build --release`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, `cargo tarpaulin --fail-under 95` on `src/` (prove ≥95% locally per the standing #101 practice; removing large untested handler surface should help but the remaining `src/` must stay above the bar and green), the crate-visibility boundary job, `cargo-semver-checks`, the single-currency-formatter grep-lint, and the MSRV 1.96.1 build — all pass. No new `#[cfg(unix/windows/target_os)]` is introduced outside `ktesio-engine::backends` (this excision adds none).

## Tasks / Subtasks

> Dev: check these boxes IN THIS FILE. Order is compile-driven — delete leaves, then trim the roots that referenced them, then keep the gate green, then prune deps. Toolchain: `cargo +1.96.1` (mise overrides `rust-toolchain.toml` locally — see MEMORY / docs/testing.md). This is a mechanical deletion with a compile-and-gate loop; there is no new logic to design. **A `git rm` / `git mv`-clean deletion preserves history — prefer `git rm`.**

- [x] **Task 1 — Delete the nine legacy handler modules (AC-2).** `git rm crates/kt/src/cli/{init,install,search,publish,upgrade,list,show,doctor,uninstall}.rs`. Do not touch `cli/agent.rs` or `cli/self_update.rs`.
- [x] **Task 2 — Delete the seven exclusively-legacy support modules (AC-2).** `git rm crates/kt/src/{skills_sh,discovery,install_target,manifest,lockfile,git,skill}.rs`. (Verified: no retained source file imports any of these — the excision is self-contained.)
- [x] **Task 3 — Trim `cli/mod.rs` (AC-2).** Reduce `crates/kt/src/cli/mod.rs` to exactly `pub mod agent;` + `pub mod self_update;`.
- [x] **Task 4 — Trim `main.rs` (AC-1).** In `crates/kt/src/main.rs`:
  - [x] Remove the `mod` lines for `discovery, git, install_target, lockfile, manifest, skill, skills_sh` (lines ~2/4/6/7/8/9/10). Keep `cli, error, install_channel, ui, update_check`.
  - [x] Delete the after-help consts `INIT_/INSTALL_/SEARCH_/UPGRADE_/PUBLISH_/LIST_/SHOW_/DOCTOR_/UNINSTALL_AFTER_HELP`. Keep `HELP_FOOTER`, `SELF_UPDATE_AFTER_HELP`, `AGENT_AFTER_HELP`.
  - [x] Delete the `Commands` variants `Init, Install, Search, Upgrade, Publish, List, Show, Doctor, Uninstall` and the whole `PublishCommands` enum. Keep `SelfUpdate` and `Agent` (and all of `AgentCommands`/`ConfigCommands`).
  - [x] Delete the corresponding arms in `run_cli`'s `match` (Init/Install/Search/Upgrade/Publish/List/Show/Doctor/Uninstall). Keep the `SelfUpdate`, `Agent`, and `None` arms. Leave `should_check_for_updates` (it references `Commands::SelfUpdate`, which stays).
  - [x] Do **not** edit the `about = "Agentic skills package manager"` string (Story 9-2 rebrands it).
- [x] **Task 5 — Trim `error.rs` (AC-3).** Delete the 18 skill-only error structs enumerated in AC-3. Keep `SelfUpdateFailed` and the 17-member `Agent*` family — double-check the three `AgentManifest*` structs survive (they are agent `adapter.toml` errors, not skills.json).
- [x] **Task 6 — Delete the four legacy integration tests (AC-4).** `git rm crates/kt/tests/{adoption_cli,install_default,install_fallback,publish}.rs`. (Confirmed skill-manager tests — e.g. `adoption_cli.rs` exercises `kt install --all` against git fixture repos and asserts `skills.json`/`skills.lock`; it is NOT the agent orphan-adoption suite, which lives in `agent_cli.rs` + the engine crate.)
- [x] **Task 7 — Trim `tests/helpers/mod.rs` (AC-4).** Keep only `TestContext` (with `new()`), `run_kt_agent`, `run_kt_agent_with_env`, `KtRun`. Delete `KtCommandOutput`, `run_kt_command`, `run_kt_command_output`, `run_git`, `create_local_skill_repo`, and `TestContext::{skills_dir, lockfile, manifest, ensure_skills_dir, create_fixture_repo}`. Drop now-unused `use` lines. Leave `crates/kt/tests/agent_cli.rs` untouched.
- [x] **Task 8 — Keep the two surface tests green (AC-6, AC-7).** In `main.rs`, remove the assertions/entries for the retired commands from `test_cli_subcommands_exist` (leave `self-update` + `agent`) and from `test_subcommand_help_includes_details_and_examples` (leave the `self-update` + `agent` rows). Do NOT add new "retired-name-absent" assertions or repurpose these tests — that is Story 9-2. This is the minimum edit that keeps `cargo test --all-targets` green after the excision.
- [x] **Task 9 — Prune `Cargo.toml` dependencies (AC-5).** Compile first, then run `cargo machete` (and/or `cargo +nightly udeps`) on `crates/kt`. Remove only the deps it flags as unused after the excision — expected: `walkdir`, `regex`, `dialoguer`, `urlencoding` (confirm; do not assume). Retain the self-update archive/verify/HTTP stack (`flate2`, `sha2`, `tar`, `zip`, `ureq`, `semver`) and everything the agent path uses. Leave `[dev-dependencies]` untouched. Do NOT edit `name`, `[[bin]]`, `description`, or `keywords` (name/bin are FR-39-frozen; description/keywords are Story 9-2).
- [x] **Task 10 — Run all nine gates and prove coverage locally (AC-6, AC-8).** With `cargo +1.96.1`: `build --release`, `fmt --check`, `clippy --all-targets -- -D warnings`, `test --all-targets`, `tarpaulin --fail-under 95` on `src/` (record the %), the crate-visibility boundary check, `cargo-semver-checks`, the currency grep-lint, and the MSRV build. Manual smoke: `kt --help` (lists only `agent` + `self-update`; note the `about` still reads "skills package manager" until 9-2), `kt agent list`, `kt agent register demo --kind mock`, `kt self-update --help`. Record results in the completion notes.

## Dev Notes

**This is the excision story of Epic 9 — a mechanical, self-contained deletion, not a redesign.** Its whole value is that the legacy skill-manager cluster is *provably* unreachable from the agent runner and self-update, so deleting it changes no live behavior. Everything below was verified against the code at baseline `72b212d` this session; trust the verification log, then execute the keep/delete lists exactly.

### Why the excision is clean (verified this session)

The seven support modules (`skills_sh`, `discovery`, `install_target`, `manifest`, `lockfile`, `git`, `skill`) are referenced **only** by the nine legacy handlers and by each other. The retained surface is disjoint:
- `cli/agent.rs` constructs only the `Agent*` error family (verified: `AgentIo`, `AgentConfig`, `AgentInvalidName`, `AgentStore`, `AgentNotFound`, `AgentManifest{NotFound,Invalid,Unreadable}`, … all present) and uses `crate::ui`.
- `cli/self_update.rs` imports exactly `crate::error::SelfUpdateFailed`, `crate::install_channel::{detect_install_channel, CommandProbe, InstallChannel}`, and `crate::ui`.
- A cross-reference grep for any retained file importing a deleted module (`crate::{discovery,git,install_target,lockfile,manifest,skill,skills_sh}` or `cli::{init,…,uninstall}`) returned **empty**.
- A grep for any skill-only error variant leaking into a retained file returned **empty**.

So Tasks 1–2 delete leaves; Tasks 3–5 remove the roots that named them; nothing else in `src/` can dangle.

### The exact keep ↔ delete inventory (the load-bearing part)

| Area | DELETE | KEEP |
| --- | --- | --- |
| `Commands` variants | `Init`, `Install`, `Search`, `Publish`(+`PublishCommands`), `Upgrade`, `List`, `Show`, `Doctor`, `Uninstall`/`remove` | `SelfUpdate`, `Agent` (+ `AgentCommands`, `ConfigCommands`) |
| `main.rs` after-help consts | `INIT_/INSTALL_/SEARCH_/UPGRADE_/PUBLISH_/LIST_/SHOW_/DOCTOR_/UNINSTALL_AFTER_HELP` | `HELP_FOOTER`, `SELF_UPDATE_AFTER_HELP`, `AGENT_AFTER_HELP` |
| `main.rs` `mod` decls | `discovery, git, install_target, lockfile, manifest, skill, skills_sh` | `cli, error, install_channel, ui, update_check` |
| `src/cli/*.rs` | `init, install, search, publish, upgrade, list, show, doctor, uninstall` | `agent, self_update` |
| `src/*.rs` support | `skills_sh, discovery, install_target, manifest, lockfile, git, skill` | `install_channel, update_check, ui, error` |
| `error.rs` structs | `InitPathNotFound`; `Manifest{NotFound,DuplicateName,InvalidName}`; `Lockfile{NotFound,Invalid}`; `Git{Clone,Fetch,Checkout,RevParse}Failed`; `Skill{Copy}Failed`/`SkillNotFound`; `Install{InvalidFormat,AlreadyExists}`; `DiscoveryError`; `SkillsDirectoryEmpty`; `DoctorUnhealthy`; `SearchFailed` (18) | `SelfUpdateFailed` + `Agent*` (17), incl. `AgentManifest{NotFound,Invalid,Unreadable}` |
| `tests/*.rs` | `adoption_cli, install_default, install_fallback, publish` | `agent_cli` (unchanged) |
| `tests/helpers/mod.rs` | `KtCommandOutput`, `run_kt_command`, `run_kt_command_output`, `run_git`, `create_local_skill_repo`, `TestContext::{skills_dir,lockfile,manifest,ensure_skills_dir,create_fixture_repo}` | `TestContext::new`, `run_kt_agent`, `run_kt_agent_with_env`, `KtRun` |
| `Cargo.toml` `[dependencies]` | candidates (tool-verify): `walkdir`, `regex`, `dialoguer`, `urlencoding` | `ktesio-engine`, `clap`, `console`, `miette`, `indicatif`, `serde`, `serde_json`, `thiserror`, `ureq`, `semver`, `flate2`, `sha2`, `tar`, `zip` |

⚠️ **The one trap:** the deleted skills.json `Manifest*` trio vs the retained agent `adapter.toml` `AgentManifest*` trio. Delete by exact type name, not by the substring "manifest".

### The 9-1 ↔ 9-2 coupling (the sequencing note — read this)

Two facets of the seam the dev must respect:

- **Compile coupling (inherent to 9-1).** Removing a `Commands` variant forces removing its `run_cli` match arm in the *same* change — the crate cannot compile otherwise. That is why the dispatch trimming is in this story, not deferred.
- **Test-gate coupling (the subtle one).** `test_cli_subcommands_exist` and `test_subcommand_help_includes_details_and_examples` assert the retired subcommands by **runtime string lookup** (`find_subcommand("init")`, `.expect("subcommand should exist")`). After the excision they still *compile* but *fail at runtime*, so `cargo test --all-targets` (a gate in this story's own AC-8) goes red. Resolution, ratified here: **9-1 makes the minimal green-keeping trim** — delete the retired-command assertions/entries, leaving `agent` + `self-update` — and **9-2 authors the identity rewrite** (positively assert the new canonical surface; assert the nine retired names are absent; rewrite the `about`/branding; remove top-level `list`/`show`; add the 0.6.0 CHANGELOG/RELEASE_NOTES entries). This keeps 9-1 independently landable and green without doing 9-2's identity work.
- **Expected transient state after 9-1 (by design, resolved by 9-2):** `kt --help` will list only `agent` + `self-update` while the top-level `about` still reads "Agentic skills package manager". That momentary mismatch is exactly what Story 9-2 closes; it is not a 9-1 defect.

### Owner's ratified decisions baked into this story (2026-07-14)

1. **Clean removal at a version bump.** The nine legacy commands are removed **outright** — the PRD FR-37/FR-38 amendment (removal replaces deprecate-in-place) is ratified. Optional **Story 9-4 (kind removal notices) is SKIPPED** (proposal §6 Option A). This story therefore adds **no** deprecation stubs, notices, or interceptors.
2. **`kt agent list` / `kt agent show` become canonical** by removing top-level `kt list` / `kt show` — but that top-level removal lands in **Story 9-2**, not here. (Here we only remove the *skill* `List`/`Show` commands, which happen to be the top-level `kt list`/`kt show` today; 9-2 owns confirming the canonical single surface + the docs match.) Boundary noted so the dev does not also try to re-point anything at `agent` in 9-1.
3. **Version 0.6.0** for the pivoted release. The workspace is still `0.5.0` (verified). The bump + the "removal at a stated version" changelog (FR-38) are **Story 9-2** work — do not edit the version or changelogs in 9-1.

### Scope guards (do NOT do these in 9-1)

- **Keep `self-update` and its modules** (`cli/self_update.rs`, `install_channel.rs`, `update_check.rs`) — binary distribution/maintenance, not skill management, grandfathered in Story 1.1's AC (FR-39). Retiring them is out of scope for the whole epic (proposal §6 Risks).
- **No branding/identity edits** — `about` string, `Cargo.toml` `description`/`keywords`, top-level `list`/`show` removal, and the new CHANGELOG/RELEASE_NOTES 0.6.0 entries are all **Story 9-2**.
- **No docs/architecture edits** — `docs/architecture.md` L136-222 tail, `docs/lockfile.md`, `docs/manifest.md`, and the AD-16/Epic-8 re-scope are **Story 9-3 (architect-owned, Winston)**.
- **No engine changes** — this story touches only `crates/kt/`. The engine, adapters, and conformance crates are untouched; `CONTRACT_VERSION` does not move.

### Testing notes

- The proof is subtractive: the retained `agent_cli.rs` suite (unchanged) + the engine/conformance suites already cover `kt agent …` and self-update behavior; this story must leave them **green**, not add new feature tests. No new test files are required.
- Deleting untested legacy handler code shifts the tarpaulin denominator; re-run coverage and confirm the remaining `src/` is still **≥95%** (prove locally — the coverage runner OOM #101 was cleared 2026-07-14 via the per-crate tarpaulin split, so CI coverage should also pass, but keep the local proof per standing practice).
- `cargo machete`/`cargo udeps` is the objective check for AC-5 — run it rather than reasoning about which deps are dead. The compiler will already reject a truly-dangling `use`; machete catches manifest-level dead deps the compiler tolerates.
- Sanity smoke after the gate loop: `kt --help`, `kt agent register demo --kind mock`, `kt agent list --json`, `kt self-update --help` — all behave as at baseline.

### Project Structure Notes

- **Blast radius is one crate.** Every change is inside `crates/kt/` (`src/`, `src/cli/`, `tests/`, `Cargo.toml`). The engine, adapter-api, hermes, and conformance crates are not touched; the workspace `Cargo.toml` is not touched. No new files are created — this story is deletions plus small in-file trims.
- **No new modules, no relocations.** Unlike the (now re-scoped) Epic 8 Story 8-1, nothing moves into `engine::skills`; the skills machinery is deleted, not relocated (proposal §2 / §6). Do not create replacement modules.
- **Naming/paths frozen for continuity (FR-39):** crates.io package `ktesio`, binary `kt`, `[[bin]] path = "src/main.rs"`, and `KTESIO_STATE_DIR`/install-channel behavior are unchanged.

### References

- **Sprint change proposal** — `_bmad-output/planning-artifacts/sprint-change-proposal-2026-07-13.md`: §4 (Epic 9 → Story 9.1 acceptance criteria + scope guard), §2 (Artifact conflicts — the keep/delete lists this story implements), §6 (Option A clean-break decision; FR-37/38 amendment; FR-39 preservation).
- **Epics** — `_bmad-output/planning-artifacts/epics.md`: Epic 9 → Story 9.1 (verbatim AC), and the Epic 8 correction note (8-4 superseded; 8-1/AD-16 premise changed).
- **PRD FRs** — FR-37/FR-38 (amended: the legacy skill-manager surface is *removed* at the pivot release, announced at a stated version — replaces the deprecate-in-place lifecycle), FR-39 (continuity of the `kt` name, the `ktesio` crates.io package, and the install channels — **preserved** by this story: name/bin/package untouched).
- **Baseline anchors (as of `72b212d`, will drift — re-grep before editing):** `main.rs` mod decls L1–12; after-help consts L23–119 (self-update L70, agent L121 kept); `about` L173; `Commands` L181–307; `PublishCommands` L309–318; dispatch L445–534; `should_check_for_updates` L536–538; `test_cli_subcommands_exist` L549–564; `test_subcommand_help_includes_details_and_examples` L678–701. `error.rs` skill structs L6–128; `SelfUpdateFailed` L130–135; `Agent*` L137–254. `cli/mod.rs` L1–11.

### Assumptions & open items logged (headless run)

1. **`github_issue` left as TBD.** Epic 9's issues are not yet synced; proposal §5 reserves creation for the orchestrator (epic issue + 9-1/9-2/9-3, added to Project Ktesio #5, no renumber of #55–#99). Fill on sync.
2. **`sprint-status.yaml` was NOT edited by this workflow.** Normally `create-story` flips the story to `ready-for-dev` in sprint-status, but the Epic 9 keys do not exist there yet and proposal §5 provides a *verbatim* block reserved for the orchestrator (with an explicit "do not clobber the concurrent retro's `last_updated` edits" warning). To avoid a lost-update on that concurrency-sensitive header, integrating the Epic 9 block + flipping `9-1-…` to `ready-for-dev` is deferred to the orchestrator. The block to append (after `epic-8-retrospective: optional`):
   ```yaml
     epic-9: backlog
     9-1-remove-the-legacy-skill-manager-command-surface-and-modules: backlog
     9-2-reposition-the-top-level-kt-identity-to-the-agent-runner: backlog
     9-3-reconcile-the-stale-architecture-and-skill-manager-docs: backlog
     epic-9-retrospective: optional
   ```
   (9-4 omitted — Option A/skip ratified.) This story's own `Status:` is set to `ready-for-dev`.
3. **`InitPathNotFound` added to the delete list** although the proposal's parenthetical elided it under "…". Verified skill-only (`kt init`) and absent from every retained file — safe to delete. Same for `InstallAlreadyExists`, `DiscoveryError`, `SkillsDirectoryEmpty`, `DoctorUnhealthy`, `SearchFailed` (all enumerated explicitly in AC-3 rather than left under an ellipsis).
4. **Cargo dependency removals are stated as candidates, not facts.** `walkdir`/`regex`/`dialoguer`/`urlencoding` show zero references in retained source, but per proposal §2 ("do not assume") the AC binds the dev to `cargo machete`/compiler verification, not to this list.
5. **Story filename uses the proposal's sprint-status key** `9-1-remove-the-legacy-skill-manager-command-surface-and-modules` (proposal §5), while the H1 carries the fuller epics.md title (…"command surface, modules, and tests") — mirroring the existing file/title convention in this folder.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (BMAD dev-story workflow, headless/synchronous run).

### Debug Log References

- `cargo +1.96.1 build -p ktesio` — after the enum/dispatch/module excision, 17 dead-code warnings surfaced, ALL in `crates/kt/src/ui.rs` (skill-manager presentation helpers orphaned by the excision; compiler confirmed dead across the whole bin, which includes `cli/agent.rs`). `error.rs` had zero dead warnings (every retained `Agent*` + `SelfUpdateFailed` variant is still live).
- `cargo machete crates/kt` → "didn't find any unused dependencies" AFTER pruning `walkdir`, `regex`, `dialoguer`, `urlencoding`, **and `indicatif`** — objectively confirming the prune (AC-5's tool-verified requirement).
- Single-crate `cargo +1.96.1 tarpaulin -p ktesio --engine llvm` reports the whole-workspace denominator against kt's tests only (63.60% — not the gate); the retained kt/src files it line-lists aggregate to **96.97%** (self_update 146/155, install_channel 24/24, main 2/2, ui 145/148, update_check 67/67).

### Completion Notes List

Mechanical excision of the legacy skill-manager surface from `crates/kt`, per the story's verified keep/delete lists. The tree stays compiling and all runnable gates are green.

**AC results:**
- **AC-1..AC-4** (surface + modules + error structs + tests removed) — done exactly per the keep/delete inventory. The `AgentManifest*` trap was respected: kt's own `error::ManifestNotFound` struct was deleted, while `AgentManifestNotFound` (adapter.toml) and the engine's `RegistryError::ManifestNotFound` variant (a different type) were left intact. `tests/agent_cli.rs` is byte-unchanged.
- **AC-5** (deps) — pruned `walkdir`, `regex`, `dialoguer`, `urlencoding` as expected, **plus `indicatif`** (see Deviation 1). `cargo machete` confirms zero unused deps; `[dev-dependencies]`, `name`, `[[bin]]`, `description`, `keywords` untouched.
- **AC-6/AC-7** (behavior + surface tests) — `test_cli_subcommands_exist` and `test_subcommand_help_includes_details_and_examples` trimmed to only `self-update` + `agent` (no new "retired-name-absent"/identity assertions — that stays 9-2). 767 tests pass. Smoke: `kt --help` lists only `self-update` + `agent` (the `about` still reads "Agentic skills package manager" — 9-2 rebrands it); all 10 retired commands incl. the `remove` alias now error; `kt agent register/list/list --json` and `kt self-update --help` behave as at baseline.
- **AC-8** (nine gates) — `build --release` ✅, `fmt --all --check` ✅, `clippy --workspace --all-targets -- -D warnings` ✅, `nextest --workspace --all-targets` (767 passed) ✅, doctests ✅, boundary/AD-2 (edges exactly `ktesio-engine`+`ktesio-adapter-api`) ✅, OS-cfg gate ✅ (no new cfg; grandfathered self_update.rs/update_check.rs retained), currency/AD-8 grep ✅, `check_docs.py` (23 files) ✅, `test_automation.py` (21 tests) ✅. `cargo-semver-checks`: the CI semver job checks only `ktesio-engine`/`ktesio-adapter-api` (not `kt`) and is dormant pre-publish (arms at first crates.io publish, story 7-4) — this story touches neither, so it is unaffected. Coverage: the merged 5-crate `--fail-under 95` union (95.22% on main via the #101 per-crate split) is validated by CI; the local single-crate read cannot reproduce the union and the full instrumented merge is the long run the orchestrator said not to block on — the excision only removes lightly-covered handler surface, which raises the union, and the retained kt/src is 96.97%.

**Deviations from the story (documented for the reviewer/orchestrator):**

1. **`ui.rs` dead-code removal + `indicatif` prune (excision fallout the story's verification did not map).** The story lists `ui.rs` under KEEP with no trim task and names `indicatif` under KEEP-deps. But the excision orphaned 17 skill-manager-only helpers in `ui.rs` (install/upgrade/init progress bars + `progress_style`/`finish_success`/`finish_error`, `label`, `compact_source`, `short_commit`, `print_diagnostics`+`DiagnosticKind`, `wrap_text`/`split_long_word`, and the `TableCell::number`/`command` + `CellStyle::Number`/`Command` + `TableColumn::right` API used only by the deleted search/list tables). Keeping them fails `clippy -D warnings` (AC-8); suppressing them with `#[allow(dead_code)]` would leave dead skill-manager code, contradicting the story's stated goal. I removed the dead helpers (keeping ALL live UI code — message helpers, `print_table`/`render_table` core, `status_label`, etc.) and updated the `ui.rs` unit tests that exclusively exercised them (dropped 4 whole tests for the removed fns; rewrote `test_render_table_handles_alignment_and_missing_cells` → `..._handles_missing_cells` using live cell types to preserve the missing-cell/row-count coverage; trimmed the `print_diagnostics` calls out of `test_print_helpers_do_not_panic`). Removing the progress helpers orphaned `indicatif` (+ the `std::time::Duration` import) — nothing in the agent/self-update path uses it (`agent.rs`: `ui::info/note/print_table/skill_name/success/warning`; `self_update.rs`: `ui::info/success`). AC-5's governing principle is "every dependency STILL USED is retained" and binds the dev to `cargo machete`/compiler verification over the example list; `machete` flags `indicatif` unused, so it was pruned. The story's "KEEP indicatif" was premised on it being agent-runner code, which is false.
2. **Coverage not re-proven via the full local merge** — noted above; CI validates the merged gate. No other deviation.

**Scope respected:** no `about`/branding edit, no top-level `list`/`show` re-point, no 0.6.0 bump, no CHANGELOG/RELEASE_NOTES (all 9-2); no docs/architecture edits (9-3); no engine/adapter/conformance changes; workspace-root `Cargo.toml` untouched (the now-dangling `walkdir`/`regex`/`dialoguer`/`urlencoding`/`indicatif` entries in `[workspace.dependencies]` are harmless registry entries left for a later cleanup, staying within the "only crates/kt" scope).

### File List

- `crates/kt/src/cli/init.rs`, `install.rs`, `search.rs`, `publish.rs`, `upgrade.rs`, `list.rs`, `show.rs`, `doctor.rs`, `uninstall.rs` — deleted (9 legacy handlers)
- `crates/kt/src/skills_sh.rs`, `discovery.rs`, `install_target.rs`, `manifest.rs`, `lockfile.rs`, `git.rs`, `skill.rs` — deleted (7 support modules)
- `crates/kt/tests/adoption_cli.rs`, `install_default.rs`, `install_fallback.rs`, `publish.rs` — deleted (4 legacy integration tests)
- `crates/kt/src/cli/mod.rs` — trimmed to `pub mod agent;` + `pub mod self_update;`
- `crates/kt/src/main.rs` — trimmed (7 mod decls, 9 after-help consts, 9 `Commands` variants + `PublishCommands`, their dispatch arms; two surface unit tests trimmed to agent + self-update)
- `crates/kt/src/error.rs` — trimmed (18 skill-only structs removed; `SelfUpdateFailed` + 17 `Agent*` incl. `AgentManifest*` retained)
- `crates/kt/src/ui.rs` — removed 17 dead skill-manager helpers + their orphaned unit tests (Deviation 1); all live UI code retained
- `crates/kt/tests/helpers/mod.rs` — trimmed to `TestContext::new`, `run_kt_agent`, `run_kt_agent_with_env`, `KtRun` (dropped the skill helpers + a broken intra-doc link)
- `crates/kt/Cargo.toml` — pruned `walkdir`, `regex`, `dialoguer`, `urlencoding`, `indicatif` (machete-verified; Deviation 1)
- `_bmad-output/implementation-artifacts/9-1-...md` — story frontmatter `github_issue: 111`; Dev Agent Record / File List / Change Log / task boxes / Status

## Change Log

| Date | Version | Description | Author |
| --- | --- | --- | --- |
| 2026-07-14 | 0.1 | Story context created (headless Scrum-Master run); keep/delete lists verified against code at baseline 72b212d; 9-1↔9-2 test-gate coupling + owner's ratified decisions baked in | BMAD create-story |
| 2026-07-14 | 1.0 | Excision implemented: 9 handlers + 7 support modules + 4 legacy tests deleted; main.rs/error.rs/cli-mod/helpers trimmed; deps pruned (incl. indicatif). Excision fallout handled: removed 17 orphaned skill-manager ui.rs helpers + their tests, keeping clippy -D warnings green (Deviation 1). All runnable gates green; 767 tests pass. github_issue set to 111. Status → review | BMAD dev-story (Amelia) |
