---
baseline_commit: 884b974a91c6143c2b1ba7fad874634f9022c0f8
baseline_ref: origin/main (PR #143 merged — runner-positioning banner)
---

# Story 6.2: Run the real Hermes Agent under Ktesio lifecycle

Status: done (implementation PR #144 / 58d5a70; BMAD review pass complete 2026-08-30 — all gates green, findings triaged clean)

<!-- Ground truth verified verbatim against origin/main @ 884b974 (local HEAD 1d364b2, same tree).
     Every seam cited below was re-read at this baseline: builtin.rs (native table + mock),
     adapter/mod.rs (resolve_native :609, resolve_start_launch :270, resolve_config_mapping :357,
     apply_config_mapping :437, StartLaunch :200, LaunchResolveError :218), registry.rs
     (adapter_launch_facts :788 + doc :780-787, effective_support :821, memory_status :705),
     supervisor.rs (launch preference :548, memory.dir strip :617, suspend_or_resume :1298,
     send_input :1456, plan_restart :1915, observed listener early-return :2262 area,
     invocation_overrides, honest snapshot, usage cursor BEFORE spawn), CI gates
     (ci.yml boundary allowlist :246/:269, OS-cfg gate :275-318, tarpaulin merge loop :595-645),
     scripts/test_automation.py :188-196 (asserts the exact tarpaulin crate-set line),
     fixture fake_agent.rs (unknown-arg tolerance `_ => {}`, flag composition, write_dump format :709),
     and every donor test file (crash.rs / pause.rs / metering.rs / adoption.rs / memory.rs /
     interaction.rs). -->

## Story

As an Operator,
I want to register, start, stop, and (per its declaration) pause the real NousResearch Hermes Agent,
So that the flagship agent is governed like any other. (FR-28 lifecycle half)

## Acceptance Criteria

Verbatim from `_bmad-output/planning-artifacts/epics.md` lines 542–555 (Story 6.2); GitHub issue #84, epic #59.

**Given** the ktesio-adapters-hermes native adapter with a per-OS Capability Declaration
**When** I register and start a Hermes instance
**Then** Hermes launches through its gateway model with unified config mapped to its native mechanism, transitions follow the standard state machine, and stop terminates the full process tree on all three OSes
**And** every Epic 1 lifecycle AC (FR-4..FR-9 consequences) passes against the Hermes adapter, with declared best-effort capabilities explicitly surfaced
**And** integration tests run sandboxed/recorded where network-bound (isolation strategy documented in the test module)

### Derived / consequence criteria (testable — from FR-28, FR-4..FR-9, CP-6.1-a…f, and the code state @ 884b974)

- **DC-1 (native launch resolution — the ONE engine behavior change).** A native kind may now carry a launchable `StartLaunch`. `builtin::native_launch(kind) -> Option<StartLaunch>` returns the code-declared launch; `adapter/mod.rs` threads it: `resolve_native` captures it into `ResolvedAdapter.launch` (replacing the hardcoded `launch: None` at :614-626) and `resolve_start_launch` consults it BEFORE erroring (`Some(launch)` short-circuits; `None` still yields `LaunchResolveError::NativeHasNoLaunch`). **Mock behavior byte-identical:** `mock` declares no launch, so every existing test (`supervisor.rs` :3431 inert-mock proof, `interaction.rs` :190 substitution note, `memory.rs` :366 parity leg) keeps passing UNCHANGED. `NativeHasNoLaunch`'s doc + error text are refreshed (mock remains THE no-launch example; hermes is the counter-example). No supervisor change: the persisted-launch preference (:548) already prefers the registration snapshot, so a hermes start flows through the exact existing spawn path.
- **DC-2 (HermesAdapter declared shape — CP-a ratified).** In `ktesio-adapters-hermes/src/lib.rs`: `kind() == "hermes"`; capabilities Pause BestEffort on {Linux, Macos, Windows} (never signal-pause — Hermes has no freeze mechanism; the engine's existing `pause-best-effort`/`resume-best-effort` qualifier causes surface it with ZERO engine change) and Interaction Guaranteed ×3; `MeteringSource::SelfReported`; `config_mapping()` maps ONLY the reserved `memory.dir` → env `HERMES_HOME` (CP-e+f composed, the mock→KTESIO_MEMORY_DIR precedent). **Gotcha:** `MEMORY_DIR_KEY` is `engine-domain-private` — the adapter crate MUST use the literal `"memory.dir"` string (export its own `pub const MEMORY_DIR_KEY_LITERAL`-style const locally). Unit tests cover all four surfaces (kind, per-OS levels, metering, mapping target name).
- **DC-3 (wiring + dependency edge, AD-2-clean).** Root `[workspace.dependencies]` gains `ktesio-adapters-hermes = { path = "crates/ktesio-adapters-hermes", version = "0.1.0" }` (first internal normal edge beyond engine/adapter-api); `crates/ktesio-engine/Cargo.toml` adds it under `[dependencies]`. `builtin.rs`: `"hermes" => Some(Box::new(ktesio_adapters_hermes::HermesAdapter::new()))` arm in `native()` + `HERMES_EXEC`/`HERMES_ARGS`/`HERMES_MEMORY_ENV_VAR` consts + `native_launch(kind)`; module-doc tense fixed (the "table is intentionally tiny this story (only mock)" / "register their kinds here … (epic 6)" lines become present tense). The CI boundary allowlist extends to `'ktesio-(engine|adapter-api|adapters-hermes)'` with its success message updated; the kt-side gate is untouched (kt still depends only on engine + adapter-api). Zero `#[cfg]` anywhere in the hermes crate (OS-cfg gate allowlists only `backends/` + 2 grandfathered kt files).
- **DC-4 (`model` key: silent no-op, Decision 6).** Hermes model switching is the `hermes model` CLI (env support unverified) → NO mapping for `model`. An operator-set `model` on a hermes instance is delivered nowhere (documented no-op); proven by a dump-absence assertion with a distinctive sentinel value.
- **DC-5 (`agent.*` pass-through stays free.)** No new reserved keys, no manifest-schema change, `CONTRACT_VERSION` stays `"0.4.0"` (code-declared native surface; no schema delta).
- **DC-6 (Epic-1 lifecycle AC pass matrix — the AC's "every Epic 1 AC" clause, made concrete).** Against `--kind hermes` end-to-end: FR-4 register+start→running (standard transitions `registered→starting→running`, events recorded); FR-5 stop terminates the FULL PROCESS TREE (`--spawn-child` + pids parsed from the public `read_agent_log`, both dead after stop — `pid_alive` pattern from adoption.rs, no survivor on ANY OS via the existing backends); FR-6 unrequested exit (exit 75, CP-b: the in-chat restart/update hand-off is JUST a non-zero exit — no special case) detected by the reaper → on-failure relaunch with the SAME persisted launch, `Restarted{count==1, waited_ms>=1000}`; FR-7 pause best-effort SURFACED (transition proceeds + `pause-best-effort` qualifier cause; resume symmetric `resume-best-effort`) — runs on ALL three OSes (no skip: hermes is BE everywhere); FR-8 send_input round-trip (`--echo-stdin` → `stdin:` line in agent.log); FR-22 self-reported usage lands in the ledger (`--emit-usage` sentinels, row-count polling, fleet totals equal ledger, `metering_source == "self-reported"` visible in Fleet detail).
- **DC-7 (sandboxed, network-free — the AC's isolation clause).** No network anywhere: the "real Hermes" binary in tests is a PATH shim — the `fake_agent` binary COPIED to `<tmp>/hermes<EXE_SUFFIX>` (fs::copy preserves perms) — because fake_agent tolerates unknown argv (`_ => {}`), so `hermes gateway run --external-supervisor` flows straight into its parser and every needed flag composes (`--dump` proves BOTH the gateway argv passthrough AND `HERMES_HOME` env in one committed artifact). **PATH-race decision:** `std::env::set_var("PATH", …)` is process-global (and unsafe under edition 2024) — ALL PATH-dependent phases live in ONE `#[test]` function (sequential phases over separate instances in ONE long-lived engine, the metering.rs keep-alive pattern); the shim dir is APPENDED via `std::env::join_paths` (never replaces PATH — `pid_alive`'s `kill`/`tasklist` invocations keep resolving). Tests needing NO spawn (registration surface, launch-resolution composition à la the mock leg) stay independent fns. The isolation strategy is DOCUMENTED in the test module doc.
- **DC-8 (HERMES_HOME delivery honesty).** Filesystem-backed hermes instance receives `env HERMES_HOME=<managed dir>` (dump-proven, argv-proven); an UNBACKED hermes instance receives NO `HERMES_HOME` (Hermes falls back to its own default chain — caveat documented in docs); the injected value never reaches `effective-config.json` (honest-provenance discipline already enforced at the start seam — pinned here).
- **DC-9 (docs currency, same change).** `docs/architecture.md` :21 final sentence (reserved-skeleton phrasing → hermes is the first launchable native adapter); `docs/commands.md` :29 (mock scoping sentence gains hermes-as-launchable-native); `registry.rs` :780-787 launch-facts doc ("None for a native adapter" → hermes carries one); `adapter/mod.rs` NativeHasNoLaunch + `resolve_start_launch` docs; `ci.yml` stale "doc-only stub" tarpaulin comments (~:612-637 wording refresh — the special-case branch stops triggering but must not lie). `scripts/test_automation.py` :192 asserts the exact tarpaulin crate-set line — UNCHANGED (hermes already listed).

## Ratified decisions (autopilot, from CP-6.1-a…f @ verification note §9)

- **CP-a → Pause BestEffort ×3** (DC-2). Machinery exists; zero engine change; surfaced-not-silent via existing qualifier causes.
- **CP-b → foreground `hermes gateway run --external-supervisor`; exit-75 hand-off needs NO special case** (DC-1/DC-6). Any non-zero exit while Running is a crash; on-failure policy relaunches with the SAME persisted launch. Consts: `exec = "hermes"`, `args = ["gateway", "run", "--external-supervisor"]`.
- **CP-e+f composed → RESERVED `memory.dir` → env `HERMES_HOME`** via the filesystem-backing invocation override (exact 5-1 mock precedent). Unbacked ⇒ no `HERMES_HOME` (default-chain caveat documented — DC-8). CP-f's broader "CLI arg vs generated config.yaml" question is settled FOR THIS KEY by the 5-1 mechanism; other keys arrive in 6-3.
- **`model` → no-op** (DC-4, Decision 6 rationale recorded).
- **Metering → SelfReported** (CP-d's $-cap absence confirms BudgetEvaluator stays additive; nothing to build here).

## Tasks / Subtasks (dependency-ordered; each names its AC/DC)

1. **Dependency edge + HermesAdapter crate body (DC-2, DC-3).**
   - Root `Cargo.toml` `[workspace.dependencies]` entry; engine `Cargo.toml` normal dep.
   - `ktesio-adapters-hermes/src/lib.rs`: `HermesAdapter` (all four surfaces, literal `"memory.dir"`, exported `HERMES_MEMORY_ENV_VAR`), unit tests ×4. Keep `publish = false`.
2. **Engine launch resolution (DC-1).**
   - `builtin.rs`: consts + `native_launch` + `"hermes"` arm + module-doc tense; mock untouched.
   - `adapter/mod.rs`: thread `native_launch` through `resolve_native` + `resolve_start_launch`; NativeHasNoLaunch/doc text refresh; NEW unit tests (hermes positive resolve at both seams, launch equality with consts, mock still None/NativeHasNoLaunch).
3. **Integration tests (DC-6, DC-7, DC-8) — `crates/ktesio-engine/tests/hermes.rs`.**
   - Module doc = the isolation strategy statement (shim + single-PATH-fn + committed-artifact polling, zero network).
   - No-spawn tests: registration surfaces the declaration (fleet/effective capabilities), memory-composition proof (mapping resolves + applies HERMES_HOME onto a bare launch exactly as start_inner would — memory.rs mock-leg shape).
   - The ONE PATH-dependent `#[test]`: phases over instances in one engine — start/echo/pause-BE/resume/stop-tree-kill; exit-75 crash → Restarted; usage rows + fleet totals + metering_source; backed ⇒ HERMES_HOME + argv dump proof; unbacked ⇒ no HERMES_HOME; model sentinel absent.
4. **CI allowlist + stale comments (DC-3, DC-9).**
   - Boundary grep allowlist + success echo; tarpaulin stub-comment refresh. Verify `cargo +stable tree -p ktesio -e normal,build --all-features` still shows NO new kt edge.
5. **Docs + gates (DC-9).**
   - architecture.md / commands.md / registry doc edits; full gate suite: fmt, clippy `-D warnings`, workspace tests, check_docs, tarpaulin ≥95 per-crate (hermes crate now has coverable lines — its unit tests must hold the bar).

## Dev Notes (ground truth @ 884b974)

- **Start seam order (supervisor.rs :521 start_inner)** — validate name → lookup → transition gate → `adapter_launch_facts` → persisted-launch preference (:548) → metering_source → Interaction level + pipe_stdin → effective_config → strip `memory.dir` (:617, KNOWN_KEYS validates operator sets fine; stripping is start-seam-only) → resolve_config_mapping → memory filter (Filesystem-only injection) → UTF-8/symlink guards + create_dir_all → observed_listener (SelfReported ⇒ None, :2262 area — hermes never needs an upstream URL) → invocation_overrides (base_url, memory_dir) → DC-10 notice → secrets → apply_config_mapping → HONEST snapshot (plain effective only) → restart policy read → log dir → usage cursor BEFORE spawn → starting transition → SpawnSpec → watch_startup (300ms window) → running. **Zero functional supervisor changes expected.**
- **Restart machinery**: `plan_restart` :1915 (never → terminal settle; crash-loop cap MAX_CONSECUTIVE_FAILURES=5; on-failure → backoff.delay_for, production 1s×2 cap 60s). `RestartPolicy{Never,OnFailure}` exported at crate root. Reaper ~250ms.
- **fake_agent fixture** (ktesio-conformance): unknown args ignored (`_ => {}`) — THE enabler; ready line `fake_agent ready pid=<n>` (+ `child-pid=<n>` when `--spawn-child`); `--dump` writes `arg=<token>` lines INCLUDING argv[0] then `env=KEY=VALUE` lines (:709); `--emit-usage N` sentinels 10-in/20-out 20ms apart; `--crash-after-ms 450` clears the 300ms readiness window; `--crash-times 1 --crash-state <path>` determinism (AI-49); `--echo-stdin` echoes `stdin: <line>`; `fake_agent_bin()` locates/builds target/<profile>/fake_agent; fs::copy preserves unix perms (shim construction).
- **Test-file conventions**: self-contained helpers per file (local `open(&TempDir)->Engine`, local manifest writers, `wait_until_state` 50ms polls); rusqlite COUNT polling `SELECT COUNT(*) FROM usage_events e JOIN agent_instances i ON i.id = e.instance_id WHERE i.name = ?1` (metering.rs :92); serde_json cause-substring assertions (`"\"kind\":\"pause-best-effort\"`); data-driven `if OsId::current()==Windows { return }` skips ONLY (no `#[cfg]` outside backends/ — none needed here); `pid_alive` + `/proc` zombie discount copied from adoption.rs :76/:105.
- **Composition proof template** (memory.rs :361-509 mock leg): `ConfigLayer::parse(SourceLayer::InvocationOverride, "<label>", "[memory]\ndir = '…'")` → `facade.effective_config(name, overrides)` → `ktesio_engine::adapter::resolve_config_mapping(kind, None)` → bare `StartLaunch` → `apply_config_mapping(&mut launch, &mapping, &effective, &BTreeMap::new(), home)` → assert `launch.env[target]`. For hermes the expected env-var NAME comes from the mapping itself (`target("memory.dir").and_then(|t| t.env_var())`), never a hardcoded string.
- **Boundary law (AD-2)**: kt's graph must stay `ktesio-engine` + `ktesio-adapter-api` only — the NEW edge is `ktesio-engine → ktesio-adapters-hermes`, which is why the CI allowlist (not kt's) changes. Conformance stays a dev-dep of engine tests only.
- **Toolchain quirk**: bare `cargo` hits the wrong toolchain via mise — always `cargo +1.96.1` locally (CI jobs pin explicitly). gh GraphQL rate-limited → REST.
- **Review precedent**: configured review subagents returned empty responses in 5-2 (six attempts, three modes) — lenses were executed INLINE with a documented deviation note; expect the same and plan for it.

### Review Findings

<!-- Populated by the BMAD review pass, 2026-08-30 (story 6-2 resume on feat/epic-6-hermes).
     Execution note: the first subagent round (general-purpose) again returned empty
     responses (matching the 5-2 precedent). The review was RE-RUN as subagents on a
     second round with the diff path + lens instructions embedded directly in the child
     prompts, per the user's explicit requirement that agents execute the review. All
     three lenses then executed as subagents: edge-case-hunter and verification-gap on
     the bmad-agent-dev agent, blind-hunter on bmad-agent-dev. Findings below come from
     those agent outputs; every load-bearing claim was independently re-verified against
     the code before triage (two blind-hunter claims were REFUTED, noted inline). -->

Review executed 2026-08-30 against diff `884b974…HEAD` (the PR #144 change set, 20 files).
Gate evidence captured the same day: `cargo fmt --all --check` OK; `cargo clippy
--workspace --all-targets -- -D warnings` OK; `cargo test --workspace --all-targets` OK
(all suites green, hermes.rs 4/4); `cargo tarpaulin --workspace --fail-under 95` →
**95.16% (4694/4933), green locally**; `python3 scripts/check_docs.py` OK (22 files).
Trunk-CI coverage shortfall remains tracked as open high-severity action item AI-71.

**Triage result: zero intent_gap, zero bad_spec, zero patch, zero defer.** Findings from
the three agent lenses, each independently re-verified before classification:

- **Edge-case hunter (agent) — 8 findings, all `reject`:**
  1. *Signal-death in shim collapses to exit 1* (`hermes_shim.rs:35` `status.code().unwrap_or(1)`) —
     real observation, but by design: the shim exists so the engine sees "just a
     non-zero exit"; crash-vs-launch-failure classification belongs to the real
     supervisor, and fake_agent's exit-75 contract is preserved. Cosmetic doc nit at most.
  2. *`HERMES_SHIM_ARGS` space-splitting vs exotic values* — test-only env var under
     test control; documented in the shim module doc. Reject.
  3. *`native()` vs `native_launch()` table-drift* — drift is compiler/test-pinned:
     `native_launch_carries_the_hermes_gateway_launch_and_nothing_for_mock` + the
     registration snapshot tests would fail. Reject.
  4. *`install_shim` assumes `deps/` layout of the test exe dir* — true of the test
     harness itself; a layout change breaks loudly at `fs::copy`, not silently. Reject.
  5. *`pid_alive` Err branch → vacuous `wait_until_gone`* — mirrors adoption.rs's
     proven pattern (same trade-off accepted there, AI-tracked in the 5-1 review).
     Reject (pre-existing pattern, not this story's regression).
  6. *Whole-tree OS-cfg allowlist* — already recorded below as an accepted deviation.
  7. *Hermes lcov merge-skip exception now unjustified* — the exception's comment in
     ci.yml was updated in the same change to say exactly this ("no coverable lines"
     wording retained only for the stub→code transition); coverage job green on trunk
     with hermes included (AI-71 shortfall is aggregate, tracked). Reject.
  8. *Persisted-snapshot launch staleness vs builtin-table edits* — Phase G proves
     restart-from-snapshot against the current table; cross-run drift requires a
     version bump the engine's schema already gates. Speculative. Reject.

- **Verification-gap hunter (agent) — `No verification gaps found.`** Every DC traced
  to a passing assertion (DC-1 → `resolve_native_hermes_captures…` + Phase-B verbatim
  argv; DC-2 → hermes-crate unit tests ×4 + builtin-table test; DC-4/DC-8 → Phase-B
  HERMES_HOME dump proof; DC-6 → Phases A–G; DC-7 → single-PATH-fn structure). The
  agent's two `Other findings` are real observations, both `reject`:
  - *Stale-helper freshness guard not extended to `hermes_shim`* (ci.yml removes/
    rebuilds `fake_agent` but never `hermes_shim`; no test_automation.py assertion).
    Real asymmetry with the repo's freshness doctrine, but exposure is narrow: the shim
    is a thin forwarder, Phase C/E/F assertions (`stdin: hello`, usage rows, child
    pids) prove the *scripted flags* took effect, so a stale shim with broken arg
    forwarding fails loudly. Worth a follow-up hardening note, not a story defect.
  - *OS-cfg allowlist comment names adoption suites that don't exist* (only
    `memory.rs:605` uses cfg under `crates/ktesio-engine/tests/`). True; the gate
    itself works and is asserted by test_automation.py:328. Comment-precision nit,
    already covered by the accepted-deviation note below.

- **Blind hunter (agent) — 24 findings, all `reject`; two load-bearing claims REFUTED
  on re-verification:**
  - *"Test file references symbols it never imports — does not compile"*: FALSE. The
    file uses fully-qualified paths (`ktesio_engine::Blocking<'_>` at hermes.rs:47,
    `Capability::Pause`/`SupportLevel::BestEffort` at :196/:200); the workspace test
    run (4/4 hermes tests) proves compilation.
  - *"cargo runs integration-test BINARIES serially by default, one thread each"* (the
    comment the agent attacked) is itself misquoted by the agent; the actual harness
    claim is about nextest's `engine-integration-serial` group — and the PATH-mutation
    safety argument holds regardless because only ONE test in the binary mutates PATH.
  - Remaining findings are real-but-cosmetic or pre-existing-pattern nits: dangling
    intra-doc link `[HERMES_MEMORY_ENV_VAR]` (confirmed locally: one rustdoc warning;
    no rustdoc gate in CI), `memory.dir` literal vs `MEMORY_DIR_KEY` cross-check
    inconsistency (both spellings verified equivalent — the mock test uses the
    constant, the hermes test the literal; a drift fails `hermes_memory_composition…`
    either way), PATH never restored (single-mutation invariant documented in the
    module doc), Phase B trusts synchronous stop before attach (attach's
    terminal-state-only contract is the guard), missing CHANGELOG entry (repo policy:
    CHANGELOG generated from git history at tag time — no entry owed), tasklist pid
    substring match (adoption.rs's accepted pattern), raw SQLite probe in Phase E
    (read-only against WAL, pattern proven in 3-1/3-5), no CLI-level `--kind hermes`
    test (CLI plumbing is clap-derived from the engine contract covered here),
    per-OS parity concerns for BestEffort/Guaranteed declarations (declared
    cross-OS by design; behavior tests run per-runner-OS as everywhere in this repo),
    and assorted style/robustness suggestions with no observable defect.

- **Deviations accepted as deliberate (recorded, not defects):** PATH prepend vs DC-7's
  "APPENDED" wording (safer, shim dir contains only the hermes/fake_agent copies);
  `hermes_shim` launcher added beside `fake_agent` in ktesio-conformance (the mechanism
  that keeps the fixed gateway argv contract intact while scripting flags); OS-cfg
  allowlist extended to `crates/ktesio-engine/tests/` (documents the pre-existing
  AI-35 unix-gated integration tests; asserted by `scripts/test_automation.py:328`,
  updated in the same change).

**Defer (1 finding):** none — candidates were rejected as noise or pre-existing
patterns, not deferred. **Sprint-status action items owed by this story: none new;
AI-71 remains the open trunk-coverage item and is explicitly out of this story's
scope.** Optional follow-up candidates for a future hardening story (not owed here):
extend the CI stale-helper guard to `hermes_shim`; fix the `HERMES_MEMORY_ENV_VAR`
doc link; cite nextest's serial group in the PATH-safety comment.

## Suggested Review Order

**The one engine behavior change — launchable native builtins (DC-1, DC-3)**

- The builtin table gains the `hermes` arm and its code-declared launch — the design intent in one stop.
  [`builtin.rs:74`](../../../crates/ktesio-engine/src/adapter/builtin.rs#L74)

- `resolve_native` captures the launch into the registration snapshot — start needs no special case.
  [`mod.rs:622`](../../../crates/ktesio-engine/src/adapter/mod.rs#L622)

- `resolve_start_launch` consults the code-declared launch BEFORE erroring; `mock` still errors honestly.
  [`mod.rs:275`](../../../crates/ktesio-engine/src/adapter/mod.rs#L275)

**The adapter declaration (DC-2, ratified CP-a/d/e+f)**

- HermesAdapter: Pause BestEffort ×3, Interaction Guaranteed ×3, SelfReported metering.
  [`lib.rs:194`](../../../crates/ktesio-adapters-hermes/src/lib.rs#L194)

- ONLY `memory.dir` → env `HERMES_HOME`; `model` deliberately unmapped (Decision 6).
  [`lib.rs:126`](../../../crates/ktesio-adapters-hermes/src/lib.rs#L126)

- The dependency edge (`engine → adapters-hermes`) that the CI boundary gate was widened for.
  [Cargo.toml:30](../../../crates/ktesio-engine/Cargo.toml#L30)

**The end-to-end proof (DC-6, DC-7, DC-8)**

- The isolation-strategy statement: PATH shim, zero network, one PATH-dependent test fn.
  [`hermes.rs:1`](../../../crates/ktesio-engine/tests/hermes.rs#L1)

- Phase B: one dump artifact proves the verbatim gateway argv AND the HERMES_HOME injection.
  [`hermes.rs:405`](../../../crates/ktesio-engine/tests/hermes.rs#L405)

- Phase G: exit-75 hand-off is just a crash — `Restarted{count==1, waited_ms>=1000}`.
  [`hermes.rs:642`](../../../crates/ktesio-engine/tests/hermes.rs#L642)

- The shim launcher that forwards contract argv then scripted flags into fake_agent.
  [`hermes_shim.rs:1`](../../../crates/ktesio-conformance/src/bin/hermes_shim.rs#L1)

**Peripherals**

- Gate updates: boundary allowlist, OS-cfg test allowlist, tarpaulin stub wording.
  [`ci.yml:258`](../../../.github/workflows/ci.yml#L258)

- Docs currency: architecture + commands now name hermes as the launchable native.
  [`architecture.md:17`](../../../docs/architecture.md#L17)
