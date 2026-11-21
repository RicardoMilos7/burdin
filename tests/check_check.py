#!/usr/bin/env python3
"""Checks `burdin check`, the type-check-only command (stage 3, issue #237). Needs a built driver.
Usage: tests/check_check.py [--bless]

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks (spec/check.md has the rules, CK1-CK8):
  * every fixture under tests/check/ (a `.bd` file, or a package under tests/check/packages/) reports exactly the
    diagnostics (file, line, code) its `//~ E0000` markers say, with the exit status its `//% exit:` directive says
    (default: 1 when it has markers, otherwise 0). A fixture without markers proves that nothing is reported, and
    that a clean check prints nothing at all;
  * the whole output of the failing fixtures (short format, no color) equals the golden file in tests/check/golden/
    (`--bless` rewrites the goldens from the driver's output; review the diff before committing it);
  * the diagnostics of `burdin check` are the diagnostics of `burdin build`, line for line, for every failing
    fixture, and for every `compile-fail` conformance test (with `--tests` for `test-compile-fail`) the diagnostics
    are exactly the test's `//~` markers; every conformance test that builds, runs, or passes `burdin test` is
    accepted (exit 0). That is the contract: `check` stops exactly where code generation would start;
  * no code generation and no linking: a clean check succeeds with a linker that does not exist, writes no file,
    and rejects the options that only a build has (`-o`, `--emit-ir`, `--link-lib`);
  * `--release`, `--tests`, `--comptime-steps`, the machine-readable output (JSON and SARIF), and the command line
    (usage errors exit 2, a missing target exits 1, `--help` exits 0);
  * the JSON result (CK12): every line validates against docs/diagnostic.schema.json, the last line is the check
    summary with its counts, `files`, and `elapsed_ms` (masked in the golden), a clean program prints the summary
    alone, `--explain` adds no text trace, and the human format is unchanged.

Fixture directives are comment lines at the top of a `.bd` file:
    //% args: --comptime-steps 1000      extra command line arguments
    //% exit: 1                          the expected exit status
    //% stderr: some text                text the output must contain
"""

import json
import os
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import harness
import jsonschema_lite
import run_tests

problems = []
checks = 0

CHECK = harness.TESTS / "check"
GOLDEN = CHECK / "golden"
DIAGNOSTIC = re.compile(r"^(.+?):([0-9]+):([0-9]+): (warning|error)\[(E[0-9]{4})\]")
MARKER = re.compile(r"//~ ((?:E[0-9]{4}[ ]*)+)")
COMMON = ["--no-color", "--diagnostic-format=short"]
BLESS = "--bless" in sys.argv[1:]

# The packages and what each mode of checking them must do: (extra arguments, exit status, whether the diagnostics
# are the `//~` markers, text the output must contain).
PACKAGES = {
    "lib_tests": [
        ([], 1, False, "package `lib_tests` has no entry point"),
        (["--tests"], 0, False, None),
    ],
    "tests_error": [
        ([], 0, False, None),
        (["--tests"], 1, True, None),
    ],
    "multi_module": [
        ([], 1, True, None),
        (["--tests"], 1, True, None),
    ],
    "two_errors": [
        ([], 1, True, None),
        (["--tests"], 1, True, None),
    ],
    # A module with syntax errors: the modules that parse are checked, against the declarations the broken module
    # read, and the follow-on errors of its missing declarations are not reported (spec/check.md, CK4).
    "syntax_neighbors": [
        ([], 1, True, None),
    ],
    "syntax_no_main": [
        ([], 1, True, None),
    ],
}


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)


def driver(command, args, cwd, env=None):
    """Runs `burdin <command> <args>`; the merged output with carriage returns removed, and the exit status."""
    result = harness.run([burdin, command, *args], cwd=cwd, env=env, merge=True)
    return harness.text(harness.normalize(result.stdout)), result.returncode


def scrub(output: str) -> str:
    """The output with the fixtures' absolute directory removed and `/` separators, so a golden is portable."""
    output = output.replace("\\", "/")
    for base in {CHECK.as_posix(), CHECK.resolve().as_posix()}:
        output = output.replace(base + "/", "")
    return output


def diagnostics(output: str, severity: str = "error"):
    found = set()
    for line in output.split("\n"):
        match = DIAGNOSTIC.match(scrub(line))
        if match and match.group(4) == severity:
            found.add((match.group(1), int(match.group(2)), match.group(5)))
    return found


