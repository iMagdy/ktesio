//! The serializable Fleet view (spine AD-14 seed) — the `kt --json` shape.
//!
//! [`FleetEntry`] is the per-instance row `kt agent list`/`show` renders and, in
//! `--json` mode, serializes. It COMPOSES the already-`Serialize` domain types —
//! [`InstanceName`](crate::InstanceName) (a plain string on the wire),
//! [`LifecycleState`] (snake_case), and [`RestartPolicy`] (kebab-case) — rather
//! than hand-rolling field serialization, so `kt --json` and the future 7-2 Host
//! event stream share ONE schema (AD-14: "one event schema, two consumers").
//!
//! ## The honest Epic-1 metering seed (get this right)
//!
//! Budgets/caps and the Usage Ledger are **Epic 3** (metering, AD-7) — they do
//! NOT exist in Epic 1. [`FleetEntry::budget`] and [`FleetEntry::usage`] are
//! therefore `Option`-typed and ALWAYS `None` in Epic 1: they serialize as JSON
//! `null` (a TYPED absence, never `0`, never a fabricated number). The fields are
//! PRESENT so the `--json` shape is stable for Epic 3 to populate additively
//! (AD-14 wants the schema not to churn); the truthful Epic-1 value is "none
//! yet". When Epic 3 lands it replaces the `Option<Never>`-shaped seed with the
//! real budget/usage types — a backward-additive change that does not bump
//! [`crate::FLEET_SCHEMA_VERSION`]. Until then, `kt` renders the human cell as a
//! single `—` and prints one stderr note that metering arrives in Epic 3.
//!
//! ## Boundary (what this is NOT)
//!
//! This is a READ/VIEW type: it holds no logic, spawns nothing, and builds no
//! paths. [`crate::Engine::fleet`] composes it from the existing `list()` +
//! `instance_status()` reads (one SQLite read pass). It is the AD-14 SEED, not
//! the 7-2 subscription bus.

use serde::{Deserialize, Serialize};

use super::event::FLEET_SCHEMA_VERSION;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::restart::RestartPolicy;

/// The honest Epic-1 metering seed for the `budget`/`usage` columns.
///
/// Metering is Epic 3 (AD-7); in Epic 1 there is NO budget/cap evaluation and NO
/// Usage Ledger, so every metering cell is a typed ABSENCE. This uninhabited
/// enum makes that a compile-time guarantee: an [`Option<MeteringSeed>`] can only
/// ever be `None` (there is no way to construct a `Some`), so the `--json`
/// `budget`/`usage` fields are ALWAYS `null` and the human cells are ALWAYS `—`.
/// Epic 3 replaces this seed with the real budget/usage types (a backward-
/// additive change). It derives `Serialize`/`Deserialize` only so the `Option`
/// wrapper is (de)serializable; no value is ever produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeteringSeed {}

/// One Agent Instance as the Fleet listing / `--json` document sees it (story
/// 1-7, FR-4).
///
/// Composes the registry identity (`name`, `kind`, `agent_home`), the live
/// runtime status (`state`, `restart_count`, `restart_policy`), and the honest
/// Epic-1 metering seed (`budget`/`usage`, always `null`). Field names are
/// snake_case so the `--json` document is stable and re-parseable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetEntry {
    /// Fleet-unique instance name (serializes as a plain string).
    pub name: InstanceName,
    /// Agent kind (the adapter identity).
    pub kind: String,
    /// Current Lifecycle State, read live from the store (snake_case on the wire).
    pub state: LifecycleState,
    /// Consecutive-failure restart count (story 1-6). `0` when never restarted.
    pub restart_count: u32,
    /// The effective per-instance Restart Policy (story 1-6; kebab-case on the
    /// wire).
    pub restart_policy: RestartPolicy,
    /// The last-known failed cause, present for a `failed` instance (story 1-6).
    /// `None` (JSON `null`) otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_cause: Option<String>,
    /// Budget / cap status — the HONEST Epic-1 seed. Metering is Epic 3 (AD-7),
    /// so this is ALWAYS `None` (JSON `null`) today; never `0`, never fabricated.
    pub budget: Option<MeteringSeed>,
    /// Usage Ledger totals — the HONEST Epic-1 seed. Metering is Epic 3 (AD-7),
    /// so this is ALWAYS `None` (JSON `null`) today; never `0`, never fabricated.
    pub usage: Option<MeteringSeed>,
    /// Absolute Agent Home path (engine-computed; the path authority).
    pub agent_home: String,
}

impl FleetEntry {
    /// The human-readable metering-seed token rendered in the `budget`/`usage`
    /// cells of the `kt agent list`/`show` tables (story 1-7). A single `—` is the
    /// truthful Epic-1 value: metering does not exist yet (Epic 3). Kept here so
    /// `list` and `show` render the SAME token.
    pub const METERING_SEED_CELL: &'static str = "—";
}

