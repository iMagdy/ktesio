# Hermes Agent — Primary-Source Verification Note (Story 6.1)

<!-- BMAD Epic 6 / Story 6.1 deliverable. Closes the §H caveat on brief
     brief-ktesio-2026-07-02/addendum.md (§C Hermes analysis rests on search
     excerpts; this note re-verifies every claim against fetched pages and the
     repository source). -->

**Verified:** 2026-08-25
**Sources:** official docs at <https://hermes-agent.nousresearch.com/docs> (pages fetched directly, plus its machine-readable `/llms.txt` index) and a clone of <https://github.com/NousResearch/hermes-agent> at **HEAD `41447a6d7063b2772b0c2f26a5b22d9bd444fb43` (2026-08-25, v0.20.5)**. Repo citations below are `path:line` at that SHA.
**Freshness:** upstream moves fast (single-repo, near-daily merges). Facts below are pinned to the SHA above; re-check before relying on any single line for adapter code in 6.2+.

**Verdict up front:** every §C claim that matters to the Adapter Contract **confirmed**, three **corrected/refined**, and six **contract-impacting findings** requiring change proposals before Story 6.2 (listed last).

---

## 1. Identity

**CONFIRMED.** NousResearch/hermes-agent, Python agent ("the agent that grows with you"), distinct from the Hermes 2/3 fine-tuned LLMs. Model-agnostic across ~30 documented first-class/custom/self-hosted providers (Nous Portal, Anthropic native, GitHub Copilot, Bedrock, Vertex, Ollama/vLLM/llama.cpp/LM Studio local stacks, arbitrary OpenAI-compatible endpoints via `model.base_url`) — `website/docs/integrations/providers.md`.

## 2. Gateway / process model

**CONFIRMED, with one refinement.**

| Claim (§C) | Verdict | Evidence |
|---|---|---|
| One long-running gateway process per profile | ✅ | Default deployment installs a per-profile OS service: launchd (`ai.hermes.gateway-<name>.plist`), systemd (`hermes-gateway-<name>.service`), Windows Scheduled Task (`schtasks /SC ONLOGON /TN HermesGateway`, Startup-folder fallback) — `website/docs/user-guide/windows-native.md:169–180`, `hermes_cli/gateway.py` |
| Opt-in profile multiplexing | ✅ | `gateway.multiplex_profiles: true` set on the *default* profile only; secondary gateways hard-error while the multiplexer runs — `website/docs/user-guide/multi-profile-gateways.md:86–130` |
| s6-overlay supervision in Docker | ✅ | s6 is PID 1 supervising `main-hermes` + optional dashboard; `gateway run` CMD is a `sleep infinity` heartbeat — `website/docs/user-guide/docker.md:62–66`, `docker/s6-rc.d/`, `tools/docker.py`. Per-profile dynamic service slots (`/run/service/gateway-<name>/`). Pre-s6 images ran foreground instead. |
| Cron scheduler lives in the gateway | ✅ | Built-in ticker `TICKER_INTERVAL_SECONDS = 60` runs inside the gateway process — `cron/jobs.py:99`; pluggable trigger via `cron.provider`. |

