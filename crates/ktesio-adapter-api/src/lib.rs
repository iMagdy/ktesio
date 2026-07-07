//! # ktesio-adapter-api
//!
//! Home of the **Adapter Contract** (architecture spine AD-1, AD-2, AD-3): the
//! types and traits every agent adapter implements, plus the `adapter.toml`
//! manifest schema — defined here and **only** here, versioned independently
//! from the engine under the contract semver.
//!
//! This crate depends on **nothing internal** (spine AD-1): it is pure contract
//! types + the [`AgentAdapter`] trait, with only serde, `toml` (the manifest
//! parser it owns), and `thiserror`. The engine depends on this crate and
//! consumes its parsed form; `kt` depends on the engine's public API plus these
//! types.
//!
//! ## What this crate exposes (story 1.3 — its first real code)
//!
//! - [`AgentAdapter`] — the Adapter Contract trait (lifecycle op signatures +
//!   declaration accessors). **Nothing is executed this story** — execution is
//!   story 1.4.
//! - [`OsId`] — the operating-system identifier resolved as **data** at runtime
//!   ([`OsId::current`]), never via compile-time platform selection (AD-4).
//! - [`Capability`], [`SupportLevel`], [`CapabilityDeclaration`],
//!   [`EffectiveCapabilities`] — the per-OS Capability Declaration and its
//!   projection onto the running OS (AD-4).
//! - [`MeteringSource`] — the declared Metering Source (AD-7). "No viable
//!   source" is modeled as a validation error, not a variant (FR-19 hard line).
//! - [`Manifest`] / [`ManifestError`] — the `adapter.toml` serde schema, with
//!   [`Manifest::from_toml_str`] and [`Manifest::validate`] (mandatory-section +
//!   viable-metering checks, each naming the failing section — AC2/AC4).
//! - [`CONTRACT_VERSION`] — the Adapter Contract semver **seed** (FR-27/FR-30).
//!
//! ## Seed, not freeze
//!
//! This is the minimal contract seed. The full capability set, the conformance
//! test-kit, and the contract-version freeze/negotiation land in epic 6 (stories
//! 6.4 / 6.6). Keep the surface small and additively extensible; do NOT build
//! negotiation or freeze the set here.

mod adapter;
mod capability;
mod config;
mod manifest;
mod metering;
mod os;

/// The Adapter Contract version this build implements (spine FR-27/FR-30 seed).
///
/// A semver string. This SEEDS the versioned contract; it is **not** frozen
/// here (epic 6.6 freezes v1 and adds negotiation). Manifests declare the
/// version they target via `contract_version`; this story stores it, and does
/// not yet negotiate or enforce compatibility beyond presence.
///
/// Bumped `0.1.0 → 0.2.0` in story 2-2 (FR-12): the manifest schema gains an
/// OPTIONAL `[config]` mapping section + the trait gains a `config_mapping()`
/// accessor with an empty default — an ADDITIVE, backward-compatible extension
/// (a manifest with no `[config]` and a native adapter that does not override
/// `config_mapping()` both keep their 1-3 behavior). An additive minor bump under
/// 0.x semver; nothing was removed or changed incompatibly.
pub const CONTRACT_VERSION: &str = "0.2.0";

pub use adapter::{AdapterError, AgentAdapter};
pub use capability::{Capability, CapabilityDeclaration, EffectiveCapabilities, SupportLevel};
pub use config::{ConfigMapping, ConfigTarget, FilePlacement, FileTarget};
pub use manifest::{
    AdapterIdentity, Interaction, Lifecycle, Manifest, ManifestError, Metering, OpTemplate,
};
pub use metering::MeteringSource;
pub use os::OsId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_version_parses_as_semver() {
        // The seed must be a valid semver so 6.6 can build negotiation on it.
        // Bumped to 0.2.0 in story 2-2 (additive optional `[config]` section +
        // the `config_mapping()` accessor — an additive minor bump under 0.x).
        let parsed = semver::Version::parse(CONTRACT_VERSION).expect("CONTRACT_VERSION is semver");
        assert_eq!(parsed.major, 0);
        assert_eq!(parsed.minor, 2);
        assert_eq!(parsed.patch, 0);
    }

    #[test]
    fn public_surface_is_reachable() {
        // A smoke test that the re-exported contract surface composes: build a
        // declaration, project it, and confirm the metering type is usable.
        let decl = CapabilityDeclaration::new().with(
            Capability::Pause,
            OsId::current(),
            SupportLevel::Guaranteed,
        );
        let eff: EffectiveCapabilities = decl.effective(OsId::current());
        assert_eq!(eff.entries.len(), 1);
        assert_eq!(MeteringSource::SelfReported.as_str(), "self-reported");
    }
}
