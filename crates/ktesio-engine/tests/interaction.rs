//! Integration tests for story-4.1 "send input to any running agent with one
//! command" (FR-24, spine AD-12), driven through the engine's PUBLIC async API
//! and blocking facade only (spine AD-2/AD-13), spawning the REAL `fake_agent`
//! helper (`--echo-stdin`) so delivery is genuinely exercised end to end.
//!
//! The dispatch is proven across:
//! * **AC-A** — the same `send_input` command delivers on a manifest adapter,
//!   and identically across two DIFFERENT manifest-adapter registrations (see
//!   `send_input_works_identically_across_two_adapter_registrations` for why
//!   this — not a native `--kind mock` instance — is what actually proves "no
//!   per-kind branching" in v1).
//! * **AC-B** — `interaction: unsupported` on the current OS fails fast with
//!   `EngineError::CapabilityUnsupported`, no I/O attempted.
//! * **AC-C** — a non-`running` instance fails with `EngineError::NotRunning`.
//! * **AC-D** — the genuinely novel edge case: an instance ADOPTED from a
//!   prior engine session (AD-5) is truly `running` (its Capability
//!   Declaration still truthfully reports `interaction: guaranteed`) but has
//!   no recoverable stdin pipe in THIS engine session, so `send_input` must
//!   fail with `EngineError::InteractionUnavailable` — NEVER
//!   `CapabilityUnsupported`, NEVER a silent success. Also covers the sibling
//!   "no handle at all" branch (mirrors pause.rs's no-in-memory-handle test,
//!   but with the opposite — hard-error — outcome).
//! * **AC-F** — a trailing `\n` is appended only when absent (no double
//!   newline when the caller's text already ends with one).
//! * **Guaranteed == BestEffort** — both interaction levels take the
//!   IDENTICAL delivery path (unlike pause, there is no OS-conditional
//!   difference in writing bytes to a pipe).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use ktesio_engine::{
    AdapterRef, Capability, Engine, EngineError, LifecycleState, OsId, SupportLevel,
};
use tempfile::TempDir;

/// The current-OS key for a manifest `[capabilities.*]` table, as the wire
/// string the manifest uses (`linux`/`macos`/`windows`). Runtime data, not cfg.
fn current_os_key() -> &'static str {
    match OsId::current() {
        OsId::Linux => "linux",
        OsId::Macos => "macos",
        OsId::Windows => "windows",
        OsId::Other => "other",
    }
}

