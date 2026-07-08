//! `fake_agent` — a tiny, cross-platform, spawnable test agent (story 1.4, AD-3).
//!
//! A DEV/TEST artifact: the engine's start/stop integration tests point a
//! manifest adapter's `[lifecycle.start]` `exec` at this binary so the supervisor
//! spawns a REAL process to launch, supervise, and kill — proving supervision end
//! to end (1-3's `ScriptedFakeAgent` was inert). It is pure `std` with NO
//! OS-conditional code (the OS-cfg CI gate applies to `ktesio-conformance` too),
//! so it builds and runs identically on Linux, macOS, and Windows.
//!
//! ## Behaviors (selected by args)
//!
//! * (default) print a ready line to stdout, then loop-sleep "forever" (until the
//!   supervisor kills it) — proves `start → running` and `stop → killed`.
//! * `--exit-fast <code>`  exit immediately with `<code>` — proves AC2
//!   launch-failure / immediate non-zero exit during startup.
//! * `--spawn-child`  before looping, spawn ANOTHER copy of this binary (also
//!   looping) as a child — proves AC3 "no process of the instance survives"
//!   catches the WHOLE process group / Job, not just the parent PID. The child's
//!   pid is printed as `child-pid=<n>` so a test can assert the child is gone too.
//! * `--linger-ms <ms>`  after receiving no signal, keep running for at least
//!   `<ms>` before self-exiting. Tests set the stop window SHORTER than this so
//!   the graceful window elapses and the supervisor's forced kill is exercised —
//!   the cfg-free way to force escalation (a SIGTERM-ignoring handler would need
//!   OS-cfg, which the gate forbids here). With no `--linger-ms` the process
//!   loops effectively forever (a very long sleep), so a normal graceful stop
//!   still ends it promptly via the kill.
//! * `--heartbeat-ms <ms>`  print an incrementing `heartbeat <n>` line to stdout
//!   every `<ms>` and flush (story 1-5). When the process is SIGSTOP'd the whole
//!   process freezes, so its captured log STOPS growing; SIGCONT resumes it — the
//!   OBSERVABLE suspension proof for the guaranteed pause path. With no
//!   `--heartbeat-ms` the loop is a quiet sleep (existing 1-4 tests that only
//!   assert `ready`/lifecycle are unaffected). Pure `std`, NO OS-cfg.
//! * `--crash-after-ms <ms>` (+ optional `--crash-with <code>`)  run normally
//!   (announcing readiness, heartbeating if asked) for `<ms>`, THEN exit with
//!   `<code>` (default 1) — simulating an UNREQUESTED crash AFTER the readiness
//!   window (story 1-6). Distinct from `--exit-fast`, which exits DURING startup
//!   (a launch failure); `--crash-after-ms` stays alive long enough to reach
//!   `running`, so the supervisor's reaper detects the later `Exited` as a crash
//!   and the Restart Policy fires. The `--heartbeat-ms` line count proves a
//!   RESTARTED instance is alive again. Pure `std`, NO OS-cfg.
//!
//! The binary writes a small marker file (`--marker <path>`) on startup if asked,
//! so a test can confirm it actually ran without racing on stdout capture.
//!
//! * `--dump <path>` (story 2-2)  write a small observation file at startup: the
//!   full received argv (one `arg=<token>` line each) followed by every
//!   environment variable (one `env=<KEY>=<VALUE>` line each). The engine's
//!   start-seam config mapping proof reads this back to confirm a mapped unified
//!   key landed in the agent's native mechanism — a FLAG in the args, or an ENV
//!   var in the environment — WITHOUT racing on stdout capture. Pure `std`, NO
//!   OS-cfg. (A FILE-target mapping is observed directly as a rendered file in the
//!   Agent Home, so it needs no dump.)
//! * `--emit-usage <N>` (story 3-1)  after announcing readiness, emit `<N>`
//!   self-reported usage sentinel lines — `KTESIO_USAGE {json}` — on stdout, with
//!   monotonic `sequence` 0..N and FIXED token sentinels (`input_tokens = 10`,
//!   `output_tokens = 20` per event), spaced across the loop so the engine's reaper
//!   ingests them into the Usage Ledger. This is the `self-reported` half of FR-19:
//!   the engine parses these captured lines into UsageEvents. Determinism: the test
//!   waits for the KNOWN number of committed rows (the DB is the source of truth),
//!   NOT a wall-clock sleep. Pure `std`, NO OS-cfg (the parser + this emitter share
//!   the documented convention; the OS-cfg gate covers this crate).
//! * `--replay-usage` (story 3-1)  after the `--emit-usage` batch, re-emit
//!   `sequence 0` ONCE (a DELAYED/replayed batch). The engine's ledger dedup must
//!   recognize it and NOT double-count — the AC-A no-double-count proof.
//! * `--usage-input-tokens <N>` / `--usage-output-tokens <N>` (story 3-1)  override
//!   the FIXED per-event token sentinels so a test can emit an arbitrary value —
//!   notably `u64::MAX` — to prove the storage boundary SATURATE-CLAMPS the `u64`
//!   into SQLite's signed `i64` column (a positive `i64::MAX`) rather than a raw
//!   `as i64` that bit-wraps NEGATIVE and poisons the billing SUM (the C1/C2
//!   boundary). Default to the fixed sentinels when absent.
//! * `--final-usage-no-newline` (story 3-1)  after the batch (+ any replay), emit
//!   ONE more usage line WITHOUT a trailing newline (`sequence = emit_usage`), then
//!   exit immediately. The process dies with a half-line in the log, so ONLY the
//!   engine's TERMINAL drain-on-reap can rescue it — the H1 under-count proof (a
//!   mid-run drain, which stops at the last newline, would strand it forever).

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

