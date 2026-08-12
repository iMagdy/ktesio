---
type: sprint-change-proposal
date: 2026-07-13
author: Correct-Course workflow (BMAD), for Islam
status: proposed
change_scope: MODERATE  # new epic + Epic-8 re-scope + flagged PRD FR amendments
trigger: "Implementation reality (the shipped kt CLI) diverged from the completed product pivot (agent runner). CLI still self-describes and behaves as the retired skill manager while the live docs describe only the agent runner."
proposes: "New Epic 9 — Retire the Legacy Skill-Manager CLI"
inputDocuments:
  - _bmad-output/planning-artifacts/epics.md
  - _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md
  - _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md
  - _bmad-output/implementation-artifacts/sprint-status.yaml
  - crates/kt/src/main.rs
  - docs/architecture.md
verified_against_code: true
---

# Sprint Change Proposal — Retire the Legacy Skill-Manager CLI

## Section 1 — Issue Summary

**Problem.** Ktesio completed its product pivot from a spec-kit-style **skill manager** to an **AI agent runner** at the planning and feature level — Epics 1–3 are merged to `main` (metering / budgets / cost-control live), and the README + `docs/` were repositioned to describe *only* the agent runner (`kt agent …`). But the **shipped `kt` binary never completed the pivot.** The command surface still is, and still presents itself as, the old skill manager.

**Evidence (verified against source, not memory):**

- `crates/kt/src/main.rs:173` — the top-level clap `about` is still `"Agentic skills package manager"`. `crates/kt/Cargo.toml:8` repeats it as the crates.io `description`, and `keywords = ["agentic", "skills", "package-manager", "cli"]` (both ship on the next publish).
- The `Commands` enum (`main.rs:181-307`) still wires **nine legacy skill-manager subcommands**: `init`, `install`, `search`, `publish` (+ `publish add`), `upgrade`, `list`, `show`, `doctor`, `uninstall` (alias `remove`). Their `*_AFTER_HELP` blocks still document `skills.json` / `.agents/skills` / `kt install docs:owner/repo/…` / `skills.sh`.
- Two of them collide by name with the new surface: top-level **`kt list` / `kt show`** are *skill* commands, distinct from `kt agent list` / `kt agent show`.
- Nine legacy handler modules back them (`cli/{init,install,search,publish,upgrade,list,show,doctor,uninstall}.rs`) over seven support modules exclusively theirs: `skills_sh.rs`, `discovery.rs`, `install_target.rs`, `manifest.rs`, `lockfile.rs`, `git.rs`, `skill.rs`.
- Legacy tests still exercise them: `crates/kt/tests/{adoption_cli,install_default,install_fallback,publish}.rs`, the skill-only half of `tests/helpers/mod.rs`, and two unit tests in `main.rs` (`test_cli_subcommands_exist`, `test_subcommand_help_includes_details_and_examples`).
- `docs/architecture.md` has a stale tail (L136-222): a **Modules** list naming `lockfile.rs` / `manifest.rs` / `skills_sh.rs`, a **Command Flow** for Install / Search / Publish / Upgrade, **Design Choices** about the JSON manifest/lockfile, and a **See Also** linking `manifest.md` / `lockfile.md`. `docs/lockfile.md` still documents `skills.lock`.

**Why now.** The CLI and the already-live docs are inconsistent: `kt --help` advertises a product the docs no longer describe. Islam has approved retiring the legacy CLI as a **new epic** to finish the pivot before the first pivoted release ships. This is a genuine course correction because the original plan (Epic 8, stories 8-4/8-5) intended to *keep* the legacy commands with deprecation notices for a ≥90-day window (FR-37/FR-38) — the "retire now" direction supersedes that plan and must be reconciled against it.

## Section 2 — Impact Analysis

### Epic impact

