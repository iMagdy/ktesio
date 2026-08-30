---
title: 'Story 6-2 round-1 triage: resolve the 13 accepted review findings'
type: 'bugfix'
created: '2026-08-30'
status: 'done'
route: 'one-shot'
context: []
---

# Story 6-2 round-1 triage: resolve the 13 accepted review findings

## Intent

**Problem:** Story 6-2's three-lens BMAD review (PR #149) left 13 triaged findings
(blind-2/3/7/10/12/13/15/18/19/22/23/24, vg-o2) unresolved — a mix of missing
drift-guard tests, hardcoded literals that erode the change-safety story, an
unsafe test-env mutation, scheduler-timing assertions that flake CI, and docs/CI
comments that misstate the system.

**Approach:** One batch of surgical fixes: const-driven kind literals +
a CLI registration test; PATH save/restore at hermes.rs teardown; Option-state
polling in `wait_until_state`; manifest-precedence and env-empty drift guards;
`waited_ms > 0` semantics; the shared-default-home warning in commands.md; the
shipping-consequence sentence in architecture.md; and honest WHY/OS-cfg comments
in ci.yml — then the one-shot blind-hunter pass over the fix diff.

## Suggested Review Order

- The kind-identifier fix that everything else composes against: `HERMES_KIND`
  const as match pattern + `kind()` impl.
  [`../../crates/ktesio-adapters-hermes/src/lib.rs`](../../crates/ktesio-adapters-hermes/src/lib.rs)
- Builtin match arms + unit tests switched to consts (`HERMES_KIND`,
  `MEMORY_DIR_KEY`, `HERMES_HOME`).
  [`../../crates/ktesio-engine/src/adapter/builtin.rs`](../../crates/ktesio-engine/src/adapter/builtin.rs)
- Two new drift guards: production resolve env stays empty; manifest kind beats
  the builtin table.
  [`../../crates/ktesio-engine/src/adapter/mod.rs`](../../crates/ktesio-engine/src/adapter/mod.rs)
- Integration rework: Option-state `wait_until_state`, PATH restore at teardown,
  `waited_ms > 0` Phase G, const-composed launch (blind-21 pin).
  [`../../crates/ktesio-engine/tests/hermes.rs`](../../crates/ktesio-engine/tests/hermes.rs)
- The CLI-level proof the kind plumbs end to end.
  [`../../crates/kt/tests/agent_cli.rs`](../../crates/kt/tests/agent_cli.rs)
- The CI boundary WHY + corrected OS-cfg allowlist comment, and its test twin.
  [`../../.github/workflows/ci.yml`](../../.github/workflows/ci.yml) /
  [`../../scripts/test_automation.py`](../../scripts/test_automation.py)
- Docs currency: unbacked-home warning + remedy; shipping-consequence sentence.
  [`../../docs/commands.md`](../../docs/commands.md) /
  [`../../docs/architecture.md`](../../docs/architecture.md)
