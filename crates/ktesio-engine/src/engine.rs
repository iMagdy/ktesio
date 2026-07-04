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
    AgentInstance, EngineError, InstanceName, Registry, RegistryError, RemoveDisposition,
    Supervisor, TransitionEvent,
};

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
    /// facade owns and an empty in-memory supervisor.
    pub fn open(base: Option<PathBuf>) -> Result<Self, RegistryError> {
        let registry = Registry::open(base)?;
        let rt = Runtime::new().map_err(|e| RegistryError::Io {
            name: "<engine-runtime>".to_string(),
            path: "<tokio-runtime>".to_string(),
            source: e,
        })?;
        let inner = EngineInner {
            registry: Mutex::new(registry),
            supervisor: Mutex::new(Supervisor::new()),
        };
        Ok(Self {
            rt: Arc::new(rt),
            inner: Arc::new(inner),
        })
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
    pub async fn remove(
        &self,
        name: &str,
        disposition: RemoveDisposition,
        force: bool,
    ) -> Result<(), RegistryError> {
        let inner = Arc::clone(&self.inner);
        let name = name.to_string();
        self.run_blocking(move || {
            inner
                .registry
                .lock()
                .expect("registry mutex poisoned")
                .remove(&name, disposition, force)
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

    /// Blocking [`Engine::transition_events`].
    pub fn transition_events(&self, name: &str) -> Result<Vec<TransitionEvent>, EngineError> {
        self.engine.rt.block_on(self.engine.transition_events(name))
    }
}
