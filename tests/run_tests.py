#!/usr/bin/env python3
"""Conformance test runner for Burdin. The test format is described in tests/README.md.

Usage: tests/run_tests.py [options] [filter ...]
  -r, --release       use the release profile (tests marked debug-only are skipped)
  -v, --verbose       show diffs and compiler output for failures
  -l, --list          list the tests that would run, without running them
  -t, --timeout SEC   seconds allowed per test (default 30)
  -j, --jobs N        tests to run in parallel (default: the CPU count, at most 8; 1 is sequential)
  -h, --help          show this help
  filter              run only tests whose path contains one of these strings
                      (for example: m05, drop_order, m06_conventions/exclusivity)

Environment:
  BURDIN   path to the compiler driver (default: target/{debug,release}/burdin[.exe], then PATH)

--------------------------------------------------------------------------------
Driver contract assumed by this runner. If your driver differs, change only this section.
The full contract, including --emit-ir, --link-lib (and LLVM-C), and BURDIN_CC, is documented in
README.md at the repository root (`The driver contract`), and tests/check_docs.py checks it.

  burdin build <target> [--release]
      Compiles. Diagnostics go to stderr; each starts with a line in the form
          path:LINE:COL: error[E0000]: message
      (notes and suggestions may follow; --diagnostic-format=short gives exactly that line and nothing
      else). The exit code is non-zero when compilation fails.
  burdin run <target> [--release] [--link-lib NAME]... [-- args...]
      Compiles and runs. The program's stdout and stderr pass through, and its exit code
      is the command's exit code. A panic abort must give a non-zero exit code.
  burdin test <target> [--release]
      Prints "test result: N passed, M failed" (with ", I ignored" when any test is marked [ignore]) and
      one "FAIL <test name>" line per failed test.

<target> is the test's .bd file, or the package directory (the nearest folder with a burdin.toml).
--------------------------------------------------------------------------------
"""

from __future__ import annotations

import difflib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import traceback
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

DIAGNOSTIC_REGEX = re.compile(rb":([0-9]+):[0-9]+: error\[(E[0-9]{4})\]")
DIAGNOSTIC_LINE_REGEX = re.compile(rb"error\[E[0-9]{4}\]")
TEST_SUMMARY_REGEX = re.compile(rb"test result: ([0-9]+) passed, ([0-9]+) failed(, ([0-9]+) ignored)?")
BURDIN_TEST_DIRECTIVE_REGEX = re.compile(rb"^burdin-test passed=([0-9]+) failed=([0-9]+)( ignored=([0-9]+))?$")
MARKER_REGEX = re.compile(rb"//~ (E[0-9]{4})")
FAILED_TEST_PREFIX = b"FAIL "
KINDS = (b"run-abort", b"run", b"test-compile-fail", b"compile-fail", b"burdin-test")
KIND_WORDS = {b"run", b"run-abort", b"compile-fail", b"test-compile-fail", b"burdin-test", b"undecided"}
DEFAULT_TIMEOUT = 30
DEFAULT_JOBS = min(8, os.cpu_count() or 1)

USAGE = __doc__.split("--------------------------------------------------------------------------------")[0].rstrip()


def normalize(data: bytes) -> bytes:
    """Match the shell runner's `tr -d '\\r'`: delete every carriage return."""
    return data.replace(b"\r", b"")


def first(values):
    return values[0] if values else None


def text(data: bytes) -> str:
    """Decode compiler output for a diagnostic message without ever failing."""
    return data.decode("utf-8", "replace")


def head_bytes(data: bytes, limit: int = 2000) -> str:
    return text(data[:limit])


def head_lines(lines, limit: int) -> str:
    return "\n".join(text(line) for line in lines[:limit])


class Case:
    """One test file with everything the workers need, computed before they start."""

    def __init__(self, rel: str, root: Path, target: Path, kind: str, directives, markers, expected_lines):
        self.rel = rel
        self.target = target
        self.kind = kind
        self.directives = directives
        self.markers = markers
        self.expected_lines = expected_lines
        self.expected_bytes = b"".join(line + b"\n" for line in expected_lines)
        self.base = os.fsencode(root.name)
        self.skip = None


