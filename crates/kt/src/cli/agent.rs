//! `kt agent register | remove | list` — thin CLI over the engine's
//! synchronous registration API (spine AD-2, CLI-first gate).
//!
//! This module holds NO domain logic and constructs NO paths: the engine is
//! the sole path authority (it computes and returns the Agent Home path, which
//! we merely display). Every capability is reachable here (register, remove
//! with an explicit retain/delete disposition, the running-guard via `--force`,
//! and a list to observe results), satisfying the CLI-first gate.
//!
//! Errors: the engine returns `thiserror` [`RegistryError`]; we translate them
//! into `miette` diagnostics with remediation hints (miette lives in `kt`
//! only — conventions). Output discipline (AD-12): command results to stdout,
//! diagnostics/notices to stderr.

use std::time::Duration;

use ktesio_engine::{
    AdapterRef, Capability, ConfigError, ConfigLayer, EffectiveCapabilities, EffectiveConfig,
    Engine, EngineError, FleetEntry, FleetListing, RegistryError, RemoveDisposition, SupportLevel,
    FLEET_SCHEMA_VERSION,
};
use serde::Serialize;

use crate::error::{
    AgentCapabilityUnsupported, AgentConfig, AgentDuplicateName, AgentInvalidName,
    AgentInvalidTransition, AgentIo, AgentLaunchFailed, AgentManifestInvalid,
    AgentManifestNotFound, AgentManifestUnreadable, AgentNoCapabilities, AgentNoMeteringSource,
    AgentNotFound, AgentRunningRequiresForce, AgentStore, AgentUnknownConfigKey, AgentUnknownKind,
};
use crate::ui;

/// Retain/delete choice as parsed from the CLI flags.
///
/// `[ASSUMPTION]` when neither `--delete` nor `--retain` is given we default to
/// **retain** — the safer choice, since it never destroys data silently. The
/// two flags are mutually exclusive at the clap layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispositionArg {
    /// Neither flag given → default to retain.
    Unspecified,
    /// `--retain`.
    Retain,
    /// `--delete`.
    Delete,
}

impl DispositionArg {
    /// Resolve clap booleans into a [`DispositionArg`].
    ///
    /// clap marks `--delete` and `--retain` mutually exclusive
    /// (`conflicts_with`), so both-true cannot happen through the CLI. As
    /// defense-in-depth we still fail **closed** to `Retain` if both are
    /// somehow set — retain is the safe default and must never lose to delete
    /// on an ambiguous input (it would silently destroy data).
    pub fn from_flags(delete: bool, retain: bool) -> Self {
        match (delete, retain) {
            // Both set (should be unreachable via clap): fail closed to Retain.
            (true, true) => DispositionArg::Retain,
            (true, false) => DispositionArg::Delete,
            (false, true) => DispositionArg::Retain,
            (false, false) => DispositionArg::Unspecified,
        }
    }

    /// Map to the engine's [`RemoveDisposition`], defaulting Unspecified to
    /// Retain (the safe default).
    fn resolve(self) -> RemoveDisposition {
        match self {
            DispositionArg::Delete => RemoveDisposition::Delete,
            DispositionArg::Retain | DispositionArg::Unspecified => RemoveDisposition::Retain,
        }
    }
}

/// How the operator selected the adapter on the command line.
///
/// `--kind` and `--manifest` are mutually exclusive at the clap layer, and at
/// least one is required; this enum resolves the parsed flags into the engine's
/// [`AdapterRef`]. `[ASSUMPTION]` on the mutually-exclusive-required shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterArg {
    /// `--kind <kind>` — a native builtin adapter by kind.
    Kind(String),
    /// `--manifest <path>` — a manifest adapter loaded from a dir or file.
    Manifest(String),
}

impl AdapterArg {
    /// Resolve clap's `Option`s into an [`AdapterArg`].
    ///
    /// clap enforces "exactly one of --kind/--manifest"; as defense-in-depth we
    /// prefer `--kind` if both somehow arrive and error if neither does.
    pub fn from_flags(
        kind: Option<String>,
        manifest: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        match (kind, manifest) {
            (Some(k), _) => Ok(AdapterArg::Kind(k)),
            (None, Some(m)) => Ok(AdapterArg::Manifest(m)),
            (None, None) => Err(AgentInvalidName {
                message: "one of --kind <kind> or --manifest <path> is required".to_string(),
            }
            .into()),
        }
    }

    /// Translate into the engine's [`AdapterRef`].
    fn to_ref(&self) -> AdapterRef {
        match self {
            AdapterArg::Kind(k) => AdapterRef::Native(k.clone()),
            AdapterArg::Manifest(p) => AdapterRef::Manifest(std::path::PathBuf::from(p)),
        }
    }
}

/// `kt agent register <name> (--kind <kind> | --manifest <path>)`.
///
/// Opens the engine (default state dir, or `KTESIO_STATE_DIR`), resolves +
/// validates the adapter, registers the instance, and prints the engine-computed
/// Agent Home path plus the effective (current-OS) Capability Declaration to
/// stdout. On an adapter/validation failure, nothing is written and a miette
/// diagnostic naming the problem goes to stderr.
pub fn register(name: &str, adapter: &AdapterArg) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let engine = engine.blocking();
    match engine.register_with_adapter(name, &adapter.to_ref()) {
        Ok(instance) => {
            ui::success(format!(
                "Registered Agent Instance {} ({})",
                ui::skill_name(instance.name.as_str()),
                instance.kind
            ));
            // Command result to stdout: the created Agent Home path.
            println!("{}", instance.agent_home);

            // Surface the effective per-OS Capability Declaration (AC1). Read it
            // back from the just-persisted snapshot so what we print is exactly
            // what `kt agent show` will render.
            match engine.effective_capabilities(instance.name.as_str()) {
                Ok(caps) => render_capabilities(instance.name.as_str(), &caps),
                // A render read-back failure must not fail a successful
                // registration; note it to stderr and move on.
                Err(err) => ui::warning(format!(
                    "Registered, but could not read back the Capability Declaration: {err}"
                )),
            }
            Ok(())
        }
        Err(err) => Err(map_error(err)),
    }
}

/// The `kt agent show <name> --json` document (story 1-7, AD-14).
///
/// A versioned wrapper carrying the SAME [`FLEET_SCHEMA_VERSION`] as the
/// `list --json` [`FleetListing`] (so `kt --json` speaks ONE schema, AD-14) plus
/// the single instance's [`FleetEntry`]. Presentation-only — the engine owns the
/// domain types; this wraps one entry with the shared schema version for the
/// `show` surface.
#[derive(Serialize)]
struct ShowDocument {
    /// The Fleet document schema version ([`FLEET_SCHEMA_VERSION`]).
    schema_version: u32,
    /// The single instance's Fleet entry (runtime fields + honest metering seed).
    instance: FleetEntry,
}

impl ShowDocument {
    /// Wrap one [`FleetEntry`], stamping the current [`FLEET_SCHEMA_VERSION`].
    fn new(instance: FleetEntry) -> Self {
        Self {
            schema_version: FLEET_SCHEMA_VERSION,
            instance,
        }
    }
}

/// Serialize the Fleet entries into the pretty `list --json` document (a
/// versioned [`FleetListing`]). Pure (no engine, no I/O) so it is unit-testable
/// in-process; the CLI just prints the returned string to stdout. A serialize
/// failure (not reachable for these plain serde structs) becomes an [`AgentIo`]
/// diagnostic rather than a panic.
fn fleet_json(entries: Vec<FleetEntry>) -> Result<String, Box<dyn std::error::Error>> {
    let listing = FleetListing::new(entries);
    serde_json::to_string_pretty(&listing).map_err(|e| serialize_error("Fleet", e))
}

/// Serialize one entry into the pretty `show --json` document (a versioned
/// [`ShowDocument`]). Pure, for the same reason as [`fleet_json`].
fn show_json(entry: FleetEntry) -> Result<String, Box<dyn std::error::Error>> {
    let document = ShowDocument::new(entry);
    serde_json::to_string_pretty(&document).map_err(|e| serialize_error("instance", e))
}

/// Wrap a `serde_json` serialization failure into an [`AgentIo`] diagnostic. Not
/// reachable for the plain serde structs `--json` emits (serialization of a
/// derive-only struct cannot fail), so this is defense-in-depth, never a panic.
fn serialize_error(what: &str, err: serde_json::Error) -> Box<dyn std::error::Error> {
    AgentIo {
        message: format!("Failed to serialize the {what}: {err}"),
    }
    .into()
}

