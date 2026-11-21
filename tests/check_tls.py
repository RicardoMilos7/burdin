#!/usr/bin/env python3
"""Checks the `tls` official package (stage 3, issue #262; spec/tls.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py) and OpenSSL 3 to link against (TLS7). The
`openssl` command-line tool is used when it is installed, and every step that needs it says so when it is skipped.
BURDIN_PACKAGES points at a copy of this repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * the committed test certificates under tests/s3_tls/conformance/src/certs: every file is there, the valid chain
    verifies against the test root, the expired and self-signed leaves do not, the wrong-host leaf names another host,
    the valid leaves last at least 60 more years (so the tests do not go stale), every key belongs to its certificate,
    and the encrypted key is encrypted (needs the `openssl` tool);
  * the conformance program `tests/s3_tls/conformance`: its output is compared with the `//>` lines of its
    `src/main.bd`, the way tests/run_tests.py does;
  * every function `packages/tls/src/openssl.bd` declares exists in the installed libssl and libcrypto (a mistyped
    name fails the link anyway, but only for the functions a program uses; this finds all of them. Needs `nm` and a
    Unix library; skipped with a message elsewhere);
  * the README's client and server examples type-check (`burdin check`);
  * `burdin add tls` writes `tls = true`, prints the official-package line, and pins the package in `burdin.lock`
    with `source = "official"` and the PKG7 checksum of its files;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it.
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timedelta, timezone
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

PACKAGES = harness.ROOT / "packages"
CERTS = harness.TESTS / "s3_tls" / "conformance" / "src" / "certs"
HTTP_CERTS = harness.TESTS / "s3_http" / "tls" / "src" / "certs"
MANIFEST = '[package]\nname = "tlscheck"\nversion = "0.1.0"\n\n[dependencies]\ntls = true\n'
APP = 'use tls\nuse tls.{ClientConfig, TlsError}\n\nfn main() {\n    match ClientConfig.new() {\n        Ok(_) => println("ok"),\n        Err(failure) => println("{}", failure),\n    }\n}\n'

CERTIFICATES = ["root", "inter", "server", "chain", "rsa", "ed25519", "wrong_host", "self_signed", "expired", "client"]
KEYS = ["server", "rsa", "ed25519", "wrong_host", "self_signed", "expired", "client"]
SIXTY_YEARS = str(60 * 365 * 24 * 3600)


def first_difference(got, wanted) -> str:
    for index in range(max(len(got), len(wanted))):
        a = got[index] if index < len(got) else "<missing>"
        b = wanted[index] if index < len(wanted) else "<missing>"
        if a != b:
            return f"line {index + 1} is {a!r}, expected {b!r}"
    return "no difference"


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


def tool(*arguments, cwd=None):
    """Runs the openssl command-line tool; returns (status, output)."""
    result = subprocess.run(["openssl", *arguments], cwd=cwd, capture_output=True, text=True)
    return result.returncode, result.stdout + result.stderr


# ----- the package files -----------------------------------------------------------------------------------------

manifest_text = harness.read_text(PACKAGES / "tls" / "burdin.toml")
check(re.search(r'^name = "tls"$', manifest_text, re.M) is not None, "packages/tls does not declare `name = \"tls\"`")
check("[target.'cfg(unix)'.link]" in manifest_text and "[target.'cfg(windows)'.link]" in manifest_text, "packages/tls/burdin.toml has no [link] tables")
for name in ("tls", "openssl", "errors", "certs", "config", "engine", "stream", "ffi"):
    check((PACKAGES / "tls" / "src" / f"{name}.bd").is_file(), f"packages/tls has no src/{name}.bd")
readme = PACKAGES / "tls" / "README.md"
check(readme.is_file() and "tls = true" in harness.read_text(readme), "packages/tls/README.md is missing or does not say how to depend on tls")

# Every public name the modules declare is re-exported from the root, because a dependency's other modules are hidden.
root_text = harness.read_text(PACKAGES / "tls" / "src" / "tls.bd")
for module, names in {
    "certs": ["Certificate", "PrivateKey", "TrustStore"],
    "config": ["ClientConfig", "ServerConfig", "TlsVersion"],
    "errors": ["TlsError"],
    "stream": ["TlsStream"],
}.items():
    for name in names:
        check(name in root_text, f"packages/tls/src/tls.bd does not re-export {module}.{name}")

# ----- the certificates ------------------------------------------------------------------------------------------

for name in CERTIFICATES:
    check((CERTS / f"{name}.pem").is_file(), f"tests/s3_tls/conformance/src/certs/{name}.pem is missing")
for name in KEYS:
    check((CERTS / f"{name}.key").is_file(), f"tests/s3_tls/conformance/src/certs/{name}.key is missing")
check((CERTS / "encrypted.key").is_file(), "tests/s3_tls/conformance/src/certs/encrypted.key is missing")
check((harness.TESTS / "tls" / "regenerate.py").is_file(), "tests/tls/regenerate.py is missing")
check("ENCRYPTED" in harness.read_text(CERTS / "encrypted.key"), "certs/encrypted.key is not an encrypted key")
check(not any("ENCRYPTED" in harness.read_text(CERTS / f"{name}.key") for name in KEYS), "a key meant to be plain is encrypted")

if shutil.which("openssl") is None:
    print("openssl command-line tool not found: skipping the certificate checks")
else:
    root, inter = CERTS / "root.pem", CERTS / "inter.pem"

    def verify(leaf: str, *extra):
        return tool("verify", "-CAfile", str(root), *extra, str(CERTS / f"{leaf}.pem"))

    status, output = verify("server", "-untrusted", str(inter))
    check(status == 0, f"the server leaf does not verify through the intermediate: {output[:300]}")
    status, output = verify("server")
    check(status != 0, "the server leaf verifies without its intermediate: the test for a missing intermediate is void")
    for leaf in ("rsa", "ed25519", "client"):
        status, output = verify(leaf)
        check(status == 0, f"the {leaf} leaf does not verify against the root: {output[:300]}")
    status, output = verify("expired")
    check(status != 0 and "expired" in output, f"the expired leaf does not fail as expired: {output[:300]}")
    status, output = verify("self_signed")
    check(status != 0, "the self-signed leaf verifies against the test root")
    status, output = verify("wrong_host")
    check(status == 0, f"the wrong-host leaf does not verify: {output[:300]}")
    status, output = tool("x509", "-noout", "-ext", "subjectAltName", "-in", str(CERTS / "wrong_host.pem"))
    check("other.test" in output and "localhost" not in output, f"the wrong-host leaf names {output[:200]!r}")
    for leaf in ("server", "rsa", "ed25519"):
        status, output = tool("x509", "-noout", "-ext", "subjectAltName", "-in", str(CERTS / f"{leaf}.pem"))
        check(
            "DNS:localhost" in output and "DNS:example.test" in output and "127.0.0.1" in output,
            f"the {leaf} leaf does not name localhost, example.test, and 127.0.0.1: {output[:200]!r}",
        )
    for name in ("root", "inter", "server", "rsa", "ed25519", "wrong_host", "self_signed", "client"):
        status, output = tool("x509", "-noout", "-checkend", SIXTY_YEARS, "-in", str(CERTS / f"{name}.pem"))
        check(status == 0, f"{name}.pem expires within 60 years: the conformance tests would go stale")
    for name in ("root", "inter", "server", "rsa", "ed25519", "wrong_host", "self_signed", "client"):
        # Valid from a year before they were made, so a runner whose clock or zone is off by days still verifies them.
        status, output = tool("x509", "-noout", "-startdate", "-in", str(CERTS / f"{name}.pem"))
        try:
            started = datetime.strptime(output.strip().split("=", 1)[1], "%b %d %H:%M:%S %Y GMT").replace(tzinfo=timezone.utc)
        except (IndexError, ValueError):
            started = datetime.now(timezone.utc)
        check(started <= datetime.now(timezone.utc) - timedelta(days=300), f"{name}.pem is valid from {started}: it is not backdated by a year")
    for name in KEYS:
        status_cert, from_cert = tool("x509", "-noout", "-pubkey", "-in", str(CERTS / f"{name}.pem"))
        status_key, from_key = tool("pkey", "-pubout", "-in", str(CERTS / f"{name}.key"))
        check(status_cert == 0 and status_key == 0 and from_cert == from_key, f"{name}.key is not the key of {name}.pem")
    status, output = tool("pkey", "-noout", "-passin", "pass:burdin", "-in", str(CERTS / "encrypted.key"))
    check(status == 0, f"encrypted.key does not open with the password `burdin`: {output[:200]}")
    status, output = tool("pkey", "-noout", "-passin", "pass:wrong", "-in", str(CERTS / "encrypted.key"))
    check(status != 0, "encrypted.key opens without its password")
    # The `http` tests keep their own copies of the files they use (tests/tls/regenerate.py writes both).
    for name in ("root.pem", "chain.pem", "server.key", "wrong_host.pem", "wrong_host.key", "expired.pem", "expired.key", "self_signed.pem", "self_signed.key"):
        copy = HTTP_CERTS / name
        check(copy.is_file() and harness.read_bytes(copy) == harness.read_bytes(CERTS / name), f"tests/s3_http/tls/src/certs/{name} is missing or differs from the conformance copy")
    chain = harness.read_text(CERTS / "chain.pem")
    check(
        chain == harness.read_text(CERTS / "server.pem") + harness.read_text(CERTS / "inter.pem"),
        "chain.pem is not server.pem followed by inter.pem",
    )

# ----- the declared functions exist in the libraries --------------------------------------------------------------


def library_files():
    directories = [Path(p) for p in os.environ.get("LIBRARY_PATH", "").split(os.pathsep) if p]
    directories += [Path(p) for p in ("/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib", "/lib/x86_64-linux-gnu", "/lib/aarch64-linux-gnu", "/usr/local/lib", "/opt/homebrew/opt/openssl@3/lib", "/usr/local/opt/openssl@3/lib")]
    found = []
    for directory in directories:
        if not directory.is_dir():
            continue
        for stem in ("libssl", "libcrypto"):
            for path in sorted(directory.glob(stem + ".*")):
                if (".so" in path.name or path.name.endswith(".dylib")) and path not in found:
                    found.append(path)
    return found


declared = re.findall(r"^\s*fn (\w+)\(", harness.read_text(PACKAGES / "tls" / "src" / "openssl.bd"), re.M)
check(len(declared) >= 60, f"packages/tls/src/openssl.bd declares only {len(declared)} functions")
check(len(declared) == len(set(declared)), "packages/tls/src/openssl.bd declares a function twice")
libraries = library_files()
if harness.PLATFORM == "windows" or shutil.which("nm") is None or not libraries:
    print("no nm or no OpenSSL shared libraries found: skipping the symbol check of packages/tls/src/openssl.bd")
else:
    exported = set()
    for library in libraries:
        flags = ["-gU"] if harness.PLATFORM == "macos" else ["-D", "--defined-only"]
        result = subprocess.run(["nm", *flags, str(library)], capture_output=True, text=True)
        for line in result.stdout.splitlines():
            parts = line.split()
            if len(parts) >= 3 and parts[1] in ("T", "t", "D", "B", "W", "w"):
                exported.add(parts[2].split("@")[0].lstrip("_") if harness.PLATFORM == "macos" else parts[2].split("@")[0])
    missing = [name for name in declared if name not in exported]
    check(not missing, f"openssl.bd declares functions the installed libssl/libcrypto do not export: {missing}")

# ----- the conformance program, the official pin, and `burdin add` -----------------------------------------------

with harness.temporary_dir("burdin-tls-") as work:
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(work / "home")
    official = work / "packages"
    shutil.copytree(PACKAGES, official)
    env["BURDIN_PACKAGES"] = str(official)

    def run_in(directory, *arguments):
        return harness.run([burdin, *arguments], cwd=directory, env=env)

    def out(result):
        return harness.text(harness.normalize(result.stdout))

    def err(result):
        return harness.text(harness.normalize(result.stderr))

    app = work / "app"
    write(app / "burdin.toml", MANIFEST)
    write(app / "src" / "main.bd", APP)
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"a program that depends on tls did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "tlscheck")
    if binary.is_file():
        ran = harness.run([binary], cwd=app, env=env, timeout=60)
        check(ran.returncode == 0 and out(ran) == "ok\n", f"the smoke program printed {out(ran)!r} (status {ran.returncode})")
    else:
        check(False, "`burdin build .` did not produce the smoke program")

    # The conformance program: clients and servers in one process. It is built from a copy, so its `burdin.lock` and
    # `target` stay out of the tree, and its output is the `//>` lines of its main file.
    conformance = work / "conformance"
    shutil.copytree(harness.TESTS / "s3_tls" / "conformance", conformance)
    wanted_lines = [
        line[4:] if line.startswith("//> ") else line[3:]
        for line in harness.read_text(conformance / "src" / "main.bd").splitlines()
        if line.startswith("//>")
    ]
    check(len(wanted_lines) >= 100, f"tests/s3_tls/conformance expects too few lines ({len(wanted_lines)})")
    conformance_built = run_in(conformance, "build", ".")
    check(conformance_built.returncode == 0, f"tests/s3_tls/conformance did not build: {err(conformance_built)[:400]}")
    conformance_binary = harness.executable_in(conformance, "tls_conformance")
    if conformance_binary.is_file():
        ran = harness.run([conformance_binary], cwd=conformance, env=env, timeout=180)
        check(ran.returncode == 0, f"tests/s3_tls/conformance ended with status {ran.returncode}: {err(ran)[:300]}")
        got_lines = out(ran).splitlines()
        check(got_lines == wanted_lines, "tests/s3_tls/conformance printed other lines than its //> lines: " + first_difference(got_lines, wanted_lines))
    else:
        check(False, "`burdin build .` did not produce the conformance program")

    # The README's examples must compile: every ```burdin block that starts with `// example: name` is type-checked as a
    # program of its own.
    examples = re.findall(r"```burdin\n(// example: (\w+)\n.*?)```", harness.read_text(readme), re.S)
    check(len(examples) >= 2, f"packages/tls/README.md has {len(examples)} checked examples, expected a client and a server")
    for source, name in examples:
        example = work / f"example_{name}"
        write(example / "burdin.toml", f'[package]\nname = "example_{name}"\nversion = "0.1.0"\n\n[dependencies]\ntls = true\n')
        write(example / "src" / "main.bd", source)
        checked = run_in(example, "check", ".")
        check(checked.returncode == 0, f"the README's {name} example does not compile: {err(checked)[:500]}")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    check(f'checksum = "{tree_checksum(official / "tls")}"' in lock, "burdin.lock does not pin the tls package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # `burdin add tls` in a package that does not declare it yet.
    adder = work / "adder"
    write(adder / "burdin.toml", '[package]\nname = "adder"\nversion = "0.1.0"\n')
    write(adder / "src" / "main.bd", APP)
    added = run_in(adder, "add", "tls")
    check(added.returncode == 0, f"`burdin add tls` failed: {err(added)[:300]}")
    check(out(added) == "Added dependency `tls` (official)\n", f"`burdin add tls` printed {out(added)!r}")
    check(
        harness.read_text(adder / "burdin.toml") == '[package]\nname = "adder"\nversion = "0.1.0"\n\n[dependencies]\ntls = true\n',
        f"`burdin add tls` wrote {harness.read_text(adder / 'burdin.toml')!r}",
    )
    adder_built = run_in(adder, "build", ".")
    check(adder_built.returncode == 0, f"the package `burdin add tls` edited did not build: {err(adder_built)[:300]}")
    check('source = "official"' in harness.read_text(adder / "burdin.lock"), "`burdin add tls` wrote no official pin")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "tls" / "src" / "stream.bd", harness.read_text(official / "tls" / "src" / "stream.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed tls package (status {changed.returncode})")
    check(
        "`tls` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "tls")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the tls package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
