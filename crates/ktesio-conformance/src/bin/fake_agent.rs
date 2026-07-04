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
//!
//! The binary writes a small marker file (`--marker <path>`) on startup if asked,
//! so a test can confirm it actually ran without racing on stdout capture.

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
    /// Heartbeat interval (story 1-5). `None` = no heartbeat (quiet sleep loop).
    heartbeat: Option<Duration>,
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
    let mut heartbeat = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--exit-fast" => {
                let code = args.next().and_then(|s| s.parse::<i32>().ok()).unwrap_or(1);
                exit_fast = Some(code);
            }
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
            "--marker" => {
                if let Some(path) = args.next() {
                    marker = Some(PathBuf::from(path));
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
        heartbeat,
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

    // Loop until we are killed, or until the linger window elapses (the self-exit
    // fallback). When a heartbeat interval is set, print an incrementing
    // `heartbeat <n>` line every interval and flush — while SIGSTOP'd the whole
    // process freezes, so the captured log stops growing (the story-1-5
    // observable-suspension proof); SIGCONT resumes it. With no heartbeat this is
    // a quiet short-poll sleep (unchanged 1-4 behavior). A short poll keeps the
    // process responsive to signals in both modes.
    let deadline = Instant::now() + opts.linger;
    let mut beats: u64 = 0;
    let mut next_beat = opts.heartbeat.map(|interval| Instant::now() + interval);
    while Instant::now() < deadline {
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
