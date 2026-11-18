#!/usr/bin/env python3
"""Checks the black-box behavior of `burdin bench` (spec/bench.md, issues #251, #653, #654, #655): measurement,
statistics, the report formats, selection, baselines, and the profiler hook. Needs a built driver.
Usage: tests/check_bench.py

BURDIN selects the driver, like tests/run_tests.py. The fixtures under tests/bench/ are ordinary packages with
no `//@` directives, so the conformance runner ignores them.

What it checks:
  * a bench is measured, its statistics line has the documented shape, and the summary counts it
    (warmup, calibration, samples; `black_box` keeps the work measured, BN4-BN6);
  * `--warmup`, `--sample-count`, and `--sample-time` reach the harness: the JSON event carries exactly that many
    samples, and a bench never runs longer than its `--timeout`;
  * `--list` prints the selected benches, in the human and JSON formats (BN7);
  * `--format json` streams the documented events with the documented fields, and the human report is unchanged;
  * positional filters, `--filter`, `--exact`, and `--skip` select by full path, and a filter that matches
    nothing exits 0 with a note (BN8);
  * `-p` and `--workspace` select packages and qualify the displayed names; dependency benches do not run by
    default (BN2);
  * a panicking bench fails with its captured output, a hanging bench is killed at `--timeout`, and a compile
    error in a bench body fails the run (BN12, BN13);
  * `--save-baseline` writes the file, `--baseline` compares and reports the wording, a hand-written tiny baseline
    with `--fail-on-regression` fails the run, and missing or single-file baselines are usage errors (BN11);
  * `--profile-time` runs one bench for the given seconds and refuses none or several (BN10);
  * bench-only options are rejected for every other command, and `--release` is the default (BN3).
"""

import json
import os
import re
import shutil
import sys
import time
from pathlib import Path

import harness

problems = []
checks = 0


def check(ok: bool, message: str) -> None:
    global checks
    checks += 1
    if not ok:
        problems.append(message)


burdin = harness.require_burdin()
fixtures = harness.TESTS / "bench"

STATS_LINE = re.compile(
    r"^(?P<display>.+): (?P<median>[0-9.]+) (?P<unit>ns|us|ms|s) "
    r"\(mean [0-9.]+ \w+, ci 95% [0-9.]+ \w+ to [0-9.]+ \w+, mad [0-9.]+ \w+, [0-9]+/[0-9]+ outliers\)$"
)


def run(*args, cwd=None, timeout=None):
    return harness.run([burdin, *args], cwd=cwd or Path.cwd(), timeout=timeout)


def bench(*args, target=None, cwd=None, timeout=None):
    """Runs `burdin bench` and returns (status, stdout, stderr)."""
    result = run("bench", str(target or fixtures / "basic"), *args, cwd=cwd, timeout=timeout)
    return (
        result.returncode,
        harness.text(harness.normalize(result.stdout)).rstrip("\n"),
        harness.text(harness.normalize(result.stderr)).rstrip("\n"),
    )


