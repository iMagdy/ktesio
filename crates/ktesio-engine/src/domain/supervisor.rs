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
//! ## Single-lifetime boundary (AD-5 is story 1-6)
//!
//! The running-handle map lives only for this engine's lifetime. Cross-restart
//! orphan adoption (a new engine re-attaching to processes started by a previous
//! one, AD-5) is story 1-6; here a process is supervised only while THIS engine
//! runs. State the boundary explicitly.
//!
//! ## What "an event" is here (AD-14 seed)
//!
//! Each transition RECORDS a [`TransitionEvent`] to the per-instance log and
//! returns it (observable to tests / embedders). This is NOT the 7-2 bounded
//! subscription bus — only the seed struct + its recording.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::adapter::{self, LaunchResolveError};
use crate::backends;
use crate::ports::{BackendError, ProcessBackend, ProcessStatus, SpawnSpec};
use crate::time::now_rfc3339;

use super::error::EngineError;
use super::event::{TransitionCause, TransitionEvent};
use super::instance::AgentInstance;
use super::lifecycle::LifecycleState;
use super::name::InstanceName;
use super::registry::Registry;
use super::transition::{next_state, LifecycleCommand};

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

/// The lifecycle supervisor: owns running process handles + drives transitions.
///
/// Constructed empty by [`Engine::open`](crate::Engine::open). Holds ONE
/// [`ProcessBackend`](crate::ports::ProcessBackend) (the current OS's) and a map
/// of the instances it currently supervises.
pub struct Supervisor {
    backend: backends::Backend,
    running: HashMap<InstanceName, backends::Handle>,
}

impl Supervisor {
    /// Construct an empty supervisor with the current OS's process backend.
    pub fn new() -> Self {
        Self {
            backend: backends::current(),
            running: HashMap::new(),
        }
    }

    /// Start a registered (or previously stopped) Agent Instance (AC1/AC2).
    ///
    /// Order (so a rejection leaves NO spurious state change):
    /// 1. look up the instance and validate `Start` against the transition table
    ///    (AC4 — invalid transitions reject here, before any side effect),
    /// 2. resolve the launch spec (a bad/native-only adapter rejects here too),
    /// 3. persist `registered/stopped → starting` + emit,
    /// 4. spawn via the backend; a spawn failure → persist `starting → failed`
    ///    + emit (diagnostic preserved, no zombie) and return [`EngineError::LaunchFailed`] (AC2),
    /// 5. watch briefly for an immediate death (AC2 immediate-exit) → `failed`,
    /// 6. otherwise persist `starting → running` + emit, store the handle, return.
    pub fn start(&mut self, registry: &Registry, name: &str) -> Result<AgentInstance, EngineError> {
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
        let (kind, manifest_path) = registry
            .adapter_launch_facts(&name)
            .map_err(registry_to_engine)?;
        let launch = adapter::resolve_start_launch(&kind, manifest_path.as_deref())
            .map_err(|e| launch_to_engine(&name, e))?;

        let home = registry.agent_home(&name);
        // The spawned agent's stdout/stderr go to a SEPARATE agent.log, never the
        // engine's JSON-Lines transition-event log (instance.log) — otherwise the
        // agent's plain-text output would corrupt the structured event log.
        let agent_log_path = registry.agent_output_log_path(&name);
        // Ensure the log directory exists (AD-12 seed) so spawn can redirect
        // stdout/stderr into it and we can append transition events.
        self.ensure_log_dir(registry, &name)?;

        // (3) registered/stopped → starting.
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

        // (6) starting → running (adapter ready).
        self.transition(
            registry,
            &name,
            starting,
            LifecycleState::Running,
            TransitionCause::AdapterReady,
        )?;
        self.running.insert(name.clone(), handle);

        registry.lookup(&name).map_err(registry_to_engine)
    }

    /// Stop a running Agent Instance (AC3/AC4).
    ///
    /// Transitions `running → stopping`, requests graceful shutdown via the
    /// backend and escalates to a forced kill after `window` (default
    /// [`DEFAULT_STOP_WINDOW`]) if needed, records the escalation in the instance
    /// log, then `stopping → stopped`. No process of the instance survives (the
    /// backend kills the whole group/job).
    pub fn stop(
        &mut self,
        registry: &Registry,
        name: &str,
        window: Option<Duration>,
    ) -> Result<AgentInstance, EngineError> {
        let name = InstanceName::new(name).map_err(|reason| EngineError::InvalidName {
            name: name.to_string(),
            reason,
        })?;
        let instance = registry.lookup(&name).map_err(registry_to_engine)?;

        // Transition gate (AC4): stop on stopped / registered / … rejects here
        // with the uniform InvalidTransition, before touching any process.
        let stopping = next_state(instance.state, LifecycleCommand::Stop)?;

        let window = window.unwrap_or(DEFAULT_STOP_WINDOW);
        self.ensure_log_dir(registry, &name)?;

        // running → stopping.
        self.transition(
            registry,
            &name,
            instance.state,
            stopping,
            TransitionCause::command(LifecycleCommand::Stop.as_str()),
        )?;

        // Ask the backend to stop the process (group/job). If we have no handle
        // for it (e.g. the instance's row says running but this engine never
        // started it — cross-lifetime, which is 1-6's job), the desired end state
        // "no process of the instance survives" already holds for THIS engine, so
        // we treat it as a graceful stop.
        let outcome = match self.running.get_mut(&name) {
            Some(handle) => {
                self.backend
                    .stop(handle, window)
                    .map_err(|source| EngineError::Backend {
                        name: name.as_str().to_string(),
                        source,
                    })?
            }
            None => crate::ports::StopOutcome { forced: false },
        };
        // Drop the handle (also closes the Job / releases the child on Windows).
        self.running.remove(&name);

        // stopping → stopped, recording whether escalation happened (AC3).
        let cause = if outcome.forced {
            TransitionCause::stop_forced(format!(
                "graceful window ({}s) elapsed; escalated to a forced kill of the process group/job",
                window.as_secs()
            ))
        } else {
            TransitionCause::StopGraceful
        };
        self.transition(registry, &name, stopping, LifecycleState::Stopped, cause)?;

        registry.lookup(&name).map_err(registry_to_engine)
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

    // ---- internals ----

    /// Apply one transition: persist the new state, then append the event to the
    /// per-instance log. Persist-before-log so the durable state leads; a log
    /// append failure surfaces (the escalation record is load-bearing for AC3).
    fn transition(
        &self,
        registry: &Registry,
        name: &InstanceName,
        prior: LifecycleState,
        new: LifecycleState,
        cause: TransitionCause,
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
        Ok(event)
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
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
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

/// Map a registry lookup/persist error into the lifecycle [`EngineError`].
///
/// Registration and lifecycle share the same NotFound/InvalidName shapes; keep
/// them as the lifecycle variants so `kt` maps them consistently.
fn registry_to_engine(err: super::error::RegistryError) -> EngineError {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
