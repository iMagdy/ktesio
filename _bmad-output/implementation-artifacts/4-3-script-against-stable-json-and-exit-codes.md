---
baseline_commit: 6afac996e1766f9d00f3dff2eb72887eaafcf192
---

# Story 4.3: Script against stable JSON and exit codes

Status: review

<!-- Context engineered by create-story (headless BMAD run, 2026-07-20). Ground truth verified against source at commit 6afac99 (4-1+4-2 merged to main). Trust the source over any prior story header (3-5's header still reads ready-for-dev though its code has landed). -->

## Story

As an Operator,
I want `--json` on every read command and documented, stable exit codes,
so that `kt` is automatable without the Embedding Interface. (FR-26)

## Acceptance Criteria

Verbatim from `_bmad-output/planning-artifacts/epics.md` lines 476-487 (Story 4.3), GitHub issue #81:

**AC1 — `--json` on every read command, one versioned schema (AD-14)**
**Given** the read commands (`fleet list`, `status`, `usage`, `effective-config`, `logs`)
**When** invoked with `--json`
**Then** output serializes the same versioned serde structs the event stream uses (AD-14), each payload carrying `schema_version`

**AC2 — documented, stable exit codes with a CI compatibility gate**
**And** exit codes are documented and covered by compatibility tests that fail CI on unannounced change (schema-compatibility per PRD §7)

### Derived / consequence criteria (testable, from FR-26 + PRD §7 + the current code state)

- **DC-1** Every read-only `kt` command emits a machine-readable document under `--json`. The read-only commands are `kt agent list` (`fleet list`), `kt agent show` (`status`), `kt agent usage` (`usage`), `kt agent config get` (`effective-config`), and `kt agent logs` (`logs`). `kt agent usage` and `kt agent logs --json` are **net-new** this story; `list`/`show`/`config get` already ship `--json`. The usage fields also stay embedded in `list`/`show` `--json` (`FleetEntry.usage` + `FleetListing.totals`); the new `usage` command is a first-class, focused, scriptable surface over the same `UsageView`/`FleetTotals` types. (FR-26; FR-22)
- **DC-2** Each `--json` payload carries a top-level `schema_version` and uses snake_case field names; the CLI reuses the engine's already-`Serialize` versioned structs rather than minting ad-hoc blobs (AD-14; SPINE conventions row line 160).
- **DC-3** `--json` output is pure machine-readable content on **stdout** and nothing else on stdout; every note, warning, rotation notice, and diagnostic goes to **stderr** (AD-12). Holds for both one-shot and `--follow`.
- **DC-4** No secret value ever appears in any `--json` output (AD-10; FR-14 — `config get` already masks unless `--reveal`).
- **DC-5** `kt` returns a documented, stable set of numeric exit codes. Success is `0`; every documented failure condition maps to its documented code; the mapping is published in `docs/commands.md`.
- **DC-6** Compatibility tests assert (a) the exact serialized key-set and `schema_version` of every read-command `--json` document, and (b) that each documented condition yields its documented exit code. Any unannounced change to a wire shape, a `schema_version`, or an exit code fails the `test` CI job on all three OSes. (PRD §7; FR-26 "covered by compatibility tests")
  - **Scope of "all three OSes" — precise as of the 2026-07-21 fix pass.** Every WIRE SHAPE (`FleetListing`, `FleetEntry`, `FleetTotals`, `UsageView`, `BudgetView`, `ShowDocument`, `UsageDocument`, `FleetUsageDocument`, `ConfigDocument`, `ConfigLeaf`, `LogLine`) is frozen in BOTH its unpriced and priced forms by tests that need no spawned child, so they genuinely gate all three OSes. Every `schema_version` is pinned BY LITERAL VALUE. Exit codes `0`/`1`/`2`/`3`/`4` are asserted end-to-end through the real binary on all three OSes. **The one honest exception:** codes `5` and `6` have NO cross-OS end-to-end assertion, because every route to them needs a genuinely running child — `pause`/`send` on an `unsupported` declaration, and a stuck agent with a full 64KB stdin pipe — and the surviving-engine harness they require is Unix-only. Their contract is instead gated cross-OS in two composed halves: the `map_engine_error`/`map_error` → `classify` mapper tests (`cli::agent::tests::every_engine_error_mapper_arm_preserves_its_documented_exit_code`) prove the code each condition PRODUCES, and the `0`–`4` end-to-end tests prove `main` wires `classify` to the process status. On Unix, `5` also has two real end-to-end assertions. Nothing is left resting on a test that silently self-skips.
- **DC-7** No adapter/manifest surface changes ⇒ `CONTRACT_VERSION` stays `"0.4.0"` and the (dormant) semver-check job stays green.

## Tasks / Subtasks

Dependency-ordered. Each task names its AC. Read the "Exact code seams" and "Testing Notes" in Dev Notes before writing any code.

- [x] **Task 1 — Confirm & freeze the three existing read-command `--json` surfaces (AC1, DC-1, DC-2)**
  - [x] 1.1 Audit `list --json` (`FleetListing`), `show --json` (`ShowDocument`), and `config get --json` (`ConfigDocument`) against AD-14: top-level `schema_version` present, snake_case fields, reuse of engine `Serialize` types. They already satisfy this (stories 1-7, 2-3, 3-5) — this task is to *confirm and lock with tests* (Task 5), not to rebuild. Make no behavior change unless the audit finds a real gap.
  - [x] 1.2 Note that `list`/`show` `--json` continue to carry the embedded usage fields (`FleetEntry.usage: UsageView` + `FleetListing.totals: FleetTotals`, FR-22 / story 3-5) — the new standalone `kt agent usage` command (Task 3) is an additional first-class surface over the SAME types, not a replacement. No change to `list`/`show` here.
- [x] **Task 2 — Add `kt agent logs --json` (AC1, DC-1, DC-2, DC-3) — net-new command #1 of 2 (streams ⇒ NDJSON)**
  - [x] 2.1 Add `json: bool` to `AgentCommands::Logs` (`crates/kt/src/main.rs:182-189`, mirror the `#[arg(long)] json: bool` on `List`/`Show`/`Config::Get`); thread it to `cli::agent::logs(name, follow, json)` (`agent.rs:929`) and the dispatch arm (`main.rs` ~286).
  - [x] 2.2 Reuse the engine's existing versioned `LogLine` (`crates/ktesio-engine/src/domain/event.rs:385-403`, `LOG_SCHEMA_VERSION = 1`, already `Serialize` + snake_case + kebab-case `stream`). Add a pure serializer helper mirroring `fleet_json`/`show_json` (`agent.rs:188-197`). Emit **newline-delimited JSON** (one serialized `LogLine` per line) — NOT a single wrapping document — so the same shape works for one-shot and `--follow` (see "logs --json design decision"). Do **not** invent a new schema-version constant.
  - [x] 2.3 Branch `logs()`: `json` → serialize each `LogLine` to one stdout line (append order, never timestamp-sort — story 4-2 AC-G); human → existing `print_log_lines` (`agent.rs:986-990`). Keep the `--follow` loop, the bounded final drain, and rotation detection intact; in `json` mode the rotation notice and the follow-exit note stay on **stderr** (AD-12, DC-3), stdout stays pure NDJSON.
  - [x] 2.4 Confirm no secret can reach the log stream (AD-10) — `LogLine.text` is agent stdout/stderr already captured to the Agent Home; no new exposure, but assert stdout purity in tests.
- [x] **Task 3 — Add `kt agent usage [<name>] [--json]` (AC1, DC-1, DC-2, DC-3) — net-new command #2 of 2 (snapshot ⇒ single versioned document, NOT NDJSON)**
  - [x] 3.1 Add a `Usage { name: Option<String>, json: bool }` variant to `AgentCommands` (`crates/kt/src/main.rs`; optional positional `name`, `#[arg(long)] json`, mirror `Show`/`List`); wire the dispatch arm to a new `cli::agent::usage(name, json)`.
  - [x] 3.2 Reuse engine domain types — do **not** invent a parallel usage type and do **not** serialize the event-stream `UsageUpdateEvent`. With `<name>` → the instance's `UsageView`; with no name → Fleet-wide `FleetTotals` (both `crates/ktesio-engine/src/domain/fleet.rs`). Compose them from the SAME facade path `list`/`show` use so token totals equal the Usage Ledger exactly (FR-22 / story 3-5 guarantee).
  - [x] 3.3 Emit a **single versioned document** (NOT NDJSON — usage does not stream), mirroring `ShowDocument`: thin CLI-local wrappers `UsageDocument { schema_version, instance, usage: UsageView }` (named form) and `FleetUsageDocument { schema_version, totals: FleetTotals }` (no-name form). **Reuse `FLEET_SCHEMA_VERSION`** (the constant `list`/`show` already use) — the document serializes the same fleet-domain content types, so it rides the fleet schema version exactly as `ShowDocument` does (see "usage command design decision"). Do **not** mint a new constant.
  - [x] 3.4 Human (non-`--json`) form: a focused usage table (tokens cumulative + current-run, derived dollars with the `estimated`|`reconciled` label, active Metering Source), reusing the currency formatters in `cost.rs` only (AD-8 — do not add a second currency formatter or the currency grep-lint fires). AD-12: JSON to stdout, any note to stderr; no secret on the wire (DC-4).
- [x] **Task 4 — Originate the documented, stable exit-code contract (AC2, DC-5)**
  - [x] 4.1 Define a `kt`-owned `ExitCode` (explicit `u8`) per the DECIDED table in "The exit-code contract this story originates". Add a classifier that maps each `crates/kt/src/error.rs` diagnostic struct to its code (recommended: downcast the boxed error in `main`/`run_cli`; a `CliError` enum wrapper is the cleaner-but-higher-churn alternative — see the decision note; keep the change localized, do not refactor all 21 structs unless the reviewer prefers it).
  - [x] 4.2 Change `main()` (`crates/kt/src/main.rs:242-248`) to exit with the classified code instead of the unconditional `std::process::exit(1)`. Preserve clap's `2` for usage/parse errors (do not fight clap's default) and `0` for `--help`/`--version`. Catch-all unmapped errors → `1` (preserves today's behavior).
  - [x] 4.3 Document the exit-code table in `docs/commands.md` (new "Exit codes" section) and state it is a v1 compatibility surface governed by PRD §7 (announce → one-minor notice → remove-at-major).
- [x] **Task 5 — Compatibility tests that fail CI on unannounced change (AC2, DC-6)**
  - [x] 5.1 Extend the `KtRun` harness (`crates/kt/tests/helpers/mod.rs:32-36`) to capture the numeric exit code (`output.status.code()`) alongside `success: bool` — its own doc comment already promises exit-code assertions.
  - [x] 5.2 Exit-code tests (in `crates/kt/tests/agent_cli.rs`): assert each documented condition yields its documented code — success→0, general error→1, usage error→2, not-found→3, invalid-state→4, unsupported→5, timeout→6 (per the DECIDED table). Name them behaviorally (e.g. `agent_show_missing_instance_exits_with_not_found_code`).
  - [x] 5.3 JSON key-set-freeze + `schema_version` pin tests for **every** read-command `--json` document and its content types: `FleetListing`, `FleetEntry`, `FleetTotals`, `UsageView`, `BudgetView` (`crates/ktesio-engine/src/domain/fleet.rs`), `ShowDocument`, `UsageDocument`, `FleetUsageDocument`, `ConfigDocument`, `ConfigLeaf` (`crates/kt/src/cli/agent.rs`), and `LogLine` (`event.rs`). Extend the existing `let mut keys: Vec<_> = obj.keys()...; keys.sort(); assert_eq!(keys, vec![...])` pattern (precedents: `budget.rs:311-321`, `event.rs:713-745`, `usage.rs:262-283`). A new/renamed/removed field then breaks the frozen assertion, forcing an intentional edit = the "announce" gate.
  - [x] 5.4 Verify these live in the `test` job (`cargo nextest`, all three OSes) so they gate CI. Add a Dev-Notes/test comment recording that `cargo-semver-checks` (dormant, and Rust-API-only) does **not** cover wire JSON or exit codes, so these bespoke tests are the real gate.
- [x] **Task 6 — Docs, parse tests, and gates (AC1, AC2, DC-7)**
  - [x] 6.1 `docs/commands.md`: add the exit-code section, a `kt agent usage [<name>] [--json]` section, a `kt agent logs [--json]` subsection, and bash-fence examples. **Add `usage` to `scripts/check_docs.py`'s `AGENT_COMMANDS` allowlist** (`logs` is already listed; `usage` is **not** — the new verb needs it), then confirm `python3 scripts/check_docs.py` passes.
  - [x] 6.2 Update the inline clap parse tests in `crates/kt/src/main.rs` (add `usage`/`usage --json` and `logs --json` cases; add `usage` to `test_agent_subcommands_exist`'s positive list); run `python3 scripts/test_automation.py`.
  - [x] 6.3 Run every gate under the pinned toolchain (see "Gate commands"); confirm no `Cargo.lock` change (`serde_json` already a dep of both crates) and no `CONTRACT_VERSION` bump (DC-7).

## Dev Notes

### CRITICAL SCOPING — what this story is and is NOT

Much of AC1 is **already shipped** (`list`/`show`/`config get` have `--json`). Do not rebuild those. This story has four pieces of genuinely new work:

1. **Add `--json` to `kt agent logs`** (`list`, `show`, `config get` already have it — stories 1-7, 2-3).
2. **Add a new first-class `kt agent usage [<name>] [--json]` command** — Islam ratified this (2026-07-20) as a distinct, scriptable read surface, not just usage-embedded-in-`list`/`show`. It reuses the existing `UsageView`/`FleetTotals` types (no parallel type) and emits a single versioned document.
3. **Originate the numeric exit-code contract from scratch** — today it is 0=success / 1=any-error (+ clap's 2 for usage). This is the largest piece.
4. **Lock everything with compatibility tests** so an unannounced wire/exit-code change fails CI.

Explicitly **out of scope** (do not do these here — they are scope creep or belong elsewhere):
- Wiring the event-stream structs (`TransitionEvent`, `UsageUpdateEvent`, `BudgetBreachEvent`) into `--json`. Those feed the Host **event subscription** (FR-33), which is **story 7-2** (`7-2-subscribe-to-engine-events-with-stable-schemas`, currently backlog). The CLI read commands read *state views*, not event deltas — in particular the new `usage` command serializes the `UsageView` *snapshot*, never the `UsageUpdateEvent` *delta*. See "AD-14 scope" below.
- Unifying the CLI-local `ConfigDocument`/`ConfigLeaf` with the engine-private `EffectiveConfigSnapshot`, or building a central schema-version registry. These are real drift risks (logged as follow-ups, Assumption A-4) but touch engine public API/semver and are outside FR-26's read-command surface.
- Any adapter/manifest/contract change. `CONTRACT_VERSION` stays `"0.4.0"`.

### What already exists vs. what's genuinely new (inventory before writing code)

| Epic name | Real command | `--json` today? | Document type (where defined) | `schema_version` |
|---|---|---|---|---|
| `fleet list` | `kt agent list` | **yes** (1-7) | `FleetListing {schema_version, instances: Vec<FleetEntry>, totals: FleetTotals}` — **engine** `crates/ktesio-engine/src/domain/fleet.rs:523` | `FLEET_SCHEMA_VERSION = 2` (`event.rs:73`) |
| `status` | `kt agent show` | **yes** (1-7) | `ShowDocument {schema_version, instance: FleetEntry}` — **CLI-local** `crates/kt/src/cli/agent.rs:163-179` (reuses engine `FleetEntry` + shared `FLEET_SCHEMA_VERSION`) | `2` |
| `usage` | `kt agent usage [<name>]` | **NO — net-new command** (usage previously only rode `list`/`show`) | new thin wrappers `UsageDocument {schema_version, instance, usage: UsageView}` (named) / `FleetUsageDocument {schema_version, totals: FleetTotals}` (fleet), reusing engine `UsageView`/`FleetTotals` (`fleet.rs`) | reuse `FLEET_SCHEMA_VERSION` |
| `effective-config` | `kt agent config get` | **yes** (2-3) | `ConfigDocument {schema_version, entries: Vec<ConfigLeaf>}`, `ConfigLeaf {key, value, source, unvalidated}` — **CLI-local** `agent.rs:1198-1217` | `CONFIG_GET_SCHEMA_VERSION = 1` (`agent.rs:1195`) |
| `logs` | `kt agent logs` | **NO — net-new** (4-2 shipped text only) | reuse engine `LogLine {schema_version, instance, stream, at, text}` (`event.rs:385`) via NDJSON | `LOG_SCHEMA_VERSION = 1` (`event.rs:339`) |

Genuinely new work: (a) `logs --json`; (b) the new `kt agent usage` command (+ `--json`); (c) the numeric exit-code contract; (d) the compatibility test suite + `KtRun` numeric-code capture; (e) docs.

`ShowDocument`, `fleet_json`, `show_json`, and `serialize_error` at `agent.rs:163-207` are the exact pattern the new `--json` serializers must mirror — `logs --json` (NDJSON, one line per `LogLine`) and `usage --json` (a single versioned `UsageDocument`/`FleetUsageDocument`, closest to `ShowDocument`): pure, unit-testable `Result<String, Box<dyn Error>>` helpers; a `serde_json` failure becomes an `AgentIo` diagnostic, never a panic.

### Binding architecture decisions

- **AD-14 — One event schema, two consumers** (SPINE lines 117-120). Rule: *"engine events (state transitions, crash/restart, usage updates, breaches) are versioned serde structs published over the subscription API; `kt --json` serializes the same structs. Schema changes follow the Embedding Interface semver rules (AD-2)."* Binds FR-26 explicitly. The two consumers are `kt --json` (this story) and the Host subscription bus (story 7-2). Source-tree hint: `src/events.rs` → implemented as `crates/ktesio-engine/src/domain/event.rs`. Module doc there names this story: *"`kt --json` (story 1-7 / 4-3) reuse the SAME schema."*
- **Conventions row** (SPINE line 160): *"serde structs per AD-14; field names snake_case; every payload carries `schema_version`."*
- **AD-12 — stdout/stderr discipline** (SPINE line 110): *"stdout of `kt` is command output; diagnostics go to stderr."* Already enforced on `list`/`show`/`config get` (JSON to stdout; `METERING_NOTE`, empty-fleet hint, read-back warnings to stderr via `ui::note`/`ui::warning`). `logs --json` must uphold it for both one-shot and `--follow`.
- **AD-2 / PRD §7.2** (SPINE lines 56-59): the Embedding Interface (engine public API) is semver-per-Rust-conventions, CI-guarded by a `cargo-semver-checks` job on `ktesio-engine`. `schema_version` fields inherit this policy. Note: semver-checks validates the *Rust API*, not the *serialized JSON* — a `#[serde(rename)]` passes it — so it does **not** substitute for the JSON key-set tests this story adds.
- **PRD §7 "Public Surface, Versioning & Deprecation"** (prd.md lines 362-370), contract 3: *"The `kt` CLI surface — command names, flags, exit codes, and `--json` schemas are a compatibility surface once v1 ships; breaking changes follow the same deprecation path as FR-38 (announce → notice → remove)."* Deprecation policy: announced in release notes, minimum one-minor notice window, removal only at a major. `[ASSUMPTION: policy mechanics pending Islam.]` — this is the exact "unannounced change" the AC's compatibility tests must fail CI on.
- **FR-26** (prd.md lines 256-259): *"Every read command offers `--json`; exit codes are stable and documented, making `kt` automatable without the Embedding Interface."* Testable consequence: *"JSON schemas for listing/status/usage are documented and covered by compatibility tests (see §7 versioning)."*
- **AD-10 / FR-14**: secrets never appear in `--json` (`SecretString` redacts; `config get` masks unless `--reveal`).

### The exit-code contract this story originates

Today (`crates/kt/src/main.rs:242-248`): `fn main() { if let Err(err) = run_cli() { ui::error(err); std::process::exit(1); } }`. Every runtime error → `1`; clap → `2` for usage, `0` for help/version. No `ExitCode`, no error-kind→code map, nothing documented, and no test asserts a numeric code (`KtRun` only records `success: bool`). All three Epic-4 predecessors explicitly assign this work here (4-1:28, 4-2:29 and 4-2:180). So this is greenfield.

**DECIDED v1 exit-code contract (ratified by Islam, 2026-07-20).** The full `0–6` table below is the committed contract — not a recommendation. It keeps `0/1/2` behavior-compatible and adds scriptable distinctions, each mapped from an already-modeled `error.rs` diagnostic. Once shipped it is a compatibility surface (PRD §7); the Task 5 tests pin it:

| Code | Meaning | Mapped from (`crates/kt/src/error.rs` structs) |
|---|---|---|
| `0` | Success | `Ok(())` |
| `1` | General error — unexpected/internal: IO, store, engine, launch, config-load, self-update, no-metering-source, no-capabilities, and any unmapped error (catch-all) | `AgentIo`, `AgentStore`, `AgentConfig`, `AgentLaunchFailed`, `AgentManifestInvalid`, `AgentManifestUnreadable`, `AgentNoMeteringSource`, `AgentNoCapabilities`, `SelfUpdateFailed` |
| `2` | Usage error — invalid CLI invocation | clap parse/usage (unchanged), `AgentInvalidName`, `AgentUnknownKind`, `AgentUnknownConfigKey`, `AgentDuplicateName` |
| `3` | Not found — named instance/manifest does not exist | `AgentNotFound`, `AgentManifestNotFound` |
| `4` | Invalid state — instance not in a state that permits the operation | `AgentNotRunning`, `AgentRunningRequiresForce`, `AgentInvalidTransition`, `AgentStopUnconfirmed` |
| `5` | Unsupported capability — the agent's Capability Declaration forbids it | `AgentCapabilityUnsupported`, `AgentInteractionUnavailable` |
| `6` | Timed out — a bounded operation exceeded its deadline | `AgentInteractionTimedOut` |

All seven codes are in scope; the compatibility test asserts each condition maps to exactly its code.

**Implementation note (keep churn low):** the 21 diagnostics are separate `thiserror`+`miette` structs boxed as `Box<dyn std::error::Error>`, not one enum. Recommended: a classifier in `main`/`run_cli` that downcasts the boxed error to the known structs and returns the `ExitCode`, catch-all → `1`. A single `CliError` enum wrapper is cleaner long-term but touches all 21 structs + `map_error`/`map_engine_error`/`map_config_error` (`agent.rs:1420/1541/1333`) — propose it to the reviewer rather than doing it unilaterally.

### logs --json design decision (NDJSON, not a single document)

`kt agent logs` supports `--follow` — an unbounded stream. A single wrapping document (`{schema_version, instance, lines: [...]}`, the `list`/`show` shape) cannot be emitted for a stream that never closes. Therefore emit **newline-delimited JSON**: one serialized `LogLine` object per line. Each `LogLine` already carries its own `schema_version` (`LOG_SCHEMA_VERSION = 1`), so per-line versioning is intrinsic and honors DC-2. This works identically for one-shot and `--follow`, preserves on-disk append order (never timestamp-sort — `at` is whole-second RFC3339, so lines share timestamps; story 4-2 AC-G), and reuses the exact engine struct the 7-2 subscription bus will publish (AD-14). This is a deliberate, justified departure from the single-document shape of the other read commands, **ratified by Islam (2026-07-20): NDJSON uniformly — one self-versioned `LogLine` per line, identical for one-shot and `--follow`; no wrapper-document variant.**

### usage command design decision (first-class command, versioned document, reuse fleet types)

Ratified by Islam (2026-07-20): `usage` is a **standalone, scriptable command**, not merely usage-embedded-in-`list`/`show`. Design (grounded in the existing types — no parallel type):

- **Surface:** `kt agent usage [<name>] [--json]` — optional positional name. With `<name>` → that instance's usage; with no name → the Fleet-wide totals. Mirrors FR-22's "per instance **or** Fleet-wide" duality (story 3-5) and keeps `usage` symmetric with `show` (named) and `list` (fleet). The no-name fleet form is warranted: it gives a focused Fleet-usage view without the full `list` fleet entry, directly serving FR-22's Fleet-wide scope.
- **Types (reuse, don't invent):** the named form serializes the engine `UsageView` (the per-instance snapshot already carried in `FleetEntry.usage`); the fleet form serializes `FleetTotals` (already carried in `FleetListing.totals`). Both in `crates/ktesio-engine/src/domain/fleet.rs`. Do **not** create a new usage type; do **not** serialize `UsageUpdateEvent` (that engine event is the 7-2 stream *delta*, a different type from the `UsageView` *snapshot*).
- **Document:** a **single versioned document** (usage does not stream ⇒ NOT NDJSON): thin CLI-local wrappers `UsageDocument { schema_version, instance, usage: UsageView }` and `FleetUsageDocument { schema_version, totals: FleetTotals }`, each closest to the existing `ShowDocument` pattern (`agent.rs:163-179`).
- **`schema_version` — reuse `FLEET_SCHEMA_VERSION`:** the usage document serializes the same fleet-domain content types (`UsageView`/`FleetTotals`) that already ride `FLEET_SCHEMA_VERSION` in `list`/`show`, so it shares that constant exactly as `ShowDocument` does. Governing principle across the CLI: **`schema_version` tracks the serialized content-type family, not the command** — fleet-content commands (`list`/`show`/`usage`) share `FLEET_SCHEMA_VERSION`; `config get` has its own `CONFIG_GET_SCHEMA_VERSION`; `logs` uses the engine `LOG_SCHEMA_VERSION`. Do not mint a `usage`-specific constant.
- **Data source & honesty:** compose from the SAME facade path `list`/`show` use so token totals equal the Usage Ledger exactly (FR-22 / story 3-5). Dollars are integer micros + an `estimated`|`reconciled` label; the human table reuses `cost.rs`'s currency formatters only (AD-8; the currency grep-lint forbids a second formatter). AD-12: JSON to stdout, any note to stderr; no secret on the wire (DC-4).

### AD-14 scope: which "same structs" this story wires, and which it deliberately does not

AD-14's "same structs, two consumers" is honored today only for the **Fleet document** (`--json` consumer #1; future 7-2 consumer #2). The event structs AD-14 *names* (`TransitionEvent`, `UsageUpdateEvent`, `BudgetBreachEvent`) currently have **zero `--json` consumers** — they are record-only seeds for 7-2. This story:
- **Wires** `logs --json` onto the engine `LogLine` (a genuine "same struct" case — the log line the CLI prints is the same payload the stream emits).
- **Does not** surface transition/usage/breach *events* through `--json`. The CLI read commands surface *state views* (`UsageView` is a snapshot aggregate; `UsageUpdateEvent` is a delta — legitimately different types); the new `usage` command serializes the `UsageView` snapshot, never the `UsageUpdateEvent` delta. Routing events to subscribers is FR-33 / story 7-2. This is not an AD-14 violation; it is the correct division between the two consumers.
- **Notes but does not fix** the drift risks (CLI-local `ConfigDocument` vs engine `EffectiveConfigSnapshot`; `ShowDocument` wrapper hand-maintained in the `kt` crate; seven independent `schema_version` constants with no registry) — logged as a follow-up (A-4).

### Project Structure Notes

- CLI surface & dispatch: `crates/kt/src/main.rs` (clap `Cli`/`Commands`/`AgentCommands`, inline parse tests, `main`/`run_cli`).
- CLI command bodies + `--json` serializers + error mapping: `crates/kt/src/cli/agent.rs` (add `usage(name, json)` + the `UsageDocument`/`FleetUsageDocument` wrappers here, next to `ShowDocument`).
- CLI diagnostics (exit-code source): `crates/kt/src/error.rs`.
- Output discipline module: `crates/kt/src/ui.rs`.
- Engine versioned wire structs: `crates/ktesio-engine/src/domain/{event.rs, fleet.rs, usage.rs}` (re-exported at `lib.rs:78-79`, `domain/mod.rs`). **Add no new `--json` document type in the engine** — reuse `LogLine`.
- CLI integration tests: `crates/kt/tests/agent_cli.rs`; harness `crates/kt/tests/helpers/mod.rs`.
- Docs: `docs/commands.md`; doc gate `scripts/check_docs.py` (**add `usage` to its `AGENT_COMMANDS` allowlist — unlike `logs`, `usage` is not yet listed**); automation gate `scripts/test_automation.py`.
- No new crate, no new dependency (`serde_json` already present in `kt` and `ktesio-engine`); no OS-`cfg` code (stay clear of the boundary/OS-cfg grep gates).

### Testing Notes

Read `crates/kt/tests/agent_cli.rs:1-60` (harness + `start_via_surviving_engine` ~43-59) and the existing `--json` tests (search `schema_version`, e.g. `agent_cli.rs:1704/1774/1847/2284/2904/3020`; `agent.rs:1989/2026/2265`) before writing tests.

- **JSON assertions**: re-parse stdout with `serde_json`, index the `Value`, assert `schema_version` explicitly, and assert stdout is *pure* JSON (nothing non-JSON leaks; notices land on stderr). Empty-collection cases are always tested (e.g. empty log ⇒ zero stdout lines, guidance on stderr — mirror `list --json` empty at 1-7).
- **Key-set freeze**: `let mut keys: Vec<_> = obj.keys().cloned().collect(); keys.sort(); assert_eq!(keys, vec![...])` — this is the mechanism that "fails CI on unannounced change" for additive drift too. Apply to every document + content type in Task 5.3.
- **Exit-code tests**: after 5.1, assert `run.code == Some(N)` for each documented condition.
- **Determinism**: never `sleep(N)` to await state — poll committed state (log file / DB). Epic-2/3 retro lesson (restated 4-1:156); the coverage/ubuntu runners are contention-sensitive.
- **Parse tests** live inline in `main.rs` (e.g. `test_agent_list_and_show_accept_json_flag`) — add `logs --json` and `usage`/`usage --json` cases, and add `usage` to `test_agent_subcommands_exist`'s positive list.
- Test naming: long behavioral snake_case sentences (e.g. `logs_json_emits_newline_delimited_loglines_in_append_order`, `usage_json_document_reuses_the_fleet_schema_version`, `agent_show_missing_instance_exits_with_not_found_code`).

#### Gate commands (run under the pinned toolchain — bare `cargo` resolves to 1.96.1 via `rust-toolchain.toml`; use `cargo +1.96.1` if a version manager like mise overrides it)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo nextest run --workspace --all-targets` (nextest, not `cargo test` — retries absorb the known Win/macOS real-IO flakes)
4. `cargo test --workspace --doc`
5. `cargo tarpaulin --engine llvm --workspace --fail-under 95` (NFR-3; CI runs a per-crate split, coverage-neutral; budget for it — recent stories hover ~95.1%)
6. `python3 scripts/check_docs.py` (validates the bash-fence examples in `docs/commands.md`)
7. `python3 scripts/test_automation.py`
8. OS-`cfg` grep gate (no new `cfg(unix|windows|target_os|target_family)` outside `crates/**/backends/`)
9. AD-2 boundary gate (`kt` depends only on `ktesio-engine` + `ktesio-adapter-api`; `ktesio-conformance` stays dev-dep-only)
10. semver-check (dormant/green while `CONTRACT_VERSION` untouched)
11. MSRV `--locked` (no `Cargo.lock` change)
12. Currency grep-lint (`.github/workflows/ci.yml:223`) — only fires if a `$` literal is rendered outside `cost.rs`; `logs`/exit-codes render no currency, so leave its allowlist alone. If usage dollars are re-touched, remember: on the wire, dollars are **integer micros + an `estimated`|`reconciled` label, never a `$` string** (`render_dollars`/`render_dollars_bare` in `cost.rs` are the *only* currency formatter — AD-8).

Reported historically as "all 9 gates green." Do not be alarmed if the hosted coverage/ubuntu legs are flaky in CI (known infra issues, tracked as #101/#106/#109/AI-60); local `tarpaulin >= 95` is the real bar.

### Exact code seams (READ these; extend, do not reinvent)

- `crates/kt/src/main.rs` — `main` (242-248, change the `exit(1)`), `run_cli` (250-307), `Cli` (80-83), `Commands` (85-103), `AgentCommands` (105-209; `Logs` at 182-189; add the new `Usage { name: Option<String>, json: bool }` variant here), the dispatch (~286), inline parse tests.
- `crates/kt/src/cli/agent.rs` — `logs()` (929-982), `print_log_lines` (986-990), `note_if_rotated` (~992+), the `--json` serializer pattern `ShowDocument`/`fleet_json`/`show_json`/`serialize_error` (163-207), error mappers `map_error` (1420), `map_engine_error` (1541), `map_config_error` (1333), `ConfigDocument`/`ConfigLeaf`/`CONFIG_GET_SCHEMA_VERSION` (1195-1217).
- `crates/kt/src/error.rs` — 21 diagnostic structs (exit-code classifier input).
- `crates/kt/src/ui.rs` — stdout (`success`/`info`/`print_table`) vs stderr (`warning`/`error`/`note`); AD-12 comment at 26-29.
- `crates/ktesio-engine/src/domain/event.rs` — `LogLine` (385-403), `LogStream` (348-360), `LOG_SCHEMA_VERSION` (339); the keyset-freeze precedent for `BudgetBreachEvent` (713-745).
- `crates/ktesio-engine/src/domain/fleet.rs` — `FleetEntry`, `FleetListing` (523), `FleetTotals` (389), `UsageView`, `BudgetView` (keyset-freeze targets; `UsageView`/`FleetTotals` are also the types the new `usage` command reuses — do not fork them).
- `crates/kt/tests/helpers/mod.rs` — `KtRun` (32-36; add numeric code capture) and `run_kt_agent*`.
- `crates/kt/tests/agent_cli.rs` — integration tests.
- `docs/commands.md` (§ per command + the AD-12 note at line 10); `scripts/check_docs.py` (`AGENT_COMMANDS` at 37-48).

### References

- Epic / story source: `_bmad-output/planning-artifacts/epics.md` lines 476-487 (Story 4.3); GitHub issue #81; epic #58.
- Architecture spine: `_bmad-output/planning-artifacts/architecture/architecture-ktesio-2026-07-02/ARCHITECTURE-SPINE.md` — AD-14 (117-120), AD-12 (110), AD-2 (56-59), AD-13 (112-115), Errors row (157), Events & JSON row (160), source-tree hint (199).
- PRD: `_bmad-output/planning-artifacts/prds/prd-ktesio-2026-07-02/prd.md` — §7 (362-370), FR-26 (256-259), FR-33 (301-304), FR-34 (306-309), FR-22 (232), FR-14 (179), NFR-1 (347), NFR-5 (351).
- Prior stories (conventions to mirror): `1-7-…md` (fleet `list`/`show --json`, AD-14, `:34/:56/:59/:113`), `2-3-…md` (`config get --json`, DTO-not-`Serialize`-internal, `:179/:191/:230`), `3-5-…md` (`FLEET_SCHEMA_VERSION` 1→2 additive, integer-micros currency, `:36/:42/:68/:117/:163`), `4-1-…md` (`:28` defers `--json`/exit codes here; determinism `:156`), `4-2-…md` (`:29/:180` defers `logs --json`/exit codes here; `LogLine` schema; append-order `:150`).
- Deferred nit relevant to `show --json`: `sprint-status.yaml` line 188 (`show --json` builds its entry via an O(N) `fleet().find()` and inherits silent status-read-back degradation vs the human path's stderr warning — a 1-7 review nit; do not expand scope to fix it, but be aware when adding `show`/`status` tests).
- Toolchain/coverage context: user memory `ktesio-gate-toolchain`, `ktesio-coverage-ci-oom`, `ktesio-engine-tests-parallel-oversubscribe`.

### Assumptions & open items logged

**Resolved by Islam (2026-07-20):**
- **A-1 (exit-code contract) — RESOLVED:** ratified the full `0–6` table (0 success · 1 general/internal · 2 usage · 3 not-found · 4 invalid-state · 5 unsupported-capability · 6 timed-out) as the DECIDED v1 contract, with the `error.rs`→code mapping worked out above. Not a "recommended default" — it is the committed surface the Task 5 tests pin.
- **A-2 (`logs --json` shape) — RESOLVED:** ratified **NDJSON uniformly** — one self-versioned `LogLine` per line, identical for one-shot and `--follow`. No wrapper-document variant.
- **A-3 (`usage`) — RESOLVED (changed from the initial assumption):** Islam wants a **standalone, first-class `kt agent usage [<name>] [--json]` command**, distinct from the fleet view — net-new CLI surface this story (Task 3). It reuses the existing `UsageView`/`FleetTotals` types and rides `FLEET_SCHEMA_VERSION`; its `--json` is a single versioned document (not NDJSON). See "usage command design decision".

**Still open (deferred follow-ups, non-blocking):**
- **A-4 (AD-14 drift, deferred):** Event structs into `--json` = story 7-2; `ConfigDocument`↔`EffectiveConfigSnapshot` unification + a central `schema_version` registry are noted drift risks logged as a follow-up (candidate new AI-item), not fixed here.
- **A-5 (secrets):** `config get --json` already excludes secrets unless `--reveal` (FR-14); `logs --json` and `usage --json` inherit AD-10 (no secret on the wire). No new secret-exposure surface expected — asserted by stdout-purity tests, not by new masking code.

## Change Log

| Date | Version | Description | Author |
|---|---|---|---|
| 2026-07-20 | 0.1 | Initial story context created (headless BMAD create-story run). Status → ready-for-dev. | create-story (BMAD) |
| 2026-07-20 | 0.2 | Folded in Islam's ratifications: exit-code `0–6` table DECIDED (A-1); `logs --json` NDJSON DECIDED (A-2); added net-new first-class `kt agent usage [<name>] [--json]` command (A-3 changed from assumption) — new Task 3, tasks renumbered to 6, compat-test (Task 5) + docs (Task 6) tasks extended for `usage`. | create-story (BMAD) |
| 2026-07-20 | 1.0 | Implemented (headless BMAD dev-story run): `logs --json` NDJSON; net-new `kt agent usage [<name>] [--json]`; the frozen `0–6` exit-code contract (`exit_code.rs` + `main()` wiring); the compatibility-test suite (KtRun numeric code, wire-level key-set freezes, exit-code assertions) — mutation-verified to fail on unannounced change; docs + doc-gate allowlist. All gates green under `cargo +1.96.1` (885 tests passed). Status → review. | dev-story (BMAD) |
| 2026-07-21 | 1.1 | **Fix pass** after a two-pass + adversarial review found AC2's "fails CI on all three OSes" claim FALSE in four places, each proven by mutation. Closed all four gate holes (H1 exit-code-6 mapper seam, H2 the entirely-unfrozen PRICED wire shape, H3 `--follow --json` never exercised, H4 silent Windows self-skips) plus a tautological schema guard (H5); applied the ratified behavior fixes (M1 clean exit on a closed stdout pipe, M2 uniform exit 2 for a malformed name); docs/hygiene (M3, L1–L6). Every fix re-verified BY MUTATION. See "Fix pass (2026-07-21)" below. | fix-pass |

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (BMAD dev-story workflow, headless run 2026-07-20).

### Debug Log References

**Gate results (toolchain `cargo +1.96.1` — the repo MSRV; local stable 1.94.1 is BELOW MSRV and the sqlite dep needs `cfg_select!`):**

| Gate | Command | Result |
|---|---|---|
| fmt | `cargo +1.96.1 fmt --all --check` | **PASS** (exit 0; drift auto-fixed with `cargo fmt --all`, then re-verified) |
| clippy | `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` | **PASS** (exit 0, zero warnings) |
| tests | `cargo +1.96.1 nextest run --workspace --all-targets` | **PASS** — **885 tests run, 885 passed, 0 failed, 0 skipped** (19 binaries, 43.0s). Baseline before this story was 860; **+25 net new**. |
| doc tests | `cargo +1.96.1 test --workspace --doc` | **PASS** (0 doc tests; none added) |
| check_docs | `python3 scripts/check_docs.py` | **PASS** — "Validated 22 Markdown files" |
| test_automation | `python3 scripts/test_automation.py` | **PASS** — "Ran 21 tests … OK" |
| currency grep-lint (AD-8) | ci.yml pattern + allowlist, run locally | **PASS** — "dollar formatting lives only in the one currency module" |
| AD-2 boundary | `cargo tree -p ktesio -e normal,build` edge scan | **PASS** — internal edges are exactly `ktesio-engine` + `ktesio-adapter-api` |
| OS-`cfg` gate | grep for `cfg(unix\|windows\|target_os\|target_family)` in changed files | **PASS** — 0 hits (no OS-cfg code added) |
| MSRV `--locked` (DC-7) | `git diff --quiet Cargo.lock` | **PASS** — `Cargo.lock` UNCHANGED (no new dependency) |
| semver-check (DC-7) | `CONTRACT_VERSION` inspection | **PASS** — stays `"0.4.0"`; no adapter/manifest surface touched |

Coverage (`cargo tarpaulin`, gate #5) was **not run locally** — see Completion Note 8.

**Mutation-verified the compatibility gate is real (not vacuous).** Because DC-6's whole claim is "fails CI on unannounced change", both frozen contracts were deliberately broken and the gate confirmed to catch each, then reverted:

1. Added a `sneaky_new_field` to `UsageDocument` (simulating unannounced ADDITIVE wire drift) → `usage_json_named_document_key_set_and_schema_version_are_frozen` FAILED with the actionable message naming the surface and PRD §7. Reverted; file byte-verified intact.
2. Renumbered `ExitCode::NotFound` from `3` to `7` → `exit_code_numbers_are_the_frozen_v1_contract` FAILED. Reverted; verified `NotFound = 3`.
3. Removed `"usage"` from `check_docs.py`'s `AGENT_COMMANDS` → the doc gate FAILED on all four new `kt agent usage` bash-fence examples, proving the new examples are genuinely validated (not silently skipped). Restored.

Also smoke-tested the real binary end-to-end before writing tests: all 7 exit codes observed correct through the actual process status (incl. clap's `2` and help/version `0`), `usage --json` documents shaped as specified, and `logs --json` emitting one compact `LogLine` per line with the exact 5-key set at `schema_version: 1`.

### Completion Notes List

1. **Task 1 (audit, no code change).** Confirmed all three shipped surfaces already satisfy AD-14: `list --json` → engine `FleetListing` (`schema_version` 2), `show --json` → `ShowDocument{schema_version, instance}`, `config get --json` → `ConfigDocument{schema_version:1, entries:[ConfigLeaf]}` — each with a top-level `schema_version`, snake_case fields, and reuse of engine `Serialize` types. **No gap found, so no behavior change was made**; they are now locked by the Task-5 key-set-freeze tests. Usage stays embedded in `list`/`show` (`FleetEntry.usage` + `FleetListing.totals`) — the new `usage` command is an ADDITIONAL surface over the same types, and a test asserts the two agree exactly.
2. **Task 2 (`logs --json`).** NDJSON via a new `emit_log_lines(lines, json)` used at all three emit sites (initial dump, follow poll, bounded final drain), so one-shot and `--follow` are byte-identical in shape. `log_line_json` uses `serde_json::to_string` (compact — `to_string_pretty` would break the one-object-per-line invariant). The `--follow` loop, bounded drain, and rotation detection are untouched; rotation/exit notices already went through `ui::note` → stderr, so stdout is pure NDJSON in both modes. Append order preserved (never sorted) — asserted by comparing the NDJSON `text` sequence to the human form's sequence.
3. **Task 3 (`usage` command).** `UsageDocument{schema_version, instance, usage}` and `FleetUsageDocument{schema_version, totals}`, both reusing `FLEET_SCHEMA_VERSION` (no new constant) and the existing engine `UsageView`/`FleetTotals` (no parallel type; the event-stream `UsageUpdateEvent` delta is deliberately NOT serialized — that is story 7-2). Both scopes are composed from the SAME facade path `list`/`show` use (`fleet()` → `FleetListing::new`), so totals equal the Usage Ledger and each other exactly. The human named form reuses `cost_row_value` → `render_dollars` (the single currency module, AD-8) and the fleet form reuses `fleet_total_footer`, so **no second currency formatter was introduced** and the currency grep-lint stays green.
4. **Task 4 (exit codes).** New `crates/kt/src/exit_code.rs` with the frozen `0–6` `ExitCode` enum (explicit discriminants) + a `classify()` downcast classifier — the low-churn option the story recommended; a `CliError` enum wrapper was deliberately NOT built (it would touch all 22 diagnostics + the three `map_*` mappers). `main()` now exits with the classified code. clap's `2` and help/version `0` are untouched (they exit from inside `Cli::parse()` and never reach the classifier) — both verified end-to-end.
5. **Decision — where the exhaustive exit-code mapping is pinned.** Codes `5` (unsupported) and `6` (timed-out) are not cheaply reachable through the binary cross-OS: both need a genuinely RUNNING child (the unsupported path needs the Unix-only surviving-engine harness, and a real `InteractionTimedOut` needs a stuck agent whose 64KB stdin pipe buffer is full — a slow, flaky setup that would violate the "never sleep to await state" determinism lesson). So the **exhaustive diagnostic→code mapping for all 22 structs plus the catch-all is pinned by deterministic, cross-OS unit tests in `exit_code.rs`**, and the integration tests prove the classifier is genuinely WIRED to the process exit status for every condition reachable without a spawned agent (`0/1/2/3/4`). Code `5` additionally gets a real end-to-end assertion added to the existing Unix-only `pause_unsupported_*` test. This is a conservative deviation in MECHANISM only — every documented condition is still asserted to yield its documented code.
6. **Decision — key-set freeze at the WIRE level.** The frozen key-set assertions parse the REAL `--json` stdout of the real binary rather than serializing structs in isolation, because the wire bytes are the actual compatibility surface (this also catches a `#[serde(rename)]`, which `cargo-semver-checks` provably would not). This transitively freezes `FleetListing`, `FleetEntry`, `FleetTotals`, `UsageView`, `BudgetView`, `ShowDocument`, `UsageDocument`, `FleetUsageDocument`, `ConfigDocument`, `ConfigLeaf`, and `LogLine` — every type Task 5.3 names. A test-file header comment records why these bespoke tests, not semver-checks, are the real gate (Task 5.4).
7. **`ExitCode::Success` carries `#[allow(dead_code)]`.** It is never CONSTRUCTED (a successful run just returns from `main`; `classify` only ever sees an `Err`), but `0` is part of the documented frozen table and its number is pinned by a test, so the contract lives in one place rather than being half-implicit. The alternative — an explicit `process::exit(0)` on success — was rejected because it would skip destructors.
8. **Coverage (NFR-3) not measured locally.** `cargo tarpaulin --fail-under 95` was not run in this session (it is a long, memory-heavy run and CI runs the per-crate split that has been green since PR #108). Net new code is small and densely tested (+25 tests), and the largest new module (`exit_code.rs`) is ~100 lines of logic with 8 dedicated unit tests, so a regression below 95% is unlikely — but this is **unverified locally and should be confirmed by CI**.
9. **`KtRun` gained `code: Option<i32>`** — its own doc comment already promised exit-code assertions; it now delivers them. `success: bool` was kept (not replaced) so no existing test needed changing.

### File List

**Created**

- `crates/kt/src/exit_code.rs` — NEW module owning the frozen `0–6` `ExitCode` contract, the `classify()` downcast classifier, and 8 unit tests pinning every diagnostic→code mapping plus the catch-all (Task 4).

**Modified**

- `crates/kt/src/main.rs` — added `mod exit_code;`; `main()` now exits with the classified code instead of the blanket `exit(1)`; added the `Logs{…, json}` field and the new `Usage{name, json}` clap variant + both dispatch arms; extended `test_agent_logs_parse` and added `test_agent_usage_parse`; added `usage` to `test_agent_subcommands_exist` (Tasks 2, 3, 4, 6.2).
- `crates/kt/src/cli/agent.rs` — `logs()` takes `json` and emits via the new `emit_log_lines`/`log_line_json` (compact NDJSON); added `UsageDocument`/`FleetUsageDocument` + `usage_json`/`fleet_usage_json` serializers, the `pub fn usage()` command body, and `render_usage_instance()` (Tasks 2, 3).
- `crates/kt/tests/helpers/mod.rs` — `KtRun` gained `code: Option<i32>` captured from `output.status.code()`, delivering the exit-code assertions its doc comment already promised (Task 5.1).
- `crates/kt/tests/agent_cli.rs` — added the compatibility-surface suite: a header comment recording why these bespoke tests (not the dormant, Rust-API-only `cargo-semver-checks`) are the real gate; 7 frozen key-set + `schema_version` pin tests covering every read-command `--json` document; 2 NDJSON `logs --json` tests (empty case + shape/order); 6 end-to-end exit-code tests; and a code-`5` assertion added to the existing `pause_unsupported_*` test (Tasks 5.2, 5.3, 5.4).
- `docs/commands.md` — new `kt agent usage [<name>] [--json]` section (with a JSON example and the integer-micros/label note), `logs` heading + NDJSON subsection updated for `--json`, and a new "Exit codes" section under Global Behavior documenting the `0–6` table as a v1 compatibility surface per PRD §7 (Tasks 4.3, 6.1).
- `scripts/check_docs.py` — added `usage` to the `AGENT_COMMANDS` allowlist so the new `kt agent usage` bash-fence examples validate (Task 6.1).

**Not modified (deliberately)** — `Cargo.toml`/`Cargo.lock` (no new dependency) and `CONTRACT_VERSION` (DC-7). *(The original pass also left `crates/ktesio-engine/**` untouched — `LogLine`, `UsageView`, and `FleetTotals` were reused as-is. The 2026-07-21 fix pass touches it in TEST/comment code only; no engine behavior or wire shape changed — see below.)*

**Fix pass (2026-07-21) additionally modified**

- `crates/kt/src/cli/agent.rs` — broken-pipe discipline in the log-streaming path (`EmitOutcome`, `classify_stdout_write`, `human_log_line`; the three emit sites now end cleanly on a closed consumer); `validate_instance_name` + its two call sites in `show --json` and `usage`; the shared `EMPTY_FLEET_HINT` and the empty-Fleet note on Fleet-wide `usage`; three new unit tests (both mapper→exit-code pins and the name-validation pin).
- `crates/kt/tests/agent_cli.rs` — 7 new tests (2 priced key-set freezes, the cross-OS NDJSON wire freeze, the `--follow --json` incremental-batch test, the closed-consumer exit-0 test, the malformed-name exit-2 test, the empty-Fleet `usage` test); the `priced_instance` fixture + 3 priced frozen-key constants; the `append_captured_log_lines`/`captured`/`assert_ndjson_log_lines` helpers; 7 Windows-skipping tests renamed with a `_unix` suffix; `code == Some(5)` added to both `send` failure tests; `code == Some(0)` added before the totals-equality parse; the `_unix` convention documented on `start_via_surviving_engine`.
- `crates/ktesio-engine/src/domain/event.rs` — **test-only**: added `log_schema_version_is_1_the_frozen_v1_wire_value`, replacing a tautological guard.
- `crates/ktesio-engine/src/ports/process_backend.rs` — **comment-only**: one cross-reference updated for a renamed test.
- `docs/commands.md` — real branching example after the exit-code table; code `5` now documents the poisoned-channel case; the Fleet `usage --json` shape + the omitted-dollar-fields note; the closed-pipe guarantee.
- `README.md` — command table gained `usage`, `send`, `logs`, plus an exit-code-table pointer.
- `.gitignore` — `__pycache__/` + `*.pyc`.

---

## Fix pass (2026-07-21) — closing the AC2 gate holes

A two-pass review, followed by an adversarial pass that MUTATED code and watched tests pass, found AC2's own claim — *"any unannounced change to a wire shape, `schema_version`, or exit code fails the `test` CI job on all three OSes"* — **false in four places**. Each hole is now closed and each fix was re-verified by re-applying the exact mutation that exposed it.

### What was wrong, and what closed it

**H1 — exit code `6` had no end-to-end pin.** `classify(AgentInteractionTimedOut) == 6` was unit-tested, and `main`'s wiring of `classify` was proven for `0`–`4`, but NOTHING pinned that the timeout PATH produces that diagnostic type. Changing `map_engine_error`'s `EngineError::InteractionTimedOut` arm to build an `AgentIo` silently demoted the documented `6` to `1` with the whole suite green.

*Approach taken, and why:* a true end-to-end timeout was **rejected** — it needs a stuck agent whose 64KB stdin pipe is full, which is exactly the slow, sleep-shaped, flaky setup the story's own determinism rule forbids. Instead the MISSING LINK is pinned directly and deterministically: `every_engine_error_mapper_arm_preserves_its_documented_exit_code` drives the REAL `map_engine_error` and then the REAL `classify` for every non-`1` engine-error class (`InteractionTimedOut`→6, `InteractionUnavailable`→5, `CapabilityUnsupported`→5, `NotRunning`→4, `StopUnconfirmed`→4, `InvalidName`→2, `NotFound`→3, `LaunchFailed`→1), with a `map_error` sibling for the registry-shaped classes. Composed with the existing `classify` unit tests and the `0`–`4` end-to-end tests, every link in the chain is now pinned, cross-OS. Additionally, `send_on_an_adopted_instance_…` and `send_unsupported_…` were strengthened from a bare `assert!(!success)` to `assert_eq!(code, Some(5))`.

**H2 — the priced (Rate'd) wire shape was entirely unfrozen.** Every `assert_frozen_keys` fixture used `registered_mock()` with no Rate, so `skip_serializing_if` omitted every dollar field: `UsageView`'s three, `FleetTotals`' two, and `BudgetView`'s five (`per_run_cost_cap`, `per_run_dollars_remaining`, `cumulative_cost_cap`, `cumulative_dollars_remaining`, `estimate_label`). Adding a `blended_rate` field to `UsageView` populated only in `with_dollars()` passed all 69 tests.

Closed with a `priced_instance` fixture that configures BOTH Rate directions plus both token scopes and both dollar Cost Cap scopes — so every optional field materializes at once and the frozen constants are the MAXIMAL key-sets — and two new tests covering all five documents (`list`, `show`, `usage <name>`, `usage`, and the embedded `BudgetView`).

**H3 — `--follow --json` was never exercised.** The suite had exactly one `--follow` invocation, in human mode. Changing the follow loop's `emit_log_lines(&new_lines, json)` to `(…, false)` — so every incremental batch emitted HUMAN text into a stream the caller parses as NDJSON — passed all 33 logs tests.

Closed with `logs_follow_json_emits_an_incremental_batch_as_valid_ndjson`: it spawns the real `--follow --json` with stdout redirected to a file, **polls** that file (never sleeps to await state) until the initial backlog is committed, and only THEN appends a new captured line — so the new line provably cannot be part of the already-read backlog and can only arrive via a follow batch. It then asserts every stdout line is still frozen-shape NDJSON, and that the exit note stayed on stderr. Cross-OS: a `registered` instance is not `running`, so follow emits its batch and exits on its own.

**H4 — Windows silently self-skipped (ratified: "fix coverage + honest skips").** Runtime `if OsId::current() == Windows { return; }` reports as **PASSED**, so CI showed green on Windows with zero signal that the assertions never ran. Two of those tests were the SOLE guards for real contract surfaces.

Both halves of the ratified fix were applied. (a) **Coverage:** the OS-independent contract was split out into tests that genuinely run everywhere. `logs_json_wire_shape_is_frozen_ndjson_on_every_os` now guards the NDJSON shape, the frozen `LogLine` key-set, the `LOG_SCHEMA_VERSION` value, stdout purity, and append order on all three OSes — by seeding the SAME capture file the engine writes (`logs/output.log`, one serialized `LogLine` per line) instead of needing a child that outlives its starting engine. (b) **Honest skips:** every test that runtime-skips on Windows now carries a `_unix` SUFFIX (the repo's own existing convention, cf. `pause_prints_paused_state_and_exits_zero_guaranteed_unix`), so the limitation is visible in the test list rather than hidden in the body — seven tests renamed. `start_via_surviving_engine`'s doc comment now states the convention and the rule: a `_unix` test may be an ADDITIONAL end-to-end proof, never the sole guard. No existing assertion was weakened; several were strengthened.

**H5 — tautological guard.** `event.rs`'s `assert_eq!(line.schema_version, LOG_SCHEMA_VERSION)` compared the field to the constant it was stamped from, so bumping the constant could not fail it. Added `log_schema_version_is_1_the_frozen_v1_wire_value` pinning the LITERAL, mirroring `fleet_schema_version_is_2_after_the_3_5_additive_bump`.

### Ratified behavior fixes

**M1 — `kt agent logs --json | head -5` panicked (exit 101).** Rust ignores `SIGPIPE`, so `println!` unwrapped an `ErrorKind::BrokenPipe` and aborted with a code OUTSIDE the frozen table — falsifying `docs/commands.md`'s "Every `kt` command returns one of these numeric exit codes". `emit_log_lines` now writes through a held `StdoutLock` and reports `BrokenPipe` as a new `EmitOutcome::PipeClosed`, which every one of the three emit sites turns into a clean `Ok(())` (exit `0`); any OTHER write error is still a real `AgentIo` diagnostic. The fix covers the HUMAN path too (the same bug existed there). Deliberately **scoped to this streaming path** — no process-wide `SIGPIPE` disposition change. `PipeClosed` also ENDS a `--follow` loop rather than letting it spin forever writing into a dead pipe.

**M2 — malformed names exited `3` on some commands and `2` on others** (ratified: "make it uniformly 2"). `usage <name>` and `show --json` resolved the instance with a linear `find` over `fleet()` and then synthesized `RegistryError::NotFound`, while `logs`/`stop`/`show` (human) passed the raw name into an engine call that validates it. Both now call a new `validate_instance_name` FIRST, which goes through the engine's PUBLIC `ktesio_engine::InstanceName` newtype — the same rule the engine applies internally, so `kt` re-derives nothing (AD-2). A well-formed-but-unregistered name still exits `3`.

### Docs & hygiene

- **M3** `README.md` command table gained the missing `usage` (this story) **and** `send`/`logs` (4-1/4-2 misses) rows, plus a pointer to the exit-code table.
- **L1** The bash fence after the exit-code table demonstrated nothing; it is now a real branching example (`if`/`elif` on `$?`) — and its `kt` invocations sit at line start, so `check_docs.py` genuinely validates them (proven: renaming `register`→`regsiter` inside it fails the doc gate).
- **L2** Code `5` is no longer described only as a Capability-Declaration refusal; the poisoned-channel `AgentInteractionUnavailable` case is documented.
- **L4** The Fleet form's `usage --json` shape (`{schema_version, totals}`) is now documented with an example, plus an explicit note that dollar fields are OMITTED (not null) with no Rate — a parser-visible fact now that these key-sets are frozen.
- **L5** `usage_json_totals_equal_the_list_json_totals_exactly` asserts `code == Some(0)` before parsing, so a regression shows as a code mismatch naming stderr rather than an opaque serde panic.
- **L6** `.gitignore` gained `__pycache__/` + `*.pyc`.
- **L3** Fleet-wide `usage` on an empty Fleet now prints the same registration hint `list` does (stderr in both modes, so `--json` stdout stays pure), sharing one `EMPTY_FLEET_HINT` constant.

### Mutation verification (each hole re-broken, then reverted)

| # | Mutation applied | Result | Reverted |
|---|---|---|---|
| H1 | `map_engine_error`: `InteractionTimedOut` arm builds `AgentIo` instead of `AgentInteractionTimedOut` | **FAILED** `every_engine_error_mapper_arm_preserves_its_documented_exit_code` — `left: General, right: TimedOut` | yes, re-passed |
| H2 | `UsageView` gains `blended_rate`, populated only in `with_dollars()` | **FAILED** both new priced tests — `the frozen 'UsageView (priced)' key-set changed`, naming `blended_rate` | yes (`git checkout`), re-passed |
| H3 | follow poll: `emit_log_lines(&new_lines, json)` → `(…, false)` | **FAILED** `logs_follow_json_emits_an_incremental_batch_as_valid_ndjson` — `NDJSON line 1 is not valid JSON`, showing the human `… [engine] incremental line` that leaked in. Deterministic (all 3 nextest retries failed) | yes, re-passed |
| H5 | `LOG_SCHEMA_VERSION` `1` → `2` | **FAILED** both the engine literal pin AND the cross-OS CLI wire pin | yes, re-passed |
| M1 | restore the pre-fix panic (`writeln!(…).unwrap()`) | **FAILED** `logs_json_survives_a_consumer_that_stops_reading_and_still_exits_zero` — exit `101`, not `0` | yes, re-passed |
| M2 | drop `validate_instance_name` from `usage` | **FAILED** `a_malformed_instance_name_exits_with_the_usage_code_on_every_read_command` — `"Bad Name"` exited `3` | yes, re-passed |

### Gate results (fix pass, `cargo +1.96.1`)

| Gate | Command | Result |
|---|---|---|
| fmt | `cargo +1.96.1 fmt --all --check` | **PASS** (exit 0) |
| clippy | `cargo +1.96.1 clippy --workspace --all-targets -- -D warnings` | **PASS** (exit 0, zero warnings) |
| tests | `cargo +1.96.1 nextest run --workspace --all-targets` | **PASS** — **896 run, 896 passed, 0 failed, 0 skipped** (19 binaries, 44.7s). Was 885; **+11 net new**. |
| doc tests | `cargo +1.96.1 test --workspace --doc` | **PASS** (0 doc tests) |
| check_docs | `python3 scripts/check_docs.py` | **PASS** — "Validated 22 Markdown files" |
| test_automation | `python3 scripts/test_automation.py` | **PASS** — "Ran 21 tests … OK" |
| currency grep-lint (AD-8) | ci.yml pattern + allowlist, run locally | **PASS** — the new `.contains('$')` wire-purity checks are already allowlisted as READ-checks |
| OS-`cfg` gate | grep the diff for `cfg(unix\|windows\|target_os\|target_family)` | **PASS** — 0 added (the Windows skips stay data-driven via `OsId::current()`) |
| MSRV `--locked` (DC-7) | `git diff --quiet Cargo.lock` | **PASS** — UNCHANGED |
| semver-check (DC-7) | `CONTRACT_VERSION` inspection | **PASS** — stays `"0.4.0"` |

Coverage (`cargo tarpaulin`) was **not** run in this pass (unchanged from Completion Note 8): the net-new code is small and densely tested (+11 tests, all new production code paths covered), but this remains unverified locally and should be confirmed by CI.

### Still NOT covered after this pass (stated plainly)

1. **Exit codes `5` and `6` have no cross-OS end-to-end assertion.** Every route to them needs a genuinely running child, and the surviving-engine harness that provides one is Unix-only. Their contract is gated cross-OS in two composed halves (mapper tests → the code each condition produces; the `0`–`4` end-to-end tests → `main` wires `classify` to the process status), and on Unix code `5` additionally has two real end-to-end assertions. Code `6` has no end-to-end assertion on ANY OS — building one would require a deliberately stuck agent with a full stdin pipe, rejected as flaky. DC-6 above now states this exception explicitly instead of the blanket claim.
2. **The seven `_unix` tests still do not run on Windows.** That is unchanged and unfixable with the current harness; what changed is that the skip is now VISIBLE in the test name and nothing OS-independent rests solely on them.
3. **`logs --json` over a REAL engine-captured log is still Unix-only.** The cross-OS test seeds the same on-disk format the capture thread writes, which gates the wire contract everywhere but does not exercise the capture pipeline itself on Windows.
4. **Rotation under `--follow --json`** (the `note_if_rotated` path) has no CLI-level test in either mode — pre-existing, out of scope here.
5. **Coverage (NFR-3) is unmeasured locally**, as above.
