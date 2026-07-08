#!/usr/bin/env python3
"""Prepare and push a Ktesio release tag."""

from __future__ import annotations

import argparse
import os
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback
    tomllib = None


SEMVER_TAG = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")
BREAKING_SUBJECT = re.compile(r"^[A-Za-z][A-Za-z0-9-]*(\([^)]+\))?!:")
FEAT_SUBJECT = re.compile(r"^feat(\([^)]+\))?!?:")
BREAKING_FOOTER = re.compile(r"(?m)^BREAKING[ -]CHANGE:")


@dataclass(frozen=True, order=True)
class Version:
    major: int
    minor: int
    patch: int

    @classmethod
    def parse(cls, value: str) -> "Version":
        match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", value)
        if not match:
            raise ReleaseError(f"Unsupported version '{value}'. Expected MAJOR.MINOR.PATCH.")
        return cls(*(int(part) for part in match.groups()))

    @classmethod
    def from_tag(cls, tag: str) -> "Version | None":
        match = SEMVER_TAG.match(tag)
        if not match:
            return None
        return cls(*(int(part) for part in match.groups()))

    def bump(self, kind: str) -> "Version":
        if kind == "major":
            return Version(self.major + 1, 0, 0)
        if kind == "minor":
            return Version(self.major, self.minor + 1, 0)
        if kind == "patch":
            return Version(self.major, self.minor, self.patch + 1)
        raise ReleaseError(f"Unknown release kind '{kind}'.")

    def __str__(self) -> str:
        return f"{self.major}.{self.minor}.{self.patch}"

    @property
    def tag(self) -> str:
        return f"v{self}"


@dataclass(frozen=True)
class CommitSignal:
    sha: str
    subject: str
    kind: str
    reason: str


class ReleaseError(Exception):
    pass


