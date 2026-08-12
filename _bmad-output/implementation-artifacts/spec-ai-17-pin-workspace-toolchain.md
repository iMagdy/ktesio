---
title: 'AI-17: Pin workspace toolchain to 1.96.1 via rust-toolchain.toml'
type: 'chore'
created: '2026-07-06'
status: 'done'
baseline_commit: '721c6d9b7401478fde624c65da5e86f990cb2ebb'
context:
  - '{project-root}/docs/testing.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The workspace MSRV is 1.96.1 (bundled `libsqlite3-sys 0.38.1` needs `cfg_select!`, stabilized in 1.96.1), but a bare `cargo` here resolves to a lower toolchain (mise pins rust and, absent a toolchain file, falls back below MSRV), so contributors must type `cargo +1.96.1` for every gate or the build fails with E0658.

**Approach:** Add a root `rust-toolchain.toml` pinning channel `1.96.1` with `clippy`+`rustfmt` and `profile = "minimal"`, so bare `cargo build/test/clippy/fmt` auto-select 1.96.1. Because a root toolchain file makes bare `cargo` in CI resolve to 1.96.1 too, reconcile CI so its "latest stable" jobs stay honestly on stable (explicit `cargo +stable`) while the MSRV gate keeps proving the 1.96.1 floor — preserving BOTH coverages.

## Boundaries & Constraints

**Always:** Keep CI covering both (i) the pinned 1.96.1 floor and (ii) forward-compat on latest `stable`. Keep every existing job's intent. Keep `rust-version = "1.96.1"` in `Cargo.toml` and the MSRV job in lockstep. Keep `Cargo.lock` `--locked`-consistent. Keep the exact CI strings that `scripts/test_automation.py` asserts (`rustup toolchain install 1.96.1 --profile minimal`, `cargo +1.96.1 check --workspace`).

**Ask First:** Changing the pinned version away from 1.96.1; changing what any job fundamentally tests (e.g. dropping stable coverage or the MSRV gate); modifying the release workflow's toolchain in a way that changes which compiler ships artifacts.

**Never:** Do not edit `sprint-status.yaml` or touch GitHub. Do not commit. Do not change `rust-version` in `Cargo.toml`. Do not pin release artifact builds to 1.96.1 (release must stay on `stable` for shipped binaries) — verify the pin does not silently force that. Do not use a `[toolchain]` `components`/`targets` set that forces extra downloads on release jobs.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Local bare cargo | Repo root has `rust-toolchain.toml` (channel 1.96.1) | `cargo --version` = 1.96.1; full gate suite runs without `+1.96.1` | N/A |
| Explicit MSRV still works | `cargo +1.96.1 …` | Still resolves 1.96.1 (unchanged) | N/A |
| CI stable jobs | `rust-toolchain.toml` present + `stable` installed | fmt/clippy/test/build/boundary/semver/coverage run on **stable** (explicit), catching future stable regressions | N/A |
| CI MSRV job | `rust-toolchain.toml` present | `msrv` job still builds workspace on 1.96.1 via `cargo +1.96.1 check` | Fails if code needs > 1.96.1 |
| Release build jobs | tag push, `stable` + target installed | release artifacts still built with **stable** (not forced to 1.96.1) | N/A |

</frozen-after-approval>

## Code Map

- `rust-toolchain.toml` -- NEW at repo root; the pin. `[toolchain]` channel=1.96.1, components=[clippy,rustfmt], profile=minimal.
- `Cargo.toml` -- `[workspace.package] rust-version = "1.96.1"` (line 18); the MSRV the pin must match. Read-only reference.
- `.github/workflows/ci.yml` -- jobs fmt/clippy/test/build/boundary/semver/coverage run bare `cargo` on `stable`; `msrv` job uses `cargo +1.96.1`. Stable jobs must become explicit so the pin doesn't silently switch them to 1.96.1.
- `.github/workflows/release.yml` -- installs `stable` + target, runs bare `cargo build --release --target …`; must stay on stable despite the pin.
- `scripts/test_automation.py` -- `test_ci_enforces_msrv_floor` (lines 157-170) locks the MSRV job strings + `rust-version`; other tests lock stable-job command substrings. Any CI edit must keep these passing.
- `scripts/check_docs.py` -- validates docs links/fences; `docs/testing.md` edits must stay link/fence-valid.
- `docs/testing.md` -- "Required Checks" bare-cargo commands; add a note that the pin makes them use 1.96.1.

## Tasks & Acceptance

**Execution:**
- [x] `rust-toolchain.toml` -- CREATE at repo root: `[toolchain]` with `channel = "1.96.1"`, `components = ["clippy", "rustfmt"]`, `profile = "minimal"`. Add a top comment tying it to `Cargo.toml` `rust-version` and the MSRV rationale. -- makes bare cargo auto-select 1.96.1.
- [x] `.github/workflows/ci.yml` -- Make the stable jobs explicit: for jobs fmt, clippy, test, build, boundary, semver, coverage, change their run invocations from bare `cargo …` to `cargo +stable …` (keep their `rustup toolchain install stable …` install steps unchanged). Leave the `msrv` job exactly as-is (`rustup toolchain install 1.96.1 --profile minimal` + `cargo +1.96.1 check --workspace`). -- preserves latest-stable coverage now that the pin would otherwise redirect bare cargo to 1.96.1.
- [x] `.github/workflows/release.yml` -- Make both `cargo build --release …` (build job) and `cargo publish` / `cargo metadata` (publish job) explicit with `+stable` so shipped artifacts + the publish stay on stable, not 1.96.1. Keep `rustup toolchain install stable …` steps. -- prevents the pin from silently changing the release compiler.
- [x] `scripts/test_automation.py` -- IF (and only if) an existing assertion breaks because a locked substring changed, update the assertion to match the new explicit-toolchain command AND keep asserting the MSRV strings. Prefer NOT weakening existing MSRV/stable assertions. -- keeps the automation gate honest. (Updated 6 substring assertions to `+stable` forms; MSRV assertions preserved; added a new assertion locking the `rust-toolchain.toml` pin to `channel = "1.96.1"`.)
- [x] `docs/testing.md` -- In "Required Checks", add a short note: bare `cargo` now uses the pinned 1.96.1 via `rust-toolchain.toml`; `+1.96.1` remains valid but is no longer required locally. -- documents the new local ergonomics.