/// Write a manifest whose `[lifecycle.start]` exec points at `fake_agent` with
/// `args`, and whose `[capabilities.interaction]` declares `interaction_level`
/// for the CURRENT OS (so the effective projection is exactly that level at
/// read time). No `[interaction]` channel table — its absence still means
/// "stdio" (the engine's unconditional pipe, Task 1, IS that default).
fn write_interaction_manifest(
    dir: &Path,
    kind: &str,
    args: &[&str],
    interaction_level_current_os: &str,
) {
    let bin = ktesio_conformance::fake_agent_bin();
    let args_toml = args
        .iter()
        .map(|a| format!("{a:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "{kind}"

[lifecycle.start]
exec = {exec:?}
args = [{args_toml}]

[capabilities.interaction]
{os} = "{interaction_level_current_os}"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
        os = current_os_key(),
    );
    std::fs::write(dir.join("adapter.toml"), body).unwrap();
}

/// Open an engine over a fresh temp state dir.
fn open(base: &TempDir) -> Engine {
    Engine::open(Some(base.path().to_path_buf())).expect("open engine")
}

/// The agent.log path inside an instance's Agent Home.
fn agent_log_path(base: &Path, name: &str) -> PathBuf {
    base.join("agents")
        .join(name)
        .join("logs")
        .join("agent.log")
}

/// Wall-clock budget for the real-child-process SPAWN / TEARDOWN poll waits in
/// this file — `wait_for_agent_pid` and `wait_until_gone`, the only two callers.
///
/// AI-67: under `cargo tarpaulin`'s LLVM source-coverage instrumentation, a
/// real child-process round trip (engine dispatch + a real child, BOTH
/// instrumented) runs orders of magnitude slower, so the deadlines tuned for
/// plain `cargo nextest` are missed DETERMINISTICALLY on the coverage job.
/// Gate the inflation on `cfg!(tarpaulin)` (the coverage build passes
/// `--cfg=tarpaulin`) so ONLY the instrumented run gets the generous budget;
/// the normal `nextest` job keeps its fast, tight `plain_secs` bound untouched
/// (proven by `cargo nextest`, which compiles WITHOUT `--cfg=tarpaulin`).
///
/// This inflation is a THROUGHPUT margin and nothing more: it buys time for a
/// round trip that DOES still complete under instrumentation, only slowly. It
/// deliberately no longer covers `wait_for_stdin_line` — that helper's round
/// trip does not complete under instrumentation AT ALL, a tool incompatibility
/// no deadline value can fix (see the AI-67 block below it).
///
/// 120s stays under tarpaulin's 180s per-test `--timeout` AND nextest's 240s
/// terminate-after, so a genuine miss still fires THIS poll's own assertion
/// (the useful "never observed ..." message) rather than an opaque outer
/// timeout kill.
///
/// `cfg!(tarpaulin)` is a coverage-tool cfg (set by cargo-tarpaulin), NOT an
/// OS-conditional cfg, so it sits outside the AD-4 backends-only OS-`cfg`
/// boundary gate — and it is a plain value chosen at compile time, with no
/// `#[cfg]` attribute at all.
fn io_deadline(plain_secs: u64) -> Duration {
    Duration::from_secs(if cfg!(tarpaulin) { 120 } else { plain_secs })
}

/// Poll `agent_log` until it contains a line EQUAL to `wanted` — committed,
/// observable state, never a wall-clock sleep-then-assert (the Epic-2-retro
/// AI-35/38 lesson every later story mirrors).
fn wait_for_stdin_line(agent_log: &Path, wanted: &str) {
    // Flat 20s, NOT scaled through `io_deadline`. Every caller of this helper is
    // one of the four AI-67 stdin-round-trip tests below, and all four are
    // `#[ignore]`d under `cargo tarpaulin` — so this poll never runs under
    // instrumentation and an inflated bound would be unreachable dead weight
    // that also misdescribes the failure ("merely slow" rather than "never
    // completes"). Keeping the honest 20s means that if a FUTURE test ever calls
    // this helper without carrying the same skip, it fails fast and loudly at
    // 20s instead of silently burning 120s per call on the coverage job.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(contents) = std::fs::read_to_string(agent_log) {
            if contents.lines().any(|l| l == wanted) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "never observed {wanted:?} in {}",
            agent_log.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- AI-67: the four stdin-round-trip tests are IGNORED under `cargo tarpaulin` ----
//
// WHY (root cause, pinned — NOT a timing margin). The four tests marked below
// prove delivery by writing to a running `fake_agent` child's stdin and polling
// for the ECHO the child writes back to its agent.log. Under `cargo tarpaulin`'s
// LLVM source-coverage instrumentation the CHILD IS INSTRUMENTED TOO and the
// echo never comes back at all: each of the four burned its FULL deadline on
// every run, on every fresh runner (at a 120s bound the interaction binary ran
// ~487s ≈ 4 x 120s), and each panicked inside the POLL helper — `send_input`
// itself returned `Ok`, so the write landed and only the round trip is broken.
// Three competing hypotheses (runner contention, the engine usage-drain lock,
// deadline margin) were tested and falsified. No deadline value can fix a round
// trip that never completes, so the coverage job is simply not a place these
// four can run.
//
// NOT a blanket "stdin cannot be tested under coverage" claim. The backend unit
// test `write_stdin_delivers_a_line_that_is_echoed_into_the_captured_log`
// completes the SAME echo round trip under instrumentation at a 120s bound and
// PASSES — it drives the backend directly, with no Engine/Supervisor dispatch
// layer also instrumented in the path. That test is what keeps the OS-level
// stdin plumbing credited under coverage, and its inflation is load-bearing and
// stays. What is unavailable under instrumentation is specifically the FULL
// engine-dispatch round trip these four need.
//
// WHY `#[cfg_attr(tarpaulin, ignore = ...)]` AND NOT A RUNTIME EARLY-RETURN.
// This repo's hard-won story-4-3 lesson is that a runtime early-return is
// reported by the harness as **PASSED**, so CI shows green for assertions that
// never ran — which is exactly why the OS-limited tests in
// `crates/kt/tests/agent_cli.rs` carry a visible `_unix` name SUFFIX. Here the
// condition is known at COMPILE time, so the stronger form of that same honesty
// is available: `#[ignore]` is the harness's own first-class skip. The test is
// listed as `... ignored, <reason>` and counted in a SEPARATE "N ignored" tally,
// never as a pass, and the reason string travels with it into the CI log. A name
// suffix was deliberately NOT added on top: unlike the `_unix` tests, these four
// DO run, unskipped and fully asserted, in the primary 3-OS `test` job — the
// caveat belongs to one measurement job, not to the tests' identity.
//
// COVERAGE HONESTY. What these four uniquely exercise is the `send_input`
// delivery path end to end. That path is still gated on all three OSes by the
// plain `cargo nextest` `test` job, which compiles WITHOUT `--cfg=tarpaulin` and
// runs them normally and reliably; only the coverage NUMBER loses their credit.
//
// FRAGILITY NOTE for a future maintainer: adding `--ignored` (or a `--run-types`
// that includes ignored tests) to the coverage job's tarpaulin invocation in
// `.github/workflows/ci.yml` would silently re-enable all four and put the job
// back into the same deterministic red.

#[test]
#[cfg_attr(
    tarpaulin,
    ignore = "AI-67: the instrumented fake_agent child never completes the stdin round trip under \
              cargo tarpaulin; fully exercised by the plain 3-OS test job"
)]
fn send_input_delivers_text_to_a_running_manifest_adapter_agent() {
    // AC-A + AC-F: one continuous engine session — register a manifest
    // adapter pointing `[lifecycle.start]` at `fake_agent --echo-stdin`,
    // declaring `[capabilities.interaction] <current-os> = "guaranteed"`,
    // start it, send text, and poll the captured log for the echoed line.
    // Also proves AC-F's newline handling both ways: a text with NO trailing
    // newline gets exactly one appended, and a text that ALREADY ends with
    // `\n` gets no SECOND one (no stray empty `stdin: ` line).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_interaction_manifest(
        manifest.path(),
        "svc",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    let started = facade.start("svc").unwrap();
    assert_eq!(started.state, LifecycleState::Running);

    let agent_log = agent_log_path(state.path(), "svc");

    // No trailing newline → exactly one appended.
    facade.send_input("svc", "hello").unwrap();
    wait_for_stdin_line(&agent_log, "stdin: hello");

    // Already ends with `\n` → NOT doubled (no extra blank "stdin: " line).
    facade.send_input("svc", "world\n").unwrap();
    wait_for_stdin_line(&agent_log, "stdin: world");

    let contents = std::fs::read_to_string(&agent_log).unwrap();
    let stdin_lines: Vec<&str> = contents
        .lines()
        .filter(|l| l.starts_with("stdin:"))
        .collect();
    assert_eq!(
        stdin_lines,
        vec!["stdin: hello", "stdin: world"],
        "exactly two echoed lines, neither doubled by a stray appended newline: {contents:?}"
    );

    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
}

