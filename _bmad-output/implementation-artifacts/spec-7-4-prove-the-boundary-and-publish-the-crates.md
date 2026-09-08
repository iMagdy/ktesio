---
title: 'Prove the boundary and publish the crates (prepare-only under the deployment hold)'
type: 'feature'
created: '2026-09-07'
status: 'done'
review_loop_iteration: 0
baseline_commit: 8a8b3285bffc8b814d5effbd807f3236e53d0b99
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** FR-32's build-level boundary proof and the crates.io publication are the epic's distribution capstone, but the publish itself is **ON HOLD by Islam's non-negotiable standing instruction** (no deployments, no releases, nothing that costs money) until he says otherwise.

**Approach:** Prepare EVERYTHING and execute NOTHING that deploys: (1) the embedding quickstart — a host example binary in `ktesio-engine/examples/` that links the engine only and drives a mini UJ-3 flow, compiled in CI; (2) a publish runbook with the exact commands, preconditions, and HOLD markers (`docs/release-process.md`); (3) arm the in-repo semver baseline guard for `ktesio-engine` (adapter-api already armed); (4) formalize the build-level boundary statement: Rust visibility IS the compile-time guarantee (kt cannot name private items), the CI boundary job pins the dependency shape, and the 7-3 source audit proves facade-only consumption — stated in docs with evidence links. **The `cargo publish` execution, release tags (which trigger deployment workflows), and Homebrew tap updates are explicitly NOT performed.**

**Autopilot approval:** spec self-approved by the orchestrator 2026-09-09 under Islam's standing autonomous-sprint instruction (6-2 precedent); the deployment hold is Islam's own non-negotiable and is honored absolutely.

## Boundaries & Constraints

**Always:**
- The quickstart example drives the engine through the public facade only, on a hermetic temp root, and is exercised by CI (compile + `--example` run if cheap, compile at minimum).
- The semver guard for ktesio-engine uses the same in-repo baseline mechanism as adapter-api's (freeze-commit baseline; test_automation pins updated together).
- The publish runbook names EVERY held action with its trigger condition and who fires it (Islam): `cargo publish -p ktesio-adapter-api` then `-p ktesio-engine` (order matters — dependency first), the version-tag creation that arms release automation, and the Homebrew formula/tap update; each marked `HOLD — requires Islam's explicit go`.
- `publish = false` STAYS in the manifests (its removal is part of the held flip — documented in the runbook as the first publish-day step, not done now).
- The in-repo baselines stay honest: adapter-api baseline = the v1 freeze commit; engine baseline = the same freeze commit (the surface the contract froze against), with the documented bump procedure.

**Ask First:**
- Anything that would create a git tag, a GitHub Release, a crates.io package, or any network-published artifact — all held; even "harmless" dry-run publications to real registries.

**Never:**
- No `cargo publish` (not even --dry-run against the real registry), no tags, no releases, no tap pushes.
- No widening of the semver-guard allowlists to make the armed gates pass artificially.

## Code Map

