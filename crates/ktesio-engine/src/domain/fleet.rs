//! The serializable Fleet view (spine AD-14 seed) — the `kt --json` shape.
//!
//! [`FleetEntry`] is the per-instance row `kt agent list`/`show` renders and, in
//! `--json` mode, serializes. It COMPOSES the already-`Serialize` domain types —
//! [`InstanceName`](crate::InstanceName) (a plain string on the wire),
//! [`LifecycleState`] (snake_case), and [`RestartPolicy`] (kebab-case) — rather
//! than hand-rolling field serialization, so `kt --json` and the future 7-2 Host
//! event stream share ONE schema (AD-14: "one event schema, two consumers").
//!
//! ## Metering fields — usage (3-1), budget (3-2), and DOLLARS (3-3) are real
//!
//! Metering is Epic 3 (AD-7). Story 3-1 makes [`FleetEntry::usage`] REAL: a
//! [`UsageView`] carrying the instance's cumulative (and current-Run) TOKEN totals
//! from the Usage Ledger. The active [`FleetEntry::metering_source`] rides alongside
//! it (AC-C — read from the persisted adapter snapshot). Story 3-2 makes
//! [`FleetEntry::budget`] REAL for TOKENS: a [`BudgetView`] carrying the configured
//! per-run + cumulative ceilings, the Breach Action, and the REMAINING tokens per
//! scope. Story 3-3 makes the DOLLAR dimension real WHEN a Rate is configured: the
//! [`UsageView`] gains the DERIVED cost + [`EstimateLabel`], and the [`BudgetView`]
//! gains the dollar Cost Cap + dollars-remaining per scope — all as INTEGER MICROS +
//! the label (NEVER a `$` string on the wire — AD-14; the human render is `kt`'s,
//! through the one currency module). The dollar fields stay ABSENT for a no-Rate
//! instance (AC-B: no Rate ⇒ no dollar figure, never a fabricated `$0.00`), and
//! `budget` stays `Option`-typed (`None` when NEITHER a token budget nor an
//! enforceable dollar cap exists — a truthful ABSENCE). Populating these fields is a
//! backward-additive change that does not bump [`crate::FLEET_SCHEMA_VERSION`] (a
//! new reader parses every old document; the dollar fields default-null/absent).
//!
//! ## Boundary (what this is NOT)
//!
//! This is a READ/VIEW type: it holds no logic, spawns nothing, and builds no
//! paths. [`crate::Engine::fleet`] composes it from the existing `list()` +
//! `instance_status()` reads (one SQLite read pass). It is the AD-14 SEED, not
//! the 7-2 subscription bus.

use serde::{Deserialize, Serialize};

use super::budget::{BreachAction, TokenBudget};
use super::cost::{CostCap, EstimateLabel, Micros};
use super::event::FLEET_SCHEMA_VERSION;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::restart::RestartPolicy;
use super::usage::UsageTotals;

/// The per-instance TOKEN budget/status surfaced in Fleet detail (story 3-2, AC9)
/// — TOKENS ONLY (AD-8: NO dollar cap, NO dollar headroom → 3-3/3-5).
///
/// Present iff a budget is CONFIGURED (at least one scope set); an un-budgeted
/// instance carries `None` in [`FleetEntry::budget`] (a truthful absence, never a
/// fabricated `0` ceiling). Carries the configured per-run + cumulative ceilings,
/// the Breach Action, and the REMAINING tokens per scope (ceiling − current total,
/// saturating at 0 — a breached scope reports `0` remaining, never a negative).
/// The remaining values are computed from the SAME ledger totals `usage` reports,
/// so they equal the Usage Ledger exactly (the FR-22 discipline). snake_case on
/// the wire (AD-14); `Deserialize` so `--json` round-trips.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetView {
    /// The configured per-run token ceiling (`null` when the per-run scope is
    /// unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_run_limit: Option<u64>,
    /// Remaining tokens in the per-run scope for the CURRENT Run (ceiling − current
    /// Run total, saturating at 0). `null` when the per-run scope is unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_run_remaining: Option<u64>,
    /// The configured cumulative token ceiling (`null` when the cumulative scope is
    /// unset).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_limit: Option<u64>,
    /// Remaining tokens in the cumulative scope (ceiling − cumulative total,
    /// saturating at 0). `null` when the cumulative scope is unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_remaining: Option<u64>,
    /// The configured per-run DOLLAR Cost Cap in micro-dollars (story 3-3) —
    /// present ONLY when a Rate is configured AND the per-run dollar scope is set;
    /// `null`/absent otherwise (AC-B: a cap with no Rate is inert → no dollar
    /// figure). NEVER a `$` string (AD-14).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_run_cost_cap: Option<Micros>,
    /// Remaining dollars in the per-run scope (cap − current-Run derived cost,
    /// saturating at 0 — a breached scope reports `$0`, never negative) in
    /// micro-dollars (story 3-3). Present under the same condition as
    /// [`per_run_cost_cap`](Self::per_run_cost_cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_run_dollars_remaining: Option<Micros>,
    /// The configured cumulative DOLLAR Cost Cap in micro-dollars (story 3-3) —
    /// present ONLY when a Rate is configured AND the cumulative dollar scope is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_cost_cap: Option<Micros>,
    /// Remaining dollars in the cumulative scope (cap − cumulative derived cost,
    /// saturating at 0) in micro-dollars (story 3-3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_dollars_remaining: Option<Micros>,
    /// The estimate label on the dollar cap/remaining figures (story 3-3, AD-8) —
    /// present ONLY when a Rate is configured; v1 always `estimated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_label: Option<EstimateLabel>,
    /// The configured Breach Action (`pause` / `stop` / `warn`). Governs BOTH the
    /// token and the dollar breach (story 3-3: one action).
    pub breach_action: BreachAction,
}

