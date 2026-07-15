//! Integration tests for story-4.2 "attach to live output and read retained
//! logs" (FR-25, NFR-5, spine AD-12), driven through the engine's PUBLIC
//! async API and blocking facade only (spine AD-2/AD-13), spawning the REAL
//! `fake_agent` helper so capture, attribution, and rotation are genuinely
//! exercised end to end.
//!
//! The story's non-negotiable constraint — `agent.log` (the legacy, raw,
//! unattributed combined stdout+stderr capture) MUST stay byte-identical to
//! its pre-story format, since Epic 3's `Supervisor::drain_usage_for`
//! (adversarially-reviewed billing-critical metering ingestion) parses it
//! today — is proven by `legacy_agent_log_is_byte_identical_to_pre_story_content`,
//! the automated regression guard CRITICAL SCOPING #3 requires.
//!
//! The dispatch is proven across:
//! * **AC-A/AC-G** — retained logs are ordered (append order, NEVER a
//!   timestamp re-sort), timestamped, and attributed.
//! * **AC-B** — `--follow`'s poll loop observes new lines INCREMENTALLY, not
//!   merely present in a final snapshot.
//! * **AC-C** — follow drains and exits cleanly (never hangs) on stop AND on
//!   pause, with state-appropriate honesty.
//! * **AC-E** — capture is unconditional, independent of declared
//!   `interaction` support (contrast with 4.1's `send`, which IS gated).
//! * **AC-H** — the genuinely novel edge case: an instance ADOPTED from a
//!   prior engine session (AD-5) can still be read/followed from a FRESH
//!   session — the mirror image of 4.1's AC-D (there, `send_input` fails;
//!   here, by design, it succeeds).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ktesio_engine::{
    AdapterRef, Capability, Engine, LifecycleState, LogLine, LogStream, OsId, SupportLevel,
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

