//! The story 6-4 HERMES TCK pass (AC line 2): the conformance Test Kit runs
//! against the REAL shipping `hermes` builtin — a native adapter registered by
//! kind — and every section APPLICABLE to Hermes' actual declaration must pass,
//! while the EngineObserved metering section reads `not_applicable` because
//! Hermes declares SelfReported (CP-d). No section may demand EngineObserved of
//! a SelfReported adapter; conversely the BestEffort pause declaration is still
//! APPLICABLE — the harness must DEMONSTRATE the best-effort path
//! (`pause-best-effort` / `resume-best-effort` cause tags), never skip it.
//!
//! ## Isolation (the recorded/sandboxed hermes.rs pattern — never a gateway)
//!
//! The declared launch (`hermes gateway run --external-supervisor`) resolves
//! through PATH to the committed `hermes_shim` launcher COPIED as
//! `<tmp>/hermes<EXE_SUFFIX>` (with `fake_agent` beside it), scripted via
//! `HERMES_SHIM_ARGS` — the same no-network sandbox `tests/hermes.rs` uses.
//! The harness's own probe fixtures exec `fake_agent` by ABSOLUTE path, so the
//! PATH shim only ever captures the hermes-kind subject.
//!
//! **PATH discipline** (process-global, `unsafe` under edition 2024): exactly
//! one `#[test]` here mutates the environment, at its start, before any child
//! is spawned, and RESTORES both `PATH` and `HERMES_SHIM_ARGS` at teardown.
//! Under nextest this binary is its own process; under plain `cargo test`
//! binaries run sequentially — and no other test in this file touches the
//! environment.
//!
//! ## Expected report shape (derived from Hermes' declaration, not hardcoded
//! per adapter — the harness derives it from the registered snapshot; these
//! assertions PIN it for the shipping adapter):
//!
//! * `capability_edges`, `lifecycle` (incl. the crash leg), `pause`
//!   (BestEffort — demonstrated, not skipped), `metering_self_reported`,
//!   `memory` (Hermes declares `memory.dir` → `HERMES_HOME`), `interaction`
//!   (Guaranteed) → **pass**.
//! * `metering_engine_observed` → **not_applicable** (SelfReported).
//! * `config_mapping` → **not_applicable**: the launch is CODE-declared
//!   (contract argv, no `--dump <path>` seam the harness could author), so
//!   delivered config has no observable artifact for a native subject; the
//!   reserved-key delivery Hermes DOES declare is proven by `memory`.

use std::path::PathBuf;

use ktesio_conformance::{
    run_conformance, section_ids, ConformanceReport, SectionResult, TckAdapter,
};
use tempfile::TempDir;