def has_directive(directives, name: bytes) -> bool:
    for d in directives:
        if d == name or d.startswith(name + b" ") or d.startswith(name + b":"):
            return True
    return False


def directive_values(directives, name: bytes):
    prefix = name + b":"
    values = []
    for d in directives:
        if d.startswith(prefix):
            value = d[len(prefix):]
            if value.startswith(b" "):
                value = value[1:]
            values.append(value)
    return values


def kind_of(directives) -> str:
    if has_directive(directives, b"undecided"):
        return "undecided"
    for kind in KINDS:
        if has_directive(directives, kind):
            return kind.decode("ascii")
    return "unknown"


def expected_stdout(lines):
    """The expected stdout lines: everything after `//> `, with any carriage returns already removed."""
    return [line[4:] for line in lines if line.startswith(b"//> ")]


def expected_markers(lines):
    markers = set()
    for number, line in enumerate(lines, 1):
        for match in MARKER_REGEX.finditer(line):
            markers.add((number, match.group(1).decode("ascii")))
    return markers


def target_of(root: Path, tests_dir: Path) -> Path:
    """The package directory (nearest ancestor with burdin.toml inside tests/), or the file itself."""
    directory = root.parent
    while directory != tests_dir and directory != directory.parent:
        if (directory / "burdin.toml").is_file():
            return directory
        directory = directory.parent
    return root


def host_platform() -> str:
    if sys.platform.startswith("linux"):
        return "linux"
    if sys.platform == "darwin":
        return "macos"
    if sys.platform.startswith("win"):
        return "windows"
    return "unknown"


def resolve_burdin(repo_dir: Path):
    value = os.environ.get("BURDIN")
    if value:
        return str(Path(value).resolve())
    for profile in ("debug", "release"):
        for name in ("burdin", "burdin.exe"):
            candidate = repo_dir / "target" / profile / name
            if candidate.is_file():
                return str(candidate)
    return shutil.which("burdin")


def discover(tests_dir: Path):
    """Every .bd file with a directive line, sorted by path."""
    found = []
    for path in sorted(tests_dir.rglob("*.bd"), key=lambda p: p.as_posix()):
        if path.suffix != ".bd":
            continue  # GNU grep's --include is case-sensitive, even on Windows
        data = path.read_bytes()
        if any(line.startswith(b"//@ ") for line in normalize(data).split(b"\n")):
            found.append(path)
    return found


def matches_filter(rel: str, filters) -> bool:
    return not filters or any(f in rel for f in filters)


def run_command(args, cwd: Path, timeout: int, merge: bool = False, input_data=None):
    """Run a command. `input_data` is the bytes it reads on standard input; without it, standard input is closed,
    so a test never waits for input it did not declare."""
    if input_data is None:
        kwargs = {"stdin": subprocess.DEVNULL}
    else:
        kwargs = {"input": input_data}
    return subprocess.run(
        args,
        cwd=str(cwd),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT if merge else subprocess.PIPE,
        timeout=timeout,
        check=False,
        **kwargs,
    )


def unescape_stdin(value: bytes) -> bytes:
    """The text of one `//@ stdin:` line: `\\n`, `\\r`, and `\\\\` are escapes; any other backslash is literal."""
    out = bytearray()
    index = 0
    while index < len(value):
        if value[index:index + 1] == b"\\" and index + 1 < len(value):
            escape = value[index + 1:index + 2]
            if escape == b"n":
                out.append(10)
                index += 2
                continue
            if escape == b"r":
                out.append(13)
                index += 2
                continue
            if escape == b"\\":
                out.append(92)
                index += 2
                continue
        out.append(value[index])
        index += 1
    return bytes(out)


