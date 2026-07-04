//! SQLite [`StateStore`] implementation (spine AD-6).
//!
//! One database holds all registry + lifecycle state. Opened with the ratified
//! pragmas: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`.
//! SQLite is bundled (compiled from source via `libsqlite3-sys`) so there is
//! no system-SQLite dependency — good for Windows CI.
//!
//! ## Schema & migration
//!
//! Schema version is tracked with `PRAGMA user_version` (chosen over a `_meta`
//! table for simplicity — no extra table, atomic with the connection). On open:
//! `user_version == 0` → apply schema v1 → set `user_version = 1`. Reopening an
//! existing DB is idempotent (version already 1 → no DDL runs). A future story
//! adds v2 by checking the version and stepping.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::domain::{AgentInstance, InstanceName, LifecycleState};
use crate::ports::{StateStore, StoreError};

/// Current schema version applied by this build.
const SCHEMA_VERSION: i64 = 1;

/// Schema v1 DDL: registry+lifecycle table and the append-only Usage Ledger.
///
/// `usage_events` is created EMPTY this story; its shape is frozen per AD-7's
/// minimum UsageEvent fields so story 3.1 (which populates it) needs no
/// breaking migration. `ON DELETE CASCADE` + `foreign_keys=ON` makes removing
/// an instance clean up its ledger rows automatically.
const SCHEMA_V1: &str = "\
CREATE TABLE agent_instances (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL,
    state        TEXT NOT NULL,
    agent_home   TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE TABLE usage_events (
    id              INTEGER PRIMARY KEY,
    instance_id     INTEGER NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
    run_id          TEXT NOT NULL,
    input_tokens    INTEGER NOT NULL,
    output_tokens   INTEGER NOT NULL,
    metering_source TEXT NOT NULL,
    occurred_at     TEXT NOT NULL
);
CREATE INDEX idx_usage_events_instance ON usage_events(instance_id);
";

/// A SQLite-backed state store over a single connection.
///
/// One handle owns one connection. This story is single-threaded per handle
/// (no `Send + Sync` sharing requirement yet); story 1.4 decides pooling when
/// the engine goes async and blocking DB calls move to a blocking pool.
pub struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    /// Open (creating if absent) the DB at `path`, set pragmas, and migrate.
    ///
    /// The parent directory must already exist — the registry facade creates
    /// the state dir before opening. Idempotent across reopens.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(backend)?;
        Self::configure(&conn)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory DB (unit tests that do not need a file). Migrated.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(backend)?;
        Self::configure(&conn)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Apply the AD-6 connection pragmas.
    ///
    /// `foreign_keys` is OFF by default in SQLite and must be enabled on every
    /// connection. `journal_mode=WAL` is a persistent DB property but is set
    /// here so a freshly created file adopts it immediately.
    fn configure(conn: &Connection) -> Result<(), StoreError> {
        // WAL returns the resulting mode as a row; query_row consumes it.
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(backend)?;
        if !mode.eq_ignore_ascii_case("wal") {
            // In-memory DBs report "memory" and cannot use WAL; that is fine
            // for tests. A file DB that failed to enter WAL is a real problem.
            if !mode.eq_ignore_ascii_case("memory") {
                return Err(StoreError::Backend(format!(
                    "expected WAL journal mode, got '{mode}'"
                )));
            }
        }
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(backend)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(backend)?;
        Ok(())
    }

    /// Look up the internal row id for a name (used to scope ledger queries).
    fn instance_id(&self, name: &InstanceName) -> Result<Option<i64>, StoreError> {
        self.conn
            .query_row(
                "SELECT id FROM agent_instances WHERE name = ?1",
                [name.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(backend)
    }

    /// Test-only fault injection: install a `BEFORE DELETE` trigger that aborts
    /// every delete on `agent_instances`, so [`StateStore::delete_instance`]
    /// fails deterministically. Used to exercise the registration rollback's
    /// orphan-row branch (the compensating delete failing) portably, without a
    /// mock store. Inserts still succeed.
    #[cfg(test)]
    pub(crate) fn break_deletes_for_test(&self) {
        self.conn
            .execute_batch(
                "CREATE TRIGGER block_delete BEFORE DELETE ON agent_instances \
                 BEGIN SELECT RAISE(ABORT, 'delete blocked for test'); END",
            )
            .expect("install delete-blocking trigger");
    }
}

/// Run any outstanding migrations to bring the DB to [`SCHEMA_VERSION`].
///
/// Forward compatibility: a DB whose `user_version` is *ahead* of
/// [`SCHEMA_VERSION`] was written by a newer ktesio. We refuse it with
/// [`StoreError::SchemaTooNew`] rather than skip the DDL and stamp the version
/// back down — the old behavior silently downgraded a forward schema.
fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(backend)?;

    if version > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }

    // Up-migration path only (version < SCHEMA_VERSION). A DB already at
    // SCHEMA_VERSION needs neither DDL nor a version bump (idempotent reopen).
    if version < SCHEMA_VERSION {
        conn.execute_batch(SCHEMA_V1).map_err(backend)?;
        // pragma_update cannot bind user_version; format the constant in. It is
        // a compile-time integer, so this is injection-safe.
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
            .map_err(backend)?;
    }
    Ok(())
}

