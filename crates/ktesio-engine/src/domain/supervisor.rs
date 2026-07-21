//! The lifecycle supervisor (spine AD-1, AD-4, AD-12/AD-14/AD-15 seeds).
//!
//! The supervisor owns the running Agent Instances' process handles in memory
//! for the current engine lifetime and drives every lifecycle transition:
//!
//! 1. apply the transition table ([`next_state`](super::transition::next_state)),
//! 2. spawn / stop via the per-OS
//!    [`ProcessBackend`](crate::ports::ProcessBackend) (selected in
//!    `backends/mod.rs`; the supervisor names only the trait + cfg-selected
//!    aliases — it is cfg-free),
//! 3. persist the new state via the [`Registry`], and
//! 4. emit the [`TransitionEvent`] (append to the per-instance log + return it).
//!
//! ## Cross-lifetime supervision (AD-5, story 1-6: IMPLEMENTED)
//!
//! The running-handle map lives for THIS engine's lifetime, but the write-ahead
//! spawn records (AD-5) persist across lifetimes. Story 1-6 IMPLEMENTS orphan
//! adoption: [`Supervisor::adopt_orphans`] (called from [`Engine::open`]) reads
//! every persisted [`SpawnRecord`] and re-attaches to a still-live process whose
//! start-time fingerprint matches (`backend.adopt`), re-populating the handle map
//! so `stop`/`pause`/`poll` work on it again; a record whose process is gone (or
//! whose PID was reused) reconciles to `failed`. So a process started by a prior
//! engine that CRASHED is now re-adopted (or honestly failed) — the single-
//! lifetime boundary is lifted for the durable-record case.
//!
//! ## Crash detection + Restart Policy (AD-5/AD-15, story 1-6)
//!
//! [`Supervisor::poll_once`] is the reaper: it polls every held handle via the
//! EXISTING `backend.poll` and, on an unrequested `Exited` for an instance the
//! store still shows `running`/`paused`, applies the EVENT-driven `running →
//! failed` edge (a [`TransitionCause::Crashed`]) and consults the per-instance
//! [`RestartPolicy`] to decide whether to schedule a restart (returning a
//! [`RestartPlan`] the engine cadence times). The reaper + restart executor stay
//! SYNC + cfg-free; the engine owns the poll interval and the backoff timer.
//!
//! ## What "an event" is here (AD-14 seed)
//!
//! Each transition RECORDS a [`TransitionEvent`] to the per-instance log and
//! returns it (observable to tests / embedders). This is NOT the 7-2 bounded
//! subscription bus — only the seed struct + its recording.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ktesio_adapter_api::{Capability, OsId, SupportLevel};

use crate::adapter::{self, ConfigApplyError, LaunchResolveError};
use crate::backends;
use crate::metering::{ListenerError, ObservedListener};
use crate::ports::{
    assemble_usage_event, BackendError, LogCapture, ObservedUsageSource, ParsedUsage,
    ProcessBackend, ProcessStatus, SelfReportedUsageSource, SpawnRecord, SpawnSpec, UsageSource,
    KILL_CONFIRM_TIMEOUT, LOG_ROTATE_GENERATIONS,
};
use crate::time::now_rfc3339;

use super::budget::{BreachAction, BreachDecision, BreachScope, BudgetEvaluator};
use super::config::{self, ConfigLayer};
use super::cost::{CostEvaluator, EstimateLabel, Micros};
use super::error::EngineError;
use super::event::{
    BreachDimension, BudgetBreachEvent, LogLine, LogStream, TransitionCause, TransitionEvent,
};
use super::instance::AgentInstance;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::registry::Registry;
use super::restart::{is_crash_loop, BackoffSchedule, RestartPolicy, MAX_CONSECUTIVE_FAILURES};
use super::transition::{next_state, LifecycleCommand};
use super::usage::{RecordOutcome, RunId, UsageUpdateEvent};

/// The default graceful-shutdown window before a stop escalates to a forced kill
/// (AC3). Per-instance configurable via [`Supervisor::stop`]'s `window` argument;
/// this is the conservative fallback when the caller passes `None`.
pub const DEFAULT_STOP_WINDOW: Duration = Duration::from_secs(30);

/// How long to watch a freshly spawned process for an immediate failure before
/// declaring it `running` (the readiness definition, `[ASSUMPTION]`).
///
/// "Adapter ready" this story = "the process spawned and did not die during this
/// short startup window". A process that exits (especially non-zero) within it is
/// treated as a launch failure (AC2 "immediate non-zero exit during startup").
/// Kept small so `start` stays snappy; the fake test agent's `--exit-fast` path
/// exits well inside it.
const READINESS_WINDOW: Duration = Duration::from_millis(300);

/// How often the readiness watch polls the freshly spawned process.
const READINESS_POLL: Duration = Duration::from_millis(10);

/// A scheduled restart of a crashed instance (story 1-6, AC4). Returned by
/// [`Supervisor::poll_once`] for each crashed `on-failure` instance that has not
/// hit the crash-loop threshold; the engine cadence sleeps [`RestartPlan::delay`]
/// then calls [`Supervisor::restart`] with the plan's `attempt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartPlan {
    /// The instance to restart.
    pub name: InstanceName,
    /// The consecutive restart attempt number (1-based) this plan represents.
    pub attempt: u32,
    /// The backoff to wait before performing the restart.
    pub delay: Duration,
}

/// The Restart Policy outcome for a just-crashed instance (internal to the
/// reaper). Carries the crash cause to record in the event log — enriched with
/// the policy conclusion on a terminal outcome so the failed cause survives after
/// the write-ahead record is cleared — and, when a restart is scheduled, the
/// [`RestartPlan`] the engine cadence should time.
struct RestartDecision {
    /// The crash cause detail to record on the `running → failed` event.
    crash_cause: String,
    /// The restart to schedule, or `None` on a terminal (`never`/crash-loop) outcome.
    plan: Option<RestartPlan>,
}

/// How [`Supervisor::drain_usage_for`] treats the tail of the agent-output log
/// (story 3-1 under-count fix, H1) — the difference is whether a final line that
/// lacks a trailing newline is consumed now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrainMode {
    /// The process is (believed) still alive — the reaper cadence. Consume only up
    /// to the last newline; a partial final line may still be completed, so it waits
    /// for the next pass.
    MidRun,
    /// The process is DEAD (drain-on-stop / drain-on-reap) — no more bytes will ever
    /// append. Consume the WHOLE tail, INCLUDING a final newline-less line, so a last
    /// usage line flushed without a trailing `\n` is not stranded and lost when the
    /// next Run's cursor anchors past it.
    Terminal,
}

/// What a single [`Supervisor::drain_usage_for`] pass should do with the captured
/// log, decided purely from `(bytes, cursor, mode)` (story 3-1 — the H1 terminal-
/// tail rule + the M2 shrink guard, unit-testable without a process handle).
#[derive(Clone, Debug, PartialEq, Eq)]
enum DrainPlan {
    /// The log shrank below the cursor (a truncate/rotation — M2). Snap the cursor
    /// to `new_cursor` (the new length) and ingest NOTHING — never re-read from 0
    /// under the same live `run_id` (that would double-count → an inflated bill).
    Shrunk { new_cursor: u64 },
    /// No complete unit to consume this pass (an empty tail, or a MidRun tail with no
    /// newline yet). Leave the cursor where it is.
    Nothing,
    /// Consume `bytes[range]` and set the cursor to `new_cursor`.
    Consume {
        range: std::ops::Range<usize>,
        new_cursor: u64,
    },
}

/// Decide what one drain pass reads (spine AD-7; story 3-1 H1/M2). Pure — no I/O.
///
/// * Shrink (M2): `cursor > len` ⇒ [`DrainPlan::Shrunk`] (snap to `len`, ingest
///   nothing) — the anti-double-count fallback for a truncated/rotated log.
/// * Otherwise consume the tail `bytes[cursor..]`:
///   - [`DrainMode::Terminal`] consumes the WHOLE tail (the process is dead; a
///     final newline-less usage line must land now or be lost — H1).
///   - [`DrainMode::MidRun`] consumes only up to the last `\n` (a live process may
///     still complete a partial final line on a later pass); no newline ⇒ nothing.
fn plan_drain(bytes: &[u8], cursor: u64, mode: DrainMode) -> DrainPlan {
    let len = bytes.len() as u64;
    if cursor > len {
        return DrainPlan::Shrunk { new_cursor: len };
    }
    let start = cursor as usize;
    let tail = &bytes[start..];
    let consumable = match mode {
        DrainMode::Terminal => tail.len(),
        DrainMode::MidRun => match tail.iter().rposition(|b| *b == b'\n') {
            Some(pos) => pos + 1, // include the newline
            None => 0,            // no complete line yet — nothing to consume
        },
    };
    if consumable == 0 {
        return DrainPlan::Nothing;
    }
    DrainPlan::Consume {
        range: start..start + consumable,
        new_cursor: cursor + consumable as u64,
    }
}

/// What one `Supervisor::read_agent_log_since` poll should do, decided purely
/// from `(bytes, cursor)` (story 4-2, Task 5, AC-D/AC-H/AC-G). MIRRORS (does
/// NOT literally reuse) [`plan_drain`]'s shrink-guard + "consume only up to
/// the last complete newline" shape — deliberately kept as an INDEPENDENT
/// pure function rather than a shared generalization: this is Epic 4's READ
/// path, `plan_drain` is Epic 3's adversarially-reviewed BILLING ingestion
/// path (story 3-1/AD-7), and coupling them would put a change to one at risk
/// of silently affecting the other's already-hardened behavior (a
/// genericization was evaluated and deliberately NOT taken — Task 1's Dev
/// Notes).
#[derive(Clone, Debug, PartialEq, Eq)]
enum FollowPlan {
    /// The file is SHORTER than the cursor — a rotation happened since the
    /// last poll. Snap the cursor to `new_cursor` (the file's new length) and
    /// deliver nothing new THIS pass; the caller detects the snap-back
    /// (`new_cursor < cursor`) and prints the one-line rotation notice
    /// (Task 6) — never a claim of completeness across the boundary.
    Shrunk { new_cursor: u64 },
    /// Consume `bytes[range]` (a whole number of COMPLETE lines only — a
    /// trailing partial line, if any, waits for the next poll, exactly like
    /// `plan_drain`'s MidRun tail rule) and advance the cursor to
    /// `new_cursor`.
    Consume {
        range: std::ops::Range<usize>,
        new_cursor: u64,
    },
}

/// Decide what one `read_agent_log_since` poll reads. Pure — no I/O.
fn plan_follow(bytes: &[u8], cursor: u64) -> FollowPlan {
    let len = bytes.len() as u64;
    if cursor > len {
        return FollowPlan::Shrunk { new_cursor: len };
    }
    let start = cursor as usize;
    let tail = &bytes[start..];
    let consumable = match tail.iter().rposition(|b| *b == b'\n') {
        Some(pos) => pos + 1,
        None => 0,
    };
    FollowPlan::Consume {
        range: start..start + consumable,
        new_cursor: cursor + consumable as u64,
    }
}

/// The in-memory supervision state for ONE running Agent Instance (story 3-1).
///
/// Beyond the process [`Handle`](backends::Handle) the supervisor has always held,
/// this carries the metering context ingestion needs during the instance's Run:
/// the current [`RunId`] (minted at `starting`, spine AD-7), the declared metering
/// source (its wire string, stamped on every ingested [`UsageEvent`]), and a byte
/// CURSOR into the per-instance agent-output log so each reaper pass ingests only
/// the NEWLY-captured tail (never re-reading — and never re-attributing a prior
/// Run's lines under a fresh Run id after a stop→start). It lives for THIS engine
/// lifetime alongside the handle, exactly like the handle map it replaced.
struct Supervised {
    /// The backend-owned process handle (group/job control).
    handle: backends::Handle,
    /// The current Run this instance is in (spine AD-7) — minted at `starting`.
    run_id: RunId,
    /// The declared Metering Source wire string (`self-reported` / `engine-observed`),
    /// stamped on every [`UsageEvent`] ingested during this Run.
    metering_source: String,
    /// Byte offset already consumed from the agent-output log — the ingestion read
    /// cursor. Advanced past each block the drain reads, so lines are ingested at
    /// most once from the capture (the DB dedup is the second, authoritative guard).
    usage_cursor: u64,
    /// The per-Run breach LATCH (story 3-2 idempotence fix; story 3-3 keyed by
    /// dimension): the set of `(dimension, scope)` pairs that have ALREADY fired a
    /// breach for THIS Run. Enforcement (`enforce_budget`) runs on EVERY committed
    /// usage event, but a breach must fire **at most once per (dimension, scope) per
    /// Run** — otherwise every post-crossing event re-records a `BudgetBreachEvent`
    /// and re-fires the action (unbounded duplicate records for `warn`; redundant
    /// records for `pause`/`stop`). A pair is inserted the first time it trips; a
    /// subsequent event whose pair is already latched short-circuits BOTH the record
    /// and the action.
    ///
    /// STORY 3-3 — DIMENSION KEY: the latch key is `(BreachDimension, BreachScope)`
    /// so a TOKEN breach and a DOLLAR breach of the SAME scope latch INDEPENDENTLY —
    /// each fires once per Run (a run can legitimately trip both its token ceiling
    /// and its dollar cap; the action is identical, so both fire once each). The
    /// per-run and cumulative scopes still latch independently within each dimension.
    /// The latch lives on `Supervised`, so it RESETS automatically when a new Run
    /// starts — a fresh `Supervised` (built at `starting`, where the `run_id` is
    /// freshly minted) begins empty, giving "at most one breach per (dimension,
    /// scope) per Run".
    breached_scopes: std::collections::HashSet<(BreachDimension, BreachScope)>,
    /// The per-instance loopback forward listener for an `engine-observed` instance
    /// (story 3-4), or `None` for a `self-reported` instance (whose start path is
    /// UNCHANGED). Held for the Run; DROPPED at the terminal transition (which
    /// aborts its accept-loop task — teardown bounded to the Run, no orphan
    /// listeners, NFR-1). A restart opens a NEW listener under the new Run.
    observed_listener: Option<ObservedListener>,
    /// The `engine-observed` source (story 3-4): the per-Run monotonic `sequence`
    /// minter for observed completions (the agent supplies no ordinal). Fresh per
    /// Run (built here with the freshly-minted `run_id`), so the ordinal resets per
    /// Run — preserving the `UNIQUE(instance_id, run_id, sequence)` dedup invariant.
    /// Present only for an `engine-observed` instance (a `self-reported` instance
    /// leaves it `None` and drives the log-tail `drain_usage_for` instead).
    observed_source: Option<ObservedUsageSource>,
    /// Set when a PRIOR [`Supervisor::stop`] call on this handle's `stop_inner`
    /// pass got [`BackendError::StopUnconfirmed`] back from the backend (fix
    /// pass, review of #80 follow-up — the CRITICAL finding): SIGKILL was sent
    /// but death could not be confirmed within [`KILL_CONFIRM_TIMEOUT`], most
    /// likely because the process is stuck in an OS-level uninterruptible I/O
    /// wait. Defaults `false` for a freshly started OR adopted instance (an
    /// ordinary stop attempt never sets it). Lets BOTH a RETRY `stop()` call
    /// (see `stop_inner`'s docs) and the crash reaper (`poll_once`) recognize
    /// "this handle's death is pending reconciliation" — via a cheap,
    /// NON-BLOCKING liveness poll, never a repeat of the whole bounded
    /// SIGTERM/SIGKILL/confirm sequence — distinctly from an ORDINARY
    /// in-flight stop or an externally-forced `stopping` row (neither of
    /// which ever sets this flag), so this fix pass changes behavior ONLY
    /// for the specific scenario it targets.
    stop_unconfirmed: bool,
}

/// The lifecycle supervisor: owns running process handles + drives transitions.
///
/// Constructed empty by [`Engine::open`](crate::Engine::open). Holds ONE
/// [`ProcessBackend`](crate::ports::ProcessBackend) (the current OS's), a map of
/// the instances it currently supervises (each with its process handle + metering
/// context, story 3-1), the self-reported [`UsageSource`](crate::ports::UsageSource)
/// ingestion adapter, and the [`BackoffSchedule`] the restart executor uses
/// (production 1s×2 cap 60s; tests inject a scaled one).
pub struct Supervisor {
    backend: backends::Backend,
    running: HashMap<InstanceName, Supervised>,
    usage_source: SelfReportedUsageSource,
    backoff: BackoffSchedule,
    /// The engine's tokio runtime handle (story 3-4), used to SPAWN the loopback
    /// forward listener's accept loop for an `engine-observed` instance. The
    /// supervisor's sync start path runs on the blocking pool, so it cannot use
    /// `Handle::current`; the engine threads its handle in via
    /// [`Supervisor::with_runtime`]. `None` (the [`Supervisor::new`]/
    /// [`Supervisor::with_backoff`] default) means "no runtime to spawn a
    /// listener" — an `engine-observed` start then fails fast with a clear error
    /// (only the sync unit tests, which never start an observed instance, use the
    /// handle-less constructors).
    runtime: Option<tokio::runtime::Handle>,
}

