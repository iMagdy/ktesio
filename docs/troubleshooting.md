---
title: Troubleshooting
description: Common Ktesio setup, git, install, upgrade, and release-doc issues with practical fixes.
---

# Troubleshooting

## `skills.json` Not Found

Run:

```bash
kt init .
```

Then add skills manually or with `kt install <name:repo>`.

## Git Clone Fails

Ktesio hides raw git clone progress during normal installs, then prints the useful git error line when a clone fails.

Check that:

- `git` is installed and on `PATH`.
- The repo URL is correct.
- SSH keys or credential helpers are configured for private repositories.
- Your network can reach the remote.

If you need full git diagnostics, run the equivalent `git clone <repo-url>` manually from the same shell.

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

## Installer Package Manager Step Fails

When Homebrew or Cargo is available, the installer lets that package manager do
the install or upgrade. Re-run the printed command directly to see full package
manager diagnostics:

```bash
brew install imagdy/tap/ktesio
cargo install ktesio --force
```

Use `KTESIO_INSTALL_METHOD=binary` to bypass Homebrew and Cargo and install the
prebuilt binary instead.

## Installer Refuses to Overwrite `kt`

The installer checks `kt --version` before replacing an existing `kt` command.
If the command is not Ktesio, the installer stops rather than overwrite another
tool with the same name.

Choose a different install directory and make sure it appears before the other
`kt` command on `PATH`, or remove the conflicting command if it is no longer
needed.

## Update Check Is Unavailable Or Unwanted

Ktesio checks GitHub Releases through an hourly cache before running subcommands.
Network failures, cache write failures, and unexpected release responses are
ignored so the requested command can continue.

If you do not want automatic update checks, run commands with:

```bash
KTESIO_NO_UPDATE_CHECK=1 kt list
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

## Search Is Rate Limited Or Unavailable

`kt search` uses skills.sh for discovery only. If skills.sh returns a rate limit or temporary service failure, Ktesio retries automatically up to 3 total attempts and prints messages such as:

```text
skills.sh rate limit reached; retrying in 12s (attempt 2/3).
```

If all attempts fail, retry later, search less frequently, or configure `KTESIO_SKILLS_SH_API_KEY` after receiving skills.sh API access. Ktesio avoids unbounded retry loops so it can use the public API responsibly.

## Search Result Is Not Installable

Search results from non-GitHub sources are shown as `not installable yet`. Ktesio currently uses skills.sh for discovery and still installs by cloning git repositories.

## Skill Is Listed as Missing

`kt list` reports `missing` when `skills.lock` has an entry but `.agents/skills/<name>/` is absent.

Fix it with:

```bash
kt install
```

## Skill Is Listed as Orphaned

`orphaned` means `skills.lock` has an entry that is no longer in `skills.json`.

Options:

- Add the skill back under `dependencies` if it should be restored to the manifest.
- Remove the stale lock entry by uninstalling or editing the lockfile.

## Project State Looks Wrong

Run:

```bash
kt doctor
```

`kt doctor` checks the manifest, lockfile, installed directories, published local paths, orphaned entries, and git availability, then prints repair hints.

## Release Workflow Did Not Update Docs

The tag workflow publishes the GitHub Release first, then opens a pull request for `CHANGELOG.md` and `docs/RELEASE_NOTES.md`.

Check the release workflow logs and open pull requests for a branch named like:

```text
release-docs/<tag>
```