/// `kt agent show <name> [--json]` — render an instance's effective Capability
/// Declaration (AC1 "visible for the instance") plus its runtime status (story
/// 1-6, AC9): the current Lifecycle State, the active Restart Policy, the restart
/// count, the honest Budget/cap + Usage metering seed (Epic 3 — `—`/`null`), and
/// — for a `failed` instance — the failed cause.
///
/// `--json` mode (story 1-7) writes a single versioned document to STDOUT and
/// nothing else there: `{ schema_version, instance: <FleetEntry> }` — the SAME
/// [`FleetEntry`] shape `list --json` emits (RUNTIME fields only; the effective
/// Capability Declaration stays a human-`show` concern — decision recorded in the
/// Dev Agent Record). Output discipline (AD-12): result → stdout; the Epic-3
/// metering note + any read-back diagnostic → stderr.
pub fn show(name: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let facade = engine.blocking();

    if json {
        // Reuse the SAME composition as `list --json` and pick the named entry, so
        // the `show` object is byte-identical to that instance's `list` row. A
        // missing name is the uniform not-found diagnostic (to stderr).
        let entry = facade
            .fleet()
            .map_err(map_error)?
            .into_iter()
            .find(|e| e.name.as_str() == name)
            .ok_or_else(|| {
                map_error(RegistryError::NotFound {
                    name: name.to_string(),
                })
            })?;
        let json = show_json(entry)?;
        println!("{json}");
        // The metering-seed note rides on stderr (AD-12), keeping stdout pure JSON.
        ui::note(METERING_EPIC3_NOTE);
        return Ok(());
    }

    let caps = facade.effective_capabilities(name).map_err(map_error)?;
    render_capabilities(name, &caps);
    // Runtime status (story 1-6, AC9): state + policy + restart count + failed
    // cause. A status read-back failure must not fail `show` (the capabilities
    // already printed); note it and continue.
    match facade.instance_status(name) {
        Ok(status) => {
            render_runtime_status(&status);
            // One stderr note (AD-12) that the budget/usage rows are Epic-3 seeds.
            ui::note(METERING_EPIC3_NOTE);
        }
        Err(err) => ui::warning(format!("Could not read runtime status for '{name}': {err}")),
    }
    Ok(())
}

/// Render the per-instance runtime status (story 1-6, AC9) as a small table:
/// State, Restart Policy, Restart count, and the honest Budget/cap + Usage
/// metering seed (story 1-7 — Epic 3, rendered `—`); for a `failed` instance the
/// failed cause is printed below (result → stdout, AD-12). The caller prints the
/// Epic-3 metering note to stderr.
fn render_runtime_status(status: &ktesio_engine::InstanceStatus) {
    let title = format!("Runtime status for {}", status.instance.name.as_str());
    let columns = [
        ui::TableColumn::new("Field", 14, 20),
        ui::TableColumn::new("Value", 14, 48),
    ];
    let rows = vec![
        vec![
            ui::TableCell::plain("State"),
            ui::TableCell::status(status.instance.state.as_str()),
        ],
        vec![
            ui::TableCell::plain("Restart policy"),
            ui::TableCell::plain(status.restart_policy.as_str()),
        ],
        vec![
            ui::TableCell::plain("Restart count"),
            ui::TableCell::plain(status.restart_count.to_string()),
        ],
        // The honest Epic-1 metering seed rows (story 1-7): a single `—`, never a
        // fabricated number. Populated by Epic 3 metering.
        vec![
            ui::TableCell::plain("Budget/cap"),
            ui::TableCell::muted(FleetEntry::METERING_SEED_CELL),
        ],
        vec![
            ui::TableCell::plain("Usage"),
            ui::TableCell::muted(FleetEntry::METERING_SEED_CELL),
        ],
    ];
    ui::print_table(&title, &columns, &rows);
    // For a failed instance, surface the last-known cause (the crash / crash-loop
    // detail) so the operator sees WHY it failed and the active policy (AC9).
    if status.instance.state == ktesio_engine::LifecycleState::Failed {
        if let Some(cause) = &status.failed_cause {
            ui::info(format!("Failed cause: {cause}"));
        }
    }
}

/// Render the effective (current-OS) Capability Declaration as a small table.
///
/// Command output → stdout (AD-12), reusing `ui.rs`. Each row is a capability
/// and its support level on the current OS.
fn render_capabilities(name: &str, caps: &EffectiveCapabilities) {
    let title = format!("Capabilities for {name} (OS: {})", caps.os);
    if caps.is_empty() {
        ui::info(format!("{title}: none declared"));
        return;
    }
    let columns = [
        ui::TableColumn::new("Capability", 12, 24),
        ui::TableColumn::new("Support (current OS)", 14, 16),
    ];
    let rows: Vec<Vec<ui::TableCell>> = caps
        .entries
        .iter()
        .map(|(capability, level)| {
            vec![
                ui::TableCell::skill(capability.as_str()),
                ui::TableCell::status(level.as_str()),
            ]
        })
        .collect();
    ui::print_table(&title, &columns, &rows);
}

/// `kt agent remove <name> [--delete|--retain] [--force]`.
pub fn remove(
    name: &str,
    disposition: DispositionArg,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // `--force` is only meaningful for a running instance. If the caller did
    // not choose a disposition we default to retain (safe — never destroys data
    // silently); see DispositionArg docs.
    let engine = open_engine()?;
    match engine.blocking().remove(name, disposition.resolve(), force) {
        Ok(()) => {
            let verb = match disposition.resolve() {
                RemoveDisposition::Delete => "removed (Agent Home deleted)",
                RemoveDisposition::Retain => "removed (Agent Home retained)",
            };
            ui::success(format!("Agent Instance {} {}", ui::skill_name(name), verb));
            Ok(())
        }
        Err(err) => Err(map_error(err)),
    }
}

/// The one-line stderr NOTE (AD-12: notices → stderr) that the budget/cap +
/// Usage Ledger columns are HONEST Epic-1 seeds — metering arrives in Epic 3.
/// Shared by `list` and `show` so both surfaces state it identically.
const METERING_EPIC3_NOTE: &str =
    "budget/cap status and Usage Ledger totals arrive with metering in Epic 3; \
     they show as '—' (JSON null) until then.";

/// `kt agent list [--json]` — render the Fleet (FR-4).
///
/// Human mode prints a table: Name, Kind, State, Restarts (story 1-6), the honest
/// Budget/cap + Usage metering-seed columns (Epic 3 — rendered `—`), and the
/// Agent Home; one stderr note explains the metering seed (AD-12: result →
/// stdout, note → stderr). `--json` mode writes a single versioned
/// [`FleetListing`] document to STDOUT and nothing else there (AD-14: `kt --json`
/// serializes the same struct the Host event stream will publish). Freshness
/// (≤2s, AC6) is structural: each invocation opens the engine and reads live
/// persisted state via [`ktesio_engine::Engine::fleet`] — there is no cache, so
/// any committed transition is reflected on the next listing (a single DB read).
pub fn list(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let facade = engine.blocking();
    let entries = facade.fleet().map_err(map_error)?;

    if json {
        // AC5/AC9: the whole result is ONE JSON document to stdout (an empty Fleet
        // is a valid empty `instances` array). Any guidance/notes go to stderr so
        // stdout is always parseable JSON.
        let empty = entries.is_empty();
        let document = fleet_json(entries)?;
        println!("{document}");
        if empty {
            ui::note("No Agent Instances registered yet. Register one with: kt agent register <name> --kind <kind>");
        }
        // The metering-seed note still rides on stderr (AD-12).
        ui::note(METERING_EPIC3_NOTE);
        return Ok(());
    }

    if entries.is_empty() {
        ui::info("No Agent Instances registered yet. Register one with: kt agent register <name> --kind <kind>");
        return Ok(());
    }

    let columns = [
        ui::TableColumn::new("Name", 12, 32),
        ui::TableColumn::new("Kind", 8, 24),
        ui::TableColumn::new("State", 10, 12),
        ui::TableColumn::new("Restarts", 8, 10),
        ui::TableColumn::new("Budget/cap", 10, 12),
        ui::TableColumn::new("Usage", 8, 12),
        ui::TableColumn::new("Agent Home", 20, 64),
    ];
    let rows: Vec<Vec<ui::TableCell>> = entries
        .iter()
        .map(|entry| {
            vec![
                ui::TableCell::skill(entry.name.as_str()),
                ui::TableCell::plain(entry.kind.clone()),
                ui::TableCell::status(entry.state.as_str()),
                ui::TableCell::plain(entry.restart_count.to_string()),
                // The honest Epic-1 metering seed: a single `—`, never a number.
                ui::TableCell::muted(FleetEntry::METERING_SEED_CELL),
                ui::TableCell::muted(FleetEntry::METERING_SEED_CELL),
                ui::TableCell::muted(entry.agent_home.clone()),
            ]
        })
        .collect();
    ui::print_table("Fleet", &columns, &rows);
    // One stderr note (AD-12) that budget/usage are Epic-3 seeds, not fabricated.
    ui::note(METERING_EPIC3_NOTE);
    Ok(())
}

