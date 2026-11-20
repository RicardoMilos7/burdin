#!/usr/bin/env python3
"""Checks `-q/--quiet` and `-v/--verbose` on `burdin build`, `burdin run`, and `burdin check` (issue #799,
`spec/build-tool.md`, BT8).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

The fixture packages are under tests/verbosity/: `clean` prints a line, `warn` compiles with one warning (E0215),
`bad` has that warning and one error (E0002), and `units` has eight functions, so `BURDIN_CODEGEN_UNITS=4` splits
it into four codegen units. Each run copies the fixtures into a fresh directory, so no cached executable is reused
unless a check asks for one.

What it checks:
  * the help of `build`, `run`, and `check` lists `--quiet` and `--verbose`;
  * `-q` on success prints nothing (warnings are left out) and on failure prints the errors exactly as the golden
    files in tests/verbosity/golden/ say, for `build` and `check`, in human, JSON, and SARIF formats;
  * `run -q` leaves the program's output alone;
  * `-v` prints `compiling`, each phase in the same order with a time, and the codegen units in unit order, with the
    same sequence for `-j 1` and `-j 8`; a failed compilation stops after the phase that failed;
  * a fresh cached executable says so in place of codegen and link;
  * `-q` with `-v`, `-q` on `test`, `-v` on `fuzz`, and a value given to `--quiet` are usage errors (exit status 2).
"""

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
FIXTURES = Path(__file__).resolve().parent / "verbosity"
GOLDEN = FIXTURES / "golden"

# The phases of a compiled build, in the order `-v` prints them.
PHASES = ["dependencies", "load", "build scripts", "parse and check", "codegen", "link"]
PHASE_LINE = re.compile(r"^phase ([a-z ]+): \d+\.\d{2} ms$")
UNIT_LINE = re.compile(r"^  codegen unit (\d+) of (\d+): \d+\.\d{2} ms$")


def clean_env(**extra):
    """The environment without the variables that change the output: the width, the job count, and the unit split."""
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("COLUMNS", "BURDIN_JOBS", "BURDIN_CODEGEN_UNITS", "NO_COLOR", "FORCE_HYPERLINK")
    }
    env.update(extra)
    return env


def fresh_fixture(work: Path, name: str, label: str) -> str:
    """Copies fixture `name` into `work` under `label` and returns the directory name to pass to burdin."""
    shutil.copytree(FIXTURES / name, work / label)
    return label


def normalize(text: str, work: Path) -> str:
    """The output with the temporary directory replaced by `<work>`, the way the golden files spell it."""
    # The longest spelling first: on macOS the temporary directory is `/var/...`, which resolves to `/private/var/...`,
    # and replacing the short form first would leave `/private<work>`.
    spellings = {str(work), str(work.resolve()), str(work.resolve()).replace("\\", "/")}
    for spelling in sorted(spellings, key=len, reverse=True):
        text = text.replace(spelling, "<work>")
    return text.replace("\\", "/")


def golden(name: str) -> str:
    return (GOLDEN / name).read_text(encoding="utf-8")


def phases_of(stderr: str) -> list:
    return [m.group(1) for line in stderr.split("\n") if (m := PHASE_LINE.match(line))]


def units_of(stderr: str) -> list:
    return [(int(m.group(1)), int(m.group(2))) for line in stderr.split("\n") if (m := UNIT_LINE.match(line))]


