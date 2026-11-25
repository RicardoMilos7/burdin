#!/usr/bin/env python3
"""Checks the `csv` official package (stage 3, issue #257; spec/serialization.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every fixture under `tests/csv` (round trips, quoting, embedded line breaks, CRLF and lone-CR line endings,
    other delimiters, files with no header row, and malformed input) against its `.expected` golden: the records
    written one at a time as they are read, the decoded fields, the raw fields, and the error message with its
    line, column, and path;
  * a malformed fixture never aborts the process: the exit status is 0 and the error is reported as text;
  * `use csv` loads the package with no manifest entry, and `burdin.lock` pins it
    with `source = "official"` and the PKG7 checksum of its files;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it.

The line endings of a fixture are not left to the checkout: the folders named `crlf*` and `cr*` are converted by this
script (every `\\n` becomes `\\r\\n`, or `\\r`) before the program reads them, so the check does not depend on what
Git does to line endings.
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

FIXTURES = harness.TESTS / "csv"
PACKAGES = harness.ROOT / "packages"
MANIFEST = '[package]\nname = "csvcheck"\nversion = "0.1.0"\n'

# The program the fixtures drive. It reads a fixture's path from the command line (so a non-ASCII fixture never
# depends on how a platform spells an argument). In `rows` mode it prints the raw fields of every record. In the other
# modes it reads `Person` records one at a time, writes each one back as it is read (printing what the writer holds
# after every record), and then reports the decoded fields and whether writing the whole list at once gives the same
# text as writing the records one by one.
PROGRAM = r"""
use csv
use csv.{Reader, Writer}

enum Role: Serialize, Deserialize, Debug {
    Admin
    Member
    Guest
}

struct Person: Serialize, Deserialize, Debug {
    name: String
    age: u8
    email: Option<String>
    score: f64
    active: bool
    role: Role
}

fn escape(text: str, all: bool): String {
    let mut out = text.replace("\r", "\\r")
    if all {
        out = out.replace("\n", "\\n")
    }
    out
}

fn columns(): List<String> {
    let mut names = List<String>.new()
    names.push("name".to_string())
    names.push("age".to_string())
    names.push("email".to_string())
    names.push("score".to_string())
    names.push("active".to_string())
    names.push("role".to_string())
    names
}

fn rows(text: str) {
    let mut reader = match Reader.with_options(text, ',', false) {
        Result.Ok(reader) => reader,
        Result.Err(e) => {
            println("error: {}", e.to_string())
            return
        }
    }
    let mut count: usize = 0
    loop {
        match reader.read_row() {
            Result.Ok(Option.Some(fields)) => {
                count = count + 1
                println("row {} ({} fields): [{}]", count, fields.len(), escape(fields.join("|").as_str(), true))
            }
            Result.Ok(Option.None) => break,
            Result.Err(e) => {
                println("error: {}", e.to_string())
                break
            }
        }
    }
    println("rows={}", count)
}

