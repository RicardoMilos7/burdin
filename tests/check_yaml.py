#!/usr/bin/env python3
"""Checks the `yaml` official package (stage 3, issue #258; spec/serialization.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every fixture under `tests/yaml` (round trips, scalar forms, anchors and aliases, nested collections, and
    malformed input) against its `.expected` golden: the re-emitted document, the decoded fields, and the error
    message with its line, column, and path;
  * what the writer prints reads back and prints the same again (the program reports any difference itself);
  * a malformed fixture never aborts the process: the exit status is 0 and the error is reported as text, including
    an alias bomb and a document nested past the depth limit, and neither takes a noticeable time;
  * the same document with CRLF line breaks, and with a byte order mark, reads as the LF document does;
  * large documents (thousands of entries, in block and in single-line flow form) read in a reasonable time;
  * `use yaml` loads the package with no manifest entry, and `burdin.lock` pins it
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

FIXTURES = harness.TESTS / "yaml"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "yamlcheck"\nversion = "0.1.0"\n'

# The program the fixtures drive. It reads a mode and a fixture's path from the command line (so a non-ASCII fixture
# never depends on how a platform spells an argument). The `config` mode reads a `Config`, re-emits it, reads what it
# emitted, and reports the decoded fields; the `doc` mode does the same for a `Doc` that has tuples, a unit, a
# list of lists (written by hand, because the derives do not handle one yet), an `Option`, and the float cases.
PROGRAM = """\
use yaml

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

struct Row: Serialize, Deserialize, Debug {
    id: u32
    tags: List<String>
}

// The derives do not yet handle a list of lists, so this one is written by hand.
struct Grid: Debug {
    rows: List<List<i32>>
}

Grid: Serialize {
    fn serialize<S: Serializer>(self, mut s: S): Result<(), DeError> {
        s.begin_struct("Grid", 1)?
        s.struct_field("rows")?
        s.begin_seq(self.rows.len())?
        for row in self.rows {
            s.begin_seq(row.len())?
            for cell in row {
                serialize_i32(mut s, cell)?
            }
            s.end_seq()?
        }
        s.end_seq()?
        s.end_struct()
    }
}

Grid: Deserialize {
    fn deserialize<D: Deserializer>(mut d: D): Result<Grid, DeError> {
        d.begin_struct("Grid")?
        if !d.has_next_field()? {
            return Err(missing_field("rows"))
        }
        let name = d.field_name()?
        if name.as_str() != "rows" {
            let failure = unknown_field(name.as_str())
            return Err(failure.field(name.as_str()))
        }
        d.begin_seq()?
        let mut rows = List<List<i32>>.new()
        loop {
            let more = d.has_next_seq_element()?
            if !more {
                break
            }
            d.begin_seq()?
            let mut row = List<i32>.new()
            loop {
                let more_cell = d.has_next_seq_element()?
                if !more_cell {
                    break
                }
                row.push(deserialize_i32(mut d)?)
            }
            d.end_seq()?
            rows.push(row)
        }
        d.end_seq()?
        d.end_struct()?
        Ok(Grid { rows })
    }
}

struct Doc: Serialize, Deserialize, Debug {
    grid: Grid
    rows: List<Row>
    pair: (i32, String)
    flag: Option<bool>
    ratio: f32
    big: u64
    small: i8
    raw: List<u8>
    unit: ()
    inf: f64
    nan: f64
}

fn main() {
    let args = env.args()
    let mode = args[1]
    let text = read_to_string(args[2].as_str()).unwrap()
    if mode.as_str() == "doc" {
        run_doc(text.as_str())
    } else {
        run_config(text.as_str())
    }
}

fn run_doc(text: str) {
    let read: Result<Doc, DeError> = yaml.from_str(text)
    match read {
        Ok(doc) => {
            print(yaml.to_string(doc).unwrap())
            let back: Result<Doc, DeError> = yaml.from_str(yaml.to_string(doc).unwrap().as_str())
            match back {
                Ok(again) => println("grid={} rows={} raw={} pair={} {}", again.grid.rows.len(), again.rows.len(), again.raw.len(), again.pair.0, again.pair.1),
                Err(e) => println("round trip failed: {}", e.to_string()),
            }
        }
        Err(e) => println(e.to_string()),
    }
}

