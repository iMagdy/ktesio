---
title: 'Epic-6 retro remediation — actionable action items'
type: 'chore'
created: '2026-09-04'
status: 'in-review'
review_loop_iteration: 0
baseline_commit: 20ddc204403a5c412e0e3249d4609dd47c30854e
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The epic-6 retrospective (2026-09-04) recorded 9 action items; 7 are mechanically actionable now and their findings are already triple-sourced (they came FROM adversarial review). The two highest-severity ones leave the frozen v1 contract unguarded (#160) and its gate bypassable (#161).

**Approach:** Land items 1, 2, 3, 4, 6, 7, 8 as one remediation PR — each issue's body and the retro doc (`_bmad-output/implementation-artifacts/epic-6-retro-2026-09-04.md`) are the sourced detail; this spec is the scope wrapper. GitHub issues #160-#163, #165-#167 close via `Fixes #N` on the PR; #164 (fixture consolidation — 12-file test refactor, own story) and #168 (bind-or-retire, Islam's call) stay open by scope.

## Boundaries & Constraints

**Always:**
- Every fix follows its retro finding's source reference; where the retro named a suggested shape (e.g. A1's two guard options), the implementer verifies feasibility locally and picks, documenting the choice in the PR.
- New test-bearing code ships with tests (the repo gate); `contract_version` on `ConformanceReport` follows the report family's documented bump policy (tck.rs:135-137) and updates the serde/round-trip pins.
- The AgentAdapter default-Unavailable reason-string replacement is ANNOUNCED in CHANGELOG/RELEASE_NOTES (frozen v1 surface text change under the ratified deprecation policy).
- Docs changes land in the same change as the behavior they describe (durable gate).
- GH issues are the tracking mirror: the PR body lists `Fixes #160 #161 #162 #163 #165 #166 #167` so merge closes them; #164/#168 remain open.

**Ask First:**
- Any fix that would change contract *semantics* rather than correcting an error (the retro findings are corrections, not renegotiations — if one turns out to need a semantic change, HALT).
- If the API-surface guard's preferred mechanism (cargo-semver-checks in-repo baseline) cannot run on stable Rust in CI, the fallback (inventory pin test) is pre-approved — no HALT needed.

**Never:**
- No fixture consolidation (#164) — that refactor is deliberately out of scope.
- No new features, no adapter behavior changes.

## Code Map

- `_bmad-output/implementation-artifacts/epic-6-retro-2026-09-04.md` — findings A1, B1-B5, B10, C1, C6, C7, D1-D7 with file:line sources; THE detail source for every item.
- Issues #160-#163, #165-#167 — per-item scope and acceptance.
- `crates/ktesio-engine/src/adapter/mod.rs:290-303` — the fallback path missing negotiation (#161).
- `crates/ktesio-adapter-api/src/lib.rs` + `src/` — the frozen surface to guard (#160).
- `crates/ktesio-adapter-api/src/adapter.rs:96-125` — default-Unavailable seed reason strings (#163).
- `crates/ktesio-conformance/src/tck.rs` — hardening sites (#163): `declares_memory_dir` (:250), UpstreamStub drop (:2239-2251), memory section subject-delivery, report struct (:185), bump policy (:135).
- `crates/kt/src/cli/agent.rs:1813-1831` — attach read-back partial state (#166).
- Docs: `docs/adapter-contract.md:25-50` (stdin claim), `docs/commands.md:53,173` + exit-code table, `docs/testing.md:79`, `docs/manifest.md:173`, `README.md` TCK dev-dep, `docs/meta.json`, `scripts/check_docs.py:30-33` (stale patterns) (#162/#165).
- `.github/workflows/ci.yml` semver job — candidate home for an in-repo-baseline run (#160).

## Tasks & Acceptance

**Execution:**
- [x] #160 -- API-surface guard (preferred: semver-checks in-repo baseline in the existing semver job; fallback: committed inventory pin test) -- the freeze is guarded before 7-4.
- [x] #161 -- negotiate_contract_version on the fallback path + bypass test -- the gate has no bypass.
- [x] #162 -- the four frozen-doc corrections -- normative text matches shipped behavior.
- [x] #163 -- TCK hardening batch (7 sub-items, issue body) -- the kit proves what the contract claims; report carries the negotiated version; placeholder strings replaced + announced.
- [x] #165 -- docs hygiene batch (7 sub-items, issue body) -- docs currency incl. the supported-agents page.
- [x] #166 -- attach --json partial-state diagnostic + test -- honest failure text.
- [x] #167 -- epic-6 heading reconcile, deferred-work resolution convention + sweep, architecture.md dependency sentence -- tracking tells the truth.

**Acceptance Criteria:**
- Given the merged PR, when a public `ktesio-adapter-api` item is removed/renamed without announcement, then CI fails (verified by the guard's own negative test or mechanism documentation).
- Given a registered manifest edited to a different major, when the fallback start path runs, then it fails naming both versions + the rule.
- Given `grep -rn "PolyForm\|tokens only\|own\[s\] their memory entirely"` style stale-pattern checks from the retro findings, then the named stale texts are gone and `check_docs.py` passes.
- Given the full workspace battery (fmt, clippy, tests, tarpaulin ≥95%, check_docs, test_automation), then everything passes.
- Given the PR merge, then issues #160-#163, #165-#167 auto-close and #164/#168 remain open.

## Spec Change Log

## Design Notes

- Item 4's `ConformanceReport.contract_version` is additive; if the documented policy calls for a schema_version bump, take it in the same change and update every pin — the report has few consumers today (in-repo tests + docs).
- #160 preferred mechanism detail: `cargo-semver-checks check-release -p ktesio-adapter-api --baseline-rev <freeze-commit>` run in the existing semver job would arm real API-diff protection on stable; the agent must verify the flag exists in the installed 0.50.0 (`semver-checks check-release --help`) and that it runs locally green BEFORE wiring CI, else fall back.
- The `kt --help` HELP_FOOTER drift guard already pins the license title; the reason-string replacement in #163 must not disturb that test.

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all --check` && `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- clean
- `cargo +1.96.1 test --workspace --all-targets` -- all pass incl. new guard/bypass/diagnostic tests
- `cargo +1.96.1 tarpaulin --workspace --fail-under 95` -- ≥95%
- `python3 scripts/check_docs.py` && `python3 scripts/test_automation.py` -- validate