def run(
    args: list[str],
    *,
    cwd: Path,
    capture: bool = True,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    print(f"$ {shlex.join(args)}")
    completed = subprocess.run(
        args,
        cwd=cwd,
        text=True,
        check=False,
        capture_output=capture,
    )
    if check and completed.returncode != 0:
        if capture and completed.stdout:
            print(completed.stdout, end="")
        if capture and completed.stderr:
            print(completed.stderr, end="", file=sys.stderr)
        raise ReleaseError(f"Command failed with exit code {completed.returncode}: {shlex.join(args)}")
    return completed


def git(args: list[str], *, cwd: Path, capture: bool = True) -> str:
    completed = run(["git", *args], cwd=cwd, capture=capture)
    return completed.stdout.strip() if capture else ""


def cargo_package(repo: Path) -> tuple[str, Version]:
    """Resolve the shipping package name and version in the workspace layout.

    The repository root is a virtual `[workspace]` manifest: the `ktesio`
    package lives at crates/kt/Cargo.toml and inherits its version from the
    root `[workspace.package]` table.
    """
    if tomllib is None:
        raise ReleaseError("Python 3.11+ is required so tomllib can parse Cargo.toml.")
    package_toml = repo / "crates" / "kt" / "Cargo.toml"
    if not package_toml.exists():
        raise ReleaseError(f"{package_toml} not found. Run this from the Ktesio checkout.")
    with package_toml.open("rb") as handle:
        package = tomllib.load(handle).get("package") or {}
    name = package.get("name")
    if not isinstance(name, str):
        raise ReleaseError("crates/kt/Cargo.toml is missing package.name.")
    raw_version = package.get("version")
    if isinstance(raw_version, dict) and raw_version.get("workspace") is True:
        with (repo / "Cargo.toml").open("rb") as handle:
            workspace = tomllib.load(handle).get("workspace") or {}
        raw_version = (workspace.get("package") or {}).get("version")
    if not isinstance(raw_version, str):
        raise ReleaseError(
            "Could not resolve the package version from crates/kt/Cargo.toml "
            "or the root [workspace.package] table."
        )
    return name, Version.parse(raw_version)


def git_root(start: Path) -> Path:
    root = git(["rev-parse", "--show-toplevel"], cwd=start)
    return Path(root).resolve()


def ensure_repository(repo: Path, branch: str, remote: str, fetch: bool) -> None:
    cargo_toml = repo / "Cargo.toml"
    if not cargo_toml.exists():
        raise ReleaseError(f"Cargo.toml not found in {repo}. Run this from the Ktesio checkout.")
    package_name, _ = cargo_package(repo)
    if package_name != "ktesio":
        raise ReleaseError(f"Expected package 'ktesio', found '{package_name}'.")

    current_branch = git(["rev-parse", "--abbrev-ref", "HEAD"], cwd=repo)
    if current_branch != branch:
        raise ReleaseError(f"Release must run from {branch}; current branch is {current_branch}.")

    status = git(["status", "--porcelain"], cwd=repo)
    if status:
        raise ReleaseError("Working tree is dirty. Commit, stash, or remove local changes before release.")

    if fetch:
        git(["fetch", remote, f"+refs/heads/{branch}:refs/remotes/{remote}/{branch}", "--tags"], cwd=repo, capture=False)

    counts = git(["rev-list", "--left-right", "--count", f"{remote}/{branch}...HEAD"], cwd=repo)
    behind_text, ahead_text = counts.split()
    behind = int(behind_text)
    ahead = int(ahead_text)
    if behind or ahead:
        raise ReleaseError(
            f"Local {branch} must match {remote}/{branch} before release; behind={behind}, ahead={ahead}."
        )


def latest_semver_tag(repo: Path) -> tuple[str | None, Version | None]:
    raw_tags = git(["tag", "--merged", "HEAD", "--list", "v[0-9]*", "--sort=-v:refname"], cwd=repo)
    for tag in raw_tags.splitlines():
        version = Version.from_tag(tag.strip())
        if version is not None:
            return tag.strip(), version
    return None, None


def unreleased_commits(repo: Path, latest_tag: str | None) -> list[tuple[str, str, str]]:
    revision = f"{latest_tag}..HEAD" if latest_tag else "HEAD"
    raw = git(["log", "--format=%H%x00%s%x00%b%x1e", revision], cwd=repo)
    commits: list[tuple[str, str, str]] = []
    for entry in raw.split("\x1e"):
        entry = entry.strip("\n")
        if not entry:
            continue
        parts = entry.split("\x00", 2)
        if len(parts) != 3:
            continue
        commits.append((parts[0], parts[1], parts[2]))
    commits.reverse()
    return commits


def classify(commits: list[tuple[str, str, str]]) -> tuple[str, list[CommitSignal]]:
    if not commits:
        raise ReleaseError("No unreleased commits found since the latest release tag.")

    signals: list[CommitSignal] = []
    release_kind = "patch"
    for sha, subject, body in commits:
        full_message = f"{subject}\n{body}"
        if BREAKING_SUBJECT.search(subject) or BREAKING_FOOTER.search(full_message):
            release_kind = "major"
            signals.append(CommitSignal(sha, subject, "major", "breaking change marker"))
        elif FEAT_SUBJECT.search(subject):
            if release_kind != "major":
                release_kind = "minor"
            signals.append(CommitSignal(sha, subject, "minor", "feat commit"))

    if not signals:
        signals.append(CommitSignal(commits[-1][0], commits[-1][1], "patch", "default patch release"))
    return release_kind, signals


def replace_cargo_version(cargo_toml: Path, target: Version) -> None:
    """Rewrite the version where it lives now: root [workspace.package]."""
    text = cargo_toml.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    in_workspace_package = False
    replaced = False
    output: list[str] = []
    for line in lines:
        section = re.match(r"\s*\[([^\]]+)\]\s*$", line)
        if section:
            in_workspace_package = section.group(1) == "workspace.package"
        if in_workspace_package and re.match(r'\s*version\s*=\s*"', line):
            newline = "\n" if line.endswith("\n") else ""
            prefix = re.match(r'(\s*version\s*=\s*)"', line)
            if prefix is None:
                raise ReleaseError("Could not rewrite workspace.package.version in Cargo.toml.")
            line = f'{prefix.group(1)}"{target}"{newline}'
            replaced = True
        output.append(line)
    if not replaced:
        raise ReleaseError("Could not find workspace.package.version in Cargo.toml.")
    cargo_toml.write_text("".join(output), encoding="utf-8")


def update_cargo_lock(repo: Path) -> None:
    run(["cargo", "generate-lockfile"], cwd=repo, capture=False)


def run_checks(repo: Path) -> None:
    run(["cargo", "fmt", "--all", "--check"], cwd=repo, capture=False)
    run(["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], cwd=repo, capture=False)
    run(["cargo", "test", "--workspace", "--all-targets"], cwd=repo, capture=False)


def ensure_only_release_files_changed(repo: Path) -> None:
    changed = set(git(["diff", "--name-only"], cwd=repo).splitlines())
    allowed = {"Cargo.toml", "Cargo.lock"}
    unexpected = sorted(changed - allowed)
    if unexpected:
        raise ReleaseError(f"Unexpected changed files after release preparation: {', '.join(unexpected)}")

    status = git(["status", "--porcelain"], cwd=repo)
    untracked = [line for line in status.splitlines() if line.startswith("?? ")]
    if untracked:
        raise ReleaseError("Unexpected untracked files after checks: " + ", ".join(line[3:] for line in untracked))


def print_summary(
    *,
    latest_tag: str | None,
    cargo_version: Version,
    kind: str,
    target: Version,
    signals: list[CommitSignal],
    commits: list[tuple[str, str, str]],
) -> None:
    print("\nRelease summary")
    print(f"  latest tag: {latest_tag or '(none)'}")
    print(f"  current Cargo.toml version: {cargo_version}")
    print(f"  unreleased commits: {len(commits)}")
    print(f"  inferred release: {kind}")
    print(f"  target tag: {target.tag}")
    print("  evidence:")
    for signal in signals[:8]:
        print(f"    - {signal.kind}: {signal.sha[:12]} {signal.subject} ({signal.reason})")
    if len(signals) > 8:
        print(f"    - ... {len(signals) - 8} more signals")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Prepare, commit, push, and tag a Ktesio release.")
    parser.add_argument("--dry-run", action="store_true", help="Infer and print the release without mutating files.")
    parser.add_argument(
        "--confirm-major",
        action="store_true",
        help="Allow a major release after explicit user confirmation.",
    )
    parser.add_argument("--skip-checks", action="store_true", help="Skip cargo fmt, clippy, and test checks.")
    parser.add_argument("--remote", default="origin", help="Git remote to fetch and push. Default: origin.")
    parser.add_argument("--branch", default="main", help="Release branch. Default: main.")
    parser.add_argument("--no-fetch", action="store_true", help="Do not fetch before checking release state.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        repo = git_root(Path.cwd())
        os.chdir(repo)
        ensure_repository(repo, args.branch, args.remote, fetch=not args.no_fetch)
        package_name, cargo_version = cargo_package(repo)
        latest_tag, latest_version = latest_semver_tag(repo)
        commits = unreleased_commits(repo, latest_tag)
        kind, signals = classify(commits)
        base_version = latest_version or cargo_version
        target = base_version.bump(kind)

        if latest_version is not None and cargo_version != latest_version:
            if cargo_version == target:
                print(f"Cargo.toml is already prepared for {target.tag}; no version edit is needed.")
            else:
                raise ReleaseError(
                    f"Cargo.toml version {cargo_version} does not match latest tag {latest_tag} "
                    f"and is not the inferred target {target}."
                )

        if git(["tag", "--list", target.tag], cwd=repo):
            raise ReleaseError(f"Tag {target.tag} already exists locally.")

        print_summary(
            latest_tag=latest_tag,
            cargo_version=cargo_version,
            kind=kind,
            target=target,
            signals=signals,
            commits=commits,
        )

        if args.dry_run:
            print("\nDry run complete. No files, commits, or tags were changed.")
            return 0

        if kind == "major" and not args.confirm_major:
            raise ReleaseError(
                f"Major release {target.tag} requires explicit user confirmation. "
                "Ask the user to confirm this exact tag, then rerun with --confirm-major."
            )

        if cargo_version != target:
            replace_cargo_version(repo / "Cargo.toml", target)
            update_cargo_lock(repo)

        if not args.skip_checks:
            run_checks(repo)

        ensure_only_release_files_changed(repo)
        git(["add", "Cargo.toml", "Cargo.lock"], cwd=repo, capture=False)
        staged = run(["git", "diff", "--cached", "--quiet"], cwd=repo, capture=True, check=False)
        if staged.returncode == 0:
            print("No version changes to commit; proceeding to push and tag existing HEAD.")
        else:
            git(["commit", "-s", "-m", f"chore(release): bump version to {target}"], cwd=repo, capture=False)

        git(["push", args.remote, f"HEAD:{args.branch}"], cwd=repo, capture=False)
        git(["tag", target.tag], cwd=repo, capture=False)
        git(["push", args.remote, target.tag], cwd=repo, capture=False)

        print(f"\nStarted release flow for {package_name} {target.tag}.")
        print("GitHub Actions will build artifacts, publish crates.io/Homebrew, and open the release docs PR.")
        return 0
    except ReleaseError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
