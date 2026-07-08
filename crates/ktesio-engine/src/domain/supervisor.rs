//! The lifecycle supervisor (spine AD-1, AD-4, AD-12/AD-14/AD-15 seeds).
//!
//! The supervisor owns the running Agent Instances' process handles in memory
//! for the current engine lifetime and drives every lifecycle transition:
//!
//! 1. apply the transition table ([`next_state`](super::transition::next_state)),
//! 2. spawn / stop via the per-OS
//!    [`ProcessBackend`](crate::ports::ProcessBackend) (selected in
//!    `backends/mod.rs`; the supervisor names only the trait + cfg-selected
//!    aliases — it is cfg-free),
//! 3. persist the new state via the [`Registry`], and
//! 4. emit the [`TransitionEvent`] (append to the per-instance log + return it).
//!
//! ## Cross-lifetime supervision (AD-5, story 1-6: IMPLEMENTED)
//!
//! The running-handle map lives for THIS engine's lifetime, but the write-ahead
//! spawn records (AD-5) persist across lifetimes. Story 1-6 IMPLEMENTS orphan
//! adoption: [`Supervisor::adopt_orphans`] (called from [`Engine::open`]) reads
//! every persisted [`SpawnRecord`] and re-attaches to a still-live process whose
//! start-time fingerprint matches (`backend.adopt`), re-populating the handle map
//! so `stop`/`pause`/`poll` work on it again; a record whose process is gone (or
//! whose PID was reused) reconciles to `failed`. So a process started by a prior
//! engine that CRASHED is now re-adopted (or honestly failed) — the single-
//! lifetime boundary is lifted for the durable-record case.
//!
//! ## Crash detection + Restart Policy (AD-5/AD-15, story 1-6)
//!
//! [`Supervisor::poll_once`] is the reaper: it polls every held handle via the
//! EXISTING `backend.poll` and, on an unrequested `Exited` for an instance the
//! store still shows `running`/`paused`, applies the EVENT-driven `running →
//! failed` edge (a [`TransitionCause::Crashed`]) and consults the per-instance
//! [`RestartPolicy`] to decide whether to schedule a restart (returning a
//! [`RestartPlan`] the engine cadence times). The reaper + restart executor stay
//! SYNC + cfg-free; the engine owns the poll interval and the backoff timer.
//!
//! ## What "an event" is here (AD-14 seed)
//!
//! Each transition RECORDS a [`TransitionEvent`] to the per-instance log and
//! returns it (observable to tests / embedders). This is NOT the 7-2 bounded
//! subscription bus — only the seed struct + its recording.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ktesio_adapter_api::{Capability, OsId, SupportLevel};

use crate::adapter::{self, ConfigApplyError, LaunchResolveError};
use crate::backends;
use crate::ports::{BackendError, ProcessBackend, ProcessStatus, SpawnRecord, SpawnSpec};
use crate::time::now_rfc3339;

use super::error::EngineError;
use super::event::{TransitionCause, TransitionEvent};
use super::instance::AgentInstance;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::registry::Registry;
use super::restart::{is_crash_loop, BackoffSchedule, RestartPolicy, MAX_CONSECUTIVE_FAILURES};
use super::transition::{next_state, LifecycleCommand};

/// The default graceful-shutdown window before a stop escalates to a forced kill
/// (AC3). Per-instance configurable via [`Supervisor::stop`]'s `window` argument;
/// this is the conservative fallback when the caller passes `None`.
pub const DEFAULT_STOP_WINDOW: Duration = Duration::from_secs(30);

/// How long to watch a freshly spawned process for an immediate failure before
/// declaring it `running` (the readiness definition, `[ASSUMPTION]`).
///
/// "Adapter ready" this story = "the process spawned and did not die during this
/// short startup window". A process that exits (especially non-zero) within it is
/// treated as a launch failure (AC2 "immediate non-zero exit during startup").
/// Kept small so `start` stays snappy; the fake test agent's `--exit-fast` path
/// exits well inside it.
const READINESS_WINDOW: Duration = Duration::from_millis(300);

/// How often the readiness watch polls the freshly spawned process.
const READINESS_POLL: Duration = Duration::from_millis(10);

/// A scheduled restart of a crashed instance (story 1-6, AC4). Returned by
/// [`Supervisor::poll_once`] for each crashed `on-failure` instance that has not
/// hit the crash-loop threshold; the engine cadence sleeps [`RestartPlan::delay`]
/// then calls [`Supervisor::restart`] with the plan's `attempt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartPlan {
    /// The instance to restart.
    pub name: InstanceName,
    /// The consecutive restart attempt number (1-based) this plan represents.
    pub attempt: u32,
    /// The backoff to wait before performing the restart.
    pub delay: Duration,
}

/// The Restart Policy outcome for a just-crashed instance (internal to the
/// reaper). Carries the crash cause to record in the event log — enriched with
/// the policy conclusion on a terminal outcome so the failed cause survives after
/// the write-ahead record is cleared — and, when a restart is scheduled, the
/// [`RestartPlan`] the engine cadence should time.
struct RestartDecision {
    /// The crash cause detail to record on the `running → failed` event.
    crash_cause: String,
    /// The restart to schedule, or `None` on a terminal (`never`/crash-loop) outcome.
    plan: Option<RestartPlan>,
}

/// The lifecycle supervisor: owns running process handles + drives transitions.
///
/// Constructed empty by [`Engine::open`](crate::Engine::open). Holds ONE
/// [`ProcessBackend`](crate::ports::ProcessBackend) (the current OS's), a map of
/// the instances it currently supervises, and the [`BackoffSchedule`] the restart
/// executor uses (production 1s×2 cap 60s; tests inject a scaled one).
pub struct Supervisor {
    backend: backends::Backend,
    running: HashMap<InstanceName, backends::Handle>,
    backoff: BackoffSchedule,
}

