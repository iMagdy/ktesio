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

Because nothing durable lives in volatile memory, that one SQLite database is also what makes Fleet state survive an engine restart or a machine reboot (FR-10). A reboot is simply the "all processes gone" case of engine-crash recovery: the on-disk database and Agent Homes are untouched, so on the next `Engine::open` the orphan reconciliation runs with zero live matches and every previously-running instance is honestly reconciled to `failed` (never left as a phantom `running` row), while cleanly-stopped instances stay `stopped` and every registration, restart policy, and restart count survives byte-intact. The durability loss bound is `≤1s`: each state mutation is one committed transaction under WAL + `synchronous=NORMAL`, so at most the last in-flight transaction can be lost. The Usage Ledger (see "Metering & the Usage Ledger" below) is held to the same bound — each usage event is one committed transaction.

The Fleet is observable through `kt agent list` (a human table) and `kt agent list --json` / `kt agent show <name> --json` (a machine-readable document). The JSON is a versioned serde struct carrying a `schema_version` and reusing the same domain types the engine's transition-event log serializes, so `kt --json` and the future Host event stream stay one schema rather than forking into two dialects (AD-14). Each listing opens the engine and reads live persisted state, so any committed transition is reflected on the next listing (well under the 2-second freshness bound) with no cache to invalidate. The `usage` value now carries **real** token totals from the Usage Ledger (see "Metering & the Usage Ledger" below) and the active Metering Source is shown in Fleet detail; `budget` now carries the **real** token budget — the configured per-run/cumulative ceilings, the Breach Action, and the remaining tokens per scope (see "Budget enforcement" below) — or an honest typed absence (`—` in the human table, `null` in JSON, never a fabricated number) for an instance with no budget configured. No dollar figure is shown yet — tokens only — with a one-line stderr note stating that honest boundary. The fields are present so the shape stays stable for later Epic-3 stories to populate additively.

Registration resolves an adapter before any state is written. A native adapter is selected by kind (`--kind`) from a small builtin table; a manifest adapter is loaded from a directory or file (`--manifest`), its `adapter.toml` parsed and validated by `ktesio-adapter-api`. The adapter's per-OS Capability Declaration and Metering Source are validated first — an adapter with no capabilities or no viable metering source is rejected, and nothing is written — then the row and Agent Home are created. The effective (current-OS) Capability Declaration is projected as data (via a runtime OS identifier, never conditional compilation) and persisted as a JSON snapshot in the Agent Home, so `kt agent show` can render it.

#### Metering & the Usage Ledger (AD-7 / AD-6 / FR-19)

Consumption is tracked from day one. Every registered adapter declares a viable **Metering Source** — `self-reported` (the agent forwards its own usage accounting) or `engine-observed` (a later story) — resolved at registration and persisted on the adapter snapshot. The engine ingests usage per that declaration into a per-instance **Usage Ledger**: the append-only `usage_events` table in the one SQLite store, one committed transaction per event (the same `≤1s` durability substrate as lifecycle state).

The `self-reported` channel reuses the log capture the engine already owns: the agent emits `KTESIO_USAGE {json}` sentinel lines on its stdout (the JSON carries `sequence`, `input_tokens`, `output_tokens` — snake_case), the engine captures them into the per-instance agent-output log, and the supervisor's reaper drains the newly-captured tail, parses each line, and records it. A malformed usage line is a diagnostic (skipped), never fatal and never mixed into `kt`'s own output. The ingestion side is a hexagonal port (`UsageSource`, distinct from the declaration enum), so the `engine-observed` loopback listener is a later implementation behind the same port.

A **Run** is the span from a `starting` transition to the next terminal state (`stopped`/`failed`) of an Agent Instance. The supervisor mints a fresh Run id at each `starting` (an operator start or a Restart-Policy restart both open a new Run), holds it in memory alongside the process handle, and stamps it on every usage event ingested during that Run — so per-run totals never bleed across a crash/restart boundary. A per-run total is the sum over `usage_events` scoped to `(instance, run)`; the cumulative total sums all of the instance's rows.

