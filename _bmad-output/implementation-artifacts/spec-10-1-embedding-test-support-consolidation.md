---
title: 'One test-support home for the embedding suites'
type: 'refactor'
created: '2026-09-10'
status: 'ready-for-dev'
review_loop_iteration: 0
baseline_commit: 999164c
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The epic-7 suites accumulated duplicated test infrastructure: 5+ near-identical manifest-TOML builders (uj3::write_flow_manifest, three in events_subscription.rs, perf-budgets write_heartbeat_manifest) and 2 lag-accumulating try_recv drain implementations (uj3::drain_receiver over the raw receiver, plus the EventSubscription-typed drain in events_subscription.rs). A schema or convention change must now be replicated by hand in every copy — the exact failure mode #164 predicted.

**Approach:** ONE parameterized test-support home in `ktesio-conformance` (the established shared dev-dep). A general manifest builder with the uj3 flow manifest as its primary preset; the drain helpers unified so both receiver forms share the logic; all suites consume the shared module. The embedding quickstart stays deliberately standalone (it is the copy-paste artifact for hosts).

## Boundaries & Constraints

**Always:**
- Zero behavior change: every suite asserts exactly what it asserted before; the consolidation is pure test-infrastructure refactoring.
- The shared home is `ktesio-conformance` (test-infrastructure role already documented); the boundary graph stays unchanged (conformance is dev-only for engine and kt; sysinfo stays the perf example's dev-dep).
- Parameterization over presets: the shared builder takes (kind, args, capabilities per-OS, metering source, optional config section) with named preset constructors for the known shapes (flow/crash/replay/heartbeat) so call sites shrink to one line each.
- The drains: ONE lag-accumulating implementation; the EventSubscription-typed surface either delegates to it or becomes a thin adapter — no duplicated Lagged math.
- The quickstart example's inline fixture is EXEMPT (host copy-paste artifact; its independence is the point).
- Standings: coverage ≥95%, docs currency (testing.md's test-inventory mentions updated if file/symbol names change), boundary gate, fmt/clippy clean.

**Ask First:**
- If consolidating a builder would change what any suite asserts or measures (e.g. the perf harness's fake_agent locator with its examples/ hop), HALT with the finding.

**Never:**
- No production (non-test) code changes; no new dependencies; no quickstart changes beyond nothing.

## Code Map

- `crates/ktesio-conformance/src/uj3.rs` — the existing shared module (write_flow_manifest, wait_for_state, committed readers, drain_receiver, stop_resilient/stop_all_resilient).
- `crates/ktesio-engine/tests/events_subscription.rs` — 3 private builders (flow/crash/replay shapes), the EventSubscription-typed drain, committed_usage_rows usage (already hoisted to uj3 in 7-3).
- `crates/ktesio-engine/examples/perf-budgets.rs` — write_heartbeat_manifest + locate_fake_agent with the examples/ hop.
- `crates/ktesio-engine/tests/embed_clean.rs` — consumes uj3 already; verify nothing further to move.
- `crates/kt/tests/agent_cli.rs` — the uj3 CLI journey + `fake_agent_manifest` local builder (:555 area).
- `crates/ktesio-conformance/src/lib.rs` — module registration.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/ktesio-conformance` — the parameterized builder + unified drain helpers, with the uj3 flow manifest as a named preset.
- [ ] `crates/ktesio-engine/tests/` — events_subscription + any engine suite consuming the shared builders (builders collapse to one-line preset calls).
- [ ] `crates/ktesio-engine/examples/perf-budgets.rs` + `crates/kt/tests/agent_cli.rs` — consume the shared builder where their shapes fit; the kt fake_agent_bin local seam consolidates or is documented-why-not.
- [ ] Docs — testing.md test-inventory wording matches the new module layout.

**Acceptance Criteria:**
- Given the consolidated module, when any suite's fixture shape changes, then exactly ONE place edits.
- Given all suites, when the full battery runs, then every assertion is unchanged and everything passes (fmt/clippy/tests/tarpaulin ≥95%/check_docs/test_automation).
- Given the boundary gate, then kt's normal/build graph is unchanged.

## Spec Change Log

## Design Notes

- The `examples/` hop in fake_agent resolution (current_exe of an example lands in target/release, not deps/) is the one reason a locator may need to stay per-binary — parameterize the shared locator instead of duplicating.
- Keep module docs stating the test-infrastructure-only role (never a driving surface for conformance claims).

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass, zero assertion changes
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `cargo +1.96.1 tree -p ktesio -e normal,build` -- unchanged
