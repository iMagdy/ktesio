---
title: Testing
description: Required checks, integration fixtures, coverage gates, and documentation validation for Ktesio contributors.
---

# Testing

The test suite covers unit behavior, CLI workflows, and local git fixtures.

## Required Checks

Run these before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
python3 scripts/check_docs.py
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

### Toolchain

The workspace ships a root `rust-toolchain.toml` pinning the toolchain to the MSRV (Rust **1.96.1**, with `clippy` + `rustfmt`), so a bare `cargo` — `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt` — uses 1.96.1 without typing `cargo +1.96.1`. The explicit `+1.96.1` form still works and remains the escape hatch. Keep the pinned `channel` in lockstep with `rust-version` in the root `Cargo.toml`.

One caveat: rustup only honors the file when nothing higher-precedence overrides it. A `RUSTUP_TOOLCHAIN` environment variable, an active `rustup override`, or a version manager that shims `cargo` (e.g. **mise** or asdf, which export `RUSTUP_TOOLCHAIN`) all win over `rust-toolchain.toml`. If bare `cargo --version` in the repo does not report 1.96.1, check `rustup show` (it names the active override) and either `unset RUSTUP_TOOLCHAIN` / drop the version-manager rust pin, or just use `cargo +1.96.1`.

CI is deliberately explicit the other way: its "latest stable" jobs run `cargo +stable …` so they still catch future stable regressions, while the dedicated `msrv` job proves the 1.96.1 floor with `cargo +1.96.1`. (This means local bare `cargo` lints/tests on the MSRV while those CI jobs use latest stable — an intentional split, so an occasional new-stable clippy or rustfmt nit can surface in CI that a local MSRV run did not; reproduce it with `cargo +stable clippy …` / `cargo +stable fmt …` if needed.)

## Unit and Integration Tests

```bash
cargo test --workspace --all-targets
```

Integration tests create local temporary git repositories. They do not require network access. The agent-lifecycle tests spawn a small cross-platform helper binary (`fake_agent`, in `ktesio-conformance`) as a real child process to prove start, stop, launch-failure, no-survivor, pause/resume, crash-detection, restart, and orphan-adoption behavior end to end; it is a dev/test artifact and never ships. For pause, `fake_agent --heartbeat-ms <ms>` prints a periodic incrementing line, so a guaranteed (Unix) pause is provable — the heartbeat stops growing under `SIGSTOP` and resumes under `SIGCONT`. For survival, `fake_agent --crash-after-ms <ms>` runs normally past the readiness window and then exits non-zero, simulating an unrequested crash so the reaper detects it and the Restart Policy fires. The crash/restart legs (`tests/crash.rs`) prove a `never`-policy crash lands `failed` with a `crashed` cause and no restart, and an `on-failure` crash is automatically restarted by the reaper (the crash-loop-stops-at-5 and count-reset legs run in the supervisor unit tests with an injected fast backoff, so they never sleep for real seconds while production keeps the 1s×2/60s constants). The engine-kill adoption test (`tests/adoption.rs`) is the NFR-1 proof: it runs a first engine in a subprocess that starts an agent and exits WITHOUT a graceful stop (a `kill -9` model — no destructors run, so the agent survives and re-parents to init), then opens a new engine over the same state dir and asserts the live child is adopted (row `running`, a subsequent `stop` truly kills it, no orphan remains) while a record whose process is gone reconciles to `failed`; the AI-7 (paused-live process adopted and resumable) and AI-8 (phantom `running` row → `failed`) cases ride the same file. The reboot-durability test in the same file simulates a machine reboot — a true reboot is infeasible in CI, so it registers several instances in different states, leaves one running via the surviving-engine subprocess, then kills every live agent process (their PIDs would not survive a reboot) and reopens the engine over the same state dir — and asserts the reboot invariants: every registration survives with its name/kind/home intact, the previously-running instance reconciles to `failed`, the cleanly-stopped one stays `stopped`, each restart policy and count is unchanged, and no orphan process remains. The `≤1s` durability bound behind that guarantee is asserted structurally by the store tests (WAL, `synchronous=NORMAL`, one committed transaction per state mutation, and a reopen that finds every row intact). The Fleet `--json` shape is covered end-to-end in `crates/kt/tests/agent_cli.rs`: `kt agent list --json` and `kt agent show <name> --json` emit a single parseable document on stdout (nothing else there), carrying a `schema_version` and per-instance objects whose `budget`/`usage` are the honest JSON `null` seed (never `0`), with the Epic-3 metering note routed to stderr and an empty Fleet rendered as a valid empty array.

