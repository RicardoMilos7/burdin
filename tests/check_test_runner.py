#!/usr/bin/env python3
"""Checks the black-box behavior of `burdin test`: attributes, parallel execution, output capture, isolation, and
package discovery. Needs a built driver. Usage: tests/check_test_runner.py

BURDIN selects the driver, like tests/run_tests.py. The fixtures under tests/runner/ are ordinary packages with
no `//@` directives, so the conformance runner ignores them.

What it checks (issues #438, #439, #440, #441, #442, #443, #444, #638, #639, #640, #641):
  * [ignore], [should_panic], [should_panic("text")], [should_fail], and [timeout(ms)] decide pass/fail;
    --ignored and --include-ignored select ignored tests and the summary counts what was skipped;
  * [differential] compares the comptime interpreter's Ok/Err outcome with the compiled run, and a divergent
    fixture is reported as a failure with the two outcomes;
  * -j runs independent tests at once (a test that waits for another only finishes with -j 2), and the report
    keeps declaration order;
  * a test's stdout and stderr are captured and printed only when it fails, and --verbose always prints them;
  * every test runs in its own fresh working directory with a clean environment, and the runner removes every
    temporary directory it created;
  * -p and --workspace select packages, positional filters and --filter select tests by full path, --exact and
    --skip refine them, and dependency tests do not run by default (#638);
  * --list prints the selected tests and their attributes without running them, in the human and JSON formats (#639);
  * --format json streams events and --junit writes a JUnit XML report, and the human output does not change (#640);
  * --failed reruns the tests that failed before, --shard runs a deterministic slice, and --slow-threshold lists the
    tests that took long (#641).
"""

