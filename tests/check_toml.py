#!/usr/bin/env python3
"""Checks the `toml` official package (stage 3, issue #256; spec/toml.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * `roundtrip` (parse, then render the canonical form) for every fixture under `tests/toml/roundtrip`, and the
    syntax error of every malformed fixture under `tests/toml/bad`, each against its `.expected` golden;
  * `spans` (each key and value with its line and column) under `tests/toml/spans`;
  * `config` (reading a `Config` with `from_str`) under `tests/toml/config`, one fixture per framework rule;
  * `write` (writing a `Config` with `to_string`, reading it back) against `tests/toml/write/sample.expected`;
  * every valid `roundtrip` and `spans` fixture again with CRLF line endings, which must print the same output;
  * a malformed or mismatched fixture never aborts the process: the exit status is 0 and the error is text;
  * `use toml` loads the package with no manifest entry, and `burdin.lock` pins it
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

FIXTURES = harness.TESTS / "toml"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "tomlcheck"\nversion = "0.1.0"\n'

# The program the fixtures drive. The first argument is the mode and the second the fixture (`write` takes none).
# `roundtrip` prints the canonical text or the syntax error, `spans` prints each key and value with its position,
# `config` reads a `Config` and reports its fields or the error, and `write` writes two samples and reads them back.
PROGRAM = """\
use toml

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

fn print_spans(value: toml.Value, path: String) {
    match value {
        toml.Value.Table(entries) => {
            for entry in entries {
                let child = if path.len() == 0 { entry.key.clone() } else { format("{path}.{entry.key}") }
                println("{} key {}:{} value {}:{}", child, entry.key_span.line, entry.key_span.column, entry.item.span.line, entry.item.span.column)
                print_spans(*entry.item.value, child)
            }
        }
        toml.Value.Array(items) => {
            let mut index: usize = 0
            for item in items {
                let child = format("{path}[{index}]")
                println("{} value {}:{}", child, item.span.line, item.span.column)
                print_spans(*item.value, child)
                index = index + 1
            }
        }
        _ => {}
    }
}

fn reread(text: str) {
    let read: Result<Config, DeError> = toml.from_str(text)
    match read {
        Result.Ok(config) => {
            let backup = match config.backup {
                Option.Some(server) => server.host,
                Option.None => "none".to_string(),
            }
            println("reread: name={} servers={} backup={}", config.name, config.servers.len(), backup)
        }
        Result.Err(e) => println("reread: {}", e.to_string()),
    }
}

fn sample(with_backup: bool): Config {
    let mut servers = List<Server>.new()
    servers.push(Server { host: "a.example".to_string(), port: 80, tls: false, ratio: 0.25 })
    servers.push(Server { host: "b.example".to_string(), port: 443, tls: true, ratio: 1.0 })
    let mut weights = Map<String, i32>.new()
    weights.insert("a".to_string(), 1)
    let backup: Option<Server> = if with_backup {
        Option.Some(Server { host: "c.example".to_string(), port: 8080, tls: true, ratio: 2.5 })
    } else {
        Option.None
    }
    Config {
        name: "edge".to_string(),
        servers,
        protocol: Protocol.Both(80, 443),
        backup,
        weights,
        note: 'é',
    }
}

