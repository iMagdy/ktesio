---
title: 'Prove any adapter with the conformance test-kit'
type: 'feature'
created: '2026-08-31'
status: 'done'
review_loop_iteration: 0
baseline_commit: 613d4ec978dc79f04a5d7fce29adfab3abc0915b
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The Adapter Contract (ktesio-adapter-api) has no systematic proof that an adapter actually honors it — "identical controls" across adapters is aspirational. ktesio-conformance ships only fixtures (MockAdapter, fake_agent, hermes_shim); the TCK it promises does not exist, and no third-party adapter has a test harness to invoke.

**Approach:** Build the TCK as a public `ktesio-conformance` entry point that runs every contract-section suite against any adapter the caller registers, reports per-capability compliance, and is invoked from the story's own integration tests both ways: against the mock (all sections) and against the Hermes adapter (all sections applicable to its declaration). Ship the mock variant as a ready-made public harness function.

## Boundaries & Constraints

**Always:**
- Every TCK capability section runs against the SAME live instance lifecycle as the engine integration tests (real supervisor, real registry) — no shadow reimplementation of engine behavior.
- Per-capability compliance is explicit: each section reports `pass`/`fail`/`not_applicable` with a machine-readable reason string; `not_applicable` requires the declaration to justify it (e.g. `pause: unsupported`).
- A failed section must never abort the run silently mid-suite: the report names every failed section and at least the first failure reason per section.
- The harness stays a library API in ktesio-conformance (any adapter crate adds it as a dev-dependency and calls it from its own `#[test]`), following the established `fake_agent_bin()` precedent — no test-runner framework, no macros that hide panics.
- The Hermes-applicability pass respects Hermes' actual declaration: Pause BestEffort (all OSes), Interaction Guaranteed, SelfReported metering only — no section may demand EngineObserved of a SelfReported adapter.
- The TCK covers BOTH Metering Sources: self-reported (KTESIO_USAGE sentinels, replay-dedup) AND engine-observed loopback.
- Manifest-constructed adapters and native adapters both exercise the TCK path (the mock fixtures cover native; a manifest adapter.toml pointing at fake_agent covers manifest).
- CI contract honored: binaries the harness spawns are built explicitly and stale copies removed before tests (same pattern as existing fake_agent/hermes_shim CI steps).