**Acceptance Criteria:**
- Given the repo root has `rust-toolchain.toml`, when I run bare `cargo --version` in the repo, then it reports `1.96.1`.
- Given the pin exists, when I run the full gate suite with bare `cargo` (fmt/clippy/test/tarpaulin) plus the python + grep gates, then all pass with no `+1.96.1`.
- Given the pin exists, when I run `cargo +1.96.1 --version`, then it still reports 1.96.1 (explicit form unbroken).
- Given `scripts/test_automation.py`, when it runs, then all tests pass — including the MSRV-floor assertions.
- Given CI, when it runs, then the stable jobs test on `stable` and the `msrv` job still proves the 1.96.1 floor (both coverages preserved).
- Given the release workflow, when a tag is pushed, then release artifacts are built with `stable`, not forced onto 1.96.1.

## Spec Change Log

- **2026-07-06 (review, iteration 1) — doc-accuracy patch (no code loopback).** Blind-hunter + edge-case-hunter reviews both flagged that the `docs/testing.md` "Toolchain" note and the `rust-toolchain.toml` header comment overclaimed "bare `cargo` automatically uses 1.96.1" — false when `RUSTUP_TOOLCHAIN` / a `rustup override` / a `cargo`-shimming version manager (mise/asdf) takes precedence over the toolchain file. Hard-verified on this machine (mise-driven): bare `cargo --version` = 1.94.1 and `cargo check --workspace` exits 101 until `RUSTUP_TOOLCHAIN` is unset. Amended both to add the precedence caveat + a `rustup show` / `unset RUSTUP_TOOLCHAIN` / `cargo +1.96.1` recovery hint, and documented the intentional local-MSRV-vs-CI-stable lint skew. Classified `patch` (docs only; the `[toolchain]` body and all CI/release logic were correct and unchanged — the acceptance auditor found no AC violations). Known-bad avoided: a contributor on a version-managed box trusting the doc and silently building off-MSRV (or hitting a confusing floor error with no pointer to the cause). KEEP: the explicit `+stable` on every CI/release stable-job cargo invocation and the untouched `msrv` job (`cargo +1.96.1 check --workspace`) — this is the load-bearing reconciliation and must survive any re-derivation. Deferred (not this story): propagating the caveat to other contributor docs, and a pre-existing tarpaulin CI caching asymmetry — both in `deferred-work.md`.

## Design Notes

Toolchain-file precedence: rustup honors `rust-toolchain.toml` for bare `cargo`, but a `+toolchain` argument (and the `RUSTUP_TOOLCHAIN` env var) overrides the file. So `cargo +stable` on CI stable jobs deterministically stays on stable even with the pin present, and `cargo +1.96.1` (MSRV job, local escape hatch) stays on 1.96.1. This is why the reconciliation uses explicit `+stable` on stable jobs rather than deleting the toolchain file or relying on job-local env.

Chosen `rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.96.1"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

## Verification

**Commands:**
- `cargo --version` -- expected: `cargo 1.96.1 …` (bare, no `+1.96.1`).
- `cargo fmt --all --check` -- expected: clean, no diff.
- `cargo clippy --workspace --all-targets -- -D warnings` -- expected: zero warnings.
- `cargo test --workspace --all-targets` -- expected: all tests pass; record count.
- `cargo tarpaulin --engine llvm --workspace --fail-under 95` -- expected: coverage ≥ 95%; record %.
- `python3 scripts/check_docs.py` -- expected: "Validated N Markdown files", exit 0.
- `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` -- expected: all tests OK.
- `cargo +1.96.1 --version` -- expected: still 1.96.1 (explicit form intact).
- OS-cfg grep gate + boundary gate (from ci.yml `boundary` job) -- expected: both green.

## Suggested Review Order

**The pin**

- Start here — the whole change exists to make bare `cargo` = MSRV; note the override caveat in the comment.
  [`rust-toolchain.toml:14`](../../rust-toolchain.toml#L14)

**CI reconciliation (the real work — both coverages preserved)**

- Stable jobs made explicit `+stable` so the pin doesn't silently switch them off latest stable.
  [`ci.yml:31`](../../.github/workflows/ci.yml#L31)

- The MSRV floor gate — left untouched on `+1.96.1`; this is what still proves 1.96.1.
  [`ci.yml:301`](../../.github/workflows/ci.yml#L301)

- Boundary + semver gates also pinned to `+stable` (dependency-law, API semver).
  [`ci.yml:157`](../../.github/workflows/ci.yml#L157)

**Release stays on stable**

- Shipped artifacts + crates.io publish forced to `+stable`, not the MSRV pin.
  [`release.yml:45`](../../.github/workflows/release.yml#L45)

- The publish/metadata calls in the publish job.
  [`release.yml:129`](../../.github/workflows/release.yml#L129)

**Docs + guard (peripherals)**

- Contributor-facing note, incl. the mise/`RUSTUP_TOOLCHAIN` caveat added in review.
  [`testing.md:23`](../../docs/testing.md#L23)

- Automation assertions updated to the `+stable` forms; MSRV assertions preserved; new pin assertion added.
  [`test_automation.py:180`](../../scripts/test_automation.py#L180)
