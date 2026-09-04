---
title: 'Freeze and publish the Adapter Contract v1'
type: 'feature'
created: '2026-09-04'
status: 'done'
review_loop_iteration: 0
baseline_commit: 46b1804be337416d2bd840a3418f0c3202b663d8
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Adapter Contract is a seed (0.4.0) whose version string is parsed then ignored — nothing negotiates, no compatibility rule exists, the memory wire surface Epic 5 deferred is still missing, and the contract has never been proven against a second agent. Epics bind 6.6 to apply 6.4+6.5 feedback, tag contract v1, make incompatible loading fail naming BOTH versions plus the compatibility rule (FR-30), publish contract docs with the versioning/deprecation policy (NFR-7, AI-68), and freeze the deferred `--json` memory wire — v1 must NOT tag with that surface still deferred (epics.md:600-618).

**Approach:** One cohesive freeze: (1) ratify/decline each CP-6.5-a…f and resolve R1-R11 with recorded verdicts (story-file Ratified-decisions section, 6-2 precedent); (2) bump CONTRACT_VERSION to 1.0.0 and enforce an engine-side compatibility rule at manifest registration — compatible iff same major — failing with both versions + the rule named; (3) land the deferred `--json` memory wire (typed `GuaranteeLevel` snake_case adopted verbatim; exactly ONE announced key-set edit; 4-3 assertions re-pinned in the same change); (4) publish the contract docs page with versioning + deprecation policy; (5) arm the semver gate's plumbing (AI-3 cache-key fix folds in here). AI-64's independent adversarial mutation pass is mandatory for this freeze.

## Boundaries & Constraints

**Always:**
- Every CP/R verdict is RECORDED (ratified / declined / deferred-post-v1, with one-line rationale) in the story file's Ratified-decisions section and reflected in the contract docs where normative; none silently dropped (6-5's discipline).
- The compatibility rule is stated and enforced consistently everywhere it appears: crate docs, docs page, error message, tests. Recommended rule (ratify at checkpoint): compatible iff manifest major == engine major; pre-v1 (0.x) manifests are NOT grandfathered — the contract was never published, so no back-compat obligation exists.
- AI-6 verdict recorded: keep the strict `X.Y.Z` semver parse (no `v` prefix, no partials), document it in the error text and docs page (prerelease/build-metadata handling stated).
- The memory wire adopts `MemoryBackingKind`/`GuaranteeLevel` snake_case strings VERBATIM (`filesystem`/`native`, `managed_dir_byte_durable`/`home_persistence_only` — the stable keys already documented as "reused verbatim").
- The ONE announced key-set edit: new `--json` documents on the memory commands carry complete key-sets, frozen with AI-64-grade fixtures (EVERY optional field populated); the 4-3 frozen assertions (`crates/kt/tests/agent_cli.rs:4183-4330`) are re-pinned in the SAME change; announcement in CHANGELOG/RELEASE_NOTES + docs (epics.md:614-618).
- Freeze fixtures/tests cover: compatible-major loads; incompatible-major fails naming both versions + rule; the AI-6 edges (`1`, `1.0`, `v1.0.0`, prerelease strings); memory `--json` key-sets exact.
- Existing fixture fallout is fixed in-story: workspace test manifests hard-coding `0.1.0`/`0.3.0` update to `1.0.0` (or an explicitly compatible version), including the engine-observed registration asserts.

**Ask First:**
- The deprecation policy TEXT (AI-68/SS7 — owner Islam): proposed for ratification at spec checkpoint — within a major, deprecations announced ≥1 minor ahead via CHANGELOG/RELEASE_NOTES + doc notices; removals only at next major; enforced by semver-checks CI. Any Islam edit overrides.
- Ratifying CP-a's option (ii) (relaxing `has_any_support`) — recommendation: DO NOT relax; ratify CP-a option (i) (`InteractionChannelKind::Http` additive vocabulary, engine never branches on channel) only.
- The `{env:VAR}` unset-render rider on CP-d — small engine behavior change; recommendation: ratify (fail the render with a named reason), but it is the only behavioral rider.
- Any further behavior change discovered mid-story — file it post-v1 instead.