/// Write a manifest whose `[lifecycle.start]` exec points at `fake_agent`
/// with `args`, declaring BOTH `[capabilities.interaction]` and
/// `[capabilities.pause]` at `level` for the CURRENT OS (uniform across the
/// tests that don't specifically care about the per-OS honesty dispatch —
/// mirrors `interaction.rs`/`pause.rs`'s fixture manifests).
fn write_logs_manifest(dir: &Path, kind: &str, args: &[&str], level: &str) {
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
{os} = "{level}"

[capabilities.pause]
{os} = "{level}"

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

/// The legacy (raw, unattributed) agent-output log path inside an instance's
/// Agent Home.
fn agent_log_path(base: &Path, name: &str) -> PathBuf {
    base.join("agents")
        .join(name)
        .join("logs")
        .join("agent.log")
}

/// Poll `read_agent_log` until at least `min_out` `agent-out` lines and
/// `min_err` `agent-err` lines are retained (committed, observable state —
/// never a wall-clock sleep, per the Epic-2-retro AI-35/38 lesson every
/// later story internalizes). Returns the full retained set once satisfied.
fn wait_for_min_lines_per_stream(
    facade: &ktesio_engine::Blocking<'_>,
    name: &str,
    min_out: usize,
    min_err: usize,
) -> Vec<LogLine> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let lines = facade.read_agent_log(name).expect("read_agent_log");
        let out = lines
            .iter()
            .filter(|l| l.stream == LogStream::AgentOut)
            .count();
        let err = lines
            .iter()
            .filter(|l| l.stream == LogStream::AgentErr)
            .count();
        if out >= min_out && err >= min_err {
            return lines;
        }
        assert!(
            Instant::now() < deadline,
            "never observed enough lines per stream (want out>={min_out} err>={min_err}, have out={out} err={err})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Assert the numbered lines of `stream` whose text starts with `prefix`
/// (e.g. `"heartbeat "` -> `0, 1, 2, ...`) appear in STRICTLY INCREASING
/// numeric order across `lines` — the append-order fidelity proof (AC-G):
/// since the counter is monotonic at the SOURCE, observing it monotonic in
/// the READ order proves the read preserved emission order (a re-sort by a
/// whole-second timestamp, where many lines share the same `at`, would not
/// reliably preserve this).
fn assert_monotonic_counter(lines: &[LogLine], stream: LogStream, prefix: &str) {
    let mut last: Option<u64> = None;
    for l in lines.iter().filter(|l| l.stream == stream) {
        let Some(n) = l
            .text
            .strip_prefix(prefix)
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if let Some(prev) = last {
            assert!(
                n > prev,
                "counter must be strictly increasing in read/append order: {prev} then {n} ({lines:?})"
            );
        }
        last = Some(n);
    }
    assert!(
        last.is_some(),
        "expected at least one {prefix:?} line: {lines:?}"
    );
}

#[test]
fn retained_logs_are_ordered_timestamped_and_attributed() {
    // AC-A, AC-G: one Engine::open, start a manifest instance emitting on
    // BOTH streams via --heartbeat-ms/--heartbeat-stderr-ms, let several of
    // each accrue, call read_agent_log, and assert: every line has a
    // well-formed RFC3339 `at`, the attribution set is exactly
    // {agent-out, agent-err, engine} (at least one `engine` line from the
    // `start` transition), and order matches emission order.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &[
            "--heartbeat-ms",
            "30",
            "--heartbeat-stderr-ms",
            "30",
            "--linger-ms",
            "600000",
        ],
        "guaranteed",
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    facade.start("svc").unwrap();

    let lines = wait_for_min_lines_per_stream(&facade, "svc", 5, 5);

    for l in &lines {
        assert_eq!(l.at.len(), 20, "RFC3339 whole-second shape: {:?}", l.at);
        assert!(l.at.ends_with('Z'), "{:?}", l.at);
        assert_eq!(l.instance, "svc");
    }

    let streams: std::collections::HashSet<LogStream> = lines.iter().map(|l| l.stream).collect();
    assert_eq!(
        streams,
        [LogStream::AgentOut, LogStream::AgentErr, LogStream::Engine]
            .into_iter()
            .collect(),
        "attribution set must be exactly {{agent-out, agent-err, engine}}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.stream == LogStream::Engine && l.text.contains("-> running")),
        "an engine line from the start transition must be present: {lines:?}"
    );

    assert_monotonic_counter(&lines, LogStream::AgentOut, "heartbeat ");
    assert_monotonic_counter(&lines, LogStream::AgentErr, "stderr-heartbeat ");

    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
}

#[test]
fn follow_observes_new_lines_incrementally_not_just_a_final_snapshot() {
    // AC-B: a background OS thread calls read_agent_log_since in a poll
    // loop while the main test thread drives more heartbeats — assert the
    // follower's observed line count increases across MULTIPLE polls
    // (genuinely incremental), not merely present in one final read.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &["--heartbeat-ms", "20", "--linger-ms", "600000"],
        "guaranteed",
    );
    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    facade.start("svc").unwrap();

    let stop_flag = AtomicBool::new(false);
    let growth_snapshots: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut cursor = 0u64;
            let mut total = 0usize;
            let deadline = Instant::now() + Duration::from_secs(20);
            while !stop_flag.load(Ordering::Relaxed) {
                let (new_lines, next_cursor) = facade
                    .read_agent_log_since("svc", cursor)
                    .expect("read_agent_log_since");
                if !new_lines.is_empty() {
                    total += new_lines.len();
                    growth_snapshots.lock().unwrap().push(total);
                }
                cursor = next_cursor;
                assert!(Instant::now() < deadline, "follower must not hang");
                std::thread::sleep(Duration::from_millis(15));
            }
        });

        // Drive the accrual in explicit ROUNDS, checking after EACH round
        // whether the follower observed FURTHER growth since the previous
        // round — ties the "genuinely incremental" proof to checkpoints the
        // MAIN thread controls, rather than to the follower thread's
        // independent OS-scheduling luck (robust under CI / parallel-test
        // CPU contention, where a single early snapshot could otherwise
        // coincidentally catch everything accrued so far in one batch).
        let mut last_seen = 0usize;
        let mut rounds_with_growth = 0;
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(200)); // ~10 heartbeats/round @ 20ms
            let seen = growth_snapshots
                .lock()
                .unwrap()
                .last()
                .copied()
                .unwrap_or(0);
            if seen > last_seen {
                rounds_with_growth += 1;
                last_seen = seen;
            }
            if rounds_with_growth >= 2 {
                break;
            }
        }
        stop_flag.store(true, Ordering::Relaxed);
        assert!(
            rounds_with_growth >= 2,
            "the follower must observe growth across MULTIPLE distinct rounds, not one final \
             snapshot (rounds_with_growth={rounds_with_growth})"
        );
    });

    let snapshots = growth_snapshots.into_inner().unwrap();
    assert!(
        snapshots.len() >= 2,
        "the follower must observe INCREMENTAL growth across multiple polls, not one final \
         snapshot: {snapshots:?}"
    );
    for w in snapshots.windows(2) {
        assert!(
            w[1] > w[0],
            "each growing poll must see MORE than before: {snapshots:?}"
        );
    }

    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
}

