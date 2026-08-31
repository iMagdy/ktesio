---
title: Installation
description: Install Ktesio on macOS, Linux, or Windows with the hosted installer, Homebrew, Cargo, release archives, or source builds.
---

# Installation

Ktesio is a Rust CLI. It works on macOS, Linux, and Windows with no runtime dependencies beyond the operating system.

## Prerequisites

- None for the installed binary. Rust is only required when installing through Cargo or from source.

## Install with the Installer

On macOS or Linux:

```bash
curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

On Windows with PowerShell:

```powershell
irm https://cli.ktesio.dev/install.ps1 | iex
```

The installer preserves an existing Ktesio install channel when it can:

- Homebrew installs are updated with `brew upgrade imagdy/tap/ktesio`.
- Cargo installs are updated with `cargo install ktesio --force`.
- Manual binary installs are replaced in their existing writable directory.

For new macOS and Linux installs, the installer prefers Homebrew, then Cargo,
then a prebuilt GitHub Release binary. For new Windows installs, it prefers
Cargo, then a prebuilt GitHub Release binary.

Installer overrides:

```bash
KTESIO_INSTALL_METHOD=binary curl -fsSL https://cli.ktesio.dev/install.sh | sh
KTESIO_INSTALL_DIR="$HOME/.local/bin" curl -fsSL https://cli.ktesio.dev/install.sh | sh
KTESIO_INSTALL_DRY_RUN=1 curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

`KTESIO_INSTALL_METHOD` accepts `auto`, `brew`, `cargo`, or `binary` on macOS
and Linux. Windows accepts `auto`, `cargo`, or `binary`.

The installer does not install Homebrew, Rust, Cargo, Git, or shell profile
entries. If it installs a binary into a directory that is not on `PATH`, it
prints the directory to add.

## Install from Source

```bash
git clone https://github.com/iMagdy/ktesio.git
cd ktesio
cargo install --path .
```

Verify:

```bash
kt --version
kt --help
```

## Install from crates.io

```bash
cargo install ktesio
```

The crates.io package is named `ktesio`; it installs the `kt` binary.

## Install from a Release

Download the archive for your platform from [GitHub Releases](https://github.com/iMagdy/ktesio/releases), then unpack it and place the `kt` binary on your `PATH`.

Release archives use this naming pattern:

```text
ktesio-<tag>-<target>.tar.gz
ktesio-<tag>-<target>.zip
```

Each release also includes `.sha256` files and an aggregate checksum file.

## Install with Homebrew

After a release is published to the Homebrew tap:

```bash
brew install imagdy/tap/ktesio
```

The formula installs the prebuilt macOS or Linux release archive for your platform.

## Updating Ktesio

Ktesio checks GitHub Releases through an hourly cache when a subcommand runs. If a
newer release is available, it prints a small stderr notice that asks you to run:

```bash
kt self-update
```

`kt self-update` preserves the current install channel automatically. Homebrew
installs upgrade with Homebrew, Cargo installs upgrade with Cargo, and manual
release installs download the latest GitHub Release archive, verify its
`.sha256` checksum, and replace the current binary.

Set `KTESIO_NO_UPDATE_CHECK=1` to skip automatic update checks.

## Platform Notes

- macOS may require Xcode Command Line Tools when building from source.
- Windows users building from source need the MSVC build tools (see [Rust's Windows setup](https://rustup.rs/)).
- Linux users may need standard build tools for Rust crates.

## Next Steps

- [Getting started](get-started.md)
- [Command reference](commands.md)
