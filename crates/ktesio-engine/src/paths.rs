//! Engine path authority (spine "Filesystem layout" convention).
//!
//! The engine is the SOLE path authority: the state-dir location and the
//! Agent Home layout are computed only here. `kt`, adapters, and Hosts receive
//! paths from the API and never construct them.
//!
//! ## cfg-free rule (CRITICAL)
//!
//! The state-dir base is resolved through the [`directories`] crate, which
//! hides every OS-conditional compilation attribute inside itself. This engine
//! code therefore stays free of platform `#[cfg]` attributes, satisfying the
//! OS-cfg CI gate (which allows such attributes only under `src/backends/`).
//! Do NOT hand-roll platform branches here — reach for `directories`.
//!
//! ## Base-dir resolution order
//!
//! 1. An explicit override passed to [`EnginePaths::new`] (tests pass a
//!    `TempDir`; the registry facade threads it through).
//! 2. Else the `KTESIO_STATE_DIR` environment variable, if set — this makes
//!    `kt` integration tests (which spawn the real binary) hermetic, mirroring
//!    the existing `KTESIO_NO_UPDATE_CHECK` / `XDG_CACHE_HOME` precedent.
//! 3. Else the platform data dir via `ProjectDirs::from("", "", "ktesio")`.
//!
//! ## Layout (`[ASSUMPTION]`: exact names not spine-fixed)
//!
//! Recorded CURRENT (Q-4 ruling: this module OWNS the Agent Home layout doc —
//! every story that adds an entry records it here in the same commit):
//!
//! ```text
//! <state_base>/
//!   state.db                 # the one SQLite state store (AD-6)
//!   secrets.toml             # the engine-SHARED 0600 secret store (AD-10; optional)
//!   agents/
//!     <instance_name>/       # one Agent Home per instance
//!       config.toml          # instance-level config (AD-9)
//!       adapter.json         # persisted adapter snapshot (story 1-3)
//!       effective-config.json  # resolved config + per-value provenance, written
//!                            #   at START, overwritten every start (story 2-3)
//!       <rendered files>     # native config FILE targets of a manifest `[config]`
//!                            #   mapping (story 2-2; paths are adapter-declared,
//!                            #   validated relative to the home)
//!       logs/                # per-instance logs (AD-12): instance.log (JSON-Lines
//!                            #   transitions), agent.log (raw stdout capture),
//!                            #   agent-stderr.log (raw stderr capture),
//!                            #   output.log[.1|.2] (attributed, rotated, story 4-2),
//!                            #   breaches.log (JSON-Lines budget breaches, story 3-2)
//!       memory/              # the managed Memory Backing directory (story 5-1,
//!                            #   spine AD-11 — engine-managed, survives restarts
//!                            #   byte-identically; contents are OPERATOR data the
//!                            #   engine never touches)
//! ```

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::domain::InstanceName;

/// Environment override for the state-dir base (integration-test hermeticity).
pub const STATE_DIR_ENV: &str = "KTESIO_STATE_DIR";

/// File name of the SQLite state store inside the state base. `[ASSUMPTION]`
pub const STATE_DB_FILE: &str = "state.db";

/// File name of the engine SECRETS store inside the state base (story 2-4, spine
/// AD-10 "the engine secrets file, mode 0600"). A state-dir-level file (NOT
/// per-Agent-Home — it is the engine's SHARED secret store, resolving every
/// instance's `secret:NAME` references), beside [`STATE_DB_FILE`]. `[ASSUMPTION]`
/// recorded (Assumption 5): TOML `NAME = "value"` (reuses the engine's `toml`
/// dep), at `<state base>/secrets.toml`, expected mode `0600` (owner-only —
/// enforced on Unix by the backend permission check, AD-4). It is NOT a SQLite
/// blob (AD-6): secrets are files under path authority, never a DB column.
pub const SECRETS_FILE: &str = "secrets.toml";

/// Directory (under the state base) that holds all Agent Homes. `[ASSUMPTION]`
pub const AGENTS_DIR: &str = "agents";

/// File name of the per-instance config file inside an Agent Home. `[ASSUMPTION]`
pub const INSTANCE_CONFIG_FILE: &str = "config.toml";

/// File name of the persisted effective-config snapshot inside an Agent Home
/// (story 2-3, spine AD-9 "the effective-config snapshot persisted in the Agent
/// Home" + AD-6 "effective-config snapshots are files inside the Agent Home").
/// Written at START (the resolved four-layer config + per-value provenance),
/// OVERWRITTEN every start/restart. `[ASSUMPTION]` recorded (Decision 5): JSON,
/// mirroring the `adapter.json` snapshot convention — OS-portable, serializes the
/// provenance tags cleanly, and kept DISTINCT from the editable `config.toml`
/// (this file is engine-owned, read-only-to-humans, never hand-edited).
pub const EFFECTIVE_CONFIG_SNAPSHOT_FILE: &str = "effective-config.json";

