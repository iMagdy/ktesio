---
title: Command Reference
description: Every kt agent command, its arguments and flags, and the unified config keys for budgets, rates, and metering.
---

# Command Reference

The agent runner lives under `kt agent`. Every command supports `--help`, and `kt --version` prints the package version.

Output discipline: command results go to stdout, while notices and diagnostics go to stderr, so `--json` output on stdout is always machine-parseable.

## `kt agent register <name> (--kind <kind> | --manifest <path>)`

Register a new Agent Instance under a Fleet-unique name.

```bash
kt agent register demo --kind mock
kt agent register my-agent --manifest ./my-agent
```

Arguments and options:

- `<name>` — Fleet-unique instance name matching `^[a-z0-9][a-z0-9_-]*$`.
- `--kind <kind>` — a native builtin adapter by kind (e.g. `mock`). Mutually exclusive with `--manifest`.
- `--manifest <path>` — a manifest adapter loaded from a directory (or an `adapter.toml` file). Mutually exclusive with `--kind`.

Exactly one of `--kind` or `--manifest` is required. Registration validates the adapter's per-OS Capability Declaration and Metering Source **before** any state is written — an adapter with no capabilities or no viable metering source is rejected and nothing is created. On success it prints the engine-computed Agent Home path and the effective (current-OS) Capability Declaration.

The native `mock` kind is a fixture with no launch command; it registers and configures but cannot be started. Use a manifest adapter to run a real process.

## `kt agent list [--json]`

List every Agent Instance in the Fleet.

```bash
kt agent list
kt agent list --json
```

The human table shows name, kind, state, restart count, the token budget (ceilings + remaining + Breach Action, or `—` when un-budgeted), the real usage token totals, and the Agent Home, followed by a Fleet-wide totals footer. `--json` emits a versioned document (`schema_version`, `instances`, `totals`); token totals equal the Usage Ledger exactly, and dollar figures appear only when a Rate is configured (integer micro-dollars in JSON, labeled estimates).

## `kt agent show <name> [--json]`

Show one instance's effective per-OS Capability Declaration plus its runtime status.

```bash
kt agent show demo
kt agent show demo --json
```

The runtime status includes the Lifecycle State, Restart Policy, restart count, the token budget and dollar Cost Cap, real usage token totals (cumulative and current-run), the derived dollar cost when a Rate exists, the active Metering Source, and — for a failed instance — the failed cause. `--json` emits the same `FleetEntry` shape a `list` row uses, wrapped with the Fleet `schema_version`.

## `kt agent start <name>`

Start a registered Agent Instance.

```bash
kt agent start my-agent
```

On success the instance transitions to `running` and the new state prints to stdout. A launch failure lands the instance in `failed` with a diagnostic on stderr.

A standalone `kt agent start` supervises the process only for that command's lifetime and stops it when the command exits (a note is printed to stderr). Durable supervision across separate CLI invocations is future work.

## `kt agent stop <name> [--timeout <secs>]`

Stop a running Agent Instance: graceful shutdown, then a forced kill after the window.

```bash
kt agent stop my-agent
kt agent stop my-agent --timeout 10
```

- `--timeout <secs>` — the graceful-shutdown window before a forced kill (default 30). No child process survives.

## `kt agent pause <name>` / `kt agent resume <name>`

Pause a running instance, or resume a paused one, with honest per-OS semantics.

```bash
kt agent pause my-agent
kt agent resume my-agent
```

A **guaranteed** pause suspends the process (SIGSTOP on Unix); a **best-effort** pause proceeds cooperatively and prints a visible qualifier note; an **unsupported** pause fails fast, quoting the Capability Declaration. The posture is per-OS, read from the adapter's declaration.

## `kt agent send <name> <text>`

Send text input to a running Agent Instance's native input channel (v1: the spawned child's OS stdin pipe).

```bash
kt agent send my-agent "hello there"
```

A trailing newline is appended to `<text>` if it does not already end with one. Unlike `pause`/`resume`, `send` is not a lifecycle transition: the instance's state is unchanged, and only a confirmation prints to stdout.

The same three-way honesty as `pause`, with one difference: a **guaranteed** and a **best-effort** interaction level both deliver the input identically (there is no OS-conditional difference in writing to a pipe — best-effort is purely an adapter-author signal), while an **unsupported** interaction level fails fast, quoting the Capability Declaration.

