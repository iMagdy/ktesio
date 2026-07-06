//! The unified layered-config MODEL (spine AD-9, FR-11) — story 2-1.
//!
//! This is the FOUNDATION of Epic 2. It owns exactly two things, both of which
//! are pure and I/O-free (mirroring [`super::transition`] / [`super::restart`]):
//!
//! * The **deterministic precedence resolver** ([`resolve`]): a total, pure fold
//!   of the four ordered [`ConfigLayer`]s — engine defaults < agent-kind defaults
//!   < Agent Home instance config < invocation overrides — into a single
//!   [`EffectiveConfig`]. A later layer wins for a given key, EVERY time, on
//!   EVERY machine (no clock, no env, no OS). The merge is DEEP / per-leaf: an
//!   instance `a.b` overrides only `a.b`, leaving a kind-level `a.c` intact.
//! * **Write-time validation** ([`validate_write`]): accept a known unified key
//!   OR an `agent.*` pass-through key; reject any other key with the nearest
//!   valid key suggested (a tiny hand-rolled Levenshtein over the known-key set).
//!   Runs BEFORE any persistence, so a rejected `set` touches nothing.
//!
//! ## The ONE forward-design seam (for story 2-3, FR-13)
//!
//! [`resolve`] records, per resolved leaf key, the WINNING [`SourceLayer`] (a tag
//! carried alongside each value in [`EffectiveConfig`]). Story 2-3 renders "each
//! value's source layer" and persists the `EffectiveConfig` snapshot at start;
//! by keeping the tag here, that story is purely ADDITIVE. 2-1 does NOT render or
//! persist provenance — it just does not discard it. This is the only capability
//! 2-1 builds ahead of its own AC, and it is cheap (a tag threaded through the
//! fold).
//!
//! ## Explicitly OUT of scope (later Epic-2 stories)
//!
//! * The per-adapter NATIVE-mechanism mapping (files/env/flags) — **2-2 (FR-12)**.
//!   2-1 only reserves the `agent.*` pass-through namespace + honors the bypass.
//! * Provenance RENDERING + the persisted `EffectiveConfig` snapshot — **2-3
//!   (FR-13)**. 2-1 only leaves the [`SourceLayer`] seam.
//! * SECRETS (`secret:` resolvers, `SecretString`, masking) — **2-4 (FR-14,
//!   AD-10)**. A `secret:`-prefixed value is an ORDINARY opaque string here that
//!   round-trips untouched; this module builds NONE of the secret machinery.
//!
//! The on-disk layer plumbing (loading the instance `config.toml`, the embedded
//! engine defaults, writing a key) lives alongside the registry under path
//! authority — see [`super::registry`]. This module is deliberately I/O-free so
//! the resolver + validation stay exhaustively unit-testable in-process.

use std::collections::BTreeMap;

use thiserror::Error;
use toml::Value;

/// The reserved pass-through namespace prefix (spine AD-9's `agent.*`), story
/// 2-1 (AC7). A key under this prefix BYPASSES unknown-key validation and is
/// delivered verbatim (the mapping into an agent's native mechanism is 2-2,
/// FR-12). Recorded decision: the exact prefix is `agent.` (a dotted segment, so
/// a key named exactly `agent` — with no child — is NOT pass-through).
pub const PASS_THROUGH_PREFIX: &str = "agent.";

/// The Levenshtein distance threshold for the nearest-valid-key suggestion
/// (AC-B / AC6). A candidate is only suggested when its edit distance to the
/// offending key is `<=` this bound; beyond it the error honestly says "no close
/// match" rather than suggesting nonsense. `[ASSUMPTION]` recorded: `3` catches
/// the common near-misses (`modle`→`model`, a transposition + a typo) without
/// matching unrelated keys.
const SUGGESTION_MAX_DISTANCE: usize = 3;

/// The layer that supplied a resolved value (spine AD-9's precedence order).
///
/// The variant ORDER encodes precedence directly: `EngineDefault` is the weakest
/// and `InvocationOverride` the strongest, so the derived [`Ord`] makes "a later
/// layer wins" a simple `>=` comparison. [`resolve`] folds the four layers
/// weakest-first and records the winning layer per leaf key. This tag is the
/// seam story 2-3 (FR-13) renders + persists — 2-1 only records it.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLayer {
    /// Engine-owned defaults, the same for every instance (weakest). Story 2-1:
    /// an embedded `const` TOML (see [`super::registry`]).
    EngineDefault,
    /// Agent-kind defaults supplied by the resolved adapter. Empty for `mock`.
    KindDefault,
    /// The per-Agent-Home instance `config.toml` (`kt agent config set` writes
    /// here).
    Instance,
    /// Ephemeral invocation-time overrides (strongest). Not persisted in 2-1.
    InvocationOverride,
}

impl SourceLayer {
    /// The stable wire/label form (`"engine-default"`, …). Matches the serde
    /// kebab-case rename so a DB/JSON string and this label never diverge (the
    /// [`super::restart::RestartPolicy`] convention).
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceLayer::EngineDefault => "engine-default",
            SourceLayer::KindDefault => "kind-default",
            SourceLayer::Instance => "instance",
            SourceLayer::InvocationOverride => "invocation-override",
        }
    }

    /// The four layers in precedence order (weakest → strongest). This is the
    /// order [`resolve`] folds them, and the index each occupies in the
    /// `[ConfigLayer; 4]` input.
    pub const ORDER: [SourceLayer; 4] = [
        SourceLayer::EngineDefault,
        SourceLayer::KindDefault,
        SourceLayer::Instance,
        SourceLayer::InvocationOverride,
    ];
}

impl std::fmt::Display for SourceLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One config layer: a parsed TOML table (the deserialized form of that layer's
/// TOML text). Constructed by the engine's path-authority loaders (from the
/// embedded engine-defaults string, an adapter's kind-defaults, the instance
/// `config.toml`, or an invocation-override map). An EMPTY table is a valid layer
/// (e.g. a kind with no defaults resolves to an empty layer, NOT an error —
/// AC8).
///
/// A newtype over [`toml::value::Table`] so the resolver's signature is explicit
/// and the "empty layer" concept has a name; it derefs to the table for reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigLayer {
    table: toml::value::Table,
}

