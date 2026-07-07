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

Because nothing durable lives in volatile memory, that one SQLite database is also what makes Fleet state survive an engine restart or a machine reboot (FR-10). A reboot is simply the "all processes gone" case of engine-crash recovery: the on-disk database and Agent Homes are untouched, so on the next `Engine::open` the orphan reconciliation runs with zero live matches and every previously-running instance is honestly reconciled to `failed` (never left as a phantom `running` row), while cleanly-stopped instances stay `stopped` and every registration, restart policy, and restart count survives byte-intact. The durability loss bound is `≤1s`: each state mutation is one committed transaction under WAL + `synchronous=NORMAL`, so at most the last in-flight transaction can be lost. The Usage Ledger inherits the same one-transaction-per-event bound when metering lands in Epic 3.

The Fleet is observable through `kt agent list` (a human table) and `kt agent list --json` / `kt agent show <name> --json` (a machine-readable document). The JSON is a versioned serde struct carrying a `schema_version` and reusing the same domain types the engine's transition-event log serializes, so `kt --json` and the future Host event stream stay one schema rather than forking into two dialects (AD-14). Each listing opens the engine and reads live persisted state, so any committed transition is reflected on the next listing (well under the 2-second freshness bound) with no cache to invalidate. Metering is Epic 3, so the `budget/cap` and `usage` columns are rendered as an honest, typed absence today — `—` in the human table and `null` in JSON, never a fabricated number — with a one-line stderr note that budget and Usage Ledger totals arrive with metering; the columns are present so the shape stays stable for Epic 3 to populate additively.

Registration resolves an adapter before any state is written. A native adapter is selected by kind (`--kind`) from a small builtin table; a manifest adapter is loaded from a directory or file (`--manifest`), its `adapter.toml` parsed and validated by `ktesio-adapter-api`. The adapter's per-OS Capability Declaration and Metering Source are validated first — an adapter with no capabilities or no viable metering source is rejected, and nothing is written — then the row and Agent Home are created. The effective (current-OS) Capability Declaration is projected as data (via a runtime OS identifier, never conditional compilation) and persisted as a JSON snapshot in the Agent Home, so `kt agent show` can render it.

#### Unified layered configuration (AD-9)

Configuration is layered TOML with a deterministic precedence: **engine defaults < agent-kind defaults < Agent Home instance config < invocation overrides**. A pure, I/O-free resolver folds the four layers into a single effective config with a **structural, per-leaf merge**. Where shapes agree it is a deep merge — setting `a.b` at a stronger layer overrides only `a.b` and leaves a weaker layer's sibling `a.c` intact, so a single override never silently drops sibling keys. Where shapes disagree the **stronger layer's shape wins and prunes** the weaker layer's orphans: a strong scalar at `a.b` masks a weak `[a.b]` subtree (no contradictory `a.b="scalar"` plus `a.b.c=1`), and symmetrically a strong subtree replaces a weak scalar — so the resolved tree is never self-contradictory and every surviving leaf's recorded source layer reflects the layer that actually defines it. The same key set at the instance layer overrides the same key at the kind or engine-default layer, every time and on every machine (the resolver depends on no clock, environment, or OS). The resolver records, per resolved key, which layer supplied the winning value; that per-value **source layer** is now rendered and persisted (see "Effective-config provenance" below).

Config lives as TOML files under path authority, **not** in SQLite: SQLite remains the registry/lifecycle/ledger store, while the engine owns every config path and is the only reader/writer. The four sources are an embedded engine-defaults constant (present but empty today — the engine seeds only unified keys it can honestly honor, and no engine-wide key is config-controlled yet; the Restart Policy default keeps coming from the engine, not from config), per-kind adapter defaults (absent kinds — including `mock` today — resolve to an empty layer, never an error), the per-Agent-Home `config.toml` the registration step already writes (the instance layer `kt agent config set` edits in place), and an ephemeral invocation-override map supplied at resolve time. A malformed layer surfaces a typed error naming the layer and path, never a panic.

Config is validated at **write time**: a write of an unknown key outside the reserved `agent.*` pass-through namespace is rejected before anything is persisted (the instance file is left byte-unchanged), with the nearest valid key suggested via a small edit-distance match over the known-key set — or an honest "no close match" when nothing is near. A write with an empty dotted segment (`agent..b`) is rejected, and a write that would nest a child under an existing scalar (`agent.a.b` when `agent.a` is already a value) **fails closed** rather than silently destroying the scalar — both leave the file byte-unchanged. Keys under `agent.*` are the escape hatch for agent-native extras: they bypass the known-key check and round-trip verbatim (secret resolution/masking for `secret:` values is a later Epic-2 story — here a `secret:` value is ordinary opaque text). `kt agent config set <name> <key> <value>` writes to the instance layer; `kt agent config get <name> [<key>]` reads the effective (resolved) config, printing the value(s) with per-value provenance to stdout with output discipline (results to stdout, diagnostics to stderr). The instance identity (`name`, seeded at registration) is filtered from the resolved view, so it is not presented as a settable key.

