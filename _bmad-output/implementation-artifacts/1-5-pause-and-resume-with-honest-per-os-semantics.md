---
github_issue: 67
baseline_commit: db17af834ea590c60dbaf86eefd15dab08718279
---

# Story 1.5: Pause and resume with honest, per-OS semantics

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an Operator,
I want pause/resume that tells the truth about what it can guarantee for this agent on this OS,
so that I never mistake a best-effort pause for a guaranteed one. (FR-7)

## Acceptance Criteria

Verbatim from `epics.md` Story 1.5 (the HONESTY story). Each maps to the three-level dispatch on the agent's EFFECTIVE (current-OS) Capability Declaration for `Capability::Pause`.

1. **Guaranteed (Unix signal).** **Given** an adapter declaring `pause: guaranteed-via-signal` on Unix — i.e. `Capability::Pause` projects to `SupportLevel::Guaranteed` on the current (Unix) `OsId` — **when** I pause and resume the running instance **then** SIGSTOP/SIGCONT are used (delivered to the whole process group), states transition `running→paused→running`, and the Usage Ledger continues from where it left off (ledger untouched by pause — see Dev Notes; there is no metering yet in Epic 1, so "continues from where it left off" = pause/resume do not reset or mutate any usage state).

2. **Best-effort (cooperative; the Windows default per AD-4).** **Given** an adapter declaring `pause: best-effort` (`Capability::Pause` → `SupportLevel::BestEffort` on the current OS) **when** I pause **then** the pause proceeds cooperatively and a **visible qualifier** is emitted in CLI text (stdout result + a stderr note) AND in the transition-event payload (a dedicated best-effort cause/field) — **never** a silent success.

3. **Unsupported (fail fast).** **Given** an adapter declaring `pause: unsupported` (`Capability::Pause` → `SupportLevel::Unsupported` on the current OS, including the honest default when pause is simply not declared for this OS) **when** I pause **then** the command **fails fast quoting the Capability Declaration** and attempts **no** fake pause — no state change, no process signal.

### Derived / consequence criteria (make explicit; enforced by tests)

4. **Transition-table additions (AD-15).** `Pause` and `Resume` commands and the `paused` state join the pure transition table: `running --Pause--> paused`, `paused --Resume--> running`. Every invalid pair (e.g. `Pause` on `stopped`/`registered`/`paused`/`starting`/`stopping`/`failed`; `Resume` on `running`/anything-not-`paused`) returns the ONE uniform `LifecycleError::InvalidTransition { from, command }` — the SAME class story 1-4 built, identical for every adapter (the rejection comes from the shared table before any adapter/backend code runs). `Stop` from `paused` → `stopping` is ALSO wired (the spine state diagram has `paused --> stopping`; a paused instance must be stoppable). The table stays pure + exhaustively unit-tested.

5. **Effective declaration is READ, not re-derived.** The pause level is obtained from the persisted `AdapterSnapshot`'s FULL per-OS declaration projected onto `OsId::current()` at read time (the 1-3 F3 mechanism), via a new supervisor/registry read of the pause `SupportLevel`. Do NOT re-parse the manifest or re-resolve the adapter to decide the level; do NOT freeze a level at register time.

6. **CLI surface.** `kt agent pause <name>` and `kt agent resume <name>` exist, drive the engine `blocking()` facade, and honor the output discipline (AD-12: result → stdout, diagnostics/qualifier notes → stderr). On `unsupported`, fail fast with a miette diagnostic quoting the declaration (to stderr, non-zero exit); on `best-effort`, print the result AND the visible qualifier.

7. **Conformance fixtures prove all three levels.** The `fake_agent` becomes **observably suspendable** (prints a periodic heartbeat; paused = heartbeat stops, resume = heartbeat resumes). Integration tests prove: (a) a declaration with pause `guaranteed` on the test OS (Unix) → real SIGSTOP suspension is provable (heartbeat stops while paused, resumes after SIGCONT, states `running→paused→running`); (b) a declaration with pause `best-effort` → the qualifier is surfaced (CLI + event); (c) a declaration with pause `unsupported` → fail-fast, no state change.

8. **Windows honesty stated.** The Windows best-effort/cooperative pause path is BEHAVIOR-verified only on the `windows-latest` CI matrix (as 1-4). Locally on Unix hosts it is compile-checked only. No undocumented Windows suspend API (no `NtSuspendProcess`) — Windows pause is adapter-cooperative only (AD-4).

## Tasks / Subtasks

> Dev: track progress by checking these boxes IN THIS FILE (do not use external task tools). Order is dependency-first: pure table → port → backends → supervisor dispatch → engine facade → CLI → fixtures → tests → gates/docs.

- [x] **Task 1 — Transition table: add `Pause`/`Resume` + `paused` edges (AC4).** (`crates/ktesio-engine/src/domain/transition.rs`)
  - [x] Add `Pause` and `Resume` to `LifecycleCommand` (uncomment/replace the two future-row stubs at lines 54-55); add their `as_str()` arms (`"pause"`, `"resume"`).
  - [x] Add rows to `next_state`: `(Running, Pause) => Ok(Paused)`, `(Paused, Resume) => Ok(Running)`, `(Paused, Stop) => Ok(Stopping)` (spine diagram `paused --> stopping`). All other pairs keep falling through to the uniform `InvalidTransition`.
  - [x] Update the existing exhaustive tests: `all_commands` array must now include `Pause`, `Resume`; the `exhaustive_over_every_state_command_pair` expected-match must add the three new Ok rows; extend `invalid_command_pairs_all_yield_the_same_error_class` with new invalid pairs (`Pause` on `stopped`/`registered`/`paused`, `Resume` on `running`/`stopped`). Add `command_labels_are_stable` arms for pause/resume.
  - [x] Keep the module PURE (no I/O, no adapter, no OS). Confirm the doc comment's "Reachable this story" / "NOT wired" notes are updated to reflect 1-5 wiring `paused`.
