//! The lifecycle state machine as DATA (spine AD-15) — the transition table.
//!
//! One place decides "given the current [`LifecycleState`] and a
//! [`LifecycleCommand`], what is the next state (or the uniform error)". The
//! table is a total `match` over the two enums — which IS "data" per AD-15 (a
//! total function over a finite product is a table; no external data file is
//! needed). The supervisor calls [`next_state`] to decide, then persists the new
//! state and emits the transition event. Keeping this function PURE (no I/O, no
//! adapter, no OS) makes it exhaustively unit-testable and keeps the domain core
//! clean (AD-1).
//!
//! ## Command-driven vs event-driven edges (documented shape)
//!
//! `Start` and `Stop` are OPERATOR **commands** — the rows in [`next_state`].
//! The remaining edges this story reaches are SUPERVISOR reactions to process
//! events, not commands, and are applied directly by the supervisor (each still
//! persisting + emitting the AD-14 event):
//!
//! * `starting → running`  — adapter ready (process spawned and not immediately dead)
//! * `starting → failed`   — launch error
//! * `stopping → stopped`  — process exited (gracefully or after a forced kill)
//!
//! Modeling only the command edges here keeps the AC4 uniform-error contract
//! precise: `InvalidTransition` is returned for a rejected COMMAND (e.g. `Stop`
//! on `stopped`, `Start` on `running`), and the event-driven edges never reject
//! (the supervisor only applies them when the corresponding process event has
//! actually happened).
//!
//! ## Reachable this story
//!
//! `registered → starting → running → stopping → stopped`, plus
//! `starting → failed` (launch error) and the `stopped → starting` restart of a
//! previously stopped instance (FR-5: start applies to registered *or* stopped).
//! `paused` (story 1-5) and the `running → failed` crash edge + Restart Policy
//! (story 1-6) are intentionally NOT wired; the table lists them as commented
//! future rows so the shape is complete.

use thiserror::Error;

use super::lifecycle::LifecycleState;

/// An operator lifecycle command (spine AD-15 verbs).
///
/// Seed set for this story: [`LifecycleCommand::Start`] and
/// [`LifecycleCommand::Stop`]. `Pause`/`Resume` arrive with story 1-5; they are
/// deliberately absent so an attempt to reach `paused` cannot even be expressed
/// yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleCommand {
    /// Start a registered (or previously stopped) instance.
    Start,
    /// Stop a running instance.
    Stop,
    // Pause,  // story 1-5 — not wired here.
    // Resume, // story 1-5 — not wired here.
}

impl LifecycleCommand {
    /// A short, stable label used in diagnostics and the transition event cause.
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleCommand::Start => "start",
            LifecycleCommand::Stop => "stop",
        }
    }
}

impl std::fmt::Display for LifecycleCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The single uniform lifecycle error class (spine AD-15; AC4).
///
/// Every invalid `(state, command)` pair returns
/// [`LifecycleError::InvalidTransition`] — the SAME error for every adapter,
/// because the rejection comes from the shared table in the engine core before
/// any adapter code runs (AC4 "same error class for every adapter"). `thiserror`
/// in the engine; `kt` maps it to a miette diagnostic.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LifecycleError {
    /// The command is not valid from the instance's current state (e.g. `stop`
    /// on `stopped`, `start` on `running`). Names both so the diagnostic is
    /// precise and identical across adapters.
    #[error("cannot {command} an Agent Instance while it is '{from}'")]
    InvalidTransition {
        /// The state the instance was in when the command arrived.
        from: LifecycleState,
        /// The command that was rejected.
        command: LifecycleCommand,
    },
}

/// The transition table (AD-15): `(state, command) -> next state | error`.
///
/// PURE — no I/O. Total over the two enums: every reachable command edge maps to
/// the right next state; every other pair returns the single
/// [`LifecycleError::InvalidTransition`] class (AC4). The supervisor calls this,
/// then persists + emits.
pub fn next_state(
    from: LifecycleState,
    command: LifecycleCommand,
) -> Result<LifecycleState, LifecycleError> {
    use LifecycleCommand::*;
    use LifecycleState::*;
    match (from, command) {
        // Start a registered instance, or restart a previously stopped one
        // (FR-5: start applies to registered OR stopped instances).
        (Registered, Start) => Ok(Starting),
        (Stopped, Start) => Ok(Starting),
        // Stop a running instance.
        (Running, Stop) => Ok(Stopping),
        // Every other (state, command) pair is an invalid COMMAND transition.
        // The event-driven edges (starting→running, starting→failed,
        // stopping→stopped) are applied by the supervisor on process events, not
        // through this command table, so they are not rows here.
        (from, command) => Err(LifecycleError::InvalidTransition { from, command }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LifecycleCommand::*;
    use LifecycleState::*;

    #[test]
    fn start_from_registered_enters_starting() {
        assert_eq!(next_state(Registered, Start), Ok(Starting));
    }

    #[test]
    fn start_from_stopped_restarts_into_starting() {
        // FR-5: a previously stopped instance can be started again.
        assert_eq!(next_state(Stopped, Start), Ok(Starting));
    }

    #[test]
    fn stop_from_running_enters_stopping() {
        assert_eq!(next_state(Running, Stop), Ok(Stopping));
    }

    #[test]
    fn invalid_command_pairs_all_yield_the_same_error_class() {
        // A representative set of invalid pairs (AC4). Each must be the ONE
        // uniform InvalidTransition class, naming from + command.
        let invalid = [
            (Stopped, Stop),    // stop on stopped
            (Running, Start),   // start on running
            (Registered, Stop), // stop on registered
            (Starting, Start),  // start while starting
            (Starting, Stop),   // stop while starting
            (Stopping, Start),  // start while stopping
            (Stopping, Stop),   // stop while stopping
            (Failed, Stop),     // stop on failed
            (Paused, Start),    // start on paused (paused unreachable this story)
            (Paused, Stop),     // stop on paused
        ];
        for (from, command) in invalid {
            let err = next_state(from, command).unwrap_err();
            assert_eq!(
                err,
                LifecycleError::InvalidTransition { from, command },
                "({from:?}, {command:?}) must be InvalidTransition"
            );
        }
    }

    #[test]
    fn exhaustive_over_every_state_command_pair() {
        // AD-15 "exhaustively unit-tested": drive EVERY (state, command) pair and
        // assert the reachable ones map correctly and all others reject. This is
        // the cheapest, most complete coverage in the story.
        let all_states = [
            Registered, Starting, Running, Paused, Stopping, Stopped, Failed,
        ];
        let all_commands = [Start, Stop];
        for from in all_states {
            for command in all_commands {
                let result = next_state(from, command);
                let expected = match (from, command) {
                    (Registered, Start) => Ok(Starting),
                    (Stopped, Start) => Ok(Starting),
                    (Running, Stop) => Ok(Stopping),
                    (from, command) => Err(LifecycleError::InvalidTransition { from, command }),
                };
                assert_eq!(result, expected, "({from:?}, {command:?})");
            }
        }
    }

    #[test]
    fn error_message_names_state_and_command() {
        let err = next_state(Stopped, Stop).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("stop"), "{msg}");
        assert!(msg.contains("stopped"), "{msg}");
    }

    #[test]
    fn command_labels_are_stable() {
        assert_eq!(Start.as_str(), "start");
        assert_eq!(Stop.as_str(), "stop");
        assert_eq!(Start.to_string(), "start");
    }
}
