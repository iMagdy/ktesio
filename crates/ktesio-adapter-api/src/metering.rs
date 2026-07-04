//! [`MeteringSource`] — the declared usage-metering source (spine AD-7).
//!
//! Every adapter that registers must declare a **viable** Metering Source. The
//! FR-19 hard line: an adapter with no viable source is rejected at
//! registration. That rejection is modeled as the **absence** of a valid
//! `[metering]` section (a validation error), NOT a `MeteringSource::None`
//! variant — so this enum only ever holds a real, viable source. An
//! [`crate::AgentAdapter`] that successfully registers always has one.
//!
//! ## Scope this story
//!
//! This story only DECLARES the source and validates its presence. The metering
//! pipeline (UsageEvent ingestion → ledger → budget evaluation) is epic 3.

use serde::{Deserialize, Serialize};

/// Where an agent's usage measurements come from (spine AD-7).
///
/// Exactly two viable kinds. The serde wire form is kebab-case
/// (`self-reported`, `engine-observed`) so an `adapter.toml` `[metering]`
/// section reads naturally:
///
/// ```toml
/// [metering]
/// source = "self-reported"
/// ```
///
/// There is deliberately no `None`/`none` variant: "no viable source" is a
/// missing/invalid section caught by validation, keeping this type honest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeteringSource {
    /// The agent reports its own token usage (e.g. via a usage channel).
    SelfReported,
    /// The engine observes usage externally (e.g. by parsing output).
    EngineObserved,
}

impl MeteringSource {
    /// The kebab-case wire name, matching the serde form and manifest value.
    pub fn as_str(&self) -> &'static str {
        match self {
            MeteringSource::SelfReported => "self-reported",
            MeteringSource::EngineObserved => "engine-observed",
        }
    }
}

impl std::fmt::Display for MeteringSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_wire_form_is_kebab_case() {
        assert_eq!(
            serde_json::to_string(&MeteringSource::SelfReported).unwrap(),
            "\"self-reported\""
        );
        assert_eq!(
            serde_json::to_string(&MeteringSource::EngineObserved).unwrap(),
            "\"engine-observed\""
        );
    }

    #[test]
    fn serde_round_trips_both_variants() {
        for source in [MeteringSource::SelfReported, MeteringSource::EngineObserved] {
            let json = serde_json::to_string(&source).unwrap();
            let back: MeteringSource = serde_json::from_str(&json).unwrap();
            assert_eq!(back, source);
            assert_eq!(source.to_string(), source.as_str());
        }
    }

    #[test]
    fn unknown_source_string_is_rejected_by_serde() {
        // "none" is NOT a variant — a manifest that spells an invalid source
        // fails to deserialize, which the manifest layer turns into a
        // section-naming validation error (AC4).
        assert!(serde_json::from_str::<MeteringSource>("\"none\"").is_err());
        assert!(serde_json::from_str::<MeteringSource>("\"bogus\"").is_err());
    }
}
