---
title: Embedding the Engine
description: Drive the Ktesio engine as a Rust library — the facade surface, the event bus, and the hermetic quickstart example.
---

# Embedding the Engine

Ktesio is a library first and a CLI second: the `kt` binary is itself just an
embedder that drives the engine's public Rust facade. A hosting platform can do
the same — register agents, configure them, enforce token and dollar budgets,
subscribe to lifecycle events, and supervise processes, with no CLI, no TTY, and
no prompts. This page is the quickstart; the flow-level guarantees are covered
in [Architecture](architecture.md).

## Adding the dependency

Until the crates are published to crates.io, depend on this repository pinned
to an exact revision:

```toml
[dependencies]
ktesio-engine = { git = "https://github.com/iMagdy/ktesio", rev = "FULL_COMMIT_SHA" }
```

Pin a **full-length commit SHA**, never a branch: a pinned `rev` makes your
build reproducible and upgrades deliberate (the engine's public surface is
CI-guarded against breaking changes between freezes, but a moving target is
still a moving target). After the publish executes (see
[the release runbook](release-process.md#publishing-the-engine-crates-on-hold)),
switch to the versioned crates.io form — the facade you compile against does
not change:

```toml
[dependencies]
ktesio-engine = "0.1"
```

Two things to know before depending: the engine's minimum supported Rust is
**1.96.1** (the workspace `rust-version`; any toolchain at or above it
builds), and Ktesio is **source-available**, not open source — the
[Ktesio Noncommercial-Attribution License 1.0.0](../LICENSE) keeps
noncommercial use free and requires the author's written approval for
commercial use.

Only `ktesio-engine` is needed. It is a normal Rust dependency — the engine
never touches your TTY, never reads stdin, holds no global process state, and
several engines can live in one process side by side, each rooted at its own
state directory.

## The facade surface

Open an engine with `Engine::open(base)` and either call its `async` methods on
your own runtime or take `engine.blocking()` for a synchronous view — the same
surface `kt` uses. The capabilities you will reach for first:

| Facade | Purpose |
|--------|---------|
| `Engine::open(base)` | Open (or create) an engine rooted at a state directory; `None` uses the OS default. |
| `register` / `register_with_adapter` | Register an instance under a built-in adapter kind or a manifest (`adapter.toml`) directory. |
| `set_config` / `effective_config` | Write and read the unified configuration (budgets, rates, model keys) with per-leaf provenance. |
| `start` / `stop` / `pause` / `resume` | Drive the lifecycle; `stop` takes a graceful-shutdown window and kills the whole process group. |
| `subscribe` / `Blocking::subscribe` | Receive the event stream (below). |
| `fleet` / `instance_status` | Read per-instance rows — state, usage, budget remaining, metering source — what `kt agent list` renders. |
| `budget_breach_events` / `transition_events` / `read_agent_log` | Query the durable records directly (a `subscribe` sees only later commits; the query APIs reach the past). |
| `send_input` / `attach_memory` / `detach_memory` | Interaction and memory wiring, where the adapter declares support. |

Every method returns a typed `Result` — the engine reports partial failures
with a reason and a remediation instead of panicking.

## The event bus

`engine.subscribe()` hands you a receiver over a bounded, ordered event bus.
Three rules cover the whole contract:

1. **Subscribe before it happens.** A receiver observes only events committed
   after it subscribed, in commit order, per-instance FIFO. Anything earlier is
   readable through the query APIs.
2. **Payloads are versioned structs.** Every event carries a `schema_version`
   and is one of: a lifecycle transition, a budget breach, or a committed
   usage measurement — the exact wire shapes `kt --json` documents.
3. **A slow subscriber never stalls supervision.** The bus is bounded; if you
   fall more than its capacity behind, your next receive observes `Lagged` and
   resynchronizes at the tail — the dropped events stay readable in the
   durable logs. Drain promptly or poll `try_recv` on your own cadence.

## The quickstart example

A complete, runnable host lives at
[`crates/ktesio-engine/examples/embedding-quickstart.rs`](https://github.com/iMagdy/ktesio/blob/main/crates/ktesio-engine/examples/embedding-quickstart.rs).
Run it from the repository root:

```bash
cargo run -p ktesio-engine --example embedding-quickstart
```

It is deliberately dependency-free — no `kt`, no test fixtures, no helper
crates, not even `tempfile` — so it shows exactly what a host depends on. The
seven legs, matching the numbered steps in the file:

1. **Open**: a scratch state root (a per-process directory under the OS temp
   dir, cleaned up best-effort at the end) and `Engine::open(Some(root))`
   with the blocking facade.
2. **Register** a manifest adapter: the example writes a minimal
   `contract_version = "1.0.0"` `adapter.toml` (see
   [the manifest reference](manifest.md) and the
   [Adapter Contract](adapter-contract.md)) whose `[lifecycle.start]` command
   re-executes the example binary itself as a stand-in agent — a real host
   points that field at its own agent executable. (The engine's built-in
   kinds, such as `hermes`, register with `AdapterRef::Native` instead.)
3. **Configure** one budget key — `budget.tokens.cumulative = 100000` — and
   assert the `effective_config` read-back (value and provenance layer).
4. **Subscribe** before starting, so the transition events are observable.
5. **Start**: the engine spawns the manifest's `[lifecycle.start]` command
   and reaches `running` before the call returns.
6. **Observe**: drain the committed events (`Lagged` resyncs at the tail and
   keeps draining), then assert `instance_status` is `running` and the
   `fleet` row carries the configured budget ceiling.
7. **Stop** with a five-second graceful window, then remove the scratch root
   (best-effort).

CI compiles the example on all three OS legs and **runs** it hermetically on
ubuntu on every push — the quickstart cannot silently rot.

## The boundary is the guarantee

What a host can reach is enforced by the compiler, not by convention: the
engine's private modules are Rust-private, so **if you cannot name it, you
cannot call it** — a host compiles against exactly the same public API `kt`
does. Four instruments keep that statement honest:

- **The dependency-shape gate** — CI's `boundary` job allowlists the internal
  edges of the shipped CLI's graph (`cargo tree`), so `kt` cannot quietly grow
  a dependency on anything but the engine, the adapter-contract types, and the
  built-in hermes adapter; a future internal crate fails the gate
  automatically ([the workflow](https://github.com/iMagdy/ktesio/blob/main/.github/workflows/ci.yml)).
- **The facade audits** — the embed-clean suites prove `kt` consumes only the
  blocking facade (no async APIs, no runtime of its own) and that every async
  method has a blocking counterpart, with no TTY, prompt, or global-state
  escapes.
- **The library-alone flow proof** — a host test drives the full
  register → configure → cap → start → breach → pause → stop journey through
  the facade alone and shares its assertions with the CLI suite, proving the
  library path and the CLI path behave identically
  ([the host test](https://github.com/iMagdy/ktesio/blob/main/crates/ktesio-engine/tests/uj3_library_host.rs)).
- **The semver gate** — CI diffs both public crates' surfaces against their
  freeze baselines, so a breaking change cannot land unnoticed.

## Availability

The crates are **prepared for publication and currently held**: the manifests
still carry `publish = false`, and the exact ordered publish commands live in
[the release runbook](release-process.md#publishing-the-engine-crates-on-hold).
Until that go, pin the git dependency as shown above.