| Epic | Status | Impact |
|------|--------|--------|
| Epics 1–3 | done (merged) | None. Agent runner, metering, config, secrets are untouched. |
| Epics 4–7 | backlog | None. No dependency in either direction. |
| **Epic 8 — Provision Skills and Migrate Legacy Users** | backlog | **Directly affected.** Story **8-4** (deprecate-in-place with notices) is **superseded** by retirement. Story **8-1** (relocate `manifest/lockfile/git/install_target` into `engine::skills` and shim the legacy commands over them, AD-16) has its **premise removed** — the modules it planned to relocate-and-reuse are being deleted. Stories **8-2/8-3** (provision skills to an *agent*, FR-35/36) survive but must build skill provisioning **in the engine**, not by relocating kt modules. Story **8-5** (channel continuity, FR-39) survives and overlaps this epic's channel-safety guard. |
| **New Epic 9** | proposed | Retire the legacy CLI; complete the pivot. |

### Story impact

- **No in-flight or done story is modified.** This is purely additive plus a re-scope of not-yet-started Epic 8.
- New: Epic 9 stories 9-1, 9-2, 9-3 (below), and an optional 9-4 gated on the migration decision.

### Artifact conflicts (what must change)

**Code — `crates/kt/`** (verified; the skill cluster is self-contained):
- `src/main.rs` — remove the 9 legacy `Commands` variants + `PublishCommands` + their dispatch arms + their `*_AFTER_HELP` consts; rewrite `about`; rewrite the two legacy-asserting unit tests. **Keep** `Agent` and `SelfUpdate`.
- `src/cli/` — delete `init.rs, install.rs, search.rs, publish.rs, upgrade.rs, list.rs, show.rs, doctor.rs, uninstall.rs`; trim `mod.rs`. **Keep** `agent.rs`, `self_update.rs`.
- Support modules — delete `skills_sh.rs, discovery.rs, install_target.rs, manifest.rs, lockfile.rs, git.rs, skill.rs`. **Keep** `install_channel.rs`, `update_check.rs` (self-update), `ui.rs`, `error.rs`.
- `src/error.rs` — remove skill-only variants (`ManifestNotFound/DuplicateName/InvalidName`, `LockfileNotFound/Invalid`, `Git*`, `Skill*`, `InstallInvalidFormat`, …). **Keep the `Agent*` family and `SelfUpdateFailed`.** ⚠️ The skill `Manifest*` variants (skills.json) go; the distinct **`AgentManifest*`** variants (adapter.toml) stay.
- `Cargo.toml` — rewrite `description` + `keywords`; prune `[dependencies]` left unused after the excision (compiler / `cargo-machete`-verified — do **not** assume; several deps such as the archive/verify stack are still used by `self-update`).
- Tests — delete `tests/{adoption_cli,install_default,install_fallback,publish}.rs`; trim the skill-only helpers out of `tests/helpers/mod.rs` (keep `TestContext::new`, `run_kt_agent`, `run_kt_agent_with_env`, `KtRun`). **`tests/agent_cli.rs` needs no change** — it has zero skill-manager references (the earlier "skill parts of agent_cli" note did not hold up against the code).

**Docs — `docs/`:**
- `architecture.md` — re-author or remove the L136-222 tail (Modules / Command Flow / Design Choices / See Also). **Architect-owned** (touches the spine + AD-16).
- `lockfile.md` — remove or repurpose (documents `skills.lock`). `manifest.md` — verify (it no longer references skill *commands*; confirm whether it documents skills.json or already `adapter.toml`, and fix the architecture "See Also" links accordingly).
- **Keep** `commands.md` (already agent-only; documents `kt agent …` and `kt --version` and — correctly — does *not* document top-level `kt list`/`show`), `get-started.md`, `installation.md`, `troubleshooting.md`, `README.md`.
- **Do NOT rewrite** `CHANGELOG.md:174` or `docs/RELEASE_NOTES.md:179` — those are historical release records. Instead, the *new* pivot release's notes should state the removal (FR-38 "removal at a stated version").
- `AGENTS.md` says Ktesio "is being repositioned from a skills package manager" — transitional wording; optional to finalize after the pivot ships.

### Technical impact

