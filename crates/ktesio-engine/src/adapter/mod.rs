//! Adapter resolution + manifest loading/validation (spine AD-3).
//!
//! This module RESOLVES a requested adapter — a native builtin by kind, or a
//! manifest by path — into a [`ResolvedAdapter`]: a uniform, parsed view the
//! registry consumes (its kind, its effective current-OS Capability
//! Declaration, its Metering Source, and, for manifest adapters, the manifest
//! path 1-4 will need to launch it).
//!
//! ## Boundary: PARSE + VALIDATE only — EXECUTES NOTHING (CRITICAL)
//!
//! The loader reads, parses (via [`ktesio_adapter_api::Manifest`]), and
//! validates. It stores templates and declarations. It does **not** run any
//! lifecycle op, spawn any process, or add tokio — that is story 1-4's manifest
//! executor. The `AgentAdapter` lifecycle methods are never called here; only
//! the declaration accessors are read.
//!
//! ## Schema ownership (AD-3)
//!
//! The engine defines **no** manifest schema. It consumes
//! `ktesio-adapter-api`'s parsed [`Manifest`]/[`CapabilityDeclaration`]/
//! [`MeteringSource`] types. The manifest-adapter view built here is derived
//! entirely from that crate's validated output.
//!
//! ## Native builtins and the conformance boundary
//!
//! The shipping engine resolves native kinds through a small builtin table
//! ([`resolve_native`]). The `mock` kind resolves to an **engine-internal**
//! [`builtin::BuiltinMock`], deliberately NOT the `ktesio-conformance`
//! `MockAdapter`: a normal `engine → conformance` edge would be transitive into
//! `kt` and trip the AD-2 boundary gate. The conformance mock stays a
//! dev/test fixture (imported as a dev-dependency where tests need it); the two
//! share the same declared shape.

mod builtin;

use std::path::{Path, PathBuf};

use ktesio_adapter_api::{
    CapabilityDeclaration, EffectiveCapabilities, Manifest, ManifestError, MeteringSource, OsId,
};

use thiserror::Error;

/// The canonical `adapter.toml` filename inside a manifest-adapter directory.
pub const MANIFEST_FILE: &str = "adapter.toml";

/// A request to resolve an adapter for a registration (spine AD-3 "two kinds").
///
/// `kt` builds this from its flags: `--kind <kind>` → [`AdapterRef::Native`],
/// `--manifest <path>` → [`AdapterRef::Manifest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterRef {
    /// A native, compiled-in adapter selected by its kind string (e.g. `mock`).
    Native(String),
    /// A manifest adapter loaded from a directory (or a direct file) path.
    Manifest(PathBuf),
}

/// A resolved, validated adapter view the registry persists with an instance.
///
/// Uniform across both kinds (AD-3 "two kinds, one trait"): the registry never
/// branches on native-vs-manifest after resolution. Holds the effective
/// current-OS Capability Declaration (already projected) plus the full
/// declaration (for completeness) and the Metering Source.
#[derive(Clone, Debug)]
pub struct ResolvedAdapter {
    /// The value stored in the instance `kind` column.
    kind: String,
    /// The full per-OS Capability Declaration (AD-4).
    declaration: CapabilityDeclaration,
    /// The declared Metering Source (AD-7; always viable — validated).
    metering_source: MeteringSource,
    /// For a manifest adapter, the resolved absolute manifest path (1-4 needs
    /// it to launch). `None` for a native adapter.
    manifest_path: Option<PathBuf>,
}

impl ResolvedAdapter {
    /// The kind string to store on the Agent Instance.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The full per-OS Capability Declaration.
    pub fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    /// The declared Metering Source.
    pub fn metering_source(&self) -> MeteringSource {
        self.metering_source
    }

    /// The manifest path, if this is a manifest adapter.
    pub fn manifest_path(&self) -> Option<&Path> {
        self.manifest_path.as_deref()
    }

    /// Project the declaration onto `os` (the effective current-OS view).
    pub fn effective(&self, os: OsId) -> EffectiveCapabilities {
        self.declaration.effective(os)
    }
}

