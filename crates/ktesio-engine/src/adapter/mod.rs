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
    CapabilityDeclaration, ConfigMapping, ConfigTarget, EffectiveCapabilities, Manifest,
    ManifestError, MeteringSource, OsId,
};

use crate::domain::{pass_through_tail, EffectiveConfig};

use thiserror::Error;

pub use builtin::native_config_mapping;

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

    /// The adapter's config mapping (story 2-2, FR-12) is invalid — a malformed
    /// rule, or a `File` target whose `path` is absolute or escapes the Agent
    /// Home. Applied symmetrically to BOTH kinds ([`resolve_config_mapping`]): a
    /// manifest mapping is validated at registration, but re-checked here; a
    /// NATIVE (code-declared) mapping is validated here too, so a native `File`
    /// target can never escape the home (AD-6 — symmetric trust). Names the
    /// offending unified key + why.
    #[error("adapter '{adapter}' has an invalid config mapping for key '{key}': {detail}")]
    InvalidConfigMapping {
        /// The adapter kind/identity whose mapping is invalid.
        adapter: String,
        /// The offending unified key.
        key: String,
        /// Why the rule is invalid.
        detail: String,
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

/// Why applying the adapter's config mapping to a launch failed (story 2-2). The
/// only failure is rendering a FILE target into the Agent Home — env/flag targets
/// are pure in-memory mutations of the launch that cannot fail. The supervisor
/// maps this into its launch error surface (never a panic).
///
/// ATOMICITY (accurate guarantee): the mapping is applied BEFORE the `starting`
/// transition, so a file-render failure REJECTS the start and the instance stays
/// in its PRIOR state (registered/stopped/failed) — the start STATE is atomic
/// (all mapping failures reject before any state change). It is NOT a
/// whole-filesystem atomic write: multi-file apply is not atomic across files, so
/// a failure on a LATER file can leave an EARLIER file already rendered in the
/// Agent Home (a harmless stale artifact the next successful start overwrites). An
/// atomic temp-then-rename per rendered file is a deferred follow-up (same family
/// as the existing non-atomic-write item).
#[derive(Debug, Error)]
pub enum ConfigApplyError {
    /// A FILE-target config file could not be rendered/written into the Agent
    /// Home. Names the unified key, the target path, and the underlying detail.
    #[error(
        "could not render config key '{key}' into the file '{path}' in the Agent Home: {detail}"
    )]
    FileRender {
        /// The unified key whose file target failed.
        key: String,
        /// The target path (relative to the Agent Home).
        path: String,
        /// The underlying I/O or serialization detail.
        detail: String,
    },
}

/// Resolve the adapter's unified→native config [`ConfigMapping`] for a start
/// (story 2-2, AC3): a MANIFEST adapter's mapping comes from its parsed `[config]`
/// section; a NATIVE adapter's from the builtin table's code-declared mapping.
/// Both yield the same uniform [`ConfigMapping`] the start seam applies (AD-3
/// "two kinds, one trait"). A manifest that cannot be re-read/parsed surfaces the
/// same [`LaunchResolveError::ManifestUnreadable`] as [`resolve_start_launch`]
/// (the launch already re-read it, but this stays defensive + symmetric). An
/// unknown native kind yields an EMPTY mapping (delivers nothing) rather than an
/// error — the launch resolution already rejected an unknown/native-only kind.
pub fn resolve_config_mapping(
    kind: &str,
    manifest_path: Option<&Path>,
) -> Result<ConfigMapping, LaunchResolveError> {
    let mapping = match manifest_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| {
                LaunchResolveError::ManifestUnreadable {
                    path: path.to_string_lossy().into_owned(),
                    detail: e.to_string(),
                }
            })?;
            let manifest = Manifest::from_toml_str(&text).map_err(|e| {
                LaunchResolveError::ManifestUnreadable {
                    path: path.to_string_lossy().into_owned(),
                    detail: e.to_string(),
                }
            })?;
            manifest.config_mapping()
        }
        // A native adapter's mapping is code-declared; an unknown kind → empty.
        None => native_config_mapping(kind).unwrap_or_default(),
    };
    // Validate the mapping SYMMETRICALLY for both kinds (AD-6 — symmetric trust):
    // a manifest mapping is validated at registration, but re-checked here; a
    // NATIVE code-declared mapping is validated here too, so a native `File`
    // target can never be absolute or escape the Agent Home. A malformed rule
    // rejects the start (a bad rule is an adapter authoring bug).
    if let Err((key, detail)) = mapping.validate() {
        return Err(LaunchResolveError::InvalidConfigMapping {
            adapter: kind.to_string(),
            key,
            detail,
        });
    }
    Ok(mapping)
}

