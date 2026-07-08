//! The async engine handle + its blocking facade (spine AD-13).
//!
//! [`Engine`] is the async-first Embedding Interface. It owns a multi-thread
//! tokio [`Runtime`](tokio::runtime::Runtime), the registration [`Registry`]
//! (an internal collaborator), and the in-memory lifecycle [`Supervisor`]. Its
//! public methods are `async`; the blocking DB/FS work runs on tokio's blocking
//! pool via [`tokio::task::spawn_blocking`] (rusqlite is a synchronous C binding
//! that must never run on an async worker — AD-13, Approach A).
//!
//! ## The `blocking()` facade (FR-34 / FR-31 / story 7-3 seed)
//!
//! Sync callers — `kt` today — drive the engine through [`Engine::blocking`],
//! which returns a [`Blocking`] view whose methods are the sync equivalents,
//! each `runtime.block_on(async_method(..))`. `kt` stays a synchronous binary
//! (no `#[tokio::main]`); a Host with its OWN runtime (story 7-1/7-3) calls the
//! async methods directly. This story covers exactly the commands `kt` uses
//! today (register / remove / list / effective-capabilities) plus `start` /
//! `stop`; the FULL facade + the embedding proof are 7-1/7-3.
//!
//! ## No global state, no ambient runtime (AD-13 forward contract)
//!
//! The engine takes its state-dir base explicitly (mirroring [`Registry::open`])
//! and owns its runtime; it never assumes an ambient runtime elsewhere and holds
//! no thread-locals or globals. This is what keeps the facade sound.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Runtime;

use ktesio_adapter_api::EffectiveCapabilities;

use crate::adapter::AdapterRef;
use crate::domain::{
    AgentInstance, EngineError, FleetEntry, InstanceName, LifecycleState, Registry, RegistryError,
    RemoveDisposition, RestartPolicy, Supervisor, TransitionCause, TransitionEvent,
};

/// How often the crash-detection reaper polls supervised processes (story 1-6,
/// `[ASSUMPTION]`). Small enough that a crash is detected promptly, large enough
/// to avoid busy-work; not spine-mandated. The engine owns this cadence and
/// calls the sync [`Supervisor::poll_once`] via `spawn_blocking`, keeping the
/// supervisor cfg-free + sync.
const CRASH_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The per-instance runtime status the CLI surfaces (story 1-6, AC9): the
/// current Lifecycle State, the effective Restart Policy, the restart count, and
/// (for a `failed` instance) the last-known failed cause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceStatus {
    /// The instance the status is for.
    pub instance: AgentInstance,
    /// The effective per-instance Restart Policy (AD-15).
    pub restart_policy: RestartPolicy,
    /// The consecutive-failure restart count (0 when never restarted / reset on a
    /// clean run).
    pub restart_count: u32,
    /// The last-known cause (e.g. the crash / crash-loop detail), if any. Present
    /// for a `failed` instance whose spawn record recorded a cause.
    pub failed_cause: Option<String>,
}

/// Extract a human-readable failed-cause detail from a transition [`TransitionCause`]
/// for the `instance_status` event-log fallback (AC9). Returns the carried detail
/// for the failure-bearing causes (a launch error, or a crash), or `None` for a
/// cause that does not describe a failure.
fn failed_cause_detail(cause: &TransitionCause) -> Option<String> {
    match cause {
        TransitionCause::LaunchError { detail } => Some(detail.clone()),
        TransitionCause::Crashed { detail } => Some(detail.clone()),
        _ => None,
    }
}

