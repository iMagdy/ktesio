//! The registry service — `register` / `remove` (spine AD-1, AD-2).
//!
//! This is the engine's public surface for the registration capability
//! (FR-1/FR-2/FR-3). It is re-exported at the crate root and IS part of the
//! Embedding Interface (AD-2): `kt` drives register/remove/list through these
//! methods and the returned domain types only.
//!
//! ## Facade-friendliness (forward contract for story 1.4)
//!
//! The service takes its state-dir base explicitly ([`Registry::open`]) and
//! holds no global or thread-local state, so when the engine goes async (AD-13,
//! story 1.4) these sync methods can sit behind the `blocking()` facade
//! unchanged. Do NOT introduce hidden globals or assume a running runtime.

use std::path::Path;

use crate::paths::EnginePaths;
use crate::ports::{StateStore, StoreError};
use crate::store::SqliteStore;
use crate::time::now_rfc3339;

use super::error::RegistryError;
use super::instance::AgentInstance;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;

/// Retain or delete the Agent Home when removing an instance (AC4).
///
/// Named directly from the acceptance criterion's "retain or delete". The DB
/// row is always deleted; this only decides the fate of the on-disk Agent Home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveDisposition {
    /// Leave the Agent Home directory intact on disk.
    Retain,
    /// Remove the Agent Home directory tree.
    Delete,
}

/// The registry service: the engine's registration capability.
///
/// Owns a resolved [`EnginePaths`] (path authority) and a [`StateStore`]. Open
/// one with [`Registry::open`]; each call is self-contained.
pub struct Registry {
    paths: EnginePaths,
    store: SqliteStore,
}