/// APPLY the adapter's config mapping to a [`StartLaunch`], from the resolved
/// [`EffectiveConfig`] (story 2-2 — the heart of AC-A/AC4/AC5/AC6). Runs at start,
/// after the launch's exec/args/env are read from the `[lifecycle.start]`
/// template and BEFORE the `SpawnSpec` is built, so the spawned process already
/// reflects the mapped native config.
///
/// For every resolved leaf (`dotted key → value`), in the effective config's
/// deterministic sorted order:
/// * a leaf under the `agent.*` PASS-THROUGH namespace is delivered VERBATIM
///   (AC6): the key-tail after `agent.` + the value, with NO known-key mapping
///   lookup and NO rewriting. The recorded delivery convention (Decision 5): a
///   pass-through key maps to an ENV var named by its verbatim key-tail
///   (`agent.FOO=bar` → env `FOO=bar`), the value as-is;
/// * any other leaf is a documented KNOWN key: if the adapter's `mapping` declares
///   a rule for it, the value lands in that rule's native target — **env** →
///   inserted into [`StartLaunch::env`]; **flag** → appended to
///   [`StartLaunch::args`] as two tokens (`--model` `gpt-4`); **file** →
///   rendered into a native TOML file in the Agent `home` (the engine is the sole
///   writer — path authority). A documented key with NO rule is a no-op
///   (Decision 6 — not every adapter maps every unified key).
///
/// PURE for env/flag (in-memory mutation); FILE targets are the only side effect
/// (a write into `home`), and a bad/unwritable target is a typed
/// [`ConfigApplyError::FileRender`] the supervisor lands as a launch failure. The
/// resolved-config → launch transform is deterministic (sorted iteration + a
/// per-file merge keyed by the native key), so the same inputs always yield the
/// same launch + files.
///
/// SEAMS for later stories (recorded, built here as no-ops): a leaf's value is a
/// plain string here; story 2-4 will intercept a `secret:` value and resolve/mask
/// it BEFORE this placement (here it is delivered as-is, opaque). The per-leaf
/// `SourceLayer` (2-1) rides on the `EffectiveConfig` untouched; 2-3 renders it.
pub fn apply_config_mapping(
    launch: &mut StartLaunch,
    mapping: &ConfigMapping,
    effective: &EffectiveConfig,
    home: &Path,
) -> Result<(), ConfigApplyError> {
    // Accumulate FILE-target writes keyed by target path, so multiple keys
    // mapping into the SAME file merge into one rendered document (deterministic:
    // the effective config iterates sorted, and each file's keys are set into a
    // sorted TOML table). Rendered + written once at the end.
    let mut files: std::collections::BTreeMap<String, FileDoc> = std::collections::BTreeMap::new();

    for (dotted_key, resolved) in effective.iter() {
        let value = resolved.display();
        if let Some(tail) = pass_through_tail(dotted_key) {
            // AC6: pass-through delivered VERBATIM into the native mechanism (the
            // recorded convention: an env var named by the verbatim key-tail).
            // NO known-key mapping lookup, NO rewriting of the tail or value.
            launch.env.insert(tail.to_string(), value);
            continue;
        }
        // A documented known key: apply the adapter's rule if it declares one.
        let Some(target) = mapping.target(dotted_key) else {
            // Decision 6: a documented key the adapter does not map is a no-op.
            continue;
        };
        match target {
            ConfigTarget::Env { env } => {
                launch.env.insert(env.clone(), value);
            }
            ConfigTarget::Flag { .. } => {
                if let Some([flag, val]) = target.render_flag_args(&value) {
                    launch.args.push(flag);
                    launch.args.push(val);
                }
            }
            ConfigTarget::File(file_target) => {
                let placement = &file_target.file;
                files
                    .entry(placement.path.clone())
                    .or_default()
                    .set(&placement.key, value);
            }
        }
    }

    // Render + write each accumulated file into the Agent Home (sole writer,
    // AD-6). The path was validated RELATIVE at manifest-load time; a native
    // mapping is trusted (code-declared). Join defensively onto the home.
    for (rel_path, doc) in files {
        write_config_file(home, &rel_path, &doc)?;
    }
    Ok(())
}

