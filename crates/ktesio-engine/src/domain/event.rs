//! Transition events (spine AD-14 SEED) — one schema, two consumers.
//!
//! A [`TransitionEvent`] is the "event" for this story: a RECORDED state
//! transition carrying the prior state, the new state, a cause, and an RFC 3339
//! UTC timestamp. It is a versioned serde struct so story 7-2 (the host
//! subscription bus) and `kt --json` (story 1-7 / 4-3) reuse the SAME schema
//! ("one event schema, two consumers"). [`TransitionEvent::schema_version`] is
//! carried from the start so the schema can evolve compatibly.
//!
//! ## Boundary (what this is NOT)
//!
//! This story SEEDS the struct and RECORDS it (to the per-instance log, and
//! returns it so tests can assert). It does NOT build the bounded-channel
//! subscription bus — that is story 7-2. AC1's "each transition emits an event"
//! is satisfied here by "each transition records a `TransitionEvent`".

use serde::{Deserialize, Serialize};

use super::budget::{BreachAction, BreachScope};
use super::lifecycle::LifecycleState;

/// The schema version stamped on every emitted [`TransitionEvent`].
///
/// Bumped only on an incompatible change to the event shape. 7-2 / `--json`
/// negotiate on this; seeding it now means those consumers never see an
/// unversioned event.
///
/// NOTE (additive vs breaking): story 1-5 ADDS `TransitionCause` variants
/// (`pause-best-effort` / `resume-best-effort`), story 1-6 ADDS `crashed` /
/// `restarted`, and story 3-2 ADDS `budget-exceeded` (the Breach-Action cause on
/// the `running → paused`/`stopping` edge). Adding a new closed-vocabulary variant
/// is a backward-ADDITIVE change: a NEW reader parses every OLD event, and no
/// field is renamed or removed, so the version is NOT bumped. (The
/// converse — an OLD reader meeting a NEW cause — is a separate forward-compat
/// question: because `TransitionCause` is `#[serde(tag = "kind")]` with no
/// `#[serde(other)]` fallback, an old reader that hits an unknown tag ERRORS
/// rather than silently skipping it. That is acceptable precisely because 7-2 /
/// `--json` negotiate on THIS version field — a consumer that understands
/// version N knows exactly which cause tags exist at N, so it never meets a tag
/// it cannot match.) Only a shape change (renaming/removing a field, or an
/// incompatible restructure) would bump the version.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// The schema version stamped on the `kt --json` Fleet document (story 1-7,
/// AD-14).
///
/// AD-14 requires `kt --json` and the (future 7-2) Host event stream to be ONE
/// contract, so the Fleet document is a versioned serde struct just like
/// [`TransitionEvent`]. This starts at the SAME value as [`EVENT_SCHEMA_VERSION`]
/// so the two versioning stories begin aligned; it is a SEPARATE constant so the
/// Fleet document can evolve independently of the event schema (a change to one
/// shape must not force a version bump on the other). It rides on the
/// [`crate::FleetListing`] wrapper (for `list`) and each `show --json` object.
///
/// Bumped only on an INCOMPATIBLE change to the Fleet document shape. Adding a
/// field (e.g. populating the Epic-3 `budget`/`usage` from `null` to a real
/// type) is backward-ADDITIVE and does NOT bump the version — a new reader
/// parses every old document and no field is renamed or removed.
pub const FLEET_SCHEMA_VERSION: u32 = 1;

/// The schema version stamped on every emitted [`BudgetBreachEvent`] (story 3-2,
/// AD-14).
///
/// AD-14 names "breaches" explicitly among the versioned engine event structs the
/// subscription API + `kt --json` share. 3-2 FREEZES the breach-event wire shape
/// now — a versioned serde struct carrying the TOKEN breach fields — so `kt --json`
/// and the future 7-2 Host stream cannot drift into two dialects. A SEPARATE
/// constant from the sibling schemas ([`EVENT_SCHEMA_VERSION`],
/// [`FLEET_SCHEMA_VERSION`], [`crate::USAGE_SCHEMA_VERSION`]) — the wire shapes
/// evolve independently, so a change to one must not force a version bump on the
/// others. It starts at 1, aligned with the siblings. Bumped only on an
/// INCOMPATIBLE change; adding a field is backward-additive and does NOT bump it.
pub const BUDGET_SCHEMA_VERSION: u32 = 1;

