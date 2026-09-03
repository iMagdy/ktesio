---
title: Adapter Manifest
description: The adapter.toml shape that registers an agent — how to launch it, its per-OS capabilities, its metering source, and its config mapping.
---

# Adapter Manifest (`adapter.toml`)

A **manifest adapter** registers any agent from a single `adapter.toml`. It declares how the engine launches the agent process, the agent's per-OS capabilities, its metering source, and (optionally) how unified config keys map into the agent's native mechanism. Register one with:

```bash
kt agent register <name> --manifest <dir-or-adapter.toml>
```

The manifest is parsed and validated **before** any state is written. Unknown keys are rejected (typo protection), and the first missing or invalid mandatory section is named in the error.

## Complete Example

```toml
contract_version = "0.4.0"

[adapter]
kind = "my-agent"
name = "My Agent"

[lifecycle.start]
exec = "my-agent"
args = ["--serve"]
env = { LOG_LEVEL = "info" }

[lifecycle.stop]
exec = "my-agent"
args = ["--shutdown"]

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[capabilities.interaction]
linux = "guaranteed"
macos = "guaranteed"
windows = "guaranteed"

[metering]
source = "self-reported"

[interaction]
channel = "stdio"

[config.model]
env = "MODEL"
```

## Mandatory Sections

A manifest is valid only if it declares all of the following. Validation reports the first one that is missing, empty, or invalid.

### `contract_version`

The Adapter Contract version the manifest targets, as a semver string (current: `"0.4.0"`). A non-semver value is rejected.

Any valid semver string is accepted today — there is no minimum and no negotiation; `"current"` is informational only. The engine does not refuse an older contract version.

### `[adapter]`

Adapter identity.

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `kind` | string | yes | The adapter kind, stored on the Agent Instance |
| `name` | string | no | A human-friendly name |

### `[lifecycle]`

Lifecycle op templates. At minimum `[lifecycle.start]` is required; `stop`, `pause`, and `resume` are optional.

Each op template has:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `exec` | string | yes | The executable to run (a program name on `PATH`, or an absolute path) |
| `args` | array of strings | no | Positional arguments (default empty) |
| `env` | table of strings | no | Environment overrides (default empty) |

The resolved start launch (`exec`/`args`/`env`) is snapshotted **at registration**: editing the manifest afterward has no effect on an already-registered instance until it is removed and re-registered.

### `[capabilities]`

A non-empty, per-OS Capability Declaration. Each capability is a sub-table keyed by OS (`linux`, `macos`, `windows`) whose value is a support level: `"guaranteed"`, `"best-effort"`, or `"unsupported"`. A capability with no entry for an OS projects to `unsupported`. The declaration must have at least one non-`unsupported` entry.

Modeled capabilities today are `pause` and `interaction`:

```toml
[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"
```

`pause` is typically `guaranteed` on Unix (SIGSTOP) and `best-effort` on Windows.

### `[metering]`

A viable Metering Source. `source` is one of:

- `"self-reported"` — the agent emits its own usage accounting (`KTESIO_USAGE {json}` sentinel lines on stdout).
- `"engine-observed"` — the engine meters the agent's model traffic through a per-instance loopback proxy. The operator points the agent's OpenAI-compatible `base_url` at the engine (via the config mapping below) and sets `metering.upstream_base_url` to the real provider endpoint.

```toml
[metering]
source = "self-reported"
```

A `self-reported` agent forwards its own usage accounting by emitting `KTESIO_USAGE {json}` sentinel lines on stdout. The JSON payload carries three agent-supplied fields (snake_case):

```json
{"sequence": 0, "input_tokens": 128, "output_tokens": 512}
```

| Field | Type | Meaning |
|-------|------|---------|
| `sequence` | non-negative integer | A per-Run monotonic ordinal stamped by the agent. It is the replay-dedup key: a re-delivered batch with the same `sequence` is recognized and not double-counted. |
| `input_tokens` | non-negative integer | Input token count for the event. |
| `output_tokens` | non-negative integer | Output token count for the event. |

The engine stamps the Run id, the instance, the Metering Source, and the timestamp. A malformed usage line is a diagnostic (ignored), never fatal.

## Optional Sections

### `[interaction]`

Interaction channel wiring. Optional: omitting this section entirely still means `"stdio"` — the engine unconditionally pipes stdin for every spawned process, regardless of what (or whether) this section says.

| Field | Type | Meaning |
|-------|------|---------|
| `channel` | closed enum | The interaction channel. Currently the only recognized value is `"stdio"` (the spawned child's OS stdin pipe) — an unrecognized value is rejected at parse time. |

### `[config]` — unified → native config mapping

Maps documented unified config keys into the agent's native mechanism at start time, so operators configure any agent in one vocabulary. Each `[config.<key>]` sub-table declares **exactly one** of three targets:

```toml
# Deliver as an environment variable.
[config.model]
env = "MODEL"

# Deliver as a CLI flag (rendered as two argv tokens: --model <value>).
[config.model]
flag = "--model"

# Render into a native TOML file inside the Agent Home, at a dotted native key.
[config.model]
file = { path = "config/agent.toml", key = "llm.model" }
```

Notes:

- A `file` target's `path` is **relative to the Agent Home**; an absolute path or one escaping the home is rejected at load time (the engine is the sole writer).
- A documented key the adapter maps nowhere is a silent no-op — not every adapter supports every unified key.
- `agent.*` pass-through keys are delivered verbatim (by convention, as an env var named by the key tail) without a mapping entry.
- For a `secret:NAME` value, the resolved cleartext is delivered into the native target while every Ktesio display of the same key stays masked. Prefer `env`/`file` targets over `flag` for secret-carrying keys — an argv flag is visible to other local users on the process list.

## See Also

- [Command reference](commands.md) — the `kt agent` commands and unified config keys.
- [Architecture](architecture.md) — the Adapter Contract, the Usage Ledger, and budget enforcement.
- [The Conformance Test Kit](testing.md#the-conformance-test-kit) — add `ktesio-conformance` as a dev-dependency and prove your `adapter.toml` honors the contract from your own `#[test]`.