impl ConfigLayer {
    /// An empty layer (the "no defaults" / absent case — AC8).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Wrap an already-parsed TOML table as a layer.
    pub fn from_table(table: toml::value::Table) -> Self {
        Self { table }
    }

    /// Parse a layer from TOML text, naming the layer + source path on failure so
    /// a malformed layer surfaces a typed [`ConfigError::MalformedLayer`] (never a
    /// panic — AC8). `path` is a human label for the source (a file path, or e.g.
    /// `<engine-defaults>` for the embedded constant).
    pub fn parse(layer: SourceLayer, path: &str, text: &str) -> Result<Self, ConfigError> {
        let table = text
            .parse::<toml::Table>()
            .map_err(|e| ConfigError::MalformedLayer {
                layer,
                path: path.to_string(),
                detail: e.to_string(),
            })?;
        Ok(Self { table })
    }

    /// The underlying parsed table (read access).
    pub fn as_table(&self) -> &toml::value::Table {
        &self.table
    }

    /// Whether this layer contributes nothing (the absent/empty case).
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

/// A single resolved leaf: the winning value + the layer that supplied it.
///
/// The [`SourceLayer`] is the story-2-3 provenance seam (FR-13). 2-1 populates it
/// but renders/persists nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedValue {
    /// The winning value (a scalar or array `toml::Value`; nested tables are
    /// walked, so a `ResolvedValue` is always a LEAF, never a table).
    pub value: Value,
    /// The layer that supplied this value (AD-9 provenance seam for 2-3).
    pub source: SourceLayer,
}

impl ResolvedValue {
    /// Render the winning value for human/plain output WITHOUT exposing the
    /// underlying `toml::Value` type to callers (so `kt` needs no `toml`
    /// dependency — AD-2 keeps the engine the sole owner of the config TOML
    /// crate). A string renders as its bare contents (`gpt-4`, no quotes); every
    /// other scalar/array uses TOML's own inline form (`42`, `true`,
    /// `["a", "b"]`). A `secret:` value renders as-is (opaque text in 2-1;
    /// masking is Epic 2.4 — and 2-1 resolves no real secret to leak).
    pub fn display(&self) -> String {
        display_value(&self.value)
    }
}

/// Render a resolved [`toml::Value`] leaf for human/plain output (see
/// [`ResolvedValue::display`]).
fn display_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The resolved effective config (spine AD-9): every LEAF key (dotted path)
/// mapped to its winning value + [`SourceLayer`] provenance.
///
/// Keys are dotted paths into the merged TOML tree (`a.b.c`), so a per-leaf merge
/// is directly observable: setting `a.b` at a higher layer changes only the
/// `a.b` entry, leaving `a.c` (from a lower layer) untouched. A [`BTreeMap`]
/// keeps iteration deterministic (sorted), reinforcing "same inputs → same
/// output". The provenance tag on each [`ResolvedValue`] is the 2-3 seam; 2-1's
/// CLI renders VALUES only (see [`EffectiveConfig::values`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectiveConfig {
    leaves: BTreeMap<String, ResolvedValue>,
}

impl EffectiveConfig {
    /// Look up a single resolved leaf by its dotted key (value + provenance).
    pub fn get(&self, key: &str) -> Option<&ResolvedValue> {
        self.leaves.get(key)
    }

    /// Look up just the winning VALUE for a dotted key (provenance dropped) — the
    /// `kt agent config get <key>` read (source layers are 2-3, AC5/AC11).
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.leaves.get(key).map(|r| &r.value)
    }

    /// The rendered winning value for a dotted key, if present — the display form
    /// `kt agent config get <key>` prints, WITHOUT exposing `toml::Value` to
    /// callers (AD-2). See [`ResolvedValue::display`].
    pub fn value_display(&self, key: &str) -> Option<String> {
        self.leaves.get(key).map(|r| r.display())
    }

    /// Iterate all resolved leaves (dotted key → value + provenance), sorted by
    /// key. The full-map `kt agent config get <name>` read.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ResolvedValue)> {
        self.leaves.iter()
    }

    /// Remove a leaf by its dotted key, returning it if present. Used by the
    /// engine to drop a reserved IDENTITY key (`name`) that the registration step
    /// seeds into the instance `config.toml` — it is instance identity, not user
    /// config, so it must not be presented as a settable key (review patch #4).
    pub fn remove(&mut self, key: &str) -> Option<ResolvedValue> {
        self.leaves.remove(key)
    }

    /// A values-only view: dotted key → winning value, sorted by key (provenance
    /// dropped). This is what 2-1's CLI renders — source layers are 2-3 (AC5).
    pub fn values(&self) -> BTreeMap<String, Value> {
        self.leaves
            .iter()
            .map(|(k, r)| (k.clone(), r.value.clone()))
            .collect()
    }

    /// Whether the effective config is empty (the all-empty-layers case).
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Number of resolved leaf keys.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }
}