/// Map a [`Supervisor::stop`] failure encountered during [`Engine::remove`]'s
/// live-instance teardown (AI-11) into a [`RegistryError`], since `remove` speaks
/// the registry error type. A store failure passes through as
/// [`RegistryError::Store`]; any other stop failure (a backend terminate/signal
/// error, a log-write error, …) surfaces as a filesystem-shaped
/// [`RegistryError::Io`] naming the instance — so a teardown failure ABORTS the
/// remove (the row is NOT deleted and the process is NOT orphaned) with a
/// diagnostic the CLI already renders, rather than inventing a new public variant.
fn stop_error_to_registry(err: EngineError) -> RegistryError {
    match err {
        EngineError::Store(inner) => RegistryError::Store(inner),
        EngineError::InvalidName { name, reason } => RegistryError::InvalidName { name, reason },
        EngineError::NotFound { name } => RegistryError::NotFound { name },
        other => RegistryError::Io {
            name: "<remove-teardown>".to_string(),
            path: "<process teardown>".to_string(),
            source: std::io::Error::other(format!(
                "could not stop the live instance before removal: {other}"
            )),
        },
    }
}

/// The async engine handle (the Embedding Interface, AD-2/AD-13).
///
/// Constructed once per embedding via [`Engine::open`]. Owns the runtime, the
/// registry, and the supervisor for a single engine lifetime (cross-restart
/// orphan adoption is story 1-6).
pub struct Engine {
    /// The engine-owned multi-thread runtime the blocking facade drives.
    rt: Arc<Runtime>,
    /// Shared engine state (registry + supervisor), guarded for `spawn_blocking`.
    inner: Arc<EngineInner>,
    /// The crash-detection reaper task's abort handle (story 1-6). Aborted on
    /// [`Engine::drop`] so the background poll stops with the engine.
    reaper: tokio::task::AbortHandle,
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Stop the background reaper when the engine goes away (its supervised
        // handles drop with the supervisor, killing spawned processes).
        self.reaper.abort();
    }
}

/// Shared, `Send + Sync` engine state moved onto the blocking pool.
///
/// The [`Registry`] owns a rusqlite `Connection` (`Send` but `!Sync`); wrapping
/// it in a [`Mutex`] makes the whole `EngineInner` `Send + Sync` so a
/// `spawn_blocking` closure can capture an `Arc<EngineInner>`. Registration is
/// not a hot path and the engine is single-lifetime, so a coarse mutex is the
/// correct, simplest altitude here.
struct EngineInner {
    registry: Mutex<Registry>,
    supervisor: Mutex<Supervisor>,
}

impl Engine {
    /// Open an engine rooted at an optional state-dir base.
    ///
    /// `base` is threaded straight into [`Registry::open`] (see its docs for the
    /// resolution order). Builds the multi-thread tokio runtime the blocking
    /// facade owns and an empty in-memory supervisor, then:
    /// 1. ADOPTS ORPHANS (story 1-6, AC-B): reconciles every write-ahead spawn
    ///    record against live processes — a live fingerprint match is re-held
    ///    under supervision, a non-match is reconciled to `failed`, so no agent
    ///    process is left unsupervised and no phantom `running` row survives.
    /// 2. Spawns the crash-detection reaper (story 1-6, AC-A): a tokio interval
    ///    task that periodically runs [`Supervisor::poll_once`] via
    ///    `spawn_blocking` and times the Restart Policy backoffs.
    pub fn open(base: Option<PathBuf>) -> Result<Self, RegistryError> {
        let registry = Registry::open(base)?;
        let rt = Runtime::new().map_err(|e| RegistryError::Io {
            name: "<engine-runtime>".to_string(),
            path: "<tokio-runtime>".to_string(),
            source: e,
        })?;
        let inner = Arc::new(EngineInner {
            registry: Mutex::new(registry),
            supervisor: Mutex::new(Supervisor::new()),
        });

        // (1) Orphan adoption on open (AC-B / AI-7 / AI-8). Reconcile BEFORE the
        // reaper starts so an adopted process is already held when the first poll
        // runs (and a phantom row is already `failed`).
        {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            supervisor.adopt_orphans(&registry);
        }

        // (2) Spawn the crash-detection reaper on the engine's runtime. It is
        // aborted on Engine::drop. Cloning the Arc<EngineInner> keeps the shared
        // state alive for the task without keeping the Engine itself alive.
        let reaper = Self::spawn_reaper(&rt, Arc::clone(&inner));

        Ok(Self {
            rt: Arc::new(rt),
            inner,
            reaper,
        })
    }

