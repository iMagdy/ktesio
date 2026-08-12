# AI-63(a): Global-lock unbounded-operation sweep (Epic 4 retro)

**Status:** IN PROGRESS (durable partial output — updated incrementally)
**Date:** 2026-07-21
**Scope:** READ-ONLY audit. Size the problem only; do NOT propose/implement the fix (that is AI-63(b)).
**Question:** The engine serializes every instance's every operation behind a coarse lock. Two Epic-4 CRITICAL bugs were both "an unbounded-duration op performed while holding that lock" (4-1 unbounded stdin write; 4-2 unbounded post-SIGKILL wait). Enumerate what ELSE runs unbounded under the lock. The retro said "we do not know if there are 2 or 12."

---

## Lock model (Confirmed — engine.rs)

**Nuance the retro framing missed: there are TWO coarse mutexes, not one.** `EngineInner`
(engine.rs:130-133) holds `registry: Mutex<Registry>` AND `supervisor: Mutex<Supervisor>`.
Almost every mutating op locks BOTH (registry first, then supervisor) and holds both for the
ENTIRE synchronous operation. So the effective serialization point is even broader than "one
`Mutex<Supervisor>`": the registry (rusqlite `Connection`) is *also* globally serialized, and
every supervisor op that touches persisted state does its SQLite I/O while holding both locks.

- Locks are acquired INSIDE the `spawn_blocking` closure (e.g. start engine.rs:597-599, stop
  618-621, poll reaper 203-208), held for the whole closure, dropped at closure end. So every
  byte of FS/DB/process I/O a supervisor method performs happens under the lock.
- It is ONE supervisor lock for ALL instances (no per-instance locking). While `start("A")`
  runs, `stop("B")`, `send_input("C")`, `fleet()`, and the reaper's `poll_once` ALL block on
  that lock. An unbounded op on instance A stalls the entire fleet INCLUDING crash detection.
- `run_blocking` (engine.rs:932-940) is the only bridge; `Blocking::*` (948-1099) is
  `rt.block_on(async_method)`. The blocking pool means the async runtime isn't starved, but the
  shared state is still fully serialized by the two mutexes.
- Reaper (spawn_reaper engine.rs:194-238): every `CRASH_POLL_INTERVAL` (250ms) it runs
  `poll_once` under both locks via spawn_blocking. Restart backoff `sleep(plan.delay)` happens
  BEFORE re-acquiring the locks (219) — good, the backoff is NOT under lock — but the `restart`
  → `start_inner` that follows IS under both locks (221-231).

Backend timeout constants (ports/process_backend.rs):
- `STDIN_WRITE_TIMEOUT = 5s` (306) — the 4-1 fix; bounds `write_stdin`.
- `KILL_CONFIRM_TIMEOUT = 5s` (357) — the 4-2 fix; bounds `stop`'s post-SIGKILL confirm wait.
  Its docs (308-352) explicitly state pre-fix `stop` did "a bare, unbounded `Child::wait()`...
  and `stop` runs while the [lock is held]" — the codebase already names the lock-amplification.
- `DEFAULT_STOP_WINDOW = 30s` (supervisor.rs:76) — the graceful-shutdown wait `stop` blocks on
  BEFORE escalating to SIGKILL. Bounded, but see the stop finding.
- `READINESS_WINDOW = 300ms` / `READINESS_POLL = 10ms` (86/89) — start's readiness watch.
- `LOG_ROTATE_MAX_BYTES = 10MB`, `LOG_ROTATE_GENERATIONS = 3` (367/374).

**Key off-lock finding:** the agent-output log *tailer/append/rotation* runs on its OWN
background thread (`spawn_tailer_thread` process_backend.rs:985, `append_attributed_line` 617,
`rotate_generations` 717) — NOT under the engine lock. So log *writes/rotation* are off-lock.
But log *reads* (`read_agent_log`, `read_agent_log_since`, `drain_usage_for`) run UNDER the lock.

## Operation inventory (under the lock)

