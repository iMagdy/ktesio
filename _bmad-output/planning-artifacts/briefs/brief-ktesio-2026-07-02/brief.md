---
title: "Product Brief: Ktesio — Unified Personal-Agent Runtime Engine"
status: updated
created: 2026-07-02
updated: 2026-07-02
---

# Product Brief: Ktesio — Unified Personal-Agent Runtime Engine

> **Update note (2026-07-02).** First drafted headless from Islam's written vision; updated the same day after Islam confirmed the key open questions (Hermes identity, north-star metric, skills-manager fate) and expanded the vision to **dual delivery** — operator CLI plus embeddable engine. **[FIXED]** marks constraints Islam supplied or confirmed; remaining **[ASSUMPTION]** tags mark inferences still awaiting his correction. Open questions are collected at the end.

## Executive Summary

Running a personal AI agent today means living inside whatever runner its authors shipped. Each agent has its own way to start and stop, its own config file, its own idea of memory, and — critically — its own (or no) answer to "how much is this costing me and how do I stop it before it runs away." Run a *second* agent on a different stack and none of that knowledge transfers: you relearn lifecycle, config, and cost controls per agent, with no consistent way to cap spend or operate them as a fleet.

Ktesio is a personal-agent **runtime engine** with dual delivery **[FIXED]**: the `kt` CLI for operators, and an embeddable engine core for platforms and apps that host agents for their users. However it is driven, it gives heterogeneous personal agents one control surface: the same lifecycle commands (start / pause / stop / resume), config model, memory wiring, token-limit controls, and — when you supply a $/1M-token rate — cost caps. Agents plug in through a downward **adapter contract**: the reference adapter integrates NousResearch's **Hermes Agent** **[FIXED — confirmed]**, and the contract is deliberately checked against a second, structurally different agent so the abstraction is not shaped around any single agent. Hosts drive the engine through an upward embedding interface; the CLI is the engine's complete reference frontend.

