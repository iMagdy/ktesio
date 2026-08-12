---
stepsCompleted: [1, 2, 3, 4, 5, 6]
inputDocuments:
  - ../architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md
  - ../prds/prd-ktesio-2026-07-02/prd.md
  - ../epics.md
  - crates/ktesio-engine/src/backends/unix/mod.rs
  - crates/ktesio-engine/src/backends/windows/mod.rs
  - crates/ktesio-engine/src/domain/supervisor.rs
  - crates/kt/src/cli/agent.rs
workflowType: 'research'
lastStep: 6
research_type: 'technical'
research_topic: 'Durable cross-CLI-command agent supervision — engine-as-daemon vs detach vs document-as-v1'
research_goals: 'Inform action item AI-20: choose how Ktesio provides durable, cross-CLI-command agent supervision; recommend an option and the owning epic/story.'
user_name: 'Islam'
date: '2026-07-06'
web_research_enabled: true
source_verification: true
spike_for: 'AI-20'
---

# Research Report: Durable Cross-CLI-Command Agent Supervision

**Date:** 2026-07-06
**Author:** Islam
**Research Type:** technical (design SPIKE — report, not code)
**Informs:** action item **AI-20**

---

## Research Overview

### The problem, precisely

`kt agent start <name>` does **not** keep the agent alive across separate CLI commands. Each `kt` invocation constructs its own short-lived engine; on a clean exit the engine tears down, the supervisor drops its process handles, and the Unix backend's `Drop` `SIGKILL`s the whole process group (Windows: the Job Object closes with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, killing the tree). This kill-on-drop is story 1-4's **single-lifetime safety net** — the thing that guarantees NFR-1 "no unsupervised orphans" for the common case.

Consequences, grounded in the code:

- A standalone `kt agent start` prints the new state to stdout, then **the started process is killed when that CLI process exits cleanly** (`crates/kt/src/cli/agent.rs:433-468`). A subsequent, separate `kt agent pause`/`stop` has nothing live to control.
- Story 1-6's **orphan adoption (AD-5)** rescues only the engine-**crash** case: a crash means no destructors ran, so a process survives and the *next* `Engine::open` re-adopts it by `{pid, start-time}` fingerprint (`crates/ktesio-engine/src/domain/supervisor.rs:14-24`). A **clean** CLI exit runs `Drop`, so there is nothing to adopt.
- This is surfaced **honestly, not silently**: `start` emits a one-line stderr note — *"the started process is supervised only for this engine session and stops when this command exits; durable supervision across separate CLI invocations is future work."* (`crates/kt/src/cli/agent.rs:456-463`). The CLI integration tests assert this notice goes to **stderr**, never stdout (`crates/kt/tests/agent_cli.rs:558-592`).

So the gap is real and known: **`kt agent start` → separate `kt agent pause`/`stop` cannot control a still-running agent**, because a clean CLI exit already killed it.

### First-party constraints that bound the answer

These are not guesses — they are already ratified in the PRD and spine:

1. **Library-first is a v1 decision; service/IPC is explicitly deferred to v1.x.** PRD §3 defines the Engine as *"Delivered as a Rust library. `[ASSUMPTION: library-first; service/IPC delivery is out of v1 — see §13 Q6.]`"* PRD §9 lists *"Service/IPC delivery of the Engine (embed = Rust library only in v1)"* as out of scope, and **§13 open question Q6** names the v1.x path as owned by **"Islam + architecture."** (`prd.md:59, 402, 438, 447`)
2. **The spine already anticipated a daemon/service.** The **Deferred** list in `ARCHITECTURE-SPINE.md` reads: *"**Service/IPC embedding transport (PRD Q6)** — the Embedding Interface stays transport-agnostic (plain async API + serde events) so a JSON-RPC/gRPC shim can wrap it at v1.x without reshaping the engine."* The event schema (AD-14) is versioned serde over a subscription API precisely so a wire transport can be layered on later.
3. **Cross-OS parity is a hard constraint (NFR-2, 3 OSes).** Every FR must behave equivalently on Linux, macOS, Windows; where an OS lacks a primitive the difference is *documented, not silent*. Process control per OS lives only in `backends::{unix,windows}` (AD-4).
4. **NFR-1 orphan-safety and the "surfaced, not silent" principle are durable gates**, already enforced by the kill-on-drop net + adoption + the stderr notice.
5. **NFR-4 performance budgets** (placeholders to be validated in Epic 7): read commands <1s @ 25-instance Fleet; supervision overhead ≤2% CPU and ≤50MB RSS steady-state **per running instance**. An always-on host changes the idle-cost profile and is what Epic 7 benchmarks.