def markers(path: Path):
    """The `//~` markers of a source file as (path relative to tests/check, line, code)."""
    found = set()
    relative = path.relative_to(CHECK).as_posix()
    for number, line in enumerate(harness.read_text(path).split("\n"), 1):
        match = MARKER.search(line)
        if match:
            for code in match.group(1).split():
                found.add((relative, number, code))
    return found


def directives(path: Path):
    """`//% key: value` lines at the top of a fixture."""
    found = {}
    for line in harness.read_text(path).split("\n"):
        match = re.match(r"^//% ([a-z]+): ?(.*)$", line)
        if match:
            found.setdefault(match.group(1), []).append(match.group(2))
    return found


def compare_golden(name: str, output: str) -> None:
    golden = GOLDEN / (name + ".txt")
    output = scrub(output)
    if BLESS:
        GOLDEN.mkdir(exist_ok=True)
        golden.write_text(output, encoding="utf-8", newline="\n")
        return
    check(golden.is_file(), f"{name}: no golden file {golden.relative_to(harness.ROOT).as_posix()} (run --bless)")
    if golden.is_file():
        want = harness.read_text(golden)
        check(output == want, f"{name}: the output differs from its golden file:\n--- got\n{output}--- want\n{want}")


def check_fixture(target: Path, args, want_exit, want_markers, want_text, name):
    output, status = driver("check", [*COMMON, *args, target.relative_to(CHECK).as_posix()], CHECK)
    check(status == want_exit, f"{name}: exit status {status}, expected {want_exit}\n{output}")
    if want_markers is not None:
        got = diagnostics(output)
        check(
            got == want_markers,
            f"{name}: diagnostics differ from the //~ markers\n  expected: {sorted(want_markers)}\n"
            f"  actual:   {sorted(got)}",
        )
    if want_text:
        check(want_text in scrub(output), f"{name}: the output does not contain: {want_text}")
    if want_exit == 1:
        compare_golden(name, output)
    return output


# ----- the fixtures ---------------------------------------------------------------------------------------------

sources = sorted(CHECK.glob("*.bd"))
check(len(sources) >= 8, f"tests/check has only {len(sources)} single-file fixtures")
for path in sources:
    found = directives(path)
    expected = markers(path)
    args = " ".join(found.get("args", [])).split()
    want_exit = int(found["exit"][0]) if "exit" in found else (1 if expected else 0)
    text = found.get("stderr", [None])[0]
    output = check_fixture(path, args, want_exit, expected, text, path.stem)
    if want_exit == 0 and text is None:
        check(output == "", f"{path.name}: a clean check printed output:\n{output}")

    # The diagnostics are the build's: same lines, same order, from the same front end.
    if want_exit == 1:
        built, build_status = driver("build", [*COMMON, *args, "-o", "never-built", path.name], CHECK)
        check(build_status == 1, f"{path.name}: `burdin build` exited {build_status}, expected 1")
        want_lines = [scrub(l) for l in output.split("\n") if DIAGNOSTIC.match(scrub(l))]
        got_lines = [scrub(l) for l in built.split("\n") if DIAGNOSTIC.match(scrub(l))]
        check(want_lines == got_lines, f"{path.name}: `check` and `build` report different diagnostics")
        check(not (CHECK / "never-built").exists(), f"{path.name}: a failed build left an output file")

# A constant that needs more steps than the budget: fine with the default budget, E0225 with a small one.
output, status = driver("check", [*COMMON, "budget.bd"], CHECK)
check(status == 0 and output == "", f"budget.bd: with the default budget, exit {status} and output {output!r}")

for name, modes in sorted(PACKAGES.items()):
    package = CHECK / "packages" / name
    expected = set()
    for path in sorted(package.rglob("*.bd")):
        expected |= markers(path)
    for args, want_exit, use_markers, text in modes:
        label = name + (".tests" if "--tests" in args else "")
        output = check_fixture(package, args, want_exit, expected if use_markers else set(), text, label)
        if want_exit == 0:
            check(output == "", f"{label}: a clean check printed output:\n{output}")

# ----- ownership annotations and step-by-step explanations (issue #587) -----------------------------------------

OWNERSHIP = CHECK / "ownership"
EXPLAIN = CHECK / "explain"