Ingestion is idempotent by construction: each event carries the agent-supplied `sequence` ordinal, recorded under a `UNIQUE(instance_id, run_id, sequence)` index, so a **delayed or replayed batch is recognized and skipped, not double-counted** — "no double-count" is a database invariant, not fragile application bookkeeping. All usage writes funnel through **one** engine choke point (the sole `usage_events` writer — no other code path may mutate the ledger), so token-budget enforcement (Epic 3.2) slots into that same commit path with no re-plumbing. The committed event also rides the versioned event schema as a `usage update` struct (frozen now; Host delivery is Epic 7), so `kt --json` and the future Host stream stay one schema (AD-14).

Fleet detail surfaces the ledger honestly: `usage` shows real cumulative and current-Run **token** totals that equal the ledger exactly, the active Metering Source is visible, and `budget` now shows the real **token** budget (see "Budget enforcement" below). It renders **tokens only** — there is no dollar figure until a Rate exists (Epic 3.3); that cell stays a typed absence. Because the ledger touches the adapter-facing metering surface (the documented usage-line channel), the Adapter Contract version took an additive minor bump; budget enforcement is engine-side and touches no adapter surface, so it took **no** further bump.

#### Budget enforcement (AD-7 / FR-18 / FR-21)

Token consumption is bounded even when nobody is watching. An operator sets a **Token Budget** per Agent Instance at two scopes — **per-run** and **cumulative** — as engine-namespace config values (`budget.tokens.per_run`, `budget.tokens.cumulative`), plus a **Breach Action** (`budget.breach_action`) among `pause` (the ratified default), `stop`, or `warn`. These are ordinary layered-config keys (validated at write time: a non-numeric budget or an unknown action is rejected before anything persists, never silently defaulted), so a budget is **inspectable and changeable while `running` and applies immediately** — the enforcer reads the *current* resolved config on every ingestion, not a value frozen at start.