impl Registry {
    /// Open a registry rooted at an optional state-dir base.
    ///
    /// `base`:
    /// * `Some(path)` — use it (tests / explicit embedding).
    /// * `None` — resolve via `KTESIO_STATE_DIR` then the platform data dir
    ///   ([`EnginePaths::new`]).
    ///
    /// Ensures the state dir exists and opens (creating + migrating) the DB.
    pub fn open(base: Option<std::path::PathBuf>) -> Result<Self, RegistryError> {
        // Preserve the offending base (if one was supplied) so the diagnostic
        // can name it instead of showing a blank path.
        let offending = base
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("<default via {}>", crate::paths::STATE_DIR_ENV));
        let paths = EnginePaths::new(base).map_err(|e| RegistryError::Io {
            name: "<state-dir>".to_string(),
            path: offending,
            source: std::io::Error::other(e.to_string()),
        })?;
        // Create the state base (and thus its parent chain) before opening the
        // DB — a fresh install has no directory yet (AC1 "fresh state").
        ensure_dir(paths.state_base(), "<state-dir>")?;
        let store = SqliteStore::open(&paths.state_db())?;
        Ok(Self { paths, store })
    }

    /// The engine-computed Agent Home path for `name` (path authority helper
    /// for callers that want to display it without a full lookup).
    pub fn agent_home(&self, name: &InstanceName) -> std::path::PathBuf {
        self.paths.agent_home(name)
    }

    /// Register a new Agent Instance under `name` of `kind`.
    ///
    /// On success the instance is in [`LifecycleState::Registered`], its Agent
    /// Home exists with an instance `config.toml`, and its Usage Ledger is
    /// empty (zero `usage_events` rows). Returns the created [`AgentInstance`]
    /// (whose `agent_home` the caller may display).
    ///
    /// ## Atomicity ordering (chosen — commented per the story's Atomicity note)
    ///
    /// **Row first, files second, roll the row back on a filesystem failure.**
    /// 1. Validate the name (rejected here, nothing touched).
    /// 2. Insert the DB row via the store. The `UNIQUE` constraint detects a
    ///    duplicate atomically and returns before ANY file is created — so a
    ///    duplicate registration performs **no partial writes** (AC2).
    /// 3. Create the Agent Home directory and write `config.toml`.
    /// 4. If step 3 fails, delete the just-inserted row and remove any partial
    ///    directory, then surface the I/O error — leaving no orphan row and no
    ///    half-created home.
    ///
    /// (The story's alternate "files first, row second" ordering is equally
    /// valid; this variant keeps duplicate detection filesystem-side-effect
    /// free without needing uncommitted-transaction access through the port.)
    pub fn register(&self, name: &str, kind: &str) -> Result<AgentInstance, RegistryError> {
        // (1) Validate the name.
        let name = InstanceName::new(name).map_err(|reason| RegistryError::InvalidName {
            name: name.to_string(),
            reason,
        })?;

        let home = self.paths.agent_home(&name);
        let now = now_rfc3339();
        let instance = AgentInstance {
            name: name.clone(),
            kind: kind.to_string(),
            state: LifecycleState::Registered,
            agent_home: home.to_string_lossy().into_owned(),
            created_at: now.clone(),
            updated_at: now,
        };

        // (2) Insert the row. Duplicate -> DuplicateName, nothing on disk yet.
        self.store.create_instance(&instance).map_err(|e| match e {
            StoreError::DuplicateName { name } => RegistryError::DuplicateName { name },
            other => RegistryError::Store(other),
        })?;

        // (3) Create the Agent Home + instance config; (4) roll back on failure.
        if let Err(io_err) = self.materialize_home(&name) {
            // Rollback: remove the row first (restoring atomicity), then any
            // partial directory. The row delete is the load-bearing step — if
            // it fails we would leak an orphan `registered` row with no home,
            // breaking the atomicity contract, so we surface that distinctly
            // (naming the orphaned row + remediation, NFR-1) rather than
            // discarding it. The partial-directory cleanup is best-effort.
            if let Err(rollback_err) = self.store.delete_instance(&name) {
                let _ = std::fs::remove_dir_all(&home);
                return Err(RegistryError::RegisterOrphanRow {
                    name: name.as_str().to_string(),
                    home_error: io_err.to_string(),
                    rollback_error: rollback_err.to_string(),
                });
            }
            let _ = std::fs::remove_dir_all(&home);
            return Err(io_err);
        }

        Ok(instance)
    }

    /// Create the Agent Home directory and write the instance `config.toml`.
    ///
    /// `[ASSUMPTION]` the filename `config.toml` and a minimal instance-level
    /// body. Full layered config resolution is Epic 2; here we only persist the
    /// instance layer so "created with instance config" (FR-1/AC1) holds.
    fn materialize_home(&self, name: &InstanceName) -> Result<(), RegistryError> {
        let home = self.paths.agent_home(name);
        ensure_dir(&home, name.as_str())?;
        let config_path = self.paths.instance_config(name);
        // Minimal instance-level TOML (AD-9: TOML at every layer). Kept tiny;
        // Epic 2 owns the real config schema and layering.
        let body = format!(
            "# Ktesio Agent Instance config (instance layer).\n\
             # Managed by the engine; edit with care.\n\
             name = \"{name}\"\n",
            name = name.as_str(),
        );
        std::fs::write(&config_path, body).map_err(|source| RegistryError::Io {
            name: name.as_str().to_string(),
            path: config_path.to_string_lossy().into_owned(),
            source,
        })?;
        Ok(())
    }

    /// Remove an Agent Instance, honoring the retain/delete disposition (AC4)
    /// and the running-guard (AC5).
    ///
    /// ## Running-guard SCOPE BOUNDARY (AC5)
    ///
    /// Nothing can actually `start`/run until story 1.4 (tokio supervision
    /// core), so this guard is **state-machine validation only**: if the stored
    /// Lifecycle State is `running` and `force` is false, it returns
    /// [`RegistryError::RunningRequiresForce`]. Because a real `running`
    /// instance cannot be produced yet, tests prove this path by directly
    /// seeding a `running` row via the store. Real running-instance teardown
    /// (stopping the process before removal) lands in story 1.4/1.6.
    ///
    /// ## Removal ordering
    ///
    /// Delete the DB row first; then, for [`RemoveDisposition::Delete`], remove
    /// the Agent Home tree. If the tree removal fails after the row is gone,
    /// return [`RegistryError::RemoveLeftoverHome`] naming the leftover path
    /// (NFR-1) rather than reporting silent success.
    pub fn remove(
        &self,
        name: &str,
        disposition: RemoveDisposition,
        force: bool,
    ) -> Result<(), RegistryError> {
        // Validate the name shape so a malformed name yields InvalidName rather
        // than a confusing NotFound.
        let name = InstanceName::new(name).map_err(|reason| RegistryError::InvalidName {
            name: name.to_string(),
            reason,
        })?;

        // Look up the instance (also gives us its Lifecycle State for the guard).
        let instance = self
            .store
            .get_instance(&name)?
            .ok_or_else(|| RegistryError::NotFound {
                name: name.as_str().to_string(),
            })?;

        // Running-guard (AC5): refuse a running instance unless --force.
        if !force && !instance.state.is_removable_without_force() {
            return Err(RegistryError::RunningRequiresForce {
                name: name.as_str().to_string(),
            });
        }

        // Always delete the row first.
        self.store.delete_instance(&name)?;

        // Then handle the Agent Home per disposition.
        if disposition == RemoveDisposition::Delete {
            let home = self.paths.agent_home(&name);
            // Delete the tree directly (no exists() pre-check — that would be a
            // TOCTOU race). An already-absent home is success: the desired end
            // state (no home) already holds. Only surface real removal failures.
            if let Err(err) = std::fs::remove_dir_all(&home) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    return Err(RegistryError::RemoveLeftoverHome {
                        name: name.as_str().to_string(),
                        path: home.to_string_lossy().into_owned(),
                        detail: err.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// List the whole Fleet, ordered by name (delegates to the store).
    pub fn list(&self) -> Result<Vec<AgentInstance>, RegistryError> {
        Ok(self.store.list_instances()?)
    }

    /// Count Usage Ledger events for an instance (used to prove empty ledgers).
    pub fn usage_event_count(&self, name: &InstanceName) -> Result<u64, RegistryError> {
        Ok(self.store.count_usage_events(name)?)
    }

    /// Test-only escape hatch to seed an instance in an arbitrary Lifecycle
    /// State (e.g. `running`) so the AC5 guard can be exercised without a real
    /// supervision core (which does not exist until story 1.4).
    #[cfg(test)]
    pub(crate) fn seed_instance(&self, instance: &AgentInstance) -> Result<(), RegistryError> {
        self.store.create_instance(instance)?;
        Ok(())
    }

    /// Test-only accessor to the resolved paths for assertions.
    #[cfg(test)]
    pub(crate) fn paths(&self) -> &EnginePaths {
        &self.paths
    }
}

/// Create `dir` (and parents) if absent, mapping failures to [`RegistryError::Io`].
fn ensure_dir(dir: &Path, name: &str) -> Result<(), RegistryError> {
    std::fs::create_dir_all(dir).map_err(|source| RegistryError::Io {
        name: name.to_string(),
        path: dir.to_string_lossy().into_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_temp() -> (TempDir, Registry) {
        let tmp = TempDir::new().unwrap();
        let reg = Registry::open(Some(tmp.path().to_path_buf())).unwrap();
        (tmp, reg)
    }

    #[test]
    fn register_happy_path_creates_home_config_row_and_empty_ledger() {
        let (_tmp, reg) = open_temp();
        let instance = reg.register("demo", "mock").unwrap();
        assert_eq!(instance.state, LifecycleState::Registered);
        assert_eq!(instance.kind, "mock");

        // Agent Home + config file exist.
        let name = InstanceName::new("demo").unwrap();
        let home = reg.paths().agent_home(&name);
        assert!(home.is_dir(), "home dir should exist");
        let config = reg.paths().instance_config(&name);
        assert!(config.is_file(), "config.toml should exist");
        let body = std::fs::read_to_string(&config).unwrap();
        assert!(body.contains("name = \"demo\""));

        // agent_home reported is the engine-computed path, and the public
        // agent_home() helper agrees with it (path-authority display helper).
        assert_eq!(instance.agent_home, home.to_string_lossy());
        assert_eq!(reg.agent_home(&name), home);

        // Row present, ledger empty.
        assert!(reg
            .list()
            .unwrap()
            .iter()
            .any(|i| i.name.as_str() == "demo"));
        assert_eq!(reg.usage_event_count(&name).unwrap(), 0);
    }

    #[test]
    fn register_invalid_name_rejected_without_touching_disk() {
        let (_tmp, reg) = open_temp();
        let err = reg.register("Bad Name", "mock").unwrap_err();
        assert!(matches!(err, RegistryError::InvalidName { .. }));
        // No agents dir entries created.
        let agents = reg.paths().agents_dir();
        assert!(!agents.join("Bad Name").exists());
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn duplicate_registration_leaves_no_partial_state() {
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        let config_before = std::fs::read_to_string(
            reg.paths()
                .instance_config(&InstanceName::new("demo").unwrap()),
        )
        .unwrap();

        let err = reg.register("demo", "different-kind").unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateName { name } if name == "demo"));

        // The original is untouched; exactly one row; config byte-identical.
        assert_eq!(reg.list().unwrap().len(), 1);
        assert!(home.is_dir());
        let config_after = std::fs::read_to_string(
            reg.paths()
                .instance_config(&InstanceName::new("demo").unwrap()),
        )
        .unwrap();
        assert_eq!(config_before, config_after);
    }

    #[test]
    fn two_instances_same_kind_get_disjoint_homes() {
        // AC3: two instances of the same kind get distinct, independent homes.
        let (_tmp, reg) = open_temp();
        reg.register("alpha", "mock").unwrap();
        reg.register("beta", "mock").unwrap();
        let alpha_home = reg.paths().agent_home(&InstanceName::new("alpha").unwrap());
        let beta_home = reg.paths().agent_home(&InstanceName::new("beta").unwrap());
        assert_ne!(alpha_home, beta_home);
        assert!(alpha_home.is_dir() && beta_home.is_dir());

        // Writing into alpha's home leaves beta's config byte-unchanged.
        let beta_config = reg
            .paths()
            .instance_config(&InstanceName::new("beta").unwrap());
        let beta_before = std::fs::read(&beta_config).unwrap();
        std::fs::write(alpha_home.join("scratch.txt"), b"hello").unwrap();
        let beta_after = std::fs::read(&beta_config).unwrap();
        assert_eq!(beta_before, beta_after);
    }

    #[test]
    fn remove_retain_keeps_home_and_deletes_row() {
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        reg.remove("demo", RemoveDisposition::Retain, false)
            .unwrap();
        assert!(home.is_dir(), "retain leaves the home on disk");
        assert!(reg.list().unwrap().is_empty(), "row is gone");
    }

    #[test]
    fn remove_delete_removes_home_and_row() {
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        reg.remove("demo", RemoveDisposition::Delete, false)
            .unwrap();
        assert!(!home.exists(), "delete removes the home tree");
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn remove_delete_isolation_other_home_byte_identical() {
        // AC4 isolation proof: removing one instance leaves every other Agent
        // Home byte-identical.
        let (_tmp, reg) = open_temp();
        reg.register("keep", "mock").unwrap();
        reg.register("drop", "mock").unwrap();
        let keep_config = reg
            .paths()
            .instance_config(&InstanceName::new("keep").unwrap());
        let before = std::fs::read(&keep_config).unwrap();

        reg.remove("drop", RemoveDisposition::Delete, false)
            .unwrap();

        let after = std::fs::read(&keep_config).unwrap();
        assert_eq!(
            before, after,
            "other home must be byte-identical after removal"
        );
        let drop_home = reg.paths().agent_home(&InstanceName::new("drop").unwrap());
        assert!(!drop_home.exists());
    }

    #[test]
    fn remove_missing_returns_not_found() {
        let (_tmp, reg) = open_temp();
        let err = reg
            .remove("ghost", RemoveDisposition::Delete, false)
            .unwrap_err();
        assert!(matches!(err, RegistryError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn remove_invalid_name_returns_invalid_name() {
        let (_tmp, reg) = open_temp();
        let err = reg
            .remove("Bad Name", RemoveDisposition::Delete, false)
            .unwrap_err();
        assert!(matches!(err, RegistryError::InvalidName { .. }));
    }

    #[test]
    fn remove_running_without_force_is_rejected() {
        // AC5: seed a running instance directly (no supervision core yet) and
        // confirm the guard fires without --force.
        let (_tmp, reg) = open_temp();
        let now = now_rfc3339();
        let running = AgentInstance {
            name: InstanceName::new("live").unwrap(),
            kind: "mock".to_string(),
            state: LifecycleState::Running,
            agent_home: reg
                .paths()
                .agent_home(&InstanceName::new("live").unwrap())
                .to_string_lossy()
                .into_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        reg.seed_instance(&running).unwrap();

        let err = reg
            .remove("live", RemoveDisposition::Delete, false)
            .unwrap_err();
        assert!(matches!(err, RegistryError::RunningRequiresForce { name } if name == "live"));
        // Still present (removal refused).
        assert_eq!(reg.list().unwrap().len(), 1);
    }

    #[test]
    fn remove_running_with_force_succeeds() {
        // AC5: --force bypasses the running-guard.
        let (_tmp, reg) = open_temp();
        let now = now_rfc3339();
        let running = AgentInstance {
            name: InstanceName::new("live").unwrap(),
            kind: "mock".to_string(),
            state: LifecycleState::Running,
            agent_home: reg
                .paths()
                .agent_home(&InstanceName::new("live").unwrap())
                .to_string_lossy()
                .into_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        reg.seed_instance(&running).unwrap();

        reg.remove("live", RemoveDisposition::Retain, true).unwrap();
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn register_rolls_back_row_when_home_cannot_be_created() {
        // Force materialize_home to fail by pre-creating a *file* where the
        // agents dir must be a directory, so create_dir_all errors.
        let tmp = TempDir::new().unwrap();
        let reg = Registry::open(Some(tmp.path().to_path_buf())).unwrap();
        // Place a regular file at <base>/agents so create_dir_all(<base>/agents/demo) fails.
        let agents = reg.paths().agents_dir();
        std::fs::write(&agents, b"not a dir").unwrap();

        let err = reg.register("demo", "mock").unwrap_err();
        assert!(matches!(err, RegistryError::Io { .. }), "got {err:?}");
        // Row must have been rolled back — the Fleet is empty.
        assert!(
            reg.list().unwrap().is_empty(),
            "row should be rolled back after home creation failure"
        );
    }

    #[test]
    fn register_rolls_back_when_config_file_cannot_be_written() {
        // Force the config *write* (not the dir creation) to fail by placing a
        // directory where config.toml must be a file. Exercises the config
        // write-failure Io branch and its rollback.
        let tmp = TempDir::new().unwrap();
        let reg = Registry::open(Some(tmp.path().to_path_buf())).unwrap();
        let name = InstanceName::new("demo").unwrap();
        // Pre-create <base>/agents/demo/config.toml AS A DIRECTORY.
        let config_as_dir = reg.paths().instance_config(&name);
        std::fs::create_dir_all(&config_as_dir).unwrap();

        let err = reg.register("demo", "mock").unwrap_err();
        assert!(matches!(err, RegistryError::Io { .. }), "got {err:?}");
        // Row rolled back.
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn register_orphan_row_when_rollback_delete_also_fails() {
        // F2 compound failure: materialize_home fails (file at the agents dir)
        // AND the rollback delete fails (delete-blocking trigger). The result
        // must be the distinct RegisterOrphanRow error naming the leaked row,
        // not the bare Io error (which would hide the orphaned row).
        let tmp = TempDir::new().unwrap();
        let reg = Registry::open(Some(tmp.path().to_path_buf())).unwrap();
        // (a) make materialize_home fail: a regular file where agents/ must be.
        let agents = reg.paths().agents_dir();
        std::fs::write(&agents, b"not a dir").unwrap();
        // (b) make the compensating delete fail deterministically.
        reg.store.break_deletes_for_test();

        let err = reg.register("demo", "mock").unwrap_err();
        match err {
            RegistryError::RegisterOrphanRow {
                name,
                home_error,
                rollback_error,
            } => {
                assert_eq!(name, "demo");
                assert!(!home_error.is_empty());
                assert!(!rollback_error.is_empty());
            }
            other => panic!("expected RegisterOrphanRow, got {other:?}"),
        }
        // The orphan row is indeed still present (the delete was blocked), which
        // is exactly the partial-failure state the error names for the operator.
        assert_eq!(reg.list().unwrap().len(), 1);
    }

    #[test]
    fn open_maps_path_resolution_failure_and_names_the_base() {
        // F8: when EnginePaths::new fails (here: a relative KTESIO_STATE_DIR),
        // Registry::open must surface RegistryError::Io whose `path` names the
        // offending base rather than being blank. Save/restore the shared env.
        let prev = std::env::var_os(crate::paths::STATE_DIR_ENV);
        std::env::set_var(crate::paths::STATE_DIR_ENV, "relative/base");
        let result = Registry::open(None);
        std::env::set_var(crate::paths::STATE_DIR_ENV, ""); // neutralize before restore
        match prev {
            Some(v) => std::env::set_var(crate::paths::STATE_DIR_ENV, v),
            None => std::env::remove_var(crate::paths::STATE_DIR_ENV),
        }
        // (Registry has no Debug impl, so inspect the Err arm without binding
        // the Ok value into a formatted panic.)
        let err = match result {
            Ok(_) => panic!("expected Io from a relative env base, got Ok"),
            Err(e) => e,
        };
        match err {
            RegistryError::Io { path, .. } => {
                // The diagnostic names the default-resolution context, not blank.
                assert!(!path.is_empty(), "path must be populated (F8)");
                assert!(path.contains(crate::paths::STATE_DIR_ENV));
            }
            other => panic!("expected Io from a relative env base, got {other:?}"),
        }
    }

    #[test]
    fn remove_delete_succeeds_when_home_already_gone() {
        // F6 TOCTOU: if the Agent Home is already absent, a Delete removal must
        // succeed (desired end state already holds) rather than fail with
        // RemoveLeftoverHome. Register, delete the home out from under the
        // engine, then remove --delete.
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        std::fs::remove_dir_all(&home).unwrap();
        assert!(!home.exists());

        // Must succeed: the home is already gone.
        reg.remove("demo", RemoveDisposition::Delete, false)
            .unwrap();
        assert!(reg.list().unwrap().is_empty(), "row is gone");
        assert!(!home.exists());
    }

    #[test]
    fn remove_delete_reports_leftover_home_on_failure() {
        // Exercise the RemoveLeftoverHome branch portably: register, then
        // replace the Agent Home directory with a regular *file*. A Delete
        // removal deletes the row, then calls remove_dir_all on a path that
        // exists but is not a directory, which errors -> RemoveLeftoverHome
        // (naming the leftover path, NFR-1).
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        std::fs::remove_dir_all(&home).unwrap();
        std::fs::write(&home, b"now a file").unwrap();

        let err = reg
            .remove("demo", RemoveDisposition::Delete, false)
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::RemoveLeftoverHome { name, .. } if name == "demo"),
            "got {err:?}"
        );
        // Row was still deleted (removal proceeds past the row).
        assert!(reg.list().unwrap().is_empty());
    }
}