**Never:**
- No crates.io publication, no `publish = false` removal (story 7-4's job — the semver gate stays honestly dormant until then; docs must say so).
- No re-pin or alteration of 4-3's frozen key-sets beyond the ONE announced memory edit.
- No opencode/Hermes adapter code changes (ratifications are vocabulary/docs/enforcement, not adapter redesign).
- No new `kt` verb; the memory surface extends existing commands with `--json` only.

## Code Map

**Freeze seam:**
- `crates/ktesio-adapter-api/src/lib.rs:76` -- CONTRACT_VERSION "0.4.0" + history (:45-75); unit test pinning major=0 (:92-101) must bump. New: accepted-range/compat rule type or const + check fn.
- `crates/ktesio-adapter-api/src/manifest.rs:277-291` -- today's ONLY check: presence + strict `semver::Version::parse`; error texts (:180-208). `ManifestError` gains the incompatible-variant (or the check lands engine-side with its own error — decide by where the rule text reads cleanest).
- `crates/ktesio-engine/src/adapter/mod.rs:641` -- `resolve_manifest`: where registration validates; the snapshot drops contract_version today — negotiation compares HERE.
- Fixture fallout sites: workspace-wide `contract_version = "0.1.0"`/`"0.3.0"` test manifests (kt/tests/agent_cli.rs ×10, conformance tck.rs, engine tests incl. `tests/observed_metering.rs:697-732`'s 0.3.0-era assert).

**Memory wire (the ONE edit):**
- `crates/ktesio-engine/src/ports/memory_backing.rs` -- `MemoryBackingKind` :43-52 (wire :59-77), `GuaranteeLevel` :110-130 + the :93-96 freeze note, `MemoryBackingStatus.guarantee` :185-190 (the stable key reused verbatim).
- `crates/kt/src/cli/agent.rs:1672-1740` -- `memory_attach`/`memory_detach`, human-output-only today; `--json` lands here.
- `crates/kt/tests/agent_cli.rs:4183-4330` -- 4-3's frozen key-set constants + `assert_frozen_keys` (:4328); the one announced edit re-pins these.
- `docs/commands.md` + `scripts/check_docs.py` AGENT_COMMANDS (:45-59) -- command-fence allowlists need the new `--json` forms.

**Docs + CI:**
- `docs/manifest.md:58-62` -- "no negotiation" text replaced by the freeze rule.
- New docs home for the contract page (docs/adapter-contract.md + meta.json entry, or architecture.md section — pick one, check_docs.py must pass): trait surface, manifest schema pointer, capability declarations, versioning + deprecation policy, CP-derived normative text, R-resolutions, semver-gate dormancy note.
- `.github/workflows/ci.yml:388-395` -- AI-3: semver-job cache key is constant; key on the installed version/presence hash. `scripts/test_automation.py:324-371` pins the exact strings (lazy install :366, check-release :367, transient arm :368, cache key :371; coverage `needs` :117) — keep truthful.

**Decision inputs (read fully):**
- `_bmad-output/planning-artifacts/opencode-conformance-mapping-2026-09-03.md` §5-6 -- CP-6.5-a…f + R1-R11 (the ratification agenda).
- `epics.md:600-618` (6.6 ACs + the 2026-08-28 extension), `:663-669` (7-4 boundary); `prd.md:282-285` (FR-30 wording), `:353` (NFR-7), `:364-372` (SS7); `sprint-status.yaml` AI-3/AI-6/AI-64/AI-68 entries.
- `6-2-run-the-real-hermes-agent-under-ktesio-lifecycle.md:52-58` -- Ratified-decisions format precedent.

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-adapter-api` -- CONTRACT_VERSION → 1.0.0 + history entry; compat rule (same-major) as a typed const/fn; manifest validation (or engine registration) enforces it with the both-versions+rule error; AI-6 strict-parse decision documented; `--help`-adjacent error texts updated. -- FR-30's core.
- [x] `crates/ktesio-engine/src/adapter/mod.rs` + fixture fallout -- enforcement at registration + workspace fixture contract_version updates. -- The rule must actually gate loading.
- [x] `crates/kt/src/cli/agent.rs` + `crates/kt/tests/agent_cli.rs` + `docs/commands.md` + `scripts/check_docs.py` -- memory attach/detach `--json` documents (snake_case backing/guarantee verbatim, managed dir, every optional field populated in fixtures); 4-3 key-set re-pin; allowlist updates. -- The deferred wire surface, the ONE announced edit.
- [x] `docs/` (new contract page + meta.json or architecture.md section; manifest.md negotiation text; README pointer) -- versioning + deprecation policy (Islam-ratified text), CP/R normative resolutions, dormancy note. -- NFR-7 + AI-68.
- [x] `.github/workflows/ci.yml` + `scripts/test_automation.py` -- AI-3 cache-key fix, pins truthful. -- Semver plumbing armed.
- [x] `CHANGELOG.md`/`RELEASE_NOTES` (repo's announcement machinery) + story-file Ratified-decisions section -- the announcement + all CP/R verdicts. -- epics.md:614-618 announcement requirement.

**Acceptance Criteria:**
- Given a manifest whose contract major differs from the engine's, when registering, then loading fails naming BOTH versions and quoting the compatibility rule; a same-major manifest loads.
- Given the frozen contract, when reading the docs page, then the versioning policy, deprecation policy (as ratified), CP/R resolutions, and the semver-gate dormancy are all stated — no open "TBD" text.
- Given the memory commands, when invoked with `--json`, then the documents carry the typed snake_case backing/guarantee keys verbatim and their key-sets are pinned by exact-match fixtures with every optional field populated.
- Given the 4-3 frozen assertions, when the workspace test suite runs, then the re-pinned key-sets (including the ONE memory edit) pass and no other frozen set changed.
- Given the repo after the story, when reading the story file's Ratified-decisions section, then every CP-6.5-a…f and R1-R11 carries an explicit verdict — none silently dropped.
- Given AI-64, when the adversarial mutation pass runs, then every closed hole is re-verified by re-applying its original mutation, and freeze fixtures populate every optional field.

## Spec Change Log

## Design Notes

- Negotiation error shape (illustrative): `incompatible adapter contract: manifest declares 2.1.0, engine speaks 1.0.0 — compatible iff the major versions match (contract v1 policy, docs/adapter-contract.md#versioning)`.
- CP ratification recommendations to approve at checkpoint: CP-a (i) Http variant YES / option (ii) has_any_support relaxation NO; CP-b YES (documentary — note: the engine never executes a declared stop template today; text must say signal-termination is the normative stop regardless of template); CP-c YES (documentary); CP-d YES + the `{env:VAR}` unset-render-fail rider (only behavioral rider); CP-e YES (documentary + optional TCK assertion later); CP-f YES (docs-only). R-resolutions follow from those + docs stances (R7: unknown-vs-zero keeps the surfaced-not-silent labels; R9: per-instance isolated roots RECOMMENDED normative for networked adapters, docs-only; R11: per-OS honesty is already the declaration's job — windows `best_effort` cap is an adapter-author choice, not contract mandate).
- The semver gate cannot fire until 7-4 publishes — the docs page and PR description must say this plainly (honest surfaced-not-silent, no fake protection claims).
- Deprecation policy proposal for Islam's ratification is in Ask-First; if edited at checkpoint, the edited text is authoritative.

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. negotiation + memory `--json` freeze fixtures
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` -- validates (new page + allowlist entries)
- `git grep -n "contract_version = \\"0\\." -- crates/` -- no stale pre-v1 fixture versions

## Ratified decisions (6-6 checkpoint — ratified by Islam, 2026-09-04, at spec approval; CP-6.5-a…f and R1-R11 exactly as recommended in Design Notes, plus the AI-68 policy text ratified verbatim. Format follows the 6-2 precedent.)

- **CP-6.5-a → option (i) ratified: `InteractionChannelKind::Http` lands as v1 vocabulary; option (ii) (relax `has_any_support`) DECLINED.** The `http` variant is additive vocabulary — the engine never branches on the declared channel (the AD-12 stdin pipe stays unconditional) — and it is what makes an HTTP-native agent like opencode registerable with an honest, supported `interaction` declaration instead of an illegal all-unsupported one. A real engine-side HTTP `send_input` implementation is post-v1 (R1's executable TCK leg deferred with it).
- **CP-6.5-b → ratified (documentary).** An omitted `[lifecycle.stop]` means the engine's signal-termination is the normative stop and the TCK asserts child exit. Note recorded with the ratification: the engine never executes a declared stop template today, so signal-termination is the normative stop REGARDLESS of template; the template is the adapter's forward declaration.
- **CP-6.5-c → ratified (documentary).** `self-reported` explicitly covers adapter-implemented shims over the agent's own usage surfaces; `sequence` derives from the agent's per-Run message ordinals; the wire form is unchanged.
- **CP-6.5-d → ratified (documentary) PLUS the `{env:VAR}` unset-render rider — the ONLY behavioral rider of the freeze.** Env-content delivery is the documented pattern for JSON-config agents; the format-qualified `file` target is reserved post-v1. The rider: an adapter to a `{env:VAR}`-substituting agent must guarantee every referenced var is set — or fail the render with a named reason — before the child launches; a silently-empty rendered config is a contract violation.
- **CP-6.5-e → ratified (documentary + optional TCK assertion later).** Self-updating agents must have their update-disable mechanism mapped and delivered; adapter docs must disclose any higher-authority (managed/MDM) config layer that could override supervisor-delivered values.
- **CP-6.5-f → ratified (docs-only).** Isolation keys are named per agent (opencode: `XDG_DATA_HOME` + `XDG_CONFIG_HOME`) with the explicit churn caveat (undocumented lever; re-validate per pinned release).
- **R1 → settled by CP-a.** Vocabulary in v1 (`http`), delivery out; TCK interaction leg stays the fail-fast unsupported probe for stdin-less adapters.
- **R2 → normative path named.** For opencode-class agents both v1 variants are conformant: a `self-reported` shim (dedup rides `sequence`-as-message-ordinal) or `engine-observed` (inherently per-request); FR-19's no-double-count guarantee binds in both, stated in the metering docs.
- **R3 → wire form frozen.** `GuaranteeLevel`'s snake_case strings (`managed_dir_byte_durable`/`home_persistence_only`) and `MemoryBackingKind` (`filesystem`/`native`) adopted VERBATIM on the new `memory attach|detach --json` documents — the ONE announced key-set edit; adapters that cannot consume `memory.dir` declare no mapping (delivered nowhere, honestly surfaced as `declared: false`/start-time notice).
- **R4 → settled by CP-b.** No graceful-stop acknowledgment concept enters v1 (Hermes' exit-75 hand-off needs no special case: any non-zero exit while Running is a crash handled by the restart policy).
- **R5 → settled by CP-d.** Env-content blessed for v1; format qualifier reserved.
- **R6 → undocumented levers acceptable WITH obligation.** Named in adapter docs + per-release re-validation; never silently load-bearing.
- **R7 → keeps the surfaced-not-silent labels.** Unknown usage is never coerced to zero: a zero event asserts the provider reported zero; an omitted event is unknown. Docs stance, both sources.
- **R8 → deferred post-v1.** No readiness-vs-liveness concept and no endpoint-discovery surface in the v1 contract; pin-the-port or startup-line parsing are documented adapter conventions, not contract machinery.
- **R9 → per-instance isolated roots RECOMMENDED normative (docs-only).** Each instance's isolation keys inside its own Agent Home make shared-root concurrency out of contract scope; shared-root modes are not blessed.
- **R10 → agent-side abort declared out-of-contract.** Verbs stay start/stop/pause/resume; engine-owned termination semantics; the TCK asserts process-level outcomes. Mapping abort as an interruption surface is a post-v1 idea.
- **R11 → per-OS honesty is already the declaration's job.** A windows `best_effort` cap for WSL-recommended agents is an adapter-author choice, not a contract mandate; no constraint added.
- **AI-6 → strict `X.Y.Z` parse KEPT and documented.** No `v` prefix, no partials (`1`, `1.0`); the rejection text states the requirement, prerelease/build-metadata parses and negotiates by MAJOR only, and the docs page states the stance. Test buckets pin the edges (`1`, `1.0`, `v1.0.0`, `1.0.0-rc.1`).
- **AI-68 (deprecation policy, SS7) → text ratified VERBATIM as proposed, recorded as Islam-ratified 2026-09-04** wherever the policy is announced (contract docs page + CHANGELOG/RELEASE_NOTES): within a major, deprecations announced ≥1 minor ahead via CHANGELOG/RELEASE_NOTES + doc notices; removals only at next major; enforced by semver-checks CI (dormant until 7-4 — stated plainly).
- **AI-3 → cache-key fix folded in.** The semver job's binary cache keys on the resolved `cargo-semver-checks` version, installs that pinned version, and saves only after a real install; `scripts/test_automation.py` pins the new strings.

## AI-64 adversarial mutation record

The independent adversarial mutation pass RAN on 2026-09-04 (per the AI-64 mandate — a reviewer who did not write the gate): **15 mutations applied and reverted; 14 killed; 1 hole found** (the full mutation list survives in the 6-6 review). The surviving hole, **M3** — no fixture negotiated a different-major value carrying a prerelease/build suffix, so `major ==` flipped to `major == … || !pre.is_empty()` passed every suite — is CLOSED in this story's fix pass: `negotiate_contract_version("2.0.0-rc.1")` / `"2.0.0+build.9"` / `"2.0.0-rc.1+build.9"` / `"0.9.0-beta"` fixtures pin `Incompatible` in the adapter-api tests, mirrored in the engine's incompatible-major fixture set (`incompatible_major_fails_naming_both_versions_and_the_rule`), and the closing mutation was re-applied by re-running the same flip against the new fixtures before reverting. The independent pass's second finding — the kt-side FR-30 proof was unit-only — is closed by the new e2e test (`register_incompatible_contract_manifest_exits_one_naming_both_versions_and_the_rule` drives the real binary: exit 1, both versions + rule on stderr, stdout empty, nothing registered). Post-fix verification: fmt/clippy/test/tarpaulin(≥95)/check_docs all green on the final tree.

Implementing-agent sweep (pre-review, retained for the record): the fixtures populate every optional field — both memory kinds, both guarantee levels, and both delivery facts (`declared` true via the mock's `memory.dir` mapping, false via native) materialize on the wire — plus the in-process serializer key-set pins, stdout-purity assertions, and the negotiation-after-validation ordering pin. AI-6's strict-parse requirement text is a single shared const (`STRICT_SEMVER_REQUIREMENT`) interpolated by BOTH rejection sites with drift asserts.

Announcement-placement mechanics (review of the fix pass, finding 17): the freeze notice lives in `CHANGELOG.md`/`docs/RELEASE_NOTES.md` as a header BANNER ABOVE the first `## ` heading — NOT as a `## Unreleased` section — because `scripts/generate_release_docs.py::upsert_release_section` inserts every generated release section directly above the first `## ` heading (a `## Unreleased` heading would be pushed below the fold at the very next tag; verified by simulating the insert regex against the edited file). The script never touches the header region, so the banner stays visible through the next release cut; at that cut the release author moves the banner's content into the release section and deletes the banner.

Scope ruling recorded (review of the fix pass): a reviewer demand to BUILD engine-side enforcement of CP-6.5-d's `{env:VAR}` unset-render rider in this story was REJECTED as out of scope — the checkpoint ratified the rider as documentary normative text, and the spec's Ask-First boundary files any further behavior change post-v1. The prose-only stance is the shipped scope; enforcement tooling remains a post-v1 candidate.
## Suggested Review Order

**The freeze seam (FR-30)**

- CONTRACT_VERSION 1.0.0, the compatibility rule, the typed error naming both versions, and the negotiate fn.
  [`lib.rs:100`](../../crates/ktesio-adapter-api/src/lib.rs#L100)

- Enforcement at the single registration load gate — after per-field validation, before any adapter work.
  [`mod.rs:695`](../../crates/ktesio-engine/src/adapter/mod.rs#L695)

- The CLI-facing rejection: distinct diagnostic + remediation, exit 1 (no new exit-code number).
  [`exit_code.rs:139`](../../crates/kt/src/exit_code.rs#L139)

**The ONE announced key-set edit (memory wire)**

- The frozen documents: schema_version 1, snake_case backing/guarantee verbatim, JSON-only read-back.
  [`agent.rs:253`](../../crates/kt/src/cli/agent.rs#L253)

- AI-64-grade freeze fixtures — every optional field populated, exact key-set asserts, both kinds/guarantees.
  [`agent_cli.rs:4335`](../../crates/kt/tests/agent_cli.rs#L4335)

- The e2e link: real binary registers a 2.1.0 manifest → exit 1, both versions + rule named, stdout empty.
  [`agent_cli.rs:4810`](../../crates/kt/tests/agent_cli.rs#L4810)

**The mutation-pass story (AI-64)**

- The hole the independent pass caught: different-major prerelease/build must stay Incompatible.
  [`lib.rs:261`](../../crates/ktesio-adapter-api/src/lib.rs#L261)

**Docs + policy (NFR-7/AI-68)**

- The contract page: trait surface, CP/R verdict table, Versioning + verbatim ratified deprecation policy.
  [`adapter-contract.md:1`](../../docs/adapter-contract.md#L1)

**CI plumbing (AI-3)**

- Version-resolved cache key + the verify-or-reinstall guard + the garbage-version regex.
  [`ci.yml:393`](../../.github/workflows/ci.yml#L393)

**Peripherals**

- Ratified-decisions section: every CP-6.5-a…f, R1-R11, AI-6/AI-68/AI-3 verdict with rationale.
  [`sprint-status.yaml:94`](../../_bmad-output/implementation-artifacts/sprint-status.yaml#L94)