### Methodology

Read-only investigation. First-party: the architecture spine, PRD, epics, and the live `unix`/`windows` backends, supervisor, and `kt` start/stop/pause commands. External: current web sources on comparable systems (systemd, Docker/dockerd, Podman, BuildKit, Tailscale, tmux/dtach, nohup, supervisord) and on the concrete cross-OS detach/daemon primitives (Unix `fork`+`setsid`+double-fork; Windows `CREATE_BREAKAWAY_FROM_JOB` / `DETACHED_PROCESS` / `CREATE_NEW_PROCESS_GROUP`; Rust crate landscape). Sources are cited inline and collected at the end. No code was written; no sprint/GitHub state was touched.

---

## How comparable systems solve durable supervision

| System | Shape | Who owns the supervised process across client invocations | Reachable control channel? | Lesson for Ktesio |
| --- | --- | --- | --- | --- |
| **systemd + systemctl** | init system **is** PID 1; supervises services directly as its own children, no intermediary daemon | The init system itself; self-recovers if it crashes | Yes (D-Bus / systemctl) | Deep OS integration and self-recovery — but only because it *is* PID 1. A userland agent runner cannot be PID 1, and can't assume systemd (macOS/Windows). Not directly available to Ktesio. |
| **Docker (dockerd + docker CLI)** | long-lived daemon owns all containers; CLI is a thin client over a socket/REST API | `dockerd` | Yes (REST over socket) | The canonical "engine-as-daemon." Buys reachability and durability — but the daemon is *"a single point of failure for all the containers"* and adds install/lifecycle overhead. |
| **Podman** | **daemonless**; containers run under a transient conmon per container, integrates with systemd | conmon / systemd, no central daemon | Via systemd/API | The industry's explicit reaction to dockerd's single-point-of-failure. Shows a daemonless split is viable — but leans on the host init system for durability, which Ktesio can't assume cross-OS. |
| **BuildKit (buildkitd + buildctl)** | client/daemon over gRPC; supports **both** systemd socket-activation (`--addr fd://`) **and** a client-side "daemonless" wrapper that auto-starts the daemon | `buildkitd` | Yes (gRPC) | Best example that the two "optional daemon" strategies can coexist — an always-on service *and* a lazily-auto-started local daemon behind the same client. |
| **Tailscale (tailscaled + tailscale)** | long-running daemon; CLI is a LocalAPI client over a **Unix socket (Linux/macOS) / named pipe (Windows)** | `tailscaled` | Yes (HTTP LocalAPI) | Directly relevant cross-OS reachability precedent (socket vs named pipe). Notably does **not** silently auto-start the daemon — it errors with actionable guidance ("run `sudo systemctl start tailscaled`"). Also: CLI and daemon can be the *same binary*, dispatched by argv0. |
| **tmux / screen / dtach** | a multiplexer owns the real TTY over its own Unix socket; you detach/reattach | the multiplexer process | Yes — **full interactive reattach** | Detach + a socket buys reattachable I/O. `dtach` is the minimal form: one binary, one Unix-domain socket. This is the pattern that actually preserves a *control channel*, unlike bare detach. |
| **nohup / setsid / disown** | fire-and-forget detach from the controlling terminal | init (reparented to PID 1) | **No** — stdin/stdout are severed | Keeps the process *alive* but leaves *"a zombie you can't reattach to."* Detach ≠ reachability. This is the crux for Epic 4 (below). |
| **supervisord / daemontools** | a long-running supervisor runs foreground children, restarts them | the supervisor daemon | Via its own control interface | The prevailing lesson: *under a supervisor, don't self-daemonize* — run children in the foreground and let the supervisor own them. Ktesio's engine already **is** that supervisor; the question is only how long *it* lives. |