    /// Spawn the crash-detection reaper task on `rt` and return its abort handle.
    ///
    /// The task ticks every [`CRASH_POLL_INTERVAL`], running the sync
    /// [`Supervisor::poll_once`] via `spawn_blocking` (rusqlite + syscalls off the
    /// async workers, AD-13). For each returned [`RestartPlan`] it spawns a
    /// DELAYED restart: sleep the backoff, then run [`Supervisor::restart`] via
    /// `spawn_blocking`. A restart whose instance was meanwhile stopped is a
    /// harmless no-op (the transition gate rejects a non-`failed` start — AC7).
    fn spawn_reaper(rt: &Runtime, inner: Arc<EngineInner>) -> tokio::task::AbortHandle {
        let handle = rt.spawn(async move {
            let mut ticker = tokio::time::interval(CRASH_POLL_INTERVAL);
            // Skip missed ticks rather than bursting after a slow poll.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let poll_inner = Arc::clone(&inner);
                let plans = tokio::task::spawn_blocking(move || {
                    let registry = poll_inner.registry.lock().expect("registry mutex poisoned");
                    let mut supervisor = poll_inner
                        .supervisor
                        .lock()
                        .expect("supervisor mutex poisoned");
                    supervisor.poll_once(&registry)
                })
                .await
                .unwrap_or_default();

                // Time each restart's backoff, then perform it. Spawned as
                // independent tasks so one instance's backoff does not delay
                // another's; each holds its own Arc<EngineInner> clone.
                for plan in plans {
                    let restart_inner = Arc::clone(&inner);
                    tokio::spawn(async move {
                        tokio::time::sleep(plan.delay).await;
                        let name = plan.name.as_str().to_string();
                        let _ = tokio::task::spawn_blocking(move || {
                            let registry = restart_inner
                                .registry
                                .lock()
                                .expect("registry mutex poisoned");
                            let mut supervisor = restart_inner
                                .supervisor
                                .lock()
                                .expect("supervisor mutex poisoned");
                            supervisor.restart(&registry, &name, plan.attempt, plan.delay)
                        })
                        .await;
                    });
                }
            }
        });
        handle.abort_handle()
    }

    /// A synchronous facade over the async API for non-async callers (`kt`).
    ///
    /// Each [`Blocking`] method is `runtime.block_on(async_method(..))`. See the
    /// module docs for why `kt` uses this instead of becoming an async binary.
    pub fn blocking(&self) -> Blocking<'_> {
        Blocking { engine: self }
    }

    /// The engine-computed Agent Home path for `name` (display helper).
    ///
    /// Pure path arithmetic — no I/O, no blocking pool needed.
    pub fn agent_home(&self, name: &InstanceName) -> PathBuf {
        self.inner
            .registry
            .lock()
            .expect("registry mutex poisoned")
            .agent_home(name)
    }

    /// Register a new Agent Instance of a native `kind`.
    ///
    /// Async wrapper over the blocking [`Registry::register`]; the FS+SQLite work
    /// runs on the blocking pool.
    pub async fn register(&self, name: &str, kind: &str) -> Result<AgentInstance, RegistryError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        let kind = kind.to_string();
        self.run_blocking(move || {
            inner
                .registry
                .lock()
                .expect("registry mutex poisoned")
                .register(&name, &kind)
        })
        .await
    }

    /// Register a new Agent Instance, resolving `reference` to an adapter first.
    pub async fn register_with_adapter(
        &self,
        name: &str,
        reference: &AdapterRef,
    ) -> Result<AgentInstance, RegistryError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        let reference = reference.clone();
        self.run_blocking(move || {
            inner
                .registry
                .lock()
                .expect("registry mutex poisoned")
                .register_with_adapter(&name, &reference)
        })
        .await
    }

    /// Remove an Agent Instance, honoring the retain/delete disposition (AC4)
    /// and the running-guard (AC5).
    ///
    /// ## Live/adopted-instance teardown — remove never leaves an orphan (AI-11)
    ///
    /// SEMANTICS DECISION (Away-Mode, conservative): `remove` must NEVER leave a
    /// live, unsupervised agent process behind — for BOTH plain and `--force`
    /// remove. If the supervisor still holds a handle for this instance
    /// (running/paused, including a process ADOPTED from a prior engine),
    /// [`Engine::remove`] STOPS it first — reusing the ordinary
    /// [`Supervisor::stop`] path, which terminates the whole process group/job,
    /// drops the handle, and CLEARS the write-ahead spawn record — and only THEN
    /// deletes the row. This closes the NFR-1 counterexample the story-1-2 `remove`
    /// docstring deferred to 1.4/1.6: without it, `kt agent remove <live> --force`
    /// deleted the record while the process kept running, and because the
    /// write-ahead record was gone a later engine crash left a TRUE unsupervised
    /// orphan no future engine could adopt.
    ///
    /// `--force` keeps its EXISTING meaning — it governs whether a `running` row
    /// may be removed at all (the [`RegistryError::RunningRequiresForce`] guard),
    /// NOT whether the process is torn down. So the teardown runs whenever we are
    /// actually going to delete the row: `force`, or a non-`running` live state
    /// (a `paused` instance is removable without `--force`, yet may still hold a
    /// live — SIGSTOP'd — process, so it is stopped too). A `running` instance
    /// without `--force` is rejected by the registry guard BEFORE any teardown, so
    /// a refused remove never touches the process.
    ///
    /// PARTIAL FAILURE: the teardown and the row deletion are two steps. If the
    /// [`Supervisor::stop`] succeeds (process killed, write-ahead record cleared)
    /// but the subsequent `registry.remove` then fails, the instance is left
    /// `stopped` with NO live process and NO spawn record — a coherent, safe state
    /// (never an orphan): the operator can simply re-run `remove` to delete the now
    /// already-stopped row. (A teardown-step failure aborts BEFORE any deletion —
    /// see the teardown call — so that case leaves the instance exactly as it was.)
    pub async fn remove(
        &self,
        name: &str,
        disposition: RemoveDisposition,
        force: bool,
    ) -> Result<(), RegistryError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            // (1) Stop a live/adopted instance FIRST so no unsupervised process is
            // ever orphaned (AI-11). Only when we will actually delete the row —
            // i.e. `force`, or a live state other than `running` (a `running`
            // instance without `--force` is refused by the registry guard below, so
            // we must NOT tear its process down for a remove that gets rejected).
            if let Ok(iname) = InstanceName::new(&name) {
                if let Ok(instance) = registry.lookup(&iname) {
                    let live = matches!(
                        instance.state,
                        LifecycleState::Running | LifecycleState::Paused
                    );
                    let will_delete = force || instance.state.is_removable_without_force();
                    if live && will_delete {
                        // Reuse the ordinary stop path: terminates the whole
                        // group/job, drops the handle, and clears the write-ahead
                        // record. A teardown failure aborts the remove (surfaced as
                        // a RegistryError) rather than deleting the row and leaking
                        // the process.
                        supervisor
                            .stop(&registry, &name, None)
                            .map_err(stop_error_to_registry)?;
                    }
                }
            }
            // (2) Delete the row (+ handle the Agent Home per disposition). The
            // authoritative name validation and the `running` running-guard live
            // here, so a malformed name / not-found / running-without-force is
            // reported exactly as before.
            registry.remove(&name, disposition, force)
        })
        .await
    }

    /// List the whole Fleet, ordered by name.
    pub async fn list(&self) -> Result<Vec<AgentInstance>, RegistryError> {
        let inner = Arc::clone(&self.inner);
        self.run_blocking(move || {
            inner
                .registry
                .lock()
                .expect("registry mutex poisoned")
                .list()
        })
        .await
    }

    /// The whole Fleet as serializable [`FleetEntry`] rows (story 1-7, FR-4 /
    /// AD-14). This is the read `kt agent list [--json]` uses.
    ///
    /// Composes the existing reads in ONE blocking pass: [`Registry::list`]
    /// (ordered by name) plus, per instance, the same runtime status
    /// ([`Engine::instance_status`]) the CLI already surfaces — the restart
    /// count/policy + (for a `failed` instance) the last-known cause — plus the
    /// honest Epic-1 metering seed (`budget`/`usage` = `None`; metering is Epic
    /// 3). Reading live persisted state on every call is what makes the listing
    /// ≤2s fresh (AC6): there is no cache, so any committed transition is
    /// reflected on the next `fleet()` (a single DB read, far under 2s).
    ///
    /// A per-instance status read-back failure DEGRADES that entry's runtime
    /// fields (count `0`, policy default, no cause) rather than failing the whole
    /// Fleet — mirroring the 1-6 `list` fallback — so one bad row never hides the
    /// rest. The top-level `list` read is the only hard failure.
    pub async fn fleet(&self) -> Result<Vec<FleetEntry>, RegistryError> {
        let inner = Arc::clone(&self.inner);
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let instances = registry.list()?;
            let entries = instances
                .into_iter()
                .map(|instance| Self::fleet_entry_for(&registry, instance))
                .collect();
            Ok(entries)
        })
        .await
    }

    /// Build a [`FleetEntry`] from an already-listed instance by reading its
    /// runtime status through the SAME registry lock (one read pass, no
    /// re-entrancy). A status read-back failure degrades the runtime fields to
    /// their defaults rather than failing the whole Fleet (the 1-6 `list`
    /// fallback). `budget`/`usage` are the honest Epic-1 `None` seed (Epic 3).
    fn fleet_entry_for(registry: &Registry, instance: AgentInstance) -> FleetEntry {
        // Read the write-ahead spawn record for the restart count/policy + cause,
        // exactly as `instance_status` does. A missing record → defaults (count 0,
        // policy default) — this is the normal case for a never-started instance.
        let record = registry.spawn_record(&instance.name).ok().flatten();
        let restart_policy = record
            .as_ref()
            .map(|r| r.restart_policy)
            .unwrap_or_default();
        let restart_count = record.as_ref().map(|r| r.restart_count).unwrap_or(0);
        let failed_cause = record.and_then(|r| r.last_known_cause).or_else(|| {
            // No record cause. For a `failed` instance, fall back to the last
            // event-log cause (a launch-error / cleared-terminal crash) — the same
            // fallback `instance_status` uses (AC9).
            if instance.state != LifecycleState::Failed {
                return None;
            }
            let events =
                Supervisor::read_events(registry, instance.name.as_str()).unwrap_or_default();
            events
                .iter()
                .rev()
                .find(|e| e.new_state == LifecycleState::Failed)
                .and_then(|e| failed_cause_detail(&e.cause))
        });
        FleetEntry {
            name: instance.name,
            kind: instance.kind,
            state: instance.state,
            restart_count,
            restart_policy,
            failed_cause,
            // The honest Epic-1 metering seed: metering is Epic 3 (AD-7), so these
            // are always `None` (JSON `null`), never `0` and never fabricated.
            budget: None,
            usage: None,
            agent_home: instance.agent_home,
        }
    }

    /// The effective (current-OS) Capability Declaration for a registered
    /// instance (AC1 "visible for the instance"). `kt agent show` renders this.
    pub async fn effective_capabilities(
        &self,
        name: &str,
    ) -> Result<EffectiveCapabilities, RegistryError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            inner
                .registry
                .lock()
                .expect("registry mutex poisoned")
                .effective_capabilities(&name)
        })
        .await
    }

    /// Start a registered Agent Instance (AC1/AC2).
    ///
    /// Drives the supervisor: resolve the launch spec, spawn via the per-OS
    /// [`ProcessBackend`](crate::ports::ProcessBackend), transition
    /// `registered/stopped → starting → running` on success or `starting →
    /// failed` on a launch error (diagnostic preserved, no zombie). Returns the
    /// instance in its new state.
    pub async fn start(&self, name: &str) -> Result<AgentInstance, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            supervisor.start(&registry, &name)
        })
        .await
    }

    /// Stop a running Agent Instance (AC3/AC4).
    ///
    /// Transitions `running → stopping`, requests graceful shutdown via the
    /// backend, escalates to a forced kill after `window` (default 30s) if the
    /// process has not exited, records the escalation in the instance log, then
    /// `stopping → stopped`. No process of the instance survives (the whole
    /// group/job is killed).
    pub async fn stop(
        &self,
        name: &str,
        window: Option<Duration>,
    ) -> Result<AgentInstance, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            supervisor.stop(&registry, &name, window)
        })
        .await
    }

    /// Pause a running Agent Instance with honest, per-OS semantics (story 1-5,
    /// AC1/AC2/AC3). Drives the supervisor's three-level dispatch on the effective
    /// (current-OS) pause `SupportLevel`: guaranteed → real SIGSTOP suspension +
    /// `running→paused`; best-effort → `running→paused` with a visible
    /// `pause-best-effort` qualifier in the transition event; unsupported → fail
    /// fast ([`EngineError::CapabilityUnsupported`]) with no state change. Returns
    /// the instance in its new state.
    pub async fn pause(&self, name: &str) -> Result<AgentInstance, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            supervisor.pause(&registry, &name)
        })
        .await
    }

    /// Resume a paused Agent Instance (story 1-5, AC1/AC2). The symmetric
    /// counterpart of [`Engine::pause`]: guaranteed → SIGCONT + `paused→running`;
    /// best-effort → `paused→running` with a `resume-best-effort` qualifier.
    pub async fn resume(&self, name: &str) -> Result<AgentInstance, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let mut supervisor = inner.supervisor.lock().expect("supervisor mutex poisoned");
            supervisor.resume(&registry, &name)
        })
        .await
    }

    /// The per-instance runtime status (story 1-6, AC9): Lifecycle State +
    /// effective Restart Policy + restart count + (for `failed`) the last-known
    /// cause. This is the read `kt agent list`/`show` uses to surface the restart
    /// count and, for a failed instance, the failed cause + active policy.
    ///
    /// Failed-cause precedence (AC9 requires the cause for ANY `failed` instance):
    /// the write-ahead record's `last_known_cause` if present; otherwise, for a
    /// `failed` instance, a fallback to the LAST transition-event-log cause. The
    /// fallback covers two cases the record cannot: a LAUNCH-ERROR failure
    /// (`starting → failed` returns before any spawn record is written), and a
    /// TERMINAL crash (`never` / crash-loop) whose record was cleared — both keep
    /// their cause in the JSON-Lines event log.
    pub async fn instance_status(&self, name: &str) -> Result<InstanceStatus, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let iname = InstanceName::new(&name).map_err(|reason| EngineError::InvalidName {
                name: name.clone(),
                reason,
            })?;
            let instance = registry
                .lookup(&iname)
                .map_err(crate::domain::registry_error_to_engine)?;
            let record = registry
                .spawn_record(&iname)
                .map_err(crate::domain::registry_error_to_engine)?;
            let restart_policy = record
                .as_ref()
                .map(|r| r.restart_policy)
                .unwrap_or_default();
            let restart_count = record.as_ref().map(|r| r.restart_count).unwrap_or(0);
            let failed_cause = record.and_then(|r| r.last_known_cause).or_else(|| {
                // No record cause. For a `failed` instance, fall back to the last
                // event-log cause so a launch-error or a cleared-terminal crash
                // still surfaces a reason (AC9).
                if instance.state != LifecycleState::Failed {
                    return None;
                }
                let events = Supervisor::read_events(&registry, &name).unwrap_or_default();
                events
                    .iter()
                    .rev()
                    .find(|e| e.new_state == LifecycleState::Failed)
                    .and_then(|e| failed_cause_detail(&e.cause))
            });
            Ok(InstanceStatus {
                instance,
                restart_policy,
                restart_count,
                failed_cause,
            })
        })
        .await
    }

    /// Set the per-instance Restart Policy (story 1-6, AC4 "per-instance
    /// configurable") — the config SEED (Epic-2 layered TOML config is later).
    /// Persists the policy so a subsequent `start` reads it and the reaper honors
    /// it on a crash. Runs behind the blocking pool like the other mutations.
    pub async fn set_restart_policy(
        &self,
        name: &str,
        policy: RestartPolicy,
    ) -> Result<(), EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            let iname = InstanceName::new(&name).map_err(|reason| EngineError::InvalidName {
                name: name.clone(),
                reason,
            })?;
            registry
                .set_restart_policy(&iname, policy)
                .map_err(crate::domain::registry_error_to_engine)
        })
        .await
    }

    /// Read the recorded transition events for an instance from its log (AC1
    /// "each transition emits an event"; AC3 escalation recorded). Test/embedding
    /// observation helper — this is the AD-14 seed, NOT the 7-2 subscription bus.
    pub async fn transition_events(&self, name: &str) -> Result<Vec<TransitionEvent>, EngineError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            let registry = inner.registry.lock().expect("registry mutex poisoned");
            Supervisor::read_events(&registry, &name)
        })
        .await
    }

    /// Run a blocking closure on tokio's blocking pool and await its result.
    ///
    /// Centralizes the `spawn_blocking` bridge so every async wrapper follows the
    /// same shape: rusqlite/FS work never touches an async worker. A join failure
    /// (the blocking task panicked) re-panics on the awaiting task rather than
    /// being silently swallowed.
    async fn run_blocking<T, F>(&self, f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        tokio::task::spawn_blocking(f)
            .await
            .expect("engine blocking task panicked")
    }
}