/// Directory name of the managed Memory Backing inside an Agent Home (story 5-1,
/// spine AD-11 "`filesystem` — engine-managed directory inside the Agent Home;
/// survives restarts byte-identically"). The ONE true name for the directory: the
/// path is computed only through [`EnginePaths::agent_memory_dir`] (path
/// authority, conventions row) and `kt`/adapters/Hosts never join this segment
/// themselves. The engine CREATES the directory (attach + a defensive start-time
/// self-heal, each one idempotent `create_dir_all`) but NEVER touches its
/// CONTENTS — they are operator data that must survive byte-identically (DC-7).
pub const MEMORY_DIR: &str = "memory";

/// Computes engine-owned paths from a resolved state-dir base.
///
/// Construct with [`EnginePaths::new`]; every path method derives from the
/// single stored base, so there is no global or thread-local state (this keeps
/// the API facade-friendly for the async migration in story 1.4 and satisfies
/// FR-34 "no global-state collisions").
#[derive(Clone, Debug)]
pub struct EnginePaths {
    state_base: PathBuf,
}

/// Reasons the state-dir base could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// No override, no `KTESIO_STATE_DIR`, and the platform data dir could not
    /// be determined (e.g. no `HOME` on Unix).
    #[error("could not determine a state directory; set {STATE_DIR_ENV} to an explicit path")]
    NoStateDir,

    /// `KTESIO_STATE_DIR` was set to a relative path. A relative base would be
    /// resolved against the current working directory — a non-portable,
    /// surprising state location that would also get baked into the stored
    /// absolute Agent Home paths — so we reject it with an explicit error.
    #[error(
        "{STATE_DIR_ENV} must be an absolute path, but was '{value}'; set it to an absolute path"
    )]
    RelativeStateDir {
        /// The offending (relative) value.
        value: String,
    },
}

impl EnginePaths {
    /// Resolve the state-dir base and build an [`EnginePaths`].
    ///
    /// `override_base`:
    /// * `Some(path)` — use it verbatim (tests / explicit embedding).
    /// * `None` — consult `KTESIO_STATE_DIR`, then the platform data dir.
    pub fn new(override_base: Option<PathBuf>) -> Result<Self, PathError> {
        let state_base = match override_base {
            Some(base) => base,
            None => match std::env::var_os(STATE_DIR_ENV) {
                Some(env_base) if !env_base.is_empty() => {
                    let base = PathBuf::from(env_base);
                    // Reject a relative env-provided base: it would resolve
                    // CWD-relative and leak a non-portable path into the stored
                    // Agent Home paths. An explicit override (the Some arm) is
                    // trusted; the environment is not.
                    if !base.is_absolute() {
                        return Err(PathError::RelativeStateDir {
                            value: base.to_string_lossy().into_owned(),
                        });
                    }
                    base
                }
                _ => ProjectDirs::from("", "", "ktesio")
                    .map(|dirs| dirs.data_dir().to_path_buf())
                    .ok_or(PathError::NoStateDir)?,
            },
        };
        Ok(Self { state_base })
    }

    /// The resolved state-dir base (holds the DB and the `agents/` tree).
    pub fn state_base(&self) -> &Path {
        &self.state_base
    }

    /// Absolute path to the SQLite state store.
    pub fn state_db(&self) -> PathBuf {
        self.state_base.join(STATE_DB_FILE)
    }

    /// Absolute path to the engine secrets file (story 2-4, AD-10) — the
    /// state-dir-level TOML `NAME = "value"` store the 0600-file
    /// [`crate::ports::SecretResolver`] reads. Mirrors [`state_db`](Self::state_db);
    /// the engine is the SOLE path authority (AD-6). The file is optional (a
    /// missing secrets file is not an error — env may resolve every reference);
    /// only its PRESENCE triggers the permission check + lookup.
    pub fn secrets_file(&self) -> PathBuf {
        self.state_base.join(SECRETS_FILE)
    }

    /// Directory holding all Agent Homes.
    pub fn agents_dir(&self) -> PathBuf {
        self.state_base.join(AGENTS_DIR)
    }

    /// Absolute Agent Home directory for `name` (keyed by the unique name).
    ///
    /// Two distinct names always yield two distinct directories — the
    /// isolation guarantee behind FR-2 / AC3.
    pub fn agent_home(&self, name: &InstanceName) -> PathBuf {
        self.agents_dir().join(name.as_str())
    }

