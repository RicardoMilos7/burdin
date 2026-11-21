#!/usr/bin/env python3
"""Checks the `regex` official package (stage 3, issue #264; spec/regex.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). BURDIN_PACKAGES points at a copy of this
repository's `packages/` folder, so nothing touches an installed compiler.

What it checks:
  * every row of `tests/regex/cases.tsv` (the goldens: one row per RX rule, the error messages, the flags, the
    classes, the iteration, the replacement templates, splitting, escaping, and the non-ASCII cases) against the
    `expected` column, after the runner prints it;
  * a differential run against Python's `re` (the oracle): seeded random patterns over a subset that both engines
    define the same way (no anchors, no backreferences, no quantified groups), with random ASCII texts, for every
    operation the runner exposes. A disagreement is reported with its row;
  * the whole golden file runs within a fixed time budget (RX1: the linear-time cases finish);
  * `burdin add regex` writes `regex = true`, prints the official-package line, and pins the package in
    `burdin.lock` with `source = "official"` and the PKG7 checksum of its files;
  * `--locked` accepts the pin, rejects a changed package by name, and a plain build re-pins it;
  * the package folder has its README and declares its name and version.
"""

import hashlib
import os
import random
import re
import shutil
import subprocess
import sys
import time
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

FIXTURES = harness.TESTS / "regex"
PACKAGES = harness.ROOT / "packages"
CASES = FIXTURES / "cases.tsv"
TIME_BUDGET = 120.0
MANIFEST = '[package]\nname = "regexcheck"\nversion = "0.1.0"\n\n[dependencies]\nregex = true\n'
SEED = 264
PATTERNS = 300
TEXTS = 5


def percent_decode(text: str) -> str:
    return re.sub(r"%([0-9A-Fa-f]{2})", lambda match: chr(int(match.group(1), 16)), text)


def percent_encode(text: str) -> str:
    return text.replace("%", "%25").replace("\t", "%09").replace("\n", "%0A").replace("\r", "%0D")


def show(text: str) -> str:
    """A text as the runner prints it: `<>` when empty, otherwise with the separators percent-encoded (RX14)."""
    if text == "":
        return "<>"
    return (
        text.replace("%", "%25")
        .replace("<", "%3C")
        .replace(" ", "%20")
        .replace("\t", "%09")
        .replace("\n", "%0A")
        .replace("\r", "%0D")
    )


def span_text(match, group: int = 0) -> str:
    return f"{match.start(group)}:{match.end(group)}:{show(match.group(group))}"


def groups_text(match) -> str:
    parts = []
    for group in range(0, match.re.groups + 1):
        parts.append("-" if match.start(group) < 0 else span_text(match, group))
    return "[" + " ".join(parts) + "]"


def pieces(compiled, text: str, limit=None):
    """The pieces between successive matches (RX12): at most `limit` of them when a limit is given."""
    out = []
    previous = 0
    splits = 0
    for match in compiled.finditer(text):
        if limit is not None and splits >= limit - 1:
            break
        out.append(text[previous:match.start()])
        previous = match.end()
        splits += 1
    out.append(text[previous:])
    return out


