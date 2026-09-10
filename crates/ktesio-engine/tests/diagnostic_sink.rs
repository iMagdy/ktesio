//! Story 10-2 (Epic 10): the host-provided diagnostic sink.
//!
//! The engine's TWO operational diagnostics — the DC-10 memory-delivery
//! notice and the enforcement breadcrumb — route through a host-provided
//! [`DiagnosticSink`](ktesio_engine::DiagnosticSink) when one is installed
//! (at open via `Engine::open_with_diagnostics`, or any time later via
//! `Engine::with_diagnostics` / `Blocking::with_diagnostics`), and keep their
//! historical stderr behavior when none is. These tests pin the whole
//! acceptance surface:
//!
//! * **Sink receives BOTH diagnostics' exact texts** — driven through the
//!   REAL production paths, not synthetic emissions: the notice via the
//!   attach-unmapped-manifest-then-start path (the DC-10 fault-install), and
//!   the breadcrumb via a budget breach with `breach_action = pause` on an
//!   instance the token breach has ALREADY paused (the dollar dimension then
//!   fails to enforce a second pause — the exact production path the 7-1
//!   suites exercise; the enforcement site evaluates the TOKEN ceilings
//!   first, the pause commits synchronously in the same lock pass, and the
//!   dollar breach's `enforce_pause` hits the already-paused transition
//!   gate). The expected lines are computed from the run itself (the
//!   engine-reported managed dir; the SAME transition-gate error the
//!   enforcement path received, reproduced through the facade), so the
//!   assertions are byte-exact without hardcoding an error string.
//! * **No-sink default is byte-identical to today** — a subprocess re-exec of
//!   this test binary (the `adoption.rs` pattern; std cannot capture a
//!   process's own stderr) runs the SAME flow with no sink and the parent
//!   asserts the child's captured stderr carries exactly the two `[ktesio]`
//!   lines and nothing else — same wording, same stream, one line each.
//! * **With a sink installed, stderr stays silent** — the sink-mode child
//!   drives the same flow with the sink installed; the parent asserts the
//!   child's stderr contains no `[ktesio]` line at all while the sink's
//!   captured bytes (relayed via a file) equal the expected pair exactly.
//!
//! Determinism posture (the house style): every wait polls COMMITTED state
//! (the shared `uj3` bounded poller) or the sink's own captured bytes — never
//! a wall-clock guess. No `OsId` gate; runs unmodified on all three OSes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ktesio_conformance::test_support::ManifestFixture;
use ktesio_conformance::uj3;
use ktesio_engine::{AdapterRef, DiagnosticSink, Engine, LifecycleState, MemoryBackingKind};
use tempfile::TempDir;

/// The instance that fires the DC-10 memory-delivery notice (an unmapped
/// manifest adapter with a `filesystem` backing attached).
const NOTICE_INSTANCE: &str = "svc";

/// The instance that fires the enforcement breadcrumb (the uj3 flow: token
/// ceiling AND dollar cap, both with `breach_action = pause`).
const BREADCRUMB_INSTANCE: &str = "flow-probe";

/// The shared bounded budget for committed-state / sink-content polls.
const POLL_BUDGET: Duration = Duration::from_secs(30);

/// Env var turning the re-exec'd test binary into the helper child (the
/// `adoption.rs` pattern). Value: `default` or `sink`.
const HELPER_ENV: &str = "KTESIO_DIAG_SINK_HELPER";
/// Env var: the child's engine state root (created by the child).
const STATE_ENV: &str = "KTESIO_DIAG_SINK_STATE";
/// Env var: where the child writes the two EXPECTED diagnostic lines.
const EXPECTED_ENV: &str = "KTESIO_DIAG_SINK_EXPECTED";
/// Env var: where the sink-mode child relays the sink's captured bytes.
const CAPTURED_ENV: &str = "KTESIO_DIAG_SINK_CAPTURED";

// ---------------------------------------------------------------------------
// The shared sink capture + the shared two-diagnostic flow
// ---------------------------------------------------------------------------

/// A `Write` adapter delegating into a shared buffer, so the test can read the
/// sink's captured bytes while the engine owns the boxed writer. This mirrors
/// the real host shape: a sink that routes lines into state shared with the
/// rest of the host process.
#[derive(Clone)]
struct SharedCapture(Arc<Mutex<Vec<u8>>>);

impl Write for SharedCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Build the engine-named sink handle over a shared capture buffer.
fn make_sink(shared: &Arc<Mutex<Vec<u8>>>) -> DiagnosticSink {
    Arc::new(Mutex::new(Box::new(SharedCapture(Arc::clone(shared)))))
}