// This whole binary runs only as a SPAWNED SUBPROCESS of the supervision tests,
// so a coverage harness (tarpaulin) that instruments the PARENT process can never
// record its lines — they show as 0% and would drag the workspace number down
// dishonestly. Exclude the bin from coverage (the same `#[cfg(not(tarpaulin_include))]`
// convention `kt`'s `main`/`run_cli` use). Its BEHAVIOR is proven by the
// supervision tests that spawn and kill it, not by line coverage.

/// Parsed invocation options.
struct Opts {
    exit_fast: Option<i32>,
    spawn_child: bool,
    linger: Duration,
    marker: Option<PathBuf>,
    /// Dump the received argv + environment to this file at startup (story 2-2:
    /// the config-mapping observation point). `None` = no dump.
    dump: Option<PathBuf>,
    /// Heartbeat interval (story 1-5). `None` = no heartbeat (quiet sleep loop).
    heartbeat: Option<Duration>,
    /// Crash AFTER this interval (story 1-6): run normally, then exit non-zero.
    /// `None` = no self-crash (the linger loop governs exit).
    crash_after: Option<Duration>,
    /// The exit code the `--crash-after-ms` self-crash uses (default 1).
    crash_with: i32,
    /// Emit this many self-reported usage sentinel lines (story 3-1). `0` = none.
    emit_usage: u64,
    /// After the usage batch, re-emit `sequence 0` once — a replayed batch for the
    /// no-double-count proof (story 3-1). Ignored unless `emit_usage > 0`.
    replay_usage: bool,
    /// Override the per-event input-token sentinel (story 3-1 C1/C2 boundary test).
    /// `None` = the fixed [`USAGE_INPUT_TOKENS`]. Lets a test emit a huge value (e.g.
    /// `u64::MAX`) to prove the storage boundary saturate-clamps rather than wraps.
    usage_input_tokens: Option<u64>,
    /// Override the per-event output-token sentinel (see `usage_input_tokens`).
    usage_output_tokens: Option<u64>,
    /// Emit ONE final usage sentinel line WITHOUT a trailing newline, then exit
    /// promptly (story 3-1 H1 under-count test). Its `sequence` is `emit_usage` (one
    /// past the batch), so the TERMINAL drain-on-reap must consume the newline-less
    /// tail or that event is stranded and lost.
    final_usage_no_newline: bool,
}

/// The FIXED token sentinels every emitted usage event carries (story 3-1), so a
/// test asserting the ledger total is an exact-match (`N * INPUT` etc.), never a
/// fuzzy range. Kept small + memorable.
const USAGE_INPUT_TOKENS: u64 = 10;
const USAGE_OUTPUT_TOKENS: u64 = 20;

/// Format ONE `KTESIO_USAGE {json}` self-reported usage sentinel line (story 3-1).
///
/// MUST match the engine's `ktesio_engine::ports::parse_usage_line` convention:
/// the prefix `KTESIO_USAGE ` + a JSON object with snake_case `sequence`,
/// `input_tokens`, `output_tokens`. This binary cannot depend on the engine, so it
/// re-implements the shared shape in pure `std`; the engine's
/// `format_and_parse_round_trip_agree_on_the_convention` test guards the two
/// against drift. NO OS-cfg — the line is identical text on every OS.
fn usage_line(sequence: u64, input_tokens: u64, output_tokens: u64) -> String {
    format!(
        "KTESIO_USAGE {{\"sequence\":{sequence},\"input_tokens\":{input_tokens},\"output_tokens\":{output_tokens}}}"
    )
}

