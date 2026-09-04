---
title: Adapter Contract
description: The versioned contract between the Ktesio engine and every agent adapter — trait surface, capability declarations, version negotiation, and the versioning and deprecation policy.
---

# Adapter Contract (v1)

The **Adapter Contract** is the agreement between the Ktesio engine and every agent adapter — native builtins and manifest adapters alike. It lives in the `ktesio-adapter-api` crate (the schema's single home), is versioned independently of the engine under its own semver, and is **frozen at v1 (`1.0.0`)** as of 2026-09-04 after validation against a second agent (opencode) and the story 6-4/6-5 conformance passes. This page is the contract's normative home: what an adapter implements, what the engine enforces, and the rules for changing any of it.

Everything here applies uniformly to both adapter kinds ("two kinds, one trait"): a native builtin declares the same shapes in code that a manifest adapter declares in `adapter.toml`. The concrete manifest syntax is documented in [the manifest reference](manifest.md); the Conformance Test Kit ([testing](testing.md#the-conformance-test-kit)) proves an adapter honors every applicable section.

## The adapter trait surface

An adapter implements (or a manifest declares) exactly:

| Surface | Meaning |
|---------|---------|
| `kind` | The adapter's identifier (e.g. `mock`, `hermes`, or the manifest's `[adapter].kind`), stored on the Agent Instance |
| `capabilities()` | The per-OS Capability Declaration (below) |
| `metering_source()` | The Metering Source: `self-reported` or `engine-observed` (a "no viable source" adapter is a validation error, never a variant) |
| `config_mapping()` | The unified→native config mapping; the default is an EMPTY mapping (an unmapped documented key is delivered nowhere — a no-op, never a guess) |
| `start` / `stop` / `pause` / `resume` | The lifecycle templates. Only `start` is mandatory; an omitted `stop` means the engine's process termination IS the normative stop (below) |
| `interaction` (manifest-only section) | The declared interaction channel: `stdio` or `http` (documentary — see below) |

The engine is the sole launcher: every adapter is spawned through the identical process-backend mechanism, and the engine unconditionally pipes the child's stdin (AD-12). Adapter trait lifecycle methods are the declaration surface; the engine's supervisor owns the actual transitions.

## Capability declarations

Capabilities are declared **per OS** (`linux`, `macos`, `windows`) with one of three support levels:

- `guaranteed` — the engine may rely on it.
- `best-effort` — surfaced honestly (a qualifier cause on the transition, e.g. `pause-best-effort`), never silently treated as guaranteed.
- `unsupported` — the command fails fast, quoting the declaration; nothing is faked.

A capability with no entry for an OS projects to `unsupported`. A declaration must have at least one non-`unsupported` entry somewhere — an adapter that supports nothing is not viable and is rejected at registration. Modeled capabilities in v1: `pause` and `interaction`.

Per-OS honesty is the adapter author's job: the contract lets an author declare any level on any OS and holds them to it; it does not police vendor OS-support stances (see R11 in the decisions below).

## Lifecycle and stop semantics

- **Start** is the only mandatory template. The resolved launch (exec/args/env) is snapshotted at registration.
- **An omitted stop template is normative**: the engine's process termination (signal, then escalation) IS the stop; the TCK asserts the child process exits. Adapters to agents with a graceful-stop verb declare a stop template. There is no graceful-stop acknowledgment concept in v1 (R4): an agent's self-exit (e.g. an in-chat restart hand-off) is an unrequested exit handled by the crash/restart policy — any non-zero exit while Running is a crash, and the restart policy relaunches with the same persisted launch.
- **Readiness vs liveness (R8)**: v1 defines no readiness concept distinct from process liveness and no endpoint-discovery surface. A supervisor that must know where an HTTP-native child listens should pin its port via the start template and own collision handling, or parse the child's documented startup line — that is an adapter-convention concern, not contract machinery.
- **Agent-side abort (R10)** is out of contract scope: verbs are start/stop/pause/resume. An agent's own abort facility (e.g. ending one session's work inside a still-running server) is the agent's business; the contract's stop asserts process-level outcomes only.

## Interaction channels

`[interaction].channel` is a closed enum: `stdio` (the spawned child's OS stdin pipe — the only channel the engine delivers through in v1) or `http` (an HTTP-native interaction surface, added at the v1 freeze via CP-6.5-a option (i)).

`http` is **additive vocabulary, deliberately documentary**: the engine never branches on the declared channel — the stdin pipe is unconditional, and `send_input` still writes the child's stdin (failing fast and typed where the declaration says unsupported). Its purpose is honesty and registerability: an HTTP-native agent (e.g. opencode, whose programmatic surface is `POST /session/:id/message` + SSE and whose spawned `serve` never reads stdin) declares `http` to name its real transport and registers with `interaction` supported instead of being forced into an all-unsupported (and therefore illegal) declaration. A real engine-side HTTP send implementation is a **post-v1** change under the versioning policy; the relaxation alternative (letting a single-capability adapter declare `unsupported` everywhere) was **declined** — the viability bar stays.

## Metering

- `self-reported`: the agent's usage accounting reaches the engine as `KTESIO_USAGE {json}` sentinel lines on the captured stdout. **The channel is adapter-implemented** (contract v1): the stream may be native or synthesized by an adapter shim over the agent's own surfaces (SSE tails, session reads); a shim derives `sequence` from the agent's per-Run message ordinals so the `UNIQUE(instance, run, sequence)` replay-dedup invariant holds identically (R2). Delayed batches reconcile without double-counting in both variants.
- `engine-observed`: the engine meters the agent's model traffic through a per-instance loopback forward proxy — no agent cooperation needed.

**Unknown-vs-zero is a contract stance** (R7): when the provider omits usage, an adapter must surface it as unknown — never coerce it to zero. A zero event asserts the provider reported zero; an absent event means the usage is unknown. (Hermes labels cost honesty; opencode silently coerces missing usage to zero — the contract sides with the label.)

## Memory

Memory Backing vocabulary is part of the frozen v1 surface, carried on `kt agent memory attach|detach --json` (story 6-6's wire freeze; before it, the surface was human-output-only):

- **Kinds** (snake_case wire strings, verbatim): `filesystem` (an engine-managed directory inside the Agent Home, byte-durable across restarts) and `native` (an explicit delegation marker).
- **Guarantee levels** (snake_case wire strings, verbatim): `managed_dir_byte_durable` and `home_persistence_only`.
- The memory commands' outputs are reachable via `--json` (attach and detach emit versioned documents); the documents are frozen key-set-for-key-set by the compatibility tests, and the freeze landed as the ONE announced key-set edit (announced in the release notes). There is deliberately NO memory read verb in v1 — adding one (or touching these key-sets again) is a post-v1 change under the policy below, explicitly out of scope at the freeze.

Adapters that cannot consume a memory directory are fine: the reserved `memory.dir` key is delivered only through the adapter's own declared mapping (delivery is offered, never imposed), and an unmapped key is delivered nowhere — never silently dropped into a file the agent will not read (R3).

## Configuration delivery

The `[config]` mapping turns documented unified keys into native mechanisms (`env`, `flag`, `file`). v1's normative stances from the second-agent validation:

- **JSON-native config agents** (R5): the documented pattern is **env-content delivery** — the agent's config-content environment variable carries the runtime config (e.g. opencode's `OPENCODE_CONFIG_CONTENT`). A format-qualified `file` target (JSON alongside TOML) is **reserved post-v1**; the current `file` target renders TOML only.
- **`{env:VAR}` substitution honesty**: agents that substitute `{env:VAR}` placeholders and render an unset variable as a silent empty string impose a duty on the adapter — every substituted variable must be guaranteed set, or the render must fail with a named reason, before the child launches. This is the freeze's only behavioral rider; a silently-empty rendered config is a contract violation.
- **Self-updating agents** must have their update-disable mechanism mapped and delivered by the start seam, and adapter docs must state whether the agent's config chain contains a higher-authority (managed/MDM) layer that could override supervisor-delivered values (so an operator can verify the pin survived).
- **Isolation keys** (R6/R9): adapters document the environment levers that give each instance its own data/config roots (Hermes: `HERMES_HOME`; XDG-based agents: `XDG_DATA_HOME` + `XDG_CONFIG_HOME`). **Per-instance isolated roots inside each Agent Home are the recommended normative posture** — they make shared-root concurrency out of contract scope. Where a lever is an undocumented agent interface, the adapter must say so and re-validate per pinned release (undocumented levers are acceptable with that per-release obligation, not silently).

## Versioning

The contract carries a semantic version; **the engine states which contract versions it accepts and rejects mismatches informatively** (FR-30):

- **The rule: a manifest is compatible iff its `contract_version` major matches the engine's contract major.** A different major always fails registration, naming BOTH versions and quoting the rule. Read the rule precisely: major-match is **necessary but not sufficient**. It guarantees only that an engine will never reject a manifest SOLELY for version distance within the major — an older engine still enforces its own frozen schema, so a NEWER same-major manifest using a post-`1.0.0` section fails that engine's unknown-key check. Same-major forward compatibility comes from the schema staying additive within a major, not from the major check alone:

  ```text
  incompatible adapter contract: manifest declares 2.1.0, engine speaks 1.0.0 — compatible iff the major versions match (contract v1 policy, docs/adapter-contract.md#versioning)
  ```

- **Strict `X.Y.Z` parsing** (AI-6, resolved at the freeze): the manifest's `contract_version` must be a strict semver triple — no `v` prefix, no partial versions (`1`, `1.0`). Prerelease and build-metadata suffixes (`1.0.0-rc.1+build.5`) parse as semver; **negotiation compares majors only**, so a same-major prerelease manifest is compatible. Prerelease spellings are development-time conveniences, not a published compatibility promise — release tooling always publishes clean triples.
- **Pre-v1 (`0.x`) manifests are not grandfathered**: the contract was never published under 0.x, so the seed values (0.1.0–0.4.0) carry no back-compat obligation. They fail registration exactly like any other major mismatch.
- **Within a major**, changes are additive (a new optional manifest section, a new enum variant, a new optional JSON field). Anything that removes or renumbers is breaking and requires the next major.
- The Rust API of `ktesio-adapter-api` is guarded by the CI `semver` job (`cargo-semver-checks`); the serialized wire shapes and exit codes are guarded by the workspace compatibility tests (`crates/kt/tests/agent_cli.rs`), which fail CI on an unannounced change on all three OSes.

## Deprecation policy

*Ratified by Islam, 2026-09-04 — quoted verbatim, the whole ratified policy:* **within a major, deprecations announced ≥1 minor ahead via CHANGELOG/RELEASE_NOTES + doc notices; removals only at next major; enforced by semver-checks CI.** The semver-checks enforcement is armed by story 7-4's crates.io publish (see the dormancy note below); until then the announcement rule is enforced by the release-notes discipline itself. The ratified sentence names the Adapter Contract and its docs; it deliberately does not expand to other surfaces — each other surface's own announcement machinery (e.g. the `--json` key-set freeze rule) governs it in the meantime.

## The semver gate is dormant — stated plainly

The CI `semver` job **cannot fire until the crates publish to crates.io** (story 7-4): `cargo-semver-checks` compares against a published baseline, and there is none yet. Today the job checks crates.io, sees the 404, and emits a notice saying exactly that. That is honest surfaced-not-silent: **the gate provides no protection today.** What protects the contract right now are the workspace compatibility tests and the review discipline; the semver gate arms itself automatically at first publish, and its cache plumbing is already version-keyed and armed-ready.

## Ratified checkpoint decisions (recorded)

Story 6-5's paper validation of opencode raised six change proposals (CP-6.5-a…f) and eleven freeze risks (R1–R11). Every one carries an explicit verdict from the 6-6 checkpoint (ratified by Islam, 2026-09-04); none silently dropped. The story file (`_bmad-output/implementation-artifacts/spec-6-6-*.md`, Ratified-decisions section) records the one-line rationale for each; the normative text above is where the ratified decisions live.

| ID | Verdict |
|----|---------|
| CP-6.5-a / R1 | Option (i) ratified: `InteractionChannelKind::Http` added as v1 vocabulary, engine never branches on channel. Option (ii) — relaxing `has_any_support` — DECLINED. HTTP send implementation deferred post-v1. |
| CP-6.5-b / R4 | Ratified (documentary): an omitted `[lifecycle.stop]` means engine signal-termination is the normative stop; TCK asserts child exit. No graceful-acknowledgment concept in v1. |
| CP-6.5-c / R2 | Ratified (documentary): `self-reported` covers adapter-implemented shims; `sequence` derives from the agent's per-Run message ordinals; both sources carry the no-double-count guarantee. |
| CP-6.5-d / R5 | Ratified (documentary): env-content delivery is the documented pattern for JSON-config agents; format-qualified `file` target reserved post-v1. PLUS the one behavioral rider: `{env:VAR}` substitution must guarantee set variables or fail the render with a named reason. |
| CP-6.5-e | Ratified (documentary): self-updating agents must have their update pin mapped and delivered; adapter docs must disclose higher-authority config layers. Optional TCK assertion deferred. |
| CP-6.5-f / R6 | Ratified (docs-only): isolation keys named per agent (opencode: `XDG_DATA_HOME` + `XDG_CONFIG_HOME`) with the churn caveat — undocumented levers acceptable with per-release re-validation. |
| R3 | Ratified: the memory wire froze in v1 — typed snake_case `GuaranteeLevel`/`MemoryBackingKind` strings adopted verbatim on the new `--json` documents (the ONE announced key-set edit); adapters that cannot consume `memory.dir` simply declare no mapping (delivered nowhere, honestly surfaced). |
| R7 | Ratified: unknown-vs-zero stance — omitted usage is surfaced as unknown, never coerced to zero (surfaced-not-silent labels). |
| R8 | Deferred-post-v1: no readiness/liveness distinction or endpoint-discovery surface in v1; pin-the-port or startup-line-parsing are adapter conventions. |
| R9 | Recommended normative (docs-only): per-instance isolated roots for networked adapters; shared-root modes are out of contract scope. |
| R10 | Deferred-post-v1: agent-side abort is out of contract; engine-owned termination semantics only, TCK asserts process-level outcomes. |
| R11 | Docs-only: per-OS honesty is already the declaration's job — a windows `best_effort` cap for WSL-recommended agents is an adapter-author choice, not a contract mandate. |

## What changed at the freeze

For adapter authors upgrading from a pre-freeze seed manifest:

1. Set `contract_version = "1.0.0"`. Any `0.x` value now fails registration with the both-versions error quoted above.
2. `[interaction].channel` accepts `"http"` in addition to `"stdio"` — optional, documentary.
3. The manifest syntax is otherwise unchanged; every 0.x-era section parses identically under 1.0.0.

## See Also

- [Adapter manifest (`adapter.toml`)](manifest.md) — the concrete manifest syntax.
- [Command reference](commands.md) — the `kt agent` commands, including the memory `--json` documents.
- [Architecture](architecture.md) — how the engine, the contract crate, and the conformance kit fit together.
- [The Conformance Test Kit](testing.md#the-conformance-test-kit) — prove your adapter conforms.
