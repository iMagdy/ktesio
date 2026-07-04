---
title: Architecture
description: A compact tour of Ktesio's Rust modules, install flow, and file-based project state.
---

# Architecture

Ktesio ships a single `kt` binary, built from a Cargo workspace. The CLI keeps domain logic small and file-based so users can understand and repair project state manually when needed.

## Workspace Layout

```text
crates/
├── kt/                     # package "ktesio" — the shipping kt CLI (all current behavior)
├── ktesio-engine/          # engine library (registration + adapter resolution live)
├── ktesio-adapter-api/     # Adapter Contract: trait, per-OS capability + metering types, manifest schema
├── ktesio-adapters-hermes/ # native adapter home (reserved skeleton)
└── ktesio-conformance/     # adapter conformance fixtures (mock adapter + scripted fake agent)
```

`kt` may depend only on `ktesio-engine`'s public API (plus `ktesio-adapter-api` types); CI enforces that dependency boundary. `ktesio-adapter-api` depends on nothing internal — it owns the Adapter Contract types and the `adapter.toml` manifest schema (with validation), versioned under a contract-version constant. The engine consumes that crate's parsed form and defines no schema of its own. The `ktesio-conformance` mock adapter is a dev/test fixture: the engine and `kt` reference it as a dev-dependency only, so it never appears in the shipping dependency graph (a normal edge would trip the boundary gate). The `hermes` adapter crate stays a reserved skeleton until epic 6.

### Engine modules

The engine follows a hexagonal layout (domain core + ports + backing implementations). Registration and the agent lifecycle (start/stop) are live:

```text
crates/ktesio-engine/src/
├── lib.rs      # re-exports the public API (the Embedding Interface)
├── engine.rs   # the async Engine handle + its blocking() facade (owns the tokio runtime + the supervisor)
├── adapter/    # adapter resolution: native builtins + manifest loader/validator; also resolves the start launch (exec/args/env)
├── domain/     # core: LifecycleState, the transition table + events, AgentInstance, the Registry service, the Supervisor
├── ports/      # hexagonal ports: StateStore + ProcessBackend traits (+ SpawnSpec/StopOutcome/ProcessStatus/errors)
├── backends/   # the ONLY OS-conditional code: unix/ (process groups + signals) and windows/ (Job Objects), selected per OS
├── store/      # SQLite StateStore implementation + schema/migrations (internal)
├── paths.rs    # engine-only path authority (state dir + Agent Home), resolved cross-platform
└── time.rs     # RFC 3339 UTC timestamp formatting
```

The engine is the sole path authority: it computes the state-directory location and each Agent Home layout; `kt` receives paths from the API and never constructs them. All registry and lifecycle state lives in one SQLite database (WAL journaling, `synchronous=NORMAL`, foreign keys on) under the engine state directory; bulky per-instance artifacts live as files inside each Agent Home. Errors use `thiserror` inside the engine and are wrapped into `miette` diagnostics in `kt`.

Registration resolves an adapter before any state is written. A native adapter is selected by kind (`--kind`) from a small builtin table; a manifest adapter is loaded from a directory or file (`--manifest`), its `adapter.toml` parsed and validated by `ktesio-adapter-api`. The adapter's per-OS Capability Declaration and Metering Source are validated first — an adapter with no capabilities or no viable metering source is rejected, and nothing is written — then the row and Agent Home are created. The effective (current-OS) Capability Declaration is projected as data (via a runtime OS identifier, never conditional compilation) and persisted as a JSON snapshot in the Agent Home, so `kt agent show` can render it.

#### Async engine + blocking facade (AD-13)

The engine runs its supervision core on a tokio multi-thread runtime. The public `Engine` API is asynchronous; blocking filesystem and SQLite work runs on tokio's blocking pool (`spawn_blocking`), since rusqlite is a synchronous C binding that must never stall an async worker. A thin `blocking()` facade wraps each async method in `runtime.block_on(...)`; `kt` uses that facade and stays a synchronous binary (no async main, no TTY or prompts inside the engine — interactivity lives only in `kt`). A Host embedding the engine with its own runtime calls the async methods directly.

#### Lifecycle: transition table, supervisor, and per-OS process backends (AD-4, AD-15)

The agent lifecycle is a data-driven state machine: one pure transition table maps `(state, command)` to the next state or a single uniform `InvalidTransition` error, so an invalid command (for example `stop` on a stopped instance) is rejected identically for every adapter — the rejection comes from the shared table before any adapter code runs. The command set is `start`, `stop`, `pause`, and `resume`; the wired edges are `registered/stopped → starting`, `starting → running` (adapter ready) or `→ failed` (launch error), `running → stopping → stopped`, and — added with pause — `running → paused`, `paused → running`, and `paused → stopping` (a paused instance is stoppable). A supervisor owns the running instances' process handles in memory for the current engine lifetime and drives each transition: apply the table, act via the process backend, persist the new state, and record a transition event. Each transition is recorded to a per-instance JSON-Lines event log in the Agent Home (the agent's own stdout/stderr are captured to a separate file); the escalation from a graceful to a forced stop is recorded there too, as is the best-effort qualifier on a cooperative pause (below).