def check_ownership_fixture(path: Path) -> None:
    """`--explain-ownership` prints the fixture's functions with their move, copy, borrow, drop, drop-flag, call
    convention, and clone annotations; the golden holds the whole output."""
    name = path.stem
    output, status = driver(
        "check", [*COMMON, "--explain-ownership", path.relative_to(CHECK).as_posix()], CHECK
    )
    check(status == 0, f"ownership/{name}: exit {status}, expected 0\n{output}")
    compare_golden("ownership_" + name, output)


ownership_fixtures = sorted(OWNERSHIP.glob("*.bd"))
check(len(ownership_fixtures) >= 5, f"tests/check/ownership has only {len(ownership_fixtures)} fixtures")
for path in ownership_fixtures:
    check_ownership_fixture(path)

# `--fn` names one function; the annotated source has that function only.
output, status = driver(
    "check", [*COMMON, "--explain-ownership", "--fn", "main", "ownership/moves.bd"], CHECK
)
check(status == 0, f"`--fn main`: exit {status}, expected 0\n{output}")
headers = [line for line in output.splitlines() if line.startswith("fn ")]
check(len(headers) == 1, f"--fn main printed {len(headers)} functions, expected one")
compare_golden("ownership_fn_main", output)

# `--format json` lists the events with their spans for other tools.
output, status = driver("check", ["--explain-ownership", "--format", "json", "ownership/drops.bd"], CHECK)
check(status == 0, f"`--format json`: exit {status}, expected 0\n{output}")
try:
    functions = json.loads(output)
except json.JSONDecodeError:
    functions = None
    check(False, f"`--format json` is not one JSON document: {output[:400]}")
if functions is not None:
    check(isinstance(functions, list) and functions, "`--format json` lists no functions")
    kinds = {event.get("kind") for function in functions for event in function.get("events", [])}
    check({"move", "drop", "drop_flag"} <= kinds, f"the JSON events miss kinds: {sorted(kinds)}")
    check(
        all(isinstance(event.get("span", {}).get("byte_start"), int) for f in functions for event in f.get("events", [])),
        "a JSON event has no span offsets",
    )
compare_golden("ownership_drops_json", output)

# Every memory error code carries a `--explain` trace with its rule ID and the suggested fix.
explain_fixtures = sorted(EXPLAIN.glob("*.bd"))
check(len(explain_fixtures) >= 9, f"tests/check/explain has only {len(explain_fixtures)} fixtures")
for path in explain_fixtures:
    code = path.stem
    output, status = driver("check", [*COMMON, "--explain", path.relative_to(CHECK).as_posix()], CHECK)
    check(status == 1, f"explain/{code}: exit {status}, expected 1\n{output}")
    # The trace header repeats the location and code; it is not a second diagnostic.
    found = {diagnostic for diagnostic in diagnostics(output) if not diagnostic[0].startswith("trace")}
    check(found == markers(path), f"explain/{code}: diagnostics differ from the //~ markers\n{output}")
    check(f"trace for " in output, f"explain/{code}: no trace")
    check(f"rule: spec/memory-rules.md " in output, f"explain/{code}: no rule ID")
    check("fix: " in output, f"explain/{code}: no suggested fix")
    compare_golden("explain_" + code, output)

# The human diagnostic itself carries the one-line pointer (the short format does not).
output, status = driver("check", ["--no-color", "explain/E0100.bd"], CHECK)
check(status == 1 and "run `burdin check --explain`" in output, f"the human diagnostic has no explanation note\n{output}")

# Ownership options need their command, and the usage says so.
for bad in (["--fn", "main", "ok_program.bd"], ["--format", "json", "ok_program.bd"], ["--format", "x", "--explain-ownership", "ok_program.bd"]):
    output, status = driver("check", bad, CHECK)
    check(status == 2 and "usage:" in output, f"`burdin check {' '.join(bad)}` exited {status}, expected 2")

# ----- the same front end as a build: the conformance suite -----------------------------------------------------

TESTS = harness.TESTS


def conformance_cases():
    cases = []
    platform = run_tests.host_platform()
    for root in run_tests.discover(TESTS):
        lines = run_tests.normalize(root.read_bytes()).split(b"\n")
        found = [line[4:] for line in lines if line.startswith(b"//@ ")]
        kind = run_tests.kind_of(found)
        skip = {v.decode() for raw in run_tests.directive_values(found, b"skip-platform") for v in raw.split()}
        if kind in ("undecided", "unknown") or platform in skip:
            continue
        cases.append((root, kind, found, run_tests.target_of(root, TESTS), run_tests.expected_markers(lines)))
    return cases