#[test]
fn follow_drains_and_exits_cleanly_on_stop() {
    // AC-C: stop the instance mid-follow; assert the follower's final poll
    // includes every line emitted up to the stop's `engine`-attributed
    // line, with no further growth after.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &["--heartbeat-ms", "20", "--linger-ms", "600000"],
        "guaranteed",
    );
    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    facade.start("svc").unwrap();
    wait_for_min_lines_per_stream(&facade, "svc", 3, 0);

    let saw_stop_line = AtomicBool::new(false);
    let final_cursor: Mutex<u64> = Mutex::new(0);

    std::thread::scope(|scope| {
        let follower = scope.spawn(|| {
            let mut cursor = 0u64;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let (new_lines, next_cursor) = facade
                    .read_agent_log_since("svc", cursor)
                    .expect("read_agent_log_since");
                cursor = next_cursor;
                if new_lines
                    .iter()
                    .any(|l| l.stream == LogStream::Engine && l.text.contains("-> stopped"))
                {
                    saw_stop_line.store(true, Ordering::Relaxed);
                }
                let status = facade.instance_status("svc").expect("instance_status");
                if status.instance.state != LifecycleState::Running {
                    // One final drain (AC-C): no line up to the transition
                    // may be lost. The engine-attributed "stopped" line is
                    // enqueued synchronously inside `stop_inner` (before the
                    // DB write `instance_status` just observed even
                    // returns), but the WRITER thread's OWN dequeue-then-
                    // fsync is a separate, independently-scheduled step —
                    // under heavy parallel-test CPU contention this can lag
                    // by a few milliseconds. Retry the drain briefly
                    // (committed-state polling, never a fixed sleep) rather
                    // than a single one-shot read, so a slow scheduler
                    // moment cannot turn into a false test failure; the
                    // PRODUCT's own `kt agent logs --follow` still performs
                    // exactly the one drain AC-C specifies — this loop is a
                    // test-robustness allowance only.
                    let final_deadline = Instant::now() + Duration::from_secs(3);
                    loop {
                        let (more, final_c) = facade
                            .read_agent_log_since("svc", cursor)
                            .expect("final drain");
                        cursor = final_c;
                        if more
                            .iter()
                            .any(|l| l.stream == LogStream::Engine && l.text.contains("-> stopped"))
                        {
                            saw_stop_line.store(true, Ordering::Relaxed);
                        }
                        if saw_stop_line.load(Ordering::Relaxed) || Instant::now() >= final_deadline
                        {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    *final_cursor.lock().unwrap() = cursor;
                    return;
                }
                assert!(Instant::now() < deadline, "follower must not hang");
                std::thread::sleep(Duration::from_millis(15));
            }
        });

        // Give the follower a head start so it is genuinely mid-poll, then stop.
        std::thread::sleep(Duration::from_millis(60));
        facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
        follower.join().expect("follower thread must not panic");
    });

    assert!(
        saw_stop_line.load(Ordering::Relaxed),
        "the follower must observe the stop's engine-attributed line"
    );

    // No further growth after the follower's own final drain — the
    // instance is stopped, so nothing new can ever arrive.
    let cursor = *final_cursor.lock().unwrap();
    let (more, _c) = facade.read_agent_log_since("svc", cursor).unwrap();
    assert!(
        more.is_empty(),
        "no growth may follow the final drain: {more:?}"
    );
}