- [x] **Task 2 — `ProcessBackend` port: add a pause/resume (signal) method (AC1/AC2).** (`crates/ktesio-engine/src/ports/process_backend.rs`)
  - [x] Add two trait methods (RECOMMENDED shape): `fn pause(&self, handle: &mut Self::Handle) -> Result<(), BackendError>;` and `fn resume(&self, handle: &mut Self::Handle) -> Result<(), BackendError>;`. Sync. Domain terms only.
  - [x] Documented as GUARANTEED-path primitives; dispatch is the supervisor's job; Windows body is cooperative best-effort.
  - [x] `BackendError::Control` op-label note extended for `"pause"`/`"resume"`/`"signal"`.
- [x] **Task 3 — Unix backend: SIGSTOP/SIGCONT to the process group (AC1).** (`crates/ktesio-engine/src/backends/unix/mod.rs`)
  - [x] Implemented `pause`/`resume` reusing `signal_group(handle.pgid, Signal::SIGSTOP/SIGCONT)`. No new dependency.
  - [x] `reap_if_exited` guard before signalling; DECISION: pause/resume on a dead process is a harmless `Ok(())` no-op (documented — SIGSTOP to a gone group is ESRCH→Ok anyway).
  - [x] Unit tests: `pause_freezes_the_process_then_resume_wakes_it` (cross-Unix-safe heartbeat-count assertion — count stable while paused, grows after resume) + `pause_and_resume_on_an_already_exited_process_are_harmless_no_ops`. Both PASS on macOS.
- [x] **Task 4 — Windows backend: cooperative best-effort pause/resume (AC2/AC8).** (`crates/ktesio-engine/src/backends/windows/mod.rs`)
  - [x] Implemented `pause`/`resume` as the cooperative best-effort body (succeed without a hard suspension, reap-guard for parity). NO `NtSuspendProcess`/undocumented API.
  - [x] Module docs state the honesty rides on the supervisor/CLI qualifier, and the path is BEHAVIOR-verified only on `windows-latest` CI (compile-checked on Unix).
