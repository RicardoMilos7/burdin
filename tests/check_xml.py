#!/usr/bin/env python3
"""Checks the `xml` official package (stage 3, issue #259; spec/serialization.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every fixture under `tests/xml` against its `.expected` golden: the serialization mapping (`roundtrip`, `pretty`,
    and `bad`: the compact form, the decoded fields, and the error message with its line, column, and path), and
    the DOM (`dom`: the compact and pretty forms and the walked tree, with namespaces resolved; `dombad`: the
    message, line, and column of every rejection, including every DOCTYPE, external entity, and entity bomb);
  * a malformed fixture never aborts the process: the exit status is 0 and the error is reported as text, and every
    prefix of a document, and the document with any one character removed, parses to a value or an error;
  * `use xml` loads the package with no manifest entry, and `burdin.lock` pins it
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

FIXTURES = harness.TESTS / "xml"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "xmlcheck"\nversion = "0.1.0"\n'

# The program the fixtures drive. It reads a fixture's path from the command line (so a non-ASCII fixture never
# depends on how a platform spells an argument). The first mode set drives the serialization mapping with a
# configuration type, the second drives the DOM.
PROGRAM = """\
use xml
use xml.{Element, Node}

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

// One line per element, then its attributes and children, indented by depth: every node kind is shown by its
// kind, so a CDATA section is told apart from text and a namespace is the one the parser resolved.
fn dump(element: Element, depth: usize) {
    let pad = "  ".repeat(depth)
    println("{}{} ns=[{}] at {}:{}", pad, element.name, element.namespace, element.line, element.column)
    let mut a: usize = 0
    while a < element.attributes.len() {
        let attribute = element.attributes[a].clone()
        println("{}  @{}={:?} ns=[{}]", pad, attribute.name, attribute.value, attribute.namespace)
        a = a + 1
    }
    let mut i: usize = 0
    while i < element.children.len() {
        match element.children[i] {
            Node.Element(child) => dump((*child).copy(), depth + 1),
            Node.Text(text) => println("{}  text {:?}", pad, text),
            Node.CData(text) => println("{}  cdata {:?}", pad, text),
            Node.Comment(text) => println("{}  comment {:?}", pad, text),
            Node.Pi(instruction) => println("{}  pi {} {:?}", pad, instruction.target, instruction.data),
        }
        i = i + 1
    }
}

fn parses(text: str): bool {
    match xml.parse(text) {
        Ok(_) => true,
        Err(_) => false,
    }
}

fn main() {
    let args = env.args()
    let mode = args[1]
    let text = read_to_string(args[2].as_str()).unwrap()

    if mode.as_str() == "dom" {
        match xml.parse(text.as_str()) {
            Ok(document) => {
                println("{}", document.to_xml().unwrap())
                print(document.to_xml_pretty().unwrap())
                match document.declaration {
                    Some(d) => println(
                        "declaration version={} encoding={:?} standalone={:?}",
                        d.version,
                        d.encoding,
                        d.standalone,
                    ),
                    None => println("declaration none"),
                }
                println("before={} after={}", document.before.len(), document.after.len())
                dump(document.root.copy(), 0)
            }
            Err(e) => println(e.to_string()),
        }
        return
    }

    if mode.as_str() == "dombad" {
        match xml.parse(text.as_str()) {
            Ok(document) => println("parsed {}", document.to_xml().unwrap()),
            Err(e) => println(e.to_string()),
        }
        return
    }

    if mode.as_str() == "truncate" {
        // Every prefix of the text, and the text with one character removed: each parses or reports, never aborts.
        let mut total: usize = 0
        let mut good: usize = 0
        let mut start: usize = 0
        while start <= text.len() {
            if text.is_char_boundary(start) {
                total = total + 1
                if parses(text[0..start]) {
                    good = good + 1
                }
                let mut end = start + 1
                while end <= text.len() && !text.is_char_boundary(end) {
                    end = end + 1
                }
                if end <= text.len() {
                    let mut cut = String.new()
                    cut.push_str(text[0..start])
                    cut.push_str(text[end..text.len()])
                    total = total + 1
                    if parses(cut.as_str()) {
                        good = good + 1
                    }
                }
            }
            start = start + 1
        }
        println("variants={} parsed={}", total, good)
        return
    }

    let read: Result<Config, DeError> = xml.from_str(text.as_str())
    match read {
        Ok(config) => {
            if mode.as_str() == "pretty" {
                print(xml.to_string_pretty(config).unwrap())
            } else {
                println(xml.to_string(config).unwrap())
            }
        }
        Err(e) => println(e.to_string()),
    }

    let again: Result<Config, DeError> = xml.from_str(text.as_str())
    match again {
        Ok(config) => {
            println("name={:?} servers={} note={:?}", config.name, config.servers.len(), config.note)
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

MODES = (
    ("roundtrip", "compact"),
    ("pretty", "pretty"),
    ("bad", "compact"),
    ("dom", "dom"),
    ("dombad", "dombad"),
    ("truncate", "truncate"),
)
MINIMUM = {"roundtrip": 6, "pretty": 3, "bad": 20, "dom": 8, "dombad": 30, "truncate": 2}


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


manifest_text = harness.read_text(PACKAGES / "xml" / "burdin.toml")
check(re.search(r'^name = "xml"$', manifest_text, re.M) is not None, "packages/xml does not declare `name = \"xml\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/xml has no version")
check((PACKAGES / "xml" / "src" / "xml.bd").is_file(), "packages/xml has no src/xml.bd")
readme = PACKAGES / "xml" / "README.md"
check(readme.is_file() and "use xml" in harness.read_text(readme), "packages/xml/README.md is missing or does not show `use xml`")

with harness.temporary_dir("burdin-xml-") as work:
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
    check(built.returncode == 0, f"the xml package did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "xmlcheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    if binary.is_file():
        for folder, mode in MODES:
            fixtures = sorted((FIXTURES / folder).glob("*.xml"))
            check(len(fixtures) >= MINIMUM[folder], f"tests/xml/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{folder}/{fixture.name} has no .expected file")
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
    checksum = tree_checksum(official / "xml")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the xml package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "xml" / "src" / "xml.bd", harness.read_text(official / "xml" / "src" / "xml.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed xml package (status {changed.returncode})")
    check(
        "`xml` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "xml")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the xml package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
