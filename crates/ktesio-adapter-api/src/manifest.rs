//! The `adapter.toml` manifest schema + validation (spine AD-3 — OWNED HERE).
//!
//! The manifest-adapter schema's serde types and [`Manifest::validate`] live
//! **only** in this crate, versioned under the same Adapter Contract semver
//! ([`crate::CONTRACT_VERSION`]). The engine parses `adapter.toml` exclusively
//! through [`Manifest::from_toml_str`] and defines no schema of its own.
//!
//! ## Mandatory sections (the AC2 seed)
//!
//! A manifest is valid only if it declares, and [`Manifest::validate`] names the
//! first one that is missing/empty/invalid:
//!
//! * `contract_version` — the Adapter Contract version the manifest targets.
//! * `[adapter]` identity — `kind` (and optional `name`).
//! * `[lifecycle]` — at least a `start` op template (`exec`, optional `args`/`env`).
//! * `[capabilities]` — a non-empty per-OS Capability Declaration (AD-4).
//! * `[metering]` — a viable Metering Source (AD-7; the FR-19 hard line, AC4).
//!
//! `#[serde(deny_unknown_fields)]` (the repo's manifest style) additionally
//! rejects unknown keys, catching typos.
//!
//! ## Not executed this story
//!
//! The lifecycle templates are stored, never run. The manifest executor / process
//! launch is story 1-4.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;

use crate::capability::CapabilityDeclaration;
use crate::config::ConfigMapping;
use crate::metering::MeteringSource;

/// The parsed `adapter.toml` (spine AD-3 manifest schema).
///
/// Fields are `Option` where a section is mandatory-by-validation rather than
/// mandatory-by-deserialization, so a missing section yields a precise,
/// section-naming [`ManifestError`] from [`Manifest::validate`] instead of an
/// opaque serde error. Unknown keys are rejected (`deny_unknown_fields`).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The Adapter Contract version this manifest targets (e.g. `"0.1.0"`).
    pub contract_version: Option<String>,
    /// Adapter identity (`kind`, optional `name`).
    pub adapter: Option<AdapterIdentity>,
    /// Lifecycle op templates (`start`/`stop`/`pause`/`resume`).
    pub lifecycle: Option<Lifecycle>,
    /// Per-OS Capability Declaration (AD-4). Reuses the contract type directly.
    pub capabilities: Option<CapabilityDeclaration>,
    /// Metering configuration (AD-7).
    pub metering: Option<Metering>,
    /// Interaction wiring (channel). Optional this story (defaults documented).
    pub interaction: Option<Interaction>,
    /// The unified→native config MAPPING (story 2-2, FR-12). OPTIONAL: an adapter
    /// that maps no unified keys omits it entirely (an empty mapping is valid and
    /// common). Each `[config.<key>]` sub-table declares one native mechanism
    /// (`env`/`flag`/`file`). Validated by [`Manifest::validate`] when present.
    pub config: Option<ConfigMapping>,
}

/// The `[adapter]` identity block.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterIdentity {
    /// The adapter kind (stored on the Agent Instance; e.g. `"hermes"`).
    pub kind: String,
    /// An optional human-friendly name.
    pub name: Option<String>,
}

/// The `[lifecycle]` block: one [`OpTemplate`] per lifecycle op.
///
/// `start` is the minimum this story requires; the others are optional seeds
/// that 1-4's executor will consume. `[ASSUMPTION]` on requiring only `start`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    /// The start op template (required by [`Manifest::validate`]).
    pub start: Option<OpTemplate>,
    /// The stop op template.
    pub stop: Option<OpTemplate>,
    /// The pause op template.
    pub pause: Option<OpTemplate>,
    /// The resume op template.
    pub resume: Option<OpTemplate>,
}

/// A single lifecycle op's exec/args/env template (carried, not executed).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpTemplate {
    /// The executable to run (a path or program name).
    pub exec: String,
    /// Positional arguments (defaults to empty).
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment overrides (defaults to empty).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The `[metering]` block.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metering {
    /// The declared Metering Source. An unknown value fails deserialization,
    /// which [`Manifest::from_toml_str`] surfaces as a section-naming error.
    pub source: MeteringSource,
}

