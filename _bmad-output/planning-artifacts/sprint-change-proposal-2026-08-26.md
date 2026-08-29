---
type: sprint-change-proposal
date: 2026-08-26
author: Correct-Course workflow (BMAD), for Islam
status: approved  # ratified by Islam 2026-08-13 ("Approve all 4"); edits applied same day, merged forward onto post-Epic-5 main 2026-08-26; completion re-confirmed by Islam 2026-08-26
change_scope: MINOR  # record drift only — zero epic, story, or PRD change; Direct Adjustment (checklist Option 1)
trigger: "The sprint record decayed faster than the work: sprint-status.yaml went 24 days without a last_updated bump across three edits, its severity signal stopped discriminating (2 of 4 open HIGH items described a problem another item had already closed), and AI-63 was being quoted for a gate that ARCHITECTURE-SPINE.md AD-17 places somewhere else."
proposes: "Reconcile sprint-status.yaml against shipped reality; no plan change"
inputDocuments:
  - _bmad-output/implementation-artifacts/sprint-status.yaml
  - _bmad-output/planning-artifacts/epics.md
  - _bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md
  - _bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md
  - _bmad-output/implementation-artifacts/ai-63-lock-sweep-2026-07-21.md
verified_against_code: true
---

# Sprint Change Proposal — Reconcile the Sprint Record Against Shipped Reality

## Section 1 — Issue Summary

**Problem.** Not a failing story and not a plan defect: **drift between the tracking record and the shipped work**, surfaced mechanically during the 2026-08-12/13 dependency-backlog cleanup. `sprint-status.yaml` is the project's single tracking authority (`tracking_system: file-system`), and its priority signal had stopped discriminating — an item marked HIGH was as likely to describe a solved problem as a live one.

**Evidence (verified against the file, git history, and CI — not memory):**