#[test]
#[cfg_attr(
    tarpaulin,
    ignore = "AI-67: the instrumented fake_agent child never completes the stdin round trip under \
              cargo tarpaulin; fully exercised by the plain 3-OS test job"
)]
fn send_input_works_identically_across_two_adapter_registrations() {
    // AC-A's "the same command works on both the mock adapter and a manifest
    // adapter" — VERIFIED (see the story's Dev Notes): `resolve_start_launch`
    // unconditionally returns `LaunchResolveError::NativeHasNoLaunch` whenever
    // `manifest_path` is `None`, which is ALWAYS true for a native adapter
    // (`--kind mock`) — so a native instance cannot be started as a real
    // process at all in v1 (see
    // `resolve_start_launch_native_has_no_launch_command`). This test
    // therefore satisfies AC-A's actual INTENT — `send_input` needs ZERO
    // per-kind branching — via TWO manifest-adapter registrations under
    // DIFFERENT `kind` strings; both still route through the IDENTICAL
    // `ProcessBackend`/`SpawnSpec` mechanism, which is the real property worth
    // proving. Documented here precisely so a future reader does not mistake
    // this for an oversight rather than a deliberate, verified substitution.
    let state = TempDir::new().unwrap();
    let manifest_a = TempDir::new().unwrap();
    let manifest_b = TempDir::new().unwrap();
    write_interaction_manifest(
        manifest_a.path(),
        "kind-a",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );
    write_interaction_manifest(
        manifest_b.path(),
        "kind-b",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("a", &AdapterRef::Manifest(manifest_a.path().to_path_buf()))
        .unwrap();
    facade
        .register_with_adapter("b", &AdapterRef::Manifest(manifest_b.path().to_path_buf()))
        .unwrap();
    facade.start("a").unwrap();
    facade.start("b").unwrap();

    // The SAME send_input call, unchanged, delivers to BOTH registrations.
    facade.send_input("a", "from-a").unwrap();
    facade.send_input("b", "from-b").unwrap();

    wait_for_stdin_line(&agent_log_path(state.path(), "a"), "stdin: from-a");
    wait_for_stdin_line(&agent_log_path(state.path(), "b"), "stdin: from-b");

    facade.stop("a", Some(Duration::from_secs(5))).unwrap();
    facade.stop("b", Some(Duration::from_secs(5))).unwrap();
}

#[test]
fn send_input_on_unsupported_interaction_fails_fast_with_no_io() {
    // AC-B: a manifest declaring interaction ONLY for a DIFFERENT OS (so the
    // current-OS projection is the honest Unsupported default — mirrors
    // pause.rs's `unsupported_pause_fails_fast_with_no_state_change_and_no_event`)
    // makes `send_input` FAIL FAST with `EngineError::CapabilityUnsupported`
    // naming "interaction" + the OS + the level — and NO I/O is attempted (no
    // `stdin:` line ever appears in the captured log).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    let other_os = match OsId::current() {
        OsId::Linux => "windows",
        OsId::Macos => "windows",
        OsId::Windows => "linux",
        OsId::Other => "linux",
    };
    let bin = ktesio_conformance::fake_agent_bin();
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "unsup"

[lifecycle.start]
exec = {exec:?}
args = ["--echo-stdin", "--linger-ms", "600000"]

[capabilities.interaction]
{other} = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
        other = other_os,
    );
    std::fs::write(manifest.path().join("adapter.toml"), body).unwrap();

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "unsup",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();
    facade.start("unsup").unwrap();

    let err = facade.send_input("unsup", "hello").unwrap_err();
    assert!(
        matches!(err, EngineError::CapabilityUnsupported { .. }),
        "expected CapabilityUnsupported, got {err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("interaction"), "{msg}");
    assert!(msg.contains("unsupported"), "{msg}");
    assert!(msg.contains(current_os_key()), "{msg}");

    // No I/O attempted: the fail-fast check runs BEFORE any process I/O, so
    // the log never gains a `stdin:` echo line.
    let contents =
        std::fs::read_to_string(agent_log_path(state.path(), "unsup")).unwrap_or_default();
    assert!(
        !contents.lines().any(|l| l.starts_with("stdin:")),
        "no I/O may be attempted on the Unsupported fail-fast path: {contents:?}"
    );

    facade.stop("unsup", Some(Duration::from_secs(5))).unwrap();
}

