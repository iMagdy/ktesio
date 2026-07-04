//! Hexagonal ports (spine AD-1).
//!
//! Ports are the traits through which all variability enters the engine core.
//! This crate realizes two: [`StateStore`] (persistence) and, from story 1.4,
//! [`ProcessBackend`] (per-OS process control, AD-4 — its per-OS impls live in
//! `backends/`, the sole allowlisted `#[cfg]` home). Other ports
//! (`MeteringSource`, `MemoryBacking`, `SecretResolver`) arrive with the stories
//! that need them (entity-timing) — no speculative port trees.

mod process_backend;
mod state_store;

pub use process_backend::{BackendError, ProcessBackend, ProcessStatus, SpawnSpec, StopOutcome};
pub use state_store::StateStore;

use thiserror::Error;

/// Errors surfaced by a [`StateStore`] implementation, in domain terms.
///
/// The port speaks these — not SQLite error codes. The SQLite implementation
/// maps `rusqlite::Error` into these variants (e.g. a `UNIQUE` constraint
/// violation on `name` becomes [`StoreError::DuplicateName`]). Kept `thiserror`
/// only (no `miette` in the lib — conventions).
#[derive(Debug, Error)]
pub enum StoreError {
    /// A row with the same instance name already exists.
    #[error("an Agent Instance named '{name}' already exists")]
    DuplicateName {
        /// The conflicting instance name.
        name: String,
    },

    /// The requested instance does not exist.
    #[error("no Agent Instance named '{name}' exists")]
    NotFound {
        /// The missing instance name.
        name: String,
    },

    /// A stored row held a value the domain could not decode (e.g. an
    /// unrecognized Lifecycle State written by a future schema version).
    #[error("corrupt state row for '{name}': {detail}")]
    CorruptRow {
        /// The instance name whose row failed to decode.
        name: String,
        /// What specifically failed to decode.
        detail: String,
    },

    /// The database was created by a newer ktesio (its `user_version` is
    /// ahead of the schema this build understands). Refuse rather than
    /// silently downgrade it (which would corrupt a forward schema).
    #[error(
        "state database was created by a newer ktesio (schema v{found}; this build understands v{supported}); upgrade ktesio"
    )]
    SchemaTooNew {
        /// The `user_version` found on disk.
        found: i64,
        /// The highest schema version this build applies.
        supported: i64,
    },

    /// Any other backend failure (open, migrate, I/O, SQL execution).
    #[error("state store backend error: {0}")]
    Backend(String),
}
