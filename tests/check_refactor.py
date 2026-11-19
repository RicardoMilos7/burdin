#!/usr/bin/env python3
"""Checks `burdin refactor` (stage 3, issue #613; spec/refactor.md, RF7-RF14).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). Each case is a package under
tests/refactor/<case>/before; the expected result is tests/refactor/<case>/after.
BURDIN_UPDATE_GOLDEN=1 rewrites the `after` trees from the current driver.

What it checks, for each refactoring:
  * the refactoring changes the package into the `after` tree, the result passes `burdin check`, and `--dry-run`
    prints the diff and writes nothing (RF7);
  * the extracted function's parameters follow what the selection does to each local (RF10);
  * the inlined local has one use and the inlined function one call, and both keep running where they were (RF11,
    RF12);
  * a changed signature updates every call, named arguments included (RF13);
  * a moved item is re-exported from where it was, and its new module imports what it used (RF14);
  * each refactoring refuses with an explanation, and changes nothing, when the result would mean something else
    (RF8): an extraction out of a loop, an extraction that would leak a local, an inlining of a local with two uses,
    an inlining of a function with two calls, a removal of a parameter the body uses, and a move to a module the
    package does not have;
  * a package with errors is refused before anything is attempted.
"""

import os
import shutil
import sys
from pathlib import Path

import harness

REFACTOR = harness.TESTS / "refactor"
UPDATE = os.environ.get("BURDIN_UPDATE_GOLDEN") == "1"

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()


def run(args, cwd):
    result = harness.run([burdin, *args], cwd=cwd)
    return (
        result.returncode,
        harness.text(harness.normalize(result.stdout)),
        harness.text(harness.normalize(result.stderr)),
    )


def tree(directory: Path):
    out = {}
    for path in sorted(directory.rglob("*")):
        if path.is_file() and ".git" not in path.relative_to(directory).parts:
            out[path.relative_to(directory).as_posix()] = harness.read_text(path)
    return out


def expect_tree(case: str, actual_dir: Path) -> None:
    expected_dir = REFACTOR / case / "after"
    actual = tree(actual_dir)
    if UPDATE:
        shutil.rmtree(expected_dir, ignore_errors=True)
        for rel, content in actual.items():
            (expected_dir / rel).parent.mkdir(parents=True, exist_ok=True)
            harness.write_text(expected_dir / rel, content)
        return
    check(tree(expected_dir) == actual, f"{case}: the refactored package differs from tests/refactor/{case}/after")


def copy_case(case: str, work: Path, name: str = "pkg") -> Path:
    destination = work / name
    shutil.copytree(REFACTOR / case / "before", destination)
    return destination


def case(name: str, args, refusal: str) -> None:
    """Runs one refactoring: the dry run, the change, `burdin check`, and the refusal."""
    with harness.temporary_dir(f"burdin-refactor-{name}-") as raw:
        work = Path(raw)
        pkg = copy_case(name, work)
        before = tree(pkg)
        code, out, err = run([*args, "--dry-run"], work)
        check(code == 0, f"{name} --dry-run: exit status {code}\n{err}")
        check("--- a/" in out and "+++ b/" in out, f"{name} --dry-run: no diff\n{out}")
        check(tree(pkg) == before, f"{name} --dry-run: files changed")
        # The refusal runs before the change, on the package as it was.
        code, out, err = run(refusal, work)
        check(code == 1 and "error:" in err, f"{name} refusal: exit status {code}\n{err}")
        check(refusal_text in err, f"{name} refusal: the message does not explain:\n{err}")
        check(tree(pkg) == before, f"{name} refusal: files changed")
        code, out, err = run(args, work)
        check(code == 0, f"{name}: exit status {code}\n{err}")
        expect_tree(name, pkg)
        code, out, err = run(["check", "pkg"], work)
        check(code == 0, f"{name}: the result does not pass `burdin check`:\n{err}")


# Each case: the refactoring, and the refusal, both against the `before` package.
refusal_text = ""