/// An in-progress native config FILE document being assembled by
/// [`apply_config_mapping`] (a set of dotted native keys → string values). Kept a
/// small newtype so the per-file merge is explicit + testable; serialized to TOML
/// once, deterministically (a [`std::collections::BTreeMap`] sorts its keys).
#[derive(Debug, Default)]
struct FileDoc {
    entries: std::collections::BTreeMap<String, String>,
}

impl FileDoc {
    /// Set a dotted native key to `value` (last write wins — the effective config
    /// yields one value per unified key, so a collision only arises if two unified
    /// keys map to the SAME file+native-key, which is an adapter authoring choice).
    fn set(&mut self, key: &str, value: String) {
        self.entries.insert(key.to_string(), value);
    }

    /// Render to a TOML document string: each dotted native key set as a nested
    /// TOML value (`llm.model = "gpt-4"` → `[llm]\nmodel = "gpt-4"`). Reuses the
    /// same dotted-set discipline the instance-config writer uses, so a native key
    /// path lands in the right nested table.
    fn to_toml_string(&self) -> Result<String, String> {
        let mut table = toml::value::Table::new();
        for (key, value) in &self.entries {
            set_dotted_string(&mut table, key, value.clone());
        }
        toml::to_string_pretty(&table).map_err(|e| e.to_string())
    }
}

/// Set a DOTTED native key (`a.b.c`) into a TOML table as a string value,
/// creating intermediate tables. A collision where an intermediate is already a
/// non-table value overwrites it with a table (the rendered native file is
/// engine-authored from the mapping, not user input — there is no existing scalar
/// to preserve, unlike the instance-config write path which fails closed).
fn set_dotted_string(table: &mut toml::value::Table, dotted_key: &str, value: String) {
    let mut segments = dotted_key.split('.').peekable();
    let mut current = table;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current.insert(segment.to_string(), toml::Value::String(value));
            return;
        }
        let entry = current
            .entry(segment.to_string())
            .or_insert_with(|| toml::Value::Table(toml::value::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::value::Table::new());
        }
        current = entry
            .as_table_mut()
            .expect("intermediate ensured to be a table");
    }
}

