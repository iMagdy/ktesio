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

use super::lifecycle::LifecycleState;

/// The schema version stamped on every emitted [`TransitionEvent`].
///
/// Bumped only on an incompatible change to the event shape. 7-2 / `--json`
/// negotiate on this; seeding it now means those consumers never see an
/// unversioned event.
///
/// NOTE (additive vs breaking): story 1-5 ADDS `TransitionCause` variants
/// (`pause-best-effort` / `resume-best-effort`). Adding a new closed-vocabulary
/// variant is a backward-ADDITIVE change: a NEW reader parses every OLD event,
/// and no field is renamed or removed, so the version is NOT bumped. (The
/// converse — an OLD reader meeting a NEW cause — is a separate forward-compat
/// question: because `TransitionCause` is `#[serde(tag = "kind")]` with no
/// `#[serde(other)]` fallback, an old reader that hits an unknown tag ERRORS
/// rather than silently skipping it. That is acceptable precisely because 7-2 /
/// `--json` negotiate on THIS version field — a consumer that understands
/// version N knows exactly which cause tags exist at N, so it never meets a tag
/// it cannot match.) Only a shape change (renaming/removing a field, or an
/// incompatible restructure) would bump the version.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

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
        ];
        for (cause, tag) in cases {
            let json = serde_json::to_string(&cause).unwrap();
            assert!(json.contains(&format!("\"kind\":\"{tag}\"")), "{json}");
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