- [x] **Task 5 — Supervisor: the three-level dispatch (AC1/AC2/AC3/AC5).** (`crates/ktesio-engine/src/domain/supervisor.rs`)
  - [x] Added `pause`/`resume` delegating to a shared `suspend_or_resume(registry, name, command)` driver: name → `InstanceName`; `lookup`; transition gate `next_state(state, Pause/Resume)?` (AC4 rejects first); read effective pause `SupportLevel` (Task 6 helper); DISPATCH on the level.
  - [x] **Guaranteed:** `signal_backend` → `self.backend.pause/resume(handle)` (via `self.running.get_mut`), then transition + plain `TransitionCause::command`.
  - [x] **Best-effort:** transition + `TransitionCause::pause_best_effort`/`resume_best_effort` qualifier (Task 7). CLI learns best-effort via the RECOMMENDED option (re-reads `effective_capabilities` — documented in Decisions).
  - [x] **Unsupported:** `EngineError::CapabilityUnsupported { name, capability, os, level }` returned BEFORE any transition/backend/persist — fail fast.
  - [x] `resume` dispatches on the same level for symmetry (defensive on unsupported).
  - [x] No-in-memory-handle case: `signal_backend` treats a missing handle as a documented best-effort no-op (cannot signal a process not in this engine's custody; single-lifetime boundary, 1-6).
- [x] **Task 6 — Registry: read the effective pause level (AC5).** (`crates/ktesio-engine/src/domain/registry.rs`)
  - [x] Added `pub(crate) fn effective_support(&self, name, capability) -> Result<SupportLevel, RegistryError>` reading `read_adapter_snapshot` + projecting onto `OsId::current()` (F3). Two unit tests (read-time projection for current OS; unsupported default when absent).
  - [x] `Capability`/`SupportLevel` imported into registry.rs.
- [x] **Task 7 — Event: best-effort qualifier in the transition payload (AC2).** (`crates/ktesio-engine/src/domain/event.rs`)
  - [x] Added `TransitionCause::PauseBestEffort { detail }` (tag `pause-best-effort`) + `ResumeBestEffort { detail }` (tag `resume-best-effort`) + constructors + extended the stable-tag guard test + a round-trip test. Additive-vs-breaking note added to `EVENT_SCHEMA_VERSION`.
  - [x] Guaranteed pause emits a plain `TransitionCause::command("pause")`; only best-effort carries the qualifier.
- [x] **Task 8 — Engine error + facade + CLI (AC3/AC6).**
  - [x] `error.rs`: `EngineError::CapabilityUnsupported { name, capability, os, level }` with a declaration-quoting `#[error(...)]` (thiserror only).
  - [x] `engine.rs`: async `pause`/`resume` + `Blocking` facade methods (lock registry+supervisor, `spawn_blocking`).
  - [x] `kt/src/error.rs`: `AgentCapabilityUnsupported { message }` (`code(ktesio::agent::capability_unsupported)`).
  - [x] `kt/src/cli/agent.rs`: `pause`/`resume` handlers; `map_engine_error` `CapabilityUnsupported` arm quoting the declaration + `kt agent show` hint; best-effort detection via `note_if_best_effort` (stderr note via `ui::note`).
  - [x] `kt/src/main.rs`: `agent pause`/`resume` clap subcommands + dispatch + help text + parse tests.
- [x] **Task 9 — `fake_agent`: make it observably suspendable (AC7).** (`crates/ktesio-conformance/src/bin/fake_agent.rs`)
  - [x] Added `--heartbeat-ms <ms>` (default off): prints incrementing `heartbeat <n>` to stdout + flush each interval; SIGSTOP freezes the log, SIGCONT resumes it. Pure `std`, NO OS-cfg.
  - [x] Every existing arg/behavior (`--exit-fast`/`--spawn-child`/`--linger-ms`/`--marker`) preserved.
- [x] **Task 10 — Conformance mock declaration (AC7).** (`crates/ktesio-conformance/src/lib.rs`)
  - [x] `MockAdapter` reused as-is (pause Guaranteed on Linux/macOS, BestEffort on Windows). Best-effort/unsupported proofs use manifests. `mock_lifecycle_ops_are_inert_until_1_4` stays valid (trait bodies unchanged — the supervisor+backend drive real suspension, not the trait method).
- [x] **Task 11 — Integration tests: prove all three levels (AC1/AC2/AC3/AC7).** (new `crates/ktesio-engine/tests/pause.rs`, 9 tests, all PASS)
  - [x] **Guaranteed (Unix, runtime-gated):** `guaranteed_pause_really_suspends_then_resume_wakes_it_unix` — heartbeat STOPS while paused, RESUMES after SIGCONT, states running→paused→running, plain command causes. Windows-skipped via `if OsId::current() == OsId::Windows { return; }` (no cfg).
  - [x] **Best-effort:** `best_effort_pause_transitions_and_surfaces_the_qualifier_in_the_event` — state→paused AND the event carries the `pause-best-effort` (and `resume-best-effort`) cause tag.
  - [x] **Unsupported:** `unsupported_pause_fails_fast_with_no_state_change_and_no_event` — fails with `CapabilityUnsupported`, state UNCHANGED (running), NO event appended.
  - [x] Guaranteed transition-event sequence asserted (registered→starting→running→paused→running→stopping→stopped). Plus AC4 tests (pause-on-registered, resume-on-running, stop-from-paused) + not-found/invalid-name.
- [x] **Task 12 — CLI integration tests (AC6).** (`crates/kt/tests/agent_cli.rs`, 6 new tests, all PASS)
  - [x] `pause_prints_paused_state_and_exits_zero_guaranteed_unix` (stdout=`paused`, no best-effort note); `pause_best_effort_prints_qualifier_note_to_stderr_only` (LOW-1 stdout/stderr discipline: state on stdout, `best-effort` note on stderr, NOT stdout); `pause_unsupported_exits_nonzero_quoting_the_declaration` (non-zero, stderr quotes `unsupported`+OS+`kt agent show`, state unchanged); `pause_on_registered_returns_uniform_invalid_transition`.
- [x] **Task 13 — Docs + gates (NFR-7/NFR-3/NFR-2).**
  - [x] Updated `docs/architecture.md` lifecycle section: the `paused` state + `Pause`/`Resume`/`Stop-from-paused` edges, the three-level pause dispatch, SIGSTOP/SIGCONT guaranteed path, the best-effort qualifier mechanism (event cause + CLI stderr note), the unsupported fail-fast, and the Windows-best-effort-is-CI-verified honesty.
  - [x] Updated `docs/testing.md`: `fake_agent --heartbeat-ms` suspension proof; Unix leg adds SIGSTOP/SIGCONT; Windows leg carries the cooperative best-effort pause + qualifier.
  - [x] ALL 9 gates run locally with `cargo +1.96.1` — ALL GREEN (see Gate results in Dev Agent Record): fmt, clippy (0 warnings), test (610 pass), tarpaulin (95.58%), check_docs (23 files), test_automation (20 tests), MSRV check, OS-cfg grep (no new cfg outside backends/), boundary.
  - [x] Boundary gate green: `kt` normal+build tree is only `ktesio-engine`/`ktesio-adapter-api`; `ktesio-conformance` absent from the engine normal tree (dev-dep only); no new runtime dependency.

## Dev Notes

**This story is the HONESTY story (FR-7). Its product value is "surfaced not silent": pause tells the truth about what it can guarantee for THIS agent on THIS OS.** The whole of Epic 1's supervision machinery already exists (story 1-4, commit `db17af8`); pause/resume is a focused extension that reuses it. Do NOT rebuild the transition table, the supervisor, the backends, or the CLI plumbing — extend them along the seams below. Everything is on `main` behind you; read the cited files, do not reinvent.

### The binding architecture decisions (spine, FINAL)

- **AD-4 (per-OS process control; capabilities are capability × OS).** Pause via SIGSTOP/SIGCONT is used ONLY when the adapter declares pause guaranteed on the current OS. Windows pause is **adapter-cooperative only** — Windows has no clean guaranteed whole-process suspend from `std`; the honest declaration there is best-effort or unsupported. **DO NOT reach for undocumented `NtSuspendProcess`.** The per-OS Capability Declaration is the source of truth. [Source: ARCHITECTURE-SPINE.md#AD-4]
  - **Projection clause (F3, ratified 2026-07-04):** the persisted snapshot stores the FULL per-OS declaration; the effective (current-OS) view is projected onto `OsId::current()` at READ time, not frozen at register time. This story READS that projection to pick the pause level. [Source: ARCHITECTURE-SPINE.md#AD-4 projection clause; registry.rs `effective_capabilities`]
- **AD-15 (state machine as data).** This story adds the `paused` state + `Pause`/`Resume` commands to the transition table story 1-4 built. Keep the table a pure, exhaustively-tested total function. The spine state diagram already shows `running --> paused` (pause/breach), `paused --> running` (resume), and `paused --> stopping` (stop) — wire all three. [Source: ARCHITECTURE-SPINE.md#AD-15 + state diagram]
- **AD-1 hexagonal / AD-2 boundary.** Domain logic (the dispatch decision) lives in the engine core; OS syscalls (SIGSTOP/SIGCONT) live ONLY in `backends/unix`. `kt` uses only the public `Engine`/`Blocking` facade + re-exported types. No new internal crate edge.
- **AD-12 / AD-14 seeds.** Record pause/resume transitions as `TransitionEvent`s appended to the per-instance log (as 1-4 does). The best-effort qualifier rides IN the event payload (a `TransitionCause`), the same "one event schema, two consumers" struct 7-2/`--json` will reuse. stdout = command result; stderr = diagnostics + the best-effort qualifier note. [Source: ARCHITECTURE-SPINE.md#AD-12, #AD-14; supervisor.rs `transition`]
- **FR-7 (PRD).** Pause/resume with honest per-Adapter AND per-OS semantics: guaranteed / best-effort / unsupported, **surfaced not silent**. Glossary terms exact: Capability Declaration, Lifecycle State, Agent Instance, Adapter. [Source: epics.md FR-7]

### THE honest-semantics design (the story's core value — define exactly)

The pause LEVEL is `SupportLevel` for `Capability::Pause` on `OsId::current()`, read from the persisted snapshot. The three levels dispatch as follows. **Nail these mechanisms — they are the deliverable.**

| Level (`SupportLevel`) | Mechanism | State change | Qualifier / failure surface |
| --- | --- | --- | --- |
| `Guaranteed` (Unix signal) | `ProcessBackend::pause` → `signal_group(pgid, SIGSTOP)`; resume → `SIGCONT`. Real, verifiable suspension (SIGSTOP is uncatchable). | `running→paused` / `paused→running` | Plain `TransitionCause::command("pause")` — NO qualifier (it is a true suspension). CLI prints the new state to stdout. |
| `BestEffort` (cooperative; Windows default) | Cooperative — the pause "proceeds" (transitions state) but does NOT hard-suspend. On Windows the backend body is effectively a succeed-without-suspension. | `running→paused` / `paused→running` | **VISIBLE QUALIFIER, never silent:** (a) CLI: a stderr note like `note: pause for '<name>' is best-effort on <os> (adapter-cooperative); the process may keep running.`; (b) event: a dedicated `TransitionCause` variant (recommended `PauseBestEffort { detail }`, wire tag `pause-best-effort`) so log/`--json`/7-2 consumers can match on it. |
| `Unsupported` | FAIL FAST. No backend call, no state change, no event. | none | `EngineError::CapabilityUnsupported { name, capability, os, level }` (thiserror) → `kt` renders `AgentCapabilityUnsupported` (miette, code `ktesio::agent::capability_unsupported`) whose message QUOTES the declaration (the level + OS) and points at `kt agent show <name>`. Non-zero exit, stderr. |

**Two precise design decisions the dev must make and record (both have a conservative recommendation):**

1. **How does the CLI learn "this pause was best-effort" so it can print the qualifier?** The engine returns `AgentInstance` (state) from `pause`, not the cause. Two clean options:
   - **(RECOMMENDED, simplest, no signature change):** the CLI, after a successful `pause`, calls the existing `engine.blocking().effective_capabilities(name)` (already the mechanism `kt agent show` uses) and inspects the pause `SupportLevel`; if `BestEffort`, print the qualifier note. This keeps `pause` returning a plain `AgentInstance` like `stop`, reuses a proven read, and the level is authoritative from the same snapshot. Cost: one extra cheap read.
   - **(Alternative):** change `Engine::pause` to return a small struct/enum carrying the level (or the emitted `TransitionEvent`). More precise but a wider signature change. Only do this if the extra read feels wrong.
   - Either way, the EVENT payload must carry the best-effort qualifier (that is non-negotiable — it is the machine-readable half of "surfaced not silent"). The CLI-side detection is the human half.
2. **Port method shape.** RECOMMENDED: two methods `pause(&mut Handle)` / `resume(&mut Handle)` (clearest, matches `stop`'s single-purpose style). Alternative: one `signal(&mut Handle, kind)` with a domain `SignalKind::{Stop, Continue}` enum — more general but introduces a new port enum. Prefer the two-method shape; the Windows body is a cooperative succeed-without-suspend (documented), the Unix body sends the real signals.

**Why the `AgentAdapter::pause`/`resume` trait methods stay INERT (do not wire real suspension into them):** the real suspension is a PROCESS operation (SIGSTOP to the OS process group), which belongs to the `ProcessBackend`, not the adapter. The `AgentAdapter` trait's pause/resume default bodies (in `adapter.rs`, currently "unavailable until 1-4/1-5") are for FUTURE native adapters that implement a bespoke pause (e.g. an in-band "please pause" message); the engine's supervisor drives real OS suspension through the backend regardless of adapter kind, using the DECLARED level. So: leave `AgentAdapter::pause`/`resume` inert (or lightly touch their doc comments); the mock's `mock.pause().is_err()` test stays valid. The dispatch lives in the SUPERVISOR + BACKEND, keyed on the declared `SupportLevel`. State this explicitly so the dev does not wire suspension into the wrong layer.

### Exact code seams (READ these; extend, do not reinvent)

Every path is absolute. Line numbers are as of commit `db17af8`; if drifted, the identifiers are stable.

**1. Transition table — `Pause`/`Resume` are already stubbed as future rows.** `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/transition.rs`
- `LifecycleCommand` (lines 48-56): `Start`, `Stop`, and commented `// Pause,` / `// Resume,` at lines 54-55 labeled "story 1-5 — not wired here." Uncomment/add them. `as_str()` (lines 60-65) needs `Pause => "pause"`, `Resume => "resume"`.
- `LifecycleError::InvalidTransition { from, command }` (lines 81-93) — THE uniform error class. Reuse verbatim; adding commands automatically routes invalid pairs to it via the catch-all `(from, command) => Err(...)` at line 118.
- `next_state` (lines 101-120): add `(Running, Pause) => Ok(Paused)`, `(Paused, Resume) => Ok(Running)`, `(Paused, Stop) => Ok(Stopping)`. The `Paused` state variant ALREADY EXISTS in `LifecycleState` (see below); the tests already reference `(Paused, Start)`/`(Paused, Stop)` as invalid — update those (`(Paused, Stop)` becomes VALID → `Stopping`).
- Tests (lines 122-207): update `all_commands` (line 178) to `[Start, Stop, Pause, Resume]`; update the `expected` match in `exhaustive_over_every_state_command_pair` (lines 182-187); adjust `invalid_command_pairs_all_yield_the_same_error_class` (remove `(Paused, Stop)` from invalid — it is now valid — add e.g. `(Stopped, Pause)`, `(Registered, Pause)`, `(Paused, Pause)`, `(Running, Resume)`, `(Stopped, Resume)`); extend `command_labels_are_stable`.

**2. Lifecycle state — `Paused` already exists as data.** `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/lifecycle.rs`
- `LifecycleState::Paused` (line 34) with wire form `"paused"` (`as_str` line 54, `from_wire` line 68) — ALREADY present. No new state to add; it becomes reachable this story. `serde(rename_all = "snake_case")` gives `"paused"` in the DB `state` column + events. `is_removable_without_force` (line 84) already treats `Paused` as removable-without-force (only `Running` requires force) — that is correct and needs no change (a paused instance can be removed without `--force`; note this is a deliberate 1-2 decision).

**3. Capability model — the level source of truth. NO "guaranteed-via-signal" variant exists.** `/Users/imagdy/dev/ktesio/crates/ktesio-adapter-api/src/capability.rs`
- `SupportLevel` enum (lines 27-36): `Guaranteed`, `BestEffort`, `Unsupported` (serde kebab-case: `guaranteed`, `best-effort`, `unsupported`). **CRITICAL:** the epic AC text says `pause: guaranteed-via-signal`, but there is NO such Rust variant and NO such TOML spelling. The spine's `guaranteed-via-signal` phrase maps to `SupportLevel::Guaranteed` for `Capability::Pause` on a Unix `OsId` — the "via signal" part is the ENGINE's chosen MECHANISM for `Guaranteed` on Unix (SIGSTOP), not a distinct declared level. Adapters declare `pause: guaranteed` (the manifest spelling) or `best-effort` / `unsupported`, keyed per OS. **[ASSUMPTION — flagged, see Open Questions]** treat `guaranteed` on Unix as "use the signal mechanism".
- `Capability::Pause` (line 67, wire `pause`). Accessor: `CapabilityDeclaration::support(capability, os) -> SupportLevel` (lines 175-180) returns the declared level, defaulting to `Unsupported` when absent (the honest default — so a manifest that omits pause for the current OS correctly fails fast). `effective(os) -> EffectiveCapabilities` (lines 186-193) is the full projection `kt agent show` renders.
- `OsId::current()` (`/Users/imagdy/dev/ktesio/crates/ktesio-adapter-api/src/os.rs` line 59) reads `std::env::consts::OS` at RUNTIME (NOT a compile-time cfg — the per-OS-as-data rule). Variants `Linux`/`Macos`/`Windows`/`Other`. This is the exact API the story reads the level against.

**4. The persisted snapshot + read-time projection (F3) — READ, do not re-derive.** `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/registry.rs`
- `AdapterSnapshot` (lines 54-64): stores `kind`, `metering_source`, `manifest_path`, and the FULL `declaration: CapabilityDeclaration`. Written into `<home>/adapter.json` at registration (`materialize_home`, lines 274-320).
- `read_adapter_snapshot(&self, name) -> Result<AdapterSnapshot, RegistryError>` (lines 246-258) — the existing reader (handles missing/corrupt → `RegistryError::Io`).
- `effective_capabilities(name)` (lines 221-240) already does `snapshot.declaration.effective(OsId::current())`. **Task 6 adds a sibling** `effective_support(name, Capability::Pause) -> SupportLevel` that does `snapshot.declaration.support(Capability::Pause, OsId::current())`. This is THE read the supervisor uses to pick the level. `adapter_launch_facts` (lines 424-431) shows the same `read_adapter_snapshot` pattern the supervisor already calls from `start`.

**5. `ProcessBackend` port + the Unix signal helper (the guaranteed primitive already exists).** 
- Port: `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/ports/process_backend.rs` — trait `ProcessBackend` (lines 131-164): `type Handle: Send`, `spawn`, `stop`, `poll`, `pid`. Add `pause`/`resume`. `BackendError` (lines 91-124) has `Control { op: &'static str, detail }` for signal failures. Sync methods, called via `spawn_blocking` (documented at lines 12-17).
- Unix: `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/backends/unix/mod.rs` — `UnixProcess { child, pgid: Pid, pid }` (lines 40-47). `signal_group(pgid, signal) -> Result<(), BackendError>` (lines 229-238) ALREADY sends any `Signal` to the group via `killpg`, treating `ESRCH` (group gone) as success. `use nix::sys::signal::{killpg, Signal}` (line 21) — `Signal::SIGSTOP` and `Signal::SIGCONT` are available with the `signal` feature (already enabled). **`pause` = `signal_group(handle.pgid, Signal::SIGSTOP)`; `resume` = `signal_group(handle.pgid, Signal::SIGCONT)`.** `reap_if_exited` (lines 211-222) is the liveness guard `stop` uses — mirror it (a dead process cannot be paused; decide: `Ok(())` no-op vs `BackendError::Control` — recommend documenting a no-op-on-dead as harmless, since SIGSTOP to a gone group is `ESRCH`→Ok anyway).
- Windows: `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/backends/windows/mod.rs` — `WindowsProcess { child, job: HANDLE, pid }` (lines 69-76). No signal API. `pause`/`resume` = cooperative best-effort body (recommend: `Ok(())` succeed-without-suspend; the qualifier carries honesty). Document in the module docs next to the existing `[ASSUMPTION]` blocks (lines 16-38). Behavior verified only on `windows-latest` CI.
- Backend selection: `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/backends/mod.rs` — `Backend`/`Handle` cfg-selected aliases (lines 29-40). The supervisor names only these; adding trait methods needs no change here.

**6. Supervisor — `stop` is the exact template for `pause`/`resume`.** `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/supervisor.rs`
- `Supervisor { backend: backends::Backend, running: HashMap<InstanceName, backends::Handle> }` (lines 67-70). `self.running.get_mut(&name)` is how `stop` reaches the handle (lines 209-219).
- `stop` (lines 176-235): the exact shape to copy — name→InstanceName, `registry.lookup`, `next_state(state, Stop)?` gate, transition to `stopping`, `backend.stop(handle, window)`, transition to `stopped` with the right cause. **`pause`/`resume` follow this to the letter**, except the middle step DISPATCHES on the level (Task 5) instead of always calling the backend.
- `transition(&self, registry, name, prior, new, cause)` (lines 261-282): persist state (`registry.set_state`) then append the event (`append_event` → `<home>/logs/instance.log`, JSON Lines). Reuse verbatim for the pause/resume transitions.
- `read_events` (lines 240-254) + the crate-root `Engine::transition_events` are how tests assert the recorded transitions + causes.

**7. Event — add the best-effort qualifier cause.** `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/event.rs`
- `TransitionCause` (lines 33-57): `#[serde(tag = "kind", rename_all = "kebab-case")]` closed vocabulary — `Command { command }`, `AdapterReady`, `LaunchError { detail }`, `StopGraceful`, `StopForced { detail }`. **Add `PauseBestEffort { detail: String }`** (wire tag `pause-best-effort`) — and, for symmetry, resume's best-effort can reuse it or add `ResumeBestEffort`. Add a constructor (mirror `stop_forced`, lines 75-79) + extend the `cause_variants_serialize_with_stable_tags` guard test (lines 173-187). `TransitionEvent` (lines 88-102) carries `schema_version`, `instance`, `prior_state`, `new_state`, `cause`, `at`. No schema bump needed (additive enum variant — but note additive-vs-breaking for 7-2 in a comment).

**8. Engine error + facade.** 
- `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/domain/error.rs` — `EngineError` (lines 164-233) is the lifecycle surface (thiserror). Existing variants: `NotFound`, `InvalidName`, `InvalidTransition(#[from] LifecycleError)`, `LaunchFailed`, `AdapterUnresolved`, `Log`, `Backend`, `Store`. **Add `CapabilityUnsupported { name, capability, os, level }`** with `#[error("Agent Instance '{name}' cannot pause: this adapter declares pause '{level}' on {os} (see its Capability Declaration)")]` (quote the declaration, AC3). There is NO existing "unsupported capability" variant (confirmed — grep found none).
- `/Users/imagdy/dev/ktesio/crates/ktesio-engine/src/engine.rs` — async `stop` (lines 219-232) + `Blocking::stop` (lines 323-325) are the template. Add `pause`/`resume` (async on `Engine`, sync on `Blocking`), locking `registry` + `supervisor`, calling `supervisor.pause/resume(&registry, &name)` inside `run_blocking`. `EngineError` is already re-exported at the crate root (`lib.rs` line 67).

**9. CLI — `stop` handler + `map_engine_error` are the template.** `/Users/imagdy/dev/ktesio/crates/kt/src/cli/agent.rs`
- `stop` handler (lines 295-310): opens engine, `engine.blocking().stop(...)`, prints `instance.state` to stdout on success, `map_engine_error(err)` to stderr on failure. Copy for `pause`/`resume`. For BEST-EFFORT, after the successful `pause`, read `engine.blocking().effective_capabilities(name)` (the `show` handler at lines 153-161 shows the exact call) and if pause is `BestEffort`, emit a stderr note via `ui::note`/`ui::warning`.
- `map_engine_error` (lines 434-487): the arm-per-variant translator. **Add a `CapabilityUnsupported` arm** producing `AgentCapabilityUnsupported` whose message quotes the level+OS and points at `kt agent show <name>`. Note it currently has NO arm for the new variant — a non-exhaustive match will fail to compile, which is the right forcing function.
- `render_capabilities` (lines 167-188) + `EffectiveCapabilities` (already re-exported) are available if a `pause`/`resume` handler wants to show the level.
- Miette diagnostics: `/Users/imagdy/dev/ktesio/crates/kt/src/error.rs` — add `AgentCapabilityUnsupported { message }` with `#[diagnostic(code(ktesio::agent::capability_unsupported))]` (mirror `AgentInvalidTransition` lines 221-226). All agent diagnostics are `#[error("{}", message)]` message-carriers.
- Clap wiring: `/Users/imagdy/dev/ktesio/crates/kt/src/main.rs` — add `agent pause <name>` / `agent resume <name>` subcommands + dispatch + help + parse tests (mirror `start`/`stop`; 1-4 added those there).

**10. `fake_agent` — the observably-suspendable upgrade.** `/Users/imagdy/dev/ktesio/crates/ktesio-conformance/src/bin/fake_agent.rs`
- Current: `main` (lines 93-142) parses args, optionally spawns a child, prints `fake_agent ready pid=<n>`, then loop-sleeps 25ms until `--linger-ms` elapses. Pure `std`, `#[cfg(not(tarpaulin_include))]`, NO OS-cfg.
- Add `--heartbeat-ms <ms>` to `Opts` (lines 45-50) + `parse` (lines 53-91). In the loop (lines 136-139), if a heartbeat interval is set, print `heartbeat <n>` (incrementing) + flush on each interval tick. When SIGSTOP freezes the process, the heartbeat file stops growing; SIGCONT resumes it — the observable suspension proof. Keep ALL existing behaviors intact (1-4 tests depend on `--exit-fast`/`--spawn-child`/`--linger-ms`/`--marker`). Recommend heartbeat to stdout (captured to `<home>/logs/agent.log`), so a test reads the log's line count. `fake_agent_bin()` (`lib.rs` lines 193-218) resolves/builds the binary — unchanged.

**11. Conformance mock declaration — already correct for the guaranteed proof.** `/Users/imagdy/dev/ktesio/crates/ktesio-conformance/src/lib.rs`
- `MockAdapter::new` (lines 52-72) declares pause `Guaranteed` on Linux/macOS, `BestEffort` on Windows, interaction `Guaranteed` everywhere. Reuse for the guaranteed (Unix) proof. For best-effort and unsupported PROOFS, tests write a MANIFEST with the desired per-OS pause level (the manifest is the injection mechanism — see the lifecycle test harness `write_fake_manifest`). `mock_lifecycle_ops_are_inert_until_1_4` (lines 279-288) asserts `mock.pause().is_err()` — this STAYS valid (the trait method stays inert; the engine drives real suspension through the backend, not the trait). Only touch this test if you change the trait bodies (you should not).

## Dev Agent Record

### Context Reference

- Implemented by the BMAD developer agent (Amelia) in Away Mode; owner Islam away.
- Toolchain: MSRV 1.96.1; ALL cargo gate commands run with `cargo +1.96.1` (the local default `stable` is 1.94.1, below MSRV — `libsqlite3-sys 0.38.1` needs `cfg_select!` stabilized in 1.96.1). Host: macOS (aarch64-apple-darwin) — the Unix backend + guaranteed-pause SIGSTOP path is behavior-verified here; the Windows best-effort path is compile-checked only locally and rides the `windows-latest` CI leg.

### Decisions & Assumptions (recorded per Away-Mode instruction)

1. **CLI best-effort detection — took the RECOMMENDED option (no `Engine::pause` signature change).** After a successful `pause`/`resume`, the CLI re-reads `engine.blocking().effective_capabilities(name)` and, if pause is `BestEffort` on the current OS, emits the stderr qualifier note (`note_if_best_effort` in `crates/kt/src/cli/agent.rs`). `Engine::pause` returns a plain `AgentInstance` like `stop`. The machine-readable half is non-negotiable and IS present: the event payload carries a dedicated `pause-best-effort` / `resume-best-effort` `TransitionCause` (emitted by the supervisor, independent of the CLI read).
2. **Port method shape — took the RECOMMENDED two-method shape** `pause(&mut Handle)` / `resume(&mut Handle)` on `ProcessBackend` (not a single `signal(kind)` with a new enum). Clearest, matches `stop`'s single-purpose style.
3. **`guaranteed` = signal mechanism — proceeded on the flagged ASSUMPTION.** There is NO `guaranteed-via-signal` TOML/Rust variant; adapters declare `pause: guaranteed`. `Guaranteed` for `Capability::Pause` on a Unix `OsId` means the engine uses SIGSTOP/SIGCONT (the Unix backend's `pause`/`resume`). Recorded and unchanged.
4. **Pause/resume on a dead or unheld process = harmless no-op (documented).** The Unix backend `reap_if_exited`-guards and returns `Ok(())` on an already-exited process (SIGSTOP to a gone group is `ESRCH`→Ok anyway). The supervisor's `signal_backend` treats a missing in-memory handle (cross-lifetime: row says running but this engine holds no handle — 1-6 territory) as a best-effort no-op while the state transition still proceeds. Proven by `guaranteed_pause_without_an_in_memory_handle_is_a_no_op_transition` and `pause_and_resume_on_an_already_exited_process_are_harmless_no_ops`.
5. **`TransitionCause` — added BOTH `PauseBestEffort` and `ResumeBestEffort` variants** (closed matchable vocabulary, parity with `StopForced`/`StopGraceful`), rather than reusing one for both. `EVENT_SCHEMA_VERSION` NOT bumped — a new closed-vocabulary variant is an additive, forward-compatible change (documented in `event.rs`).
6. **`AgentAdapter::pause`/`resume` trait bodies left INERT** (unchanged) — real suspension is a PROCESS op that belongs to the `ProcessBackend`, driven by the supervisor keyed on the declared `SupportLevel`. `mock_lifecycle_ops_are_inert_until_1_4` stays valid.

### Gate Results (all run with `cargo +1.96.1`)

| # | Gate | Result |
| --- | --- | --- |
| 1 | `fmt --all --check` | PASS (clean) |
| 2 | `clippy --workspace --all-targets -- -D warnings` | PASS (0 warnings) |
| 3 | `test --workspace --all-targets` | PASS — **610 tests, 0 failed** (1-4 baseline 589; +21) |
| 4 | `tarpaulin --workspace --fail-under 95` | PASS — **95.58% coverage, 3545/3709 lines (+0.33% vs 1-4's 95.25%)** |
| 5 | `python3 scripts/check_docs.py` | PASS (23 Markdown files validated) |
| 6 | `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py` | PASS (20 tests OK) |
| 7 | MSRV: `cargo +1.96.1 check --workspace` | PASS |
| 8 | OS-cfg grep gate | PASS — no new `cfg(unix\|windows\|target_os\|target_family)` outside `crates/ktesio-engine/src/backends/` |
| 9 | Boundary gate | PASS — `kt` normal+build edges are only `ktesio-engine`/`ktesio-adapter-api`; `ktesio-conformance` absent from engine normal tree (dev-dep); no new runtime dep |

### AC → proof map

- **AC1 (Guaranteed/Unix signal):** `tests/pause.rs::guaranteed_pause_really_suspends_then_resume_wakes_it_unix` (heartbeat stops under SIGSTOP, resumes under SIGCONT, states running→paused→running, plain command causes) + backend unit `backends::unix::tests::pause_freezes_the_process_then_resume_wakes_it`. Code: `UnixBackend::pause/resume` (`signal_group(pgid, SIGSTOP/SIGCONT)`), supervisor `Guaranteed` arm.
- **AC2 (Best-effort qualifier, CLI + event):** `tests/pause.rs::best_effort_pause_transitions_and_surfaces_the_qualifier_in_the_event` (event carries `pause-best-effort`/`resume-best-effort`) + `tests/agent_cli.rs::pause_best_effort_prints_qualifier_note_to_stderr_only` (note on stderr, not stdout). Code: supervisor `BestEffort` arm + `TransitionCause::pause_best_effort`, CLI `note_if_best_effort`.
- **AC3 (Unsupported fail-fast):** `tests/pause.rs::unsupported_pause_fails_fast_with_no_state_change_and_no_event` + `tests/agent_cli.rs::pause_unsupported_exits_nonzero_quoting_the_declaration`. Code: `EngineError::CapabilityUnsupported`, supervisor `Unsupported` arm (returns before any transition/backend/persist), CLI `AgentCapabilityUnsupported` arm quoting level+OS+`kt agent show`.
- **AC4 (transition-table additions, uniform error):** `domain::transition::tests::{pause_resume_stop_from_paused_are_the_wired_1_5_edges, exhaustive_over_every_state_command_pair, invalid_command_pairs_all_yield_the_same_error_class}` + `tests/pause.rs::{pause_on_a_registered_instance_is_the_uniform_invalid_transition, resume_on_a_running_instance_is_the_uniform_invalid_transition, stop_from_paused_reaches_stopped}` + CLI `pause_on_registered_returns_uniform_invalid_transition`.
- **AC5 (effective declaration READ, not re-derived):** `domain::registry::tests::{effective_support_reads_the_current_os_pause_level_at_read_time, effective_support_defaults_to_unsupported_when_capability_absent_for_this_os}`. Code: `Registry::effective_support` (reads snapshot, projects onto `OsId::current()`).
- **AC6 (CLI surface):** `crates/kt/src/main.rs` `agent pause`/`resume` subcommands + `test_agent_pause_resume_parse` / `test_agent_subcommands_exist`; handlers `cli::agent::pause`/`resume` (result→stdout, qualifier/diagnostic→stderr). CLI integration tests above.
- **AC7 (conformance fixtures prove all three levels):** `fake_agent --heartbeat-ms` (observably suspendable) + the three `tests/pause.rs` level proofs (guaranteed/best-effort/unsupported). Mock unchanged.
- **AC8 (Windows honesty stated):** `WindowsBackend::pause/resume` cooperative best-effort body + module `[ASSUMPTION]` docs (no `NtSuspendProcess`); behavior-verified only on `windows-latest` CI, compile-checked on Unix. Documented in `docs/architecture.md` + `docs/testing.md`.

### Completion Notes

- Story 1-5 EXTENDS 1-4's supervisor + pure transition table + backends along the named seams; nothing reinvented. All 8 ACs implemented and proven; all 9 gates green. Status → review.
- Uncovered supervisor lines that remain (e.g. `signal_backend` backend-error map at 385-388) are DEFENSIVE error paths (a SIGSTOP/SIGCONT to our own group does not fail on Unix) — parity with the pre-existing uncovered `stop` backend-error arm; overall coverage rose to 95.58%.
- NOT committed (per orchestration split — the parent commits after code review). `sprint-status.yaml` and GitHub issue/project state NOT touched (parent-owned). Working tree left with all changes uncommitted.

#### Post-review fixes (Approved with nits — 4 triaged items applied)

Independent review returned **Approved with nits**; both high-severity adversarial findings were empirically REFUTED (a SIGSTOP'd process is terminated by SIGTERM without needing SIGCONT, so `stop` from `paused` does not hang — `stop()` is correct as-is). Applied ONLY the four triaged fixes; deferred items (LOW-1 no-handle plain-command cause, LOW-2 resume-on-Unsupported strand, signal-vs-persist ordering) left as-is per instruction (cross-lifetime / 1-6 territory, unreachable in 1-5).

- **NIT-1** (`event.rs` `EVENT_SCHEMA_VERSION` doc): corrected the inaccurate "old reader can skip the unknown cause" claim — with `#[serde(tag="kind")]` and no `#[serde(other)]`, an old reader hitting a new tag ERRORS; the version-negotiation reason it is still fine is spelled out. Decision NOT to bump the version kept.
- **NIT-2** (`tests/pause.rs` guaranteed test): added the symmetric assertion that the RESUME transition cause is a plain `command`/`resume` with no best-effort qualifier (previously only PAUSE was asserted).
- **LOW-3** (`tests/pause.rs` + `backends/unix/mod.rs` guaranteed tests): hardened the "heartbeat frozen while paused" assertions against scheduler jitter (they failed intermittently when the suite ran concurrently with tarpaulin). Now: settle 200ms, snapshot a baseline, then poll across a 1s window asserting the count NEVER exceeds baseline. Still a GENUINE suspension proof — a live 50ms heartbeat would emit ~20 lines and exceed baseline immediately if `pause()` were removed (sanity-checked: see "surprises" in the report). Suspension proof NOT weakened.
- **NIT-3** (this story doc): corrected the pause.rs test count from "8 tests" to "9 tests" (Task 11 line; DAR/File List already said 9).

All 9 gates re-run green after these fixes: **610 tests, 0 failed; tarpaulin 95.58%** (unchanged — the fixes are test-hardening + comments/doc, no production-line change except the corrected doc comment).

## File List

**crates/ktesio-engine** (engine core + backends + ports + tests)
- `src/domain/transition.rs` — `Pause`/`Resume` commands + `as_str`; `next_state` rows `(Running,Pause)→Paused`, `(Paused,Resume)→Running`, `(Paused,Stop)→Stopping`; updated exhaustive + invalid-pair + label tests + doc comment.
- `src/domain/event.rs` — `TransitionCause::PauseBestEffort`/`ResumeBestEffort` + constructors + stable-tag/round-trip tests; additive-vs-breaking note on `EVENT_SCHEMA_VERSION`.
- `src/domain/error.rs` — `EngineError::CapabilityUnsupported { name, capability, os, level }`.
- `src/domain/registry.rs` — `effective_support(name, capability) -> SupportLevel` (read-time F3 projection) + 2 tests; `Capability`/`SupportLevel` import.
- `src/domain/supervisor.rs` — `pause`/`resume` + shared `suspend_or_resume` three-level dispatch + `signal_backend`; adapter-api imports.
- `src/ports/process_backend.rs` — `ProcessBackend::pause`/`resume` trait methods + `BackendError::Control` op-note.
- `src/backends/unix/mod.rs` — `UnixBackend::pause`/`resume` (SIGSTOP/SIGCONT) + 2 unit tests.
- `src/backends/windows/mod.rs` — `WindowsBackend::pause`/`resume` (cooperative best-effort) + `[ASSUMPTION]` module docs.
- `src/engine.rs` — async `Engine::pause`/`resume` + `Blocking::pause`/`resume`.
- `tests/pause.rs` — NEW: 9 integration tests (guaranteed/best-effort/unsupported + AC4 edges + cross-lifetime no-handle + not-found/invalid-name).

**crates/kt** (CLI)
- `src/error.rs` — `AgentCapabilityUnsupported` miette diagnostic (`code(ktesio::agent::capability_unsupported)`).
- `src/cli/agent.rs` — `pause`/`resume` handlers + `note_if_best_effort` + `map_engine_error` `CapabilityUnsupported` arm; imports.
- `src/main.rs` — `agent pause`/`resume` clap subcommands + dispatch + help text + parse/subcommand tests.
- `tests/agent_cli.rs` — 6 new pause/resume CLI integration tests + helpers.

**crates/ktesio-conformance** (fixtures)
- `src/bin/fake_agent.rs` — `--heartbeat-ms <ms>` (observable suspension); all existing args preserved.

**docs**
- `docs/architecture.md` — lifecycle section: `paused` edges, three-level pause dispatch, SIGSTOP/SIGCONT, best-effort qualifier, unsupported fail-fast, Windows-CI honesty.
- `docs/testing.md` — `fake_agent --heartbeat-ms` proof; Unix SIGSTOP/SIGCONT leg; Windows best-effort qualifier leg.

**story file**
- `_bmad-output/implementation-artifacts/1-5-pause-and-resume-with-honest-per-os-semantics.md` — frontmatter `baseline_commit`; Status; task checkboxes; this Dev Agent Record / File List / Change Log.

## Change Log

| Date | Change |
| --- | --- |
| 2026-07-04 | Story 1-5 implemented: pause/resume with honest per-OS semantics (guaranteed SIGSTOP/SIGCONT, best-effort qualifier, unsupported fail-fast). All 8 ACs + 9 gates green (610 tests, 95.58% coverage). Status ready-for-dev → in-progress → review. |
| 2026-07-04 | Post-review (Approved with nits): applied 4 triaged fixes — NIT-1 (accurate `EVENT_SCHEMA_VERSION` forward-compat wording), NIT-2 (symmetric guaranteed-resume cause assertion), LOW-3 (hardened paused-heartbeat assertions against scheduler jitter, proof preserved), NIT-3 (test-count 8→9). Deferred items (LOW-1/LOW-2/ordering) left as-is. Gates re-verified green: 610 tests, 95.58% coverage. |