Enforcement is the last stage of the one metering pipeline, and it runs **inside the same commit path** as the ledger write (the AD-7 rule): the instant a fresh usage event commits at the sole ingestion choke point, a **pure `BudgetEvaluator`** compares the just-committed per-run and cumulative totals against the resolved budget and returns a decision. The evaluator is total, I/O-free, and unit-tested exhaustively; it *decides*, the supervisor *acts* — so the boundary/scope logic is testable without spawning anything. The threshold is **`≥`**: consumption *reaching* a ceiling of `N` (total `≥ N`) is the breach, so the guardrail fires at the ceiling, not one token past it. Both scopes are enforced on every event; when both would trip, the tighter per-run scope is reported (the action is the same either way). This is the sole enforcement site — no other code path evaluates a budget or triggers an action (the companion to the ledger's single-writer invariant), so the enforcement race the AD explicitly forbids can never open between "usage recorded" and "budget checked".

On a breach the supervisor first **records the breach event** — a versioned, `schema_version`-stamped serde struct (instance, run, scope, limit, observed, action, metering source, timestamp; tokens only) appended to a durable per-instance breach log — **before, and independently of,** the lifecycle side-effect, so a best-effort, unsupported, or failed pause never loses the breach record (the FR-21 "always recorded regardless of action" invariant). It then executes the Breach Action through **Epic 1's existing lifecycle**: `pause` drives `running → paused` via the existing pause path (honoring the adapter's pause Capability Declaration exactly — a guaranteed pause suspends, a best-effort pause transitions with its honest posture, an unsupported pause is surfaced honestly and is *not* faked and *not* silently escalated to stop); `stop` drives `running → stopping → stopped`; `warn` performs no transition at all. A budget breach is a new **cause** (`TransitionCause::BudgetExceeded`, carrying the scope/limit/observed) on those existing transitions — not a new state and not a new transition-table edge — so the lifecycle log itself explains *why* while the standalone breach event is the subscription payload (Host delivery is Epic 7). Enforcement is best-effort to the Run: a lifecycle error is a diagnostic on the engine log/stderr, never a crash of the supervision loop (the ledger's "ingestion must never crash the supervisor" rule extends to enforcement).

Fleet detail's `budget` cell reports the configured ceiling(s), the Breach Action, and the **remaining** tokens per scope (ceiling − current total, saturating at zero) — computed from the same ledger totals `usage` reports, so it equals the ledger exactly. An instance with no budget configured shows an honest absent budget (`—` / `null`), never a fabricated ceiling. Dollars stay absent until a Rate exists (Epic 3.3), which reuses this same evaluator/breach/lifecycle machinery in front of a token→dollar derivation.

#### Unified layered configuration (AD-9)

Configuration is layered TOML with a deterministic precedence: **engine defaults < agent-kind defaults < Agent Home instance config < invocation overrides**. A pure, I/O-free resolver folds the four layers into a single effective config with a **structural, per-leaf merge**. Where shapes agree it is a deep merge — setting `a.b` at a stronger layer overrides only `a.b` and leaves a weaker layer's sibling `a.c` intact, so a single override never silently drops sibling keys. Where shapes disagree the **stronger layer's shape wins and prunes** the weaker layer's orphans: a strong scalar at `a.b` masks a weak `[a.b]` subtree (no contradictory `a.b="scalar"` plus `a.b.c=1`), and symmetrically a strong subtree replaces a weak scalar — so the resolved tree is never self-contradictory and every surviving leaf's recorded source layer reflects the layer that actually defines it. The same key set at the instance layer overrides the same key at the kind or engine-default layer, every time and on every machine (the resolver depends on no clock, environment, or OS). The resolver records, per resolved key, which layer supplied the winning value; that per-value **source layer** is now rendered and persisted (see "Effective-config provenance" below).

Config lives as TOML files under path authority, **not** in SQLite: SQLite remains the registry/lifecycle/ledger store, while the engine owns every config path and is the only reader/writer. The four sources are an embedded engine-defaults constant (present but empty today — the engine seeds only unified keys it can honestly honor, and no engine-wide key is config-controlled yet; the Restart Policy default keeps coming from the engine, not from config), per-kind adapter defaults (absent kinds — including `mock` today — resolve to an empty layer, never an error), the per-Agent-Home `config.toml` the registration step already writes (the instance layer `kt agent config set` edits in place), and an ephemeral invocation-override map supplied at resolve time. A malformed layer surfaces a typed error naming the layer and path, never a panic.

Config is validated at **write time**: a write of an unknown key outside the reserved `agent.*` pass-through namespace is rejected before anything is persisted (the instance file is left byte-unchanged), with the nearest valid key suggested via a small edit-distance match over the known-key set — or an honest "no close match" when nothing is near. A write with an empty dotted segment (`agent..b`) is rejected, and a write that would nest a child under an existing scalar (`agent.a.b` when `agent.a` is already a value) **fails closed** rather than silently destroying the scalar — both leave the file byte-unchanged. Keys under `agent.*` are the escape hatch for agent-native extras: they bypass the known-key check and round-trip verbatim (a `secret:NAME` value is stored as an ordinary TOML string here — the reference; it is resolved + masked at start/read per **Secrets (AD-10 / FR-14)** below). `kt agent config set <name> <key> <value>` writes to the instance layer; `kt agent config get <name> [<key>]` reads the effective (resolved) config, printing the value(s) with per-value provenance to stdout with output discipline (results to stdout, diagnostics to stderr). The instance identity (`name`, seeded at registration) is filtered from the resolved view, so it is not presented as a settable key.

#### Unified → native config mapping (AD-9 / FR-12)

Each Adapter maps documented unified keys into the Agent's native mechanism at **start time** — a config file, an environment variable, or a CLI flag — so an operator configures an agent in one unified vocabulary without learning its per-agent format. The mapping is **adapter-declared** in one uniform shape (`ConfigMapping`: unified key → `ConfigTarget::{Env, Flag, File}`), whose types and validation live only in `ktesio-adapter-api` (the engine consumes the parsed form and defines no schema — AD-3). A **manifest** adapter declares it in an optional `[config]` section of `adapter.toml` (`[config.model]` with exactly one of `env = "MODEL"`, `flag = "--model"`, or `file = { path = "...", key = "..." }`); a **native** adapter (the builtin `mock`, later `hermes`) declares the same shape in code via the `AgentAdapter::config_mapping()` accessor. An absent `[config]` section and a native adapter that does not override the accessor both yield an empty mapping — the "two kinds, one trait" invariant. This is an additive, optional extension of the Adapter Contract, so the contract-version constant took an additive minor bump.

The mapping is **applied at start, from the resolved effective config, at one seam**: where the launch spec (`exec`/`args`/`env`) is built for the `[lifecycle.start]` template and before the process is spawned, the engine resolves the instance's effective config (2-1's four-layer fold), reads the adapter's mapping, and places each value into its declared native target — an env var goes into the spawned process's environment, a flag is appended to the launch arguments (as two tokens, `--model gpt-4`), and a file target is rendered into a native TOML file inside the Agent Home (the engine is the sole writer — path authority). A documented key the adapter maps nowhere is a silent no-op (not every adapter supports every unified key), and a file path that is absolute or escapes the Agent Home is rejected at manifest-load time. Keys under `agent.*` are delivered **verbatim** through the same seam — the key-tail after `agent.` and its value, with no rewriting and no known-key lookup (the recorded convention delivers a pass-through key as an env var named by its verbatim tail). In effective-config output, each `agent.*` leaf is rendered as **unvalidated** (a per-row marker derived purely from the pass-through prefix, so the operator sees which values skipped known-key validation) while a known key is rendered as validated. For a `secret:NAME` leaf, this seam is where **display and delivery diverge**: the value placed into the native mechanism is the **resolved cleartext** (the agent needs a usable key), while every display of the same leaf is masked — see **Secrets (AD-10 / FR-14)** below.

