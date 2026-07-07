//! [`AgentAdapter`] — the Adapter Contract trait (spine AD-1 downward port, AD-3).
//!
//! Every agent integrates as an `AgentAdapter` of exactly one kind:
//! **native** (a Rust type compiled in — e.g. the conformance mock) or
//! **manifest** (a directory with `adapter.toml`; the engine builds a
//! manifest-backed view). The trait declares:
//!
//! * the **lifecycle ops** (`start`, `stop`, `pause`, `resume` — the ratified
//!   state-machine verbs, AD-15) as method signatures a native adapter
//!   implements and a manifest adapter carries templates for; and
//! * **accessors** for the adapter's [`CapabilityDeclaration`] and
//!   [`MeteringSource`], which the engine reads at registration.
//!
//! ## NOTHING is executed this story (CRITICAL boundary)
//!
//! Lifecycle EXECUTION — actually starting/stopping/pausing a process, the
//! manifest executor, the process launch — is **story 1-4**. This story stores
//! and validates declarations and templates only. The lifecycle methods exist so
//! the interface is complete (a native adapter can implement them; the mock does,
//! inertly) and so the trait documents what manifest templates must cover. The
//! engine's registration path calls **only** the accessors, never a lifecycle
//! op. `[ASSUMPTION]` the exact lifecycle method set is minimal and seeded here;
//! 1-4 and the conformance TCK (6.4) widen it.

use thiserror::Error;

use crate::capability::CapabilityDeclaration;
use crate::config::ConfigMapping;
use crate::metering::MeteringSource;

/// An error from an adapter lifecycle op (spine AD-3; seed surface).
///
/// `thiserror`, never `miette` (adapter-api conventions). Kept minimal this
/// story — real execution failures (spawn errors, timeouts) are modeled by the
/// executor in 1-4. The single variant lets the inert mock return a typed error
/// shape without implying any process semantics yet.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// The lifecycle op is not available (e.g. an inert fixture, or a capability
    /// the adapter declares as unsupported on this OS). Names the op.
    #[error("adapter lifecycle op '{op}' is unavailable: {reason}")]
    Unavailable {
        /// The lifecycle op that was unavailable (`start`/`stop`/`pause`/`resume`).
        op: &'static str,
        /// Why it was unavailable.
        reason: String,
    },
}

/// The Adapter Contract: what every agent adapter exposes (spine AD-3).
///
/// Implemented by native adapters directly. Manifest adapters are represented by
/// an engine-side view built from a validated `adapter.toml`. The engine treats
/// both identically through the accessors (AD-3 "two kinds, one trait").
///
/// The trait is deliberately small this story. Lifecycle methods are present but
/// **unused by the engine** (execution is 1-4); only [`AgentAdapter::kind`],
/// [`AgentAdapter::capabilities`], and [`AgentAdapter::metering_source`] are read
/// at registration.
pub trait AgentAdapter {
    /// The adapter's kind identifier (e.g. `"mock"`, `"hermes"`), as stored on
    /// the Agent Instance and used to resolve a native adapter.
    fn kind(&self) -> &str;

    /// The adapter's Capability Declaration (per-OS support matrix, AD-4).
    ///
    /// Read at registration; an empty declaration is rejected (AC2).
    fn capabilities(&self) -> &CapabilityDeclaration;

    /// The adapter's declared Metering Source (AD-7).
    ///
    /// Read at registration; the type guarantees a viable source (absence is a
    /// validation error before an adapter exists, per [`MeteringSource`]).
    fn metering_source(&self) -> MeteringSource;

    /// The adapter's declared unified→native config [`ConfigMapping`] (story 2-2,
    /// FR-12). Read by the engine's START seam to map each documented unified key
    /// (2-1's resolved [`EffectiveConfig`]) into this adapter's native mechanism
    /// (a config file, an env var, or a CLI flag).
    ///
    /// The DEFAULT is an EMPTY mapping (mirrors the "empty declaration" defaults):
    /// a native adapter that maps no unified keys need not override it, and a
    /// manifest adapter with no `[config]` section yields the same empty mapping —
    /// the "two kinds, one trait" invariant (AD-3). An unmapped documented key is
    /// delivered NOWHERE (a no-op — Decision 6), so this stays additive: adding a
    /// mapping only makes MORE keys land, never changes an existing launch.
    fn config_mapping(&self) -> ConfigMapping {
        ConfigMapping::default()
    }