impl Supervisor {
    /// Construct an empty supervisor with the current OS's process backend and
    /// the PRODUCTION backoff schedule (1s base, ×2, 60s cap — spine AD-15).
    ///
    /// NO runtime handle (story 3-4) — so this cannot start an `engine-observed`
    /// listener. Production uses [`Supervisor::with_runtime`] (the engine threads
    /// its runtime handle in); this handle-less form remains for the sync unit
    /// tests that only exercise self-reported / lifecycle paths.
    pub fn new() -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
            usage_source: SelfReportedUsageSource::new(),
            backoff: BackoffSchedule::production(),
            runtime: None,
        }
    }

    /// Construct an empty supervisor with the PRODUCTION backoff schedule AND the
    /// engine's tokio runtime handle (story 3-4) — the production constructor the
    /// engine uses. The handle lets an `engine-observed` start SPAWN its loopback
    /// forward listener's accept loop on the engine runtime (the supervisor's sync
    /// start path runs on the blocking pool, so `Handle::current` is unavailable;
    /// a `Handle` spawns onto its runtime from any thread).
    pub fn with_runtime(runtime: tokio::runtime::Handle) -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
            usage_source: SelfReportedUsageSource::new(),
            backoff: BackoffSchedule::production(),
            runtime: Some(runtime),
        }
    }

    /// Construct an empty supervisor with a custom backoff schedule (TEST
    /// injection, so the crash-loop / backoff legs run in milliseconds without
    /// weakening the production constants). Production always uses
    /// [`Supervisor::with_runtime`]. NO runtime handle — the lib tests using this
    /// never start an `engine-observed` instance.
    #[cfg(test)]
    pub(crate) fn with_backoff(backoff: BackoffSchedule) -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
            usage_source: SelfReportedUsageSource::new(),
            backoff,
            runtime: None,
        }
    }

    /// Start a registered / previously stopped / FAILED Agent Instance
    /// (AC1/AC2; AC3 restart-from-failed via the 1-6 transition row).
    ///
    /// Thin wrapper over [`Supervisor::start_inner`] with no restart context (a
    /// fresh operator `start`): the `starting → running` transition records a
    /// plain [`TransitionCause::AdapterReady`], and the write-ahead spawn record's
    /// restart count is RESET to 0 (a clean run resets the count, AC4).
    pub fn start(&mut self, registry: &Registry, name: &str) -> Result<AgentInstance, EngineError> {
        self.start_inner(registry, name, None)
    }

    /// The shared start path (AC1/AC2 + the 1-6 write-ahead record commit).
    ///
    /// `restart`:
    /// * `None` — a fresh `start` (operator or first launch). The
    ///   `starting → running` cause is [`TransitionCause::AdapterReady`]; the
    ///   spawn record's restart count is RESET to 0.
    /// * `Some((attempt, waited))` — a Restart Policy restart (from
    ///   [`Supervisor::restart`]). The `starting → running` cause is
    ///   [`TransitionCause::Restarted`] recording the consecutive `attempt` +
    ///   the backoff `waited`; the record keeps that count.
    ///
    /// Order (so a rejection leaves NO spurious state change):
    /// 1. look up + validate `Start` against the transition table (AC4),
    /// 2. resolve the launch spec (a bad/native-only adapter rejects here),
    /// 3. persist `registered/stopped/failed → starting` + emit,
    /// 4. spawn; a spawn failure → `starting → failed` (diagnostic preserved),
    /// 5. readiness watch: an immediate death → `failed` (AC2),
    /// 6. **commit the write-ahead spawn record** (AD-5: `{pid, fingerprint}` +
    ///    policy + count) BEFORE the instance is treated as supervised — "no
    ///    spawn without its record committed first",
    /// 7. persist `starting → running` + emit, store the handle, return.
    fn start_inner(
        &mut self,
        registry: &Registry,
        name: &str,
        restart: Option<(u32, Duration)>,
    ) -> Result<AgentInstance, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // (1) Transition gate (AC4). Rejects (e.g. start on running) before any
        // side effect and BEFORE we touch the backend.
        let starting = next_state(instance.state, LifecycleCommand::Start)?;

        // (2) Resolve the launch spec (may reject: native-only / bad manifest),
        // still before any persisted state change.
        let (kind, manifest_path, persisted_launch) = registry
            .adapter_launch_facts(&name)
            .map_err(registry_to_engine)?;
        // Prefer the launch SNAPSHOTTED at registration — this removes the fragile
        // start-time manifest re-read that dropped `args` on hosted CI runners
        // (the agent spawned with the right binary but ZERO args). Fall back to
        // re-reading the manifest ONLY when the snapshot carries no launch: a
        // native adapter (→ NativeHasNoLaunch, preserved) or an instance
        // registered before the launch was persisted (legacy snapshot).
        let mut launch = match persisted_launch {
            Some(launch) => launch,
            None => adapter::resolve_start_launch(&kind, manifest_path.as_deref())
                .map_err(|e| launch_to_engine(&name, e))?,
        };

        // Read the declared Metering Source (story 3-1) from the persisted adapter
        // snapshot — stamped on every UsageEvent ingested during this Run. Read here
        // (a pure snapshot read) before any side effect; a corrupt snapshot surfaces
        // the same way the launch-facts read above would.
        let metering_source = registry
            .metering_source(&name)
            .map_err(registry_to_engine)?;

        // Read the effective (current-OS) Capability::Interaction level (story
        // 4.1 fix pass, HIGH finding — review of #79) to decide whether THIS
        // spawn should pipe stdin at all. The story's original implementation
        // piped UNCONDITIONALLY for every process; an adversarial audit showed
        // this can hang an adapter that declares no interaction support: a
        // process that blocks reading stdin at startup (a common "sniff for
        // piped input" real-CLI idiom) never sees EOF, because the engine
        // holds the pipe's write end open for the process's whole supervised
        // lifetime and nothing ever writes to it unless `send` is called — the
        // child hangs forever yet is reported `running` (readiness here is
        // just "the process didn't exit immediately"), a silent deadlock with
        // no error signal anywhere. Mirrors how the rest of this codebase
        // gates BEHAVIOR (not just callability) on declared capabilities
        // (e.g. pause's SIGSTOP-vs-noop branching). Read here (a pure
        // snapshot read, mirroring `metering_source` above) before any side
        // effect, so a corrupt snapshot rejects the start cleanly like every
        // other pre-transition read.
        let interaction_level = registry
            .effective_support(&name, Capability::Interaction)
            .map_err(registry_to_engine)?;
        let pipe_stdin = matches!(
            interaction_level,
            SupportLevel::Guaranteed | SupportLevel::BestEffort
        );

        // (2b) Map the resolved unified config into the adapter's NATIVE mechanism
        // (story 2-2, FR-12) — still before any persisted state change, so a
        // config/mapping failure rejects the start cleanly (no spurious state
        // change, no half-launched process). Resolve the instance's effective
        // config (2-1's four-layer fold; empty invocation overrides for a plain
        // start — the parameter is threaded so a future `start --set k=v` supplies
        // it without an API change), the adapter's declared mapping (manifest
        // `[config]` or the native code-declared table), then apply: known keys
        // land in their declared native target (env → launch.env; flag →
        // launch.args; file → a rendered file in the Agent Home), and `agent.*`
        // pass-through leaves are delivered VERBATIM (AC6). The Agent Home already
        // exists (created at registration); file targets render into it here.
        let home = registry.agent_home(&name);
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .map_err(|e| config_to_engine(&name, e))?;
        let mapping = adapter::resolve_config_mapping(&kind, manifest_path.as_deref())
            .map_err(|e| launch_to_engine(&name, e))?;

        // (2b-observed) ENGINE-OBSERVED metering (story 3-4, AC-A/AC6): for an
        // `engine-observed` instance, START the loopback forward listener HERE
        // (before the mapping application + the `starting` transition, so a listener
        // failure rejects the start cleanly with NO state change — mirroring the
        // secret/snapshot failures), then INJECT its loopback `http://127.0.0.1:<port>`
        // address as a `metering.base_url` INVOCATION-OVERRIDE so the adapter's
        // EXISTING config-mapping (2-2) delivers it into the agent's native mechanism
        // (e.g. env `OPENAI_BASE_URL`). The address is ENGINE-computed (the engine is
        // the sole authority — AC-B); the adapter merely receives it. A `self-reported`
        // instance leaves `observed_listener` None and its start path UNCHANGED. The
        // held listener is moved into `Supervised` on success; on any later start
        // failure its `Drop` aborts the accept-loop task (RAII teardown, no leak).
        let observed_listener =
            self.start_observed_listener(&name, &metering_source, &effective)?;
        // The effective config the MAPPING applies: for an observed instance it
        // carries the engine-injected loopback base_url as an override (so the mapping
        // delivers it); otherwise it is the plain operator config. The SNAPSHOT (2c)
        // below stays on the plain `effective` (the operator config), so the ephemeral
        // loopback URL is NOT persisted as "what applied" — honest provenance.
        let mapping_effective = match observed_listener.as_ref() {
            Some(listener) => registry
                .effective_config(&name, base_url_override(listener.base_url()))
                .map_err(|e| config_to_engine(&name, e))?,
            None => effective.clone(),
        };

        // (2b-secret) Resolve every `secret:NAME` leaf into a SecretString BEFORE
        // the mapping application (story 2-4, spine AD-10, AC-A/AC9). This is where
        // display and delivery DIVERGE: `effective`'s `display()`-based surfaces
        // (the snapshot at (2c), `config get`) stay MASKED, but the resolved
        // cleartext flows into `apply_config_mapping` so the ADAPTER gets a usable
        // key. Resolution (env → the 0600 secrets file) runs here, still before any
        // persisted state change, so an unresolved/ill-permissioned secret REJECTS
        // the start cleanly (no half-launch, mirroring the config-apply + snapshot
        // failures) — a typed `EngineError::Secret` that NEVER echoes a value.
        let secrets = registry
            .resolve_secrets(&mapping_effective)
            .map_err(|e| secret_to_engine(&name, e))?;
        adapter::apply_config_mapping(&mut launch, &mapping, &mapping_effective, &secrets, &home)
            .map_err(|e| config_apply_to_engine(&name, e))?;

        // (2c) Persist the effective-config snapshot into the Agent Home (story
        // 2-3, spine AD-9 "start resolves to an EffectiveConfig snapshot persisted
        // in the Agent Home, every value tagged with its source layer" + AD-6
        // "effective-config snapshots are files inside the Agent Home"). The
        // resolved `effective` is already in hand from (2b); write it HERE, right
        // after the mapping application and BEFORE the `starting` transition below,
        // so a snapshot-write failure rejects the start cleanly (NO state change —
        // exactly mirroring how the config-apply failure at (2b) rejects before the
        // transition). The snapshot is a PROMISED AD-9 artifact (a Host/debugging
        // record of "what will apply on next start"), not a best-effort nicety, so
        // its failure is a typed start error. Because RESTART also flows through
        // this path (story 1-6), the snapshot is refreshed on restart too (AC7:
        // OVERWRITTEN every successful start/restart, never a stale resolution). It
        // is NOT written at registration (there is no "effective at start" until a
        // start happens) and NOT deleted at stop.
        registry
            .write_effective_config_snapshot(&name, &effective)
            .map_err(snapshot_to_engine)?;

        // Read the per-instance Restart Policy so the write-ahead record carries
        // it (AD-15 per-instance configurable). Read once, before any side effect.
        let policy = registry
            .effective_restart_policy(&name)
            .map_err(registry_to_engine)?;

        // The spawned agent's stdout/stderr go to a SEPARATE agent.log, never the
        // engine's JSON-Lines transition-event log (instance.log) — otherwise the
        // agent's plain-text output would corrupt the structured event log.
        let agent_log_path = registry.agent_output_log_path(&name);
        // Ensure the log directory exists (AD-12 seed) so spawn can redirect
        // stdout/stderr into it and we can append transition events.
        self.ensure_log_dir(registry, &name)?;

        // Anchor the usage-ingestion cursor at the agent-output log length BEFORE
        // the spawn (story 3-1). This Run's own output is appended AFTER this point,
        // so ingestion reads ALL of it — while a PRIOR Run's already-captured lines
        // (a stop→start reuses the same append-only agent.log) stay BEHIND the cursor
        // and are never re-ingested under this fresh Run id. Capturing it HERE (not
        // after the readiness watch below) is essential: a fast agent emits its first
        // usage lines within the ~300ms readiness window, so a cursor set post-
        // readiness would skip them — the ingestion bug this prevents.
        let usage_cursor = self.agent_log_len(registry, &name);

        // (3) registered/stopped/failed → starting.
        self.transition(
            registry,
            &name,
            instance.state,
            starting,
            TransitionCause::command(LifecycleCommand::Start.as_str()),
        )?;

        let spec = SpawnSpec {
            exec: launch.exec.clone(),
            args: launch.args,
            env: launch.env,
            working_dir: home,
            log_file: Some(agent_log_path),
            // Story 4-2 (AC-E): capture is unconditional, computed from the
            // SAME Registry path authority as `log_file` (never gated on
            // `pipe_stdin`/`Capability::Interaction` — that gate governs only
            // the stdin *write* direction).
            attributed_log_path: Some(registry.attributed_output_log_path(&name)),
            // Fix pass (review of #80): the crash-immune raw STDERR capture,
            // computed from the SAME path authority, paired 1:1:1 with
            // `log_file`/`attributed_log_path` (all three Some together).
            stderr_log_file: Some(registry.agent_stderr_log_path(&name)),
            instance_name: name.as_str().to_string(),
            pipe_stdin,
        };

        // (4) Spawn. A spawn failure lands the instance in `failed` with the
        // diagnostic preserved and no zombie (the backend spawned nothing / reaps).
        let mut handle = match self.backend.spawn(&spec) {
            Ok(handle) => handle,
            Err(err) => return Err(self.fail_launch(registry, &name, &err)),
        };

        // (5) Readiness watch: a process that dies immediately (especially
        // non-zero) during startup is a launch failure (AC2). Watch briefly;
        // `watch_startup` returns the exit code (if it died) or `None` (ready).
        if let Some(exit_code) = self.watch_startup(&mut handle) {
            let detail = match exit_code {
                Some(c) => format!("exited immediately during startup with code {c}"),
                None => "exited immediately during startup".to_string(),
            };
            // Reap already done by poll; nothing survives.
            return Err(self.fail_launch_detail(registry, &name, detail));
        }

        // (6) Commit the write-ahead spawn record (AD-5) BEFORE the instance is
        // treated as supervised — "no spawn without its record committed first".
        // A fresh start resets the restart count to 0; a restart keeps its
        // attempt count. The fingerprint is the PID-reuse guard for later
        // orphan adoption. A record-commit failure fails the start (leaving the
        // instance `failed`) — we must not run an unrecorded supervised process.
        let restart_count = restart.map(|(attempt, _)| attempt).unwrap_or(0);
        let record = SpawnRecord {
            name: name.clone(),
            fingerprint: self.backend.fingerprint(&handle),
            restart_policy: policy,
            restart_count,
            last_known_cause: None,
        };
        if let Err(e) = registry.write_spawn_record(&record) {
            // Persisting the record failed: kill the just-spawned process (drop
            // the handle → group/job kill) and land the instance in `failed` so
            // we never supervise an unrecorded process (AD-5 safety).
            drop(handle);
            return Err(self.fail_launch_detail(
                registry,
                &name,
                format!("could not commit the write-ahead spawn record: {e}"),
            ));
        }

        // (7) starting → running (adapter ready, or a Restart Policy restart).
        let ready_cause = match restart {
            Some((attempt, waited)) => {
                TransitionCause::restarted(attempt, waited.as_millis() as u64)
            }
            None => TransitionCause::AdapterReady,
        };
        // Story 4-2, Task 4: `handle` already exists (spawned above) and
        // carries a live `log_capture` (capture is unconditional, AC-E), but
        // it is not YET in `self.running` (inserted below) — so the default
        // `self.transition(...)`'s `self.running`-based lookup would miss
        // it. Pass the capture explicitly so the `starting → running` line
        // lands in the attributed capture too.
        self.transition_with_log_capture(
            registry,
            &name,
            starting,
            LifecycleState::Running,
            ready_cause,
            self.backend.log_capture(&handle),
        )?;
        // Mint the fresh Run id for this `starting`→terminal span (spine AD-7). Each
        // `starting` — operator start OR restart (story 1-6) — mints a distinct id
        // (AC-B), so a restarted instance opens a NEW Run whose per-run totals never
        // bleed in the previous Run's usage. The ingestion cursor was anchored at the
        // pre-spawn log length (above), so this Run ingests all of its own output.
        let run_id = RunId::mint();
        // Story 3-4: an `engine-observed` instance holds its listener + a fresh
        // per-Run observed `sequence` minter (built here with the just-minted
        // run_id, so the ordinal resets per Run — the AD-7 Run boundary + the dedup
        // invariant). A `self-reported` instance leaves both `None` (its log-tail
        // drain is unchanged).
        let observed_source = observed_listener
            .as_ref()
            .map(|_| ObservedUsageSource::new());
        self.running.insert(
            name.clone(),
            Supervised {
                handle,
                run_id,
                metering_source,
                usage_cursor,
                // A fresh Run starts with an EMPTY breach latch (story 3-2): the
                // run_id was just minted, so no scope has fired for it yet. This is
                // how the latch RESETS per Run — a persistently-over-cumulative agent
                // that stops and starts again gets a new Run + a clean latch, so it
                // can fire one cumulative breach in the new Run too.
                breached_scopes: std::collections::HashSet::new(),
                observed_listener,
                observed_source,
                // A fresh start's stop attempt has not happened yet.
                stop_unconfirmed: false,
            },
        );

        registry.lookup(&name).map_err(registry_to_engine)
    }

    /// Perform ONE Restart Policy restart of a crashed instance (story 1-6, AC4).
    ///
    /// Called by the engine cadence AFTER it has waited the backoff
    /// [`RestartPlan::delay`]. Re-runs the start path (`failed → starting →
    /// running`) recording a [`TransitionCause::Restarted`] with the consecutive
    /// `attempt` + the `waited` backoff, and keeps the persisted restart count at
    /// `attempt`.
    ///
    /// Interaction with a concurrent `stop`: `restart` re-runs the start path, so
    /// its transition gate is `next_state(state, Start)`. That gate only accepts
    /// `failed` (or registered/stopped); if the instance was already restarted to
    /// `running` by an EARLIER plan, or an operator stopped it back to `stopped`,
    /// the gate rejects and this restart is a harmless no-op. NOTE: during the
    /// backoff WINDOW the instance is `failed`, and `next_state(Failed, Stop)` is
    /// an `InvalidTransition` — so an operator cannot `stop` a mid-backoff
    /// instance to pre-empt this restart (there is no `failed → stopping` edge
    /// this story; adding one is out of scope). The restart therefore proceeds; a
    /// stop is only effective once the instance is `running` again.
    pub fn restart(
        &mut self,
        registry: &Registry,
        name: &str,
        attempt: u32,
        waited: Duration,
    ) -> Result<AgentInstance, EngineError> {
        self.start_inner(registry, name, Some((attempt, waited)))
    }

    /// Stop a running Agent Instance (AC3/AC4).
    ///
    /// Transitions `running → stopping`, requests graceful shutdown via the
    /// backend and escalates to a forced kill after `window` (default
    /// [`DEFAULT_STOP_WINDOW`]) if needed, records the escalation in the instance
    /// log, then `stopping → stopped`. No process of the instance survives (the
    /// backend kills the whole group/job) — in the NORMAL case.
    ///
    /// **Bounded death confirmation (fix pass, review of #80 follow-up — the
    /// CRITICAL finding):** after escalating to a forced kill, the backend
    /// CONFIRMS death bounded to [`crate::ports::KILL_CONFIRM_TIMEOUT`] (see
    /// its docs for the mechanism: a fast writer can exhaust disk and enter
    /// an OS-level uninterruptible I/O wait immune to every signal, including
    /// the one just sent). If confirmation is not reached within that bound,
    /// this returns [`EngineError::StopUnconfirmed`] instead of continuing to
    /// block — the instance stays `stopping` (never a false `stopped`), and
    /// the handle is RETAINED (not dropped) so the situation can be
    /// reconciled later.
    ///
    /// **No compounding on retry:** a SUBSEQUENT `stop` call against an
    /// instance still `stopping` with a retained (unconfirmed) handle does
    /// NOT re-run the whole SIGTERM/graceful-window/SIGKILL/confirm sequence
    /// — it performs a single cheap, NON-BLOCKING liveness poll instead
    /// (`ProcessBackend::poll`, never `ProcessBackend::stop`). If the process
    /// has since actually exited (the OS condition cleared), this
    /// SELF-HEALS: it completes the stuck `stopping → stopped` transition
    /// right here. If it is still alive, this fails fast with the SAME
    /// honest [`EngineError::StopUnconfirmed`], with no new signal and no new
    /// wait. (The crash-detection reaper's own poll, `poll_once`, performs
    /// the identical reconciliation if it observes the exit first — whichever
    /// happens first, the row does not stay permanently stuck.)
    pub fn stop(
        &mut self,
        registry: &Registry,
        name: &str,
        window: Option<Duration>,
    ) -> Result<AgentInstance, EngineError> {
        self.stop_inner(registry, name, window, None)
    }

    /// Stop driven by a budget BREACH (story 3-2). Identical to [`Supervisor::stop`]
    /// (graceful → forced escalation, story 1-4) except the `running → stopping`
    /// edge carries the [`TransitionCause::BudgetExceeded`] cause instead of a plain
    /// `stop` command, so the lifecycle log explains WHY. The terminal
    /// `stopping → stopped` edge keeps its graceful/forced cause (the escalation
    /// detail). Takes `&InstanceName` (the caller already validated it).
    fn stop_with_cause(
        &mut self,
        registry: &Registry,
        name: &InstanceName,
        cause: TransitionCause,
    ) -> Result<AgentInstance, EngineError> {
        self.stop_inner(registry, name.as_str(), None, Some(cause))
    }

    /// The shared stop driver (story 1-4 + story 3-2 cause override).
    ///
    /// `cause_override`: when `Some`, replaces the `running → stopping` cause
    /// (a budget stop records `BudgetExceeded`); `None` uses the plain `stop`
    /// command cause (an operator `kt agent stop` is unchanged). The terminal edge
    /// always records the graceful/forced escalation cause regardless.
    fn stop_inner(
        &mut self,
        registry: &Registry,
        name: &str,
        window: Option<Duration>,
        cause_override: Option<TransitionCause>,
    ) -> Result<AgentInstance, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // Fix pass (review of #80 follow-up — the CRITICAL finding): a RETRY
        // `stop` against an instance already `stopping` whose handle is
        // marked `stop_unconfirmed` means a PRIOR pass through THIS function
        // already sent SIGKILL but could not confirm death within
        // KILL_CONFIRM_TIMEOUT (`EngineError::StopUnconfirmed`) — most likely
        // because the process is stuck in an OS-level uninterruptible I/O
        // wait. The transition gate below (`next_state`) has no
        // `(Stopping, Stop)` row, so an unmodified retry would either reject
        // with a generic, non-self-healing `InvalidTransition`, or (if that
        // gate were bypassed) re-run the WHOLE SIGTERM/graceful-window/
        // SIGKILL/confirm sequence for an outcome we can already suspect is
        // unchanged — exactly the compounding wait this fix pass closes.
        // Instead: a single cheap, NON-BLOCKING poll (`ProcessBackend::poll`,
        // never `ProcessBackend::stop`) decides the outcome — self-heals if
        // the process has since actually died (the OS condition cleared), or
        // fails fast with the SAME honest error if it is still alive, with NO
        // new signal and NO new wait. Gated specifically on `stop_unconfirmed`
        // (not merely "state is `stopping`") so this new branch changes
        // behavior ONLY for the scenario it targets — an externally-forced
        // `stopping` row with no real stop attempt behind it (as
        // `poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash`
        // exercises) takes the ORIGINAL, unchanged path below.
        if instance.state == LifecycleState::Stopping {
            let stuck = match self.running.get_mut(&name) {
                Some(supervised) if supervised.stop_unconfirmed => {
                    let status = self
                        .backend
                        .poll(&mut supervised.handle)
                        .map_err(|source| EngineError::Backend {
                            name: name.as_str().to_string(),
                            source,
                        })?;
                    let log_capture = self.backend.log_capture(&supervised.handle);
                    Some((status, log_capture))
                }
                _ => None,
            };
            if let Some((status, log_capture)) = stuck {
                if !status.is_exited() {
                    // Still stuck: fail fast, honestly, with no new blocking.
                    return Err(EngineError::StopUnconfirmed {
                        name: name.as_str().to_string(),
                        timeout_secs: KILL_CONFIRM_TIMEOUT.as_secs(),
                    });
                }
                // Self-healing: the process has now actually exited (the OS
                // condition that made confirmation time out has cleared).
                // Complete the stuck `stopping -> stopped` transition exactly
                // as the ordinary path below would have on confirmed death.
                self.running.remove(&name);
                registry
                    .clear_spawn_record(&name)
                    .map_err(registry_to_engine)?;
                self.transition_with_log_capture(
                    registry,
                    &name,
                    LifecycleState::Stopping,
                    LifecycleState::Stopped,
                    TransitionCause::stop_forced(
                        "SIGKILL was sent by an earlier stop attempt; the process's death was \
                         confirmed on a later reconciliation (it may have been stuck in an \
                         OS-level I/O wait that has since cleared)",
                    ),
                    log_capture,
                )?;
                return registry.lookup(&name).map_err(registry_to_engine);
            }
        }

        // Transition gate (AC4): stop on stopped / registered / … rejects here
        // with the uniform InvalidTransition, before touching any process.
        let stopping = next_state(instance.state, LifecycleCommand::Stop)?;

        let window = window.unwrap_or(DEFAULT_STOP_WINDOW);
        self.ensure_log_dir(registry, &name)?;

        // running → stopping (a story-3-2 budget stop overrides the cause).
        self.transition(
            registry,
            &name,
            instance.state,
            stopping,
            cause_override
                .unwrap_or_else(|| TransitionCause::command(LifecycleCommand::Stop.as_str())),
        )?;

        // Drain any final self-reported usage the agent emitted before the stop, so
        // the last batch of a Run is not lost to the race between "agent printed it"
        // and "we killed the process" (story 3-1). TERMINAL drain: the process is
        // about to be gone, so a final newline-less usage line is consumed to
        // end-of-log rather than stranded (H1). Best-effort — a drain hiccup never
        // blocks the stop.
        self.drain_usage_for(registry, &name, DrainMode::Terminal);
        // Drain any final ENGINE-OBSERVED usage still queued before the listener is
        // torn down (story 3-4): a completion the proxy parsed just before the stop
        // must land, not be lost when the `Supervised` (and its listener) is dropped
        // below. Best-effort, mirroring the self-reported terminal drain.
        self.drain_observed_for(registry, &name);

        // Ask the backend to stop the process (group/job). If we have no handle
        // for it (the row says running but this engine holds no handle AND orphan
        // adoption found no live process), the desired end state "no process of
        // the instance survives" already holds, so we treat it as a graceful
        // stop. With story 1-6 adoption, a handle for a still-live process
        // started by a PRIOR engine IS re-held (via `adopt_orphans`), so a
        // cross-restart stop now really terminates it.
        // Story 4-2, Task 4 (fix pass, review of #80): capture the
        // log_capture HERE, before `running.remove` drops the handle below
        // — the default `self.transition(...)` lookup (via `self.running`)
        // would find NOTHING by the time the terminal transition below
        // runs. By the time `backend.stop` (below) returns, the process is
        // provably dead (its raw capture files can never grow again), and
        // `send_engine_line`'s inline catch-up folds in every remaining
        // byte of agent output BEFORE the "-> stopped" line, so the engine
        // line still lands correctly ordered after it, regardless of
        // whether the process handle's `Drop` (which also signals the
        // background tailer thread to stop) has run yet.
        let (outcome, log_capture) = match self.running.get_mut(&name) {
            Some(supervised) => {
                let log_capture = self.backend.log_capture(&supervised.handle);
                let outcome = self.backend.stop(&mut supervised.handle, window).map_err(
                    |source| match source {
                        // Fix pass (review of #80 follow-up — the CRITICAL
                        // finding): mark the handle so a RETRY `stop` (or the
                        // crash reaper's own poll) recognizes this EXACT
                        // scenario and reconciles it with a cheap,
                        // non-blocking poll instead of re-running the whole
                        // bounded SIGTERM/SIGKILL/confirm sequence (see
                        // `stop_inner`'s retry-branch docs above and
                        // `poll_once`'s docs). This `?` skips
                        // `self.running.remove` below, so the handle is
                        // RETAINED, never silently dropped — the instance
                        // stays `stopping` (the terminal transition below is
                        // never reached), an honest, non-terminal state.
                        BackendError::StopUnconfirmed { timeout_secs } => {
                            supervised.stop_unconfirmed = true;
                            EngineError::StopUnconfirmed {
                                name: name.as_str().to_string(),
                                timeout_secs,
                            }
                        }
                        other => EngineError::Backend {
                            name: name.as_str().to_string(),
                            source: other,
                        },
                    },
                )?;
                (outcome, log_capture)
            }
            None => (crate::ports::StopOutcome { forced: false }, None),
        };
        // Drop the handle (also closes the Job / releases the child on Windows) and
        // the Run's metering context — the Run ends at this terminal transition.
        self.running.remove(&name);

        // Clear the write-ahead spawn record (AD-5): a cleanly-stopped instance
        // must NOT be later adopted or reconciled-to-failed as an orphan. Cleared
        // BEFORE the terminal transition so the durable record leads the state.
        registry
            .clear_spawn_record(&name)
            .map_err(registry_to_engine)?;

        // stopping → stopped, recording whether escalation happened (AC3).
        let cause = if outcome.forced {
            TransitionCause::stop_forced(format!(
                "graceful window ({}s) elapsed; escalated to a forced kill of the process group/job",
                window.as_secs()
            ))
        } else {
            TransitionCause::StopGraceful
        };
        self.transition_with_log_capture(
            registry,
            &name,
            stopping,
            LifecycleState::Stopped,
            cause,
            log_capture,
        )?;

        registry.lookup(&name).map_err(registry_to_engine)
    }

    /// Pause a running Agent Instance with honest, per-OS semantics (story 1-5,
    /// AC1/AC2/AC3/AC5 — the "surfaced not silent" HONESTY command).
    ///
    /// Order mirrors [`Supervisor::stop`], except the middle step DISPATCHES on
    /// the effective (current-OS) pause `SupportLevel` read from the persisted
    /// snapshot (AC5), rather than always calling the backend:
    /// 1. name → [`InstanceName`]; look up the instance,
    /// 2. transition gate `next_state(state, Pause)?` — an invalid transition
    ///    (e.g. pause on `stopped`/`paused`) rejects HERE with the uniform
    ///    [`LifecycleError::InvalidTransition`] (AC4), before any side effect or
    ///    level read,
    /// 3. read the effective pause level (AC5) and dispatch:
    ///    * **Guaranteed** → `backend.pause(handle)` (real SIGSTOP suspension on
    ///      Unix), then persist `running→paused` + a plain
    ///      [`TransitionCause::Command`] (`"pause"`) — no qualifier,
    ///    * **BestEffort** → persist `running→paused` + a
    ///      [`TransitionCause::PauseBestEffort`] qualifier (the machine-readable
    ///      half of "surfaced not silent"); the process may keep running,
    ///    * **Unsupported** → FAIL FAST with
    ///      [`EngineError::CapabilityUnsupported`], NO transition, NO backend
    ///      call, NOTHING persisted (AC3).
    pub fn pause(&mut self, registry: &Registry, name: &str) -> Result<AgentInstance, EngineError> {
        self.suspend_or_resume(registry, name, LifecycleCommand::Pause, None)
    }

    /// Pause driven by a budget BREACH (story 3-2 AC6). Identical to
    /// [`Supervisor::pause`] — honoring the adapter pause Capability Declaration
    /// EXACTLY (guaranteed suspends; best-effort transitions with the honest
    /// posture; UNSUPPORTED fails fast, NO fake pause, NO silent escalation) —
    /// except the resulting `running → paused` transition carries the
    /// [`TransitionCause::BudgetExceeded`] cause instead of a plain `pause` command,
    /// so the lifecycle log itself explains WHY (the standalone breach event is the
    /// AD-14 subscription payload). Takes `&InstanceName` (the caller already has
    /// the validated name inside the ingestion path).
    fn pause_with_cause(
        &mut self,
        registry: &Registry,
        name: &InstanceName,
        cause: TransitionCause,
    ) -> Result<AgentInstance, EngineError> {
        self.suspend_or_resume(
            registry,
            name.as_str(),
            LifecycleCommand::Pause,
            Some(cause),
        )
    }

    /// Resume a paused Agent Instance (story 1-5, AC1/AC2).
    ///
    /// The symmetric counterpart of [`Supervisor::pause`]: the transition gate is
    /// `next_state(state, Resume)?` (`paused → running`; anything else rejects
    /// with the uniform invalid-transition, AC4), and the dispatch is on the same
    /// effective pause level:
    /// * **Guaranteed** → `backend.resume(handle)` (SIGCONT), then `paused→running`
    ///   + a plain `resume` command cause,
    /// * **BestEffort** → `paused→running` + a [`TransitionCause::ResumeBestEffort`]
    ///   qualifier,
    /// * **Unsupported** → fail fast (defensive; a `paused` instance implies pause
    ///   was allowed, so this is not normally reachable — see the note on the
    ///   symmetric dispatch below).
    pub fn resume(
        &mut self,
        registry: &Registry,
        name: &str,
    ) -> Result<AgentInstance, EngineError> {
        self.suspend_or_resume(registry, name, LifecycleCommand::Resume, None)
    }

    /// Shared pause/resume driver (the three-level dispatch), keyed on `command`
    /// (`Pause` or `Resume`). Kept as one method so the pause and resume paths
    /// cannot drift: the transition gate, the level read, and the three-way
    /// dispatch are identical; only the target state and the cause differ.
    ///
    /// `cause_override` (story 3-2): when `Some`, it REPLACES the default cause on
    /// the resulting transition for the GUARANTEED + BEST-EFFORT paths — a
    /// budget-driven pause records [`TransitionCause::BudgetExceeded`] instead of a
    /// plain `pause` command / a best-effort qualifier, so the lifecycle log
    /// explains WHY. `None` preserves the story-1-5 causes exactly (an operator
    /// `kt agent pause` is unchanged). The UNSUPPORTED fail-fast is identical
    /// regardless (no transition, nothing persisted — the override is moot).
    fn suspend_or_resume(
        &mut self,
        registry: &Registry,
        name: &str,
        command: LifecycleCommand,
        cause_override: Option<TransitionCause>,
    ) -> Result<AgentInstance, EngineError> {
        debug_assert!(
            matches!(command, LifecycleCommand::Pause | LifecycleCommand::Resume),
            "suspend_or_resume only handles Pause/Resume"
        );
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // (1) Transition gate (AC4): pause on stopped/paused, resume on running,
        // etc. reject HERE with the uniform InvalidTransition, before any level
        // read or side effect.
        let new_state = next_state(instance.state, command)?;

        // (2) Read the effective (current-OS) pause level from the persisted
        // snapshot (AC5). Projected at read time onto OsId::current(); NOT
        // re-derived from the manifest, NOT frozen at register time.
        let level = registry
            .effective_support(&name, Capability::Pause)
            .map_err(registry_to_engine)?;
        let os = OsId::current();

        // (3) Dispatch on the level.
        match level {
            // FAIL FAST (AC3): no transition, no backend call, nothing persisted.
            SupportLevel::Unsupported => Err(EngineError::CapabilityUnsupported {
                name: name.as_str().to_string(),
                capability: Capability::Pause.as_str().to_string(),
                os: os.as_str().to_string(),
                level: level.as_str().to_string(),
            }),
            // GUARANTEED (AC1): real suspension via the backend, then a plain
            // command-cause transition (no qualifier — it is a true suspension). A
            // story-3-2 budget pause overrides the cause with BudgetExceeded.
            SupportLevel::Guaranteed => {
                self.ensure_log_dir(registry, &name)?;
                self.signal_backend(&name, command)?;
                let cause = cause_override
                    .clone()
                    .unwrap_or_else(|| TransitionCause::command(command.as_str()));
                self.transition(registry, &name, instance.state, new_state, cause)?;
                registry.lookup(&name).map_err(registry_to_engine)
            }
            // BEST-EFFORT (AC2): transition + a VISIBLE qualifier cause, never a
            // silent success. No backend suspension is guaranteed here (on Unix a
            // best-effort declaration is unusual, but we still do NOT SIGSTOP — the
            // declared level is the contract; the qualifier is the honesty). A
            // story-3-2 budget pause overrides the cause with BudgetExceeded (the
            // best-effort posture is captured in the standalone breach event + a
            // diagnostic, so the lifecycle cause stays the honest WHY).
            SupportLevel::BestEffort => {
                self.ensure_log_dir(registry, &name)?;
                let cause = cause_override.clone().unwrap_or_else(|| {
                    let detail = format!(
                        "{} is best-effort for '{}' on {} (adapter-cooperative); the process may keep running",
                        Capability::Pause.as_str(),
                        name.as_str(),
                        os.as_str(),
                    );
                    match command {
                        LifecycleCommand::Pause => TransitionCause::pause_best_effort(detail),
                        _ => TransitionCause::resume_best_effort(detail),
                    }
                });
                self.transition(registry, &name, instance.state, new_state, cause)?;
                registry.lookup(&name).map_err(registry_to_engine)
            }
        }
    }

    /// Signal the running process for a GUARANTEED pause/resume, via the in-memory
    /// handle map (same `self.running.get_mut(&name)` pattern as `stop`).
    ///
    /// Cross-lifetime honesty (AD-5, story 1-6: adoption re-holds handles): with
    /// orphan adoption, a still-live process started by a PRIOR engine is
    /// re-acquired at [`Engine::open`] (via [`Supervisor::adopt_orphans`]), so its
    /// handle IS in the map and this path really signals it. The no-handle branch
    /// now only occurs when the row says `running`/`paused` but adoption found NO
    /// live process — a state adoption would already have reconciled to `failed`;
    /// so a lingering no-handle case is a best-effort no-op (nothing to signal),
    /// which still lets the transition proceed. A real held (spawned or adopted)
    /// process IS signalled.
    fn signal_backend(
        &mut self,
        name: &InstanceName,
        command: LifecycleCommand,
    ) -> Result<(), EngineError> {
        let Some(supervised) = self.running.get_mut(name) else {
            return Ok(());
        };
        let result = match command {
            LifecycleCommand::Pause => self.backend.pause(&mut supervised.handle),
            _ => self.backend.resume(&mut supervised.handle),
        };
        result.map_err(|source| EngineError::Backend {
            name: name.as_str().to_string(),
            source,
        })
    }

    /// Send text input to a running Agent Instance's native input channel
    /// (story 4.1, FR-24, spine AD-12) — the v1 interaction surface. For
    /// every adapter that can actually run today (native mock or manifest),
    /// "the native input channel" is the spawned child's OS stdin pipe (both
    /// backends pipe it unconditionally at spawn, Task 1); this needs ZERO
    /// per-kind branching, so one method serves both (AC-A).
    ///
    /// Unlike [`Supervisor::suspend_or_resume`], `send` is NOT itself a state
    /// transition (AD-15's transition table has no `send` entry): no
    /// `next_state` call, no [`TransitionEvent`]. The dispatch order:
    ///
    /// 1. name-resolve (`NotFound` unchanged),
    /// 2. **AC-C**: the instance MUST be [`LifecycleState::Running`] —
    ///    anything else fails with [`EngineError::NotRunning`], checked
    ///    BEFORE the capability read (mirrors "transition gate before any
    ///    side effect"),
    /// 3. **AC-B**: read the effective (current-OS) `Capability::Interaction`
    ///    level — `Unsupported` FAILS FAST with
    ///    [`EngineError::CapabilityUnsupported`] (the already-generic
    ///    machinery, reused verbatim — same shape pause already produces),
    ///    no I/O attempted,
    /// 4. **AC-D**: `Guaranteed` and `BestEffort` take the IDENTICAL action —
    ///    unlike pause/resume there is no OS-conditional difference in
    ///    writing bytes to a pipe, so a declared `best-effort` is purely an
    ///    adapter-author honesty signal, not a different code path. A
    ///    missing handle, or one with no live stdin pipe (an ADOPTED
    ///    instance has no recoverable pipe — see
    ///    [`crate::ports::ProcessBackend::has_stdin`]'s docs), is a HARD
    ///    ERROR ([`EngineError::InteractionUnavailable`]): unlike
    ///    [`Supervisor::signal_backend`]'s "no handle = harmless no-op" (a
    ///    suspend/resume of an already-gone process trivially satisfies its
    ///    own desired end state), there is no equivalent "desired end state"
    ///    for text that was never delivered — a silent success would violate
    ///    FR-24's "honest failure" framing, and this must NEVER be
    ///    misattributed to `CapabilityUnsupported` (the declaration is
    ///    truthful; it is this engine session's reach that is limited).
    ///    **Fix pass addition (review of #79):** a handle whose PRIOR write
    ///    already timed out ([`crate::ports::ProcessBackend::stdin_timed_out`])
    ///    fails fast with [`EngineError::InteractionTimedOut`] here too — a
    ///    cheap, no-I/O check, never a repeat doomed write.
    /// 5. **AC-F**: append exactly one trailing `\n` if `text` doesn't
    ///    already end with one, then write + flush via
    ///    [`crate::ports::ProcessBackend::write_stdin`] — BOUNDED to
    ///    [`crate::ports::STDIN_WRITE_TIMEOUT`] (fix pass, the CRITICAL
    ///    finding: the original unbounded write could freeze the ENTIRE
    ///    engine, since this call runs while the caller already holds the
    ///    single, engine-wide supervisor lock — see `write_stdin`'s docs). A
    ///    timeout maps to [`EngineError::InteractionTimedOut`]; any OTHER
    ///    [`BackendError`] maps to [`EngineError::Backend`] — the SAME
    ///    generic mapping `signal_backend` already uses for pause/resume.
    pub fn send_input(
        &mut self,
        registry: &Registry,
        name: &str,
        text: &str,
    ) -> Result<(), EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // (1) AC-C: send is not a transition, so this is a dedicated
        // pre-flight state check — before any capability read or I/O.
        if instance.state != LifecycleState::Running {
            return Err(EngineError::NotRunning {
                name: name.as_str().to_string(),
                state: instance.state.as_str().to_string(),
            });
        }

        // (2) AC-B: reuse the already-generic capability-unsupported
        // fail-fast machinery verbatim.
        let level = registry
            .effective_support(&name, Capability::Interaction)
            .map_err(registry_to_engine)?;
        let os = OsId::current();
        if level == SupportLevel::Unsupported {
            return Err(EngineError::CapabilityUnsupported {
                name: name.as_str().to_string(),
                capability: Capability::Interaction.as_str().to_string(),
                os: os.as_str().to_string(),
                level: level.as_str().to_string(),
            });
        }

        // (3) AC-D: Guaranteed and BestEffort collapse to the SAME action
        // below (no OS-conditional difference in delivering bytes to a
        // pipe). A missing handle, or one with no live stdin pipe (an
        // adopted instance), is a HARD error — never a silent success.
        let Some(supervised) = self.running.get_mut(&name) else {
            return Err(EngineError::InteractionUnavailable {
                name: name.as_str().to_string(),
                detail: "no live process handle is held in this engine session".to_string(),
            });
        };
        // Fix pass (CRITICAL finding, review of #79): a cheap, no-I/O check
        // FIRST — a handle whose prior write already exceeded the bounded
        // timeout is PERMANENTLY broken for the rest of this engine session
        // (see `write_stdin`'s docs). Checked before `has_stdin` (which would
        // also read `false` here) so the more precise, honest diagnostic
        // wins: "we had a pipe and it stopped draining" is a materially
        // different fact from "no pipe was ever recoverable", and the CLI's
        // remediation differs (restart to get a fresh channel either way, but
        // the cause is not the same).
        if self.backend.stdin_timed_out(&supervised.handle) {
            return Err(EngineError::InteractionTimedOut {
                name: name.as_str().to_string(),
                timeout_secs: crate::ports::STDIN_WRITE_TIMEOUT.as_secs(),
            });
        }
        if !self.backend.has_stdin(&supervised.handle) {
            return Err(EngineError::InteractionUnavailable {
                name: name.as_str().to_string(),
                detail: "no live stdin pipe is held for this instance in this engine session \
                         (an adopted instance has no recoverable pipe; durable cross-invocation \
                         interaction needs a persistent engine session, planned for Epic 7/v1.x)"
                    .to_string(),
            });
        }

        // (4) AC-F: append exactly one trailing newline if absent, so a
        // line-oriented agent (`BufRead::read_line`) receives a complete
        // line.
        let mut bytes = text.as_bytes().to_vec();
        if !text.ends_with('\n') {
            bytes.push(b'\n');
        }
        // Fix pass (CRITICAL finding): this write is now BOUNDED to
        // `STDIN_WRITE_TIMEOUT` (`write_stdin`'s new contract) rather than
        // the story's original unbounded `write_all` — still runs while
        // `self` (the supervisor) is held under the caller's lock, exactly
        // like `stop`'s existing bounded graceful-window wait; a deliberate,
        // ACCEPTED, BOUNDED tradeoff, not the unbounded-freeze problem this
        // fix closes.
        match self.backend.write_stdin(&mut supervised.handle, &bytes) {
            Ok(()) => Ok(()),
            Err(BackendError::StdinTimedOut { timeout_secs }) => {
                Err(EngineError::InteractionTimedOut {
                    name: name.as_str().to_string(),
                    timeout_secs,
                })
            }
            Err(source) => Err(EngineError::Backend {
                name: name.as_str().to_string(),
                source,
            }),
        }
    }

    /// The current [`RunId`] for a supervised instance (story 3-1), or `None` if
    /// this engine holds no live handle for it (never started this lifetime, or
    /// already stopped/crashed). The Fleet read uses it to scope the current-Run
    /// token totals; a `None` simply means "no active Run" (current-run totals are
    /// zero). Held in memory alongside the process handle for this engine lifetime.
    pub fn current_run_id(&self, name: &InstanceName) -> Option<RunId> {
        self.running.get(name).map(|s| s.run_id.clone())
    }

    /// Read the recorded [`TransitionEvent`]s for an instance from its log
    /// (observation helper for tests / embedders; the AD-14 seed, NOT the 7-2
    /// bus). Returns an empty vec if the log does not exist yet.
    pub fn read_events(
        registry: &Registry,
        name: &str,
    ) -> Result<Vec<TransitionEvent>, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let path = registry.instance_log_path(&name);
        read_events_from(&path).map_err(|detail| EngineError::Log {
            name: name.as_str().to_string(),
            path: path.to_string_lossy().into_owned(),
            detail,
        })
    }

    /// One-shot full read of every currently-retained ATTRIBUTED output line
    /// for an instance (story 4-2, AC-A/AC-G) — reads the rotated generations
    /// OLDEST-to-newest (`.2`, `.1`, current — skipping any that do not exist
    /// yet), concatenates, and parses each JSON-Lines [`LogLine`] record in
    /// ON-DISK APPEND ORDER (the sole ordering authority; NEVER re-sorted by
    /// `at` — AC-G, since `now_rfc3339`'s whole-second resolution makes
    /// same-second lines common). This reads the NEW, SEPARATE
    /// `logs/output.log[.N]` file (CRITICAL SCOPING #3) — never `agent.log`,
    /// which stays byte-identical and untouched for Epic 3's
    /// `drain_usage_for`.
    ///
    /// DELIBERATE IMPROVEMENT over the `read_events`/`read_breach_events`
    /// precedent above (which never check the registry for the instance's
    /// existence at all — harmless there, since neither is exposed via any
    /// `kt` command): `read_agent_log` is the FIRST CLI-facing consumer of
    /// this shape (`kt agent logs`, Task 6), where silently showing "no
    /// output" for a mistyped name would be genuinely confusing UX
    /// (indistinguishable from "the agent just hasn't said anything yet").
    /// So this DOES check the registry first: a truly UNREGISTERED name
    /// fails [`EngineError::NotFound`] (matching every other CLI-facing
    /// command — `show`/`send`/`pause` all do this); a REGISTERED-but-never-
    /// started instance still falls through to an honest empty vec (mirrors
    /// `read_events_from`'s "missing file → empty" precedent).
    ///
    /// Fix pass (M1, review of #80): ALSO returns the byte-cursor position
    /// (into the CURRENT generation, matching
    /// [`Supervisor::read_agent_log_since`]'s cursor shape exactly) this
    /// read reached — computed from the SAME bytes this call parsed, never a
    /// second, separately-timed read. `kt agent logs --follow` (the sole
    /// production caller) primes its poll loop's cursor from this value
    /// directly, instead of a SEPARATE `read_agent_log_since(name, 0)` call
    /// whose returned lines it used to discard — that discarding call read
    /// up to a slightly LATER point in time than this one-shot dump, so
    /// anything emitted in the gap between the two reads was silently lost
    /// before `--follow` ever started polling. Returning the cursor here
    /// closes that gap: there is only ever ONE read establishing both the
    /// dump and the resume point.
    pub fn read_agent_log(
        registry: &Registry,
        name: &str,
    ) -> Result<(Vec<LogLine>, u64), EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        registry.lookup(&name).map_err(registry_to_engine)?;

        let mut lines = Vec::new();
        // Oldest generation first (LOG_ROTATE_GENERATIONS - 1 down to 1),
        // then the current generation last — append order overall.
        for generation in (1..LOG_ROTATE_GENERATIONS).rev() {
            let path = registry.attributed_output_log_generation_path(&name, generation);
            read_log_lines_from(&path, &mut lines).map_err(|detail| EngineError::Log {
                name: name.as_str().to_string(),
                path: path.to_string_lossy().into_owned(),
                detail,
            })?;
        }
        let current = registry.attributed_output_log_path(&name);
        // Read the CURRENT generation's raw text ONCE so its exact byte
        // length (the cursor) and its parsed lines come from the identical
        // bytes — never a second, later, potentially-inconsistent read.
        let current_text = match std::fs::read_to_string(&current) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(EngineError::Log {
                    name: name.as_str().to_string(),
                    path: current.to_string_lossy().into_owned(),
                    detail: e.to_string(),
                })
            }
        };
        let cursor = current_text.len() as u64;
        parse_log_lines(&current_text, &mut lines).map_err(|detail| EngineError::Log {
            name: name.as_str().to_string(),
            path: current.to_string_lossy().into_owned(),
            detail,
        })?;
        Ok((lines, cursor))
    }

    /// A CURSOR-based follow read for `kt agent logs --follow`'s poll loop
    /// (story 4-2, AC-B/AC-C/AC-H, AD-13). `cursor` is a byte offset into the
    /// CURRENT generation ONLY (mirrors `agent_log_len`/`plan_drain`'s
    /// existing cursor shape) — distinct from `read_agent_log`'s
    /// concatenated multi-generation view, so a caller must not mix cursors
    /// from the two methods. Returns `(new_lines, next_cursor)` — plain
    /// request/response (AD-13), never a `Stream`-typed API (see the story's
    /// Dev Notes on why: this keeps the existing async/blocking pairing with
    /// zero new API shape).
    ///
    /// On a detected SHRINK (the current generation's length is now LESS
    /// than `cursor` — a rotation happened since the last poll), the cursor
    /// snaps to the new length and this returns `(vec![], new_len)` — the
    /// CALLER detects the signal itself by comparing the returned cursor to
    /// the one it just passed in (`next_cursor < cursor`) and prints one
    /// honest notice (Task 6); `read_agent_log` WITHOUT `--follow` always
    /// re-reads everything currently retained, so this never loses data
    /// permanently — only a possible (rare) live-tail gap at the rotation
    /// boundary.
    pub fn read_agent_log_since(
        registry: &Registry,
        name: &str,
        cursor: u64,
    ) -> Result<(Vec<LogLine>, u64), EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        registry.lookup(&name).map_err(registry_to_engine)?;

        let path = registry.attributed_output_log_path(&name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                return Err(EngineError::Log {
                    name: name.as_str().to_string(),
                    path: path.to_string_lossy().into_owned(),
                    detail: e.to_string(),
                })
            }
        };
        match plan_follow(&bytes, cursor) {
            FollowPlan::Shrunk { new_cursor } => Ok((Vec::new(), new_cursor)),
            FollowPlan::Consume { range, new_cursor } => {
                let mut lines = Vec::new();
                if !range.is_empty() {
                    parse_log_lines(&String::from_utf8_lossy(&bytes[range]), &mut lines).map_err(
                        |detail| EngineError::Log {
                            name: name.as_str().to_string(),
                            path: path.to_string_lossy().into_owned(),
                            detail,
                        },
                    )?;
                }
                Ok((lines, new_cursor))
            }
        }
    }

    /// The crash-detection reaper pass (story 1-6, AC-A / AC3 / AC5).
    ///
    /// Polls every held handle via the EXISTING `backend.poll` and reacts to an
    /// unrequested exit: for each instance the store still shows `running` or
    /// `paused` (a `stopping` in flight means an operator stop is under way — NOT
    /// a crash, so it is skipped), applies the EVENT-driven `running → failed`
    /// edge with a [`TransitionCause::Crashed`] (AC5), removes the handle, and
    /// consults the per-instance [`RestartPolicy`] (AD-15):
    /// * [`RestartPolicy::Never`] — leave `failed`; record the crash cause; NO
    ///   restart plan.
    /// * [`RestartPolicy::OnFailure`] — increment the consecutive restart count;
    ///   if it hit the crash-loop threshold ([`is_crash_loop`]) leave `failed`
    ///   with the crash-loop reason and NO plan; otherwise persist the new count
    ///   and return a [`RestartPlan`] with the backoff delay for that attempt.
    ///
    /// Returns the [`RestartPlan`]s the engine cadence should time. SYNC +
    /// cfg-free (the engine calls it via `spawn_blocking` on an interval); it
    /// performs NO sleeping itself. Idempotent per exit: once an instance is
    /// moved to `failed` and its handle removed, a later pass will not see it in
    /// `self.running` again.
    pub fn poll_once(&mut self, registry: &Registry) -> Vec<RestartPlan> {
        // First, INGEST self-reported usage from every running instance's captured
        // output (story 3-1): the reaper is the natural cadence for draining the
        // agent-output log into the Usage Ledger while an instance is `running`.
        // Best-effort per instance; a drain hiccup never blocks crash detection.
        self.drain_usage_all(registry);
        // Then INGEST engine-observed usage (story 3-4): drain each observed
        // instance's listener queue (the counts the loopback proxy parsed out of the
        // agent's model traffic) into the SAME `ingest_usage` choke point, minting
        // the per-Run `sequence`. This reaper cadence (~250ms) lands observed usage
        // well within the AD-7/FR-19 flush bound (≤5s) of call completion. Best-
        // effort per instance, exactly like the self-reported drain.
        self.drain_observed_all(registry);

        // Snapshot the currently-held names (we mutate self.running as we react).
        let names: Vec<InstanceName> = self.running.keys().cloned().collect();
        let mut plans = Vec::new();

        for name in names {
            // Poll liveness. A poll error is treated as still-alive (transient);
            // the next pass re-checks. Reap on exit is done inside `poll`.
            let exited = match self.running.get_mut(&name) {
                Some(supervised) => match self.backend.poll(&mut supervised.handle) {
                    Ok(ProcessStatus::Exited { code }) => Some(code),
                    Ok(ProcessStatus::Alive) => None,
                    Err(_) => None,
                },
                None => continue,
            };
            let Some(code) = exited else { continue };
            // The process exited: drain any usage it emitted right before dying, so a
            // final batch is not lost between "agent printed it" and this reap.
            // TERMINAL drain — the process is dead, so consume a final newline-less
            // usage line to end-of-log instead of stranding it (H1).
            self.drain_usage_for(registry, &name, DrainMode::Terminal);
            // Drain any final ENGINE-OBSERVED usage still queued before the crashed
            // instance's listener is torn down (story 3-4): a completion parsed just
            // before the crash must land, not be lost when the `Supervised` is
            // removed below. Best-effort, mirroring the self-reported terminal drain.
            self.drain_observed_for(registry, &name);

            // Read the store state: only an instance the store still shows
            // running/paused is an UNREQUESTED crash. A `stopping` (operator
            // stop) or any other state is not a crash — drop the (now-dead)
            // handle without a `failed` transition.
            let state = match registry.lookup(&name) {
                Ok(inst) => inst.state,
                // The row is gone (removed concurrently) — just drop the handle.
                Err(_) => {
                    self.running.remove(&name);
                    continue;
                }
            };
            if !matches!(state, LifecycleState::Running | LifecycleState::Paused) {
                // Requested stop (or already-terminal) — not a crash.
                //
                // Fix pass (review of #80 follow-up — the CRITICAL finding,
                // self-healing requirement): if this handle's PRIOR stop
                // attempt sent SIGKILL but could not confirm death within
                // KILL_CONFIRM_TIMEOUT (`stop_unconfirmed` — set ONLY by
                // that specific path, see `stop_inner`'s docs) and the store
                // still shows `stopping`, THIS poll's own observed `Exited`
                // is the reconciliation event the stuck stop() call itself
                // could not wait for: finalize `stopping -> stopped` here
                // rather than silently dropping the handle, so the row does
                // not stay permanently stuck even if no operator ever
                // retries `stop` manually. This is DELIBERATELY narrower
                // than "any exit while stopping" — an ordinary in-flight
                // (non-stuck) stop() call ALWAYS finalizes this transition
                // itself upon its own return, so only the
                // stuck-then-abandoned case needs the reaper's help; every
                // OTHER "not a crash" exit (mirrored by
                // `poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash`,
                // which never sets `stop_unconfirmed`) keeps its EXISTING,
                // unchanged silent-drop behavior.
                let stuck_stopping = state == LifecycleState::Stopping
                    && self.running.get(&name).is_some_and(|s| s.stop_unconfirmed);
                if stuck_stopping {
                    let log_capture = self
                        .running
                        .get(&name)
                        .and_then(|s| self.backend.log_capture(&s.handle));
                    self.running.remove(&name);
                    if registry.clear_spawn_record(&name).is_ok() {
                        let _ = self.transition_with_log_capture(
                            registry,
                            &name,
                            LifecycleState::Stopping,
                            LifecycleState::Stopped,
                            TransitionCause::stop_forced(
                                "SIGKILL was sent by an earlier stop attempt; the \
                                 crash-detection reaper confirmed the process's death on a \
                                 later poll (it may have been stuck in an OS-level I/O wait \
                                 that has since cleared)",
                            ),
                            log_capture,
                        );
                    }
                    continue;
                }
                self.running.remove(&name);
                continue;
            }

            // A crash. Consult the Restart Policy FIRST (so a terminal outcome —
            // `never` or crash-loop — can enrich the recorded crash cause), then
            // apply running/paused → failed with that detail (AC5).
            //
            // Story 4-2, Task 4: capture the log_capture BEFORE removing the
            // entry below — same reasoning as `stop_inner`'s terminal
            // transition (the default `self.transition(...)` lookup would
            // otherwise miss it).
            let crash_log_capture = self
                .running
                .get(&name)
                .and_then(|s| self.backend.log_capture(&s.handle));
            self.running.remove(&name);
            let base_detail = match code {
                Some(c) => format!("process exited unexpectedly with code {c}"),
                None => "process exited unexpectedly (terminated by signal)".to_string(),
            };
            let decision = self.plan_restart(registry, &name, &base_detail);
            if self.ensure_log_dir(registry, &name).is_err() {
                // If we cannot even prepare the log dir, still persist the state
                // so the durable state leads; skip the event append best-effort.
            }
            // The recorded crash cause carries the full story: the exit detail,
            // plus (on a terminal outcome) the policy conclusion (crash-loop, or
            // "policy is never — not restarting"). This is what `instance_status`
            // falls back to for the failed cause once the terminal record is
            // cleared (AC9).
            if self
                .transition_with_log_capture(
                    registry,
                    &name,
                    state,
                    LifecycleState::Failed,
                    TransitionCause::crashed(decision.crash_cause.clone()),
                    crash_log_capture,
                )
                .is_err()
            {
                // Persisting the crash transition failed; leave the record for a
                // later reconcile and move on (do not panic the reaper).
                continue;
            }

            if let Some(plan) = decision.plan {
                plans.push(plan);
            }
        }
        plans
    }

    /// Decide the Restart Policy action for a just-crashed instance (AC4).
    ///
    /// Reads the per-instance record (policy + current consecutive count) and
    /// returns a [`RestartDecision`]: the crash cause to record in the event log
    /// (enriched with the policy conclusion on a terminal outcome) and, when a
    /// restart is scheduled, the [`RestartPlan`]. Side effects (all best-effort —
    /// a store hiccup is never a panic):
    /// * `on-failure`, below the crash-loop threshold → increment the persisted
    ///   restart count; the plan carries the backoff delay for that attempt.
    /// * `on-failure`, at the crash-loop threshold ([`is_crash_loop`]) → TERMINAL:
    ///   CLEAR the write-ahead record (F-Low-2: no needless adopt-attempt against
    ///   a dead/reused PID on a later open) and enrich the crash cause with the
    ///   crash-loop reason; no plan.
    /// * `never` → TERMINAL: clear the write-ahead record and note the policy in
    ///   the crash cause; no plan.
    fn plan_restart(
        &self,
        registry: &Registry,
        name: &InstanceName,
        crash_detail: &str,
    ) -> RestartDecision {
        let record = registry.spawn_record(name).ok().flatten();
        let policy = record
            .as_ref()
            .map(|r| r.restart_policy)
            .unwrap_or_default();
        let current = record.as_ref().map(|r| r.restart_count).unwrap_or(0);

        if !policy.restarts_on_crash() {
            // `never`: TERMINAL. Settle the record so a later open does not
            // adopt-attempt a dead PID; the crash cause names the policy.
            self.settle_terminal_record(registry, name, policy);
            return RestartDecision {
                crash_cause: format!("{crash_detail}; restart policy is 'never' — not restarting"),
                plan: None,
            };
        }

        let next = current.saturating_add(1);
        if is_crash_loop(next) {
            // Crash loop: TERMINAL. Settle the record (F-Low-2), leave `failed`
            // with the reason STATED in the crash cause.
            self.settle_terminal_record(registry, name, policy);
            return RestartDecision {
                crash_cause: format!(
                    "{crash_detail}; crash-loop: {} consecutive failures reached — \
                     not restarting, inspect the agent and start it manually",
                    MAX_CONSECUTIVE_FAILURES,
                ),
                plan: None,
            };
        }

        // Schedule a restart: persist the incremented count + the crash cause,
        // and return the plan with the backoff delay for this attempt.
        let _ = registry.set_restart_count(name, next, Some(crash_detail));
        let delay = self.backoff.delay_for(next);
        RestartDecision {
            crash_cause: crash_detail.to_string(),
            plan: Some(RestartPlan {
                name: name.clone(),
                attempt: next,
                delay,
            }),
        }
    }

    /// Settle the write-ahead record on a TERMINAL `failed` outcome (F-Low-2).
    ///
    /// Drops the record's LIVE fingerprint (so a later [`Supervisor::adopt_orphans`]
    /// does NOT adopt-attempt the dead/reused PID — the reconcile skips a pid-0
    /// record, exactly like a policy-only config seed), while RE-SEEDING the
    /// per-instance policy so `kt agent show` still reports the active Restart
    /// Policy for the failed instance (AC9). Concretely: clear the record, then
    /// re-persist the policy as a pid-0 seed. The failed CAUSE is not kept in the
    /// record — it rides in the event log, which `instance_status` falls back to.
    /// Best-effort (a store hiccup here is never a panic).
    fn settle_terminal_record(
        &self,
        registry: &Registry,
        name: &InstanceName,
        policy: RestartPolicy,
    ) {
        let _ = registry.clear_spawn_record(name);
        let _ = registry.set_restart_policy(name, policy);
    }

    /// Adopt orphaned processes on engine start (story 1-6, AC-B / AC7 / AI-7 /
    /// AI-8) — the HONEST cross-lifetime reconcile.
    ///
    /// Reads EVERY write-ahead [`SpawnRecord`] (AD-5) and, for each, asks the
    /// backend to re-acquire a live process matching the fingerprint
    /// (`backend.adopt`):
    /// * `Some(handle)` — a live process whose start-time matches: ADOPT it
    ///   (re-hold the handle so `stop`/`pause`/`poll` work again); the persisted
    ///   state stays as-is (`running`/`paused` — AI-7: a live paused process is
    ///   re-held so a later `resume` works).
    /// * `None` — no live match (PID gone, or reused by a different process):
    ///   reconcile HONESTLY to `failed` with an "orphan not found" cause + the
    ///   last-known cause (AI-8: never leave a phantom `running`/`paused` row),
    ///   and clear the record.
    ///
    /// Called from [`Engine::open`]. Best-effort per record: a single
    /// adopt/persist failure does not abort the whole reconcile; it leaves that
    /// record for the next open. Returns the number of processes adopted (for
    /// diagnostics/tests).
    pub fn adopt_orphans(&mut self, registry: &Registry) -> usize {
        let records = match registry.list_spawn_records() {
            Ok(records) => records,
            Err(_) => return 0,
        };
        let mut adopted = 0;
        for record in records {
            let name = record.name.clone();
            // A pid-0 record is a policy-only config SEED (set via
            // `set_restart_policy` before the instance was ever started), NOT a
            // supervised process — skip it (it names no real process to adopt or
            // fail, and clearing it would wipe the persisted policy).
            if record.fingerprint.pid == 0 {
                continue;
            }
            match self.backend.adopt(&record.fingerprint) {
                Ok(Some(handle)) => {
                    // Live match: re-hold the handle. State stays as persisted
                    // (running/paused). AI-7: a paused process is now resumable.
                    //
                    // Metering across a crash/adoption (story 3-1, documented
                    // assumption): the pre-crash Run id lived only in the crashed
                    // engine's memory, so the adopted instance opens a NEW Run and
                    // begins ingestion at the CURRENT end of its agent-output log
                    // (skipping pre-crash lines). This keeps per-run totals honest for
                    // the post-adoption span without re-attributing (or double-
                    // counting) the old Run's already-captured usage; the DB dedup key
                    // includes the run id, so even an overlapping sequence is safe.
                    let run_id = RunId::mint();
                    let usage_cursor = self.agent_log_len(registry, &name);
                    let metering_source = registry
                        .metering_source(&name)
                        .unwrap_or_else(|_| "self-reported".to_string());
                    self.running.insert(
                        name,
                        Supervised {
                            handle,
                            run_id,
                            metering_source,
                            usage_cursor,
                            // The adopted instance opens a NEW Run (the pre-crash
                            // run_id died with the crashed engine), so its breach latch
                            // starts empty too (story 3-2).
                            breached_scopes: std::collections::HashSet::new(),
                            // ENGINE-OBSERVED across a crash/adoption (story 3-4,
                            // tracked follow-up — NOT just a metering gap): the pre-crash
                            // listener died with the crashed engine, but the already-
                            // running agent's `base_url` STILL points at that now-DEAD
                            // loopback port. So the adopted agent's MODEL TRAFFIC ITSELF
                            // breaks — its completion calls hit the dead port and fail
                            // with a connection-refused error (not merely un-metered).
                            // This fails LOUD (a transport error the agent surfaces),
                            // never a corrupt/silent-wrong output. We cannot rebind the
                            // old port to a fresh listener here (the agent chose no port;
                            // the OS did), so we leave it un-observed with no listener;
                            // RECOVERY is an operator stop→start, which relaunches the
                            // agent pointed at a fresh listener. The full fix (re-launch
                            // an adopted observed instance / a stable per-instance listener
                            // port / the Epic-7 daemon owning the listener) is a tracked
                            // follow-up, not done here. A self-reported instance's
                            // log-tail drain is unaffected (it needs no listener).
                            observed_listener: None,
                            observed_source: None,
                            // An adopted instance's stop attempt has not
                            // happened yet in THIS engine session.
                            stop_unconfirmed: false,
                        },
                    );
                    adopted += 1;
                }
                Ok(None) => {
                    // No live match — reconcile to `failed` HONESTLY (AI-8).
                    self.reconcile_orphan_failed(registry, &record);
                }
                Err(_) => {
                    // A backend adopt error is treated as "cannot confirm live" —
                    // reconcile to failed rather than leave a phantom row (AI-8).
                    self.reconcile_orphan_failed(registry, &record);
                }
            }
        }
        adopted
    }

    /// Reconcile a non-adopted orphan record to `failed` (AI-8): the process is
    /// gone (or unconfirmable), so a persisted `running`/`paused` row must NOT be
    /// left implying supervision that does not exist. Records a `Crashed` cause
    /// naming the orphan + the last-known cause, then clears the record. If the
    /// current state is already terminal (`failed`/`stopped`) we only clear the
    /// stale record. Best-effort — a persist failure leaves the record for the
    /// next open.
    fn reconcile_orphan_failed(&self, registry: &Registry, record: &SpawnRecord) {
        let name = &record.name;
        let state = match registry.lookup(name) {
            Ok(inst) => inst.state,
            Err(_) => {
                // Row gone — just drop the stale record.
                let _ = registry.clear_spawn_record(name);
                return;
            }
        };
        if matches!(state, LifecycleState::Running | LifecycleState::Paused) {
            let last = record
                .last_known_cause
                .as_deref()
                .unwrap_or("no prior cause recorded");
            let detail = format!(
                "orphan not found on engine restart (process pid {} is gone or was reused); \
                 last known: {last}",
                record.fingerprint.pid,
            );
            let _ = self.ensure_log_dir(registry, name);
            let _ = self.transition(
                registry,
                name,
                state,
                LifecycleState::Failed,
                TransitionCause::crashed(detail),
            );
        }
        // Clear the stale record either way (its process is gone).
        let _ = registry.clear_spawn_record(name);
    }

    // ---- internals ----

    /// Apply one transition: persist the new state, then append the event to the
    /// per-instance log. Persist-before-log so the durable state leads; a log
    /// append failure surfaces (the escalation record is load-bearing for AC3).
    ///
    /// Story 4-2, Task 4: ALSO best-effort-projects the transition into the
    /// unified attributed-output stream as an `engine`-attributed [`LogLine`]
    /// — a human-readable mirror of the SAME fact `instance.log` (above,
    /// unchanged, machine-authoritative) just recorded. The output-capture
    /// handle is looked up via [`Supervisor::log_capture_for`] (the
    /// instance's CURRENT `self.running` entry, when one exists); fix pass
    /// (review of #80): [`LogCapture::send_engine_line`] catches up any
    /// pending agent-out/agent-err content FIRST, so the engine line lands
    /// after whatever agent output already existed at this moment rather
    /// than racing the background tailer thread's own poll schedule.
    fn transition(
        &self,
        registry: &Registry,
        name: &InstanceName,
        prior: LifecycleState,
        new: LifecycleState,
        cause: TransitionCause,
    ) -> Result<TransitionEvent, EngineError> {
        let log_capture = self.log_capture_for(name);
        self.transition_with_log_capture(registry, name, prior, new, cause, log_capture)
    }

    /// Like [`Supervisor::transition`], but the `engine`-attributed
    /// [`LogCapture`] is supplied EXPLICITLY rather than looked up via
    /// `self.running` — needed at the three call sites (`start_inner`'s
    /// `starting → running`, `stop_inner`'s `stopping → stopped`, and
    /// `poll_once`'s crash `→ failed`) where the just-spawned/about-to-be-
    /// torn-down handle is not (or no longer) present in `self.running` at
    /// the exact moment of the call, even though a live capture pipeline
    /// still exists (captured by the caller a few lines earlier, before the
    /// map mutation that would otherwise hide it).
    fn transition_with_log_capture(
        &self,
        registry: &Registry,
        name: &InstanceName,
        prior: LifecycleState,
        new: LifecycleState,
        cause: TransitionCause,
        log_capture: Option<LogCapture>,
    ) -> Result<TransitionEvent, EngineError> {
        registry.set_state(name, new).map_err(registry_to_engine)?;
        let event = TransitionEvent::new(name.as_str(), prior, new, cause, now_rfc3339());
        append_event(&registry.instance_log_path(name), &event).map_err(|detail| {
            EngineError::Log {
                name: name.as_str().to_string(),
                path: registry
                    .instance_log_path(name)
                    .to_string_lossy()
                    .into_owned(),
                detail,
            }
        })?;
        if let Some(capture) = log_capture {
            let text = engine_transition_line_text(&event);
            capture.send_engine_line(LogLine::new(
                name.as_str(),
                LogStream::Engine,
                text,
                event.at.clone(),
            ));
        }
        Ok(event)
    }

    /// The output-capture handle this engine session currently holds for
    /// `name`, if any (story 4-2, Task 4) — `None` when the instance has no
    /// `self.running` entry (not started this session, already torn down, or
    /// adopted with no recoverable capture pipeline).
    fn log_capture_for(&self, name: &InstanceName) -> Option<LogCapture> {
        self.running
            .get(name)
            .and_then(|s| self.backend.log_capture(&s.handle))
    }

    /// Land a spawn failure in `failed` with the backend diagnostic preserved
    /// (AC2), returning the [`EngineError::LaunchFailed`] to surface.
    fn fail_launch(
        &self,
        registry: &Registry,
        name: &InstanceName,
        err: &BackendError,
    ) -> EngineError {
        self.fail_launch_detail(registry, name, err.to_string())
    }

    /// Land a launch failure in `failed` with `detail` preserved (AC2).
    ///
    /// Records the `starting → failed` transition (cause = launch-error, detail
    /// verbatim) and returns [`EngineError::LaunchFailed`]. If persisting the
    /// failed state itself errors, that store error is surfaced instead (it is
    /// the more fundamental problem).
    fn fail_launch_detail(
        &self,
        registry: &Registry,
        name: &InstanceName,
        detail: String,
    ) -> EngineError {
        if let Err(e) = self.transition(
            registry,
            name,
            LifecycleState::Starting,
            LifecycleState::Failed,
            TransitionCause::launch_error(detail.clone()),
        ) {
            return e;
        }
        EngineError::LaunchFailed {
            name: name.as_str().to_string(),
            detail,
        }
    }

    /// Start the loopback forward listener for an `engine-observed` instance
    /// (story 3-4, AC-A/AC-B/AC6), or return `Ok(None)` for a `self-reported`
    /// instance (whose start path is UNCHANGED). Runs at `starting`, BEFORE any
    /// persisted state change, so a failure rejects the start cleanly.
    ///
    /// For an `engine-observed` instance it: (1) resolves the operator-configured
    /// real upstream provider URL (`metering.upstream_base_url`) from `effective`;
    /// (2) requires the engine runtime handle (the listener's accept loop runs on
    /// it) — absent → a clear error (only the handle-less unit-test supervisor lacks
    /// it, and it never starts an observed instance); (3) binds `127.0.0.1:0`
    /// (loopback ONLY — AC-B) and spawns the accept loop. Every failure maps to a
    /// TRAFFIC-FREE [`EngineError::ObservedMetering`] (no body/header/key — 2-4
    /// no-leak). The returned [`ObservedListener`] is moved into `Supervised`; its
    /// `base_url` is what the caller injects via the config-mapping (AC6).
    fn start_observed_listener(
        &self,
        name: &InstanceName,
        metering_source: &str,
        effective: &crate::domain::EffectiveConfig,
    ) -> Result<Option<ObservedListener>, EngineError> {
        // Only an `engine-observed` instance runs a listener. `self-reported`
        // (and any other) leaves it None — its start path is byte-unchanged.
        if metering_source != "engine-observed" {
            return Ok(None);
        }
        // The operator MUST configure the real upstream provider URL (there is
        // nowhere to forward otherwise). Absent → a clear start error naming the key.
        let upstream = config::resolve_upstream_base_url(effective).ok_or_else(|| {
            EngineError::ObservedMetering {
                name: name.as_str().to_string(),
                detail: format!(
                    "no upstream provider URL configured; set `{}` to the agent's real \
                     OpenAI-compatible endpoint",
                    crate::domain::METERING_UPSTREAM_BASE_URL_KEY
                ),
            }
        })?;
        // The listener's accept loop runs on the engine runtime; the sync start path
        // (on the blocking pool) cannot use `Handle::current`, so the engine threads
        // its handle in (`with_runtime`). A handle-less supervisor cannot observe.
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| EngineError::ObservedMetering {
                name: name.as_str().to_string(),
                detail: "the engine has no runtime handle to run the loopback listener \
                     (engine-observed metering requires the async engine)"
                    .to_string(),
            })?;
        // Bind loopback + spawn. A ListenerError is TRAFFIC-FREE by construction
        // (bind/upstream-shape only — never a body/header/key), so mapping it into
        // the detail cannot leak a secret (2-4 rigor).
        let listener = ObservedListener::start(runtime, upstream).map_err(|e: ListenerError| {
            EngineError::ObservedMetering {
                name: name.as_str().to_string(),
                detail: e.to_string(),
            }
        })?;
        Ok(Some(listener))
    }

    /// Watch a freshly spawned process for [`READINESS_WINDOW`]. Returns
    /// `Some(exit_code)` if the process died within the window (a launch failure,
    /// AC2 — the inner `Option<i32>` is the OS exit code, `None` if killed by a
    /// signal with no code), or `None` if it stayed alive the whole window
    /// (ready). Reaps on exit (no zombie).
    fn watch_startup(&self, handle: &mut backends::Handle) -> Option<Option<i32>> {
        let deadline = std::time::Instant::now() + READINESS_WINDOW;
        loop {
            match self.backend.poll(handle) {
                Ok(ProcessStatus::Exited { code }) => return Some(code),
                Ok(ProcessStatus::Alive) => {}
                // A poll error during startup is treated as still-alive; the next
                // stop/poll will surface a real problem. Don't fail the start on a
                // transient poll hiccup.
                Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(READINESS_POLL);
        }
    }

    /// Ensure the per-instance log directory exists (AD-12 seed).
    fn ensure_log_dir(&self, registry: &Registry, name: &InstanceName) -> Result<(), EngineError> {
        let dir = registry.instance_log_dir(name);
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Log {
            name: name.as_str().to_string(),
            path: dir.to_string_lossy().into_owned(),
            detail: e.to_string(),
        })
    }

    // ---- Self-reported usage ingestion → the ONE ledger-commit choke point ----
    //      (story 3-1, spine AD-6/AD-7/AD-12)

    /// The current byte length of an instance's agent-output log, or 0 if it does
    /// not exist yet. Used to set the ingestion cursor at a Run's start so a new
    /// Run never re-reads a prior Run's already-captured lines.
    fn agent_log_len(&self, registry: &Registry, name: &InstanceName) -> u64 {
        std::fs::metadata(registry.agent_output_log_path(name))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Drain self-reported usage from EVERY currently-running instance (the reaper
    /// cadence). Best-effort per instance — one instance's drain failure never
    /// blocks another's or crash detection.
    ///
    /// This is the MID-RUN cadence: the process is (believed) still alive, so a
    /// half-written final line is left for the next pass ([`DrainMode::MidRun`]).
    fn drain_usage_all(&mut self, registry: &Registry) {
        let names: Vec<InstanceName> = self.running.keys().cloned().collect();
        for name in names {
            self.drain_usage_for(registry, &name, DrainMode::MidRun);
        }
    }

    /// Drain the NEWLY-captured tail of one instance's agent-output log, ingesting
    /// each well-formed usage sentinel line through the commit choke point
    /// ([`Supervisor::ingest_usage`]), and advance the read cursor.
    ///
    /// Reads from the per-instance cursor to the file's end (only the bytes written
    /// since the last drain), parses usage lines via the self-reported
    /// [`UsageSource`](crate::ports::UsageSource), and records each. A read error
    /// (log gone / unreadable) is a best-effort skip — the DB is the source of
    /// truth, and the next pass retries. Malformed usage lines are skipped inside
    /// the parser (a diagnostic, never fatal — AD-12).
    ///
    /// The `mode` decides how the TAIL is treated (story 3-1 under-count fix, H1):
    /// * [`DrainMode::MidRun`] — the process may still be mid-`writeln!`, so only
    ///   bytes UP TO the last newline are consumed; a partial trailing line waits
    ///   for the next drain (it lands whole then).
    /// * [`DrainMode::Terminal`] — the process is DEAD (drain-on-stop / drain-on-
    ///   reap); no more bytes will ever append, so a final usage line flushed
    ///   WITHOUT a trailing newline is consumed to end-of-log rather than stranded
    ///   (which the next Run's cursor would skip past → a permanent under-count).
    ///
    /// Log-shrink guard (M2): if the file is shorter than the cursor (a truncate /
    /// rotation — nothing in-tree does this yet; Epic 4 owns rotation), we do NOT
    /// re-read from 0 under the same live `run_id` (that would re-ingest already-
    /// counted lines → a double-count, an INFLATED bill). We instead treat it as an
    /// anomaly: advance the cursor to the new length and ingest nothing this pass.
    /// Proper rotation handling is deferred to Epic 4.
    fn drain_usage_for(&mut self, registry: &Registry, name: &InstanceName, mode: DrainMode) {
        // Only running/adopted instances have a cursor + metering context.
        let (cursor, run_id, metering_source) = match self.running.get(name) {
            Some(s) => (s.usage_cursor, s.run_id.clone(), s.metering_source.clone()),
            None => return,
        };
        let path = registry.agent_output_log_path(name);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        // Decide (purely) what this pass reads: a shrink anomaly (M2), nothing yet,
        // or a byte range to consume + the resulting cursor (H1 terminal-tail rule).
        match plan_drain(&bytes, cursor, mode) {
            DrainPlan::Shrunk { new_cursor } => {
                // M2: the file is shorter than where we last read — do NOT restart
                // from 0 (that double-counts). Snap the cursor to the new end and
                // ingest nothing this pass.
                if let Some(s) = self.running.get_mut(name) {
                    s.usage_cursor = new_cursor;
                }
            }
            DrainPlan::Nothing => {}
            DrainPlan::Consume { range, new_cursor } => {
                let block = String::from_utf8_lossy(&bytes[range]);
                let parsed = self.usage_source.drain(&block);
                // Advance the cursor FIRST (past the consumed lines) so a mid-batch
                // record failure does not re-ingest earlier lines on the next pass —
                // the DB dedup key is the ultimate guard, but not re-reading keeps it
                // cheap.
                if let Some(s) = self.running.get_mut(name) {
                    s.usage_cursor = new_cursor;
                }
                for usage in parsed {
                    self.ingest_usage(registry, name, &run_id, &metering_source, &usage);
                }
            }
        }
    }

    /// Drain ENGINE-OBSERVED usage from EVERY currently-running observed instance
    /// (story 3-4 — the reaper cadence, parallel to [`Self::drain_usage_all`]).
    /// Best-effort per instance — one instance's drain never blocks another's or
    /// crash detection. A `self-reported` instance (no observed listener) is a
    /// no-op here (it rides the log-tail drain instead).
    fn drain_observed_all(&mut self, registry: &Registry) {
        let names: Vec<InstanceName> = self.running.keys().cloned().collect();
        for name in names {
            self.drain_observed_for(registry, &name);
        }
    }

    /// Drain one instance's OBSERVED usage queue (the counts the loopback listener
    /// parsed out of the agent's model traffic) into the SAME [`Self::ingest_usage`]
    /// choke point (story 3-4), minting the per-Run `sequence` for each.
    ///
    /// The listener task PUSHES each parsed `(input, output)` pair; this reaper pass
    /// DRAINS the queue (event-driven, NOT the log-tail path — observed usage does
    /// NOT ride the agent-output log, AD-12 contrast), the [`ObservedUsageSource`]
    /// mints the engine-side `sequence` (the agent supplies none), and each becomes
    /// a `ParsedUsage` fed to `ingest_usage` under the instance's CURRENT Run id +
    /// `engine-observed` source. NO new ledger writer, NO new enforcement path — the
    /// SAME choke point stamps + records + enforces (so 3-2 budgets + 3-3 caps apply
    /// unchanged). A `self-reported` instance (no `observed_source`/`observed_listener`)
    /// is a no-op. Best-effort: a lock hiccup skips this pass, never a crash.
    fn drain_observed_for(&mut self, registry: &Registry, name: &InstanceName) {
        // Read the Run context + drain the queue under the instance's held state.
        // Collect the pushed counts + mint the per-Run sequence for each FIRST (a
        // short critical section), then ingest OUTSIDE the borrow so `ingest_usage`
        // can take `&mut self`.
        let (run_id, metering_source, minted) = match self.running.get(name) {
            Some(s) => {
                // Only an observed instance has both a listener (its queue) + a source
                // (the sequence minter). A self-reported instance skips (no-op).
                let (Some(listener), Some(source)) =
                    (s.observed_listener.as_ref(), s.observed_source.as_ref())
                else {
                    return;
                };
                let queue = listener.queue();
                // Drain the queue: take every pushed pair (the lock is held only for
                // the swap). A poisoned/failed lock is a best-effort skip.
                let drained: Vec<(u64, u64)> = match queue.lock() {
                    Ok(mut q) => q.drain(..).collect(),
                    Err(_) => return,
                };
                if drained.is_empty() {
                    return;
                }
                // Mint the per-Run ParsedUsage for each observed completion (the
                // engine stamps `sequence`; the agent supplies none).
                let minted: Vec<ParsedUsage> = drained
                    .into_iter()
                    .map(|(input, output)| source.mint(input, output))
                    .collect();
                (s.run_id.clone(), s.metering_source.clone(), minted)
            }
            None => return,
        };
        // Ingest each observed event through the SAME single choke point (stamps the
        // Run id + `engine-observed` source + timestamp, records, and enforces).
        for usage in &minted {
            self.ingest_usage(registry, name, &run_id, &metering_source, usage);
        }
    }

    /// THE ledger-commit choke point (story 3-1, spine AD-7) — the SOLE writer of
    /// the `usage_events` table.
    ///
    /// Constructs the full [`UsageEvent`] from the agent-supplied [`ParsedUsage`]
    /// plus the engine-stamped fields (the current Run id, the instance name, the
    /// metering source, and the commit timestamp), then records it in its OWN
    /// transaction via `record_usage_event` (AD-6: one transaction per event). A
    /// re-delivered batch is classified [`RecordOutcome::DuplicateReplay`] by the
    /// DB `UNIQUE` index and is a no-op (AC-A no-double-count). On a fresh insert it
    /// builds the AD-14 [`UsageUpdateEvent`] (the wire shape frozen now; Host
    /// delivery is story 7-2). A store error is a best-effort diagnostic — usage
    /// ingestion must never crash the supervisor or a lifecycle op (the ledger is
    /// advisory to the RUN, not gating it this story).
    ///
    /// **The AD-7 single-writer invariant lives here:** no other code path may call
    /// `record_usage_event`.
    ///
    /// **The AD-7 ENFORCEMENT stage lives here too (story 3-2):** IMMEDIATELY after
    /// a fresh `Inserted` commit — in the SAME synchronous path, before returning —
    /// this method reads the CURRENT resolved [`TokenBudget`] + [`BreachAction`]
    /// (a LIVE config read, so a budget changed while `running` applies on the very
    /// next event — AC-B), reads the just-committed per-run + cumulative token
    /// totals (3-1's `usage_totals`/`run_totals`), and calls the pure
    /// [`BudgetEvaluator`]. On a [`BreachDecision::Breached`] it RECORDS the breach
    /// event FIRST/independently ([`Self::record_breach`]) — so a best-effort/
    /// unsupported/failed pause never loses the breach record (FR-21 "always
    /// recorded regardless of action") — and THEN executes the action via Epic-1's
    /// lifecycle (`pause`/`stop`/`warn`). This is the SOLE enforcement site (the
    /// AD-7 companion to the single-writer invariant). A [`RecordOutcome::DuplicateReplay`]
    /// is NOT evaluated (nothing new was committed → no new breach can occur).
    /// Ingestion + enforcement stay best-effort to the RUN: a store/lifecycle error
    /// is a diagnostic, NEVER a supervisor crash (3-1's rule extended to enforcement).
    fn ingest_usage(
        &mut self,
        registry: &Registry,
        name: &InstanceName,
        run_id: &RunId,
        metering_source: &str,
        parsed: &ParsedUsage,
    ) -> Option<UsageUpdateEvent> {
        let event = assemble_usage_event(
            parsed,
            name.as_str(),
            run_id.clone(),
            metering_source,
            now_rfc3339(),
        );
        // Story 3-3 — NO-RETROACTIVE-REPRICING: resolve the EFFECTIVE Rate at COMMIT
        // (a live config read) and PERSIST it onto this row, so historical dollars
        // keep the Rate in force when consumed. A later Rate change re-prices FUTURE
        // events only (each row is priced at its own stored Rate on read). A degraded
        // config read / absent-or-half Rate → `None` (the row contributes $0; AC-B).
        let rate = registry
            .effective_config(name, ConfigLayer::empty())
            .ok()
            .and_then(|eff| config::resolve_cost(&eff).0);
        match registry.record_usage_event(&event, rate) {
            // A fresh row: build the AD-14 usage-update wire struct (frozen now; 7-2
            // delivers it), THEN run the AD-7 enforcement stage on the just-committed
            // totals — synchronously, in this same commit path.
            Ok(RecordOutcome::Inserted) => {
                self.enforce_budget(registry, name, run_id, metering_source);
                Some(UsageUpdateEvent::new(event))
            }
            // A recognized replay — no double-count, no event emitted (nothing new
            // was committed). This is the AC-A guarantee in action; the evaluator is
            // NOT run (AC5 — no new total, no new breach).
            Ok(RecordOutcome::DuplicateReplay) => None,
            // A store error: usage ingestion is best-effort — do not crash the
            // supervisor. The ledger is the source of truth for what WAS recorded;
            // a transient write failure just means this event is not counted (the
            // agent may re-send it, and the dedup key keeps that safe).
            Err(_) => None,
        }
    }

    /// The AD-7 ENFORCEMENT stage (story 3-2 tokens + story 3-3 dollars), run INSIDE
    /// [`Self::ingest_usage`] right after a fresh commit — the SOLE place a budget or
    /// Cost Cap is evaluated + a Breach Action fired.
    ///
    /// Reads the CURRENT resolved budget/Rate/cap + action (live, AC-B), reads the
    /// committed per-run + cumulative totals, evaluates purely (TOKENS then DOLLARS,
    /// in the SAME choke point), and on a breach records the event FIRST then
    /// executes the action. Every step is best-effort to the RUN: a failed config
    /// read / totals read / lifecycle op is a diagnostic, never a crash (AD-12). A
    /// no-budget + no-cap instance evaluates to `WithinBudget` for both — so the
    /// common path is a cheap config read + two pure comparisons and nothing else.
    ///
    /// **STORY 3-3 — the DOLLAR evaluation folds in HERE (AD-7, no new path):** after
    /// the token evaluation, IF a [`Rate`](super::cost::Rate) is present AND the
    /// [`CostCap`](super::cost::CostCap) `is_set()`, derive the per-run + cumulative
    /// COST (each row priced at its own persisted Rate — no retro-repricing) and run
    /// the pure [`CostEvaluator`]. NO Rate ⇒ dollar enforcement is SKIPPED entirely
    /// (AC-B inert — a `CostCap` with no Rate cannot be enforced). Both dimensions
    /// reuse the SAME record-first-then-act path + the SAME per-Run latch, keyed by
    /// `(dimension, scope)` so a token breach and a dollar breach of the same scope
    /// each fire ONCE per Run (both can fire on the same event; the action is
    /// identical).
    ///
    /// **Idempotence — at most one breach per (dimension, scope) per Run:** this runs
    /// on EVERY committed usage event, so once a total crosses a ceiling every
    /// subsequent event would re-evaluate to the SAME breach. The per-Run breach
    /// LATCH ([`Supervised::breached_scopes`], keyed by `(dimension, scope)`)
    /// short-circuits BOTH the [`Self::record_breach`] and the action for an
    /// already-fired pair; the latch resets when a new Run starts.
    fn enforce_budget(
        &mut self,
        registry: &Registry,
        name: &InstanceName,
        run_id: &RunId,
        metering_source: &str,
    ) {
        // (1) LIVE config read (AC-B "changes apply immediately"): resolve the
        // CURRENT effective config ONCE, for BOTH the token budget and the dollar
        // Rate/cap. A malformed on-disk layer degrades to "no budget / no Rate"
        // (best-effort — never a crash mid-ingestion).
        let Ok(effective) = registry.effective_config(name, ConfigLayer::empty()) else {
            return;
        };
        let (budget, token_action) = config::resolve_token_budget(&effective);
        let (rate, cost_cap, cost_action) = config::resolve_cost(&effective);

        // Whether each dimension is ARMED: a token budget is armed when a ceiling is
        // set; the dollar cap is armed ONLY when BOTH a Rate is present AND a cap
        // scope is set (AC-B: a cap with no Rate is inert). If NEITHER is armed, skip
        // the totals reads entirely (the common un-governed path).
        let token_armed = budget.is_set();
        let dollar_armed = rate.is_some() && cost_cap.is_set();
        if !token_armed && !dollar_armed {
            return;
        }

        // (2) TOKEN dimension (story 3-2): the just-committed token totals + the pure
        // evaluator. Reuses the record-first-then-act helper with the tokens dimension.
        if token_armed {
            let run_total = registry
                .run_usage_totals(name, run_id)
                .map(|t| t.total_tokens())
                .unwrap_or(0);
            let cumulative_total = registry
                .usage_totals(name)
                .map(|t| t.total_tokens())
                .unwrap_or(0);
            let decision =
                BudgetEvaluator::evaluate(run_total, cumulative_total, &budget, token_action);
            if let BreachDecision::Breached {
                scope,
                action,
                limit,
                observed,
            } = decision
            {
                let cause = TransitionCause::budget_exceeded(scope, limit, observed);
                self.apply_breach(
                    registry,
                    name,
                    run_id,
                    BreachDimension::Tokens,
                    scope,
                    action,
                    cause,
                    metering_source,
                    // The token breach event carries token counts (no dollar fields).
                    |registry, sup, run_id, scope, action, src| {
                        sup.record_token_breach(
                            registry, name, run_id, scope, limit, observed, action, src,
                        );
                    },
                );
            }
        }

        // (3) DOLLAR dimension (story 3-3): derive the per-run + cumulative COST from
        // the ledger (each row priced at its own persisted Rate — no retro-repricing),
        // then the pure CostEvaluator. Only when a Rate is present AND the cap is set
        // (AC-B inert otherwise). v1 the estimate label is always `estimated`.
        if dollar_armed {
            let run_cost = registry
                .run_cost_totals(name, run_id)
                .unwrap_or(Micros::ZERO);
            let cumulative_cost = registry.cost_totals(name).unwrap_or(Micros::ZERO);
            let decision =
                CostEvaluator::evaluate(run_cost, cumulative_cost, &cost_cap, cost_action);
            if let BreachDecision::Breached {
                scope,
                action,
                limit,
                observed,
            } = decision
            {
                let label = EstimateLabel::Estimated;
                let cause = TransitionCause::cost_cap_exceeded(
                    scope,
                    Micros(limit as i64),
                    Micros(observed as i64),
                    label,
                );
                self.apply_breach(
                    registry,
                    name,
                    run_id,
                    BreachDimension::Dollars,
                    scope,
                    action,
                    cause,
                    metering_source,
                    // The dollar breach event carries integer micros + the label.
                    |registry, sup, run_id, scope, action, src| {
                        sup.record_cost_breach(
                            registry,
                            name,
                            run_id,
                            scope,
                            Micros(limit as i64),
                            Micros(observed as i64),
                            label,
                            action,
                            src,
                        );
                    },
                );
            }
        }
    }

    /// Apply ONE breach decision for a given `dimension` (story 3-3 shared path):
    /// consult the per-Run `(dimension, scope)` latch, and if this pair has NOT yet
    /// fired this Run, RECORD the breach (via `record`, the dimension-specific event
    /// writer) FIRST/INDEPENDENTLY and THEN execute the action via Epic-1's lifecycle
    /// (AD-15 — a REASON, not a new edge). A pair already latched short-circuits both.
    /// A missing `Supervised` (not currently supervised — a race with stop) declines
    /// to enforce. All best-effort: a lifecycle error is a diagnostic, never a crash.
    #[allow(clippy::too_many_arguments)]
    fn apply_breach(
        &mut self,
        registry: &Registry,
        name: &InstanceName,
        run_id: &RunId,
        dimension: BreachDimension,
        scope: BreachScope,
        action: BreachAction,
        cause: TransitionCause,
        metering_source: &str,
        record: impl FnOnce(&Registry, &Self, &RunId, BreachScope, BreachAction, &str),
    ) {
        // IDEMPOTENCE LATCH (story 3-2/3-3): fire at most once per (dimension, scope)
        // per Run. Insert the pair; if it was already present, short-circuit.
        match self.running.get_mut(name) {
            Some(supervised) => {
                if !supervised.breached_scopes.insert((dimension, scope)) {
                    return;
                }
            }
            None => return,
        }
        // RECORD THE BREACH FIRST (AC7/AC10 / FR-21 "always recorded regardless of
        // action"), BEFORE the lifecycle side-effect, so a best-effort/unsupported/
        // failing pause never loses the record.
        record(registry, self, run_id, scope, action, metering_source);
        // EXECUTE THE ACTION via Epic-1's EXISTING lifecycle. The breach is already
        // recorded; a lifecycle error here is a best-effort diagnostic, never a crash.
        match action {
            BreachAction::Warn => {
                // No lifecycle transition — the breach event is the whole guardrail.
            }
            BreachAction::Pause => {
                self.enforce_pause(registry, name, cause);
            }
            BreachAction::Stop => {
                self.enforce_stop(registry, name, cause);
            }
        }
    }

    /// Execute a `pause` Breach Action honestly (story 3-2 AC6, honoring story
    /// 1-5's Capability Declaration). Drives `running → paused` and STAMPS the
    /// resulting transition with the [`TransitionCause::BudgetExceeded`] cause (so
    /// the lifecycle log explains WHY), via [`Self::pause`]. A best-effort pause
    /// still transitions (1-5) and the breach is already recorded; an UNSUPPORTED
    /// pause fails fast in [`Self::pause`] — we do NOT fake a pause and do NOT
    /// silently escalate to stop (AC6), we surface the honest diagnostic on the
    /// engine log (the breach event already captured the fact). All best-effort:
    /// never a supervisor crash.
    fn enforce_pause(&mut self, registry: &Registry, name: &InstanceName, cause: TransitionCause) {
        match self.pause_with_cause(registry, name, cause) {
            Ok(_) => {}
            Err(e) => {
                // Honest surface (AD-12): pause could not be honored (unsupported /
                // not running / backend hiccup). The breach is ALREADY recorded; log
                // and move on — no fake pause, no escalation.
                self.log_enforcement_diagnostic(
                    registry,
                    name,
                    &format!("budget breach pause could not be honored: {e}"),
                );
            }
        }
    }

    /// Execute a `stop` Breach Action (story 3-2). Drives `running → stopping →
    /// stopped` (story 1-4) and, before that, records the [`TransitionCause::BudgetExceeded`]
    /// as the WHY marker on the `running → stopping` edge (the stop path itself
    /// records the graceful/forced escalation on the terminal edge). Best-effort:
    /// a stop error is logged, never a crash (the breach is already recorded).
    fn enforce_stop(&mut self, registry: &Registry, name: &InstanceName, cause: TransitionCause) {
        match self.stop_with_cause(registry, name, cause) {
            Ok(_) => {}
            Err(e) => {
                self.log_enforcement_diagnostic(
                    registry,
                    name,
                    &format!("budget breach stop could not be honored: {e}"),
                );
            }
        }
    }

    /// Record a TOKEN [`BudgetBreachEvent`] (story 3-2, AC7) — the token-dimension
    /// event writer passed to [`Self::apply_breach`]. Builds the token breach struct
    /// (token `limit`/`observed`, no dollar fields) and persists it via
    /// [`Self::persist_breach_event`].
    #[allow(clippy::too_many_arguments)]
    fn record_token_breach(
        &self,
        registry: &Registry,
        name: &InstanceName,
        run_id: &RunId,
        scope: BreachScope,
        limit: u64,
        observed: u64,
        action: BreachAction,
        metering_source: &str,
    ) {
        let event = BudgetBreachEvent::new(
            name.as_str(),
            run_id.as_str(),
            scope,
            limit,
            observed,
            action,
            metering_source,
            now_rfc3339(),
        );
        self.persist_breach_event(registry, name, &event);
    }

    /// Record a DOLLAR [`BudgetBreachEvent`] (story 3-3, AC10) — the dollar-dimension
    /// event writer passed to [`Self::apply_breach`]. Builds the dollar breach struct
    /// (integer-micro `dollar_limit`/`dollar_observed` + the [`EstimateLabel`]) and
    /// persists it via [`Self::persist_breach_event`]. NO `$` string, NO `f64` — the
    /// wire carries integer micros + the label (AD-14).
    #[allow(clippy::too_many_arguments)]
    fn record_cost_breach(
        &self,
        registry: &Registry,
        name: &InstanceName,
        run_id: &RunId,
        scope: BreachScope,
        limit_micros: Micros,
        observed_micros: Micros,
        label: EstimateLabel,
        action: BreachAction,
        metering_source: &str,
    ) {
        let event = BudgetBreachEvent::new_cost(
            name.as_str(),
            run_id.as_str(),
            scope,
            limit_micros,
            observed_micros,
            label,
            action,
            metering_source,
            now_rfc3339(),
        );
        self.persist_breach_event(registry, name, &event);
    }

    /// Persist a built [`BudgetBreachEvent`] to the durable per-instance breach log
    /// (story 3-2 shared path, AC7 / FR-21 "always recorded regardless of action").
    /// Recorded for EVERY action (including `warn`) and BEFORE the lifecycle
    /// side-effect, so the breach is never lost.
    ///
    /// Non-fatal but NOT swallowed: this is the PRIMARY durable record of the breach
    /// (FR-21), so a write failure (disk full / IO / perms) must not vanish silently
    /// while the action still fires — that would lose the mandated record with no
    /// diagnostic. We keep enforcement acting (the write failure is NOT made fatal),
    /// but SURFACE the error on the engine-log stderr breadcrumb (mirroring how
    /// `enforce_pause`/`enforce_stop` log their best-effort diagnostics), so a lost
    /// breach record is visible to an operator. Both the dir-create and the append
    /// failure are surfaced.
    fn persist_breach_event(
        &self,
        registry: &Registry,
        name: &InstanceName,
        event: &BudgetBreachEvent,
    ) {
        let path = registry.instance_breach_log_path(name);
        // Ensure the log dir exists (a never-transitioned instance may lack it) —
        // non-fatal, mirroring `ensure_log_dir`, but a failure is surfaced (below) if
        // it then makes the append fail.
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.log_enforcement_diagnostic(
                    registry,
                    name,
                    &format!(
                        "could not create the breach-log directory {}: {e} — the breach \
                         record may be lost",
                        parent.display()
                    ),
                );
            }
        }
        // Surface (do NOT swallow) an append failure: the breach record is the FR-21
        // mandated durable artifact; a lost record with no diagnostic is the bug. Log
        // and move on — enforcement still acts.
        if let Err(e) = append_breach_event(&path, event) {
            self.log_enforcement_diagnostic(
                registry,
                name,
                &format!(
                    "could not record the budget breach event to {}: {e} — the mandated \
                     breach record was NOT written",
                    path.display()
                ),
            );
        }
    }

    /// Surface one enforcement diagnostic on STDERR (AD-12: enforcement
    /// diagnostics ride the engine log / stderr, NEVER `kt` stdout, NEVER a crash).
    /// Used when a breach action (pause/stop) could not be honored — the breach
    /// itself is already durably recorded in the breach log, so this is only an
    /// operator breadcrumb, not the record of the breach. `registry` is unused (the
    /// diagnostic is not persisted to a strict-parse log to avoid corrupting the
    /// transition-event reader) but kept for signature symmetry with the other
    /// enforcement helpers.
    fn log_enforcement_diagnostic(&self, _registry: &Registry, name: &InstanceName, detail: &str) {
        eprintln!("[ktesio] {}: {detail}", name.as_str());
    }

    /// Read back the recorded [`BudgetBreachEvent`]s for an instance from its
    /// breach log (observation helper for tests / embedders — the AD-14 seed, NOT
    /// the 7-2 bus). Empty vec if none recorded yet.
    pub fn read_breach_events(
        registry: &Registry,
        name: &str,
    ) -> Result<Vec<BudgetBreachEvent>, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let path = registry.instance_breach_log_path(&name);
        read_breach_events_from(&path).map_err(|detail| EngineError::Log {
            name: name.as_str().to_string(),
            path: path.to_string_lossy().into_owned(),
            detail,
        })
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the INVOCATION-OVERRIDE config layer that injects the engine-observed
/// loopback `base_url` into the mapping (story 3-4, AC6). The engine writes the
/// listener's `http://127.0.0.1:<port>` at the reserved
/// [`METERING_BASE_URL_KEY`](crate::domain::METERING_BASE_URL_KEY) so the
/// adapter's EXISTING config-mapping (2-2) delivers it into the agent's native
/// mechanism. Because it is the INVOCATION layer (the strongest — AD-9), it always
/// wins over any hand-set lower-layer value; and because the mapping reads it as an
/// ordinary string leaf, NO new contract surface is introduced (no
/// `CONTRACT_VERSION` bump). Pure — builds a one-key TOML table.
fn base_url_override(base_url: &str) -> ConfigLayer {
    let mut table = toml::value::Table::new();
    // A DOTTED key (`metering.base_url`) is a nested table in TOML; build the nested
    // shape so `resolve` flattens it to the dotted leaf the mapping targets.
    let mut metering = toml::value::Table::new();
    metering.insert(
        "base_url".to_string(),
        toml::Value::String(base_url.to_string()),
    );
    table.insert("metering".to_string(), toml::Value::Table(metering));
    ConfigLayer::from_table(table)
}