/// Why a lifecycle transition happened (the transition event's `cause`).
///
/// A small closed vocabulary so consumers (log readers, 7-2, `--json`) can match
/// on the reason rather than parse free text. `LaunchError`/`StopForced` carry a
/// detail string (the adapter diagnostic / escalation note); the rest are plain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TransitionCause {
    /// An operator command drove the transition (`start` / `stop`).
    Command {
        /// The command label (`"start"` / `"stop"`).
        command: String,
    },
    /// The adapter became ready (process spawned and did not immediately die):
    /// `starting → running`.
    AdapterReady,
    /// The launch failed: `starting → failed`. Carries the preserved diagnostic.
    LaunchError {
        /// The adapter's launch diagnostic, preserved verbatim (AC2).
        detail: String,
    },
    /// Graceful shutdown succeeded within the window: `stopping → stopped`.
    StopGraceful,
    /// The graceful window elapsed and the process was force-killed:
    /// `stopping → stopped` after escalation (AC3). Carries the escalation note.
    StopForced {
        /// The escalation detail recorded in the instance log (AC3).
        detail: String,
    },
    /// A pause that was BEST-EFFORT, not a real suspension (story 1-5, AC2):
    /// `running → paused` on an OS/adapter where pause is
    /// [`SupportLevel::BestEffort`](ktesio_adapter_api::SupportLevel). This is the
    /// machine-readable half of "surfaced not silent" — a dedicated, matchable
    /// wire tag (`pause-best-effort`) so log/`--json`/7-2 consumers can tell a
    /// cooperative pause from a guaranteed one. A GUARANTEED pause emits a plain
    /// [`TransitionCause::Command`] (`"pause"`), never this. Carries a detail
    /// (the OS + declared level) for the record.
    PauseBestEffort {
        /// The best-effort detail (names the OS + declared level) recorded in the
        /// instance log (AC2).
        detail: String,
    },
    /// A resume that was BEST-EFFORT, the counterpart of [`TransitionCause::PauseBestEffort`]
    /// (story 1-5, AC2): `paused → running` on a best-effort OS/adapter. Wire tag
    /// `resume-best-effort`.
    ResumeBestEffort {
        /// The best-effort detail (names the OS + declared level) recorded in the
        /// instance log (AC2).
        detail: String,
    },
    /// The supervised process CRASHED — exited without a requested stop (story
    /// 1-6, AC5): the EVENT-driven `running → failed` edge the reaper applies.
    /// Wire tag `crashed`. Carries the exit code / signal detail so the
    /// log/`--json`/7-2 consumers can match on it. DISTINCT from
    /// [`TransitionCause::LaunchError`] (a startup failure) — a crash is a
    /// running process dying unrequested.
    Crashed {
        /// The exit detail (e.g. `"exited with code 1"` / `"exited via signal"`),
        /// preserved for the record (AC5).
        detail: String,
    },
    /// A Restart Policy RESTART of a crashed instance (story 1-6, AC4): the
    /// `failed → starting` edge the restart executor drives. Wire tag `restarted`.
    /// Records the consecutive restart `count` and the backoff `waited_ms` so the
    /// CLI + 7-2/`--json` consumers can surface both (AC9).
    Restarted {
        /// The consecutive restart count this restart represents (1-based).
        count: u32,
        /// The backoff waited before this restart, in milliseconds.
        waited_ms: u64,
    },
    /// A Token-Budget BREACH drove the transition (story 3-2, AD-7/AD-15): the
    /// Breach Action `pause`/`stop` pulled the EXISTING `running → paused` /
    /// `running → stopping` lever, so the lifecycle log itself explains WHY. Wire
    /// tag `budget-exceeded`. Carries the breached scope + the ceiling that was
    /// reached + the observed total (tokens only — no dollars, 3-3), so a
    /// log/`--json`/7-2 consumer sees the honest reason without the standalone
    /// breach event. A `warn` action produces NO transition, so it NEVER carries
    /// this cause (only the standalone [`BudgetBreachEvent`] records a `warn`).
    BudgetExceeded {
        /// Which budget scope tripped (`per-run` / `cumulative`).
        scope: BreachScope,
        /// The token ceiling that was reached.
        limit: u64,
        /// The committed token total that reached it (`>= limit`).
        observed: u64,
    },
}

impl TransitionCause {
    /// A command cause for `command`.
    pub fn command(command: impl Into<String>) -> Self {
        TransitionCause::Command {
            command: command.into(),
        }
    }