The unified layered config (AD-9) is tested at three levels. The pure precedence resolver is exhaustively unit-tested in `crates/ktesio-engine/src/domain/config.rs` with no I/O: a key present in exactly one layer (each of the four), a key present in each adjacent precedence pair (the stronger layer wins), a key present in all four (the strongest wins), a nested-table per-leaf merge where shapes agree (a stronger layer's `a.b` overrides only `a.b` while a weaker layer's sibling `a.c` survives — the data-loss guard), the scalar-over-subtree and subtree-over-scalar **shape collisions** where they disagree (the stronger layer's shape wins and prunes the weaker layer's orphans, with the surviving leaf tagged to the layer that defines it — no self-contradictory tree, no stale provenance), the empty-single-layer and all-empty cases, and a determinism check; every case asserts the recorded source layer, proving the provenance seam. Write-time validation is unit-tested against fixed inputs: the sole known key (`model`) and an `agent.*` pass-through key are accepted; an equally-unknown non-`agent.*` key is rejected; a near-miss (`modle`) suggests the nearest key (`model`, with a deterministic candidate-string tie-break) while a far-miss suggests nothing; an empty dotted segment (`agent..b`) is rejected; and the hand-rolled Levenshtein has its own coverage. The set/get round trip is proven at the registry level (`set_config model` then `effective_config` reflects it tagged as the instance layer, and an invocation override beats it; an unknown key is rejected leaving the on-disk `config.toml` byte-unchanged; nesting a child under an existing scalar fails closed byte-unchanged; an `agent.*` key round-trips verbatim; the seeded `name` identity key is filtered from the resolved view; a malformed instance layer surfaces a typed error, not a panic) and end-to-end through the CLI in `crates/kt/tests/agent_cli.rs` (`kt agent config set model` then `kt agent config get` prints the value on stdout; an empty effective config before any set says so; an unknown key exits non-zero with the suggestion on stderr and the config unchanged; a scalar-shape conflict fails non-zero byte-unchanged; an `agent.*` key sets and gets verbatim; the whole-config table lists keys on stdout).

