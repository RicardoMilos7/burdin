#!/usr/bin/env python3
"""Checks `burdin test --interpret` (spec/test-runner.md, "Interpreting a test"; issue #622). Needs a built driver.

BURDIN selects the driver, like tests/run_tests.py. The interpreter runs every test body with strict memory checks,
so the fixtures under tests/interpret each plant one bug and mark the line it happens on:

  tests/interpret/bugs.bd   an out-of-bounds store, a use after free, a double free, a read of uninitialized memory,
                            a misaligned store, and a leak, each with its own `//~ interpret-error` marker;
  tests/interpret/passing.bd  correct code, with the standard library's containers, iterators, and closures: the run
                            passes it, so a check that fires on valid programs would show here;
  tests/interpret/calls_c.bd  a test that calls C and a test that reads the environment: both are skipped, with the
                            reason, and the other test runs.

The check also runs the test-runner conformance files that compiled `burdin test` already passes (`m10` and `m12`)
through the interpreter, so std code inside ordinary tests runs there too, and
`tests/m24_stdlib/interpreted_stdlib.bd` holds the same std-heavy tests as the conformance suite runs compiled. `--interpret` is rejected with
`--fuzz-replay`, `--sanitize`, `--bless`, and on any command but `test`.
"""

import re
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()
fixtures = harness.TESTS / "interpret"


def run(*args):
    result = harness.run(args, cwd=harness.TESTS, timeout=600)
    return (
        result.returncode,
        harness.text(harness.normalize(result.stdout)),
        harness.text(harness.normalize(result.stderr)),
    )


def expect(description: str, condition: bool, detail: str = "") -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(f"{description}{': ' + detail if detail else ''}")


def markers(path):
    """The `//~ interpret-error: <text>` markers of a fixture: line number and expected report text."""
    found = []
    for number, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
        match = re.search(r"//~ interpret-error: (.+)$", line)
        if match:
            found.append((number, match.group(1).strip()))
    return found


# Every planted bug is reported at its own line, with the class of error named.
code, out, err = run(burdin, "test", str(fixtures / "bugs.bd"), "--interpret")
expect("bugs: the run fails", code == 1, f"exit status {code}: {out}{err}")
reports = [line for line in out.split("\n") if re.match(r"^.+\.bd:[0-9]+:[0-9]+: ", line)]
expected = markers(fixtures / "bugs.bd")
expect("bugs: every planted bug has a marker", len(expected) == 6, f"{len(expected)} markers")
expect("bugs: one report per bug", len(reports) == len(expected), "\n".join(reports))
for (line, text), report in zip(expected, reports):
    location = re.match(r"^.+:([0-9]+):([0-9]+): (.*)$", report)
    expect(
        f"bugs: line {line} reports `{text}`",
        location is not None and int(location.group(1)) == line and location.group(3).startswith(text),
        report,
    )
for name in ("out of bounds", "use after free", "double free", "uninitialized read", "misaligned", "leak"):
    expect(f"bugs: {name} fails", f'FAIL {name}' in out, out)
expect("bugs: the summary counts them", "test result: 0 passed, 6 failed" in out, out)

# Correct code passes: the checks never fire on a program that frees what it allocates.
code, out, err = run(burdin, "test", str(fixtures / "passing.bd"), "--interpret")
expect("passing: exit 0", code == 0, f"exit status {code}: {out}{err}")
expect("passing: every test runs", "test result: 3 passed, 0 failed" in out, out)

# Tests that need C or the operating system are skipped, with the reason; the rest run.
code, out, err = run(burdin, "test", str(fixtures / "calls_c.bd"), "--interpret")
expect("calls_c: exit 0", code == 0, f"exit status {code}: {out}{err}")
expect("calls_c: the C test is skipped with its reason", "SKIP asks C for the process id (calling the C function `getpid`)" in out, out)
expect("calls_c: the environment test is skipped", "SKIP reads the environment (" in out, out)
expect("calls_c: the other test runs", "test result: 1 passed, 0 failed, 2 skipped" in out, out)

# Ordinary test files run in the interpreter too, std code included: the same verdicts a compiled run gives.
for relative, summary, failed in [
    ("m10_test_runner/all_pass.bd", "test result: 3 passed, 0 failed", []),
    ("m10_test_runner/assert_messages_typed.bd", "test result: 4 passed, 0 failed", []),
    ("m10_test_runner/helper_propagation.bd", "test result: 1 passed, 1 failed", ["FAIL helper fails"]),
    ("m12_closures/closure_in_test.bd", None, []),
    ("m24_stdlib/interpreted_stdlib.bd", "test result: 5 passed, 0 failed", []),
]:
    code, out, err = run(burdin, "test", str(harness.TESTS / relative), "--interpret")
    if summary is not None:
        expect(f"{relative}: the same summary", summary in out, out + err)
    expect(f"{relative}: the exit status matches the failures", (code == 1) == bool(failed), f"exit {code}: {out}")
    for line in failed:
        expect(f"{relative}: {line}", line in out, out)

# Usage errors.
for args, message in [
    (("build", str(fixtures / "passing.bd"), "--interpret"), "only valid for `burdin test`"),
    (("test", str(fixtures / "passing.bd"), "--interpret", "--fuzz-replay"), "choose one"),
    (("test", str(fixtures / "passing.bd"), "--interpret", "--sanitize=address"), "does not apply"),
    (("test", str(fixtures / "passing.bd"), "--interpret", "--bless"), "cannot write snapshots"),
]:
    code, _, err = run(burdin, *args)
    expect(f"usage error for `{' '.join(args)}`", code == 2, f"exit status {code}: {err}")
    expect(f"usage error for `{' '.join(args)}`", message in err, err)

print(f"Checked {checks} interpreter behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
