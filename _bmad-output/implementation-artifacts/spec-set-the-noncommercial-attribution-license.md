---
title: 'Set the Ktesio Noncommercial-Attribution License'
type: 'feature'
created: '2026-09-04'
status: 'done'
review_loop_iteration: 1
baseline_commit: b661c13422cc45e8ae3e5983889fd2550203ebd4
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The owner wants the license to explicitly require **visible credit** — any product or distribution using Ktesio must prominently credit the Ktesio project and its author, Islam Magdy — while keeping everything the current PolyForm Noncommercial 1.0.0 already provides (noncommercial use free, commercial use prohibited without the owner's prior written approval). PolyForm does not impose display credit, so the license must become a **custom license: the PolyForm Noncommercial 1.0.0 terms plus an added Attribution condition**. The project stays labeled **source-available** — never "open source."

**Approach:** Re-title and amend the existing LICENSE in place: keep the entire PolyForm Noncommercial 1.0.0 body verbatim, retitle the license "Ktesio Noncommercial-Attribution License 1.0.0", add the Attribution section (text below, owner-ratified at checkpoint), and update the plain-terms header with a credit bullet. Then update every reference so no document claims a license that no longer exists: workspace Cargo manifest (`license` → `license-file`), README badge + License section, Homebrew formula generator, audit-checklist mentions, and a CHANGELOG/RELEASE_NOTES announcement.

## The new words (owner ratifies verbatim at checkpoint)

**License title:** `Ktesio Noncommercial-Attribution License 1.0.0`

**New header bullet (added to the plain-terms list; re-ratified 2026-09-04):**
> * You must visibly credit the Ktesio project and its author whenever you
>   distribute Ktesio, use it in your own product or distribution, or operate
>   it to provide functionality to others (see "Attribution" below).

**New section (inserted after `## Notices`; re-ratified 2026-09-04):**
```markdown
## Attribution

Whenever you distribute the software, distribute a modified version of
it, use it in your own product or distribution, or operate it to
provide functionality to third parties, you must prominently credit
the Ktesio project and its author. The credit must name both the
project ("Ktesio") and the author ("Islam Magdy", the copyright
holder), and must appear in at least one place a reasonable user or
recipient would readily see — such as your product's documentation, an
"About" or credits screen, or a public README or about page. Private,
internal use that reaches no third party owes no credit under this
section. This section supplements, and does not replace, the Notices
obligation above. You may not state or imply that the author endorses
you or your use.
```

**Body fixes (re-ratified 2026-09-04 — amendments to the otherwise-verbatim PolyForm body):**
1. The body's PolyForm H1 is replaced by the license's own name plus a bridging line: "These terms are the text of the PolyForm Noncommercial License 1.0.0 (polyformproject.org), retitled and amended as set out in this document. Where this document and the PolyForm text differ, this document governs."
2. The Notices clause's URL alternative is removed — pass-along must be the full terms text (the PolyForm URL serves the unamended text and must not be a sanctioned escape from the Attribution condition).
3. The derivation note moves into the non-binding preface (after the plain-terms list), not the binding footer.
4. Announcement banners state noncommercial usage rights are unchanged AND the new credit condition applies to products/distributions.

**Derivation note (one line, footer of LICENSE or README License section):** "Based on the PolyForm Noncommercial License 1.0.0 (polyformproject.org), modified: an Attribution condition has been added. This is a custom license, not a PolyForm license."

## Boundaries & Constraints

**Always:**
- The PolyForm body text already in LICENSE is reused **verbatim except the four ratified body fixes** (inner heading + bridging line, Notices URL-escape removal, derivation-note placement, banner scope alignment) — do not rewrite PolyForm's clauses beyond those.
- Every reference that names the old license must be updated in the same change; no document may name a license the repo doesn't ship.
- The commercial-approval mechanism stays exactly as-is: all commercial use is unlicensed without the copyright holder's separate written license (the header already says this; do not soften it).
- The project remains labeled **source-available**, never open source (README's existing disclaimer stays).
- CLA.md and CONTRIBUTING.md stay untouched — copyright assignment is what keeps unified ownership (and thus commercial licensing) possible.

**Ask First:**
- Any edit to the ratified Attribution wording itself.
- Any change that would allow commercial use without written approval.

**Never:**
- No code changes — manifests/docs/tooling-references only.
- No removal or weakening of the PolyForm grant/condition sections.
- No claim of OSI open-source status anywhere.

## Code Map

- `LICENSE` -- the file being retitled/amended: header (:1-17), PolyForm body verbatim below; `## Notices` at :54 is the insertion anchor for `## Attribution`.
- `Cargo.toml:19` -- `license = "PolyForm-Noncommercial-1.0.0"` → replace with `license-file = "LICENSE"`; crates at crates/*/Cargo.toml (`license.workspace = true`, 5 files) → `license-file.workspace = true`. Note: a custom license has no SPDX id, so crates.io (story 7-4) will upload LICENSE via license-file — correct mechanism.
- `README.md:9` -- badge → `license-Ktesio%20NC--Attribution%201.0-blue.svg` style (shields.io, spaces as `%20` or `--`); README:206-213 -- License section: new license name, keep the source-available disclaimer (:213), add the visible-credit bullet + derivation note.
- `scripts/generate_homebrew_formula.py:14` -- `LICENSE = "PolyForm-Noncommercial-1.0.0"` → Homebrew has no SPDX id for a custom license: set the const to `":any"` and adapt the template (`license "{LICENSE}"` at :82 renders quotes around the symbol — fix the template so it emits `license :any` correctly); regenerate/verify the formula renders.
- `docs/github-repository-audit-checklist.md:27,111` -- replace "PolyForm Noncommercial 1.0.0" references with the new license name (GitHub will continue to display "Other").
- `CHANGELOG.md` + `docs/RELEASE_NOTES.md` -- add the license-change announcement (what changed, what it means for existing noncommercial users — nothing — and commercial users — contact for a license; the new credit condition).
- `CONTRIBUTING.md:32` / `CLA.md` -- read-only sanity: they say "source-available" generically, no PolyForm naming; confirm no edit needed.

## Tasks & Acceptance

**Execution:**
- [x] `LICENSE` -- retitle to "Ktesio Noncommercial-Attribution License 1.0.0", add the header bullet + Attribution section + derivation note; PolyForm body otherwise verbatim. -- The ratified text ships.
- [x] `Cargo.toml` + 5 crate manifests -- license → license-file (workspace inheritance). -- Manifest metadata must match the shipped license; no SPDX exists for a custom license.
- [x] `README.md` + `scripts/generate_homebrew_formula.py` -- badge/section + formula const/template. -- User-facing references and the packaging artifact.
- [x] `docs/github-repository-audit-checklist.md`, `CHANGELOG.md`, `docs/RELEASE_NOTES.md` -- reference updates + announcement. -- Docs currency; the change is announced, not silent.

**Acceptance Criteria:**
- Given the merged LICENSE, when reading it, then it is titled "Ktesio Noncommercial-Attribution License 1.0.0", contains the ratified Attribution section verbatim, keeps the PolyForm Noncommercial body verbatim otherwise, and retains the commercial-requires-written-approval terms.
- Given the repo after the change, when grepping for "PolyForm-Noncommercial-1.0.0" and "PolyForm Noncommercial License 1.0.0" outside LICENSE's derivation note, then no stale references remain.
- Given `cargo +1.96.1 verify-project --workspace`, then every manifest's license-file metadata resolves cleanly.
- Given the Homebrew formula generator, when run, then it renders a formula whose license is `:any` (or equivalent valid Homebrew form) without error.
- Given `python3 scripts/check_docs.py`, then it validates (README/docs links intact).

## Spec Change Log

- **2026-09-04 (review loop 1 — re-ratified by Islam):** Adversarial review found legal-mechanism defects in the drafted instrument. Triggering findings: (a) the verbatim PolyForm heading + Notices' "or the URL for them above" gave distributors a license-sanctioned path to serve the UNAMENDED PolyForm text and drop the Attribution condition; (b) the header bullet understated the binding trigger ("product or distribution" vs "any use"); (c) the any-use trigger made private use owe credit with no compliance path; (d) banners claimed "noncommercial users: nothing changes" while the condition applied to them. Amended: revised Attribution clause (public-facing trigger + private-use carve-out + supplements-Notices + author=copyright-holder tie), inner-heading retitle + bridging line, Notices URL escape removed, derivation note moved to non-binding preface, header bullet + banners aligned. Known-bad state avoided: a self-contradicting license whose own text enabled dropping its newest condition. KEEP instructions: the ratified Attribution wording and the four body fixes are now the contract — the PolyForm body stays verbatim beyond them; mechanical findings (formula `license :any` test assertion, formula desc modernization, badge 1.0.0, HELP_FOOTER drift guard, README link rewording) ride as patches without further ratification.

## Design Notes

- The Attribution clause is deliberately "reasonable-effort" (documentation/About/README placement) — an absolute "credit everywhere always" clause is unenforceable and adoption-hostile; the ratified text names acceptable placements.
- A custom license will show on GitHub as "Other" — the audit checklist already anticipated this pattern; the updated checklist text says so.
- No SPDX id exists for a custom license; `license-file` is the correct Cargo/crates.io mechanism and must land before story 7-4 publishes the crates.

## Verification

**Commands:**
- `cargo +1.96.1 verify-project --workspace` -- all manifests valid
- `grep -rn "PolyForm-Noncommercial" --include="*.toml" --include="*.py" --include="*.md" .` -- only the derivation-note mention in LICENSE/README survives
- `python3 scripts/check_docs.py` -- validates
- `python3 scripts/generate_homebrew_formula.py` (or its documented invocation) -- formula renders with the new license form
## Suggested Review Order

**The instrument itself**

- Non-binding preface: plain terms, the re-ratified credit bullet, derivation note before the separator.
  [`LICENSE:1`](../../LICENSE#L1)

- The binding text opens under its own name with the PolyForm bridging line — no self-contradiction, no URL escape hatch.
  [`LICENSE:32`](../../LICENSE#L32)

- The re-ratified Attribution clause: public-facing trigger, private-use carve-out, supplements-Notices.
  [`LICENSE:75`](../../LICENSE#L75)

**Packaging metadata**

- license-file replaces the retired SPDX id; the 5 crates inherit.
  [`Cargo.toml:19`](../../Cargo.toml#L19)

- The drift guard: `--help`'s license title must match the shipped LICENSE's binding region.
  [`main.rs:729`](../../crates/kt/src/main.rs#L729)

- Formula emits `license :any` with the real license named in a comment; render is test-asserted.
  [`generate_homebrew_formula.py:76`](../../scripts/generate_homebrew_formula.py#L76)

**References + announcement**

- README badge/License section; audit checklist; CHANGELOG/RELEASE_NOTES banners with the corrected noncommercial-users claim.
  [`README.md:206`](../../README.md#L206)

**Change record**

- Review loop 1: what the re-ratification amended and the known-bad state it avoids.
  [`spec:Spec Change Log`](spec-set-the-noncommercial-attribution-license.md)