def stdin_bytes(directives) -> bytes:
    """The standard input of a run test: each `//@ stdin:` line and its newline, then the `//@ stdin-eof:` text
    (which has no terminator)."""
    data = b"".join(unescape_stdin(value) + b"\n" for value in directive_values(directives, b"stdin"))
    data += b"".join(unescape_stdin(value) for value in directive_values(directives, b"stdin-eof"))
    return data


def unified_diff(expected, actual_bytes: bytes, limit: int) -> str:
    actual = actual_bytes.split(b"\n")
    if actual and actual[-1] == b"":
        actual.pop()
    diff = difflib.unified_diff(
        [text(line) for line in expected],
        [text(line) for line in actual],
        fromfile="expected",
        tofile="actual",
        lineterm="",
    )
    return "\n".join(list(diff)[:limit])


def test_run(case: Case, burdin: str, release: bool, timeout: int):
    directives = case.directives
    args = first(directive_values(directives, b"args")) or b""
    libs = directive_values(directives, b"link-lib")
    want_code = first(directive_values(directives, b"exit-code"))
    try:
        want_code = int(want_code) if want_code is not None else 0
    except ValueError:
        return False, "invalid exit-code directive", ""

    command = [burdin, "run", str(case.target)]
    if release:
        command.append("--release")
    for lib in libs:
        command.extend(["--link-lib", os.fsdecode(lib)])
    command.append("--")
    command.extend(part.decode("utf-8", "surrogateescape") for part in args.split())

    work = Path(tempfile.mkdtemp(prefix="burdin-test-"))
    try:
        try:
            proc = run_command(command, work, timeout, input_data=stdin_bytes(directives))
        except subprocess.TimeoutExpired:
            return False, f"timed out after {timeout}s", ""
        except OSError as error:
            return False, f"cannot run the compiler: {error}", ""

        diagnostics = [line for line in proc.stderr.split(b"\n") if DIAGNOSTIC_LINE_REGEX.search(line)]
        diagnostics += [line for line in proc.stdout.split(b"\n") if DIAGNOSTIC_LINE_REGEX.search(line)]
        if diagnostics:
            return False, "compilation failed", head_lines(diagnostics, 10)

        if case.kind == "run":
            if proc.returncode != want_code:
                return False, f"exit code {proc.returncode}, expected {want_code}", head_bytes(proc.stderr)
        else:
            if proc.returncode == 0:
                return False, "expected an abort but the program exited with 0", ""
        for value in directive_values(directives, b"stderr"):
            if not value:
                continue
            if value not in proc.stderr:
                return False, f"stderr does not contain: {text(value)}", head_bytes(proc.stderr)

        actual = normalize(proc.stdout)
        if actual != case.expected_bytes:
            return False, "stdout differs from the //> lines", unified_diff(case.expected_lines, actual, 40)
        return True, "", ""
    finally:
        shutil.rmtree(work, ignore_errors=True)


def test_compile_fail(case: Case, burdin: str, release: bool, timeout: int, mode: str):
    command = [burdin, mode, str(case.target)]
    if release:
        command.append("--release")

    work = Path(tempfile.mkdtemp(prefix="burdin-test-"))
    try:
        try:
            proc = run_command(command, work, timeout, merge=True)
        except subprocess.TimeoutExpired:
            return False, f"timed out after {timeout}s", ""
        except OSError as error:
            return False, f"cannot run the compiler: {error}", ""

        if proc.returncode == 0:
            return False, "compilation succeeded but errors were expected", ""

        output = normalize(proc.stdout)
        actual = set()
        for line in output.split(b"\n"):
            if case.base + b":" not in line:
                continue
            # The diagnostic regex is greedy, like the `sed` expression the shell runner used.
            match = DIAGNOSTIC_REGEX.search(line)
            if match:
                actual.add(f"{int(match.group(1))} {match.group(2).decode('ascii')}")

        expected = sorted(f"{line} {code}" for line, code in case.markers)
        actual_lines = sorted(actual)
        if expected != actual_lines:
            diff = "\n".join(
                list(
                    difflib.unified_diff(
                        expected, actual_lines, fromfile="expected", tofile="actual", lineterm=""
                    )
                )[:30]
            )
            return False, "diagnostics differ from the //~ markers (- expected, + actual; format: LINE CODE)", diff
        return True, "", ""
    finally:
        shutil.rmtree(work, ignore_errors=True)