/// `kt agent start <name>` — start a registered Agent Instance (AC1/AC2).
///
/// Opens the engine, drives `start` through the blocking facade, and prints the
/// new Lifecycle State (`running`) to stdout on success. On a launch failure the
/// instance lands in `failed`, and a miette diagnostic preserving the adapter's
/// diagnostic goes to stderr (AC2). Output discipline (AD-12): result → stdout,
/// diagnostics/notices → stderr.
///
/// SINGLE-LIFETIME SUPERVISION BOUNDARY (honest notice, AD-5): the engine
/// supervises the started process only for the lifetime of THIS engine session.
/// Because the backend kills the process group / job on handle drop, a
/// standalone `kt agent start <name>` stops the agent when this CLI process
/// exits cleanly — the persisted `running` row then outlives the live process.
/// Story 1-6 delivers CRASH recovery: if the engine CRASHES (no clean drop), a
/// surviving process is re-adopted on the next `Engine::open` (by pid +
/// start-time fingerprint) and crashes are detected + handled by the Restart
/// Policy. It does NOT make a cleanly-exited standalone `kt agent start` leave a
/// durably-supervised process across separate CLI invocations — that remains
/// future work. To keep the operator honest at the point of pain, the success
/// path prints a one-line notice to STDERR (never stdout — existing tests assert
/// the stdout result line).
pub fn start(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    match engine.blocking().start(name) {
        Ok(instance) => {
            ui::success(format!(
                "Started Agent Instance {}",
                ui::skill_name(instance.name.as_str())
            ));
            // Command result to stdout: the new Lifecycle State.
            println!("{}", instance.state);
            // Honest single-lifetime notice to STDERR (AD-12: notices → stderr,
            // never stdout). A clean CLI exit stops the agent; durable supervision
            // across separate CLI invocations is future work.
            ui::note(
                "the started process is supervised only for this engine session \
                 and stops when this command exits; durable supervision across \
                 separate CLI invocations is future work.",
            );
            Ok(())
        }
        Err(err) => Err(map_engine_error(err)),
    }
}

/// `kt agent stop <name> [--timeout <secs>]` — stop a running Agent Instance
/// (AC3/AC4).
///
/// Opens the engine and drives `stop` through the blocking facade with the
/// graceful window (default 30s, or `--timeout <secs>`). Prints the final state
/// (`stopped`) to stdout, or the uniform invalid-transition diagnostic (e.g.
/// stop on `stopped`, AC4) to stderr.
pub fn stop(name: &str, timeout_secs: Option<u64>) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let window = timeout_secs.map(Duration::from_secs);
    match engine.blocking().stop(name, window) {
        Ok(instance) => {
            ui::success(format!(
                "Stopped Agent Instance {}",
                ui::skill_name(instance.name.as_str())
            ));
            // Command result to stdout: the final Lifecycle State.
            println!("{}", instance.state);
            Ok(())
        }
        Err(err) => Err(map_engine_error(err)),
    }
}

/// `kt agent pause <name>` — pause a running Agent Instance with honest, per-OS
/// semantics (AC2/AC3/AC6 — "surfaced not silent").
///
/// Drives `pause` through the blocking facade. On success prints the new state
/// (`paused`) to stdout; then — per the AC2 honesty contract — re-reads the
/// effective Capability Declaration and, if pause is `BestEffort` on this OS,
/// emits a VISIBLE qualifier NOTE to STDERR (never a silent success; the
/// machine-readable half rides in the transition event's `pause-best-effort`
/// cause). On `unsupported` the engine fails fast and we render
/// [`AgentCapabilityUnsupported`] quoting the declaration to stderr with a
/// non-zero exit (AC3). Output discipline (AD-12): result → stdout, the
/// qualifier note/diagnostics → stderr.
pub fn pause(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let facade = engine.blocking();
    match facade.pause(name) {
        Ok(instance) => {
            ui::success(format!(
                "Paused Agent Instance {}",
                ui::skill_name(instance.name.as_str())
            ));
            // Command result to stdout: the new Lifecycle State.
            println!("{}", instance.state);
            // AC2 honesty: if pause is best-effort on this OS, surface the
            // qualifier to STDERR (the human half of "surfaced not silent").
            note_if_best_effort(&facade, name, "pause");
            Ok(())
        }
        Err(err) => Err(map_engine_error(err)),
    }
}

/// `kt agent resume <name>` — resume a paused Agent Instance (AC2/AC6).
///
/// The symmetric counterpart of [`pause`]: prints the new state (`running`) to
/// stdout, and if pause is best-effort on this OS emits the resume qualifier note
/// to stderr.
pub fn resume(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let facade = engine.blocking();
    match facade.resume(name) {
        Ok(instance) => {
            ui::success(format!(
                "Resumed Agent Instance {}",
                ui::skill_name(instance.name.as_str())
            ));
            // Command result to stdout: the new Lifecycle State.
            println!("{}", instance.state);
            note_if_best_effort(&facade, name, "resume");
            Ok(())
        }
        Err(err) => Err(map_engine_error(err)),
    }
}

/// After a successful best-effort-eligible pause/resume, re-read the effective
/// Capability Declaration and, if pause is [`SupportLevel::BestEffort`] on the
/// current OS, print a one-line qualifier NOTE to STDERR (AD-12: notices →
/// stderr, never stdout). This is the RECOMMENDED best-effort detection (a cheap
/// extra read via the same `effective_capabilities` mechanism `kt agent show`
/// uses — no `Engine::pause` signature change). A read-back failure is swallowed:
/// it must never turn a successful pause into a CLI error (the state already
/// changed and the machine-readable qualifier is already in the event log).
fn note_if_best_effort(facade: &ktesio_engine::Blocking<'_>, name: &str, op: &str) {
    let Ok(caps) = facade.effective_capabilities(name) else {
        return;
    };
    let pause_level = caps
        .entries
        .iter()
        .find(|(c, _)| *c == Capability::Pause)
        .map(|(_, level)| *level);
    if pause_level == Some(SupportLevel::BestEffort) {
        ui::note(format!(
            "{op} for '{name}' is best-effort on {os} (adapter-cooperative); \
             the process may keep running.",
            os = caps.os,
        ));
    }
}

/// `kt agent config set <name> <key> <value>` — write one key to the Agent
/// Instance config layer (story 2-1, AC-B/AC10, AD-12).
///
/// Validated at WRITE time by the engine: a known unified key or an `agent.*`
/// pass-through key is accepted and persisted to the instance `config.toml`
/// through path authority; an unknown key OUTSIDE `agent.*` is REJECTED before
/// anything is written (the on-disk config is byte-unchanged) and a miette
/// diagnostic naming the offending key + the nearest valid key goes to STDERR
/// with a non-zero exit. On success `ui::success` confirms to stdout. The value
/// is stored verbatim — a `secret:NAME` REFERENCE is what is persisted here
/// (story 2-4 resolves + masks it at start/read, FR-14; this write neither
/// resolves nor echoes a secret).
pub fn config_set(name: &str, key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    match engine.blocking().set_config(name, key, value) {
        Ok(()) => {
            ui::success(format!(
                "Set {} = {} on Agent Instance {} (instance layer)",
                key,
                value,
                ui::skill_name(name)
            ));
            Ok(())
        }
        Err(err) => Err(map_config_error(err)),
    }
}