/// Why an adapter could not be resolved/validated (engine-internal).
///
/// The registry maps each variant to a `RegistryError` so `kt` can render a
/// remediation hint. `thiserror`, never `miette` (conventions). Each carries
/// enough context to name the problem (the kind, the path, or the section).
#[derive(Debug, Error)]
pub enum AdapterResolveError {
    /// A native kind was requested that no builtin provides.
    #[error("unknown adapter kind '{kind}'")]
    UnknownKind {
        /// The unrecognized kind string.
        kind: String,
    },

    /// The manifest file did not exist at the resolved path.
    #[error("no adapter.toml found at {path}")]
    ManifestNotFound {
        /// The path that was searched (the file, or `<dir>/adapter.toml`).
        path: String,
    },

    /// The manifest existed but could not be read.
    #[error("could not read adapter.toml at {path}: {detail}")]
    ManifestUnreadable {
        /// The manifest path.
        path: String,
        /// The underlying I/O error.
        detail: String,
    },

    /// The manifest parsed/validated with an error naming the failing section.
    #[error("adapter.toml at {path} is invalid: {detail}")]
    ManifestInvalid {
        /// The manifest path.
        path: String,
        /// The section-naming detail from [`ManifestError`].
        detail: String,
    },

    /// The adapter declared no viable Metering Source (FR-19 hard line, AC4).
    ///
    /// For a manifest this is caught by [`Manifest::validate`] (a missing
    /// `[metering]` section), but the registry re-checks the resolved adapter so
    /// the FR-19 line is enforced uniformly for native adapters too.
    #[error("adapter '{adapter}' declares no viable Metering Source (the `[metering]` section)")]
    NoMeteringSource {
        /// The adapter kind/identity.
        adapter: String,
    },

    /// The adapter's Capability Declaration was empty (AC2 seed requirement).
    #[error("adapter '{adapter}' declares no capabilities (the `[capabilities]` section)")]
    NoCapabilities {
        /// The adapter kind/identity.
        adapter: String,
    },
}

/// The resolved `start` launch of an adapter (story 1.4): exec + args + env.
///
/// Built by [`resolve_start_launch`] from a manifest adapter's
/// `[lifecycle.start]` [`OpTemplate`](ktesio_adapter_api::OpTemplate) or a native
/// adapter's equivalent. The supervisor turns this into a
/// [`SpawnSpec`](crate::ports::SpawnSpec) (adding the working dir + log file) and
/// hands it to the [`ProcessBackend`](crate::ports::ProcessBackend).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartLaunch {
    /// The executable to run (program name or path).
    pub exec: String,
    /// Positional arguments.
    pub args: Vec<String>,
    /// Environment overrides.
    pub env: std::collections::BTreeMap<String, String>,
}

/// Why a `start` launch could not be resolved (story 1.4).
#[derive(Debug, Error)]
pub enum LaunchResolveError {
    /// The stored manifest could not be read/parsed (corrupt or now-missing).
    #[error("could not read the adapter manifest at {path}: {detail}")]
    ManifestUnreadable {
        /// The manifest path.
        path: String,
        /// The underlying detail.
        detail: String,
    },

    /// The manifest parsed but declares no `[lifecycle.start]` `exec` (should not
    /// happen for a manifest that passed registration validation, but guarded).
    #[error("adapter manifest at {path} declares no `[lifecycle.start]` exec")]
    NoStartTemplate {
        /// The manifest path.
        path: String,
    },

    /// The adapter is a native builtin with no launch command. Native builtins
    /// (e.g. `mock`) carry no process to spawn this story; a launchable agent is
    /// supplied as a manifest adapter whose `[lifecycle.start]` exec points at
    /// the real program (AD-3). Names the kind.
    #[error(
        "native adapter kind '{kind}' has no launch command; supply a manifest adapter (its `[lifecycle.start]` exec) to start a real process"
    )]
    NativeHasNoLaunch {
        /// The native kind.
        kind: String,
    },
}

