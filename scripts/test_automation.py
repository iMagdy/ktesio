#!/usr/bin/env python3
"""Unit tests for repository automation helpers."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import generate_release_docs as release_docs
import generate_homebrew_formula as homebrew_formula


class ReleaseDocsTests(unittest.TestCase):
    def test_release_body_has_single_download_table(self) -> None:
        body = release_docs.render_release_body("v1.2.3", None, [])

        self.assertIn("Initial release history", body)
        self.assertEqual(1, body.count("| Platform | Target | Archive | Checksum |"))

    def test_asset_table_has_all_tier_one_targets_and_checksums(self) -> None:
        table = "\n".join(release_docs.render_asset_table("v1.2.3"))

        for _platform, target, extension in release_docs.TARGETS:
            self.assertIn(f"ktesio-v1.2.3-{target}.{extension}", table)
            self.assertIn(f"ktesio-v1.2.3-{target}.{extension}.sha256", table)
        self.assertIn("ktesio-v1.2.3-checksums.txt", table)

    def test_changelog_groups_conventional_commits(self) -> None:
        grouped = release_docs.group_commits(
            [
                release_docs.Commit("abc1234", "feat: add export"),
                release_docs.Commit("def5678", "fix: repair docs"),
                release_docs.Commit("fff0000", "plain commit"),
            ]
        )

        self.assertEqual(["abc1234"], [commit.sha for commit in grouped["feat"]])
        self.assertEqual(["def5678"], [commit.sha for commit in grouped["fix"]])
        self.assertEqual(["fff0000"], [commit.sha for commit in grouped["other"]])

    def test_semver_key_accepts_only_v_tags(self) -> None:
        self.assertEqual((1, 2, 3), release_docs.semver_key("v1.2.3"))
        self.assertIsNone(release_docs.semver_key("1.2.3"))
        self.assertIsNone(release_docs.semver_key("v1.2.3-beta"))

    def test_release_workflow_contains_expected_asset_and_release_steps(self) -> None:
        workflow = (release_docs.ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )

        for _platform, target, _extension in release_docs.TARGETS:
            self.assertIn(target, workflow)
        self.assertIn(".sha256", workflow)
        self.assertIn("checksums.txt", workflow)
        self.assertIn("gh release create", workflow)
        self.assertIn("gh release upload", workflow)
        self.assertIn("gh pr create", workflow)
        self.assertIn("generate_homebrew_formula.py", workflow)
        self.assertIn("HOMEBREW_TAP_TOKEN", workflow)
        self.assertIn("CARGO_REGISTRY_TOKEN", workflow)
        self.assertIn("cargo publish --locked", workflow)
        self.assertNotIn("packages: write", workflow)
        self.assertNotIn("oras-project/setup-oras", workflow)
        self.assertNotIn("oras push", workflow)
        self.assertNotIn("ghcr.io", workflow)
        self.assertNotIn("GHCR_TOKEN", workflow)
        self.assertNotIn("org.opencontainers.image", workflow)
        self.assertNotIn("application/vnd.ktesio.release.v1", workflow)

    def test_homebrew_formula_uses_release_assets_and_checksums(self) -> None:
        checksums = {
            "ktesio-v1.2.3-x86_64-apple-darwin.tar.gz": "a" * 64,
            "ktesio-v1.2.3-aarch64-apple-darwin.tar.gz": "b" * 64,
            "ktesio-v1.2.3-x86_64-unknown-linux-gnu.tar.gz": "c" * 64,
            "ktesio-v1.2.3-x86_64-pc-windows-msvc.zip": "d" * 64,
        }

        formula = homebrew_formula.render_formula("v1.2.3", checksums)

        self.assertIn('class Ktesio < Formula', formula)
        self.assertIn('version "1.2.3"', formula)
        self.assertIn('depends_on "git"', formula)
        self.assertIn("on_macos do", formula)
        self.assertIn("on_arm do", formula)
        self.assertIn("on_intel do", formula)
        self.assertIn("x86_64-apple-darwin", formula)
        self.assertIn("aarch64-apple-darwin", formula)
        self.assertIn("on_linux do", formula)
        self.assertIn("x86_64-unknown-linux-gnu", formula)
        self.assertNotIn("x86_64-pc-windows-msvc", formula)
        self.assertIn('bin.install "kt"', formula)

    def test_homebrew_checksum_parser_accepts_sha256sum_lines(self) -> None:
        checksums = homebrew_formula.parse_checksums(
            "\n".join(
                [
                    f"{'A' * 64}  ktesio-v1.2.3-x86_64-apple-darwin.tar.gz",
                    f"{'b' * 64} *ktesio-v1.2.3-aarch64-apple-darwin.tar.gz",
                ]
            )
        )

        self.assertEqual("a" * 64, checksums["ktesio-v1.2.3-x86_64-apple-darwin.tar.gz"])
        self.assertEqual("b" * 64, checksums["ktesio-v1.2.3-aarch64-apple-darwin.tar.gz"])

    def test_ci_runs_coverage_after_primary_gates(self) -> None:
        ci = (release_docs.ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("needs: [fmt, clippy, test, build, docs]", ci)
        self.assertIn("cargo tarpaulin --fail-under 95", ci)


class InstallerScriptTests(unittest.TestCase):
    def run_install_sh(
        self,
        env: dict[str, str],
        *,
        expect_success: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        script = release_docs.ROOT / "scripts" / "public" / "install.sh"
        merged_env = os.environ.copy()
        merged_env.update(
            {
                "KTESIO_INSTALL_DRY_RUN": "1",
                "KTESIO_INSTALL_TEST_KT_PATH": "",
                "KTESIO_INSTALL_TEST_HAS_BREW": "0",
                "KTESIO_INSTALL_TEST_HAS_CARGO": "0",
                "KTESIO_INSTALL_TEST_OS": "Linux",
                "KTESIO_INSTALL_TEST_ARCH": "x86_64",
                "CARGO_HOME": "",
            }
        )
        merged_env.update(env)

        result = subprocess.run(
            ["sh", str(script)],
            cwd=release_docs.ROOT,
            env=merged_env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )

        if expect_success:
            self.assertEqual(
                0,
                result.returncode,
                result.stdout + result.stderr,
            )
        else:
            self.assertNotEqual(0, result.returncode, result.stdout + result.stderr)

        return result

    def fake_kt(self, directory: Path, output: str = "kt 1.2.3") -> Path:
        path = directory / "kt"
        path.write_text(f"#!/bin/sh\nprintf '%s\\n' '{output}'\n", encoding="utf-8")
        path.chmod(0o755)
        return path

    def test_install_sh_prefers_homebrew_for_new_installs(self) -> None:
        result = self.run_install_sh({"KTESIO_INSTALL_TEST_HAS_BREW": "1"})

        self.assertIn("DRY RUN: brew install imagdy/tap/ktesio", result.stdout)

    def test_install_sh_uses_cargo_when_homebrew_is_unavailable(self) -> None:
        result = self.run_install_sh({"KTESIO_INSTALL_TEST_HAS_CARGO": "1"})

        self.assertIn("DRY RUN: cargo install ktesio --force", result.stdout)

    def test_install_sh_uses_prebuilt_binary_without_package_managers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_install_sh({"HOME": tmp})

        self.assertIn(
            "DRY RUN: install prebuilt x86_64-unknown-linux-gnu",
            result.stdout,
        )

    def test_install_sh_updates_existing_homebrew_install(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            kt_path = self.fake_kt(Path(tmp))
            result = self.run_install_sh(
                {
                    "KTESIO_INSTALL_TEST_KT_PATH": str(kt_path),
                    "KTESIO_INSTALL_TEST_BREW_INSTALLED": "1",
                    "KTESIO_INSTALL_TEST_HAS_BREW": "1",
                }
            )

        self.assertIn("DRY RUN: brew upgrade imagdy/tap/ktesio", result.stdout)

    def test_install_sh_updates_existing_cargo_install(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            cargo_bin = home / ".cargo" / "bin"
            cargo_bin.mkdir(parents=True)
            kt_path = self.fake_kt(cargo_bin)
            result = self.run_install_sh(
                {
                    "HOME": str(home),
                    "KTESIO_INSTALL_TEST_KT_PATH": str(kt_path),
                    "KTESIO_INSTALL_TEST_HAS_CARGO": "1",
                }
            )

        self.assertIn("DRY RUN: cargo install ktesio --force", result.stdout)

    def test_install_sh_replaces_existing_manual_install(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp) / "bin"
            bin_dir.mkdir()
            kt_path = self.fake_kt(bin_dir)
            result = self.run_install_sh(
                {
                    "KTESIO_INSTALL_TEST_KT_PATH": str(kt_path),
                    "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}",
                }
            )

        self.assertIn(f"to {kt_path}", result.stdout)

    def test_install_sh_rejects_unwritable_manual_install_dir(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp) / "bin"
            bin_dir.mkdir()
            kt_path = self.fake_kt(bin_dir)
            bin_dir.chmod(0o555)
            try:
                result = self.run_install_sh(
                    {"KTESIO_INSTALL_TEST_KT_PATH": str(kt_path)},
                    expect_success=False,
                )
            finally:
                bin_dir.chmod(0o755)

        output = result.stdout + result.stderr
        self.assertIn("is not writable", output)
        self.assertIn("KTESIO_INSTALL_DIR", output)

    def test_install_sh_rejects_unsupported_binary_target_without_cargo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = self.run_install_sh(
                {"HOME": tmp, "KTESIO_INSTALL_TEST_ARCH": "aarch64"},
                expect_success=False,
            )

        self.assertIn("No prebuilt Ktesio binary is available", result.stderr)

    def test_install_sh_refuses_non_ktesio_kt_conflict(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            kt_path = self.fake_kt(Path(tmp), "not ktesio")
            result = self.run_install_sh(
                {"KTESIO_INSTALL_TEST_KT_PATH": str(kt_path)},
                expect_success=False,
            )

        self.assertIn("Refusing to overwrite non-Ktesio kt", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
