---
title: Troubleshooting
description: Common Ktesio install, registration, config, and lifecycle issues with practical fixes.
---

# Troubleshooting

## Adapter Manifest Not Found or Invalid

`kt agent register --manifest <path>` reports the exact problem and writes nothing when a manifest is missing or invalid.

- **Not found** — pass a directory containing `adapter.toml`, or the path to the `adapter.toml` file itself.
- **Invalid** — the error names the first missing or invalid mandatory section (`contract_version`, `[adapter]`, `[lifecycle.start]`, `[capabilities]`, or `[metering]`) or the unknown key it rejected.

See the [adapter manifest reference](manifest.md) for the required shape.

## Agent Won't Start ("no launch command")

The native `mock` kind is a registration/config fixture with no launch command, so `kt agent start` fails for it:

```text
native adapter kind 'mock' has no launch command; supply a manifest adapter
```

Register a **manifest adapter** whose `[lifecycle.start]` declares a real `exec` to start a process.

## Agent Shows `failed` After Starting

A standalone `kt agent start` supervises the process only for that command's lifetime and stops it when the command exits. A later, separate `kt agent list` then reports the instance as `failed` because the supervised process is gone.

This is expected today — durable supervision across separate CLI invocations is future work (a supervising daemon is a later epic). If the engine crashes with a surviving process, the next engine open re-adopts it, detects crashes, and applies the Restart Policy.

## Invalid Lifecycle Transition

Commands are rejected uniformly when they don't apply to the current state (for example, `stop` on an instance that is `registered` or `failed`):

```text
cannot stop an Agent Instance while it is 'registered'
```

Check the current state with `kt agent list` or `kt agent show <name>`, then issue a valid command. To remove a **running** instance, pass `--force`.

## Config Key Rejected

`kt agent config set` validates at write time and changes nothing when a key is rejected. An unknown key outside the `agent.*` pass-through namespace is refused with the nearest valid key suggested:

- Use a known unified key (see [Unified Config Keys](commands.md#unified-config-keys)).
- Or put agent-native extras under the `agent.*` namespace, e.g. `kt agent config set demo agent.temperature 0.2`.

Budget and rate values are validated too: token budgets must parse as integers, and rates/caps must be dollar strings (e.g. `3.00`).

## A Secret Won't Resolve at Start

A `secret:NAME` value is resolved at start from the process environment first, then the engine secrets file at `<state base>/secrets.toml`. If neither provides it, the start is rejected with an error naming the `NAME` and the resolvers tried (never the value). Export the variable or add it to the secrets file, then start again.

On Unix the secrets file must be mode `0600` (owner-only); a group- or world-accessible file is refused with a `chmod 600` remediation.

## Installer Cannot Find `kt` After Installing

When the installer uses a prebuilt binary, it installs into the detected manual
install directory, `KTESIO_INSTALL_DIR`, or a user-local default directory. If
that directory is not on `PATH`, the installer prints a warning with the exact
directory to add.

Run a dry run to see the selected path without installing:

```bash
KTESIO_INSTALL_DRY_RUN=1 curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

Then either add the printed directory to `PATH` or choose an existing directory:

```bash
KTESIO_INSTALL_DIR="$HOME/.local/bin" curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

## Installer Reports an Unsupported OS or Architecture

The prebuilt binary fallback supports macOS Intel, macOS Apple Silicon, Linux
x64, and Windows x64. Other platforms should install with Cargo:

```bash
cargo install ktesio --force
```

If Cargo is unavailable, install Rust from [rustup](https://rustup.rs/) first.

## Installer Checksum Verification Fails

The binary installer downloads both the release archive and its `.sha256` file
from GitHub Releases. A checksum mismatch usually means the download was
interrupted, cached incorrectly, or replaced by a network proxy.

Retry the installer. If the error repeats, download the archive and checksum
from [GitHub Releases](https://github.com/iMagdy/ktesio/releases) directly and
compare them locally before installing.

## Installer Refuses to Overwrite `kt`

The installer checks `kt --version` before replacing an existing `kt` command.
If the command is not Ktesio, the installer stops rather than overwrite another
tool with the same name.

Choose a different install directory and make sure it appears before the other
`kt` command on `PATH`, or remove the conflicting command if it is no longer
needed.

## Update Check Is Unavailable or Unwanted

Ktesio checks GitHub Releases through an hourly cache before running subcommands.
Network failures, cache write failures, and unexpected release responses are
ignored so the requested command can continue.

If you do not want automatic update checks, run commands with:

```bash
KTESIO_NO_UPDATE_CHECK=1 kt agent list
```

Ktesio also skips automatic update checks when `CI=true`.

## Self Update Fails

`kt self-update` is an explicit update action, so it reports failures instead of
ignoring them.

For Homebrew or Cargo installs, re-run the underlying package manager command to
see full diagnostics:

```bash
brew upgrade imagdy/tap/ktesio
cargo install ktesio --force
```

For manual installs, Ktesio downloads the latest release archive and its
`.sha256` file from GitHub Releases. Retry the command if the download was
interrupted. If checksum verification keeps failing, download the archive and
checksum from [GitHub Releases](https://github.com/iMagdy/ktesio/releases) and
compare them locally before replacing the binary.

If your platform does not have a prebuilt release archive, install with Cargo:

```bash
cargo install ktesio --force
```

## Usage Totals Stay Zero

`kt agent usage <name>` (or the usage columns in `list`/`show`) reporting all zeros means no usage was recorded — check the Metering Source:

- **Self-reported** — the agent must emit `KTESIO_USAGE {json}` sentinel lines on its stdout (e.g. `KTESIO_USAGE {"sequence": 0, "input_tokens": 128, "output_tokens": 512}`). Check they are actually reaching stdout: run `kt agent logs <name>` and look for the lines. A malformed JSON payload is silently dropped as a diagnostic, and stdout that is redirected or wrapped by the agent's own tooling may never reach the captured stream.
- **Engine-observed** — the engine meters only traffic pointed at its loopback proxy. Verify the config mapping that points the agent's OpenAI-compatible `base_url` at the engine-injected `metering.base_url` is declared in the manifest, and that `metering.upstream_base_url` names the real provider endpoint (see [Unified Config Keys](commands.md#unified-config-keys)).

An instance that has never started also reports zeros — that is expected.

## Release Workflow Did Not Update Docs

The tag workflow publishes the GitHub Release first, then opens a pull request for `CHANGELOG.md` and `docs/RELEASE_NOTES.md`.

Check the release workflow logs and open pull requests for a branch named like:

```text
release-docs/<tag>
```