/// Resolve the `start` launch for an adapter, given its persisted snapshot
/// facts (story 1.4).
///
/// * A **manifest** adapter (`manifest_path` is `Some`) re-reads its
///   `adapter.toml` and returns the `[lifecycle.start]` template's exec/args/env
///   (reusing [`ktesio_adapter_api::Manifest`] — the same parser registration
///   used). This is where 1-3's stored `OpTemplate` is finally EXECUTED (AD-3).
/// * A **native** adapter (`manifest_path` is `None`) has no launch command this
///   story (the builtin `mock` is inert) → [`LaunchResolveError::NativeHasNoLaunch`].
///
/// PARSE only — executes nothing here (the supervisor spawns).
pub fn resolve_start_launch(
    kind: &str,
    manifest_path: Option<&Path>,
) -> Result<StartLaunch, LaunchResolveError> {
    let Some(manifest_path) = manifest_path else {
        return Err(LaunchResolveError::NativeHasNoLaunch {
            kind: kind.to_string(),
        });
    };
    let text = std::fs::read_to_string(manifest_path).map_err(|e| {
        LaunchResolveError::ManifestUnreadable {
            path: manifest_path.to_string_lossy().into_owned(),
            detail: e.to_string(),
        }
    })?;
    let manifest =
        Manifest::from_toml_str(&text).map_err(|e| LaunchResolveError::ManifestUnreadable {
            path: manifest_path.to_string_lossy().into_owned(),
            detail: e.to_string(),
        })?;
    let start = manifest
        .lifecycle
        .as_ref()
        .and_then(|l| l.start.as_ref())
        .ok_or_else(|| LaunchResolveError::NoStartTemplate {
            path: manifest_path.to_string_lossy().into_owned(),
        })?;
    Ok(StartLaunch {
        exec: start.exec.clone(),
        args: start.args.clone(),
        env: start.env.clone(),
    })
}

/// Resolve an [`AdapterRef`] into a validated [`ResolvedAdapter`].
///
/// PARSE + VALIDATE only — executes nothing. On success the returned adapter is
/// guaranteed to have a non-empty Capability Declaration and a viable Metering
/// Source (the FR-19 hard line), so the registry can proceed to the atomic
/// row-insert / home-creation with confidence.
pub fn resolve(reference: &AdapterRef) -> Result<ResolvedAdapter, AdapterResolveError> {
    let resolved = match reference {
        AdapterRef::Native(kind) => resolve_native(kind)?,
        AdapterRef::Manifest(path) => resolve_manifest(path)?,
    };
    enforce_registration_invariants(&resolved)?;
    Ok(resolved)
}

/// Resolve a native kind through the engine's builtin table.
fn resolve_native(kind: &str) -> Result<ResolvedAdapter, AdapterResolveError> {
    let adapter = builtin::native(kind).ok_or_else(|| AdapterResolveError::UnknownKind {
        kind: kind.to_string(),
    })?;
    Ok(ResolvedAdapter {
        kind: adapter.kind().to_string(),
        declaration: adapter.capabilities().clone(),
        metering_source: adapter.metering_source(),
        manifest_path: None,
    })
}

/// Load, parse, and validate a manifest adapter from a directory or file path.
fn resolve_manifest(path: &Path) -> Result<ResolvedAdapter, AdapterResolveError> {
    // A directory means "<dir>/adapter.toml"; a file path is used verbatim. This
    // lets an operator point at either the Agent's directory or the file.
    let manifest_path = if path.is_dir() {
        path.join(MANIFEST_FILE)
    } else {
        path.to_path_buf()
    };

    if !manifest_path.exists() {
        return Err(AdapterResolveError::ManifestNotFound {
            path: manifest_path.to_string_lossy().into_owned(),
        });
    }

    let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        AdapterResolveError::ManifestUnreadable {
            path: manifest_path.to_string_lossy().into_owned(),
            detail: e.to_string(),
        }
    })?;

    let manifest = Manifest::from_toml_str(&text).map_err(|e| to_invalid(&manifest_path, e))?;
    manifest
        .validate()
        .map_err(|e| to_invalid(&manifest_path, e))?;

    // Validated: the declaration is non-empty and a viable source is present.
    let metering_source =
        manifest
            .metering_source()
            .ok_or_else(|| AdapterResolveError::NoMeteringSource {
                adapter: manifest.adapter_kind().unwrap_or("manifest").to_string(),
            })?;

    let kind = manifest.adapter_kind().unwrap_or("manifest").to_string();

    Ok(ResolvedAdapter {
        kind,
        declaration: manifest.capability_declaration(),
        metering_source,
        manifest_path: Some(manifest_path),
    })
}

