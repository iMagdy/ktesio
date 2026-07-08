//! # ktesio-conformance
//!
//! The adapter conformance fixtures: a **mock native adapter** and an **inert
//! scripted fake agent**, used by this story and all later lifecycle/governance
//! tests (architecture spine AD-2, AD-3; PRD FR-28). The full conformance
//! test-kit (TCK) itself lands with story 6.4 — this crate ships only the
//! reusable fixtures now.
//!
//! ## Dependency boundary (CRITICAL — why this is a DEV fixture downstream)
//!
//! This crate is a normal dependency of nothing that ships to operators. The
//! engine and `kt` reference it as a **dev-dependency only**: a normal
//! `engine → conformance` edge would be transitive into `kt` and trip the AD-2
//! boundary CI gate (which inspects `cargo tree -p ktesio -e normal,build`). The
//! shipping `--kind mock` path resolves to the engine's own internal builtin
//! adapter, not this fixture; this [`MockAdapter`] is the richer, reusable
//! fixture later stories (1-4 start/stop, epic 3 metering, 6.4 TCK) import to
//! drive lifecycle and governance tests.
//!
//! ## Inert this story
//!
//! Nothing here spawns a process. The [`ScriptedFakeAgent`] describes a canned
//! lifecycle-op script that story 1-4 will actually run; this story only
//! constructs and inspects it.

use ktesio_adapter_api::{
    AdapterError, AgentAdapter, Capability, CapabilityDeclaration, ConfigMapping, ConfigTarget,
    MeteringSource, OsId, SupportLevel,
};

/// The kind string the mock adapter registers under.
pub const MOCK_KIND: &str = "mock";

/// The mock's code-declared config-mapping target for the documented `model` key
/// (story 2-2): the ENV var `MODEL`. MUST match the shipping engine builtin's
/// [`MOCK_MODEL_ENV_VAR`] — the cross-boundary parity test in the engine guards
/// the two fixtures against drift.
pub const MOCK_MODEL_ENV_VAR: &str = "MODEL";

/// A native [`AgentAdapter`] fixture with a per-OS Capability Declaration.
///
/// Declares `pause` as **guaranteed** on Linux/macOS and **best-effort** on
/// Windows (the AD-4 exemplar — SIGSTOP is reliable on Unix, Job-Object
/// suspension is approximate on Windows), and `interaction` as guaranteed
/// everywhere. Its Metering Source is [`MeteringSource::SelfReported`], so it
/// registers successfully (a viable source, AC4).
///
/// Lifecycle ops are inert this story (the default trait bodies report them
/// unavailable until 1-4). This fixture exists to exercise the AC1 per-OS
/// projection through a real adapter and to be reused by later stories.
#[derive(Clone, Debug)]
pub struct MockAdapter {
    capabilities: CapabilityDeclaration,
}

impl MockAdapter {
    /// Construct the mock with its canonical per-OS Capability Declaration.
    pub fn new() -> Self {
        let capabilities = CapabilityDeclaration::new()
            .with(Capability::Pause, OsId::Linux, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Macos, SupportLevel::Guaranteed)
            .with(Capability::Pause, OsId::Windows, SupportLevel::BestEffort)
            .with(
                Capability::Interaction,
                OsId::Linux,
                SupportLevel::Guaranteed,
            )
            .with(
                Capability::Interaction,
                OsId::Macos,
                SupportLevel::Guaranteed,
            )
            .with(
                Capability::Interaction,
                OsId::Windows,
                SupportLevel::Guaranteed,
            );
        Self { capabilities }
    }