fn run_config(text: str) {
    let read: Result<Config, DeError> = yaml.from_str(text)
    match read {
        Ok(config) => {
            print(yaml.to_string(config).unwrap())
        }
        Err(e) => println(e.to_string()),
    }

    let again: Result<Config, DeError> = yaml.from_str(text)
    match again {
        Ok(config) => {
            println("name={:?} servers={} note={}", config.name, config.servers.len(), config.note)
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

# Each fixture folder, and the mode of the program that reads it.
FOLDERS = (
    ("roundtrip", "config"),
    ("scalars", "config"),
    ("anchors", "config"),
    ("nested", "doc"),
    ("bad", "config"),
)

# The documents that are also read with CRLF line breaks and with a byte order mark: (folder, file, mode).
VARIANTS = (
    ("roundtrip", "full.yaml", "config"),
    ("scalars", "literal.yaml", "config"),
    ("scalars", "folded.yaml", "config"),
    ("scalars", "double_folded.yaml", "config"),
    ("nested", "comments.yaml", "doc"),
)


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


manifest_text = harness.read_text(PACKAGES / "yaml" / "burdin.toml")
check(re.search(r'^name = "yaml"$', manifest_text, re.M) is not None, "packages/yaml does not declare `name = \"yaml\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/yaml has no version")
check((PACKAGES / "yaml" / "src" / "yaml.bd").is_file(), "packages/yaml has no src/yaml.bd")
readme = PACKAGES / "yaml" / "README.md"
check(readme.is_file() and "use yaml" in harness.read_text(readme), "packages/yaml/README.md is missing or does not show `use yaml`")

with harness.temporary_dir("burdin-yaml-") as work:
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
    check(built.returncode == 0, f"the yaml package did not build: {err(built)[:400]}")
    binary = harness.executable_in(app, "yamlcheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    def run_program(mode, path):
        return harness.run([binary, mode, str(path)], cwd=app, env=env, timeout=120)

    if binary.is_file():
        for folder, mode in FOLDERS:
            fixtures = sorted((FIXTURES / folder).glob("*.yaml"))
            check(len(fixtures) >= 5, f"tests/yaml/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{folder}/{fixture.name} has no .expected file")
                if not expected.is_file():
                    continue
                result = run_program(mode, fixture)
                actual = harness.normalize(result.stdout)
                wanted = harness.normalize(harness.read_bytes(expected))
                check(result.returncode == 0, f"{folder}/{fixture.name} ended with status {result.returncode}: {err(result)[:300]}")
                check(
                    actual == wanted,
                    f"{folder}/{fixture.name} printed {actual!r}, expected {wanted!r}",
                )
                check(b"round trip" not in actual, f"{folder}/{fixture.name}: what the writer printed did not read back the same")

        # The same document with CRLF line breaks, and with a byte order mark, reads as the LF document does.
        for folder, name, mode in VARIANTS:
            source = FIXTURES / folder / name
            wanted = harness.normalize(harness.read_bytes(source.with_suffix(".expected")))
            raw = harness.read_bytes(source).replace(b"\r", b"")
            for label, data in (("crlf", raw.replace(b"\n", b"\r\n")), ("bom", b"\xef\xbb\xbf" + raw)):
                variant = work / f"{label}-{name}"
                variant.write_bytes(data)
                result = run_program(mode, variant)
                check(result.returncode == 0, f"{label} {folder}/{name} ended with status {result.returncode}: {err(result)[:300]}")
                check(
                    harness.normalize(result.stdout) == wanted,
                    f"{label} {folder}/{name} printed {harness.normalize(result.stdout)!r}, expected {wanted!r}",
                )

        # Large documents: thousands of entries in block form and in single-line flow form.
        count = 3000
        tail = "protocol: Http\nbackup: null\nnote: z\n"
        block = "name: big\nservers:\n"
        for i in range(count):
            block += f"  - host: h{i}\n    port: {i % 60000}\n    tls: true\n    ratio: 1.5\n"
        block += "weights:\n" + "".join(f"  w{i}: {i}\n" for i in range(count)) + tail
        flow = (
            "{name: big, servers: ["
            + ", ".join(f"{{host: h{i}, port: {i % 60000}, tls: true, ratio: 1.5}}" for i in range(count))
            + "], weights: {"
            + ", ".join(f"w{i}: {i}" for i in range(count))
            + "}, protocol: Http, backup: null, note: z}\n"
        )
        for label, text in (("block", block), ("flow", flow)):
            path = work / f"big-{label}.yaml"
            harness.write_text(path, text)
            result = run_program("config", path)
            lines = harness.text(harness.normalize(result.stdout)).splitlines()
            check(result.returncode == 0, f"the big {label} document ended with status {result.returncode}: {err(result)[:300]}")
            check(f'name="big" servers={count} note=z' in lines, f"the big {label} document read as {lines[-6:]!r}")
            check(f"weights={count}" in lines, f"the big {label} document lost map entries: {lines[-6:]!r}")
            check("first=h0 0 true 1.5" in lines, f"the big {label} document read its first server as {lines[-6:]!r}")
            check("  w0: 0" in lines and f"  w{count - 1}: {count - 1}" in lines, f"the big {label} document was not written back")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "yaml")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the yaml package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "yaml" / "src" / "yaml.bd", harness.read_text(official / "yaml" / "src" / "yaml.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed yaml package (status {changed.returncode})")
    check(
        "`yaml` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "yaml")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the yaml package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
