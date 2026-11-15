#!/usr/bin/env python3
"""Checks diagnostics presentation and tooling (issues #434, #435, #436). Needs a built driver.
Usage: tests/check_diagnostics.py

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * default output is plain when stderr is not a terminal, and --color=always/--no-color/NO_COLOR behave as
    documented (an explicit --color always wins over NO_COLOR; the last color flag wins);
  * --width (and COLUMNS) wraps the human format, with no line over the width, and --width 0 does not wrap;
  * --diagnostic-format=short is one plain line per diagnostic, and --diagnostic-format=json is one JSON
    object per diagnostic plus a summary object;
  * a diagnostic's note and suggestion reach the human output (`= note:`, `= help:`) and the JSON output;
  * --error-limit prints the first N errors after sorting and reports how many were hidden;
  * the output is byte-for-byte deterministic between runs;
  * the flags parse as `--option value` and `--option=value`, and bad values exit 2 with a usage error.
"""

import os
import re
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check() -> None:
    global checks
    checks += 1


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)

DIAGNOSTIC = re.compile(rb":[0-9]+:[0-9]+: error\[E[0-9]{4}\]")
SHORT_LINE = re.compile(r"^[^:]+:[0-9]+:[0-9]+: error\[E[0-9]{4}\]: ")
with harness.temporary_dir("burdin-diagnostics-") as work:
    harness.write_text(
        work / "two_errors.bd",
        'fn main() {\n    let a: i32 = "x"\n    let b: i32 = "y"\n}\n',
    )
    harness.write_text(
        work / "long_message.bd",
        "fn main() {\n    this_identifier_is_far_too_long_to_fit_on_one_terminal_line(1)\n}\n",
    )
    harness.write_text(
        work / "suggestion.bd",
        "struct Counter {\n"
        "    n: i32\n"
        "}\n"
        "\n"
        "fn bump(mut c: Counter) {\n"
        "    c.n = c.n + 1\n"
        "}\n"
        "\n"
        "fn main() {\n"
        "    let mut c = Counter { n: 0 }\n"
        "    bump(c)\n"
        "}\n",
    )
    harness.write_text(
        work / "note.bd",
        "struct Thing {\n"
        "    id: i32\n"
        "}\n"
        "\n"
        "fn take(own t: Thing) {}\n"
        "\n"
        "fn main() {\n"
        "    let t = Thing { id: 1 }\n"
        "    take(t)\n"
        "    let u = t\n"
        "}\n",
    )
    harness.write_text(work / "run_bad.bd", 'fn main() {\n    let n: i32 = "x"\n}\n')

    status = 0
    stdout = ""
    stderr = ""
    stderr_raw = b""

    def execute(args, env=None) -> None:
        global status, stdout, stderr, stderr_raw
        result = harness.run(args, cwd=work, env=env)
        status = result.returncode
        stdout = harness.text(result.stdout).rstrip("\n")
        stderr_raw = result.stderr
        stderr = harness.text(result.stderr).rstrip("\n")

    def drive(*args, env=None) -> None:
        execute([burdin, *args], env=env)

    def expect_status(description: str, expected: int) -> None:
        check()
        if status != expected:
            add_problem(f"{description}: exit status {status}, expected {expected}")

    def expect_contains(description: str, needle: str, haystack: str) -> None:
        check()
        if needle not in haystack:
            add_problem(f"{description}: output does not contain: {needle}")

    def stderr_lines():
        lines = harness.text(stderr_raw).split("\n")
        if lines and lines[-1] == "":
            lines.pop()
        return lines

    def diagnostic_line_list():
        lines = stderr_lines()
        end = next(
            (index for index, line in enumerate(lines) if line.startswith("error: could not compile")),
            None,
        )
        if end is not None:
            return lines[:end]
        return lines[:-1] if lines else []

    def diagnostic_lines() -> str:
        return "\n".join(diagnostic_line_list())

    def expect_no_color(description: str) -> None:
        check()
        if b"\x1b[" in stderr_raw:
            add_problem(f"{description}: output contains ANSI color")

    def expect_color(description: str) -> None:
        check()
        if b"\x1b[" not in stderr_raw:
            add_problem(f"{description}: output has no ANSI color")

    # --- Default output and color (issue #434) -----------------------------------

    drive("build", "two_errors.bd")
    expect_status("default output", 1)
    expect_no_color("piped stderr")
    check()
    count = sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode()))
    if count != 2:
        add_problem(f"default output: {count} diagnostic lines, expected 2")
    expect_contains("default output", "error: could not compile two_errors.bd (2 errors)", stderr)
    expect_contains("default output", "burdin explain", stderr)

    drive("build", "two_errors.bd", "--color=always")
    expect_status("--color=always", 1)
    expect_color("--color=always")

    drive("build", "two_errors.bd", "--color", "never")
    expect_status("--color never", 1)
    expect_no_color("--color never")

    drive("build", "two_errors.bd", "--no-color")
    expect_status("--no-color", 1)
    expect_no_color("--no-color")

    drive("build", "two_errors.bd", "--color=always", "--no-color")
    expect_no_color("the last color flag wins (no-color last)")

    drive("build", "two_errors.bd", "--no-color", "--color=always")
    expect_color("the last color flag wins (always last)")

    drive("build", "two_errors.bd", "--color=always", env={**os.environ, "NO_COLOR": "1"})
    expect_color("NO_COLOR yields to an explicit --color=always")

    drive("build", "two_errors.bd", env={**os.environ, "NO_COLOR": "1"})
    expect_no_color("NO_COLOR with the default color choice")

    # --- Wrapping (issue #434) ---------------------------------------------------

    drive("build", "long_message.bd", "--width", "60")
    expect_status("--width", 1)
    check()
    if any(len(line) > 60 for line in diagnostic_line_list()):
        add_problem("--width 60: a diagnostic line is wider than 60 characters")
    check()
    if not any(line.startswith("    ") for line in stderr_lines()):
        add_problem("--width 60: no continuation line is indented")
    check()
    # Wrapping only breaks at whitespace or inside a word; removing the newlines and indentation reconstructs the
    # message, including the long identifier.
    joined = "".join(
        line[4:] if line.startswith("    ") else line for line in diagnostic_line_list()
    )
    if "this_identifier_is_far_too_long_to_fit_on_one_terminal_line" not in joined:
        add_problem("--width 60: the wrapped message does not contain the identifier")

    drive("build", "long_message.bd", "--width", "0")
    check()
    if len(diagnostic_line_list()) != 1:
        add_problem("--width 0: the message was wrapped")

    drive("build", "long_message.bd", env={**os.environ, "COLUMNS": "60"})
    check()
    if any(len(line) > 60 for line in diagnostic_line_list()):
        add_problem("COLUMNS=60: a diagnostic line is wider than 60 characters")

    # --- Short and JSON formats (issues #434, #435) ------------------------------

    drive("build", "suggestion.bd", "--diagnostic-format=short")
    expect_status("--diagnostic-format=short", 1)
    expect_no_color("short format ignores color")
    check()
    if sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode())) != 1:
        add_problem("short format: not one diagnostic line")
    check()
    if "= help:" in stderr:
        add_problem("short format: contains a help line")
    check()
    for line in stderr_lines():
        if "error[" in line and not SHORT_LINE.match(line):
            add_problem("short format: a diagnostic line does not match path:LINE:COL: error[CODE]: message")
            break

    drive("build", "two_errors.bd", "--diagnostic-format=json")
    expect_status("--diagnostic-format=json", 1)
    check()
    if any(not re.fullmatch(r"\{.*\}", line) for line in stderr_lines()):
        add_problem("json format: a line is not a JSON object")
    check()
    if sum(1 for line in stderr_lines() if '"type":"diagnostic"' in line) != 2:
        add_problem("json format: expected two diagnostics")
    expect_contains("json format", '"code":"E0003"', stderr)
    expect_contains(
        "json format", '"span":{"file":"two_errors.bd","line":2,"column":18', stderr
    )
    expect_contains("json format", '"type":"summary","errors":2,"shown":2,"limit":null', stderr)

    drive("build", "two_errors.bd", "--diagnostic-format=json", "--color=always")
    expect_no_color("json format ignores color")

    drive("build", "suggestion.bd", "--diagnostic-format=json")
    expect_status("json suggestion", 1)
    expect_contains(
        "json suggestion", '"suggestions":[{"message":"add `mut` before this argument"', stderr
    )
    expect_contains("json suggestion", '"replacement":"mut "', stderr)

    drive("build", "suggestion.bd", "--no-color")
    expect_contains("human suggestion", "= help: add `mut` before this argument", stderr)

    drive("build", "note.bd", "--diagnostic-format=json")
    expect_status("json note", 1)
    expect_contains(
        "json note", '"notes":[{"message":"value moved here","span":{"file":"note.bd"', stderr
    )

    drive("build", "note.bd", "--no-color")
    expect_contains("human note", "= note: note.bd:", stderr)
    expect_contains("human note", "value moved here", stderr)

    # --- Error limit and ordering (issue #436) -----------------------------------

    drive("build", "two_errors.bd", "--error-limit", "1")
    expect_status("--error-limit 1", 1)
    check()
    if sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode())) != 1:
        add_problem("--error-limit 1: more than one diagnostic line")
    expect_contains("--error-limit 1", "note: 1 more error not shown (--error-limit 1)", stderr)
    expect_contains("--error-limit 1", "error: could not compile two_errors.bd (2 errors)", stderr)

    drive("build", "two_errors.bd", "--error-limit", "0")
    check()
    if sum(1 for line in stderr_lines() if DIAGNOSTIC.search(line.encode())) != 2:
        add_problem("--error-limit 0: did not print every diagnostic")
    check()
    if "not shown" in stderr:
        add_problem("--error-limit 0: reported hidden errors")

    drive("build", "two_errors.bd", "--error-limit", "5")
    check()
    if "not shown" in stderr:
        add_problem("--error-limit 5: reported hidden errors")

    drive("build", "two_errors.bd", "--diagnostic-format=short")
    first = stderr_raw
    drive("build", "two_errors.bd", "--diagnostic-format=short")
    check()
    if first != stderr_raw:
        add_problem("determinism: two runs of the same input differ")

    # The diagnostics are sorted by position even though the checker may report them in another order.
    drive("build", "two_errors.bd", "--diagnostic-format=short")
    check()
    numbers = ""
    for line in stderr_lines():
        match = re.search(r":([0-9]+):[0-9]+: error\[E[0-9]{4}\]", line)
        if match:
            numbers += match.group(1) + " "
    if numbers != "2 3 ":
        add_problem(f"ordering: diagnostic lines are {numbers}, expected 2 3")

    # --- Flag syntax and usage errors -------------------------------------------

    drive("build", "two_errors.bd", "--diagnostic-format", "xml")
    expect_status("unknown diagnostic format", 2)
    expect_contains("unknown diagnostic format", "unknown diagnostic format", stderr)

    drive("build", "two_errors.bd", "--color", "sparkles")
    expect_status("unknown color choice", 2)

    drive("build", "two_errors.bd", "--width", "wide")
    expect_status("bad width", 2)

    drive("build", "two_errors.bd", "--error-limit", "-1")
    expect_status("bad error limit", 2)

    drive("build", "two_errors.bd", "--release=yes")
    expect_status("a value on --release", 2)

    # `run` accepts the same options and still exits 1 on a compilation failure.
    drive("run", "run_bad.bd", "--no-color", "--error-limit", "1")
    expect_status("run with diagnostic options", 1)
    expect_no_color("run with diagnostic options")
    expect_contains(
        "run with diagnostic options",
        "error: could not compile run_bad.bd (1 error)",
        harness.text(stderr_raw),
    )

print(f"Checked {checks} diagnostics behaviors from {work}")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
