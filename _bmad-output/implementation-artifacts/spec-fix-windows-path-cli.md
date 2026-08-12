---
title: 'Fix 6 Windows-only CI failures in the legacy skill CLI (cross-platform PATH handling)'
type: 'bugfix'
created: '2026-07-07'
status: 'in-review'
context: []
baseline_commit: 'b97a6a1a27634d81d61dbc2d5970da9516c2888f'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Epic 1's PR #100 ran CI for the first time and 6 tests in the pre-existing legacy skill CLI fail ONLY on `windows-latest` (ubuntu + macOS pass). Root cause is cross-platform PATH handling: (a) relative paths stored in the manifest use the OS separator, so a Windows manifest stores `\` instead of the portable `/`; (b) the `name:url` install-target parser and the local-path detector collide with Windows drive-letter colons and backslash-absolute paths; (c) four tests build JSON fixtures by string-interpolating a `Path::display()` value, and on Windows the embedded backslashes are invalid JSON escapes so the fixture fails to parse.

**Approach:** Establish and apply one rule — relative paths persisted in manifests/lockfiles are normalized to forward slashes `/`; path parsing/comparison is separator-agnostic; path construction keeps using `Path`/`PathBuf::join`. Fix the two genuine code bugs (manifest path normalization in `publish.rs`; drive-letter-safe install-target parsing in `install.rs` + `install_target.rs`) as portable logic (no `#[cfg(windows)]`). Fix the four JSON-fixture tests to serialize paths through `serde_json` so they escape correctly on every OS. Keep changes minimal and targeted; do not refactor the legacy CLI broadly.

## Boundaries & Constraints

**Always:**
- Normalize relative paths to `/` before storing them in a manifest/lockfile (portable + deterministic across OSes).
- Keep path construction via `Path`/`PathBuf::join`; never concatenate a hardcoded separator.
- Make path classification separator-agnostic (a Windows drive-letter path must never be read as a `name:url` spec).
- Keep macOS/Linux behavior byte-identical (on Unix, `/` is already the separator, so normalization is a no-op there).
- Fixes must be portable logic; no `#[cfg(windows)]` branches.

**Ask First:**
- Any need for a genuinely Windows-specific path API (would require flagging, not a cfg branch).
- Any change touching Epic 1/2 engine code or a shared helper living outside the `kt` crate.

**Never:**
- No `#[cfg(windows)]` anywhere outside `crates/ktesio-engine/src/backends/`.
- No changes to `sprint-status.yaml`, GitHub, or engine code.
- No `Cargo.lock` changes; no git commits.
- Do not fix the publish separator failure by weakening the test — fix the code.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Store adopted local skill path (publish) | Candidate at `<root>/.agents/skills/local` on any OS | Manifest dependency `path` == `.agents/skills/local` (forward slashes) | N/A |
| Store validated publish path (`kt publish add`) | Relative/absolute path inside project on any OS | Stored manifest path uses `/` separators | Outside-project path → error (unchanged) |
| Install from local repo path (Windows) | Target = `C:\Users\...\source` (drive-letter colon, backslashes) | Classified as a repo path, not `name:url`; installs selected skill | Nonexistent path → clone error (unchanged) |
| Install `name:url` spec | Target = `docs:https://github.com/o/r.git` | Classified as Named (name=`docs`) | Empty/invalid name → format error (unchanged) |
| Install `name:local-path` on Windows | Target = `docs:C:\repos\x` | Classified as Named (name=`docs`, repo=`C:\repos\x`) | N/A |

</frozen-after-approval>

## Code Map