/// `kt agent config get <name> [<key>] [--json]` — read the EFFECTIVE (resolved)
/// config WITH per-value provenance (story 2-1 read + story 2-3 provenance,
/// AC10/AC-A/AC3/AC4, AD-12/AD-9).
///
/// With `<key>`, prints that key's effective VALUE to stdout (a not-set key is a
/// diagnostic on stderr + non-zero exit); `--json` emits that one leaf as
/// `{ key, value, source, unvalidated }`. Without a key, prints the whole
/// effective config as a Key/Value/Validated/**Source** table to stdout, or (with
/// `--json`) a single versioned document to stdout and nothing else there. Each
/// value NAMES its source layer (`engine-default` / `kind-default` / `instance` /
/// `invocation-override`), read from the [`ktesio_engine::SourceLayer`] tag the
/// engine records per leaf (AD-2: `kt` never re-derives it). Deep-resolved via the
/// engine (engine defaults < kind defaults < instance < invocation overrides); a
/// key set at the instance layer overrides the same key at a lower layer, every
/// time (FR-11). No invocation overrides are supplied here (a plain read).
///
/// Output discipline (AD-12): result → stdout; `--json` is pure JSON on stdout,
/// with any note on stderr. The 2-1/2-2 "provenance arrives in Epic 2.3" stderr
/// note is RETIRED here (Decision 3) — the "Source" column now IS the provenance,
/// so a residual deferral note would be false.
///
/// SECRETS (story 2-4, AC-C/AC11): `secret:NAME` values are MASKED by default (the
/// engine's [`ResolvedValue::display`] masks them — `kt` renders whatever the
/// engine hands it, AD-2). `--reveal` (`reveal == true`) is the SOLE un-mask: it
/// asks the engine to re-resolve the secret leaves LIVE and overlays their
/// cleartext into BOTH the human table and `--json` (Assumption 11 — symmetric). A
/// reveal resolution failure is a stderr diagnostic (mapped from
/// [`ConfigError::SecretReveal`]), never a crash; `--reveal` NEVER touches the
/// snapshot/logs/events.
pub fn config_get(
    name: &str,
    key: Option<&str>,
    json: bool,
    reveal: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let engine = open_engine()?;
    let blocking = engine.blocking();
    let effective = blocking
        .effective_config(name, ConfigLayer::empty())
        .map_err(map_config_error)?;

    // With --reveal, ask the ENGINE for the resolved cleartext of the secret leaves
    // (kt never resolves secrets itself, AD-2). A live-resolution failure surfaces
    // as a stderr diagnostic (never a crash). Without --reveal, an empty overlay
    // leaves every secret masked via the engine's display().
    let revealed = if reveal {
        blocking
            .reveal_secrets(name, ConfigLayer::empty())
            .map_err(map_config_error)?
    } else {
        std::collections::BTreeMap::new()
    };

    match key {
        Some(key) => {
            // A syntactically fine key that has no effective value (unset at every
            // layer) is the honest not-found diagnostic (stderr, non-zero exit) —
            // in BOTH human and --json mode (stdout stays clean of a partial doc).
            if effective.get(key).is_none() {
                return Err(AgentUnknownConfigKey {
                    message: format!(
                        "Agent Instance '{name}' has no effective value for config key '{key}'. \
                         List the effective config with: kt agent config get {name}"
                    ),
                }
                .into());
            }
            if json {
                // Emit just that one leaf as the same per-leaf object shape.
                let document = config_json(&effective, Some(key), &revealed)?;
                println!("{document}");
            } else {
                // Command result to stdout: the effective value (revealed cleartext
                // for a secret leaf under --reveal, else the masked display).
                println!("{}", leaf_display(&effective, key, &revealed));
            }
            Ok(())
        }
        None => {
            if json {
                // AC4/AD-12: the whole result is ONE JSON document to stdout.
                let document = config_json(&effective, None, &revealed)?;
                println!("{document}");
            } else {
                render_effective_config(name, &effective, &revealed);
            }
            Ok(())
        }
    }
}

/// The display string for one leaf, honoring a `--reveal` overlay (story 2-4). If
/// `revealed` holds this key (a secret leaf under `--reveal`), its CLEARTEXT is
/// shown; otherwise the engine's masked/plain `value_display` is used. The overlay
/// only ever contains secret leaves the engine resolved, so a non-secret key is
/// always the plain display.
fn leaf_display(
    effective: &EffectiveConfig,
    key: &str,
    revealed: &std::collections::BTreeMap<String, String>,
) -> String {
    match revealed.get(key) {
        Some(cleartext) => cleartext.clone(),
        None => effective.value_display(key).unwrap_or_default(),
    }
}

/// The `kt agent config get --json` document (story 2-3, AC4 / AD-12).
///
/// A versioned wrapper — its own `schema_version` (this surface had NO prior
/// `--json`; recorded Decision 4) — carrying each resolved leaf as
/// `{ key, value, source, unvalidated }`: `value` is the rendered display string
/// (the ONE display path shared with the human table + the persisted snapshot, so
/// story 2-4 masks a `secret:` value at this single choke point — AC8), OVERLAID
/// with the engine-resolved cleartext for a secret leaf when `--reveal` is passed
/// (AC-C — the sole un-mask of machine-readable output); `source` is the kebab-case
/// [`ktesio_engine::SourceLayer`] wire label; `unvalidated` is the story-2-2
/// pass-through marker (derived via the engine accessor, AD-2). When `only` is
/// `Some(key)` the document carries just that one leaf (the single-key
/// `config get <name> <key> --json` form).
///
/// Presentation-only: the engine owns the domain types; this wraps the rendered
/// leaves for the `config get` surface.
const CONFIG_GET_SCHEMA_VERSION: u32 = 1;

/// One leaf in the `config get --json` document (story 2-3).
#[derive(Serialize)]
struct ConfigLeaf {
    /// The dotted leaf key.
    key: String,
    /// The rendered winning value (via the single display path — AC8).
    value: String,
    /// The winning source layer's kebab-case label (AC4).
    source: String,
    /// Whether the leaf skipped known-key validation (`agent.*` — story 2-2).
    unvalidated: bool,
}

/// The versioned `config get --json` document (story 2-3, AC4).
#[derive(Serialize)]
struct ConfigDocument {
    /// The config-get document schema version ([`CONFIG_GET_SCHEMA_VERSION`]).
    schema_version: u32,
    /// The resolved leaves (all, or just the single requested key).
    entries: Vec<ConfigLeaf>,
}

/// Serialize the effective config into the pretty `config get --json` document
/// (a versioned [`ConfigDocument`]). Pure (no engine, no I/O) so it is
/// unit-testable in-process; the CLI just prints the returned string to stdout.
/// `only` selects a single leaf (the single-key form) or `None` for the whole
/// config. Every value renders via the engine's ONE display path
/// ([`ktesio_engine::EffectiveConfig::value_display`]) and every source via the
/// engine's [`ktesio_engine::EffectiveConfig::source_label`] accessor — `kt` never
/// re-derives either (AD-2). A serialize failure (not reachable for these plain
/// serde structs) becomes an [`AgentIo`] diagnostic, never a panic.
fn config_json(
    effective: &EffectiveConfig,
    only: Option<&str>,
    revealed: &std::collections::BTreeMap<String, String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let entries: Vec<ConfigLeaf> = effective
        .iter()
        .filter(|(key, _)| only.is_none_or(|k| k == key.as_str()))
        .map(|(key, resolved)| ConfigLeaf {
            key: key.clone(),
            // The engine renders the value (the ONE display path — AC8), which
            // MASKS a secret by default, so `kt` needs no `toml` dep and cannot leak
            // (AD-2/AD-10). `--reveal` overlays the engine-resolved cleartext for a
            // secret leaf (AC-C) — the SOLE way machine-readable output carries an
            // unmasked secret; a non-secret leaf is never in the overlay.
            value: match revealed.get(key.as_str()) {
                Some(cleartext) => cleartext.clone(),
                None => resolved.display(),
            },
            // The source layer is READ from the engine tag, never re-derived.
            source: resolved.source.as_str().to_string(),
            unvalidated: effective.is_unvalidated(key),
        })
        .collect();
    let document = ConfigDocument {
        schema_version: CONFIG_GET_SCHEMA_VERSION,
        entries,
    };
    serde_json::to_string_pretty(&document).map_err(|e| serialize_error("effective config", e))
}