/// The `[interaction]` block (channel wiring).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interaction {
    /// The interaction channel (e.g. `"stdio"`). Free-form this story; the
    /// channel model firms up with the interaction epic.
    pub channel: String,
}

/// Why a manifest is invalid (spine AD-3; `thiserror`, never `miette`).
///
/// Every message NAMES the failing section or the invalid value (AC2), so `kt`
/// can render a diagnostic quoting it and the engine can map it to a
/// `RegistryError` variant.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The TOML failed to parse (syntax error, or a section held the wrong
    /// type / an unknown key). Carries the parser's message.
    #[error("adapter.toml is not valid TOML: {detail}")]
    Toml {
        /// The underlying parser message.
        detail: String,
    },

    /// A mandatory section (or field) was missing or empty. Names it.
    #[error("adapter.toml is missing the required {section}")]
    MissingSection {
        /// A human phrase naming the section, e.g. "`[metering]` section" or
        /// "`contract_version` field".
        section: String,
    },

    /// A present field held a syntactically invalid value (e.g. a
    /// `contract_version` that is not semver, or a `kind` that breaks the
    /// adapter-kind charset). Names the field and why it is invalid.
    #[error("adapter.toml has an invalid {field}: {detail}")]
    InvalidField {
        /// A human phrase naming the field, e.g. "`contract_version`" or
        /// "`kind` field in the `[adapter]` section".
        field: String,
        /// Why the value is invalid.
        detail: String,
    },
}

impl ManifestError {
    /// Convenience constructor naming a missing section.
    fn missing(section: impl Into<String>) -> Self {
        ManifestError::MissingSection {
            section: section.into(),
        }
    }
}

impl Manifest {
    /// Parse an `adapter.toml` document into a [`Manifest`].
    ///
    /// Only parses (syntax + shape + unknown-key rejection). Presence of the
    /// mandatory sections is enforced separately by [`Manifest::validate`], so
    /// callers get a precise section-naming error rather than an opaque serde
    /// message. A bad `source`/`support-level`/OS value fails here (wrong type
    /// for the field) and is reported as a TOML error naming the position.
    pub fn from_toml_str(input: &str) -> Result<Self, ManifestError> {
        toml::from_str(input).map_err(|e| {
            let message = e.message().to_string();
            // The one enum in the schema is the `[metering]` source. `toml`
            // reports an out-of-range value as "unknown variant `x`, expected
            // ..." without naming the section; attribute it to `[metering]` so
            // the diagnostic names the section (AC2/AC4), matching every other
            // rejection. This is the only field whose value is a closed enum.
            let detail = if message.starts_with("unknown variant")
                && (message.contains("self-reported") || message.contains("engine-observed"))
            {
                format!("the `[metering]` section has an invalid `source`: {message}")
            } else {
                message
            };
            ManifestError::Toml { detail }
        })
    }

