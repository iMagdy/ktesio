//! Integration test: register → list → remove through the engine's PUBLIC API
//! only (spine AD-2 "public API is sufficient" proof for the registration
//! capability).
//!
//! Everything here uses only items re-exported from the crate root. If this
//! file needs a `pub(crate)` item, that is a signal the public surface is
//! insufficient — it must compile against the public API alone.

use std::path::PathBuf;

use ktesio_engine::{
    AgentInstance, InstanceName, LifecycleState, Registry, RegistryError, RemoveDisposition,
};
use tempfile::TempDir;

/// Open a registry against a fresh temp state dir (no env, explicit base).
fn open(base: &TempDir) -> Registry {
    Registry::open(Some(base.path().to_path_buf())).expect("open registry")
}

#[test]
fn register_list_remove_full_cycle_via_public_api() {
    let tmp = TempDir::new().unwrap();
    let reg = open(&tmp);

    // Fresh state: empty Fleet.
    assert!(reg.list().unwrap().is_empty());

    // Register two instances of the same kind.
    let alpha: AgentInstance = reg.register("alpha", "mock").unwrap();
    let beta: AgentInstance = reg.register("beta", "mock").unwrap();
    assert_eq!(alpha.state, LifecycleState::Registered);
    assert_eq!(beta.state, LifecycleState::Registered);

    // The reported Agent Home paths are engine-computed, distinct, and exist.
    assert_ne!(alpha.agent_home, beta.agent_home);
    assert!(PathBuf::from(&alpha.agent_home).is_dir());
    assert!(PathBuf::from(&beta.agent_home).is_dir());

    // List returns both, ordered by name.
    let names: Vec<String> = reg
        .list()
        .unwrap()
        .into_iter()
        .map(|i| i.name.into())
        .collect();
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);

    // Empty Usage Ledger for a fresh instance.
    let alpha_name = InstanceName::new("alpha").unwrap();
    assert_eq!(reg.usage_event_count(&alpha_name).unwrap(), 0);

    // Remove beta with delete; alpha's home stays byte-identical (isolation).
    let alpha_home = PathBuf::from(&alpha.agent_home);
    let alpha_config_before = std::fs::read(alpha_home.join("config.toml")).unwrap();
    reg.remove("beta", RemoveDisposition::Delete, false)
        .unwrap();
    let alpha_config_after = std::fs::read(alpha_home.join("config.toml")).unwrap();
    assert_eq!(alpha_config_before, alpha_config_after);
    assert!(!PathBuf::from(&beta.agent_home).exists());

    // Only alpha remains.
    let remaining: Vec<String> = reg
        .list()
        .unwrap()
        .into_iter()
        .map(|i| i.name.into())
        .collect();
    assert_eq!(remaining, vec!["alpha".to_string()]);
}

#[test]
fn duplicate_registration_surfaces_typed_error_via_public_api() {
    let tmp = TempDir::new().unwrap();
    let reg = open(&tmp);
    reg.register("demo", "mock").unwrap();
    let err = reg.register("demo", "other").unwrap_err();
    assert!(matches!(err, RegistryError::DuplicateName { name } if name == "demo"));
}

#[test]
fn state_persists_across_reopen() {
    // A second Registry over the same base sees the prior registration — proves
    // migration-on-reopen is idempotent through the public API.
    let tmp = TempDir::new().unwrap();
    {
        let reg = open(&tmp);
        reg.register("persisted", "mock").unwrap();
    }
    let reopened = open(&tmp);
    let names: Vec<String> = reopened
        .list()
        .unwrap()
        .into_iter()
        .map(|i| i.name.into())
        .collect();
    assert_eq!(names, vec!["persisted".to_string()]);
}
