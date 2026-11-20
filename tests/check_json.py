#!/usr/bin/env python3
"""Checks the `json` official package (stage 3, issue #255; spec/serialization.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every fixture under `tests/json` (round trips, pretty output, and malformed input) against its `.expected`
    golden: the compact form, the decoded fields, and the error message with its line, column, and path;
  * a malformed fixture never aborts the process: the exit status is 0 and the error is reported as text;
  * `burdin add json` writes `json = true`, prints the official-package line, and pins the package in `burdin.lock`
    with `source = "official"` and the PKG7 checksum of its files;
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

FIXTURES = harness.TESTS / "json"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "jsoncheck"\nversion = "0.1.0"\n\n[dependencies]\njson = true\n'

# The program the fixtures drive. It reads a fixture's path from the command line (so a non-ASCII fixture never
# depends on how a platform spells an argument), then re-emits it and reports what the decoded fields hold.
PROGRAM = """\
use json

struct Server: Serialize, Deserialize, Debug {
    host: String
    port: u16
    tls: bool
    ratio: f64
}

enum Protocol: Serialize, Deserialize, Debug {
    Http
    Https(u16)
    Both(u16, u16)
}

struct Config: Serialize, Deserialize, Debug {
    name: String
    servers: List<Server>
    protocol: Protocol
    backup: Option<Server>
    weights: Map<String, i32>
    note: char
}

fn main() {
    let args = env.args()
    let mode = args[1]
    let text = read_to_string(args[2].as_str()).unwrap()
    let read: Result<Config, DeError> = json.from_str(text.as_str())
    match read {
        Ok(config) => {
            if mode.as_str() == "pretty" {
                print(json.to_string_pretty(config).unwrap())
            } else {
                println(json.to_string(config).unwrap())
            }
        }
        Err(e) => println(e.to_string()),
    }

    let again: Result<Config, DeError> = json.from_str(text.as_str())
    match again {
        Ok(config) => {
            println("name={} servers={} note={}", config.name, config.servers.len(), config.note)
            match config.protocol {
                Protocol.Http => println("protocol=Http"),
                Protocol.Https(port) => println("protocol=Https {port}"),
                Protocol.Both(first, second) => println("protocol=Both {first} {second}"),
            }
            match config.backup {
                Some(server) => println("backup={} {}", server.host, server.port),
                None => println("backup=none"),
            }
            println("weights={}", config.weights.len())
            match config.weights.get("a") {
                Some(weight) => println("a={weight}"),
                None => println("a=none"),
            }
            if config.servers.len() > 0 {
                println(
                    "first={} {} {} {}",
                    config.servers[0].host,
                    config.servers[0].port,
                    config.servers[0].tls,
                    config.servers[0].ratio,
                )
            }
        }
        Err(_) => {}
    }
}
"""

MODES = (("roundtrip", "compact"), ("pretty", "pretty"), ("bad", "compact"))


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


manifest_text = harness.read_text(PACKAGES / "json" / "burdin.toml")
check(re.search(r'^name = "json"$', manifest_text, re.M) is not None, "packages/json does not declare `name = \"json\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/json has no version")
check((PACKAGES / "json" / "src" / "json.bd").is_file(), "packages/json has no src/json.bd")
readme = PACKAGES / "json" / "README.md"
check(readme.is_file() and "json = true" in harness.read_text(readme), "packages/json/README.md is missing or does not say how to depend on json")

with harness.temporary_dir("burdin-json-") as work:
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
    write(app / "src" / "main.bd", PROGRAM)
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"the json package did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "jsoncheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    if binary.is_file():
        for folder, mode in MODES:
            fixtures = sorted((FIXTURES / folder).glob("*.json"))
            check(len(fixtures) >= 3, f"tests/json/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{fixture.name} has no .expected file")
                if not expected.is_file():
                    continue
                result = harness.run([binary, mode, str(fixture)], cwd=app, env=env)
                actual = harness.normalize(result.stdout)
                wanted = harness.normalize(harness.read_bytes(expected))
                check(result.returncode == 0, f"{folder}/{fixture.name} ended with status {result.returncode}: {err(result)[:300]}")
                check(
                    actual == wanted,
                    f"{folder}/{fixture.name} printed {actual!r}, expected {wanted!r}",
                )

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "json")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the json package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # `burdin add json` in a package that does not declare it yet.
    adder = work / "adder"
    write(adder / "burdin.toml", '[package]\nname = "adder"\nversion = "0.1.0"\n')
    write(adder / "src" / "main.bd", "use json\n\nfn main() {\n    println(\"ok\")\n}\n")
    added = run_in(adder, "add", "json")
    check(added.returncode == 0, f"`burdin add json` failed: {err(added)[:300]}")
    check(out(added) == "Added dependency `json` (official)\n", f"`burdin add json` printed {out(added)!r}")
    check(
        harness.read_text(adder / "burdin.toml") == '[package]\nname = "adder"\nversion = "0.1.0"\n\n[dependencies]\njson = true\n',
        f"`burdin add json` wrote {harness.read_text(adder / 'burdin.toml')!r}",
    )
    adder_built = run_in(adder, "build", ".")
    check(adder_built.returncode == 0, f"the package `burdin add json` edited did not build: {err(adder_built)[:300]}")
    check('source = "official"' in harness.read_text(adder / "burdin.lock"), "`burdin add json` wrote no official pin")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "json" / "src" / "json.bd", harness.read_text(official / "json" / "src" / "json.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed json package (status {changed.returncode})")
    check(
        "`json` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "json")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the json package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