**Refinement:** the gateway publishes a 30s liveness heartbeat (`state/gateway.heartbeat`, #66892) and supports an optional systemd notify watchdog (`gateway.systemd_watchdog_seconds`, `gateway/systemd_notify.py`, Type=notify). Relevant to Ktesio's crash detection.

## 3. Lifecycle verbs

**CONFIRMED and EXPANDED — §C's "no native stop/status" is WRONG.**

Full parser surface (`hermes_cli/subcommands/gateway.py:39–314`, mirrored in `website/docs/reference/cli-commands.md:234–260`):

- `run` — **foreground**, recommended for WSL/Docker/Termux. Flags: `--replace`, `--force` (refuses when a service already supervises the profile), `--no-supervise` (s6 opt-out), `--external-supervisor`.
- `start` / `stop [--system] [--all]` / `restart` / `status [--deep] [-l/--full]` / `list` (all profiles w/ PIDs)
- `install [--force --system --start-now --start-on-login …]` / `uninstall`
- `setup` (messaging platforms), `migrate-legacy`, `enroll` (relay connector credentials → `.env`)

Stop semantics (`gateway/status.py:305–330`): POSIX SIGTERM→grace→SIGKILL; **Windows force-stop is already a true tree kill** (`taskkill /PID <pid> /T /F`, CREATE_NO_WINDOW). Wedged-loop escalation bounds the stop at ~10s (heartbeat-detected, #81642). Drain budgets: `agent.restart_drain_timeout` defaults **0** (interrupt immediately; several comments still cite a legacy "180s default"), cron gets its own floor `cron_drain_timeout: 30`, in-band restart waits `restart_after_turn_timeout: 1800` for active turns first — `hermes_cli/config_defaults.py:90–118`, `gateway/restart.py`.

In-chat restart contract: SIGUSR1 → drain → **exit code 75** back to the supervisor (`--external-supervisor`, RestartPreventExitStatus guidance) — `website/docs/reference/cli-commands.md:258–266`, `hermes_cli/gateway.py:283–300`.

**Pause (the big one): `hermes pause` / `hermes resume` exist** — top-level subcommands (`hermes_cli/subcommands/pause.py`, parser wired at `main.py:13029`). Semantics: writes an ESTOP sentinel at `$HERMES_HOME/ESTOP`; halts **NEW work only** (cron dispatch `cron/scheduler.py:7326`, kanban spawns `gateway/kanban_watchers.py:66–74`, new gateway turns `gateway/run.py:16502+`). *"In-flight work is NEVER killed."* A `/estop` chat command mirrors it. **Zero SIGSTOP/SIGCONT anywhere in the tree** (grep-verified). §C said pause didn't exist; it does — but as a cooperative sentinel, not a signal freeze.

## 4. Config mechanism

**CONFIRMED.**

- Layout: `$HERMES_HOME/config.yaml` + `.env` (secrets) + `auth.json` (OAuth); precedence CLI args > config.yaml > .env > defaults; `${VAR}` substitution supported — `website/docs/user-guide/configuration.md`.
- **`HERMES_HOME` env var is honored everywhere** through one resolver: context override → `HERMES_HOME` → platform default (`~/.hermes` POSIX, `%LOCALAPPDATA%\hermes` Windows) — `hermes_constants.py:45–140`. Subprocess spawners are expected to propagate it explicitly.
- Isolation flags for third-party integrations: `--ignore-user-config` (skip config.yaml, keep .env), `--ignore-rules` (skip AGENTS.md/SOUL/memory/skills injection), `--safe-mode` (both + plugins/hooks/MCP off) — `website/docs/reference/cli-commands.md:128–143`.
- Profiles = isolated HERMES_HOMEs. Provider endpoints live in config.yaml (`model.base_url`), NOT `.env`; legacy top-level `custom_providers:` auto-migrates to a `providers:` dict — `providers.md:305–360`.
- `hermes backup` zips the whole home dir; SessionDB export/import exists (`hermes_state_portability.py`) → portability story support.

## 5. Usage / analytics surface

**CONFIRMED.**

- `/usage` slash command (all surfaces incl. messaging): tokens, input/output cost breakdown, context-window state, duration, provider account-limits where available — `website/docs/reference/slash-commands.md:131,245`.
- `/insights [days]` + CLI `hermes insights --days N --source PLATFORM`: InsightsEngine over the SQLite state DB — token/cost/tool/activity/model/platform breakdowns — `agent/insights.py:97–300`, `hermes_cli/subcommands/insights.py`.
- Cost honesty is a design invariant: `CostStatus ∈ {actual, estimated, included, unknown}`, `CostSource` names the pricing provenance, sub-cent amounts render at 4dp (`~$0.0000` forbidden), unknown-pricing labeled "included"/"n/a" rather than "$0" — `agent/usage_pricing.py:40–61,1510–1545`, `agent/insights.py:36–48` (regression-fixed via #79220).
- The agent itself tracks cache-read/write + reasoning tokens + `estimated_cost_usd` (`run_agent.py` AIAgent).

**No $-denominated cap or enforcement anywhere** — only token/iteration guardrails: IterationBudget parent 500 / subagent 50 (`agent/iteration_budget.py`), `loops.max_ticks` 100 (`hermes_cli/loops.py:27`), tool loop caps (web_search 50 etc.), `tool_loop_guardrails.hard_stop_enabled` default **false** (recommended true unattended — `configuration.md:1724–1738`). §C's "no $-cap" stands; Ktesio's BudgetEvaluator remains genuinely additive.

## 6. Interaction channels

**CONFIRMED.**

- Messaging gateway: 35 platform docs shipped; index matrix lists 20+ platforms (Telegram, Discord, Slack, WhatsApp(+Cloud), Signal, SMS, Email, Home Assistant, Mattermost, Matrix, DingTalk, Feishu, WeCom, BlueBubbles/iMessage, QQ, Teams, LINE, ntfy, webhooks, OpenAI-compatible frontends…) with explicit per-platform capability flags (voice/images/files/threads/reactions/typing/streaming) — `website/docs/user-guide/messaging/index.md`, `BasePlatformAdapter` capability attrs (`supports_code_blocks`, `supports_async_delivery`, `splits_long_messages`, …) at `gateway/platforms/base.py:2890+`.
- Programmatic protocols (three, all driving the same AIAgent core — `website/docs/developer-guide/programmatic-integration.md`):
  1. **ACP** — JSON-RPC over stdio for IDE clients (`acp_adapter/`, `hermes acp`);
  2. **TUI gateway JSON-RPC** — stdio/WebSocket, full method catalog (`prompt.submit`, `session.interrupt`, `session.usage`, `approval.respond`, …);
  3. **OpenAI-compatible API server** — HTTP+SSE (`POST /v1/chat/completions`, `/v1/runs/{id}/steer|stop|approval`, `GET /v1/capabilities` machine-readable flags).
- Plus: MCP client (`mcp_servers` config) **and server mode** (`hermes mcp serve` exposes conversations over MCP — `hermes_cli/subcommands/mcp.py:19–24`); webhooks; peer-to-peer DM; Bot Mode; `hermes send` (one-shot outbound delivery, no LLM, Unix exit codes); `hermes serve` headless backend (what the Desktop app drives).
- Terminal execution backends behind one ABC: Local/Docker/SSH/Daytona/Modal/Singularity/Vercel Sandbox (`tools/environments/base.py:650 BaseEnvironment`).

## 7. Memory

**CONFIRMED.**

- Native stores bounded: MEMORY.md 2200 chars (~800 tok) + USER.md 1375 chars (~500 tok) under `~/.hermes/memories/`, injected frozen at session start; `memory` tool add/replace/remove; overflow errors loudly, no silent truncation — `website/docs/user-guide/features/memory.md:20–23`, `agent/agent_init.py:1859–1860`.
- 8 external provider plugins (Honcho, Mem0, Hindsight, …), ONE active, additive.
- **Explicit warning: ONE AGENT PER HERMES HOME** — two writers compound each other's memory entries; give a second agent its own profile/home — `memory.md:22–23`. Directly validates Ktesio's per-instance Agent Home isolation.

---

## 8. Corrections vs. brief §C

| §C claim | Ruling |
|---|---|
| "No native `stop`/`status`" | ❌ **WRONG** — `hermes gateway stop/restart/status [--deep]/list` are first-class, cross-platform, service-aware. |
| Lifecycle = `run/start/setup/enroll` only | ⚠️ Understated — full verb set above, plus `install/uninstall/migrate-legacy/list`. |
| Pause absent | ⚠️ Corrected — ESTOP sentinel pause exists (new-work-only, never kills in-flight). No signal-based pause exists. |
| Gateway supervises channels from one process | ✅ Confirmed (per-profile by default; opt-in multiplexing; s6 in Docker). |
| `/usage` + analytics for token/cost breakdown | ✅ Confirmed (+ honest cost-status labels; SQLite-backed insights). |
| Loop/iteration guardrails, hard-stop in unattended mode | ✅ Confirmed, with nuance: hard_stop_enabled defaults **false** even in gateways; docs recommend enabling it unattended. |
| Model-agnostic via `hermes model` | ✅ Confirmed (~30 providers incl. custom OpenAI-compatible endpoints). |
| Distinct from Hermes 2/3 models | ✅ Confirmed. |

## 9. Contract-impacting surprises → Adapter Contract change proposals (gate before Story 6.2)

1. **CP-6.1-a (pause declaration):** declare Hermes pause as **best-effort / new-work-only** (sentinel ESTOP), never `pause: guaranteed-via-signal`. The Capability Declaration vocabulary needs a phrasing that distinguishes "in-flight turns keep running" from "processes frozen".
2. **CP-6.1-b (supervision ownership):** Ktesio should spawn **foreground** `hermes gateway run` under its own ProcessBackend and pass `--external-supervisor` so Hermes' in-chat restarts/updates **exit with code 75** and let Ktesio relaunch — instead of fighting launchd/systemd ownership. Contract question: does the lifecycle section need an "external-supervisor handoff / exit-code contract" concept?
3. **CP-6.1-c (Windows stop alignment):** Hermes' own force-stop already tree-kills (`taskkill /T /F`); Ktesio's Job Object backend composes cleanly. No conflict, but the conformance TCK should assert tree-kill parity rather than assume adapters delegate it.
4. **CP-6.1-d (metering):** self-reported usage is rich and honestly labeled but has **no $-denominated cap** — BudgetEvaluator stays additive. Reconciliation must be idempotent over batched reports; the metering source mapping should name which Hermes surfaces feed it (`/usage`, insights DB).
5. **CP-6.1-e (home isolation lever):** `HERMES_HOME` resolution chain means Ktesio can point each instance at its own home inside its Agent Home — satisfies Hermes' one-agent-per-home warning and keeps state inside the managed dir. Config-mapping section should treat `HERMES_HOME` as the canonical isolation key.
6. **CP-6.1-f (config delivery seam):** Hermes honors CLI args (highest precedence) *and* config.yaml. Decision needed in 6.2: does the reserved engine-namespace key arrive as an invocation override (CLI arg) or via generated config.yaml? 3-4's metering.base_url precedent suggests invocation override; memory.dir (5-1) already rides that path.

## 10. Residual risks

- Single-commit shallow clone: history-dependent claims (e.g., whether the stale "180s default" drain comments reflect older releases) were resolved from code-as-truth at this SHA, not archaeology.
- Docs site pages drift independently of master; line numbers above pin today's layout.
- Not verified hands-on (no live gateway run) — 6.2's integration work will exercise everything above against reality.