#[test]
fn send_input_on_a_non_running_instance_is_not_running() {
    // AC-C: `send` is not a lifecycle transition, so a dedicated pre-flight
    // check rejects any instance not in `LifecycleState::Running` — here, a
    // registered-but-never-started instance — with `EngineError::NotRunning`,
    // naming the instance and its actual current state.
    let state = TempDir::new().unwrap();
    let engine = open(&state);
    let facade = engine.blocking();
    facade.register("nat", "mock").unwrap();

    let err = facade.send_input("nat", "hello").unwrap_err();
    match err {
        EngineError::NotRunning { name, state } => {
            assert_eq!(name, "nat");
            assert_eq!(state, "registered");
        }
        other => panic!("expected NotRunning, got {other:?}"),
    }
}

#[test]
fn send_input_on_a_running_row_with_no_in_memory_handle_is_interaction_unavailable() {
    // AC-D coverage (the "no handle at all" branch, distinct from the
    // "has_stdin == false on an adopted handle" branch the AC-D adoption test
    // below covers): an instance whose row says `running` but for which THIS
    // engine holds NO in-memory handle at all must ALSO fail honestly with
    // `InteractionUnavailable` — the OPPOSITE of `pause`'s "no handle = a
    // harmless no-op" tolerance (mirrors pause.rs's
    // `guaranteed_pause_without_an_in_memory_handle_is_a_no_op_transition`,
    // but `send_input` has no equivalent "desired end state" for undelivered
    // text, so this must be a hard error, never a silent success).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_interaction_manifest(
        manifest.path(),
        "svc",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );
    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    // Force the row to `running` directly in the state DB — no in-memory
    // handle (the instance was never actually started THIS engine session).
    {
        let conn = rusqlite::Connection::open(state.path().join("state.db")).unwrap();
        let n = conn
            .execute(
                "UPDATE agent_instances SET state = 'running' WHERE name = 'svc'",
                [],
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    let err = facade.send_input("svc", "hello").unwrap_err();
    assert!(
        matches!(err, EngineError::InteractionUnavailable { .. }),
        "expected InteractionUnavailable (no in-memory handle), got {err:?}"
    );
}

#[test]
#[cfg_attr(
    tarpaulin,
    ignore = "AI-67: the instrumented fake_agent child never completes the stdin round trip under \
              cargo tarpaulin; fully exercised by the plain 3-OS test job"
)]
fn send_input_best_effort_still_delivers() {
    // Completeness (not an epics.md-explicit AC, but the exhaustive dispatch
    // needs it covered): a manifest declaring `interaction: best-effort`
    // still delivers the bytes — Guaranteed and BestEffort take the
    // IDENTICAL action (unlike pause, there is no OS-conditional difference
    // in writing to a pipe; best-effort is purely an adapter-author honesty
    // signal, not a different code path).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_interaction_manifest(
        manifest.path(),
        "be",
        &["--echo-stdin", "--linger-ms", "600000"],
        "best-effort",
    );
    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("be", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    facade.start("be").unwrap();

    facade.send_input("be", "hello").unwrap();
    wait_for_stdin_line(&agent_log_path(state.path(), "be"), "stdin: hello");

    facade.stop("be", Some(Duration::from_secs(5))).unwrap();
}