/// The per-row marker (story 2-2, AC-B/AC7) shown in the `config get` table's
/// "Validated" column for a leaf that skipped known-key validation — i.e. a leaf
/// under the `agent.*` pass-through namespace. A validated (known) key shows the
/// affirmative marker. The marker is DERIVED from the pass-through prefix via the
/// engine's [`EffectiveConfig::is_unvalidated`] accessor (so `kt` owns no config
/// internals — AD-2), NOT from a new persisted field; the full per-value source
/// layer stays Epic 2.3.
const UNVALIDATED_MARKER: &str = "unvalidated";
/// The affirmative counterpart shown for a validated (known) key.
const VALIDATED_MARKER: &str = "validated";

/// Render the whole effective config as a table (result → stdout, AD-12). VALUES,
/// the story-2-2 "Validated" marker column, and the story-2-3 **"Source"** column
/// naming each value's winning layer (FR-13). A leaf under `agent.*` is marked
/// **unvalidated** (it bypassed known-key validation, AC-B/AC7); a known key is
/// marked validated. The "Source" column shows the winning [`ktesio_engine::SourceLayer`]
/// label (`engine-default` / `kind-default` / `instance` / `invocation-override`),
/// read per leaf from the engine's `source` tag (AD-2: `kt` never re-derives it).
/// An empty effective config prints a plain info line rather than an empty table.
///
/// `revealed` (story 2-4, AC-C) overlays the engine-resolved cleartext for a secret
/// leaf under `--reveal`; without it (empty map) every `secret:` value stays masked
/// via the engine's `display()`.
fn render_effective_config(
    name: &str,
    effective: &EffectiveConfig,
    revealed: &std::collections::BTreeMap<String, String>,
) {
    let title = format!("Effective config for {name}");
    if effective.is_empty() {
        ui::info(format!("{title}: no config keys set"));
        return;
    }
    let columns = [
        ui::TableColumn::new("Key", 12, 40),
        ui::TableColumn::new("Value", 12, 48),
        ui::TableColumn::new("Validated", 9, 12),
        ui::TableColumn::new("Source", 12, 20),
    ];
    let rows: Vec<Vec<ui::TableCell>> = effective
        .iter()
        .map(|(key, resolved)| {
            // The marker is derived from the `agent.*` pass-through prefix via the
            // engine accessor (AD-2: `kt` never re-implements the boundary). A
            // pass-through leaf is "unvalidated"; a known key is "validated".
            let marker = if effective.is_unvalidated(key) {
                ui::TableCell::muted(UNVALIDATED_MARKER)
            } else {
                ui::TableCell::plain(VALIDATED_MARKER)
            };
            // The engine renders the value (no `toml::Value` in `kt` — AD-2),
            // masking a secret by default; --reveal overlays the resolved cleartext.
            let value = match revealed.get(key.as_str()) {
                Some(cleartext) => cleartext.clone(),
                None => resolved.display(),
            };
            vec![
                ui::TableCell::skill(key.clone()),
                ui::TableCell::plain(value),
                marker,
                // The source layer is READ from the engine tag (story 2-3, FR-13);
                // `kt` never re-derives it (AD-2).
                ui::TableCell::muted(resolved.source.as_str()),
            ]
        })
        .collect();
    ui::print_table(&title, &columns, &rows);
}

/// Translate a [`ConfigError`] (story 2-1) into a `miette` diagnostic with a
/// remediation hint (NFR-1). The unknown-key class (AC-B) carries the offending
/// key + the nearest-key suggestion the engine computed; the shared name/store
/// classes reuse the existing agent diagnostics for a consistent surface;
/// malformed-layer names the layer + path (AC8).
fn map_config_error(err: ConfigError) -> Box<dyn std::error::Error> {
    match err {
        // AC-B: an unknown key outside `agent.*` — the engine already computed the
        // nearest valid key (or "no close match"); surface the whole message
        // (which names the key + the suggestion) with a pass-through remediation.
        ConfigError::UnknownKey { .. } => AgentUnknownConfigKey {
            message: format!(
                "{err}. Set a known unified key, or use the agent.* pass-through namespace for \
                 agent-native extras (e.g. kt agent config set <name> agent.<key> <value>)."
            ),
        }
        .into(),
        // Patch #3: a write that would nest a child under an existing scalar is
        // rejected (nothing persisted) — the message names the conflicting
        // ancestor; add the remediation.
        ConfigError::WriteShapeConflict { .. } => AgentConfig {
            message: format!(
                "{err}. Nothing was changed. Unset or rename the conflicting key first, then \
                 set the nested key."
            ),
        }
        .into(),
        ConfigError::InvalidName { name, reason } => AgentInvalidName {
            message: format!(
                "Invalid Agent Instance name '{name}': {reason}. Names must match \
                 ^[a-z0-9][a-z0-9_-]*$ (lowercase letters, digits, '_' or '-', not starting \
                 with '_' or '-')."
            ),
        }
        .into(),
        ConfigError::NotFound { name } => AgentNotFound {
            message: format!(
                "No Agent Instance named '{name}' is registered. List the Fleet with: kt agent list"
            ),
        }
        .into(),
        ConfigError::MalformedLayer {
            layer,
            path,
            detail,
        } => AgentConfig {
            message: format!(
                "The {layer} config layer at '{path}' could not be read/parsed: {detail}. \
                 Fix the TOML (or restore the file) and try again."
            ),
        }
        .into(),
        ConfigError::Store { name, detail } => AgentStore {
            message: format!(
                "State store error for Agent Instance '{name}': {detail}. The state database may \
                 be inaccessible."
            ),
        }
        .into(),
        // Story 2-4 (AC-C/AC11): `--reveal` re-resolved a secret and it failed
        // (unset env var, ill-permissioned/absent secrets file). A read-surface
        // DIAGNOSTIC (stderr, non-zero exit) — the detail names the NAME + the
        // resolvers tried + a remediation, never a value.
        ConfigError::SecretReveal { detail } => AgentConfig {
            message: format!(
                "{detail}. Set the environment variable, or add it to the engine secrets file \
                 (chmod 600), then try --reveal again."
            ),
        }
        .into(),
    }
}

/// Open the engine using the default (or env-overridden) state dir.
///
/// Passing `None` lets the engine resolve the base via `KTESIO_STATE_DIR` then
/// the platform data dir — the engine remains the sole path authority. The
/// engine owns its tokio runtime; `kt` drives it through the blocking facade.
fn open_engine() -> Result<Engine, Box<dyn std::error::Error>> {
    Engine::open(None).map_err(map_error)
}