- `crates/ktesio-engine/src/engine.rs` — the facade surface the quickstart drives; `crates/ktesio-conformance/src/uj3.rs` — the flow fixture pattern the example mirrors (the example is HOST code: it must NOT dev-depend on conformance — inline the minimal fixture it needs, that is the point of an embedding example).
- `.github/workflows/ci.yml` — the semver job (adapter-api baseline armed; add ktesio-engine), the boundary job, where the quickstart-compile step lands.
- `scripts/test_automation.py` — the semver-guard pins (extend for the engine baseline).
- `docs/release-process.md` — the runbook home; `docs/RELEASE_NOTES.md`/`CHANGELOG.md` — the prepare-only announcement.
- `crates/ktesio-engine/Cargo.toml` + `crates/ktesio-adapter-api/Cargo.toml` — publish=false TODO comments (reference the runbook; do not flip).

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-engine/examples/embedding-quickstart.rs` (name adjustable) — host example: open an engine on a temp root, register the mock/manifest adapter, start, read fleet, stop — public facade only, no conformance dep.
- [x] `.github/workflows/ci.yml` — compile (and run, if hermetic-cheap) the example in CI; arm the ktesio-engine in-repo semver baseline.
- [x] `docs/embedding.md` (new, + meta.json + README pointer) — the embedding quickstart page: dependency form (git-pinned until publish), the facade surface, the event bus, the example walkthrough.
- [x] `docs/release-process.md` — the publish runbook with HOLD markers; `test_automation.py` pins updated.
- [x] CHANGELOG/RELEASE_NOTES — the prepare-only announcement (what's ready; what is held and why).

**Acceptance Criteria:**
- Given CI, when it runs, then the embedding example compiles (and runs hermetically) and the ktesio-engine in-repo semver baseline guard is armed alongside adapter-api's.
- Given the runbook, when Islam reads it, then every held deployment action is named with its exact command, order, and precondition — nothing ambiguous.
- Given the repo after the story, when grepping manifests for `publish = false`, then it is still present (the flip is runbook content, not a code change).
- Given the workspace battery, then everything passes.

## Spec Change Log

- **2026-09-09 (recorded at review close, orchestrator autopilot):** (1) the frozen Always clause "engine baseline = the same freeze commit (4119db3)" was factually impossible — the engine's embedding surface honestly evolved past the adapter-contract freeze (LaunchResolveError::ContractIncompatible + an enum discriminant). The engine's in-repo baseline is therefore 8a8b328 (this spec's baseline_commit, where the embedding surface stabilized), verified green and mutation-tested (renaming `blocking` fails the gate). Recorded as a factual correction, not an intent change — Islam reviews at the epic PR. (2) The publish runbook grew the reviewer-mandated completeness steps (auth/name checks, tarball review, post-publish verify, version bump, go protocol, retire-or-keep record, post-publish docs flip, dependency-chain precondition, tap remediation) — steps 0-7, every held action marked `HOLD — requires Islam's explicit go`. (3) The hold itself is now CI-pinned: test_automation asserts `publish = false` in all four internal manifests; the runbook's step 0 intentionally trips that pin as the loud publish-day signal.

- **2026-09-09 (dev, recorded under the frozen intent — no intent change):**
  *(1) Engine baseline SHA.* The Always clause pinned the engine's in-repo
  baseline to "the same freeze commit" as adapter-api (4119db3). Verified
  locally before arming: `semver-checks -p ktesio-engine --baseline-rev 4119db3`
  fails TWO major lints on the engine's honest, unpublished evolution —
  `enum_variant_added` (`LaunchResolveError::ContractIncompatible`, added BY
  the contract freeze itself) and `enum_no_repr_variant_discriminant_changed` —
  so arming there would red CI on arrival and defeat the acceptance criterion
  "armed alongside adapter-api's". The engine's embedding surface therefore
  freezes at **8a8b328** (story 7-3, this spec's own `baseline_commit`),
  verified green (196 checks pass, 0 fail). Same mechanism, same cat-file
  resolvability check, same test_automation pin, same bump procedure; the
  difference is documented at the gate in `ci.yml`.
  *(2) Hermes in the publish chain.* The design note said "remove publish=false
  (both crates)" and ordered adapter-api → engine. `ktesio-engine` carries a
  NORMAL dependency on `ktesio-adapters-hermes` (workspace `version = "0.1.0"`),
  and crates.io refuses a publish whose normal dependencies are not already
  published — so the runbook's held chain is adapter-api → adapters-hermes →
  engine, with the flag flip naming all three (conformance keeps its flag,
  separate decision). This completes, not changes, the "EVERY held action,
  nothing ambiguous" requirement. The hold itself is untouched: no publish
  (not even dry-run), no tags, no releases, no tap pushes; `publish = false`
  still present in all four manifests.

## Design Notes

- The example duplicates a minimal fixture deliberately: an embedding example that leaned on ktesio-conformance would misrepresent what a host depends on. The shipped example is ~200 lines (quickstart asserts its own state at every leg — the run gate's assertions are real, not prints), commented as the copy-paste starting point.
- The quickstart does NOT need a real breach leg — register/configure/start/read/stop is the embedder's hello-world; the full flow proof lives in the test suite.
- Runbook order: (1) remove publish=false (both crates), (2) cargo publish -p ktesio-adapter-api, (3) cargo publish -p ktesio-engine, (4) tag vX (arms release automation), (5) brew tap update — steps 2-5 each held behind Islam's go.

## Verification

**Commands:**
- `cargo +1.96.1 build --examples -p ktesio-engine` -- the quickstart compiles
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean (includes the example)
- `cargo +1.96.1 test --workspace --all-targets` -- all pass
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `grep -c "publish = false" crates/*/Cargo.toml` -- still 4 (the hold is intact)