// ---- AC-D: the adopted-instance edge case (mirrors adoption.rs's harness) ----
//
// Faithfully simulating an ENGINE CRASH so the agent OUTLIVES it (a `kill -9`
// of the engine runs no destructors, so the kill-on-drop handle never fires):
// engine 1's work runs in a SEPARATE child process (a re-exec of THIS test
// binary via the `interaction_adoption_helper_subprocess` entry) that starts
// the agent then `std::process::exit`s WITHOUT dropping the engine. The
// parent test then opens a NEW engine over the SAME state dir, which adopts
// the still-live process — with no recoverable stdin pipe.

/// Linux AND running under CI (GitHub sets `CI`). Mirrors `adoption.rs`'s skip
/// for the heavy re-exec + surviving-orphan harness (#109: an x86 ubuntu-CI-only
/// D-state deadlock, unrelated to this story's logic).
fn is_linux_ci() -> bool {
    OsId::current() == OsId::Linux && std::env::var_os("CI").is_some()
}

/// Whether a pid is still alive. NO OS-cfg here (the gate allowlists only
/// `backends/`); branch on the runtime OS id and shell out, exactly like
/// `adoption.rs`'s identical helper.
fn pid_alive(pid: u32) -> bool {
    match OsId::current() {
        OsId::Windows => {
            let out = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output();
            match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
                Err(_) => false,
            }
        }
        _ => {
            let exists = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            exists && !proc_pid_is_zombie(pid)
        }
    }
}

/// Whether `/proc/<pid>/stat` reports process state `Z` (zombie). No-ops
/// (returns `false`) off Linux, matching `adoption.rs`'s identical helper.
fn proc_pid_is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.split_whitespace().next() == Some("Z"))
        })
        .unwrap_or(false)
}