/// The `kt agent list --json` document (story 1-7, AD-14).
///
/// A versioned wrapper carrying [`FLEET_SCHEMA_VERSION`] plus the per-instance
/// [`FleetEntry`] rows, so `--json` consumers (and the future 7-2 Host stream)
/// negotiate on the version and never see an unversioned document. An empty Fleet
/// serializes with an empty `instances` array (valid JSON — AC9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetListing {
    /// The Fleet document schema version ([`FLEET_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Every Agent Instance in the Fleet, ordered by name.
    pub instances: Vec<FleetEntry>,
}

impl FleetListing {
    /// Build a listing document from the composed entries, stamping the current
    /// [`FLEET_SCHEMA_VERSION`].
    pub fn new(instances: Vec<FleetEntry>) -> Self {
        Self {
            schema_version: FLEET_SCHEMA_VERSION,
            instances,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(name: &str) -> FleetEntry {
        FleetEntry {
            name: InstanceName::new(name).unwrap(),
            kind: "mock".to_string(),
            state: LifecycleState::Registered,
            restart_count: 0,
            restart_policy: RestartPolicy::OnFailure,
            failed_cause: None,
            budget: None,
            usage: None,
            agent_home: format!("/x/agents/{name}"),
        }
    }

    #[test]
    fn entry_serializes_budget_and_usage_as_null_the_honest_seed() {
        // THE Epic-1-has-no-metering nuance: budget/usage MUST be JSON `null`
        // (a typed absence), never `0` and never a fabricated number.
        let entry = sample_entry("demo");
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["budget"], serde_json::Value::Null, "{value}");
        assert_eq!(value["usage"], serde_json::Value::Null, "{value}");
        // Never a zero (the tempting-but-dishonest value).
        assert_ne!(value["budget"], serde_json::json!(0));
        assert_ne!(value["usage"], serde_json::json!(0));
    }

    #[test]
    fn entry_uses_plain_string_and_wire_forms_for_composed_types() {
        // The composed domain types keep their wire forms: name is a plain
        // string, state is snake_case, policy is kebab-case (AD-14: reuse the
        // already-Serialize types, do not re-derive).
        let mut entry = sample_entry("web-1");
        entry.state = LifecycleState::Running;
        entry.restart_policy = RestartPolicy::Never;
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["name"], serde_json::json!("web-1"));
        assert_eq!(value["state"], serde_json::json!("running"));
        assert_eq!(value["restart_policy"], serde_json::json!("never"));
    }

    #[test]
    fn entry_round_trips_through_json() {
        // The --json document must be stable + re-parseable (AC5).
        let entry = sample_entry("demo");
        let json = serde_json::to_string(&entry).unwrap();
        let back: FleetEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn failed_cause_is_omitted_when_absent_and_present_when_set() {
        // A non-failed entry omits `failed_cause` (skip_serializing_if); a failed
        // one carries the cause string.
        let entry = sample_entry("demo");
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert!(value.get("failed_cause").is_none(), "{value}");

        let mut failed = sample_entry("boom");
        failed.state = LifecycleState::Failed;
        failed.failed_cause = Some("crashed with code 1".to_string());
        let value: serde_json::Value = serde_json::to_value(&failed).unwrap();
        assert_eq!(
            value["failed_cause"],
            serde_json::json!("crashed with code 1")
        );
    }

    #[test]
    fn listing_carries_schema_version_and_instances() {
        let listing = FleetListing::new(vec![sample_entry("a"), sample_entry("b")]);
        assert_eq!(listing.schema_version, FLEET_SCHEMA_VERSION);
        let value: serde_json::Value = serde_json::to_value(&listing).unwrap();
        assert_eq!(
            value["schema_version"],
            serde_json::json!(FLEET_SCHEMA_VERSION)
        );
        assert_eq!(value["instances"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn empty_listing_serializes_with_an_empty_array() {
        // AC9: an empty Fleet is still valid JSON — an empty `instances` array.
        let listing = FleetListing::new(vec![]);
        let value: serde_json::Value = serde_json::to_value(&listing).unwrap();
        assert_eq!(value["instances"], serde_json::json!([]));
        // And it re-parses.
        let json = serde_json::to_string(&listing).unwrap();
        let back: FleetListing = serde_json::from_str(&json).unwrap();
        assert_eq!(back, listing);
    }

    #[test]
    fn metering_seed_cell_is_the_em_dash_token() {
        // The human cell token is `—` (consistent between list + show).
        assert_eq!(FleetEntry::METERING_SEED_CELL, "—");
    }
}
