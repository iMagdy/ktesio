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

## `kt agent usage [<name>] [--json]`

Read Usage Ledger totals for one instance, or for the whole Fleet.

```bash
kt agent usage
kt agent usage my-agent
kt agent usage my-agent --json
kt agent usage --json
```

- `<name>` — optional; with a name, reports that instance's usage. Omitted, reports the Fleet-wide totals.
- `--json` — emit a single versioned document (not newline-delimited; usage is a snapshot, not a stream).

The named form reports tokens by both scopes (cumulative and current-run), the derived dollar cost per scope, and the active Metering Source. The Fleet-wide form reports summed tokens plus the summed derived dollar cost across the instances that have a Rate — flagged as a lower bound, naming how many instances are unpriced, when some metered instance has no Rate.

Token totals equal the Usage Ledger exactly, and are the same numbers `list`/`show` report — this command is a focused surface over the same data, never a second, independently-summed figure. Dollar figures appear only when a Rate is configured and are always labeled estimates; with no Rate the dollar view is honestly inert (`—`), never a fabricated `$0.00`.

Both `--json` forms carry the same Fleet `schema_version` as `list`/`show`, since they serialize the same fleet domain types:

```json
{
  "schema_version": 2,
  "instance": "my-agent",
  "usage": {
    "cumulative_input_tokens": 1200,
    "cumulative_output_tokens": 3400,
    "current_run_input_tokens": 120,
    "current_run_output_tokens": 340,
    "cumulative_dollars": 54600,
    "current_run_dollars": 5460,
    "estimate_label": "estimated"
  }
}
```

The Fleet-wide form (no name) is the same document family with `totals` in place of `instance`/`usage`:

```json
{
  "schema_version": 2,
  "totals": {
    "total_input_tokens": 1200,
    "total_output_tokens": 3400,
    "total_dollars": 54600,
    "dollars_partial": true,
    "unpriced_count": 1,
    "estimate_label": "estimated"
  }
}
```

`totals` is byte-identical to the `totals` object `kt agent list --json` carries — one aggregate, two surfaces. `dollars_partial` is `true` when some metered instance has no Rate, making `total_dollars` a lower bound; `unpriced_count` then names how many instances were left out.

Dollars are **integer micro-dollars** (1,000,000 = $1.00) plus an `estimate_label` of `estimated` or `reconciled` — never a preformatted `$` string, so a caller formats its own currency.

Dollar fields are **omitted entirely** when no Rate is configured (`cumulative_dollars`, `current_run_dollars`, `estimate_label` on the named form; `total_dollars` and `estimate_label` on the Fleet form) — an honest absence rather than a fabricated `$0.00`. A parser must treat them as optional.

With no instances registered, the Fleet form still emits a valid document (all-zero totals) and prints a short registration hint to stderr, so an empty Fleet is never mistaken for instances that consumed nothing.

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

## `kt agent logs <name> [--follow] [--json]`

Read an Agent Instance's retained output, optionally following live output.

```bash
kt agent logs my-agent
kt agent logs my-agent --follow
kt agent logs my-agent --json
kt agent logs my-agent --follow --json
```

Every currently-retained line is printed to stdout as `<at> [<stream>] <text>`, in the order it was captured (append order — never re-sorted by timestamp, since same-second lines are common). `<stream>` is one of `agent-out`, `agent-err`, or `engine`: the spawned process's stdout and stderr are captured separately (so you can tell them apart), and a best-effort `engine` line is added at each lifecycle transition (start, stop, pause, resume, crash, restart), mirroring the same facts the structured transition log already records.

Log capture is **unconditional and capability-independent** — unlike `send`, it does not depend on the adapter's declared `interaction` support. Reading an instance's output always works, even for an adapter that declares `interaction: unsupported`; only writing to a process (`send`) is gated on that capability.

The captured output is bounded: each generation caps at 10MB, with the current generation plus its 2 most recent rotated predecessors retained (10MB × 3 total, fixed and non-configurable). `kt agent logs` never errors due to rotation — a read that spans a rotation boundary returns whatever is currently retained, not a claim of the instance's entire lifetime history.