    /// A launch-error cause preserving `detail`.
    pub fn launch_error(detail: impl Into<String>) -> Self {
        TransitionCause::LaunchError {
            detail: detail.into(),
        }
    }

    /// A forced-stop cause recording the escalation `detail`.
    pub fn stop_forced(detail: impl Into<String>) -> Self {
        TransitionCause::StopForced {
            detail: detail.into(),
        }
    }

    /// A best-effort PAUSE cause recording `detail` (the OS + declared level).
    pub fn pause_best_effort(detail: impl Into<String>) -> Self {
        TransitionCause::PauseBestEffort {
            detail: detail.into(),
        }
    }

    /// A best-effort RESUME cause recording `detail` (the OS + declared level).
    pub fn resume_best_effort(detail: impl Into<String>) -> Self {
        TransitionCause::ResumeBestEffort {
            detail: detail.into(),
        }
    }

    /// A CRASH cause recording the exit `detail` (story 1-6, AC5).
    pub fn crashed(detail: impl Into<String>) -> Self {
        TransitionCause::Crashed {
            detail: detail.into(),
        }
    }

    /// A RESTART cause recording the consecutive `count` + backoff `waited_ms`
    /// (story 1-6, AC4/AC9).
    pub fn restarted(count: u32, waited_ms: u64) -> Self {
        TransitionCause::Restarted { count, waited_ms }
    }

    /// A BUDGET-EXCEEDED cause recording the breached `scope` + the `limit`
    /// reached + the `observed` total (story 3-2, AC7). Mirrors the other
    /// constructors; used on the `running → paused`/`stopping` transition the
    /// Breach Action drives.
    pub fn budget_exceeded(scope: BreachScope, limit: u64, observed: u64) -> Self {
        TransitionCause::BudgetExceeded {
            scope,
            limit,
            observed,
        }
    }
}

/// A recorded lifecycle state transition (spine AD-14 seed).
///
/// Emitted on every transition the supervisor applies, carrying everything AC1
/// requires: prior state, new state, cause, timestamp — plus the instance name
/// and the schema version. Serde-serializable so it round-trips through the
/// per-instance log and (later) the 7-2 bus / `--json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionEvent {
    /// The event schema version ([`EVENT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The Agent Instance the transition is for.
    pub instance: String,
    /// The state before the transition.
    pub prior_state: LifecycleState,
    /// The state after the transition.
    pub new_state: LifecycleState,
    /// Why the transition happened.
    pub cause: TransitionCause,
    /// RFC 3339 UTC timestamp of the transition.
    pub at: String,
}

impl TransitionEvent {
    /// Build a transition event, stamping the current schema version.
    ///
    /// `at` is an RFC 3339 UTC timestamp (the caller passes
    /// [`crate::time::now_rfc3339`] — kept a parameter so the struct stays pure
    /// and unit-testable with a fixed clock).
    pub fn new(
        instance: impl Into<String>,
        prior_state: LifecycleState,
        new_state: LifecycleState,
        cause: TransitionCause,
        at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: EVENT_SCHEMA_VERSION,
            instance: instance.into(),
            prior_state,
            new_state,
            cause,
            at: at.into(),
        }
    }
}

/// A recorded Token-Budget BREACH (spine AD-14, story 3-2) — the ALWAYS-recorded
/// event FR-21 requires "regardless of action".
///
/// Emitted from the ledger-commit choke point the instant a just-committed total
/// reaches a configured ceiling ([`super::budget::BudgetEvaluator`] returns
/// `Breached`), recorded BEFORE/independently of the lifecycle side-effect so a
/// best-effort/unsupported/failed pause NEVER loses the breach record (the FR-21
/// invariant + the NFR safety note). Recorded for EVERY action — including `warn`
/// (no transition) — as a durable JSON line, and (for `pause`/`stop`) mirrored as
/// a [`TransitionCause::BudgetExceeded`] on the resulting transition.
///
/// TOKENS ONLY (AD-8): the `limit`/`observed` are token counts, no dollars (3-3).
/// A [`BUDGET_SCHEMA_VERSION`]-stamped serde struct (snake_case) so `kt --json` +
/// the future 7-2 Host subscription share ONE schema. Full subscription DELIVERY
/// is 7-2's; 3-2 records + freezes the struct (the discipline 3-1 used for
/// [`crate::UsageUpdateEvent`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetBreachEvent {
    /// The breach-event schema version ([`BUDGET_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The Agent Instance whose ledger crossed the ceiling.
    pub instance: String,
    /// The Run (spine AD-7) the breaching event was committed under.
    pub run_id: String,
    /// Which budget scope tripped (`per-run` / `cumulative`).
    pub scope: BreachScope,
    /// The token ceiling that was reached.
    pub limit: u64,
    /// The committed token total that reached it (`>= limit`).
    pub observed: u64,
    /// The Breach Action taken (`pause` / `stop` / `warn`).
    pub action: BreachAction,
    /// The Metering Source that produced the breaching event's usage, as its wire
    /// string (`self-reported` / `engine-observed`).
    pub metering_source: String,
    /// RFC 3339 UTC timestamp the engine stamped when it recorded the breach.
    pub at: String,
}

