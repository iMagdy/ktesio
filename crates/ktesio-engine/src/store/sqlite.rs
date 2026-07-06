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
//! table for simplicity — no extra table, atomic with the connection). On open,
//! the migrator STEPS from the DB's current `user_version` up to
//! [`SCHEMA_VERSION`], applying each version's DDL in order and stamping the new
//! version. Reopening an existing DB is idempotent (already at the target → no
//! DDL runs). Story 1-6 adds v2: the `agent_runtime` write-ahead spawn-record
//! table (AD-5/AD-6). A DB ahead of this build is refused (forward-compat guard).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use crate::domain::{AgentInstance, InstanceName, LifecycleState, RestartPolicy};
use crate::ports::{ProcessFingerprint, SpawnRecord, StateStore, StoreError};

/// Current schema version applied by this build.
///
/// v1: registry + lifecycle + Usage Ledger. v2 (story 1-6): the `agent_runtime`
/// write-ahead spawn-record table (AD-5).
const SCHEMA_VERSION: i64 = 2;

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

/// Schema v2 DDL (story 1-6): the write-ahead spawn-record table (spine AD-5).
///
/// One row per SUPERVISED instance, keyed by `instance_id` (UNIQUE, FK with
/// `ON DELETE CASCADE` so removing an instance drops its record). Holds the
/// process fingerprint (`pid` + opaque `start_time`), the per-instance Restart
/// Policy (AD-15), the consecutive-failure `restart_count` (survives an engine
/// restart), and the `last_known_cause`. The row EXISTS only while the instance
/// is supervised (`running`/`paused`); a clean stop deletes it, so a
/// normally-stopped instance is never later adopted/failed as an orphan.
const SCHEMA_V2: &str = "\
CREATE TABLE agent_runtime (
    id               INTEGER PRIMARY KEY,
    instance_id      INTEGER NOT NULL UNIQUE REFERENCES agent_instances(id) ON DELETE CASCADE,
    pid              INTEGER NOT NULL,
    start_time       INTEGER NOT NULL,
    restart_policy   TEXT NOT NULL,
    restart_count    INTEGER NOT NULL DEFAULT 0,
    last_known_cause TEXT
);
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

    /// Test-only fault injection: DROP the `agent_instances` table so every
    /// subsequent read/write of it fails with a SQL error. Used to exercise the
    /// config surface's store-error arms (`require_instance`'s
    /// [`StoreError`]-mapping branch) deterministically, without a mock store —
    /// mirrors [`SqliteStore::break_deletes_for_test`]. Irreversible for the
    /// connection; call it last in a test.
    #[cfg(test)]
    pub(crate) fn break_instance_reads_for_test(&self) {
        self.conn
            .execute_batch("DROP TABLE agent_instances")
            .expect("drop agent_instances for test");
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

    // Step up one version at a time, applying each version's DDL in order. A DB
    // already at SCHEMA_VERSION runs no DDL (idempotent reopen). Each step is
    // additive; a partially-migrated DB from a crashed migration re-runs only
    // the steps it still needs.
    if version < 1 {
        conn.execute_batch(SCHEMA_V1).map_err(backend)?;
    }
    if version < 2 {
        conn.execute_batch(SCHEMA_V2).map_err(backend)?;
    }

    if version < SCHEMA_VERSION {
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

    fn upsert_spawn_record(&self, record: &SpawnRecord) -> Result<(), StoreError> {
        // Resolve the instance row id (the FK). An absent instance is NotFound.
        let id = self
            .instance_id(&record.name)?
            .ok_or_else(|| StoreError::NotFound {
                name: record.name.as_str().to_string(),
            })?;
        // Insert-or-replace on the UNIQUE instance_id, in one statement (AD-6:
        // one transaction per event — a single INSERT ... ON CONFLICT is atomic).
        self.conn
            .execute(
                "INSERT INTO agent_runtime \
                 (instance_id, pid, start_time, restart_policy, restart_count, last_known_cause) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(instance_id) DO UPDATE SET \
                 pid = excluded.pid, start_time = excluded.start_time, \
                 restart_policy = excluded.restart_policy, \
                 restart_count = excluded.restart_count, \
                 last_known_cause = excluded.last_known_cause",
                rusqlite::params![
                    id,
                    record.fingerprint.pid as i64,
                    record.fingerprint.start_time as i64,
                    record.restart_policy.as_str(),
                    record.restart_count as i64,
                    record.last_known_cause,
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn clear_spawn_record(&self, name: &InstanceName) -> Result<(), StoreError> {
        // Idempotent: clearing an absent record (or an absent instance) is
        // success — the desired end state (no record) already holds.
        let Some(id) = self.instance_id(name)? else {
            return Ok(());
        };
        self.conn
            .execute("DELETE FROM agent_runtime WHERE instance_id = ?1", [id])
            .map_err(backend)?;
        Ok(())
    }

    fn get_spawn_record(&self, name: &InstanceName) -> Result<Option<SpawnRecord>, StoreError> {
        let Some(id) = self.instance_id(name)? else {
            return Ok(None);
        };
        self.conn
            .query_row(
                "SELECT pid, start_time, restart_policy, restart_count, last_known_cause \
                 FROM agent_runtime WHERE instance_id = ?1",
                [id],
                |row| Ok(row_to_spawn_record(name.clone(), row)),
            )
            .optional()
            .map_err(backend)?
            .transpose()
    }

    fn list_spawn_records(&self) -> Result<Vec<SpawnRecord>, StoreError> {
        // Join to the instance name (the domain key), ordered by name.
        let mut stmt = self
            .conn
            .prepare(
                "SELECT i.name, r.pid, r.start_time, r.restart_policy, r.restart_count, \
                 r.last_known_cause \
                 FROM agent_runtime r JOIN agent_instances i ON i.id = r.instance_id \
                 ORDER BY i.name",
            )
            .map_err(backend)?;
        let rows = stmt
            .query_map([], |row| {
                let name_raw: String = row.get("name")?;
                let name =
                    InstanceName::new(name_raw.clone()).map_err(|e| StoreError::CorruptRow {
                        name: name_raw,
                        detail: format!("invalid stored name: {e}"),
                    });
                Ok(name.and_then(|n| row_to_spawn_record(n, row)))
            })
            .map_err(backend)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(backend)??);
        }
        Ok(out)
    }

    fn set_restart_count(
        &self,
        name: &InstanceName,
        restart_count: u32,
        last_known_cause: Option<&str>,
    ) -> Result<(), StoreError> {
        // No-op if the instance has no spawn record (e.g. it was cleanly stopped
        // between the crash and this update). One UPDATE = one transaction.
        let Some(id) = self.instance_id(name)? else {
            return Ok(());
        };
        self.conn
            .execute(
                "UPDATE agent_runtime SET restart_count = ?1, last_known_cause = ?2 \
                 WHERE instance_id = ?3",
                rusqlite::params![restart_count as i64, last_known_cause, id],
            )
            .map_err(backend)?;
        Ok(())
    }

    fn set_restart_policy(
        &self,
        name: &InstanceName,
        policy: RestartPolicy,
    ) -> Result<(), StoreError> {
        // The instance must exist (the FK). An absent instance is NotFound.
        let id = self
            .instance_id(name)?
            .ok_or_else(|| StoreError::NotFound {
                name: name.as_str().to_string(),
            })?;
        // Upsert on UNIQUE(instance_id): if a record exists, update ONLY the
        // policy (preserving pid/start_time/count/cause); if none exists, create a
        // minimal record carrying just the policy (zero fingerprint, count 0) so
        // the per-instance config persists before the first start. On the insert
        // path we set pid/start_time to 0 (a not-yet-supervised placeholder;
        // `start` overwrites them with the real fingerprint).
        self.conn
            .execute(
                "INSERT INTO agent_runtime \
                 (instance_id, pid, start_time, restart_policy, restart_count, last_known_cause) \
                 VALUES (?1, 0, 0, ?2, 0, NULL) \
                 ON CONFLICT(instance_id) DO UPDATE SET restart_policy = excluded.restart_policy",
                rusqlite::params![id, policy.as_str()],
            )
            .map_err(backend)?;
        Ok(())
    }
}

/// Build a [`SpawnRecord`] from a result row (the `pid, start_time,
/// restart_policy, restart_count, last_known_cause` columns), decoding the
/// policy wire form and clamping the integer columns into domain types.
fn row_to_spawn_record(
    name: InstanceName,
    row: &rusqlite::Row<'_>,
) -> Result<SpawnRecord, StoreError> {
    let pid: i64 = row.get("pid").map_err(backend)?;
    let start_time: i64 = row.get("start_time").map_err(backend)?;
    let policy_raw: String = row.get("restart_policy").map_err(backend)?;
    let restart_count: i64 = row.get("restart_count").map_err(backend)?;
    let last_known_cause: Option<String> = row.get("last_known_cause").map_err(backend)?;
    let restart_policy =
        RestartPolicy::from_wire(&policy_raw).ok_or_else(|| StoreError::CorruptRow {
            name: name.as_str().to_string(),
            detail: format!("unknown restart policy '{policy_raw}'"),
        })?;
    Ok(SpawnRecord {
        name,
        fingerprint: ProcessFingerprint::new(pid.max(0) as u32, start_time.max(0) as u64),
        restart_policy,
        restart_count: restart_count.max(0) as u32,
        last_known_cause,
    })
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
        // AD-6 / story-1.7 AC-C durability substrate: WAL + synchronous=NORMAL is
        // what bounds the crash loss window to ≤1s (one committed transaction per
        // state mutation). synchronous=NORMAL reports as 1. Epic 1 persists
        // lifecycle state per-event, so the loss window already holds; the same
        // bound governs the Usage Ledger once Epic 3 adds `usage_events`.
        let sync: i64 = store
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            sync, 1,
            "synchronous should be NORMAL (=1) for the ≤1s bound"
        );
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
            // Materialize a valid current-schema DB, then bump user_version
            // ABOVE the current version to simulate a newer schema on disk.
            let store = SqliteStore::open(&db).unwrap();
            store
                .conn
                .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
                .unwrap();
        }
        // Reopening must refuse rather than downgrade. (SqliteStore has no
        // Debug impl, so destructure the Result rather than unwrap_err.)
        let err = match SqliteStore::open(&db) {
            Ok(_) => panic!("expected SchemaTooNew, got Ok"),
            Err(e) => e,
        };
        assert!(
            matches!(err, StoreError::SchemaTooNew { found, supported }
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION),
            "got {err:?}"
        );
        // And the version on disk is untouched (not downgraded).
        let probe = Connection::open(&db).unwrap();
        let version: i64 = probe
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            version,
            SCHEMA_VERSION + 1,
            "user_version must not be downgraded"
        );
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

    // ---- Story 1-6: write-ahead spawn records (AD-5/AD-6) ----

    fn record(n: &str, pid: u32, start: u64, count: u32) -> SpawnRecord {
        SpawnRecord {
            name: name(n),
            fingerprint: ProcessFingerprint::new(pid, start),
            restart_policy: RestartPolicy::OnFailure,
            restart_count: count,
            last_known_cause: None,
        }
    }

    #[test]
    fn spawn_record_round_trips_and_clears() {
        // AD-5: write a record, read it back identically, then clear it.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        assert!(store.get_spawn_record(&name("demo")).unwrap().is_none());

        let rec = record("demo", 4321, 987_654, 0);
        store.upsert_spawn_record(&rec).unwrap();
        let back = store.get_spawn_record(&name("demo")).unwrap().unwrap();
        assert_eq!(back, rec);

        // Clear (a clean stop) → gone, and clearing again is idempotent.
        store.clear_spawn_record(&name("demo")).unwrap();
        assert!(store.get_spawn_record(&name("demo")).unwrap().is_none());
        store.clear_spawn_record(&name("demo")).unwrap(); // idempotent
    }

    #[test]
    fn upsert_replaces_an_existing_record() {
        // The UNIQUE(instance_id) upsert replaces on conflict (a re-spawn or a
        // restart re-arming the record).
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        store
            .upsert_spawn_record(&record("demo", 1, 100, 0))
            .unwrap();
        store
            .upsert_spawn_record(&record("demo", 2, 200, 3))
            .unwrap();
        let back = store.get_spawn_record(&name("demo")).unwrap().unwrap();
        assert_eq!(back.fingerprint, ProcessFingerprint::new(2, 200));
        assert_eq!(back.restart_count, 3);
        // Exactly one row (replaced, not duplicated).
        let all = store.list_spawn_records().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn list_spawn_records_is_ordered_by_name() {
        let store = SqliteStore::open_in_memory().unwrap();
        for n in ["beta", "alpha", "gamma"] {
            store
                .create_instance(&sample(n, "mock", &format!("/x/agents/{n}")))
                .unwrap();
            store.upsert_spawn_record(&record(n, 10, 20, 0)).unwrap();
        }
        let names: Vec<String> = store
            .list_spawn_records()
            .unwrap()
            .iter()
            .map(|r| r.name.as_str().to_string())
            .collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn upsert_spawn_record_for_missing_instance_is_not_found() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = store
            .upsert_spawn_record(&record("ghost", 1, 1, 0))
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn set_restart_count_updates_count_and_cause() {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        store
            .upsert_spawn_record(&record("demo", 5, 50, 0))
            .unwrap();
        store
            .set_restart_count(&name("demo"), 2, Some("crashed with code 1"))
            .unwrap();
        let back = store.get_spawn_record(&name("demo")).unwrap().unwrap();
        assert_eq!(back.restart_count, 2);
        assert_eq!(
            back.last_known_cause.as_deref(),
            Some("crashed with code 1")
        );
        // set_restart_count on an instance with no record is a harmless no-op.
        store
            .create_instance(&sample("norecord", "mock", "/x/agents/norecord"))
            .unwrap();
        store.set_restart_count(&name("norecord"), 9, None).unwrap();
        assert!(store.get_spawn_record(&name("norecord")).unwrap().is_none());
    }

    #[test]
    fn set_restart_policy_creates_then_updates_the_seed() {
        // AC4 per-instance config: setting the policy before a start creates a
        // policy-only record (pid 0); a later start-written record + a re-set
        // update only the policy, preserving the fingerprint + count.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        // No record yet → effective policy defaults (read via get_spawn_record).
        assert!(store.get_spawn_record(&name("demo")).unwrap().is_none());

        // Seed `never` before any start: creates a minimal policy-only record.
        store
            .set_restart_policy(&name("demo"), RestartPolicy::Never)
            .unwrap();
        let seed = store.get_spawn_record(&name("demo")).unwrap().unwrap();
        assert_eq!(seed.restart_policy, RestartPolicy::Never);
        assert_eq!(seed.fingerprint, ProcessFingerprint::new(0, 0));
        assert_eq!(seed.restart_count, 0);

        // A start writes the real fingerprint + count; then re-setting the policy
        // updates ONLY the policy.
        store
            .upsert_spawn_record(&record("demo", 99, 999, 4))
            .unwrap();
        store
            .set_restart_policy(&name("demo"), RestartPolicy::OnFailure)
            .unwrap();
        let back = store.get_spawn_record(&name("demo")).unwrap().unwrap();
        assert_eq!(back.restart_policy, RestartPolicy::OnFailure);
        assert_eq!(back.fingerprint, ProcessFingerprint::new(99, 999));
        assert_eq!(back.restart_count, 4, "count preserved across a policy set");
    }

    #[test]
    fn set_restart_policy_for_missing_instance_is_not_found() {
        let store = SqliteStore::open_in_memory().unwrap();
        let err = store
            .set_restart_policy(&name("ghost"), RestartPolicy::Never)
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { name } if name == "ghost"));
    }

    #[test]
    fn set_restart_count_on_an_instance_without_a_record_is_a_noop() {
        // The no-record branch: setting a count for an instance that has no spawn
        // record (e.g. cleanly stopped between crash and update) is a harmless
        // no-op — no row is created, no error.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        store
            .set_restart_count(&name("demo"), 3, Some("ignored"))
            .unwrap();
        assert!(store.get_spawn_record(&name("demo")).unwrap().is_none());
        // Also a no-op for a wholly-absent instance.
        store.set_restart_count(&name("absent"), 1, None).unwrap();
    }

    #[test]
    fn list_spawn_records_flags_a_corrupt_name_row() {
        // The corrupt-name branch in list_spawn_records: a stored name that
        // violates the newtype rule is flagged CorruptRow on read.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO agent_instances \
                 (name, kind, state, agent_home, created_at, updated_at) \
                 VALUES ('Bad Name', 'mock', 'running', '/x', '2026-07-03T00:00:00Z', '2026-07-03T00:00:00Z')",
                [],
            )
            .unwrap();
        let id: i64 = store
            .conn
            .query_row(
                "SELECT id FROM agent_instances WHERE name = 'Bad Name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO agent_runtime (instance_id, pid, start_time, restart_policy, restart_count) \
                 VALUES (?1, 1, 1, 'on-failure', 0)",
                [id],
            )
            .unwrap();
        let err = store.list_spawn_records().unwrap_err();
        assert!(
            matches!(&err, StoreError::CorruptRow { name, .. } if name == "Bad Name"),
            "got {err:?}"
        );
    }

    #[test]
    fn spawn_record_cascades_on_instance_delete() {
        // Removing the instance drops its runtime row (FK ON DELETE CASCADE).
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        store
            .upsert_spawn_record(&record("demo", 7, 70, 1))
            .unwrap();
        store.delete_instance(&name("demo")).unwrap();
        let remaining: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM agent_runtime", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0, "runtime row must cascade-delete");
    }

    #[test]
    fn corrupt_restart_policy_row_is_reported() {
        // A stored policy the domain cannot decode is flagged CorruptRow on read.
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .create_instance(&sample("demo", "mock", "/x/agents/demo"))
            .unwrap();
        let id = store.instance_id(&name("demo")).unwrap().unwrap();
        store
            .conn
            .execute(
                "INSERT INTO agent_runtime (instance_id, pid, start_time, restart_policy, restart_count) \
                 VALUES (?1, 1, 1, 'teleport-on-failure', 0)",
                [id],
            )
            .unwrap();
        let err = store.get_spawn_record(&name("demo")).unwrap_err();
        assert!(
            matches!(&err, StoreError::CorruptRow { name, detail }
                if name == "demo" && detail.contains("teleport-on-failure")),
            "got {err:?}"
        );
    }

    #[test]
    fn migration_v1_db_upgrades_to_v2_preserving_rows() {
        // A DB written at schema v1 (no agent_runtime table) must upgrade to v2
        // on open — the step migration adds the table WITHOUT dropping the v1
        // rows. Simulate a v1 DB by creating the v1 schema + a row, then reopen.
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("state.db");
        {
            let conn = Connection::open(&db).unwrap();
            SqliteStore::configure(&conn).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            conn.execute(
                "INSERT INTO agent_instances \
                 (name, kind, state, agent_home, created_at, updated_at) \
                 VALUES ('legacy', 'mock', 'registered', '/x', '2026-07-03T00:00:00Z', '2026-07-03T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        // Reopen: migrator steps 1 → 2, adds agent_runtime, keeps the row.
        let store = SqliteStore::open(&db).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(store.get_instance(&name("legacy")).unwrap().is_some());
        // The new table exists and is usable.
        store
            .upsert_spawn_record(&record("legacy", 3, 30, 0))
            .unwrap();
        assert!(store.get_spawn_record(&name("legacy")).unwrap().is_some());
    }
}
