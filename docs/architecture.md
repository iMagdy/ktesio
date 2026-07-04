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

The engine follows a hexagonal layout (domain core + ports + backing implementations). The registration slice is live:

```text
crates/ktesio-engine/src/
├── lib.rs      # re-exports the public API (the Embedding Interface)
├── adapter/    # adapter resolution: native builtins + manifest loader/validator (parses via adapter-api; executes nothing)
├── domain/     # core: LifecycleState, AgentInstance, InstanceName, RegistryError, the Registry service
├── ports/      # hexagonal ports; StateStore trait + StoreError
├── store/      # SQLite StateStore implementation + schema/migrations (internal)
├── paths.rs    # engine-only path authority (state dir + Agent Home), resolved cross-platform
└── time.rs     # RFC 3339 UTC timestamp formatting
```

The engine is the sole path authority: it computes the state-directory location and each Agent Home layout; `kt` receives paths from the API and never constructs them. All registry and lifecycle state lives in one SQLite database (WAL journaling, `synchronous=NORMAL`, foreign keys on) under the engine state directory; bulky per-instance artifacts live as files inside each Agent Home. Errors use `thiserror` inside the engine and are wrapped into `miette` diagnostics in `kt`.

Registration resolves an adapter before any state is written. A native adapter is selected by kind (`--kind`) from a small builtin table; a manifest adapter is loaded from a directory or file (`--manifest`), its `adapter.toml` parsed and validated by `ktesio-adapter-api`. The adapter's per-OS Capability Declaration and Metering Source are validated first — an adapter with no capabilities or no viable metering source is rejected, and nothing is written — then the row and Agent Home are created. The effective (current-OS) Capability Declaration is projected as data (via a runtime OS identifier, never conditional compilation) and persisted as a JSON snapshot in the Agent Home, so `kt agent show` can render it. Lifecycle execution (starting and stopping agents) is a later story; this slice stores and validates declarations and templates only.

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