    /// A scripted fake agent driven by this adapter (inert this story).
    pub fn scripted_fake_agent(&self) -> ScriptedFakeAgent {
        ScriptedFakeAgent::canned()
    }
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentAdapter for MockAdapter {
    fn kind(&self) -> &str {
        MOCK_KIND
    }

    fn capabilities(&self) -> &CapabilityDeclaration {
        &self.capabilities
    }

    fn metering_source(&self) -> MeteringSource {
        MeteringSource::SelfReported
    }

    /// The code-declared unified→native config mapping (story 2-2): `model` → the
    /// ENV var [`MOCK_MODEL_ENV_VAR`]. Mirrors the shipping engine `BuiltinMock`
    /// so this fixture stays a faithful stand-in (the engine's cross-boundary
    /// parity test guards the two against drift).
    fn config_mapping(&self) -> ConfigMapping {
        ConfigMapping::new().with("model", ConfigTarget::env(MOCK_MODEL_ENV_VAR))
    }

    // Lifecycle ops intentionally use the trait's default (unavailable) bodies:
    // execution is story 1-4. Overriding them here with real process spawning is
    // explicitly out of scope this story.
}

/// One step in a [`ScriptedFakeAgent`]'s canned lifecycle script.
///
/// A described, inspectable stand-in for a real agent action — **not** executed
/// this story. Story 1-4's executor consumes these to drive the fake agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptStep {
    /// The agent would emit this line on its output channel.
    Emit(String),
    /// The agent would report this many input/output tokens (metering seed).
    ReportUsage {
        /// Input tokens the step would report.
        input_tokens: u64,
        /// Output tokens the step would report.
        output_tokens: u64,
    },
    /// The agent would exit with this code.
    Exit(i32),
}

/// An **inert** scripted fake agent: a canned, inspectable lifecycle script.
///
/// This is the fixture "used by this and all later lifecycle/governance tests".
/// It spawns nothing. Story 1-4 will execute the script for real; here it only
/// describes what a run *would* do, so tests can assert on structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptedFakeAgent {
    /// The ordered steps a real run would perform.
    pub steps: Vec<ScriptStep>,
}

impl ScriptedFakeAgent {
    /// A small canonical script: greet, report usage, exit cleanly.
    pub fn canned() -> Self {
        Self {
            steps: vec![
                ScriptStep::Emit("hello from the scripted fake agent".to_string()),
                ScriptStep::ReportUsage {
                    input_tokens: 3,
                    output_tokens: 7,
                },
                ScriptStep::Exit(0),
            ],
        }
    }

    /// Whether this fixture is inert (never spawns a process). Always `true`
    /// this story — a guard that makes the inert boundary explicit and testable.
    pub fn is_inert(&self) -> bool {
        true
    }

    /// The exit code the script ends with, if it declares one.
    pub fn declared_exit_code(&self) -> Option<i32> {
        self.steps.iter().rev().find_map(|step| match step {
            ScriptStep::Exit(code) => Some(*code),
            _ => None,
        })
    }
}

/// Try to demonstrate that the mock's lifecycle ops are inert this story.
///
/// Returns the [`AdapterError`] the (default) `start` op reports, so tests can
/// assert the adapter carries declarations without implying execution.
pub fn probe_inert_start(adapter: &MockAdapter) -> AdapterError {
    adapter
        .start()
        .expect_err("mock start must be inert (unavailable) until story 1-4")
}

