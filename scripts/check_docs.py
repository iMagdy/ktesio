#!/usr/bin/env python3
"""Validate Markdown links, JSON snippets, and stale repository references."""

from __future__ import annotations

import json
import re
import shlex
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[1]
DOC_PATHS = [
    *ROOT.glob("*.md"),
    *ROOT.glob("docs/*.md"),
]
STALE_PATTERNS = [
    "github.com/imagdy/skills",
    "../specs/002-project-docs/quickstart.md",
    "specs/005-github-ci-pipeline",
    "docs/RELEASES_NOTES.md",
]

LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")
JSON_FENCE_RE = re.compile(r"```json\s*(.*?)```", re.DOTALL | re.IGNORECASE)
BASH_FENCE_RE = re.compile(r"```(?:bash|sh|shell)\s*(.*?)```", re.DOTALL | re.IGNORECASE)
KT_COMMANDS = {
    "init",
    "search",
    "install",
    "upgrade",
    "self-update",
    "publish",
    "list",
    "show",
    "doctor",
    "uninstall",
    "remove",
    "help",
    "agent",
}
# `kt agent` owns its own subcommand tree (crates/kt/src/main.rs `AgentCommands`).
# Model it the same way as the top level — an allowlist per nesting level — so the
# agent-runner surface validates without a blanket bypass.
AGENT_COMMANDS = {
    "register",
    "remove",
    "start",
    "stop",
    "pause",
    "resume",
    "list",
    "show",
    "config",
}
# `kt agent config` nests one level deeper (`ConfigCommands`).
CONFIG_COMMANDS = {"get", "set"}


def main() -> int:
    errors: list[str] = []

    for path in DOC_PATHS:
        text = path.read_text(encoding="utf-8")
        rel_path = path.relative_to(ROOT)

        for pattern in STALE_PATTERNS:
            if pattern in text:
                errors.append(f"{rel_path}: stale reference `{pattern}`")

        for match in LINK_RE.finditer(text):
            target = match.group(1).strip()
            validate_link(path, target, errors)

        for index, match in enumerate(JSON_FENCE_RE.finditer(text), start=1):
            snippet = match.group(1).strip()
            if not snippet:
                continue
            try:
                json.loads(snippet)
            except json.JSONDecodeError as exc:
                errors.append(f"{rel_path}: invalid JSON fence #{index}: {exc}")

        for index, match in enumerate(BASH_FENCE_RE.finditer(text), start=1):
            validate_command_examples(path, index, match.group(1), errors)

    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    print(f"Validated {len(DOC_PATHS)} Markdown files")
    return 0


def validate_link(path: Path, target: str, errors: list[str]) -> None:
    if (
        target.startswith(("http://", "https://", "mailto:", "#"))
        or target.startswith("app://")
        or target.startswith("file:")
    ):
        return

    clean_target = target.split("#", 1)[0].split("?", 1)[0]
    if not clean_target:
        return

    clean_target = unquote(clean_target)
    candidate = (path.parent / clean_target).resolve()

    try:
        candidate.relative_to(ROOT)
    except ValueError:
        errors.append(f"{path.relative_to(ROOT)}: link escapes repo: {target}")
        return

    if not candidate.exists():
        errors.append(f"{path.relative_to(ROOT)}: broken link: {target}")


def validate_command_examples(
    path: Path, fence_index: int, snippet: str, errors: list[str]
) -> None:
    rel_path = path.relative_to(ROOT)
    for line_number, raw_line in enumerate(snippet.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#") or line.endswith("\\"):
            continue
        if "kt" not in line:
            continue
        try:
            tokens = shlex.split(line)
        except ValueError as exc:
            errors.append(
                f"{rel_path}: shell fence #{fence_index}, line {line_number}: cannot parse command: {exc}"
            )
            continue
        if not tokens:
            continue

        command_index = 0
        while command_index < len(tokens) and "=" in tokens[command_index]:
            command_index += 1
        if command_index >= len(tokens):
            continue

        binary = tokens[command_index]
        if not (binary == "kt" or binary.endswith("/kt") or binary.endswith("\\kt")):
            continue

        if len(tokens) <= command_index + 1:
            errors.append(
                f"{rel_path}: shell fence #{fence_index}, line {line_number}: `kt` example is missing a command"
            )
            continue

        command = tokens[command_index + 1]
        if command.startswith("-"):
            continue
        if command not in KT_COMMANDS:
            errors.append(
                f"{rel_path}: shell fence #{fence_index}, line {line_number}: unknown `kt` command `{command}`"
            )
            continue
        if command == "agent":
            validate_agent_subcommands(
                rel_path, fence_index, line_number, tokens[command_index + 2 :], errors
            )


def first_non_flag(tokens: list[str]) -> tuple[int, str] | None:
    """Return the (index, token) of the first non-flag token, or None if every token
    is a flag (or the list is empty)."""
    for index, token in enumerate(tokens):
        if not token.startswith("-"):
            return index, token
    return None


def validate_agent_subcommands(
    rel_path: Path,
    fence_index: int,
    line_number: int,
    rest: list[str],
    errors: list[str],
) -> None:
    """Validate the `kt agent` subcommand tree (crates/kt/src/main.rs `AgentCommands`
    / `ConfigCommands`), mirroring the top-level allowlist at each nesting level.

    `rest` is the tokens AFTER `kt agent`. A flag-only tail (e.g. `kt agent --help`)
    is left to clap and not flagged; only a present-but-unknown subcommand is an error.
    """
    found = first_non_flag(rest)
    if found is None:
        return
    index, subcommand = found
    if subcommand not in AGENT_COMMANDS:
        errors.append(
            f"{rel_path}: shell fence #{fence_index}, line {line_number}: unknown `kt agent` command `{subcommand}`"
        )
        return
    if subcommand == "config":
        config_found = first_non_flag(rest[index + 1 :])
        if config_found is None:
            return
        _, config_subcommand = config_found
        if config_subcommand not in CONFIG_COMMANDS:
            errors.append(
                f"{rel_path}: shell fence #{fence_index}, line {line_number}: "
                f"unknown `kt agent config` command `{config_subcommand}`"
            )


if __name__ == "__main__":
    raise SystemExit(main())
