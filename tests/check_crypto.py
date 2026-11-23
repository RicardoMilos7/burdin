#!/usr/bin/env python3
"""Checks the `crypto` official package (stage 3, issue #266; spec/crypto.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every known-answer fixture under `tests/crypto` (hashes, SHAKE, HMAC, HKDF, PBKDF2, ChaCha20, HChaCha20,
    Poly1305, ChaCha20-Poly1305, XChaCha20-Poly1305, AES block/CTR/CBC/GCM, X25519, Ed25519) against the package,
    through the runner in `tests/crypto/runner`. Every vector either is published (RFC 4231, 5869, 6070, 7748, 8032,
    8439, FIPS 180/197/202, NIST SP 800-38A/D) or comes from OpenSSL; `tests/crypto/regenerate.py` says which and
    asserts the published ones before writing anything. The runner also hashes incrementally in odd chunk sizes,
    decrypts what it encrypts, and checks that a changed message, tag, nonce length, or signature is rejected;
  * the error fixture: the message of every failing call (wrong key or nonce length, a bad tag, bad padding,
    a low-order X25519 point, a malformed signature), so no failure is a crash;
  * the runner itself notices a wrong vector (a fixture with one flipped digit must report a failure);
  * `use crypto` loads the package with no manifest entry, and `burdin.lock` pins it with `source = "official"` and the PKG7 checksum of its files;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it.
"""

import hashlib
import os
import re
import shutil
import sys
from pathlib import Path

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()

FIXTURES = harness.TESTS / "crypto"
RUNNER = FIXTURES / "runner"
PACKAGES = harness.ROOT / "packages"

# fixture file (relative to tests/crypto) -> (runner mode, the fewest fixture lines it must hold)
KNOWN_ANSWERS = {}
for algorithm in ("md5", "sha1", "sha224", "sha256", "sha384", "sha512", "sha3_224", "sha3_256", "sha3_384", "sha3_512"):
    KNOWN_ANSWERS[f"hash/{algorithm}.kat"] = ("hash", 11)
    KNOWN_ANSWERS[f"hmac/{algorithm}.kat"] = ("hmac", 11)
KNOWN_ANSWERS["hash/sha256_million.kat"] = ("hash", 1)
KNOWN_ANSWERS["hash/shake.kat"] = ("shake", 18)
for algorithm in ("sha1", "sha256", "sha384", "sha512", "sha3_256"):
    KNOWN_ANSWERS[f"hkdf/{algorithm}.kat"] = ("hkdf", 5)
    KNOWN_ANSWERS[f"pbkdf2/{algorithm}.kat"] = ("pbkdf2", 6)
KNOWN_ANSWERS.update(
    {
        "chacha20/rfc8439.kat": ("chacha20", 1),
        "chacha20/derived.kat": ("chacha20", 18),
        "chacha20/hchacha20.kat": ("hchacha20", 1),
        "poly1305/rfc8439.kat": ("poly1305", 1),
        "poly1305/derived.kat": ("poly1305", 11),
        "aead/chacha20poly1305.kat": ("aead", 17),
        "aead/xchacha20poly1305.kat": ("aead", 16),
        "aead/aesgcm.kat": ("aead", 50),
        "aes/ecb.kat": ("aes_ecb", 3),
        "aes/ctr.kat": ("aes_ctr", 26),
        "aes/cbc.kat": ("aes_cbc", 25),
        "x25519/dh.kat": ("x25519_dh", 8),
        "x25519/public.kat": ("x25519_public", 6),
        "ed25519/sign.kat": ("ed25519", 18),
        "errors/scenarios.kat": ("errors", 28),
    }
)
MANIFEST = '[package]\nname = "cryptokat"\nversion = "0.1.0"\n'


def tree_checksum(directory: Path) -> str:
    """PKG7: `sha256:` and the SHA-256 of every file's path, a NUL, its length, a NUL, and its bytes, by path."""
    files = []
    for path in directory.rglob("*"):
        relative = path.relative_to(directory).as_posix()
        if path.is_file() and relative.split("/")[0] not in (".git", "target"):
            files.append((relative, path))
    digest = hashlib.sha256()
    for relative, path in sorted(files):
        data = path.read_bytes()
        digest.update(relative.encode() + b"\0" + str(len(data)).encode() + b"\0" + data)
    return "sha256:" + digest.hexdigest()


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def snapshot(directory: Path):
    def read(name):
        path = directory / name
        return harness.read_bytes(path) if path.exists() else None

    return read("burdin.toml"), read("burdin.lock")


