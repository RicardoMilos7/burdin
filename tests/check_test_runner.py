#!/usr/bin/env python3
"""Checks the black-box behavior of `burdin test`: attributes, parallel execution, output capture, isolation, and
package discovery. Needs a built driver. Usage: tests/check_test_runner.py

BURDIN selects the driver, like tests/run_tests.py. The fixtures under tests/runner/ are ordinary packages with
no `//@` directives, so the conformance runner ignores them.

What it checks (issues #438, #439, #440, #441, #442, #443, #444):
  * [ignore], [should_panic], [should_panic("text")], [should_fail], and [timeout(ms)] decide pass/fail;
    --ignored and --include-ignored select ignored tests and the summary counts what was skipped;
  * [differential] compares the comptime interpreter's Ok/Err outcome with the compiled run, and a divergent
    fixture is reported as a failure with the two outcomes;
  * -j runs independent tests at once (a test that waits for another only finishes with -j 2), and the report
    keeps declaration order;
  * a test's stdout and stderr are captured and printed only when it fails, and --verbose always prints them;
  * every test runs in its own fresh working directory with a clean environment, and the runner removes every
    temporary directory it created;
  * -p and --workspace select packages, --filter selects tests, and dependency tests do not run by default.
"""

import os
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()
fixtures = harness.TESTS / "runner"

