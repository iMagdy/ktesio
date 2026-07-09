//! # ktesio-engine
//!
//! Home of the Ktesio engine: agent lifecycle, governance, and interaction
//! logic live here, behind a public API that *is* the Embedding Interface
//! (architecture spine AD-1, AD-2, AD-13).
//!
//! ## What this crate exposes (stories 1.2–1.3)
//!
//! The registration capability (FR-1/FR-2/FR-3), now with adapter resolution
//! and validation (story 1.3, FR-1 path registration / FR-19 metering / FR-27
//! contract seed). The public surface is deliberately small — the registry
//! service, the domain types it returns, and the adapter-contract types `kt`
//! renders:
//!
//! - [`Registry`] — `open`, `register`, `register_with_adapter`, `remove`,
//!   `list`, `get` (the Embedding Interface for registration; `kt` drives these
//!   directly).
//! - [`AdapterRef`] — a native-kind or manifest-path adapter request;
//!   [`ResolvedAdapter`] — the validated adapter view registration persists.
//! - [`RemoveDisposition`] — retain-or-delete choice for `remove`.
//! - [`AgentInstance`], [`InstanceName`], [`LifecycleState`] — returned domain
//!   types.
//! - [`CapabilityDeclaration`], [`EffectiveCapabilities`], [`Capability`],
//!   [`SupportLevel`], [`MeteringSource`], [`OsId`] — re-exported from
//!   `ktesio-adapter-api` so `kt` can render the effective per-OS declaration.
//! - [`RegistryError`] / [`NameError`] — the `thiserror` error surface `kt`
//!   wraps into `miette` diagnostics (no `miette` in this lib — conventions).
//!
//! Adapter resolution PARSES + VALIDATES only; it executes no lifecycle op
//! (story 1.4 owns the manifest executor and process launch).
//!
//! Everything else — the [`StateStore`] port, its SQLite implementation, and
//! the path authority — stays crate-internal (AD-1/AD-2). The `ports` module is
//! `pub` so the *port trait* is nameable by future in-workspace collaborators,
//! but the concrete store is not re-exported.
//!
//! ## Synchronous this story
//!
//! The engine API is synchronous here; the async-first tokio internals and the
//! `blocking()` facade (AD-13) arrive in story 1.4. The registry API is
//! facade-friendly by construction: it takes its state-dir base explicitly and
//! holds no global state.
//!
//! ## Dependency law (AD-2)
//!
//! The `kt` binary depends only on this crate's public API (plus
//! `ktesio-adapter-api` types); this crate depends on `ktesio-adapter-api` and
//! never on `kt` or concrete adapters.
//!
//! [`StateStore`]: crate::ports::StateStore

pub mod adapter;
mod backends;
pub mod domain;
mod engine;
// Engine-observed metering (AD-7 / FR-19), story 3-4: the loopback forward
// listener + the OpenAI `usage` parse. Engine-INTERNAL (AD-2 — `kt` never sees
// the listener); the supervisor drives it. Portable — NO OS cfg (in core, not
// `backends/`; the OS-cfg gate stays green).
mod metering;
pub mod paths;
pub mod ports;
mod store;
mod time;

// Re-export the registration + lifecycle public surface at the crate root
// (the Embedding Interface). `kt` drives the engine through the async [`Engine`]
// and its `blocking()` facade (AD-13); `Registry` stays public for in-workspace
// collaborators + tests but is no longer what `kt` uses directly.
pub use adapter::{AdapterRef, ResolvedAdapter};
pub use domain::{
    is_pass_through, render_dollars, render_dollars_bare, resolve, AgentInstance, BreachAction,
    BreachDimension, BreachScope, BudgetBreachEvent, BudgetView, ConfigError, ConfigLayer, CostCap,
    EffectiveConfig, EngineError, EstimateLabel, FleetEntry, FleetListing, InstanceName,
    LifecycleCommand, LifecycleError, LifecycleState, Micros, NameError, Rate, Registry,
    RegistryError, RemoveDisposition, ResolvedValue, RestartPolicy, RunId, SourceLayer,
    TokenBudget, TransitionCause, TransitionEvent, UsageEvent, UsageTotals, UsageUpdateEvent,
    UsageView, BUDGET_SCHEMA_VERSION, EVENT_SCHEMA_VERSION, FLEET_SCHEMA_VERSION,
    MICROS_PER_DOLLAR, PASS_THROUGH_PREFIX, SECRET_MASK, USAGE_SCHEMA_VERSION,
};
pub use engine::{Blocking, Engine, InstanceStatus};

// Re-export the adapter-contract types `kt` needs to render the effective
// per-OS Capability Declaration (AD-2: `kt` names these types, not the schema).
pub use ktesio_adapter_api::{
    Capability, CapabilityDeclaration, EffectiveCapabilities, MeteringSource, OsId, SupportLevel,
};