Why now: the agent landscape has fragmented into single-stack frameworks (LangGraph, CrewAI, the Microsoft Agent Framework), single-agent CLI harnesses (Goose, Aider, OpenHands, Letta Code, Hermes Agent), parallel launchers (Emdash), governance SDKs you embed in code (Microsoft's Agent Governance Toolkit), and proxy-level budget tools (LiteLLM, Helicone). Missing is the piece an operator stands in front of every day: a `pm2`/`systemd`-style **runner** that unifies lifecycle, config, memory, *and* dollar-denominated cost governance across agents built on different stacks. The same gap bites platforms that host agents for their users — each rebuilds supervision, budget enforcement, and memory plumbing in-house. Ktesio is repositioning to be that missing runner: operated directly through a CLI, and embeddable as an engine by hosts. **[FIXED]**

## The Problem

An individual who runs one or more personal agents pays a per-agent tax on everything operational:

- **Lifecycle is inconsistent.** One agent is `docker compose up`, another a bespoke `serve` command, a third a background gateway with its own `start`/`enroll` verbs and no clean `stop` or `status`. There is no single "what is running, pause it, resume it" surface.
- **Config is per-agent and non-portable.** Every agent invents its own file, env vars, and precedence rules; knowledge of agent A does not transfer to agent B.
- **Memory is ad hoc.** "Memory" means different things per agent (conversation buffer, vector store, MemGPT/Letta-style tiers, mem0-style fact extraction), and wiring an agent to a backing is a bespoke exercise each time, if it is exposed at all.
- **Cost and token spend are opaque and hard to bound.** Some agents surface usage after the fact; few let you set a hard ceiling; almost none let you say "cap this at $X given a $/1M-token rate" and enforce it by pausing or stopping the agent. Today's hard-cap tooling (e.g. LiteLLM proxy budgets) lives at the API-gateway layer and rejects individual requests — it does not control the agent process's lifecycle. The operator carries the runaway-cost risk personally.
- **Interaction is bespoke.** Talking to, inspecting, or scripting against each agent differs per agent, so they cannot be treated as a uniform fleet.

The status quo costs cognitive load, per-agent lock-in, and uncontrolled spend: a fixed tax on every new agent adopted, plus the financial risk of an agent looping without a ceiling.

**Target operator (confirmed).** The primary user is a **technically capable individual running personal agents for themselves** (solo operator / power user / indie developer). **[FIXED — confirmed by Islam.]** A second audience carries the same pain at a different altitude: **agent-hosting platforms and apps**, which today rebuild lifecycle supervision, spend controls, and memory plumbing in-house for every agent they host. **[FIXED — dual delivery.]** Team/enterprise *governance* remains adjacent, not primary (see Who This Serves).

## The Solution

Ktesio becomes an agent-agnostic **runner engine**: a core that sits *around* an agent process and imposes one operating model on top of it, delivered two ways **[FIXED]** — operated directly through a single CLI (`kt`), or embedded as an engine by a platform that hosts agents for its users. The operating mental model is uniform no matter which agent is underneath:

- **One lifecycle.** `start`, `pause`, `stop`, `resume` behave the same for every registered agent. **[FIXED]** One command answers "what is running and in what state." **[ASSUMPTION — e.g. `kt ps`/`kt status`; exact verbs are an architecture decision.]**
- **One config model.** A single configuration surface applies across agents; per-agent specifics are expressed through the adapter, not a new file format per agent. **[FIXED — "unified config"]**
- **One way to wire memory.** The operator attaches a memory backing through a consistent interface, regardless of the agent's native memory story. **[FIXED — "memory wiring"]** The supported backings are an open question. **[ASSUMPTION]**
- **One set of guardrails.** Token-limit controls apply uniformly. **[FIXED]** Given a $/1M-token rate, Ktesio derives and enforces a **cost cap** on the underlying agent. **[FIXED]** Enforcing it at the runner/lifecycle level (pause/stop when the ceiling is hit) is the differentiating behavior. **[ASSUMPTION — enforcement action on breach: pause vs stop vs warn is an open design question.]**
- **One interaction interface.** A consistent way to interact with and inspect any running agent. **[FIXED — "consistent interaction interface across heterogeneous agents"]**

**One engine, two surfaces (product-level).** The engine core is decoupled from the CLI. `kt` is the engine's first and *complete* reference frontend — everything the engine can do is reachable via `kt`, preserving the CLI-first gate — while hosts integrate the same engine through an **upward embedding interface** to give their users a resilient, consistent, predictable runtime for personal agents. **[FIXED — dual delivery. The library-core-plus-complete-CLI framing is [ASSUMPTION], to be refined in architecture.]**

**How agents plug in (product-level).** Agents integrate through a documented **adapter contract**: the agent-agnostic control-and-metering core knows nothing about any specific agent, and each agent is made runnable by an adapter satisfying the contract. A **reference adapter** integrates a real agent end-to-end, and the contract is paper-checked against a second, structurally different agent so the model is not "Hermes-shaped." **[FIXED — design directive; architecture belongs in the PRD/architecture phases, not here.]** The adapter contract is the *downward* interface; the embedding interface is its *upward* counterpart — how a host drives the engine. The brief fixes only that both exist and stay distinct.

## Unified Controls as Concrete Capabilities

The core promise — *"the same controls no matter which agent"* — expressed as testable capabilities. (Command names are **[ASSUMPTION]**; the *capabilities* are **[FIXED]** unless noted.)

| Capability | What the operator can do, identically across agents | Source |
|---|---|---|
| Lifecycle control | Start, pause, resume, and stop any registered agent with the same commands | [FIXED] |
| Fleet visibility | See every agent Ktesio manages and its current state | [ASSUMPTION] |
| Unified config | Configure any agent through one consistent config model and precedence | [FIXED] |
| Memory wiring | Attach/detach a memory backing to an agent through one interface | [FIXED]; backings = [ASSUMPTION] |
| Token-limit controls | Set token ceilings that apply uniformly regardless of agent | [FIXED] |
| Cost caps | Provide a $/1M-token rate; Ktesio derives a spend ceiling and enforces it | [FIXED]; enforcement action = [ASSUMPTION] |
| Usage/cost visibility | Read current token and (rate-derived) dollar consumption per agent | [ASSUMPTION] |
| Consistent interaction | Interact with / send input to / inspect any running agent uniformly | [FIXED] |
| Adapter contract | Make a new agent runnable by writing an adapter, without touching the core | [FIXED] |

**Definition of the promise (ratified, testable):** *An operator who has learned Ktesio's controls on one agent can start, pause, resume, stop, configure, wire memory for, bound the token and dollar spend of, and interact with a second, structurally different agent using the same commands and mental model, without reading that agent's native runner docs.* **[FIXED — ratified by Islam as the north-star acceptance statement.]**

## What Makes This Different

Honest differentiation, grounded in a scan of the current landscape (see addendum for sources):

- **Framework-agnostic runner, not another framework.** LangGraph, CrewAI, and the Microsoft Agent Framework each run *their own* agents; Ktesio runs *other people's* agents behind one surface. It does not compete to be the framework you build in.
- **Lifecycle + cost governance in one operator-facing CLI.** The market splits into (a) parallel launchers that start many agents but don't govern spend (Emdash), (b) governance SDKs you embed in code rather than a CLI you operate (Microsoft Agent Governance Toolkit), (c) isolation/packaging runtimes (container-style agent runtimes), and (d) proxy/gateway budget tools that cap *API requests*, not the *agent lifecycle* (LiteLLM, Helicone, Langfuse, Cloudflare AI Gateway). Ktesio's angle — deriving a dollar cap from a token rate and enforcing it on the agent's lifecycle — sits in the gap between these. **[ASSUMPTION — "gap" is research-grounded, not a guarantee no competitor exists; treat as a hypothesis to keep validating.]**
- **Consistency is the product.** The value is not any single control but that *every* control is identical across heterogeneous agents. That uniformity is the moat — an execution/design moat, not a proprietary-technology one, stated honestly.
- **Operator CLI *and* embeddable engine.** The governance SDKs are libraries without an operator surface; the launchers are surfaces without a governing engine. Ktesio ships both from one core: operators get `kt`, hosts embed the engine. **[FIXED — dual delivery]**
- **Adapter-spec-first discipline.** Checking the contract against a second, structurally different agent before declaring it stable hedges against building a single-agent tool wearing a general-purpose costume.

**Candor on the reference agent.** The reference agent is NousResearch's **Hermes Agent**. **[FIXED — confirmed by Islam.]** It already ships its own gateway, supervision, persistent memory, and usage analytics, and is model-agnostic. That makes it an excellent *reference adapter* but also a *partial overlap*: Ktesio must be clearly more valuable **across** agents than Hermes Agent is **for itself**. The differentiator holds only at the fleet/heterogeneity level, and the brief should not claim otherwise.

## Who This Serves

**Primary (confirmed) — the solo agent operator.** **[FIXED]** A technically capable individual who runs one or more personal agents on their own machine or server, is comfortable in a terminal, and wants easy installation, isolation, full control, and predictable cost without mastering each agent's bespoke runner. Success: adopt a new agent and operate it within minutes using controls they already know, never surprised by an unbounded bill.

**Second audience (confirmed) — the agent-hosting platform.** **[FIXED — dual delivery.]** A platform or app that hosts personal agents for its users and embeds the Ktesio engine instead of building supervision, budget enforcement, and memory plumbing in-house. Success: a resilient, consistent, predictable runtime for every agent it hosts — while the host keeps ownership of its own user management, billing, and policy.

**Third audience (candidate) — the agent author.** **[ASSUMPTION]** Someone who builds an agent and writes a Ktesio adapter so their users get lifecycle, config, memory, and cost governance for free. Success: a small, well-documented contract makes their agent a first-class Ktesio citizen.

**Adjacent / not in v1 — team and multi-user *governance*** (shared policy, org budgets, RBAC, audit) — even when the engine is embedded by a platform, that layer belongs to the host. **[ASSUMPTION — non-goals list still awaiting re-confirmation post-expansion.]**

## Success Criteria

The single most important metric is confirmed; supporting targets remain proposals whose **numbers are placeholders [ASSUMPTION]**.

**North-star (confirmed):** **Cross-agent operability** — the share of Ktesio's core controls (lifecycle, config, memory, token limit, cost cap, interaction) that work *identically* across **every** integrated agent. Target: **100% of core controls behave consistently across at least 2 structurally different agents** by first release. This encodes the core promise and the adapter-spec-first directive. **[FIXED — ratified by Islam as the #1 metric.]**

Supporting signals:

- **Cost-cap efficacy:** in test, an agent driven past its dollar cap is reliably paused/stopped (target: 100% of runaway scenarios bounded). **[ASSUMPTION]**
- **Adapter effort:** a competent developer can make a new, structurally different agent fully runnable by writing only an adapter, within a defined time budget (e.g. ≤ 1 day). **[ASSUMPTION]**
- **Second-agent proof:** the contract is validated against an agent that is *not* the reference agent before v1 is done (binary; from the design directive). **[FIXED as a gate; framed as success criterion.]**
- **Engine embeddability:** the CLI itself consumes the engine's public interface — proof a host can too (binary: `kt` is built on the same engine surface a host would embed). **[ASSUMPTION — proposed signal for the dual-delivery mandate.]**
- **Engineering gates (carried, non-negotiable):** every feature reachable via `kt`; test coverage ≥ 95%; Linux/macOS/Windows parity; docs updated with the code; graceful degradation on partial failure. **[FIXED — from the retired constitution / AGENTS.md.]**
- **Adoption (if public):** installs / active operators / agents-run-through-Ktesio. **[ASSUMPTION — depends on whether this is a public product or a personal/portfolio tool; see licensing open question.]**

## Scope

Tight boundary, not a feature list.

**In scope (v1 target).**
- Unified lifecycle: start / pause / stop / resume, plus a state/inspection view. **[FIXED core; inspection = ASSUMPTION.]**
- Unified configuration model across agents. **[FIXED]**
- Memory wiring through a consistent interface. **[FIXED]**
- Token-limit controls. **[FIXED]**
- Cost caps derived from a supplied $/1M-token rate, enforced at the runner level. **[FIXED]**
- Consistent interaction/inspection interface across agents. **[FIXED]**
- An adapter contract + one reference adapter (NousResearch Hermes Agent) + a paper-check against a second, structurally different agent. **[FIXED]**
- Engine/CLI split: the runtime engine as an embeddable core, `kt` as its complete reference frontend, and an upward embedding interface for hosts. **[FIXED — dual delivery; the v1 interface surface is an architecture decision.]**
- Agent-scoped skills provisioning: installing/locking skills *for the managed agent*, reusing the proven v0.5.0 install/lock machinery. **[FIXED]**
- CLI-first delivery via `kt`, cross-platform, ≥95% coverage, current docs, graceful degradation. **[FIXED — durable gates.]**

**Out of scope (v1 non-goals — proposed).** **[ASSUMPTION — Islam to re-confirm the list in light of the embedding expansion.]**
- **An agent framework or authoring toolkit.** Ktesio runs agents; it does not help you *build* agent logic.
- **An LLM/model server or inference engine.** Model hosting (Ollama/LM Studio-style) is out; Ktesio governs agents, not models.
- **Team/multi-user governance:** shared policies, org budgets, RBAC, audit trails, dashboards — including when embedded: the host owns its users, billing, and policy; Ktesio owns the runtime.
- **A hosted control plane / web UI / SaaS *from Ktesio itself*.** Ktesio ships a CLI and an embeddable engine; hosts may build hosted experiences *on top of* the engine — that is the point of embedding, not a Ktesio deliverable.
- **Provider-side hard billing enforcement.** Ktesio enforces caps by acting on the agent it runs (pause/stop) via rate-derived estimation; it does not replace provider billing limits or a gateway proxy, and cost figures are estimates unless an agent reports actuals.
- **Deep per-agent feature parity.** Ktesio exposes the *unified* surface, not every native capability of every agent.
- **General-purpose skills package management.** The standalone v0.5.0 skills manager is deprecated; skills functionality survives only as agent-scoped provisioning (see disposition below). **[FIXED]**

**Legacy skills-manager disposition (confirmed).** The shipping `kt` skills package manager (skills.json / skills.lock / `.agents/skills/`, crates.io + Homebrew, v0.5.0) is **not** carried forward as a general-purpose product. **[FIXED — full repositioning.]** It becomes a **sub-feature focused on provisioning skills to the managed agent** — an agent's "what it knows" — reusing the existing install/lock machinery. The standalone, general-purpose manager is **deprecated**; existing behavior is retired on a published path (freeze features, mark deprecated in docs/release notes, keep existing installs working for a defined window, then retire — mechanics are **[ASSUMPTION]**). The `kt` name and crates.io/Homebrew channels carry over. A concrete migration/communication plan for existing users belongs in the PRD. **[FIXED — Islam's decision on the destination; the deprecation-window mechanics remain proposals.]**

