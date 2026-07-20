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
        # Release publish is explicit about its toolchain: the root
        # rust-toolchain.toml pins bare cargo to the MSRV (1.96.1), but shipped
        # artifacts and the crates.io publish run on latest stable (AI-17).
        self.assertIn("cargo +stable publish --locked -p ktesio", workflow)
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

        self.assertIn("needs: [fmt, clippy, test, build, docs, boundary, semver]", ci)
        # Stable jobs are explicit about their toolchain: the root
        # rust-toolchain.toml pins bare cargo to the MSRV (1.96.1), so these jobs
        # select +stable to keep exercising latest stable (AI-17). The `msrv` job
        # (asserted in test_ci_enforces_msrv_floor) still proves the 1.96.1 floor.
        #
        # The `test` job runs the suite via cargo-nextest (#106): nextest gives a
        # per-test kill-timeout so a hung/slow engine integration test becomes a
        # NAMED, time-boxed failure instead of a silent job-level cancel whose
        # logs GitHub discards (the ubuntu symptom), and its retries absorb the
        # windows-latest per-test timing flake. Doctests run separately because
        # nextest does not execute them. nextest is installed with the repo's
        # standard `cargo install --locked` idiom (same as the semver / tarpaulin
        # gates), not a third-party install action, so the SHA-pinned-actions
        # posture is preserved. Assert the whole shape so a regression back to the
        # hang-prone plain `cargo test` suite run — or a dropped doctest pass — is
        # caught.
        self.assertIn("cargo +stable nextest run --workspace --all-targets", ci)
        self.assertIn("cargo +stable test --workspace --doc", ci)
        # nextest (unlike `cargo test --all-targets`) does not hardlink the
        # fake_agent bin to target/debug/fake_agent, so the test job rebuilds it
        # explicitly between the stale-bin `rm` and the nextest run — otherwise the
        # process-spawning tests race on the lib's on-demand build fallback. Guard
        # both the `rm` and the explicit rebuild so neither is dropped.
        self.assertIn("rm -f target/debug/fake_agent target/debug/fake_agent.exe", ci)
        self.assertIn("cargo +stable build -p ktesio-conformance --bin fake_agent", ci)
        self.assertIn(
            "command -v cargo-nextest >/dev/null 2>&1 "
            "|| cargo +stable install cargo-nextest --locked",
            ci,
        )
        self.assertIn("${{ runner.os }}-cargo-nextest-bin", ci)
        # The old plain-`cargo test` suite command must be GONE — the only
        # remaining `cargo +stable test` in the test job is the `--doc` pass.
        self.assertNotIn("cargo +stable test --workspace --all-targets", ci)
        # Coverage runs tarpaulin ONCE PER CRATE (#101), not one --workspace pass:
        # the --workspace instrumented run OOM'd the 7 GB runner even after the
        # AI-23 fixes. Each per-crate run keeps its resident set small; the UNION of
        # the per-crate LCOV tracefiles reproduces the --workspace result exactly
        # (tarpaulin reports coverage for ALL workspace source each test binary
        # touches, so cross-crate coverage is credited), and the merged aggregate is
        # gated at the SAME workspace >= 95. Assert the whole structure so a
        # regression to the OOM-prone single --workspace pass — or a dropped merge /
        # gate — is caught. The old `--workspace --fail-under 95` command must be
        # GONE (tarpaulin's own per-run fail-under is replaced by the merged gate).
        self.assertNotIn("--workspace --fail-under 95", ci)
        # The per-crate tarpaulin invocation shape (llvm + skip-clean + timeout 180 +
        # verbose preserved; -p "$pkg" + Lcov out into cov/$pkg replaces --workspace).
        self.assertIn(
            'cargo +stable tarpaulin --engine llvm --skip-clean --timeout 180 '
            "--verbose \\\n                -p \"$pkg\" --out Lcov --output-dir "
            '"cov/$pkg"',
            ci,
        )
        # The exact five workspace crates, lightest → heaviest with ktesio-engine
        # LAST (so a heavy-crate OOM still leaves the lighter crates' numbers logged).
        self.assertIn(
            "set -- ktesio-adapter-api ktesio-conformance ktesio-adapters-hermes "
            "ktesio ktesio-engine",
            ci,
        )
        # Each per-crate run is grouped and dumps free/df first, so a per-crate OOM
        # is diagnosable (which crate, at what memory/disk) instead of one opaque
        # --workspace failure.
        self.assertIn("::group::tarpaulin -p $pkg", ci)
        self.assertIn("free -h || true", ci)
        self.assertIn("df -h / || true", ci)
        # lcov merges the per-crate tracefiles into the workspace aggregate; apt
        # installs it. --ignore-errors inconsistent tolerates lcov 2.x's benign
        # cross-run FN-start-line drift (line data unaffected).
        self.assertIn("sudo apt-get install -y -qq lcov", ci)
        self.assertIn("-o cov/merged.info --ignore-errors inconsistent", ci)
        # The merged aggregate is gated at the SAME workspace >= 95: parse the
        # `(<hit> of <found> lines)` fraction from `lcov --summary` and fail (exit 1,
        # ::error::) below 95, else pass.
        self.assertIn("lcov --summary cov/merged.info", ci)
        self.assertIn(
            r"sed -E 's/.*\(([0-9]+) of ([0-9]+) lines\).*/\1 \2/'", ci
        )
        self.assertIn("BEGIN { exit (p + 0 >= 95) ? 0 : 1 }", ci)
        self.assertIn(
            "::error::Merged workspace coverage ${pct}% (${hit}/${found} lines) "
            "is below the 95% gate",
            ci,
        )
        # --engine llvm: parity with the local macOS gate (ptrace is unavailable
        # there). llvm-tools-preview supplies the llvm-profdata/llvm-cov it shells
        # out to. --timeout 180 lifts tarpaulin's 60 s per-test default so a heavy
        # survival test under instrumentation is not killed spuriously.
        self.assertIn("rustup component add llvm-tools-preview", ci)
        # The coverage TIMEOUT fix (AI-23): a DEDICATED cache key so the instrumented
        # target — whose fingerprints differ from the other jobs' normal-profile
        # build — actually persists. The shared key gave coverage nothing reusable
        # and, running last, never saved its own, so every run recompiled the graph
        # cold and blew the cap — not the engine, not the tarpaulin binary install.
        self.assertIn(
            "${{ runner.os }}-cargo-coverage-${{ hashFiles('**/Cargo.lock') }}", ci
        )
        # The source-installed tarpaulin binary is still cached and its install made
        # idempotent (hygiene, mirroring the semver gate's binary cache, AI-1).
        self.assertIn("${{ runner.os }}-cargo-tarpaulin-bin", ci)
        self.assertIn(
            "command -v cargo-tarpaulin >/dev/null 2>&1 "
            "|| cargo +stable install cargo-tarpaulin --locked",
            ci,
        )
        # The instrumented run is serialised, given swap headroom, has `/` disk
        # reclaimed, and builds with line-tables-only debuginfo — the OOM ("runner
        # lost communication", AI-23) came from RAM+disk pressure of the instrumented
        # build (coverage counters + full debuginfo + subprocess-spawning tests), not
        # the engine. CARGO_PROFILE_DEV_DEBUG=1 leaves coverage unchanged (llvm needs
        # only line tables); the SDK cleanup frees ~30 GB on `/` where target/ lives.
        self.assertIn('RUST_TEST_THREADS: "1"', ci)
        self.assertIn('CARGO_PROFILE_DEV_DEBUG: "1"', ci)
        self.assertIn("swapon /mnt/covswap", ci)
        self.assertIn("rm -rf /usr/share/dotnet", ci)

    def test_ci_test_job_runs_on_three_os_matrix(self) -> None:
        # Story 1.4 (AD-4, NFR-2): the `test` job runs on a 3-OS matrix so the
        # per-OS ProcessBackend supervision code — in particular the Windows
        # Job-Object backend, which does not even compile on Linux — is proven on
        # a real Windows runner. Lock the matrix shape (mirrors the MSRV-floor
        # lock in test_ci_enforces_msrv_floor). Coverage stays Linux-only; the
        # matrix is the parity-honesty mechanism, not tarpaulin.
        ci = (release_docs.ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("os: [ubuntu-latest, macos-latest, windows-latest]", ci)
        self.assertIn("runs-on: ${{ matrix.os }}", ci)
        self.assertIn("fail-fast: false", ci)
        # Only the `test` job matrixes; the other jobs stay ubuntu-only. The
        # coverage job still stays Linux-only (now a per-crate tarpaulin split
        # merged into one aggregate, #101 — still Linux, still one job).
        self.assertIn("name: coverage", ci)

    def test_ci_test_job_single_run_with_serialized_engine_integration(self) -> None:
        # #106: a per-binary CI bisect pinned the x86-ubuntu hang to the `adoption`
        # engine integration binary (its heavy re-exec + surviving-orphan harness
        # wedged the genuine 2-core runner into an uninterruptible D-state under
        # nextest's concurrency). The fix is SOURCE-side — serialize the engine
        # integration tests via a nextest test-group (max-threads=1) — so the CI
        # `test` job is back to a SINGLE clean run for all OSes. Lock that clean
        # shape and assert the serialization exists; guard that the temporary
        # per-binary bisect scaffold AND the earlier watchdog/off-pipe machinery are
        # gone (so a future edit that reintroduces per-OS hang hacks is caught).
        ci = (release_docs.ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )
        nextest = (release_docs.ROOT / ".config" / "nextest.toml").read_text(
            encoding="utf-8"
        )
        # Clean shape: build test binaries once (--no-run), ONE unified run for all
        # OSes, doctests separate.
        self.assertIn(
            "cargo +stable nextest run --workspace --all-targets --no-run", ci
        )
        self.assertIn("- name: Run tests (nextest)", ci)
        self.assertIn("cargo +stable nextest run --workspace --all-targets", ci)
        self.assertIn("cargo +stable test --workspace --doc", ci)
        # The SOURCE fix: a max-threads=1 test-group over exactly the engine
        # integration binaries, with the existing retries + slow-timeout untouched.
        self.assertIn("[test-groups.engine-integration-serial]", nextest)
        self.assertIn("max-threads = 1", nextest)
        self.assertIn('filter = "kind(test) & package(ktesio-engine)"', nextest)
        self.assertIn('test-group = "engine-integration-serial"', nextest)
        self.assertIn("retries = 2", nextest)
        self.assertIn("slow-timeout = ", nextest)
        # The temporary bisect scaffold and the watchdog/off-pipe machinery are GONE
        # (no per-OS special-casing, no per-binary steps, no in-step capture hacks).
        self.assertNotIn("ubuntu bisect:", ci)
        self.assertNotIn("binary_id(=ktesio-engine::", ci)
        self.assertNotIn("RUNLOG", ci)
        self.assertNotIn("kill -USR1 $$", ci)
        self.assertNotIn("matrix.os == 'ubuntu-latest'", ci)
        self.assertNotIn("matrix.os != 'ubuntu-latest'", ci)

    def test_ci_enforces_workspace_boundary_and_semver_gates(self) -> None:
        ci = (release_docs.ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

        # Stable jobs select +stable so the root rust-toolchain.toml pin (MSRV
        # 1.96.1) does not silently redirect them off latest stable (AI-17).
        self.assertIn("cargo +stable check -p ktesio", ci)
        self.assertIn("cargo +stable tree -p ktesio -e normal,build --all-features", ci)
        # Boundary gate is an allowlist: only these internal edges may exist.
        self.assertIn("ktesio-(engine|adapter-api)", ci)
        # OS-cfg gate uses the broadened class pattern (compound cfg forms).
        self.assertIn("cfg[!(]?.*(unix|windows|target_os|target_family)", ci)
        self.assertIn("crates/ktesio-engine/src/backends/", ci)
        # Currency gate (story 3-3, AD-8): exactly one module formats a `$` string.
        # It scans for the BROADENED set of dollar-string-building forms (`${`, `$ {`,
        # `}$`, a `"$`-prefixed string, and a bare `'$'` char) and allowlists the
        # single currency module (domain/cost.rs) PLUS the precise non-currency `$`
        # uses the broadened pattern also matches (`.contains('$')` discipline
        # read-checks; the `$PWD`/`$KT_TEST` shell vars in a backend test). Guard both
        # the pattern and the allowlist so a regression that scatters currency
        # formatting is caught — and so the broadening is not silently narrowed back.
        self.assertIn("Enforce single currency-formatting module", ci)
        self.assertIn(r"""pattern='\$ ?\{|\}\$|"\$|'\''\$'\''""", ci)
        self.assertIn(
            r"allowlist='^crates/ktesio-engine/src/domain/cost\.rs:"
            r"|contains\(.\$|\$PWD|\$KT_TEST'",
            ci,
        )
        # Semver gate: lazy install inside the armed branch, transient skip.
        self.assertIn("cargo +stable install cargo-semver-checks --locked", ci)
        self.assertIn("cargo +stable semver-checks check-release", ci)
        self.assertIn("000|429|5[0-9][0-9]", ci)
        # Semver gate caches the source-installed binary so it is not rebuilt
        # (~10 min) on every fresh runner (AI-1).
        self.assertIn("${{ runner.os }}-cargo-semver-checks-bin", ci)

    def test_ci_enforces_msrv_floor(self) -> None:
        ci = (release_docs.ROOT / ".github" / "workflows" / "ci.yml").read_text(
            encoding="utf-8"
        )

        # MSRV job installs the pinned floor toolchain explicitly and checks the
        # whole workspace against it. Keep the version in lockstep with
        # rust-version in the root Cargo.toml [workspace.package].
        self.assertIn("name: msrv", ci)
        self.assertIn("rustup toolchain install 1.96.1 --profile minimal", ci)
        self.assertIn("cargo +1.96.1 check --workspace", ci)

        cargo_toml = (release_docs.ROOT / "Cargo.toml").read_text(encoding="utf-8")
        self.assertIn('rust-version = "1.96.1"', cargo_toml)

        # AI-17: a root rust-toolchain.toml pins bare cargo to the MSRV so
        # local `cargo build/test/clippy/fmt` need no `+1.96.1`. It must stay in
        # lockstep with rust-version; the `msrv` job above still proves the floor
        # (bare cargo in CI would otherwise resolve to this pin, not stable —
        # hence the explicit +stable on the stable jobs).
        toolchain_toml = (release_docs.ROOT / "rust-toolchain.toml").read_text(
            encoding="utf-8"
        )
        self.assertIn('channel = "1.96.1"', toolchain_toml)


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