- `crates/kt/src/cli/publish.rs` -- `add_publish_candidate` + `validate_publish_path` build the stored manifest path via `strip_prefix(...).to_string_lossy()` (OS separator). CODE FIX: normalize to `/`.
- `crates/kt/src/cli/install.rs` -- `parse_named_target` (`split_once(':')`) misclassifies a Windows drive-letter path as `name:url`. CODE FIX: only treat as `name:url` when the segment before `:` is a valid skill name AND the value after `:` is not itself a local path / bare drive letter.
- `crates/kt/src/install_target.rs` -- `looks_like_local_path` only recognizes Unix path prefixes. CODE FIX: also recognize Windows absolute (`C:\`, `C:/`) and UNC (`\\`) forms, and any input containing a backslash. Add a shared `normalize_separators_to_slash` helper here (imported by publish.rs) so the rule lives in one place.
- `crates/kt/src/cli/upgrade.rs` (test `test_upgrade_success_updates_lockfile_commit`) -- TEST FIX: build the lockfile JSON via `serde_json`, not `format!` with `Path::display()`.
- `crates/kt/src/cli/install.rs` (tests `test_run_bulk_clone_fails`, `test_run_bulk_with_manifest_installs_local_repo`, `test_run_bulk_with_manifest_clone_fails`) -- TEST FIX: build the `skills.json` fixture via `serde_json` so backslashes are escaped on Windows.
- `crates/kt/src/manifest.rs`, `crates/kt/src/lockfile.rs` -- read-only reference: `save` already serializes via `serde_json` (correct); the CLI never writes JSON via `format!`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/kt/src/install_target.rs` -- Added `pub fn normalize_separators_to_slash(path: &str) -> String` (replaces `\`→`/` for persisted manifest/lockfile paths) and `fn is_windows_absolute_path` (drive-letter `X:\`/`X:/` + UNC `\\`). Split local-path detection into a broad private `looks_like_local_path` (adds "contains backslash", used by `resolve_repo_target` to reject GitHub shorthand) and a narrow `pub fn is_local_path_target` (absolute/`./`-relative/existing/drive/UNC — NOT bare backslash) for the `name:url` parser guard. Added unit tests. -- Centralizes the slash rule and makes local-path detection separator-aware.
- [x] `crates/kt/src/cli/install.rs` -- In `parse_named_target`, before `split_once(':')`, return `Ok(None)` when `install_target::is_local_path_target(target)` (fall through to repo-target resolution). Used the NARROW predicate (not the broad `looks_like_local_path`) so `docs:C:\repo` stays a valid `name:url` spec while a bare `C:\repo` becomes a repo target. -- Fixes the drive-letter-colon misclassification.
- [x] `crates/kt/src/cli/publish.rs` -- In `add_publish_candidate` and `validate_publish_path`, pass the `strip_prefix(...).to_string_lossy()` result through `install_target::normalize_separators_to_slash(...)` before storing/returning. -- Guarantees manifest paths use `/` on every OS.
- [x] `crates/kt/src/cli/upgrade.rs` (tests) -- Rewrote the lockfile fixture in `test_upgrade_success_updates_lockfile_commit` with `serde_json::json!(...).to_string()` so the repo path is JSON-escaped. -- Test builds valid JSON on Windows.
- [x] `crates/kt/src/cli/install.rs` (tests) -- Added a `manifest_json_with_repo(name, &Path)` test helper (serde_json-built) and used it in `test_run_bulk_clone_fails`, `test_run_bulk_with_manifest_installs_local_repo`, `test_run_bulk_with_manifest_clone_fails`. -- Tests build valid JSON on Windows.
- [x] Added unit tests asserting `normalize_separators_to_slash(r"a\b\c") == "a/b/c"`, `is_local_path_target` recognizes `C:\x`/`C:/x`/`\\srv\share` (and rejects `docs:C:\repo`), and `resolve_repo_target` maps Windows/UNC paths to `Repo` verbatim. -- Locks in the I/O matrix rows.

**Acceptance Criteria:**
- Given the full local gate suite on macOS (`cargo +1.96.1`), when fmt / clippy -D warnings / test --workspace --all-targets / tarpaulin (fail-under 95) / check_docs.py / test_automation.py / check --workspace / OS-cfg grep / boundary run, then all pass with no regression.
- Given a candidate skill at `.agents/skills/local`, when `add_publish_candidate` stores it, then the manifest `path` is exactly `.agents/skills/local` (forward slashes) — the byte-for-byte assertion the publish test makes, satisfied on every OS.
- Given a local repo path as an install target, when `parse_install_target` classifies it, then it resolves to `InstallTarget::Repo` (never `Named`) even when the path contains a drive-letter colon or backslashes.
- Given the whole change, when I grep the diff, then there is no new `#[cfg(windows)]` outside `crates/ktesio-engine/src/backends/`, and `Cargo.lock` is unchanged.

## Design Notes

The normalization rule is narrow: only relative paths persisted into `skills.json`/`skills.lock` are forced to `/`. We do NOT normalize repo URLs, clone-target absolute paths passed to `git`, or on-disk `PathBuf`s used for filesystem I/O — those keep native form. `\` is illegal in a POSIX filename, so replacing `\`→`/` on a *relative repo-internal* path is safe and lossless for the manifest use case.

Why `parse_named_target` is the fix point, and why two predicates: `parse_named_target` runs FIRST and short-circuits to the `Named` arm before `resolve_repo_target` is ever consulted. On Unix a local path has no `:` so it already returns `None`; the added guard makes Windows behave the same way. Detection is split in two: the NARROW `is_local_path_target` (absolute / `./`-relative / drive-letter / UNC / existing path — but NOT a bare mid-string backslash) guards `parse_named_target`, so `docs:C:\repos\x` (a legitimate `name:local-path` spec — whole string is not itself a bare local path) stays `Named`, while `C:\repos\x` becomes a `Repo` target. The BROAD `looks_like_local_path` (which also treats any backslash as a local signal, since GitHub `owner/repo` components never contain one) is used only by `resolve_repo_target` to steer a Windows path away from the GitHub-shorthand branch. Using the broad predicate as the `parse_named_target` guard would wrongly drop `docs:C:\repos\x` out of the `Named` path — hence the split.

Why the four JSON tests are test-fixes, not code-fixes: the CLI persists JSON exclusively through `serde_json::to_string_pretty` (`Manifest::save`, `Lockfile::save`), which escapes `\` correctly. Only the tests hand-roll JSON with `format!("...{}", path.display())`, so only the tests produce invalid JSON on Windows. Fixing the code here would be wrong; the fixtures are the defect.

## Verification

**Commands:**
- `cargo +1.96.1 fmt --all -- --check` -- expected: clean
- `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` -- expected: no warnings
- `cargo +1.96.1 test --workspace --all-targets` -- expected: all pass (incl. the 6 previously Windows-failing tests, which stay green on macOS)
- `cargo +1.96.1 tarpaulin --engine llvm --workspace --fail-under 95` -- expected: >= 95% coverage
- `python3 <docs check> && python3 <test automation>` -- expected: pass (resolve exact script paths from the repo's gate tooling)
- `cargo +1.96.1 check --workspace` -- expected: clean
- OS-cfg grep -- expected: no `#[cfg(windows)]`/`#[cfg(unix)]` outside `crates/ktesio-engine/src/backends/`
- `git diff --stat -- Cargo.lock` -- expected: empty (Cargo.lock unchanged)

**Manual checks (if no CLI):**
- Reason through the 6 tests on Windows separators: confirm each now produces valid JSON / correct classification / `/`-normalized stored path.