def test_burdin_test(case: Case, burdin: str, release: bool, timeout: int):
    command = [burdin, "test", str(case.target)]
    if release:
        command.append("--release")

    directive = None
    for value in case.directives:
        match = BURDIN_TEST_DIRECTIVE_REGEX.search(value)
        if match:
            directive = match
            break
    if directive is None:
        return False, "burdin-test directive is missing or malformed", ""

    want_passed = directive.group(1).decode("ascii")
    want_failed = directive.group(2).decode("ascii")
    want_ignored = directive.group(4).decode("ascii") if directive.group(4) else "0"

    work = Path(tempfile.mkdtemp(prefix="burdin-test-"))
    try:
        try:
            proc = run_command(command, work, timeout, merge=True)
        except subprocess.TimeoutExpired:
            return False, f"timed out after {timeout}s", ""
        except OSError as error:
            return False, f"cannot run the compiler: {error}", ""

        output = normalize(proc.stdout)
        summary = None
        for line in output.split(b"\n"):
            match = TEST_SUMMARY_REGEX.search(line)
            if match:
                summary = match
        if summary is None:
            return False, "no test summary line in the output", head_bytes(output)

        got_passed = summary.group(1).decode("ascii")
        got_failed = summary.group(2).decode("ascii")
        got_ignored = summary.group(4).decode("ascii") if summary.group(4) else "0"
        if (got_passed, got_failed, got_ignored) != (want_passed, want_failed, want_ignored):
            return (
                False,
                f"got {got_passed} passed, {got_failed} failed, {got_ignored} ignored; "
                f"expected {want_passed} passed, {want_failed} failed, {want_ignored} ignored",
                head_bytes(output),
            )

        for value in directive_values(case.directives, b"failed-test"):
            if not value:
                continue
            if FAILED_TEST_PREFIX + value not in output:
                return False, f"test not reported as failed: {text(value)}", head_bytes(output)
        return True, "", ""
    finally:
        shutil.rmtree(work, ignore_errors=True)


def run_case(case: Case, burdin: str, release: bool, timeout: int):
    try:
        if case.kind in ("run", "run-abort"):
            return test_run(case, burdin, release, timeout)
        if case.kind == "compile-fail":
            return test_compile_fail(case, burdin, release, timeout, "build")
        if case.kind == "test-compile-fail":
            return test_compile_fail(case, burdin, release, timeout, "test")
        if case.kind == "burdin-test":
            return test_burdin_test(case, burdin, release, timeout)
        return False, "unknown test kind", ""
    except Exception:  # a runner bug must fail one test, not the whole run
        return False, "internal error in the test runner", traceback.format_exc()


def parse_args(argv):
    options = {"release": False, "verbose": False, "list": False, "timeout": DEFAULT_TIMEOUT,
               "jobs": DEFAULT_JOBS, "filters": []}
    index = 0
    while index < len(argv):
        argument = argv[index]
        if argument in ("-r", "--release"):
            options["release"] = True
        elif argument in ("-v", "--verbose"):
            options["verbose"] = True
        elif argument in ("-l", "--list"):
            options["list"] = True
        elif argument in ("-h", "--help"):
            print(USAGE)
            raise SystemExit(0)
        elif argument in ("-t", "--timeout"):
            index += 1
            if index >= len(argv):
                return usage_error(f"{argument} needs a value")
            try:
                options["timeout"] = int(argv[index])
            except ValueError:
                return usage_error(f"invalid timeout: {argv[index]}")
            if options["timeout"] < 1:
                return usage_error(f"invalid timeout: {argv[index]}")
        elif argument in ("-j", "--jobs"):
            index += 1
            if index >= len(argv):
                return usage_error(f"{argument} needs a value")
            try:
                options["jobs"] = int(argv[index])
            except ValueError:
                return usage_error(f"invalid job count: {argv[index]}")
            if options["jobs"] < 1:
                return usage_error(f"invalid job count: {argv[index]}")
        elif argument.startswith("-"):
            return usage_error(f"unknown option: {argument}")
        else:
            options["filters"].append(argument)
        index += 1
    return options