/// Translate a [`RegistryError`] into a `miette` diagnostic carrying a
/// remediation hint (NFR-1: name the instance + reason + remediation).
fn map_error(err: RegistryError) -> Box<dyn std::error::Error> {
    match err {
        RegistryError::DuplicateName { name } => AgentDuplicateName {
            message: format!(
                "An Agent Instance named '{name}' already exists. Choose a different name, \
                 or remove the existing instance with: kt agent remove {name}"
            ),
        }
        .into(),
        RegistryError::InvalidName { name, reason } => AgentInvalidName {
            message: format!(
                "Invalid Agent Instance name '{name}': {reason}. Names must match \
                 ^[a-z0-9][a-z0-9_-]*$ (lowercase letters, digits, '_' or '-', not starting \
                 with '_' or '-')."
            ),
        }
        .into(),
        RegistryError::NotFound { name } => AgentNotFound {
            message: format!(
                "No Agent Instance named '{name}' is registered. List the Fleet with: kt agent list"
            ),
        }
        .into(),
        RegistryError::RunningRequiresForce { name } => AgentRunningRequiresForce {
            message: format!(
                "Agent Instance '{name}' is running. Stop it first, or pass --force to remove \
                 it anyway: kt agent remove {name} --delete --force"
            ),
        }
        .into(),
        RegistryError::Io { name, path, source } => AgentIo {
            message: format!(
                "Filesystem error for Agent Instance '{name}' at '{path}': {source}. Check \
                 directory permissions and available disk space."
            ),
        }
        .into(),
        RegistryError::RemoveLeftoverHome { name, path, detail } => AgentIo {
            message: format!(
                "Agent Instance '{name}' was removed from the Fleet, but its Agent Home at \
                 '{path}' could not be deleted: {detail}. Remove the directory manually."
            ),
        }
        .into(),
        // Story 2-3: the effective-config snapshot could not be written (AD-9/AD-6).
        // Surfaced through the lifecycle path as EngineError::Snapshot; this arm
        // keeps the RegistryError mapper exhaustive with a matching diagnostic.
        RegistryError::SnapshotWrite { name, path, detail } => AgentIo {
            message: format!(
                "Could not write the effective-config snapshot for '{name}' at '{path}': {detail}. \
                 Check directory permissions and available disk space, then start it again."
            ),
        }
        .into(),
        RegistryError::RegisterOrphanRow {
            name,
            home_error,
            rollback_error,
        } => AgentIo {
            message: format!(
                "Agent Instance '{name}' left an orphaned registry row: its Agent Home could not \
                 be created ({home_error}) and the automatic rollback also failed \
                 ({rollback_error}). Remove the stale entry with: kt agent remove {name} --force"
            ),
        }
        .into(),
        RegistryError::UnknownAdapterKind { kind } => AgentUnknownKind {
            message: format!(
                "Unknown adapter kind '{kind}'. Register a native adapter with a known kind \
                 (e.g. --kind mock), or supply a manifest adapter with --manifest <path>."
            ),
        }
        .into(),
        RegistryError::ManifestNotFound { path } => AgentManifestNotFound {
            message: format!(
                "No adapter.toml found at '{path}'. Point --manifest at a directory containing \
                 an adapter.toml, or at the file itself."
            ),
        }
        .into(),
        RegistryError::ManifestUnreadable { path, detail } => AgentManifestUnreadable {
            message: format!(
                "Could not read the adapter manifest at '{path}': {detail}. Check that it exists \
                 and is readable (a regular file, with read permission)."
            ),
        }
        .into(),
        RegistryError::ManifestInvalid { path, detail } => AgentManifestInvalid {
            message: format!(
                "The adapter manifest at '{path}' is invalid: {detail}. Fix the named section \
                 and try again."
            ),
        }
        .into(),
        RegistryError::NoMeteringSource { adapter } => AgentNoMeteringSource {
            message: format!(
                "Adapter '{adapter}' declares no viable Metering Source. Add a `[metering]` \
                 section with source = \"self-reported\" or \"engine-observed\" — Ktesio rejects \
                 adapters with no metering source."
            ),
        }
        .into(),
        RegistryError::NoCapabilities { adapter } => AgentNoCapabilities {
            message: format!(
                "Adapter '{adapter}' declares no capabilities. Add a `[capabilities]` section \
                 declaring at least one capability."
            ),
        }
        .into(),
        RegistryError::Store(inner) => AgentStore {
            message: format!("State store error: {inner}. The state database may be inaccessible."),
        }
        .into(),
    }
}

