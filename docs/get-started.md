---
title: Getting Started
description: Register an AI agent, give it a budget, inspect it, and run it under supervision — end to end.
---

# Quickstart

This guide runs an agent through Ktesio end to end: register it, budget it, inspect the Fleet, and drive its lifecycle.

## Install Ktesio

Install the `kt` binary (see the [installation guide](installation.md) for every channel):

```bash
curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

Or build from source:

```bash
git clone https://github.com/iMagdy/ktesio.git
cd ktesio
cargo install --path .
```

Verify:

```bash
kt --version
kt agent --help
```

## Describe Your Agent With a Manifest Adapter

Ktesio registers an agent through an **adapter** — either a native builtin (`--kind`) or a **manifest adapter** you supply as an `adapter.toml` (`--manifest`). A manifest declares how to launch the agent, its per-OS capabilities, and its metering source.

Create a directory `my-agent/` containing `adapter.toml`:

```toml
contract_version = "0.3.0"

[adapter]
kind = "my-agent"
name = "My Agent"

# How the engine launches the agent. exec must resolve on PATH (or be absolute);
# args and env are optional. Replace this with your agent's real command.
[lifecycle.start]
exec = "my-agent"
args = ["--serve"]

# A non-empty, per-OS Capability Declaration (linux / macos / windows), each
# "guaranteed", "best-effort", or "unsupported".
[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

# A viable Metering Source: "self-reported" or "engine-observed".
[metering]
source = "self-reported"
```

See the [adapter manifest reference](manifest.md) for every section and field.

## Register the Agent

```bash
kt agent register my-agent --manifest ./my-agent
```

Registration validates the manifest, creates an isolated **Agent Home**, and prints its path plus the effective (current-OS) Capability Declaration. Nothing is written if validation fails.

To try the flow without writing a manifest, register the native builtin:

```bash
kt agent register demo --kind mock
```

`mock` is a registration/config fixture — it declares capabilities and a metering source but has **no launch command**, so it cannot be started. Use a manifest adapter to run a real process.

## Set a Budget and a Cost Cap

Budgets and rates are ordinary unified-config values, validated at write time and changeable at any time:

```bash
# Token budget: cap cumulative usage, and pause the agent when it is reached.
kt agent config set my-agent budget.tokens.cumulative 500000
kt agent config set my-agent budget.breach_action pause

# Optional dollar cost control: price tokens in $/1M, then cap the derived cost.
kt agent config set my-agent cost.rate.input 3.00
kt agent config set my-agent cost.rate.output 15.00
kt agent config set my-agent budget.dollars.cumulative 10.00
```

The Breach Action (`pause`, `stop`, or `warn`) fires the instant a ceiling is reached, on real usage from the Usage Ledger. A dollar cap set without a Rate is inert until a Rate exists.

## Inspect the Fleet

```bash
kt agent list                  # name, kind, state, restarts, budget, usage
kt agent show my-agent         # capabilities, runtime status, usage, budget, cost, metering source
kt agent config get my-agent   # the effective config with the source layer of each value
```

Add `--json` to `list` or `show` for a versioned, machine-readable document. Token totals equal the Usage Ledger exactly; dollar figures appear only when a Rate is configured and are always labeled estimates.

## Drive the Lifecycle

```bash
kt agent start my-agent
kt agent pause my-agent
kt agent resume my-agent
kt agent stop my-agent --timeout 10
```

`pause` is honest per-OS: a guaranteed pause suspends the process, a best-effort pause proceeds cooperatively and prints a visible note, and an unsupported pause fails fast quoting the Capability Declaration. `stop` requests a graceful shutdown and escalates to a forced kill after the window (`--timeout`, default 30s).

> **Supervision boundary:** a standalone `kt agent start` supervises the process only for that command's lifetime and stops it when the command exits. Durable supervision across separate CLI invocations is future work (a supervising daemon is a later epic). If the engine crashes with a surviving process, the next engine open re-adopts it, detects crashes, and applies the Restart Policy.

## Manage Secrets

Reference secrets indirectly with a `secret:NAME` value — the reference is stored, and the real value is resolved from the environment (then the engine secrets file) at start and delivered to the agent, while staying masked in `kt agent config get`, snapshots, logs, and events:

```bash
kt agent config set my-agent agent.api_key secret:OPENAI_KEY
kt agent config get my-agent               # shows secret:**** for that key
kt agent config get my-agent --reveal      # the sole explicit un-mask
```

## Remove an Agent

```bash
kt agent remove my-agent            # keeps the Agent Home by default
kt agent remove my-agent --delete   # also deletes the Agent Home
```

## Next Steps

- Read the [command reference](commands.md).
- Learn the [adapter manifest format](manifest.md).
- Check [troubleshooting](troubleshooting.md) for common setup and PATH issues.
