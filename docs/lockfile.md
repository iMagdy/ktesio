---
title: Lockfile (removed)
description: Ktesio no longer uses a project lockfile — durable Fleet state lives in the engine's SQLite store.
---

# Lockfile (removed)

Earlier versions of Ktesio were a skill package manager that wrote a `skills.lock` file to reproduce installs. Ktesio is now an **AI agent runner**, and there is no project lockfile.

Durable state — every Agent Instance registration, Lifecycle State, Restart Policy, restart count, and the Usage Ledger — lives in a single **SQLite database** under the engine state directory, plus per-instance files inside each **Agent Home**. That state survives an engine restart or reboot and reconciles orphaned processes on the next engine open.

See:

- [Architecture](architecture.md) — durable state, the Usage Ledger, and reconciliation.
- [Adapter manifest](manifest.md) — how an agent is described and registered.
- [Command reference](commands.md) — the `kt agent` commands.