def check_conformance_case(case):
    root, kind, found, target, expected = case
    args = ["--no-color", "--diagnostic-format=short"]
    if kind in ("burdin-test", "test-compile-fail"):
        args.append("--tests")
    if run_tests.has_directive(found, b"release-only"):
        args.append("--release")
    output, status = driver("check", [*args, str(target)], harness.ROOT)
    rel = root.relative_to(TESTS).as_posix()
    if kind in ("compile-fail", "test-compile-fail"):
        # The runner's rule: only the diagnostics in the test's own root file count.
        got = set()
        for line in output.split("\n"):
            if root.name + ":" not in line:
                continue
            match = re.search(r":([0-9]+):[0-9]+: error\[(E[0-9]{4})\]", line)
            if match:
                got.add((int(match.group(1)), match.group(2)))
        if status != 1 or got != expected:
            return f"{rel}: exit {status}; diagnostics {sorted(got)}, markers {sorted(expected)}"
    elif status != 0:
        return f"{rel}: expected the check to pass, exit {status}\n{output[:600]}"
    return None


cases = conformance_cases()
check(len(cases) > 500, f"only {len(cases)} conformance tests found")
with ThreadPoolExecutor(max_workers=min(8, os.cpu_count() or 1)) as pool:
    for failure in pool.map(check_conformance_case, cases):
        check(failure is None, f"conformance test: {failure}")

# ----- no code generation, no linking ---------------------------------------------------------------------------

with harness.temporary_dir("burdin-check-") as work:
    program = CHECK / "ok_program.bd"
    env = dict(os.environ, BURDIN_CC=str(work / "no-such-linker"))
    output, status = driver("check", [str(program)], work, env=env)
    check(status == 0 and output == "", f"a clean check with no linker: exit {status}, output {output!r}")
    check(list(work.iterdir()) == [], f"a clean check wrote files: {[p.name for p in work.iterdir()]}")
    package = CHECK / "packages" / "lib_tests"
    output, status = driver("check", ["--tests", str(package)], work, env=env)
    check(status == 0 and list(work.iterdir()) == [], "checking a package wrote files or needed a linker")
    for option in (["-o", "out"], ["--emit-ir", "out.ll"], ["--link-lib", "m"], ["--lto"], ["-g"], ["--strip"]):
        output, status = driver("check", [*option, str(program)], work)
        check(status == 2, f"`burdin check {' '.join(option)}` exited {status}, expected 2")
        check("unknown option" in output, f"`burdin check {' '.join(option)}` did not say the option is unknown")

# ----- options and the command line -----------------------------------------------------------------------------

output, status = driver("check", [*COMMON, "--release", "ok_program.bd"], CHECK)
check(status == 0 and output == "", f"`--release` on a clean program: exit {status}, output {output!r}")

# ----- the machine-readable result (spec/check.md, CK12; issue #787) ------------------------------------------

SUMMARY_KEYS = ["type", "version", "errors", "warnings", "shown", "limit", "files", "elapsed_ms"]
SCHEMA = json.loads(harness.read_text(harness.ROOT / "docs" / "diagnostic.schema.json"))


def elapsed_masked(output: str) -> str:
    """The output with the one field that differs between runs replaced, so the whole output can be a golden."""
    return re.sub(r'"elapsed_ms":[0-9]+', '"elapsed_ms":0', output)


def json_objects(name: str, output: str):
    """Every line of a JSON stream, parsed; each line must be JSON and must validate against the schema (DG7)."""
    objects = []
    for line in output.split("\n"):
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            add_problem(f"{name}: a line is not JSON: {line[:200]!r}")
            continue
        problems_found = jsonschema_lite.validate(obj, SCHEMA)
        check(not problems_found, f"{name}: a line does not match the schema: {problems_found[:3]}\n{line}")
        objects.append(obj)
    return objects


def check_summary(name: str, output: str, want: dict):
    """The last line is the one check summary, with the documented fields in order and the counts `want` gives."""
    objects = json_objects(name, output)
    summaries = [obj for obj in objects if obj.get("type") == "check-summary"]
    check(len(summaries) == 1, f"{name}: {len(summaries)} check summaries, expected one")
    if not objects or objects[-1].get("type") != "check-summary":
        add_problem(f"{name}: the check summary is not the last line:\n{output}")
        return None
    summary = objects[-1]
    check(list(summary) == SUMMARY_KEYS, f"{name}: the summary's fields are {list(summary)}")
    for key, value in want.items():
        check(summary.get(key) == value, f"{name}: {key} is {summary.get(key)!r}, expected {value!r}")
    check(
        isinstance(summary.get("elapsed_ms"), int) and summary["elapsed_ms"] >= 0,
        f"{name}: elapsed_ms is {summary.get('elapsed_ms')!r}",
    )
    return objects


