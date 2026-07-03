//! The [`StateStore`] port (hexagonal, spine AD-1).
//!
//! Trait methods speak in domain types, never SQL. The SQLite implementation
//! ([`crate::store::SqliteStore`]) sits behind this port; the registry service
//! depends on the trait, not the concrete store.
//!
//! ## Synchronous this story (spine AD-13 is 1.4)
//!
//! These methods are plain synchronous functions. AD-13's async-first engine
//! and `blocking()` facade land in story 1.4; registration/removal are
//! filesystem + SQLite operations with no supervision or concurrency need, so
//! sync is the correct altitude now. The trait takes no runtime handle and
//! holds no global state, keeping it facade-friendly for the 1.4 migration.

use crate::domain::{AgentInstance, InstanceName};

use super::StoreError;

/// Persistence port for registry + lifecycle + Usage Ledger reads.
///
/// The minimal surface this story needs. Later stories widen it (lifecycle
/// transitions, usage-event writes) additively.
pub trait StateStore {
    /// Insert a new instance row.
    ///
    /// Fails with [`StoreError::DuplicateName`] if an instance with the same
    /// name already exists — enforced by the `UNIQUE` constraint on
    /// `agent_instances.name`, not a pre-check (which would race).
    fn create_instance(&self, instance: &AgentInstance) -> Result<(), StoreError>;

    /// Fetch an instance by name, or `None` if absent.
    fn get_instance(&self, name: &InstanceName) -> Result<Option<AgentInstance>, StoreError>;

    /// List every instance in the Fleet, ordered by name for determinism.
    fn list_instances(&self) -> Result<Vec<AgentInstance>, StoreError>;

    /// Delete an instance row by name.
    ///
    /// Fails with [`StoreError::NotFound`] if no such row exists, so callers
    /// can distinguish "removed" from "was never there". `ON DELETE CASCADE`
    /// cleans up any `usage_events` rows (none this story).
    fn delete_instance(&self, name: &InstanceName) -> Result<(), StoreError>;

    /// Count Usage Ledger events for an instance.
    ///
    /// Used to prove the "empty Usage Ledger" acceptance criterion: a freshly
    /// registered instance returns `0`. The Usage Ledger is table rows scoped
    /// by instance, not a file (AD-6).
    fn count_usage_events(&self, name: &InstanceName) -> Result<u64, StoreError>;
}
