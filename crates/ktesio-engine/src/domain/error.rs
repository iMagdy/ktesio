//! [`RegistryError`] — the registry service's error type (thiserror, no miette).
//!
//! Each variant carries enough for `kt` to render a remediation hint (NFR-1:
//! every partial failure names the instance + reason + remediation). `miette`
//! wrapping happens in `kt`, never here (conventions).

use thiserror::Error;

use crate::ports::{BackendError, StoreError};

use super::name::NameError;
use super::transition::LifecycleError;

/// Errors from the registry service (`register` / `remove`).
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The requested name collides with an existing instance.
    ///
    /// Distinct from [`StoreError::DuplicateName`] so the service layer can
    /// attach registry-level context; the store variant is the low-level cause.
    #[error("an Agent Instance named '{name}' already exists")]
    DuplicateName {
        /// The conflicting instance name.
        name: String,
    },

    /// The supplied name failed the naming rule at construction.
    #[error("invalid Agent Instance name '{name}': {reason}")]
    InvalidName {
        /// The rejected candidate string.
        name: String,
        /// The specific rule that failed.
        reason: NameError,
    },

    /// `remove` targeted a name that is not registered.
    #[error("no Agent Instance named '{name}' is registered")]
    NotFound {
        /// The missing instance name.
        name: String,
    },

    /// `remove` targeted a `running` instance without `--force` (AC5).
    #[error("Agent Instance '{name}' is running; stop it first or pass --force")]
    RunningRequiresForce {
        /// The running instance's name.
        name: String,
    },

    /// A filesystem operation on the Agent Home failed.
    ///
    /// Carries the offending path so the diagnostic can name it (NFR-1). Used
    /// both for creation failures (rolled back) and the removal partial-failure
    /// case (row already deleted, directory could not be removed).
    #[error("filesystem error for Agent Instance '{name}' at {path}: {source}")]
    Io {
        /// The instance the operation was for.
        name: String,
        /// The path that could not be created/written/removed.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The Agent Home directory was deleted but the DB row already gone, or a
    /// removal left an artifact behind — a partial-failure state needing
    /// operator attention. Kept distinct from [`RegistryError::Io`] so `kt` can
    /// phrase "removed from the Fleet, but ..." precisely.
    #[error(
        "Agent Instance '{name}' was removed from the Fleet, but its Agent Home at {path} could not be deleted: {detail}"
    )]
    RemoveLeftoverHome {
        /// The removed instance's name.
        name: String,
        /// The leftover Agent Home path.
        path: String,
        /// Why deletion failed.
        detail: String,
    },

    /// Registration's Agent Home step failed AND the compensating row delete
    /// also failed, so a `registered` row survives with no Agent Home behind
    /// it — a partial-failure state needing operator attention. Distinct from
    /// [`RegistryError::Io`] so `kt` can name the orphaned row and its cleanup
    /// (mirrors [`RegistryError::RemoveLeftoverHome`]).
    #[error(
        "Agent Instance '{name}' left an orphaned registry row after its Agent Home could not be created ({home_error}) and the rollback delete also failed ({rollback_error}); remove it with: kt agent remove {name} --force"
    )]
    RegisterOrphanRow {
        /// The orphaned instance's name.
        name: String,
        /// Why the Agent Home could not be created (the original failure).
        home_error: String,
        /// Why the compensating row delete failed.
        rollback_error: String,
    },

    /// A native adapter `kind` was requested that no builtin provides (story
    /// 1.3). Carries the unrecognized kind so `kt` can suggest alternatives.
    #[error("unknown adapter kind '{kind}'")]
    UnknownAdapterKind {
        /// The unrecognized native kind string.
        kind: String,
    },

    /// A manifest adapter was requested but no `adapter.toml` was found at the
    /// resolved path (story 1.3). Names the path searched.
    #[error("no adapter.toml found at {path}")]
    ManifestNotFound {
        /// The path searched (the file, or `<dir>/adapter.toml`).
        path: String,
    },

    /// A manifest adapter's `adapter.toml` exists but could not be read (an I/O
    /// error — e.g. permissions, or the path is a directory). Distinct from
    /// [`RegistryError::ManifestInvalid`] because the operator's remediation is
    /// different: check existence/readability, not "fix the section" (F4).
    #[error("could not read adapter.toml at {path}: {detail}")]
    ManifestUnreadable {
        /// The manifest path that could not be read.
        path: String,
        /// The underlying I/O error.
        detail: String,
    },

    /// A manifest adapter's `adapter.toml` failed to parse or validate (story
    /// 1.3). `detail` NAMES the failing section (AC2) so the diagnostic can
    /// quote it.
    #[error("adapter.toml at {path} is invalid: {detail}")]
    ManifestInvalid {
        /// The manifest path.
        path: String,
        /// The section-naming validation detail.
        detail: String,
    },

    /// An adapter declared no viable Metering Source and was rejected at
    /// registration (story 1.3; FR-19 hard line, AC4). Names the adapter.
    #[error("adapter '{adapter}' declares no viable Metering Source; add a `[metering]` section")]
    NoMeteringSource {
        /// The adapter kind/identity that lacked a source.
        adapter: String,
    },

    /// An adapter declared no capabilities and was rejected at registration
    /// (story 1.3; AC2). Names the adapter.
    #[error("adapter '{adapter}' declares no capabilities; add a `[capabilities]` section")]
    NoCapabilities {
        /// The adapter kind/identity that lacked capabilities.
        adapter: String,
    },

    /// A [`StateStore`](crate::ports::StateStore) operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Errors from the lifecycle supervision surface (`start` / `stop`, story 1.4).
