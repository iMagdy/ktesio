# Spine Review — version & reality verification lens

*Configured finalize reviewer #1: "verify every committed decision was web-researched or reality-checked rather than asserted from training data." Sequential fallback (subagents gated). 2026-07-02.*

**Verdict: pass with two author-flagged gaps and one scheduled verification — nothing asserted-from-training-data without a caveat.**

## Checked

- **Existing stack rows** (clap 4, miette 7, thiserror 2, serde 1, indicatif 0.18, console 0.16, dialoguer 0.12, ureq 3, git shell-out): verified against the repo's real `Cargo.toml` (read this session) — reality-checked, not asserted. ✓
- **tokio / rusqlite (starred rows):** NOT web-verified — WebSearch and `cargo search` both classifier-gated at authoring. Author flagged in the Stack caption + memlog with a concrete resolution path (resolve at first `cargo add`, record actual pins in that story). **Accepted as flagged gap, not a miss.**
- **Hermes Agent integration facts** (gateway process model, `/usage` accounting, CLI verbs) trace to the brief addendum §C, which itself carries a search-excerpt caveat (§H) — the spine does not bind any Hermes specifics beyond "native adapter maps them," so nothing over-committed. **Recommendation:** the Hermes adapter story must start with a primary-source verification pass (docs/repo) before coding against those specifics.
- **opencode** (second agent): deliberately NOT researched — Islam's selection; the spine defers all opencode specifics to the scheduled structural characterization (first conformance-mapping step). Correctly uncommitted. ✓
- **SQLite/WAL semantics, POSIX process groups/signals, Windows Job Objects:** OS/library fundamentals, stable for a decade-plus; treated as durable knowledge, not version-sensitive claims. Acceptable without web verification.
- **No starter adopted** (brownfield extension of an existing Rust workspace) — the greenfield starter-verification duty does not apply.

## Required follow-ups (carried to open items)

1. Pin tokio/rusqlite exact versions at adoption; record in the introducing story.
2. Primary-source verification of Hermes Agent CLI/gateway/usage surfaces at the start of the hermes-adapter story.
3. opencode structural characterization before contract freeze (already an Islam-scheduled open item).
