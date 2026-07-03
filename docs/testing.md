---
title: Testing
description: Required checks, integration fixtures, coverage gates, and documentation validation for Ktesio contributors.
---

# Testing

The test suite covers unit behavior, CLI workflows, and local git fixtures.

## Required Checks

Run these before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
python3 scripts/check_docs.py
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

## Unit and Integration Tests

```bash
cargo test --workspace --all-targets
```

Integration tests create local temporary git repositories. They do not require network access.

## Coverage

CI runs `cargo tarpaulin --workspace --fail-under 95` as the coverage gate. To run it locally:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --workspace --fail-under 95
```

Generate an HTML report:

```bash
cargo tarpaulin --out Html
```

## Documentation Checks

```bash
python3 scripts/check_docs.py
```

The docs check validates:

- Root and `docs/` Markdown links.
- JSON fenced code blocks.
- Documented `kt` command examples.
- Stale links to old repository names or generated spec quickstarts.

## Release Script Checks

```bash
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
```

This verifies the release-note generator can handle a first-release style tag when no previous tag exists.

## Automation Helper Tests

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

These tests cover release-note and changelog rendering, Homebrew formula
generation, installer dry-run decisions, and CI/workflow expectations.
