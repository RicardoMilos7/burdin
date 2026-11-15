#!/usr/bin/env python3
"""Checks the release pipeline (issues #452-#456):
  * .github/workflows/release.yml builds the six supported targets on native runners, smoke-tests each one,
    uploads archives and per-target checksums, verifies reproducibility, and publishes only from a tag push or an
    explicit manual `publish`;
  * tests/build_release.py prints the manifest version, packages a versioned archive whose contents and checksum
    are deterministic, and rejects a binary whose --version disagrees with the archive name;
  * install.sh and install.ps1 verify SHA256SUMS before installing, and install.sh installs from a local release
    directory (the rehearsal path) and refuses a tampered archive or an unsupported platform;
  * tests/release_notes.py extracts a changelog section and fails on a broken relative link;
  * docs/releasing.md records the version scheme, channels, artifact protocol, reproducibility, and rehearsal;
  * `burdin --version` reports the version and channel the policy requires.

Needs a built driver for the last section. BURDIN selects it, like tests/run_tests.py.
"""

import hashlib
import os
import re
import shutil
import sys
import tarfile
import time
from pathlib import Path

import harness

WORKFLOW = harness.ROOT / ".github" / "workflows" / "release.yml"
RELEASE_SCRIPT = harness.TESTS / "build_release.py"
INSTALL_SH = harness.ROOT / "install.sh"
INSTALL_PS1 = harness.ROOT / "install.ps1"
NOTES = harness.TESTS / "release_notes.py"
POLICY = harness.ROOT / "docs" / "releasing.md"

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check() -> None:
    global checks
    checks += 1


burdin = harness.require_burdin()


def expect_file(description: str, path) -> None:
    check()
    if not Path(path).is_file():
        add_problem(f"{description}: missing {path}")


def expect_status(description: str, actual: int, expected: int) -> None:
    check()
    if actual != expected:
        add_problem(f"{description}: exit status {actual}, expected {expected}")


def expect_contains(description: str, needle: str, output: str) -> None:
    check()
    if needle not in output:
        add_problem(f"{description}: output does not contain: {needle}")