/// Translate an [`EngineError`] (lifecycle: start / stop) into a `miette`
/// diagnostic with a remediation hint (NFR-1). The invalid-transition class
/// (AC4) and the launch-failed diagnostic (AC2) get their own codes; the shared
/// registry-shaped variants (NotFound / InvalidName / Store) reuse the existing
/// agent diagnostics for a consistent surface.
fn map_engine_error(err: EngineError) -> Box<dyn std::error::Error> {
    match err {
        EngineError::NotFound { name } => AgentNotFound {
            message: format!(
                "No Agent Instance named '{name}' is registered. List the Fleet with: kt agent list"
            ),
        }
        .into(),
        EngineError::InvalidName { name, reason } => AgentInvalidName {
            message: format!(
                "Invalid Agent Instance name '{name}': {reason}. Names must match \
                 ^[a-z0-9][a-z0-9_-]*$ (lowercase letters, digits, '_' or '-', not starting \
                 with '_' or '-')."
            ),
        }
        .into(),
        // AC4: the ONE uniform invalid-transition class, identical for every
        // adapter (it comes from the shared transition table).
        EngineError::InvalidTransition(inner) => AgentInvalidTransition {
            message: format!("{inner}. Check the instance's current state with: kt agent list"),
        }
        .into(),
        // AC2: the adapter/process diagnostic is preserved verbatim; the instance
        // is left in `failed`.
        EngineError::LaunchFailed { name, detail } => AgentLaunchFailed {
            message: format!(
                "Agent Instance '{name}' failed to launch: {detail}. The instance is now in the \
                 'failed' state; fix the adapter's launch command and try starting it again."
            ),
        }
        .into(),
        // AC3: pause is UNSUPPORTED on this OS — fail fast QUOTING the declaration
        // (the level + OS) and pointing at `kt agent show`. No state changed.
        EngineError::CapabilityUnsupported {
            name,
            capability,
            os,
            level,
        } => AgentCapabilityUnsupported {
            message: format!(
                "Agent Instance '{name}' cannot {capability}: this agent declares {capability} \
                 '{level}' on {os}. Inspect its Capability Declaration with: kt agent show {name}"
            ),
        }
        .into(),
        EngineError::AdapterUnresolved { name, detail } => AgentLaunchFailed {
            message: format!(
                "Could not resolve the adapter to start Agent Instance '{name}': {detail}."
            ),
        }
        .into(),
        EngineError::Log { name, path, detail } => AgentIo {
            message: format!(
                "Could not write the instance log for '{name}' at '{path}': {detail}. Check \
                 directory permissions and available disk space."
            ),
        }
        .into(),
        // Story 2-3: the effective-config snapshot could not be written at start
        // (AD-9/AD-6). It lands before the `starting` transition, so the instance
        // stays in its prior state; name the snapshot path + a disk/permissions
        // remediation (NFR-1).
        EngineError::Snapshot { name, path, detail } => AgentIo {
            message: format!(
                "Could not write the effective-config snapshot for '{name}' at '{path}': {detail}. \
                 Check directory permissions and available disk space, then start it again."
            ),
        }
        .into(),
        // Story 2-4 (AC-A/AC9): a `secret:NAME` reference could not be resolved at
        // start (unset env var, ill-permissioned/absent secrets file). Resolution
        // runs before the `starting` transition, so the instance stays in its prior
        // state; the detail names the NAME + resolvers + a remediation, never a
        // value (NFR-6).
        EngineError::Secret { name, detail } => AgentConfig {
            message: format!(
                "Agent Instance '{name}' could not start: {detail}. Nothing was changed; set the \
                 secret and start it again."
            ),
        }
        .into(),
        EngineError::Backend { name, source } => AgentIo {
            message: format!("Process control failed for Agent Instance '{name}': {source}."),
        }
        .into(),
        EngineError::Store(inner) => AgentStore {
            message: format!("State store error: {inner}. The state database may be inaccessible."),
        }
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the in-process engine-driver tests that mutate the shared
    /// `KTESIO_STATE_DIR` process env var (`config_get_*` + `list_and_show_*`), so
    /// they never race each other under the multi-threaded test runner (one test
    /// clearing the var mid-run would break another's `open_engine`). A poisoned
    /// lock is fine — the guard is only for env-var mutual exclusion.
    static STATE_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn disposition_from_flags_resolves_each_combination() {
        assert_eq!(
            DispositionArg::from_flags(true, false),
            DispositionArg::Delete
        );
        assert_eq!(
            DispositionArg::from_flags(false, true),
            DispositionArg::Retain
        );
        assert_eq!(
            DispositionArg::from_flags(false, false),
            DispositionArg::Unspecified
        );
    }

    #[test]
    fn disposition_from_flags_fails_closed_to_retain_when_both_set() {
        // F10: clap makes these mutually exclusive, but as defense-in-depth an
        // ambiguous both-set input must fail CLOSED to Retain (the safe
        // default), never silently prefer Delete.
        assert_eq!(
            DispositionArg::from_flags(true, true),
            DispositionArg::Retain
        );
    }

    #[test]
    fn unspecified_resolves_to_retain_the_safe_default() {
        assert_eq!(
            DispositionArg::Unspecified.resolve(),
            RemoveDisposition::Retain
        );
        assert_eq!(DispositionArg::Retain.resolve(), RemoveDisposition::Retain);
        assert_eq!(DispositionArg::Delete.resolve(), RemoveDisposition::Delete);
    }

    #[test]
    fn map_error_includes_remediation_hints() {
        let dup = map_error(RegistryError::DuplicateName {
            name: "demo".into(),
        });
        assert!(dup.to_string().contains("kt agent remove demo"));

        let running = map_error(RegistryError::RunningRequiresForce {
            name: "live".into(),
        });
        assert!(running.to_string().contains("--force"));

        let invalid = map_error(RegistryError::InvalidName {
            name: "Bad".into(),
            reason: ktesio_engine::NameError::BadChar,
        });
        assert!(invalid.to_string().contains("^[a-z0-9]"));

        let missing = map_error(RegistryError::NotFound {
            name: "ghost".into(),
        });
        assert!(missing.to_string().contains("kt agent list"));

        let io = map_error(RegistryError::Io {
            name: "demo".into(),
            path: "/x/agents/demo".into(),
            source: std::io::Error::other("boom"),
        });
        assert!(io.to_string().contains("/x/agents/demo"));

        let leftover = map_error(RegistryError::RemoveLeftoverHome {
            name: "demo".into(),
            path: "/x/agents/demo".into(),
            detail: "still there".into(),
        });
        assert!(leftover.to_string().contains("removed from the Fleet"));

        // F2: the orphan-row partial failure renders the --force remediation.
        let orphan = map_error(RegistryError::RegisterOrphanRow {
            name: "demo".into(),
            home_error: "mkdir failed".into(),
            rollback_error: "delete blocked".into(),
        });
        let orphan_msg = orphan.to_string();
        assert!(orphan_msg.contains("orphaned registry row"));
        assert!(orphan_msg.contains("kt agent remove demo --force"));

        // Store errors surface as a state-store diagnostic.
        let store = map_error(RegistryError::Store(
            ktesio_engine::ports::StoreError::Backend("db gone".into()),
        ));
        assert!(store.to_string().contains("State store error"));
    }

    #[test]
    fn map_error_covers_story_1_3_adapter_variants() {
        let unknown = map_error(RegistryError::UnknownAdapterKind {
            kind: "nope".into(),
        });
        assert!(unknown.to_string().contains("Unknown adapter kind 'nope'"));
        assert!(unknown.to_string().contains("--manifest"));

        let not_found = map_error(RegistryError::ManifestNotFound {
            path: "/x/adapter.toml".into(),
        });
        assert!(not_found.to_string().contains("/x/adapter.toml"));

        // F4: unreadable is its own diagnostic with an existence/readability
        // remediation, NOT the "fix the section" message.
        let unreadable = map_error(RegistryError::ManifestUnreadable {
            path: "/x/adapter.toml".into(),
            detail: "permission denied".into(),
        });
        let unreadable_msg = unreadable.to_string();
        assert!(unreadable_msg.contains("Could not read"));
        assert!(unreadable_msg.contains("permission denied"));
        assert!(unreadable_msg.contains("readable"));
        assert!(
            !unreadable_msg.contains("Fix the named section"),
            "unreadable must not claim a section fix"
        );

        let invalid = map_error(RegistryError::ManifestInvalid {
            path: "/x/adapter.toml".into(),
            detail: "missing the required `[metering]` section".into(),
        });
        assert!(invalid.to_string().contains("[metering]"));

        let no_metering = map_error(RegistryError::NoMeteringSource {
            adapter: "demo".into(),
        });
        assert!(no_metering.to_string().contains("[metering]"));
        assert!(no_metering.to_string().contains("self-reported"));

        let no_caps = map_error(RegistryError::NoCapabilities {
            adapter: "demo".into(),
        });
        assert!(no_caps.to_string().contains("[capabilities]"));
    }

    #[test]
    fn adapter_arg_from_flags_resolves_and_requires_one() {
        assert_eq!(
            AdapterArg::from_flags(Some("mock".into()), None).unwrap(),
            AdapterArg::Kind("mock".into())
        );
        assert_eq!(
            AdapterArg::from_flags(None, Some("/x".into())).unwrap(),
            AdapterArg::Manifest("/x".into())
        );
        // --kind wins if both somehow arrive (clap prevents this normally).
        assert_eq!(
            AdapterArg::from_flags(Some("mock".into()), Some("/x".into())).unwrap(),
            AdapterArg::Kind("mock".into())
        );
        // Neither is an error.
        assert!(AdapterArg::from_flags(None, None).is_err());
    }

    #[test]
    fn adapter_arg_to_ref_maps_both_kinds() {
        assert_eq!(
            AdapterArg::Kind("mock".into()).to_ref(),
            AdapterRef::Native("mock".into())
        );
        assert_eq!(
            AdapterArg::Manifest("/x/dir".into()).to_ref(),
            AdapterRef::Manifest(std::path::PathBuf::from("/x/dir"))
        );
    }

    fn sample_fleet_entry(name: &str) -> FleetEntry {
        FleetEntry {
            name: ktesio_engine::InstanceName::new(name).unwrap(),
            kind: "mock".to_string(),
            state: ktesio_engine::LifecycleState::Registered,
            restart_count: 0,
            restart_policy: ktesio_engine::RestartPolicy::OnFailure,
            failed_cause: None,
            budget: None,
            usage: None,
            agent_home: format!("/x/agents/{name}"),
        }
    }

    #[test]
    fn fleet_json_emits_versioned_document_with_null_seeds() {
        // The `list --json` document is a versioned FleetListing whose per-entry
        // budget/usage are the honest JSON null seed (never 0). Pure — no engine.
        let doc = fleet_json(vec![sample_fleet_entry("alpha")]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(
            value["schema_version"],
            serde_json::json!(FLEET_SCHEMA_VERSION)
        );
        let entry = &value["instances"][0];
        assert_eq!(entry["name"], serde_json::json!("alpha"));
        assert_eq!(entry["budget"], serde_json::Value::Null);
        assert_eq!(entry["usage"], serde_json::Value::Null);
    }

    #[test]
    fn fleet_json_on_empty_is_a_valid_empty_array() {
        // AC9: an empty Fleet serializes as a valid empty `instances` array.
        let doc = fleet_json(vec![]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(value["instances"], serde_json::json!([]));
    }

    #[test]
    fn show_json_wraps_one_entry_with_the_shared_schema_version() {
        // `show --json` is { schema_version, instance: <FleetEntry> } — the SAME
        // schema version as list --json (AD-14: one schema), null metering seed.
        let doc = show_json(sample_fleet_entry("web-1")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(
            value["schema_version"],
            serde_json::json!(FLEET_SCHEMA_VERSION)
        );
        assert_eq!(value["instance"]["name"], serde_json::json!("web-1"));
        assert_eq!(value["instance"]["budget"], serde_json::Value::Null);
        assert_eq!(value["instance"]["usage"], serde_json::Value::Null);
    }

    #[test]
    fn serialize_error_wraps_into_an_agent_io_diagnostic() {
        // The defense-in-depth serialization-failure wrapper names what failed.
        let err = serde_json::from_str::<i32>("not-json").unwrap_err();
        let wrapped = serialize_error("Fleet", err);
        assert!(wrapped
            .to_string()
            .contains("Failed to serialize the Fleet"));
    }

    #[test]
    fn list_and_show_drive_the_engine_in_process_json_and_human() {
        // Cover the list()/show() success paths in-process (both --json and
        // human) against a real temp state dir. `open_engine()` reads
        // KTESIO_STATE_DIR; set it, seed one instance via the engine, then drive
        // each surface. They print to stdout (test noise, harmless) and must all
        // return Ok — proving the full CLI read path, not just the pure helpers.
        // Hold the shared env lock so this and the config_get driver test never
        // race on the process-global KTESIO_STATE_DIR.
        let _guard = STATE_DIR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        // SAFETY: guarded by STATE_DIR_ENV_LOCK; set the state dir the CLI resolves.
        unsafe {
            std::env::set_var("KTESIO_STATE_DIR", tmp.path());
        }
        {
            let engine = Engine::open(Some(tmp.path().to_path_buf())).unwrap();
            engine.blocking().register("demo", "mock").unwrap();
        }
        // Human + JSON list, human + JSON show — every success path.
        list(false).unwrap();
        list(true).unwrap();
        show("demo", false).unwrap();
        show("demo", true).unwrap();
        // Empty-Fleet JSON + human paths (a different state dir, no instances).
        let empty = tempfile::TempDir::new().unwrap();
        unsafe {
            std::env::set_var("KTESIO_STATE_DIR", empty.path());
        }
        list(true).unwrap();
        list(false).unwrap();
        unsafe {
            std::env::remove_var("KTESIO_STATE_DIR");
        }
    }

    /// Build a small effective config in-process for the config_json unit tests
    /// (a known key from the instance layer + an agent.* pass-through leaf from a
    /// weaker layer), reusing the engine's public resolver.
    fn sample_effective() -> EffectiveConfig {
        use ktesio_engine::{resolve, SourceLayer};
        let layers = [
            ConfigLayer::parse(
                SourceLayer::EngineDefault,
                "<e>",
                "agent = { legacy = \"on\" }\n",
            )
            .unwrap(),
            ConfigLayer::empty(),
            ConfigLayer::parse(SourceLayer::Instance, "<i>", "model = \"gpt-4\"\n").unwrap(),
            ConfigLayer::empty(),
        ];
        resolve(layers)
    }

    #[test]
    fn config_json_emits_versioned_document_with_source_and_unvalidated_per_leaf() {
        // Story 2-3 (AC4): the pure serializer emits a versioned document with
        // { key, value, source, unvalidated } per leaf. The known `model` key is
        // instance-sourced + validated; the agent.* leaf is engine-sourced +
        // unvalidated. Values render via the ONE display path (bare strings).
        let eff = sample_effective();
        let doc = config_json(&eff, None, &std::collections::BTreeMap::new()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(value["schema_version"], serde_json::json!(1));
        let entries = value["entries"].as_array().unwrap();

        let model = entries.iter().find(|e| e["key"] == "model").unwrap();
        assert_eq!(model["value"], serde_json::json!("gpt-4"));
        assert_eq!(model["source"], serde_json::json!("instance"));
        assert_eq!(model["unvalidated"], serde_json::json!(false));

        let legacy = entries.iter().find(|e| e["key"] == "agent.legacy").unwrap();
        assert_eq!(legacy["value"], serde_json::json!("on"));
        assert_eq!(legacy["source"], serde_json::json!("engine-default"));
        assert_eq!(legacy["unvalidated"], serde_json::json!(true));
    }

    #[test]
    fn config_json_single_key_emits_just_that_leaf() {
        // The single-key `config get <name> <key> --json` form emits exactly one
        // leaf, sourced + rendered identically to the whole-config form.
        let eff = sample_effective();
        let doc = config_json(&eff, Some("model"), &std::collections::BTreeMap::new()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["key"], serde_json::json!("model"));
        assert_eq!(entries[0]["source"], serde_json::json!("instance"));
    }

    #[test]
    fn config_json_value_matches_the_human_display_form() {
        // AC8: the --json value and the human value both render via the ONE display
        // path — a non-string scalar renders in the same inline form in both.
        use ktesio_engine::{resolve, SourceLayer};
        let eff = resolve([
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::parse(SourceLayer::Instance, "<i>", "n = 42\narr = [1, 2]\n").unwrap(),
            ConfigLayer::empty(),
        ]);
        let doc = config_json(&eff, None, &std::collections::BTreeMap::new()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
        let entries = value["entries"].as_array().unwrap();
        let n = entries.iter().find(|e| e["key"] == "n").unwrap();
        // Same inline rendering as effective.value_display / the human table.
        assert_eq!(
            n["value"],
            serde_json::json!(eff.value_display("n").unwrap())
        );
        let arr = entries.iter().find(|e| e["key"] == "arr").unwrap();
        assert_eq!(
            arr["value"],
            serde_json::json!(eff.value_display("arr").unwrap())
        );
    }

    #[test]
    fn config_get_drives_the_engine_in_process_human_and_json() {
        // Cover the config_get() success paths in-process (human + --json, whole +
        // single-key) against a real temp state dir, mirroring the list/show cover
        // test. Prints to stdout (harmless test noise) and must all return Ok.
        // Hold the shared env lock: this test mutates KTESIO_STATE_DIR, which other
        // in-process engine-driver tests also touch.
        let _guard = STATE_DIR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        // SAFETY: guarded by STATE_DIR_ENV_LOCK; set the state dir the CLI resolves.
        unsafe {
            std::env::set_var("KTESIO_STATE_DIR", tmp.path());
        }
        {
            let engine = Engine::open(Some(tmp.path().to_path_buf())).unwrap();
            let blocking = engine.blocking();
            blocking.register("demo", "mock").unwrap();
            blocking.set_config("demo", "model", "gpt-4").unwrap();
            blocking.set_config("demo", "agent.flag", "on").unwrap();
        }
        // Whole-config: human + JSON. Single-key: human + JSON.
        config_get("demo", None, false, false).unwrap();
        config_get("demo", None, true, false).unwrap();
        config_get("demo", Some("model"), false, false).unwrap();
        config_get("demo", Some("model"), true, false).unwrap();
        // A not-set key is a non-zero (Err) diagnostic in both modes.
        assert!(config_get("demo", Some("missing"), false, false).is_err());
        assert!(config_get("demo", Some("missing"), true, false).is_err());
        unsafe {
            std::env::remove_var("KTESIO_STATE_DIR");
        }
    }

    #[test]
    fn config_get_reveal_overlays_resolved_cleartext_in_process() {
        // Story 2-4 (AC-C): cover the `--reveal` paths in-process — the reveal
        // overlay (`leaf_display`, `config_json` + `render_effective_config` with a
        // non-empty overlay) and the `leaf_display` fallback. Sets a secret env var
        // so the engine resolves it. Holds the shared env lock (mutates
        // KTESIO_STATE_DIR + the secret env var).
        let _guard = STATE_DIR_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        let sentinel = "s3cr3t-inproc-reveal";
        let secret_key = "KTESIO_INPROC_REVEAL_KEY";
        // SAFETY: guarded by STATE_DIR_ENV_LOCK.
        unsafe {
            std::env::set_var("KTESIO_STATE_DIR", tmp.path());
            std::env::set_var(secret_key, sentinel);
        }
        {
            let engine = Engine::open(Some(tmp.path().to_path_buf())).unwrap();
            let blocking = engine.blocking();
            blocking.register("sec", "mock").unwrap();
            blocking
                .set_config("sec", "model", &format!("secret:{secret_key}"))
                .unwrap();
            blocking
                .set_config("sec", "agent.plain", "visible")
                .unwrap();

            // reveal_secrets returns the resolved cleartext for the secret leaf only.
            let revealed = blocking
                .reveal_secrets("sec", ktesio_engine::ConfigLayer::empty())
                .unwrap();
            assert_eq!(revealed.get("model").map(String::as_str), Some(sentinel));
            assert!(!revealed.contains_key("agent.plain"), "only secret leaves");
        }
        // The reveal render paths run without error (whole + single-key, human +
        // JSON), and the default (masked) paths too.
        config_get("sec", None, true, true).unwrap(); // --json --reveal (whole)
        config_get("sec", None, false, true).unwrap(); // human --reveal (whole)
        config_get("sec", Some("model"), true, true).unwrap(); // single-key reveal
        config_get("sec", Some("model"), false, true).unwrap(); // single-key human reveal
        config_get("sec", None, true, false).unwrap(); // default masked --json

        // leaf_display: revealed overlay wins; absent key falls back to display().
        let engine = Engine::open(Some(tmp.path().to_path_buf())).unwrap();
        let eff = engine
            .blocking()
            .effective_config("sec", ktesio_engine::ConfigLayer::empty())
            .unwrap();
        let mut overlay = std::collections::BTreeMap::new();
        overlay.insert("model".to_string(), sentinel.to_string());
        assert_eq!(leaf_display(&eff, "model", &overlay), sentinel);
        // A non-overlaid secret leaf falls back to the masked display().
        assert_eq!(
            leaf_display(&eff, "model", &std::collections::BTreeMap::new()),
            ktesio_engine::SECRET_MASK
        );

        unsafe {
            std::env::remove_var("KTESIO_STATE_DIR");
            std::env::remove_var(secret_key);
        }
    }

    #[test]
    fn render_capabilities_handles_empty_and_nonempty() {
        use ktesio_engine::{Capability, OsId, SupportLevel};
        // Empty projection: prints the "none declared" info line without panic.
        let empty = EffectiveCapabilities {
            os: OsId::current(),
            entries: vec![],
        };
        render_capabilities("demo", &empty);

        // Non-empty: renders a table without panic.
        let full = EffectiveCapabilities {
            os: OsId::current(),
            entries: vec![
                (Capability::Pause, SupportLevel::Guaranteed),
                (Capability::Interaction, SupportLevel::BestEffort),
            ],
        };
        render_capabilities("demo", &full);
    }
}