impl BudgetView {
    /// Build a [`BudgetView`] from the resolved [`TokenBudget`] + [`BreachAction`]
    /// and the CURRENT per-run + cumulative token totals (from the Usage Ledger),
    /// or `None` when no budget is configured (an honest absent budget — AC9).
    ///
    /// Remaining is `ceiling.saturating_sub(total)` per scope, so a scope that has
    /// reached/exceeded its ceiling reports `0` remaining (never a negative). Only
    /// a SET scope contributes a limit/remaining; an unset scope stays `None`.
    pub fn from_budget(
        budget: &TokenBudget,
        action: BreachAction,
        current_run_total: u64,
        cumulative_total: u64,
    ) -> Option<Self> {
        if !budget.is_set() {
            return None;
        }
        Some(Self {
            per_run_limit: budget.per_run,
            per_run_remaining: budget.per_run.map(|c| c.saturating_sub(current_run_total)),
            cumulative_limit: budget.cumulative,
            cumulative_remaining: budget
                .cumulative
                .map(|c| c.saturating_sub(cumulative_total)),
            per_run_cost_cap: None,
            per_run_dollars_remaining: None,
            cumulative_cost_cap: None,
            cumulative_dollars_remaining: None,
            estimate_label: None,
            breach_action: action,
        })
    }

    /// Build a combined token + DOLLAR [`BudgetView`] (story 3-3, AC10) — present
    /// iff a token budget OR a dollar Cost Cap is configured. The token fields come
    /// from `budget` + the token totals (as [`Self::from_budget`]); the DOLLAR
    /// fields come from `cost_cap` + the DERIVED costs, present ONLY when
    /// `has_rate` (a cap with no Rate is inert — AC-B: no dollar figure surfaced).
    /// Remaining is `cap.saturating_remaining(cost)` per scope (a breached scope →
    /// `$0`, never negative). Returns `None` when NEITHER a token budget nor an
    /// enforceable dollar cap exists (an honest absent budget — never a fabricated
    /// ceiling).
    #[allow(clippy::too_many_arguments)]
    pub fn from_budget_and_cost(
        budget: &TokenBudget,
        cost_cap: &CostCap,
        action: BreachAction,
        current_run_total: u64,
        cumulative_total: u64,
        has_rate: bool,
        current_run_cost: Micros,
        cumulative_cost: Micros,
        label: EstimateLabel,
    ) -> Option<Self> {
        // The dollar dimension is surfaced ONLY when a Rate exists AND a cap scope is
        // set (AC-B inert otherwise: no Rate ⇒ no dollar figure at all).
        let dollar_active = has_rate && cost_cap.is_set();
        // Present iff SOMETHING is configured (a token budget OR an enforceable cap).
        if !budget.is_set() && !dollar_active {
            return None;
        }
        Some(Self {
            per_run_limit: budget.per_run,
            per_run_remaining: budget.per_run.map(|c| c.saturating_sub(current_run_total)),
            cumulative_limit: budget.cumulative,
            cumulative_remaining: budget
                .cumulative
                .map(|c| c.saturating_sub(cumulative_total)),
            per_run_cost_cap: dollar_active.then_some(cost_cap.per_run).flatten(),
            per_run_dollars_remaining: dollar_active
                .then(|| {
                    cost_cap
                        .per_run
                        .map(|c| c.saturating_remaining(current_run_cost))
                })
                .flatten(),
            cumulative_cost_cap: dollar_active.then_some(cost_cap.cumulative).flatten(),
            cumulative_dollars_remaining: dollar_active
                .then(|| {
                    cost_cap
                        .cumulative
                        .map(|c| c.saturating_remaining(cumulative_cost))
                })
                .flatten(),
            // The label rides only when the dollar dimension is active.
            estimate_label: dollar_active.then_some(label),
            breach_action: action,
        })
    }
}