### `start` / `start_inner` (supervisor.rs:384-707) — the heaviest path
Runs under BOTH locks. Ordered chain, each classified:
| # | Operation | src | Class | Blocking trigger |
|---|-----------|-----|-------|------------------|
| 1 | `registry.lookup` | 419 | UNBOUNDED* | SQLite read (disk/WAL/lock) |
| 2 | `next_state` gate | 423 | BOUNDED | pure |
| 3 | `registry.adapter_launch_facts` | 427 | UNBOUNDED* | SQLite read |
| 4 | `adapter::resolve_start_launch` (manifest re-read, fallback) | 438 | UNBOUNDED | **FS read of manifest file** + parse |
| 5 | `registry.metering_source` | 446 | UNBOUNDED* | SQLite read |
| 6 | `registry.effective_support` | 467 | UNBOUNDED* | SQLite read |
| 7 | `registry.effective_config` (4-layer fold) | 488 | UNBOUNDED | **FS read of instance config.toml** + TOML parse |
| 8 | `adapter::resolve_config_mapping` | 491 | UNBOUNDED | **FS read of manifest file** + parse |
| 9 | `start_observed_listener` (observed only) | 507 | SUSPECT | binds loopback TCP socket + spawns accept task |
| 10 | `registry.effective_config` (again, w/ base_url) | 514 | UNBOUNDED | **FS read config.toml** again |
| 11 | `registry.resolve_secrets` | 529 | UNBOUNDED | **FS read of 0600 secrets file** + env |
| 12 | `adapter::apply_config_mapping` (file target) | 532 | UNBOUNDED | **FS WRITE: renders a config file into Agent Home** |
| 13 | `registry.write_effective_config_snapshot` | 550 | UNBOUNDED | **FS WRITE of snapshot file** into Agent Home |
| 14 | `registry.effective_restart_policy` | 556 | UNBOUNDED* | SQLite read |
| 15 | `ensure_log_dir` → `create_dir_all` | 566 | UNBOUNDED | **FS mkdir** |
| 16 | `agent_log_len` → `fs::metadata` | 576 | UNBOUNDED | **FS stat** |
| 17 | `transition` (starting) → append_event + SQLite | 579 | UNBOUNDED | **FS append instance.log** + SQLite write |
| 18 | `backend.spawn` (Command::spawn) | 608 | SUSPECT | fork/exec; opens+creates log files for redirect (FS) |
| 19 | `watch_startup` (sleep loop) | 616 | BOUNDED | `thread::sleep` bounded to 300ms — but SLEEPS under lock |
| 20 | `backend.fingerprint` | 634 | SUSPECT | reads process start-time (/proc or sysctl) |
| 21 | `registry.write_spawn_record` | 639 | UNBOUNDED* | SQLite write |
| 22 | `transition_with_log_capture` (running) | 664 | UNBOUNDED | **FS append instance.log** + SQLite write |
| 23 | `registry.lookup` (final) | 706 | UNBOUNDED* | SQLite read |

`*` SQLite: rusqlite on a single connection; a read/write can block on disk, the WAL, or a
busy DB lock. Normally fast, but has NO explicit upper bound — "probably fine, not bounded".

### `stop` / `stop_inner` (supervisor.rs:768-992)
Runs under BOTH locks. **Worst-case lock hold ≈ 35s** (30s graceful window + 5s kill confirm).
| Operation | src | Class | Blocking trigger |
|-----------|-----|-------|------------------|
| `registry.lookup` | 809 | UNBOUNDED* | SQLite read |
| retry branch: `backend.poll` (stop_unconfirmed) | 838 | BOUNDED | non-blocking try_wait |
| retry self-heal: `clear_spawn_record` + transition | 861-875 | UNBOUNDED | SQLite write + FS append |
| `ensure_log_dir` → create_dir_all | 885 | UNBOUNDED | **FS mkdir** |
| `transition` (running→stopping) | 888 | UNBOUNDED | FS append + SQLite |
| `drain_usage_for(Terminal)` | 903 | UNBOUNDED | **FS read whole agent log** + SQLite ingest |
| `drain_observed_for` | 908 | BOUNDED? | in-memory queue drain + SQLite ingest |
| `backend.stop(handle, window)` | 931 | BOUNDED | **graceful window (≤30s) + KILL_CONFIRM_TIMEOUT (5s)** — 4-2 fix bounds the confirm phase; graceful window is the bigger hold |
| `running.remove` | 964 | BOUNDED | in-memory |
| `registry.clear_spawn_record` | 969 | UNBOUNDED* | SQLite write |
| `transition_with_log_capture` (stopped) | 982 | UNBOUNDED | FS append + SQLite |