The unified → native config mapping (FR-12) is tested at four levels. The mapping model in `crates/ktesio-adapter-api/src/config.rs` is unit-tested pure and I/O-free: each `ConfigTarget` renders its native form (an env var name, a two-token `--flag value` pair, a file path plus native key), a `ConfigMapping` builds and reads back deterministically, each target kind deserializes from the `[config.<key>]` TOML shape (a sub-table naming no native mechanism, or a file sub-table with an unknown field, fails to parse), and `validate` rejects an empty native token or a file path that is absolute or escapes the Agent Home. The manifest `[config]` section is tested in `manifest.rs`: an absent section validates with an empty mapping, all three target kinds parse and validate and read back through the accessor, a malformed rule is an `InvalidField` naming the `[config.<key>]` sub-section, and a path escaping the home is rejected. Both adapter kinds declare a mapping in shape-parity: the builtin `mock` and the conformance `MockAdapter` each declare `model` → env `MODEL`, and the cross-boundary parity test in `crates/ktesio-engine/tests/registration.rs` asserts the two mappings are identical (guarding the fixture against drift). The application transform (`adapter::apply_config_mapping`) is unit-tested in isolation: a `model` value maps to each declared target (env → the launch env, flag → two args, file → a rendered TOML file in the Agent Home at the native key), an `agent.*` leaf is delivered verbatim to an env var named by its key-tail, an unmapped documented key is a no-op, and the resolved-config → launch transform is deterministic. The end-to-end proof at start runs on BOTH adapter kinds in the supervisor tests: the inert builtin `mock` proves `model` → env on the mapped launch the mapping produces (a native adapter carries no live process), while a live `fake_agent` manifest carrying a `[config]` section proves `model` → flag (observed in the spawned process's argv via `fake_agent --dump`) and `model` → file (the engine renders the native file into the Agent Home) and an `agent.*` key delivered verbatim into the process environment. The `agent.*`-unvalidated marker is covered end-to-end through the CLI in `crates/kt/tests/agent_cli.rs` (`kt agent config get` shows a Validated column marking an `agent.*` leaf `unvalidated` and a known `model` key `validated`, on stdout; a known-key-only config shows no unvalidated marker).

Effective-config provenance (FR-13) is tested at three levels. The provenance accessor is unit-tested in `crates/ktesio-engine/src/domain/config.rs` (`EffectiveConfig::source_label` reports the winning layer for a leaf resolved from each of the four layers, agrees with `SourceLayer::as_str`, respects precedence, and is `None` for a missing key). The persisted-snapshot DTO and writer are unit-tested in `crates/ktesio-engine/src/domain/registry.rs`: building the snapshot from a multi-layer effective config yields one entry per leaf with the rendered value plus its source label and the schema version; it round-trips through JSON (the source is the kebab-case wire form, the value is the rendered display string); the writer persists it at `EnginePaths::effective_config_snapshot` and it parses back; a second write **overwrites** in place; and a write failure (the snapshot path pre-created as a directory) surfaces a typed `SnapshotWrite` error, never a panic. The start-seam write is proven in the supervisor tests (`crates/ktesio-engine/src/domain/supervisor.rs`): starting a live `fake_agent` instance writes `effective-config.json` into the Agent Home carrying `model` tagged `instance`; a re-start (through the same `start_inner` seam) **overwrites** the snapshot with the newly resolved value (AC7); and a snapshot-write failure rejects the start with a typed `EngineError::Snapshot` **before** the `starting` transition, leaving the instance in its prior state (no spurious change). The CLI rendering is covered in `crates/kt/tests/agent_cli.rs`: the human `config get` gains a **Source** column showing the `instance` layer for a set key (on stdout, with the stale Epic 2.3 deferral note retired from both streams); `config get --json` emits a versioned document whose per-leaf objects carry `{ key, value, source, unvalidated }` as pure JSON on stdout (an `instance`-sourced validated key and an `agent.*` unvalidated leaf); the single-key `--json` form emits just that leaf; and starting an instance persists the snapshot into the Agent Home (read back from the path `register` printed). The pure `config_json` serializer is unit-tested in-process in `crates/kt/src/cli/agent.rs` (the versioned document carries source + unvalidated per leaf, the single-key form emits one leaf, and the `--json` value matches the human display form — proving the single display path). Every surface renders through that one display path, the single choke point where secret masking hooks (below).

Secrets (FR-14 / NFR-6 / AD-10) are tested at four levels, anchored by a **no-leak matrix** that proves the "safe by construction" guarantee. The primitives are unit-tested pure in `crates/ktesio-engine/src/domain/`: the `secret:NAME` classifier + `secret_name` extractor (a non-empty `NAME` after the prefix classifies; a bare `secret:` does not; classification is on the value regardless of key) and the `display()` mask (a `secret:NAME` leaf renders `secret:****` at the single choke point while a non-secret leaf is unchanged) in `config.rs`; and the `SecretString` newtype in `secret.rs` (`Display` and `Debug` both redact to `[REDACTED]` and never the cleartext, `expose_secret()` returns it, and a struct embedding a `SecretString` and deriving `Debug` does not leak — the structural guard). The `SecretResolver` port + resolvers are unit-tested in `ports/secret_resolver.rs`: the env resolver reads a set var and misses an unset one; the 0600-file resolver reads a `NAME = "value"` entry, treats a missing file and a non-string entry as a miss, and errors hard on malformed TOML; the composite tries env-then-file (env wins, file resolves when env is absent), a hard error short-circuits (never silently "absent"), and an unresolved reference names the `NAME` + resolvers tried with no value. The Unix 0600 permission check lives in `backends/unix` (the allowlisted `#[cfg]` home): a `0644` (group/other-readable) secrets file is **refused** with a `chmod 600` remediation while a `0600`/`0400` file passes; the Windows posture is a documented portable skip. At the **engine level** (`supervisor.rs` + `registry.rs`) a `model = secret:NAME` leaf resolves (env) to a sentinel: the spawned agent's argv carries the **cleartext** (usable — delivery), while the persisted snapshot and every transition-event payload carry the **mask** (no leak); an unresolved secret rejects the start with a typed `EngineError::Secret` (naming the `NAME`, never a value) **before** any state change or snapshot write. The **end-to-end matrix** in `crates/kt/tests/agent_cli.rs` drives the real `kt` binary: a `secret:MODEL_KEY` leaf resolves to a sentinel that (positively) reaches the adapter's native env (`env=MODEL=<sentinel>` in the `fake_agent --dump`) and (no-leak) appears in **none** of the effective-config snapshot, `config get --json`, the human `config get`, or **any file in the Agent Home** (logs + event payloads included) — the mask appears instead; `config get --json --reveal` (and the single-key form) is the sole surface that carries the sentinel, while default `--json` masks, a `--reveal` on a non-secret leaf is a harmless no-op, and an unresolved secret exits non-zero with a diagnostic naming the `NAME` while the instance stays `registered`. The `--reveal` render + overlay paths are also covered in-process in `crates/kt/src/cli/agent.rs`.

## Cross-platform testing (3-OS matrix)

The per-OS process-control code lives only under `crates/ktesio-engine/src/backends/{unix,windows}` (the sole place OS-conditional compilation is allowed). This code cannot be verified on a single operating system — the Windows Job-Object backend does not even compile on Linux. The CI `test` job therefore runs on a matrix of `ubuntu-latest`, `macos-latest`, and `windows-latest`:

- Linux and macOS run the Unix backend (process groups, `SIGTERM`/`SIGKILL`, and `SIGSTOP`/`SIGCONT` for the guaranteed pause) on both Unixes.
- Windows runs the Windows backend (Job Objects, `TerminateJobObject`), the only place its behavior — real spawn, terminate, no-survivor, and the cooperative best-effort pause — is actually exercised. The best-effort pause is honest by surfacing a qualifier (a `pause-best-effort` transition cause plus a CLI stderr note), never a silent fake; that path rides this `windows-latest` leg and is compile-checked only on Unix.

Only the `test` job matrixes; the other jobs (fmt, clippy, build, docs, boundary, semver, msrv, coverage) stay Linux-only.

## Coverage

CI runs `cargo tarpaulin --engine llvm --workspace --fail-under 95` as the coverage gate, on Linux only. To run it locally:

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --engine llvm --workspace --fail-under 95
```

Both CI and local use the `--engine llvm` (source-based) engine. On macOS the default ptrace engine is unavailable outright — tarpaulin errors with `missing section: CoverageFunctions` — so llvm is the only option on a macOS dev host; running the same engine on CI keeps the two on the same reported percentage. CI adds the `llvm-tools-preview` rustup component, which provides the `llvm-profdata`/`llvm-cov` the engine shells out to.

CI also keeps a dedicated cargo cache key for coverage (`<os>-cargo-coverage-<lockhash>`), separate from the other jobs. Tarpaulin compiles the whole dependency graph with coverage instrumentation, whose fingerprints differ from the normal-profile artifacts the other jobs cache — so a shared key gave coverage nothing reusable and, because the job runs last, never saved its own instrumented target either. The effect was a full cold instrumented recompile every run, which is what blew the coverage timeout (AI-23) — not the engine, and not the test run. The dedicated key persists the instrumented target, so only the first run pays the cold-build cost.

The coverage step is also tuned to survive the hosted runner's limits. The instrumented build (coverage counters + the fleet/adoption suite each spawning several child processes) overflowed the 7 GB runner and it "lost communication" — an OOM that kills the whole job with no log. Four levers address it: `RUST_TEST_THREADS=1` runs the instrumented tests serially (one subprocess-spawning test's process tree resident at a time); an 8 GB swap file on the `/mnt` temp disk absorbs any RAM spike; `CARGO_PROFILE_DEV_DEBUG=1` builds with line-tables-only debuginfo, which shrinks the instrumented binaries (less RAM) and the `target/` on `/` (less disk) while leaving coverage unchanged (llvm line-mapping needs only line tables); and the step frees ~30 GB of unused preinstalled SDKs so the instrumented `target/` — which lives on `/`, not the `/mnt` swap disk — has room.

Generate an HTML report:

```bash
cargo tarpaulin --out Html
```

### Coverage honesty for per-OS code

`cargo tarpaulin` runs on Linux and cannot instrument `#[cfg(windows)]` code, so the 95% gate is measured on Linux against the OS-agnostic core plus the Unix backend (which compiles and runs on the Linux tarpaulin host, so its lines are covered). The Windows backend's lines are `cfg`-excluded on Linux and never enter the Linux coverage denominator, so the reported percentage is honest for what Linux can see. The Windows backend's correctness is proven instead by the `windows-latest` matrix `test` run passing — a real Job-Object spawn, terminate, and no-survivor check — not by a coverage number. The `fake_agent` helper binary runs only as a spawned subprocess, so it too is excluded from coverage (its behavior is proven by the tests that spawn and kill it).

## Documentation Checks

```bash
python3 scripts/check_docs.py
```

The docs check validates:

- Root and `docs/` Markdown links.
- JSON fenced code blocks.
- Documented `kt` command examples.
- Stale links to old repository names or generated spec quickstarts.

## Release Script Checks

```bash
python3 scripts/generate_release_docs.py v0.0.0 --output-dir target/release-docs-test
```

This verifies the release-note generator can handle a first-release style tag when no previous tag exists.

## Automation Helper Tests

```bash
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_automation.py
```

These tests cover release-note and changelog rendering, Homebrew formula
generation, installer dry-run decisions, and CI/workflow expectations.