# A clean program: the summary alone, and exit 0.
output, status = driver("check", ["--diagnostic-format=json", "ok_program.bd"], CHECK)
check(status == 0, f"json, clean program: exit {status}, expected 0")
check(
    check_summary("json, clean program", output, {"errors": 0, "warnings": 0, "shown": 0, "limit": None, "files": 1})
    is not None
    and len([line for line in output.split("\n") if line]) == 1,
    "json, clean program: the output is not the summary alone",
)

# A warning does not fail the check: it is one diagnostic, the summary counts it, and the exit status is 0.
output, status = driver("check", ["--diagnostic-format=json", "ok_warning.bd"], CHECK)
check(status == 0, f"json, warning-only program: exit {status}, expected 0")
objects = check_summary("json, warning-only program", output, {"errors": 0, "warnings": 1, "shown": 1, "limit": None, "files": 1})
if objects is not None:
    check(
        [obj.get("severity") for obj in objects[:-1]] == ["warning"],
        "json, warning-only program: the diagnostic is not one warning",
    )

# Four errors in one file: four diagnostic lines, then the summary with the counts; the exit status is 1.
output, status = driver("check", ["--diagnostic-format=json", "err_types.bd"], CHECK)
check(status == 1, f"json, failing program: exit {status}, expected 1")
objects = check_summary("json, failing program", output, {"errors": 4, "warnings": 0, "shown": 4, "limit": None, "files": 1})
if objects is not None:
    check(
        [obj.get("type") for obj in objects] == ["diagnostic"] * 4 + ["check-summary"],
        "json, failing program: the lines are not four diagnostics and the summary",
    )
    check(
        all(obj.get("severity") == "error" for obj in objects[:-1]),
        "json, failing program: a diagnostic is not an error",
    )
compare_golden("json_err_types", elapsed_masked(output))

# `--error-limit` hides diagnostics, not counts: the summary still counts all four, and says how many were shown.
output, status = driver("check", ["--diagnostic-format=json", "--error-limit", "2", "err_types.bd"], CHECK)
check(status == 1, f"json, --error-limit: exit {status}, expected 1")
check_summary("json, --error-limit", output, {"errors": 4, "warnings": 0, "shown": 2, "limit": 2, "files": 1})

# A package: `files` counts each module on disk, two here.
package = CHECK / "packages" / "two_errors"
output, status = driver("check", ["--diagnostic-format=json", package.relative_to(CHECK).as_posix()], CHECK)
check(status == 1, f"json, package: exit {status}, expected 1")
package_files = len(list((package / "src").rglob("*.bd")))
check(
    check_summary("json, package", output, {"errors": 2, "warnings": 0, "shown": 2, "limit": None, "files": package_files})
    is not None,
    "json, package: the summary is wrong",
)

# `--explain` prints a text trace, which would break the JSON lines: in JSON the trace is left out.
output, status = driver("check", ["--diagnostic-format=json", "--explain", "explain/E0100.bd"], CHECK)
check(status == 1 and "trace for" not in output, "json, --explain: a text trace was printed")
check_summary("json, --explain", output, {"errors": 1, "warnings": 0, "shown": 1, "limit": None, "files": 1})

# The human format is unchanged: a warning prints as before, and no summary line appears.
output, status = driver("check", ["--no-color", "ok_warning.bd"], CHECK)
check(status == 0 and "warning[E0215]" in output, "human, warning-only program: the warning is not printed")
check("check-summary" not in output and "could not check" not in output, "human output has a summary")

# A failure before the checks is a plain line: no JSON at all.
output, status = driver("check", ["--diagnostic-format=json", "no-such-file.bd"], CHECK)
check(status == 1 and "cannot open" in output, f"json, a missing target: exit {status}, output {output!r}")
check(not any(line.startswith("{") for line in output.split("\n")), "json, a missing target: a JSON line was printed")

