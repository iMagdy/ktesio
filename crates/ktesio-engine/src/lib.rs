//! # ktesio-engine
//!
//! Home of the Ktesio engine: agent lifecycle, governance, and interaction
//! logic live here, behind a public API that *is* the Embedding Interface
//! (architecture spine AD-1, AD-2, AD-13).
//!
//! This crate is intentionally empty at the workspace restructure (story 1-1).
//! Engine modules (domain, ports, backends, store, metering, skills, events)
//! arrive with the stories that need them — no speculative module trees.
//!
//! Dependency law (AD-2): the `kt` binary depends only on this crate's public
//! API (plus `ktesio-adapter-api` types); this crate depends on
//! `ktesio-adapter-api` and never on `kt` or concrete adapters.