/// Map an arbitrary `rusqlite::Error` into a backend [`StoreError`].
fn backend(err: rusqlite::Error) -> StoreError {
    StoreError::Backend(err.to_string())
}

/// Classify an insert failure: a `UNIQUE` (or primary-key) constraint
/// violation becomes [`StoreError::DuplicateName`]; any other constraint
/// violation (or non-constraint failure) falls through to a backend error.
///
/// Matching the *extended* result code (not just the primary
/// `ConstraintViolation` class) keeps unrelated constraint failures — a
/// future `CHECK`, `NOT NULL`, or foreign-key violation — from masquerading
/// as a duplicate-name error.
fn classify_insert(err: rusqlite::Error, name: &str) -> StoreError {
    if let rusqlite::Error::SqliteFailure(inner, _) = &err {
        let ext = inner.extended_code;
        if ext == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            || ext == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        {
            return StoreError::DuplicateName {
                name: name.to_string(),
            };
        }
    }
    backend(err)
}

/// Build an [`AgentInstance`] from a result row, decoding domain types.
fn row_to_instance(row: &rusqlite::Row<'_>) -> Result<AgentInstance, StoreError> {
    let name_raw: String = row.get("name").map_err(backend)?;
    let state_raw: String = row.get("state").map_err(backend)?;
    let name = InstanceName::new(name_raw.clone()).map_err(|e| StoreError::CorruptRow {
        name: name_raw.clone(),
        detail: format!("invalid stored name: {e}"),
    })?;
    let state = LifecycleState::from_wire(&state_raw).ok_or_else(|| StoreError::CorruptRow {
        name: name_raw.clone(),
        detail: format!("unknown lifecycle state '{state_raw}'"),
    })?;
    Ok(AgentInstance {
        name,
        kind: row.get("kind").map_err(backend)?,
        state,
        agent_home: row.get("agent_home").map_err(backend)?,
        created_at: row.get("created_at").map_err(backend)?,
        updated_at: row.get("updated_at").map_err(backend)?,
    })
}