#### Effective-config provenance (AD-9 / FR-13)

An operator can see exactly **what will apply on next start and where each value came from**. The resolver already tags every resolved leaf with the layer that supplied it (`engine-default`, `kind-default`, `instance`, or `invocation-override`); that provenance is now rendered and persisted. `kt agent config get <name>` gains a **Source** column beside the Validated column, naming each value's winning layer, and `kt agent config get <name> [<key>] --json` emits a versioned document whose per-leaf objects carry `{ key, value, source, unvalidated }` — pure JSON on stdout, sourced from the engine's source tag (the CLI never re-derives a layer). At **start**, the engine writes a persisted effective-config snapshot — every resolved value plus its source layer — as `effective-config.json` inside the Agent Home, through path authority (the engine is the sole writer; effective-config snapshots are files in the Agent Home, never SQLite blobs). It mirrors the `adapter.json` snapshot convention: a dedicated versioned JSON document, written with the same mechanics but at start rather than registration, and **overwritten on every successful start/restart** so it always reflects the config resolved for the current run (it answers "what will apply on *next* start"). The write lands right after the native-mapping application and **before** the `starting` transition, so a snapshot-write failure rejects the start cleanly with no state change; the snapshot is not written at registration and not deleted at stop. `config get` continues to resolve **live** (showing what would apply next start); the persisted snapshot is the durable record for Hosts and debugging. Every surface — the human Source column, `--json`, and the snapshot — renders each value through **one display path**, the single choke point at which secret masking hooks (see **Secrets** below) without touching the rendering call sites.

#### Secrets (AD-10 / FR-14 / NFR-6)

Secret-classified config values (API keys, tokens) are referenced **indirectly** and never logged, echoed, or rendered unmasked — safe by construction. A config value whose string form is `secret:NAME` (a non-empty `NAME` after the `secret:` prefix — a bare `secret:` is ordinary text) is a **secret reference**: the reference is what is stored in `config.toml`; the real value is never persisted by the engine.

