//! The event-bus resync helper (story 10-3, FR-33 / AD-14) — the one-call
//! remedy for the bus's documented crash-window at-most-once delivery.
//!
//! ## The problem it closes
//!
//! The 7-2 bus publishes every event right AFTER its durable append succeeds —
//! "never publish what did not commit" — which makes delivery at-most-once in
//! the crash window between the two steps: a process crash there loses that ONE
//! event from the STREAM while the durable logs stay complete. The documented
//! recourse was "re-read the query APIs" — three different log formats a host
//! had to hand-roll into one stream. This module is the engine's own backfill:
//! it reads the COMMITTED truth (the same `instance.log` / `breaches.log` /
//! `usage_events` ledger the query APIs serve) and converts each record to its
//! [`EngineEvent`] bus payload verbatim, so a host heals the window in one
//! call and the caveat reads "recoverable via `resync_events`".
//!
//! ## The contract
//!
//! * **Committed truth only.** A READ-side helper over the same records the
//!   query APIs return — the bus, its publish points, and the subscribe
//!   semantics are untouched. An event that never committed (the VG1
//!   append-failure arm) never appears here either: only durable records
//!   backfill.
//! * **Per-instance.** `name` scopes every read: the instance's transition
//!   log, breach log, and ledger rows. A Fleet-wide backfill is the host
//!   composing per-instance calls.
//! * **Order: exact per family, family-major across families.** Within each
//!   family the batch IS commit order — `instance.log` line order, breach-log
//!   line order, ledger `rowid` order (the same orders the 7-2 suite pins
//!   against the bus). Across families the durable record carries NO global
//!   sequence (commit order was only ever the bus's in-memory publish order,
//!   and the per-family timestamps share whole-second resolution), so the
//!   batch is emitted family-major — transitions, then breaches, then usage —
//!   and does NOT fabricate a cross-family interleaving. That is exactly why
//!   the documented contract is **backfill FIRST, then subscribe live**: a
//!   host that resyncs and only then attaches a receiver gets clean
//!   continuity (backfilled prefix + live suffix, no gap, no duplicate). A
//!   host that subscribes first may see the overlap window as duplicates
//!   (never gaps) — its own ordering choice, detectable and dedupable by it.
//!   No watermark/dedup machinery is built for that case: the overlap is
//!   bounded by the host's own subscribe-before-backfill window and the
//!   per-family records are exact, so a watermark would add a persistence
//!   surface to guard against a self-inflicted ordering the contract already
//!   avoids.
//! * **Crash-recovery read posture (torn-tail tolerance).** This helper is
//!   called MOST often right after the crash it heals, and a crash mid-append
//!   tears the log's trailing line. Unlike the strict query-API reads
//!   ([`Supervisor::read_events`] hard-errors any malformed line), the reads
//!   here skip ONE unparseable TRAILING line per log (a torn append — the
//!   next append overwrites nothing; the record it would have carried is
//!   simply absent from the durable truth too) and still hard-error a
//!   malformed INTERIOR line (that is not a race — that is the wrong file or
//!   a corrupting engine, worth surfacing).
//! * **Cursor-based, idempotent.** [`ResyncCursor`] records how many records
//!   per family a host has already consumed; passing it to the next call
//!   skips exactly that prefix (clamped to what exists — a recreated/truncated
//!   log degrades to "whatever is there", never an error), so re-running a
//!   resync never re-delivers what the cursor already covers. Each family's
//!   log is append-only (transitions/breaches are JSON-Lines appends; ledger
//!   rows are INSERT-only under the no-double-count key), which is what makes
//!   a per-family count a stable position.
//!
//! ## Boundary (what this is NOT)
//!
//! No new event kinds (the payloads are the same three [`EngineEvent`] verse
//! the bus carries), no schema change, no bus/publish-path change, no
//! persistence change. Additive facade API: [`Engine::resync_events`] (async)
//! and [`Blocking::resync_events`](crate::Blocking::resync_events) (sync),
//! documented for hosts in docs/embedding.md.
//!
//! [`Supervisor::read_events`]: super::supervisor::Supervisor::read_events
//! [`Engine::resync_events`]: crate::Engine::resync_events

use serde::{Deserialize, Serialize};

use super::bus::EngineEvent;
use super::event::BudgetBreachEvent;
use super::event::TransitionEvent;
use super::registry::Registry;
use super::usage::{UsageEvent, UsageUpdateEvent};
use super::EngineError;