fn wait_until_gone(pid: u32, what: &str) {
    // Real process-teardown wait (same shape / same AI-67 instrumentation
    // exposure as the other polls). Only the adoption test calls this, and that
    // test is runtime-skipped on Linux CI (#109) — so it does NOT actually run
    // under the coverage job today; scaled through `io_deadline` purely for
    // uniformity + future-proofing should the skip ever be lifted.
    let deadline = Instant::now() + io_deadline(8);
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid} still alive)");
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Read the pid `fake_agent` announced (`ready pid=<n>`) from its agent.log.
fn wait_for_agent_pid(agent_log: &Path) -> u32 {
    // A real spawn -> child-startup -> "ready pid=" round trip. UNLIKE
    // `wait_for_stdin_line`, this one genuinely COMPLETES under instrumentation
    // (it needs only the child to reach startup and write its own log line — no
    // stdin echo back through an instrumented reader), it is merely slow. The
    // non-skipped `..._sniffs_stdin_at_startup...` test DOES exercise it under
    // the coverage job on its thin 10s bound, so AI-67's inflation is still
    // load-bearing HERE and stays.
    let deadline = Instant::now() + io_deadline(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(agent_log) {
            if let Some(line) = contents.lines().find(|l| l.contains("ready pid=")) {
                if let Some(idx) = line.find("pid=") {
                    if let Ok(pid) = line[idx + 4..].trim().parse::<u32>() {
                        return pid;
                    }
                }
            }
        }
        assert!(Instant::now() < deadline, "agent pid never announced");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run "engine 1" in a SEPARATE child process (mirrors `adoption.rs`'s
/// `run_engine1`): register + start `svc`, then exit WITHOUT dropping the
/// engine (crash semantics). Blocks until the child exits.
fn run_engine1(state: &Path, manifest: &Path) {
    let exe = std::env::current_exe().expect("test exe");
    let status = Command::new(exe)
        .args([
            "--exact",
            "interaction_adoption_helper_subprocess",
            "--nocapture",
        ])
        .env("KTESIO_INTERACTION_ADOPTION_HELPER", "1")
        .env("KTESIO_INTERACTION_ADOPTION_STATE", state)
        .env("KTESIO_INTERACTION_ADOPTION_MANIFEST", manifest)
        .status()
        .expect("run engine-1 helper subprocess");
    assert!(
        status.success(),
        "engine-1 helper subprocess failed: {status}"
    );
}

/// The re-exec entry for the "engine 1" work (see the section docs above).
/// When `KTESIO_INTERACTION_ADOPTION_HELPER` is unset this is a trivial pass
/// (it runs as a normal test in the parent binary too, and does nothing).
#[test]
fn interaction_adoption_helper_subprocess() {
    let Ok(_mode) = std::env::var("KTESIO_INTERACTION_ADOPTION_HELPER") else {
        return; // normal in-process invocation: nothing to do.
    };
    let state = PathBuf::from(std::env::var("KTESIO_INTERACTION_ADOPTION_STATE").unwrap());
    let manifest = PathBuf::from(std::env::var("KTESIO_INTERACTION_ADOPTION_MANIFEST").unwrap());

    let engine = Engine::open(Some(state)).expect("engine1 open");
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest))
        .unwrap();
    facade.start("svc").unwrap();
    // Crash: exit WITHOUT dropping the engine, so the kill-on-drop handle
    // never fires and the agent (its own session leader) survives, re-parented
    // to init.
    std::process::exit(0);
}

#[test]
fn send_input_on_an_adopted_instance_is_interaction_unavailable() {
    // AC-D — the genuinely novel edge case this story surfaces. An instance
    // that is genuinely `running` (adopted from a prior engine session,
    // AD-5/story 1-6) but whose live stdin pipe cannot be recovered by THIS
    // engine session must fail `send_input` with `InteractionUnavailable` —
    // NEVER `CapabilityUnsupported` (the adapter's Capability Declaration
    // truthfully still reports `interaction: guaranteed`) and NEVER a silent
    // success.
    //
    // Runtime-skip on Windows: this needs the child to genuinely SURVIVE the
    // engine-1 subprocess's exit (Unix re-parenting to init); on Windows
    // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills it when the helper exits, so
    // cross-lifetime survival cannot be simulated there — the SAME reason
    // `adoption.rs`'s survivor tests skip Windows. NO `#[cfg]` (data-driven;
    // this file is outside the backends allowlist).
    if OsId::current() == OsId::Windows {
        return;
    }
    // Temporary CI mitigation (#109), mirroring `adoption.rs`: this harness
    // (heavy re-exec + a surviving orphan process) deadlocks uninterruptibly
    // on the x86-64 ubuntu GitHub runner ONLY. Skip it there; #109 tracks the
    // root cause + un-skip.
    if is_linux_ci() {
        return;
    }

    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_interaction_manifest(
        manifest.path(),
        "svc",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );

    // Engine 1 (a subprocess): start `svc`, then crash (exit without drop).
    run_engine1(state.path(), manifest.path());

    // The process survives the "crash" (re-parented to init).
    let agent_log = agent_log_path(state.path(), "svc");
    let pid = wait_for_agent_pid(&agent_log);
    assert!(pid_alive(pid), "svc must survive the engine crash");

    // Engine 2: open over the SAME state dir → adopt_orphans re-acquires the
    // still-live process into a handle with NO recoverable stdin pipe.
    let engine = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade = engine.blocking();
    let status = facade.instance_status("svc").unwrap();
    assert_eq!(
        status.instance.state,
        LifecycleState::Running,
        "a live orphan must be adopted as running"
    );

    // The Capability Declaration STILL truthfully reports `guaranteed` — this
    // is provably NOT a capability problem.
    let caps = facade.effective_capabilities("svc").unwrap();
    let interaction_level = caps
        .entries
        .iter()
        .find(|(c, _)| *c == Capability::Interaction)
        .map(|(_, level)| *level);
    assert_eq!(
        interaction_level,
        Some(SupportLevel::Guaranteed),
        "the adapter's declaration is unchanged and still says guaranteed"
    );

    // send_input on the ADOPTED instance fails — InteractionUnavailable,
    // NEVER CapabilityUnsupported, NEVER a silent success.
    let err = facade.send_input("svc", "hello").unwrap_err();
    assert!(
        matches!(err, EngineError::InteractionUnavailable { .. }),
        "expected InteractionUnavailable, got {err:?}"
    );
    let msg = err.to_string().to_lowercase();
    assert!(
        !msg.contains("unsupported") && !msg.contains("declares"),
        "must never be misattributed to CapabilityUnsupported: {msg}"
    );

    // No input was ever delivered (no partial/garbled write).
    let contents = std::fs::read_to_string(&agent_log).unwrap();
    assert!(
        !contents.lines().any(|l| l.starts_with("stdin:")),
        "a failed send must deliver NOTHING: {contents:?}"
    );

    // Teardown: stop the adopted process so no orphan remains.
    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
    wait_until_gone(pid, "stop must terminate the adopted process");
}

// ---- Fix pass (review of #79): the CRITICAL bounded-write-timeout finding
// and the HIGH conditional-piping finding. ----

#[test]
fn unsupported_interaction_agent_that_sniffs_stdin_at_startup_reaches_running_promptly() {
    // HIGH finding fix: the story's ORIGINAL implementation piped stdin
    // UNCONDITIONALLY for every spawned process, including adapters that
    // declare `interaction: unsupported`. An adversarial audit showed this
    // hangs a process that does a common "sniff for piped input at startup"
    // idiom (a blocking read of one stdin line before its normal ready
    // loop): the engine holds the pipe's write end open for the process's
    // whole supervised lifetime, so the child never sees EOF and never
    // unblocks — yet is reported `running` regardless (readiness here is
    // just "the process didn't exit immediately"), a silent deadlock with no
    // error signal anywhere. The fix gates piping on the declared
    // Capability::Interaction level: `unsupported` now gets `Stdio::null()`
    // (the pre-story-4.1 safe default), so the sniff read sees immediate EOF
    // and startup proceeds normally.
    //
    // Merely asserting `state == Running` would NOT catch a regression here
    // (a process blocked on a stdin read has not EXITED, so the engine's
    // ~300ms "didn't die immediately" readiness watch would still call it
    // `running` even if genuinely deadlocked). The real proof is that the
    // agent's OWN "ready pid=" line — printed only AFTER the sniff read
    // returns — actually appears in the captured log within a bound FAR
    // shorter than "never" (10s; the old bug would hang forever, not merely
    // slowly).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    let other_os = match OsId::current() {
        OsId::Linux => "windows",
        OsId::Macos => "windows",
        OsId::Windows => "linux",
        OsId::Other => "linux",
    };
    let bin = ktesio_conformance::fake_agent_bin();
    // Declares interaction only for a DIFFERENT os (mirrors
    // send_input_on_unsupported_interaction_fails_fast_with_no_io) so the
    // CURRENT-os effective level honestly projects to Unsupported (the
    // registration-time has_any_support() bar is satisfied by the OTHER os's
    // entry; the per-OS projection this test cares about is unaffected).
    let body = format!(
        r#"
contract_version = "0.1.0"

[adapter]
kind = "sniffer"

[lifecycle.start]
exec = {exec:?}
args = ["--sniff-stdin-at-startup", "--linger-ms", "600000"]

[capabilities.interaction]
{other} = "guaranteed"

[metering]
source = "self-reported"
"#,
        exec = bin.to_string_lossy(),
        other = other_os,
    );
    std::fs::write(manifest.path().join("adapter.toml"), body).unwrap();

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "sniffer",
            &AdapterRef::Manifest(manifest.path().to_path_buf()),
        )
        .unwrap();

    // Sanity: the effective level on THIS os really is Unsupported (so this
    // test is honestly exercising the `pipe_stdin == false` path, not some
    // other configuration).
    let caps = facade.effective_capabilities("sniffer").unwrap();
    let interaction_level = caps
        .entries
        .iter()
        .find(|(c, _)| *c == Capability::Interaction)
        .map(|(_, level)| *level);
    assert_eq!(interaction_level, Some(SupportLevel::Unsupported));

    let started = facade.start("sniffer").unwrap();
    assert_eq!(started.state, LifecycleState::Running);

    let agent_log = agent_log_path(state.path(), "sniffer");
    // Asserts internally (within a 10s bound) if the ready line never
    // appears — the regression-catching proof.
    let _pid = wait_for_agent_pid(&agent_log);

    facade
        .stop("sniffer", Some(Duration::from_secs(5)))
        .unwrap();
}