**Resolution (a hexagonal port, AD-10).** At **start**, in the supervisor's start seam (right after the effective config is resolved and before the native-mapping application, so it is still before any persisted state change), each `secret:NAME` leaf is resolved through the `SecretResolver` port. v1 composes two resolvers in order: **process environment first** (`secret:OPENAI_KEY` → `std::env::var("OPENAI_KEY")` — the operator's ad-hoc override), then the **engine secrets file** at `<state base>/secrets.toml` (a TOML `NAME = "value"` table — the durable store). A reference resolved by neither **rejects the start** with a typed error naming the `NAME` and the resolvers tried (never a value), leaving the instance in its prior state — no half-launch, mirroring how a snapshot-write failure rejects. An OS-keychain resolver is a deferred implementation behind the same port. The resolved cleartext lives only in a `SecretString` newtype whose `Display` and `Debug` both redact (`[REDACTED]`) and which is not `Serialize`-derived, so a secret in `launch.env`/`launch.args` cannot leak through a `{:?}` on a launch spec or an event payload; the cleartext is reachable only through an explicit, greppable `expose_secret()`.

**The 0600 secrets file, cross-OS.** The secrets file is expected at mode `0600` (owner-only). The permission **inspection** is OS-specific, so it lives only in `backends::{unix,windows}` (the sole allowlisted `#[cfg]` home, AD-4). On **Unix** the resolver reads the file's mode bits and **refuses** a group/other-accessible file (`mode & 0o077 != 0`) with a `chmod 600` remediation — a world-/group-readable secrets file defeats the guarantee. On **Windows** unix mode bits do not exist; v1 takes a **documented portable posture**: it does not attempt a unix-style refusal and instead relies on the default per-user profile ACLs (the state dir lives under the user's profile). This is an honest boundary — it avoids a false pass masquerading as a unix-grade check and avoids a hard failure that would make secrets unusable on Windows; a future ACL-checking resolver can strengthen it behind the same port.

**Masking at one choke point; delivery diverges.** Because provenance routed the human table, `config get --json`, and the persisted `effective-config.json` snapshot all through the single `ResolvedValue::display()` path, masking a secret there masks all three at once (a secret leaf renders `secret:****`, never the cleartext). The one deliberate exception is **delivery**: the native-mapping application places the resolved cleartext (via `expose_secret()`) into the adapter's native env/flag/file, because the agent needs a usable key. So the same leaf shows a mask in `config get`/the snapshot/logs while the agent's private native config holds the real value. That rendered native config file inside the Agent Home holds cleartext **by necessity** — an accepted boundary: the Agent Home is process/filesystem-isolated (FR-2), **not** a security sandbox (NFR-6); the file is the agent's own secret to hold, exactly like the 0600 secrets file. A secret delivered to an **env** target is likewise cleartext in the child process's environment (an accepted, home-scoped boundary). One delivery target is **stricter** and called out explicitly: a secret mapped to a command-line **`flag`** target is passed as an argv token, and argv is world-readable **cross-user** on the host process list (`ps`, `/proc/<pid>/cmdline`) — a wider exposure than the filesystem-isolated env/file boundaries above. This too is accepted (the agent needs a usable key and Ktesio's own surfaces stay masked), but operators should **prefer `env`/`file` targets for secret-carrying keys** and treat a secret-in-`flag` mapping as visible to any local user. Ktesio's guarantee is that a secret never leaks through **Ktesio's** own logs, event payloads, `--json`, or the snapshot — proven by a no-leak test matrix (see `docs/testing.md`). Event payloads (`TransitionCause` details) name the binary/kind/exit-status, never env values, so a crash/launch-error diagnostic cannot carry a resolved secret.

**`--reveal` is the sole un-mask.** `kt agent config get <name> [<key>] --json` (and the human table) masks secret values by default; `--reveal` is the only explicit acknowledgment that emits the unmasked value in machine-readable output. It re-resolves the secret **live** through the engine (the CLI never resolves secrets itself, env → the secrets file, at read time) and un-masks both `--json` and the table symmetrically; a resolution failure under `--reveal` is a stderr diagnostic, not a crash. Because the resolution is live at read time, a revealed value may **differ** from what a currently-running instance resolved at its start (that run captured the value that was live then; `--reveal` shows what would resolve now). `--reveal` affects only the on-demand read surface — it never un-masks the persisted snapshot, the logs, or event payloads (those are always masked, no flag touches them). No new interactive prompt: `--reveal` is a flag.

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
