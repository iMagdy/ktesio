# Agent Notes

Ktesio is being repositioned from a skills package manager into a unified
runner for personal agents (runtime controls, config, memory wiring, token
limits, and cost caps). Planning for that pivot runs through the BMAD Method;
its artifacts live under `_bmad-output/` and `_bmad/`, which are gitignored.

When working here:

- Prefer the public docs in `README.md` and `docs/` for current, shipping
  behavior (the `kt` skills CLI). Treat the runner pivot as in progress: do
  not document or ship runner features until their BMAD story lands.
- Before handing off code changes, run:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test --all-targets`
  - `python3 scripts/check_docs.py`

## Durable engineering gates

Carried over from the retired project constitution; these hold across the
pivot and are re-ratified as the BMAD PRD/architecture lands:

- **CLI-first** — every feature is reachable via the `kt` CLI; output goes to
  stdout, diagnostics to stderr; all commands support `--help`/`--version`.
- **Test coverage MUST stay ≥ 95%** — enforced in CI via
  `cargo tarpaulin --fail-under 95`. New code ships with tests.
- **Documentation currency** — update `docs/` and `README.md` in the same
  change as the code they describe; stale docs are treated as a bug.
- **Cross-platform** — Linux, macOS, and Windows; use path-agnostic std APIs.
- **Graceful degradation** — partial failures report a clear reason and a
  remediation, and do not abort the whole operation.