with harness.temporary_dir("burdin-bench-") as work:
    # A copy of the fixtures keeps the baseline files of the run out of the repository's fixture trees; the
    # packages in tests/bench are also written to directly where the check only reads them.
    for name in ("basic", "panics", "hangs", "broken", "deps", "single.bd"):
        source = fixtures / name
        copied = work / name
        if source.is_dir():
            shutil.copytree(source, copied)
        else:
            shutil.copy2(source, copied)
    basic = work / "basic"

    # --- measurement and the human report ------------------------------------------------------------------------

    # The small knobs keep the whole script quick; every check that measures uses them.
    SMALL = ["--warmup", "30", "--sample-count", "5", "--sample-time", "5"]
    status, out, err = bench(*SMALL, target=basic)
    check(status == 0, f"the basic run failed: exit {status}\n{err}")
    lines = out.split("\n")
    stats = [line for line in lines if STATS_LINE.match(line)]
    check(len(stats) == 4, f"the run printed {len(stats)} statistics lines, expected 4:\n{out}")
    check(
        any(line.startswith("counts: ") for line in stats),
        f"a statistics line does not name its bench:\n{out}",
    )
    check("bench result: 4 measured, 0 failed" in out, f"the summary does not count every bench:\n{out}")
    check("FAIL" not in out, f"a passing bench was reported as failed:\n{out}")

    # The package's main never runs in bench mode (BN2).
    check("never runs in bench mode" not in out, "the package's main ran in bench mode")

    # A bench that prints is tolerated: its output is captured and shown only with --verbose (BN6).
    check("a chatty bench prints" not in out, "a passing bench's stdout was printed without --verbose")
    status, out, err = bench(*SMALL, "--verbose", target=basic)
    check(status == 0, f"`--verbose` failed: exit {status}\n{err}")
    check(
        '---- bench "chatty" stdout ----' in out and "a chatty bench prints" in out,
        f"`--verbose` does not print the captured output:\n{out}",
    )

    # --- the harness protocol -------------------------------------------------------------------------------------

    # The JSON event carries exactly the samples asked for, one iteration at least, and positive totals.
    status, out, err = bench(*SMALL, "--format", "json", target=basic)
    events = [json.loads(line) for line in out.split("\n") if line]
    finished = [e for e in events if e.get("event") == "bench_finished"]
    check(status == 0 and len(finished) == 4, f"the JSON stream did not finish four benches: exit {status}\n{err}")
    for event in finished:
        name = event["name"]
        samples = event.get("samples")
        check(isinstance(samples, list) and len(samples) == 5, f"{name}: the event lists {len(samples)} samples")
        check(event.get("iters", 0) >= 1, f"{name}: the event has no iterations")
        check(
            all(isinstance(total, int) and total > 0 for total in samples or []),
            f"{name}: a sample total is not a positive integer",
        )
        for field in ("median_ns", "mean_ns", "ci_low_ns", "ci_high_ns", "mad_ns"):
            check(field in event and isinstance(event[field], float), f"{name}: the event lacks {field}")
        check(event.get("outliers", -1) <= len(samples or []), f"{name}: more outliers than samples")
        check(event.get("reason") is None, f"{name}: a passing bench carries a reason: {event.get('reason')}")
        check("stdout" not in event and "stderr" not in event, f"{name}: a passing bench carries captured output")
    started = [e for e in events if e.get("event") == "suite_started"]
    check(len(started) == 1 and started[0].get("version") == 1, "no versioned suite_started event")
    check(started[0].get("bench_count") == 4, "suite_started does not count the benches")
    summary = [e for e in events if e.get("event") == "suite_finished"]
    check(
        len(summary) == 1 and summary[0].get("measured") == 4 and summary[0].get("failed") == 0,
        f"the JSON summary disagrees with the run: {summary}",
    )
    order = [e["name"] for e in events if e.get("event") == "bench_started"]
    check(
        order == ["area", "counts", "empty body", "chatty"],
        f"the benches are not reported in discovery order: {order}",
    )

    # A bench never runs longer than its timeout (BN12); a generous one still finishes.
    status, out, err = bench("--timeout", "30000", *SMALL, target=basic)
    check(status == 0, f"a generous timeout failed the run: exit {status}\n{err}")

    # --- --list ----------------------------------------------------------------------------------------------------

    status, out, err = bench("--list", target=basic)
    check(status == 0, f"`--list` failed: exit {status}\n{err}")
    check(
        out.split("\n") == ["geometry.shapes.area", "counts", "empty body", "chatty", "4 benches"],
        f"the human listing is not the four full paths and a count:\n{out}",
    )

    status, out, err = bench("--list", "--format", "json", target=basic)
    rows = [json.loads(line) for line in out.split("\n") if line]
    check(status == 0 and len(rows) == 4, f"the JSON listing is not four objects: exit {status}\n{err}")
    by_path = {row["path"]: row for row in rows}
    check(
        set(by_path) == {"geometry.shapes.area", "counts", "empty body", "chatty"},
        f"the JSON listing paths are wrong: {sorted(by_path)}",
    )
    area = by_path.get("geometry.shapes.area", {})
    check(
        area.get("module") == "geometry.shapes" and area.get("name") == "area",
        f"the module path is wrong: {area}",
    )
    check(area.get("line") == 2 and area.get("file", "").endswith("shapes.bd"), f"the site is wrong: {area}")

    # --- selection ------------------------------------------------------------------------------------------------

    status, out, err = bench("--list", "geo", target=basic)
    check(status == 0 and out.split("\n")[:1] == ["geometry.shapes.area"], f"a positional filter did not select:\n{out}")
    check("1 bench" in out and "3 filtered out" in out, f"the filtered-out count is wrong:\n{out}")

    status, out, err = bench("--list", "--exact", "geometry.shapes.area", target=basic)
    check(status == 0 and "1 bench" in out, f"`--exact` with the whole path did not select:\n{out}")

    status, out, err = bench("--list", "--exact", "geometry", target=basic)
    check(status == 0 and out.split("\n")[-1] == "0 benches, 4 filtered out", f"`--exact` on a prefix selected:\n{out}")

    status, out, err = bench("--list", "--skip", "chatty", "--skip", "empty", target=basic)
    check(status == 0 and out.split("\n")[-1] == "2 benches, 2 filtered out", f"`--skip` did not remove:\n{out}")

    status, out, err = bench("--list", "no such bench", target=basic)
    check(status == 0, f"a filter that matches nothing must exit 0, got {status}")
    check(out.split("\n")[-1] == "0 benches, 4 filtered out", f"a no-match listing is wrong:\n{out}")
    check("no bench matches the filters" in err, f"the no-match note did not go to stderr:\n{err}")

    # --- packages -------------------------------------------------------------------------------------------------

    app = work / "deps" / "app"
    status, out, err = bench(*SMALL, target=app)
    check(status == 0 and "app bench:" in out and "bench result: 1 measured, 0 failed" in out,
          f"the default run did not measure only the entry package:\n{out}")

    status, out, err = bench(*SMALL, "-p", "lib", target=app)
    check(status == 0 and "lib bench:" in out and "app bench:" not in out,
          f"`-p lib` did not measure only the dependency:\n{out}")

    status, out, err = bench(*SMALL, "--workspace", target=app)
    check(status == 0 and "app::app bench:" in out and "lib::lib bench:" in out,
          f"`--workspace` did not qualify both packages' benches:\n{out}")

    status, out, err = bench("--list", "-p", "nope", target=app)
    check(status == 2 and "no package named `nope`" in err, f"an unknown package must be a usage error:\n{err}")

    # --- failure paths --------------------------------------------------------------------------------------------

    status, out, err = bench(*SMALL, target=work / "panics")
    check(status == 1, f"a panicking bench must fail the run, got exit {status}")
    check("FAIL panics on purpose: the bench was terminated by a signal" in out, f"the panic was not reported:\n{out}")
    check('---- bench "panics on purpose" stderr ----' in out and "panic: boom" in out,
          f"the panic output was not captured:\n{out}")
    check("still measured:" in out, f"a good bench next to a failure was not measured:\n{out}")
    check("bench result: 1 measured, 1 failed" in out, f"the failure summary is wrong:\n{out}")

    status, out, err = bench("--timeout", "2000", *SMALL, target=work / "hangs")
    check(status == 1, f"a hanging bench must fail the run, got exit {status}")
    check("FAIL hangs: timed out after 2000 ms" in out, f"the timeout was not reported:\n{out}")
    check("bench result: 0 measured, 1 failed" in out, f"the timeout summary is wrong:\n{out}")

    status, out, err = bench(target=work / "broken")
    check(status == 1, f"a compile error must fail the run, got exit {status}")
    check("error[E0003]" in err and "main.bd:4" in err, f"the bench body error was not reported:\n{err}")

    # A single .bd file is a one-module package; its bench has no module in its path.
    status, out, err = bench("--list", target=work / "single.bd")
    check(status == 0 and out.split("\n")[:1] == ["only bench"], f"a single-file target did not list:\n{err}")

    # --- baselines ------------------------------------------------------------------------------------------------

    status, out, err = bench(*SMALL, "--save-baseline", "nightly", target=basic)
    check(status == 0, f"saving a baseline failed: exit {status}\n{err}")
    saved = basic / "target" / "burdin" / "bench" / "nightly.json"
    check(saved.is_file(), f"the baseline file was not written: {saved}")
    if saved.is_file():
        data = json.loads(saved.read_text())
        check(data.get("version") == 1, "the baseline has no version 1")
        check(
            set(data.get("benches", {})) == {"geometry.shapes.area", "counts", "empty body", "chatty"},
            f"the baseline is keyed by the wrong paths: {sorted(data.get('benches', {}))}",
        )
        entry = data["benches"]["geometry.shapes.area"]
        check("median_ns" in entry and "samples" in entry and "iters" in entry, f"the entry lacks its numbers: {entry}")

    status, out, err = bench(*SMALL, "--baseline", "nightly", target=basic)
    check(status == 0, f"comparing with the fresh baseline failed: exit {status}\n{err}")
    check(out.count("baseline nightly:") == 4, f"every bench needs its comparison line:\n{out}")
    check("new (not in the baseline)" not in out, f"a saved bench was reported as new:\n{out}")

    # A tiny hand-written baseline: three of the benches are slower beyond noise by any standard, so the run
    # fails. The empty bench is left out on purpose: it measures about nothing, so against a tiny baseline it can
    # be faster, and a bench missing from the file is `new`, which never fails a run (BN11).
    tiny = basic / "target" / "burdin" / "bench" / "tiny.json"
    tiny.parent.mkdir(parents=True, exist_ok=True)
    entries = {path: {"iters": 1, "samples": [1], "median_ns": 0.001, "mean_ns": 0.001,
                      "ci_low_ns": 0.001, "ci_high_ns": 0.001, "mad_ns": 0.0}
               for path in ("geometry.shapes.area", "counts", "chatty")}
    tiny.write_text(json.dumps({"version": 1, "benches": entries}))
    status, out, err = bench(*SMALL, "--baseline", "tiny", "--fail-on-regression", "50", target=basic)
    check(status == 1, f"a regressed run must exit 1, got {status}")
    check(out.count("FAIL") == 3 and "regressed over the baseline tiny" in out, f"the regressions were not reported:\n{out}")
    # The missing bench was still measured, so it counts as measured, not failed.
    check("bench result: 1 measured, 3 failed" in out, f"the regression summary is wrong:\n{out}")
    check("new (not in the baseline)" in out, f"the missing bench was not reported as new:\n{out}")

    # Without the gate, the same comparison is only a report.
    status, out, err = bench(*SMALL, "--baseline", "tiny", target=basic)
    check(status == 0, f"a regressed run without the gate must exit 0, got {status}")
    check("slower (beyond noise)" in out and "FAIL" not in out, f"the comparison wording is wrong:\n{out}")

    # A missing baseline, a wrong version, and a single-file target are usage errors.
    status, out, err = bench("--baseline", "never-saved", target=basic)
    check(status == 2 and "no baseline named `never-saved`" in err, f"a missing baseline must be exit 2:\n{err}")

    wrong = basic / "target" / "burdin" / "bench" / "wrong.json"
    wrong.write_text('{"version": 7, "benches": {}}')
    status, out, err = bench("--baseline", "wrong", target=basic)
    check(status == 2 and "version 7" in err, f"an unknown baseline version must be refused:\n{err}")

    status, out, err = bench("--save-baseline", "x", target=work / "single.bd")
    check(status == 2 and "not a single file" in err, f"a single-file baseline must be exit 2:\n{err}")

    # `--fail-on-regression` needs a baseline to compare with.
    status, out, err = bench("--fail-on-regression", "5", target=basic)
    check(status == 2 and "needs `--baseline <name>`" in err, f"the gate without a baseline must be exit 2:\n{err}")

    # --- --profile-time -------------------------------------------------------------------------------------------

    started = time.monotonic()
    status, out, err = bench("counts", "--profile-time", "1", target=basic)
    took = time.monotonic() - started
    check(status == 0 and out == "profiled counts for 1 s", f"the profile run is wrong:\n{out}\n{err}")
    check(took >= 0.9, f"the profile run stopped early: {took:.2f}s")

    status, out, err = bench("--profile-time", "1", target=basic)
    check(status == 2 and "several benches" in err, f"profiling several benches must be exit 2:\n{err}")

    status, out, err = bench("no such bench", "--profile-time", "1", target=basic)
    check(status == 2 and "no bench" in err, f"profiling no bench must be exit 2:\n{err}")

    status, out, err = bench("counts", "--profile-time", "0", target=basic)
    check(status == 2, f"a zero profile time must be exit 2, got {status}")

    # --- usage errors ---------------------------------------------------------------------------------------------

    # Bench-only options are rejected for the other commands.
    for option, value in (("--save-baseline", "x"), ("--baseline", "x"), ("--fail-on-regression", "5"),
                          ("--profile-time", "1"), ("--warmup", "10"), ("--sample-count", "3"),
                          ("--sample-time", "2")):
        result = run("build", str(basic), option, value)
        check(
            result.returncode == 2 and "only valid for `burdin bench`" in harness.text(result.stderr),
            f"`burdin build {option} {value}` was not rejected",
        )
    result = run("test", str(basic), "--warmup", "10")
    check(
        result.returncode == 2 and "only valid for `burdin bench`" in harness.text(result.stderr),
        "`burdin test --warmup` was not rejected",
    )

    # Test-only options are rejected for `burdin bench`, and the shared ones are accepted.
    for option in ("--ignored", "--failed", "--shard 1/2", "--junit out.xml", "--slow-threshold 5"):
        argv = [burdin, "bench", str(basic), *option.split()]
        result = harness.run(argv, cwd=Path.cwd())
        check(
            result.returncode == 2 and "only valid for `burdin test`" in harness.text(result.stderr),
            f"`burdin bench {option}` was not rejected",
        )
    for option in ("--exact", "--skip x", "--format json", "--timeout 100"):
        argv = [burdin, "bench", "--list", str(basic), *option.split()]
        result = harness.run(argv, cwd=Path.cwd())
        check(result.returncode == 0, f"`burdin bench --list {option}` was rejected:\n{harness.text(result.stderr)}")

    for bad, message in (("--sample-count", "0"), ("--warmup", "soon"), ("--profile-time", "-1"),
                         ("--fail-on-regression", "-2")):
        result = run("bench", str(basic), bad, message)
        check(result.returncode == 2, f"`burdin bench {bad} {message}` was not rejected")

    result = run("bench", str(basic), "--", "an argument")
    check(
        result.returncode == 2 and "no program arguments" in harness.text(result.stderr),
        "`burdin bench -- <args>` was not rejected",
    )

    # `--release` is the default and changes nothing (BN3).
    plain = run("bench", "--list", str(basic))
    released = run("bench", "--list", str(basic), "--release")
    check(plain.returncode == 0 and released.returncode == 0, "`--release` was not accepted")
    check(plain.stdout == released.stdout, "`--release` changed the bench build")

print(f"Checked {checks} bench behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