impl BudgetBreachEvent {
    /// Build a breach event, stamping the current [`BUDGET_SCHEMA_VERSION`]. `at`
    /// is an RFC 3339 UTC timestamp (a parameter so the struct stays pure and
    /// unit-testable with a fixed clock, like [`TransitionEvent::new`]).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instance: impl Into<String>,
        run_id: impl Into<String>,
        scope: BreachScope,
        limit: u64,
        observed: u64,
        action: BreachAction,
        metering_source: impl Into<String>,
        at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: BUDGET_SCHEMA_VERSION,
            instance: instance.into(),
            run_id: run_id.into(),
            scope,
            limit,
            observed,
            action,
            metering_source: metering_source.into(),
            at: at.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_carries_schema_version_and_all_fields() {
        let e = TransitionEvent::new(
            "demo",
            LifecycleState::Registered,
            LifecycleState::Starting,
            TransitionCause::command("start"),
            "2026-07-04T00:00:00Z",
        );
        assert_eq!(e.schema_version, EVENT_SCHEMA_VERSION);
        assert_eq!(e.instance, "demo");
        assert_eq!(e.prior_state, LifecycleState::Registered);
        assert_eq!(e.new_state, LifecycleState::Starting);
        assert_eq!(e.cause, TransitionCause::command("start"));
        assert_eq!(e.at, "2026-07-04T00:00:00Z");
    }

    #[test]
    fn event_round_trips_through_json_one_schema_two_consumers() {
        // AD-14: the same serde struct 7-2 / --json reuse. Prove a lossless
        // round-trip through JSON (the per-instance log's line format).
        let e = TransitionEvent::new(
            "demo",
            LifecycleState::Starting,
            LifecycleState::Failed,
            TransitionCause::launch_error("exec not found: no-such-bin"),
            "2026-07-04T01:02:03Z",
        );
        let json = serde_json::to_string(&e).unwrap();
        let back: TransitionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
        // The preserved diagnostic survives (AC2).
        match back.cause {
            TransitionCause::LaunchError { detail } => {
                assert!(detail.contains("no-such-bin"))
            }
            other => panic!("expected LaunchError, got {other:?}"),
        }
    }

    #[test]
    fn cause_variants_serialize_with_stable_tags() {
        // The closed cause vocabulary uses a stable kebab-case tag so consumers
        // can match on it. Guard the wire tags.
        let cases = [
            (TransitionCause::command("start"), "command"),
            (TransitionCause::AdapterReady, "adapter-ready"),
            (TransitionCause::launch_error("x"), "launch-error"),
            (TransitionCause::StopGraceful, "stop-graceful"),
            (TransitionCause::stop_forced("x"), "stop-forced"),
            (TransitionCause::pause_best_effort("x"), "pause-best-effort"),
            (
                TransitionCause::resume_best_effort("x"),
                "resume-best-effort",
            ),
            (TransitionCause::crashed("x"), "crashed"),
            (TransitionCause::restarted(1, 1000), "restarted"),
            (
                TransitionCause::budget_exceeded(BreachScope::PerRun, 100, 120),
                "budget-exceeded",
            ),
        ];
        for (cause, tag) in cases {
            let json = serde_json::to_string(&cause).unwrap();
            assert!(json.contains(&format!("\"kind\":\"{tag}\"")), "{json}");
        }
    }