def oracle(op: str, pattern: str, text: str, arg: str) -> str:
    """The line the runner prints for a row whose operation Python's `re` defines the same way."""
    compiled = re.compile(pattern)
    if op == "as_str":
        return show(pattern)
    if op == "match":
        return "true" if compiled.search(text) else "false"
    if op == "groups":
        return str(compiled.groups)
    if op == "index":
        return str(compiled.groupindex[arg]) if arg in compiled.groupindex else "none"
    if op == "name":
        if arg not in compiled.groupindex:
            return "none"
        match = compiled.search(text)
        if match is None or match.start(compiled.groupindex[arg]) < 0:
            return "none"
        return span_text(match, compiled.groupindex[arg])
    if op == "find":
        match = compiled.search(text)
        return "none" if match is None else span_text(match)
    if op == "find_all":
        return "[" + " ".join(span_text(match) for match in compiled.finditer(text)) + "]"
    if op == "captures":
        match = compiled.search(text)
        return "none" if match is None else groups_text(match)
    if op == "captures_all":
        return "[" + " ".join(groups_text(match) for match in compiled.finditer(text)) + "]"
    if op == "replace":
        return show(compiled.sub(lambda match: arg, text, count=1))
    if op == "replacen":
        limit, template = arg.split(";", 1)
        if int(limit) == 0:
            return show(text)
        return show(compiled.sub(lambda match: template, text, count=int(limit)))
    if op == "replace_all":
        return show(compiled.sub(lambda match: arg, text))
    if op == "split":
        return "[" + " ".join(show(piece) for piece in pieces(compiled, text)) + "]"
    if op == "splitn":
        limit = int(arg)
        if limit == 0:
            return "[]"
        return "[" + " ".join(show(piece) for piece in pieces(compiled, text, limit)) + "]"
    raise ValueError(f"no oracle for {op}")


def random_pattern(rng: random.Random):
    """A pattern in the subset both engines define the same way, and the names of its groups."""
    atoms = ["a", "b", "c", "1", " ", ".", "\\d", "\\w", "\\s", "[ab]", "[^a]", "[a-c1]", "\\.", "\\-"]
    quantifiers = ["", "", "", "*", "+", "?", "{1,2}", "{0,2}", "*?", "+?", "??", "{2}"]
    names = []

    def concat(depth: int) -> str:
        parts = []
        for _ in range(rng.randint(1, 4)):
            if depth < 2 and rng.random() < 0.25:
                kind = rng.random()
                if kind < 0.4:
                    opening = "("
                elif kind < 0.6:
                    opening = "(?:"
                else:
                    name = f"g{len(names) + 1}"
                    names.append(name)
                    opening = f"(?P<{name}>"
                parts.append(opening + alternation(depth + 1) + ")")
            else:
                parts.append(rng.choice(atoms) + rng.choice(quantifiers))
        return "".join(parts)

    def alternation(depth: int) -> str:
        branches = []
        for _ in range(1 if rng.random() < 0.7 else rng.randint(2, 3)):
            branches.append("" if rng.random() < 0.1 else concat(depth))
        return "|".join(branches)

    flags = "(?i)" if rng.random() < 0.15 else ""
    return flags + alternation(0), names


def random_text(rng: random.Random) -> str:
    return "".join(rng.choice("abc1 -\t\n") for _ in range(rng.randint(0, 10)))


def differential_rows():
    """(row, expected line or a validator) pairs for the seeded differential run."""
    rng = random.Random(SEED)
    rows = []
    for _ in range(PATTERNS):
        pattern, names = random_pattern(rng)
        arguments = ["g1", "g2", "zz"] + names
        for _ in range(TEXTS):
            text = random_text(rng)
            name = rng.choice(arguments)
            limit = rng.randint(0, 3)
            template = rng.choice(["xy", "-", "", "x-y"])
            cases = [
                ("match", "", None),
                ("find", "", None),
                ("find_all", "", None),
                ("captures", "", None),
                ("captures_all", "", None),
                ("groups", "", None),
                ("name", name, None),
                ("index", name, None),
                ("replace", template, None),
                ("replacen", f"{limit};{template}", None),
                ("replace_all", template, None),
                ("split", "", None),
                ("splitn", str(limit), None),
                ("as_str", "", None),
            ]
            for op, arg, _ in cases:
                expected = oracle(op, pattern, text, arg)
                rows.append(((op, pattern, text, arg), expected))
            rows.append((("escape", "", text, ""), None))
    return rows


def escape_matches(line: str, text: str) -> bool:
    """`Regex.escape(text)` is a pattern that matches exactly `text` (RX13), checked with Python's `re`."""
    pattern = "" if line == "<>" else percent_decode(line)
    try:
        return re.fullmatch(pattern, text) is not None
    except re.error:
        return False