#### Unified → native config mapping (AD-9 / FR-12)

Each Adapter maps documented unified keys into the Agent's native mechanism at **start time** — a config file, an environment variable, or a CLI flag — so an operator configures an agent in one unified vocabulary without learning its per-agent format. The mapping is **adapter-declared** in one uniform shape (`ConfigMapping`: unified key → `ConfigTarget::{Env, Flag, File}`), whose types and validation live only in `ktesio-adapter-api` (the engine consumes the parsed form and defines no schema — AD-3). A **manifest** adapter declares it in an optional `[config]` section of `adapter.toml` (`[config.model]` with exactly one of `env = "MODEL"`, `flag = "--model"`, or `file = { path = "...", key = "..." }`); a **native** adapter (the builtin `mock`, later `hermes`) declares the same shape in code via the `AgentAdapter::config_mapping()` accessor. An absent `[config]` section and a native adapter that does not override the accessor both yield an empty mapping — the "two kinds, one trait" invariant. This is an additive, optional extension of the Adapter Contract, so the contract-version constant took an additive minor bump.

The mapping is **applied at start, from the resolved effective config, at one seam**: where the launch spec (`exec`/`args`/`env`) is built for the `[lifecycle.start]` template and before the process is spawned, the engine resolves the instance's effective config (2-1's four-layer fold), reads the adapter's mapping, and places each value into its declared native target — an env var goes into the spawned process's environment, a flag is appended to the launch arguments (as two tokens, `--model gpt-4`), and a file target is rendered into a native TOML file inside the Agent Home (the engine is the sole writer — path authority). A documented key the adapter maps nowhere is a silent no-op (not every adapter supports every unified key), and a file path that is absolute or escapes the Agent Home is rejected at manifest-load time. Keys under `agent.*` are delivered **verbatim** through the same seam — the key-tail after `agent.` and its value, with no rewriting and no known-key lookup (the recorded convention delivers a pass-through key as an env var named by its verbatim tail). In effective-config output, each `agent.*` leaf is rendered as **unvalidated** (a per-row marker derived purely from the pass-through prefix, so the operator sees which values skipped known-key validation) while a known key is rendered as validated. Secret resolution/masking (`secret:` values) remains a later Epic-2 story — a `secret:` value is delivered here as opaque text.

#### Effective-config provenance (AD-9 / FR-13)

An operator can see exactly **what will apply on next start and where each value came from**. The resolver already tags every resolved leaf with the layer that supplied it (`engine-default`, `kind-default`, `instance`, or `invocation-override`); that provenance is now rendered and persisted. `kt agent config get <name>` gains a **Source** column beside the Validated column, naming each value's winning layer, and `kt agent config get <name> [<key>] --json` emits a versioned document whose per-leaf objects carry `{ key, value, source, unvalidated }` — pure JSON on stdout, sourced from the engine's source tag (the CLI never re-derives a layer). At **start**, the engine writes a persisted effective-config snapshot — every resolved value plus its source layer — as `effective-config.json` inside the Agent Home, through path authority (the engine is the sole writer; effective-config snapshots are files in the Agent Home, never SQLite blobs). It mirrors the `adapter.json` snapshot convention: a dedicated versioned JSON document, written with the same mechanics but at start rather than registration, and **overwritten on every successful start/restart** so it always reflects the config resolved for the current run (it answers "what will apply on *next* start"). The write lands right after the native-mapping application and **before** the `starting` transition, so a snapshot-write failure rejects the start cleanly with no state change; the snapshot is not written at registration and not deleted at stop. `config get` continues to resolve **live** (showing what would apply next start); the persisted snapshot is the durable record for Hosts and debugging. Every surface — the human Source column, `--json`, and the snapshot — renders each value through **one display path**, the single choke point at which a later Epic-2 story adds secret masking without touching the rendering call sites.

#### Async engine + blocking facade (AD-13)