    #[test]
    fn budget_exceeded_cause_round_trips_with_its_fields() {
        // AC7: the Breach-Action cause carries the honest WHY (scope + limit +
        // observed, tokens only) and survives a JSON round-trip through the log.
        let cause = TransitionCause::budget_exceeded(BreachScope::Cumulative, 500, 512);
        let json = serde_json::to_string(&cause).unwrap();
        assert!(json.contains("\"kind\":\"budget-exceeded\""), "{json}");
        assert!(json.contains("\"scope\":\"cumulative\""), "{json}");
        // Tokens only — no dollar field leaked into the cause payload.
        assert!(!json.contains("cost"), "{json}");
        let back: TransitionCause = serde_json::from_str(&json).unwrap();
        match back {
            TransitionCause::BudgetExceeded {
                scope,
                limit,
                observed,
            } => {
                assert_eq!(scope, BreachScope::Cumulative);
                assert_eq!(limit, 500);
                assert_eq!(observed, 512);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn budget_breach_event_round_trips_with_schema_version_and_snake_case() {
        // AC10: the versioned breach wire struct `kt --json` + 7-2 share. Carries
        // the schema version + the token breach fields, snake_case, tokens only.
        let e = BudgetBreachEvent::new(
            "web-1",
            "run-42-7",
            BreachScope::PerRun,
            1000,
            1000,
            BreachAction::Pause,
            "self-reported",
            "2026-07-08T00:00:00Z",
        );
        assert_eq!(e.schema_version, BUDGET_SCHEMA_VERSION);
        let value: serde_json::Value = serde_json::to_value(&e).unwrap();
        let obj = value.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "action",
                "at",
                "instance",
                "limit",
                "metering_source",
                "observed",
                "run_id",
                "schema_version",
                "scope",
            ]
        );
        assert_eq!(value["scope"], serde_json::json!("per-run"));
        assert_eq!(value["action"], serde_json::json!("pause"));
        assert_eq!(value["limit"], serde_json::json!(1000));
        // Tokens only — no dollars in the payload.
        assert!(obj.get("cost").is_none());
        assert!(obj.get("dollars").is_none());
        let json = serde_json::to_string(&e).unwrap();
        let back: BudgetBreachEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn crashed_and_restarted_causes_round_trip_with_their_fields() {
        // AC5: the crash detail rides IN the event payload (matchable `crashed`
        // tag). AC4/AC9: the restart cause carries the count + waited backoff.
        let crashed = TransitionCause::crashed("exited with code 137");
        let json = serde_json::to_string(&crashed).unwrap();
        assert!(json.contains("\"kind\":\"crashed\""), "{json}");
        let back: TransitionCause = serde_json::from_str(&json).unwrap();
        match back {
            TransitionCause::Crashed { detail } => assert!(detail.contains("137"), "{detail}"),
            other => panic!("expected Crashed, got {other:?}"),
        }

        let restarted = TransitionCause::restarted(3, 4000);
        let json = serde_json::to_string(&restarted).unwrap();
        assert!(json.contains("\"kind\":\"restarted\""), "{json}");
        let back: TransitionCause = serde_json::from_str(&json).unwrap();
        match back {
            TransitionCause::Restarted { count, waited_ms } => {
                assert_eq!(count, 3);
                assert_eq!(waited_ms, 4000);
            }
            other => panic!("expected Restarted, got {other:?}"),
        }
    }

    #[test]
    fn pause_best_effort_cause_carries_the_detail_and_round_trips() {
        // AC2: the best-effort qualifier rides IN the event payload (the
        // machine-readable half of "surfaced not silent") and survives a JSON
        // round-trip through the instance log.
        let cause = TransitionCause::pause_best_effort("pause is best-effort on windows");
        let json = serde_json::to_string(&cause).unwrap();
        assert!(json.contains("\"kind\":\"pause-best-effort\""), "{json}");
        let back: TransitionCause = serde_json::from_str(&json).unwrap();
        match back {
            TransitionCause::PauseBestEffort { detail } => {
                assert!(detail.contains("best-effort"), "{detail}");
                assert!(detail.contains("windows"), "{detail}");
            }
            other => panic!("expected PauseBestEffort, got {other:?}"),
        }
    }

    #[test]
    fn stop_forced_cause_carries_the_escalation_detail() {
        let cause = TransitionCause::stop_forced("graceful window (1s) elapsed; sent SIGKILL");
        match cause {
            TransitionCause::StopForced { detail } => {
                assert!(detail.contains("SIGKILL"))
            }
            other => panic!("expected StopForced, got {other:?}"),
        }
    }
}
