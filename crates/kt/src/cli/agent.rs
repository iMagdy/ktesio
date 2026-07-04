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

use ktesio_engine::{
    AdapterRef, EffectiveCapabilities, Registry, RegistryError, RemoveDisposition,
};

use crate::error::{
    AgentDuplicateName, AgentInvalidName, AgentIo, AgentManifestInvalid, AgentManifestNotFound,
    AgentManifestUnreadable, AgentNoCapabilities, AgentNoMeteringSource, AgentNotFound,
    AgentRunningRequiresForce, AgentStore, AgentUnknownKind,
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
    let registry = open_registry()?;
    match registry.register_with_adapter(name, &adapter.to_ref()) {
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
            match registry.effective_capabilities(instance.name.as_str()) {
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

/// `kt agent show <name>` — render an instance's effective Capability
/// Declaration (AC1 "visible for the instance").
///
/// Human-readable only; `--json` is out of scope (FR-4 / story 1.7).
pub fn show(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let registry = open_registry()?;
    let caps = registry.effective_capabilities(name).map_err(map_error)?;
    render_capabilities(name, &caps);
    Ok(())
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
    let registry = open_registry()?;
    match registry.remove(name, disposition.resolve(), force) {
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

/// `kt agent list` — render the Fleet as a plain human table.
///
/// A `--json` variant is out of scope here (Fleet visibility with `--json` is
/// FR-4 / story 1.7); a human table suffices. Deferral noted.
pub fn list() -> Result<(), Box<dyn std::error::Error>> {
    let registry = open_registry()?;
    let instances = registry.list().map_err(map_error)?;

    if instances.is_empty() {
        ui::info("No Agent Instances registered yet. Register one with: kt agent register <name> --kind <kind>");
        return Ok(());
    }

    let columns = [
        ui::TableColumn::new("Name", 12, 32),
        ui::TableColumn::new("Kind", 8, 24),
        ui::TableColumn::new("State", 10, 12),
        ui::TableColumn::new("Agent Home", 20, 64),
    ];
    let rows: Vec<Vec<ui::TableCell>> = instances
        .iter()
        .map(|instance| {
            vec![
                ui::TableCell::skill(instance.name.as_str()),
                ui::TableCell::plain(instance.kind.clone()),
                ui::TableCell::status(instance.state.as_str()),
                ui::TableCell::muted(instance.agent_home.clone()),
            ]
        })
        .collect();
    ui::print_table("Fleet", &columns, &rows);
    Ok(())
}

/// Open the engine registry using the default (or env-overridden) state dir.
///
/// Passing `None` lets the engine resolve the base via `KTESIO_STATE_DIR` then
/// the platform data dir — the engine remains the sole path authority.
fn open_registry() -> Result<Registry, Box<dyn std::error::Error>> {
    Registry::open(None).map_err(map_error)
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

#[cfg(test)]
mod tests {
    use super::*;

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