/// Resolve the four ordered layers into an [`EffectiveConfig`] (spine AD-9, the
/// AC-A heart). PURE: no I/O, no clock, no env — the SAME inputs always yield the
/// SAME output on every machine/OS.
///
/// `layers` is indexed by precedence, weakest first, matching
/// [`SourceLayer::ORDER`]: `[EngineDefault, KindDefault, Instance,
/// InvocationOverride]`. The merge is a STRUCTURAL tree-merge, weakest-first: at
/// each key a stronger layer's contribution overrides the weaker one, and — the
/// subtle correctness point — the stronger layer's SHAPE wins and PRUNES the
/// weaker layer's now-orphaned leaves:
///
/// * A strong SCALAR at `a.b` MASKS a weak `[a.b] c = 1` subtree — the resolved
///   tree carries `a.b = "scalar"` and NO orphaned `a.b.c` (a shape-agnostic
///   flatten would keep both, yielding a self-contradictory `a.b="scalar"` +
///   `a.b.c=1` with stale provenance).
/// * A strong SUBTREE at `a.b` REPLACES a weak scalar at `a.b`, then merges
///   per-leaf into a weak subtree (so a weak sibling `a.b.d` survives while a weak
///   `a.b.c` overridden by the strong layer is replaced).
///
/// The result is that the merge is DEEP / per-leaf where shapes AGREE (an
/// instance `a.b` overrides only `a.b`, a kind-level sibling `a.c` survives — the
/// AC4 guard), the tree is never self-contradictory where shapes DISAGREE, and
/// every surviving leaf's recorded [`SourceLayer`] reflects the layer that
/// actually DEFINES it (no stale/orphan provenance — the 2-3 seam stays honest).
pub fn resolve(layers: [ConfigLayer; 4]) -> EffectiveConfig {
    // Build one merged tree, folding weakest → strongest so a stronger layer's
    // shape wins and prunes at each node.
    let mut root: BTreeMap<String, MergedNode> = BTreeMap::new();
    for (layer, source) in layers.iter().zip(SourceLayer::ORDER) {
        merge_table_into(&mut root, layer.as_table(), source);
    }
    // Flatten the merged tree once to dotted leaves (each already tagged with the
    // layer that defines it).
    let mut leaves: BTreeMap<String, ResolvedValue> = BTreeMap::new();
    for (key, node) in &root {
        node.flatten_into(key.clone(), &mut leaves);
    }
    EffectiveConfig { leaves }
}

/// One node of the merged config tree during [`resolve`]: either a resolved LEAF
/// (a scalar/array value + the layer that defines it) or a SUBTREE (nested
/// children). Modeling the merge as a tree — rather than flattening each layer
/// independently — is what lets a stronger layer's SHAPE prune a weaker layer's
/// orphaned leaves on a scalar↔subtree collision.
enum MergedNode {
    /// A leaf value defined by `source`.
    Leaf { value: Value, source: SourceLayer },
    /// A subtree of child nodes, keyed by segment (sorted for determinism).
    Subtree(BTreeMap<String, MergedNode>),
}

impl MergedNode {
    /// Flatten this merged node into dotted-key leaves under `prefix`. An empty
    /// subtree contributes no leaves (config values live at leaves; 2-1 never
    /// represents an explicitly empty table as a value).
    fn flatten_into(&self, prefix: String, out: &mut BTreeMap<String, ResolvedValue>) {
        match self {
            MergedNode::Leaf { value, source } => {
                out.insert(
                    prefix,
                    ResolvedValue {
                        value: value.clone(),
                        source: *source,
                    },
                );
            }
            MergedNode::Subtree(children) => {
                for (key, child) in children {
                    let dotted = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    child.flatten_into(dotted, out);
                }
            }
        }
    }
}

/// Merge a TOML `table` from `source` INTO the subtree map `children`, weakest
/// layers merged first. A stronger layer's contribution wins per key, its SHAPE
/// pruning the weaker layer's orphans:
/// * a strong SUBTREE ensures the slot is a subtree (masking any weaker leaf),
///   then recurses so weak siblings survive while overlapping leaves are
///   overridden;
/// * a strong LEAF REPLACES whatever was in the slot — a weaker leaf OR a weaker
///   subtree — pruning that subtree's now-orphaned leaves so the resolved tree is
///   never self-contradictory and provenance never goes stale.
fn merge_table_into(
    children: &mut BTreeMap<String, MergedNode>,
    table: &toml::value::Table,
    source: SourceLayer,
) {
    for (key, value) in table {
        match value {
            Value::Table(child_table) => {
                let slot = children
                    .entry(key.clone())
                    .or_insert_with(|| MergedNode::Subtree(BTreeMap::new()));
                // If the weaker slot was a leaf, mask it with a fresh subtree.
                if !matches!(slot, MergedNode::Subtree(_)) {
                    *slot = MergedNode::Subtree(BTreeMap::new());
                }
                let MergedNode::Subtree(slot_children) = slot else {
                    unreachable!("slot was just ensured to be a subtree");
                };
                merge_table_into(slot_children, child_table, source);
            }
            leaf => {
                children.insert(
                    key.clone(),
                    MergedNode::Leaf {
                        value: leaf.clone(),
                        source,
                    },
                );
            }
        }
    }
}

/// The 2-1 KNOWN-KEY set: the unified config keys this story recognizes
/// (AC6/AC7). Deliberately SMALL and honest — the unified schema GROWS additively
/// in later stories; 2-1 needs only enough to make write-time validation + the
/// nearest-key suggestion testable.
///
/// `[ASSUMPTION]` recorded (the exact set + rationale):
/// * `model` — the SOLE known writable key in 2-1: a representative agent-native
///   config key (the one story 2-2 will map into an agent's native mechanism) and
///   the driver of the `modle`→`model` suggestion test.
///
/// HONESTY RULE (review decision #1, Islam): 2-1 ships ONLY keys it can honestly
/// honor. `restart.policy` was DROPPED from this set — the reaper reads the
/// Restart Policy from the SQLite spawn record, not config, so a
/// `config set restart.policy` would have been a misleading no-op. Wiring
/// config → engine-runtime (a config-sourced Restart Policy) is deferred to a
/// later story; until then config does not advertise a key it does not control.
///
/// Adding unified keys later is purely additive (a new entry here); 2-1 does NOT
/// freeze the schema. Keys are compared as full DOTTED paths. (The resolver
/// itself is key-agnostic — it merges whatever TOML the layers hold; this set
/// governs only WRITE validation.)
const KNOWN_KEYS: &[&str] = &["model"];

/// Whether `key` is a recognized unified config key (an exact dotted-path match
/// against [`KNOWN_KEYS`]).
fn is_known_key(key: &str) -> bool {
    KNOWN_KEYS.contains(&key)
}

/// Whether `key` lives under the reserved `agent.*` pass-through namespace (AC7).
/// Requires a non-empty child after the prefix (so a bare `agent` is NOT
/// pass-through — it would be an ordinary unknown key).
fn is_pass_through(key: &str) -> bool {
    key.strip_prefix(PASS_THROUGH_PREFIX)
        .is_some_and(|rest| !rest.is_empty())
}

