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

use crate::domain::{AgentInstance, InstanceName, LifecycleState, RestartPolicy};

use super::{ProcessFingerprint, StoreError};

/// A write-ahead spawn record (spine AD-5) — the durable supervision state for
/// one Agent Instance, persisted BEFORE the process is treated as supervised.
///
/// AD-5's rule: "before exec completes, persist {instance id, PID, process
/// start-time fingerprint}". This record carries that fingerprint plus the
/// per-instance Restart Policy (AD-15 "per-instance configurable"), the current
/// consecutive-failure restart count (survives an engine restart so the CLI can
/// read it), and the last-known transition cause (so a reconcile-to-`failed` can
/// name why). It lives in the one SQLite state store (AD-6), keyed by instance
/// name; it is CLEARED on a clean stop so a normally-stopped instance is never
/// later mistaken for an orphan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRecord {
    /// The Agent Instance this record supervises.
    pub name: InstanceName,
    /// The supervised process's durable fingerprint (`{ pid, start-time }`).
    pub fingerprint: ProcessFingerprint,
    /// The per-instance Restart Policy (AD-15).
    pub restart_policy: RestartPolicy,
    /// Consecutive-failure restart count (resets on a clean run). Persisted so
    /// it survives an engine restart and the CLI can surface it (AC9).
    pub restart_count: u32,
    /// The last-known transition cause label (e.g. the crash detail), used when
    /// reconciling a non-adopted record to `failed`. `None` if not yet set.
    pub last_known_cause: Option<String>,
}

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

    /// Update an instance's Lifecycle State (and its `updated_at`) in place.
    ///
    /// Called by the supervisor on every persisted lifecycle transition (story
    /// 1.4). Domain-typed (a [`LifecycleState`], not a SQL string) — the SQLite
    /// implementation maps it to the `state` column. Fails with
    /// [`StoreError::NotFound`] if no such instance exists. The `state` column
    /// already exists (schema v1); no migration is needed.
    fn set_state(&self, name: &InstanceName, state: LifecycleState) -> Result<(), StoreError>;

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

    // ---- Write-ahead spawn records (story 1-6, spine AD-5/AD-6) ----

    /// Insert or replace the write-ahead spawn record for an instance, in ONE
    /// transaction (AD-6). Called by the supervisor BEFORE the process is
    /// treated as supervised ("no spawn without its record committed first").
    /// Fails with [`StoreError::NotFound`] if the instance row is gone.
    fn upsert_spawn_record(&self, record: &SpawnRecord) -> Result<(), StoreError>;

    /// Clear the write-ahead spawn record for an instance (a clean stop, so it is
    /// not later adopted/failed as an orphan). Idempotent: clearing an absent
    /// record is success.
    fn clear_spawn_record(&self, name: &InstanceName) -> Result<(), StoreError>;

    /// Read the write-ahead spawn record for an instance, or `None` if absent.
    fn get_spawn_record(&self, name: &InstanceName) -> Result<Option<SpawnRecord>, StoreError>;

    /// List every write-ahead spawn record (the reconcile input on engine start).
    /// Ordered by instance name for determinism.
    fn list_spawn_records(&self) -> Result<Vec<SpawnRecord>, StoreError>;

    /// Persist a new restart count + last-known cause for an instance's spawn
    /// record (a restart bump or a reset), in ONE transaction (AD-6). No-op if
    /// the instance has no spawn record. Kept distinct from
    /// [`StateStore::upsert_spawn_record`] so a restart bump does not need the
    /// full fingerprint again.
    fn set_restart_count(
        &self,
        name: &InstanceName,
        restart_count: u32,
        last_known_cause: Option<&str>,
    ) -> Result<(), StoreError>;

    /// Set the per-instance Restart Policy (AD-15 "per-instance configurable"),
    /// in ONE transaction (AD-6). If a spawn record exists its policy is updated;
    /// if none exists, a MINIMAL record is created carrying only the policy (a
    /// zero fingerprint + count 0) so the setting persists before the instance is
    /// ever started. Fails with [`StoreError::NotFound`] if the instance row is
    /// gone. This is the per-instance config SEED (Epic-2 layered TOML is later).
    fn set_restart_policy(
        &self,
        name: &InstanceName,
        policy: RestartPolicy,
    ) -> Result<(), StoreError>;
}