impl StateStore for SqliteStore {
    fn create_instance(&self, instance: &AgentInstance) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO agent_instances \
                 (name, kind, state, agent_home, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    instance.name.as_str(),
                    instance.kind,
                    instance.state.as_str(),
                    instance.agent_home,
                    instance.created_at,
                    instance.updated_at,
                ],
            )
            .map_err(|e| classify_insert(e, instance.name.as_str()))?;
        Ok(())
    }

    fn set_state(&self, name: &InstanceName, state: LifecycleState) -> Result<(), StoreError> {
        // Persist the new Lifecycle State (as its wire string) and bump
        // updated_at. A row-count of 0 means the instance is gone → NotFound.
        let affected = self
            .conn
            .execute(
                "UPDATE agent_instances SET state = ?1, updated_at = ?2 WHERE name = ?3",
                rusqlite::params![state.as_str(), crate::time::now_rfc3339(), name.as_str(),],
            )
            .map_err(backend)?;
        if affected == 0 {
            return Err(StoreError::NotFound {
                name: name.as_str().to_string(),
            });
        }
        Ok(())
    }

    fn get_instance(&self, name: &InstanceName) -> Result<Option<AgentInstance>, StoreError> {
        self.conn
            .query_row(
                "SELECT name, kind, state, agent_home, created_at, updated_at \
                 FROM agent_instances WHERE name = ?1",
                [name.as_str()],
                |row| Ok(row_to_instance(row)),
            )
            .optional()
            .map_err(backend)?
            .transpose()
    }

    fn list_instances(&self) -> Result<Vec<AgentInstance>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, kind, state, agent_home, created_at, updated_at \
                 FROM agent_instances ORDER BY name",
            )
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| Ok(row_to_instance(row)))
            .map_err(backend)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(backend)??);
        }
        Ok(out)
    }

    fn delete_instance(&self, name: &InstanceName) -> Result<(), StoreError> {
        let affected = self
            .conn
            .execute(
                "DELETE FROM agent_instances WHERE name = ?1",
                [name.as_str()],
            )
            .map_err(backend)?;
        if affected == 0 {
            return Err(StoreError::NotFound {
                name: name.as_str().to_string(),
            });
        }
        Ok(())
    }

    fn count_usage_events(&self, name: &InstanceName) -> Result<u64, StoreError> {
        // Scope by the instance's row id. An absent instance has zero events.
        let Some(id) = self.instance_id(name)? else {
            return Ok(0);
        };
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE instance_id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(backend)?;
        Ok(count.max(0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn name(s: &str) -> InstanceName {
        InstanceName::new(s).unwrap()
    }

    fn sample(n: &str, kind: &str, home: &str) -> AgentInstance {
        AgentInstance {
            name: name(n),
            kind: kind.to_string(),
            state: LifecycleState::Registered,
            agent_home: home.to_string(),
            created_at: "2026-07-03T00:00:00Z".to_string(),
            updated_at: "2026-07-03T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn create_get_list_delete_round_trip() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert!(store.list_instances().unwrap().is_empty());

        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        let got = store.get_instance(&name("demo")).unwrap().unwrap();
        assert_eq!(got.kind, "mock");
        assert_eq!(got.state, LifecycleState::Registered);
        assert_eq!(got.agent_home, "/x/agents/demo");

        store
            .create_instance(&sample("other", "mock", "/x/agents/other"))
            .unwrap();
        let list = store.list_instances().unwrap();
        // Ordered by name.
        assert_eq!(
            list.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            vec!["demo", "other"]
        );

        store.delete_instance(&name("demo")).unwrap();
        assert!(store.get_instance(&name("demo")).unwrap().is_none());
        assert_eq!(store.list_instances().unwrap().len(), 1);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = SqliteStore::open_in_memory().unwrap();
        assert!(store.get_instance(&name("nope")).unwrap().is_none());
    }

    #[test]
    fn set_state_updates_the_state_column() {
        // Story 1.4: the supervisor persists lifecycle transitions via set_state.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        assert_eq!(
            store.get_instance(&name("demo")).unwrap().unwrap().state,
            LifecycleState::Registered
        );
        store
            .set_state(&name("demo"), LifecycleState::Running)
            .unwrap();
        assert_eq!(
            store.get_instance(&name("demo")).unwrap().unwrap().state,
            LifecycleState::Running
        );
        // A further transition also persists.
        store
            .set_state(&name("demo"), LifecycleState::Stopped)
            .unwrap();
        assert_eq!(
            store.get_instance(&name("demo")).unwrap().unwrap().state,
            LifecycleState::Stopped
        );
    }

    #[test]
    fn set_state_on_missing_instance_is_not_found() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = store
            .set_state(&name("ghost"), LifecycleState::Running)
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("dup", "mock", "/x/agents/dup"))
            .unwrap();
        let err = store
            .create_instance(&sample("dup", "other", "/x/agents/dup"))
            .unwrap_err();
        match err {
            StoreError::DuplicateName { name } => assert_eq!(name, "dup"),
            other => panic!("expected DuplicateName, got {other:?}"),
        }
    }

    #[test]
    fn delete_missing_returns_not_found() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = store.delete_instance(&name("ghost")).unwrap_err();
        assert!(matches!(err, StoreError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn empty_ledger_counts_zero() {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        assert_eq!(store.count_usage_events(&name("demo")).unwrap(), 0);
        // Absent instance also counts zero (no row id).
        assert_eq!(store.count_usage_events(&name("absent")).unwrap(), 0);
    }

    #[test]
    fn usage_events_cascade_on_delete() {
        // Prove ON DELETE CASCADE + foreign_keys=ON: seed a ledger row, delete
        // the instance, and confirm the ledger row is gone.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        let id = store.instance_id(&name("demo")).unwrap().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO usage_events \
                 (instance_id, run_id, input_tokens, output_tokens, metering_source, occurred_at) \
                 VALUES (?1, 'run-1', 10, 20, 'self-reported', '2026-07-03T00:00:00Z')",
                [id],
            )
            .unwrap();
        assert_eq!(store.count_usage_events(&name("demo")).unwrap(), 1);
        store.delete_instance(&name("demo")).unwrap();
        let remaining: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn wal_pragma_is_set_on_file_db() {
        let tmp = TempDir::new().unwrap();
        let store = SqliteStore::open(&tmp.path().join("state.db")).unwrap();
        let mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode={mode}");
        let fk: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys should be ON");
    }

    #[test]
    fn migration_is_idempotent_on_reopen() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("state.db");
        {
            let store = SqliteStore::open(&db).unwrap();
            store
                .create_instance(&sample("demo", "mock", "/x/agents/demo"))
                .unwrap();
        }
        // Reopen: must NOT re-run DDL (that would error "table exists") and the
        // existing row must survive.
        let reopened = SqliteStore::open(&db).unwrap();
        let version: i64 = reopened
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(reopened.get_instance(&name("demo")).unwrap().is_some());
    }

    #[test]
    fn newer_schema_db_is_refused_not_downgraded() {
        // Forward-compat guard: a DB whose user_version is ahead of this build
        // (e.g. written by a future ktesio with schema v2) must be refused with
        // SchemaTooNew, NOT silently stamped back to v1.
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("state.db");
        {
            // Materialize a valid v1 DB first, then bump user_version to 2 to
            // simulate a newer schema on disk.
            let store = SqliteStore::open(&db).unwrap();
            store.conn.execute_batch("PRAGMA user_version = 2").unwrap();
        }
        // Reopening must refuse rather than downgrade. (SqliteStore has no
        // Debug impl, so destructure the Result rather than unwrap_err.)
        let err = match SqliteStore::open(&db) {
            Ok(_) => panic!("expected SchemaTooNew, got Ok"),
            Err(e) => e,
        };
        assert!(
            matches!(err, StoreError::SchemaTooNew { found, supported }
                if found == 2 && supported == SCHEMA_VERSION),
            "got {err:?}"
        );
        // And the version on disk is untouched (not downgraded to 1).
        let probe = Connection::open(&db).unwrap();
        let version: i64 = probe
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2, "user_version must not be downgraded");
    }

    #[test]
    fn corrupt_name_row_is_reported() {
        // A stored name that violates the newtype rule is flagged CorruptRow on
        // read (the name-decode branch), not silently accepted.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO agent_instances \
                 (name, kind, state, agent_home, created_at, updated_at) \
                 VALUES ('Bad Name', 'mock', 'registered', '/x', '2026-07-03T00:00:00Z', '2026-07-03T00:00:00Z')",
                [],
            )
            .unwrap();
        let err = store.list_instances().unwrap_err();
        assert!(
            matches!(&err, StoreError::CorruptRow { name, detail }
                if name == "Bad Name" && detail.contains("invalid stored name")),
            "got {err:?}"
        );
    }

    #[test]
    fn non_constraint_insert_error_maps_to_backend() {
        // Drop the table so an insert fails with "no such table" — a
        // non-constraint SqliteFailure — exercising the classify_insert
        // fallthrough and the backend() mapper.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .conn
            .execute_batch("DROP TABLE agent_instances")
            .unwrap();
        let err = store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap_err();
        assert!(matches!(err, StoreError::Backend(_)), "got {err:?}");
    }

    #[test]
    fn non_unique_constraint_maps_to_backend_not_duplicate() {
        // A CHECK constraint violation is a ConstraintViolation *class* error
        // but NOT a UNIQUE/primary-key one; classify_insert must route it to
        // Backend, not mis-report it as DuplicateName.
        let store = SqliteStore::open_in_memory().unwrap();
        // A helper table whose only constraint is a CHECK that always fails.
        store
            .conn
            .execute_batch("CREATE TABLE check_probe (n INTEGER CHECK (n > 0))")
            .unwrap();
        let raw = store
            .conn
            .execute("INSERT INTO check_probe (n) VALUES (0)", [])
            .unwrap_err();
        // The extended code is SQLITE_CONSTRAINT_CHECK, not _UNIQUE.
        let mapped = classify_insert(raw, "demo");
        assert!(matches!(mapped, StoreError::Backend(_)), "got {mapped:?}");
    }

    #[test]
    fn corrupt_state_row_is_reported() {
        // Directly write an unknown lifecycle state, then confirm reads flag it
        // as CorruptRow rather than silently mis-decoding.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO agent_instances \
                 (name, kind, state, agent_home, created_at, updated_at) \
                 VALUES ('demo', 'mock', 'teleporting', '/x', '2026-07-03T00:00:00Z', '2026-07-03T00:00:00Z')",
                [],
            )
            .unwrap();
        let err = store.get_instance(&name("demo")).unwrap_err();
        assert!(matches!(err, StoreError::CorruptRow { name, detail }
                if name == "demo" && detail.contains("teleporting")),);
    }
}