#[cfg(not(tarpaulin_include))]
fn parse() -> Opts {
    let mut exit_fast = None;
    let mut spawn_child = false;
    // Default "loop forever": a very long lingering time. A graceful stop kills
    // it via signal/job long before this elapses; this is only the self-exit
    // fallback so a stray test never leaks a truly immortal process.
    let mut linger = Duration::from_secs(3600);
    let mut marker = None;
    let mut dump = None;
    let mut heartbeat = None;
    let mut crash_after = None;
    let mut crash_with = 1;
    let mut emit_usage = 0;
    let mut replay_usage = false;
    let mut usage_input_tokens = None;
    let mut usage_output_tokens = None;
    let mut final_usage_no_newline = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-fast" => {
                let code = args.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(1);
                exit_fast = Some(code);
            }
            "--emit-usage" => {
                if let Some(n) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    emit_usage = n;
                }
            }
            "--replay-usage" => replay_usage = true,
            "--usage-input-tokens" => {
                if let Some(n) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    usage_input_tokens = Some(n);
                }
            }
            "--usage-output-tokens" => {
                if let Some(n) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    usage_output_tokens = Some(n);
                }
            }
            "--final-usage-no-newline" => final_usage_no_newline = true,
            "--spawn-child" => spawn_child = true,
            "--linger-ms" => {
                if let Some(ms) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    linger = Duration::from_millis(ms);
                }
            }
            "--heartbeat-ms" => {
                if let Some(ms) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    heartbeat = Some(Duration::from_millis(ms));
                }
            }
            "--crash-after-ms" => {
                if let Some(ms) = args.next().and_then(|s| s.parse::<u64>().ok()) {
                    crash_after = Some(Duration::from_millis(ms));
                }
            }
            "--crash-with" => {
                if let Some(code) = args.next().and_then(|s| s.parse::<i32>().ok()) {
                    crash_with = code;
                }
            }
            "--marker" => {
                if let Some(path) = args.next() {
                    marker = Some(PathBuf::from(path));
                }
            }
            "--dump" => {
                if let Some(path) = args.next() {
                    dump = Some(PathBuf::from(path));
                }
            }
            // Unknown args are ignored so a manifest can pass extra tokens.
            _ => {}
        }
    }

    Opts {
        exit_fast,
        spawn_child,
        linger,
        marker,
        dump,
        heartbeat,
        crash_after,
        crash_with,
        emit_usage,
        replay_usage,
        usage_input_tokens,
        usage_output_tokens,
        final_usage_no_newline,
    }
}