# The schema describes the summary: a summary missing a field, or with an extra one, is rejected.
valid = {"type": "check-summary", "version": 1, "errors": 0, "warnings": 0, "shown": 0, "limit": None, "files": 1, "elapsed_ms": 0}
check(not jsonschema_lite.validate(valid, SCHEMA), "the schema rejects a valid check summary")
for name, broken in {
    "a check summary without files": {k: v for k, v in valid.items() if k != "files"},
    "a check summary with an extra field": {**valid, "surprise": 1},
    "a check summary with a negative time": {**valid, "elapsed_ms": -1},
}.items():
    check(bool(jsonschema_lite.validate(broken, SCHEMA)), f"the schema accepts {name}")

# The documented contract names the summary and its exit statuses.
check(
    '"check-summary"' in harness.read_text(harness.ROOT / "docs" / "diagnostics.md")
    and "check-summary" in harness.read_text(harness.ROOT / "README.md")
    and "CK12" in harness.read_text(harness.ROOT / "spec" / "check.md"),
    "the docs do not describe the check summary",
)

# `--error-limit` caps what is shown and says so; the exit status is still 1.
output, status = driver("check", [*COMMON, "--error-limit", "2", "err_types.bd"], CHECK)
check(status == 1 and len(diagnostics(output)) == 2, "`--error-limit 2` did not show exactly two errors")
check("2 more errors not shown" in output, "`--error-limit 2` did not say how many errors are hidden")

# SARIF: a check is a build-like CI gate, so it writes one log even when the package is clean and nothing follows it
# (spec/diagnostics.md, DG11).
output, status = driver("check", ["--diagnostic-format=sarif", "ok_program.bd"], CHECK)
check(status == 0, f"sarif on a clean program: exit {status}, expected 0")
try:
    log = json.loads(output)
except json.JSONDecodeError:
    log = None
    check(False, f"a clean sarif log is not one JSON document: {output[:400]}")
if log is not None:
    check(log.get("version") == "2.1.0", f"sarif version is {log.get('version')!r}")
    check(log["runs"][0]["results"] == [], "a clean sarif log reports results")
output, status = driver("check", ["--diagnostic-format=sarif", "err_move.bd"], CHECK)
check(status == 1, f"sarif on a failing program: exit {status}, expected 1")
try:
    results = json.loads(output)["runs"][0]["results"]
except (json.JSONDecodeError, KeyError, IndexError):
    results = None
    check(False, f"the failing sarif output is not one log: {output[:400]}")
if results is not None:
    check(len(results) == 1, f"the sarif log has {len(results)} results, expected 1")
    check(results[0].get("ruleId") == "E0104", f"the sarif result names {results[0].get('ruleId')!r}, expected E0104")
check("could not check" not in output, "the sarif log is followed by a summary")

# Human output names the way to explain a code; the short format does not.
output, status = driver("check", ["--no-color", "err_move.bd"], CHECK)
check("burdin explain <code>" in output, "human output does not point to `burdin explain`")
check("could not check err_move.bd (1 error)" in output, "human output does not end with the summary line")

# A missing target is an error of the package, not of the usage.
output, status = driver("check", ["no-such-file.bd"], CHECK)
check(status == 1 and "cannot open" in output, f"a missing target: exit {status}, output {output!r}")

# Usage errors exit 2 with the usage; `--help` prints it and exits 0.
for bad, reason in [
    ([], "missing target file"),
    (["a.bd", "b.bd"], "unexpected argument `b.bd`"),
    (["--bogus", "a.bd"], "unknown option `--bogus`"),
    (["--release=yes", "a.bd"], "takes no value"),
    (["--comptime-steps", "0", "a.bd"], "positive number of steps"),
    (["--diagnostic-format", "xml", "a.bd"], "unknown diagnostic format"),
]:
    output, status = driver("check", bad, CHECK)
    check(status == 2, f"`burdin check {' '.join(bad)}` exited {status}, expected 2")
    check(reason in output and "usage:" in output, f"`burdin check {' '.join(bad)}` did not say: {reason}")
output, status = driver("check", ["--help"], CHECK)
check(status == 0 and "burdin check <file.bd | package directory>" in output, "`burdin check --help` is wrong")

fixture_count = (
    len(sources)
    + 1
    + sum(len(modes) for modes in PACKAGES.values())
    + len(ownership_fixtures)
    + len(explain_fixtures)
)
print(f"Checked `burdin check` ({checks} checks, {fixture_count} fixtures, {len(cases)} conformance tests)")

if BLESS:
    print("Goldens rewritten; review the diff.")
    sys.exit(0)
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