fn typed(mode: str, text: str) {
    let semi = mode == "semi"
    let headers = mode != "noheader"
    let delimiter = if semi { ';' } else { ',' }
    let mut reader = match Reader.with_options(text, delimiter, headers) {
        Result.Ok(reader) => reader,
        Result.Err(e) => {
            println("error: {}", e.to_string())
            return
        }
    }
    if !headers {
        reader.set_headers(columns())
    }
    let mut writer = match Writer.with_options(delimiter, headers) {
        Result.Ok(writer) => writer,
        Result.Err(e) => {
            println("error: {}", e.to_string())
            return
        }
    }
    if mode == "crlf" {
        writer.use_crlf()
    }
    let mut all = List<Person>.new()
    let mut streamed = String.new()
    loop {
        let next: Result<Option<Person>, DeError> = reader.read()
        match next {
            Result.Ok(Option.Some(person)) => {
                match writer.write(person) {
                    Result.Ok(_) => {}
                    Result.Err(e) => {
                        println("error: {}", e.to_string())
                        return
                    }
                }
                let chunk = writer.take()
                print("{}", escape(chunk.as_str(), false))
                streamed.push_str(chunk.as_str())
                all.push(person)
            }
            Result.Ok(Option.None) => break,
            Result.Err(e) => {
                println("error: {}", e.to_string())
                break
            }
        }
    }
    println("records={}", all.len())
    if mode != "crlf" {
        match csv.to_string_with(all, delimiter, headers) {
            Result.Ok(whole) => println("same={}", whole == streamed),
            Result.Err(e) => println("error: {}", e.to_string()),
        }
    }
    for person in all {
        let email = match person.email {
            Option.Some(address) => address.clone(),
            Option.None => "<none>".to_string(),
        }
        let role = match person.role {
            Role.Admin => "Admin",
            Role.Member => "Member",
            Role.Guest => "Guest",
        }
        println("{} | {} | {} | {} | {} | {}", escape(person.name.as_str(), true), person.age, escape(email.as_str(), true), person.score, person.active, role)
    }
}

fn main() {
    let args = env.args()
    let mode = args[1]
    let text = read_to_string(args[2].as_str()).unwrap()
    if mode.as_str() == "rows" {
        rows(text.as_str())
    } else {
        typed(mode.as_str(), text.as_str())
    }
}
"""

# folder, program mode, and the line ending every `\n` of the fixture becomes first (None leaves the file alone).
MODES = (
    ("roundtrip", "typed", None),
    ("semicolon", "semi", None),
    ("noheader", "noheader", None),
    ("crlf", "crlf", b"\r\n"),
    ("raw", "rows", None),
    ("crlf_raw", "rows", b"\r\n"),
    ("cr_raw", "rows", b"\r"),
    ("bad", "typed", None),
    ("bad_raw", "rows", None),
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


manifest_text = harness.read_text(PACKAGES / "csv" / "burdin.toml")
check(re.search(r'^name = "csv"$', manifest_text, re.M) is not None, "packages/csv does not declare `name = \"csv\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/csv has no version")
check((PACKAGES / "csv" / "src" / "csv.bd").is_file(), "packages/csv has no src/csv.bd")
readme = PACKAGES / "csv" / "README.md"
check(readme.is_file() and "use csv" in harness.read_text(readme), "packages/csv/README.md is missing or does not show `use csv`")

with harness.temporary_dir("burdin-csv-") as work:
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
    write(app / "src" / "main.bd", PROGRAM.lstrip("\n"))
    built = run_in(app, "build", ".")
    check(built.returncode == 0, f"the csv package did not build: {err(built)[:600]}")
    binary = harness.executable_in(app, "csvcheck")
    check(binary.is_file(), "`burdin build .` did not produce the program")

    if binary.is_file():
        converted = work / "converted"
        for folder, mode, ending in MODES:
            fixtures = sorted((FIXTURES / folder).glob("*.csv"))
            check(len(fixtures) >= 2, f"tests/csv/{folder} has too few fixtures ({len(fixtures)})")
            for fixture in fixtures:
                expected = fixture.with_suffix(".expected")
                check(expected.is_file(), f"{folder}/{fixture.name} has no .expected file")
                if not expected.is_file():
                    continue
                source = fixture
                if ending is not None:
                    source = converted / folder / fixture.name
                    source.parent.mkdir(parents=True, exist_ok=True)
                    source.write_bytes(fixture.read_bytes().replace(b"\r", b"").replace(b"\n", ending))
                result = harness.run([binary, mode, str(source)], cwd=app, env=env)
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
    checksum = tree_checksum(official / "csv")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the csv package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    write(official / "csv" / "src" / "csv.bd", harness.read_text(official / "csv" / "src" / "csv.bd") + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed csv package (status {changed.returncode})")
    check(
        "`csv` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "csv")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the csv package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
