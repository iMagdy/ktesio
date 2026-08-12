# Deferred Work

Findings surfaced incidentally during quick-dev reviews that are out of scope for the triggering change. Collected for later focused attention.

## From AI-17 (pin workspace toolchain to 1.96.1) — review, 2026-07-06

- **Contributor docs still tell contributors to run bare `cargo` (fmt/clippy/test).** With the new `rust-toolchain.toml`, bare `cargo` resolves to the MSRV (1.96.1) locally for contributors without a `RUSTUP_TOOLCHAIN` override, while CI's fmt/clippy/test jobs now gate on latest `stable` (explicit `+stable`). This local-vs-CI toolchain skew is intentional but is not documented in the other contributor-facing files. Consider a one-line note (or a `+stable` reproduction hint) in: `CONTRIBUTING.md` (~L89-91), `docs/contributing.md` (~L15-24), `AGENTS.md` (~L14-16), `.github/pull_request_template.md` (~L7-9), `docs/github-repository-audit-checklist.md` (~L167-169), `.agents/skills/kt-release/SKILL.md` (~L58), and `scripts/prepare_kt_release.py` (~L244-246). `docs/testing.md` already documents the split; the rest do not. Low severity (surfaces as an occasional new-stable clippy/rustfmt CI nit, not a shipped bug).

- **Coverage CI job rebuilds `cargo-tarpaulin` on every fresh runner (no binary cache).** Pre-existing (predates AI-17): the `coverage` job in `.github/workflows/ci.yml` runs an unguarded `cargo install cargo-tarpaulin` with no `~/.cargo/bin` cache, so it recompiles tarpaulin (~several minutes) every run. The `semver` job already added a `${{ runner.os }}-cargo-semver-checks-bin` cache + `command -v` guard (AI-1); the coverage job could adopt the same pattern for symmetry and CI speed.