refusal_text = "loop"
case(
    "extract_variable",
    ["refactor", "extract-variable", "pkg/src/main.bd:4:21-4:26", "--name", "scaled"],
    ["refactor", "extract-variable", "pkg/src/main.bd:11:13-11:18", "--name", "twice"],
)
refusal_text = "binds `total`"
case(
    "extract_function",
    ["refactor", "extract-function", "pkg/src/main.bd:4:5-5:23", "--name", "report"],
    ["refactor", "extract-function", "pkg/src/main.bd:4:5-4:21", "--name", "sum"],
)
refusal_text = "2 times"
case(
    "inline_variable",
    ["refactor", "inline-variable", "pkg/src/main.bd:2:9"],
    ["refactor", "inline-variable", "pkg/src/main.bd:8:9"],
)
refusal_text = "called 2 times"

with harness.temporary_dir("burdin-refactor-inline_function-") as raw:
    work = Path(raw)
    pkg = copy_case("inline_function", work)
    before = tree(pkg)
    code, out, err = run(["refactor", "inline-function", "pkg/src/main.bd:1:4", "--dry-run"], work)
    check(code == 0 and "--- a/" in out, f"inline_function --dry-run: {code}\n{err}")
    check(tree(pkg) == before, "inline_function --dry-run: files changed")
    code, out, err = run(["refactor", "inline-function", "pkg/src/main.bd:5:4"], work)
    check(code == 1 and "called 2 times" in err, f"inline_function refusal: {code}\n{err}")
    check(tree(pkg) == before, "inline_function refusal: files changed")
    code, out, err = run(["refactor", "inline-function", "pkg/src/main.bd:1:4"], work)
    check(code == 0, f"inline_function: exit status {code}\n{err}")
    expect_tree("inline_function", pkg)
    code, out, err = run(["check", "pkg"], work)
    check(code == 0, f"inline_function: the result does not pass `burdin check`:\n{err}")

refusal_text = "uses `width`"
case(
    "change_signature",
    ["refactor", "change-signature", "pkg/src/main.bd:1:4", "--reorder", "height,width", "--rename", "width=w"],
    ["refactor", "change-signature", "pkg/src/main.bd:1:4", "--remove", "width"],
)
refusal_text = "no module `nowhere`"
case(
    "move_item",
    ["refactor", "move-item", "pkg/src/main.bd:1:4", "--to", "util"],
    ["refactor", "move-item", "pkg/src/main.bd:1:4", "--to", "nowhere"],
)

# ---- a package that does not check is refused ---------------------------------------------------------------

with harness.temporary_dir("burdin-refactor-errors-") as raw:
    work = Path(raw)
    pkg = copy_case("extract_variable", work)
    harness.write_text(pkg / "src" / "main.bd", "fn main() {\n    let x: i32 = \"no\"\n}\n")
    code, out, err = run(["refactor", "extract-variable", "pkg/src/main.bd:2:17-2:21", "--name", "v"], work)
    check(code == 1 and "the package has errors" in err, f"errors: {code}\n{err}")

# ---- usage -----------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-refactor-usage-") as raw:
    work = Path(raw)
    copy_case("extract_variable", work)
    code, out, err = run(["refactor"], work)
    check(code == 2 and "missing the refactoring" in err and "usage: burdin refactor" in err, f"no kind: {code}\n{err}")
    code, out, err = run(["refactor", "rename-everything", "pkg"], work)
    check(code == 2 and "unknown refactoring" in err, f"bad kind: {code}\n{err}")
    code, out, err = run(["refactor", "extract-variable", "pkg/src/main.bd", "--name", "x"], work)
    check(code == 2 and "needs a position" in err, f"no position: {code}\n{err}")
    code, out, err = run(["refactor", "extract-variable", "pkg/src/main.bd:1:1"], work)
    check(code == 2 and "needs a range" in err, f"no range: {code}\n{err}")
    code, out, err = run(["refactor", "extract-function", "pkg/src/main.bd:1:1-1:2"], work)
    check(code == 2 and "needs --name" in err, f"no name: {code}\n{err}")

if UPDATE:
    print("rewrote the after trees under tests/refactor")
if problems:
    sys.exit(harness.report_problems(problems))
print(f"check_refactor: {checks} checks passed")