/// A best-effort, HUMAN-READABLE one-line rendering of a [`TransitionEvent`]
/// for the `engine`-attributed capture line (story 4-2, Task 4) — the
/// RECOMMENDED default: mirror every `TransitionEvent` (start/stop/pause/
/// resume/crash/restart/breach-driven), the SAME set `instance.log` already
/// records, so this is a projection of IDENTICAL facts, not a second,
/// divergent notion of "notable". `instance.log` stays the structured,
/// machine-authoritative record; this text is NEVER parsed back — a wording
/// change here is not a wire-format change.
fn engine_transition_line_text(event: &TransitionEvent) -> String {
    format!(
        "engine: {} -> {}{}",
        event.prior_state,
        event.new_state,
        cause_suffix(&event.cause)
    )
}

/// The parenthetical detail suffix for [`engine_transition_line_text`], keyed
/// on the transition's [`TransitionCause`].
fn cause_suffix(cause: &TransitionCause) -> String {
    match cause {
        TransitionCause::Command { command } => format!(" ({command})"),
        TransitionCause::AdapterReady => String::new(),
        TransitionCause::LaunchError { detail } => format!(" (launch error: {detail})"),
        TransitionCause::StopGraceful => String::new(),
        TransitionCause::StopForced { detail } => format!(" (forced: {detail})"),
        TransitionCause::PauseBestEffort { detail } => format!(" (best-effort: {detail})"),
        TransitionCause::ResumeBestEffort { detail } => format!(" (best-effort: {detail})"),
        TransitionCause::Crashed { detail } => format!(" (crashed: {detail})"),
        TransitionCause::Restarted { count, waited_ms } => {
            format!(" (restart #{count}, waited {waited_ms}ms)")
        }
        TransitionCause::BudgetExceeded {
            scope, dimension, ..
        } => format!(" (breach: {scope} {dimension})"),
    }
}