#[cfg(not(tarpaulin_include))]
fn main() {
    let opts = parse();

    // Immediate-exit path (AC2): exit before doing anything else.
    if let Some(code) = opts.exit_fast {
        // Still drop a marker if asked, so a test can prove it launched at all.
        write_marker(&opts.marker, "exit-fast");
        std::process::exit(code);
    }

    // Optionally spawn a looping child in the same process group / Job (it
    // inherits both), to prove the no-survivor kill catches the whole tree.
    //
    // The child is DELIBERATELY never `wait()`ed on: it must keep running
    // independently until the supervisor's process-group (Unix) / Job Object
    // (Windows) kill reaps it, which is exactly what the no-survivor test
    // asserts. Waiting here would defeat the test. Hence the allow.
    #[allow(clippy::zombie_processes)]
    let child_pid = if opts.spawn_child {
        let exe = std::env::current_exe().expect("current exe");
        // The child lingers a long time and does NOT spawn its own child.
        let child = Command::new(exe)
            .arg("--linger-ms")
            .arg("3600000")
            .spawn()
            .expect("spawn child fake_agent");
        Some(child.id())
    } else {
        None
    };

    // Announce readiness. The pid lines let a test learn the parent + child pids.
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "fake_agent ready pid={}", std::process::id());
    if let Some(pid) = child_pid {
        let _ = writeln!(stdout, "child-pid={pid}");
    }
    let _ = stdout.flush();
    write_marker(&opts.marker, "ready");
    // Story 2-2: dump the received argv + environment so the config-mapping proof
    // can observe a mapped unified key that landed as a native FLAG (in the args)
    // or ENV var (in the environment), without racing on stdout capture.
    write_dump(&opts.dump);

    // Story 3-1: emit self-reported usage sentinel lines (readiness-gated — AFTER
    // the ready line, so the instance has reached `running` and the engine's reaper
    // is ingesting). Monotonic `sequence` 0..N with fixed token sentinels; a small
    // pause between lines lets the ~250ms reaper cadence drain them. If asked, then
    // re-emit `sequence 0` once — a DELAYED/replayed batch the ledger must dedup
    // (AC-A no-double-count). The test waits for the KNOWN committed row count in the
    // DB, so this schedule is a nudge, not a timing dependency. Pure `std`, NO OS-cfg.
    let input_tokens = opts.usage_input_tokens.unwrap_or(USAGE_INPUT_TOKENS);
    let output_tokens = opts.usage_output_tokens.unwrap_or(USAGE_OUTPUT_TOKENS);
    for sequence in 0..opts.emit_usage {
        let _ = writeln!(
            stdout,
            "{}",
            usage_line(sequence, input_tokens, output_tokens)
        );
        let _ = stdout.flush();
        sleep(Duration::from_millis(20));
    }
    if opts.emit_usage > 0 && opts.replay_usage {
        let _ = writeln!(stdout, "{}", usage_line(0, input_tokens, output_tokens));
        let _ = stdout.flush();
    }
    // Story 3-1 (H1): a Run's FINAL usage line, flushed WITHOUT a trailing newline,
    // must not be stranded when the process exits. First stay alive PAST the engine's
    // ~300ms startup readiness window (so `start` confirms `running` — an immediate
    // exit would instead be read as a launch failure and never reach the reaper).
    // Then emit ONE usage line WITHOUT a newline (`sequence = emit_usage`, one past
    // the batch, so the test counts it distinctly) and exit. Because the process then
    // dies with a half-line in the log, ONLY the engine's TERMINAL drain-on-reap can
    // rescue it — a MidRun drain (which stops at the last newline) would strand it.
    // `write!` (not `writeln!`) leaves NO trailing newline; flush so the bytes reach
    // the captured log before exit.
    if opts.final_usage_no_newline {
        sleep(Duration::from_millis(500));
        let _ = write!(
            stdout,
            "{}",
            usage_line(opts.emit_usage, input_tokens, output_tokens)
        );
        let _ = stdout.flush();
        std::process::exit(0);
    }

    // Loop until we are killed, until the crash-after window elapses (story 1-6:
    // a simulated UNREQUESTED crash → non-zero exit), or until the linger window
    // elapses (the clean self-exit fallback). When a heartbeat interval is set,
    // print an incrementing `heartbeat <n>` line every interval and flush — while
    // SIGSTOP'd the whole process freezes, so the captured log stops growing (the
    // story-1-5 observable-suspension proof); SIGCONT resumes it. With no
    // heartbeat this is a quiet short-poll sleep (unchanged 1-4 behavior). A
    // short poll keeps the process responsive to signals in all modes.
    let start = Instant::now();
    let deadline = start + opts.linger;
    let crash_deadline = opts.crash_after.map(|d| start + d);
    let mut beats: u64 = 0;
    let mut next_beat = opts.heartbeat.map(|interval| Instant::now() + interval);
    while Instant::now() < deadline {
        // Story 1-6: after the crash-after window, exit non-zero (a crash the
        // supervisor's reaper detects, firing the Restart Policy). Checked after
        // readiness was announced above, so the instance reaches `running` first.
        if let Some(due) = crash_deadline {
            if Instant::now() >= due {
                let _ = writeln!(stdout, "crashing with code {}", opts.crash_with);
                let _ = stdout.flush();
                std::process::exit(opts.crash_with);
            }
        }
        if let (Some(interval), Some(due)) = (opts.heartbeat, next_beat) {
            if Instant::now() >= due {
                let _ = writeln!(stdout, "heartbeat {beats}");
                let _ = stdout.flush();
                beats += 1;
                next_beat = Some(due + interval);
            }
        }
        sleep(Duration::from_millis(25));
    }
    // Lingered the whole window without being killed: exit cleanly.
    std::process::exit(0);
}

/// Best-effort write of a one-line marker file so tests can confirm startup
/// without racing on stdout capture.
#[cfg(not(tarpaulin_include))]
fn write_marker(path: &Option<PathBuf>, phase: &str) {
    if let Some(path) = path {
        let _ = std::fs::write(
            path,
            format!("fake_agent {phase} pid={}\n", std::process::id()),
        );
    }
}

/// Best-effort write of the received argv + environment to the `--dump` file
/// (story 2-2). One `arg=<token>` line per argv entry (including argv[0]), then
/// one `env=<KEY>=<VALUE>` line per environment variable. The config-mapping proof
/// greps this for a mapped FLAG (an `arg=--model` / `arg=<value>` pair) or ENV var
/// (an `env=MODEL=<value>` line). Best-effort so it never fails the process.
#[cfg(not(tarpaulin_include))]
fn write_dump(path: &Option<PathBuf>) {
    let Some(path) = path else { return };
    let mut body = String::new();
    for arg in std::env::args() {
        body.push_str("arg=");
        body.push_str(&arg);
        body.push('\n');
    }
    for (key, value) in std::env::vars() {
        body.push_str("env=");
        body.push_str(&key);
        body.push('=');
        body.push_str(&value);
        body.push('\n');
    }
    let _ = std::fs::write(path, body);
}