#[test]
fn follow_on_a_paused_instance_notes_paused_not_hang() {
    // AC-C, the pause edge case: pause instead of stop; assert the follower
    // returns a clean "paused" signal rather than blocking forever waiting
    // for output that will not arrive while suspended.
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &["--heartbeat-ms", "20", "--linger-ms", "600000"],
        "guaranteed",
    );
    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    facade.start("svc").unwrap();
    wait_for_min_lines_per_stream(&facade, "svc", 2, 0);

    let observed_state: Mutex<Option<LifecycleState>> = Mutex::new(None);

    std::thread::scope(|scope| {
        let follower = scope.spawn(|| {
            let mut cursor = 0u64;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let (_new_lines, next_cursor) = facade
                    .read_agent_log_since("svc", cursor)
                    .expect("read_agent_log_since");
                cursor = next_cursor;
                let status = facade.instance_status("svc").expect("instance_status");
                if status.instance.state != LifecycleState::Running {
                    *observed_state.lock().unwrap() = Some(status.instance.state);
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "follow must exit honestly on pause, never hang"
                );
                std::thread::sleep(Duration::from_millis(15));
            }
        });

        std::thread::sleep(Duration::from_millis(60));
        facade.pause("svc").unwrap();
        follower.join().expect("follower thread must not panic");
    });

    assert_eq!(
        *observed_state.lock().unwrap(),
        Some(LifecycleState::Paused),
        "follow must cleanly observe the paused state, not hang or misreport it"
    );

    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
}

#[test]
fn output_capture_is_unconditional_regardless_of_declared_interaction_support() {
    // AC-E: a manifest declaring `interaction: unsupported` (on the current
    // OS); assert its stdout is still captured, attributed, and readable —
    // contrast directly with 4.1's
    // `send_input_on_unsupported_interaction_fails_fast_with_no_io`: reading
    // FROM the process is never capability-gated; only writing TO it
    // (`send`) is.
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
args = ["--heartbeat-ms", "30", "--linger-ms", "600000"]

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

    let caps = facade.effective_capabilities("unsup").unwrap();
    let level = caps
        .entries
        .iter()
        .find(|(c, _)| *c == Capability::Interaction)
        .map(|(_, l)| *l);
    assert_eq!(
        level,
        Some(SupportLevel::Unsupported),
        "sanity: this test must honestly exercise the unsupported path"
    );

    facade.start("unsup").unwrap();
    let lines = wait_for_min_lines_per_stream(&facade, "unsup", 2, 0);
    assert!(
        lines
            .iter()
            .any(|l| l.stream == LogStream::AgentOut && l.text.starts_with("heartbeat ")),
        "output must be captured and attributed regardless of declared interaction support: {lines:?}"
    );

    facade.stop("unsup", Some(Duration::from_secs(5))).unwrap();
}

// ---- CRITICAL SCOPING #3: the non-negotiable agent.log byte-identity guard ----

/// The token sentinels `fake_agent --emit-usage` stamps on every event
/// (mirrors `metering.rs`'s fixture — the SAME convention, reused verbatim).
const USAGE_INPUT: u64 = 10;
const USAGE_OUTPUT: u64 = 20;

