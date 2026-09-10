---
title: 'Host-provided diagnostic sink'
type: 'feature'
created: '2026-09-10'
status: 'ready-for-dev'
review_loop_iteration: 0
baseline_commit: 77531b3
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The engine writes two operational diagnostics straight to stderr with `eprintln!` (the DC-10 memory-delivery notice and the enforcement breadcrumb — the only two, pinned by embed_clean's audit allowlist). A host embedding the engine owns stderr; uninvited writes pollute it and force the audit allowlist to exist. Retro item (7-3 deferred, ratified in the epic review round).

**Approach:** An **additive, opt-in diagnostic sink** on the engine: a host-provided writer (any `std::io::Write`) that the two diagnostic sites emit through when installed, defaulting to the current stderr behavior when not. The default path is byte-identical to today. This closes the deferred item by making the diagnostics routable, not by silencing them.

## Boundaries & Constraints

**Always:**
- The API is ADDITIVE (engine crate only — not adapter-api, so the frozen contract is untouched) and documented in docs/embedding.md + testing.md + the engine module docs.
- Default behavior (no sink installed) is byte-identical to today: the two messages go to stderr with their current wording.
- The sink receives structured-enough content: same message text as today (the host formats/routes it).
- Thread-safety: the diagnostics fire from supervisor threads — the sink must be shareable (`Arc<Mutex<dyn Write>>`-style or a `Fn(String)` callback; implementer picks, documented).
- Tests: sink-installed engine receives BOTH diagnostic texts (fault-install for the DC-10 notice: attach with an unmapped adapter, then attach a mapped one mid-observation... simplest: drive the enforcement breadcrumb via a budget breach with `breach_action` pause on an already-paused instance — the exact production path from 7-1's tests); no-sink default emits to stderr unchanged (existing tests already pin the wording — extend, don't replace).
- The embed_clean audit's allowlist entries: after routing, the `eprintln!` sites are GONE from supervisor.rs — shrink the audit (the entries fail on disappearance today; invert to assert the sink call sites exist, or drop the entries and assert zero `eprintln!` remains in production sources — implementer's call, documented). Keep audit teeth.
- Standings: coverage ≥95%, docs currency, boundary gate, fmt/clippy.

**Ask First:**
- If routing the diagnostics requires touching the supervisor lock discipline or the committed-state ordering, HALT with the finding.

**Never:**
- No silencing or rewording of the diagnostics; no behavior change when no sink is installed; no kt CLI changes.

## Code Map

- `crates/ktesio-engine/src/domain/supervisor.rs:752` (DC-10 memory-delivery notice) and `:3034` (enforcement breadcrumb) — the two `eprintln!` sites (allowlist entries in embed_clean.rs pin them today).
- `crates/ktesio-engine/src/engine.rs` — Engine/EngineInner where the sink field lives; `blocking()` facade exposure.
- `crates/ktesio-engine/tests/embed_clean.rs` — the audit + its allowlist entries; the new sink tests land in the engine suite.
- `docs/embedding.md` + `docs/testing.md` — documentation of the sink.

## Tasks & Acceptance

**Execution:**
- [ ] `crates/ktesio-engine/src/` — the sink (opt-in install, thread-safe, default stderr), both diagnostic sites routed, docs.
- [ ] `crates/ktesio-engine/tests/` — sink-receives-both-diagnostics tests; default-path unchanged pins; embed_clean audit updated per the Always clause with teeth retained.
- [ ] `docs/embedding.md` + `docs/testing.md` — the sink documented for hosts.

**Acceptance Criteria:**
- Given a sink installed, when the two diagnostics fire, then the sink receives their exact texts and stderr stays silent.
- Given no sink, when the diagnostics fire, then stderr output is byte-identical to today's.
- Given the audit, when any production `eprintln!` appears or disappears, then CI fails (teeth retained in the new shape).
- Given the workspace battery, then everything passes.

## Spec Change Log

## Design Notes

- Keep the sink OUT of adapter-api and the frozen contract — it is engine-embedder ergonomics.
- A `Mutex<Box<dyn Write + Send>>` or a `Fn(&str)` callback both work; prefer whichever keeps the supervisor's hot path free of locking contention (the diagnostics are rare; a short lock is fine).
- The embed_clean audit today pins the two `eprintln!` sites by unique fragments — after routing, those pins become sink-call-site pins (same count==1 discipline).

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. new sink tests
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
- `cargo +1.96.1 tree -p ktesio -e normal,build` -- unchanged