/// Validate a config WRITE at write time (spine AD-9, AC-B / AC6 / AC7), BEFORE
/// anything is persisted — a rejected write must leave the instance config
/// byte-unchanged (the caller enforces the "validate then persist" ordering; this
/// function is the pure gate).
///
/// A key is VALID when it is a known unified key ([`KNOWN_KEYS`]) OR it lives
/// under the `agent.*` pass-through namespace (AC7 — pass-through keys skip the
/// known-key check and round-trip verbatim; the native mapping is 2-2). Any other
/// key is REJECTED with [`ConfigError::UnknownKey`] naming the offending key and
/// carrying the nearest known key (or `None` when nothing is within
/// [`SUGGESTION_MAX_DISTANCE`], so the diagnostic says "no close match" rather
/// than suggesting nonsense).
///
/// `_value` is accepted for signature-completeness (the write API passes it) but
/// is NOT inspected here: 2-1 validates the KEY namespace only. In particular a
/// `secret:`-prefixed VALUE is an ordinary opaque string in 2-1 (secrets are 2-4,
/// AD-10) — this function neither resolves nor rejects it.
///
/// A key with an EMPTY dotted segment (`agent..b`, `agent.foo.`, `.x`, a bare
/// `.`) is rejected up front (review patch #5): an empty segment would otherwise
/// persist a `""` key in the TOML tree — a malformed, un-addressable key. It is
/// reported as an [`ConfigError::UnknownKey`] with no suggestion (the shape is
/// wrong, not a near-miss of a known key).
pub fn validate_write(key: &str, _value: &str) -> Result<(), ConfigError> {
    // Reject empty dotted segments first (a malformed key shape). An empty `key`
    // has one empty segment and is caught here too.
    if has_empty_segment(key) {
        return Err(ConfigError::UnknownKey {
            key: key.to_string(),
            suggestion: None,
        });
    }
    if is_known_key(key) || is_pass_through(key) {
        return Ok(());
    }
    Err(ConfigError::UnknownKey {
        key: key.to_string(),
        suggestion: nearest_known_key(key),
    })
}

/// Whether a dotted key has any EMPTY segment (a leading/trailing/doubled dot, or
/// an empty key). Splitting `"a..b"` yields `["a", "", "b"]`; splitting `""`
/// yields `[""]` — both have an empty segment. Used to fail malformed keys before
/// they reach the store (review patch #5).
fn has_empty_segment(key: &str) -> bool {
    key.split('.').any(str::is_empty)
}

/// The nearest known key to `key` within [`SUGGESTION_MAX_DISTANCE`] edit
/// distance, or `None` when the closest known key is farther than the threshold
/// (AC6: "no close match" rather than suggesting nonsense).
///
/// Ties (two known keys equidistant from `key`) break on the CANDIDATE STRING
/// (lexicographically smallest wins) — NOT on [`KNOWN_KEYS`] array order — so the
/// suggestion is deterministic and stable regardless of how the array is ordered
/// or later reordered (review patch #6). Keying on `(distance, candidate)` makes
/// `min_by_key` total and order-independent.
fn nearest_known_key(key: &str) -> Option<String> {
    KNOWN_KEYS
        .iter()
        .map(|candidate| (levenshtein(key, candidate), *candidate))
        .filter(|(distance, _)| *distance <= SUGGESTION_MAX_DISTANCE)
        .min_by(|(da, ca), (db, cb)| da.cmp(db).then_with(|| ca.cmp(cb)))
        .map(|(_, candidate)| candidate.to_string())
}

/// A tiny hand-rolled Levenshtein edit distance (AC6 — RECOMMENDED "no new heavy
/// dependency"). Classic two-row dynamic-programming table over Unicode scalar
/// values; adequate for the short ASCII config keys 2-1 compares. PURE.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    // `prev[j]` = distance between a[..i] and b[..j]; rolled two-row table.
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitution_cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1) // deletion
                .min(curr[j] + 1) // insertion
                .min(prev[j] + substitution_cost); // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Errors from the unified-config surface (spine AD-9, story 2-1). `thiserror`,
/// never `miette` — `kt` wraps these into diagnostics (conventions). Each variant
/// names the offending key/layer/path so `kt` can render a remediation (NFR-1),
/// mirroring [`super::error::RegistryError`].
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A config write targeted an unknown key OUTSIDE the `agent.*` pass-through
    /// namespace (AC-B). NAMES the offending key and carries the nearest valid
    /// known key (`None` when nothing is within [`SUGGESTION_MAX_DISTANCE`]). The
    /// write is rejected BEFORE any persistence — nothing is written.
    #[error("unknown config key '{key}'{}", suggestion_hint(.suggestion))]
    UnknownKey {
        /// The rejected key.
        key: String,
        /// The nearest known key, if one is within the suggestion threshold.
        suggestion: Option<String>,
    },

    /// A config write would place a child UNDER an existing scalar value — e.g.
    /// setting `agent.a.b` when `agent.a` is already a scalar (review patch #3).
    /// Accepting it would silently DESTROY the existing scalar; instead the write
    /// FAILS CLOSED here, BEFORE any persistence, leaving the instance config
    /// byte-unchanged (AC-B atomicity). NAMES the requested key + the conflicting
    /// ancestor so the operator can unset/rename it first.
    #[error(
        "cannot set config key '{key}': '{conflicting_ancestor}' is already a value, not a table; \
         unset or rename it first"
    )]
    WriteShapeConflict {
        /// The key the write requested.
        key: String,
        /// The ancestor segment that is a scalar (so a child cannot be nested).
        conflicting_ancestor: String,
    },

    /// A config layer's TOML failed to parse OR the instance layer could not be
    /// read/written/serialized (AC8). NAMES the layer + source path so the
    /// diagnostic can point at it; never a panic on malformed input or I/O
    /// failure.
    #[error("the {layer} config layer at {path} is not valid TOML: {detail}")]
    MalformedLayer {
        /// Which of the four layers failed.
        layer: SourceLayer,
        /// The source path (a file path, or `<engine-defaults>` for the embedded
        /// constant).
        path: String,
        /// The underlying parse / I/O detail.
        detail: String,
    },

    /// The supplied instance name failed the naming rule at the facade (before
    /// any layer is touched). NAMES the candidate + the reason (as a string, so
    /// this domain type does not couple to `NameError` — it mirrors the other
    /// facades' shape while keeping the config surface self-contained).
    #[error("invalid Agent Instance name '{name}': {reason}")]
    InvalidName {
        /// The rejected candidate string.
        name: String,
        /// The specific rule that failed (rendered).
        reason: String,
    },

    /// A config operation targeted an instance that is not registered. NAMES it
    /// (mirrors [`super::error::RegistryError::NotFound`]) so the config surface
    /// never leaks a registry error type across the AD-1 boundary.
    #[error("no Agent Instance named '{name}' is registered")]
    NotFound {
        /// The missing instance name.
        name: String,
    },

    /// The state store could not be consulted while resolving/setting config
    /// (e.g. the instance-existence check failed). NAMES the instance + detail.
    #[error("state store error for Agent Instance '{name}': {detail}")]
    Store {
        /// The instance the operation was for.
        name: String,
        /// The underlying store error detail.
        detail: String,
    },
}