    /// Enforce the mandatory sections (AC2) and a viable Metering Source (AC4).
    ///
    /// Returns the first failure, its message NAMING the missing/invalid
    /// section. Order is stable (contract version → adapter identity → lifecycle
    /// → capabilities → metering) so diagnostics are deterministic.
    pub fn validate(&self) -> Result<(), ManifestError> {
        // contract_version present, non-empty, AND a real semver version (so 6.6
        // can build negotiation on it — a garbage string like "banana" is not a
        // version the contract can reason about). The error names the field.
        match &self.contract_version {
            Some(v) if !v.trim().is_empty() => {
                if semver::Version::parse(v.trim()).is_err() {
                    return Err(ManifestError::InvalidField {
                        field: "`contract_version`".to_string(),
                        detail: format!("'{v}' is not a valid semver version"),
                    });
                }
            }
            _ => return Err(ManifestError::missing("`contract_version` field")),
        }

        // [adapter] identity with a non-empty kind that obeys the adapter-kind
        // charset rule (`^[a-z0-9][a-z0-9_-]*$`). A kind with whitespace,
        // newlines, or other punctuation would later corrupt DB/CLI tables and
        // the 1-4 launch, so it is rejected here naming the field.
        match &self.adapter {
            Some(a) if !a.kind.trim().is_empty() => {
                if !is_valid_adapter_kind(&a.kind) {
                    return Err(ManifestError::InvalidField {
                        field: "`kind` field in the `[adapter]` section".to_string(),
                        detail: format!(
                            "'{}' must match ^[a-z0-9][a-z0-9_-]*$ (lowercase letters, digits, \
                             '_' or '-', not starting with '_' or '-')",
                            a.kind
                        ),
                    });
                }
            }
            Some(_) => {
                return Err(ManifestError::missing(
                    "`kind` field in the `[adapter]` section",
                ))
            }
            None => return Err(ManifestError::missing("`[adapter]` section")),
        }

        // [lifecycle] with at least a start op (the minimum this story requires).
        match &self.lifecycle {
            Some(l) if l.start.is_some() => {}
            Some(_) => {
                return Err(ManifestError::missing(
                    "`start` op in the `[lifecycle]` section",
                ))
            }
            None => return Err(ManifestError::missing("`[lifecycle]` section")),
        }

        // [capabilities] declares real support (AD-4). Not merely non-empty: a
        // capability key with an empty per-OS map, or one whose every entry is
        // `unsupported`, promises support nowhere and is rejected — an adapter
        // that supports nothing is not viable.
        match &self.capabilities {
            Some(c) if c.has_any_support() => {}
            _ => {
                return Err(ManifestError::missing(
                    "`[capabilities]` section (it declares no supported capabilities)",
                ))
            }
        }

        // [metering] with a viable source (AC4 / FR-19 hard line). The source
        // type only holds viable kinds, so its mere presence proves viability;
        // an invalid value would already have failed to parse.
        if self.metering.is_none() {
            return Err(ManifestError::missing("`[metering]` section"));
        }

        // [config] mapping (story 2-2, FR-12), IF present. Absent is valid (an
        // adapter that maps no unified keys — the common case). A present-but-
        // malformed rule (an empty native token, or a `file.path` that is
        // absolute / escapes the Agent Home) is an InvalidField naming the
        // `[config.<key>]` sub-section. A rule that names an unknown/ambiguous
        // native mechanism never reaches here — `#[serde(untagged)]` fails to
        // match a variant at PARSE time (surfaced as a section error by
        // `from_toml_str`), like every other typed field.
        if let Some(mapping) = &self.config {
            if let Err((key, detail)) = mapping.validate() {
                return Err(ManifestError::InvalidField {
                    field: format!("`[config.{key}]` mapping in the `[config]` section"),
                    detail,
                });
            }
        }

        Ok(())
    }

    /// The declared Capability Declaration (present only after [`Self::validate`]).
    ///
    /// Returns a reference to the parsed declaration, or an empty one if absent
    /// (validation rejects that case, so a validated manifest always has a real
    /// declaration here).
    pub fn capability_declaration(&self) -> CapabilityDeclaration {
        self.capabilities.clone().unwrap_or_default()
    }

    /// The declared Metering Source (present only after [`Self::validate`]).
    ///
    /// Returns `None` if absent (validation rejects that case).
    pub fn metering_source(&self) -> Option<MeteringSource> {
        self.metering.as_ref().map(|m| m.source)
    }

    /// The adapter kind declared in `[adapter]` (present after validation).
    pub fn adapter_kind(&self) -> Option<&str> {
        self.adapter.as_ref().map(|a| a.kind.as_str())
    }

    /// The declared unified→native config [`ConfigMapping`] (story 2-2, FR-12).
    ///
    /// Returns the parsed `[config]` mapping, or an EMPTY mapping when the section
    /// is absent — so the engine's start seam treats "no `[config]`" as "maps no
    /// unified keys" (a no-op) uniformly with a native adapter that declares no
    /// mapping. Mirrors [`Self::capability_declaration`]'s empty-when-absent shape.
    pub fn config_mapping(&self) -> ConfigMapping {
        self.config.clone().unwrap_or_default()
    }
}