/// Copy the committed `hermes_shim` launcher onto PATH as `hermes<EXE_SUFFIX>`
/// and return the shim path — the `tests/hermes.rs` `install_shim` shape (the
/// shim resolves its script target beside ITSELF, so `fake_agent` is copied
/// into the same directory).
fn install_shim(shim_dir: &TempDir) -> PathBuf {
    let exe = std::env::current_exe().expect("locate the running test executable");
    let mut dir = exe;
    dir.pop(); // drop the test-bin file name
    if dir.ends_with("deps") {
        dir.pop(); // drop `deps`
    }
    let candidate = dir.join(format!("hermes_shim{}", std::env::consts::EXE_SUFFIX));
    let source = if candidate.exists() {
        candidate
    } else {
        // Not built by this harness — build it on demand (the same fallback
        // shape as `fake_agent_bin`'s; note this file is an INTEGRATION TEST
        // target, so its lines are not part of the coverage denominator —
        // no tarpaulin cfg is needed here, unlike the lib-side fallback).
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
        let status = std::process::Command::new(cargo)
            .args(["build", "-p", "ktesio-conformance", "--bin", "hermes_shim"])
            .env_remove("RUSTC_WRAPPER") // a shimmed PATH must not break the build
            .status()
            .expect("run cargo for hermes_shim");
        assert!(status.success(), "on-demand hermes_shim build failed");
        candidate
    };
    let shim = shim_dir
        .path()
        .join(format!("hermes{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(&source, &shim).expect("copy hermes_shim onto PATH");
    // The shim re-execs fake_agent beside ITSELF (current_exe anchoring).
    let agent = ktesio_conformance::fake_agent_bin();
    std::fs::copy(
        &agent,
        shim_dir
            .path()
            .join(format!("fake_agent{}", std::env::consts::EXE_SUFFIX)),
    )
    .expect("copy fake_agent beside the shim");
    shim
}

/// The Hermes TCK pass. One function owns the whole environment-mutating
/// journey (see the module doc's PATH discipline).
#[test]
fn hermes_tck_passes_every_section_applicable_to_its_declaration() {
    // ---- Sandbox setup: the PATH shim + the shim script (linger so the
    // harness's lifecycle/pause sections can drive the subject).
    let shim_dir = TempDir::new().unwrap();
    let _shim = install_shim(&shim_dir);

    let original_path = std::env::var_os("PATH").map(|v| v.to_os_string());
    let original_shim_args = std::env::var_os("HERMES_SHIM_ARGS");
    let joined = {
        let mut paths: Vec<PathBuf> =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
        paths.insert(0, shim_dir.path().to_path_buf());
        std::env::join_paths(paths).expect("join PATH")
    };
    // SAFETY: process-global mutation, done ONCE at this test's start before
    // any child spawn, and RESTORED at the end (teardown below) — the same
    // discipline `tests/hermes.rs` records for its PATH-dependent test.
    unsafe {
        std::env::set_var("PATH", &joined);
        std::env::set_var("HERMES_SHIM_ARGS", "--linger-ms 600000");
    }

    // THE harness call: register the shipping hermes builtin with a fresh
    // engine and run every section.
    let report: ConformanceReport = run_conformance(&TckAdapter::Native("hermes".to_string()));

    // ---- Teardown BEFORE asserting, so a failing assert cannot leak the
    // process-global mutation into other tests (review blind-3 discipline).
    match original_path {
        Some(original) => unsafe {
            std::env::set_var("PATH", original);
        },
        None => unsafe {
            std::env::remove_var("PATH");
        },
    }
    match original_shim_args {
        Some(original) => unsafe {
            std::env::set_var("HERMES_SHIM_ARGS", original);
        },
        None => unsafe {
            std::env::remove_var("HERMES_SHIM_ARGS");
        },
    }

    // ---- The report contract.
    assert_eq!(report.adapter_kind, "hermes");
    assert_eq!(report.sections.len(), 8, "the report must be complete");

    // Everything applicable to the declaration PASSED.
    assert!(
        report.is_conformant(),
        "hermes must conform: failures = {:?}",
        report.failures()
    );

    // The demonstrated sections (each proves real behavior, never a skip).
    for id in [
        section_ids::CAPABILITY_EDGES,
        section_ids::LIFECYCLE,
        section_ids::PAUSE,
        section_ids::METERING_SELF_REPORTED,
        section_ids::MEMORY,
        section_ids::INTERACTION,
    ] {
        assert_eq!(
            report.section(id),
            Some(&SectionResult::Pass),
            "{id} must PASS for hermes (applicable to its declaration)"
        );
    }

    // Pause is BestEffort on every OS (CP-a) — APPLICABLE, demonstrated with
    // the qualifier causes, never skipped. Pin that it is not a skip.
    assert_eq!(
        report.section(section_ids::PAUSE),
        Some(&SectionResult::Pass),
        "BestEffort pause is applicable: the best-effort path must be demonstrated"
    );

    // The one declaration-justified metering skip: SelfReported never owes
    // EngineObserved proof — and the reason NAMES the declaration.
    assert_eq!(
        report.section(section_ids::METERING_ENGINE_OBSERVED),
        Some(&SectionResult::NotApplicable {
            reason: "the declaration declares self-reported metering, so the engine-observed \
                     section does not apply"
                .to_string()
        })
    );

    // The native-launch shape: the code-declared gateway argv is contract, so
    // the config section cannot observe delivered config through a `--dump`
    // seam — not_applicable with the justification, never a silent pass.
    match report.section(section_ids::CONFIG_MAPPING) {
        Some(SectionResult::NotApplicable { reason }) => {
            assert!(
                reason.contains("native") && reason.contains("--dump"),
                "the config skip must name the code-declared launch seam: {reason}"
            );
        }
        other => panic!("expected NotApplicable for config_mapping, got {other:?}"),
    }
}