/// Poll the committed Usage-Ledger event count for `name` until it reaches
/// `expected` (mirrors `metering.rs`'s `wait_for_usage_rows` exactly — a
/// direct read-only connection to the same state DB the engine commits to,
/// deterministic committed state, never a wall-clock guess).
fn wait_for_usage_rows(state_dir: &Path, name: &str, expected: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let count = usage_row_count(state_dir, name);
        if count >= expected {
            return count;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} committed usage rows for '{name}' (have {count})"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn usage_row_count(state_dir: &Path, name: &str) -> u64 {
    let conn = rusqlite::Connection::open(state_dir.join("state.db")).expect("open state db");
    conn.query_row(
        "SELECT COUNT(*) FROM usage_events e \
         JOIN agent_instances i ON i.id = e.instance_id WHERE i.name = ?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u64)
    .unwrap_or(0)
}

#[test]
fn legacy_agent_log_is_byte_identical_to_pre_story_content() {
    // CRITICAL SCOPING #3 — the automated regression guard this story is
    // built around: `Supervisor::drain_usage_for` (story 3-1/AD-7,
    // adversarially-reviewed billing-critical metering ingestion) reads
    // `agent.log` TODAY, splitting on '\n' and matching a
    // "KTESIO_USAGE {json}" sentinel AT THE START of each RAW physical
    // line. Prove BOTH halves hold even though this story reworks the
    // CAPTURE PATH (piped + engine-side reader threads, not a kernel
    // passthrough): (a) `agent.log`'s bytes are UNCHANGED — the exact raw
    // lines `fake_agent` wrote, byte for byte, no JSON envelope, no
    // attribution prefix, no timestamp, even though the SAME reader threads
    // ALSO feed the new attributed capture; and (b) `drain_usage_for`'s
    // ledger ingestion still works completely unmodified end to end
    // (mirrors `metering.rs`'s fixture pattern exactly, reused verbatim —
    // this story provably does not touch Epic 3's billing path).
    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &[
            "--heartbeat-ms",
            "40",
            "--emit-usage",
            "3",
            "--linger-ms",
            "600000",
        ],
        "guaranteed",
    );

    let engine = open(&state);
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest.path().to_path_buf()))
        .unwrap();
    let started = facade.start("svc").unwrap();
    assert_eq!(started.state, LifecycleState::Running);

    // (b) The ledger ingestion is unaffected: exactly 3 committed rows, the
    // Fleet-detail totals equal the ledger exactly — the SAME assertion
    // shape story 3-1's own test makes.
    let count = wait_for_usage_rows(state.path(), "svc", 3);
    assert_eq!(count, 3, "exactly 3 usage rows, unaffected by this story");
    let fleet = facade.fleet().unwrap();
    let entry = fleet.iter().find(|e| e.name.as_str() == "svc").unwrap();
    assert_eq!(entry.usage.cumulative_input_tokens, 3 * USAGE_INPUT);
    assert_eq!(entry.usage.cumulative_output_tokens, 3 * USAGE_OUTPUT);
    assert_eq!(entry.metering_source, "self-reported");

    // (a) agent.log byte-identity: the exact raw lines, unmodified — no
    // JSON envelope, no attribution token, no timestamp prefix anywhere.
    let agent_log = agent_log_path(state.path(), "svc");
    let contents = std::fs::read_to_string(&agent_log).unwrap();
    assert!(
        contents
            .lines()
            .any(|l| l.starts_with("fake_agent ready pid=")),
        "the raw ready line must appear verbatim: {contents:?}"
    );
    assert!(
        contents.lines().any(|l| l == "heartbeat 0"),
        "a heartbeat line must appear verbatim, with no wrapping: {contents:?}"
    );
    assert!(
        !contents.contains("schema_version"),
        "agent.log must carry NO JSON envelope: {contents:?}"
    );
    assert!(
        !contents.contains("\"stream\""),
        "agent.log must carry NO attribution field: {contents:?}"
    );
    assert!(
        !contents.contains("agent-out") && !contents.contains("agent-err"),
        "agent.log must carry NO attribution token: {contents:?}"
    );
    // The exact KTESIO_USAGE sentinel lines fake_agent emits, verbatim — the
    // literal string drain_usage_for's parser matches at line-start.
    for seq in 0..3u64 {
        let want =
            format!("KTESIO_USAGE {{\"sequence\":{seq},\"input_tokens\":{USAGE_INPUT},\"output_tokens\":{USAGE_OUTPUT}}}");
        assert!(
            contents.lines().any(|l| l == want),
            "sentinel line {seq} must appear BYTE-IDENTICAL (not wrapped/reformatted): {contents:?}"
        );
    }

    // The NEW attributed capture exists SEPARATELY and carries the SAME
    // sentinel text, but wrapped as expected — proving the two files are
    // genuinely independent, not aliases of one another.
    let output_log = state
        .path()
        .join("agents")
        .join("svc")
        .join("logs")
        .join("output.log");
    let attributed = std::fs::read_to_string(&output_log).unwrap();
    assert!(attributed.contains("\"stream\":\"agent-out\""));
    assert!(attributed.contains("KTESIO_USAGE"));

    facade.stop("svc", Some(Duration::from_secs(5))).unwrap();
}

