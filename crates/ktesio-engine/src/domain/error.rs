//! [`RegistryError`] — the registry service's error type (thiserror, no miette).
//!
//! Each variant carries enough for `kt` to render a remediation hint (NFR-1:
//! every partial failure names the instance + reason + remediation). `miette`
//! wrapping happens in `kt`, never here (conventions).

use thiserror::Error;

use crate::ports::StoreError;

use super::name::NameError;

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

    /// A [`StateStore`](crate::ports::StateStore) operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}