#[test]
#[cfg_attr(
    tarpaulin,
    ignore = "AI-67: the instrumented fake_agent child never completes the stdin round trip under \
              cargo tarpaulin; fully exercised by the plain 3-OS test job"
)]
fn a_stuck_instances_send_times_out_and_does_not_block_a_different_instances_send_beyond_the_bound()
{
    // CRITICAL finding fix: the engine has ONE global `Mutex<Supervisor>`
    // guarding EVERY instance (`EngineInner::supervisor` in `engine.rs`),
    // and `send_input`'s write used to be a bare, UNBOUNDED `write_all`
    // performed WHILE that lock was held — so a genuinely stuck agent (one
    // that never drains its stdin) could freeze the ENTIRE engine forever:
    // no other instance's `start`/`stop`/`pause`/`send`, and not even the
    // crash-detection reaper, could proceed until the stuck write returned
    // (which, for a truly non-exiting agent, is never). An adversarial audit
    // reproduced this empirically: a large payload to a non-draining `stuck`
    // instance blocked an unrelated `healthy` instance's send on a SEPARATE
    // thread, unblocking only once `stuck` itself happened to self-exit.
    //
    // This test proves THREE properties of the fix in ONE flow (sharing the
    // one expensive real-timeout wait — `STDIN_WRITE_TIMEOUT` = 5s — rather
    // than paying it three times over):
    //   1. a send to a non-draining agent times out within roughly the
    //      bound (`Err(InteractionTimedOut)`), not indefinitely;
    //   2. WHILE that write is stuck, a DIFFERENT, unrelated instance's send
    //      still completes, bounded to roughly the FIRST instance's timeout
    //      — the stall is bounded, not unbounded;
    //   3. a SUBSEQUENT send to the SAME (now-timed-out) instance returns
    //      `InteractionTimedOut` immediately, with no new wait (the cheap,
    //      no-I/O fast path).
    let state = TempDir::new().unwrap();
    let stuck_manifest = TempDir::new().unwrap();
    let healthy_manifest = TempDir::new().unwrap();
    // `stuck`: NO --echo-stdin, so nothing ever reads its piped stdin —
    // exactly the adversarial audit's reproduction vehicle.
    write_interaction_manifest(
        stuck_manifest.path(),
        "stuck",
        &["--linger-ms", "600000"],
        "guaranteed",
    );
    // `healthy`: --echo-stdin, so ITS writes complete near-instantly and are
    // independently verifiable as genuinely delivered.
    write_interaction_manifest(
        healthy_manifest.path(),
        "healthy",
        &["--echo-stdin", "--linger-ms", "600000"],
        "guaranteed",
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter(
            "stuck",
            &AdapterRef::Manifest(stuck_manifest.path().to_path_buf()),
        )
        .unwrap();
    facade
        .register_with_adapter(
            "healthy",
            &AdapterRef::Manifest(healthy_manifest.path().to_path_buf()),
        )
        .unwrap();
    facade.start("stuck").unwrap();
    facade.start("healthy").unwrap();

    // Far larger than any realistic OS pipe buffer (Linux defaults to
    // 64KiB) — `stuck` never reads its stdin, so a write of this size WILL
    // block once the buffer fills.
    let huge_payload = "x".repeat(8 * 1024 * 1024);

    // Two REAL OS threads contending for the SAME engine's ONE supervisor
    // lock: the scoped child thread drives `stuck`'s send (which will block
    // for the full bound), while THIS (the scope's own) thread drives
    // `healthy`'s send concurrently, after a short head start to ensure
    // `stuck` has genuinely acquired the lock first.
    let (stuck_result, stuck_elapsed) = thread::scope(|scope| {
        let stuck_handle = scope.spawn(|| {
            let start = Instant::now();
            let result = engine.blocking().send_input("stuck", &huge_payload);
            (result, start.elapsed())
        });

        thread::sleep(Duration::from_millis(300));

        // Property #2: healthy's send, through the SAME engine (SAME
        // Mutex<Supervisor>), must still complete — bounded to roughly the
        // stuck call's timeout, never indefinitely.
        let healthy_start = Instant::now();
        let healthy_result = engine.blocking().send_input("healthy", "hi");
        let healthy_elapsed = healthy_start.elapsed();
        assert!(
            healthy_result.is_ok(),
            "a different, unrelated instance's send must still succeed once the stuck \
             instance's bounded wait elapses: {healthy_result:?}"
        );
        assert!(
            healthy_elapsed < Duration::from_secs(9),
            "healthy's send must be bounded to roughly the stuck call's timeout (5s), \
             never blocked indefinitely by an unrelated instance: {healthy_elapsed:?}"
        );

        stuck_handle.join().expect("stuck thread must not panic")
    });

    // Property #1: the stuck call itself must time out (not hang forever,
    // not silently succeed), within roughly 2x its own bound.
    match stuck_result {
        Err(EngineError::InteractionTimedOut { name, timeout_secs }) => {
            assert_eq!(name, "stuck");
            assert_eq!(timeout_secs, 5);
        }
        other => panic!("expected InteractionTimedOut, got {other:?}"),
    }
    assert!(
        stuck_elapsed < Duration::from_secs(9),
        "the stuck send must return within roughly 2x its bound (5s), not indefinitely: \
         {stuck_elapsed:?}"
    );

    // Property #3: a SUBSEQUENT send to the SAME (now-permanently-timed-out)
    // instance fails IMMEDIATELY — a cheap check, no new attempted write, no
    // new wait through another full timeout.
    let retry_start = Instant::now();
    let retry_result = engine.blocking().send_input("stuck", "second attempt");
    let retry_elapsed = retry_start.elapsed();
    match retry_result {
        Err(EngineError::InteractionTimedOut { name, .. }) => assert_eq!(name, "stuck"),
        other => panic!("expected InteractionTimedOut again, got {other:?}"),
    }
    assert!(
        retry_elapsed < Duration::from_secs(1),
        "a subsequent send on an already-timed-out instance must fail IMMEDIATELY (a \
         cheap check, no new attempted write), not wait out another timeout: {retry_elapsed:?}"
    );

    // Confirm `healthy` genuinely delivered (a real write completed, not a
    // coincidentally-fast failure).
    wait_for_stdin_line(&agent_log_path(state.path(), "healthy"), "stdin: hi");
    // Confirm `stuck` genuinely received NOTHING (the timed-out write, and
    // the immediately-rejected retry, both delivered no bytes).
    let stuck_log =
        std::fs::read_to_string(agent_log_path(state.path(), "stuck")).unwrap_or_default();
    assert!(
        !stuck_log.contains("stdin:"),
        "stuck never echoes (no --echo-stdin), but sanity-confirm no stray output: {stuck_log:?}"
    );

    // Teardown. `stuck`'s abandoned write thread (see `write_stdin_bounded`'s
    // docs — it is intentionally never joined/aborted) unblocks once the
    // process is killed below (its blocked write() call gets EPIPE once the
    // reader is gone) and cleans itself up in the background; the test does
    // not wait for it.
    facade.stop("stuck", Some(Duration::from_secs(5))).unwrap();
    facade
        .stop("healthy", Some(Duration::from_secs(5)))
        .unwrap();
}