with harness.temporary_dir("burdin-verbosity-") as work:
    # --- The help lists the two flags of each command --------------------------------------------------
    for command in ("build", "run", "check"):
        result = harness.run([burdin, command, "--help"], env=clean_env())
        text = harness.text(result.stdout)
        check(result.returncode == 0, f"`burdin {command} --help` exited {result.returncode}")
        check("-q, --quiet" in text and "-v, --verbose" in text, f"`burdin {command} --help` lacks -q or -v")

    # --- -q on success: nothing is printed, and the warning is left out -------------------------------
    warn = fresh_fixture(work, "warn", "warn")
    control = harness.run([burdin, "build", warn, "-o", work / "control", "--color", "never"], cwd=work, env=clean_env())
    check("warning[E0215]" in harness.text(control.stderr), "the control build did not print the warning")
    quiet = harness.run([burdin, "build", warn, "-o", work / "quiet", "-q"], cwd=work, env=clean_env())
    check(quiet.returncode == 0, f"`build -q` of a warning-only package exited {quiet.returncode}")
    check(quiet.stdout == b"" and quiet.stderr == b"", f"`build -q` printed output: {quiet.stderr!r}")
    check(harness.executable_in(work, "quiet").is_file(), "`build -q` wrote no executable")

    checkq = harness.run([burdin, "check", warn, "-q"], cwd=work, env=clean_env())
    check(checkq.returncode == 0 and checkq.stderr == b"", f"`check -q` of a warning-only package printed {checkq.stderr!r}")

    # --- -q on failure: the errors, exactly as the golden files say ---------------------------------
    bad = fresh_fixture(work, "bad", "bad")
    failed = harness.run(
        [burdin, "build", bad, "-o", work / "bad-out", "-q", "--color", "never", "--width", "80"],
        cwd=work,
        env=clean_env(),
    )
    check(failed.returncode == 1, f"`build -q` of the failing package exited {failed.returncode}")
    stderr = normalize(harness.text(failed.stderr), work)
    check(stderr == golden("build-bad-quiet.txt"), f"`build -q` errors are not the golden text:\n{stderr}")
    check("warning" not in stderr, "`build -q` printed a warning next to the errors")

    failed_check = harness.run(
        [burdin, "check", bad, "-q", "--color", "never", "--width", "80"], cwd=work, env=clean_env()
    )
    stderr = normalize(harness.text(failed_check.stderr), work)
    check(failed_check.returncode == 1, f"`check -q` of the failing package exited {failed_check.returncode}")
    check(stderr == golden("check-bad-quiet.txt"), f"`check -q` errors are not the golden text:\n{stderr}")

    # The machine formats keep their structure: JSON carries only the error, SARIF a log with no warning result.
    as_json = harness.run(
        [burdin, "build", bad, "-o", work / "json-out", "-q", "--diagnostic-format", "json"],
        cwd=work,
        env=clean_env(),
    )
    text = harness.text(as_json.stderr)
    check('"code":"E0002"' in text, "`build -q --diagnostic-format json` lost the error")
    check("E0215" not in text and '"severity":"warning"' not in text, "`-q` JSON still carries the warning")
    as_sarif = harness.run(
        [burdin, "build", warn, "-o", work / "sarif-out", "-q", "--diagnostic-format", "sarif"],
        cwd=work,
        env=clean_env(),
    )
    text = harness.text(as_sarif.stderr)
    check(as_sarif.returncode == 0, f"`build -q --diagnostic-format sarif` exited {as_sarif.returncode}")
    check('"results":[]' in text, "the `-q` SARIF log of a warning-only package has results")
    check('"ruleId":"E0215"' not in text, "`-q` SARIF still lists the warning as a result")
    as_sarif_bad = harness.run(
        [burdin, "build", bad, "-o", work / "sarif-bad", "-q", "--diagnostic-format", "sarif"],
        cwd=work,
        env=clean_env(),
    )
    text = harness.text(as_sarif_bad.stderr)
    check('"ruleId":"E0002"' in text and '"ruleId":"E0215"' not in text, "`-q` SARIF of the failing package is wrong")

    # --- run -q keeps the program's output ------------------------------------------------------------
    # Each scenario that builds gets its own copy: a build leaves its executable in the package's target directory,
    # and the next build of the same package reuses it (spec/target.md).
    ran_dir = fresh_fixture(work, "clean", "clean-run")
    ran = harness.run([burdin, "run", ran_dir, "-q"], cwd=work, env=clean_env())
    check(ran.returncode == 0, f"`run -q` exited {ran.returncode}")
    check(ran.stdout == b"hello\n" and ran.stderr == b"", f"`run -q`: stdout {ran.stdout!r}, stderr {ran.stderr!r}")

    # --- -v on success: each phase once, in order, and the codegen units after the codegen phase ----
    clean = fresh_fixture(work, "clean", "clean")
    verbose = harness.run([burdin, "build", clean, "-o", work / "verbose-out", "-v"], cwd=work, env=clean_env())
    stderr = harness.text(verbose.stderr)
    lines = stderr.split("\n")
    check(verbose.returncode == 0, f"`build -v` exited {verbose.returncode}")
    check(verbose.stdout == b"", "`build -v` printed on standard output")
    check(lines[0] == f"compiling {clean}", f"`build -v` does not start with the compiling line: {lines[0]!r}")
    check(phases_of(stderr) == PHASES, f"`build -v` phases are {phases_of(stderr)}, expected {PHASES}")
    check(units_of(stderr) == [(1, 1)], f"`build -v` of an unsplit program: units {units_of(stderr)}")

    # A second build of the same package reuses the executable and says so.
    cached = harness.run([burdin, "build", clean, "-o", work / "verbose-out2", "-v"], cwd=work, env=clean_env())
    stderr = harness.text(cached.stderr)
    check(cached.returncode == 0, f"the cached `build -v` exited {cached.returncode}")
    check(
        any(line.startswith("phase cache: ") for line in stderr.split("\n")),
        f"a fresh cached executable is not reported: {stderr!r}",
    )
    check(phases_of(stderr) == ["dependencies", "load", "build scripts", "parse and check"], "cached phases are wrong")

    # `check -v` has the front end's phases and no codegen.
    check_dir = fresh_fixture(work, "clean", "clean-check")
    check_verbose = harness.run([burdin, "check", check_dir, "-v"], cwd=work, env=clean_env())
    stderr = harness.text(check_verbose.stderr)
    check(check_verbose.returncode == 0, f"`check -v` exited {check_verbose.returncode}")
    check(
        phases_of(stderr) == ["dependencies", "load", "build scripts", "parse and check"],
        f"`check -v` phases are {phases_of(stderr)}",
    )

    # `run -v` prints the same phases on standard error and the program's output on standard output.
    run_dir = fresh_fixture(work, "clean", "clean-run-verbose")
    ran_verbose = harness.run([burdin, "run", run_dir, "-v"], cwd=work, env=clean_env())
    check(ran_verbose.stdout == b"hello\n", f"`run -v` changed the program's output: {ran_verbose.stdout!r}")
    check(phases_of(harness.text(ran_verbose.stderr)) == PHASES, "`run -v` does not print the phases of a run")

    # --- -v on failure: the phases up to the failure, and the errors -------------------------------
    failed_verbose = harness.run([burdin, "build", bad, "-o", work / "bad-verbose", "-v"], cwd=work, env=clean_env())
    stderr = harness.text(failed_verbose.stderr)
    check(failed_verbose.returncode == 1, f"`build -v` of the failing package exited {failed_verbose.returncode}")
    check(phases_of(stderr) == PHASES[:4], f"`build -v` of a failing package: phases {phases_of(stderr)}")
    check("error[E0002]" in stderr and "phase codegen" not in stderr, "the failing `build -v` output is wrong")

    # --- codegen units: forced into four, the same sequence for every -j ---------------------------
    sequences = {}
    for jobs in ("1", "8"):
        units_dir = fresh_fixture(work, "units", f"units-{jobs}")
        result = harness.run(
            [burdin, "build", units_dir, "-o", work / f"units-{jobs}-out", "-v", "-j", jobs],
            cwd=work,
            env=clean_env(BURDIN_CODEGEN_UNITS="4"),
        )
        stderr = harness.text(result.stderr)
        check(result.returncode == 0, f"`build -v -j {jobs}` of the units package exited {result.returncode}")
        sequences[jobs] = units_of(stderr)
        check(phases_of(stderr) == PHASES, f"`build -v -j {jobs}` phases are {phases_of(stderr)}")
        check(
            sequences[jobs] == [(1, 4), (2, 4), (3, 4), (4, 4)],
            f"`build -v -j {jobs}` units are {sequences[jobs]}, expected four in unit order",
        )
        ran_units = harness.run([harness.executable_in(work, f"units-{jobs}-out")], cwd=work)
        check(harness.text(ran_units.stdout).strip() == "108", f"the split units program (-j {jobs}) printed the wrong result")
    check(sequences.get("1") == sequences.get("8"), "codegen unit lines depend on -j")

    # --- usage errors ------------------------------------------------------------------------------
    usage = [
        (["build", clean, "-q", "-v"], "conflict"),
        (["check", clean, "-q", "-v"], "conflict"),
        (["test", clean, "-q"], "only valid for `burdin build`"),
        (["bench", clean, "-q"], "only valid for `burdin build`"),
        (["fuzz", clean, "target", "-v"], "only valid for `burdin build`"),
        (["build", clean, "--quiet=yes"], "takes no value"),
    ]
    for argv, expected in usage:
        # Only `build` takes -o; the other commands must reject the flag they do not know, not the one under test.
        extra = ["-o", "usage-out"] if argv[0] == "build" else []
        result = harness.run([burdin, *argv, *extra], cwd=work, env=clean_env())
        stderr = harness.text(result.stderr)
        check(result.returncode == 2, f"`burdin {' '.join(argv)}` exited {result.returncode}, expected 2")
        check(expected in stderr, f"`burdin {' '.join(argv)}` did not say `{expected}`: {stderr[:200]!r}")

if problems:
    for problem in problems:
        print(f"FAIL: {problem}")
    print(f"{len(problems)} of {checks} checks failed")
    sys.exit(1)
print(f"verbosity: {checks} checks passed")
