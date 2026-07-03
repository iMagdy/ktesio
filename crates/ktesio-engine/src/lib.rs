//! # ktesio-engine
//!
//! Home of the Ktesio engine: agent lifecycle, governance, and interaction
//! logic live here, behind a public API that *is* the Embedding Interface
//! (architecture spine AD-1, AD-2, AD-13).
//!
//! ## What this crate exposes (story 1.2)
//!
//! The first real engine slice: **the registration capability** (FR-1/FR-2/
//! FR-3). The public surface is deliberately small — the registry service plus
//! the domain types it returns:
//!
//! - [`Registry`] — `open`, `register`, `remove`, `list` (the Embedding
//!   Interface for registration; `kt` drives these directly).
//! - [`RemoveDisposition`] — retain-or-delete choice for `remove`.
//! - [`AgentInstance`], [`InstanceName`], [`LifecycleState`] — returned domain
//!   types.
//! - [`RegistryError`] / [`NameError`] — the `thiserror` error surface `kt`
//!   wraps into `miette` diagnostics (no `miette` in this lib — conventions).
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

// Prove the AD-2 dependency edge (engine -> ktesio-adapter-api) compiles; real
// adapter-contract usage arrives with story 1.3.
use ktesio_adapter_api as _;

pub mod domain;
pub mod paths;
pub mod ports;
mod store;
mod time;

// Re-export the registration capability's public surface at the crate root
// (the Embedding Interface for this story).
pub use domain::{
    AgentInstance, InstanceName, LifecycleState, NameError, Registry, RegistryError,
    RemoveDisposition,
};
