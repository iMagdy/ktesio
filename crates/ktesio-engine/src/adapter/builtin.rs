//! The engine's builtin native-adapter table (spine AD-3).
//!
//! Maps a native `kind` string to a compiled-in [`AgentAdapter`]. This table is
//! how `kt agent register --kind <kind>` resolves a native adapter in the
//! **shipping** engine.
//!
//! ## Why the mock lives here, not in `ktesio-conformance`
//!
//! The `mock` kind resolves to [`BuiltinMock`], defined in the engine. It is NOT
//! the `ktesio-conformance` `MockAdapter`, even though they share the same
//! declared shape: a normal `engine → conformance` dependency edge would be
//! transitive into `kt` (`kt → engine → conformance`) and appear in
//! `cargo tree -p ktesio -e normal,build`, tripping the AD-2 boundary gate. The
//! conformance mock stays a dev/test fixture (imported as a dev-dependency by
//! tests that need the richer reusable version); this builtin is what ships so a
//! real operator's `--kind mock` works.
//!
//! Lifecycle ops are inert this story (the trait's default bodies): execution is
//! story 1-4.

use ktesio_adapter_api::{
    AgentAdapter, Capability, CapabilityDeclaration, ConfigMapping, ConfigTarget, MeteringSource,
    OsId, SupportLevel,
};

/// The builtin `mock`'s code-declared config mapping (story 2-2, AC3/AC8): the
/// documented unified key `model` → the ENV var `MODEL`. `env` is the clean,
/// directly-assertable native target the inert-mock proof observes on the mapped
/// launch (Decision 4/8). Kept in shape-parity with the conformance `MockAdapter`
/// (the cross-boundary parity test guards it).
pub const MOCK_MODEL_ENV_VAR: &str = "MODEL";

/// The builtin `mock`'s code-declared env target for the RESERVED
/// [`MEMORY_DIR_KEY`] leaf (story 5-1, Task 5.4 lockstep): the engine injects the
/// managed Memory Backing directory path at `memory.dir` at start, and the mock
/// maps it to this ENV var so the descriptor has a declared native mechanism.
/// MUST stay in lockstep with the conformance `MockAdapter` — the parity test
/// (`conformance_mock_fixture_matches_builtin_shape`) fails if only one moves.
pub const MOCK_MEMORY_ENV_VAR: &str = "KTESIO_MEMORY_DIR";

/// Resolve a native `kind` to a boxed builtin adapter, or `None` if unknown.
///
/// The table is intentionally tiny this story (only `mock`). Native agents like
/// `hermes` register their kinds here in their stories (epic 6).
pub fn native(kind: &str) -> Option<Box<dyn AgentAdapter>> {
    match kind {
        "mock" => Some(Box::new(BuiltinMock::new())),
        _ => None,
    }
}

/// The code-declared config [`ConfigMapping`] for a native `kind`, or `None` if
/// the kind is unknown (story 2-2). This is how the engine's start seam obtains a
/// NATIVE adapter's mapping (a manifest adapter's mapping comes from its parsed
/// `[config]` section instead). A known native adapter that maps no unified keys
/// returns an EMPTY mapping (its trait default). Reuses the same [`native`] table
/// so the mapping can never drift from the adapter that declares it.
pub fn native_config_mapping(kind: &str) -> Option<ConfigMapping> {
    native(kind).map(|adapter| adapter.config_mapping())
}

/// The engine's builtin `mock` adapter (shipping counterpart of the conformance
/// fixture; identical declared shape).
///
/// Declares `pause` guaranteed on Linux/macOS and best-effort on Windows (the
/// AD-4 exemplar) and `interaction` guaranteed everywhere, with a
/// self-reported Metering Source so it registers successfully.
#[derive(Clone, Debug)]
struct BuiltinMock {
    capabilities: CapabilityDeclaration,
}

impl BuiltinMock {
    fn new() -> Self {
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
}

impl AgentAdapter for BuiltinMock {
    fn kind(&self) -> &str {
        "mock"
    }

    fn capabilities(&self) -> &CapabilityDeclaration {
        &self.capabilities
    }

    fn metering_source(&self) -> MeteringSource {
        MeteringSource::SelfReported
    }

    /// The code-declared unified→native config mapping (story 2-2): `model` → the
    /// ENV var [`MOCK_MODEL_ENV_VAR`], plus — since story 5-1 (Task 5.4 lockstep) —
    /// the reserved `memory.dir` key → [`MOCK_MEMORY_ENV_VAR`] so a filesystem
    /// Memory Backing has a declared native mechanism. Mirrors the conformance
    /// `MockAdapter` so the fixture stays a faithful stand-in (the parity test
    /// guards it).
    fn config_mapping(&self) -> ConfigMapping {
        ConfigMapping::new()
            .with("model", ConfigTarget::env(MOCK_MODEL_ENV_VAR))
            .with(
                crate::domain::MEMORY_DIR_KEY,
                ConfigTarget::env(MOCK_MEMORY_ENV_VAR),
            )
    }

    // Lifecycle ops use the trait's inert default bodies (execution is 1-4).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_kind_resolves() {
        let adapter = native("mock").expect("mock must resolve");
        assert_eq!(adapter.kind(), "mock");
        assert_eq!(adapter.metering_source(), MeteringSource::SelfReported);
        assert!(!adapter.capabilities().is_empty());
    }

    #[test]
    fn unknown_kind_returns_none() {
        assert!(native("nope").is_none());
        assert!(native("").is_none());
    }

    #[test]
    fn builtin_mock_declares_the_model_and_memory_env_mappings() {
        // Story 2-2 (AC3/AC8): the builtin mock code-declares `model` → env
        // `MODEL`. Story 5-1 adds the reserved `memory.dir` → env
        // `KTESIO_MEMORY_DIR` mapping so a filesystem Memory Backing has a
        // declared native mechanism.
        let adapter = native("mock").unwrap();
        let mapping = adapter.config_mapping();
        assert_eq!(mapping.len(), 2);
        assert_eq!(
            mapping.target("model").unwrap().env_var(),
            Some(MOCK_MODEL_ENV_VAR)
        );
        assert_eq!(
            mapping
                .target(crate::domain::MEMORY_DIR_KEY)
                .unwrap()
                .env_var(),
            Some(MOCK_MEMORY_ENV_VAR)
        );
        // An unmapped documented key has no rule (delivered nowhere — a no-op).
        assert!(mapping.target("temperature").is_none());
    }

    #[test]
    fn builtin_mock_declares_the_ad4_exemplar_shape() {
        // Intra-crate sanity check of the builtin's literal per-OS shape. The
        // REAL cross-boundary guard that the builtin and the conformance
        // MockAdapter agree lives in `crates/ktesio-engine/tests/registration.rs`
        // (`conformance_mock_fixture_matches_builtin_shape`), which can see both
        // (conformance is a dev-dependency of the test target); this test alone
        // cannot reference conformance without tripping the AD-2 boundary gate.
        let adapter = native("mock").unwrap();
        let decl = adapter.capabilities();
        assert_eq!(
            decl.support(Capability::Pause, OsId::Linux),
            SupportLevel::Guaranteed
        );
        assert_eq!(
            decl.support(Capability::Pause, OsId::Windows),
            SupportLevel::BestEffort
        );
        assert_eq!(
            decl.support(Capability::Interaction, OsId::Macos),
            SupportLevel::Guaranteed
        );
    }
}
