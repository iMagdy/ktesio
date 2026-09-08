---
title: 'Benchmark the performance budgets'
type: 'feature'
created: '2026-09-07'
status: 'done'
review_loop_iteration: 0
baseline_commit: b0aeed8
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** NFR-4's budgets (read commands <1s on a 25-instance Fleet; supervision overhead ≤2% CPU and ≤50MB RSS per running instance) are PRD placeholders marked "for architecture to validate" — nothing measures them, so they are asserted, not real (the story's own words).

**Approach:** A **performance harness** (an engine example or an explicitly-gated test binary) that builds a real 25-instance fleet fixture (manifest agents on fake_agent, a subset RUNNING) and measures: (1) read-command latency through the public facade — fleet listing, per-instance status, usage — over the 25-instance fleet; (2) steady-state supervision overhead — the ENGINE process's aggregate CPU% and RSS divided by running-instance count, sampled over a window while instances idle. Results print a machine-readable report and gate against the ratified budgets; a **designated CI perf job** runs the harness and fails on regressions. **Autopilot approval** recorded (Islam's standing autonomous instruction).

## Boundaries & Constraints

**Always:**
- The harness drives the PUBLIC facade only (an embedding view of perf — consistent with 7-1/7-3).
- The 25-instance fixture is hermetic (temp roots, fake_agent, established fixture shape) and tears down after itself.
- Gates: read p-anything latency budget <1s (report p50/p95/p99; gate on p99 < 1s), RSS-per-running-instance ≤ 50MB, CPU-per-running-instance ≤ 2%. CI-noise honesty: the CPU gate carries a DOCUMENTED shared-runner tolerance factor (measured-and-reported against the budget; the strict budget is what local runs gate); reads and RSS gate at budget. All numbers land in the job log as the measured record.
- If a budget FAILS on honest local measurement (not CI noise), the story's AC offers the alternative: the measured values replace the budgets via a documented update — that update is a DOCS/PRD change routed to Islam in the epic PR description, NOT silently applied.
- New measurement dependencies (e.g. a process-metrics crate) are DEV-dependencies only, named in the spec record (NFR-8's lean policy governs runtime deps; a benchmark-gated dev-dep with justification is the autopilot decision).

**Ask First:**
- Any runtime (non-dev) dependency for measurement.
- If the CPU/RSS measurement requires platform-specific code that cannot be made honest cross-platform in-story, scope the GATE to the platforms where it is honest and document the rest as measured-and-reported.

**Never:**
- No perf regressions introduced to game the gates; no wall-clock-sleep-based "steadiness" claims beyond the documented sampling window; no CI flakiness traded for budget strictness without the documented tolerance.

## Code Map

- `crates/ktesio-engine/src/engine.rs` — facade reads to measure: fleet(), instance_status(), effective_config/usage reads.
- `crates/ktesio-engine/tests/uj3_library_host.rs` + `crates/ktesio-conformance/src/uj3.rs` — the fixture pattern (manifest authoring, register, start) to replicate ×25.
- `crates/ktesio-engine/tests/embed_clean.rs` — the two-engine concurrent pattern (threads per engine) reusable for fleet-wide instance setup.
- `.github/workflows/ci.yml` — where the designated perf job lands (ubuntu; gated separately from the standard test matrix so flakiness cannot block ordinary PRs).
- `docs/testing.md` / NFR-4 in the PRD — where the measured record and any budget update proposal land.

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-engine/examples/perf-budgets.rs` (or a gated test binary — implementer's call, documented) — the harness: 25-instance fixture, read-latency measurement (p50/p95/p99 over N iterations), steady-state CPU%/RSS sampling over a documented window, machine-readable report, budget gates with the documented CI tolerance.
- [x] `.github/workflows/ci.yml` — the designated perf job running the harness (ubuntu, explicitly named, allowed to fail ONLY if the tolerance policy says so — else blocking per the AC).
- [x] `docs/testing.md` — the perf story: how to run it locally, what the budgets are, the CI tolerance policy, where the measured record lives.
- [x] If a budget fails honestly: the measured-values update proposal documented for Islam (not applied).

**Acceptance Criteria:**
- Given the perf job, when it runs on a 25-instance fleet, then read latency, CPU-per-instance, and RSS-per-instance are MEASURED (printed) and gated per the documented policy.
- Given a regression (reads over budget or RSS over budget), when the perf job runs, then it fails.
- Given the harness locally, when run, then the strict budgets gate without CI tolerance.
- Given the workspace battery, then everything passes and the perf harness is excluded from ordinary test runs (explicit gating, not `#[ignore]` sprawl).

## Spec Change Log

- **2026-09-09 (review close):** NFR-4's three budgets MEASURED and validated — reads p99 5.9-6.6ms vs <1000ms (~170x headroom), CPU/instance 0.69-0.75% vs 2% (~2.9x), RSS/instance ~1.03MiB vs 50MiB (~48x). No budget update needed; the measured record is committed in docs/testing.md and the CI job uploads the JSON report as an artifact. 3 lenses → 17 patches (gate math under test, blocking structurally pinned, sysinfo dev-dep sanctioned as NFR-8-compatible).

## Design Notes

- CPU measurement without a new runtime dep is the hard part: prefer a dev-dep process-metrics crate (sysinfo is the standard; dev-only, benchmark-gated, NFR-8-compatible — record it) over hand-rolled /proc parsing; Windows needs the crate anyway.
- Steady-state definition: instances RUNNING and idle (fake_agent heartbeat only), engine supervising, sampled over ≥10s after settle.
- RSS-per-instance = (engine process RSS) / (running instances) — the agents' own memory is their own, not supervision overhead; state that in the report.
- Keep the 25-instance fixture startup OUT of the timed windows (setup, settle, then measure).

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass (the harness is NOT part of the default test run)
- `cargo +1.96.1 run --example perf-budgets -p ktesio-engine` (or the gated invocation) -- report produced, budgets gated
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