def sha256_of(path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_binary(path, *arguments, env=None):
    path = Path(path)
    if str(path).endswith(".py"):
        return harness.run([sys.executable, str(path), *arguments], env=env)
    if harness.PLATFORM == "windows" and path.suffix.lower() != ".exe":
        return harness.run(["sh", str(path), *arguments], env=env)
    return harness.run([path, *arguments], env=env)


def run_release(*arguments, env=None):
    return harness.run([sys.executable, str(RELEASE_SCRIPT), *arguments], env=env)


def run_notes(*arguments):
    return harness.run([sys.executable, str(NOTES), *arguments])


with harness.temporary_dir("burdin-release-") as work:
    # --- The release workflow (issues #453, #455, #456) ----------------------------------

    check()
    if not WORKFLOW.is_file():
        add_problem(".github/workflows/release.yml: missing")
    else:
        text = harness.read_text(WORKFLOW)
        needs = [
            "name: Release",
            "tags:",
            "'v[0-9]*'",
            "workflow_dispatch:",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
            "ubuntu-24.04",
            "ubuntu-24.04-arm",
            "macos-15-intel",
            "macos-15",
            "windows-2025",
            "windows-11-arm",
            "tests/build_release.py",
            "--verify-reproducible",
            "tests/release_notes.py",
            "tests/check_release.py",
            "BURDIN_VERSION",
            "BURDIN_CHANNEL",
            "SHA256SUMS",
            "sha256sum -c",
            "gh release create",
            "actions/upload-artifact@v4",
            "actions/download-artifact@v4",
            "install.sh",
        ]
        for needle in needs:
            check()
            if needle not in text:
                add_problem(f"release.yml: missing: {needle}")

        # Publishing is opt-in: a manual dispatch defaults to false, and the publish step checks the output.
        lines = text.split("\n")
        publish_index = next(
            (index for index, line in enumerate(lines) if line == "      publish:"), None
        )
        check()
        if publish_index is None or "default: false" not in "\n".join(
            lines[publish_index:publish_index + 5]
        ):
            add_problem("release.yml: a manual run publishes by default (publish must default to false)")
        check()
        if "if: needs.plan.outputs.publish == 'true'" not in text:
            add_problem("release.yml: the GitHub release is not gated on the publish output")
        check()
        if "contents: write" not in text:
            add_problem("release.yml: no job has contents: write for the publish step")

    # --- The policy and the documentation index (issue #456) -----------------------------

    check()
    if not POLICY.is_file():
        add_problem("docs/releasing.md: missing (the version and release policy)")
    else:
        policy = harness.read_text(POLICY)
        for heading in (
            "## Version scheme",
            "## Channels",
            "## Breaking changes and packages",
            "## Artifacts",
            "## Checksums",
            "## Reproducibility",
            "## Installers",
            "## Release notes",
            "## Cutting a release",
            "## Rehearsal checklist",
        ):
            check()
            if heading not in policy:
                add_problem(f"docs/releasing.md: missing section: {heading}")
        for word in ("SemVer", "stable", "nightly", "dev", "SHA-256", "SOURCE_DATE_EPOCH", "LLVM 21"):
            check()
            if word not in policy:
                add_problem(f"docs/releasing.md: does not mention: {word}")
    readme = harness.read_text(harness.ROOT / "README.md")
    check()
    if "docs/releasing.md" not in readme:
        add_problem("README.md: does not link docs/releasing.md")
    check()
    if "install.sh" not in readme:
        add_problem("README.md: does not document install.sh")
    check()
    if "install.ps1" not in readme:
        add_problem("README.md: does not document install.ps1")

    # --- Packaging and checksums (issue #455) -------------------------------------------

    expect_file("tests/build_release.py", RELEASE_SCRIPT)
    expect_file("tests/release_notes.py", NOTES)
    expect_file("install.sh", INSTALL_SH)
    expect_file("install.ps1", INSTALL_PS1)

    dist_a = work / "dist-a"
    if RELEASE_SCRIPT.is_file():
        # A rewritten script must keep building the release profile, smoke-testing the binary before packaging, and
        # normalizing the build and the archive (issues #453, #455).
        release_text = harness.read_text(RELEASE_SCRIPT)
        for needle in (
            "smoke_test",
            '"--release", "--locked", "-p", "burdin_driver"',
            "--remap-path-prefix",
            "SOURCE_DATE_EPOCH",
            "GzipFile",
            "sha256_of",
        ):
            check()
            if needle not in release_text:
                add_problem(f"build_release.py: no longer contains: {needle}")

        check()
        manifest_match = re.search(r'^version = "(.*)"$', harness.read_text(harness.ROOT / "Cargo.toml"), re.MULTILINE)
        manifest = manifest_match.group(1) if manifest_match else ""
        printed = harness.run([sys.executable, str(RELEASE_SCRIPT), "--print-version"])
        printed_version = harness.text(printed.stdout).strip()
        if printed_version != manifest:
            add_problem(
                f"build_release.py --print-version: '{printed_version}', expected the manifest version '{manifest}'"
            )

        check()
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?", manifest):
            add_problem(f"Cargo.toml: '{manifest}' is not a MAJOR.MINOR.PATCH version")

        fake = work / "fake-burdin"
        harness.write_text(
            fake,
            "#!/bin/sh\n"
            'if [ "${1:-}" = "--version" ]; then\n'
            "    cat <<'VERSION'\n"
            "burdin 9.9.9\n"
            "channel: stable\n"
            "target: x86_64-unknown-linux-gnu\n"
            "llvm: 21.1.8\n"
            "commit: test\n"
            "profile: release\n"
            "VERSION\n"
            "    exit 0\n"
            "fi\n"
            "exit 1\n",
        )
        fake.chmod(0o755)

        target = "x86_64-unknown-linux-gnu"
        archive_name = f"burdin-9.9.9-{target}.tar.gz"
        check()
        first = run_release(
            "--binary", str(fake), "--target", target, "--version", "9.9.9",
            "--channel", "stable", "--out", str(dist_a),
        )
        if first.returncode != 0:
            add_problem(
                f"build_release.py --binary: {harness.text(first.stdout).rstrip(chr(10))}"
            )
        else:
            expect_file("build release archive", dist_a / archive_name)
            expect_file("build release checksum", dist_a / f"{archive_name}.sha256")
            expect_file(
                "build release per-target checksums", dist_a / f"SHA256SUMS.{target}"
            )

            digest = sha256_of(dist_a / archive_name)
            check()
            if (dist_a / f"{archive_name}.sha256").read_text().strip() != f"{digest}  {archive_name}":
                add_problem(f"the .sha256 file does not match the archive (expected {digest})")
            check()
            if (dist_a / f"SHA256SUMS.{target}").read_text().strip() != f"{digest}  {archive_name}":
                add_problem(f"SHA256SUMS.{target} does not match the archive")

            # The archive layout: one root directory holding the compiler and the documentation.
            with tarfile.open(dist_a / archive_name) as archive:
                names = {name.rstrip("/") for name in archive.getnames()}
            for entry in (
                f"burdin-9.9.9-{target}",
                f"burdin-9.9.9-{target}/burdin",
                f"burdin-9.9.9-{target}/README.md",
                f"burdin-9.9.9-{target}/CHANGELOG.md",
                f"burdin-9.9.9-{target}/docs/error-codes.md",
            ):
                expect_contains("archive layout", entry, "\n".join(sorted(names)))

            # Packaging is deterministic: the same binary, version, and epoch make the same bytes. The second run
            # starts a whole second later so a timestamp read from the clock instead of SOURCE_DATE_EPOCH cannot
            # pass by landing in the same second; bsdtar (macOS) packs the staging files' times, so that leak
            # used to make this fail intermittently there.
            time.sleep(1)
            check()
            second = run_release(
                "--binary", str(fake), "--target", target, "--version", "9.9.9",
                "--channel", "stable", "--out", str(work / "dist-b"),
            )
            if second.returncode != 0:
                add_problem(
                    "build_release.py --binary (second run): "
                    f"{harness.text(second.stdout).rstrip(chr(10))}"
                )
            else:
                second_hash = sha256_of(work / "dist-b" / archive_name)
                if digest != second_hash:
                    add_problem(f"packaging is not deterministic: {digest} vs {second_hash}")

            # A binary whose --version disagrees with the archive name is refused.
            wrong = work / "fake-wrong"
            harness.write_text(
                wrong, harness.read_text(fake).replace("burdin 9.9.9", "burdin 9.9.8")
            )
            wrong.chmod(0o755)
            check()
            packaged = run_release(
                "--binary", str(wrong), "--target", target, "--version", "9.9.9",
                "--channel", "stable", "--out", str(work / "dist-wrong"),
            )
            if packaged.returncode == 0:
                add_problem(
                    "build_release.py packaged a binary whose --version reports another version"
                )

            # The version and channel are validated before anything is built.
            check()
            if run_release(
                "--binary", str(fake), "--target", target, "--version", "9.9",
                "--channel", "stable", "--out", str(work / "dist-bad"),
            ).returncode == 0:
                add_problem("build_release.py accepted a version that is not MAJOR.MINOR.PATCH")
            check()
            if run_release(
                "--binary", str(fake), "--target", target, "--version", "9.9.9",
                "--channel", "beta", "--out", str(work / "dist-bad"),
            ).returncode == 0:
                add_problem("build_release.py accepted an unknown channel")
            check()
            if run_release(
                "--binary", str(fake), "--target", "riscv64-unknown-linux-gnu",
                "--version", "9.9.9", "--channel", "stable", "--out", str(work / "dist-bad"),
            ).returncode == 0:
                add_problem("build_release.py accepted an unsupported target")

    # --- The installers (issues #454, #455) ---------------------------------------------

    if INSTALL_SH.is_file():
        check()
        syntax = harness.run(["sh", "-n", INSTALL_SH])
        if syntax.returncode != 0:
            add_problem(
                f"install.sh: not valid POSIX sh: {harness.text(syntax.stderr).rstrip(chr(10))}"
            )
        install_text = harness.read_text(INSTALL_SH)
        for needle in (
            "SHA256SUMS",
            "sha256sum",
            "shasum",
            "BURDIN_INSTALL_DIR",
            "BURDIN_RELEASE_DIR",
            "install.ps1",
            "LLVM",
        ):
            check()
            if needle not in install_text:
                add_problem(f"install.sh: missing: {needle}")

    if INSTALL_PS1.is_file():
        ps_text = harness.read_text(INSTALL_PS1)
        for needle in (
            "Get-FileHash",
            "SHA256SUMS",
            "BURDIN_INSTALL_DIR",
            "BURDIN_RELEASE_DIR",
            "LLVM-C.dll",
        ):
            check()
            if needle not in ps_text:
                add_problem(f"install.ps1: missing: {needle}")

    linux_archive = dist_a / "burdin-9.9.9-x86_64-unknown-linux-gnu.tar.gz"
    if INSTALL_SH.is_file() and linux_archive.is_file():
        release = work / "release"
        release.mkdir()
        shutil.copyfile(linux_archive, release / linux_archive.name)
        shutil.copyfile(
            dist_a / "SHA256SUMS.x86_64-unknown-linux-gnu", release / "SHA256SUMS"
        )
        (release / "VERSION").write_text("9.9.9\n")

        check()
        installed = harness.run(
            ["sh", str(INSTALL_SH)],
            merge=True,
            env={
                **os.environ,
                "BURDIN_RELEASE_DIR": str(release),
                "BURDIN_TARGET": "x86_64-unknown-linux-gnu",
                "BURDIN_INSTALL_DIR": str(work / "install"),
            },
        )
        if installed.returncode != 0:
            add_problem(
                f"install.sh (local release): {harness.text(installed.stdout).rstrip(chr(10))}"
            )
        else:
            expect_file("install.sh installs the compiler", work / "install" / "burdin")
            check()
            version = run_binary(work / "install" / "burdin", "--version")
            first_line = harness.text(version.stdout).split("\n")[0]
            if first_line != "burdin 9.9.9":
                add_problem(f"install.sh installed a binary that reports '{first_line}'")

        # A tampered archive is refused and leaves nothing installed.
        tampered = work / "tampered"
        shutil.copytree(release, tampered)
        with open(tampered / linux_archive.name, "ab") as archive_file:
            archive_file.write(b"x")
        check()
        attacked = harness.run(
            ["sh", str(INSTALL_SH)],
            merge=True,
            env={
                **os.environ,
                "BURDIN_RELEASE_DIR": str(tampered),
                "BURDIN_TARGET": "x86_64-unknown-linux-gnu",
                "BURDIN_INSTALL_DIR": str(work / "install-tampered"),
            },
        )
        if attacked.returncode == 0:
            add_problem("install.sh accepted an archive whose checksum does not match")
        else:
            check()
            if (work / "install-tampered" / "burdin").exists():
                add_problem("install.sh installed a binary from a tampered archive")
            expect_contains(
                "install.sh checksum failure", "checksum mismatch", harness.text(attacked.stdout)
            )

        # A release without a checksum entry is refused too.
        missing = work / "missing"
        shutil.copytree(release, missing)
        (missing / "SHA256SUMS").write_bytes(b"")
        check()
        if harness.run(
            ["sh", str(INSTALL_SH)],
            merge=True,
            env={
                **os.environ,
                "BURDIN_RELEASE_DIR": str(missing),
                "BURDIN_TARGET": "x86_64-unknown-linux-gnu",
                "BURDIN_INSTALL_DIR": str(work / "install-missing"),
            },
        ).returncode == 0:
            add_problem("install.sh installed a release with no SHA256SUMS entry")

        check()
        if harness.run(
            ["sh", str(INSTALL_SH)],
            merge=True,
            env={
                **os.environ,
                "BURDIN_RELEASE_DIR": str(release),
                "BURDIN_TARGET": "riscv64-unknown-linux-gnu",
                "BURDIN_VERSION": "9.9.9",
                "BURDIN_INSTALL_DIR": str(work / "install-bad"),
            },
        ).returncode == 0:
            add_problem("install.sh accepted an unsupported target")
        check()
        if harness.run(
            ["sh", str(INSTALL_SH)],
            merge=True,
            env={
                **os.environ,
                "BURDIN_RELEASE_DIR": str(release),
                "BURDIN_TARGET": "x86_64-unknown-linux-gnu",
                "BURDIN_VERSION": "9.9.9",
                "BURDIN_CHANNEL": "dev",
                "BURDIN_INSTALL_DIR": str(work / "install-dev"),
            },
        ).returncode == 0:
            add_problem("install.sh accepted the dev channel, which is never published")

    # On Windows the PowerShell installer is exercised against the same fake release. The archive holds a shell script
    # named burdin.exe, so only the checksum, extraction, and placement are proven; running it is not.
    if INSTALL_PS1.is_file() and harness.PLATFORM == "windows":
        powershell = harness.which("powershell.exe") or harness.which("powershell")
        if powershell:
            check()
            syntax = harness.run(
                [
                    powershell,
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    f"[void][scriptblock]::Create((Get-Content -Raw -LiteralPath '{INSTALL_PS1}'))",
                ],
                merge=True,
            )
            if syntax.returncode != 0:
                detail = "\n".join(harness.text(syntax.stdout).split("\n")[:5])
                add_problem(f"install.ps1: PowerShell cannot parse it: {detail}")

            fake_windows = work / "fake-burdin-windows"
            harness.write_text(
                fake_windows,
                "#!/bin/sh\n"
                'if [ "${1:-}" = "--version" ]; then\n'
                "    cat <<'VERSION'\n"
                "burdin 9.9.9\n"
                "channel: stable\n"
                "target: x86_64-pc-windows-msvc\n"
                "llvm: 21.1.8\n"
                "commit: test\n"
                "profile: release\n"
                "VERSION\n"
                "    exit 0\n"
                "fi\n"
                "exit 1\n",
            )
            fake_windows.chmod(0o755)
            win_target = "x86_64-pc-windows-msvc"
            win_archive = f"burdin-9.9.9-{win_target}.tar.gz"
            check()
            packaged = run_release(
                "--binary", str(fake_windows), "--target", win_target, "--version", "9.9.9",
                "--channel", "stable", "--out", str(work / "dist-win"),
            )
            if packaged.returncode != 0:
                add_problem(
                    "build_release.py --binary (Windows archive): "
                    f"{harness.text(packaged.stdout).rstrip(chr(10))}"
                )
            else:
                win_release = work / "release-win"
                win_release.mkdir()
                shutil.copyfile(work / "dist-win" / win_archive, win_release / win_archive)
                shutil.copyfile(
                    work / "dist-win" / f"SHA256SUMS.{win_target}", win_release / "SHA256SUMS"
                )
                check()
                installed_ps = harness.run(
                    [
                        powershell,
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                        str(INSTALL_PS1),
                        "-Version",
                        "9.9.9",
                        "-Target",
                        win_target,
                        "-ReleaseDir",
                        str(win_release),
                        "-InstallDir",
                        str(work / "install-win"),
                    ],
                    merge=True,
                )
                if installed_ps.returncode != 0:
                    detail = "\n".join(harness.text(installed_ps.stdout).split("\n")[:5])
                    add_problem(f"install.ps1 (local release): {detail}")
                else:
                    check()
                    if not (work / "install-win" / "burdin.exe").is_file():
                        add_problem("install.ps1 did not install burdin.exe")

    # --- Release notes (issue #456) ------------------------------------------------------

    if NOTES.is_file():
        check()
        notes = run_notes("--unreleased", "--check-links")
        if notes.returncode != 0:
            add_problem(
                "release_notes.py --unreleased --check-links: "
                f"{harness.text(notes.stderr).rstrip(chr(10))}"
            )
        else:
            notes_text = harness.text(notes.stdout)
            expect_contains("release notes title", "# Burdin ", notes_text)
            check()
            if len(notes_text.rstrip("\n").split("\n")) <= 2:
                add_problem("release notes are empty")

        fixture = work / "CHANGELOG-fixture.md"
        harness.write_text(
            fixture,
            "# Changelog\n"
            "\n"
            "## Unreleased — Test\n"
            "\n"
            "- See [README](README.md) and [the error codes](docs/error-codes.md).\n"
            "\n"
            "## 1.2.3 — 2026-01-01\n"
            "\n"
            "- See [a missing file](does-not-exist.md).\n",
        )
        check()
        versioned = run_notes("--changelog", str(fixture), "--version", "1.2.3")
        if versioned.returncode != 0 or "# Burdin 1.2.3" not in harness.text(versioned.stdout):
            add_problem("release_notes.py --version did not extract the versioned section")
        check()
        if run_notes(
            "--changelog", str(fixture), "--version", "1.2.3", "--check-links"
        ).returncode == 0:
            add_problem("release_notes.py --check-links accepted a broken link")
        check()
        if run_notes("--changelog", str(fixture), "--version", "9.9.9").returncode == 0:
            add_problem("release_notes.py accepted a version with no changelog section")
        check()
        if run_notes(
            "--changelog", str(fixture), "--unreleased", "--check-links"
        ).returncode != 0:
            add_problem("release_notes.py rejected a valid Unreleased section")

    # --- The driver reports its version and channel (issues #454, #456) ------------------

    check()
    version = harness.run([burdin, "--version"])
    if version.returncode != 0:
        add_problem(
            f"burdin --version: exit status {version.returncode}: "
            f"{harness.text(version.stderr).rstrip(chr(10))}"
        )
    else:
        version_text = harness.text(version.stdout).rstrip("\n")
        lines = version_text.split("\n")
        check()
        if len(lines) != 6:
            add_problem(f"burdin --version: {len(lines)} lines, expected 6")
        for prefix in ("burdin ", "channel: ", "target: ", "llvm: ", "commit: ", "profile: "):
            check()
            if not any(line.startswith(prefix) for line in lines):
                add_problem(f"burdin --version: no '{prefix}' line")
        check()
        if not any(re.fullmatch(r"channel: (dev|nightly|stable)", line) for line in lines):
            add_problem("burdin --version: the channel is not dev, nightly, or stable")
        manifest_match = re.search(
            r'^version = "(.*)"$', harness.read_text(harness.ROOT / "Cargo.toml"), re.MULTILINE
        )
        manifest = manifest_match.group(1) if manifest_match else ""
        check()
        if not os.environ.get("BURDIN_VERSION") and f"burdin {manifest}" not in lines:
            add_problem(f"burdin --version: the version is not the manifest's ({manifest})")
        check()
        if "channel:" not in readme:
            add_problem("README.md: does not document the channel line")
        check()
        if "BURDIN_CHANNEL" not in readme:
            add_problem("README.md: does not document BURDIN_CHANNEL")

print(f"Checked the release pipeline ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