    /// Absolute path to an Agent Home's instance config file.
    pub fn instance_config(&self, name: &InstanceName) -> PathBuf {
        self.agent_home(name).join(INSTANCE_CONFIG_FILE)
    }

    /// Absolute path to an Agent Home's persisted effective-config snapshot
    /// (story 2-3, AD-9/AD-6). The engine is the SOLE writer (path authority);
    /// `kt`/Hosts/adapters read it back but never construct the path. Mirrors
    /// [`instance_config`](Self::instance_config), rooted at the same Agent Home.
    pub fn effective_config_snapshot(&self, name: &InstanceName) -> PathBuf {
        self.agent_home(name).join(EFFECTIVE_CONFIG_SNAPSHOT_FILE)
    }

    /// Absolute path to an Agent Home's managed Memory Backing directory (story
    /// 5-1, spine AD-11). The engine is the SOLE path authority: `kt` receives the
    /// path from the public API and never constructs it; adapters receive it via
    /// the reserved unified-config key injected at start. Mirrors
    /// [`effective_config_snapshot`](Self::effective_config_snapshot), rooted at
    /// the same Agent Home.
    pub fn agent_memory_dir(&self, name: &InstanceName) -> PathBuf {
        self.agent_home(name).join(MEMORY_DIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn name(s: &str) -> InstanceName {
        InstanceName::new(s).unwrap()
    }

    #[test]
    fn override_base_is_used_verbatim() {
        let tmp = TempDir::new().unwrap();
        let paths = EnginePaths::new(Some(tmp.path().to_path_buf())).unwrap();
        assert_eq!(paths.state_base(), tmp.path());
        assert_eq!(paths.state_db(), tmp.path().join("state.db"));
        assert_eq!(paths.agents_dir(), tmp.path().join("agents"));
        // Story 2-4: the engine secrets file is a state-dir-level file beside the
        // state DB (NOT per-Agent-Home), named secrets.toml.
        assert_eq!(paths.secrets_file(), tmp.path().join("secrets.toml"));
    }

    #[test]
    fn two_names_get_disjoint_homes() {
        let tmp = TempDir::new().unwrap();
        let paths = EnginePaths::new(Some(tmp.path().to_path_buf())).unwrap();
        let a = paths.agent_home(&name("alpha"));
        let b = paths.agent_home(&name("beta"));
        assert_ne!(a, b);
        assert!(a.ends_with("agents/alpha"));
        assert!(b.ends_with("agents/beta"));
        // Config file lives inside the home.
        assert_eq!(paths.instance_config(&name("alpha")), a.join("config.toml"));
        // Story 2-3: the effective-config snapshot also lives inside the home,
        // as effective-config.json, distinct from the editable config.toml.
        assert_eq!(
            paths.effective_config_snapshot(&name("alpha")),
            a.join("effective-config.json")
        );
        // Story 5-1: the managed Memory Backing directory lives inside the home
        // as memory/ (path authority — one const, one accessor).
        assert_eq!(paths.agent_memory_dir(&name("alpha")), a.join("memory"));
    }

    #[test]
    fn env_override_is_honored_when_no_explicit_base() {
        // Guard against parallel tests racing on the shared process env by
        // using a unique value and restoring afterwards.
        let tmp = TempDir::new().unwrap();
        let prev = std::env::var_os(STATE_DIR_ENV);
        std::env::set_var(STATE_DIR_ENV, tmp.path());
        let paths = EnginePaths::new(None).unwrap();
        assert_eq!(paths.state_base(), tmp.path());
        match prev {
            Some(v) => std::env::set_var(STATE_DIR_ENV, v),
            None => std::env::remove_var(STATE_DIR_ENV),
        }
    }

    #[test]
    fn relative_env_base_is_rejected() {
        // F7: a relative KTESIO_STATE_DIR must be refused (it would resolve
        // CWD-relative and leak a non-portable path). Save/restore the shared
        // env var like the sibling test.
        let prev = std::env::var_os(STATE_DIR_ENV);
        std::env::set_var(STATE_DIR_ENV, "relative/state/dir");
        let err = EnginePaths::new(None).unwrap_err();
        match prev {
            Some(v) => std::env::set_var(STATE_DIR_ENV, v),
            None => std::env::remove_var(STATE_DIR_ENV),
        }
        assert!(
            matches!(&err, PathError::RelativeStateDir { value } if value == "relative/state/dir"),
            "got {err:?}"
        );
    }

    #[test]
    fn explicit_relative_override_is_trusted() {
        // The explicit Some(base) override is trusted verbatim even if relative
        // (embedding/tests own it); only the env-provided base is rejected.
        let paths = EnginePaths::new(Some(PathBuf::from("relative/base"))).unwrap();
        assert_eq!(paths.state_base(), Path::new("relative/base"));
    }
}