/// How far into each committed event family a host has already consumed —
/// the resync helper's skip position (story 10-3).
///
/// One count per family (`instance.log` lines, `breaches.log` lines, ledger
/// rows), because the durable record keeps per-family order only — see the
/// module docs for the ordering contract. Counts are positions in APPEND
/// order, and every family's record is append-only, so a cursor returned by
/// one [`ResyncBatch`] is a stable position for the next call.
/// [`ResyncCursor::START`] (all zero) is the entry point: the full backfill.
///
/// Serde-derived so a host can persist its cursor across its own restarts
/// (snake_case, like every AD-14 wire struct).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncCursor {
    /// Transition records already consumed (skipped) from `instance.log`.
    pub transitions: u64,
    /// Breach records already consumed (skipped) from `breaches.log`.
    pub breaches: u64,
    /// Ledger rows already consumed (skipped) from the `usage_events` table.
    pub usage: u64,
}

impl ResyncCursor {
    /// The start position: consume EVERYTHING committed (the full backfill a
    /// crash-window heal begins with). Identical to `ResyncCursor::default()`;
    /// spelled as a named constant so a call site reads as intent.
    pub const START: ResyncCursor = ResyncCursor {
        transitions: 0,
        breaches: 0,
        usage: 0,
    };
}

/// One resync batch: the committed events after a cursor position, plus the
/// position to resume from (story 10-3).
///
/// `events` carries ONLY records committed after the cursor the caller passed
/// (an empty vec means the host is already caught up to the committed truth);
/// `cursor` is the position after THIS batch — pass it to the next
/// `resync_events` call and the already-delivered prefix is skipped. The two
/// fields come from one read pass, so `cursor` never names a position beyond
/// what `events` actually carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncBatch {
    /// The committed events, transitions then breaches then usage updates
    /// (family-major — see the module docs for the ordering contract), each
    /// payload the EXACT bus wrapper the live stream delivers.
    pub events: Vec<EngineEvent>,
    /// The consume position after this batch — feed it to the next
    /// `resync_events` call for an idempotent continuation.
    pub cursor: ResyncCursor,
}

/// Read the COMMITTED event records for `name` past `after` and convert them
/// to bus payloads (the resync helper's engine-side core, story 10-3).
///
/// Three committed-truth reads — the instance's transition log, breach log,
/// and `usage_events` ledger rows, each in its own commit order — skipped to
/// the caller's per-family cursor positions, then wrapped into the SAME
/// [`EngineEvent`] payloads the bus publishes. Torn-tail tolerant per the
/// module docs (one skipped unparseable trailing line per log; a malformed
/// interior line is a typed [`EngineError::Log`]).
///
/// Assumes the name is already validated and the instance exists (the facade
/// checks both) — this is pure read + conversion.
pub(crate) fn read_committed(
    registry: &Registry,
    name: &super::InstanceName,
    after: &ResyncCursor,
) -> Result<ResyncBatch, EngineError> {
    let transitions_path = registry.instance_log_path(name);
    let transitions =
        read_events_tolerant(&transitions_path, "instance-log").map_err(|detail| {
            EngineError::Log {
                name: name.as_str().to_string(),
                path: transitions_path.to_string_lossy().into_owned(),
                detail,
            }
        })?;
    let breaches_path = registry.instance_breach_log_path(name);
    let breaches = read_breach_events_tolerant(&breaches_path, "breach-log").map_err(|detail| {
        EngineError::Log {
            name: name.as_str().to_string(),
            path: breaches_path.to_string_lossy().into_owned(),
            detail,
        }
    })?;
    let usage = registry
        .usage_rows(name)
        .map_err(super::registry_error_to_engine)?;

    Ok(assemble(&transitions, &breaches, &usage, after))
}

/// The pure assembly: skip each family to its cursor position (clamped to
/// what exists — a cursor past a family's end means that family is fully
/// consumed), wrap the remainder into [`EngineEvent`] payloads family-major,
/// and compute the position after the batch. Unit-tested directly so the
/// cursor math (skip + clamp + advance) is pinned without any I/O.
pub(crate) fn assemble(
    transitions: &[TransitionEvent],
    breaches: &[BudgetBreachEvent],
    usage: &[UsageEvent],
    after: &ResyncCursor,
) -> ResyncBatch {
    let mut events = Vec::new();
    let skip_transitions = (after.transitions as usize).min(transitions.len());
    for event in &transitions[skip_transitions..] {
        events.push(EngineEvent::Transition(event.clone()));
    }
    let skip_breaches = (after.breaches as usize).min(breaches.len());
    for event in &breaches[skip_breaches..] {
        events.push(EngineEvent::BudgetBreach(event.clone()));
    }
    let skip_usage = (after.usage as usize).min(usage.len());
    for row in &usage[skip_usage..] {
        events.push(EngineEvent::UsageUpdate(UsageUpdateEvent::new(row.clone())));
    }
    ResyncBatch {
        events,
        cursor: ResyncCursor {
            transitions: transitions.len() as u64,
            breaches: breaches.len() as u64,
            usage: usage.len() as u64,
        },
    }
}

