# Epic 5 Context: Wire Memory Consistently

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Give every agent one way to wire memory: the Operator attaches or detaches a Memory Backing with identical commands regardless of the underlying agent, choosing between an engine-managed directory under Ktesio's control or explicit delegation to the agent's own memory. This removes bespoke per-agent memory plumbing, keeps the guarantee-versus-delegation boundary honest so an Operator always knows what Ktesio actually promises, and ensures the managed backing survives restarts byte-identically and travels with the Agent Home.

## Stories

- Story 5.1: Attach a managed filesystem Memory Backing
- Story 5.2: Delegate to native memory with an explicit boundary

## Requirements & Constraints

- Exactly one Memory Backing per Agent Instance, attached/detached through the same command sequence for every agent kind (mock, manifest, native).
- No hot-swap: attach/detach only while the instance is not running — permitted only in persisted terminal states (`registered`/`stopped`/`failed`) — with no force flag to escape this.
- Detach is metadata-only: it must never delete operator data.
- Two v1 backing kinds only:
  - `filesystem`: an engine-managed directory inside the Agent Home whose contents survive stop/start cycles and engine restarts byte-identically.
  - `native`: an explicit delegation marker; Ktesio guarantees only Agent Home persistence.
- Delegation is visible, never implicit: choosing `native` records the delegation where the Operator can see it (effective config / backing status).
- The guarantees-vs-delegation boundary is stated in docs **and** in command output; docs update in the same change as the code.
- Portability: a documented procedure copies an Agent Home to another machine and a `filesystem` backing travels with it — the agent runs there with memory intact.
- Standing gates apply: cross-platform parity (Linux/macOS/Windows), partial failures name instance + reason + remediation, test coverage ≥95%.

## Technical Decisions

- Memory wiring lives behind the `MemoryBacking` side port of the hexagonal engine core; richer backings (vector stores, tiered) are deferred behind that same port.
- Delivery goes through the existing layered-config seam, not through the Adapter Contract: at every start the engine injects the managed path at a reserved engine-namespace unified-config key as an invocation-override layer, and the adapter's already-declared config mapping routes it into the agent's native mechanism. No contract version bump and no new spawn/launch fields.
- Three levels are never collapsed: *guaranteed* (the directory exists inside the Agent Home, persists byte-identically, travels with the home), *offered* (the path is injected at the reserved key each start), *delegated* (whether the agent receives it — the adapter must declare a mapping for the key, and an unmapped key is a silent no-op — plus whatever it writes there).
- Because receipt is the adapter's choice, honesty is mandatory: starting with a `filesystem` backing attached but no declared target for the reserved key emits a diagnostic notice on stderr, and backing status/read reports the undelivered state. Silence would be false assurance.
- The injected override is a delivery mechanism, not operator configuration — it is never persisted into the effective-config snapshot.
- The managed directory is a file artifact inside the Agent Home, never a database blob; the engine remains the sole filesystem path authority, and any new Agent Home layout entry is recorded in the path-authority module's layout documentation in the same commit.
- New operator verbs land as a noun group under `kt agent memory …` (at most one nesting level), never as flags on register/start and never a new top-level command; they reuse the frozen exit-code table without adding codes. Glossary names (`MemoryBacking`, `AgentHome`, …) appear verbatim in code, events, and docs.

## Cross-Story Dependencies

- Builds directly on Epic 1 (instance registration, Agent Home, lifecycle states gate when attach/detach is allowed) and Epic 2 (the layered config system supplies the injection seam; the adapter config-mapping behavior from that epic determines delivered vs undelivered). Independent of Epic 4.
- Story 5.2 reuses the attach/detach surface built in Story 5.1 and extends it with the `native` kind and the portability documentation.
- Epic 6 consumes this epic end-to-end: the Hermes reference journey wires memory (both kinds) through these standard commands, and the conformance test-kit exercises memory attachment against any adapter.