/// Render `doc` into the file at `rel_path` inside the Agent `home` (the engine is
/// the sole writer — AD-6). Creates parent directories under the home as needed. A
/// write/serialize failure is a typed [`ConfigApplyError::FileRender`] naming the
/// key path + detail (never a panic). `rel_path` was validated relative at
/// manifest load; joining it onto `home` therefore stays inside the home.
fn write_config_file(home: &Path, rel_path: &str, doc: &FileDoc) -> Result<(), ConfigApplyError> {
    let body = doc
        .to_toml_string()
        .map_err(|detail| ConfigApplyError::FileRender {
            key: rel_path.to_string(),
            path: rel_path.to_string(),
            detail,
        })?;
    let full = home.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigApplyError::FileRender {
            key: rel_path.to_string(),
            path: rel_path.to_string(),
            detail: e.to_string(),
        })?;
    }
    std::fs::write(&full, body).map_err(|e| ConfigApplyError::FileRender {
        key: rel_path.to_string(),
        path: rel_path.to_string(),
        detail: e.to_string(),
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

    // ---- Story 2-2: apply_config_mapping (the resolved-config → launch transform) ----

    use crate::domain::{ConfigLayer, EffectiveConfig, SourceLayer};

    /// Build an [`EffectiveConfig`] from a single instance-layer TOML body (the
    /// other three layers empty) — the resolved input `apply_config_mapping`
    /// consumes.
    fn effective_from_instance(body: &str) -> EffectiveConfig {
        let layers = [
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::parse(SourceLayer::Instance, "<test>", body).unwrap(),
            ConfigLayer::empty(),
        ];
        crate::domain::resolve(layers)
    }

    /// An empty launch (exec only) to apply a mapping onto.
    fn empty_launch() -> StartLaunch {
        StartLaunch {
            exec: "the-agent".to_string(),
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn apply_maps_model_to_env_target() {
        // AC-A / AC4 (env): a `model` value lands in the declared env var.
        let mapping = ConfigMapping::new().with("model", ConfigTarget::env("MODEL"));
        let effective = effective_from_instance("model = \"gpt-4\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        assert_eq!(launch.env.get("MODEL").map(String::as_str), Some("gpt-4"));
        assert!(launch.args.is_empty());
    }

    #[test]
    fn apply_maps_model_to_flag_target_as_two_args() {
        // AC-A / AC4 (flag): a `model` value appends `--model gpt-4` to the args.
        let mapping = ConfigMapping::new().with("model", ConfigTarget::flag("--model"));
        let effective = effective_from_instance("model = \"gpt-4\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        assert_eq!(
            launch.args,
            vec!["--model".to_string(), "gpt-4".to_string()]
        );
        assert!(launch.env.is_empty());
    }

    #[test]
    fn apply_maps_model_to_file_target_rendered_into_home() {
        // AC-A / AC4 (file): a `model` value renders into a native TOML file in the
        // Agent Home, at the declared native key path.
        let mapping = ConfigMapping::new().with(
            "model",
            ConfigTarget::file("config/agent.toml", "llm.model"),
        );
        let effective = effective_from_instance("model = \"gpt-4\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        // The launch env/args are untouched (the value went into the file).
        assert!(launch.env.is_empty());
        assert!(launch.args.is_empty());
        // The rendered file exists at the declared path with the native key set.
        let rendered = tmp.path().join("config/agent.toml");
        assert!(rendered.is_file(), "file target must render into the home");
        let text = std::fs::read_to_string(&rendered).unwrap();
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(
            parsed["llm"]["model"].as_str(),
            Some("gpt-4"),
            "native key path set; got {text}"
        );
    }

    #[test]
    fn apply_delivers_agent_pass_through_verbatim_to_env() {
        // AC6: an `agent.*` leaf is delivered VERBATIM (the key-tail after `agent.`
        // + the value) into the native mechanism — NO known-key lookup, NO
        // rewriting. The recorded convention delivers it to an env var named by the
        // verbatim tail.
        let mapping = ConfigMapping::new(); // no known-key rules at all
        let effective = effective_from_instance("[agent]\nCUSTOM_FLAG = \"verbatim\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        assert_eq!(
            launch.env.get("CUSTOM_FLAG").map(String::as_str),
            Some("verbatim"),
            "pass-through delivered verbatim by key-tail"
        );
    }

    #[test]
    fn apply_unmapped_documented_key_is_a_noop() {
        // Decision 6 / AC5: a documented key the adapter declares NO rule for is
        // delivered nowhere — the launch is untouched.
        let mapping = ConfigMapping::new(); // model has no rule
        let effective = effective_from_instance("model = \"gpt-4\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        assert!(launch.env.is_empty(), "unmapped key must not land anywhere");
        assert!(launch.args.is_empty());
        assert!(!tmp.path().join("config").exists(), "no file rendered");
    }

    #[test]
    fn apply_is_deterministic_and_empty_config_is_a_noop() {
        // An empty effective config leaves the launch untouched; and the transform
        // is deterministic (same inputs → same launch), reinforcing the pure-ish
        // start seam.
        let mapping = ConfigMapping::new()
            .with("model", ConfigTarget::flag("--model"))
            .with("agent.a", ConfigTarget::env("IGNORED")); // pass-through ignores the rule
        let empty = EffectiveConfig::default();
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &empty, tmp.path()).unwrap();
        assert_eq!(launch, empty_launch(), "empty config is a no-op");

        // Determinism: two applies of the same non-empty config yield equal launches.
        let effective = effective_from_instance("model = \"m\"\n[agent]\nx = \"y\"\n");
        let mut a = empty_launch();
        let mut b = empty_launch();
        apply_config_mapping(&mut a, &mapping, &effective, tmp.path()).unwrap();
        apply_config_mapping(&mut b, &mapping, &effective, tmp.path()).unwrap();
        assert_eq!(a, b);
        // model flag present; the agent.* leaf delivered verbatim by its tail (NOT
        // via the env rule keyed at "agent.a").
        assert_eq!(a.args, vec!["--model".to_string(), "m".to_string()]);
        assert_eq!(a.env.get("x").map(String::as_str), Some("y"));
        assert!(
            !a.env.contains_key("IGNORED"),
            "pass-through bypasses the rule"
        );
    }

    #[test]
    fn resolve_config_mapping_reads_manifest_config_section() {
        // A manifest adapter's mapping comes from its parsed [config] section.
        let tmp = TempDir::new().unwrap();
        let body = r#"
contract_version = "0.1.0"
[adapter]
kind = "demo"
[lifecycle.start]
exec = "the-agent"
[capabilities.interaction]
linux = "guaranteed"
[metering]
source = "self-reported"
[config.model]
flag = "--model"
"#;
        let path = write_manifest(tmp.path(), body);
        let mapping = resolve_config_mapping("demo", Some(&path)).unwrap();
        assert_eq!(
            mapping.target("model").unwrap().render_flag_args("x"),
            Some(["--model".to_string(), "x".to_string()])
        );
    }

    #[test]
    fn resolve_config_mapping_native_reads_the_builtin_table() {
        // A native adapter's mapping comes from the builtin code-declared table:
        // the mock declares `model` → env `MODEL`.
        let mapping = resolve_config_mapping("mock", None).unwrap();
        assert_eq!(mapping.target("model").unwrap().env_var(), Some("MODEL"));
        // An unknown native kind → an empty mapping (delivers nothing), not an err.
        assert!(resolve_config_mapping("nope", None).unwrap().is_empty());
    }

    #[test]
    fn resolve_config_mapping_rejects_an_escaping_file_target_symmetric() {
        // Fix #2 / AD-6 (symmetric trust): `resolve_config_mapping` VALIDATES the
        // mapping for BOTH kinds. A `[config.model]` File target whose path escapes
        // the Agent Home is rejected at start with InvalidConfigMapping — even
        // though this arm parses the manifest with `from_toml_str` (not the
        // registration-time `validate`), so a post-registration manifest edit that
        // slips a bad path past registration is still caught before it can steer a
        // write outside the home. The same `mapping.validate()` gate covers a
        // native code-declared File target (a native adapter carries no manifest to
        // re-validate at registration, so this start-time check is its ONLY guard).
        let tmp = TempDir::new().unwrap();
        let body = r#"
contract_version = "0.1.0"
[adapter]
kind = "demo"
[lifecycle.start]
exec = "the-agent"
[capabilities.interaction]
linux = "guaranteed"
[metering]
source = "self-reported"
[config.model]
file = { path = "../escape.toml", key = "k" }
"#;
        let path = write_manifest(tmp.path(), body);
        let err = resolve_config_mapping("demo", Some(&path)).unwrap_err();
        match err {
            LaunchResolveError::InvalidConfigMapping {
                adapter,
                key,
                detail,
            } => {
                assert_eq!(adapter, "demo");
                assert_eq!(key, "model");
                assert!(detail.contains("RELATIVE"), "{detail}");
            }
            other => panic!("expected InvalidConfigMapping, got {other}"),
        }
    }

    #[test]
    fn resolve_config_mapping_unreadable_manifest_is_reported() {
        // A manifest path that does not exist → ManifestUnreadable (defensive: the
        // launch resolver re-read it first, but this stays symmetric).
        let missing = std::path::Path::new("/no/such/adapter.toml");
        let err = resolve_config_mapping("demo", Some(missing)).unwrap_err();
        assert!(
            matches!(err, LaunchResolveError::ManifestUnreadable { .. }),
            "got {err}"
        );
    }

    #[test]
    fn resolve_config_mapping_malformed_manifest_is_unreadable() {
        // A manifest that fails to PARSE surfaces as ManifestUnreadable.
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(tmp.path(), "not = = valid toml");
        let err = resolve_config_mapping("demo", Some(&path)).unwrap_err();
        assert!(matches!(err, LaunchResolveError::ManifestUnreadable { .. }));
    }

    #[test]
    fn apply_file_target_render_failure_is_a_typed_error() {
        // ConfigApplyError::FileRender: a file target whose PARENT path is blocked
        // (a regular file sits where a directory must be) fails the write with a
        // typed error naming the key/path — never a panic (the start then rejects
        // before the `starting` transition, so the instance state is unchanged).
        let mapping =
            ConfigMapping::new().with("model", ConfigTarget::file("blocked/agent.toml", "k"));
        let effective = effective_from_instance("model = \"gpt-4\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        // Put a regular FILE at `blocked` so create_dir_all(blocked) fails.
        std::fs::write(tmp.path().join("blocked"), b"not a dir").unwrap();
        let err = apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap_err();
        match err {
            ConfigApplyError::FileRender { key, path, .. } => {
                assert_eq!(key, "blocked/agent.toml");
                assert_eq!(path, "blocked/agent.toml");
            }
        }
        // The error message names the key + path (defensive Display coverage).
        let msg = apply_config_mapping(&mut empty_launch(), &mapping, &effective, tmp.path())
            .unwrap_err()
            .to_string();
        assert!(msg.contains("blocked/agent.toml"), "{msg}");
        assert!(msg.contains("Agent Home"), "{msg}");
    }

    #[test]
    fn apply_two_file_keys_where_a_prefix_collides_overwrites_to_a_table() {
        // set_dotted_string's collision branch: two native keys into the SAME file
        // where one is a prefix of the other (`a` then `a.b`). The engine-authored
        // file has no scalar to preserve, so the prefix is overwritten with a table
        // (unlike the instance-config write path, which fails closed). Deterministic
        // sorted iteration means `a` is set first, then `a.b` masks it.
        let mapping = ConfigMapping::new()
            .with("model", ConfigTarget::file("f.toml", "a"))
            .with("temperature", ConfigTarget::file("f.toml", "a.b"));
        let effective = effective_from_instance("model = \"m\"\ntemperature = \"t\"\n");
        let mut launch = empty_launch();
        let tmp = tempfile::tempdir().unwrap();
        apply_config_mapping(&mut launch, &mapping, &effective, tmp.path()).unwrap();
        let rendered = tmp.path().join("f.toml");
        let parsed: toml::Table = std::fs::read_to_string(&rendered).unwrap().parse().unwrap();
        // `a` became a table with `b = "t"`; the scalar `a = "m"` was masked.
        assert_eq!(
            parsed["a"]["b"].as_str(),
            Some("t"),
            "prefix overwritten to a table"
        );
    }
}