/// Read a JSON-Lines event log with the RESYNC posture (story 10-3): an
/// absent file is empty; ONE unparseable TRAILING line is skipped (a torn
/// append — the crash-recovery read must survive the very crash it heals);
/// a malformed INTERIOR line is an error naming it (wrong file, not a race —
/// mirroring the strict query-API readers' convention).
///
/// The engine appends whole lines under normal operation, so a torn line can
/// only ever be the last one; that is what bounds the tolerance to one.
fn read_json_lines_tolerant<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    what: &str,
) -> Result<Vec<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if let Some(last) = lines.last() {
        if serde_json::from_str::<T>(last).is_err() {
            lines.pop();
        }
    }
    let mut events = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        events.push(
            serde_json::from_str(line)
                .map_err(|e| format!("corrupt {what} line {}: {e}", idx + 1))?,
        );
    }
    Ok(events)
}

/// The transition-log read ([`read_json_lines_tolerant`] over the
/// [`TransitionEvent`] records — the same records the strict
/// [`Supervisor::read_events`] serves, with the resync torn-tail posture).
///
/// [`Supervisor::read_events`]: super::supervisor::Supervisor::read_events
fn read_events_tolerant(
    path: &std::path::Path,
    what: &str,
) -> Result<Vec<TransitionEvent>, String> {
    read_json_lines_tolerant(path, what)
}

/// The breach-log read ([`read_json_lines_tolerant`] over the
/// [`BudgetBreachEvent`] records — the same records the strict
/// [`Supervisor::read_breach_events`] serves, with the resync torn-tail
/// posture).
///
/// [`Supervisor::read_breach_events`]: super::supervisor::Supervisor::read_breach_events
fn read_breach_events_tolerant(
    path: &std::path::Path,
    what: &str,
) -> Result<Vec<BudgetBreachEvent>, String> {
    read_json_lines_tolerant(path, what)
}

#[cfg(test)]
mod tests {
    use super::super::event::TransitionCause;
    use super::super::LifecycleState;
    use super::*;

    fn transition(seq: usize) -> TransitionEvent {
        TransitionEvent::new(
            "probe".to_string(),
            LifecycleState::Running,
            LifecycleState::Stopped,
            TransitionCause::StopGraceful,
            format!("2026-09-10T00:00:{seq:02}Z"),
        )
    }

    fn breach(seq: u64) -> BudgetBreachEvent {
        BudgetBreachEvent::new(
            "probe",
            "run-1",
            super::super::BreachScope::Cumulative,
            90,
            90 + seq,
            super::super::BreachAction::Pause,
            "self-reported",
            format!("2026-09-10T00:00:{seq:02}Z"),
        )
    }

    fn usage(seq: u64) -> UsageEvent {
        UsageEvent {
            instance: "probe".to_string(),
            run_id: super::super::RunId::from_wire("run-1"),
            input_tokens: 10,
            output_tokens: 20,
            metering_source: "self-reported".to_string(),
            sequence: seq,
            occurred_at: format!("2026-09-10T00:00:{seq:02}Z"),
        }
    }

    #[test]
    fn assemble_wraps_every_family_family_major_and_advances_the_cursor() {
        let transitions: Vec<_> = (0..2).map(transition).collect();
        let breaches: Vec<_> = (0..2).map(breach).collect();
        let usage: Vec<_> = (0..2).map(usage).collect();

        let batch = assemble(&transitions, &breaches, &usage, &ResyncCursor::START);
        assert_eq!(batch.events.len(), 6, "every record converts");
        assert_eq!(
            batch.events[0],
            EngineEvent::Transition(transitions[0].clone())
        );
        assert_eq!(
            batch.events[1],
            EngineEvent::Transition(transitions[1].clone())
        );
        assert_eq!(
            batch.events[2],
            EngineEvent::BudgetBreach(breaches[0].clone())
        );
        assert_eq!(
            batch.events[3],
            EngineEvent::BudgetBreach(breaches[1].clone())
        );
        assert_eq!(
            batch.events[4],
            EngineEvent::UsageUpdate(UsageUpdateEvent::new(usage[0].clone()))
        );
        assert_eq!(
            batch.events[5],
            EngineEvent::UsageUpdate(UsageUpdateEvent::new(usage[1].clone()))
        );
        // The cursor names the consumed position per family — the next call
        // with it skips everything (idempotence below).
        assert_eq!(
            batch.cursor,
            ResyncCursor {
                transitions: 2,
                breaches: 2,
                usage: 2
            }
        );
    }