All process control goes through one `ProcessBackend` port whose methods speak in domain terms, never OS syscalls. The per-OS implementations are the only place in the workspace that uses OS-conditional compilation: on Unix each agent is spawned into its own process group and stopped with `SIGTERM`, escalating to `SIGKILL` across the whole group after a configurable window (default 30s) so no child process survives; on Windows each agent runs in its own Job Object and is terminated with `TerminateJobObject`, killing every process in the job. Cross-restart adoption of processes started by a previous engine is a later story; this slice supervises within a single engine lifetime and cleans up its processes when the engine shuts down.

Pause and resume are honest about what they can guarantee for a given agent on the running OS — "surfaced not silent". The level is read from the instance's persisted Capability Declaration, projected onto the current OS at read time (never re-derived from the manifest, never frozen at registration), and the supervisor dispatches on it three ways. **Guaranteed** (Unix): the backend delivers `SIGSTOP` to the whole process group — a real, verifiable suspension (a heartbeat stops) — and `SIGCONT` on resume; the transition records a plain `pause`/`resume` command cause. **Best-effort** (the Windows default per AD-4, since Windows has no clean guaranteed whole-process suspend from `std` and no undocumented suspend API is used): the state still transitions, but never silently — a visible qualifier is emitted both in the transition event (a dedicated `pause-best-effort` / `resume-best-effort` cause) and, at the CLI, as a note on stderr. **Unsupported** (including the honest default when pause is simply not declared for the current OS): the command fails fast, quoting the declaration (`EngineError::CapabilityUnsupported`), with no state change, no process signal, and no fake attempt. The Windows best-effort path is behavior-verified on the `windows-latest` CI leg; on Unix hosts it is compile-checked only, exactly like the Windows stop path.

## Modules

```text
crates/kt/src/
├── main.rs          # clap command parsing and dispatch
├── cli/             # command handlers
├── discovery.rs     # fallback local skill discovery
├── error.rs         # miette/thiserror diagnostics
├── git.rs           # git CLI wrapper functions
├── install_channel.rs # detection of how kt was installed (cargo, Homebrew, manual)
├── install_target.rs # git URL, local path, and GitHub shorthand resolution
├── lockfile.rs      # skills.lock load/save/validation
├── manifest.rs      # skills.json load/save/validation
├── skills_sh.rs     # skills.sh search client, normalization, and retries
├── skill.rs         # copy and remove skill files
├── ui.rs            # shared terminal colors, icons, statuses, and progress bars
└── update_check.rs  # cached latest-release check for update notices
```

## Command Flow

### Install

```text
read skills.json
for each dependency:
  clone repo into a temporary workspace with quiet git output and progress updates
  apply rev selector when present
  read source skills.json
  copy only selected published paths into a staged install directory
  if source skills.json is missing:
    ask before discovering directories under skills/, SKILLS/, or .agents/skills/
    copy selected directories into the staged install directory
  move staged content into .agents/skills/<name>/
  record HEAD commit in skills.lock after successful copy
write skills.lock only when entries changed
```

When no manifest is present, `kt install` looks for a local `skills/`, `SKILLS/`, or `.agents/skills/` directory and installs a discovered skill as a fallback.

GitHub shorthand such as `owner/repo` is resolved before cloning. For manifest dependencies, the dependency key is the source repo's published skill name.

### Search

```text
read query and limit
use authenticated skills.sh API when KTESIO_SKILLS_SH_API_KEY exists
otherwise use the public skills.sh search endpoint
retry 429, 503, and transient transport errors up to 3 total attempts
normalize results into GitHub install targets when possible
optionally install the selected result through the normal install flow
```

### Publish

```text
load existing skills.json or create an empty manifest
select local path dependencies or repo-local skill paths
write selected entries to publish
save skills.json
```

### Upgrade

```text
read skills.lock or skills.json
for each skill directory:
  git fetch origin with quiet git output
  resolve default branch
  checkout origin/<default-branch>
  update commit in skills.lock
write skills.lock
```

## Design Choices

- Ktesio shells out to `git` instead of using libgit2 so user SSH keys, credential helpers, proxies, and platform git config work normally.
- Git clone, fetch, and checkout output is captured so users see Ktesio progress bars instead of raw git progress. Failure messages include the useful git summary line.
- The manifest and lockfile are JSON because they are easy to inspect, diff, and repair.
- Partial failures are collected and reported after a command finishes processing remaining skills.
- Tests use local temporary git repositories instead of network fixtures.

## See Also

- [Manifest format](manifest.md)
- [Lockfile format](lockfile.md)
- [Testing](testing.md)