/// Locate the `fake_agent` test helper binary (story 1.4, AD-3).
///
/// The engine's start/stop integration tests point a manifest adapter's
/// `[lifecycle.start]` `exec` at this binary so the supervisor spawns a REAL
/// process. `CARGO_BIN_EXE_fake_agent` is only set for THIS crate's own targets,
/// so a cross-crate test resolves the path from the running test executable's
/// location instead: `fake_agent` sits next to the test-deps directory, in the
/// same `debug`/`release` profile dir.
///
/// If the binary is not present (e.g. under `cargo tarpaulin`, which builds test
/// targets but not sibling `[[bin]]` targets), it is BUILT on demand via
/// `cargo build -p ktesio-conformance --bin fake_agent` so the process-spawning
/// tests run under every harness. Panics with a clear message only if the build
/// itself fails.
///
/// Kept a plain runtime path computation — no OS-conditional compilation (the
/// executable suffix comes from [`std::env::consts::EXE_SUFFIX`], a runtime
/// constant, so the OS-cfg gate stays green).
pub fn fake_agent_bin() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("locate the running test executable");
    // .../target/<profile>/deps/<test-bin>  → go up to .../target/<profile>/
    let mut dir = exe;
    dir.pop(); // drop the test-bin file name
    if dir.ends_with("deps") {
        dir.pop(); // drop `deps`
    }
    let candidate = dir.join(format!("fake_agent{}", std::env::consts::EXE_SUFFIX));
    if candidate.exists() {
        return candidate;
    }
    // Not built by this harness — build it on demand (e.g. tarpaulin).
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "ktesio-conformance", "--bin", "fake_agent"])
        .status();
    match status {
        Ok(s) if s.success() && candidate.exists() => candidate,
        other => panic!(
            "fake_agent binary not found at {} and an on-demand build did not produce it \
             (build status: {other:?}). Build `ktesio-conformance` first.",
            candidate.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_declares_viable_metering_and_nonempty_capabilities() {
        let mock = MockAdapter::new();
        assert_eq!(mock.kind(), MOCK_KIND);
        assert_eq!(mock.metering_source(), MeteringSource::SelfReported);
        assert!(!mock.capabilities().is_empty());
        assert_eq!(mock.capabilities().len(), 2);
    }

    #[test]
    fn mock_declares_the_model_env_config_mapping() {
        // Story 2-2: the fixture mirrors the shipping builtin's `model` → env
        // `MODEL` mapping (the engine parity test guards the two against drift).
        let mock = MockAdapter::new();
        let mapping = mock.config_mapping();
        assert_eq!(mapping.len(), 1);
        assert_eq!(
            mapping.target("model").unwrap().env_var(),
            Some(MOCK_MODEL_ENV_VAR)
        );
    }

    #[test]
    fn mock_per_os_projection_matches_declared_levels() {
        // Drive every modeled OS as DATA — proves the AC1 per-OS path via the
        // mock on any host.
        let mock = MockAdapter::new();
        let decl = mock.capabilities();

        assert_eq!(
            decl.support(Capability::Pause, OsId::Linux),
            SupportLevel::Guaranteed
        );
        assert_eq!(
            decl.support(Capability::Pause, OsId::Macos),
            SupportLevel::Guaranteed
        );
        assert_eq!(
            decl.support(Capability::Pause, OsId::Windows),
            SupportLevel::BestEffort
        );
        assert_eq!(
            decl.support(Capability::Interaction, OsId::Windows),
            SupportLevel::Guaranteed
        );

        // The effective projection carries both capabilities on each OS.
        for os in OsId::MODELED {
            let eff = decl.effective(os);
            assert_eq!(eff.entries.len(), 2, "os={os}");
        }
    }

    #[test]
    fn scripted_fake_agent_is_constructible_and_inert() {
        let mock = MockAdapter::new();
        let agent = mock.scripted_fake_agent();
        assert!(agent.is_inert(), "the fake agent must not spawn a process");
        assert_eq!(agent, ScriptedFakeAgent::canned());
        assert_eq!(agent.declared_exit_code(), Some(0));
        // The script has the expected shape.
        assert!(matches!(agent.steps.first(), Some(ScriptStep::Emit(_))));
        assert!(agent
            .steps
            .iter()
            .any(|s| matches!(s, ScriptStep::ReportUsage { .. })));
    }

    #[test]
    fn mock_lifecycle_ops_are_inert_until_1_4() {
        let mock = MockAdapter::new();
        let err = probe_inert_start(&mock);
        assert!(err.to_string().contains("start"));
        // stop/pause/resume are likewise inert (default bodies).
        assert!(mock.stop().is_err());
        assert!(mock.pause().is_err());
        assert!(mock.resume().is_err());
    }

    #[test]
    fn declared_exit_code_none_when_no_exit_step() {
        let agent = ScriptedFakeAgent {
            steps: vec![ScriptStep::Emit("no exit here".to_string())],
        };
        assert_eq!(agent.declared_exit_code(), None);
        assert!(agent.is_inert());
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(MockAdapter::default().kind(), MockAdapter::new().kind());
    }
}