**Two "optional daemon" strategies emerge** for a cross-platform CLI that can't assume an init system:
- **(A) Init-system socket activation** (systemd `sd_listen_fds`, `--addr fd://`): elegant, but assumes systemd — not cross-OS, so not a fit for Ktesio's baseline.
- **(B) Client-side auto-start ("invisible daemon"):** the CLI probes the socket; on failure it spawns a detached daemon and waits for the socket, then talks per-request. This is init-system-agnostic and cross-OS, and is the shape that fits a `kt`-first tool if/when a daemon is adopted.

---

## Cross-OS feasibility of the underlying primitives (the hard NFR-2 test)

### Unix (Linux/macOS) — well-trodden

- **Detach:** `fork` → `setsid` (new session, no controlling terminal) → optional **second fork** (so the daemon can't reacquire a TTY). Ktesio *already* calls `setsid` in `pre_exec` for every spawn to get its own process group (`backends/unix/mod.rs:152-157`). Making the process *outlive* the CLI is then only about **not** killing the group on `Drop`.
- **Daemon:** the same fork/setsid dance produces the long-lived host; a Unix-domain socket is the control channel.
- Verdict: **clean on Unix for both (a) and (b).**

### Windows — the fracture point

The current backend does the **opposite** of detach on purpose: it assigns each agent to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (`backends/windows/mod.rs:178`) and spawns with `CREATE_NEW_PROCESS_GROUP` (`:228`) — so when the `kt` process (and its job handle) goes away, the tree dies. That is the Windows arm of the single-lifetime net.

To make a child **outlive** the CLI on Windows you must invert several things at once (per Microsoft's Job Objects docs and corroborating reports):
1. **Break out of the job:** `CREATE_BREAKAWAY_FROM_JOB` (requires the job to allow it via `JOB_OBJECT_LIMIT_BREAKAWAY_OK`), or `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK`. The flag *fails with access-denied* if no job permits breakaway — a real footgun.
2. **Detach the console:** `DETACHED_PROCESS`, or the child dies when the parent console window closes (a Windows-only trap: *"the child process starts correctly … but as soon as the Windows console is closed, the child process dies … in Linux the code works like a charm"*).
3. **Isolate signals:** `CREATE_NEW_PROCESS_GROUP` (already present).
4. **Redirect stdio to `NUL`** and **don't `wait()`**.

Two aggravations specific to Ktesio's stack:
- `std::process::Command` doesn't expose the child's main-thread handle, which is why the backend already documents a `CREATE_SUSPENDED` dance as impossible with std alone (`backends/windows/mod.rs:15-26`). Detach adds *more* raw `windows-sys` FFI to an already-`[ASSUMPTION]`-laden module.
- A detached-but-orphaned Windows process is **no longer in a kill-on-close job**, so the NFR-1 no-orphan guarantee now rests entirely on `{pid, creation-time}` re-adoption via `OpenProcess(... | PROCESS_TERMINATE)` (`backends/windows/mod.rs:57-68`) — which exists, but the safety margin narrows.

**Cross-OS cleanliness ranking of the primitive work:**
- **Option (a) daemon:** the *supervised* processes keep today's kill-on-close/kill-on-drop semantics unchanged — they just live inside a longer-lived host. The new cross-OS surface is the **transport** (Unix socket vs Windows named pipe — a solved, well-precedented split, per Tailscale) plus spawning the host itself. **Cleaner per-OS**, and the risky process-control code is untouched.
- **Option (b) detach:** requires **inverting** the per-OS teardown in both backends, and the Windows path is a genuinely fracture-prone multi-flag dance against a `std::process` limitation the module already fights. **This is where (b) fractures per-OS.**

The Rust ecosystem confirms there's no single turnkey answer: `daemonize`/`daemonize2` cover Unix fork/setsid only; `windows-service` covers only Windows SCM; the newer `daemon-kit` attempts a unified API but is young. The pragmatic path either way is conditional-compilation in `backends::{unix,windows}` — which is exactly where AD-4 already confines OS code.

---

## The reachability gap that separates (a) from (b)

This is the single most decision-relevant finding, and it's easy to miss.

**AD-5 adoption re-acquires *supervision*, not a *control channel*.** An adopted handle can `killpg`/`TerminateProcess`, poll liveness, and pause/resume — because those act on the pgid/job/handle from *outside* the process. That is enough for **`pause` and `stop`**. It is **not** a live pipe to the process.

**Epic 4 needs a reachable control channel, which detach does not provide.** FR-24 (send input uniformly) and FR-25 (stream output / read what was said while detached) presuppose a process you can *talk to*. But detaching a process **severs stdin/stdout** — the universal lesson across the reattach literature is that `nohup`/detach leaves *"a zombie you can't reattach to,"* and *"once fully detached, any process that awaits input will be terminated, as stdin is already closed."* The tools that preserve interaction — tmux/screen/**dtach** — do so precisely because a **long-lived process owns a socket** and brokers the I/O. Retrofitting reattach onto an already-detached process (reptyr/ptrace, gdb-rewiring FDs) is explicitly fragile: it fails when stdio is redirected to files/sockets (which Ktesio *does* — AD-12 captures stdout/stderr into rotated per-instance log files) and can be blocked by kernel ptrace policy.

Ktesio's AD-12 `InteractionChannel` (manifest default: stdin pipe in, stdout/stderr captured out) lives **inside the engine that spawned the agent**. When that engine exits, the pipe endpoints go with it. So:

- **Option (b) detach** can make `pause`/`stop`/`poll` work across commands (via adoption) — but a *detached* agent has **no live stdin** for `kt agent send`, and its output only lands in the rotated log files (readable, but not a live stream from a fresh CLI). **Epic 4's send-input is not satisfied by detach**; attach-to-output degrades to tailing a log.
- **Option (a) daemon** keeps the `InteractionChannel` **alive inside the long-lived host**; the CLI reaches it over the control socket. This is the *only* option that cleanly delivers FR-24/FR-25 across separate commands — and it is exactly how tmux/dtach/Tailscale/dockerd all work.

**Implication:** framing AI-20 as "pause/stop across commands" undersells it. The moment Epic 4 lands, the durable answer must carry an interaction channel — and that is a daemon (or embedding in a long-lived Host), not detach.

---

## Fit with the existing architecture — what each option reuses vs. demands

### Option (a) — Engine-as-daemon/service (`kt` becomes a thin client)

**Reuses:**
- The **entire domain core untouched** (AD-1): state machine, ledger, budgets, config, supervisor, both backends. The supervised processes keep today's kill-on-drop/kill-on-close semantics; only the *owner's lifetime* changes.
- The **AD-14 versioned event schema** and the **AD-2 transport-agnostic Embedding Interface** — the spine *already* designed these to be wrapped by a JSON-RPC/gRPC shim (Deferred item). This option is the spine's own anticipated path.
- The **AD-13 async tokio engine** already suits a long-lived multiplexed host; `kt` keeps using the `blocking()` facade, now over IPC.

**Demands (new surface):**
- A **transport** (Unix socket / Windows named pipe — the Tailscale split) and a serialized request/response + event-stream protocol over the existing serde structs.
- A **daemon lifecycle**: how it starts (client-side auto-start "invisible daemon", vs. an explicit `kt daemon` / user-level service), where its socket lives, version-handshake on connect (BuildKit/invisible-daemon pattern), and shutdown.
- **Single-point-of-failure hardening**: if the daemon dies, its children must still be safe — which is *exactly* what AD-5 adoption already provides on the daemon's own restart. The daemon inherits, rather than replaces, the crash-recovery machinery.
- **NFR-4 idle cost**: an always-on host must fit ≤2% CPU / ≤50MB RSS steady-state — measurable in Epic 7's benchmark story.
- **AD-5 remains the safety net**, not dead weight: a daemon crash is the same "ungraceful exit → orphan → re-adopt on next start" case adoption already handles.

**Deployment/embedding change:** this is the one option that shifts the delivery model — but only *additively*. It literally *is* PRD Q6 ("service/IPC delivery") and the spine's Deferred "Service/IPC embedding transport." A Host embedding the library in-process is the same shape (a long-lived owner of the engine); the daemon is that owner for the CLI world.

### Option (b) — `kt agent start --detach` (leans on AD-5 adoption)

**Reuses:**
- **AD-5 write-ahead records + orphan adoption** for `pause`/`stop`/`poll` re-attachment — the mechanism exists and is tested (`adopt`, `poll_once`, the AI-10 steady-state PID-reuse guard).
- No new transport, no daemon lifecycle. Smallest *conceptual* surface for the pause/stop slice.

**Demands (new surface):**
- **Invert the per-OS teardown**, gated behind the flag: skip the group `SIGKILL` on `Drop` for a detached Unix process; on Windows, add `CREATE_BREAKAWAY_FROM_JOB` (+ `JOB_OBJECT_LIMIT_BREAKAWAY_OK`) + `DETACHED_PROCESS`, i.e. *undo* `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. This is the fracture-prone Windows work above, in the module that already documents std limitations.
- A deliberate **hole in the single-lifetime net**: a cleanly-detached agent is *designed* to outlive its starter, so NFR-1 now depends wholly on adoption + reconciliation being correct for the clean-exit path too (today they cover only the crash path). The "no unsupervised orphan" gate must be re-argued for intentional detach.
- **Does not solve Epic 4** (no live control channel; send-input has no stdin) — so it's a partial answer with a second migration later when interaction lands.
- Honesty burden: the stderr notice flips from "will be killed" to "is now detached; reconnect with …", and `kt` needs a reliable *reconnect/attach* verb backed only by adoption.

### Option (c) — Document single-lifetime as intended v1

**Reuses everything; demands nothing but words.** It ratifies the status quo: per-command supervision is v1's honest behavior; the durable path is a **persistent embedding** (a long-lived Host embedding the library, or the deferred daemon). This is already *nearly* the position the code takes — the stderr notice says "future work." Option (c) is really "adopt (a) as the *design intent* but schedule it for Epic 7 / v1.x, and make v1's boundary explicit and documented rather than a surprise."

---

## Implications for other epics

- **Epic 3 (metering must keep accruing while unattended).** The `MeteringSource` → ledger → `BudgetEvaluator` → Breach pipeline (AD-7) runs **inside the engine**. Under **single-lifetime/detach**, when no `kt` engine is live, *nothing is metering* — a detached agent burns tokens with no ledger writes and no cost-cap enforcement until the next command. That quietly undercuts SM-2 and the "governance runs locally, always" promise. Under a **daemon**, metering and breach enforcement run continuously in the host. **This is a second strong pull toward (a)** and a reason the gap matters before Epic 3 ships enforcement, not only at Epic 4.
- **Epic 4 (send-input / attach-output).** As analyzed: presupposes a reachable running agent with a live channel. **Detach cannot deliver send-input**; a daemon (or embedding) can. Epic 4 is the point where a non-daemon answer visibly breaks.
- **Epic 7 (embedding — the engine-as-library/service shape).** This is the natural home. Epic 7 already: proves every capability through the library with no CLI (7.1), ships the versioned event subscription (7.2), guarantees headless/no-TTY embedding (7.3), publishes the crates (7.4), and **benchmarks NFR-4** (7.5). A daemon is "an in-repo Host that embeds the library and exposes it over IPC" — it sits directly on top of 7.1–7.3 and is measured by 7.5. The spine's Deferred "Service/IPC embedding transport (Q6)" is precisely this.

**Does durable supervision belong in Epic 7, or earlier?** The *durable mechanism* (daemon/service transport) belongs in **Epic 7 / v1.x**, because it depends on the embedding surface (7.1–7.3) being real and benchmarked (7.5), and because PRD Q6 already scheduled service/IPC for v1.x. But two things should move **earlier**, into Epic 1's resilience scope or a small dedicated story:
1. **The explicit v1 decision + documentation** that CLI supervision is per-command (option (c)'s deliverable) — so the boundary is a ratified design stance, not a TODO.
2. A note in **Epic 3** that continuous metering/enforcement presumes a long-lived engine (daemon or Host), so cost-cap semantics for the "unattended CLI agent" case are defined rather than silently absent.

---

## Orphan-safety (NFR-1) & honesty ("surfaced, not silent") per option

| | NFR-1 orphan-safety | Honesty posture |
| --- | --- | --- |
| **(a) Daemon** | Preserved and *strengthened*: children keep kill-on-drop/kill-on-close inside the host; a daemon crash is the ungraceful-exit case AD-5 adoption already recovers on daemon restart. Single point of failure is the known trade — mitigated by adoption. | Strongest: `kt` reports daemon reachability (Tailscale-style actionable errors if down); all state transitions/usage/breaches flow over AD-14 events. |
| **(b) Detach** | *Weakened by design*: a cleanly-detached process is meant to outlive its starter, so the kill-on-drop net is deliberately holed; NFR-1 rests entirely on adoption+reconciliation now covering the clean-exit path (today: crash-only). Windows loses its kill-on-close job for detached procs. | Requires a new honest "now detached; reconnect with X" contract and a reliable attach verb; the current "will be killed" notice must invert. |
| **(c) Document v1** | Fully preserved — it *is* the current kill-on-drop net; nothing changes. | Already honest (the stderr notice) — this option just elevates that notice to a ratified, documented design boundary. |

---

## Comparison table (the decision matrix)

| Option | Cross-OS cleanliness | Reuse of AD-5 adoption | Delivers Epic 4 control channel | Continuous Epic 3 metering | Complexity / new surface | Orphan-safety (NFR-1) | Which epic |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **(a) Engine-as-daemon** | **Clean** — process-control code unchanged; new surface is transport (socket/named pipe, precedented) | **Complements** it — daemon crash = the case adoption already covers | **Yes** — channel lives in the long-lived host | **Yes** — pipeline runs continuously | **High** (transport + daemon lifecycle + idle-cost), but *additive*; = PRD Q6 / spine Deferred | **Preserved & strengthened**; SPOF is the known trade | **Epic 7 / v1.x** (builds on 7.1–7.3, benchmarked by 7.5) |
| **(b) `start --detach`** | **Fractures on Windows** — must invert kill-on-close via breakaway + `DETACHED_PROCESS`; fights `std::process` limits | **Leans heavily** on it (pause/stop/poll only) | **No** — detach severs stdin; send-input unsupported | **No** — no live engine ⇒ no metering while detached | **Medium** conceptually, but **fracture-prone** per-OS teardown inversion + a deliberate NFR-1 hole | **Weakened by design**; re-argue no-orphan for clean-exit path | Would land ~Epic 1/4; but only a *partial* answer, needing rework at Epic 4 |
| **(c) Document as v1** | **N/A** — no change | Uses it exactly as today (crash-only) | No (unchanged) | No (unchanged) | **Minimal** — documentation + decision record | **Fully preserved** (the current net) | **Epic 1 now** (decision + docs), durable path → Epic 7 |

---

## RECOMMENDATION for AI-20

**Adopt a two-part answer: (c) now + (a) as the durable path in Epic 7/v1.x. Do NOT build (b).**

1. **Ratify (c) as the explicit v1 design stance (Epic 1, now).** CLI supervision is **per-command by design** in v1; the durable path is a **persistent embedding** — a long-lived Host, or the daemon of part 2. Convert the existing "future work" stderr notice into a documented, ratified boundary (README + spine note), and add an Epic 3 note that continuous metering/enforcement presumes a long-lived engine. This is nearly free, keeps NFR-1 fully intact, and turns a known gap into an owned decision. It directly closes AI-20's "document single-lifetime as intended v1."

2. **Own the durable mechanism as (a) engine-as-daemon, scheduled in Epic 7 (v1.x), realizing PRD Q6.** A daemon is *"an in-repo Host that embeds the library and exposes it over a local transport (Unix socket / Windows named pipe)."* It sits directly on Epic 7's embedding surface (7.1 capability-complete library, 7.2 versioned events, 7.3 headless/no-TTY) and is measured by 7.5's NFR-4 benchmark. Recommended new story, e.g. **Story 7.6 — "Durable supervision via a local engine service"**: the client-side **auto-start ("invisible daemon")** pattern so `kt` transparently spawns/reaches the daemon (BuildKit-style), with Tailscale-style actionable errors when it can't; adoption (AD-5) is its crash-recovery net.

**Why (a) over (b):**
- **(b) doesn't actually solve the whole problem.** It buys `pause`/`stop` across commands but **cannot deliver Epic 4's send-input** (detach severs stdin) and **stops metering while detached** (Epic 3). It's a partial fix that mandates a second, larger migration when interaction lands.
- **(b) is where cross-OS fractures.** It forces *inverting* the Windows kill-on-close job (breakaway + `DETACHED_PROCESS`) — the exact fracture NFR-2/AD-4 warn about — and deliberately holes the NFR-1 safety net for the clean-exit path.
- **(a) is the spine's own anticipated path.** The Embedding Interface was made transport-agnostic and the event schema versioned *specifically so a JSON-RPC/gRPC shim can wrap it at v1.x without reshaping the engine* (spine Deferred). PRD Q6 already assigns this decision to "Islam + architecture." Choosing (a) is ratifying an existing design seam, not inventing one — and it keeps the risky process-control code untouched while adding only transport + lifecycle.

**Net:** honest per-command v1 today (nothing built, gap owned), durable daemon in v1.x that reuses the domain core, the event schema, and AD-5 wholesale, and that alone satisfies the reachability Epics 3 and 4 will demand.

---

## Open questions for Islam

1. **Daemon start model:** client-side **auto-start** ("invisible daemon" — `kt` transparently spawns it on first miss, BuildKit-style) vs. an **explicit** `kt daemon start` / user-level service (launchd/systemd-user/Windows service) vs. **both** (BuildKit supports both)? Auto-start is the most CLI-native but has a probe/spawn race to get right; explicit is simplest to reason about.
2. **Scope of the daemon:** per-user single daemon owning the whole Fleet, vs. one host process per Fleet/state-dir? (Affects the socket namespace and the 25-instance NFR-4 budget interpretation.)
3. **Transport now or later:** wrap the Embedding Interface in a concrete transport (JSON-RPC over the socket/pipe reusing AD-14 serde structs) as part of Story 7.6, or first ship an in-repo in-process Host (7.1) and add the wire transport as a follow-up? The spine allows either; the events are already serde.
4. **Is any interim relief wanted before v1.x?** If operators need "start and walk away" *before* Epic 7, is a documented, clearly-fenced `--detach` (pause/stop only, no interaction, explicit "not metered while detached" warning) an acceptable stopgap — or does its NFR-1/Epic-3 cost make it not worth the per-OS Windows work? (My recommendation: skip it; wait for the daemon.)
5. **NFR-4 for an always-on host:** confirm the ≤2% CPU / ≤50MB RSS steady-state budget is meant to bound the *daemon* too (Epic 7 Story 7.5 already benchmarks per-instance supervision overhead; the idle daemon baseline should be added).

---

## Sources

First-party (repository):
- `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` — AD-1, AD-2, AD-4, AD-5, AD-7, AD-12, AD-13, AD-14, AD-15; **Deferred: "Service/IPC embedding transport (PRD Q6)"**
- `_bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md` — §3 (library-first `[ASSUMPTION]`), §5 (NFR-1, NFR-2, NFR-4), §9 (out of scope: service/IPC), §13 Q6 (owner: Islam + architecture)
- `_bmad-output/planning-artifacts/epics.md` — Epic 3, Epic 4 (FR-24/25), Epic 7 (Stories 7.1–7.5), NFR mapping
- `crates/ktesio-engine/src/backends/unix/mod.rs` — `setsid` in `pre_exec`; `Drop` group-`SIGKILL`; adopt/fingerprint (AD-5)
- `crates/ktesio-engine/src/backends/windows/mod.rs` — `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, `CREATE_NEW_PROCESS_GROUP`; std thread-handle limitation; adopt via `OpenProcess`
- `crates/ktesio-engine/src/domain/supervisor.rs` — in-memory handle map for engine lifetime; `adopt_orphans`, `poll_once` (crash-only recovery)
- `crates/kt/src/cli/agent.rs` (`start`/`stop`/`pause`) & `crates/kt/tests/agent_cli.rs` — the honest single-lifetime stderr notice and its stderr-only assertions

External (web, accessed 2026-07-06):
- [Systemd vs. Docker (LWN.net)](https://lwn.net/Articles/676831/)
- [How to Understand the Docker Client-Server Architecture (OneUptime)](https://oneuptime.com/blog/post/2026-02-08-how-to-understand-the-docker-client-server-architecture/view)
- [Job Objects — Win32 apps (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Closing parent console kills detached process in Windows (boostorg/process #186)](https://github.com/boostorg/process/issues/186)
- [Python subprocess CREATE_BREAKAWAY_FROM_JOB (runebook.dev)](https://runebook.dev/en/docs/python/library/subprocess/subprocess.CREATE_BREAKAWAY_FROM_JOB)
- [Destroying all child processes when the parent exits (The Old New Thing)](https://devblogs.microsoft.com/oldnewthing/20131209-00/?p=2433)
- [UNIX daemonization and the double fork (0xjet)](https://0xjet.github.io/3OHA/2022/04/11/post.html)
- [Detaching a process from terminal — setsid & nohup (mihids)](http://mihids.blogspot.com/2015/02/detaching-process-from-terminal-exec.html)
- [You don't need to daemonize (Hacker News)](https://news.ycombinator.com/item?id=9793466)
- [How to keep Claude Code running across SSH disconnects — dtach/tmux (cdmckay.org)](https://cdmckay.org/how-to-keep-claude-code-running-across-ssh-disconnects/)
- [Reattach to an Already Running Process — reptyr (Baeldung)](https://www.baeldung.com/linux/running-process-reattach)
- [Attach a Terminal to a Detached Process (Baeldung)](https://www.baeldung.com/linux/attach-terminal-detached-process)
- [Invisible Daemon: architecture patterns for local dev tools (CocoIndex)](https://cocoindex.io/blogs/building-an-invisible-daemon/)
- [systemd Socket Activation: Lazy Loading for Services (unixy.io)](https://unixy.io/blog/systemd-socket-activation/)
- [BuildKit (moby/buildkit)](https://github.com/moby/buildkit) & [BuildKit | Docker Docs](https://docs.docker.com/build/buildkit/)
- [tailscaled daemon (Tailscale Docs)](https://tailscale.com/docs/reference/tailscaled) & [Tailscale CLI Architecture (DeepWiki)](https://deepwiki.com/tailscale/tailscale/6.1-tailscale-cli-architecture)
- [daemon-kit: Cross-Platform Daemon Management in Rust (Medium)](https://medium.com/rustaceans/daemon-kit-cross-platform-daemon-management-in-rust-4ccb2f78d8b0)
- [daemon(7) — Arch manual pages](https://man.archlinux.org/man/daemon.7.en)