/// The adapter-kind charset rule: `^[a-z0-9][a-z0-9_-]*$`.
///
/// A `kind` is stored on the Agent Instance, rendered in CLI tables, and (in
/// 1-4) used to select a launch target, so it must be a safe token: lowercase
/// letters, digits, `_` or `-`, not starting with `_`/`-`, and never containing
/// whitespace, newlines, or other punctuation. This mirrors the `InstanceName`
/// rule (spine Consistency Conventions), which native builtin kinds already
/// satisfy (`mock`, `hermes`).
pub(crate) fn is_valid_adapter_kind(kind: &str) -> bool {
    let mut chars = kind.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, SupportLevel};
    use crate::os::OsId;

    /// A complete, valid manifest covering every section.
    const VALID: &str = r#"
contract_version = "0.1.0"

[adapter]
kind = "demo"
name = "Demo Agent"

[lifecycle.start]
exec = "demo-agent"
args = ["--serve"]
env = { DEMO_MODE = "1" }

[lifecycle.stop]
exec = "demo-agent"
args = ["--shutdown"]

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[capabilities.interaction]
linux = "guaranteed"

[metering]
source = "self-reported"

[interaction]
channel = "stdio"
"#;

    #[test]
    fn happy_path_parses_and_validates() {
        let m = Manifest::from_toml_str(VALID).expect("parse");
        m.validate().expect("validate");

        assert_eq!(m.adapter_kind(), Some("demo"));
        assert_eq!(m.metering_source(), Some(MeteringSource::SelfReported));

        let decl = m.capability_declaration();
        assert_eq!(decl.len(), 2);
        assert_eq!(
            decl.support(Capability::Pause, OsId::Windows),
            SupportLevel::BestEffort
        );

        // Lifecycle templates are carried (not executed).
        let lc = m.lifecycle.as_ref().unwrap();
        let start = lc.start.as_ref().unwrap();
        assert_eq!(start.exec, "demo-agent");
        assert_eq!(start.args, vec!["--serve".to_string()]);
        assert_eq!(start.env.get("DEMO_MODE").map(String::as_str), Some("1"));
        assert!(lc.stop.is_some());
        assert_eq!(m.interaction.as_ref().unwrap().channel, "stdio");
    }

    #[test]
    fn missing_contract_version_names_the_field() {
        let toml = VALID.replace("contract_version = \"0.1.0\"\n", "");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(
            matches!(&err, ManifestError::MissingSection { section } if section.contains("contract_version")),
            "got {err}"
        );
    }

    #[test]
    fn empty_contract_version_is_rejected() {
        let toml = VALID.replace("contract_version = \"0.1.0\"", "contract_version = \"\"");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("contract_version"), "{err}");
    }

    #[test]
    fn non_semver_contract_version_is_rejected_naming_the_field() {
        // F5: a present, non-empty but non-semver contract_version is rejected
        // (parsed with semver::Version::parse), naming the field.
        let toml = VALID.replace(
            "contract_version = \"0.1.0\"",
            "contract_version = \"banana\"",
        );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(
            matches!(&err, ManifestError::InvalidField { field, .. } if field.contains("contract_version")),
            "got {err}"
        );
        assert!(err.to_string().contains("banana"), "got {err}");
    }

    #[test]
    fn invalid_adapter_kind_charset_is_rejected_naming_the_field() {
        // F6: a kind with whitespace/tab/newline/uppercase is rejected by the
        // adapter-kind charset rule, naming the field. (An empty kind is caught
        // separately by the missing-field arm — see empty_adapter_kind_*.)
        for bad in [
            "a\tb",
            "a b",
            "a\nb",
            "Demo",
            "_leading",
            "-leading",
            "has.dot",
            "camelCase",
        ] {
            let toml = VALID.replace("kind = \"demo\"", &format!("kind = {bad:?}"));
            let m =
                Manifest::from_toml_str(&toml).unwrap_or_else(|e| panic!("parse for {bad:?}: {e}"));
            let err = m.validate().unwrap_err();
            assert!(
                matches!(&err, ManifestError::InvalidField { field, .. } if field.contains("kind")),
                "kind={bad:?} got {err}"
            );
        }
    }

    #[test]
    fn valid_adapter_kinds_pass_the_charset() {
        // Native builtin kinds and typical manifest kinds obey the rule.
        for good in ["mock", "hermes", "demo-manifest", "a", "a1_b-2", "0abc"] {
            assert!(
                super::is_valid_adapter_kind(good),
                "expected {good:?} to be a valid adapter kind"
            );
        }
        for bad in ["", "A", "_x", "-x", "a b", "a\tb", "a.b", "naïve"] {
            assert!(
                !super::is_valid_adapter_kind(bad),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn missing_adapter_section_names_it() {
        let toml = VALID.replace("[adapter]\nkind = \"demo\"\nname = \"Demo Agent\"\n", "");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("[adapter]"), "{err}");
    }

    #[test]
    fn empty_adapter_kind_names_the_field() {
        let toml = VALID.replace("kind = \"demo\"", "kind = \"\"");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("kind"), "{err}");
        assert!(err.to_string().contains("[adapter]"), "{err}");
    }

    #[test]
    fn missing_lifecycle_section_names_it() {
        // Remove both lifecycle op tables.
        let toml = VALID
            .replace(
                "[lifecycle.start]\nexec = \"demo-agent\"\nargs = [\"--serve\"]\nenv = { DEMO_MODE = \"1\" }\n\n",
                "",
            )
            .replace(
                "[lifecycle.stop]\nexec = \"demo-agent\"\nargs = [\"--shutdown\"]\n\n",
                "",
            );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("[lifecycle]"), "got {err}");
    }

    #[test]
    fn lifecycle_without_start_names_the_start_op() {
        // Keep [lifecycle.stop] but drop start.
        let toml = VALID.replace(
            "[lifecycle.start]\nexec = \"demo-agent\"\nargs = [\"--serve\"]\nenv = { DEMO_MODE = \"1\" }\n\n",
            "",
        );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("start"), "got {err}");
        assert!(err.to_string().contains("[lifecycle]"), "got {err}");
    }

    #[test]
    fn missing_capabilities_names_the_section() {
        let toml = VALID
            .replace(
                "[capabilities.pause]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"best-effort\"\n\n",
                "",
            )
            .replace("[capabilities.interaction]\nlinux = \"guaranteed\"\n\n", "");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("[capabilities]"), "got {err}");
    }

    #[test]
    fn all_unsupported_capabilities_are_rejected() {
        // F1: a [capabilities] section that declares keys but every entry is
        // `unsupported` promises support nowhere — rejected naming the section
        // (parallels an empty declaration).
        let toml = VALID
            .replace(
                "[capabilities.pause]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"best-effort\"",
                "[capabilities.pause]\nlinux = \"unsupported\"\nmacos = \"unsupported\"\nwindows = \"unsupported\"",
            )
            .replace(
                "[capabilities.interaction]\nlinux = \"guaranteed\"",
                "[capabilities.interaction]\nlinux = \"unsupported\"",
            );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        // Parsed fine (the keys exist) but validation rejects: no real support.
        assert!(!m.capability_declaration().is_empty());
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("[capabilities]"), "got {err}");
    }

    #[test]
    fn missing_metering_names_the_section() {
        let toml = VALID.replace("[metering]\nsource = \"self-reported\"\n", "");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(
            matches!(&err, ManifestError::MissingSection { section } if section.contains("[metering]")),
            "got {err}"
        );
    }

    #[test]
    fn invalid_metering_source_fails_to_parse_naming_metering() {
        // AC4: an invalid/"none" metering source is not a viable kind. Because
        // MeteringSource has no `none` variant, this fails at PARSE. The parser's
        // raw "unknown variant" message does not name the section, so
        // `from_toml_str` attributes it to `[metering]` (the sole enum field),
        // keeping the diagnostic section-naming like every other rejection.
        let toml = VALID.replace("source = \"self-reported\"", "source = \"none\"");
        let err = Manifest::from_toml_str(&toml).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err}");
        // The diagnostic now names the section AND surfaces the offending value.
        assert!(err.to_string().contains("[metering]"), "got {err}");
        assert!(err.to_string().contains("none"), "got {err}");
        assert!(err.to_string().contains("source"), "got {err}");
    }

    #[test]
    fn malformed_toml_is_reported_as_toml_error() {
        let err = Manifest::from_toml_str("this is not = = toml").unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err}");
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        let toml = format!("{VALID}\nbogus_key = true\n");
        let err = Manifest::from_toml_str(&toml).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err}");
        assert!(err.to_string().contains("bogus_key"), "got {err}");
    }

    #[test]
    fn unknown_nested_key_is_rejected() {
        let toml = VALID.replace(
            "[metering]\nsource = \"self-reported\"",
            "[metering]\nsource = \"self-reported\"\nextra = 1",
        );
        let err = Manifest::from_toml_str(&toml).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err}");
    }

    #[test]
    fn accessors_default_when_sections_absent() {
        // A near-empty manifest: accessors return safe defaults/None without
        // panicking (validation is what rejects it).
        let m = Manifest::from_toml_str("contract_version = \"0.1.0\"").expect("parse");
        assert!(m.capability_declaration().is_empty());
        assert_eq!(m.metering_source(), None);
        assert_eq!(m.adapter_kind(), None);
        // Story 2-2: an absent `[config]` yields an EMPTY mapping (a no-op), not
        // an error.
        assert!(m.config_mapping().is_empty());
        assert!(m.validate().is_err());
    }

    // ---- Story 2-2: the optional `[config]` mapping section (FR-12) ----

    #[test]
    fn absent_config_section_validates_with_an_empty_mapping() {
        // The common case: a valid manifest with NO `[config]` section is valid,
        // and `config_mapping()` returns an empty mapping (delivers nothing).
        let m = Manifest::from_toml_str(VALID).expect("parse");
        m.validate().expect("validate");
        assert!(m.config.is_none());
        assert!(m.config_mapping().is_empty());
    }

    #[test]
    fn config_section_with_each_target_parses_validates_and_reads_back() {
        // AC3: a manifest can declare, per documented unified key, its native
        // target (env / flag / file). All three parse, validate, and read back
        // through the accessor.
        let toml = format!(
            "{VALID}\n\
             [config.model]\nenv = \"MODEL\"\n\n\
             [config.temperature]\nflag = \"--temp\"\n\n\
             [config.seed]\nfile = {{ path = \"config/agent.toml\", key = \"llm.seed\" }}\n"
        );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        m.validate().expect("validate");

        let mapping = m.config_mapping();
        assert_eq!(mapping.len(), 3);
        assert_eq!(mapping.target("model").unwrap().env_var(), Some("MODEL"));
        assert_eq!(
            mapping
                .target("temperature")
                .unwrap()
                .render_flag_args("0.7"),
            Some(["--temp".to_string(), "0.7".to_string()])
        );
        let placement = mapping.target("seed").unwrap().file_placement().unwrap();
        assert_eq!(placement.path, "config/agent.toml");
        assert_eq!(placement.key, "llm.seed");
    }

    #[test]
    fn malformed_config_rule_is_rejected_naming_the_config_subsection() {
        // A present-but-malformed rule (an empty env var name) is an InvalidField
        // naming the `[config.<key>]` sub-section.
        let toml = format!("{VALID}\n[config.model]\nenv = \"\"\n");
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(
            matches!(&err, ManifestError::InvalidField { field, .. } if field.contains("[config.model]")),
            "got {err}"
        );
    }

    #[test]
    fn config_file_path_escaping_the_home_is_rejected() {
        // A `file.path` that escapes the Agent Home is rejected (the engine is the
        // sole writer inside the home — AD-6).
        let toml = format!(
            "{VALID}\n[config.model]\nfile = {{ path = \"../escape.toml\", key = \"k\" }}\n"
        );
        let m = Manifest::from_toml_str(&toml).expect("parse");
        let err = m.validate().unwrap_err();
        assert!(err.to_string().contains("[config.model]"), "got {err}");
        assert!(err.to_string().contains("RELATIVE"), "got {err}");
    }

    #[test]
    fn config_rule_with_no_native_mechanism_fails_to_parse_deny_unknown() {
        // A `[config.<key>]` sub-table naming no known mechanism fails at PARSE
        // (untagged variant match), surfaced as a Toml error — like every other
        // typo the manifest's deny_unknown_fields style catches.
        let toml = format!("{VALID}\n[config.model]\nbogus = \"x\"\n");
        let err = Manifest::from_toml_str(&toml).unwrap_err();
        assert!(matches!(err, ManifestError::Toml { .. }), "got {err}");
    }
}
