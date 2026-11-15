#!/usr/bin/env python3
"""Checks that the compiler front end keeps its focused unit tests (issue #45).

It does not compile anything. Usage: tests/check_unit_tests.py

`cargo test --workspace` in CI is what runs the unit tests. This script guards the
structure that step depends on:
  * every front-end crate (diag, syntax, tir, sema, check) has a test module with at
    least one #[test];
  * the four components issue #45 calls out keep their tests: the lexer token stream,
    parser recovery, type unification, and exhaustiveness;
  * every CI job runs the workspace unit tests.
"""

import sys
from pathlib import Path

import harness

problems = []


def add_problem(message: str) -> None:
    problems.append(message)


def has_unit_tests(path: Path) -> bool:
    if not path.is_file():
        return False
    content = harness.read_text(path)
    return "#[cfg(test)]" in content and "#[test]" in content


def crate_has_unit_tests(crate: str) -> bool:
    source = harness.ROOT / "crates" / crate / "src"
    if not source.is_dir():
        return False
    return any(has_unit_tests(path) for path in sorted(source.rglob("*.rs")) if path.is_file())


for crate in ("burdin_diag", "burdin_syntax", "burdin_tir", "burdin_sema", "burdin_check"):
    if not (harness.ROOT / "crates" / crate).is_dir():
        add_problem(f"crates/{crate}: missing")
    elif not crate_has_unit_tests(crate):
        add_problem(f"crates/{crate}: no unit tests (#[cfg(test)] module with a #[test])")

# The components issue #45 names. A test elsewhere in the crate does not cover these.
for relative in (
    "crates/burdin_syntax/src/lexer.rs",
    "crates/burdin_syntax/src/parser/mod.rs",
    "crates/burdin_sema/src/generics.rs",
    "crates/burdin_sema/src/exhaust.rs",
):
    if not has_unit_tests(harness.ROOT / relative):
        add_problem(f"{relative}: missing unit tests")

workflow = harness.ROOT / ".github" / "workflows" / "ci.yml"
if not workflow.is_file():
    add_problem(".github/workflows/ci.yml: missing")
else:
    text = harness.read_text(workflow)
    for job in ("linux", "macos", "windows"):
        block = harness.workflow_job_block(text, job)
        if not block.strip():
            add_problem(f"ci.yml: no {job} job")
        elif "cargo test --workspace" not in block:
            add_problem(f"ci.yml: {job} job: does not run cargo test --workspace")

if harness.report_problems(problems, blank_line=False):
    sys.exit(1)
print("Unit tests: every front-end crate and every CI job is covered.")