def encode_row(row) -> str:
    return "\t".join(percent_encode(field) for field in row)


def read_cases():
    """The golden rows: (fields, expected line), with the expected column decoded."""
    rows = []
    for line in CASES.read_text(encoding="utf-8").split("\n"):
        if line == "" or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 5:
            problems.append(f"cases.tsv row has {len(fields)} fields: {line[:80]!r}")
            continue
        rows.append((fields[:4], percent_decode(fields[4])))
    return rows


def run_runner(binary: Path, input_path: Path):
    """The runner's output lines for a TSV file, or None when it did not finish."""
    try:
        result = harness.run([binary, input_path], timeout=TIME_BUDGET)
    except subprocess.TimeoutExpired:
        return None, f"no result within {TIME_BUDGET:.0f}s"
    if result.returncode != 0:
        return None, harness.text(harness.normalize(result.stderr))
    return harness.text(harness.normalize(result.stdout)).split("\n")[:-1], ""


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


# The package itself.
manifest_text = harness.read_text(PACKAGES / "regex" / "burdin.toml")
check(re.search(r'^name = "regex"$', manifest_text, re.M) is not None, "packages/regex does not declare `name = \"regex\"`")
check(re.search(r'^version = "\d+\.\d+\.\d+"$', manifest_text, re.M) is not None, "packages/regex has no version")
check((PACKAGES / "regex" / "src" / "regex.bd").is_file(), "packages/regex has no src/regex.bd")
readme = PACKAGES / "regex" / "README.md"
check(readme.is_file() and "regex = true" in harness.read_text(readme), "packages/regex/README.md is missing or does not say how to depend on regex")
for source in sorted((PACKAGES / "regex" / "src").glob("*.bd")):
    check('println("dbg' not in harness.read_text(source), f"{source.name} still prints debugging output")