/// A synchronous facade over [`Engine`]'s async API (AD-13; FR-34/7-3 seed).
///
/// Obtained via [`Engine::blocking`]. Each method blocks the calling thread on
/// the engine's runtime until the async operation completes. This is precisely
/// the surface `kt` drives.
pub struct Blocking<'a> {
    engine: &'a Engine,
}

impl Blocking<'_> {
    /// Blocking [`Engine::register`].
    pub fn register(&self, name: &str, kind: &str) -> Result<AgentInstance, RegistryError> {
        self.engine.rt.block_on(self.engine.register(name, kind))
    }

    /// Blocking [`Engine::register_with_adapter`].
    pub fn register_with_adapter(
        &self,
        name: &str,
        reference: &AdapterRef,
    ) -> Result<AgentInstance, RegistryError> {
        self.engine
            .rt
            .block_on(self.engine.register_with_adapter(name, reference))
    }

    /// Blocking [`Engine::remove`].
    pub fn remove(
        &self,
        name: &str,
        disposition: RemoveDisposition,
        force: bool,
    ) -> Result<(), RegistryError> {
        self.engine
            .rt
            .block_on(self.engine.remove(name, disposition, force))
    }

    /// Blocking [`Engine::list`].
    pub fn list(&self) -> Result<Vec<AgentInstance>, RegistryError> {
        self.engine.rt.block_on(self.engine.list())
    }

    /// Blocking [`Engine::fleet`] (story 1-7, FR-4). The Fleet as serializable
    /// [`FleetEntry`] rows — what `kt agent list [--json]` renders.
    pub fn fleet(&self) -> Result<Vec<FleetEntry>, RegistryError> {
        self.engine.rt.block_on(self.engine.fleet())
    }

    /// Blocking [`Engine::effective_capabilities`].
    pub fn effective_capabilities(
        &self,
        name: &str,
    ) -> Result<EffectiveCapabilities, RegistryError> {
        self.engine
            .rt
            .block_on(self.engine.effective_capabilities(name))
    }

    /// Blocking [`Engine::start`].
    pub fn start(&self, name: &str) -> Result<AgentInstance, EngineError> {
        self.engine.rt.block_on(self.engine.start(name))
    }

    /// Blocking [`Engine::stop`].
    pub fn stop(&self, name: &str, window: Option<Duration>) -> Result<AgentInstance, EngineError> {
        self.engine.rt.block_on(self.engine.stop(name, window))
    }

    /// Blocking [`Engine::pause`].
    pub fn pause(&self, name: &str) -> Result<AgentInstance, EngineError> {
        self.engine.rt.block_on(self.engine.pause(name))
    }

    /// Blocking [`Engine::resume`].
    pub fn resume(&self, name: &str) -> Result<AgentInstance, EngineError> {
        self.engine.rt.block_on(self.engine.resume(name))
    }

    /// Blocking [`Engine::transition_events`].
    pub fn transition_events(&self, name: &str) -> Result<Vec<TransitionEvent>, EngineError> {
        self.engine.rt.block_on(self.engine.transition_events(name))
    }

    /// Blocking [`Engine::instance_status`] (story 1-6, AC9).
    pub fn instance_status(&self, name: &str) -> Result<InstanceStatus, EngineError> {
        self.engine.rt.block_on(self.engine.instance_status(name))
    }

    /// Blocking [`Engine::set_restart_policy`] (story 1-6, AC4).
    pub fn set_restart_policy(&self, name: &str, policy: RestartPolicy) -> Result<(), EngineError> {
        self.engine
            .rt
            .block_on(self.engine.set_restart_policy(name, policy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NameError;
    use crate::ports::StoreError;

    #[test]
    fn stop_error_to_registry_maps_each_arm() {
        // AI-11: a live-instance teardown failure during `remove` is mapped into a
        // RegistryError so `remove` (which speaks RegistryError) aborts WITHOUT
        // deleting the row or orphaning the process. Exercise every arm.

        // Store passes through as Store.
        assert!(matches!(
            stop_error_to_registry(EngineError::Store(StoreError::Backend("db gone".into()))),
            RegistryError::Store(_)
        ));

        // InvalidName / NotFound keep their shape (defensive — a well-formed remove
        // has already resolved the name, but the mapping must stay total).
        assert!(matches!(
            stop_error_to_registry(EngineError::InvalidName {
                name: "Bad Name".into(),
                reason: NameError::BadChar,
            }),
            RegistryError::InvalidName { .. }
        ));
        assert!(matches!(
            stop_error_to_registry(EngineError::NotFound { name: "ghost".into() }),
            RegistryError::NotFound { name } if name == "ghost"
        ));

        // Any other stop failure (a backend terminate/signal error, a log error)
        // surfaces as a filesystem-shaped Io naming the teardown, preserving the
        // underlying detail so the operator sees WHY the remove aborted.
        let mapped = stop_error_to_registry(EngineError::Backend {
            name: "live".into(),
            source: crate::ports::BackendError::Control {
                op: "terminate",
                detail: "boom".into(),
            },
        });
        match mapped {
            RegistryError::Io { source, .. } => {
                let msg = source.to_string();
                assert!(msg.contains("could not stop the live instance"), "{msg}");
                assert!(msg.contains("boom"), "underlying detail preserved: {msg}");
            }
            other => panic!("expected Io for a backend teardown failure, got {other:?}"),
        }

        // A log-write teardown failure also folds into the Io arm.
        assert!(matches!(
            stop_error_to_registry(EngineError::Log {
                name: "live".into(),
                path: "/x/logs/instance.log".into(),
                detail: "disk full".into(),
            }),
            RegistryError::Io { .. }
        ));
    }
}