/// Append one transition event as a single JSON line to the instance log.
///
/// One event per line (JSON Lines) so [`read_events_from`] can parse them back
/// and a human can `tail` the file. Append-only (AD-12 seed; rotation/attach are
/// Epic 4).
fn append_event(path: &Path, event: &TransitionEvent) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// Read back the JSON-Lines transition events from an instance log.
///
/// Missing file → empty vec (no events recorded yet). A malformed line is an
/// error naming it (a corrupt log is worth surfacing).
fn read_events_from(path: &Path) -> Result<Vec<TransitionEvent>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut events = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: TransitionEvent = serde_json::from_str(line)
            .map_err(|e| format!("corrupt instance-log line {}: {e}", idx + 1))?;
        events.push(event);
    }
    Ok(events)
}

/// Parse JSON-Lines [`LogLine`] records from `text`, APPENDING them to `out`
/// in encounter order (story 4-2, AC-G — append order is the sole ordering
/// authority; callers must never re-sort the result). Blank lines are
/// skipped; a malformed line is an error naming it (a corrupt capture is
/// worth surfacing, mirroring [`read_events_from`]'s convention).
fn parse_log_lines(text: &str, out: &mut Vec<LogLine>) -> Result<(), String> {
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: LogLine = serde_json::from_str(line)
            .map_err(|e| format!("corrupt output-log line {}: {e}", idx + 1))?;
        out.push(parsed);
    }
    Ok(())
}