`--follow` (`-f`) prints the retained lines first, then keeps polling for new output and printing it as it arrives — exiting cleanly with a note once the instance stops or pauses (never hanging). This works identically whether or not the current `kt` process is the one that originally started the instance: reading only needs the instance's log file, not a live process handle, so `kt agent logs --follow` also works against an instance recovered by crash adoption in a different `kt agent start` session.

`--json` emits **newline-delimited JSON (NDJSON)**: one complete, self-contained log-line object per stdout line, each carrying its own `schema_version`. This is deliberately not a single wrapping document — `--follow` is an unbounded stream that a wrapper could never close — so the shape is identical for the one-shot and `--follow` forms, and a reader can process each line as it arrives:

```json
{"schema_version": 1, "instance": "my-agent", "stream": "agent-out", "at": "2026-07-20T12:00:00Z", "text": "hello"}
```

Lines are emitted in on-disk append order (never re-sorted by `at`, whose whole-second resolution makes ties common). An empty log emits nothing at all — zero lines, not `[]`. Under `--json`, stdout is pure NDJSON: the rotation notice and the follow-exit note go to stderr like every other diagnostic.

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

### Exit codes

Every `kt` command returns one of these numeric exit codes, so failures can be branched on in a script without parsing stderr:

| Code | Meaning | Typical causes |
|------|---------|----------------|
| `0` | Success | The command completed; `--help` and `--version` also exit `0` |
| `1` | General error | An internal or unexpected failure: filesystem/IO, state store, config load, launch failure, an invalid or unreadable adapter manifest, an adapter declaring no capabilities or no metering source, a failed self-update |
| `2` | Usage error | An invalid invocation: an unknown flag or a missing/invalid argument, an invalid instance name, an unknown adapter kind, an unknown config key, or a duplicate instance name |
| `3` | Not found | The named Agent Instance does not exist, or no `adapter.toml` was found at the given `--manifest` path |
| `4` | Invalid state | The instance is not in a state that permits the operation: not running, an invalid lifecycle transition, removing a running instance without `--force`, or a stop that could not be confirmed |
| `5` | Unsupported capability | Either the agent's Capability Declaration forbids the operation on this OS (e.g. `pause` or `send` declared `unsupported`), or the operation needs a live interaction channel this session cannot reach — `kt agent send` to an instance adopted from an earlier session has no recoverable stdin pipe |
| `6` | Timed out | A bounded operation exceeded its deadline (e.g. `send` when the agent is not draining its input) |

A script branches on the code directly — no stderr parsing:

```bash
kt agent show my-agent --json > status.json
code=$?
if [ $code -eq 3 ]; then
  kt agent register my-agent --kind mock
elif [ $code -eq 4 ]; then
  kt agent start my-agent
elif [ $code -ne 0 ]; then
  echo "unexpected failure (exit $code)" >&2
  exit 1
fi
```

Here `3` means "not registered yet", `4` means "registered but not in a usable state", and any other non-zero code is a genuine failure worth surfacing.

Every command that writes machine-readable output to stdout keeps that output pure, so `kt agent logs my-agent --json | head -5` is safe: a consumer that stops reading ends the command cleanly with `0` rather than an I/O failure.

These codes are a **v1 compatibility surface**, governed by the same deprecation policy as the `--json` schemas: a breaking change is announced in the release notes, carries at least a one-minor notice window, and is removed only at a major version. Compatibility tests assert each documented condition returns its documented code, so an unannounced change fails CI.

### Update checks

When a `kt` subcommand runs, Ktesio checks whether a newer GitHub Release is available, using an hourly cache. If an update is available, it prints a short stderr notice asking you to run `kt self-update`. Machine-readable JSON on stdout is unaffected. Disable checks with `KTESIO_NO_UPDATE_CHECK=1` (also skipped when `CI=true`).

### `kt self-update`

Update the `kt` binary itself, preserving the current install channel (Homebrew, Cargo, or a manual release binary).

```bash
kt self-update
```