fn main() {
    let args = env.args()
    let mode = args[1].as_str()
    if mode == "write" {
        let with_backup = sample(true)
        match toml.to_string(with_backup) {
            Result.Ok(text) => {
                print(text.as_str())
                reread(text.as_str())
            }
            Result.Err(e) => println(e.to_string()),
        }
        match toml.to_string(sample(false)) {
            Result.Ok(text) => {
                print(text.as_str())
                reread(text.as_str())
            }
            Result.Err(e) => println(e.to_string()),
        }
        return
    }
    let text = read_to_string(args[2].as_str()).unwrap()
    if mode == "roundtrip" {
        match toml.parse(text.as_str()) {
            Result.Ok(document) => match toml.render(document) {
                Result.Ok(out) => print(out.as_str()),
                Result.Err(e) => println(e.to_string()),
            },
            Result.Err(e) => println(e.to_string()),
        }
    } else if mode == "spans" {
        match toml.parse(text.as_str()) {
            Result.Ok(document) => print_spans(document, "".to_string()),
            Result.Err(e) => println(e.to_string()),
        }
    } else {
        let read: Result<Config, DeError> = toml.from_str(text.as_str())
        match read {
            Result.Ok(config) => {
                println("name={} servers={} note={}", config.name, config.servers.len(), config.note)
                match config.protocol {
                    Protocol.Http => println("protocol=Http"),
                    Protocol.Https(port) => println("protocol=Https {port}"),
                    Protocol.Both(first, second) => println("protocol=Both {first} {second}"),
                }
                match config.backup {
                    Option.Some(server) => println("backup={} {}", server.host, server.port),
                    Option.None => println("backup=none"),
                }
                println("weights={}", config.weights.len())
                match config.weights.get("a".to_string()) {
                    Option.Some(weight) => println("a={weight}"),
                    Option.None => println("a=none"),
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
            Result.Err(e) => println(e.to_string()),
        }
    }
}
"""

MODES = {
    "roundtrip": "roundtrip",
    "bad": "roundtrip",
    "spans": "spans",
    "config": "config",
}


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


manifest_text = harness.read_text(PACKAGES / "toml" / "burdin.toml")
check(re.search(r'^name = "toml"$', manifest_text, re.M) is not None, "packages/toml does not declare `name = \"toml\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/toml has no version")
check((PACKAGES / "toml" / "src" / "toml.bd").is_file(), "packages/toml has no src/toml.bd")
readme = PACKAGES / "toml" / "README.md"
check(readme.is_file() and "use toml" in harness.read_text(readme), "packages/toml/README.md is missing or does not show `use toml`")
check((harness.ROOT / "spec" / "toml.md").is_file(), "spec/toml.md is missing")

with harness.temporary_dir("burdin-toml-") as work:
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
    check(built.returncode == 0, f"the toml package did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "tomlcheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    def run_mode(mode: str, fixture: Path = None):
        arguments = [binary, mode] if fixture is None else [binary, mode, str(fixture)]
        return harness.run(arguments, cwd=app, env=env)

    if binary.is_file():
        for folder, mode in MODES.items():
            fixtures = sorted((FIXTURES / folder).glob("*.toml"))
            check(len(fixtures) >= 3, f"tests/toml/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{fixture.relative_to(harness.TESTS)} has no .expected file")
                if not expected.is_file():
                    continue
                result = run_mode(mode, fixture)
                actual = harness.normalize(result.stdout)
                wanted = harness.normalize(harness.read_bytes(expected))
                name = f"{folder}/{fixture.name}"
                check(result.returncode == 0, f"{name} ended with status {result.returncode}: {err(result)[:300]}")
                check(actual == wanted, f"{name} printed {actual!r}, expected {wanted!r}")

                # Every valid document reads the same with CRLF line endings (TOML2).
                if folder in ("roundtrip", "spans"):
                    crlf = work / "crlf" / fixture.name
                    crlf.parent.mkdir(parents=True, exist_ok=True)
                    crlf.write_bytes(harness.read_bytes(fixture).replace(b"\n", b"\r\n"))
                    crlf_result = run_mode(mode, crlf)
                    check(
                        harness.normalize(crlf_result.stdout) == wanted,
                        f"{name} with CRLF line endings printed {harness.normalize(crlf_result.stdout)!r}",
                    )

        write_expected = FIXTURES / "write" / "sample.expected"
        check(write_expected.is_file(), "tests/toml/write/sample.expected is missing")
        written = run_mode("write")
        check(written.returncode == 0, f"write ended with status {written.returncode}: {err(written)[:300]}")
        if write_expected.is_file():
            actual = harness.normalize(written.stdout)
            wanted = harness.normalize(harness.read_bytes(write_expected))
            check(actual == wanted, f"write printed {actual!r}, expected {wanted!r}")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "toml")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the toml package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "toml" / "src" / "toml.bd", harness.read_text(official / "toml" / "src" / "toml.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed toml package (status {changed.returncode})")
    check(
        "`toml` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "toml")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the toml package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