/// Re-check the FR-19 / AC2 registration invariants on a resolved adapter,
/// uniformly for both kinds. A manifest failure is already caught by
/// `validate()`, but native adapters must clear the same bar.
fn enforce_registration_invariants(resolved: &ResolvedAdapter) -> Result<(), AdapterResolveError> {
    // The same bar the manifest clears (F1): not merely "has a capability key"
    // but "declares real support on at least one OS". A native adapter whose
    // declaration is empty OR all-`unsupported` is rejected here, uniformly.
    if !resolved.declaration.has_any_support() {
        return Err(AdapterResolveError::NoCapabilities {
            adapter: resolved.kind.clone(),
        });
    }
    // The MeteringSource type only holds viable kinds, so its presence in a
    // ResolvedAdapter already proves viability; there is no "none" to reject
    // here. The variant exists for symmetry / future native adapters that might
    // compute a source dynamically.
    Ok(())
}

/// Map a [`ManifestError`] to the invalid-manifest resolve error, preserving the
/// section-naming message (AC2).
fn to_invalid(path: &Path, err: ManifestError) -> AdapterResolveError {
    AdapterResolveError::ManifestInvalid {
        path: path.to_string_lossy().into_owned(),
        detail: err.to_string(),
    }
}

impl From<AdapterResolveError> for crate::domain::RegistryError {
    /// Translate an adapter-resolution failure into the registry's error surface
    /// so `kt` can render a remediation hint per variant. Each resolve error maps
    /// to a distinct registry variant, including an unreadable manifest (whose
    /// remediation — check existence/readability — differs from "fix the
    /// section", F4).
    fn from(err: AdapterResolveError) -> Self {
        use crate::domain::RegistryError as R;
        match err {
            AdapterResolveError::UnknownKind { kind } => R::UnknownAdapterKind { kind },
            AdapterResolveError::ManifestNotFound { path } => R::ManifestNotFound { path },
            AdapterResolveError::ManifestUnreadable { path, detail } => {
                R::ManifestUnreadable { path, detail }
            }
            AdapterResolveError::ManifestInvalid { path, detail } => {
                R::ManifestInvalid { path, detail }
            }
            AdapterResolveError::NoMeteringSource { adapter } => R::NoMeteringSource { adapter },
            AdapterResolveError::NoCapabilities { adapter } => R::NoCapabilities { adapter },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    const VALID_MANIFEST: &str = r#"
contract_version = "0.1.0"

[adapter]
kind = "demo"

[lifecycle.start]
exec = "demo-agent"

[capabilities.pause]
linux = "guaranteed"
macos = "guaranteed"
windows = "best-effort"

[metering]
source = "self-reported"
"#;

    fn write_manifest(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join(MANIFEST_FILE);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn native_mock_resolves_with_declaration_and_metering() {
        let resolved = resolve(&AdapterRef::Native("mock".to_string())).unwrap();
        assert_eq!(resolved.kind(), "mock");
        assert_eq!(resolved.metering_source(), MeteringSource::SelfReported);
        assert!(!resolved.declaration().is_empty());
        assert!(resolved.manifest_path().is_none());

        // Per-OS projection works as data on any host.
        for os in OsId::MODELED {
            let eff = resolved.effective(os);
            assert!(!eff.is_empty(), "os={os}");
        }
    }

    #[test]
    fn unknown_native_kind_is_rejected() {
        let err = resolve(&AdapterRef::Native("nope".to_string())).unwrap_err();
        assert!(matches!(err, AdapterResolveError::UnknownKind { kind } if kind == "nope"));
    }

    #[test]
    fn manifest_dir_resolves_and_records_path() {
        let tmp = TempDir::new().unwrap();
        write_manifest(tmp.path(), VALID_MANIFEST);
        // Point at the DIRECTORY — the loader appends adapter.toml.
        let resolved = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap();
        assert_eq!(resolved.kind(), "demo");
        assert_eq!(resolved.metering_source(), MeteringSource::SelfReported);
        assert_eq!(
            resolved.manifest_path().unwrap(),
            tmp.path().join(MANIFEST_FILE)
        );
    }

    #[test]
    fn manifest_file_path_resolves_verbatim() {
        let tmp = TempDir::new().unwrap();
        let file = write_manifest(tmp.path(), VALID_MANIFEST);
        // Point at the FILE directly.
        let resolved = resolve(&AdapterRef::Manifest(file.clone())).unwrap();
        assert_eq!(resolved.manifest_path().unwrap(), file.as_path());
    }

    #[test]
    fn missing_manifest_is_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap_err();
        match err {
            AdapterResolveError::ManifestNotFound { path } => {
                assert!(path.ends_with(MANIFEST_FILE), "{path}")
            }
            other => panic!("expected ManifestNotFound, got {other}"),
        }
    }

    #[test]
    fn invalid_manifest_names_the_section() {
        let tmp = TempDir::new().unwrap();
        // Drop the [metering] section.
        let body = VALID_MANIFEST.replace("[metering]\nsource = \"self-reported\"\n", "");
        write_manifest(tmp.path(), &body);
        let err = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap_err();
        match err {
            AdapterResolveError::ManifestInvalid { detail, .. } => {
                assert!(detail.contains("[metering]"), "{detail}")
            }
            other => panic!("expected ManifestInvalid, got {other}"),
        }
    }

    #[test]
    fn manifest_with_no_metering_is_rejected_as_invalid_naming_metering() {
        // AC4 at the manifest layer: a missing [metering] fails validate() and
        // surfaces as ManifestInvalid naming the section (the manifest never
        // reaches the NoMeteringSource re-check because validate() rejects it
        // first — both name `[metering]`).
        let tmp = TempDir::new().unwrap();
        let body = VALID_MANIFEST.replace("[metering]\nsource = \"self-reported\"\n", "");
        write_manifest(tmp.path(), &body);
        let err = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap_err();
        assert!(err.to_string().contains("[metering]"), "{err}");
    }

    #[test]
    fn malformed_manifest_is_invalid() {
        let tmp = TempDir::new().unwrap();
        write_manifest(tmp.path(), "not = = valid toml");
        let err = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap_err();
        assert!(matches!(err, AdapterResolveError::ManifestInvalid { .. }));
    }

    #[test]
    fn manifest_missing_capabilities_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let body = VALID_MANIFEST.replace(
            "[capabilities.pause]\nlinux = \"guaranteed\"\nmacos = \"guaranteed\"\nwindows = \"best-effort\"\n",
            "",
        );
        write_manifest(tmp.path(), &body);
        let err = resolve(&AdapterRef::Manifest(tmp.path().to_path_buf())).unwrap_err();
        assert!(err.to_string().contains("[capabilities]"), "{err}");
    }

    #[test]
    fn adapter_ref_equality() {
        assert_eq!(
            AdapterRef::Native("mock".to_string()),
            AdapterRef::Native("mock".to_string())
        );
        assert_ne!(
            AdapterRef::Native("mock".to_string()),
            AdapterRef::Manifest(PathBuf::from("/x"))
        );
    }

    #[test]
    fn enforce_invariants_rejects_all_unsupported_native_declaration() {
        // F1: a native adapter whose declaration has keys but zero real support
        // (all `unsupported`) must be rejected at the registration bar, exactly
        // like the manifest path — not slip through because a key exists.
        use ktesio_adapter_api::{Capability, SupportLevel};
        let declaration = CapabilityDeclaration::new()
            .with(Capability::Pause, OsId::Linux, SupportLevel::Unsupported)
            .with(
                Capability::Interaction,
                OsId::Macos,
                SupportLevel::Unsupported,
            );
        assert!(!declaration.is_empty(), "keys are present");
        let resolved = ResolvedAdapter {
            kind: "hollow".to_string(),
            declaration,
            metering_source: MeteringSource::SelfReported,
            manifest_path: None,
        };
        let err = enforce_registration_invariants(&resolved).unwrap_err();
        assert!(
            matches!(err, AdapterResolveError::NoCapabilities { adapter } if adapter == "hollow"),
            "expected NoCapabilities"
        );
    }

    // ---- Story 1.4: resolve_start_launch (the manifest OpTemplate → launch) ----

    #[test]
    fn resolve_start_launch_reads_the_manifest_start_template() {
        // A manifest adapter's [lifecycle.start] exec/args/env become the launch.
        let tmp = TempDir::new().unwrap();
        let body = r#"
contract_version = "0.1.0"
[adapter]
kind = "demo"
[lifecycle.start]
exec = "the-agent"
args = ["--serve", "--port", "0"]
env = { MODE = "test" }
[capabilities.pause]
linux = "guaranteed"
[metering]
source = "self-reported"
"#;
        let path = write_manifest(tmp.path(), body);
        let launch = resolve_start_launch("demo", Some(&path)).unwrap();
        assert_eq!(launch.exec, "the-agent");
        assert_eq!(launch.args, vec!["--serve", "--port", "0"]);
        assert_eq!(launch.env.get("MODE").map(String::as_str), Some("test"));
    }

    #[test]
    fn resolve_start_launch_native_has_no_launch_command() {
        // A native adapter (manifest_path None) has no launch command this story.
        let err = resolve_start_launch("mock", None).unwrap_err();
        assert!(
            matches!(&err, LaunchResolveError::NativeHasNoLaunch { kind } if kind == "mock"),
            "got {err}"
        );
        assert!(err.to_string().contains("no launch command"));
    }

    #[test]
    fn resolve_start_launch_unreadable_manifest_is_reported() {
        // A manifest path that does not exist → ManifestUnreadable.
        let missing = std::path::Path::new("/no/such/adapter.toml");
        let err = resolve_start_launch("demo", Some(missing)).unwrap_err();
        assert!(
            matches!(err, LaunchResolveError::ManifestUnreadable { .. }),
            "got {err}"
        );
    }

    #[test]
    fn resolve_start_launch_malformed_manifest_is_unreadable() {
        // A manifest that fails to PARSE surfaces as ManifestUnreadable (the
        // launch path re-reads it; a corrupt file cannot yield a launch).
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(tmp.path(), "not = = valid toml");
        let err = resolve_start_launch("demo", Some(&path)).unwrap_err();
        assert!(matches!(err, LaunchResolveError::ManifestUnreadable { .. }));
    }

    #[test]
    fn resolve_start_launch_missing_start_template_is_reported() {
        // A manifest with a [lifecycle] table but no start op → NoStartTemplate.
        // (Parses fine — validate() would reject it at registration, but the
        // launch resolver guards defensively.)
        let tmp = TempDir::new().unwrap();
        let body = r#"
contract_version = "0.1.0"
[adapter]
kind = "demo"
[lifecycle.stop]
exec = "stopper"
[capabilities.pause]
linux = "guaranteed"
[metering]
source = "self-reported"
"#;
        let path = write_manifest(tmp.path(), body);
        let err = resolve_start_launch("demo", Some(&path)).unwrap_err();
        assert!(
            matches!(err, LaunchResolveError::NoStartTemplate { .. }),
            "got {err}"
        );
    }

    #[test]
    fn enforce_invariants_accepts_a_supported_declaration() {
        use ktesio_adapter_api::{Capability, SupportLevel};
        let declaration = CapabilityDeclaration::new().with(
            Capability::Pause,
            OsId::Linux,
            SupportLevel::Guaranteed,
        );
        let resolved = ResolvedAdapter {
            kind: "ok".to_string(),
            declaration,
            metering_source: MeteringSource::SelfReported,
            manifest_path: None,
        };
        assert!(enforce_registration_invariants(&resolved).is_ok());
    }
}