// ---- AC-H: the adopted-instance edge case (mirrors interaction.rs's harness, OPPOSITE outcome) ----
//
// Faithfully simulating an ENGINE CRASH so the agent OUTLIVES it (a `kill -9`
// of the engine runs no destructors, so the kill-on-drop handle never fires):
// engine 1's work runs in a SEPARATE child process (a re-exec of THIS test
// binary via the `logs_adoption_helper_subprocess` entry) that starts the
// agent then `std::process::exit`s WITHOUT dropping the engine. The parent
// test then opens a NEW engine over the SAME state dir, which adopts the
// still-live process — and, UNLIKE 4.1's `send_input` (AC-D, a hard
// failure), reading/following it WORKS.

/// Linux AND running under CI (GitHub sets `CI`). Mirrors `adoption.rs`'s /
/// `interaction.rs`'s skip for the heavy re-exec + surviving-orphan harness
/// (#109: an x86 ubuntu-CI-only D-state deadlock, unrelated to this story's
/// logic).
fn is_linux_ci() -> bool {
    OsId::current() == OsId::Linux && std::env::var_os("CI").is_some()
}

/// Whether a pid is still alive. NO OS-cfg here (the gate allowlists only
/// `backends/`); branch on the runtime OS id and shell out, exactly like
/// `adoption.rs`'s/`interaction.rs`'s identical helper.
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
    let deadline = Instant::now() + Duration::from_secs(8);
    while pid_alive(pid) {
        assert!(Instant::now() < deadline, "{what} (pid {pid} still alive)");
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Read the pid `fake_agent` announced (`ready pid=<n>`) from its agent.log.
fn wait_for_agent_pid(agent_log: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
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

/// Run "engine 1" in a SEPARATE child process (mirrors `interaction.rs`'s
/// `run_engine1`): register + start `svc`, then exit WITHOUT dropping the
/// engine (crash semantics). Blocks until the child exits.
fn run_engine1(state: &Path, manifest: &Path) {
    let exe = std::env::current_exe().expect("test exe");
    let status = Command::new(exe)
        .args(["--exact", "logs_adoption_helper_subprocess", "--nocapture"])
        .env("KTESIO_LOGS_ADOPTION_HELPER", "1")
        .env("KTESIO_LOGS_ADOPTION_STATE", state)
        .env("KTESIO_LOGS_ADOPTION_MANIFEST", manifest)
        .status()
        .expect("run engine-1 helper subprocess");
    assert!(
        status.success(),
        "engine-1 helper subprocess failed: {status}"
    );
}

/// The re-exec entry for the "engine 1" work (see the section docs above).
/// When `KTESIO_LOGS_ADOPTION_HELPER` is unset this is a trivial pass (it
/// runs as a normal test in the parent binary too, and does nothing).
#[test]
fn logs_adoption_helper_subprocess() {
    let Ok(_mode) = std::env::var("KTESIO_LOGS_ADOPTION_HELPER") else {
        return; // normal in-process invocation: nothing to do.
    };
    let state = PathBuf::from(std::env::var("KTESIO_LOGS_ADOPTION_STATE").unwrap());
    let manifest = PathBuf::from(std::env::var("KTESIO_LOGS_ADOPTION_MANIFEST").unwrap());

    let engine = Engine::open(Some(state)).expect("engine1 open");
    let facade = engine.blocking();
    facade
        .register_with_adapter("svc", &AdapterRef::Manifest(manifest))
        .unwrap();
    facade.start("svc").unwrap();
    // Let a real, substantial pre-crash history accrue (several heartbeats
    // on stdout — this manifest's args only enable --heartbeat-ms, not the
    // stderr counterpart) BEFORE "crashing" — this engine session's
    // capture threads are what write it, so the crash-adoption test below
    // needs GENUINE content already on disk to prove engine 2 can read it.
    let _ = wait_for_min_lines_per_stream(&facade, "svc", 5, 0);
    // Crash: exit WITHOUT dropping the engine, so the kill-on-drop handle
    // never fires and the agent (its own session leader) survives,
    // re-parented to init. This process's OWN reader/writer threads die
    // WITH it (a process exit ends every thread in it, regardless of
    // Drop) — so nothing further will EVER be appended to either capture
    // file after this point; see the main test's doc comment.
    std::process::exit(0);
}

#[test]
fn adopted_instance_can_be_followed_from_a_fresh_engine_session() {
    // AC-H — the genuinely novel edge case this story surfaces, and the
    // mirror image of 4.1's AC-D test: THERE, `send_input` on an adopted
    // instance FAILS (`InteractionUnavailable` — no recoverable stdin
    // pipe); HERE, by contrast, `read_agent_log`/`read_agent_log_since` on
    // an adopted instance SUCCEEDS — reading only ever needs the
    // deterministically-computed FILE path, not a live handle, so a
    // SECOND engine session (which holds NO capture threads of its own for
    // this instance — Task 3's `adopt()` sets `log_sender: None`) can still
    // read everything ENGINE 1 captured before it exited.
    //
    // HONEST SCOPE (verified empirically, not merely assumed): a
    // `std::process::exit` (this harness's crash simulation, matching the
    // REAL crash case — a `SIGKILL` on the engine likewise reclaims every
    // fd the OS holds for it) terminates EVERY thread in that process,
    // including its output-capture reader/writer threads — there is no
    // "the old threads keep running in the background" outcome; the
    // process is simply gone. `fake_agent` itself SURVIVES (re-parented to
    // init, confirmed via `pid_alive` below) because it is a SEPARATE
    // process in its own group, but its stdout/stderr pipes now have NO
    // reader on the other end, so nothing more will EVER reach either
    // capture file — this is precisely the Dev Notes' own qualifier
    // ("...capture threads running, in WHICHEVER engine session
    // originally spawned it and has NOT YET EXITED") made concrete. So
    // this test proves AC-H's actual, achievable claim: reading an
    // ADOPTED instance's ALREADY-CAPTURED history NEVER ERRORS and NEEDS
    // NO live handle/daemon/subscription — contrasted explicitly with
    // `send_input`'s hard failure on the identical instance (4.1 AC-D) —
    // not that content magically keeps growing after the sole writer is
    // gone (a claim this test deliberately does NOT make, unlike an
    // earlier draft that incorrectly assumed it and was caught by this
    // exact test failing against the real system).
    //
    // Runtime-skip on Windows: this needs the child to genuinely SURVIVE
    // the engine-1 subprocess's exit (Unix re-parenting to init); on
    // Windows `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` kills it when the helper
    // exits, so cross-lifetime survival cannot be simulated there — the
    // SAME reason `adoption.rs`'s/`interaction.rs`'s survivor tests skip
    // Windows. NO `#[cfg]` (data-driven; this file is outside the backends
    // allowlist).
    if OsId::current() == OsId::Windows {
        return;
    }
    // Temporary CI mitigation (#109), mirroring `adoption.rs`/`interaction.rs`:
    // this harness (heavy re-exec + a surviving orphan process) deadlocks
    // uninterruptibly on the x86-64 ubuntu GitHub runner ONLY. Skip it
    // there; #109 tracks the root cause + un-skip.
    if is_linux_ci() {
        return;
    }

    let state = TempDir::new().unwrap();
    let manifest = TempDir::new().unwrap();
    write_logs_manifest(
        manifest.path(),
        "svc",
        &["--heartbeat-ms", "30", "--linger-ms", "600000"],
        "guaranteed",
    );

    // Engine 1 (a subprocess): start `svc`, let a real pre-crash history
    // accrue, then crash (exit without drop).
    run_engine1(state.path(), manifest.path());

    // The AGENT process survives the "crash" (re-parented to init) — but,
    // per the doc comment above, its capture threads (which lived in
    // engine 1's now-gone process) do not.
    let agent_log = agent_log_path(state.path(), "svc");
    let pid = wait_for_agent_pid(&agent_log);
    assert!(pid_alive(pid), "svc must survive the engine crash");

    // Engine 2: open over the SAME state dir -> adopt_orphans re-acquires
    // the still-live process into a handle with NO capture threads of its
    // own (Task 3) — this is the harness proof that AC-H does not secretly
    // depend on engine 2 having spawned anything itself.
    let engine2 = Engine::open(Some(state.path().to_path_buf())).unwrap();
    let facade2 = engine2.blocking();
    let status = facade2.instance_status("svc").unwrap();
    assert_eq!(
        status.instance.state,
        LifecycleState::Running,
        "a live orphan must be adopted as running"
    );

    // AC-H's core claim: read_agent_log (one-shot) SUCCEEDS from a session
    // that never held a live handle for this instance, returning the FULL
    // pre-crash history engine 1 captured — purely from the
    // deterministically-computed path, no daemon/subscription/live handle
    // needed. Contrast directly with 4.1's
    // `send_input_on_an_adopted_instance_is_interaction_unavailable`: THAT
    // call on this SAME kind of instance returns a hard
    // `InteractionUnavailable` error; THIS one returns real data.
    let lines = facade2.read_agent_log("svc").unwrap();
    assert!(
        !lines.is_empty(),
        "the adopted instance's already-captured output must be readable from a fresh session"
    );
    assert!(lines
        .iter()
        .any(|l| l.stream == LogStream::AgentOut && l.text.starts_with("heartbeat ")));
    assert!(
        lines
            .iter()
            .any(|l| l.stream == LogStream::Engine && l.text.contains("-> running")),
        "the start transition's engine line, recorded by engine 1, must also be readable: {lines:?}"
    );

    // read_agent_log_since (the follow primitive) ALSO succeeds — a cursor
    // walk across the retained history returns it in append order, with NO
    // error, exactly like the one-shot read.
    let (since_start, cursor_after_all) = facade2.read_agent_log_since("svc", 0).unwrap();
    assert_eq!(
        since_start.len(),
        lines.len(),
        "read_agent_log_since(0) must agree with read_agent_log's one-shot count \
         (both read the same current-generation content here — no rotation occurred)"
    );
    // A further poll from the end of the retained content honestly reports
    // nothing new (never errors, never fabricates growth) — the accurate,
    // testable expression of "no writer remains" for a session that never
    // itself held a handle.
    let (further, cursor_unchanged) = facade2
        .read_agent_log_since("svc", cursor_after_all)
        .unwrap();
    assert!(further.is_empty());
    assert_eq!(cursor_unchanged, cursor_after_all);

    // Teardown: stop via engine 2 (it holds the re-acquired handle) so no
    // orphan remains.
    facade2.stop("svc", Some(Duration::from_secs(5))).unwrap();
    wait_until_gone(pid, "stop must terminate the adopted process");
}