    /// Start the agent. **Not executed this story** (1-4 owns execution).
    ///
    /// A native adapter implements the real launch in 1-4; here the method
    /// exists to complete the interface. The default body reports the op as
    /// unavailable so an accidental early call is explicit rather than silent.
    fn start(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable {
            op: "start",
            reason: "lifecycle execution is not implemented until story 1-4".to_string(),
        })
    }

    /// Stop the agent. **Not executed this story** (1-4 owns execution).
    fn stop(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable {
            op: "stop",
            reason: "lifecycle execution is not implemented until story 1-4".to_string(),
        })
    }

    /// Pause the agent. **Not executed this story** (1-4/1-5 own execution).
    fn pause(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable {
            op: "pause",
            reason: "lifecycle execution is not implemented until story 1-4".to_string(),
        })
    }

    /// Resume the agent. **Not executed this story** (1-4/1-5 own execution).
    fn resume(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable {
            op: "resume",
            reason: "lifecycle execution is not implemented until story 1-4".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, SupportLevel};
    use crate::os::OsId;

    /// A tiny in-test adapter to exercise the trait's default bodies and
    /// accessors without pulling the conformance crate (which itself tests the
    /// real mock).
    struct Probe {
        caps: CapabilityDeclaration,
    }

    impl AgentAdapter for Probe {
        fn kind(&self) -> &str {
            "probe"
        }
        fn capabilities(&self) -> &CapabilityDeclaration {
            &self.caps
        }
        fn metering_source(&self) -> MeteringSource {
            MeteringSource::SelfReported
        }
    }

    fn probe() -> Probe {
        Probe {
            caps: CapabilityDeclaration::new().with(
                Capability::Pause,
                OsId::Linux,
                SupportLevel::Guaranteed,
            ),
        }
    }

    #[test]
    fn accessors_return_declared_values() {
        let p = probe();
        assert_eq!(p.kind(), "probe");
        assert_eq!(p.metering_source(), MeteringSource::SelfReported);
        assert!(!p.capabilities().is_empty());
    }

    #[test]
    fn default_lifecycle_ops_report_unavailable_this_story() {
        let p = probe();
        for result in [p.start(), p.stop(), p.pause(), p.resume()] {
            let err = result.unwrap_err();
            let AdapterError::Unavailable { reason, .. } = &err;
            assert!(reason.contains("1-4"), "{err}");
        }
    }

    #[test]
    fn adapter_error_names_the_op() {
        let p = probe();
        let err = p.pause().unwrap_err();
        assert!(err.to_string().contains("pause"));
    }

    #[test]
    fn config_mapping_defaults_to_empty_and_can_be_overridden() {
        use crate::config::ConfigTarget;

        // Story 2-2: the default accessor is an EMPTY mapping (a native adapter
        // that maps no unified keys — like the Probe — need not override it).
        let p = probe();
        assert!(p.config_mapping().is_empty());

        // An adapter that DOES override it declares the same ConfigMapping shape.
        struct Mapped {
            caps: CapabilityDeclaration,
        }
        impl AgentAdapter for Mapped {
            fn kind(&self) -> &str {
                "mapped"
            }
            fn capabilities(&self) -> &CapabilityDeclaration {
                &self.caps
            }
            fn metering_source(&self) -> MeteringSource {
                MeteringSource::SelfReported
            }
            fn config_mapping(&self) -> crate::config::ConfigMapping {
                crate::config::ConfigMapping::new().with("model", ConfigTarget::env("MODEL"))
            }
        }
        let m = Mapped {
            caps: CapabilityDeclaration::new().with(
                Capability::Pause,
                OsId::Linux,
                SupportLevel::Guaranteed,
            ),
        };
        assert_eq!(
            m.config_mapping().target("model").unwrap().env_var(),
            Some("MODEL")
        );
    }
}
