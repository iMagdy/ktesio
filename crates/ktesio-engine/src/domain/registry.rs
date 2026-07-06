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

use ktesio_adapter_api::{
    Capability, CapabilityDeclaration, EffectiveCapabilities, OsId, SupportLevel,
};

use crate::adapter::{self, AdapterRef, ResolvedAdapter};
use crate::paths::EnginePaths;
use crate::ports::{SpawnRecord, StateStore, StoreError};
use crate::store::SqliteStore;
use crate::time::now_rfc3339;

use super::error::RegistryError;
use super::instance::AgentInstance;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::restart::RestartPolicy;

/// Filename of the adapter snapshot inside an Agent Home (story 1.3).
///
/// Holds the effective (current-OS) Capability Declaration, the Metering Source,
/// and (for a manifest adapter) the manifest path 1.4 needs to launch it. JSON
/// so the engine avoids a `toml` dependency (AD-3: only adapter-api owns TOML).
/// `[ASSUMPTION]` on the filename.
const ADAPTER_SNAPSHOT_FILE: &str = "adapter.json";

/// The persisted adapter snapshot written into an Agent Home at registration.
///
/// This is the on-disk form of the resolved adapter. `kt agent show` reads it
/// back to render the effective per-OS Capability Declaration (AC1 "visible for
/// the instance") without re-resolving the adapter. Structured artifacts live
/// as files in the home (AD-6), keeping the DB lean.
///
/// ## Full declaration persisted; projection happens at READ time (F3)
///
/// The snapshot stores the **full** per-OS [`CapabilityDeclaration`], NOT a
/// single-OS projection frozen to the registering host. The effective
/// (current-OS) view is computed in [`Registry::effective_capabilities`] by
/// projecting onto [`OsId::current`] when the snapshot is read. This keeps the
/// state directory portable: a home registered on one OS and later read on
/// another projects correctly for the OS actually running, instead of returning
/// a stale projection captured at registration.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AdapterSnapshot {
    /// The adapter kind (mirrors the instance `kind` column).
    kind: String,
    /// The declared Metering Source, as its wire string.
    metering_source: String,
    /// The manifest path, present only for a manifest adapter (1.4 needs it).
    manifest_path: Option<String>,
    /// The full per-OS Capability Declaration (projected at read time).
    declaration: CapabilityDeclaration,
}

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

    /// Register a new Agent Instance under `name` of a native `kind`.
    ///
    /// Convenience wrapper over [`Registry::register_with_adapter`] for the
    /// `--kind <kind>` path: the kind now RESOLVES to a real native adapter
    /// (story 1.3) and its Capability Declaration + Metering Source are validated
    /// before any side effect. An unknown kind is rejected with
    /// [`RegistryError::UnknownAdapterKind`] and nothing is written.
    pub fn register(&self, name: &str, kind: &str) -> Result<AgentInstance, RegistryError> {
        self.register_with_adapter(name, &AdapterRef::Native(kind.to_string()))
    }

    /// Register a new Agent Instance, resolving `reference` to an adapter first.
    ///
    /// On success the instance is in [`LifecycleState::Registered`], its Agent
    /// Home exists with an instance `config.toml` and an `adapter.json` snapshot
    /// of the effective (current-OS) Capability Declaration + Metering Source
    /// (+ manifest path for a manifest adapter, which 1.4 needs to launch), and
    /// its Usage Ledger is empty. Returns the created [`AgentInstance`].
    ///
    /// ## Atomicity ordering (F2 lesson — adapter validation is a pure pre-step)
    ///
    /// 1. Validate the name (rejected here, nothing touched).
    /// 2. **Resolve + validate the adapter** — a pure, side-effect-free step. A
    ///    rejected adapter (unknown kind, missing/invalid manifest, no viable
    ///    Metering Source, no capabilities) returns HERE, before any row or
    ///    directory exists, so a rejection leaves ZERO partial state (AC2/AC4 +
    ///    the F2 orphan-row lesson).
    /// 3. Insert the DB row. The `UNIQUE` constraint detects a duplicate
    ///    atomically, still before any file is created.
    /// 4. Create the Agent Home, write `config.toml`, and write the effective
    ///    declaration snapshot.
    /// 5. If step 4 fails, delete the row and remove any partial directory, then
    ///    surface the error — leaving no orphan row and no half-created home.
    pub fn register_with_adapter(
        &self,
        name: &str,
        reference: &AdapterRef,
    ) -> Result<AgentInstance, RegistryError> {
        // (1) Validate the name.
        let name = InstanceName::new(name).map_err(|reason| RegistryError::InvalidName {
            name: name.to_string(),
            reason,
        })?;

        // (2) Resolve + validate the adapter FIRST (side-effect-free). Any
        // failure returns before a row or home is created — atomicity is
        // preserved by never starting the write. `?` maps AdapterResolveError
        // into RegistryError via the From impl (naming the section / kind).
        let resolved = adapter::resolve(reference)?;

        let home = self.paths.agent_home(&name);
        let now = now_rfc3339();
        let instance = AgentInstance {
            name: name.clone(),
            kind: resolved.kind().to_string(),
            state: LifecycleState::Registered,
            agent_home: home.to_string_lossy().into_owned(),
            created_at: now.clone(),
            updated_at: now,
        };

        // (3) Insert the row. Duplicate -> DuplicateName, nothing on disk yet.
        self.store.create_instance(&instance).map_err(|e| match e {
            StoreError::DuplicateName { name } => RegistryError::DuplicateName { name },
            other => RegistryError::Store(other),
        })?;

        // (4) Create the Agent Home + config + adapter snapshot; (5) roll back
        // on failure.
        if let Err(io_err) = self.materialize_home(&name, &resolved) {
            // Rollback: remove the row first (restoring atomicity), then any
            // partial directory. The row delete is the load-bearing step — if
            // it fails we would leak an orphan `registered` row with no home,
            // breaking the atomicity contract, so we surface that distinctly
            // (naming the orphaned row + remediation, NFR-1). The partial
            // directory cleanup is best-effort.
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

    /// The effective (current-OS) Capability Declaration for a registered
    /// instance, read back from its Agent Home snapshot (AC1 "visible for the
    /// instance"). `kt agent show` renders this.
    ///
    /// Returns [`RegistryError::NotFound`] if the instance is not registered, or
    /// [`RegistryError::Io`] if the snapshot is missing/unreadable (a corrupt
    /// home). The snapshot stores the FULL per-OS declaration; the effective view
    /// is projected onto [`OsId::current`] HERE, at read time (F3), so a home
    /// registered on one OS still projects correctly when read on another.
    pub fn effective_capabilities(
        &self,
        name: &str,
    ) -> Result<EffectiveCapabilities, RegistryError> {
        let name = InstanceName::new(name).map_err(|reason| RegistryError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        // Confirm the instance exists (distinguish NotFound from a read error).
        let _instance = self
            .store
            .get_instance(&name)?
            .ok_or_else(|| RegistryError::NotFound {
                name: name.as_str().to_string(),
            })?;

        let snapshot = self.read_adapter_snapshot(&name)?;
        // Project onto the OS actually running now (not the registering OS).
        Ok(snapshot.declaration.effective(OsId::current()))
    }

    /// Read + parse an instance's adapter snapshot from its Agent Home.
    ///
    /// Maps a missing/unreadable file or corrupt JSON to [`RegistryError::Io`]
    /// naming the snapshot path (never panics on a corrupt home).
    fn read_adapter_snapshot(&self, name: &InstanceName) -> Result<AdapterSnapshot, RegistryError> {
        let snapshot_path = self.adapter_snapshot_path(name);
        let text = std::fs::read_to_string(&snapshot_path).map_err(|source| RegistryError::Io {
            name: name.as_str().to_string(),
            path: snapshot_path.to_string_lossy().into_owned(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|e| RegistryError::Io {
            name: name.as_str().to_string(),
            path: snapshot_path.to_string_lossy().into_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })
    }

    /// Absolute path to an instance's adapter snapshot file inside its home.
    fn adapter_snapshot_path(&self, name: &InstanceName) -> std::path::PathBuf {
        self.paths.agent_home(name).join(ADAPTER_SNAPSHOT_FILE)
    }

    /// Create the Agent Home directory, write the instance `config.toml`, and
    /// write the adapter snapshot (`adapter.json`).
    ///
    /// `[ASSUMPTION]` the filenames `config.toml` / `adapter.json` and a minimal
    /// instance-level config body. Full layered config resolution is Epic 2;
    /// here we persist the instance layer plus the effective declaration so
    /// "the effective Capability Declaration is visible for the instance" (AC1)
    /// holds. Writing the snapshot inside `materialize_home` keeps it covered by
    /// the same registration rollback (AD-6 atomicity).
    fn materialize_home(
        &self,
        name: &InstanceName,
        resolved: &ResolvedAdapter,
    ) -> Result<(), RegistryError> {
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

        // Persist the FULL per-OS declaration + metering source (+ manifest path
        // for a manifest adapter). The effective (current-OS) view is projected
        // at read time (F3), so the snapshot stays OS-portable. JSON keeps the
        // engine free of a `toml` dependency (AD-3: only adapter-api owns TOML).
        let snapshot = AdapterSnapshot {
            kind: resolved.kind().to_string(),
            metering_source: resolved.metering_source().as_str().to_string(),
            manifest_path: resolved
                .manifest_path()
                .map(|p| p.to_string_lossy().into_owned()),
            declaration: resolved.declaration().clone(),
        };
        let snapshot_path = self.adapter_snapshot_path(name);
        let json = serde_json::to_string_pretty(&snapshot).map_err(|e| RegistryError::Io {
            name: name.as_str().to_string(),
            path: snapshot_path.to_string_lossy().into_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })?;
        std::fs::write(&snapshot_path, json).map_err(|source| RegistryError::Io {
            name: name.as_str().to_string(),
            path: snapshot_path.to_string_lossy().into_owned(),
            source,
        })?;
        Ok(())
    }

    /// Remove an Agent Instance, honoring the retain/delete disposition (AC4)
    /// and the running-guard (AC5).
    ///
    /// ## Running-guard (AC5) + process teardown (AI-11)
    ///
    /// This method is the RECORD + Agent-Home half of remove: it validates the
    /// state-machine guard — if the stored Lifecycle State is `running` and
    /// `force` is false, it returns [`RegistryError::RunningRequiresForce`] — then
    /// deletes the row (FK-cascading the write-ahead spawn record) and, for
    /// `Delete`, the Agent Home. It does NOT touch live processes: stopping a
    /// live/adopted instance's process BEFORE the row is deleted (so `remove` never
    /// leaves an unsupervised orphan — the AI-11 invariant, for both plain and
    /// `--force` remove) is the caller's job and lives in [`Engine::remove`], which
    /// holds the supervisor's in-memory handle map. Tests that exercise the guard
    /// here still seed a `running`/`paused` row directly via the store (no live
    /// process needed) — the guard is pure state-machine validation.
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

    // ---- Supervisor collaboration surface (story 1.4; crate-internal) ----
    //
    // The lifecycle supervisor drives transitions through these; they stay
    // `pub(crate)` so the public Embedding Interface is only the Engine facade,
    // not the raw store. All speak domain types (AD-1).

    /// Look up an instance, mapping absence to [`RegistryError::NotFound`].
    pub(crate) fn lookup(&self, name: &InstanceName) -> Result<AgentInstance, RegistryError> {
        self.store
            .get_instance(name)?
            .ok_or_else(|| RegistryError::NotFound {
                name: name.as_str().to_string(),
            })
    }

    /// Persist a Lifecycle State change (supervisor transition). Bumps
    /// `updated_at` in the store.
    pub(crate) fn set_state(
        &self,
        name: &InstanceName,
        state: LifecycleState,
    ) -> Result<(), RegistryError> {
        self.store.set_state(name, state)?;
        Ok(())
    }

    /// The persisted adapter facts the supervisor needs to launch an instance:
    /// its kind and (for a manifest adapter) the manifest path 1-3 recorded.
    pub(crate) fn adapter_launch_facts(
        &self,
        name: &InstanceName,
    ) -> Result<(String, Option<std::path::PathBuf>), RegistryError> {
        let snapshot = self.read_adapter_snapshot(name)?;
        let manifest_path = snapshot.manifest_path.map(std::path::PathBuf::from);
        Ok((snapshot.kind, manifest_path))
    }

    /// The effective (current-OS) [`SupportLevel`] for `capability` on an
    /// instance (story 1-5, AC5). Reads the persisted [`AdapterSnapshot`]'s FULL
    /// per-OS declaration and projects it onto [`OsId::current`] at READ time (the
    /// F3 mechanism `effective_capabilities` uses) — it does NOT re-parse the
    /// manifest or re-resolve the adapter, and it does NOT freeze a level at
    /// register time. This is THE read the supervisor uses to pick the pause
    /// dispatch level. An absent declaration for the current OS projects to
    /// [`SupportLevel::Unsupported`] — the honest default (a manifest that omits
    /// pause for this OS correctly fails fast).
    pub(crate) fn effective_support(
        &self,
        name: &InstanceName,
        capability: Capability,
    ) -> Result<SupportLevel, RegistryError> {
        let snapshot = self.read_adapter_snapshot(name)?;
        Ok(snapshot.declaration.support(capability, OsId::current()))
    }

    // ---- Write-ahead spawn records + restart policy (story 1-6, AD-5/AD-6) ----
    //
    // Thin `pub(crate)` pass-throughs to the store's spawn-record methods (same
    // pattern as `set_state` → `store.set_state`). Each store call is one
    // transaction (AD-6). The supervisor commits the record BEFORE declaring an
    // instance supervised, clears it on a clean stop, and reads every record on
    // engine start to reconcile orphans.

    /// Commit a write-ahead spawn record (AD-5) — one transaction. Called by the
    /// supervisor between `spawn` and declaring the instance `running`.
    pub(crate) fn write_spawn_record(&self, record: &SpawnRecord) -> Result<(), RegistryError> {
        self.store.upsert_spawn_record(record)?;
        Ok(())
    }

    /// Clear an instance's write-ahead spawn record (a clean stop, so it is not
    /// later adopted/failed as an orphan). Idempotent.
    pub(crate) fn clear_spawn_record(&self, name: &InstanceName) -> Result<(), RegistryError> {
        self.store.clear_spawn_record(name)?;
        Ok(())
    }

    /// Read an instance's write-ahead spawn record, or `None` if absent.
    pub(crate) fn spawn_record(
        &self,
        name: &InstanceName,
    ) -> Result<Option<SpawnRecord>, RegistryError> {
        Ok(self.store.get_spawn_record(name)?)
    }

    /// List every write-ahead spawn record (the orphan-reconcile input on engine
    /// start).
    pub(crate) fn list_spawn_records(&self) -> Result<Vec<SpawnRecord>, RegistryError> {
        Ok(self.store.list_spawn_records()?)
    }

    /// Persist a new restart count + last-known cause for an instance (a restart
    /// bump or a reset) — one transaction. No-op if the instance has no record.
    pub(crate) fn set_restart_count(
        &self,
        name: &InstanceName,
        restart_count: u32,
        last_known_cause: Option<&str>,
    ) -> Result<(), RegistryError> {
        self.store
            .set_restart_count(name, restart_count, last_known_cause)?;
        Ok(())
    }

    /// Set the per-instance [`RestartPolicy`] (story 1-6, AC4 "per-instance
    /// configurable") — the config SEED. Persists via the store (creating a
    /// minimal policy-only record if the instance was never started). One
    /// transaction (AD-6).
    pub(crate) fn set_restart_policy(
        &self,
        name: &InstanceName,
        policy: RestartPolicy,
    ) -> Result<(), RegistryError> {
        self.store.set_restart_policy(name, policy)?;
        Ok(())
    }

    /// The effective per-instance [`RestartPolicy`] (story 1-6, AC4/AC9).
    ///
    /// RECOMMENDED seed (AD-9 layered TOML config is Epic 2): read the policy from
    /// the persisted spawn record; when there is no record yet (or its policy is
    /// absent), fall back to the AD-15 default ([`RestartPolicy::default`] =
    /// `on-failure`). This is a per-instance SOURCE (the DB), not a value
    /// hard-wired at the call site.
    pub(crate) fn effective_restart_policy(
        &self,
        name: &InstanceName,
    ) -> Result<RestartPolicy, RegistryError> {
        Ok(self
            .store
            .get_spawn_record(name)?
            .map(|r| r.restart_policy)
            .unwrap_or_default())
    }

    /// The per-instance log directory inside the Agent Home (AD-12 seed).
    pub(crate) fn instance_log_dir(&self, name: &InstanceName) -> std::path::PathBuf {
        self.paths.agent_home(name).join("logs")
    }

    /// The per-instance ENGINE log FILE — the JSON-Lines transition-event log
    /// (AD-12/AD-14 seed). Engine-owned; distinct from the agent's own
    /// stdout/stderr capture so the two never interleave.
    pub(crate) fn instance_log_path(&self, name: &InstanceName) -> std::path::PathBuf {
        self.instance_log_dir(name).join("instance.log")
    }

    /// The per-instance AGENT output log FILE — the spawned process's
    /// stdout/stderr capture (AD-12 seed). Kept separate from the engine event
    /// log; full agent-out/agent-err attribution + rotation is Epic 4.
    pub(crate) fn agent_output_log_path(&self, name: &InstanceName) -> std::path::PathBuf {
        self.instance_log_dir(name).join("agent.log")
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

        // Re-register the same NAME (kind must still resolve, so reuse `mock`);
        // the UNIQUE-name constraint yields DuplicateName after adapter
        // resolution passes.
        let err = reg.register("demo", "mock").unwrap_err();
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

    // ---- Story 1.3: adapter resolution + validation at registration ----

    const VALID_MANIFEST: &str = r#"
contract_version = "0.1.0"

[adapter]
kind = "demo-manifest"

[lifecycle.start]
exec = "demo-agent"

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[metering]
source = "self-reported"
"#;

    fn write_manifest_dir(dir: &std::path::Path, body: &str) {
        std::fs::write(dir.join(crate::adapter::MANIFEST_FILE), body).unwrap();
    }

    #[test]
    fn register_native_mock_persists_effective_declaration() {
        // AC1: the mock kind resolves; the effective (current-OS) declaration is
        // persisted and re-readable via effective_capabilities.
        let (_tmp, reg) = open_temp();
        let instance = reg.register("demo", "mock").unwrap();
        assert_eq!(instance.kind, "mock");

        // The adapter snapshot exists in the home.
        let name = InstanceName::new("demo").unwrap();
        let snap = reg.paths().agent_home(&name).join(ADAPTER_SNAPSHOT_FILE);
        assert!(snap.is_file(), "adapter.json snapshot should exist");

        let eff = reg.effective_capabilities("demo").unwrap();
        assert_eq!(eff.os, OsId::current());
        // The mock declares pause + interaction, so both project onto this OS.
        assert_eq!(eff.entries.len(), 2);
    }

    #[test]
    fn register_unknown_kind_leaves_no_partial_state() {
        // AC2 + atomicity: an unknown native kind is rejected BEFORE any row or
        // home is created.
        let (_tmp, reg) = open_temp();
        let err = reg.register("demo", "does-not-exist").unwrap_err();
        assert!(
            matches!(&err, RegistryError::UnknownAdapterKind { kind } if kind == "does-not-exist"),
            "got {err:?}"
        );
        assert!(reg.list().unwrap().is_empty(), "no row on rejected adapter");
        let home = reg.paths().agent_home(&InstanceName::new("demo").unwrap());
        assert!(!home.exists(), "no home on rejected adapter");
    }

    #[test]
    fn register_manifest_from_dir_succeeds_and_records_effective_declaration() {
        // AC1: register a manifest adapter from a temp dir; read back the
        // effective declaration.
        let (_tmp, reg) = open_temp();
        let manifest_dir = TempDir::new().unwrap();
        write_manifest_dir(manifest_dir.path(), VALID_MANIFEST);

        let instance = reg
            .register_with_adapter(
                "m",
                &AdapterRef::Manifest(manifest_dir.path().to_path_buf()),
            )
            .unwrap();
        assert_eq!(instance.kind, "demo-manifest");

        let eff = reg.effective_capabilities("m").unwrap();
        assert_eq!(eff.os, OsId::current());
        assert!(!eff.is_empty());

        // The snapshot records the manifest path for 1.4.
        let name = InstanceName::new("m").unwrap();
        let snap_text =
            std::fs::read_to_string(reg.paths().agent_home(&name).join(ADAPTER_SNAPSHOT_FILE))
                .unwrap();
        assert!(
            snap_text.contains(crate::adapter::MANIFEST_FILE),
            "snapshot should record the manifest path: {snap_text}"
        );
    }

    #[test]
    fn register_manifest_not_found_leaves_no_partial_state() {
        let (_tmp, reg) = open_temp();
        let empty = TempDir::new().unwrap();
        let err = reg
            .register_with_adapter("m", &AdapterRef::Manifest(empty.path().to_path_buf()))
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::ManifestNotFound { path } if path.ends_with(crate::adapter::MANIFEST_FILE)),
            "got {err:?}"
        );
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn register_manifest_invalid_names_the_section_and_leaves_no_partial_state() {
        // AC2: a manifest missing [capabilities] is rejected naming the section,
        // with zero partial state.
        let (_tmp, reg) = open_temp();
        let manifest_dir = TempDir::new().unwrap();
        let body = VALID_MANIFEST.replace(
            "[capabilities.pause]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"best-effort\"\n",
            "",
        );
        write_manifest_dir(manifest_dir.path(), &body);

        let err = reg
            .register_with_adapter(
                "m",
                &AdapterRef::Manifest(manifest_dir.path().to_path_buf()),
            )
            .unwrap_err();
        match &err {
            RegistryError::ManifestInvalid { detail, .. } => {
                assert!(detail.contains("[capabilities]"), "detail={detail}")
            }
            other => panic!("expected ManifestInvalid, got {other:?}"),
        }
        assert!(reg.list().unwrap().is_empty());
        let home = reg.paths().agent_home(&InstanceName::new("m").unwrap());
        assert!(!home.exists());
    }

    #[test]
    fn register_manifest_no_metering_is_rejected_ac4() {
        // AC4: a manifest with no [metering] section is rejected at
        // registration; no "register anyway" path. The diagnostic names
        // [metering], and nothing is written.
        let (_tmp, reg) = open_temp();
        let manifest_dir = TempDir::new().unwrap();
        let body = VALID_MANIFEST.replace("[metering]\nsource = \"self-reported\"\n", "");
        write_manifest_dir(manifest_dir.path(), &body);

        let err = reg
            .register_with_adapter(
                "m",
                &AdapterRef::Manifest(manifest_dir.path().to_path_buf()),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("[metering]"),
            "AC4 diagnostic must name [metering]: {err}"
        );
        assert!(reg.list().unwrap().is_empty());
    }

    #[test]
    fn effective_capabilities_unknown_instance_is_not_found() {
        let (_tmp, reg) = open_temp();
        let err = reg.effective_capabilities("ghost").unwrap_err();
        assert!(matches!(&err, RegistryError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn effective_capabilities_invalid_name_is_invalid_name() {
        let (_tmp, reg) = open_temp();
        let err = reg.effective_capabilities("Bad Name").unwrap_err();
        assert!(matches!(err, RegistryError::InvalidName { .. }));
    }

    #[test]
    fn effective_capabilities_missing_snapshot_reports_io() {
        // A registered instance whose snapshot file was removed (corrupt home)
        // surfaces an Io error naming the missing snapshot path, not a panic.
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let name = InstanceName::new("demo").unwrap();
        let snap = reg.paths().agent_home(&name).join(ADAPTER_SNAPSHOT_FILE);
        std::fs::remove_file(&snap).unwrap();

        let err = reg.effective_capabilities("demo").unwrap_err();
        assert!(
            matches!(&err, RegistryError::Io { path, .. } if path.ends_with(ADAPTER_SNAPSHOT_FILE)),
            "got {err:?}"
        );
    }

    #[test]
    fn effective_capabilities_corrupt_snapshot_reports_io() {
        // A snapshot that is not valid JSON surfaces an Io(InvalidData) error
        // rather than panicking.
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let name = InstanceName::new("demo").unwrap();
        let snap = reg.paths().agent_home(&name).join(ADAPTER_SNAPSHOT_FILE);
        std::fs::write(&snap, b"{ not valid json").unwrap();

        let err = reg.effective_capabilities("demo").unwrap_err();
        assert!(matches!(err, RegistryError::Io { .. }), "got {err:?}");
    }

    #[test]
    fn adapter_snapshot_round_trips_through_json() {
        // F3: the persisted snapshot carries the FULL per-OS declaration and
        // round-trips (kind, metering source, manifest path, declaration).
        use ktesio_adapter_api::{Capability, SupportLevel};
        let declaration = CapabilityDeclaration::new()
            .with(Capability::Pause, OsId::Linux, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Windows, SupportLevel::BestEffort)
            .with(
                Capability::Interaction,
                OsId::Macos,
                SupportLevel::Guaranteed,
            );
        let snapshot = AdapterSnapshot {
            kind: "demo".to_string(),
            metering_source: "self-reported".to_string(),
            manifest_path: Some("/some/adapter.toml".to_string()),
            declaration: declaration.clone(),
        };
        let json = serde_json::to_string_pretty(&snapshot).unwrap();
        let back: AdapterSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, "demo");
        assert_eq!(back.metering_source, "self-reported");
        assert_eq!(back.manifest_path.as_deref(), Some("/some/adapter.toml"));
        // The full declaration survives, all three OS entries intact.
        assert_eq!(back.declaration, declaration);
        assert_eq!(
            back.declaration.support(Capability::Pause, OsId::Windows),
            SupportLevel::BestEffort
        );
    }

    #[test]
    fn effective_capabilities_projects_at_read_time_not_register_time() {
        // F3: a snapshot persisted with a FULL declaration whose pause level
        // differs on every modeled OS must project onto the CURRENTLY running OS
        // at read time — never a level frozen to some other (registering) OS.
        // We hand-write the snapshot (simulating "registered as if on any OS")
        // and confirm effective_capabilities returns the current-OS level.
        use ktesio_adapter_api::{Capability, SupportLevel};
        let (_tmp, reg) = open_temp();
        // Register normally to create the row + home, then overwrite the snapshot
        // with a full declaration that distinguishes every OS.
        reg.register("demo", "mock").unwrap();
        let name = InstanceName::new("demo").unwrap();

        let declaration = CapabilityDeclaration::new()
            .with(Capability::Pause, OsId::Linux, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Macos, SupportLevel::BestEffort)
            .with(Capability::Pause, OsId::Windows, SupportLevel::Unsupported);
        let snapshot = AdapterSnapshot {
            kind: "demo".to_string(),
            metering_source: "self-reported".to_string(),
            manifest_path: None,
            declaration,
        };
        let snap_path = reg.adapter_snapshot_path(&name);
        std::fs::write(&snap_path, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();

        let eff = reg.effective_capabilities("demo").unwrap();
        assert_eq!(eff.os, OsId::current(), "projection is for the running OS");
        // The pause level returned must equal the current OS's declared level —
        // proving the projection happened at read time from the full map.
        let expected = match OsId::current() {
            OsId::Linux => SupportLevel::Guaranteed,
            OsId::Macos => SupportLevel::BestEffort,
            OsId::Windows => SupportLevel::Unsupported,
            OsId::Other => SupportLevel::Unsupported,
        };
        let pause = eff
            .entries
            .iter()
            .find(|(c, _)| *c == Capability::Pause)
            .map(|(_, l)| *l);
        // On Other, pause has no entry → not present; on modeled OSes it matches.
        if OsId::current() == OsId::Other {
            assert!(pause.is_none() || pause == Some(SupportLevel::Unsupported));
        } else {
            assert_eq!(pause, Some(expected), "read-time projection for current OS");
        }
    }

    #[test]
    fn effective_support_reads_the_current_os_pause_level_at_read_time() {
        // AC5 (1-5): effective_support projects the FULL persisted declaration
        // onto OsId::current() at READ time. Overwrite the snapshot with a
        // declaration that distinguishes every OS and assert the current OS's
        // level is returned — proving it is read, not re-derived or frozen.
        use ktesio_adapter_api::{Capability, SupportLevel};
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let name = InstanceName::new("demo").unwrap();

        let declaration = CapabilityDeclaration::new()
            .with(Capability::Pause, OsId::Linux, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Macos, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Windows, SupportLevel::BestEffort);
        let snapshot = AdapterSnapshot {
            kind: "demo".to_string(),
            metering_source: "self-reported".to_string(),
            manifest_path: None,
            declaration,
        };
        std::fs::write(
            reg.adapter_snapshot_path(&name),
            serde_json::to_string_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let level = reg.effective_support(&name, Capability::Pause).unwrap();
        let expected = match OsId::current() {
            OsId::Linux | OsId::Macos => SupportLevel::Guaranteed,
            OsId::Windows => SupportLevel::BestEffort,
            OsId::Other => SupportLevel::Unsupported,
        };
        assert_eq!(level, expected, "read-time projection for the current OS");
    }

    #[test]
    fn effective_support_defaults_to_unsupported_when_capability_absent_for_this_os() {
        // AC5/AC3 default: a declaration that omits pause for the current OS
        // projects to Unsupported (the honest default that drives fail-fast).
        use ktesio_adapter_api::{Capability, SupportLevel};
        let (_tmp, reg) = open_temp();
        reg.register("demo", "mock").unwrap();
        let name = InstanceName::new("demo").unwrap();

        // Declare pause only for OsId::Other so no modeled host has an entry.
        let declaration = CapabilityDeclaration::new().with(
            Capability::Interaction,
            OsId::current(),
            SupportLevel::Guaranteed,
        );
        let snapshot = AdapterSnapshot {
            kind: "demo".to_string(),
            metering_source: "self-reported".to_string(),
            manifest_path: None,
            declaration,
        };
        std::fs::write(
            reg.adapter_snapshot_path(&name),
            serde_json::to_string_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let level = reg.effective_support(&name, Capability::Pause).unwrap();
        assert_eq!(level, SupportLevel::Unsupported);
    }

    #[test]
    fn register_manifest_rejection_when_adapter_toml_is_a_directory() {
        // Defensive: if adapter.toml exists but is a directory, reading it fails
        // with an I/O error and is surfaced as ManifestUnreadable (its own
        // variant — F4), leaving no partial state.
        let (_tmp, reg) = open_temp();
        let manifest_dir = TempDir::new().unwrap();
        std::fs::create_dir(manifest_dir.path().join(crate::adapter::MANIFEST_FILE)).unwrap();
        let err = reg
            .register_with_adapter(
                "m",
                &AdapterRef::Manifest(manifest_dir.path().to_path_buf()),
            )
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::ManifestUnreadable { path, .. } if path.ends_with(crate::adapter::MANIFEST_FILE)),
            "got {err:?}"
        );
        assert!(reg.list().unwrap().is_empty());
    }
}