with harness.temporary_dir("burdin-regex-") as work:
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

    # The runner, built against the copy of the package.
    runner = work / "runner"
    shutil.copytree(FIXTURES / "runner", runner)
    built = run_in(runner, "build", ".")
    check(built.returncode == 0, f"the regex runner did not build: {err(built)[:600]}")
    binary = harness.executable_in(runner, "regex_runner")

    if binary.is_file():
        # Goldens: every row of cases.tsv prints the line its expected column gives.
        golden = read_cases()
        check(len(golden) >= 150, f"cases.tsv has too few rows ({len(golden)})")
        started = time.monotonic()
        lines, failure = run_runner(binary, CASES)
        elapsed = time.monotonic() - started
        check(lines is not None, f"the runner failed on cases.tsv: {failure[:300]}")
        check(elapsed < TIME_BUDGET, f"the golden rows took {elapsed:.1f}s (budget {TIME_BUDGET:.0f}s)")
        if lines is not None:
            check(len(lines) == len(golden), f"the runner printed {len(lines)} lines for {len(golden)} rows")
            for index, (fields, expected) in enumerate(golden):
                if index >= len(lines):
                    break
                check(
                    lines[index] == expected,
                    f"row {index + 1} ({fields[0]} {fields[1]!r} on {fields[2]!r} arg {fields[3]!r}) printed {lines[index]!r}, expected {expected!r}",
                )

        # Differential: the same rows through Python's `re` and the package.
        rows = differential_rows()
        input_path = work / "differential.tsv"
        input_path.write_text("\n".join(encode_row(row) for row, _ in rows) + "\n", encoding="utf-8")
        lines, failure = run_runner(binary, input_path)
        check(lines is not None, f"the runner failed on the differential rows: {failure[:300]}")
        if lines is not None:
            check(len(lines) == len(rows), f"the runner printed {len(lines)} lines for {len(rows)} differential rows")
            disagreements = 0
            for index, ((op, pattern, text, arg), expected) in enumerate(rows):
                if index >= len(lines):
                    break
                if expected is None:
                    agreed = escape_matches(lines[index], text)
                else:
                    agreed = lines[index] == expected
                if not agreed:
                    disagreements += 1
                    if disagreements <= 10:
                        problems.append(
                            f"differential row {index + 1}: {op} {pattern!r} on {text!r} arg {arg!r}: "
                            f"package printed {lines[index]!r}, Python re gives {expected!r}"
                        )
            check(disagreements == 0, f"{disagreements} differential rows disagree with Python's re")
        check(len(rows) >= 10000, f"the differential run has too few rows ({len(rows)})")

    # The lockfile pins the package's files with the official source (PKG15, PKG17).
    app = work / "app"
    write(app / "burdin.toml", MANIFEST)
    write(app / "src" / "main.bd", "use regex.{Regex}\n\nfn main() {\n    println(\"{}\", Regex.new(\"a(b+)c\").unwrap().is_match(\"xabbc\"))\n}\n")
    built_app = run_in(app, "build", ".")
    check(built_app.returncode == 0, f"the regex package did not build in an app: {err(built_app)[:600]}")
    lock = harness.read_text(app / "burdin.lock")
    check('source = "official"' in lock, "burdin.lock does not record the official source")
    checksum = tree_checksum(official / "regex")
    check(f'checksum = "{checksum}"' in lock, "burdin.lock does not pin the regex package's files")
    locked = run_in(app, "build", ".", "--locked")
    check(locked.returncode == 0, f"`--locked` rejected the fresh lockfile: {err(locked)[:300]}")

    # `burdin add regex` in a package that does not declare it yet.
    adder = work / "adder"
    write(adder / "burdin.toml", '[package]\nname = "adder"\nversion = "0.1.0"\n')
    write(adder / "src" / "main.bd", "use regex\n\nfn main() {\n    println(\"ok\")\n}\n")
    added = run_in(adder, "add", "regex")
    check(added.returncode == 0, f"`burdin add regex` failed: {err(added)[:300]}")
    check(out(added) == "Added dependency `regex` (official)\n", f"`burdin add regex` printed {out(added)!r}")
    check(
        harness.read_text(adder / "burdin.toml") == '[package]\nname = "adder"\nversion = "0.1.0"\n\n[dependencies]\nregex = true\n',
        f"`burdin add regex` wrote {harness.read_text(adder / 'burdin.toml')!r}",
    )
    adder_built = run_in(adder, "build", ".")
    check(adder_built.returncode == 0, f"the package `burdin add regex` edited did not build: {err(adder_built)[:300]}")
    check('source = "official"' in harness.read_text(adder / "burdin.lock"), "`burdin add regex` wrote no official pin")

    # A changed package is a difference the lockfile reports by name, and a plain build re-pins it (PKG17).
    before = snapshot(app)
    source_path = official / "regex" / "src" / "regex.bd"
    write(source_path, harness.read_text(source_path) + "\n// changed\n")
    changed = run_in(app, "build", ".", "--locked")
    check(changed.returncode == 1, f"`--locked` accepted a changed regex package (status {changed.returncode})")
    check(
        "`regex` is an official package whose files are not the ones burdin.lock pinned" in err(changed),
        f"`--locked` did not name the changed package: {err(changed)[:400]}",
    )
    check(snapshot(app) == before, "`--locked` changed burdin.toml or burdin.lock")
    repinned = run_in(app, "build", ".")
    check(repinned.returncode == 0, f"a plain build did not re-pin the changed package: {err(repinned)[:300]}")
    check(f'checksum = "{tree_checksum(official / "regex")}"' in harness.read_text(app / "burdin.lock"), "the re-pin kept the old checksum")
    locked_again = run_in(app, "build", ".", "--locked")
    check(locked_again.returncode == 0, f"`--locked` rejected the re-pinned lockfile: {err(locked_again)[:300]}")

print(f"Checked the regex package ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