The engine runs its supervision core on a tokio multi-thread runtime. The public `Engine` API is asynchronous; blocking filesystem and SQLite work runs on tokio's blocking pool (`spawn_blocking`), since rusqlite is a synchronous C binding that must never stall an async worker. A thin `blocking()` facade wraps each async method in `runtime.block_on(...)`; `kt` uses that facade and stays a synchronous binary (no async main, no TTY or prompts inside the engine — interactivity lives only in `kt`). A Host embedding the engine with its own runtime calls the async methods directly.

#### Lifecycle: transition table, supervisor, and per-OS process backends (AD-4, AD-15)

The agent lifecycle is a data-driven state machine: one pure transition table maps `(state, command)` to the next state or a single uniform `InvalidTransition` error, so an invalid command (for example `stop` on a stopped instance) is rejected identically for every adapter — the rejection comes from the shared table before any adapter code runs. The command set is `start`, `stop`, `pause`, and `resume`; the wired command edges are `registered/stopped → starting`, `starting → running` (adapter ready) or `→ failed` (launch error), `running → stopping → stopped`, `running → paused`, `paused → running`, `paused → stopping` (a paused instance is stoppable), and `failed → starting` (a failed instance is restartable — by the Restart Policy executor or an explicit `start`). One edge is event-driven rather than a command: `running → failed` (**crash detected**) is applied by the supervisor's reaper when a supervised process exits without a requested stop, exactly like `starting → running` / `stopping → stopped`. A supervisor owns the running instances' process handles in memory for the current engine lifetime and drives each transition: apply the table, act via the process backend, persist the new state, and record a transition event. Each transition is recorded to a per-instance JSON-Lines event log in the Agent Home (the agent's own stdout/stderr are captured to a separate file); the escalation from a graceful to a forced stop is recorded there too, as is the best-effort qualifier on a cooperative pause (below), a `crashed` cause on a detected crash, and a `restarted` cause (carrying the restart count + the backoff waited) on each Restart Policy restart.

All process control goes through one `ProcessBackend` port whose methods speak in domain terms, never OS syscalls. The per-OS implementations are the only place in the workspace that uses OS-conditional compilation: on Unix each agent is spawned into its own process group and stopped with `SIGTERM`, escalating to `SIGKILL` across the whole group after a configurable window (default 30s) so no child process survives; on Windows each agent runs in its own Job Object and is terminated with `TerminateJobObject`, killing every process in the job. The port also exposes a durable **start-time fingerprint** (`{ pid, start_time }`) and an `adopt(fingerprint)` re-acquisition, whose per-OS sources live only in the backends: the process start-time comes from `/proc/<pid>/stat` field 22 on Linux, `libproc` `proc_pidinfo(PROC_PIDTBSDINFO)` on macOS, and `GetProcessTimes` creation time on Windows.

Survival — crashes and orphans (AD-5 / AD-15). A crash is detected by a periodic reaper (a tokio interval owned by the engine, ~250ms, calling the sync `poll_once` off the blocking pool) that reuses the same `poll` liveness check and reacts to an unrequested exit with the event-driven `running → failed` edge. A per-instance **Restart Policy** — `never` or the default `on-failure` (persisted per instance; the layered-config engine is Epic 2) — then drives recovery: `on-failure` restarts with exponential backoff (1s base, ×2 per consecutive failure, capped at 60s) and a visible restart count, stopping after exactly 5 consecutive failures with the crash-loop reason stated; a clean run resets the count. Before a spawned process is treated as supervised, a **write-ahead spawn record** `{ instance id, pid, start-time fingerprint, policy, restart count }` is committed to SQLite in one transaction ("no spawn without its record committed first"), and it is cleared on a clean stop. On `Engine::open` the engine reconciles every record against live processes: a matching start-time fingerprint is **adopted** back under supervision (state stays `running`/`paused`, so `stop`/`pause`/`resume` work again and a subsequent stop truly terminates it), while a record whose process is gone — or whose PID was reused by a different process — is honestly reconciled to `failed` with the last-known cause, never left as a phantom `running` row. An adopted process carries its verified start-time on the handle so the reaper re-checks that fingerprint on every liveness poll, not just a bare PID — so if the adopted agent crashes and the OS recycles its PID within a poll interval, the crash is still detected rather than masked by the recycled process. This closes the NFR-1 guarantee: after an engine crash, a surviving agent is re-adopted and no orphan process is left unsupervised. Removal upholds the same guarantee from the other direction: removing a live or adopted instance stops its process first — terminating the whole group/job and clearing its write-ahead record — before the row is deleted, so `remove` never leaves an unsupervised orphan (this holds for both a plain and a `--force` remove; `--force` only governs whether a *running* instance may be removed without an explicit prior stop, not whether the process is torn down).

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