/// Render the trailing "; did you mean 'X'?" / "; no close match" hint for a
/// [`ConfigError::UnknownKey`] message from the optional suggestion.
fn suggestion_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(nearest) => format!("; did you mean '{nearest}'?"),
        None => "; no close match among the known keys".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a single-layer input with the other three layers empty, at a chosen
    /// precedence slot, from TOML text.
    fn one_layer(source: SourceLayer, text: &str) -> [ConfigLayer; 4] {
        let mut layers = [
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
        ];
        let idx = SourceLayer::ORDER
            .iter()
            .position(|s| *s == source)
            .unwrap();
        layers[idx] = ConfigLayer::parse(source, "<test>", text).unwrap();
        layers
    }

    // ---- SourceLayer precedence ordering (the encoded AD-9 order) ----

    #[test]
    fn source_layer_order_encodes_precedence_weakest_to_strongest() {
        // AD-9: engine < kind < instance < invocation. The derived Ord must agree.
        assert!(SourceLayer::EngineDefault < SourceLayer::KindDefault);
        assert!(SourceLayer::KindDefault < SourceLayer::Instance);
        assert!(SourceLayer::Instance < SourceLayer::InvocationOverride);
        // ORDER lists them weakest-first and is strictly increasing.
        let ordered = SourceLayer::ORDER;
        for pair in ordered.windows(2) {
            assert!(pair[0] < pair[1], "{:?} !< {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn source_layer_wire_form_round_trips() {
        for layer in SourceLayer::ORDER {
            let json = serde_json::to_string(&layer).unwrap();
            // as_str() matches the kebab-case serde form (no divergence).
            assert_eq!(json, format!("\"{}\"", layer.as_str()));
            let back: SourceLayer = serde_json::from_str(&json).unwrap();
            assert_eq!(back, layer);
            assert_eq!(layer.to_string(), layer.as_str());
        }
    }

    // ---- resolve(): key present in exactly one layer (×4) ----

    #[test]
    fn key_in_only_engine_default_resolves_to_it() {
        let eff = resolve(one_layer(SourceLayer::EngineDefault, "model = \"a\""));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("a".into()));
        assert_eq!(r.source, SourceLayer::EngineDefault);
    }

    #[test]
    fn key_in_only_kind_default_resolves_to_it() {
        let eff = resolve(one_layer(SourceLayer::KindDefault, "model = \"b\""));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("b".into()));
        assert_eq!(r.source, SourceLayer::KindDefault);
    }

    #[test]
    fn key_in_only_instance_resolves_to_it() {
        let eff = resolve(one_layer(SourceLayer::Instance, "model = \"c\""));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("c".into()));
        assert_eq!(r.source, SourceLayer::Instance);
    }

    #[test]
    fn key_in_only_invocation_override_resolves_to_it() {
        let eff = resolve(one_layer(SourceLayer::InvocationOverride, "model = \"d\""));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("d".into()));
        assert_eq!(r.source, SourceLayer::InvocationOverride);
    }

    // ---- resolve(): each adjacent precedence pair (stronger wins) ----

    /// Build a two-layer input (both slots set) from TOML text.
    fn two_layers(
        weak: SourceLayer,
        weak_text: &str,
        strong: SourceLayer,
        strong_text: &str,
    ) -> [ConfigLayer; 4] {
        let mut layers = [
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
        ];
        let wi = SourceLayer::ORDER.iter().position(|s| *s == weak).unwrap();
        let si = SourceLayer::ORDER
            .iter()
            .position(|s| *s == strong)
            .unwrap();
        layers[wi] = ConfigLayer::parse(weak, "<weak>", weak_text).unwrap();
        layers[si] = ConfigLayer::parse(strong, "<strong>", strong_text).unwrap();
        layers
    }

    #[test]
    fn kind_beats_engine_for_the_same_key() {
        let eff = two_layers(
            SourceLayer::EngineDefault,
            "model = \"engine\"",
            SourceLayer::KindDefault,
            "model = \"kind\"",
        );
        let eff = resolve(eff);
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("kind".into()));
        assert_eq!(r.source, SourceLayer::KindDefault);
    }

    #[test]
    fn instance_beats_kind_for_the_same_key_every_time() {
        // AC-A heart: the SAME key set at kind + instance resolves to instance.
        let eff = resolve(two_layers(
            SourceLayer::KindDefault,
            "model = \"kind\"",
            SourceLayer::Instance,
            "model = \"instance\"",
        ));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("instance".into()));
        assert_eq!(r.source, SourceLayer::Instance);
    }

    #[test]
    fn invocation_override_beats_instance_for_the_same_key() {
        let eff = resolve(two_layers(
            SourceLayer::Instance,
            "model = \"instance\"",
            SourceLayer::InvocationOverride,
            "model = \"override\"",
        ));
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("override".into()));
        assert_eq!(r.source, SourceLayer::InvocationOverride);
    }

    // ---- resolve(): key in ALL FOUR layers (top wins) ----

    #[test]
    fn key_in_all_four_layers_resolves_to_the_strongest() {
        let layers = [
            ConfigLayer::parse(SourceLayer::EngineDefault, "<e>", "model = \"engine\"").unwrap(),
            ConfigLayer::parse(SourceLayer::KindDefault, "<k>", "model = \"kind\"").unwrap(),
            ConfigLayer::parse(SourceLayer::Instance, "<i>", "model = \"instance\"").unwrap(),
            ConfigLayer::parse(
                SourceLayer::InvocationOverride,
                "<o>",
                "model = \"override\"",
            )
            .unwrap(),
        ];
        let eff = resolve(layers);
        let r = eff.get("model").unwrap();
        assert_eq!(r.value, Value::String("override".into()));
        assert_eq!(r.source, SourceLayer::InvocationOverride);
        // Exactly one leaf (all four set the SAME key).
        assert_eq!(eff.len(), 1);
    }

    // ---- resolve(): DEEP / per-leaf merge (sibling survives) ----

    #[test]
    fn nested_table_merge_is_per_leaf_sibling_survives() {
        // AC4 subtle correctness: instance sets a.b; kind's a.c must SURVIVE (a
        // shallow whole-table replace would drop it — a data-loss bug).
        let layers = two_layers(
            SourceLayer::KindDefault,
            "[a]\nb = \"kind-b\"\nc = \"kind-c\"\n",
            SourceLayer::Instance,
            "[a]\nb = \"instance-b\"\n",
        );
        let eff = resolve(layers);
        // a.b overridden by instance...
        let ab = eff.get("a.b").unwrap();
        assert_eq!(ab.value, Value::String("instance-b".into()));
        assert_eq!(ab.source, SourceLayer::Instance);
        // ...a.c survives from kind (the per-leaf merge, NOT a table replace).
        let ac = eff.get("a.c").unwrap();
        assert_eq!(ac.value, Value::String("kind-c".into()));
        assert_eq!(ac.source, SourceLayer::KindDefault);
        assert_eq!(eff.len(), 2);
    }

    #[test]
    fn deeply_nested_dotted_keys_flatten_and_merge_per_leaf() {
        // Deeper nesting still merges per-leaf: engine deep.x.y and instance
        // deep.x.z coexist; instance deep.x.y wins.
        let layers = two_layers(
            SourceLayer::EngineDefault,
            "[deep.x]\ny = 1\nz = 2\n",
            SourceLayer::Instance,
            "[deep.x]\ny = 99\n",
        );
        let eff = resolve(layers);
        assert_eq!(eff.value("deep.x.y"), Some(&Value::Integer(99)));
        assert_eq!(eff.get("deep.x.y").unwrap().source, SourceLayer::Instance);
        assert_eq!(eff.value("deep.x.z"), Some(&Value::Integer(2)));
        assert_eq!(
            eff.get("deep.x.z").unwrap().source,
            SourceLayer::EngineDefault
        );
    }

    // ---- resolve(): empty-layer and all-empty cases ----

    #[test]
    fn a_single_empty_layer_contributes_nothing() {
        // A kind with no defaults (empty layer) does not error and adds no leaves;
        // the other layer resolves normally (AC8).
        let eff = resolve(two_layers(
            SourceLayer::KindDefault, // empty via parse of ""
            "",
            SourceLayer::Instance,
            "model = \"only-instance\"",
        ));
        assert_eq!(eff.len(), 1);
        assert_eq!(
            eff.value("model"),
            Some(&Value::String("only-instance".into()))
        );
    }

    #[test]
    fn all_empty_layers_resolve_to_an_empty_effective_map() {
        let eff = resolve([
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
            ConfigLayer::empty(),
        ]);
        assert!(eff.is_empty());
        assert_eq!(eff.len(), 0);
        assert_eq!(eff.value("anything"), None);
        assert!(eff.values().is_empty());
    }

    #[test]
    fn resolve_is_deterministic_for_the_same_inputs() {
        // The pure-function contract: same inputs → identical output (independent
        // of iteration nondeterminism — BTreeMap sorts, so this is stable).
        let build = || {
            two_layers(
                SourceLayer::EngineDefault,
                "b = 2\na = 1\n",
                SourceLayer::Instance,
                "c = 3\n",
            )
        };
        assert_eq!(resolve(build()), resolve(build()));
        // Values-only view is sorted + stable too.
        let vals = resolve(build()).values();
        let keys: Vec<&String> = vals.keys().collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn empty_nested_table_contributes_no_leaves() {
        // A structural empty table `[a]` with nothing under it adds no leaf.
        let eff = resolve(one_layer(SourceLayer::Instance, "[a]\n"));
        assert!(eff.is_empty());
    }

    // ---- resolve(): SHAPE COLLISIONS — stronger shape wins + prunes (patch #2) ----

    #[test]
    fn strong_scalar_masks_weak_subtree_no_orphan_leaves() {
        // Review patch #2: a weak layer defines the SUBTREE [a] b=1, c=2; a strong
        // layer defines the SCALAR a="scalar". The strong scalar must WIN and PRUNE
        // the weak subtree — the resolved tree has a="scalar" (Instance) and NO
        // orphaned a.b / a.c. Never a self-contradictory a="scalar" + a.b=1.
        let layers = two_layers(
            SourceLayer::KindDefault,
            "[a]\nb = 1\nc = 2\n",
            SourceLayer::Instance,
            "a = \"scalar\"\n",
        );
        let eff = resolve(layers);
        // a is the strong scalar, tagged Instance.
        let a = eff.get("a").unwrap();
        assert_eq!(a.value, Value::String("scalar".into()));
        assert_eq!(a.source, SourceLayer::Instance);
        // The weak subtree leaves are PRUNED — no orphans, no stale provenance.
        assert_eq!(eff.get("a.b"), None);
        assert_eq!(eff.get("a.c"), None);
        assert_eq!(eff.len(), 1);
    }

    #[test]
    fn strong_subtree_replaces_weak_scalar_and_prunes_it() {
        // Symmetric: a weak SCALAR a="scalar"; a strong SUBTREE [a] b=1. The strong
        // subtree must REPLACE the weak scalar — the resolved tree has a.b=1
        // (Instance) and NO orphaned scalar `a`.
        let layers = two_layers(
            SourceLayer::KindDefault,
            "a = \"scalar\"\n",
            SourceLayer::Instance,
            "[a]\nb = 1\n",
        );
        let eff = resolve(layers);
        let ab = eff.get("a.b").unwrap();
        assert_eq!(ab.value, Value::Integer(1));
        assert_eq!(ab.source, SourceLayer::Instance);
        // The weak scalar `a` is gone (masked by the subtree).
        assert_eq!(eff.get("a"), None);
        assert_eq!(eff.len(), 1);
    }

    #[test]
    fn subtree_over_scalar_preserves_strong_siblings_and_prunes_weak_scalar() {
        // A strong subtree with MULTIPLE leaves over a weak scalar: all strong
        // leaves survive with correct provenance; the weak scalar is pruned.
        let layers = two_layers(
            SourceLayer::EngineDefault,
            "a = \"weak-scalar\"\n",
            SourceLayer::Instance,
            "[a]\nb = 1\nc = 2\n",
        );
        let eff = resolve(layers);
        assert_eq!(eff.value("a.b"), Some(&Value::Integer(1)));
        assert_eq!(eff.value("a.c"), Some(&Value::Integer(2)));
        assert_eq!(eff.get("a.b").unwrap().source, SourceLayer::Instance);
        assert_eq!(eff.get("a.c").unwrap().source, SourceLayer::Instance);
        assert_eq!(eff.get("a"), None);
        assert_eq!(eff.len(), 2);
    }

    #[test]
    fn scalar_over_subtree_across_three_layers_tracks_the_defining_layer() {
        // Provenance honesty under a collision spanning >2 layers: engine subtree
        // a.b, kind subtree a.c, instance SCALAR a. The instance scalar prunes BOTH
        // weaker subtrees; the surviving leaf is tagged Instance (the layer that
        // actually defines the winning shape) — no stale engine/kind provenance.
        let layers = [
            ConfigLayer::parse(SourceLayer::EngineDefault, "<e>", "[a]\nb = 1\n").unwrap(),
            ConfigLayer::parse(SourceLayer::KindDefault, "<k>", "[a]\nc = 2\n").unwrap(),
            ConfigLayer::parse(SourceLayer::Instance, "<i>", "a = \"wins\"\n").unwrap(),
            ConfigLayer::empty(),
        ];
        let eff = resolve(layers);
        assert_eq!(eff.value("a"), Some(&Value::String("wins".into())));
        assert_eq!(eff.get("a").unwrap().source, SourceLayer::Instance);
        assert_eq!(eff.get("a.b"), None);
        assert_eq!(eff.get("a.c"), None);
        assert_eq!(eff.len(), 1);
    }

    // ---- values-only view keeps provenance out (the 2-1 CLI surface) ----

    #[test]
    fn resolved_value_display_renders_strings_bare_and_others_inline() {
        // String → bare (no quotes); non-string scalars/arrays → TOML inline form.
        let eff = resolve(one_layer(
            SourceLayer::Instance,
            "s = \"gpt-4\"\nn = 42\nb = true\narr = [1, 2]\n",
        ));
        assert_eq!(eff.value_display("s").as_deref(), Some("gpt-4"));
        assert_eq!(eff.value_display("n").as_deref(), Some("42"));
        assert_eq!(eff.value_display("b").as_deref(), Some("true"));
        assert_eq!(eff.value_display("arr").as_deref(), Some("[1, 2]"));
        // The ResolvedValue::display() method agrees with value_display().
        assert_eq!(eff.get("n").unwrap().display(), "42");
        // A missing key has no display.
        assert_eq!(eff.value_display("nope"), None);
    }

    #[test]
    fn values_view_drops_provenance_but_keeps_values() {
        let eff = resolve(two_layers(
            SourceLayer::KindDefault,
            "model = \"kind\"",
            SourceLayer::Instance,
            "temperature = 1\n",
        ));
        let vals = eff.values();
        assert_eq!(vals.get("model"), Some(&Value::String("kind".into())));
        assert_eq!(vals.get("temperature"), Some(&Value::Integer(1)));
        // get() still carries the tag (2-3 seam) even though values() drops it.
        assert_eq!(eff.get("model").unwrap().source, SourceLayer::KindDefault);
    }

    // ---- validate_write(): known key accepted ----

    #[test]
    fn validate_write_accepts_a_known_scalar_key() {
        assert!(validate_write("model", "gpt-4").is_ok());
    }

    #[test]
    fn validate_write_rejects_restart_policy_now_dropped() {
        // Review decision #1: `restart.policy` was DROPPED from the known-key set
        // (the reaper reads the policy from SQLite, not config — a config write
        // would be a misleading no-op). It is now an unknown key. `model` is the
        // sole known key.
        assert!(!is_known_key("restart.policy"));
        assert!(validate_write("restart.policy", "never").is_err());
        assert_eq!(KNOWN_KEYS, &["model"]);
    }

    #[test]
    fn validate_write_accepts_a_dotted_agent_pass_through_key() {
        // The dotted-WRITE path is exercised via the agent.* namespace (there is
        // no known dotted unified key in 2-1 after the restart.policy drop).
        assert!(validate_write("agent.tools.web_search", "on").is_ok());
    }

    // ---- validate_write(): agent.* pass-through accepted, sibling rejected ----

    #[test]
    fn validate_write_accepts_any_agent_pass_through_key() {
        // AC7: an agent.* key bypasses the known-key check (delivered verbatim).
        assert!(validate_write("agent.anything", "x").is_ok());
        assert!(validate_write("agent.deep.nested.key", "x").is_ok());
    }

    #[test]
    fn validate_write_rejects_an_equally_unknown_non_agent_key() {
        // AC7 companion: an equally-unknown NON-agent.* key is rejected.
        let err = validate_write("notagent.foo", "x").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey { key, .. } if key == "notagent.foo"));
    }

    #[test]
    fn bare_agent_key_is_not_pass_through() {
        // `agent` with no child is NOT pass-through (the prefix needs a child).
        let err = validate_write("agent", "x").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownKey { .. }));
        assert!(!is_pass_through("agent"));
        assert!(!is_pass_through("agentfoo")); // no dot → not the namespace
        assert!(is_pass_through("agent.x"));
    }

    // ---- validate_write(): empty dotted segments rejected (patch #5) ----

    #[test]
    fn validate_write_rejects_empty_dotted_segments() {
        // Patch #5: a key with an empty segment would persist a "" key — reject it
        // up front (no suggestion; the shape is wrong, not a near-miss). Covers a
        // doubled dot, a trailing dot, a leading dot, a bare dot, and the empty key
        // — including under the agent.* namespace (which otherwise bypasses the
        // known-key check).
        for bad in ["agent..b", "agent.foo.", ".x", ".", "", "a..b", "model."] {
            let err = validate_write(bad, "v").unwrap_err();
            match err {
                ConfigError::UnknownKey { key, suggestion } => {
                    assert_eq!(key, bad);
                    assert_eq!(suggestion, None, "empty-segment keys get no suggestion");
                }
                other => panic!("expected UnknownKey for {bad:?}, got {other:?}"),
            }
        }
        assert!(has_empty_segment("agent..b"));
        assert!(has_empty_segment(""));
        assert!(!has_empty_segment("agent.a.b"));
        assert!(!has_empty_segment("model"));
    }

    // ---- validate_write(): unknown key rejected WITH nearest suggestion ----

    #[test]
    fn validate_write_rejects_unknown_key_and_suggests_the_nearest() {
        // AC6 fixed near-miss: `modle` → suggest `model`.
        let err = validate_write("modle", "x").unwrap_err();
        match err {
            ConfigError::UnknownKey { key, suggestion } => {
                assert_eq!(key, "modle");
                assert_eq!(suggestion.as_deref(), Some("model"));
            }
            other => panic!("expected UnknownKey, got {other:?}"),
        }
    }

    #[test]
    fn unknown_key_error_message_names_key_and_suggestion() {
        let err = validate_write("modle", "x").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("modle"), "{msg}");
        assert!(msg.contains("did you mean 'model'"), "{msg}");
    }

    // ---- validate_write(): far-miss → NO suggestion (honest "no close match") ----

    #[test]
    fn validate_write_far_miss_yields_no_suggestion() {
        // AC6: a far-off key gets None (not nonsense). "zzzzzzzzzz" is far from
        // every known key (distance > threshold).
        let err = validate_write("zzzzzzzzzz", "x").unwrap_err();
        match err {
            ConfigError::UnknownKey { suggestion, .. } => assert_eq!(suggestion, None),
            other => panic!("expected UnknownKey, got {other:?}"),
        }
        // And the message says so honestly.
        let msg = validate_write("zzzzzzzzzz", "x").unwrap_err().to_string();
        assert!(msg.contains("no close match"), "{msg}");
    }

    #[test]
    fn validate_write_does_not_inspect_the_value_secret_is_opaque() {
        // 2-1 validates the KEY only; a `secret:` VALUE is opaque text here (2-4
        // owns resolution/masking). A known key with a secret: value is accepted.
        assert!(validate_write("model", "secret:OPENAI_KEY").is_ok());
    }

    // ---- Levenshtein unit coverage (the suggestion engine) ----

    #[test]
    fn levenshtein_basic_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("model", "model"), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("modle", "model"), 2); // transposition = 2 edits
        assert_eq!(levenshtein("kitten", "sitting"), 3); // classic example
    }

    #[test]
    fn nearest_known_key_picks_within_threshold_else_none() {
        assert_eq!(nearest_known_key("model").as_deref(), Some("model"));
        assert_eq!(nearest_known_key("modle").as_deref(), Some("model"));
        assert_eq!(nearest_known_key("modl").as_deref(), Some("model")); // 1 deletion
                                                                         // Far off → None (beyond the threshold).
        assert_eq!(nearest_known_key("zzzzzzzzzz"), None);
    }

    #[test]
    fn nearest_known_key_tie_break_is_deterministic_on_candidate_string() {
        // Review patch #6: with a synthetic equidistant set the tie must break on
        // the CANDIDATE STRING (lexicographically smallest), independent of array
        // order. Assert the tie-break policy directly on the comparator so it holds
        // regardless of how KNOWN_KEYS is (re)ordered: given two equidistant
        // candidates "alpha" and "beta", "alpha" wins.
        let cands = [(1usize, "beta"), (1usize, "alpha")];
        let picked = cands
            .iter()
            .copied()
            .min_by(|(da, ca), (db, cb)| da.cmp(db).then_with(|| ca.cmp(cb)))
            .map(|(_, c)| c);
        assert_eq!(
            picked,
            Some("alpha"),
            "tie must break on the smaller string"
        );
    }

    // ---- ConfigLayer / MalformedLayer ----

    #[test]
    fn config_layer_parse_rejects_malformed_toml_naming_the_layer_and_path() {
        let err = ConfigLayer::parse(SourceLayer::Instance, "/x/config.toml", "not = = valid")
            .unwrap_err();
        match err {
            ConfigError::MalformedLayer { layer, path, .. } => {
                assert_eq!(layer, SourceLayer::Instance);
                assert_eq!(path, "/x/config.toml");
            }
            other => panic!("expected MalformedLayer, got {other:?}"),
        }
        // The message names the layer + path (never a panic).
        let msg = ConfigLayer::parse(SourceLayer::Instance, "/x/config.toml", "bad = = ")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("instance"), "{msg}");
        assert!(msg.contains("/x/config.toml"), "{msg}");
    }

    #[test]
    fn config_layer_empty_helpers() {
        let empty = ConfigLayer::empty();
        assert!(empty.is_empty());
        assert!(empty.as_table().is_empty());
        let full = ConfigLayer::parse(SourceLayer::Instance, "<t>", "model = \"x\"").unwrap();
        assert!(!full.is_empty());
        assert_eq!(
            ConfigLayer::from_table(full.as_table().clone()).as_table(),
            full.as_table()
        );
    }
}