impl Supervisor {
    /// Construct an empty supervisor with the current OS's process backend and
    /// the PRODUCTION backoff schedule (1s base, ×2, 60s cap — spine AD-15).
    pub fn new() -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
            backoff: BackoffSchedule::production(),
        }
    }

    /// Construct an empty supervisor with a custom backoff schedule (TEST
    /// injection, so the crash-loop / backoff legs run in milliseconds without
    /// weakening the production constants). Production always uses
    /// [`Supervisor::new`].
    #[cfg(test)]
    pub(crate) fn with_backoff(backoff: BackoffSchedule) -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
            backoff,
        }
    }

    /// Start a registered / previously stopped / FAILED Agent Instance
    /// (AC1/AC2; AC3 restart-from-failed via the 1-6 transition row).
    ///
    /// Thin wrapper over [`Supervisor::start_inner`] with no restart context (a
    /// fresh operator `start`): the `starting → running` transition records a
    /// plain [`TransitionCause::AdapterReady`], and the write-ahead spawn record's
    /// restart count is RESET to 0 (a clean run resets the count, AC4).
    pub fn start(&mut self, registry: &Registry, name: &str) -> Result<AgentInstance, EngineError> {
        self.start_inner(registry, name, None)
    }

    /// The shared start path (AC1/AC2 + the 1-6 write-ahead record commit).
    ///
    /// `restart`:
    /// * `None` — a fresh `start` (operator or first launch). The
    ///   `starting → running` cause is [`TransitionCause::AdapterReady`]; the
    ///   spawn record's restart count is RESET to 0.
    /// * `Some((attempt, waited))` — a Restart Policy restart (from
    ///   [`Supervisor::restart`]). The `starting → running` cause is
    ///   [`TransitionCause::Restarted`] recording the consecutive `attempt` +
    ///   the backoff `waited`; the record keeps that count.
    ///
    /// Order (so a rejection leaves NO spurious state change):
    /// 1. look up + validate `Start` against the transition table (AC4),
    /// 2. resolve the launch spec (a bad/native-only adapter rejects here),
    /// 3. persist `registered/stopped/failed → starting` + emit,
    /// 4. spawn; a spawn failure → `starting → failed` (diagnostic preserved),
    /// 5. readiness watch: an immediate death → `failed` (AC2),
    /// 6. **commit the write-ahead spawn record** (AD-5: `{pid, fingerprint}` +
    ///    policy + count) BEFORE the instance is treated as supervised — "no
    ///    spawn without its record committed first",
    /// 7. persist `starting → running` + emit, store the handle, return.
    fn start_inner(
        &mut self,
        registry: &Registry,
        name: &str,
        restart: Option<(u32, Duration)>,
    ) -> Result<AgentInstance, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // (1) Transition gate (AC4). Rejects (e.g. start on running) before any
        // side effect and BEFORE we touch the backend.
        let starting = next_state(instance.state, LifecycleCommand::Start)?;

        // (2) Resolve the launch spec (may reject: native-only / bad manifest),
        // still before any persisted state change.
        let (kind, manifest_path) = registry
            .adapter_launch_facts(&name)
            .map_err(registry_to_engine)?;
        let mut launch = adapter::resolve_start_launch(&kind, manifest_path.as_deref())
            .map_err(|e| launch_to_engine(&name, e))?;

        // (2b) Map the resolved unified config into the adapter's NATIVE mechanism
        // (story 2-2, FR-12) — still before any persisted state change, so a
        // config/mapping failure rejects the start cleanly (no spurious state
        // change, no half-launched process). Resolve the instance's effective
        // config (2-1's four-layer fold; empty invocation overrides for a plain
        // start — the parameter is threaded so a future `start --set k=v` supplies
        // it without an API change), the adapter's declared mapping (manifest
        // `[config]` or the native code-declared table), then apply: known keys
        // land in their declared native target (env → launch.env; flag →
        // launch.args; file → a rendered file in the Agent Home), and `agent.*`
        // pass-through leaves are delivered VERBATIM (AC6). The Agent Home already
        // exists (created at registration); file targets render into it here.
        let home = registry.agent_home(&name);
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .map_err(|e| config_to_engine(&name, e))?;
        let mapping = adapter::resolve_config_mapping(&kind, manifest_path.as_deref())
            .map_err(|e| launch_to_engine(&name, e))?;
        // (2b-secret) Resolve every `secret:NAME` leaf into a SecretString BEFORE
        // the mapping application (story 2-4, spine AD-10, AC-A/AC9). This is where
        // display and delivery DIVERGE: `effective`'s `display()`-based surfaces
        // (the snapshot at (2c), `config get`) stay MASKED, but the resolved
        // cleartext flows into `apply_config_mapping` so the ADAPTER gets a usable
        // key. Resolution (env → the 0600 secrets file) runs here, still before any
        // persisted state change, so an unresolved/ill-permissioned secret REJECTS
        // the start cleanly (no half-launch, mirroring the config-apply + snapshot
        // failures) — a typed `EngineError::Secret` that NEVER echoes a value.
        let secrets = registry
            .resolve_secrets(&effective)
            .map_err(|e| secret_to_engine(&name, e))?;
        adapter::apply_config_mapping(&mut launch, &mapping, &effective, &secrets, &home)
            .map_err(|e| config_apply_to_engine(&name, e))?;

        // (2c) Persist the effective-config snapshot into the Agent Home (story
        // 2-3, spine AD-9 "start resolves to an EffectiveConfig snapshot persisted
        // in the Agent Home, every value tagged with its source layer" + AD-6
        // "effective-config snapshots are files inside the Agent Home"). The
        // resolved `effective` is already in hand from (2b); write it HERE, right
        // after the mapping application and BEFORE the `starting` transition below,
        // so a snapshot-write failure rejects the start cleanly (NO state change —
        // exactly mirroring how the config-apply failure at (2b) rejects before the
        // transition). The snapshot is a PROMISED AD-9 artifact (a Host/debugging
        // record of "what will apply on next start"), not a best-effort nicety, so
        // its failure is a typed start error. Because RESTART also flows through
        // this path (story 1-6), the snapshot is refreshed on restart too (AC7:
        // OVERWRITTEN every successful start/restart, never a stale resolution). It
        // is NOT written at registration (there is no "effective at start" until a
        // start happens) and NOT deleted at stop.
        registry
            .write_effective_config_snapshot(&name, &effective)
            .map_err(snapshot_to_engine)?;

        // Read the per-instance Restart Policy so the write-ahead record carries
        // it (AD-15 per-instance configurable). Read once, before any side effect.
        let policy = registry
            .effective_restart_policy(&name)
            .map_err(registry_to_engine)?;

        // The spawned agent's stdout/stderr go to a SEPARATE agent.log, never the
        // engine's JSON-Lines transition-event log (instance.log) — otherwise the
        // agent's plain-text output would corrupt the structured event log.
        let agent_log_path = registry.agent_output_log_path(&name);
        // Ensure the log directory exists (AD-12 seed) so spawn can redirect
        // stdout/stderr into it and we can append transition events.
        self.ensure_log_dir(registry, &name)?;

        // (3) registered/stopped/failed → starting.
        self.transition(
            registry,
            &name,
            instance.state,
            starting,
            TransitionCause::command(LifecycleCommand::Start.as_str()),
        )?;

        let spec = SpawnSpec {
            exec: launch.exec.clone(),
            args: launch.args,
            env: launch.env,
            working_dir: home,
            log_file: Some(agent_log_path),
        };

        // (4) Spawn. A spawn failure lands the instance in `failed` with the
        // diagnostic preserved and no zombie (the backend spawned nothing / reaps).
        let mut handle = match self.backend.spawn(&spec) {
            Ok(handle) => handle,
            Err(err) => return Err(self.fail_launch(registry, &name, &err)),
        };

        // (5) Readiness watch: a process that dies immediately (especially
        // non-zero) during startup is a launch failure (AC2). Watch briefly;
        // `watch_startup` returns the exit code (if it died) or `None` (ready).
        if let Some(exit_code) = self.watch_startup(&mut handle) {
            let detail = match exit_code {
                Some(c) => format!("exited immediately during startup with code {c}"),
                None => "exited immediately during startup".to_string(),
            };
            // Reap already done by poll; nothing survives.
            return Err(self.fail_launch_detail(registry, &name, detail));
        }

        // (6) Commit the write-ahead spawn record (AD-5) BEFORE the instance is
        // treated as supervised — "no spawn without its record committed first".
        // A fresh start resets the restart count to 0; a restart keeps its
        // attempt count. The fingerprint is the PID-reuse guard for later
        // orphan adoption. A record-commit failure fails the start (leaving the
        // instance `failed`) — we must not run an unrecorded supervised process.
        let restart_count = restart.map(|(attempt, _)| attempt).unwrap_or(0);
        let record = SpawnRecord {
            name: name.clone(),
            fingerprint: self.backend.fingerprint(&handle),
            restart_policy: policy,
            restart_count,
            last_known_cause: None,
        };
        if let Err(e) = registry.write_spawn_record(&record) {
            // Persisting the record failed: kill the just-spawned process (drop
            // the handle → group/job kill) and land the instance in `failed` so
            // we never supervise an unrecorded process (AD-5 safety).
            drop(handle);
            return Err(self.fail_launch_detail(
                registry,
                &name,
                format!("could not commit the write-ahead spawn record: {e}"),
            ));
        }

        // (7) starting → running (adapter ready, or a Restart Policy restart).
        let ready_cause = match restart {
            Some((attempt, waited)) => {
                TransitionCause::restarted(attempt, waited.as_millis() as u64)
            }
            None => TransitionCause::AdapterReady,
        };
        self.transition(
            registry,
            &name,
            starting,
            LifecycleState::Running,
            ready_cause,
        )?;
        self.running.insert(name.clone(), handle);

        registry.lookup(&name).map_err(registry_to_engine)
    }

    /// Perform ONE Restart Policy restart of a crashed instance (story 1-6, AC4).
    ///
    /// Called by the engine cadence AFTER it has waited the backoff
    /// [`RestartPlan::delay`]. Re-runs the start path (`failed → starting →
    /// running`) recording a [`TransitionCause::Restarted`] with the consecutive
    /// `attempt` + the `waited` backoff, and keeps the persisted restart count at
    /// `attempt`.
    ///
    /// Interaction with a concurrent `stop`: `restart` re-runs the start path, so
    /// its transition gate is `next_state(state, Start)`. That gate only accepts
    /// `failed` (or registered/stopped); if the instance was already restarted to
    /// `running` by an EARLIER plan, or an operator stopped it back to `stopped`,
    /// the gate rejects and this restart is a harmless no-op. NOTE: during the
    /// backoff WINDOW the instance is `failed`, and `next_state(Failed, Stop)` is
    /// an `InvalidTransition` — so an operator cannot `stop` a mid-backoff
    /// instance to pre-empt this restart (there is no `failed → stopping` edge
    /// this story; adding one is out of scope). The restart therefore proceeds; a
    /// stop is only effective once the instance is `running` again.
    pub fn restart(
        &mut self,
        registry: &Registry,
        name: &str,
        attempt: u32,
        waited: Duration,
    ) -> Result<AgentInstance, EngineError> {
        self.start_inner(registry, name, Some((attempt, waited)))
    }

    /// Stop a running Agent Instance (AC3/AC4).
    ///
    /// Transitions `running → stopping`, requests graceful shutdown via the
    /// backend and escalates to a forced kill after `window` (default
    /// [`DEFAULT_STOP_WINDOW`]) if needed, records the escalation in the instance
    /// log, then `stopping → stopped`. No process of the instance survives (the
    /// backend kills the whole group/job).
    pub fn stop(
        &mut self,
        registry: &Registry,
        name: &str,
        window: Option<Duration>,
    ) -> Result<AgentInstance, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // Transition gate (AC4): stop on stopped / registered / … rejects here
        // with the uniform InvalidTransition, before touching any process.
        let stopping = next_state(instance.state, LifecycleCommand::Stop)?;

        let window = window.unwrap_or(DEFAULT_STOP_WINDOW);
        self.ensure_log_dir(registry, &name)?;

        // running → stopping.
        self.transition(
            registry,
            &name,
            instance.state,
            stopping,
            TransitionCause::command(LifecycleCommand::Stop.as_str()),
        )?;

        // Ask the backend to stop the process (group/job). If we have no handle
        // for it (the row says running but this engine holds no handle AND orphan
        // adoption found no live process), the desired end state "no process of
        // the instance survives" already holds, so we treat it as a graceful
        // stop. With story 1-6 adoption, a handle for a still-live process
        // started by a PRIOR engine IS re-held (via `adopt_orphans`), so a
        // cross-restart stop now really terminates it.
        let outcome = match self.running.get_mut(&name) {
            Some(handle) => {
                self.backend
                    .stop(handle, window)
                    .map_err(|source| EngineError::Backend {
                        name: name.as_str().to_string(),
                        source,
                    })?
            }
            None => crate::ports::StopOutcome { forced: false },
        };
        // Drop the handle (also closes the Job / releases the child on Windows).
        self.running.remove(&name);

        // Clear the write-ahead spawn record (AD-5): a cleanly-stopped instance
        // must NOT be later adopted or reconciled-to-failed as an orphan. Cleared
        // BEFORE the terminal transition so the durable record leads the state.
        registry
            .clear_spawn_record(&name)
            .map_err(registry_to_engine)?;

        // stopping → stopped, recording whether escalation happened (AC3).
        let cause = if outcome.forced {
            TransitionCause::stop_forced(format!(
                "graceful window ({}s) elapsed; escalated to a forced kill of the process group/job",
                window.as_secs()
            ))
        } else {
            TransitionCause::StopGraceful
        };
        self.transition(registry, &name, stopping, LifecycleState::Stopped, cause)?;

        registry.lookup(&name).map_err(registry_to_engine)
    }

    /// Pause a running Agent Instance with honest, per-OS semantics (story 1-5,
    /// AC1/AC2/AC3/AC5 — the "surfaced not silent" HONESTY command).
    ///
    /// Order mirrors [`Supervisor::stop`], except the middle step DISPATCHES on
    /// the effective (current-OS) pause `SupportLevel` read from the persisted
    /// snapshot (AC5), rather than always calling the backend:
    /// 1. name → [`InstanceName`]; look up the instance,
    /// 2. transition gate `next_state(state, Pause)?` — an invalid transition
    ///    (e.g. pause on `stopped`/`paused`) rejects HERE with the uniform
    ///    [`LifecycleError::InvalidTransition`] (AC4), before any side effect or
    ///    level read,
    /// 3. read the effective pause level (AC5) and dispatch:
    ///    * **Guaranteed** → `backend.pause(handle)` (real SIGSTOP suspension on
    ///      Unix), then persist `running→paused` + a plain
    ///      [`TransitionCause::Command`] (`"pause"`) — no qualifier,
    ///    * **BestEffort** → persist `running→paused` + a
    ///      [`TransitionCause::PauseBestEffort`] qualifier (the machine-readable
    ///      half of "surfaced not silent"); the process may keep running,
    ///    * **Unsupported** → FAIL FAST with
    ///      [`EngineError::CapabilityUnsupported`], NO transition, NO backend
    ///      call, NOTHING persisted (AC3).
    pub fn pause(&mut self, registry: &Registry, name: &str) -> Result<AgentInstance, EngineError> {
        self.suspend_or_resume(registry, name, LifecycleCommand::Pause)
    }

    /// Resume a paused Agent Instance (story 1-5, AC1/AC2).
    ///
    /// The symmetric counterpart of [`Supervisor::pause`]: the transition gate is
    /// `next_state(state, Resume)?` (`paused → running`; anything else rejects
    /// with the uniform invalid-transition, AC4), and the dispatch is on the same
    /// effective pause level:
    /// * **Guaranteed** → `backend.resume(handle)` (SIGCONT), then `paused→running`
    ///   + a plain `resume` command cause,
    /// * **BestEffort** → `paused→running` + a [`TransitionCause::ResumeBestEffort`]
    ///   qualifier,
    /// * **Unsupported** → fail fast (defensive; a `paused` instance implies pause
    ///   was allowed, so this is not normally reachable — see the note on the
    ///   symmetric dispatch below).
    pub fn resume(
        &mut self,
        registry: &Registry,
        name: &str,
    ) -> Result<AgentInstance, EngineError> {
        self.suspend_or_resume(registry, name, LifecycleCommand::Resume)
    }

    /// Shared pause/resume driver (the three-level dispatch), keyed on `command`
    /// (`Pause` or `Resume`). Kept as one method so the pause and resume paths
    /// cannot drift: the transition gate, the level read, and the three-way
    /// dispatch are identical; only the target state and the cause differ.
    fn suspend_or_resume(
        &mut self,
        registry: &Registry,
        name: &str,
        command: LifecycleCommand,
    ) -> Result<AgentInstance, EngineError> {
        debug_assert!(
            matches!(command, LifecycleCommand::Pause | LifecycleCommand::Resume),
            "suspend_or_resume only handles Pause/Resume"
        );
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // (1) Transition gate (AC4): pause on stopped/paused, resume on running,
        // etc. reject HERE with the uniform InvalidTransition, before any level
        // read or side effect.
        let new_state = next_state(instance.state, command)?;

        // (2) Read the effective (current-OS) pause level from the persisted
        // snapshot (AC5). Projected at read time onto OsId::current(); NOT
        // re-derived from the manifest, NOT frozen at register time.
        let level = registry
            .effective_support(&name, Capability::Pause)
            .map_err(registry_to_engine)?;
        let os = OsId::current();

        // (3) Dispatch on the level.
        match level {
            // FAIL FAST (AC3): no transition, no backend call, nothing persisted.
            SupportLevel::Unsupported => Err(EngineError::CapabilityUnsupported {
                name: name.as_str().to_string(),
                capability: Capability::Pause.as_str().to_string(),
                os: os.as_str().to_string(),
                level: level.as_str().to_string(),
            }),
            // GUARANTEED (AC1): real suspension via the backend, then a plain
            // command-cause transition (no qualifier — it is a true suspension).
            SupportLevel::Guaranteed => {
                self.ensure_log_dir(registry, &name)?;
                self.signal_backend(&name, command)?;
                self.transition(
                    registry,
                    &name,
                    instance.state,
                    new_state,
                    TransitionCause::command(command.as_str()),
                )?;
                registry.lookup(&name).map_err(registry_to_engine)
            }
            // BEST-EFFORT (AC2): transition + a VISIBLE qualifier cause, never a
            // silent success. No backend suspension is guaranteed here (on Unix a
            // best-effort declaration is unusual, but we still do NOT SIGSTOP — the
            // declared level is the contract; the qualifier is the honesty).
            SupportLevel::BestEffort => {
                self.ensure_log_dir(registry, &name)?;
                let detail = format!(
                    "{} is best-effort for '{}' on {} (adapter-cooperative); the process may keep running",
                    Capability::Pause.as_str(),
                    name.as_str(),
                    os.as_str(),
                );
                let cause = match command {
                    LifecycleCommand::Pause => TransitionCause::pause_best_effort(detail),
                    _ => TransitionCause::resume_best_effort(detail),
                };
                self.transition(registry, &name, instance.state, new_state, cause)?;
                registry.lookup(&name).map_err(registry_to_engine)
            }
        }
    }

    /// Signal the running process for a GUARANTEED pause/resume, via the in-memory
    /// handle map (same `self.running.get_mut(&name)` pattern as `stop`).
    ///
    /// Cross-lifetime honesty (AD-5, story 1-6: adoption re-holds handles): with
    /// orphan adoption, a still-live process started by a PRIOR engine is
    /// re-acquired at [`Engine::open`] (via [`Supervisor::adopt_orphans`]), so its
    /// handle IS in the map and this path really signals it. The no-handle branch
    /// now only occurs when the row says `running`/`paused` but adoption found NO
    /// live process — a state adoption would already have reconciled to `failed`;
    /// so a lingering no-handle case is a best-effort no-op (nothing to signal),
    /// which still lets the transition proceed. A real held (spawned or adopted)
    /// process IS signalled.
    fn signal_backend(
        &mut self,
        name: &InstanceName,
        command: LifecycleCommand,
    ) -> Result<(), EngineError> {
        let Some(handle) = self.running.get_mut(name) else {
            return Ok(());
        };
        let result = match command {
            LifecycleCommand::Pause => self.backend.pause(handle),
            _ => self.backend.resume(handle),
        };
        result.map_err(|source| EngineError::Backend {
            name: name.as_str().to_string(),
            source,
        })
    }

    /// Read the recorded [`TransitionEvent`]s for an instance from its log
    /// (observation helper for tests / embedders; the AD-14 seed, NOT the 7-2
    /// bus). Returns an empty vec if the log does not exist yet.
    pub fn read_events(
        registry: &Registry,
        name: &str,
    ) -> Result<Vec<TransitionEvent>, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let path = registry.instance_log_path(&name);
        read_events_from(&path).map_err(|detail| EngineError::Log {
            name: name.as_str().to_string(),
            path: path.to_string_lossy().into_owned(),
            detail,
        })
    }

    /// The crash-detection reaper pass (story 1-6, AC-A / AC3 / AC5).
    ///
    /// Polls every held handle via the EXISTING `backend.poll` and reacts to an
    /// unrequested exit: for each instance the store still shows `running` or
    /// `paused` (a `stopping` in flight means an operator stop is under way — NOT
    /// a crash, so it is skipped), applies the EVENT-driven `running → failed`
    /// edge with a [`TransitionCause::Crashed`] (AC5), removes the handle, and
    /// consults the per-instance [`RestartPolicy`] (AD-15):
    /// * [`RestartPolicy::Never`] — leave `failed`; record the crash cause; NO
    ///   restart plan.
    /// * [`RestartPolicy::OnFailure`] — increment the consecutive restart count;
    ///   if it hit the crash-loop threshold ([`is_crash_loop`]) leave `failed`
    ///   with the crash-loop reason and NO plan; otherwise persist the new count
    ///   and return a [`RestartPlan`] with the backoff delay for that attempt.
    ///
    /// Returns the [`RestartPlan`]s the engine cadence should time. SYNC +
    /// cfg-free (the engine calls it via `spawn_blocking` on an interval); it
    /// performs NO sleeping itself. Idempotent per exit: once an instance is
    /// moved to `failed` and its handle removed, a later pass will not see it in
    /// `self.running` again.
    pub fn poll_once(&mut self, registry: &Registry) -> Vec<RestartPlan> {
        // Snapshot the currently-held names (we mutate self.running as we react).
        let names: Vec<InstanceName> = self.running.keys().cloned().collect();
        let mut plans = Vec::new();

        for name in names {
            // Poll liveness. A poll error is treated as still-alive (transient);
            // the next pass re-checks. Reap on exit is done inside `poll`.
            let exited = match self.running.get_mut(&name) {
                Some(handle) => match self.backend.poll(handle) {
                    Ok(ProcessStatus::Exited { code }) => Some(code),
                    Ok(ProcessStatus::Alive) => None,
                    Err(_) => None,
                },
                None => continue,
            };
            let Some(code) = exited else { continue };

            // Read the store state: only an instance the store still shows
            // running/paused is an UNREQUESTED crash. A `stopping` (operator
            // stop) or any other state is not a crash — drop the (now-dead)
            // handle without a `failed` transition.
            let state = match registry.lookup(&name) {
                Ok(inst) => inst.state,
                // The row is gone (removed concurrently) — just drop the handle.
                Err(_) => {
                    self.running.remove(&name);
                    continue;
                }
            };
            if !matches!(state, LifecycleState::Running | LifecycleState::Paused) {
                // Requested stop (or already-terminal) — not a crash.
                self.running.remove(&name);
                continue;
            }

            // A crash. Consult the Restart Policy FIRST (so a terminal outcome —
            // `never` or crash-loop — can enrich the recorded crash cause), then
            // apply running/paused → failed with that detail (AC5).
            self.running.remove(&name);
            let base_detail = match code {
                Some(c) => format!("process exited unexpectedly with code {c}"),
                None => "process exited unexpectedly (terminated by signal)".to_string(),
            };
            let decision = self.plan_restart(registry, &name, &base_detail);
            if self.ensure_log_dir(registry, &name).is_err() {
                // If we cannot even prepare the log dir, still persist the state
                // so the durable state leads; skip the event append best-effort.
            }
            // The recorded crash cause carries the full story: the exit detail,
            // plus (on a terminal outcome) the policy conclusion (crash-loop, or
            // "policy is never — not restarting"). This is what `instance_status`
            // falls back to for the failed cause once the terminal record is
            // cleared (AC9).
            if self
                .transition(
                    registry,
                    &name,
                    state,
                    LifecycleState::Failed,
                    TransitionCause::crashed(decision.crash_cause.clone()),
                )
                .is_err()
            {
                // Persisting the crash transition failed; leave the record for a
                // later reconcile and move on (do not panic the reaper).
                continue;
            }

            if let Some(plan) = decision.plan {
                plans.push(plan);
            }
        }
        plans
    }

    /// Decide the Restart Policy action for a just-crashed instance (AC4).
    ///
    /// Reads the per-instance record (policy + current consecutive count) and
    /// returns a [`RestartDecision`]: the crash cause to record in the event log
    /// (enriched with the policy conclusion on a terminal outcome) and, when a
    /// restart is scheduled, the [`RestartPlan`]. Side effects (all best-effort —
    /// a store hiccup is never a panic):
    /// * `on-failure`, below the crash-loop threshold → increment the persisted
    ///   restart count; the plan carries the backoff delay for that attempt.
    /// * `on-failure`, at the crash-loop threshold ([`is_crash_loop`]) → TERMINAL:
    ///   CLEAR the write-ahead record (F-Low-2: no needless adopt-attempt against
    ///   a dead/reused PID on a later open) and enrich the crash cause with the
    ///   crash-loop reason; no plan.
    /// * `never` → TERMINAL: clear the write-ahead record and note the policy in
    ///   the crash cause; no plan.
    fn plan_restart(
        &self,
        registry: &Registry,
        name: &InstanceName,
        crash_detail: &str,
    ) -> RestartDecision {
        let record = registry.spawn_record(name).ok().flatten();
        let policy = record
            .as_ref()
            .map(|r| r.restart_policy)
            .unwrap_or_default();
        let current = record.as_ref().map(|r| r.restart_count).unwrap_or(0);

        if !policy.restarts_on_crash() {
            // `never`: TERMINAL. Settle the record so a later open does not
            // adopt-attempt a dead PID; the crash cause names the policy.
            self.settle_terminal_record(registry, name, policy);
            return RestartDecision {
                crash_cause: format!("{crash_detail}; restart policy is 'never' — not restarting"),
                plan: None,
            };
        }

        let next = current.saturating_add(1);
        if is_crash_loop(next) {
            // Crash loop: TERMINAL. Settle the record (F-Low-2), leave `failed`
            // with the reason STATED in the crash cause.
            self.settle_terminal_record(registry, name, policy);
            return RestartDecision {
                crash_cause: format!(
                    "{crash_detail}; crash-loop: {} consecutive failures reached — \
                     not restarting, inspect the agent and start it manually",
                    MAX_CONSECUTIVE_FAILURES,
                ),
                plan: None,
            };
        }

        // Schedule a restart: persist the incremented count + the crash cause,
        // and return the plan with the backoff delay for this attempt.
        let _ = registry.set_restart_count(name, next, Some(crash_detail));
        let delay = self.backoff.delay_for(next);
        RestartDecision {
            crash_cause: crash_detail.to_string(),
            plan: Some(RestartPlan {
                name: name.clone(),
                attempt: next,
                delay,
            }),
        }
    }

    /// Settle the write-ahead record on a TERMINAL `failed` outcome (F-Low-2).
    ///
    /// Drops the record's LIVE fingerprint (so a later [`Supervisor::adopt_orphans`]
    /// does NOT adopt-attempt the dead/reused PID — the reconcile skips a pid-0
    /// record, exactly like a policy-only config seed), while RE-SEEDING the
    /// per-instance policy so `kt agent show` still reports the active Restart
    /// Policy for the failed instance (AC9). Concretely: clear the record, then
    /// re-persist the policy as a pid-0 seed. The failed CAUSE is not kept in the
    /// record — it rides in the event log, which `instance_status` falls back to.
    /// Best-effort (a store hiccup here is never a panic).
    fn settle_terminal_record(
        &self,
        registry: &Registry,
        name: &InstanceName,
        policy: RestartPolicy,
    ) {
        let _ = registry.clear_spawn_record(name);
        let _ = registry.set_restart_policy(name, policy);
    }

    /// Adopt orphaned processes on engine start (story 1-6, AC-B / AC7 / AI-7 /
    /// AI-8) — the HONEST cross-lifetime reconcile.
    ///
    /// Reads EVERY write-ahead [`SpawnRecord`] (AD-5) and, for each, asks the
    /// backend to re-acquire a live process matching the fingerprint
    /// (`backend.adopt`):
    /// * `Some(handle)` — a live process whose start-time matches: ADOPT it
    ///   (re-hold the handle so `stop`/`pause`/`poll` work again); the persisted
    ///   state stays as-is (`running`/`paused` — AI-7: a live paused process is
    ///   re-held so a later `resume` works).
    /// * `None` — no live match (PID gone, or reused by a different process):
    ///   reconcile HONESTLY to `failed` with an "orphan not found" cause + the
    ///   last-known cause (AI-8: never leave a phantom `running`/`paused` row),
    ///   and clear the record.
    ///
    /// Called from [`Engine::open`]. Best-effort per record: a single
    /// adopt/persist failure does not abort the whole reconcile; it leaves that
    /// record for the next open. Returns the number of processes adopted (for
    /// diagnostics/tests).
    pub fn adopt_orphans(&mut self, registry: &Registry) -> usize {
        let records = match registry.list_spawn_records() {
            Ok(records) => records,
            Err(_) => return 0,
        };
        let mut adopted = 0;
        for record in records {
            let name = record.name.clone();
            // A pid-0 record is a policy-only config SEED (set via
            // `set_restart_policy` before the instance was ever started), NOT a
            // supervised process — skip it (it names no real process to adopt or
            // fail, and clearing it would wipe the persisted policy).
            if record.fingerprint.pid == 0 {
                continue;
            }
            match self.backend.adopt(&record.fingerprint) {
                Ok(Some(handle)) => {
                    // Live match: re-hold the handle. State stays as persisted
                    // (running/paused). AI-7: a paused process is now resumable.
                    self.running.insert(name, handle);
                    adopted += 1;
                }
                Ok(None) => {
                    // No live match — reconcile to `failed` HONESTLY (AI-8).
                    self.reconcile_orphan_failed(registry, &record);
                }
                Err(_) => {
                    // A backend adopt error is treated as "cannot confirm live" —
                    // reconcile to failed rather than leave a phantom row (AI-8).
                    self.reconcile_orphan_failed(registry, &record);
                }
            }
        }
        adopted
    }

    /// Reconcile a non-adopted orphan record to `failed` (AI-8): the process is
    /// gone (or unconfirmable), so a persisted `running`/`paused` row must NOT be
    /// left implying supervision that does not exist. Records a `Crashed` cause
    /// naming the orphan + the last-known cause, then clears the record. If the
    /// current state is already terminal (`failed`/`stopped`) we only clear the
    /// stale record. Best-effort — a persist failure leaves the record for the
    /// next open.
    fn reconcile_orphan_failed(&self, registry: &Registry, record: &SpawnRecord) {
        let name = &record.name;
        let state = match registry.lookup(name) {
            Ok(inst) => inst.state,
            Err(_) => {
                // Row gone — just drop the stale record.
                let _ = registry.clear_spawn_record(name);
                return;
            }
        };
        if matches!(state, LifecycleState::Running | LifecycleState::Paused) {
            let last = record
                .last_known_cause
                .as_deref()
                .unwrap_or("no prior cause recorded");
            let detail = format!(
                "orphan not found on engine restart (process pid {} is gone or was reused); \
                 last known: {last}",
                record.fingerprint.pid,
            );
            let _ = self.ensure_log_dir(registry, name);
            let _ = self.transition(
                registry,
                name,
                state,
                LifecycleState::Failed,
                TransitionCause::crashed(detail),
            );
        }
        // Clear the stale record either way (its process is gone).
        let _ = registry.clear_spawn_record(name);
    }

    // ---- internals ----

    /// Apply one transition: persist the new state, then append the event to the
    /// per-instance log. Persist-before-log so the durable state leads; a log
    /// append failure surfaces (the escalation record is load-bearing for AC3).
    fn transition(
        &self,
        registry: &Registry,
        name: &InstanceName,
        prior: LifecycleState,
        new: LifecycleState,
        cause: TransitionCause,
    ) -> Result<TransitionEvent, EngineError> {
        registry.set_state(name, new).map_err(registry_to_engine)?;
        let event = TransitionEvent::new(name.as_str(), prior, new, cause, now_rfc3339());
        append_event(&registry.instance_log_path(name), &event).map_err(|detail| {
            EngineError::Log {
                name: name.as_str().to_string(),
                path: registry
                    .instance_log_path(name)
                    .to_string_lossy()
                    .into_owned(),
                detail,
            }
        })?;
        Ok(event)
    }

    /// Land a spawn failure in `failed` with the backend diagnostic preserved
    /// (AC2), returning the [`EngineError::LaunchFailed`] to surface.
    fn fail_launch(
        &self,
        registry: &Registry,
        name: &InstanceName,
        err: &BackendError,
    ) -> EngineError {
        self.fail_launch_detail(registry, name, err.to_string())
    }

    /// Land a launch failure in `failed` with `detail` preserved (AC2).
    ///
    /// Records the `starting → failed` transition (cause = launch-error, detail
    /// verbatim) and returns [`EngineError::LaunchFailed`]. If persisting the
    /// failed state itself errors, that store error is surfaced instead (it is
    /// the more fundamental problem).
    fn fail_launch_detail(
        &self,
        registry: &Registry,
        name: &InstanceName,
        detail: String,
    ) -> EngineError {
        if let Err(e) = self.transition(
            registry,
            name,
            LifecycleState::Starting,
            LifecycleState::Failed,
            TransitionCause::launch_error(detail.clone()),
        ) {
            return e;
        }
        EngineError::LaunchFailed {
            name: name.as_str().to_string(),
            detail,
        }
    }

    /// Watch a freshly spawned process for [`READINESS_WINDOW`]. Returns
    /// `Some(exit_code)` if the process died within the window (a launch failure,
    /// AC2 — the inner `Option<i32>` is the OS exit code, `None` if killed by a
    /// signal with no code), or `None` if it stayed alive the whole window
    /// (ready). Reaps on exit (no zombie).
    fn watch_startup(&self, handle: &mut backends::Handle) -> Option<Option<i32>> {
        let deadline = std::time::Instant::now() + READINESS_WINDOW;
        loop {
            match self.backend.poll(handle) {
                Ok(ProcessStatus::Exited { code }) => return Some(code),
                Ok(ProcessStatus::Alive) => {}
                // A poll error during startup is treated as still-alive; the next
                // stop/poll will surface a real problem. Don't fail the start on a
                // transient poll hiccup.
                Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(READINESS_POLL);
        }
    }

    /// Ensure the per-instance log directory exists (AD-12 seed).
    fn ensure_log_dir(&self, registry: &Registry, name: &InstanceName) -> Result<(), EngineError> {
        let dir = registry.instance_log_dir(name);
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Log {
            name: name.as_str().to_string(),
            path: dir.to_string_lossy().into_owned(),
            detail: e.to_string(),
        })
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

/// Append one transition event as a single JSON line to the instance log.
///
/// One event per line (JSON Lines) so [`read_events_from`] can parse them back
/// and a human can `tail` the file. Append-only (AD-12 seed; rotation/attach are
/// Epic 4).
fn append_event(path: &Path, event: &TransitionEvent) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// Read back the JSON-Lines transition events from an instance log.
///
/// Missing file → empty vec (no events recorded yet). A malformed line is an
/// error naming it (a corrupt log is worth surfacing).
fn read_events_from(path: &Path) -> Result<Vec<TransitionEvent>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut events = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: TransitionEvent = serde_json::from_str(line)
            .map_err(|e| format!("corrupt instance-log line {}: {e}", idx + 1))?;
        events.push(event);
    }
    Ok(events)
}

