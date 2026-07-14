# Ktesio banner regeneration brief

Tracks GitHub issue #71 / action item AI-52. The banner at `docs/assets/ktesio-banner.jpg`
still advertises the retired skill-manager positioning; Epic 9 (merged to main as `39f0a57`)
finished repositioning the shipped product, so the banner is now the one visibly stale asset.
This brief specifies exactly what to change and what to keep, for whoever generates the
replacement (a person + image tool, or another AI session with image-generation capability).

## Keep unchanged (the identity — don't redesign this)

- **Aesthetic**: aged papyrus/parchment background, weathered/torn edges, Egyptian
  hieroglyphic border motif, winged solar disk at top-center.
- **Wordmark**: the geometric hex-node "Ktesio" logo mark + logotype, unchanged.
- **Central illustration**: the ornate urn/canister icon in the middle — keep the ancient-tech
  vessel motif (a nod to Ctesibius, the ancient Greek engineer the name references).
- **Six radiating capability bubbles** around the urn, connected by dashed lines: *Code,
  Reasoning, Communication, Data, Integration, Automation*. These describe generic AI-agent
  capabilities (what an agent DOES) — Ktesio supervises agents regardless of what they do
  internally, so this row of concepts is still accurate and can stay as-is.
- **Overall layout**: wordmark + tagline + icon row on the left, urn + bubbles center, a
  scroll-styled command-reference panel on the right, hieroglyph flourishes throughout.

## Change (everything naming the retired skill-manager surface)

### 1. Tagline
- **Current**: "Share, install, and manage agent skills."
- **New**: "Run AI agents like services." (matches the shipped README H1 and the live GitHub
  repo description verbatim — see Source of truth below.)

### 2. Icon + label row (currently 4: Share skills / Install instantly / Manage easily / CLI first)
Replace with four concepts matching the actual product surface (icon style — simple line-art
badge matching the existing set — is a design choice; the labels/concepts are the spec):
- **Supervise lifecycle** — register, start, stop, pause/resume any agent
- **Meter real usage** — real token counts, not estimates
- **Enforce budgets** — dollar/token caps that stop runaway spend
- **CLI first** — keep this one as-is; still accurate

### 3. Scroll command-reference panel
- **Current** (stale — these commands no longer exist): `ktesio search <skill>`,
  `ktesio install <skill>`, `ktesio list`, `ktesio --help`
- **New** (the real, current `kt agent` surface, verified against
  `crates/kt/src/cli/agent.rs` on main @ `39f0a57`):
  ```
  $ kt agent register <name> --kind <adapter>
  Register a new Agent Instance.

  $ kt agent start <name>
  Start it under supervision.

  $ kt agent list
  See the whole Fleet at a glance.

  $ kt --help
  Learn more.
  ```
  (If space is tight, `register`+`start`+`list` is the minimum meaningful trio — `list`
  replaces the old skill-browsing `list`, now showing the Fleet instead of installed skills.)

## Source of truth (don't invent new copy — pull from these)

- README.md (H1 + opening paragraph) — the canonical positioning language.
- Live GitHub repo description (`gh repo view --json description`): *"A Rust CLI + engine
  that runs AI agents like services — supervise their lifecycle, meter real token usage, and
  enforce dollar budgets."*
- `crates/kt/src/main.rs` — the clap `about` string (rebranded in Story 9-2, commit `45d203e`):
  *"Run AI agents like services — supervise, meter, and budget them."*
- `crates/kt/src/cli/agent.rs` — the actual `kt agent <subcommand>` surface, for the scroll text.

## Out of scope for this brief

- The `crates.io v0.5.0` badge in README.md will self-correct once `ktesio` is actually
  published to crates.io at 0.6.0 (a separate release step — not a banner concern).
- No other README/docs text needs to change; only the banner image itself is stale.