import json
import os
import shutil
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

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

    # --- name filters, --exact, and --skip (issue #638) -----------------------------

    selection = fixtures / "selection"
    all_paths = [
        "geometry.area",
        "geometry.perimeter",
        "geometry.shapes.area of circle",
        "alpha",
        "beta",
        "gamma fails",
        "delta panics",
        'escapes <markup> & "quotes"',
    ]
    markup_name = 'escapes <markup> & "quotes"'

    def expect_equal(description: str, actual, expected) -> None:
        global checks
        checks += 1
        if actual != expected:
            add_problem(f"{description}: got {actual!r}, expected {expected!r}")

    def json_lines(text: str) -> list:
        return [json.loads(line) for line in text.split("\n") if line]

    def listed_paths() -> list:
        """The test paths of a human `--list`: every line before the count."""
        return out.split("\n")[:-1]

    # The full path is `module.submodule.test name`, and a test in the entry file is just its name.
    run_driver(burdin, "test", str(selection), "--list")
    expect_status("selection (--list)", 0)
    expect_equal(
        "selection (--list)",
        out,
        "geometry.area\n"
        "geometry.perimeter [should_fail]\n"
        "geometry.shapes.area of circle\n"
        "alpha\n"
        "beta [ignore, timeout(500)]\n"
        "gamma fails\n"
        'delta panics [should_panic("boom")]\n'
        'escapes <markup> & "quotes"\n'
        "8 tests",
    )

    run_driver(burdin, "test", str(selection), "geometry", "-j", "2")
    expect_status("selection (substring)", 0)
    expect_equal("selection (substring)", out, "test result: 3 passed, 0 failed, 5 filtered out")

    # A module name is part of the path, so a filter can select one submodule.
    run_driver(burdin, "test", str(selection), "shapes", "-j", "2")
    expect_equal("selection (submodule)", out, "test result: 1 passed, 0 failed, 7 filtered out")

    # Several filters select the union; --filter is the same as a positional filter.
    run_driver(burdin, "test", str(selection), "alpha", "--filter", "circle", "-j", "2")
    expect_status("selection (two filters)", 0)
    expect_equal("selection (two filters)", out, "test result: 2 passed, 0 failed, 6 filtered out")

    run_driver(burdin, "test", str(selection), "--filter=alpha", "--list")
    expect_equal("selection (--filter alias)", out, "alpha\n1 test, 7 filtered out")

    # Matching is case-sensitive.
    run_driver(burdin, "test", str(selection), "ALPHA", "--list")
    expect_equal("selection (case)", out, "0 tests, 8 filtered out")

    run_driver(burdin, "test", str(selection), "area", "--exact", "--list")
    expect_equal("selection (--exact, partial)", out, "0 tests, 8 filtered out")

    run_driver(burdin, "test", str(selection), "geometry.area", "--exact", "-j", "2")
    expect_status("selection (--exact)", 0)
    expect_equal("selection (--exact)", out, "test result: 1 passed, 0 failed, 7 filtered out")

    run_driver(burdin, "test", str(selection), "geometry.shapes.area of circle", "gamma fails", "--exact", "--list")
    expect_equal(
        "selection (--exact, two paths)", out, "geometry.shapes.area of circle\ngamma fails\n2 tests, 6 filtered out"
    )

    run_driver(burdin, "test", str(selection), "--skip", "geometry", "--skip", "beta", "--list")
    expect_equal(
        "selection (--skip)",
        out,
        'alpha\ngamma fails\ndelta panics [should_panic("boom")]\nescapes <markup> & "quotes"\n4 tests, 4 filtered out',
    )

    run_driver(burdin, "test", str(selection), "area", "--skip", "circle", "--list")
    expect_equal("selection (filter and --skip)", out, "geometry.area\n1 test, 7 filtered out")

    # With --exact a --skip pattern must equal the path too, so a bare module name skips nothing.
    run_driver(burdin, "test", str(selection), "--skip", "geometry", "--exact", "--list")
    expect_equal("selection (--skip --exact)", out.split("\n")[-1], "8 tests")

    # The failing test is the only failure; the summary reports what the filters left out.
    run_driver(burdin, "test", str(selection), "--skip", "geometry", "-j", "2")
    expect_status("selection (--skip run)", 1)
    expect_contains("selection (--skip run)", "FAIL gamma fails", out)
    expect_contains("selection (--skip run)", "test result: 3 passed, 1 failed, 1 ignored, 3 filtered out", out)

    # A filter that matches nothing is not an error: status 0, a summary, and a note on stderr.
    run_driver(burdin, "test", str(selection), "no such test", "-j", "2")
    expect_status("selection (no match)", 0)
    expect_equal("selection (no match)", out, "test result: 0 passed, 0 failed, 8 filtered out")
    expect_contains("selection (no match)", "note: no test matches the filters", err)

    # Filters apply after discovery across packages, so -p, --workspace, and filters combine; with more than one
    # package the path starts with the package.
    run_driver(burdin, "test", str(workspace), "--workspace", "--list")
    expect_equal("workspace (--list)", out, "app::root one\napp::root two\nutil::lib one\n3 tests")

    run_driver(burdin, "test", str(workspace), "--workspace", "util::", "--list")
    expect_equal("workspace (package filter)", out, "util::lib one\n1 test, 2 filtered out")

    run_driver(burdin, "test", str(workspace), "-p", "util", "lib", "-j", "2")
    expect_status("workspace (-p and a filter)", 0)
    expect_equal("workspace (-p and a filter)", out, "test result: 1 passed, 0 failed")

    run_driver(burdin, "test", str(workspace), "--workspace", "app::root one", "--exact", "-j", "2")
    expect_equal("workspace (--exact)", out, "test result: 1 passed, 0 failed, 2 filtered out")

    run_driver(burdin, "test", str(workspace), "--workspace", "root", "--skip", "two", "--list")
    expect_equal("workspace (filter and --skip)", out, "app::root one\n1 test, 2 filtered out")

    # --- --list in the JSON format (issue #639) -------------------------------------

    run_driver(burdin, "test", str(selection), "--list", "--format", "json")
    expect_status("list json", 0)
    listed = json_lines(out)
    expect_equal("list json (paths)", [entry["path"] for entry in listed], all_paths)
    expect_equal(
        "list json (keys)", sorted(listed[0]), ["attributes", "file", "line", "module", "name", "package", "path"]
    )
    by_path = {entry["path"]: entry for entry in listed}
    circle = by_path["geometry.shapes.area of circle"]
    expect_equal(
        "list json (location)",
        (Path(circle["file"]).resolve(), circle["line"]),
        ((selection / "src" / "geometry" / "shapes.bd").resolve(), 1),
    )
    expect_equal("list json (module)", (circle["module"], circle["name"]), ("geometry.shapes", "area of circle"))
    expect_equal(
        "list json (entry file)",
        (by_path["gamma fails"]["module"], by_path["gamma fails"]["line"]),
        ("", 13),
    )
    expect_equal("list json (package)", {entry["package"] for entry in listed}, {"selection"})
    expect_equal(
        "list json (attributes)",
        by_path["delta panics"]["attributes"],
        {
            "ignore": False,
            "timeout_ms": None,
            "should_panic": True,
            "should_panic_message": "boom",
            "should_fail": False,
            "differential": False,
        },
    )
    expect_equal(
        "list json (ignore and timeout)",
        (by_path["beta"]["attributes"]["ignore"], by_path["beta"]["attributes"]["timeout_ms"]),
        (True, 500),
    )
    expect_equal("list json (should_fail)", by_path["geometry.perimeter"]["attributes"]["should_fail"], True)
    expect_not_contains("list json (nothing runs)", "test result", out)

    run_driver(burdin, "test", str(selection), "geometry.", "--list", "--format=json")
    expect_equal("list json (filter)", [entry["path"] for entry in json_lines(out)], all_paths[:3])

    run_driver(burdin, "test", str(workspace), "--workspace", "--list", "--format", "json")
    expect_equal(
        "list json (workspace)",
        [(entry["package"], entry["path"]) for entry in json_lines(out)],
        [("app", "app::root one"), ("app", "app::root two"), ("util", "util::lib one")],
    )

    # --list shows an ignored test too; --ignored narrows the list the way it narrows a run.
    run_driver(burdin, "test", str(selection), "--list", "--ignored")
    expect_equal("list (--ignored)", out, "beta [ignore, timeout(500)]\n1 test")

    # A list is nothing to run; an invalid package or a conflicting option is a usage error.
    run_driver(burdin, "test", str(workspace), "-p", "missing", "--list")
    expect_status("list (unknown package)", 2)
    run_driver(burdin, "test", str(selection), "--list", "--junit", str(work / "never.xml"))
    expect_status("list (--junit)", 2)
    expect_contains("list (--junit)", "cannot be used together", err)

    # --- the JSON event stream (issue #640) -----------------------------------------

    run_driver(burdin, "test", str(selection), "--format", "json", "-j", "1")
    expect_status("json events (status)", 1)
    events = json_lines(out)
    kinds = [event["event"] for event in events]
    expect_equal("json events (every line is an event)", len(events), len(out.split("\n")))
    expect_equal("json events (first)", (kinds[0], events[0]["version"]), ("suite_started", 1))
    expect_equal(
        "json events (suite_started)",
        {key: events[0][key] for key in ("packages", "test_count", "ignored", "filtered_out")},
        {"packages": ["selection"], "test_count": 8, "ignored": 1, "filtered_out": 0},
    )
    expect_equal("json events (last)", kinds[-1], "suite_finished")
    expect_equal(
        "json events (counts)",
        [kinds.count(kind) for kind in ("test_started", "test_passed", "test_failed", "test_ignored")],
        [7, 6, 1, 1],
    )
    # With -j 1 each test finishes before the next starts, in declaration order.
    started = [event["path"] for event in events if event["event"] == "test_started"]
    finished = [event["path"] for event in events if event["event"] in ("test_passed", "test_failed")]
    expect_equal("json events (order)", started, finished)
    expect_equal("json events (declaration order)", started, [path for path in all_paths if path != "beta"])
    failed_event = next(event for event in events if event["event"] == "test_failed")
    expect_equal(
        "json events (failed test)", (failed_event["path"], failed_event["line"], failed_event["reason"]), ("gamma fails", 13, None)
    )
    expect_contains("json events (failed stderr)", "assertion failed: assert(false)", failed_event["stderr"])
    expect_equal(
        "json events (durations)",
        all(isinstance(event["duration_ms"], int) for event in events if event["event"] in ("test_passed", "test_failed")),
        True,
    )
    passed_event = next(event for event in events if event["event"] == "test_passed")
    expect_equal("json events (a pass has no output by default)", "stdout" in passed_event, False)
    expect_equal(
        "json events (suite_finished)",
        {key: events[-1][key] for key in ("passed", "failed", "ignored", "filtered_out", "slow")},
        {"passed": 6, "failed": 1, "ignored": 1, "filtered_out": 0, "slow": []},
    )
    expect_not_contains("json events (no human report)", "test result", out)
    expect_not_contains("json events (no human report)", "FAIL ", out)

    run_driver(burdin, "test", str(selection), "--format", "json", "--verbose", "-j", "1", "markup")
    expect_status("json events (--verbose)", 0)
    markup_event = next(event for event in json_lines(out) if event["event"] == "test_passed")
    expect_equal("json events (--verbose stdout)", markup_event["stdout"], "<b>&</b>\n")
    expect_equal("json events (--verbose name)", markup_event["name"], markup_name)

    run_driver(burdin, "test", str(fixtures / "timeout"), "--format", "json", "-j", "1")
    expect_status("json events (timeout)", 1)
    timed_out = next(event for event in json_lines(out) if event["event"] == "test_failed")
    expect_equal("json events (timeout reason)", timed_out["reason"], "timed out after 100 ms")

    # A target that cannot be loaded prints its error on stderr and no events.
    run_driver(burdin, "test", str(fixtures / "does-not-exist"), "--format", "json")
    expect_status("json events (load failure)", 1)
    expect_equal("json events (load failure)", out, "")

    run_driver(burdin, "test", str(selection), "--format", "xml")
    expect_status("json events (unknown format)", 2)
    expect_contains("json events (unknown format)", "unknown test format `xml`", err)

    # --- JUnit XML (issue #640) -----------------------------------------------------

    junit = work / "report.xml"
    run_driver(burdin, "test", str(selection), "--junit", str(junit), "--verbose", "-j", "2")
    expect_status("junit (status)", 1)
    expect_contains("junit (human output is unchanged)", "FAIL gamma fails", out)
    expect_contains("junit (human output is unchanged)", "test result: 6 passed, 1 failed, 1 ignored", out)
    root = ET.parse(junit).getroot()
    expect_equal("junit (root)", root.tag, "testsuites")
    expect_equal(
        "junit (totals)",
        {key: root.get(key) for key in ("tests", "failures", "skipped")},
        {"tests": "8", "failures": "1", "skipped": "1"},
    )
    suites = root.findall("testsuite")
    expect_equal("junit (one suite per package)", [suite.get("name") for suite in suites], ["selection"])
    cases = {case.get("name"): case for case in suites[0].findall("testcase")}
    expect_equal(
        "junit (testcases)",
        sorted(cases),
        sorted(["area", "perimeter", "area of circle", "alpha", "beta", "gamma fails", "delta panics", markup_name]),
    )
    expect_equal("junit (classname)", cases["area of circle"].get("classname"), "selection.geometry.shapes")
    expect_equal("junit (entry classname)", cases["alpha"].get("classname"), "selection")
    expect_equal("junit (line)", cases["gamma fails"].get("line"), "13")
    expect_equal(
        "junit (file)", Path(cases["gamma fails"].get("file")).resolve(), (selection / "src" / "main.bd").resolve()
    )
    expect_equal("junit (time)", float(cases["alpha"].get("time")) >= 0.0, True)
    failure = cases["gamma fails"].find("failure")
    checks += 1
    if failure is None or "assertion failed: assert(false)" not in (failure.text or ""):
        add_problem("junit (failure): the failed test has no failure element with its stderr")
    expect_equal("junit (skipped)", cases["beta"].find("skipped") is not None, True)
    expect_equal("junit (a pass has no failure)", cases["alpha"].find("failure"), None)
    expect_equal("junit (--verbose system-out)", cases[markup_name].findtext("system-out"), "<b>&</b>\n")

    # Without --verbose a passing test carries no output; the failed one still does.
    run_driver(burdin, "test", str(selection), "--junit", str(junit), "-j", "2")
    cases = {case.get("name"): case for case in ET.parse(junit).getroot().iter("testcase")}
    expect_equal("junit (quiet pass)", cases[markup_name].find("system-out"), None)
    expect_equal("junit (failure output)", cases["gamma fails"].find("system-err") is not None, True)

    # A timeout is a failure with the reason as its message.
    run_driver(burdin, "test", str(fixtures / "timeout"), "--junit", str(junit), "-j", "2")
    expect_status("junit (timeout)", 1)
    timeout_cases = {case.get("name"): case for case in ET.parse(junit).getroot().iter("testcase")}
    expect_equal(
        "junit (timeout message)", timeout_cases["hangs"].find("failure").get("message"), "timed out after 100 ms"
    )

    # The file is written next to the JSON stream, and only the selected tests are in it.
    run_driver(burdin, "test", str(selection), "alpha", "--junit", str(junit), "--format", "json")
    expect_status("junit (with json)", 0)
    expect_equal("junit (with json)", ET.parse(junit).getroot().get("tests"), "1")

    run_driver(burdin, "test", str(selection), "alpha", "--junit", str(work / "missing-dir" / "report.xml"))
    expect_status("junit (unwritable)", 2)
    expect_contains("junit (unwritable)", "cannot write", err)

    # --- rerun failures (issue #641) ------------------------------------------------

    rerun = work / "rerun"
    shutil.copytree(selection, rerun, ignore=shutil.ignore_patterns("target"))
    record = rerun / "target" / "burdin" / "debug" / "test-failed.txt"

    # With no record there is nothing to narrow by: every test runs, and the failure is recorded.
    run_driver(burdin, "test", str(rerun), "--failed", "-j", "2")
    expect_status("failed (no record)", 1)
    expect_contains("failed (no record)", "no record of a previous run", err)
    expect_contains("failed (no record)", "test result: 6 passed, 1 failed, 1 ignored", out)
    expect_equal("failed (record)", record.read_text().replace("\r", "").split("\n")[1:], ["selection\tgamma fails", ""])

    run_driver(burdin, "test", str(rerun), "--failed", "--list")
    expect_equal("failed (--list)", out, "gamma fails\n1 test, 7 filtered out")

    run_driver(burdin, "test", str(rerun), "--failed", "-j", "2")
    expect_status("failed (rerun)", 1)
    expect_contains("failed (rerun)", "FAIL gamma fails", out)
    expect_equal("failed (rerun)", out.split("\n")[-1], "test result: 0 passed, 1 failed, 7 filtered out")

    # A run that does not touch the failed test keeps it in the record.
    run_driver(burdin, "test", str(rerun), "alpha", "-j", "2")
    expect_status("failed (unrelated run)", 0)
    run_driver(burdin, "test", str(rerun), "--failed", "--list")
    expect_equal("failed (an unrelated run keeps the record)", out, "gamma fails\n1 test, 7 filtered out")

    # --failed combines with the name filters and --skip.
    run_driver(burdin, "test", str(rerun), "--failed", "--skip", "gamma", "--list")
    expect_equal("failed (--skip)", out, "0 tests, 8 filtered out")

    # Fixing the test removes it from the record; a later --failed has nothing to rerun.
    main_file = rerun / "src" / "main.bd"
    main_file.write_text(main_file.read_text().replace("assert(false)?", "assert(true)?"))
    run_driver(burdin, "test", str(rerun), "--failed", "-j", "2")
    expect_status("failed (fixed)", 0)
    expect_equal("failed (fixed)", out, "test result: 1 passed, 0 failed, 7 filtered out")
    expect_equal("failed (fixed record)", record.read_text().replace("\r", ""), "burdin-test-failed 1\n")

    run_driver(burdin, "test", str(rerun), "--failed", "-j", "2")
    expect_status("failed (nothing failed)", 0)
    expect_contains("failed (nothing failed)", "the previous run had no failed tests", err)
    expect_equal("failed (nothing failed)", out, "test result: 0 passed, 0 failed, 8 filtered out")

    # The record belongs to the profile that ran, and a single file has no package directory to keep one in.
    expect_equal("failed (other profiles are untouched)", (rerun / "target" / "burdin" / "release").exists(), False)
    run_driver(burdin, "test", str(harness.TESTS / "m10_test_runner" / "all_pass.bd"), "--failed")
    expect_status("failed (single file)", 2)
    expect_contains("failed (single file)", "needs a package directory", err)

    # A green suite with no record leaves nothing behind.
    clean = work / "clean"
    shutil.copytree(fixtures / "parallel", clean, ignore=shutil.ignore_patterns("target"))
    run_driver(burdin, "test", str(clean), "-j", "2")
    expect_equal("failed (a green run writes nothing)", (clean / "target").exists(), False)

    # --- shards (issue #641) --------------------------------------------------------

    shard_paths = []
    for index in (1, 2, 3):
        run_driver(burdin, "test", str(selection), "--shard", f"{index}/3", "--list", "--format", "json")
        expect_status(f"shard {index}/3", 0)
        shard_paths.append([entry["path"] for entry in json_lines(out)])
        run_driver(burdin, "test", str(selection), f"--shard={index}/3", "--list", "--format", "json")
        expect_equal(f"shard {index}/3 (deterministic)", [entry["path"] for entry in json_lines(out)], shard_paths[-1])
    expect_equal("shards (partition)", sorted(sum(shard_paths, [])), sorted(all_paths))
    expect_equal("shards (round robin)", shard_paths[0], [all_paths[0], all_paths[3], all_paths[6]])

    run_driver(burdin, "test", str(selection), "--shard", "2/2", "-j", "2")
    expect_status("shard (run)", 1)
    expect_equal("shard (run)", out.split("\n")[-1], "test result: 3 passed, 1 failed, 4 filtered out")

    # A shard of one is the whole suite, and shards are taken after the name filters.
    run_driver(burdin, "test", str(selection), "--shard", "1/1", "--list")
    expect_equal("shard (1/1)", out.split("\n")[-1], "8 tests")
    run_driver(burdin, "test", str(selection), "geometry", "--shard", "2/2", "--list")
    expect_equal("shard (after filters)", out, "geometry.perimeter [should_fail]\n1 test, 7 filtered out")

    for bad in ("0/2", "3/2", "1/0", "2", "x/y", "-1/2"):
        run_driver(burdin, "test", str(selection), "--shard", bad)
        expect_status(f"shard ({bad})", 2)
        expect_contains(f"shard ({bad})", "`--shard` needs `<i>/<n>`", err)

    # --- slow tests (issue #641) ----------------------------------------------------

    slow = fixtures / "slow"
    run_driver(burdin, "test", str(slow), "--slow-threshold", "100", "-j", "2")
    expect_status("slow (threshold)", 0)
    expect_contains("slow (threshold)", "slow test: sleeper (", out)
    expect_not_contains("slow (threshold)", "slow test: quick", out)
    expect_equal("slow (the summary stays last)", out.split("\n")[-1], "test result: 2 passed, 0 failed")

    run_driver(burdin, "test", str(slow), "-j", "2")
    expect_not_contains("slow (default threshold)", "slow test", out)

    run_driver(burdin, "test", str(slow), "--slow-threshold", "0", "-j", "2")
    expect_not_contains("slow (disabled)", "slow test", out)

    run_driver(burdin, "test", str(slow), "--slow-threshold", "100", "--format", "json", "-j", "2")
    slow_list = json_lines(out)[-1]["slow"]
    expect_equal("slow (json)", [entry["path"] for entry in slow_list], ["sleeper"])
    expect_equal("slow (json duration)", slow_list[0]["duration_ms"] >= 100, True)

    run_driver(burdin, "test", str(slow), "--slow-threshold", "soon")
    expect_status("slow (bad threshold)", 2)
    expect_contains("slow (bad threshold)", "milliseconds", err)

    # The new options belong to `burdin test`.
    run_driver(burdin, "build", str(selection), "--list")
    expect_status("options (--list on build)", 2)
    expect_contains("options (--list on build)", "only valid for `burdin test`", err)

print(f"Checked {checks} runner behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