/// Map a registry lookup/persist error into the lifecycle [`EngineError`].
///
/// Registration and lifecycle share the same NotFound/InvalidName shapes; keep
/// them as the lifecycle variants so `kt` maps them consistently. Exposed
/// `pub(crate)` so the engine facade's status read (story 1-6, AC9) maps registry
/// errors the same way the supervisor does.
pub(crate) fn registry_to_engine(err: super::error::RegistryError) -> EngineError {
    use super::error::RegistryError as R;
    match err {
        R::NotFound { name } => EngineError::NotFound { name },
        R::InvalidName { name, reason } => EngineError::InvalidName { name, reason },
        R::Io { name, path, source } => EngineError::Log {
            name,
            path,
            detail: source.to_string(),
        },
        R::Store(inner) => EngineError::Store(inner),
        // Any other registry error surfaces as an adapter-unresolved detail
        // (e.g. a missing/corrupt snapshot the supervisor needs to launch).
        other => EngineError::AdapterUnresolved {
            name: "<unknown>".to_string(),
            detail: other.to_string(),
        },
    }
}

/// Map a launch-spec resolution failure into the lifecycle [`EngineError`].
fn launch_to_engine(name: &InstanceName, err: LaunchResolveError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map a config-resolution failure (story 2-2) encountered while mapping the
/// resolved unified config into the launch into the lifecycle [`EngineError`]. A
/// malformed config layer / missing instance surfaces as an unresolved-adapter
/// launch failure (the config could not be mapped into the launch), naming the
/// instance + detail; the start rejects BEFORE any state change (mirrors a bad
/// manifest).
fn config_to_engine(name: &InstanceName, err: crate::domain::ConfigError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map a config-mapping APPLICATION failure (story 2-2) — a FILE target that
/// could not be rendered into the Agent Home — into the lifecycle [`EngineError`].
/// Surfaces as an unresolved-adapter launch failure naming the instance + detail;
/// the start rejects before any state change (the file write happens before the
/// `starting` transition), so a bad file target never leaves a spurious state.
fn config_apply_to_engine(name: &InstanceName, err: ConfigApplyError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map an effective-config SNAPSHOT-write failure (story 2-3) into the lifecycle
/// [`EngineError`]. The snapshot write lands BEFORE the `starting` transition, so
/// a failure here rejects the start with no state change. A
/// [`RegistryError::SnapshotWrite`] already carries the instance + snapshot path +
/// detail; map it to the dedicated [`EngineError::Snapshot`] naming the same, so
/// `kt` renders a precise "could not write the effective-config snapshot"
/// diagnostic with a permissions/disk remediation (NFR-1). Any other registry
/// error (not expected from this call) falls back to the shared registry mapper.
fn snapshot_to_engine(err: super::error::RegistryError) -> EngineError {
    match err {
        super::error::RegistryError::SnapshotWrite { name, path, detail } => {
            EngineError::Snapshot { name, path, detail }
        }
        other => registry_to_engine(other),
    }
}

/// Map a SECRET-resolution failure (story 2-4) into the lifecycle [`EngineError`].
/// The resolution runs BEFORE the config mapping + the `starting` transition, so a
/// failure here rejects the start with no state change (mirroring
/// [`snapshot_to_engine`]). The [`SecretError`] message names the `NAME` + the
/// resolvers tried (or the `chmod 600` remediation) but NEVER a resolved value, so
/// mapping it into [`EngineError::Secret`]'s `detail` cannot leak a secret (AC-B).
fn secret_to_engine(name: &InstanceName, err: crate::ports::SecretError) -> EngineError {
    EngineError::Secret {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterRef, StartLaunch};
    use crate::domain::RestartPolicy;
    use std::time::Instant;

    /// A fast backoff schedule so the crash/restart/crash-loop lib tests never
    /// sleep for real seconds (production stays 1s×2 cap 60s — Task 2 guards it).
    fn fast_backoff() -> BackoffSchedule {
        BackoffSchedule::with_base_and_cap(Duration::from_millis(5), Duration::from_millis(20))
    }

    /// Write a manifest whose `[lifecycle.start]` exec is `fake_agent` + `args`.
    fn write_fake_manifest(dir: &Path, kind: &str, args: &[&str]) {
        let bin = ktesio_conformance::fake_agent_bin();
        let args_toml = args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "contract_version = \"0.1.0\"\n\n\
             [adapter]\nkind = \"{kind}\"\n\n\
             [lifecycle.start]\nexec = {exec:?}\nargs = [{args_toml}]\n\n\
             [capabilities.interaction]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"guaranteed\"\n\n\
             [metering]\nsource = \"self-reported\"\n",
            exec = bin.to_string_lossy(),
        );
        std::fs::write(dir.join("adapter.toml"), body).unwrap();
    }

    /// Register a `fake_agent`-backed instance under `name` with `args`, in a
    /// fresh state dir. Returns the (state dir, manifest dir, registry).
    fn setup_fake(name: &str, args: &[&str]) -> (tempfile::TempDir, tempfile::TempDir, Registry) {
        let state = tempfile::tempdir().unwrap();
        let manifest = tempfile::tempdir().unwrap();
        write_fake_manifest(manifest.path(), name, args);
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry
            .register_with_adapter(name, &AdapterRef::Manifest(manifest.path().to_path_buf()))
            .unwrap();
        (state, manifest, registry)
    }

    /// Story 2-2: write a `fake_agent` manifest with `args` PLUS a `[config]`
    /// mapping section (`config_toml` is the section body, e.g.
    /// `"[config.model]\nflag = \"--model\"\n"`). Used by the manifest end-to-end
    /// mapping proofs.
    fn write_fake_manifest_with_config(dir: &Path, kind: &str, args: &[&str], config_toml: &str) {
        let bin = ktesio_conformance::fake_agent_bin();
        let args_toml = args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "contract_version = \"0.1.0\"\n\n\
             [adapter]\nkind = \"{kind}\"\n\n\
             [lifecycle.start]\nexec = {exec:?}\nargs = [{args_toml}]\n\n\
             [capabilities.interaction]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"guaranteed\"\n\n\
             [metering]\nsource = \"self-reported\"\n\n\
             {config_toml}",
            exec = bin.to_string_lossy(),
        );
        std::fs::write(dir.join("adapter.toml"), body).unwrap();
    }

    /// Register a `fake_agent`-backed instance carrying a `[config]` mapping.
    /// Returns the (state dir, manifest dir, registry).
    fn setup_fake_with_config(
        name: &str,
        args: &[&str],
        config_toml: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, Registry) {
        let state = tempfile::tempdir().unwrap();
        let manifest = tempfile::tempdir().unwrap();
        write_fake_manifest_with_config(manifest.path(), name, args, config_toml);
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry
            .register_with_adapter(name, &AdapterRef::Manifest(manifest.path().to_path_buf()))
            .unwrap();
        (state, manifest, registry)
    }

    /// Poll for a `--dump` file to appear (the spawned `fake_agent` writes it at
    /// startup) and return its contents, bounded — avoids racing the spawn.
    fn wait_for_dump(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            assert!(
                Instant::now() < deadline,
                "dump file never appeared at {path:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Poll until `poll_once` reports the crash (returns its plans), bounded.
    fn wait_for_crash(sup: &mut Supervisor, registry: &Registry) -> Vec<RestartPlan> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let plans = sup.poll_once(registry);
            // Once the instance has crashed it is no longer in `running`; the
            // crash transition has landed. `poll_once` returns the plan on the
            // pass that detects the exit.
            if !plans.is_empty() {
                return plans;
            }
            // Also stop once nothing is supervised AND state is failed (a `never`
            // policy returns no plan but still crashes).
            if sup.running.is_empty() {
                return plans;
            }
            assert!(Instant::now() < deadline, "crash was never detected");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn state_of(registry: &Registry, name: &str) -> LifecycleState {
        registry
            .lookup(&InstanceName::new(name).unwrap())
            .unwrap()
            .state
    }

    // ---- Story 2-2: unified→native config mapping proven at start (AC-A/AC-B) ----

    #[test]
    fn mock_native_start_maps_model_to_the_declared_env_target() {
        // AC-A + AC8 (the MOCK/native proof). The builtin `mock` is INERT (no live
        // process — NativeHasNoLaunch), so a `mock` start cannot spawn to observe.
        // Per the recorded inert-mock strategy (Decision 8), we assert on the
        // MAPPED launch the mapping application PRODUCES: register a mock, set the
        // documented `model` key (2-1), then resolve the mock's code-declared
        // mapping + the effective config and apply — the mock's declared native
        // target (env `MODEL`) must carry the value. This is exactly the transform
        // the start seam runs; a launchable native agent is a manifest adapter.
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry.register("mck", "mock").unwrap();
        let name = InstanceName::new("mck").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();

        // Resolve exactly as start_inner would for a native adapter.
        let (kind, manifest_path) = registry.adapter_launch_facts(&name).unwrap();
        assert_eq!(kind, "mock");
        assert!(manifest_path.is_none(), "mock is native (no manifest)");
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .unwrap();
        let mapping = adapter::resolve_config_mapping(&kind, manifest_path.as_deref()).unwrap();
        // The mock declares `model` → env `MODEL`.
        assert_eq!(mapping.target("model").unwrap().env_var(), Some("MODEL"));

        // Apply onto a bare launch (the mock has no [lifecycle.start] template;
        // this is the launch shape the mapping would produce).
        let mut launch = StartLaunch {
            exec: "mock".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
        };
        adapter::apply_config_mapping(
            &mut launch,
            &mapping,
            &effective,
            &std::collections::BTreeMap::new(),
            &registry.agent_home(&name),
        )
        .unwrap();
        assert_eq!(
            launch.env.get("MODEL").map(String::as_str),
            Some("gpt-4"),
            "the documented model key must land in the mock's declared env target"
        );
    }

    #[test]
    fn manifest_start_maps_model_to_the_declared_flag_target_live() {
        // AC-A + AC8 (the MANIFEST proof, live). A `fake_agent` manifest declares
        // `[config.model]` → flag `--model`; set model, start the REAL process
        // with `--dump`, and assert the mapped flag landed in the spawned
        // process's argv (observed via the dump file — no stdout race).
        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("argv.txt");
        let (_state, _manifest, registry) = setup_fake_with_config(
            "flg",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "[config.model]\nflag = \"--model\"\n",
        );
        let name = InstanceName::new("flg").unwrap();
        registry.set_config(&name, "model", "gpt-4o").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "flg").unwrap();
        assert_eq!(state_of(&registry, "flg"), LifecycleState::Running);

        // The spawned fake_agent dumped its argv; the mapped flag + value are there.
        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == "arg=--model"),
            "the mapped --model flag must reach the process argv; dump=\n{dumped}"
        );
        assert!(
            dumped.lines().any(|l| l == "arg=gpt-4o"),
            "the mapped model VALUE must reach the process argv; dump=\n{dumped}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "flg", Some(Duration::from_millis(200)));
    }

    #[test]
    fn secret_leaf_delivers_cleartext_to_the_adapter_but_masks_snapshot_and_events() {
        // Story 2-4 (AC-A/AC9 delivery + AC-B no-leak, engine level). A
        // `model = secret:NAME` leaf resolves (env resolver) to a sentinel; the
        // spawned agent's argv carries the CLEARTEXT (usable), while the persisted
        // snapshot AND every transition event carry the MASK, never the sentinel.
        // Uses a UNIQUE env-var name to avoid racing sibling in-process tests.
        const SENTINEL: &str = "s3cr3t-engine-sentinel-abc";
        let env_key = "KTESIO_SUP_SECRET_TEST_KEY";
        let prev = std::env::var_os(env_key);
        std::env::set_var(env_key, SENTINEL);

        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("argv.txt");
        let (_state, _manifest, registry) = setup_fake_with_config(
            "sekeng",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "[config.model]\nflag = \"--model\"\n",
        );
        let name = InstanceName::new("sekeng").unwrap();
        registry
            .set_config(&name, "model", &format!("secret:{env_key}"))
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "sekeng").unwrap();
        assert_eq!(state_of(&registry, "sekeng"), LifecycleState::Running);

        // (POSITIVE) the spawned process argv carries the resolved CLEARTEXT.
        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == format!("arg={SENTINEL}")),
            "the resolved secret cleartext must reach the process argv; dump=\n{dumped}"
        );

        // (NO-LEAK) the persisted snapshot masks the secret.
        let snapshot = std::fs::read_to_string(registry.paths().effective_config_snapshot(&name))
            .expect("snapshot written");
        assert!(
            !snapshot.contains(SENTINEL),
            "the snapshot leaked the secret:\n{snapshot}"
        );
        assert!(
            snapshot.contains("secret:****"),
            "snapshot must mask; {snapshot}"
        );

        // (NO-LEAK) no transition event payload carries the sentinel (AD-14).
        let events = Supervisor::read_events(&registry, "sekeng").unwrap();
        let events_json = serde_json::to_string(&events).unwrap();
        assert!(
            !events_json.contains(SENTINEL),
            "a transition event leaked the secret:\n{events_json}"
        );

        // Teardown + restore env.
        let _ = sup.stop(&registry, "sekeng", Some(Duration::from_millis(200)));
        match prev {
            Some(v) => std::env::set_var(env_key, v),
            None => std::env::remove_var(env_key),
        }
    }

    #[test]
    fn unresolved_secret_rejects_start_before_any_state_change() {
        // Story 2-4 (AC5/AC9, engine level): a `secret:NAME` unresolved by env AND
        // the (absent) secrets file rejects the start with a typed EngineError::Secret
        // that NEVER echoes a value, leaving the instance in its PRIOR state and NO
        // snapshot written. The env var is deliberately unset.
        let env_key = "KTESIO_SUP_DEFINITELY_UNSET_SECRET_KEY_XYZ";
        std::env::remove_var(env_key);
        let (_state, _manifest, registry) = setup_fake("noresolve_eng", &["--linger-ms", "600000"]);
        let name = InstanceName::new("noresolve_eng").unwrap();
        registry
            .set_config(&name, "model", &format!("secret:{env_key}"))
            .unwrap();
        let prior = state_of(&registry, "noresolve_eng");

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "noresolve_eng").unwrap_err();
        match &err {
            EngineError::Secret { name: n, detail } => {
                assert_eq!(n, "noresolve_eng");
                // Names the NAME + resolvers, NEVER a value.
                assert!(detail.contains(env_key), "detail must name NAME; {detail}");
            }
            other => panic!("expected EngineError::Secret, got {other:?}"),
        }
        // Prior state preserved; no snapshot written (rejected before both).
        assert_eq!(state_of(&registry, "noresolve_eng"), prior);
        assert!(
            !registry.paths().effective_config_snapshot(&name).exists(),
            "an unresolved secret must reject before the snapshot write"
        );
    }

    #[test]
    fn manifest_start_maps_model_to_the_declared_file_target_live() {
        // AC-A + AC4 (the MANIFEST FILE proof, live). A `fake_agent` manifest
        // declares `[config.model]` → a file target; set model, start, and assert
        // the engine RENDERED the native config file into the Agent Home at the
        // declared native key (the engine is the sole writer — path authority).
        let (_state, _manifest, registry) = setup_fake_with_config(
            "fil",
            &["--linger-ms", "600000"],
            "[config.model]\nfile = { path = \"config/agent.toml\", key = \"llm.model\" }\n",
        );
        let name = InstanceName::new("fil").unwrap();
        registry.set_config(&name, "model", "claude-opus").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "fil").unwrap();
        assert_eq!(state_of(&registry, "fil"), LifecycleState::Running);

        // The engine rendered the native config file into the Agent Home.
        let rendered = registry.agent_home(&name).join("config/agent.toml");
        assert!(
            rendered.is_file(),
            "the file target must render into the home"
        );
        let parsed: toml::Table = std::fs::read_to_string(&rendered).unwrap().parse().unwrap();
        assert_eq!(
            parsed["llm"]["model"].as_str(),
            Some("claude-opus"),
            "the documented model key must land at the declared native key path"
        );
        // Teardown.
        let _ = sup.stop(&registry, "fil", Some(Duration::from_millis(200)));
    }

    #[test]
    fn manifest_start_delivers_agent_pass_through_verbatim_live() {
        // AC-B (the `agent.*` verbatim proof, live). Set an `agent.*` pass-through
        // key, start the REAL fake_agent with `--dump`, and assert the value was
        // delivered VERBATIM into the native mechanism (an env var named by the
        // verbatim key-tail) — no rewriting, no known-key mapping.
        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("env.txt");
        // No [config] mapping at all — pass-through does not need one (AC6).
        let (_state, _manifest, registry) = setup_fake_with_config(
            "pth",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "",
        );
        let name = InstanceName::new("pth").unwrap();
        registry
            .set_config(&name, "agent.CUSTOM_TOKEN", "verbatim-xyz")
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "pth").unwrap();
        assert_eq!(state_of(&registry, "pth"), LifecycleState::Running);

        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == "env=CUSTOM_TOKEN=verbatim-xyz"),
            "the agent.* value must be delivered verbatim into the native env; dump=\n{dumped}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "pth", Some(Duration::from_millis(200)));
    }

    // ---- Story 2-3: the persisted effective-config snapshot at start (AC5/AC6/AC7) ----

    /// Parse the persisted effective-config snapshot for `name` and return the
    /// entry map (key → (rendered value, source label)). Panics if the file is
    /// missing/unparseable (the test wants it present).
    fn read_snapshot_entries(
        registry: &Registry,
        name: &str,
    ) -> std::collections::BTreeMap<String, (String, String)> {
        let path = registry
            .paths()
            .effective_config_snapshot(&InstanceName::new(name).unwrap());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["key"].as_str().unwrap().to_string(),
                    (
                        e["value"].as_str().unwrap().to_string(),
                        e["source"].as_str().unwrap().to_string(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn start_writes_the_effective_config_snapshot_tagged_with_source() {
        // AC5 (AD-9/AD-6): starting an instance writes the effective-config
        // snapshot FILE into the Agent Home at EnginePaths::effective_config_snapshot,
        // and it parses + carries model=<v> tagged `instance`. Register a live
        // fake_agent, set model, start it, assert the snapshot.
        let (_state, _manifest, registry) = setup_fake("snp", &["--linger-ms", "600000"]);
        let name = InstanceName::new("snp").unwrap();
        registry.set_config(&name, "model", "gpt-4o").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "snp").unwrap();
        assert_eq!(state_of(&registry, "snp"), LifecycleState::Running);

        let path = registry.paths().effective_config_snapshot(&name);
        assert!(path.is_file(), "the snapshot must exist at {path:?}");
        let entries = read_snapshot_entries(&registry, "snp");
        assert_eq!(
            entries.get("model"),
            Some(&("gpt-4o".to_string(), "instance".to_string())),
            "model must be present tagged `instance`; entries={entries:?}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "snp", Some(Duration::from_millis(200)));
    }

    #[test]
    fn restart_via_start_inner_overwrites_the_snapshot_with_the_new_value() {
        // AC7: the snapshot is OVERWRITTEN on every start — a re-start (which flows
        // through the SAME start_inner seam, story 1-6) refreshes it with the newly
        // resolved value, never a stale earlier resolution. Start, stop, change the
        // value, start again; the snapshot reflects the LATEST value.
        let (_state, _manifest, registry) = setup_fake("rsn", &["--linger-ms", "600000"]);
        let name = InstanceName::new("rsn").unwrap();
        registry.set_config(&name, "model", "first").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "rsn").unwrap();
        assert_eq!(
            read_snapshot_entries(&registry, "rsn").get("model"),
            Some(&("first".to_string(), "instance".to_string()))
        );
        sup.stop(&registry, "rsn", Some(Duration::from_millis(200)))
            .unwrap();

        // Change the value and start again (stopped → starting → running via
        // start_inner). The snapshot must be overwritten with the new value.
        registry.set_config(&name, "model", "second").unwrap();
        sup.start(&registry, "rsn").unwrap();
        assert_eq!(state_of(&registry, "rsn"), LifecycleState::Running);
        assert_eq!(
            read_snapshot_entries(&registry, "rsn").get("model"),
            Some(&("second".to_string(), "instance".to_string())),
            "the snapshot must reflect the latest resolved value after re-start (AC7)"
        );
        // Teardown.
        let _ = sup.stop(&registry, "rsn", Some(Duration::from_millis(200)));
    }

    #[test]
    fn snapshot_write_failure_rejects_the_start_before_the_starting_transition() {
        // AC6: a snapshot-write failure rejects the start with NO state change (the
        // write lands before the `starting` transition). Force the write to fail by
        // making the snapshot path a DIRECTORY, then assert start errors and the
        // instance stays in its prior state (`registered`), with NO agent spawned.
        let (_state, _manifest, registry) = setup_fake("bad", &["--linger-ms", "600000"]);
        let name = InstanceName::new("bad").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();
        // A directory where the snapshot file must be → std::fs::write fails.
        let snap_path = registry.paths().effective_config_snapshot(&name);
        std::fs::create_dir(&snap_path).unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "bad").unwrap_err();
        assert!(
            matches!(&err, EngineError::Snapshot { name, .. } if name == "bad"),
            "expected a typed Snapshot error, got {err:?}"
        );
        // The instance stayed in its prior state — the start was rejected cleanly
        // BEFORE the `starting` transition (no spurious state change, AC6).
        assert_eq!(state_of(&registry, "bad"), LifecycleState::Registered);
    }

    #[test]
    fn snapshot_to_engine_maps_snapshot_write_and_falls_back_for_others() {
        // Unit-cover the snapshot error mapper: a SnapshotWrite maps to the
        // dedicated EngineError::Snapshot naming the instance + path; any other
        // registry error falls back to the shared registry mapper (NotFound here).
        let mapped = snapshot_to_engine(crate::domain::RegistryError::SnapshotWrite {
            name: "demo".into(),
            path: "/x/agents/demo/effective-config.json".into(),
            detail: "disk full".into(),
        });
        match mapped {
            EngineError::Snapshot { name, path, detail } => {
                assert_eq!(name, "demo");
                assert!(path.ends_with("effective-config.json"), "path={path}");
                assert_eq!(detail, "disk full");
            }
            other => panic!("expected Snapshot, got {other:?}"),
        }
        // Fallback: a non-snapshot registry error goes through registry_to_engine.
        let fallback = snapshot_to_engine(crate::domain::RegistryError::NotFound {
            name: "demo".into(),
        });
        assert!(matches!(fallback, EngineError::NotFound { name } if name == "demo"));
    }

    #[test]
    fn crash_of_a_never_policy_instance_lands_failed_no_restart() {
        // AC-A / AC5: a `never`-policy instance that crashes lands `failed` with a
        // `crashed` cause + NO restart. Set policy=never, start a crash-after
        // agent, poll until the crash is detected, assert failed + no plan.
        let (_state, _manifest, registry) = setup_fake("nevr", &["--crash-after-ms", "450"]);
        registry
            .set_restart_policy(&InstanceName::new("nevr").unwrap(), RestartPolicy::Never)
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "nevr").unwrap();
        assert_eq!(state_of(&registry, "nevr"), LifecycleState::Running);

        let plans = wait_for_crash(&mut sup, &registry);
        assert!(plans.is_empty(), "never policy must NOT schedule a restart");
        assert_eq!(state_of(&registry, "nevr"), LifecycleState::Failed);

        // The crash was recorded with a `crashed` cause.
        let events = Supervisor::read_events(&registry, "nevr").unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.new_state, LifecycleState::Failed);
        let cause = serde_json::to_string(&last.cause).unwrap();
        assert!(cause.contains("crashed"), "cause={cause}");
    }

    #[test]
    fn on_failure_crash_restarts_increments_count_then_a_clean_run_resets() {
        // AC-A / AC4: an `on-failure` instance that crashes is restarted, the
        // restart count increments, the `failed→starting`… restart event records
        // the backoff, and a subsequent CLEAN start resets the count to 0. Uses
        // the injected fast backoff so no real seconds elapse.
        let (_state, _manifest, registry) = setup_fake("recov", &["--crash-after-ms", "450"]);
        // Default policy is on-failure; be explicit.
        registry
            .set_restart_policy(
                &InstanceName::new("recov").unwrap(),
                RestartPolicy::OnFailure,
            )
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "recov").unwrap();

        // Detect the crash → a restart plan for attempt 1.
        let plans = wait_for_crash(&mut sup, &registry);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].attempt, 1);
        assert_eq!(state_of(&registry, "recov"), LifecycleState::Failed);

        // Perform the restart after its (fast) backoff. The instance is running
        // again and the record shows restart_count == 1.
        std::thread::sleep(plans[0].delay);
        sup.restart(&registry, "recov", plans[0].attempt, plans[0].delay)
            .unwrap();
        assert_eq!(state_of(&registry, "recov"), LifecycleState::Running);
        let rec = registry
            .spawn_record(&InstanceName::new("recov").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_count, 1);

        // The restart event recorded the count + waited backoff.
        let events = Supervisor::read_events(&registry, "recov").unwrap();
        let restart_evt = events
            .iter()
            .find(|e| matches!(e.cause, TransitionCause::Restarted { .. }))
            .expect("a restart event must be recorded");
        match &restart_evt.cause {
            TransitionCause::Restarted { count, .. } => assert_eq!(*count, 1),
            _ => unreachable!(),
        }

        // A CLEAN stop then start resets the consecutive count to 0 (AC4).
        sup.stop(&registry, "recov", Some(Duration::from_millis(200)))
            .unwrap();
        sup.start(&registry, "recov").unwrap();
        let rec = registry
            .spawn_record(&InstanceName::new("recov").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_count, 0, "a fresh start resets the count");
        // Teardown.
        let _ = sup.stop(&registry, "recov", Some(Duration::from_millis(200)));
    }

    #[test]
    fn crash_loop_stops_after_exactly_five_consecutive_failures() {
        // AC4: an instance that crashes immediately every restart stops after
        // EXACTLY 5 consecutive failures, left `failed` with the crash-loop
        // reason. Drive the crash→restart cycle manually with the fast backoff;
        // the 5th restart's crash yields NO further plan (crash-loop), and the
        // recorded cause states the crash loop.
        let (_state, _manifest, registry) = setup_fake("loopy", &["--crash-after-ms", "400"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "loopy").unwrap();

        let mut last_attempt = 0;
        // Up to 5 restarts, each preceded by a detected crash.
        for _ in 0..6 {
            let plans = wait_for_crash(&mut sup, &registry);
            assert_eq!(state_of(&registry, "loopy"), LifecycleState::Failed);
            if plans.is_empty() {
                // Crash-loop reached: no more restarts scheduled.
                break;
            }
            assert_eq!(plans.len(), 1);
            last_attempt = plans[0].attempt;
            std::thread::sleep(plans[0].delay);
            // The restart re-launches; it will crash again on the next poll.
            sup.restart(&registry, "loopy", plans[0].attempt, plans[0].delay)
                .unwrap();
        }

        // The last scheduled attempt was the 4th → the 5th crash trips the loop
        // (is_crash_loop(5) == true), so no 5th restart plan is issued.
        assert_eq!(
            last_attempt, 4,
            "the last restart plan should be attempt 4 (the 5th crash trips the loop)"
        );
        assert_eq!(state_of(&registry, "loopy"), LifecycleState::Failed);

        // F-Low-2: the crash-loop is a TERMINAL path, so the record's LIVE
        // fingerprint is dropped (settled to a pid-0 seed) — a later open will
        // NOT adopt-attempt the dead PID (the reconcile skips pid-0). The policy
        // is re-seeded so `show` can still report it (AC9).
        let rec = registry
            .spawn_record(&InstanceName::new("loopy").unwrap())
            .unwrap()
            .expect("a policy seed is retained after the terminal crash-loop");
        assert_eq!(
            rec.fingerprint.pid, 0,
            "the terminal path must drop the live fingerprint (pid-0 seed, not adopt-attempted)"
        );
        assert_eq!(
            rec.restart_policy,
            RestartPolicy::OnFailure,
            "the policy is retained for AC9's show"
        );
        // The crash-loop REASON now rides in the last `crashed` event's cause (so
        // `instance_status` surfaces it via its event-log fallback).
        let events = Supervisor::read_events(&registry, "loopy").unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.new_state, LifecycleState::Failed);
        let cause = serde_json::to_string(&last.cause).unwrap();
        assert!(
            cause.contains("crash-loop") && cause.contains("5 consecutive failures"),
            "the crash-loop reason must be in the event cause; cause={cause}"
        );
    }

    #[test]
    fn poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash() {
        // The reaper's "not a crash" branch: if the store shows the instance
        // `stopping` (an operator stop in flight) when its process exits,
        // poll_once must NOT apply a `failed` crash transition — it just drops the
        // dead handle. Start an instance, mark it `stopping` in the store, let it
        // crash, and assert poll_once returns no plans and does NOT move it to
        // `failed` (it stays `stopping`, the requested end state).
        let (_state, _manifest, registry) = setup_fake("stopping", &["--crash-after-ms", "450"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "stopping").unwrap();
        // Simulate an operator stop in flight (row → stopping) while the handle
        // is still held.
        registry
            .set_state(
                &InstanceName::new("stopping").unwrap(),
                LifecycleState::Stopping,
            )
            .unwrap();
        // Wait for the process to actually exit, then poll.
        std::thread::sleep(Duration::from_millis(700));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let plans = sup.poll_once(&registry);
            assert!(
                plans.is_empty(),
                "an exit during a requested stop is not a crash"
            );
            if sup.running.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "handle should be dropped after exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The state is NOT `failed` (no crash transition was applied); it stays
        // `stopping` (the requested end state the operator stop will finalize).
        assert_eq!(state_of(&registry, "stopping"), LifecycleState::Stopping);
    }

    #[test]
    fn poll_once_with_no_handles_is_a_noop() {
        // The empty-reaper path: with nothing supervised, poll_once returns no
        // plans and touches nothing.
        let (_state, _manifest, registry) = setup_fake("idle", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert!(sup.poll_once(&registry).is_empty());
    }

    #[test]
    fn adopt_orphans_with_no_records_adopts_nothing() {
        // The empty-reconcile path: with no persisted spawn records, adoption
        // adopts nothing and returns 0.
        let (_state, _manifest, registry) = setup_fake("idle", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert_eq!(sup.adopt_orphans(&registry), 0);
        assert!(sup.running.is_empty());
    }

    #[test]
    fn adopt_orphans_skips_a_policy_only_seed_record() {
        // A pid-0 record is a policy-only config seed (set before any start), NOT
        // a supervised process — adoption skips it (adopts nothing) and does NOT
        // reconcile the registered instance to failed or clear its policy.
        let (_state, _manifest, registry) = setup_fake("seedonly", &["--linger-ms", "600000"]);
        registry
            .set_restart_policy(
                &InstanceName::new("seedonly").unwrap(),
                RestartPolicy::Never,
            )
            .unwrap();
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert_eq!(sup.adopt_orphans(&registry), 0);
        // Still registered; the policy seed survives (was not cleared).
        assert_eq!(state_of(&registry, "seedonly"), LifecycleState::Registered);
        let rec = registry
            .spawn_record(&InstanceName::new("seedonly").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_policy, RestartPolicy::Never);
    }

    #[test]
    fn default_stop_window_is_30s() {
        assert_eq!(DEFAULT_STOP_WINDOW, Duration::from_secs(30));
    }

    #[test]
    fn read_events_from_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.log");
        assert!(read_events_from(&path).unwrap().is_empty());
    }

    #[test]
    fn append_then_read_round_trips_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        let e1 = TransitionEvent::new(
            "demo",
            LifecycleState::Registered,
            LifecycleState::Starting,
            TransitionCause::command("start"),
            "2026-07-04T00:00:00Z",
        );
        let e2 = TransitionEvent::new(
            "demo",
            LifecycleState::Starting,
            LifecycleState::Running,
            TransitionCause::AdapterReady,
            "2026-07-04T00:00:01Z",
        );
        append_event(&path, &e1).unwrap();
        append_event(&path, &e2).unwrap();
        let back = read_events_from(&path).unwrap();
        assert_eq!(back, vec![e1, e2]);
    }

    #[test]
    fn read_events_rejects_a_corrupt_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        std::fs::write(&path, "{ not valid json\n").unwrap();
        let err = read_events_from(&path).unwrap_err();
        assert!(err.contains("corrupt instance-log line 1"), "{err}");
    }

    #[test]
    fn supervisor_constructs_empty() {
        let sup = Supervisor::new();
        assert!(sup.running.is_empty());
    }

    #[test]
    fn registry_to_engine_maps_each_variant() {
        use super::super::error::RegistryError as R;
        use super::super::name::NameError;

        // NotFound → NotFound.
        assert!(matches!(
            registry_to_engine(R::NotFound { name: "x".into() }),
            EngineError::NotFound { .. }
        ));
        // InvalidName → InvalidName.
        assert!(matches!(
            registry_to_engine(R::InvalidName {
                name: "X".into(),
                reason: NameError::BadChar,
            }),
            EngineError::InvalidName { .. }
        ));
        // Io → Log (naming the path).
        assert!(matches!(
            registry_to_engine(R::Io {
                name: "x".into(),
                path: "/p".into(),
                source: std::io::Error::other("boom"),
            }),
            EngineError::Log { .. }
        ));
        // A snapshot-shaped registry error → AdapterUnresolved.
        assert!(matches!(
            registry_to_engine(R::ManifestNotFound { path: "/m".into() }),
            EngineError::AdapterUnresolved { .. }
        ));
    }

    #[test]
    fn launch_to_engine_wraps_as_adapter_unresolved() {
        let name = InstanceName::new("svc").unwrap();
        let err = launch_to_engine(
            &name,
            LaunchResolveError::NativeHasNoLaunch {
                kind: "mock".into(),
            },
        );
        match err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("no launch command"));
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
    }

    #[test]
    fn config_to_engine_and_config_apply_to_engine_wrap_as_adapter_unresolved() {
        // Story 2-2: a config-resolution failure and a config-apply (file-render)
        // failure both surface as an unresolved-adapter launch failure naming the
        // instance + preserving the detail, so `start` rejects cleanly.
        let name = InstanceName::new("svc").unwrap();
        let cfg_err = config_to_engine(
            &name,
            crate::domain::ConfigError::NotFound { name: "svc".into() },
        );
        match cfg_err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("svc"), "detail preserved: {detail}");
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
        let apply_err = config_apply_to_engine(
            &name,
            ConfigApplyError::FileRender {
                key: "config/agent.toml".into(),
                path: "config/agent.toml".into(),
                detail: "disk full".into(),
            },
        );
        match apply_err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("config/agent.toml"), "{detail}");
                assert!(detail.contains("disk full"), "{detail}");
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
    }

    #[test]
    fn start_with_an_unwritable_file_target_rejects_before_any_state_change() {
        // Story 2-2 end-to-end error path (accurate atomicity, Fix #5): a manifest
        // `[config.model]` FILE target whose parent path is blocked (a regular file
        // sits where the config directory must be in the Agent Home) fails the
        // config-mapping application at start. Because the mapping is applied BEFORE
        // the `starting` transition, the start REJECTS (AdapterUnresolved) and the
        // instance stays in its PRIOR state (`registered`) — it does NOT land
        // `failed`, and never reaches `running`. Exercises the start_inner
        // config-apply error branch + config_apply_to_engine.
        let (_state, _manifest, registry) = setup_fake_with_config(
            "badfile",
            &["--linger-ms", "600000"],
            "[config.model]\nfile = { path = \"blocked/agent.toml\", key = \"k\" }\n",
        );
        let name = InstanceName::new("badfile").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();
        // Block the file target's parent: put a regular FILE at <home>/blocked so
        // create_dir_all(<home>/blocked) fails when rendering blocked/agent.toml.
        let home = registry.agent_home(&name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("blocked"), b"not a dir").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "badfile").unwrap_err();
        assert!(
            matches!(err, EngineError::AdapterUnresolved { .. }),
            "a bad file target must fail the start; got {err}"
        );
        // Never reached running (the failure was before the starting transition, so
        // the instance stays registered).
        assert_eq!(state_of(&registry, "badfile"), LifecycleState::Registered);
    }

    #[test]
    fn read_events_skips_blank_lines() {
        // Blank lines in the log are ignored (only JSON event lines are parsed).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        let e = TransitionEvent::new(
            "demo",
            LifecycleState::Registered,
            LifecycleState::Starting,
            TransitionCause::command("start"),
            "2026-07-04T00:00:00Z",
        );
        let line = serde_json::to_string(&e).unwrap();
        std::fs::write(&path, format!("\n{line}\n\n")).unwrap();
        let back = read_events_from(&path).unwrap();
        assert_eq!(back, vec![e]);
    }
}