**KNOWN-OPEN instance the retro named — `BackendError::StopUnconfirmed` → `stop_unconfirmed = true`
(supervisor.rs:945-951):** the 4-2 fix BOUNDED the confirm wait, so the lock is no longer held
forever. Residual: on StopUnconfirmed the handle is RETAINED and the instance stays `stopping`;
reconciliation relies on a later `stop` retry or `poll_once` observing the exit (both use the
cheap non-blocking `backend.poll`). This is bounded w.r.t. the LOCK, but the underlying process
may never be confirmed dead. Flagged per task; not a residual unbounded-lock-hold.

### `send_input` (supervisor.rs:1233-1331) — BOUNDED (the 4-1 fix)
| Operation | src | Class | Trigger |
|-----------|-----|-------|---------|
| `registry.lookup` | 1243 | UNBOUNDED* | SQLite read |
| `registry.effective_support` | 1256 | UNBOUNDED* | SQLite read |
| `backend.stdin_timed_out` / `has_stdin` | 1288/1294 | BOUNDED | in-memory flag / fd check |
| `backend.write_stdin` | 1318 | BOUNDED | **STDIN_WRITE_TIMEOUT = 5s** (4-1 fix) |

### `read_agent_log` / `read_agent_log_since` (supervisor.rs:1398-1501)
| Operation | src | Class | Trigger |
|-----------|-----|-------|---------|
| `registry.lookup` | 1406/1471 | UNBOUNDED* | SQLite read |
| loop: `read_log_lines_from` × (LOG_ROTATE_GENERATIONS-1) generations | 1411-1418 | UNBOUNDED | **FS read of each rotated generation** (≤10MB each) |
| `fs::read_to_string(current)` | 1423 | UNBOUNDED | **FS read of current gen** (≤10MB) + UTF-8 + parse |
| `fs::read(path)` (since) | 1474 | UNBOUNDED | **FS read of whole current gen** (no per-pass byte cap on this READ path — unlike the off-lock tailer's MAX_TAIL_BYTES_PER_PASS) |

`read_agent_log` reads up to 3 files totalling up to ~30MB under the lock; `--follow` drives
`read_agent_log_since` in a poll loop, each iteration re-reading the WHOLE current generation
(≤10MB) under the lock. Slow disk / NFS / a full 10MB log makes each an unbounded hold.

### `poll_once` — the reaper (supervisor.rs:1523-1675). HIGHEST-FREQUENCY lock holder (every 250ms)
Under both locks, EVERY 250ms it:
| Operation | src | Class | Trigger |
|-----------|-----|-------|---------|
| `drain_usage_all` → per running instance `drain_usage_for` | 1528 | UNBOUNDED | **FS read of each instance's agent.log** + SQLite ingest — scales with fleet size AND log size |
| `drain_observed_all` → per observed instance queue drain | 1535 | BOUNDED? | in-memory queue + SQLite ingest |
| per instance: `backend.poll` | 1545 | BOUNDED | non-blocking try_wait |
| on exit: `drain_usage_for(Terminal)` | 1557 | UNBOUNDED | FS read + SQLite |
| on exit: `registry.lookup` | 1568 | UNBOUNDED* | SQLite read |
| stuck-stopping reconcile: `clear_spawn_record` + transition | 1606-1620 | UNBOUNDED | SQLite + FS append |
| crash: `plan_restart` (spawn_record read + set count / clear) | 1644 | UNBOUNDED* | SQLite read+write |
| crash: `ensure_log_dir` + `transition_with_log_capture` | 1645/1654 | UNBOUNDED | FS mkdir + FS append + SQLite |

**CRITICAL amplifier — CONFIRMED (supervisor.rs:2543-2544, 2579-2590, 783-789):** the
drain → `ingest_usage` → `enforce_budget` (2380) → `apply_breach` (2508) → on
`BreachAction::Stop` → `enforce_stop` (2579) → `stop_with_cause` (783) chain runs INSIDE
`poll_once`. `stop_with_cause` calls `stop_inner(.., window=None, ..)` (789) → `window =
DEFAULT_STOP_WINDOW` = **30s graceful + 5s kill-confirm = ~35s** `backend.stop` hold. So the
250ms reaper can AUTOMATICALLY hold the fleet-wide lock for ~35s, with NO operator action, the
moment any instance breaches a `stop`-action budget. `BreachAction::Pause` → `enforce_pause` →
`backend.pause` is a single fast SIGSTOP syscall (bounded). This is the worst self-inflicted
amplification: an automatic, data-triggered 35s fleet freeze on the crash-detection hot path.

**`drain_usage_for` reads the WHOLE file every pass (supervisor.rs:2177):** `std::fs::read(&path)`
loads the ENTIRE `agent.log` into memory (then `plan_drain` consumes only `bytes[cursor..]`).
The read is O(current file size), runs every 250ms per running instance under the lock, and the
`agent.log` (the raw direct-redirect stdout file, story-4-2 SpawnSpec.log_file) has NO rotation
in the drain path — the code comment (2164-2169) says "a truncate/rotation — nothing in-tree
does this yet ... Proper rotation handling is deferred to Epic 4." So a long-running chatty agent
grows `agent.log` without bound and the reaper re-reads all of it under the lock on every tick.
This is UNBOUNDED and grows over time — arguably worse than a one-shot unbounded op. (VERIFY:
whether Epic-4 added agent.log rotation — the LOG_ROTATE_* constants apply to the ATTRIBUTED
output.log tailer, which is off-lock, not to agent.log.)

### `adopt_orphans` (supervisor.rs:1783-1865) — runs once at Engine::open, under both locks
Iterates EVERY spawn record; per record `backend.adopt` (reads /proc or sysctl to match
fingerprint), and on a live match `agent_log_len` (fs::metadata) + SQLite reads. Bounded by
record count, but each adopt is a syscall and the whole reconcile is serial under the lock at
startup. UNBOUNDED* per record (SQLite + proc reads); not a steady-state concern.

### Backends — synchronous ops under the lock (Confirmed)
| Backend op | unix src | Class | Notes |
|-----------|----------|-------|-------|
| `spawn` (Command::spawn) | unix:140 | SUSPECT | fork/exec, opens+creates 3 log files; normally fast, not bounded |
| `stop` | unix:302-364 | BOUNDED | SIGTERM + graceful-window poll loop (≤30s, deadline 316-340) + SIGKILL + `confirm_death(KILL_CONFIRM_TIMEOUT=5s)` (362). ≤35s hold. 4-2 fix. |
| `poll` | unix:366 | BOUNDED | `reap_if_exited` = try_wait / kill(pid,0) — non-blocking |
| `pause`/`resume` | unix:370/383 | BOUNDED | single SIGSTOP/SIGCONT syscall to the group |
| `write_stdin` | unix:465 | BOUNDED | `write_stdin_bounded(STDIN_WRITE_TIMEOUT=5s)` — 4-1 fix |
| `adopt` | unix:406 | SUSPECT | kill(pid,0) + proc_pidinfo (start-time); syscalls, bounded per record |
| **handle `Drop`** | **unix:571-583** | **SUSPECT / LATENT-UNBOUNDED** | **bare `child.wait()` at 580, guarded to run only when the process is still `Alive` at drop. SAME SHAPE AS THE 4-2 BUG; NOT covered by KILL_CONFIRM_TIMEOUT (the fix bounded `backend.stop`, never `Drop`). Reachable under the lock via `start_inner`'s record-commit-failure `drop(handle)` (supervisor.rs:643) and on `Engine::drop`.** |

Windows backend mirrors unix (`stop` bounded via `confirm_death(KILL_CONFIRM_TIMEOUT)` windows:445; `poll` non-blocking; `Drop` at windows:155 — same shape, same latent residual, not re-verified line-by-line).

### rusqlite (Registry) — the SECOND global chokepoint
No `busy_timeout`, PRAGMA, WAL, or `synchronous` tuning found in registry.rs (grep empty). One
`Connection` behind the registry Mutex. Every supervisor op locks the registry FIRST, so a slow
registry op blocks the whole fleet. SQLite access is "probably fast, NOT explicitly bounded":
a COMMIT fsync can block on disk pressure; contention returns SQLITE_BUSY (error, not a block,
since no busy_timeout is set) — so contention is not an unbounded-block risk, but disk fsync is.
Registry FS ops under the lock (Epic-5-relevant): `register` create_dir_all + writes;
**`remove` → `std::fs::remove_dir_all(home)` (registry.rs:369/376/558) = recursive Agent-Home
tree delete, O(tree size), UNBOUNDED, under BOTH locks**; `set_config` writes config.toml (461);
`write_effective_config_snapshot` writes snapshot (855); config-layer reads read_to_string (419/959).

## Count of genuinely-unbounded operations

**Answer to the retro's "2 or 12": it is NOT 2. There are ~17 distinct genuinely-unbounded
FS operation sites under the lock, plus fork/exec, plus one latent unbounded wait (Drop), plus
the whole rusqlite class.** The two that were fixed (unbounded stdin write, unbounded post-kill
wait) were simply the two that manifested as observable *hangs*; the filesystem surface behind
the same lock is an order of magnitude larger and was never enumerated.

Distinct genuinely-UNBOUNDED FS sites under the lock (duration ∝ external state, no explicit bound):
1. manifest read — `resolve_start_launch` (start_inner:438, fallback)
2. manifest read — `resolve_config_mapping` (start_inner:491)
3. instance config.toml read — `effective_config` (start_inner:488 & 514; enforce_budget:2391; config-get facade)
4. secrets file read — `resolve_secrets` (start_inner:529)
5. config file render/WRITE — `apply_config_mapping` file target (start_inner:532)
6. effective-config snapshot WRITE — (start_inner:550; registry:855)
7. `create_dir_all` — `ensure_log_dir` (2112; start/stop/poll/reconcile)
8. instance.log APPEND — `append_event` (every transition; 2811)
9. **whole-file read of the UNROTATED agent.log — `drain_usage_for` `std::fs::read` (2177), on the 250ms reaper hot path, per instance — grows without bound**
10. attributed output.log read (≤3 generations) — `read_agent_log` (1411-1423)
11. attributed output.log whole-current-gen read — `read_agent_log_since` (1474), in the `--follow` poll loop
12. instance.log read — `read_events_from` (2824; status/fleet/transition_events)
13. breach log APPEND + create_dir_all — `persist_breach_event` (2677/2692)
14. breach log read — `read_breach_events_from` (2891)
15. config.toml WRITE — `set_config` (registry:461)
16. Agent Home create_dir_all — `register` (registry:1132)
17. **recursive Agent-Home tree delete — `remove` `remove_dir_all` (registry:369/376/558)**

Plus: (18) `backend.spawn` fork/exec — SUSPECT; (19) handle `Drop` bare `child.wait()` —
LATENT-UNBOUNDED, same shape as the fixed 4-2 bug; (20) the entire rusqlite read/write class
(many sites) — "probably fast, fsync can block, not bounded".

BOUNDED (for contrast): `write_stdin` (STDIN_WRITE_TIMEOUT, 4-1 fix), `backend.stop` confirm
(KILL_CONFIRM_TIMEOUT, 4-2 fix) + graceful window (deadline), `watch_startup` sleep
(READINESS_WINDOW 300ms), `backend.poll` (non-blocking), `backend.pause`/`resume` (single syscall).

## Epic-5 (filesystem) exposure

Epic 5 adds MORE filesystem work. Given the pattern above, anything Epic 5 routes through a
lifecycle op or the registry under the lock is unbounded BY DEFAULT — the codebase has no
"do FS off the lock" convention except the output-log tailer (which is on a background thread).
Most plausibly hit:
- **New Agent-Home file reads/writes during start/stop** — they join sites 1-8/13-16; the
  config-snapshot / file-render pattern (5,6) is the template Epic 5 will copy, unbounded.
- **`remove_dir_all` (site 17)** grows directly with whatever Epic 5 adds to the Agent Home
  (artifacts, larger state, captured outputs) — a bigger tree = a longer under-lock delete.
- **`drain_usage_for`'s whole-file read (site 9)** — if Epic 5 makes agents produce more
  on-disk output, the unrotated agent.log grows faster and the 250ms reaper read stalls longer.
- Any new "read a manifest/artifact file" on the start path (sites 1-4) — same shape.

## Most-severe still-open instance

**#1 (most severe genuinely-unbounded): `drain_usage_for`'s `std::fs::read` of the entire,
UNROTATED `agent.log`, every 250ms, per running instance, under both locks (supervisor.rs:2177).**
Why it is the worst still-open instance: (a) genuinely unbounded AND monotonically growing —
agent.log has no rotation (the code comment at 2164-2169 defers it to "Epic 4", which shipped
without it; rotation only covers the off-lock attributed output.log); (b) it is on the
HIGHEST-FREQUENCY lock-holding path (the 250ms crash reaper), so it blocks the entire fleet +
crash detection itself; (c) it worsens over time and hits EVERY running instance each tick —
exactly the long-running "away-mode" fleet the product targets; (d) unlike the two fixed hangs
(which needed an adversarial trigger), this one degrades silently under normal operation.

**Runner-up (most severe *amplifier*, technically bounded): the automatic 35s fleet freeze** —
`poll_once` → drain → `ingest_usage` → `enforce_budget` → `enforce_stop` → `stop_with_cause`
→ `backend.stop` with the default 30s window + 5s kill-confirm (supervisor.rs:2544/2579/789).
A single instance breaching a `stop`-action budget makes the 250ms reaper hold the fleet-wide
lock for ~35s, with no operator action. Bounded, but 35s of fleet-wide + crash-detection freeze
is catastrophic for a supervisor.

**Latent (the exact fixed bug, un-fixed in a second location): handle `Drop`'s bare
`child.wait()` (unix:580, windows Drop).** The 4-2 fix bounded `backend.stop`; it did NOT touch
the RAII teardown, which still does an unbounded post-SIGKILL `child.wait()` guarded only by
"process still alive at drop". Reachable under the lock at `start_inner:643` (record-commit
failure) and on `Engine::drop`. If Epic 5 (or any refactor) ever drops a live handle under the
lock, this reopens the 4-2 hang in a new spot.

## Surprises / framing changes

1. **It is TWO mutexes, not one.** The retro said "a SINGLE global `Mutex<Supervisor>`". The
   real chokepoint is `Mutex<Registry>` + `Mutex<Supervisor>`, acquired together (registry
   first) for every mutating op. The rusqlite registry is itself globally serialized — a slow
   registry op (e.g. `remove`'s recursive delete) blocks every supervisor op too.
2. **The count is ~17 FS sites, not 2-12.** The two fixed bugs were the two that *hung*; the FS
   surface behind the lock was never enumerated and is far larger. Most are "probably fast on
   local SSD" but NONE have an explicit bound — the same latent shape, waiting for slow disk /
   NFS / a large file / an away-mode long-run.
3. **The worst offender is on the crash-detection hot path and grows over time** (site 9),
   which inverts the usual intuition that one-shot operator commands (start/stop) are the risk.
   The 250ms automatic reaper is the biggest lock consumer, and it scales with fleet size, log
   size, AND (via enforcement) can self-trigger a 35s stop.
4. **The 4-2 bug's shape survives in `Drop`** — bounding `backend.stop` did not bound the
   handle teardown's `child.wait()`. Same class, second location, currently narrowly reachable.
5. **agent.log is unrotated by design-gap** — rotation was deferred to "Epic 4" in a code
   comment (2164-2169) and Epic 4 shipped rotation only for the *attributed* output.log tailer,
   not for the raw agent.log that the billing drain reads whole every 250ms.
6. Reassuring counter-point: SQLite contention is NOT an unbounded-block risk (no busy_timeout
   set → SQLITE_BUSY returns immediately as an error). The SQLite risk is only disk fsync at
   COMMIT, same bucket as any FS write.

## Status: CONCLUDED (read-only sizing complete; fix is AI-63(b), out of scope here).