/// The per-instance Usage Ledger totals surfaced in Fleet detail (story 3-1
/// tokens + story 3-3 dollars, AC-C/AC11/AC10).
///
/// Carries the CUMULATIVE token totals (summed over every Run) and the
/// CURRENT-RUN totals (the active `starting`→terminal span, or zero when the
/// instance is not currently running). Story 3-3 adds the DERIVED DOLLAR cost
/// (cumulative + current-run, integer micros) + the [`EstimateLabel`] — present
/// ONLY when a Rate is configured, `None`/absent otherwise (AC-B honest absence:
/// no Rate ⇒ NO dollar figure, never a fabricated `$0.00`). The dollar cost equals
/// the Usage-Ledger-derived cost exactly (each row priced at its own persisted
/// Rate — FR-22). The wire carries INTEGER MICROS + the label, NEVER a `$` string
/// (AD-14). snake_case on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageView {
    /// Cumulative input (prompt) tokens over all of the instance's Runs.
    pub cumulative_input_tokens: u64,
    /// Cumulative output (completion) tokens over all of the instance's Runs.
    pub cumulative_output_tokens: u64,
    /// Input tokens for the CURRENT Run (0 when the instance is not running).
    pub current_run_input_tokens: u64,
    /// Output tokens for the CURRENT Run (0 when the instance is not running).
    pub current_run_output_tokens: u64,
    /// The DERIVED cumulative dollar cost in micro-dollars (story 3-3) — present
    /// ONLY when a Rate is configured; `null`/absent otherwise (AC-B: no Rate ⇒ no
    /// dollar figure). Equals the ledger-derived cost exactly (FR-22).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cumulative_dollars: Option<Micros>,
    /// The DERIVED current-Run dollar cost in micro-dollars (story 3-3) — present
    /// ONLY when a Rate is configured; `null`/absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_run_dollars: Option<Micros>,
    /// The estimate label on the dollar figures (story 3-3, AD-8) — present ONLY
    /// when a Rate is configured; v1 always `estimated`. NEVER a `$` string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_label: Option<EstimateLabel>,
}

impl UsageView {
    /// Build a TOKENS-ONLY [`UsageView`] from the cumulative + current-run
    /// [`UsageTotals`] the Fleet read summed from the ledger (the dollar fields
    /// absent — the no-Rate case, AC-B). Story 3-3's [`Self::with_dollars`] adds the
    /// derived cost when a Rate is present.
    pub fn new(cumulative: UsageTotals, current_run: UsageTotals) -> Self {
        Self {
            cumulative_input_tokens: cumulative.input_tokens,
            cumulative_output_tokens: cumulative.output_tokens,
            current_run_input_tokens: current_run.input_tokens,
            current_run_output_tokens: current_run.output_tokens,
            cumulative_dollars: None,
            current_run_dollars: None,
            estimate_label: None,
        }
    }

    /// Attach the DERIVED dollar cost + label to a [`UsageView`] (story 3-3) — the
    /// Rate-present case. The costs are the ledger-derived micros (each row priced
    /// at its own persisted Rate — no retro-repricing); the label is v1 `estimated`.
    pub fn with_dollars(
        mut self,
        cumulative_dollars: Micros,
        current_run_dollars: Micros,
        label: EstimateLabel,
    ) -> Self {
        self.cumulative_dollars = Some(cumulative_dollars);
        self.current_run_dollars = Some(current_run_dollars);
        self.estimate_label = Some(label);
        self
    }

