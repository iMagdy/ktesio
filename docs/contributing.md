---
title: Contributing Guide
description: Development setup, contribution workflow, pull request expectations, and docs update guidance.
---

# Contributing Guide

This page is the hands-on development guide. For project rules and the Contributor License Agreement, see [../CONTRIBUTING.md](../CONTRIBUTING.md) and [../CLA.md](../CLA.md).

## Setup

```bash
git clone https://github.com/iMagdy/ktesio.git
cd ktesio
cargo build
cargo test --workspace --all-targets
```

## Development Loop

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
python3 scripts/check_docs.py
```

## Adding CLI Behavior

- Update `crates/kt/src/main.rs` command parsing.
- Add or update a module under `crates/kt/src/cli/`.
- Add unit tests for command logic with explicit project roots.
- Add integration tests under `crates/kt/tests/` for user-facing workflows.
- Update [commands.md](commands.md) and [get-started.md](get-started.md) when behavior changes.

## Test Fixtures

Integration tests use local temporary git repositories through `crates/kt/tests/helpers/mod.rs`. Avoid network-only tests in the default suite.

## Pull Requests

- Keep changes focused.
- Use conventional commit messages.
- By opening a pull request, you agree to the [Contributor License Agreement](../CLA.md).
- Include docs and tests in the same change when behavior changes.
- Make sure CI passes before requesting review.
