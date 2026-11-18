#!/usr/bin/env python3
"""Validates the structure of the conformance tests (directives and error codes).

It does not compile anything. Usage: tests/check_tests.py

This replaces the former check-tests.sh and check-tests.ps1 pair: one implementation for every platform.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

KIND_WORDS = (
    "run",
    "run-abort",
    "compile-fail",
    "test-compile-fail",
    "burdin-test",
    "bench-compile-fail",
    "burdin-bench",
    "undecided",
)
EXCEPTIONS_HEADING = "### Codes without a compile-fail test"
BURDIN_TEST_REGEX = re.compile(r"^burdin-test passed=[0-9]+ failed=[0-9]+( ignored=[0-9]+)?$")
MARKER_REGEX = re.compile(rb"//~ (E[0-9]{4})")


def read(path: Path) -> str:
    return path.read_bytes().replace(b"\r", b"").decode("utf-8", "replace")


def section(lines, start_marker: str, end_marker=None):
    """The `sed -n '/start/,/end/p'` range: both markers are included, like `sed`."""
    start = next((index for index, line in enumerate(lines) if line.startswith(start_marker)), None)
    if start is None:
        return None
    end = len(lines)
    if end_marker is not None:
        for index in range(start + 1, len(lines)):
            if lines[index].startswith(end_marker):
                end = index + 1
                break
    return lines[start:end]


def codes_in(lines) -> set:
    return {match.group(1) for line in lines for match in re.finditer(r"^\| (E[0-9]{4}) \|", line)}


def main() -> int:
    tests_dir = Path(__file__).resolve().parent
    readme_path = tests_dir / "README.md"
    if not readme_path.is_file():
        print(f"missing {readme_path}", file=sys.stderr)
        return 1
    readme = read(readme_path).split("\n")

    registry = section(readme, "## Error codes", EXCEPTIONS_HEADING) or []
    known = codes_in(registry)

    exceptions = section(readme, EXCEPTIONS_HEADING, "## ")
    if exceptions is None:
        exceptions = []
    documented = []
    for line in exceptions:
        match = re.match(r"^\| (E[0-9]{4}) \|([^|]*)\|", line)
        if match:
            documented.append((match.group(1), match.group(2).strip()))
    documented_codes = {code for code, _ in documented}

    problems = []
    counts = {kind: 0 for kind in KIND_WORDS}
    total = 0
    used = set()

    files = []
    for path in sorted(tests_dir.rglob("*.bd"), key=lambda p: p.as_posix()):
        if path.suffix != ".bd":
            continue  # GNU grep's --include is case-sensitive, even on Windows
        data = path.read_bytes()
        if any(line.startswith(b"//@ ") for line in data.replace(b"\r", b"").split(b"\n")):
            files.append(path)

    for path in files:
        total += 1
        rel = path.relative_to(tests_dir).as_posix()
        lines = path.read_bytes().replace(b"\r", b"").split(b"\n")
        directives = [line[4:] for line in lines if line.startswith(b"//@ ")]
        n_expected = sum(1 for line in lines if line.startswith(b"//> "))
        markers = []
        for number, line in enumerate(lines, 1):
            for match in MARKER_REGEX.finditer(line):
                markers.append((number, match.group(1).decode("ascii")))

        kinds = []
        for directive in directives:
            word = re.split(rb"[ :]", directive, maxsplit=1)[0].decode("ascii", "replace")
            if word in KIND_WORDS:
                kinds.append(word)
        if not kinds:
            problems.append(f"{rel}: no test kind directive")
            continue

        primary = "undecided" if "undecided" in kinds else kinds[0]
        counts[primary] += 1

        if primary != "undecided" and len(kinds) > 1:
            problems.append(f"{rel}: more than one test kind")
        if primary == "undecided":
            continue

        if primary == "run":
            if markers:
                problems.append(f"{rel}: run test has //~ markers")
        elif primary == "run-abort":
            if not any(directive.startswith(b"stderr:") for directive in directives):
                problems.append(f"{rel}: run-abort needs a //@ stderr: line")
        elif primary in ("compile-fail", "test-compile-fail"):
            if not markers:
                problems.append(f"{rel}: {primary} needs at least one //~ marker")
            if n_expected > 0:
                problems.append(f"{rel}: {primary} must not have //> lines")
        elif primary == "burdin-test":
            if not any(
                BURDIN_TEST_REGEX.match(directive.decode("ascii", "replace")) for directive in directives
            ):
                problems.append(f"{rel}: burdin-test needs passed=N failed=M [ignored=N]")

        for number, code in markers:
            used.add(code)
            if code not in known:
                problems.append(f"{rel}:{number}: unknown error code {code}")

    print(f"Checked {total} tests:")
    for kind in (
        "compile-fail",
        "test-compile-fail",
        "bench-compile-fail",
        "burdin-test",
        "burdin-bench",
        "run",
        "run-abort",
        "undecided",
    ):
        print(f"  {kind:<18} {counts[kind]}")

    # Every registered code needs a compile-fail marker (or a test-compile-fail one) or a documented reason. A code
    # with neither fails CI, so adding a diagnostic means adding a test (issue #445).
    if not any(EXCEPTIONS_HEADING in line for line in readme):
        problems.append(f'README.md: missing the "{EXCEPTIONS_HEADING}" section')
    for code in sorted(documented_codes):
        if code not in known:
            problems.append(f"README.md: documented exception {code} is not in the registry")
        elif code in used:
            problems.append(f"README.md: {code} has a compile-fail test; remove it from the documented exceptions")
    for code, reason in documented:
        if not reason:
            problems.append(f"README.md: the exception for {code} has no reason")
    for code in sorted(known):
        if code not in used and code not in documented_codes:
            problems.append(f"error code {code} has no compile-fail test; add one or document why it cannot have one")

    # std/src/int.bd is generated; it must match its generator (issue #704).
    sys.path.insert(0, str(tests_dir))
    import gen_int_std

    if gen_int_std.OUT.read_bytes().decode("utf-8").replace(chr(13), "") != gen_int_std.render():
        problems.append("std/src/int.bd is out of date: run python3 tests/gen_int_std.py")

    # std/src/atomic.bd is generated too (issue #217).
    import gen_atomic_std

    if gen_atomic_std.OUT.read_bytes().decode("utf-8").replace(chr(13), "") != gen_atomic_std.render():
        problems.append("std/src/atomic.bd is out of date: run python3 tests/gen_atomic_std.py")

    if problems:
        print()
        print("Problems:")
        for problem in problems:
            print(f"  {problem}")
        return 1
    print("No structural problems.")
    return 0


if __name__ == "__main__":
    # Keep the checker's output byte-identical on every platform; Python would otherwise write CRLF on Windows.
    sys.stdout.reconfigure(newline="\n")
    sys.exit(main())