## Vision

If Ktesio succeeds, "which agent are you running?" stops dictating how you operate it. Personal agents become interchangeable workloads under one runner: you adopt one the way you'd adopt a container image — plug it in, and your existing lifecycle, config, memory, and cost controls just apply. Over two to three years the adapter contract grows into a small ecosystem: agent authors ship Ktesio adapters the way tools ship `Dockerfile`s, and "runs under Ktesio" becomes shorthand for "governable, cost-bounded, and consistent to operate." For platforms, "powered by Ktesio" becomes the same promise at a different altitude: hosting personal agents without building a runtime. The uniform control surface — not any single feature — is what makes agents feel like a fleet you command rather than a drawer of incompatible gadgets. **[ASSUMPTION — the embedding intent confirms ecosystem-scale ambition directionally; its commercial scale remains tied to the open licensing/positioning question.]**

## Risks & Mitigations

- **The abstraction leaks (biggest product risk).** Agents differ so much that a "unified" control cannot honestly cover them, and Ktesio becomes lowest-common-denominator. *Mitigation:* the adapter-spec-first + second-agent check is exactly this hedge; keep the guaranteed-uniform surface small and honest, and let adapters expose extras explicitly.
- **Cost-cap fidelity.** Rate-derived caps are estimates; real spend depends on provider accounting the runner may not see, and a cap that *looks* enforced but isn't erodes trust fast. *Mitigation:* state plainly that caps are runner-level and estimate-based unless the agent reports actuals; prefer conservative enforcement; document the boundary loudly.
- **Reference-agent overlap.** Hermes Agent's own gateway/memory/analytics overlap Ktesio's scope, inviting a "why not just use Hermes' own runner" objection. *Mitigation:* lead with heterogeneity; treat single-agent operation as table stakes, not the pitch.
- **Dual-surface scope creep.** One small team shipping both an operator CLI and an embeddable engine risks splitting focus. *Mitigation:* one engine API; `kt` is its first consumer and ships first; host-facing features that smell like governance stay out of scope.
- **Licensing friction for embedders.** PolyForm Noncommercial blocks commercial hosts from embedding — the expansion sharpens the open licensing question rather than settling it. *Mitigation:* resolve licensing/positioning before courting host integrations.
- **Pause/resume semantics across heterogeneous agents.** Not every agent pauses/resumes cleanly (in-flight tool calls, external side effects, gateway connections). *Mitigation:* define what pause/resume *guarantees* vs *best-effort* in the contract; degrade gracefully (a durable-gate requirement anyway).
- **Repositioning cost & user disruption.** Existing skills-manager users are on a different value prop; the pivot can strand them and burn goodwill. *Mitigation:* explicit deprecation/migration plan in the PRD; decide subordinate-feature vs deprecate early.
- **Cross-platform lifecycle control.** Process supervision differs on Windows vs Unix, against the ≥95% coverage and 3-OS parity gates. *Mitigation:* surface early as an architecture constraint.