- **Clean excision, verified.** The seven support modules are referenced only by the nine legacy handlers and by each other; `cli/agent.rs` imports only `crate::error::Agent*` and `crate::ui`; `cli/self_update.rs` imports only `SelfUpdateFailed`, `install_channel`, `ui`. Removing the legacy surface cannot reach the agent runner or self-update.
- **Gates.** NFR-3 coverage ≥95% on `src/` — removing large untested/handler surface generally helps, but the dev must keep the remaining `src/` above the bar and green. The AD-2 crate-visibility / semver gates are unaffected (kt already depends only on the engine's public API). The single-currency-formatter grep-lint is unaffected.
- **Coverage-runner OOM (#101 / AI-31 / AI-36)** remains the standing CI reality; this epic neither fixes nor worsens it.

## Section 3 — Recommended Approach

**Direct adjustment — add a new Epic 9 and re-scope not-yet-started Epic 8.** No rollback (nothing to revert; Epics 1–3 stand). No MVP-goal reduction (this *advances* the MVP by making the shipped product match its stated identity).

- **Sequence:** run Epic 9 **next** (it has no dependency on Epics 4–7) and, non-negotiably, **before the first pivoted release** so the released `kt` binary matches the already-live repositioned docs. It may run in parallel with Epic 4 (disjoint files).
- **Effort:** small–moderate. 9-1 is a mechanical deletion with a compile-and-gate loop; 9-2 is surface-and-test rewrites; 9-3 is documentation/architecture. ~2–3 focused dev sessions plus one architect pass.
- **Risk:** low on the code (self-contained excision); the real risk is the **product/migration decision** for existing v0.5.0 users (Section 6).

## Section 4 — Detailed Change Proposals: **Epic 9**

### Epic 9: Retire the Legacy Skill-Manager CLI

Complete the agent-runner pivot in the shipped binary. Remove the retired skill-manager command surface, its exclusively-legacy modules, and its tests; re-brand the top-level `kt` identity from "Agentic skills package manager" to the agent runner; and reconcile the stale architecture/skill docs — so `kt --help` and the shipped behavior match the already-repositioned README and `docs/`. Preserve the `kt` name, the `ktesio` crates.io package, the install channels, and `kt self-update` (FR-39). This supersedes Epic 8 Story 8-4 (deprecate-in-place) and re-scopes Story 8-1 / AD-16 (the skills machinery it planned to relocate is being deleted).

**Depends on:** Epics 1–3 (done). **Blocks:** the first pivoted release. **Independent of:** Epics 4–7.

---

#### Story 9.1 — Remove the legacy skill-manager command surface, modules, and tests

As the Ktesio maintainer,
I want the retired skill-manager commands and every module and test that exists only to serve them deleted from the `kt` crate,
So that the binary carries only the agent runner (plus binary self-maintenance) and no dead skill-manager code.

**Acceptance Criteria**

**Given** the current `kt` crate
**When** the excision lands
**Then** the `Commands` enum no longer contains `Init`, `Install`, `Search`, `Publish` (+ `PublishCommands`), `Upgrade`, `List`, `Show`, `Doctor`, or `Uninstall`/`remove`, and their dispatch arms and `*_AFTER_HELP` constants are gone
**And** `cli/{init,install,search,publish,upgrade,list,show,doctor,uninstall}.rs` and the support modules `skills_sh.rs, discovery.rs, install_target.rs, manifest.rs, lockfile.rs, git.rs, skill.rs` are deleted, and `cli/mod.rs` no longer declares them
**And** `error.rs` retains the `Agent*` family and `SelfUpdateFailed` but drops the skill-only variants (`Manifest*` for skills.json, `Lockfile*`, `Git*`, `Skill*`, `InstallInvalidFormat`, …) — the **`AgentManifest*`** adapter.toml variants are explicitly preserved
**And** `crates/kt/tests/{adoption_cli,install_default,install_fallback,publish}.rs` are deleted and `tests/helpers/mod.rs` keeps only the agent helpers (`TestContext::new`, `run_kt_agent`, `run_kt_agent_with_env`, `KtRun`); `tests/agent_cli.rs` is unchanged
**And** `Cargo.toml` `[dependencies]` are pruned to those still used after the excision (verified by the compiler and `cargo-machete`/`cargo-udeps`), while every dependency still used by the agent or `self-update` path is retained
**And** `kt agent …` and `kt self-update` behave exactly as before, and all nine CI gates are green: `cargo build --release`, `fmt --check`, clippy `-D warnings`, `test --all-targets`, tarpaulin `--fail-under 95` on `src/`, crate-visibility, semver-check, currency grep-lint, MSRV 1.96.1 (coverage-runner OOM #101 notwithstanding — prove ≥95% locally per the established practice)

**Scope guard:** `self-update` and its modules (`cli/self_update.rs`, `install_channel.rs`, `update_check.rs`) are **kept** — they are binary distribution/maintenance, not skill management, and were grandfathered in Story 1.1's AC "until epic 8 relocates/deprecates them." Retiring them is out of scope for this epic (see Risks).

---

#### Story 9.2 — Reposition the top-level `kt` identity to the agent runner

As an Operator,
I want `kt --help`, `kt --version`, and the crate metadata to present Ktesio as the agent runner,
So that the tool describes itself the way the README and `docs/` already do, with one canonical way to list/show the Fleet.

**Acceptance Criteria**

**Given** the removals from Story 9.1
**When** identity repositioning lands
**Then** the top-level clap `about` (`main.rs`) no longer says "Agentic skills package manager" and instead describes the agent runner, and `crates/kt/Cargo.toml` `description` + `keywords` match (no "skills"/"package-manager" framing)
**And** `kt --help` lists only agent-runner-relevant top-level commands (`agent`, `self-update`) and no retired skill command appears
**And** top-level `kt list` / `kt show` are **removed**, making `kt agent list` / `kt agent show` the single canonical surface (matching `docs/commands.md`, which already omits the top-level forms) — see the reconciliation decision in Section 6
**And** the `main.rs` unit tests are rewritten: `test_cli_subcommands_exist` asserts the *new* surface (`agent`, `self-update` present; `init`/`install`/`search`/`upgrade`/`publish`/`list`/`show`/`doctor`/`uninstall`/`remove` absent) and `test_subcommand_help_includes_details_and_examples` iterates the surviving commands; `test_cli_help_includes_license_and_repository`, `test_self_update_skips_passive_update_check`, and every `test_agent_*` test stay green
**And** the pivot release's `CHANGELOG.md` / `RELEASE_NOTES.md` entries (new entries — historical ones untouched) state the retired commands and the version at which they were removed (FR-38 "removal at a stated version")

---

#### Story 9.3 — Reconcile the stale architecture and skill-manager docs (architect-owned)

As a reader of Ktesio's docs,
I want the architecture document and the residual skill-manager docs to describe only the shipped product,
So that no doc contradicts the retired CLI, and the AD-16 / Epic-8 skills plan reflects reality.

**Acceptance Criteria**

**Given** `docs/architecture.md` L136-222
**When** the reconciliation lands
**Then** the stale **Modules** block, the Install/Search/Publish/Upgrade **Command Flow**, the skill-oriented **Design Choices**, and the **See Also** links to `manifest.md`/`lockfile.md` are re-authored to the agent-runner architecture or removed — no reference to `skills_sh.rs`/`lockfile.rs`/`manifest.rs` or `skills.json`/`skills.lock` remains as current architecture
**And** `docs/lockfile.md` is removed or repurposed, and `docs/manifest.md` is verified (skills.json → removed, or already `adapter.toml` → kept) with the architecture "See Also" links corrected
**And** the **AD-16** spine item (skills machinery relocation + legacy shims) and **Epic 8** are updated to reflect that the legacy CLI and its modules are retired: Story **8-4** is marked superseded, Story **8-1**'s "relocate existing kt modules" premise is replaced by "build agent skill-provisioning in `engine::skills`," and Stories **8-2/8-3/8-5** are re-anchored accordingly (proposals recorded for Islam's ratification, not applied unilaterally to the PRD)
**And** `commands.md`, `get-started.md`, `installation.md`, `troubleshooting.md`, `README.md` are confirmed free of retired-command references

**Ownership flag:** this story is **architect-owned (Winston)** — it edits the architecture spine and re-scopes AD-16/Epic-8, which is beyond a mechanical doc edit. The dev may execute the pure `docs/architecture.md` tail rewrite under the architect's direction; the AD-16/Epic-8 re-scope needs the architect + Islam.

---

#### Story 9.4 — *(optional, gated on the migration decision)* Emit kind removal notices for retired commands

As a v0.5.0 user upgrading to the pivot,
I want a clear, one-line message when I run a retired command,
So that I am told the tool became an agent runner and where to migrate, instead of a bare "unrecognized subcommand" error.

**Acceptance Criteria (only if Option B in Section 6 is chosen)**

**Given** a retired command name (e.g. `kt install`)
**When** it is invoked on the pivot release
**Then** `kt` exits non-zero with exactly one stderr line naming the retirement, the agent-runner replacement, the migration doc, and the version of removal — honoring the FR-37/FR-38 "loudly and kindly" intent without retaining any skill machinery (implemented as hidden, behavior-free clap stubs or a top-level unknown-subcommand interceptor)
**And** the notice is covered by a test and appears on stderr only (never stdout)

## Section 5 — Implementation Handoff

**Scope classification: MODERATE.** Backlog reorganization (new epic + Epic-8 re-scope) plus flagged PRD amendments. Routing:

- **Developer (Amelia)** — Stories 9-1 and 9-2 (mechanical excision + surface/test rewrite), and, under the architect's direction, the pure `docs/architecture.md` tail rewrite in 9-3.
- **Architect (Winston)** — owns Story 9-3's architecture-spine + AD-16 + Epic-8 re-scope.
- **Islam (product owner)** — ratifies the two decisions in Section 6 (migration approach + FR-37/38 amendment; top-level `list`/`show` removal) before 9-2 finalizes, and the required release version bump.

**Success criteria:** `kt --help` presents the agent runner; no retired command exists; agent + self-update behavior identical; all nine gates green (coverage proven locally); no doc describes the retired product as current; the `kt` name / `ktesio` package / install channels / `self-update` are intact (FR-39 preserved).

### Sprint-status entries to add — **verbatim** (for the orchestrator to integrate; this workflow did **not** edit `sprint-status.yaml`)

Append to the `development_status:` map, after the `epic-8-retrospective: optional` line:

```yaml
  epic-9: backlog
  9-1-remove-the-legacy-skill-manager-command-surface-and-modules: backlog
  9-2-reposition-the-top-level-kt-identity-to-the-agent-runner: backlog
  9-3-reconcile-the-stale-architecture-and-skill-manager-docs: backlog
  epic-9-retrospective: optional
```

If Islam chooses Option B (Section 6), also add — after the `9-3-…` line:

```yaml
  9-4-emit-kind-removal-notices-for-retired-commands: backlog
```

Suggested `last_updated` note fragment for whoever next edits the header (do not clobber the concurrent retro's edits): `Epic 9 (Retire the Legacy Skill-Manager CLI) scoped via correct-course 2026-07-13 — sprint-change-proposal-2026-07-13.md; 3 stories (+1 optional), MODERATE; supersedes 8-4, re-scopes 8-1/AD-16; run before the first pivot release.`

### GitHub sync — for the orchestrator (this workflow did **not** touch GitHub)

- Create **one epic issue** "Epic 9: Retire the Legacy Skill-Manager CLI" and **three story issues** (9-1, 9-2, 9-3; +9-4 if Option B), titles carrying the BMAD keys above, bodies mirroring epics.md, added to Project [Ktesio #5]. **Do not renumber** existing epic issues #55–#62 or story issues #63–#99.
- On the **Epic 8** issue and its 8-1 / 8-4 story issues, add a cross-reference note: 8-4 superseded by Epic 9; 8-1/AD-16 premise changed. Do not close them here — the architect's 9-3 re-scope decides their final wording.
- Re-run the idempotent sync via `_bmad-output/implementation-artifacts/github_sync.py` and update `github-sync-map.json` with the new keys.

## Section 6 — Risks, Decisions, and Migration Notes

### Decisions required from Islam (do not proceed past 9-2 without these)

1. **Migration approach for existing v0.5.0 users (the FR-37/FR-38 reconciliation — the crux).**
   The last *published* release (0.5.0 on crates.io/Homebrew) is the skill manager. The pivot (Epics 1–3) is on `main`, **unreleased** (workspace still `0.5.0`, no new tag). Epic 8 Story 8-4 (which would have shipped the legacy commands *with* deprecation notices for a ≥90-day/one-minor window per FR-38) **never shipped**. So the pivot's *first* release will drop the skill commands with **no prior in-tool deprecation notice**.
   - **Option A — clean break (recommended for a pre-1.0 pivot):** retire now; ship the pivot as a version bump (≥ 0.6.0, or 1.0.0) with the removal stated in the release notes + README and a generic clap "unrecognized subcommand" error. Simplest; treats FR-38's window as satisfied-by-release-notes at a major pivot boundary. **Skip Story 9-4.**
   - **Option B — honor FR-38's spirit without the machinery:** retire the machinery now but keep hidden, behavior-free command **stubs** that print a kind one-line removal notice pointing at the migration path (Story 9-4). Softer for anyone who upgrades in place.
   - **Option C — literal FR-38:** ship one interim pivot release that keeps the legacy commands working *with* notices (i.e. actually do Story 8-4 first), wait the window, then run Epic 9. Slowest; contradicts "retire now" and delays the honest first release. **Not recommended.**
   This is a **product decision** — flagged, not made. Recommend **A**, or **B** if in-place upgraders matter.

2. **PRD amendment (FR-37 / FR-38).** Whichever option is chosen, FR-37 ("legacy commands remain functional with a deprecation notice") and FR-38 (the announce→window→remove lifecycle) need an explicit ruling in the PRD. Recommended restatement: FR-37 → "the legacy skill-manager surface is **removed** at the pivot release"; FR-38 → "the removal is announced in the pivot release's notes/README at a stated version" (Option A) or "…preceded by hidden removal-notice stubs for one minor" (Option B). **Not edited unilaterally** — proposed here for Islam per the boundary on PRD product-intent changes.

3. **Top-level `list` / `show` reconciliation.** Recommend **removal** (canonical = `kt agent list` / `kt agent show`; matches `docs/commands.md`, which already omits the top-level forms). Alternative, if muscle-memory ergonomics matter: keep `kt list` / `kt show` as thin **aliases** to the agent forms. Recommend removal; folded into Story 9-2.

### Risks

- **`cargo install ktesio` / the `kt` binary name — NOT broken.** The crates.io package stays `ktesio` and the binary stays `kt` (both unchanged; FR-39). `cargo install ktesio`, the Homebrew tap, and the install scripts keep producing a working `kt`. What changes is the binary's *commands*, not its identity or install path. Story 9-1's scope guard keeps `self-update` so in-place upgrades keep working. **This epic must explicitly preserve FR-39** — call it out in the release checks (overlaps Epic 8 Story 8-5).
- **crates.io / Homebrew consumers.** Anyone scripting `kt install …` / `kt search …` against the *published 0.5.0* is unaffected until they upgrade; on upgrade those scripts break (that is the intended retirement). The release notes must say so. No downstream crate depends on the `kt` binary's library surface (kt is a bin, not a lib).
- **Epic 8 / AD-16 architectural interaction.** Deleting `manifest/lockfile/git/skill` now discards the "battle-tested install/lock machinery" AD-16 planned to relocate for agent skill-provisioning (8-2/8-3). Mitigation: 8-2/8-3 build provisioning in `engine::skills` fresh (lifting from git history if useful); Story 9-3 records the re-scope. Keeping the modules as dead code in `kt` is explicitly rejected (violates the ≥95% coverage gate and the no-dead-code convention).
- **Coverage gate under a shrinking `src/`.** Removing handler code shifts the coverage denominator; the dev must confirm the remaining `src/` stays ≥95% (proven locally given the #101 runner OOM).
- **Version bump required.** The pivot release needs a deliberate bump (≥ 0.6.0 or 1.0.0). Out of this epic's code scope but a hard precondition of the release this epic gates.