with harness.temporary_dir("burdin-runner-") as work:
    scratch = work / "scratch"
    scratch.mkdir(parents=True)

    status = 0
    out = ""
    err = ""

    def run_driver(*args, env=None) -> None:
        global status, out, err
        result = harness.run(args, cwd=work, env=env)
        status = result.returncode
        out = harness.text(harness.normalize(result.stdout)).rstrip("\n")
        err = harness.text(harness.normalize(result.stderr)).rstrip("\n")

    def expect_status(description: str, expected: int) -> None:
        global checks
        checks += 1
        if status != expected:
            add_problem(f"{description}: exit status {status}, expected {expected}")

    def expect_contains(description: str, needle: str, output: str) -> None:
        global checks
        checks += 1
        if needle not in output:
            add_problem(f"{description}: output does not contain: {needle}")

    def expect_not_contains(description: str, needle: str, output: str) -> None:
        global checks
        checks += 1
        if needle in output:
            add_problem(f"{description}: output should not contain: {needle}")

    def expect_fail_order(description: str, first_name: str, last_name: str) -> None:
        global checks
        checks += 1
        failures = [line for line in out.split("\n") if line.startswith("FAIL ")]
        first = failures[0] if failures else ""
        last = failures[-1] if failures else ""
        if first != f"FAIL {first_name}":
            add_problem(f"{description}: first failure is '{first}', expected 'FAIL {first_name}'")
        if last != f"FAIL {last_name}":
            add_problem(f"{description}: last failure is '{last}', expected 'FAIL {last_name}'")

    # --- attributes -----------------------------------------------------------------

    run_driver(burdin, "test", str(fixtures / "attributes"), "-j", "4")
    expect_status("attributes (default)", 0)
    expect_contains("attributes (default)", "test result: 4 passed, 0 failed, 1 ignored", out)
    expect_not_contains("attributes (default)", "FAIL", out)

    run_driver(burdin, "test", str(fixtures / "attributes"), "--ignored", "-j", "4")
    expect_status("attributes (--ignored)", 0)
    expect_contains("attributes (--ignored)", "test result: 1 passed, 0 failed", out)
    expect_not_contains("attributes (--ignored)", "ignored", out)

    run_driver(burdin, "test", str(fixtures / "attributes"), "--include-ignored", "-j", "4")
    expect_status("attributes (--include-ignored)", 0)
    expect_contains("attributes (--include-ignored)", "test result: 5 passed, 0 failed", out)
    expect_not_contains("attributes (--include-ignored)", "ignored", out)

    # --- differential tests ---------------------------------------------------------

    # A [differential] body runs at compile time and as the compiled test; agreement passes whether both return
    # Ok or both return Err (spec/test-runner.md).
    run_driver(burdin, "test", str(fixtures / "differential"), "--filter", "agreeing", "-j", "1")
    expect_status("differential (agreement)", 0)
    expect_contains("differential (agreement)", "test result: 2 passed, 0 failed", out)
    expect_not_contains("differential (agreement)", "FAIL", out)

    # The fixture's third body depends on the unspecified Map iteration order, so the interpreter (insertion order)
    # and the compiled hash table disagree. The check must report the mismatch instead of passing.
    run_driver(burdin, "test", str(fixtures / "differential"), "--filter", "diverges", "-j", "1")
    expect_status("differential (divergence)", 1)
    expect_contains(
        "differential (divergence)",
        "the compiled test returned `Err`, but the comptime interpreter returned `Ok`",
        out,
    )
    expect_contains("differential (divergence)", "FAIL diverges on map order", out)
    expect_contains("differential (divergence)", "test result: 0 passed, 1 failed", out)

    # --- timeouts -------------------------------------------------------------------

    run_driver(burdin, "test", str(fixtures / "timeout"), "-j", "2")
    expect_status("timeout", 1)
    expect_contains("timeout", "timed out after 100 ms", out)
    expect_contains("timeout", "FAIL hangs", out)
    expect_contains("timeout", "test result: 1 passed, 1 failed", out)

    # --- output capture -------------------------------------------------------------

    run_driver(burdin, "test", str(fixtures / "output"), "-j", "4")
    expect_status("output capture (default)", 1)
    expect_contains("output capture (default)", "visible stdout", out)
    expect_contains("output capture (default)", "visible stderr", out)
    expect_not_contains("output capture (default)", "hidden pass output", out)
    expect_contains("output capture (default)", "test result: 1 passed, 2 failed", out)
    expect_fail_order("output capture (default)", "fails loudly", "also fails")

    run_driver(burdin, "test", str(fixtures / "output"), "--verbose", "-j", "4")
    expect_status("output capture (--verbose)", 1)
    expect_contains("output capture (--verbose)", "hidden pass output", out)
    expect_contains("output capture (--verbose)", "PASS passes quietly", out)
    expect_contains("output capture (--verbose)", "FAIL fails loudly", out)

    # --- typed test failures --------------------------------------------------------

    # A failed assertion returns a typed `AssertError`; the test binary prints its message (the call text, the
    # values, and the location) to stderr, which the runner shows with the captured output (spec/errors.md, ERR9).
    run_driver(burdin, "test", str(fixtures / "typed_assert"), "-j", "1")
    expect_status("typed failure", 1)
    expect_contains("typed failure", "assertion failed: assert_eq(1 + 1, 3)", out)
    expect_contains("typed failure", "2 != 3", out)
    expect_contains("typed failure", "FAIL eq fails", out)

    # --- leak detection -------------------------------------------------------------

    # A `burdin test` build allocates through the checking allocator. A test that leaves an allocation live fails,
    # and the report names the allocation site the compiler recorded (`spec/allocators.md`, AL9).
    run_driver(burdin, "test", str(fixtures / "leaks"), "-j", "1")
    expect_status("leak detection", 1)
    expect_contains("leak detection", "test leak: 1 allocation(s) leaked, 16 byte(s)", out)
    expect_contains("leak detection", "FAIL leaks", out)
    expect_contains("leak detection", f"allocated at {fixtures / 'leaks' / 'src' / 'main.bd'}:9:18", out)
    expect_contains("leak detection", "test result: 1 passed, 1 failed", out)

    # --- isolation and cleanup ------------------------------------------------------

    # The driver's temporary directories (the test binaries and each test's working directory) go under
    # TMPDIR/TMP/TEMP, so an empty scratch directory afterwards proves the runner cleaned up after itself.
    isolation_env = {
        **os.environ,
        "TMPDIR": str(scratch),
        "TMP": str(scratch),
        "TEMP": str(scratch),
    }
    run_driver(burdin, "test", str(fixtures / "isolation"), "-j", "1", env=isolation_env)
    expect_status("isolation", 0)
    expect_contains("isolation", "test result: 2 passed, 0 failed", out)
    checks += 1
    leftovers = sorted(path.name for path in scratch.iterdir())
    if leftovers:
        add_problem(f"isolation: the runner left temporary files behind: {' '.join(leftovers)}")

    # --- parallelism ----------------------------------------------------------------

    # `waiter` only finishes if `creator` runs at the same time, which the isolated directories make a real test of
    # concurrency: a sequential runner starts `waiter` first and it times out.
    run_driver(burdin, "test", str(fixtures / "parallel"), "-j", "2")
    expect_status("parallel (-j 2)", 0)
    expect_contains("parallel (-j 2)", "test result: 2 passed, 0 failed", out)

    run_driver(burdin, "test", str(fixtures / "parallel"), "-j", "1")
    expect_status("parallel (-j 1)", 1)
    expect_contains("parallel (-j 1)", "FAIL waiter", out)
    expect_contains("parallel (-j 1)", "test result: 1 passed, 1 failed", out)

    # --- discovery and selection ----------------------------------------------------

    workspace = fixtures / "workspace"
    run_driver(burdin, "test", str(workspace), "-j", "4")
    expect_status("discovery (default)", 0)
    expect_contains("discovery (default)", "test result: 2 passed, 0 failed", out)
    expect_not_contains("discovery (default)", "lib one", out)

    run_driver(burdin, "test", str(workspace), "-p", "util", "-j", "4")
    expect_status("discovery (-p util)", 0)
    expect_contains("discovery (-p util)", "test result: 1 passed, 0 failed", out)

    # A library package has no src/main.bd; its tests still run when it is the target.
    run_driver(burdin, "test", str(workspace / "util"), "-j", "4")
    expect_status("discovery (library target)", 0)
    expect_contains("discovery (library target)", "test result: 1 passed, 0 failed", out)

    run_driver(burdin, "test", str(workspace), "--workspace", "-j", "4")
    expect_status("discovery (--workspace)", 0)
    expect_contains("discovery (--workspace)", "test result: 3 passed, 0 failed", out)

    run_driver(burdin, "test", str(workspace), "--workspace", "--filter", "lib", "-j", "4")
    expect_status("discovery (--filter)", 0)
    expect_contains("discovery (--filter)", "test result: 1 passed, 0 failed", out)

    run_driver(burdin, "test", str(workspace), "--filter", "no such test", "-j", "4")
    expect_status("discovery (empty filter)", 0)
    expect_contains("discovery (empty filter)", "test result: 0 passed, 0 failed", out)

    run_driver(burdin, "test", str(workspace), "-p", "missing", "-j", "4")
    expect_status("discovery (unknown package)", 2)
    expect_contains("discovery (unknown package)", "no package named `missing`", err)

    run_driver(burdin, "test", str(workspace), "-p", "std", "-j", "4")
    expect_status("discovery (embedded package)", 2)
    expect_contains("discovery (embedded package)", "embedded", err)

    # --- job control ----------------------------------------------------------------

    run_driver(burdin, "test", str(fixtures / "attributes"), "-j", "0")
    expect_status("job control (-j 0)", 2)
    expect_contains("job control (-j 0)", "at least 1", err)

    run_driver(burdin, "test", str(fixtures / "attributes"), "--ignored", "--include-ignored")
    expect_status("job control (conflicting ignore flags)", 2)
    expect_contains("job control (conflicting ignore flags)", "cannot be used together", err)

    run_driver(burdin, "test", str(fixtures / "attributes"), "-p", "attributes", "--workspace")
    expect_status("job control (conflicting selection flags)", 2)
    expect_contains("job control (conflicting selection flags)", "cannot be used together", err)

print(f"Checked {checks} runner behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
