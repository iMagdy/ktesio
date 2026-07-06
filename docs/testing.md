---
title: Testing
description: Required checks, integration fixtures, coverage gates, and documentation validation for Ktesio contributors.
---

# Testing

The test suite covers unit behavior, CLI workflows, and local git fixtures.

## Required Checks

Run these before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
python3 scripts/check_docs.py
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

## Unit and Integration Tests

```bash
cargo test --workspace --all-targets
```

Integration tests create local temporary git repositories. They do not require network access. The agent-lifecycle tests spawn a small cross-platform helper binary (`fake_agent`, in `ktesio-conformance`) as a real child process to prove start, stop, launch-failure, no-survivor, pause/resume, crash-detection, restart, and orphan-adoption behavior end to end; it is a dev/test artifact and never ships. For pause, `fake_agent --heartbeat-ms <ms>` prints a periodic incrementing line, so a guaranteed (Unix) pause is provable — the heartbeat stops growing under `SIGSTOP` and resumes under `SIGCONT`. For survival, `fake_agent --crash-after-ms <ms>` runs normally past the readiness window and then exits non-zero, simulating an unrequested crash so the reaper detects it and the Restart Policy fires. The crash/restart legs (`tests/crash.rs`) prove a `never`-policy crash lands `failed` with a `crashed` cause and no restart, and an `on-failure` crash is automatically restarted by the reaper (the crash-loop-stops-at-5 and count-reset legs run in the supervisor unit tests with an injected fast backoff, so they never sleep for real seconds while production keeps the 1s×2/60s constants). The engine-kill adoption test (`tests/adoption.rs`) is the NFR-1 proof: it runs a first engine in a subprocess that starts an agent and exits WITHOUT a graceful stop (a `kill -9` model — no destructors run, so the agent survives and re-parents to init), then opens a new engine over the same state dir and asserts the live child is adopted (row `running`, a subsequent `stop` truly kills it, no orphan remains) while a record whose process is gone reconciles to `failed`; the AI-7 (paused-live process adopted and resumable) and AI-8 (phantom `running` row → `failed`) cases ride the same file.

## Cross-platform testing (3-OS matrix)

The per-OS process-control code lives only under `crates/ktesio-engine/src/backends/{unix,windows}` (the sole place OS-conditional compilation is allowed). This code cannot be verified on a single operating system — the Windows Job-Object backend does not even compile on Linux. The CI `test` job therefore runs on a matrix of `ubuntu-latest`, `macos-latest`, and `windows-latest`:

- Linux and macOS run the Unix backend (process groups, `SIGTERM`/`SIGKILL`, and `SIGSTOP`/`SIGCONT` for the guaranteed pause) on both Unixes.
- Windows runs the Windows backend (Job Objects, `TerminateJobObject`), the only place its behavior — real spawn, terminate, no-survivor, and the cooperative best-effort pause — is actually exercised. The best-effort pause is honest by surfacing a qualifier (a `pause-best-effort` transition cause plus a CLI stderr note), never a silent fake; that path rides this `windows-latest` leg and is compile-checked only on Unix.

Only the `test` job matrixes; the other jobs (fmt, clippy, build, docs, boundary, semver, msrv, coverage) stay Linux-only.

## Coverage

CI runs `cargo tarpaulin --workspace --fail-under 95` as the coverage gate, on Linux only. To run it locally:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --fail-under 95
```

On macOS the default ptrace engine is unavailable — tarpaulin errors with `missing section: CoverageFunctions` — so run the LLVM source-based engine instead (this is how the gate is reproduced on a macOS dev host):

```bash
cargo tarpaulin --engine llvm --workspace --fail-under 95
```

Generate an HTML report:

```bash
cargo tarpaulin --out Html
```

### Coverage honesty for per-OS code

`cargo tarpaulin` runs on Linux and cannot instrument `#[cfg(windows)]` code, so the 95% gate is measured on Linux against the OS-agnostic core plus the Unix backend (which compiles and runs on the Linux tarpaulin host, so its lines are covered). The Windows backend's lines are `cfg`-excluded on Linux and never enter the Linux coverage denominator, so the reported percentage is honest for what Linux can see. The Windows backend's correctness is proven instead by the `windows-latest` matrix `test` run passing — a real Job-Object spawn, terminate, and no-survivor check — not by a coverage number. The `fake_agent` helper binary runs only as a spawned subprocess, so it too is excluded from coverage (its behavior is proven by the tests that spawn and kill it).

## Documentation Checks

```bash
python3 scripts/check_docs.py
```

The docs check validates:

- Root and `docs/` Markdown links.
- JSON fenced code blocks.
- Documented `kt` command examples.
- Stale links to old repository names or generated spec quickstarts.

## Release Script Checks

```bash
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
```

This verifies the release-note generator can handle a first-release style tag when no previous tag exists.

## Automation Helper Tests

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

These tests cover release-note and changelog rendering, Homebrew formula
generation, installer dry-run decisions, and CI/workflow expectations.