manifest_text = harness.read_text(PACKAGES / "crypto" / "burdin.toml")
check(re.search(r'^name = "crypto"$', manifest_text, re.M) is not None, "packages/crypto does not declare `name = \"crypto\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/crypto has no version")
check((PACKAGES / "crypto" / "src" / "crypto.bd").is_file(), "packages/crypto has no src/crypto.bd")
readme = PACKAGES / "crypto" / "README.md"
check(readme.is_file() and "use crypto" in harness.read_text(readme), "packages/crypto/README.md is missing or does not show `use crypto`")
check(
    readme.is_file() and "experiment" in harness.read_text(readme).lower(),
    "packages/crypto/README.md does not say that the package is an experiment, not a security claim",
)

# Every fixture on disk is accounted for, so a new one cannot be forgotten.
on_disk = {path.relative_to(FIXTURES).as_posix() for path in FIXTURES.rglob("*.kat")}
check(on_disk == set(KNOWN_ANSWERS), f"tests/crypto fixtures and this script disagree: only on disk {sorted(on_disk - set(KNOWN_ANSWERS))}, only here {sorted(set(KNOWN_ANSWERS) - on_disk)}")

with harness.temporary_dir("burdin-crypto-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    official = work / "packages"
    shutil.copytree(PACKAGES, official)
    env["BURDIN_PACKAGES"] = str(official)

    def run_in(directory, *arguments, environment=None):
        return harness.run([burdin, *arguments], cwd=directory, env=environment or env)

    def out(result):
        return harness.text(harness.normalize(result.stdout))

    def err(result):
        return harness.text(harness.normalize(result.stderr))

    app = work / "app"
    write(app / "burdin.toml", MANIFEST)
    write(app / "src" / "main.bd", harness.read_text(RUNNER / "src" / "main.bd"))
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"the crypto package or its runner did not build: {err(built)[:600]}")
    binary = harness.executable_in(app, "cryptokat")
    check(binary.is_file(), "`burdin build .` did not produce the runner")

    if binary.is_file():
        total = 0
        for relative, (mode, minimum) in sorted(KNOWN_ANSWERS.items()):
            fixture = FIXTURES / relative
            lines = [line for line in harness.read_text(fixture).splitlines() if line.strip()]
            check(len(lines) >= minimum, f"tests/crypto/{relative} has too few vectors ({len(lines)}, want at least {minimum})")
            result = harness.run([binary, mode, str(fixture)], cwd=app, env=env)
            printed = harness.text(harness.normalize(result.stdout))
            check(result.returncode == 0, f"{relative} ended with status {result.returncode}: {err(result)[:300]}")
            report = printed.strip().splitlines()
            check(bool(report) and re.fullmatch(r"cases=\d+ failures=0", report[-1]) is not None, f"{relative}: {[l for l in report if 'FAIL' in l][:3] or report[-1:]}")
            names = {line.split(": ")[0] for line in report if line.endswith(": ok")}
            for line in lines:
                name = line.split("|")[0]
                check(name in names, f"{relative}: vector `{name}` did not pass")
            total += len(lines)
        check(total >= 450, f"only {total} known-answer vectors in all")

        # The runner must notice a wrong vector: one flipped digit in a digest has to be reported.
        wrong = work / "wrong.kat"
        original = harness.read_text(FIXTURES / "hash" / "sha256.kat").splitlines()[1]
        head, _, digest = original.rpartition("|")
        flipped = ("0" if digest[0] != "0" else "1") + digest[1:]
        write(wrong, f"{head}|{flipped}\n")
        broken = harness.run([binary, "hash", str(wrong)], cwd=app, env=env)
        check("FAIL" in out(broken) and "failures=0" not in out(broken), "the runner did not report a wrong digest")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "crypto")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the crypto package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "crypto" / "src" / "crypto.bd", harness.read_text(official / "crypto" / "src" / "crypto.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed crypto package (status {changed.returncode})")
    check(
        "`crypto` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "crypto")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the crypto package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