///
/// Distinct from [`RegistryError`] (registration) so `kt` can map lifecycle
/// failures — an invalid transition (AC4), a launch failure (AC2) — to their own
/// diagnostics. Every variant names the instance + reason so `kt` can render a
/// remediation (NFR-1). `thiserror`, never `miette` (conventions).
#[derive(Debug, Error)]
pub enum EngineError {
    /// The instance is not registered. Names it.
    #[error("no Agent Instance named '{name}' is registered")]
    NotFound {
        /// The missing instance name.
        name: String,
    },

    /// The supplied name failed the naming rule.
    #[error("invalid Agent Instance name '{name}': {reason}")]
    InvalidName {
        /// The rejected candidate string.
        name: String,
        /// The specific rule that failed.
        reason: NameError,
    },

    /// A lifecycle command was invalid from the instance's current state (AC4).
    /// The SAME error for every adapter (it comes from the shared transition
    /// table before any adapter code runs).
    #[error(transparent)]
    InvalidTransition(#[from] LifecycleError),

    /// A capability (this story: pause) is UNSUPPORTED for this Agent Instance on
    /// the current OS (story 1-5, AC3): the effective Capability Declaration
    /// projects to `Unsupported`, so the command FAILS FAST — quoting the
    /// declaration (the level + OS), with NO state change, NO process signal, and
    /// no fake attempt. Names the instance + capability + OS + declared level so
    /// `kt` can quote the declaration and point at `kt agent show`.
    #[error(
        "Agent Instance '{name}' cannot {capability}: this adapter declares {capability} '{level}' on {os} (see its Capability Declaration)"
    )]
    CapabilityUnsupported {
        /// The instance the command targeted.
        name: String,
        /// The capability that is unsupported (`"pause"`).
        capability: String,
        /// The current OS the declaration was projected onto.
        os: String,
        /// The declared support level for that capability on that OS
        /// (`"unsupported"`).
        level: String,
    },

    /// The agent failed to launch (AC2): the adapter/process diagnostic is
    /// PRESERVED in `detail`, the instance is left in `failed`, and no zombie
    /// remains. Names the instance.
    #[error("Agent Instance '{name}' failed to launch: {detail}")]
    LaunchFailed {
        /// The instance that failed to start.
        name: String,
        /// The preserved adapter/process diagnostic (verbatim, AC2).
        detail: String,
    },

    /// The instance's adapter could not be re-resolved for launch (a corrupt or
    /// now-missing manifest/snapshot). Names the instance + detail.
    #[error("could not resolve the adapter for Agent Instance '{name}': {detail}")]
    AdapterUnresolved {
        /// The instance whose adapter failed to resolve.
        name: String,
        /// Why resolution failed.
        detail: String,
    },

    /// A per-instance log I/O operation failed (AD-12 seed). Names the path.
    #[error("could not write the instance log for '{name}' at {path}: {detail}")]
    Log {
        /// The instance the log is for.
        name: String,
        /// The log path.
        path: String,
        /// The underlying I/O detail.
        detail: String,
    },

    /// A process-control backend operation failed unexpectedly (not a launch
    /// failure — a signal/terminate/wait error). Names the instance.
    #[error("process control failed for Agent Instance '{name}': {source}")]
    Backend {
        /// The instance the operation was for.
        name: String,
        /// The underlying backend error.
        source: BackendError,
    },

    /// A [`StateStore`](crate::ports::StateStore) operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}