def usage_error(message):
    print(message, file=sys.stderr)
    print(USAGE, file=sys.stderr)
    raise SystemExit(2)


def main(argv) -> int:
    options = parse_args(argv)
    tests_dir = Path(__file__).resolve().parent
    repo_dir = tests_dir.parent
    platform = host_platform()

    burdin = None
    if not options["list"]:
        burdin = resolve_burdin(repo_dir)
        if not burdin:
            print(
                "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin",
                file=sys.stderr,
            )
            return 2

    cases = []
    for root in discover(tests_dir):
        rel = root.relative_to(tests_dir).as_posix()
        if not matches_filter(rel, options["filters"]):
            continue
        data = root.read_bytes()
        lines = normalize(data).split(b"\n")
        directives = [line[4:] for line in lines if line.startswith(b"//@ ")]
        expected_lines = expected_stdout(lines)
        case = Case(
            rel,
            root,
            target_of(root, tests_dir),
            kind_of(directives),
            directives,
            expected_markers(lines),
            expected_lines,
        )
        cases.append(case)

    if options["list"]:
        for case in cases:
            print(f"{case.kind:<13} {case.rel}")
        return 0

    for case in cases:
        skip_platforms = {
            value.decode("ascii", "replace")
            for raw in directive_values(case.directives, b"skip-platform")
            for value in raw.split()
        }
        if case.kind == "undecided":
            case.skip = "undecided"
        elif platform in skip_platforms:
            case.skip = f"skip-platform: {platform}"
        elif options["release"] and has_directive(case.directives, b"debug-only"):
            case.skip = "debug-only"
        elif not options["release"] and has_directive(case.directives, b"release-only"):
            case.skip = "release-only"

    runnable = [(index, case) for index, case in enumerate(cases) if case.skip is None and case.kind != "unknown"]
    results = [None] * len(cases)
    jobs = min(options["jobs"], max(1, len(runnable)))
    if jobs == 1 or len(runnable) <= 1:
        for index, case in runnable:
            results[index] = run_case(case, burdin, options["release"], options["timeout"])
    else:
        with ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = {
                pool.submit(run_case, case, burdin, options["release"], options["timeout"]): index
                for index, case in runnable
            }
            for future in as_completed(futures):
                results[futures[future]] = future.result()

    passed = 0
    failed = 0
    skipped = 0
    failed_list = []
    for index, case in enumerate(cases):
        if case.skip is not None:
            print(f"SKIP  {case.rel} ({case.skip})")
            skipped += 1
            continue
        if case.kind == "unknown":
            print(f"FAIL  {case.rel}: no test kind directive")
            failed += 1
            failed_list.append(case.rel)
            continue
        ok, reason, detail = results[index]
        if ok:
            print(f"PASS  {case.rel}")
            passed += 1
        else:
            print(f"FAIL  {case.rel}: {reason}")
            if options["verbose"] and detail:
                for line in detail.rstrip("\n").split("\n"):
                    print(f"        {line}")
            failed += 1
            failed_list.append(case.rel)

    print()
    print(f"{passed} passed, {failed} failed, {skipped} skipped")
    if failed:
        print("Failed:")
        for rel in failed_list:
            print(f"  {rel}")
        return 1
    return 0


if __name__ == "__main__":
    # Keep the runner's own output byte-identical on every platform; Python would otherwise write CRLF on Windows.
    sys.stdout.reconfigure(newline="\n")
    sys.exit(main(sys.argv[1:]))