**Ask First:**
- Any change to ktesio-adapter-api types (contract is pre-freeze but still additive-only without Islam's sign-off).
- Any edit that would re-pin or alter Story 4-3's frozen `--json` key-set or `schema_version` assertions.
- If making Hermes pass a section requires changing ktesio-adapters-hermes behavior beyond tests/fixtures (the story proves conformance; it does not redesign the adapter).

**Never:**
- No contract freeze, no CONTRACT_VERSION bump to 1.x, no version-negotiation semantics — those belong to 6-6. CONTRACT_VERSION stays 0.4.0.
- No opencode characterization or contract change proposals — Story 6-5's deliverable.
- No engine/adapter-api redesign to make a section pass; contract-shape problems found by the TCK are recorded as findings for 6-5/6-6, not fixed here.
- No `kt` CLI surface changes (the TCK is a library/test harness, not a command).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unsupported capability requested | Declaration marks `pause: unsupported` on an OS | Section reports `not_applicable` with reason naming the declaration | N/A |
| Declared-but-failing capability | Adapter declares Pause Guaranteed but the pause op fails | Section reports `fail` with the adapter error in the reason | Report continues; other sections still run |
| Wrong usage sequence replay | Two KTESIO_USAGE lines with the same `sequence` | Reconciliation counts the batch once (existing idempotent rule) | `fail` only if double-count observed |
| Crash mid-lifecycle | fake_agent `--crash-after-ms` fires | Crash transition + restart policy exercised as a lifecycle section | Report shows the section result, not a harness panic |
| Section precondition unmet (Hermes) | EngineObserved section vs SelfReported Hermes | Section skipped as `not_applicable` | Report justifies from the declaration |

</frozen-after-approval>

## Code Map

- `crates/ktesio-conformance/src/lib.rs` — TCK home. Exports MockAdapter (L65), ScriptStep/ScriptedFakeAgent (L140-167), `fake_agent_bin()` (L248-274, the cross-crate binary-resolution seam; staleness contract doc L220-243). The public TCK entry point(s) land here.
- `crates/ktesio-conformance/Cargo.toml` — deps today: only ktesio-adapter-api. Will need ktesio-engine as a dev-dep (mirroring ktesio-engine's dev-dep on conformance — check for a dev-dep cycle; if cyclic, run TCK suites in ktesio-engine's tests instead and export only the report types from conformance).
- `crates/ktesio-adapter-api/src/adapter.rs` — AgentAdapter trait (L60): kind/capabilities/metering_source/config_mapping + lifecycle ops with default Unavailable bodies.
- `crates/ktesio-adapter-api/src/capability.rs` — CapabilityDeclaration (L113), Capability = {Pause, Interaction} seed set (L65), SupportLevel {Guaranteed, BestEffort, Unsupported} (L29), `effective(os)` (L186).
- `crates/ktesio-adapter-api/src/metering.rs` — MeteringSource {SelfReported, EngineObserved} (L61); KTESIO_USAGE stdout sentinel convention with `sequence` replay-dedup (module docs L1-55).
- `crates/ktesio-adapter-api/src/manifest.rs` — Manifest/OpTemplate/Lifecycle (L44-138) for the manifest-adapter TCK pass.
- `crates/ktesio-engine/src/engine.rs` — Engine::register/register_with_adapter (L1030/1035), Engine::send_input (L672, Interaction-unavailable fast-fail).
- `crates/ktesio-engine/src/adapter/builtin.rs` — BuiltinMock (L104) + native table (L60); shipping `--kind mock` twin of the conformance fixture; parity test contract.
- `crates/ktesio-engine/src/adapter/mod.rs` — resolve_config_mapping (L365)/apply_config_mapping (L445); manifest loading seam.
- `crates/ktesio-engine/tests/` — behavior reference for each TCK section (do NOT duplicate, port assertions): lifecycle.rs (554L), crash.rs (254L, `--crash-after-ms`/`--crash-times`+`--crash-state`), pause.rs (517L), budget.rs (814L, `--emit-usage` sentinels), metering.rs (486L), observed_metering.rs (766L, `--observed-calls` loopback), memory.rs (854L, attach/detach/self-heal; "same sequence works on mock AND manifest adapter" L361), interaction.rs (878L), registration.rs (238L, parity test L183).
- `crates/ktesio-adapters-hermes/src/lib.rs` — Hermes declaration: Pause BestEffort all OS (L86-88), Interaction Guaranteed (L90-103), SelfReported (L126), config mapping only memory.dir→HERMES_HOME (L133).
- `crates/ktesio-conformance/src/bin/fake_agent.rs` — spawnable agent; usage sentinel emission (L213-216), crash/heartbeat/observed-calls flags.
- `.github/workflows/ci.yml` — fake_agent/hermes_shim explicit-build + stale-removal steps (test job L144-160, coverage job L589-612); coverage crate list L622 includes ktesio-conformance.
- `scripts/test_automation.py` — asserts CI contract lines (L142-170, L204); must be updated if TCK adds spawned binaries.

## Tasks & Acceptance

**Execution:**
- [x] `crates/ktesio-conformance/src/lib.rs` -- Design and export the TCK report types (per-section `pass`/`fail`/`not_applicable` + reason) and the public harness entry point(s): `conformance_harness(adapter)`-style API returning the report, plus a prebuilt `run_mock_conformance()` wrapper. Sections: lifecycle transitions (incl. crash), config mapping, self-reported metering, engine-observed metering, memory attachment, interaction, capability-declaration edge cases. Keep the API thin — it registers the caller's adapter with a fresh engine and drives the same flows the engine integration tests use. -- This is the FR-27 deliverable. (Landed as `src/tck.rs`: `ConformanceReport`/`SectionReport`/`SectionResult`, `run_conformance()` + `run_mock_conformance()`, 8 sections.)
- [x] `crates/ktesio-conformance/` (new `tests/` or in-lib) -- Wire each section's assertions, ported from the engine integration tests listed in the Code Map; every section must be reachable against the mock via the harness, with a manifest-adapter (adapter.toml + fake_agent) pass for lifecycle/crash/config sections. -- The TCK must actually exercise, not just enumerate. (29 lib tests + `tests/third_party_manifest.rs`.)
- [x] `crates/ktesio-conformance/src/lib.rs` (Hermes pass) -- Run the harness against the Hermes adapter within engine tests (hermes is a workspace crate); assert every section applicable to Hermes' declaration passes and the EngineObserved section reports not_applicable (SelfReported). -- AC line 2: "Hermes passes all sections applicable to its declaration." (`crates/ktesio-engine/tests/hermes_tck.rs`.)
- [x] `crates/ktesio-conformance/Cargo.toml` + `crates/ktesio-engine/Cargo.toml` -- Wire the engine dev-dep in the direction that avoids a dependency cycle (conformance→engine dev-dep preferred; if impossible, TCK integration tests live in ktesio-engine/tests and only report types export from conformance). -- Structural decision, cheapest first. (conformance→engine is a normal dep; engine→conformance stays dev-only; kt boundary graph verified unchanged.)
- [x] `.github/workflows/ci.yml` + `scripts/test_automation.py` -- Extend the build/stale-removal contract for any newly spawned binary (likely none beyond fake_agent); keep the per-crate coverage list intact. -- CI must stay green and the asserted contract truthful. (No change needed: TCK spawns only fake_agent + hermes_shim, both already covered by the existing rm+explicit-build pairs.)
- [x] `docs/` (conformance/adapter docs page) + `README.md` -- Document the TCK: what a third-party adapter crate adds as a dev-dependency, the entry-point signature, the report shape, and per-section semantics. -- Docs currency gate. (README.md, docs/testing.md, docs/architecture.md.)

**Acceptance Criteria:**
- Given the TCK run against the mock adapter, when all sections execute, then each of lifecycle (incl. crash), config mapping, both metering sources, memory attachment, interaction, and capability edge cases reports pass, and the report is machine-readable per capability.
- Given a declaration marking `pause: unsupported`, when the pause section runs, then it reports not_applicable with a reason naming the declaration, and no harness panic occurs.
- Given the Hermes adapter, when the TCK runs, then all sections applicable to its declaration pass and SelfReported/EngineObserved applicability matches its metering declaration.
- Given a third-party adapter crate (simulated by the manifest-adapter pass), when it invokes the harness from a `#[test]`, then it compiles against ktesio-conformance alone as a dev-dependency and yields the same report shape.
- Given any section failure, when the report is produced, then every failed section is named with at least its first failure reason and the harness completes rather than aborting mid-suite.

## Spec Change Log

## Design Notes

- The TCK is a *compliance report*, not a test framework: it returns data; the caller's `#[test]` asserts on it. This keeps it usable from any crate without a custom runner and matches the "cargo test harness any third-party adapter crate can invoke" AC wording.
- Applicability is derived from the declaration under test (CapabilityDeclaration.effective(os) + MeteringSource), never hardcoded per adapter — that is what makes the same harness honest for Hermes (BestEffort pause is still *applicable*: it must demonstrate the best-effort path, not skip).
- A dev-dep cycle risk exists by construction: engine already dev-depends on conformance. If conformance needs the engine to drive lifecycles, put the TCK integration suites in ktesio-engine/tests (where fake_agent_bin is already the established seam) and export only the report types + section definitions from ktesio-conformance. Decide by trying the preferred direction first.
- Hermes-in-CI note: existing hermes.rs tests resolve via hermes_shim PATH shim + `--dump`; the Hermes TCK pass must use the same recorded/sandboxed pattern, not a live gateway.

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` -- clean
- `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass, including new TCK suites
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- coverage holds ≥95% (ktesio-conformance is in the per-crate list)
- `python3 scripts/check_docs.py` -- docs validate

## Suggested Review Order

**Entry point — the report contract**

- The public seam: registers the caller's adapter with a fresh engine, returns a report; never panics.
  [`tck.rs:325`](../../crates/ktesio-conformance/src/tck.rs#L325)

- pass/fail/not_applicable verdicts with reasons; `schema_version` + serde round-trip is the CI-gate contract.
  [`tck.rs:185`](../../crates/ktesio-conformance/src/tck.rs#L185)

- The never-panic boundary: any harness panic becomes a complete all-fail report naming the payload.
  [`tck.rs:340`](../../crates/ktesio-conformance/src/tck.rs#L340)

**Section pipelines — applicability always derived from the declaration**

- Registration + fresh-engine lifecycle; `stop_leftovers` sweeps orphaned probes before returning.
  [`tck.rs:362`](../../crates/ktesio-conformance/src/tck.rs#L362)

- Persisted capability projection vs the declaration — pure defect function, unit-tested arms.
  [`tck.rs:925`](../../crates/ktesio-conformance/src/tck.rs#L925)

- Start→running→stop transition sequence plus a crash leg on a TCK-authored twin manifest.
  [`tck.rs:988`](../../crates/ktesio-conformance/src/tck.rs#L988)

- Pause honesty per declared level; `pause_demo` makes Windows+Guaranteed an honest not_applicable, never a silent best-effort pass.
  [`tck.rs:1202`](../../crates/ktesio-conformance/src/tck.rs#L1202)

- Config mapping scope: empty/flag-file-only declarations read not_applicable; env rules proven through the `--dump` artifact.
  [`tck.rs:1565`](../../crates/ktesio-conformance/src/tck.rs#L1565)

- Self-reported metering: three committed rows, replay dedup pinned by `replay_row_defect`, fleet totals equal ledger.
  [`tck.rs:1731`](../../crates/ktesio-conformance/src/tck.rs#L1731)

- Engine-observed metering against a loopback upstream stub (EngineObserved declarations only).
  [`tck.rs:1824`](../../crates/ktesio-conformance/src/tck.rs#L1824)

- Memory attach/detach with exact-path delivery; interaction mirrors the declared level (`interaction_probe_level`).
  [`tck.rs:2136`](../../crates/ktesio-conformance/src/tck.rs#L2136)

**Polling seams — committed-state waits, fail-fast diagnostics**

- Terminal-state fail-fast: a wrong terminal state names the actual state instead of spinning 30 s.
  [`tck.rs:646`](../../crates/ktesio-conformance/src/tck.rs#L646)

**Consumers — the harness run both ways**

- Third-party simulation: conformance crate as sole dev-dep, report asserted per section (the copy-me file).
  [`third_party_manifest.rs:63`](../../crates/ktesio-conformance/tests/third_party_manifest.rs#L63)

- The shipping Hermes builtin passes every section applicable to its declaration; EngineObserved reads not_applicable.
  [`hermes_tck.rs:94`](../../crates/ktesio-engine/tests/hermes_tck.rs#L94)

**Test battery — the TCK's own detection is pinned**

- Fail-path mutations die here: pause cause defects, replay over-count, interaction no-echo, config scope fast paths.
  [`tck.rs:2265`](../../crates/ktesio-conformance/src/tck.rs#L2265)

**Peripherals**

- Dependency wiring: conformance→engine normal, engine→conformance dev-only — kt's boundary graph untouched.
  [`Cargo.toml`](../../crates/ktesio-conformance/Cargo.toml)

- Adapter-facing docs: dev-dependency + one `#[test]`; internal work-item refs scrubbed.
  [`README.md`](../../README.md)

- The TCK chapter: report shape, per-section semantics, subject-vs-probe honesty table.
  [`testing.md:79`](../../docs/testing.md#L79)