/// The unmapped manifest fixture: a `fake_agent` manifest with NO `[config]`
/// section — no target for the reserved `memory.dir` key, the DC-10 notice's
/// trigger — lingering so the instance runs. Same shape as `memory.rs`'s
/// unmapped fixture (story 10-1's shared builder).
fn write_unmapped_manifest(dir: &Path) -> PathBuf {
    ManifestFixture::fake_agent(NOTICE_INSTANCE, &["--linger-ms", "600000"]).write(dir)
}

/// Drive BOTH diagnostics through their real production paths on ONE engine
/// and return `(notice_line, breadcrumb_line, breadcrumb_needle)` — the exact
/// expected diagnostic LINES (without the trailing newlines the emitter
/// appends) plus the fixed needle sink-mode callers poll for:
///
/// 1. the DC-10 notice — register the unmapped manifest adapter, attach a
///    `filesystem` backing, start (the start still SUCCEEDS and the notice
///    fires, naming the engine-reported managed dir);
/// 2. the enforcement breadcrumb — the uj3 flow (token ceiling + dollar cap,
///    `breach_action = pause`): the token breach pauses the instance, and the
///    dollar breach's `enforce_pause` then fails on the already-paused gate —
///    the exact production path from 7-1's suites. The expected line's
///    `{detail}` is the SAME transition-gate error the enforcement path
///    received, reproduced by pausing the already-paused instance through the
///    facade (a gate rejection in both paths, no side effect either way).
fn drive_both_diagnostics(
    engine: &Engine,
    state: &Path,
    unmapped: &Path,
    flow: &Path,
) -> (String, String, String) {
    let facade = engine.blocking();

    // ---- (1) The DC-10 memory-delivery notice. ----
    facade
        .register_with_adapter(
            NOTICE_INSTANCE,
            &AdapterRef::Manifest(unmapped.to_path_buf()),
        )
        .expect("register the unmapped manifest instance");
    let dir = facade
        .attach_memory(NOTICE_INSTANCE, MemoryBackingKind::Filesystem)
        .expect("attach the filesystem backing");
    let started = facade
        .start(NOTICE_INSTANCE)
        .expect("the unmapped start STILL succeeds");
    assert_eq!(started.state, LifecycleState::Running);

    // The exact expected line: the choke point emits `[ktesio] ` + the notice
    // text (byte-pinned here, including the engine-reported managed dir) + '\n'.
    let notice_line = format!(
        "[ktesio] {NOTICE_INSTANCE}: a 'filesystem' Memory Backing is attached (managed \
         directory: {}), but this adapter declares no config mapping for the reserved key \
         'memory.dir', so the agent will NOT receive the path. Add [config.\"memory.dir\"] \
         env = \"...\" to its manifest to deliver it.",
        dir.display()
    );

    // ---- (2) The enforcement breadcrumb. ----
    facade
        .register_with_adapter(
            BREADCRUMB_INSTANCE,
            &AdapterRef::Manifest(flow.to_path_buf()),
        )
        .expect("register the flow instance");
    for (key, value) in uj3::flow_config_pairs() {
        facade
            .set_config(BREADCRUMB_INSTANCE, key, value)
            .unwrap_or_else(|e| panic!("set_config {key}={value} failed: {e}"));
    }
    facade
        .start(BREADCRUMB_INSTANCE)
        .expect("start the flow instance");

    // The token breach pauses the instance (committed state, bounded poll);
    // the dollar breach's failed pause rides the same supervisor lock pass.
    uj3::wait_for_state(
        state,
        BREADCRUMB_INSTANCE,
        LifecycleState::Paused,
        POLL_BUDGET,
    );

    // Wait for the breadcrumb to actually ARRIVE (committed-state polling
    // cannot see it — it is one lock pass after the pause commit). `needle` is
    // the breadcrumb's fixed middle; the exact line is asserted by the caller.
    let breadcrumb_needle = "budget breach pause could not be honored:";

    // The expected line, computed from the SAME gate error the enforcement
    // hit. This facade call is also the determinism barrier: it takes the
    // supervisor lock, which the ingestion pass holds through the breadcrumb
    // emission — so once it returns, the diagnostic is already out (this is
    // what makes the no-sink child's exit safe: no diagnostic can still be in
    // flight when the engine drops).
    let pause_err = facade
        .pause(BREADCRUMB_INSTANCE)
        .expect_err("pausing an already-paused instance is a gate rejection");
    let breadcrumb_line = format!(
        "[ktesio] {BREADCRUMB_INSTANCE}: budget breach pause could not be honored: {pause_err}"
    );

    // Teardown (never mask the assertions): the flow instance is SIGSTOP'd
    // (Unix), so its stop takes the shared zero window.
    let _ = facade.stop(NOTICE_INSTANCE, Some(Duration::from_secs(5)));
    let _ = facade.stop(BREADCRUMB_INSTANCE, Some(uj3::STOP_WINDOW));

    (notice_line, breadcrumb_line, breadcrumb_needle.to_string())
}

