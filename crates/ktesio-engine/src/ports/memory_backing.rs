//! The Memory Backing port surface (spine AD-11) — story 5-1.
//!
//! AD-11: *"`filesystem` (engine-managed directory inside the Agent Home;
//! survives restarts byte-identically) and `native` (delegation marker; engine
//! guarantees only Agent Home persistence). Attach/detach permitted only while
//! the Agent Instance is not `running`. The backing descriptor is handed to the
//! adapter at start; richer backings are Deferred behind this port."*
//!
//! ## This module IS the seam — deliberately NOT a trait (A-8)
//!
//! The `filesystem` implementation is pure path authority inside the engine (one
//! idempotent directory creation at attach + one defensive self-heal at start);
//! there is no behavior to dispatch. A trait with one real impl and one marker
//! would be exactly the "speculative port tree" the ports module warns against,
//! so the port is this MODULE: its types are the extension point a richer
//! backing (vector store, tiered, …) lands behind in a later epic.
//!
//! ## Descriptor delivery (AD-11 Delivery clause / Q-1 ruling)
//!
//! The backing "descriptor" rides the EXISTING layered-config seam: at start the
//! engine injects the managed directory path at the reserved unified-config key
//! ([`crate::domain::MEMORY_DIR_KEY`]) as an invocation override, and the
//! adapter's already-declared `[config]` mapping routes it into the agent's
//! native mechanism. Delivery is OFFERED, not imposed — whether the agent
//! receives it is the adapter's declared choice — which is why
//! [`MemoryBackingStatus::declared`] exists: the public read reports whether
//! the resolved mapping actually targets the key.

use std::path::PathBuf;

use crate::domain::InstanceName;

/// Which Memory Backing is attached to an Agent Instance (FR-16's closed v1
/// vocabulary).
///
/// The wire form is snake_case (`filesystem`, `native`), matching the `kind`
/// column stored by the SQLite [`StateStore`](super::StateStore) — the same
/// wire-string discipline as
/// [`LifecycleState`](crate::domain::LifecycleState). The variant set ships the
/// FULL vocabulary now so story 5-2 adds `native`'s BEHAVIOR without a schema or
/// enum-shape break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryBackingKind {
    /// An engine-managed directory inside the Agent Home whose contents survive
    /// stop/start cycles and engine restarts byte-identically (story 5-1). The
    /// engine creates it and never touches its contents (operator data, DC-7).
    Filesystem,
    /// An explicit delegation marker: Ktesio guarantees only Agent Home
    /// persistence. RESERVED for story 5-2 — the vocabulary ships now so 5-2's
    /// behavior lands without a breaking edit; nothing in 5-1 implements it.
    Native,
}

impl MemoryBackingKind {
    /// Snake_case wire form used in the DB `kind` column and diagnostics.
    ///
    /// Kept in lockstep with the vocabulary so the store can persist a plain
    /// string without pulling in serde.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryBackingKind::Filesystem => "filesystem",
            MemoryBackingKind::Native => "native",
        }
    }

    /// Parse the wire form back into a [`MemoryBackingKind`].
    ///
    /// Returns `None` for an unrecognized string (e.g. a value written by a
    /// future schema version); callers decide how to treat that.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "filesystem" => Some(MemoryBackingKind::Filesystem),
            "native" => Some(MemoryBackingKind::Native),
            _ => None,
        }
    }
}

/// The persisted Memory Backing attachment for ONE Agent Instance (spine AD-6:
/// attachment metadata lives as typed columns in the one SQLite state store;
/// never a JSON blob). Exactly ONE backing per instance — the row is UNIQUE on
/// the instance FK and cascade-deletes with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryBacking {
    /// The Agent Instance this backing is attached to.
    pub name: InstanceName,
    /// Which backing kind is attached.
    pub kind: MemoryBackingKind,
    /// RFC-3339 UTC timestamp of the attachment (conventions row).
    pub attached_at: String,
}

/// The public read of an instance's Memory Backing (story 5-1, Task 4.5): what
/// is attached, where the engine-managed directory lives (path authority), and
/// the DC-10 delivery fact — whether the adapter's declared `[config]` mapping
/// targets the reserved key, i.e. whether the injected path will actually reach
/// the agent. Shaped for reuse by story 5-2's status/effective-config surface,
/// not for one call site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryBackingStatus {
    /// The attached backing kind.
    pub kind: MemoryBackingKind,
    /// The engine-computed managed directory (path authority). For
    /// [`MemoryBackingKind::Filesystem`] it exists (created at attach, self-healed
    /// at every start); for other kinds it is the computed location only.
    pub dir: PathBuf,
    /// Whether the instance's adapter DECLARES a config mapping for the reserved
    /// key ([`crate::domain::MEMORY_DIR_KEY`]) — the Q-1 honesty rule: delivery
    /// is offered, not imposed, and an operator must be able to learn which it
    /// is. `false` means the start still succeeds but the agent will NOT receive
    /// the path (a stderr notice says so at start). Named for what it reports —
    /// a declared TARGET, not proof of runtime receipt (the agent may ignore the
    /// delivered value; only story 5-2's richer status could observe that).
    pub declared: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips_for_every_variant() {
        let all = [MemoryBackingKind::Filesystem, MemoryBackingKind::Native];
        for kind in all {
            assert_eq!(MemoryBackingKind::from_wire(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn from_wire_rejects_unknown() {
        // The rejection branch is part of the contract (a future/typo'd token
        // must not decode into a real kind).
        assert_eq!(MemoryBackingKind::from_wire("bogus"), None);
        assert_eq!(MemoryBackingKind::from_wire(""), None);
        assert_eq!(MemoryBackingKind::from_wire("Filesystem"), None);
    }
}
