---
title: 'Prove the boundary and publish the crates (prepare-only under the deployment hold)'
type: 'feature'
created: '2026-09-09'
status: 'ready-for-dev'
review_loop_iteration: 0
baseline_commit: SET_AT_IMPLEMENTATION
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
- [ ] `crates/ktesio-engine/examples/embedding-quickstart.rs` (name adjustable) — host example: open an engine on a temp root, register the mock/manifest adapter, start, read fleet, stop — public facade only, no conformance dep.
- [ ] `.github/workflows/ci.yml` — compile (and run, if hermetic-cheap) the example in CI; arm the ktesio-engine in-repo semver baseline.
- [ ] `docs/embedding.md` (new, + meta.json + README pointer) — the embedding quickstart page: dependency form (git-pinned until publish), the facade surface, the event bus, the example walkthrough.
- [ ] `docs/release-process.md` — the publish runbook with HOLD markers; `test_automation.py` pins updated.
- [ ] CHANGELOG/RELEASE_NOTES — the prepare-only announcement (what's ready; what is held and why).

**Acceptance Criteria:**
- Given CI, when it runs, then the embedding example compiles (and runs hermetically) and the ktesio-engine in-repo semver baseline guard is armed alongside adapter-api's.
- Given the runbook, when Islam reads it, then every held deployment action is named with its exact command, order, and precondition — nothing ambiguous.
- Given the repo after the story, when grepping manifests for `publish = false`, then it is still present (the flip is runbook content, not a code change).
- Given the workspace battery, then everything passes.

## Spec Change Log

## Design Notes

- The example duplicates a minimal fixture deliberately: an embedding example that leaned on ktesio-conformance would misrepresent what a host depends on. Keep it ~60-100 lines, commented as the copy-paste starting point.
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