/// Read back one attributed-output-log FILE (one generation) and append its
/// parsed [`LogLine`]s to `out` — a missing generation (not every generation
/// exists yet) is a silent no-op, mirroring [`read_events_from`]'s "missing
/// file → empty" precedent, so [`Supervisor::read_agent_log`]'s
/// oldest-to-newest loop can unconditionally probe every generation.
fn read_log_lines_from(path: &Path, out: &mut Vec<LogLine>) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    parse_log_lines(&text, out)
}

/// Append one [`BudgetBreachEvent`] as a single JSON line to the per-instance
/// breach log (story 3-2, AD-14). JSON Lines, append-only — the same shape as
/// [`append_event`] so a human can `tail` it and [`read_breach_events_from`] can
/// parse it back. The ALWAYS-recorded breach record (FR-21).
fn append_breach_event(path: &Path, event: &BudgetBreachEvent) -> Result<(), String> {
    use std::io::Write;
    let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// Read back the JSON-Lines [`BudgetBreachEvent`]s from an instance's breach log.
/// Missing file → empty vec (no breaches recorded yet). A malformed line is an
/// error naming it (a corrupt log is worth surfacing).
fn read_breach_events_from(path: &Path) -> Result<Vec<BudgetBreachEvent>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut events = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: BudgetBreachEvent = serde_json::from_str(line)
            .map_err(|e| format!("corrupt breach-log line {}: {e}", idx + 1))?;
        events.push(event);
    }
    Ok(events)
}