    #[test]
    fn assemble_skips_the_cursor_prefix_per_family() {
        let transitions: Vec<_> = (0..3).map(transition).collect();
        let breaches: Vec<_> = (0..1).map(breach).collect();
        let usage: Vec<_> = (0..2).map(usage).collect();
        let after = ResyncCursor {
            transitions: 2,
            breaches: 0,
            usage: 1,
        };

        let batch = assemble(&transitions, &breaches, &usage, &after);
        // Exactly the unconsumed suffix per family: one transition, one
        // breach, one usage row — nothing re-delivered.
        assert_eq!(
            batch.events,
            vec![
                EngineEvent::Transition(transitions[2].clone()),
                EngineEvent::BudgetBreach(breaches[0].clone()),
                EngineEvent::UsageUpdate(UsageUpdateEvent::new(usage[1].clone())),
            ]
        );
        assert_eq!(batch.cursor.usage, 2);
    }

    #[test]
    fn assemble_clamps_a_cursor_past_the_family_end() {
        // A recreated/truncated log (fewer lines than the cursor names)
        // degrades to "whatever is there" — the skip clamps, nothing panics,
        // nothing goes negative.
        let transitions: Vec<_> = (0..1).map(transition).collect();
        let after = ResyncCursor {
            transitions: 99,
            breaches: 99,
            usage: 99,
        };

        let batch = assemble(&transitions, &[], &[], &after);
        assert!(batch.events.is_empty(), "everything is already consumed");
        assert_eq!(
            batch.cursor,
            ResyncCursor {
                transitions: 1,
                breaches: 0,
                usage: 0
            }
        );
    }

    #[test]
    fn assemble_of_nothing_is_an_empty_batch_at_zero() {
        let batch = assemble(&[], &[], &[], &ResyncCursor::START);
        assert_eq!(
            batch,
            ResyncBatch {
                events: Vec::new(),
                cursor: ResyncCursor::START,
            }
        );
    }

    #[test]
    fn resync_reads_survive_a_torn_trailing_line() {
        // The crash-recovery posture: the crash tore the log's trailing
        // append; the read returns the good prefix instead of failing the
        // heal. Both log families share the reader.
        let dir = tempfile::TempDir::new().unwrap();
        let good = serde_json::to_string(&transition(0)).unwrap();
        let path = dir.path().join("instance.log");
        std::fs::write(&path, format!("{good}\n{{\"schema_version\":1,\"inst")).unwrap();
        let events: Vec<TransitionEvent> =
            read_events_tolerant(&path, "instance-log").expect("torn tail tolerated");
        assert_eq!(events, vec![transition(0)]);

        let good_breach = serde_json::to_string(&breach(0)).unwrap();
        let breach_path = dir.path().join("breaches.log");
        std::fs::write(&breach_path, format!("{good_breach}\n")).unwrap();
        let breaches: Vec<BudgetBreachEvent> =
            read_breach_events_tolerant(&breach_path, "breach-log").expect("complete log parses");
        assert_eq!(breaches, vec![breach(0)]);
    }

    #[test]
    fn resync_reads_still_reject_a_malformed_interior_line() {
        // Torn-tail tolerance is bounded: an unparseable NON-trailing line is
        // not a race — it is the wrong file or a corrupting engine, and the
        // typed error names it (mirrors the strict readers' convention).
        let dir = tempfile::TempDir::new().unwrap();
        let good = serde_json::to_string(&transition(0)).unwrap();
        let path = dir.path().join("instance.log");
        std::fs::write(&path, format!("{{\"not\":\"a transition\"}}\n{good}\n")).unwrap();
        let err = read_events_tolerant(&path, "instance-log").unwrap_err();
        assert!(err.contains("corrupt instance-log line 1"), "{err}");
    }

    #[test]
    fn resync_reads_treat_an_absent_log_as_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let events: Vec<TransitionEvent> =
            read_events_tolerant(&dir.path().join("never.log"), "instance-log")
                .expect("absent file is empty");
        assert!(events.is_empty());
    }
}
