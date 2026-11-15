#!/usr/bin/env python3
"""Checks that the parallel front end (issue #432) is deterministic: a package with many modules builds and runs
with any BURDIN_JOBS value, and a package with errors reports byte-identical diagnostics whether the modules
are parsed sequentially (BURDIN_JOBS=1) or in parallel.

Needs a built driver. BURDIN selects it, like tests/run_tests.py.
"""

import difflib
import os
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()


def make_package(directory, count: int, body: str) -> bool:
    try:
        source = directory / "src"
        source.mkdir(parents=True)
        (directory / "burdin.toml").write_bytes(b'[package]\nname = "app"\n')
        uses = ""
        terms = []
        for index in range(count):
            name = f"m{index:02d}"
            harness.write_text(source / f"{name}.bd", f"fn f(): i32 {{\n    {body}\n}}\n")
            uses += f"\nuse {name}"
            terms.append(f"{name}.f()")
        harness.write_text(
            source / "main.bd",
            f"{uses}\n\nfn main() {{\n    println(({' + '.join(terms)}).to_string())\n}}\n",
        )
        return True
    except OSError:
        return False


with harness.temporary_dir("burdin-par-") as work:
    # --- A large package builds and runs with any number of workers ---------------
    if make_package(work / "good", 24, "1"):
        for jobs in (1, 2, 8):
            checks += 1
            env = {**os.environ, "BURDIN_JOBS": str(jobs)}
            result = harness.run([burdin, "run", "good"], cwd=work, env=env)
            output = harness.text(result.stdout).rstrip("\n")
            if result.returncode != 0:
                detail = "\n".join(harness.text(result.stderr).split("\n")[:5])
                add_problem(f"BURDIN_JOBS={jobs}: exit status {result.returncode}: {detail}")
            elif output != "24":
                add_problem(f"BURDIN_JOBS={jobs}: printed '{output}', expected '24'")
    else:
        add_problem("could not create the multi-module package")

    # --- Diagnostics are identical in both modes ---------------------------------
    # Every module has its own unknown name, so each contributes one E0002 at the same line.
    if make_package(work / "bad", 12, "missing_name"):
        sequential = harness.run(
            [burdin, "build", "bad"], cwd=work, env={**os.environ, "BURDIN_JOBS": "1"}
        )
        parallel = harness.run(
            [burdin, "build", "bad"], cwd=work, env={**os.environ, "BURDIN_JOBS": "8"}
        )
        checks += 1
        if sequential.returncode == 0 or parallel.returncode == 0:
            add_problem(
                "the failing package compiled "
                f"(sequential {sequential.returncode}, parallel {parallel.returncode})"
            )
        checks += 1
        if sequential.stderr != parallel.stderr:
            diff = difflib.unified_diff(
                harness.text(sequential.stderr).split("\n"),
                harness.text(parallel.stderr).split("\n"),
                fromfile="sequential",
                tofile="parallel",
                lineterm="",
            )
            detail = "\n".join(list(diff)[:20])
            add_problem(f"diagnostics differ between sequential and parallel parsing: {detail}")
        checks += 1
        sequential_errors = harness.count_lines_containing(sequential.stderr, b"error[E0002]")
        if sequential_errors < 12:
            add_problem("the sequential run did not report every module's error")
        checks += 1
        parallel_errors = harness.count_lines_containing(parallel.stderr, b"error[E0002]")
        if parallel_errors != sequential_errors:
            add_problem("the parallel run reported a different number of errors")
    else:
        add_problem("could not create the failing package")

print(f"Checked parallel analysis determinism ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