/// Map a registry lookup/persist error into the lifecycle [`EngineError`].
///
/// Registration and lifecycle share the same NotFound/InvalidName shapes; keep
/// them as the lifecycle variants so `kt` maps them consistently. Exposed
/// `pub(crate)` so the engine facade's status read (story 1-6, AC9) maps registry
/// errors the same way the supervisor does.
pub(crate) fn registry_to_engine(err: super::error::RegistryError) -> EngineError {
    use super::error::RegistryError as R;
    match err {
        R::NotFound { name } => EngineError::NotFound { name },
        R::InvalidName { name, reason } => EngineError::InvalidName { name, reason },
        R::Io { name, path, source } => EngineError::Log {
            name,
            path,
            detail: source.to_string(),
        },
        R::Store(inner) => EngineError::Store(inner),
        // Any other registry error surfaces as an adapter-unresolved detail
        // (e.g. a missing/corrupt snapshot the supervisor needs to launch).
        other => EngineError::AdapterUnresolved {
            name: "<unknown>".to_string(),
            detail: other.to_string(),
        },
    }
}

/// Map a launch-spec resolution failure into the lifecycle [`EngineError`].
fn launch_to_engine(name: &InstanceName, err: LaunchResolveError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map a config-resolution failure (story 2-2) encountered while mapping the
/// resolved unified config into the launch into the lifecycle [`EngineError`]. A
/// malformed config layer / missing instance surfaces as an unresolved-adapter
/// launch failure (the config could not be mapped into the launch), naming the
/// instance + detail; the start rejects BEFORE any state change (mirrors a bad
/// manifest).
fn config_to_engine(name: &InstanceName, err: crate::domain::ConfigError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map a config-mapping APPLICATION failure (story 2-2) — a FILE target that
/// could not be rendered into the Agent Home — into the lifecycle [`EngineError`].
/// Surfaces as an unresolved-adapter launch failure naming the instance + detail;
/// the start rejects before any state change (the file write happens before the
/// `starting` transition), so a bad file target never leaves a spurious state.
fn config_apply_to_engine(name: &InstanceName, err: ConfigApplyError) -> EngineError {
    EngineError::AdapterUnresolved {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

/// Map an effective-config SNAPSHOT-write failure (story 2-3) into the lifecycle
/// [`EngineError`]. The snapshot write lands BEFORE the `starting` transition, so
/// a failure here rejects the start with no state change. A
/// [`RegistryError::SnapshotWrite`] already carries the instance + snapshot path +
/// detail; map it to the dedicated [`EngineError::Snapshot`] naming the same, so
/// `kt` renders a precise "could not write the effective-config snapshot"
/// diagnostic with a permissions/disk remediation (NFR-1). Any other registry
/// error (not expected from this call) falls back to the shared registry mapper.
fn snapshot_to_engine(err: super::error::RegistryError) -> EngineError {
    match err {
        super::error::RegistryError::SnapshotWrite { name, path, detail } => {
            EngineError::Snapshot { name, path, detail }
        }
        other => registry_to_engine(other),
    }
}

/// Map a SECRET-resolution failure (story 2-4) into the lifecycle [`EngineError`].
/// The resolution runs BEFORE the config mapping + the `starting` transition, so a
/// failure here rejects the start with no state change (mirroring
/// [`snapshot_to_engine`]). The [`SecretError`] message names the `NAME` + the
/// resolvers tried (or the `chmod 600` remediation) but NEVER a resolved value, so
/// mapping it into [`EngineError::Secret`]'s `detail` cannot leak a secret (AC-B).
fn secret_to_engine(name: &InstanceName, err: crate::ports::SecretError) -> EngineError {
    EngineError::Secret {
        name: name.as_str().to_string(),
        detail: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterRef, StartLaunch};
    use crate::domain::RestartPolicy;
    use std::time::Instant;

    /// A fast backoff schedule so the crash/restart/crash-loop lib tests never
    /// sleep for real seconds (production stays 1s×2 cap 60s — Task 2 guards it).
    fn fast_backoff() -> BackoffSchedule {
        BackoffSchedule::with_base_and_cap(Duration::from_millis(5), Duration::from_millis(20))
    }

    /// Write a manifest whose `[lifecycle.start]` exec is `fake_agent` + `args`.
    fn write_fake_manifest(dir: &Path, kind: &str, args: &[&str]) {
        let bin = ktesio_conformance::fake_agent_bin();
        let args_toml = args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "contract_version = \"0.1.0\"\n\n\
             [adapter]\nkind = \"{kind}\"\n\n\
             [lifecycle.start]\nexec = {exec:?}\nargs = [{args_toml}]\n\n\
             [capabilities.interaction]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"guaranteed\"\n\n\
             [metering]\nsource = \"self-reported\"\n",
            exec = bin.to_string_lossy(),
        );
        std::fs::write(dir.join("adapter.toml"), body).unwrap();
    }

    /// Register a `fake_agent`-backed instance under `name` with `args`, in a
    /// fresh state dir. Returns the (state dir, manifest dir, registry).
    fn setup_fake(name: &str, args: &[&str]) -> (tempfile::TempDir, tempfile::TempDir, Registry) {
        let state = tempfile::tempdir().unwrap();
        let manifest = tempfile::tempdir().unwrap();
        write_fake_manifest(manifest.path(), name, args);
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry
            .register_with_adapter(name, &AdapterRef::Manifest(manifest.path().to_path_buf()))
            .unwrap();
        (state, manifest, registry)
    }

    /// Story 2-2: write a `fake_agent` manifest with `args` PLUS a `[config]`
    /// mapping section (`config_toml` is the section body, e.g.
    /// `"[config.model]\nflag = \"--model\"\n"`). Used by the manifest end-to-end
    /// mapping proofs.
    fn write_fake_manifest_with_config(dir: &Path, kind: &str, args: &[&str], config_toml: &str) {
        let bin = ktesio_conformance::fake_agent_bin();
        let args_toml = args
            .iter()
            .map(|a| format!("{a:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!(
            "contract_version = \"0.1.0\"\n\n\
             [adapter]\nkind = \"{kind}\"\n\n\
             [lifecycle.start]\nexec = {exec:?}\nargs = [{args_toml}]\n\n\
             [capabilities.interaction]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"guaranteed\"\n\n\
             [metering]\nsource = \"self-reported\"\n\n\
             {config_toml}",
            exec = bin.to_string_lossy(),
        );
        std::fs::write(dir.join("adapter.toml"), body).unwrap();
    }

    /// Register a `fake_agent`-backed instance carrying a `[config]` mapping.
    /// Returns the (state dir, manifest dir, registry).
    fn setup_fake_with_config(
        name: &str,
        args: &[&str],
        config_toml: &str,
    ) -> (tempfile::TempDir, tempfile::TempDir, Registry) {
        let state = tempfile::tempdir().unwrap();
        let manifest = tempfile::tempdir().unwrap();
        write_fake_manifest_with_config(manifest.path(), name, args, config_toml);
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry
            .register_with_adapter(name, &AdapterRef::Manifest(manifest.path().to_path_buf()))
            .unwrap();
        (state, manifest, registry)
    }

    /// Poll for a `--dump` file to appear (the spawned `fake_agent` writes it at
    /// startup) and return its contents, bounded — avoids racing the spawn.
    fn wait_for_dump(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            assert!(
                Instant::now() < deadline,
                "dump file never appeared at {path:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Poll until `poll_once` reports the crash (returns its plans), bounded.
    fn wait_for_crash(sup: &mut Supervisor, registry: &Registry) -> Vec<RestartPlan> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let plans = sup.poll_once(registry);
            // Once the instance has crashed it is no longer in `running`; the
            // crash transition has landed. `poll_once` returns the plan on the
            // pass that detects the exit.
            if !plans.is_empty() {
                return plans;
            }
            // Also stop once nothing is supervised AND state is failed (a `never`
            // policy returns no plan but still crashes).
            if sup.running.is_empty() {
                return plans;
            }
            assert!(Instant::now() < deadline, "crash was never detected");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn state_of(registry: &Registry, name: &str) -> LifecycleState {
        registry
            .lookup(&InstanceName::new(name).unwrap())
            .unwrap()
            .state
    }

    // ---- Story 2-2: unified→native config mapping proven at start (AC-A/AC-B) ----

    #[test]
    fn mock_native_start_maps_model_to_the_declared_env_target() {
        // AC-A + AC8 (the MOCK/native proof). The builtin `mock` is INERT (no live
        // process — NativeHasNoLaunch), so a `mock` start cannot spawn to observe.
        // Per the recorded inert-mock strategy (Decision 8), we assert on the
        // MAPPED launch the mapping application PRODUCES: register a mock, set the
        // documented `model` key (2-1), then resolve the mock's code-declared
        // mapping + the effective config and apply — the mock's declared native
        // target (env `MODEL`) must carry the value. This is exactly the transform
        // the start seam runs; a launchable native agent is a manifest adapter.
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        registry.register("mck", "mock").unwrap();
        let name = InstanceName::new("mck").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();

        // Resolve exactly as start_inner would for a native adapter.
        let (kind, manifest_path, launch) = registry.adapter_launch_facts(&name).unwrap();
        assert_eq!(kind, "mock");
        assert!(manifest_path.is_none(), "mock is native (no manifest)");
        assert!(launch.is_none(), "mock is native (no snapshotted launch)");
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .unwrap();
        let mapping = adapter::resolve_config_mapping(&kind, manifest_path.as_deref()).unwrap();
        // The mock declares `model` → env `MODEL`.
        assert_eq!(mapping.target("model").unwrap().env_var(), Some("MODEL"));

        // Apply onto a bare launch (the mock has no [lifecycle.start] template;
        // this is the launch shape the mapping would produce).
        let mut launch = StartLaunch {
            exec: "mock".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
        };
        adapter::apply_config_mapping(
            &mut launch,
            &mapping,
            &effective,
            &std::collections::BTreeMap::new(),
            &registry.agent_home(&name),
        )
        .unwrap();
        assert_eq!(
            launch.env.get("MODEL").map(String::as_str),
            Some("gpt-4"),
            "the documented model key must land in the mock's declared env target"
        );
    }

    // ---- The launch-snapshot fix (hosted-runner arg-loss): start uses the
    //      REGISTRATION snapshot, never a start-time manifest re-read ----

    #[test]
    fn start_uses_the_registration_launch_snapshot_not_a_manifest_reread() {
        // The FIX, at the EXACT seam the supervisor uses (`adapter_launch_facts`),
        // OS-agnostically (no spawn — runs on every platform, including the
        // macOS/Windows CI that dropped the args): register a fake_agent manifest
        // with distinctive args, DELETE the manifest file, then read the launch
        // facts. The persisted exec + args + env still come back intact — and the
        // fallback re-read now FAILS (the manifest is gone), proving the snapshot,
        // not a re-read, is what carries the launch into `start`.
        let (_state, manifest, registry) =
            setup_fake("snap", &["--emit-usage", "5", "--linger-ms", "600000"]);
        let name = InstanceName::new("snap").unwrap();

        // Remove the manifest entirely — any start-time re-read of it now fails.
        std::fs::remove_file(manifest.path().join("adapter.toml")).unwrap();

        let (kind, manifest_path, launch) = registry.adapter_launch_facts(&name).unwrap();
        assert_eq!(kind, "snap");
        assert!(
            manifest_path.is_some(),
            "a manifest adapter records its path"
        );
        let launch = launch.expect("the launch is snapshotted at registration");
        let bin = ktesio_conformance::fake_agent_bin();
        assert_eq!(launch.exec, bin.to_string_lossy().into_owned());
        // The manifest's [lifecycle.start] args survived — INCLUDING the args the
        // hosted runners dropped on re-read.
        assert_eq!(
            launch.args,
            vec!["--emit-usage", "5", "--linger-ms", "600000"]
        );

        // The fallback re-read WOULD fail now (the manifest is gone): proof that
        // the snapshot — not a re-read — is what makes the start work.
        assert!(
            adapter::resolve_start_launch(&kind, manifest_path.as_deref()).is_err(),
            "the manifest re-read is gone/broken; the snapshot carried the launch"
        );
    }

    #[test]
    fn manifest_start_uses_the_snapshot_launch_even_when_the_manifest_changes_live() {
        // The FIX end-to-end: after registration the launch is FIXED by the
        // snapshot, so mutating the manifest's [lifecycle.start] args no longer
        // affects the started process. Register a fake_agent, REWRITE its manifest
        // with a decoy arg only a re-read would surface, then START — the spawned
        // argv carries the ORIGINAL args and NOT the decoy. Linux-only spawn+observe,
        // matching the sibling `_live` proofs (macOS/Windows CI spawn scaffolding is
        // the very fragility this fix removes; the OS-agnostic seam test above +
        // Epic 1 backend tests cover the rest).
        if OsId::current() != OsId::Linux {
            return;
        }
        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("argv.txt");
        let (_state, manifest, registry) = setup_fake(
            "del",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
        );

        // Rewrite the manifest AFTER registration, appending a decoy arg. The
        // manifest stays valid (no [config]), so the unchanged config-mapping
        // re-read still succeeds; only a LAUNCH re-read would surface the decoy.
        write_fake_manifest(
            manifest.path(),
            "del",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
                "--decoy-from-reread",
            ],
        );

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "del").unwrap();
        assert_eq!(state_of(&registry, "del"), LifecycleState::Running);

        // The spawned fake_agent dumped its argv: the ORIGINAL start args are there,
        // and the post-registration decoy is NOT — the launch came from the
        // registration snapshot, not a re-read of the mutated manifest.
        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == "arg=--linger-ms"),
            "the snapshotted start args must reach argv; dump=\n{dumped}"
        );
        assert!(
            !dumped.lines().any(|l| l == "arg=--decoy-from-reread"),
            "the re-read decoy must NOT appear — the launch came from the snapshot; dump=\n{dumped}"
        );
    }

    #[test]
    fn manifest_start_maps_model_to_the_declared_flag_target_live() {
        // Linux-only (RUNTIME gate, not cfg): the delivery logic proven here —
        // unified config → native env/flag/file mapping — is OS-agnostic engine
        // code, identical on every OS. Only the `_live` spawn+observe scaffolding
        // (spawn the real `fake_agent`, poll its `--dump` file) is fragile on
        // macOS/Windows CI (spawn latency, `.exe` naming). It is covered on Linux
        // here, and Epic 1's process/backend tests already prove the OS-specific
        // spawn works on all three OSes. Tarpaulin runs on Linux, so gating these
        // to Linux leaves coverage unchanged.
        if OsId::current() != OsId::Linux {
            return;
        }
        // AC-A + AC8 (the MANIFEST proof, live). A `fake_agent` manifest declares
        // `[config.model]` → flag `--model`; set model, start the REAL process
        // with `--dump`, and assert the mapped flag landed in the spawned
        // process's argv (observed via the dump file — no stdout race).
        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("argv.txt");
        let (_state, _manifest, registry) = setup_fake_with_config(
            "flg",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "[config.model]\nflag = \"--model\"\n",
        );
        let name = InstanceName::new("flg").unwrap();
        registry.set_config(&name, "model", "gpt-4o").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "flg").unwrap();
        assert_eq!(state_of(&registry, "flg"), LifecycleState::Running);

        // The spawned fake_agent dumped its argv; the mapped flag + value are there.
        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == "arg=--model"),
            "the mapped --model flag must reach the process argv; dump=\n{dumped}"
        );
        assert!(
            dumped.lines().any(|l| l == "arg=gpt-4o"),
            "the mapped model VALUE must reach the process argv; dump=\n{dumped}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "flg", Some(Duration::from_millis(200)));
    }

    #[test]
    fn secret_leaf_delivers_cleartext_to_the_adapter_but_masks_snapshot_and_events() {
        // Linux-only: see manifest_start_maps_model_to_the_declared_flag_target_live
        // — OS-agnostic delivery, fragile _live spawn on macOS/Windows CI.
        if OsId::current() != OsId::Linux {
            return;
        }
        // Story 2-4 (AC-A/AC9 delivery + AC-B no-leak, engine level). A
        // `model = secret:NAME` leaf resolves (env resolver) to a sentinel; the
        // spawned agent's argv carries the CLEARTEXT (usable), while the persisted
        // snapshot AND every transition event carry the MASK, never the sentinel.
        // Uses a UNIQUE env-var name to avoid racing sibling in-process tests.
        const SENTINEL: &str = "s3cr3t-engine-sentinel-abc";
        let env_key = "KTESIO_SUP_SECRET_TEST_KEY";
        let prev = std::env::var_os(env_key);
        std::env::set_var(env_key, SENTINEL);

        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("argv.txt");
        let (_state, _manifest, registry) = setup_fake_with_config(
            "sekeng",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "[config.model]\nflag = \"--model\"\n",
        );
        let name = InstanceName::new("sekeng").unwrap();
        registry
            .set_config(&name, "model", &format!("secret:{env_key}"))
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "sekeng").unwrap();
        assert_eq!(state_of(&registry, "sekeng"), LifecycleState::Running);

        // (POSITIVE) the spawned process argv carries the resolved CLEARTEXT.
        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == format!("arg={SENTINEL}")),
            "the resolved secret cleartext must reach the process argv; dump=\n{dumped}"
        );

        // (NO-LEAK) the persisted snapshot masks the secret.
        let snapshot = std::fs::read_to_string(registry.paths().effective_config_snapshot(&name))
            .expect("snapshot written");
        assert!(
            !snapshot.contains(SENTINEL),
            "the snapshot leaked the secret:\n{snapshot}"
        );
        assert!(
            snapshot.contains("secret:****"),
            "snapshot must mask; {snapshot}"
        );

        // (NO-LEAK) no transition event payload carries the sentinel (AD-14).
        let events = Supervisor::read_events(&registry, "sekeng").unwrap();
        let events_json = serde_json::to_string(&events).unwrap();
        assert!(
            !events_json.contains(SENTINEL),
            "a transition event leaked the secret:\n{events_json}"
        );

        // Teardown + restore env.
        let _ = sup.stop(&registry, "sekeng", Some(Duration::from_millis(200)));
        match prev {
            Some(v) => std::env::set_var(env_key, v),
            None => std::env::remove_var(env_key),
        }
    }

    #[test]
    fn unresolved_secret_rejects_start_before_any_state_change() {
        // Story 2-4 (AC5/AC9, engine level): a `secret:NAME` unresolved by env AND
        // the (absent) secrets file rejects the start with a typed EngineError::Secret
        // that NEVER echoes a value, leaving the instance in its PRIOR state and NO
        // snapshot written. The env var is deliberately unset.
        let env_key = "KTESIO_SUP_DEFINITELY_UNSET_SECRET_KEY_XYZ";
        std::env::remove_var(env_key);
        let (_state, _manifest, registry) = setup_fake("noresolve_eng", &["--linger-ms", "600000"]);
        let name = InstanceName::new("noresolve_eng").unwrap();
        registry
            .set_config(&name, "model", &format!("secret:{env_key}"))
            .unwrap();
        let prior = state_of(&registry, "noresolve_eng");

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "noresolve_eng").unwrap_err();
        match &err {
            EngineError::Secret { name: n, detail } => {
                assert_eq!(n, "noresolve_eng");
                // Names the NAME + resolvers, NEVER a value.
                assert!(detail.contains(env_key), "detail must name NAME; {detail}");
            }
            other => panic!("expected EngineError::Secret, got {other:?}"),
        }
        // Prior state preserved; no snapshot written (rejected before both).
        assert_eq!(state_of(&registry, "noresolve_eng"), prior);
        assert!(
            !registry.paths().effective_config_snapshot(&name).exists(),
            "an unresolved secret must reject before the snapshot write"
        );
    }

    #[test]
    fn manifest_start_maps_model_to_the_declared_file_target_live() {
        // AC-A + AC4 (the MANIFEST FILE proof, live). A `fake_agent` manifest
        // declares `[config.model]` → a file target; set model, start, and assert
        // the engine RENDERED the native config file into the Agent Home at the
        // declared native key (the engine is the sole writer — path authority).
        let (_state, _manifest, registry) = setup_fake_with_config(
            "fil",
            &["--linger-ms", "600000"],
            "[config.model]\nfile = { path = \"config/agent.toml\", key = \"llm.model\" }\n",
        );
        let name = InstanceName::new("fil").unwrap();
        registry.set_config(&name, "model", "claude-opus").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "fil").unwrap();
        assert_eq!(state_of(&registry, "fil"), LifecycleState::Running);

        // The engine rendered the native config file into the Agent Home.
        let rendered = registry.agent_home(&name).join("config/agent.toml");
        assert!(
            rendered.is_file(),
            "the file target must render into the home"
        );
        let parsed: toml::Table = std::fs::read_to_string(&rendered).unwrap().parse().unwrap();
        assert_eq!(
            parsed["llm"]["model"].as_str(),
            Some("claude-opus"),
            "the documented model key must land at the declared native key path"
        );
        // Teardown.
        let _ = sup.stop(&registry, "fil", Some(Duration::from_millis(200)));
    }

    #[test]
    fn manifest_start_delivers_agent_pass_through_verbatim_live() {
        // Linux-only: see manifest_start_maps_model_to_the_declared_flag_target_live
        // — OS-agnostic delivery, fragile _live spawn on macOS/Windows CI.
        if OsId::current() != OsId::Linux {
            return;
        }
        // AC-B (the `agent.*` verbatim proof, live). Set an `agent.*` pass-through
        // key, start the REAL fake_agent with `--dump`, and assert the value was
        // delivered VERBATIM into the native mechanism (an env var named by the
        // verbatim key-tail) — no rewriting, no known-key mapping.
        let dump = tempfile::tempdir().unwrap();
        let dump_path = dump.path().join("env.txt");
        // No [config] mapping at all — pass-through does not need one (AC6).
        let (_state, _manifest, registry) = setup_fake_with_config(
            "pth",
            &[
                "--linger-ms",
                "600000",
                "--dump",
                dump_path.to_str().unwrap(),
            ],
            "",
        );
        let name = InstanceName::new("pth").unwrap();
        registry
            .set_config(&name, "agent.CUSTOM_TOKEN", "verbatim-xyz")
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "pth").unwrap();
        assert_eq!(state_of(&registry, "pth"), LifecycleState::Running);

        let dumped = wait_for_dump(&dump_path);
        assert!(
            dumped.lines().any(|l| l == "env=CUSTOM_TOKEN=verbatim-xyz"),
            "the agent.* value must be delivered verbatim into the native env; dump=\n{dumped}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "pth", Some(Duration::from_millis(200)));
    }

    // ---- Story 2-3: the persisted effective-config snapshot at start (AC5/AC6/AC7) ----

    /// Parse the persisted effective-config snapshot for `name` and return the
    /// entry map (key → (rendered value, source label)). Panics if the file is
    /// missing/unparseable (the test wants it present).
    fn read_snapshot_entries(
        registry: &Registry,
        name: &str,
    ) -> std::collections::BTreeMap<String, (String, String)> {
        let path = registry
            .paths()
            .effective_config_snapshot(&InstanceName::new(name).unwrap());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["key"].as_str().unwrap().to_string(),
                    (
                        e["value"].as_str().unwrap().to_string(),
                        e["source"].as_str().unwrap().to_string(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn start_writes_the_effective_config_snapshot_tagged_with_source() {
        // AC5 (AD-9/AD-6): starting an instance writes the effective-config
        // snapshot FILE into the Agent Home at EnginePaths::effective_config_snapshot,
        // and it parses + carries model=<v> tagged `instance`. Register a live
        // fake_agent, set model, start it, assert the snapshot.
        let (_state, _manifest, registry) = setup_fake("snp", &["--linger-ms", "600000"]);
        let name = InstanceName::new("snp").unwrap();
        registry.set_config(&name, "model", "gpt-4o").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "snp").unwrap();
        assert_eq!(state_of(&registry, "snp"), LifecycleState::Running);

        let path = registry.paths().effective_config_snapshot(&name);
        assert!(path.is_file(), "the snapshot must exist at {path:?}");
        let entries = read_snapshot_entries(&registry, "snp");
        assert_eq!(
            entries.get("model"),
            Some(&("gpt-4o".to_string(), "instance".to_string())),
            "model must be present tagged `instance`; entries={entries:?}"
        );
        // Teardown.
        let _ = sup.stop(&registry, "snp", Some(Duration::from_millis(200)));
    }

    #[test]
    fn restart_via_start_inner_overwrites_the_snapshot_with_the_new_value() {
        // AC7: the snapshot is OVERWRITTEN on every start — a re-start (which flows
        // through the SAME start_inner seam, story 1-6) refreshes it with the newly
        // resolved value, never a stale earlier resolution. Start, stop, change the
        // value, start again; the snapshot reflects the LATEST value.
        let (_state, _manifest, registry) = setup_fake("rsn", &["--linger-ms", "600000"]);
        let name = InstanceName::new("rsn").unwrap();
        registry.set_config(&name, "model", "first").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "rsn").unwrap();
        assert_eq!(
            read_snapshot_entries(&registry, "rsn").get("model"),
            Some(&("first".to_string(), "instance".to_string()))
        );
        sup.stop(&registry, "rsn", Some(Duration::from_millis(200)))
            .unwrap();

        // Change the value and start again (stopped → starting → running via
        // start_inner). The snapshot must be overwritten with the new value.
        registry.set_config(&name, "model", "second").unwrap();
        sup.start(&registry, "rsn").unwrap();
        assert_eq!(state_of(&registry, "rsn"), LifecycleState::Running);
        assert_eq!(
            read_snapshot_entries(&registry, "rsn").get("model"),
            Some(&("second".to_string(), "instance".to_string())),
            "the snapshot must reflect the latest resolved value after re-start (AC7)"
        );
        // Teardown.
        let _ = sup.stop(&registry, "rsn", Some(Duration::from_millis(200)));
    }

    #[test]
    fn snapshot_write_failure_rejects_the_start_before_the_starting_transition() {
        // AC6: a snapshot-write failure rejects the start with NO state change (the
        // write lands before the `starting` transition). Force the write to fail by
        // making the snapshot path a DIRECTORY, then assert start errors and the
        // instance stays in its prior state (`registered`), with NO agent spawned.
        let (_state, _manifest, registry) = setup_fake("bad", &["--linger-ms", "600000"]);
        let name = InstanceName::new("bad").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();
        // A directory where the snapshot file must be → std::fs::write fails.
        let snap_path = registry.paths().effective_config_snapshot(&name);
        std::fs::create_dir(&snap_path).unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "bad").unwrap_err();
        assert!(
            matches!(&err, EngineError::Snapshot { name, .. } if name == "bad"),
            "expected a typed Snapshot error, got {err:?}"
        );
        // The instance stayed in its prior state — the start was rejected cleanly
        // BEFORE the `starting` transition (no spurious state change, AC6).
        assert_eq!(state_of(&registry, "bad"), LifecycleState::Registered);
    }

    #[test]
    fn snapshot_to_engine_maps_snapshot_write_and_falls_back_for_others() {
        // Unit-cover the snapshot error mapper: a SnapshotWrite maps to the
        // dedicated EngineError::Snapshot naming the instance + path; any other
        // registry error falls back to the shared registry mapper (NotFound here).
        let mapped = snapshot_to_engine(crate::domain::RegistryError::SnapshotWrite {
            name: "demo".into(),
            path: "/x/agents/demo/effective-config.json".into(),
            detail: "disk full".into(),
        });
        match mapped {
            EngineError::Snapshot { name, path, detail } => {
                assert_eq!(name, "demo");
                assert!(path.ends_with("effective-config.json"), "path={path}");
                assert_eq!(detail, "disk full");
            }
            other => panic!("expected Snapshot, got {other:?}"),
        }
        // Fallback: a non-snapshot registry error goes through registry_to_engine.
        let fallback = snapshot_to_engine(crate::domain::RegistryError::NotFound {
            name: "demo".into(),
        });
        assert!(matches!(fallback, EngineError::NotFound { name } if name == "demo"));
    }

    #[test]
    fn crash_of_a_never_policy_instance_lands_failed_no_restart() {
        // AC-A / AC5: a `never`-policy instance that crashes lands `failed` with a
        // `crashed` cause + NO restart. Set policy=never, start a crash-after
        // agent, poll until the crash is detected, assert failed + no plan.
        let (_state, _manifest, registry) = setup_fake("nevr", &["--crash-after-ms", "450"]);
        registry
            .set_restart_policy(&InstanceName::new("nevr").unwrap(), RestartPolicy::Never)
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "nevr").unwrap();
        assert_eq!(state_of(&registry, "nevr"), LifecycleState::Running);

        let plans = wait_for_crash(&mut sup, &registry);
        assert!(plans.is_empty(), "never policy must NOT schedule a restart");
        assert_eq!(state_of(&registry, "nevr"), LifecycleState::Failed);

        // The crash was recorded with a `crashed` cause.
        let events = Supervisor::read_events(&registry, "nevr").unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.new_state, LifecycleState::Failed);
        let cause = serde_json::to_string(&last.cause).unwrap();
        assert!(cause.contains("crashed"), "cause={cause}");
    }

    #[test]
    fn on_failure_crash_restarts_increments_count_then_a_clean_run_resets() {
        // AC-A / AC4: an `on-failure` instance that crashes is restarted, the
        // restart count increments, the `failed→starting`… restart event records
        // the backoff, and a subsequent CLEAN start resets the count to 0. Uses
        // the injected fast backoff so no real seconds elapse.
        let (_state, _manifest, registry) = setup_fake("recov", &["--crash-after-ms", "450"]);
        // Default policy is on-failure; be explicit.
        registry
            .set_restart_policy(
                &InstanceName::new("recov").unwrap(),
                RestartPolicy::OnFailure,
            )
            .unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "recov").unwrap();

        // Detect the crash → a restart plan for attempt 1.
        let plans = wait_for_crash(&mut sup, &registry);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].attempt, 1);
        assert_eq!(state_of(&registry, "recov"), LifecycleState::Failed);

        // Perform the restart after its (fast) backoff. The instance is running
        // again and the record shows restart_count == 1.
        std::thread::sleep(plans[0].delay);
        sup.restart(&registry, "recov", plans[0].attempt, plans[0].delay)
            .unwrap();
        assert_eq!(state_of(&registry, "recov"), LifecycleState::Running);
        let rec = registry
            .spawn_record(&InstanceName::new("recov").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_count, 1);

        // The restart event recorded the count + waited backoff.
        let events = Supervisor::read_events(&registry, "recov").unwrap();
        let restart_evt = events
            .iter()
            .find(|e| matches!(e.cause, TransitionCause::Restarted { .. }))
            .expect("a restart event must be recorded");
        match &restart_evt.cause {
            TransitionCause::Restarted { count, .. } => assert_eq!(*count, 1),
            _ => unreachable!(),
        }

        // A CLEAN stop then start resets the consecutive count to 0 (AC4).
        sup.stop(&registry, "recov", Some(Duration::from_millis(200)))
            .unwrap();
        sup.start(&registry, "recov").unwrap();
        let rec = registry
            .spawn_record(&InstanceName::new("recov").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_count, 0, "a fresh start resets the count");
        // Teardown.
        let _ = sup.stop(&registry, "recov", Some(Duration::from_millis(200)));
    }

    #[test]
    fn crash_loop_stops_after_exactly_five_consecutive_failures() {
        // AC4: an instance that crashes immediately every restart stops after
        // EXACTLY 5 consecutive failures, left `failed` with the crash-loop
        // reason. Drive the crash→restart cycle manually with the fast backoff;
        // the 5th restart's crash yields NO further plan (crash-loop), and the
        // recorded cause states the crash loop.
        let (_state, _manifest, registry) = setup_fake("loopy", &["--crash-after-ms", "400"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "loopy").unwrap();

        let mut last_attempt = 0;
        // Up to 5 restarts, each preceded by a detected crash.
        for _ in 0..6 {
            let plans = wait_for_crash(&mut sup, &registry);
            assert_eq!(state_of(&registry, "loopy"), LifecycleState::Failed);
            if plans.is_empty() {
                // Crash-loop reached: no more restarts scheduled.
                break;
            }
            assert_eq!(plans.len(), 1);
            last_attempt = plans[0].attempt;
            std::thread::sleep(plans[0].delay);
            // The restart re-launches; it will crash again on the next poll.
            sup.restart(&registry, "loopy", plans[0].attempt, plans[0].delay)
                .unwrap();
        }

        // The last scheduled attempt was the 4th → the 5th crash trips the loop
        // (is_crash_loop(5) == true), so no 5th restart plan is issued.
        assert_eq!(
            last_attempt, 4,
            "the last restart plan should be attempt 4 (the 5th crash trips the loop)"
        );
        assert_eq!(state_of(&registry, "loopy"), LifecycleState::Failed);

        // F-Low-2: the crash-loop is a TERMINAL path, so the record's LIVE
        // fingerprint is dropped (settled to a pid-0 seed) — a later open will
        // NOT adopt-attempt the dead PID (the reconcile skips pid-0). The policy
        // is re-seeded so `show` can still report it (AC9).
        let rec = registry
            .spawn_record(&InstanceName::new("loopy").unwrap())
            .unwrap()
            .expect("a policy seed is retained after the terminal crash-loop");
        assert_eq!(
            rec.fingerprint.pid, 0,
            "the terminal path must drop the live fingerprint (pid-0 seed, not adopt-attempted)"
        );
        assert_eq!(
            rec.restart_policy,
            RestartPolicy::OnFailure,
            "the policy is retained for AC9's show"
        );
        // The crash-loop REASON now rides in the last `crashed` event's cause (so
        // `instance_status` surfaces it via its event-log fallback).
        let events = Supervisor::read_events(&registry, "loopy").unwrap();
        let last = events.last().unwrap();
        assert_eq!(last.new_state, LifecycleState::Failed);
        let cause = serde_json::to_string(&last.cause).unwrap();
        assert!(
            cause.contains("crash-loop") && cause.contains("5 consecutive failures"),
            "the crash-loop reason must be in the event cause; cause={cause}"
        );
    }

    #[test]
    fn poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash() {
        // The reaper's "not a crash" branch: if the store shows the instance
        // `stopping` (an operator stop in flight) when its process exits,
        // poll_once must NOT apply a `failed` crash transition — it just drops the
        // dead handle. Start an instance, mark it `stopping` in the store, let it
        // crash, and assert poll_once returns no plans and does NOT move it to
        // `failed` (it stays `stopping`, the requested end state).
        let (_state, _manifest, registry) = setup_fake("stopping", &["--crash-after-ms", "450"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "stopping").unwrap();
        // Simulate an operator stop in flight (row → stopping) while the handle
        // is still held.
        registry
            .set_state(
                &InstanceName::new("stopping").unwrap(),
                LifecycleState::Stopping,
            )
            .unwrap();
        // Wait for the process to actually exit, then poll.
        std::thread::sleep(Duration::from_millis(700));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let plans = sup.poll_once(&registry);
            assert!(
                plans.is_empty(),
                "an exit during a requested stop is not a crash"
            );
            if sup.running.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "handle should be dropped after exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The state is NOT `failed` (no crash transition was applied); it stays
        // `stopping` (the requested end state the operator stop will finalize).
        assert_eq!(state_of(&registry, "stopping"), LifecycleState::Stopping);
    }

    // ---- Fix pass (review of #80 follow-up — the CRITICAL finding): the
    // bound-post-SIGKILL-wait fix's retry/no-compounding/self-healing logic.
    //
    // These are WHITE-BOX tests: rather than needing a genuinely OS-unkillable
    // process (which requires disk-exhaustion-induced uninterruptible I/O
    // wait — reproduced separately via a dedicated, SAFE ramdisk experiment;
    // see the story file's Dev Agent Record for the full empirical proof),
    // they directly construct the EXACT bookkeeping state a real
    // `BackendError::StopUnconfirmed` would have left behind
    // (`Supervised::stop_unconfirmed = true`, store state `stopping`, handle
    // retained) and prove `stop_inner`'s/`poll_once`'s reconciliation logic
    // against it. This is deterministic and fast (no real 5s wait), mirroring
    // `poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash`
    // immediately above's own technique of forcing `stopping` via
    // `registry.set_state` directly.

    #[test]
    fn stop_on_a_stopping_instance_without_the_unconfirmed_flag_takes_the_ordinary_path() {
        // The negative-space complement of the retry tests below: an
        // instance that is `stopping` with a held handle but is NOT marked
        // `stop_unconfirmed` (the flag ONLY a real `BackendError::
        // StopUnconfirmed` sets) must NOT take the new cheap-poll retry
        // branch — it falls through to the ORIGINAL, unchanged
        // `next_state` gate, which rejects with the uniform
        // `InvalidTransition` exactly as it did before this fix pass. This
        // proves the new branch is gated precisely on `stop_unconfirmed`,
        // not merely "state is stopping" (which
        // `poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash`
        // above ALSO forces, for a different, pre-existing reason).
        let (_state, _manifest, registry) = setup_fake("notflagged", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "notflagged").unwrap();
        let name = InstanceName::new("notflagged").unwrap();

        registry.set_state(&name, LifecycleState::Stopping).unwrap();
        assert!(
            !sup.running.get(&name).unwrap().stop_unconfirmed,
            "a freshly-started handle must default to NOT stop_unconfirmed"
        );

        let err = sup.stop(&registry, "notflagged", None).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidTransition(_)),
            "without the flag, this must be the ORIGINAL uniform InvalidTransition, not the new \
             StopUnconfirmed retry path: {err:?}"
        );

        // Teardown.
        let supervised = sup.running.get_mut(&name).unwrap();
        let _ = sup
            .backend
            .stop(&mut supervised.handle, Duration::from_secs(2));
    }

    #[test]
    fn stop_retry_on_a_stuck_unconfirmed_instance_polls_cheaply_no_compounding() {
        // A retry `stop()` against an instance whose handle is marked
        // `stop_unconfirmed` (a prior real stop attempt hit
        // KILL_CONFIRM_TIMEOUT) must NOT re-run the whole
        // SIGTERM/graceful-window/SIGKILL/confirm sequence — it polls ONCE,
        // cheaply, and fails fast with the SAME honest error while the
        // process is still genuinely alive (no new signal, no new wait).
        let (_state, _manifest, registry) = setup_fake("stuck", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "stuck").unwrap();
        let name = InstanceName::new("stuck").unwrap();

        // Simulate the aftermath of a real StopUnconfirmed (stop_inner's
        // own bookkeeping on that path — see its docs): the row is
        // `stopping`, the handle is retained, and it is marked unconfirmed.
        registry.set_state(&name, LifecycleState::Stopping).unwrap();
        sup.running.get_mut(&name).unwrap().stop_unconfirmed = true;

        let start = Instant::now();
        let err = sup.stop(&registry, "stuck", None).unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            matches!(&err, EngineError::StopUnconfirmed { name, .. } if name == "stuck"),
            "expected StopUnconfirmed, got {err:?}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "a retry against a still-alive stuck instance must poll cheaply (ProcessBackend::poll, \
             never ProcessBackend::stop), not re-block for a whole new \
             graceful-window/SIGKILL/confirm cycle: {elapsed:?}"
        );
        // No compounding: the row is untouched (still stopping), the handle
        // is still retained (not dropped) for a further retry to reconcile.
        assert_eq!(state_of(&registry, "stuck"), LifecycleState::Stopping);
        assert!(
            sup.running.contains_key(&name),
            "the handle must be retained across a failed retry, never silently dropped"
        );

        // Teardown: really kill the still-running process so it does not
        // leak past this test.
        let supervised = sup.running.get_mut(&name).unwrap();
        let _ = sup
            .backend
            .stop(&mut supervised.handle, Duration::from_secs(2));
    }

    #[test]
    fn stop_retry_self_heals_once_the_stuck_process_actually_exits() {
        // The self-healing counterpart: once the process behind a
        // stop_unconfirmed handle has ACTUALLY died (the OS condition that
        // made confirmation time out has cleared), a retry `stop()` must
        // reconcile the instance to `stopped` — never leaving it permanently
        // stuck just because one earlier attempt could not confirm death in
        // time.
        let (_state, _manifest, registry) = setup_fake("heals", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "heals").unwrap();
        let name = InstanceName::new("heals").unwrap();

        registry.set_state(&name, LifecycleState::Stopping).unwrap();
        sup.running.get_mut(&name).unwrap().stop_unconfirmed = true;

        // Simulate the OS condition clearing: the process ACTUALLY exits now.
        // A real, portable kill via the SAME ProcessBackend::stop the
        // production code already uses (cfg-free at this call site — the
        // OS-specific mechanics live entirely in `backends/`), not a raw
        // OS-specific signal call, so this stays a legitimate domain-layer
        // test. The process is NOT genuinely stuck in this test (only its
        // BOOKKEEPING pretends it was), so this succeeds quickly.
        {
            let supervised = sup.running.get_mut(&name).unwrap();
            sup.backend
                .stop(&mut supervised.handle, Duration::from_secs(2))
                .expect("the process is not genuinely stuck in this test and must die promptly");
        }

        let instance = sup
            .stop(&registry, "heals", None)
            .expect("a retry must self-heal once the process is confirmed dead");
        assert_eq!(
            instance.state,
            LifecycleState::Stopped,
            "the stuck stopping row must reconcile to stopped, not stay stuck forever"
        );
        assert!(
            !sup.running.contains_key(&name),
            "the handle must be released once reconciled"
        );
    }

    #[test]
    fn poll_once_reconciles_a_stuck_unconfirmed_stop_to_stopped_self_healing() {
        // The crash reaper's OWN reconciliation path (`poll_once`) — the
        // OTHER self-healing route besides a manual retry `stop()` (whichever
        // observes the exit first): when a `stop_unconfirmed`-marked handle's
        // process is found `Exited` during a routine reaper poll, poll_once
        // must finalize `stopping -> stopped` itself, rather than silently
        // dropping the handle (which would leave the row PERMANENTLY stuck,
        // since a later retry `stop()` would find no handle to poll).
        // Contrast directly with
        // `poll_once_ignores_an_exit_during_a_requested_stop_not_a_crash`
        // above: that test's contrived `stopping` row is NOT
        // `stop_unconfirmed` (it never went through a real stop attempt), so
        // it correctly keeps the ORIGINAL silent-drop behavior — proving this
        // fix pass changes behavior ONLY for the scenario it targets.
        // --crash-after-ms must comfortably EXCEED READINESS_WINDOW (300ms) or
        // the process looks like an immediate-exit launch failure (AC2)
        // instead of a clean start that later crashes — the same pitfall
        // documented above for `--linger-ms` in this file; 500ms is that
        // established, proven-safe margin.
        let (_state, _manifest, registry) = setup_fake("reaperheals", &["--crash-after-ms", "500"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "reaperheals").unwrap();
        let name = InstanceName::new("reaperheals").unwrap();

        registry.set_state(&name, LifecycleState::Stopping).unwrap();
        sup.running.get_mut(&name).unwrap().stop_unconfirmed = true;

        // Wait for the process to actually exit on its own, then let the
        // reaper observe it.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let plans = sup.poll_once(&registry);
            assert!(
                plans.is_empty(),
                "this is a reconciliation, never a crash/restart"
            );
            if !sup.running.contains_key(&name) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "handle should be reconciled after exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            state_of(&registry, "reaperheals"),
            LifecycleState::Stopped,
            "the reaper must finalize a stuck-unconfirmed stopping row to stopped, not leave it \
             permanently stuck"
        );
    }

    #[test]
    fn poll_once_with_no_handles_is_a_noop() {
        // The empty-reaper path: with nothing supervised, poll_once returns no
        // plans and touches nothing.
        let (_state, _manifest, registry) = setup_fake("idle", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert!(sup.poll_once(&registry).is_empty());
    }

    #[test]
    fn adopt_orphans_with_no_records_adopts_nothing() {
        // The empty-reconcile path: with no persisted spawn records, adoption
        // adopts nothing and returns 0.
        let (_state, _manifest, registry) = setup_fake("idle", &["--linger-ms", "600000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert_eq!(sup.adopt_orphans(&registry), 0);
        assert!(sup.running.is_empty());
    }

    #[test]
    fn adopt_orphans_skips_a_policy_only_seed_record() {
        // A pid-0 record is a policy-only config seed (set before any start), NOT
        // a supervised process — adoption skips it (adopts nothing) and does NOT
        // reconcile the registered instance to failed or clear its policy.
        let (_state, _manifest, registry) = setup_fake("seedonly", &["--linger-ms", "600000"]);
        registry
            .set_restart_policy(
                &InstanceName::new("seedonly").unwrap(),
                RestartPolicy::Never,
            )
            .unwrap();
        let mut sup = Supervisor::with_backoff(fast_backoff());
        assert_eq!(sup.adopt_orphans(&registry), 0);
        // Still registered; the policy seed survives (was not cleared).
        assert_eq!(state_of(&registry, "seedonly"), LifecycleState::Registered);
        let rec = registry
            .spawn_record(&InstanceName::new("seedonly").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(rec.restart_policy, RestartPolicy::Never);
    }

    #[test]
    fn default_stop_window_is_30s() {
        assert_eq!(DEFAULT_STOP_WINDOW, Duration::from_secs(30));
    }

    #[test]
    fn read_events_from_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.log");
        assert!(read_events_from(&path).unwrap().is_empty());
    }

    #[test]
    fn append_then_read_round_trips_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        let e1 = TransitionEvent::new(
            "demo",
            LifecycleState::Registered,
            LifecycleState::Starting,
            TransitionCause::command("start"),
            "2026-07-04T00:00:00Z",
        );
        let e2 = TransitionEvent::new(
            "demo",
            LifecycleState::Starting,
            LifecycleState::Running,
            TransitionCause::AdapterReady,
            "2026-07-04T00:00:01Z",
        );
        append_event(&path, &e1).unwrap();
        append_event(&path, &e2).unwrap();
        let back = read_events_from(&path).unwrap();
        assert_eq!(back, vec![e1, e2]);
    }

    #[test]
    fn read_events_rejects_a_corrupt_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        std::fs::write(&path, "{ not valid json\n").unwrap();
        let err = read_events_from(&path).unwrap_err();
        assert!(err.contains("corrupt instance-log line 1"), "{err}");
    }

    #[test]
    fn supervisor_constructs_empty() {
        let sup = Supervisor::new();
        assert!(sup.running.is_empty());
    }

    #[test]
    fn registry_to_engine_maps_each_variant() {
        use super::super::error::RegistryError as R;
        use super::super::name::NameError;

        // NotFound → NotFound.
        assert!(matches!(
            registry_to_engine(R::NotFound { name: "x".into() }),
            EngineError::NotFound { .. }
        ));
        // InvalidName → InvalidName.
        assert!(matches!(
            registry_to_engine(R::InvalidName {
                name: "X".into(),
                reason: NameError::BadChar,
            }),
            EngineError::InvalidName { .. }
        ));
        // Io → Log (naming the path).
        assert!(matches!(
            registry_to_engine(R::Io {
                name: "x".into(),
                path: "/p".into(),
                source: std::io::Error::other("boom"),
            }),
            EngineError::Log { .. }
        ));
        // A snapshot-shaped registry error → AdapterUnresolved.
        assert!(matches!(
            registry_to_engine(R::ManifestNotFound { path: "/m".into() }),
            EngineError::AdapterUnresolved { .. }
        ));
    }

    #[test]
    fn launch_to_engine_wraps_as_adapter_unresolved() {
        let name = InstanceName::new("svc").unwrap();
        let err = launch_to_engine(
            &name,
            LaunchResolveError::NativeHasNoLaunch {
                kind: "mock".into(),
            },
        );
        match err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("no launch command"));
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
    }

    #[test]
    fn config_to_engine_and_config_apply_to_engine_wrap_as_adapter_unresolved() {
        // Story 2-2: a config-resolution failure and a config-apply (file-render)
        // failure both surface as an unresolved-adapter launch failure naming the
        // instance + preserving the detail, so `start` rejects cleanly.
        let name = InstanceName::new("svc").unwrap();
        let cfg_err = config_to_engine(
            &name,
            crate::domain::ConfigError::NotFound { name: "svc".into() },
        );
        match cfg_err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("svc"), "detail preserved: {detail}");
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
        let apply_err = config_apply_to_engine(
            &name,
            ConfigApplyError::FileRender {
                key: "config/agent.toml".into(),
                path: "config/agent.toml".into(),
                detail: "disk full".into(),
            },
        );
        match apply_err {
            EngineError::AdapterUnresolved { name, detail } => {
                assert_eq!(name, "svc");
                assert!(detail.contains("config/agent.toml"), "{detail}");
                assert!(detail.contains("disk full"), "{detail}");
            }
            other => panic!("expected AdapterUnresolved, got {other}"),
        }
    }

    #[test]
    fn start_with_an_unwritable_file_target_rejects_before_any_state_change() {
        // Story 2-2 end-to-end error path (accurate atomicity, Fix #5): a manifest
        // `[config.model]` FILE target whose parent path is blocked (a regular file
        // sits where the config directory must be in the Agent Home) fails the
        // config-mapping application at start. Because the mapping is applied BEFORE
        // the `starting` transition, the start REJECTS (AdapterUnresolved) and the
        // instance stays in its PRIOR state (`registered`) — it does NOT land
        // `failed`, and never reaches `running`. Exercises the start_inner
        // config-apply error branch + config_apply_to_engine.
        let (_state, _manifest, registry) = setup_fake_with_config(
            "badfile",
            &["--linger-ms", "600000"],
            "[config.model]\nfile = { path = \"blocked/agent.toml\", key = \"k\" }\n",
        );
        let name = InstanceName::new("badfile").unwrap();
        registry.set_config(&name, "model", "gpt-4").unwrap();
        // Block the file target's parent: put a regular FILE at <home>/blocked so
        // create_dir_all(<home>/blocked) fails when rendering blocked/agent.toml.
        let home = registry.agent_home(&name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("blocked"), b"not a dir").unwrap();

        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.start(&registry, "badfile").unwrap_err();
        assert!(
            matches!(err, EngineError::AdapterUnresolved { .. }),
            "a bad file target must fail the start; got {err}"
        );
        // Never reached running (the failure was before the starting transition, so
        // the instance stays registered).
        assert_eq!(state_of(&registry, "badfile"), LifecycleState::Registered);
    }

    #[test]
    fn read_events_skips_blank_lines() {
        // Blank lines in the log are ignored (only JSON event lines are parsed).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.log");
        let e = TransitionEvent::new(
            "demo",
            LifecycleState::Registered,
            LifecycleState::Starting,
            TransitionCause::command("start"),
            "2026-07-04T00:00:00Z",
        );
        let line = serde_json::to_string(&e).unwrap();
        std::fs::write(&path, format!("\n{line}\n\n")).unwrap();
        let back = read_events_from(&path).unwrap();
        assert_eq!(back, vec![e]);
    }

    // ---- Story 3-2: the breach record write error is surfaced, not swallowed ----

    #[test]
    fn record_breach_surfaces_a_write_failure_instead_of_swallowing_it() {
        // FR-21 ("the breach is always recorded"): if the durable breach-log write
        // fails (disk full / IO / perms), the error must be SURFACED (an honest
        // stderr diagnostic), NOT silently discarded while the action still fires —
        // otherwise the mandated record is lost with no trace. We force BOTH the
        // dir-create AND the append to fail by placing a regular FILE where the
        // per-instance log DIRECTORY (`<home>/logs`, the breach log's parent) must
        // be: `create_dir_all(parent)` fails (a file sits at that path) and the
        // subsequent append cannot open `logs/breaches.log` either. `record_breach`
        // must NOT panic (it stays non-fatal) and must not lose data silently — this
        // proves both surfaced-error branches are reachable and the enforcement path
        // survives. Pure unit test: no process, no OS gate.
        let (_state, _manifest, registry) = setup_fake("breachio", &["--linger-ms", "600000"]);
        let name = InstanceName::new("breachio").unwrap();
        // Block the log-dir path with a regular file so create_dir_all(<home>/logs)
        // fails (its target is a file, not a directory) and the append fails too.
        let log_dir = registry.instance_log_dir(&name);
        std::fs::create_dir_all(log_dir.parent().unwrap()).unwrap();
        std::fs::write(&log_dir, b"not a directory").unwrap();
        assert!(
            log_dir.is_file(),
            "the log-dir path must be a FILE to force both the dir-create and append failures"
        );

        let sup = Supervisor::with_backoff(fast_backoff());
        // Must not panic — both the dir-create and the append failures are logged
        // (surfaced) and enforcement continues rather than crashing.
        sup.record_token_breach(
            &registry,
            &name,
            &RunId::mint(),
            BreachScope::Cumulative,
            30,
            60,
            BreachAction::Warn,
            "self-reported",
        );
        // The blocking file is untouched — no breach file was sneaked in, confirming
        // the write genuinely failed and we exercised the surfaced-error branches.
        assert!(log_dir.is_file());
    }

    // ---- Story 3-1 drain planning: H1 terminal-tail + M2 shrink guard ----

    #[test]
    fn plan_drain_midrun_stops_at_the_last_newline() {
        // MID-RUN: a live process may still finish a partial final line, so only bytes
        // up to the last newline are consumed; the trailing partial waits.
        let bytes = b"a\nb\nhalf-written";
        let plan = plan_drain(bytes, 0, DrainMode::MidRun);
        // Consumes "a\nb\n" (4 bytes), leaving "half-written" for the next pass.
        assert_eq!(
            plan,
            DrainPlan::Consume {
                range: 0..4,
                new_cursor: 4
            }
        );
    }

    #[test]
    fn plan_drain_midrun_with_no_newline_yet_consumes_nothing() {
        // A tail with no complete line yet: nothing to consume this pass (MidRun).
        assert_eq!(
            plan_drain(b"no newline yet", 0, DrainMode::MidRun),
            DrainPlan::Nothing
        );
    }

    #[test]
    fn plan_drain_terminal_consumes_a_newline_less_final_line() {
        // H1: on a TERMINAL drain the process is dead, so a final usage line flushed
        // WITHOUT a trailing newline must be consumed to end-of-log (or it is stranded
        // and the next Run's cursor skips past it → a permanent under-count).
        let bytes = b"a\nKTESIO_USAGE {\"sequence\":0,\"input_tokens\":10,\"output_tokens\":20}";
        let plan = plan_drain(bytes, 0, DrainMode::Terminal);
        // The WHOLE tail is consumed (no trailing newline required).
        assert_eq!(
            plan,
            DrainPlan::Consume {
                range: 0..bytes.len(),
                new_cursor: bytes.len() as u64
            }
        );
    }

    #[test]
    fn plan_drain_terminal_from_a_cursor_consumes_only_the_new_tail() {
        // The terminal tail is measured FROM the cursor (already-read bytes are not
        // re-consumed) and still needs no trailing newline.
        let bytes = b"old\nnew-tail-no-nl";
        let plan = plan_drain(bytes, 4, DrainMode::Terminal); // cursor past "old\n"
        assert_eq!(
            plan,
            DrainPlan::Consume {
                range: 4..bytes.len(),
                new_cursor: bytes.len() as u64
            }
        );
    }

    #[test]
    fn plan_drain_shrink_snaps_the_cursor_and_ingests_nothing() {
        // M2: the log is shorter than the cursor (a truncate/rotation). We must NOT
        // re-read from 0 (double-count → inflated bill); instead snap the cursor to
        // the new length and ingest nothing. Holds for BOTH modes.
        let bytes = b"short"; // len 5
        assert_eq!(
            plan_drain(bytes, 100, DrainMode::MidRun),
            DrainPlan::Shrunk { new_cursor: 5 }
        );
        assert_eq!(
            plan_drain(bytes, 100, DrainMode::Terminal),
            DrainPlan::Shrunk { new_cursor: 5 },
            "the shrink guard applies on the terminal path too"
        );
    }

    #[test]
    fn plan_drain_at_end_of_log_consumes_nothing() {
        // Cursor exactly at len (all bytes already read): an empty tail → Nothing, in
        // both modes (no phantom terminal consume of zero bytes).
        let bytes = b"a\nb\n";
        assert_eq!(plan_drain(bytes, 4, DrainMode::MidRun), DrainPlan::Nothing);
        assert_eq!(
            plan_drain(bytes, 4, DrainMode::Terminal),
            DrainPlan::Nothing
        );
    }

    // ---- Story 4-2: read_agent_log_since's follow-cursor planning (AC-D/AC-H) ----

    #[test]
    fn plan_follow_consumes_only_complete_lines_leaving_a_partial_tail() {
        let bytes = b"a\nb\nhalf-written";
        assert_eq!(
            plan_follow(bytes, 0),
            FollowPlan::Consume {
                range: 0..4,
                new_cursor: 4
            }
        );
    }

    #[test]
    fn plan_follow_with_no_newline_yet_consumes_nothing() {
        assert_eq!(
            plan_follow(b"no newline yet", 0),
            FollowPlan::Consume {
                range: 0..0,
                new_cursor: 0
            }
        );
    }

    #[test]
    fn plan_follow_from_a_cursor_consumes_only_the_new_tail() {
        let bytes = b"old\nnew-tail\n";
        assert_eq!(
            plan_follow(bytes, 4),
            FollowPlan::Consume {
                range: 4..bytes.len(),
                new_cursor: bytes.len() as u64
            }
        );
    }

    #[test]
    fn plan_follow_shrink_snaps_the_cursor_and_delivers_nothing() {
        // AC-D/AC-H rotation-notice path: the file is shorter than the
        // cursor (a rotation happened since the last poll). Snap, deliver
        // nothing this pass — the caller detects the snap-back itself.
        let bytes = b"short"; // len 5
        assert_eq!(
            plan_follow(bytes, 100),
            FollowPlan::Shrunk { new_cursor: 5 }
        );
    }

    #[test]
    fn plan_follow_at_end_of_log_consumes_nothing() {
        let bytes = b"a\nb\n";
        assert_eq!(
            plan_follow(bytes, 4),
            FollowPlan::Consume {
                range: 4..4,
                new_cursor: 4
            }
        );
    }

    // ---- Story 4-2: engine-attributed line rendering (Task 4) ----

    #[test]
    fn cause_suffix_and_engine_transition_line_text_cover_every_transition_cause() {
        // A direct, pure-function proof of the RENDERER's own completeness,
        // independent of which causes the current supervisor wiring happens
        // to route a live log_capture through (e.g. a launch-failure never
        // gets a log_capture today — see the Dev Agent Record) — the match
        // itself must stay exhaustive and correct for every variant.
        let cases: Vec<(TransitionCause, &str)> = vec![
            (TransitionCause::command("start"), " (start)"),
            (TransitionCause::AdapterReady, ""),
            (
                TransitionCause::launch_error("boom"),
                " (launch error: boom)",
            ),
            (TransitionCause::StopGraceful, ""),
            (
                TransitionCause::stop_forced("escalated"),
                " (forced: escalated)",
            ),
            (
                TransitionCause::pause_best_effort("windows"),
                " (best-effort: windows)",
            ),
            (
                TransitionCause::resume_best_effort("windows"),
                " (best-effort: windows)",
            ),
            (
                TransitionCause::crashed("exit code 1"),
                " (crashed: exit code 1)",
            ),
            (
                TransitionCause::restarted(2, 500),
                " (restart #2, waited 500ms)",
            ),
            (
                TransitionCause::budget_exceeded(BreachScope::PerRun, 1000, 1200),
                " (breach: per-run tokens)",
            ),
            (
                TransitionCause::cost_cap_exceeded(
                    BreachScope::Cumulative,
                    Micros(5_000_000),
                    Micros(5_250_000),
                    EstimateLabel::Estimated,
                ),
                " (breach: cumulative dollars)",
            ),
        ];
        for (cause, want_suffix) in cases {
            assert_eq!(cause_suffix(&cause), want_suffix, "{cause:?}");
        }

        // engine_transition_line_text wraps the suffix into the full "engine:
        // A -> B(...)" sentence.
        let event = TransitionEvent::new(
            "svc",
            LifecycleState::Running,
            LifecycleState::Paused,
            TransitionCause::pause_best_effort("windows"),
            "2026-07-15T00:00:00Z",
        );
        assert_eq!(
            engine_transition_line_text(&event),
            "engine: running -> paused (best-effort: windows)"
        );
    }

    // ---- Story 4-2: Supervisor::read_agent_log / read_agent_log_since ----

    fn log_line(instance: &str, stream: LogStream, text: &str, at: &str) -> LogLine {
        LogLine::new(instance, stream, text, at)
    }

    #[test]
    fn read_agent_log_on_an_unregistered_name_is_not_found() {
        // The deliberate improvement over read_events/read_breach_events'
        // precedent: an unregistered name is NotFound, not a silent empty.
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        let err = Supervisor::read_agent_log(&registry, "ghost").unwrap_err();
        assert!(matches!(err, EngineError::NotFound { .. }), "{err:?}");
    }

    #[test]
    fn read_agent_log_on_a_registered_but_never_started_instance_is_empty_not_an_error() {
        let (_state, _manifest, registry) = setup_fake("neverstarted", &["--linger-ms", "600000"]);
        let (lines, cursor) = Supervisor::read_agent_log(&registry, "neverstarted").unwrap();
        assert!(lines.is_empty());
        assert_eq!(cursor, 0, "no file yet → the cursor starts at 0");
    }

    #[test]
    fn read_agent_log_concatenates_generations_oldest_to_newest() {
        // AC-A/AC-G: hand-craft the 3 generations directly (deterministic,
        // no real rotation/process needed) and assert the read order is
        // oldest-generation-first, current-generation-last — append order,
        // never a timestamp re-sort.
        let (_state, _manifest, registry) = setup_fake("gens", &["--linger-ms", "600000"]);
        let name = InstanceName::new("gens").unwrap();
        std::fs::create_dir_all(registry.instance_log_dir(&name)).unwrap();

        let write_line = |path: &Path, l: &LogLine| {
            std::fs::write(path, format!("{}\n", serde_json::to_string(l).unwrap())).unwrap();
        };
        write_line(
            &registry.attributed_output_log_generation_path(&name, 2),
            &log_line(
                "gens",
                LogStream::AgentOut,
                "oldest",
                "2026-07-15T00:00:00Z",
            ),
        );
        write_line(
            &registry.attributed_output_log_generation_path(&name, 1),
            &log_line(
                "gens",
                LogStream::AgentErr,
                "middle",
                "2026-07-15T00:00:01Z",
            ),
        );
        let current_path = registry.attributed_output_log_path(&name);
        write_line(
            &current_path,
            &log_line("gens", LogStream::Engine, "newest", "2026-07-15T00:00:02Z"),
        );

        let (lines, cursor) = Supervisor::read_agent_log(&registry, "gens").unwrap();
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["oldest", "middle", "newest"]);
        let streams: Vec<LogStream> = lines.iter().map(|l| l.stream).collect();
        assert_eq!(
            streams,
            vec![LogStream::AgentOut, LogStream::AgentErr, LogStream::Engine]
        );
        // The returned cursor (M1, review of #80) is the CURRENT generation's
        // exact byte length — matching read_agent_log_since's cursor shape —
        // never the concatenated multi-generation total.
        assert_eq!(
            cursor,
            std::fs::metadata(&current_path).unwrap().len(),
            "cursor must be the CURRENT generation's byte length only"
        );
    }

    #[test]
    fn read_agent_log_since_on_an_unregistered_name_is_not_found() {
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        let err = Supervisor::read_agent_log_since(&registry, "ghost", 0).unwrap_err();
        assert!(matches!(err, EngineError::NotFound { .. }), "{err:?}");
    }

    #[test]
    fn read_agent_log_since_happy_path_reads_only_the_new_tail() {
        let (_state, _manifest, registry) = setup_fake("since", &["--linger-ms", "600000"]);
        let name = InstanceName::new("since").unwrap();
        std::fs::create_dir_all(registry.instance_log_dir(&name)).unwrap();
        let path = registry.attributed_output_log_path(&name);

        let l1 = log_line("since", LogStream::AgentOut, "one", "2026-07-15T00:00:00Z");
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&l1).unwrap())).unwrap();
        let (first, cursor1) = Supervisor::read_agent_log_since(&registry, "since", 0).unwrap();
        assert_eq!(first, vec![l1]);
        assert!(cursor1 > 0);

        // No new bytes yet: an empty read at the same cursor.
        let (none, cursor_same) =
            Supervisor::read_agent_log_since(&registry, "since", cursor1).unwrap();
        assert!(none.is_empty());
        assert_eq!(cursor_same, cursor1);

        // Append a second line; read_agent_log_since(cursor1) returns ONLY it.
        let l2 = log_line("since", LogStream::AgentErr, "two", "2026-07-15T00:00:01Z");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write as _;
        writeln!(f, "{}", serde_json::to_string(&l2).unwrap()).unwrap();
        drop(f);
        let (second, cursor2) =
            Supervisor::read_agent_log_since(&registry, "since", cursor1).unwrap();
        assert_eq!(second, vec![l2]);
        assert!(cursor2 > cursor1);
    }

    #[test]
    fn read_agent_log_since_detects_a_rotation_shrink_and_snaps_the_cursor() {
        // AC-D/AC-H: simulate a rotation having happened between two polls by
        // shrinking the current-generation file below the previously
        // returned cursor. The caller (Task 6's CLI) detects this by
        // comparing `next_cursor < cursor` — assert that property holds.
        let (_state, _manifest, registry) = setup_fake("rot", &["--linger-ms", "600000"]);
        let name = InstanceName::new("rot").unwrap();
        std::fs::create_dir_all(registry.instance_log_dir(&name)).unwrap();
        let path = registry.attributed_output_log_path(&name);

        let l1 = log_line(
            "rot",
            LogStream::AgentOut,
            "before-rotation",
            "2026-07-15T00:00:00Z",
        );
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&l1).unwrap())).unwrap();
        let (_lines, cursor) = Supervisor::read_agent_log_since(&registry, "rot", 0).unwrap();
        assert!(cursor > 0);

        // Simulate rotation: the current generation is now a FRESH, SHORTER
        // file (as if it had just been rotated and a new line appended).
        let l2 = log_line(
            "rot",
            LogStream::AgentOut,
            "after-rotation",
            "2026-07-15T00:00:05Z",
        );
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&l2).unwrap())).unwrap();
        let new_len = std::fs::metadata(&path).unwrap().len();
        assert!(new_len < cursor, "the fixture must genuinely shrink");

        let (lines, next_cursor) =
            Supervisor::read_agent_log_since(&registry, "rot", cursor).unwrap();
        assert!(lines.is_empty(), "nothing delivered on the shrink pass");
        assert!(
            next_cursor < cursor,
            "the returned cursor must snap BELOW the one just passed in — the \
             caller's rotation-notice signal"
        );
        assert_eq!(next_cursor, new_len);
    }

    #[test]
    fn read_agent_log_on_an_invalid_name_is_invalid_name() {
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        let err = Supervisor::read_agent_log(&registry, "Not Valid!").unwrap_err();
        assert!(matches!(err, EngineError::InvalidName { .. }), "{err:?}");
    }

    #[test]
    fn read_agent_log_since_on_an_invalid_name_is_invalid_name() {
        let state = tempfile::tempdir().unwrap();
        let registry = Registry::open(Some(state.path().to_path_buf())).unwrap();
        let err = Supervisor::read_agent_log_since(&registry, "Not Valid!", 0).unwrap_err();
        assert!(matches!(err, EngineError::InvalidName { .. }), "{err:?}");
    }

    #[test]
    fn read_agent_log_since_on_a_registered_but_never_started_instance_is_empty() {
        // The current-generation file does not exist yet at all (the
        // instance was registered but never started) — an honest empty
        // read (Vec::new fallback for a NotFound file), never an error.
        let (_state, _manifest, registry) = setup_fake("neverstarted2", &["--linger-ms", "600000"]);
        let (lines, cursor) =
            Supervisor::read_agent_log_since(&registry, "neverstarted2", 0).unwrap();
        assert!(lines.is_empty());
        assert_eq!(cursor, 0);
    }

    // ---- Story 3-4: engine-observed base_url injection + source selection ----

    #[test]
    fn base_url_override_builds_the_reserved_metering_leaf() {
        // AC6: the engine injects the loopback URL at the reserved `metering.base_url`
        // key as an INVOCATION override, so the adapter's config-mapping delivers it.
        let layer = base_url_override("http://127.0.0.1:54321");
        let resolved = crate::domain::resolve([
            crate::domain::ConfigLayer::empty(),
            crate::domain::ConfigLayer::empty(),
            crate::domain::ConfigLayer::empty(),
            layer,
        ]);
        assert_eq!(
            resolved
                .value_display(crate::domain::METERING_BASE_URL_KEY)
                .as_deref(),
            Some("http://127.0.0.1:54321"),
            "the loopback URL lands at the reserved metering.base_url leaf"
        );
    }

    #[test]
    fn self_reported_start_observed_listener_is_a_no_op() {
        // Source selection: a `self-reported` instance's start path is UNCHANGED —
        // start_observed_listener returns Ok(None), NO listener (even with an upstream
        // configured, which a self-reported instance ignores).
        let (_state, _manifest, registry) = setup_fake("selfrep_obs", &["--linger-ms", "1000"]);
        let name = InstanceName::new("selfrep_obs").unwrap();
        registry
            .set_config(&name, "metering.upstream_base_url", "http://127.0.0.1:9")
            .unwrap();
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .unwrap();
        let sup = Supervisor::with_backoff(fast_backoff());
        // self-reported (the fake manifest declares self-reported) → Ok(None).
        let result = sup
            .start_observed_listener(&name, "self-reported", &effective)
            .expect("self-reported is a no-op, not an error");
        assert!(
            result.is_none(),
            "a self-reported instance runs no listener"
        );
    }

    #[test]
    fn engine_observed_without_upstream_rejects_with_a_clear_error() {
        // AC-A: an `engine-observed` instance with NO configured upstream URL rejects
        // start_observed_listener with a traffic-free ObservedMetering error naming the
        // key (nothing to forward to). Uses the handle-less test supervisor, but the
        // upstream check fails FIRST (before the runtime-handle check), so the error
        // names the missing config key.
        let (_state, _manifest, registry) = setup_fake("obs_noup", &["--linger-ms", "1000"]);
        let name = InstanceName::new("obs_noup").unwrap();
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .unwrap();
        let sup = Supervisor::with_backoff(fast_backoff());
        let err = match sup.start_observed_listener(&name, "engine-observed", &effective) {
            Err(e) => e,
            Ok(_) => panic!("an engine-observed instance with no upstream must reject"),
        };
        match err {
            EngineError::ObservedMetering { name: n, detail } => {
                assert_eq!(n, "obs_noup");
                assert!(
                    detail.contains("metering.upstream_base_url"),
                    "detail names the missing key: {detail}"
                );
            }
            other => panic!("expected ObservedMetering, got {other:?}"),
        }
    }

    #[test]
    fn engine_observed_without_runtime_handle_rejects_cleanly() {
        // With an upstream configured but NO engine runtime handle (the handle-less
        // test supervisor), an engine-observed start rejects with a clear, traffic-free
        // error rather than panicking — the async engine is required to observe.
        let (_state, _manifest, registry) = setup_fake("obs_nort", &["--linger-ms", "1000"]);
        let name = InstanceName::new("obs_nort").unwrap();
        registry
            .set_config(&name, "metering.upstream_base_url", "http://127.0.0.1:9")
            .unwrap();
        let effective = registry
            .effective_config(&name, crate::domain::ConfigLayer::empty())
            .unwrap();
        let sup = Supervisor::with_backoff(fast_backoff()); // no runtime handle
        let err = match sup.start_observed_listener(&name, "engine-observed", &effective) {
            Err(e) => e,
            Ok(_) => panic!("no runtime handle must reject an engine-observed start"),
        };
        assert!(
            matches!(err, EngineError::ObservedMetering { .. }),
            "expected ObservedMetering, got {err:?}"
        );
        assert!(
            err.to_string().contains("runtime"),
            "names the cause: {err}"
        );
    }

    // ---- Story 4-1: `send_input` — narrow branches best exercised as
    // Supervisor-level unit tests (no reaper/Engine involved), complementing
    // the AC-level proofs in `crates/ktesio-engine/tests/interaction.rs`. ----

    #[test]
    fn send_input_on_invalid_name_is_rejected() {
        // The name-resolve step: an invalid name is rejected with
        // EngineError::InvalidName, BEFORE any registry lookup.
        let (_state, _manifest, registry) = setup_fake("x", &["--linger-ms", "1000"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        let err = sup.send_input(&registry, "Bad Name", "hi").unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidName { .. }),
            "expected InvalidName, got {err:?}"
        );
    }

    #[test]
    fn send_input_after_the_process_exits_on_its_own_is_a_backend_error() {
        // A genuine BackendError write failure (Testing Notes: "a genuine
        // BackendError write failure if practically triggerable"). The
        // process exits ON ITS OWN (a short --linger-ms) but nothing has yet
        // reaped/transitioned the persisted row (deliberately no
        // `poll_once` call here — that would reap the handle and transition
        // to `failed`, which is exactly the race this test avoids so the
        // write is genuinely attempted). `send_input`'s write to the now
        // read-end-closed pipe fails at the OS level (EPIPE/BrokenPipe on
        // Unix), mapped to `EngineError::Backend` — the SAME generic mapping
        // every other backend op uses — never silently swallowed, never
        // misreported as `InteractionUnavailable`.
        //
        // Unix-only (EPIPE-on-closed-pipe semantics + a portable "is this pid
        // still alive" probe both need a real Unix liveness check); runtime
        // skip on Windows, NO `#[cfg]` (this file is outside the `backends`
        // allowlist) — mirrors the rest of the codebase's data-driven OS skip
        // convention.
        if OsId::current() == OsId::Windows {
            return;
        }
        // --linger-ms must comfortably EXCEED READINESS_WINDOW (300ms) or the
        // process looks like an immediate-exit launch failure (AC2) instead
        // of a clean start reaching `running`.
        let (_state, _manifest, registry) =
            setup_fake("exiter", &["--echo-stdin", "--linger-ms", "500"]);
        let mut sup = Supervisor::with_backoff(fast_backoff());
        sup.start(&registry, "exiter").unwrap();

        // Wait past the KNOWN, self-configured linger deadline (a FIXED timer
        // this test itself set, not a guess at some other operation's
        // duration — the AI-35/38 "never guess" lesson is about polling
        // unknown async completion, which this is not). A liveness-probing
        // poll loop (`kill -0`) cannot substitute here: since nothing in this
        // test reaps the child (deliberately, so the persisted row stays
        // `running`), the process becomes a defunct zombie on exit, and a
        // zombie still answers `kill -0` as "alive" — the probe would never
        // observe the exit and the loop would spin until its own timeout.
        std::thread::sleep(Duration::from_millis(800));

        // The persisted row is STILL `running` (nothing has reaped it yet) —
        // send_input reaches the write, which fails at the OS level.
        let err = sup.send_input(&registry, "exiter", "hello").unwrap_err();
        assert!(
            matches!(err, EngineError::Backend { .. }),
            "expected Backend, got {err:?}"
        );
    }
}