    /// The cumulative total tokens (input + output over all Runs), saturating.
    pub fn cumulative_total_tokens(&self) -> u64 {
        self.cumulative_input_tokens
            .saturating_add(self.cumulative_output_tokens)
    }
}

/// One Agent Instance as the Fleet listing / `--json` document sees it (story
/// 1-7, FR-4).
///
/// Composes the registry identity (`name`, `kind`, `agent_home`), the live
/// runtime status (`state`, `restart_count`, `restart_policy`), and the REAL
/// metering view: `usage` (token totals, real since story 3-1) and `budget` (the
/// configured TOKEN ceilings + Breach Action, real for tokens since 3-2; `null`
/// only when NO budget is configured). The dollar cap/headroom stay absent until a
/// Rate exists (3-3/3-5). Field names are snake_case so the `--json` document is
/// stable and re-parseable.
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
    /// Token budget / status (story 3-2, AC9) — the configured per-run + cumulative
    /// ceilings, the Breach Action, and the REMAINING tokens per scope, TOKENS ONLY
    /// (AD-8: dollar cap/headroom stay absent → 3-3/3-5). `None` (JSON `null`) for an
    /// instance with NO budget configured — a truthful absence, never a fabricated
    /// `0` ceiling. The remaining values equal the Usage Ledger exactly (FR-22).
    /// ALWAYS PRESENT on the wire (rendered as `null` when absent), preserving the
    /// 3-1 `budget` key so `--json` consumers see a stable shape.
    pub budget: Option<BudgetView>,
    /// Usage Ledger token totals (story 3-1, AC-C/AC11) — cumulative + current-Run,
    /// TOKENS ONLY (AD-8). ALWAYS PRESENT (a concrete object, never `null`): a
    /// never-metered instance shows an honest all-ZERO [`UsageView`] (a truthful
    /// zero — the ledger genuinely holds zero tokens for it), distinct from the
    /// Epic-1 `budget` `null` "does not exist yet". The totals equal the ledger
    /// exactly (the FR-22 discipline). Dollars/headroom stay absent until 3-3/3-5.
    pub usage: UsageView,
    /// The active Metering Source wire string (`self-reported` / `engine-observed`),
    /// visible in Fleet detail (AC-C). Read from the persisted adapter snapshot.
    pub metering_source: String,
    /// Absolute Agent Home path (engine-computed; the path authority).
    pub agent_home: String,
}

