<p align="center">
  <img src="docs/assets/ktesio-banner.jpg" alt="Ktesio banner: Share, install, and manage agent skills" width="100%">
</p>

# Ktesio

[![CI](https://github.com/iMagdy/ktesio/actions/workflows/ci.yml/badge.svg)](https://github.com/iMagdy/ktesio/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/ktesio.svg)](https://crates.io/crates/ktesio)
[![License](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-orange.svg)](LICENSE)

Ktesio is a tiny Rust CLI for installing and sharing agent skills. It makes reusable agent instructions portable across projects by keeping a simple `skills.json` manifest, a reproducible `skills.lock`, and installed skills under `.agents/skills/`.

## Why Ktesio?

- **Portable skills**: move agent workflows between repositories without manual copy-paste.
- **Git-native distribution**: install skills from normal HTTPS or SSH git repositories.
- **Reproducible installs**: lock every installed skill to the exact commit that was fetched.
- **Friendly project state**: list, inspect, upgrade, publish, and remove skills from one CLI.
- **Polished terminal UX**: color-coded statuses, icons, and progress bars keep git work readable.
- **Agent-ready layout**: installed content lands where coding agents already look for skills.

## 60-Second Quickstart

Install Ktesio on macOS or Linux:

```bash
curl -fsSL https://cli.ktesio.dev/install.sh | sh
```

Install Ktesio on Windows with PowerShell:

```powershell
irm https://cli.ktesio.dev/install.ps1 | iex
```

The installer preserves existing Ktesio install channels when it can. New
macOS and Linux installs prefer Homebrew, then Cargo, then a prebuilt GitHub
Release binary. New Windows installs prefer Cargo, then a prebuilt GitHub
Release binary.

If you already have Rust, you can also install from crates.io:

```bash
cargo install ktesio
```

Or install with Homebrew:

```bash
brew install imagdy/tap/ktesio
```

You can also download a release archive from [GitHub Releases](https://github.com/iMagdy/ktesio/releases), unpack it, and place `kt` on your `PATH`.

Or install from the source repository:

```bash
git clone https://github.com/iMagdy/ktesio.git
cd ktesio
cargo install --path .
```

Then, in a project where you want to use agent skills:

```bash
kt init .
# Replace docs:example/agent-docs with skill_name:github_user/github_repo.
kt install docs:example/agent-docs
kt search tests
kt list
```

This creates:

```text
skills.json
skills.lock
.agents/skills/
```

During install and upgrade, Ktesio shows progress bars for long-running git work and hides raw `git clone` or `git fetch` output unless an error needs to be summarized.

## Commands

| Command | Purpose |
|---------|---------|
| `kt init <path>` | Create `skills.json` in a project |
| `kt search <query>` | Search public skill listings from skills.sh |
| `kt install` | Install every dependency declared in `skills.json` |
| `kt install <name:repo>` | Add and install one skill |
| `kt install --all <repo>` | Install all published skills from one repo |
| `kt publish` | Publish local skills from this repo |
| `kt publish add <name> <path>` | Add or update one published local skill |
| `kt upgrade` | Fetch latest commits for installed skills |
| `kt self-update` | Update the kt command itself |
| `kt list` | Show installed, missing, and orphaned skills |
| `kt show <name>` | Show one skill's repo, commit, path, and status |
| `kt doctor` | Validate manifest, lockfile, installed files, and git state |
| `kt uninstall <name>` | Remove a skill from manifest, lockfile, and disk |
| `kt remove <name>` | Alias for `kt uninstall <name>` |

## Manifest

`skills.json` is intentionally small:

```json
{
  "dependencies": {
    "docs": {
      "repo": "https://github.com/example/agent-docs.git",
      "rev": "branch:main"
    },
    "local-docs": {
      "path": ".agents/skills/local-docs"
    }
  },
  "publish": ["local-docs"]
}
```

`dependencies` declares skills this project uses. `publish` declares local skills this repo exposes for other projects to install. Ktesio installs only published paths from source repos; if the source repo has no `skills.json`, it asks before falling back to selectable directories under `skills/`, `SKILLS/`, or `.agents/skills/`.

## Documentation

- [Getting started](docs/get-started.md)
- [Installation](docs/installation.md)
- [Command reference](docs/commands.md)
- [Manifest format](docs/manifest.md)
- [Lockfile format](docs/lockfile.md)
- [Architecture](docs/architecture.md)
- [Testing](docs/testing.md)
- [Release process](docs/release-process.md)
- [GitHub project sync](docs/github-project-sync.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Contributing](CONTRIBUTING.md)

## Project Status

Ktesio is early, useful, and intentionally conservative. The current package format is plain JSON plus git. Future work may add registries, richer metadata, and package signing without taking away the simple manifest workflow.

## Thanks

Thank you to [Skills.sh](https://www.skills.sh/) for providing public skill search, and to [Vercel](https://vercel.com/) for making Skills.sh available to everyone for free.

## License

Ktesio is **source-available**, licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE).

- **Noncommercial use is free.** You may use, copy, modify, and share Ktesio for any noncommercial purpose under the terms of the license.
- **Commercial use requires a separate license.** Any commercial use needs the prior written permission of the copyright holder, Islam Magdy. To request a commercial license, open an issue or contact the maintainer through the project's official channels.

This is source-available software, not an OSI-approved open source license.

## Contributing

Contributions are welcome under the project's [Contributor License Agreement](CLA.md): anyone can contribute, but you assign copyright in your contribution to the project owner so Ktesio stays under unified ownership. See [CONTRIBUTING.md](CONTRIBUTING.md) and [CLA.md](CLA.md) for details.