`send` requires the instance to be genuinely `running`, and it inherits the same single-lifetime caveat as `start` (above): a standalone `kt agent start` supervises a process only for that command's lifetime, so a process adopted after an engine restart has no recoverable input channel in the new session — `send` on such an instance fails honestly (naming the cause) rather than silently dropping the input.

Stdin is piped only for adapters that declare interaction support (`guaranteed` or `best-effort`); an adapter that doesn't declare interaction sees stdin exactly as before this command existed (`/dev/null`-equivalent), so it never blocks waiting on input that will never arrive.

If an agent stops draining its input (a stuck/deadlocked process), `send`'s write is bounded — it fails with a distinct diagnostic naming the timeout rather than hanging, and the instance's interaction channel stays unavailable for the rest of that session until it is stopped and started again.

## `kt agent remove <name> [--delete | --retain] [--force]`

Remove an Agent Instance from the Fleet.

```bash
kt agent remove demo
kt agent remove demo --delete
kt agent remove my-agent --force
```

- `--retain` — keep the Agent Home directory on disk (the default).
- `--delete` — delete the Agent Home directory as well.
- `--force` — remove even if the instance is running. Required for a running instance.

`--delete` and `--retain` are mutually exclusive; when neither is given, the safe default is to retain.

## `kt agent config set <name> <key> <value>`

Set one config key on the Agent Instance layer. Validated at write time.

```bash
kt agent config set demo model gpt-4
kt agent config set demo budget.tokens.cumulative 500000
kt agent config set demo agent.api_key secret:OPENAI_KEY
```

A known unified key or an `agent.*` pass-through key is accepted and persisted; an unknown key **outside** `agent.*` is rejected before anything is written, with the nearest valid key suggested. The value is stored verbatim — a `secret:NAME` reference is stored as-is and resolved + masked at start/read (never resolved or echoed by this write).

## `kt agent config get <name> [<key>] [--json] [--reveal]`

Read the effective (resolved) config with per-value provenance.

```bash
kt agent config get demo
kt agent config get demo model
kt agent config get demo --json
kt agent config get demo --reveal
```

- `<key>` — optional; omitted prints the whole effective config. With a key, prints just that value.
- `--json` — emit a versioned document whose per-leaf objects carry `{ key, value, source, unvalidated }`.
- `--reveal` — the sole explicit un-mask for `secret:` values. It re-resolves secrets **live** (environment, then the secrets file) at read time, so a revealed value may differ from what a running instance resolved at its start. It never un-masks the persisted snapshot, logs, or events.

Values resolve across four layers — engine defaults &lt; agent-kind defaults &lt; instance config &lt; invocation overrides — and each value names its winning source layer (a **Source** column, or a `source` field with `--json`).

## Unified Config Keys

Set these with `kt agent config set <name> <key> <value>`.

| Key | Value | Meaning |
|-----|-------|---------|
| `budget.tokens.per_run` | integer | Per-run token ceiling |
| `budget.tokens.cumulative` | integer | Cumulative token ceiling |
| `budget.breach_action` | `pause` \| `stop` \| `warn` | Action on any budget/cap breach (default `pause`) |
| `cost.rate.input` | dollar string (e.g. `3.00`) | Input price per 1M tokens |
| `cost.rate.output` | dollar string | Output price per 1M tokens |
| `budget.dollars.per_run` | dollar string | Per-run dollar Cost Cap (needs a Rate to enforce) |
| `budget.dollars.cumulative` | dollar string | Cumulative dollar Cost Cap (needs a Rate to enforce) |
| `metering.upstream_base_url` | URL | Real upstream endpoint for an `engine-observed` instance |
| `agent.<key>` | any | Pass-through namespace delivered verbatim to the agent (bypasses known-key validation) |

Both Rate directions are required for dollars to be derived; with no Rate, dollar features are inert (no fabricated `$0.00`). Dollars are integer micro-dollars internally and always labeled estimates. A config value of the form `secret:NAME` (on any key) is a secret reference — resolved at start, masked everywhere Ktesio displays it.

## Global Behavior

### Update checks

When a `kt` subcommand runs, Ktesio checks whether a newer GitHub Release is available, using an hourly cache. If an update is available, it prints a short stderr notice asking you to run `kt self-update`. Machine-readable JSON on stdout is unaffected. Disable checks with `KTESIO_NO_UPDATE_CHECK=1` (also skipped when `CI=true`).

### `kt self-update`

Update the `kt` binary itself, preserving the current install channel (Homebrew, Cargo, or a manual release binary).

```bash
kt self-update
```