/// Poll the sink's captured bytes until `needle` arrives (bounded) and return
/// the full capture.
fn wait_for_capture(shared: &Arc<Mutex<Vec<u8>>>, needle: &str) -> Vec<u8> {
    let deadline = Instant::now() + POLL_BUDGET;
    loop {
        let bytes = shared.lock().unwrap().clone();
        if String::from_utf8_lossy(&bytes).contains(needle) {
            return bytes;
        }
        assert!(
            Instant::now() < deadline,
            "the enforcement breadcrumb never reached the sink (captured so far: {})",
            String::from_utf8_lossy(&bytes)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// Sink-installed engines (in process): both diagnostics, exact texts
// ---------------------------------------------------------------------------

#[test]
fn an_open_time_sink_receives_both_diagnostics_exact_texts() {
    // `Engine::open_with_diagnostics`: the airtight install — the sink is in
    // place before any supervision work. Both diagnostics must arrive as the
    // EXACT expected lines, in order, and NOTHING else: the capture equals the
    // two lines byte-for-byte (any extra engine diagnostic would corrupt it).
    let state = TempDir::new().expect("state root");
    let unmapped_dir = TempDir::new().expect("unmapped manifest dir");
    let flow_dir = TempDir::new().expect("flow manifest dir");
    let unmapped = write_unmapped_manifest(unmapped_dir.path());
    let flow = uj3::write_flow_manifest(flow_dir.path());

    let captured = Arc::new(Mutex::new(Vec::new()));
    let engine =
        Engine::open_with_diagnostics(Some(state.path().to_path_buf()), make_sink(&captured))
            .expect("open engine with sink");

    let (notice_line, breadcrumb_line, needle) =
        drive_both_diagnostics(&engine, state.path(), &unmapped, &flow);
    let bytes = wait_for_capture(&captured, &needle);
    assert_eq!(
        bytes,
        format!("{notice_line}\n{breadcrumb_line}\n").into_bytes(),
        "the sink must receive EXACTLY the two diagnostic lines (in emission order)"
    );
}

#[test]
fn a_facade_installed_sink_receives_both_diagnostics_exact_texts() {
    // The post-open install path: `Engine::open` (no sink — the default), then
    // `Blocking::with_diagnostics` BEFORE any instance starts. The engine must
    // behave identically to the open-time install from that point on.
    let state = TempDir::new().expect("state root");
    let unmapped_dir = TempDir::new().expect("unmapped manifest dir");
    let flow_dir = TempDir::new().expect("flow manifest dir");
    let unmapped = write_unmapped_manifest(unmapped_dir.path());
    let flow = uj3::write_flow_manifest(flow_dir.path());

    let captured = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::open(Some(state.path().to_path_buf())).expect("open engine");
    engine.blocking().with_diagnostics(make_sink(&captured));

    let (notice_line, breadcrumb_line, needle) =
        drive_both_diagnostics(&engine, state.path(), &unmapped, &flow);
    let bytes = wait_for_capture(&captured, &needle);
    assert_eq!(
        bytes,
        format!("{notice_line}\n{breadcrumb_line}\n").into_bytes(),
        "the facade-installed sink must receive EXACTLY the two diagnostic lines"
    );
}

// ---------------------------------------------------------------------------
// The no-sink default (subprocess): stderr stays byte-identical to today
// ---------------------------------------------------------------------------

/// The re-exec helper (the `adoption.rs` pattern). With `KTESIO_DIAG_SINK_HELPER`
/// unset this is a no-op pass (the normal in-process run). In `default` mode it
/// drives the SAME two-diagnostic flow with NO sink (stderr inherits the test
/// runner's pipe — the parent captures it); in `sink` mode it installs the sink
/// and relays the captured bytes to the parent via a file. Both modes write the
/// EXPECTED lines to a file so the parent asserts byte-exact contents without
/// duplicating the engine-computed values (the managed dir, the gate error).
#[test]
fn diagnostic_sink_helper_subprocess() {
    let Ok(mode) = std::env::var(HELPER_ENV) else {
        return; // normal in-process invocation: nothing to do.
    };
    let state = PathBuf::from(std::env::var(STATE_ENV).unwrap());
    let expected_path = PathBuf::from(std::env::var(EXPECTED_ENV).unwrap());

    let unmapped_dir = TempDir::new().expect("unmapped manifest dir");
    let flow_dir = TempDir::new().expect("flow manifest dir");
    let unmapped = write_unmapped_manifest(unmapped_dir.path());
    let flow = uj3::write_flow_manifest(flow_dir.path());

    let (notice_line, breadcrumb_line, _needle) = match mode.as_str() {
        // No sink: the diagnostics go to the inherited stderr (the parent's
        // capture). The expected-lines file is the byte-exact assertion target.
        "default" => {
            let engine = Engine::open(Some(state.clone())).expect("open default engine");
            drive_both_diagnostics(&engine, &state, &unmapped, &flow)
        }
        // Sink installed at open: the diagnostics go to the sink; relay the
        // captured bytes to the parent and assert them here too (a child-side
        // failure fails the subprocess, which fails the parent).
        "sink" => {
            let captured = Arc::new(Mutex::new(Vec::new()));
            let engine = Engine::open_with_diagnostics(Some(state.clone()), make_sink(&captured))
                .expect("open sink engine");
            let outcome = drive_both_diagnostics(&engine, &state, &unmapped, &flow);
            let bytes = wait_for_capture(&captured, &outcome.2);
            assert_eq!(
                bytes,
                format!("{}\n{}\n", outcome.0, outcome.1).into_bytes(),
                "the child's sink must receive EXACTLY the two diagnostic lines"
            );
            let captured_path = PathBuf::from(std::env::var(CAPTURED_ENV).unwrap());
            std::fs::write(captured_path, bytes).expect("relay the captured sink bytes");
            (outcome.0, outcome.1, outcome.2)
        }
        other => panic!("unknown helper mode: {other}"),
    };

    std::fs::write(expected_path, format!("{notice_line}\n{breadcrumb_line}\n"))
        .expect("write the expected-lines file");
}

/// Run the helper child in `mode` and return (child stderr, expected-lines
/// path, captured-sink path).
fn run_helper(mode: &str, root: &Path) -> (String, PathBuf, PathBuf) {
    let exe = std::env::current_exe().expect("test exe");
    let state = root.join("state");
    let expected = root.join("expected.txt");
    let captured = root.join("captured.txt");
    let output = Command::new(exe)
        .args([
            "--exact",
            "diagnostic_sink_helper_subprocess",
            "--nocapture",
        ])
        .env(HELPER_ENV, mode)
        .env(STATE_ENV, &state)
        .env(EXPECTED_ENV, &expected)
        .env(CAPTURED_ENV, &captured)
        // `.output()` captures stderr into the result by default.
        .output()
        .expect("run the diagnostic-sink helper subprocess");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "the {mode}-mode helper subprocess failed: {status}; stderr={stderr}",
        status = output.status,
    );
    (stderr, expected, captured)
}

/// Read the two expected lines the child wrote (trailing newline stripped per
/// line for `contains`-style exact-line assertions).
fn read_expected_lines(expected: &Path) -> Vec<String> {
    String::from_utf8(std::fs::read(expected).expect("read the expected-lines file"))
        .expect("expected lines are UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn without_a_sink_both_diagnostics_reach_stderr_byte_identical() {
    // The acceptance row "given no sink … stderr output is byte-identical to
    // today's": the child runs the REAL flow with no sink; the parent asserts
    // the child's stderr carries EXACTLY the two `[ktesio]` diagnostic lines —
    // same wording, same stream, one line each, nothing else engine-emitted.
    let root = TempDir::new().expect("helper root");
    let (stderr, expected, _captured) = run_helper("default", root.path());
    let lines = read_expected_lines(&expected);
    assert_eq!(lines.len(), 2, "the child pins exactly two diagnostics");

    let engine_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("[ktesio]"))
        .collect();
    assert_eq!(
        engine_lines,
        lines.iter().map(String::as_str).collect::<Vec<_>>(),
        "the no-sink stderr must carry EXACTLY the two diagnostic lines \
         (byte-identical to the pre-sink engine): stderr=\n{stderr}"
    );
}

#[test]
fn with_a_sink_installed_stderr_stays_silent() {
    // The acceptance row "given a sink installed … stderr stays silent": the
    // child installs the sink AT OPEN and drives the same flow; the parent
    // asserts NO `[ktesio]` line reached the child's stderr while the relayed
    // sink bytes equal the expected pair exactly.
    let root = TempDir::new().expect("helper root");
    let (stderr, expected, captured) = run_helper("sink", root.path());
    let lines = read_expected_lines(&expected);
    assert_eq!(lines.len(), 2, "the child pins exactly two diagnostics");

    assert!(
        !stderr.contains("[ktesio]"),
        "with a sink installed the engine must not write the diagnostics to \
         stderr: stderr=\n{stderr}"
    );

    let relayed = std::fs::read(captured).expect("read the relayed sink capture");
    assert_eq!(
        relayed,
        format!("{}\n{}\n", lines[0], lines[1]).into_bytes(),
        "the relayed sink capture must equal the two expected lines exactly"
    );
}