impl FleetEntry {
    /// The human-readable ABSENCE token rendered in a Fleet cell that has no value
    /// (story 1-7). A single `—` — used for the `budget` cell of an UN-budgeted
    /// instance (no budget configured — story 3-2 keeps this honest absence) and
    /// wherever else a truthful "nothing here" is shown. Kept here so `list` and
    /// `show` render the SAME token.
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
            usage: UsageView::new(UsageTotals::zero(), UsageTotals::zero()),
            metering_source: "self-reported".to_string(),
            agent_home: format!("/x/agents/{name}"),
        }
    }

    #[test]
    fn entry_serializes_budget_as_null_when_unbudgeted_and_usage_as_real_zero_tokens() {
        // Story 3-2: an UN-budgeted instance's `budget` is an honest `null` (a
        // truthful absence, never a fabricated `0`), while `usage` is a REAL
        // UsageView — a concrete all-zero object for a never-metered instance.
        let entry = sample_entry("demo");
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        // budget: the honest absence (present as null, key preserved for consumers).
        assert_eq!(value["budget"], serde_json::Value::Null, "{value}");
        // usage: a real object with zero token totals (NOT null, NOT a number).
        assert!(
            value["usage"].is_object(),
            "usage must be an object: {value}"
        );
        assert_eq!(
            value["usage"]["cumulative_input_tokens"],
            serde_json::json!(0)
        );
        assert_eq!(
            value["usage"]["cumulative_output_tokens"],
            serde_json::json!(0)
        );
        // The active Metering Source is surfaced (AC-C).
        assert_eq!(value["metering_source"], serde_json::json!("self-reported"));
    }

    #[test]
    fn entry_carries_real_usage_totals_when_metered() {
        // A metered instance surfaces real cumulative + current-run token totals,
        // snake_case on the wire (AC11).
        let mut entry = sample_entry("demo");
        entry.usage = UsageView::new(
            UsageTotals {
                input_tokens: 100,
                output_tokens: 250,
            },
            UsageTotals {
                input_tokens: 40,
                output_tokens: 60,
            },
        );
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            value["usage"]["cumulative_input_tokens"],
            serde_json::json!(100)
        );
        assert_eq!(
            value["usage"]["cumulative_output_tokens"],
            serde_json::json!(250)
        );
        assert_eq!(
            value["usage"]["current_run_input_tokens"],
            serde_json::json!(40)
        );
        assert_eq!(entry.usage.cumulative_total_tokens(), 350);
        // Still tokens only — no dollars leaked into the view.
        assert!(value["usage"].get("cost").is_none());
        assert!(value["usage"].get("dollars").is_none());
    }

    // ---- Story 3-2: BudgetView (AC9) ----

    #[test]
    fn budget_view_is_none_when_no_scope_configured() {
        // An un-budgeted instance yields None (an honest absent budget).
        let v = BudgetView::from_budget(&TokenBudget::none(), BreachAction::Pause, 100, 200);
        assert!(v.is_none());
    }

    #[test]
    fn budget_view_carries_limits_remaining_and_action() {
        // A budgeted instance: limits + remaining (ceiling − total, per scope) +
        // the action. Tokens only — no dollar cap/headroom.
        let budget = TokenBudget {
            per_run: Some(100),
            cumulative: Some(1000),
        };
        let v = BudgetView::from_budget(&budget, BreachAction::Stop, 30, 400).unwrap();
        assert_eq!(v.per_run_limit, Some(100));
        assert_eq!(v.per_run_remaining, Some(70)); // 100 − 30
        assert_eq!(v.cumulative_limit, Some(1000));
        assert_eq!(v.cumulative_remaining, Some(600)); // 1000 − 400
        assert_eq!(v.breach_action, BreachAction::Stop);

        let value: serde_json::Value = serde_json::to_value(v).unwrap();
        assert_eq!(value["per_run_limit"], serde_json::json!(100));
        assert_eq!(value["per_run_remaining"], serde_json::json!(70));
        assert_eq!(value["cumulative_remaining"], serde_json::json!(600));
        assert_eq!(value["breach_action"], serde_json::json!("stop"));
        // Tokens only — no dollar cap/headroom.
        assert!(value.get("cost_cap").is_none());
        assert!(value.get("dollars_remaining").is_none());
        // Round-trips.
        let back: BudgetView = serde_json::from_value(value).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn budget_view_remaining_saturates_at_zero_when_breached() {
        // A scope at/over its ceiling reports 0 remaining, never a negative.
        let budget = TokenBudget {
            per_run: None,
            cumulative: Some(100),
        };
        let v = BudgetView::from_budget(&budget, BreachAction::Pause, 0, 250).unwrap();
        assert_eq!(v.cumulative_remaining, Some(0), "saturates, never negative");
        // The unset per-run scope stays absent in the view.
        assert_eq!(v.per_run_limit, None);
        let value: serde_json::Value = serde_json::to_value(v).unwrap();
        assert!(
            value.get("per_run_limit").is_none(),
            "an unset scope is omitted: {value}"
        );
    }

    #[test]
    fn entry_with_a_budget_serializes_the_view() {
        // A budgeted entry surfaces the real BudgetView in `--json`.
        let mut entry = sample_entry("web-1");
        entry.budget = BudgetView::from_budget(
            &TokenBudget {
                per_run: None,
                cumulative: Some(500),
            },
            BreachAction::Pause,
            0,
            120,
        );
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["budget"]["cumulative_limit"], serde_json::json!(500));
        assert_eq!(
            value["budget"]["cumulative_remaining"],
            serde_json::json!(380)
        );
        assert_eq!(value["budget"]["breach_action"], serde_json::json!("pause"));
        // Round-trips.
        let json = serde_json::to_string(&entry).unwrap();
        let back: FleetEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
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

    // ---- Story 3-3: dollar fields on the views (AC-B/AC10) ----

    #[test]
    fn usage_view_with_dollars_carries_labeled_integer_micros() {
        // A Rate'd instance: the derived cost rides as integer micros + the label,
        // NEVER a `$` string, NEVER a float.
        let usage = UsageView::new(
            UsageTotals {
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            UsageTotals::zero(),
        )
        .with_dollars(Micros(18_000_000), Micros(0), EstimateLabel::Estimated);
        let value: serde_json::Value = serde_json::to_value(usage).unwrap();
        assert_eq!(value["cumulative_dollars"], serde_json::json!(18_000_000));
        assert_eq!(value["current_run_dollars"], serde_json::json!(0));
        assert_eq!(value["estimate_label"], serde_json::json!("estimated"));
        let json = serde_json::to_string(&usage).unwrap();
        assert!(!json.contains('$'), "no `$` string on the wire: {json}");
        let back: UsageView = serde_json::from_str(&json).unwrap();
        assert_eq!(back, usage);
    }

    #[test]
    fn usage_view_without_a_rate_omits_the_dollar_fields() {
        // AC-B: no Rate ⇒ no dollar figure at all (the fields are absent, not `$0`).
        let usage = UsageView::new(UsageTotals::zero(), UsageTotals::zero());
        let value: serde_json::Value = serde_json::to_value(usage).unwrap();
        assert!(value.get("cumulative_dollars").is_none(), "{value}");
        assert!(value.get("current_run_dollars").is_none(), "{value}");
        assert!(value.get("estimate_label").is_none(), "{value}");
    }

    #[test]
    fn budget_view_with_a_rate_carries_the_dollar_cap_and_remaining() {
        // AC10: a Rate'd instance with a dollar cap surfaces the cap + remaining
        // (saturating) in integer micros + the label. Cap $0.50, spent $0.30 → $0.20
        // remaining.
        let v = BudgetView::from_budget_and_cost(
            &TokenBudget::none(),
            &CostCap {
                per_run: None,
                cumulative: Some(Micros(500_000)),
            },
            BreachAction::Pause,
            0,
            0,
            true, // has_rate
            Micros(0),
            Micros(300_000),
            EstimateLabel::Estimated,
        )
        .expect("a dollar cap makes the view present even with no token budget");
        assert_eq!(v.cumulative_cost_cap, Some(Micros(500_000)));
        assert_eq!(v.cumulative_dollars_remaining, Some(Micros(200_000)));
        assert_eq!(v.estimate_label, Some(EstimateLabel::Estimated));
        let value: serde_json::Value = serde_json::to_value(v).unwrap();
        assert_eq!(value["cumulative_cost_cap"], serde_json::json!(500_000));
        assert_eq!(
            value["cumulative_dollars_remaining"],
            serde_json::json!(200_000)
        );
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains('$'), "no `$` string on the wire: {json}");
    }

    #[test]
    fn budget_view_dollar_remaining_saturates_at_zero_when_breached() {
        // A breached dollar scope reports $0 remaining, never negative.
        let v = BudgetView::from_budget_and_cost(
            &TokenBudget::none(),
            &CostCap {
                per_run: None,
                cumulative: Some(Micros(500_000)),
            },
            BreachAction::Pause,
            0,
            0,
            true,
            Micros(0),
            Micros(800_000), // spent past the cap
            EstimateLabel::Estimated,
        )
        .unwrap();
        assert_eq!(v.cumulative_dollars_remaining, Some(Micros(0)));
    }

    #[test]
    fn budget_view_cost_cap_with_no_rate_is_inert_no_dollar_fields() {
        // AC-B: a Cost Cap set WITHOUT a Rate is inert — the view carries NO dollar
        // cap/remaining (has_rate = false), and with no token budget either the view
        // is None (an honest absent budget, never a fabricated dollar ceiling).
        let v = BudgetView::from_budget_and_cost(
            &TokenBudget::none(),
            &CostCap {
                per_run: None,
                cumulative: Some(Micros(500_000)),
            },
            BreachAction::Pause,
            0,
            0,
            false, // NO rate
            Micros(0),
            Micros(0),
            EstimateLabel::Estimated,
        );
        assert!(
            v.is_none(),
            "a cap with no Rate and no token budget is an honest absent budget"
        );

        // With a token budget present but no Rate, the view IS present but its dollar
        // fields stay absent (inert).
        let v = BudgetView::from_budget_and_cost(
            &TokenBudget {
                per_run: None,
                cumulative: Some(100),
            },
            &CostCap {
                per_run: None,
                cumulative: Some(Micros(500_000)),
            },
            BreachAction::Pause,
            0,
            50,
            false, // NO rate
            Micros(0),
            Micros(0),
            EstimateLabel::Estimated,
        )
        .unwrap();
        assert_eq!(v.cumulative_limit, Some(100));
        assert_eq!(v.cumulative_cost_cap, None, "no Rate ⇒ dollar cap inert");
        assert_eq!(v.estimate_label, None);
    }
}