- **AI-32 was stale by an order of magnitude.** It recorded "1 LOW Dependabot vulnerability"; the push warning on 2026-08-12 reported **15 (8 high, 6 moderate, 1 low)**. (Corrected separately in PR #132, before this proposal; it demonstrated the decay class.)
- **AI-31 and AI-36 — half of all open HIGH items — described a problem already closed.** `AI-53` is `done` and its own status note says verbatim *"Supersedes AI-31/AI-36."* Both were resolved 2026-07-14 by PR #108 (the per-crate tarpaulin split fit the free 7 GB runner; the larger-RAM runner they escalated to Islam was never needed). The `last_updated` narrative itself records *"AI-53 (#101/#31/#36) → done"*. Only the two `status:` fields were never flipped. Empirical confirmation: coverage passed on six consecutive PRs (#122, #129, #48, #131, #132, #133).
- **AI-63 was quoted for the wrong gate.** Its entry framed part (b) — the lock-architecture decision — as an Epic-5 concern. Two records had drifted apart: AI-63 recommends part **(a), the sweep**, before Epic 5 (and (a) is done, `ai-63-lock-sweep-2026-07-21.md`); **AD-17**, written later and authoritative, adopts the coarse-lock model for v1 under a bounded-work rule and scopes part **(b)** to *"before Epic 7's daemon/embedding work begins."* AD-17 is referenced nowhere in AI-63's entry, which is why the Epic-5 framing persisted.
- **The file's own staleness detector was the only thing that noticed**: `last_updated: 2026-07-20` was 24 days old against the sprint-status workflow's documented >7-day threshold, despite three intervening edits.

**Issue type** (checklist 1.2): misunderstanding of current state — the record, not the work.

## Section 2 — Impact Analysis

### Epic impact

**None.** No epic changes scope, order, or viability. Epics 1–4 and 9 are shipped; the 1 → 2 → 3 → {4, 5} → 6 → 7 → 8 dependency order holds. During the proposal's own lifetime Epic 5 shipped (5-1 = PR #138, 5-2 = PR #139) and Epic 6 opened (6-1 done, 6-2 in progress) — which *strengthened* the no-epic-change verdict: the drift never touched what was being built, only how it was recorded. Contrast the 2026-07-13 proposal, which genuinely restructured Epic 8 and added Epic 9 — this is deliberately **not that**.

### Story impact

**None.** No story's status, scope, or acceptance criteria change. (Story 5-1, briefly the subject of a scheduling question — see Section 4, edit 4 — shipped unmodified.)

### Artifact conflicts

- **PRD** — no conflict; vision, MVP, and FR set untouched.
- **Architecture** — no conflict *in the artifact*; AD-17 is correct as written. The conflict was AI-63's entry not citing it.
- **UI/UX** — N/A (CLI project).
- **`sprint-status.yaml`** — the affected artifact; all four edits land here and only here.

### Technical impact

None. The change is 4 lines in one tracked YAML file; zero Rust lines, zero dependency or workspace changes.

## Section 3 — Recommended Approach

**Option 1 — Direct Adjustment. Effort: Low. Risk: Low. Timeline impact: none.**

- **Option 2 (Rollback):** not applicable — nothing is wrong with shipped work.
- **Option 3 (MVP review):** not viable — nothing challenges MVP scope; invoking it would be ceremony.

The plan is sound and the execution is sound; only the bookkeeping drifted. The correct intervention is the cheapest one that restores signal, with every closure **cited to another entry, a commit, or a PR — no inference-only closures** (the rule whose absence produced the drift).

## Section 4 — Detailed Change Proposals

All four edits target `_bmad-output/implementation-artifacts/sprint-status.yaml`. Ratified by Islam 2026-08-13; merged forward 2026-08-26 after Epic 5 shipped (two conflict hunks, both resolved keeping both sides — main's newer narrative and PR #126 tactical-fix note preserved, this proposal's corrections appended).

### Edit 1 — `last_updated`

OLD: `last_updated: 2026-07-20` (24 days stale, three edits behind)
NEW: `last_updated: 2026-08-26`, with an appended reconciliation note recording what changed and why.
Rationale: the staleness detector only works if the field is maintained; the note makes the reconciliation itself auditable.

### Edit 2 — AI-31 `status: open` → `done`

OLD: `status: open` (severity high)
NEW: `status: done  # RESOLVED 2026-07-14 via AI-53 (PR #108 → main 8d975e7); status field simply never flipped…`
Rationale: no judgement call — AI-53 supersedes it explicitly. The note additionally flags that the OOM diagnosis was half the story (the second coverage failure was AI-67's stale cached `fake_agent`, not memory) so nobody re-opens on the OOM theory.

### Edit 3 — AI-36 `status: open` → `done`

OLD: `status: open` (severity high)
NEW: `status: done  # RESOLVED 2026-07-14 via AI-53…`
Rationale: same supersession. Its premise — "the coverage gate no longer gates" — is no longer true: the gate has gated honestly since Epic 9. The larger-RAM-runner cost call it escalated is moot.

### Edit 4 — AI-63 annotated with the AD-17 gate correction

OLD: entry framing part (b) as governing Epic 5.
NEW: `status: in-progress` unchanged; appended note recording that (a)-before-Epic-5 is DONE, that AD-17 is the authoritative record scoping (b) to **before Epic 7**, that story 5-1 complied with AD-17's bounded-work rule by construction, and that Islam's 2026-08-13 "decide (b) before 5-1" election was a scheduling preference — recorded as **superseded/moot** after 5-1 shipped (PR #138).
Rationale: stop the wrong gate being quoted; preserve the decision history honestly.

**Net effect: open HIGH items 4 → 2 (AI-32, AI-63).**

### Flagged, deliberately NOT changed (need a product call; this proposal guarantees zero epic/story change)

1. `epic-5` reads `in-progress` although both its stories are `done` — it is either `done` or awaiting its optional retrospective.
2. `epic-6` reads `backlog` although 6-1 is `done` and 6-2 is `in-progress` — contradicting the file's own transition rule (*"backlog → in-progress: Automatically when first story is created"*).

Owner: Islam (or the next sprint-status maintenance pass with his sign-off).

## Section 5 — Implementation Handoff

**Scope classification: MINOR** — direct implementation, no backlog reorganization, no replan.

| Deliverable | Owner | Status |
|---|---|---|
| The four edits, applied and validated (`yaml.safe_load`, BMAD known-value sets, `check_docs.py`) | Developer agent | done — carried by **PR #145** |
| Merge PR #145 to `main` | Islam | **pending** — blocked only by the trunk-wide coverage failure (below), not by this change |
| Staleness sweep across the remaining 56 open action items (ratified alongside the four edits) | Developer agent (multi-agent verification workflow) | re-launched 2026-08-26 after a session-limit failure returned zero results |
| AI-63(b) lock-architecture decision — **separate workflow, not this proposal's scope**, but the AD-17 gate it feeds is now imminent (Epic 6 in progress; Epic 7 next) | Islam + Winston (architect), decision workflow re-launched 2026-08-26 | in progress |
| Two flagged epic-status corrections (above) | Islam | open |

**Known blocker, explicitly not this change's to fix:** trunk's 95% coverage gate is red at 94.9431% (4675/4924 — ~3 lines short) on `main` since 2026-08-24, across the last four pushes including two feature stories. Recorded on PR #145 (comment 5429263731). Unlike the AI-53 and AI-67 chapters, **the gate is telling the truth this time** — the fix is a few targeted tests on Epic 5/6's uncovered lines, owned by trunk, not by this bookkeeping PR, and it should not end in an admin-merge-over-red.

**Success criteria:** PR #145 merged; `sprint-status.yaml` on `main` shows 2 open HIGH items with every closure citation-backed; the sweep's verified verdicts triaged into a follow-up reconciliation; no epic/story status changed by this proposal.

## Section 6 — Process Note (why this drifted, and the guard)

Every stale entry shared one shape: **the fix landed through a channel the record didn't watch** (a superseding item's note, a PR merged under a different concern, a push warning nobody transcribed). The counter-rule applied throughout this proposal, and recommended as standing practice for `sprint-status.yaml`:

> A status field may only be flipped with a citation to a commit, PR, or another entry — and conversely, any narrative that records a resolution ("AI-53 (#101/#31/#36) → done") must flip the fields it names in the same edit.

The second half is the one that failed here: the narrative knew; the fields didn't.