## Open Questions

Resolved 2026-07-02: target operator (solo, confirmed), Hermes identity (NousResearch Hermes Agent), north-star metric (cross-agent operability), skills-manager fate (agent-scoped provisioning sub-feature). Still open, prioritized:

1. **Second test agent** — which structurally different agent the adapter contract is checked against (architecture-blocking; addendum §G suggests one *unlike* Hermes: no gateway, no native memory).
2. **Licensing/positioning** — PolyForm Noncommercial today; platform embedding makes the commercial question concrete (blocks commercial hosts as-is), and it carries the public-product-vs-personal-tool question with it.
3. **Cost-cap breach behavior** — pause / stop / warn; estimate-based enforcement vs reconciliation with provider actuals.
4. **v1 memory backings** — which backings the memory-wiring interface supports first.
5. **Non-goals re-confirmation** — the list above, re-read in light of the embedding expansion.

## Assumptions Register

Every inference still standing in this brief, collected for fast correction. (Resolved on 2026-07-02 and removed from this register: Hermes identity; primary user; north-star metric; skills-manager fate.)

1. **[ASSUMPTION]** Non-goals list (framework, model server, team/host governance, hosted SaaS *from Ktesio itself*, provider billing enforcement, deep per-agent parity, general-purpose package management) — proposed, awaiting re-confirmation post-expansion.
2. **[ASSUMPTION]** Cost-cap breach action (pause/stop/warn) and the enforcement mechanism are unspecified.
3. **[ASSUMPTION]** Command names (`kt ps`/`status`/etc.) are illustrative; only capabilities are fixed.
4. **[ASSUMPTION]** Licensing/positioning (PolyForm Noncommercial) is unresolved; platform embedding makes the commercial dimension concrete.
5. **[ASSUMPTION]** Supported memory backings are unspecified.
6. **[ASSUMPTION]** The market-gap claim is a research-grounded hypothesis, not a proof of no competitor.
7. **[ASSUMPTION]** The CLI-first reconciliation — engine as embeddable core, `kt` as its complete reference frontend, embedding as an additional delivery vehicle rather than a CLI bypass — is a proposed framing, to be refined in architecture.
8. **[ASSUMPTION]** The upward embedding interface's v1 surface (Rust library API vs also IPC/service) is an architecture decision; the brief fixes only its existence and separation from the adapter contract.
9. **[ASSUMPTION]** Deprecation-window mechanics for the legacy skills manager (freeze/notice/window/retire) are proposed; Islam fixed the destination (agent-scoped sub-feature), not the mechanics.
10. **[ASSUMPTION]** Numeric targets in Success Criteria are placeholders.

---

*Fixed constraints (`[FIXED]`) were supplied or confirmed by Islam — updated 2026-07-02 with his answers on Hermes identity, the north-star metric, skills-manager fate, and the dual-delivery engine expansion. Assumptions (`[ASSUMPTION]`) are inferences awaiting correction. Sources for the landscape scan and the Hermes analysis are in `addendum.md`.*
