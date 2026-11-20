#!/usr/bin/env python3
"""Checks `burdin ssr` (stage 3, issue #611; spec/refactor.md, RF1-RF6).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). Each case is a package under tests/ssr/<case>/before;
the expected result is tests/ssr/<case>/after, and the listing a search prints is a golden beside them.
BURDIN_UPDATE_GOLDEN=1 rewrites the `after` trees and the goldens from the current driver.

What it checks:
  * a pattern replaces every match of a whole expression, and the result equals the `after` tree and passes
    `burdin check` (RF1, RF2);
  * `--dry-run` prints the diff and writes nothing (RF6);
  * a placeholder's type constraint (`where $a: Option<_>`) keeps the matches whose expression has that type (RF5);
  * a dotted pattern matches a call the file imported under its plain name: names match by what they resolve to,
    not by their spelling (RF3);
  * without `==>>` the matches are listed with their position and nothing changes;
  * an invalid pattern is a usage error; a pattern with no match exits 1;
  * a replacement that would not compile is skipped and reported, and the file keeps its text (RF2).
"""

import os
import shutil
import sys
from pathlib import Path

import harness

SSR = harness.TESTS / "ssr"
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


def golden(name: str, actual: str) -> None:
    path = SSR / name
    if UPDATE:
        path.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(path, actual)
        return
    if not path.is_file():
        problems.append(f"missing golden tests/ssr/{name} (BURDIN_UPDATE_GOLDEN=1 writes it)")
        return
    expected = harness.read_text(path)
    check(expected == actual, f"tests/ssr/{name} differs:\n--- expected\n{expected}\n--- actual\n{actual}")


def expect_tree(case: str, actual_dir: Path) -> None:
    expected_dir = SSR / case / "after"
    actual = tree(actual_dir)
    if UPDATE:
        shutil.rmtree(expected_dir, ignore_errors=True)
        for rel, content in actual.items():
            (expected_dir / rel).parent.mkdir(parents=True, exist_ok=True)
            harness.write_text(expected_dir / rel, content)
        return
    check(tree(expected_dir) == actual, f"{case}: the replaced package differs from tests/ssr/{case}/after")


def copy_case(case: str, work: Path) -> Path:
    destination = work / "pkg"
    shutil.copytree(SSR / case / "before", destination)
    return destination


# ---- replacing every match ----------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-ssr-") as raw:
    work = Path(raw)
    pkg = copy_case("replace", work)
    before = tree(pkg)
    code, out, err = run(["ssr", "$a + $b ==>> $b + $a", "pkg", "--dry-run"], work)
    check(code == 0, f"replace --dry-run: exit status {code}\n{err}")
    golden("replace/dry-run.golden", out)
    check(tree(pkg) == before, "replace --dry-run: files changed")
    check("ssr: would replace 1 file" in err and "ssr: 1 match" in err, f"replace --dry-run: {err}")
    code, out, err = run(["ssr", "$a + $b ==>> $b + $a", "pkg"], work)
    check(code == 0, f"replace: exit status {code}\n{err}")
    expect_tree("replace", pkg)
    code, out, err = run(["check", "pkg"], work)
    check(code == 0, f"replace: the result does not pass `burdin check`:\n{err}")
    code, out, err = run(["ssr", "$a * 9", "pkg"], work)
    check(code == 1 and "ssr: 0 matches" in err and out == "", f"replace after the change: {code}\n{out}{err}")

# ---- a type constraint --------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-ssr-") as raw:
    work = Path(raw)
    pkg = copy_case("typed", work)
    code, out, err = run(["ssr", "$a.unwrap_or($b) where $a: Option<_> ==>> $b", "pkg"], work)
    check(code == 0, f"typed: exit status {code}\n{err}")
    expect_tree("typed", pkg)
    code, out, err = run(["check", "pkg"], work)
    check(code == 0, f"typed: the result does not pass `burdin check`:\n{err}")
    # A constraint the match does not have keeps it from matching.
    second = work / "second"
    second.mkdir()
    shutil.copytree(SSR / "typed" / "before", second / "pkg")
    code, out, err = run(["ssr", "$a.unwrap_or($b) where $a: i32", "pkg"], second)
    check(code == 1 and "ssr: 0 matches" in err, f"typed wrong constraint: {code}\n{err}")

# ---- names match by what they resolve to --------------------------------------------------------------------------

with harness.temporary_dir("burdin-ssr-") as raw:
    work = Path(raw)
    pkg = copy_case("import", work)
    code, out, err = run(["ssr", "geometry.rectangle_area($w, $h) ==>> $w * $h", "pkg"], work)
    check(code == 0, f"import: exit status {code}\n{err}")
    expect_tree("import", pkg)
    code, out, err = run(["check", "pkg"], work)
    check(code == 0, f"import: the result does not pass `burdin check`:\n{err}")

# ---- search only --------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-ssr-") as raw:
    work = Path(raw)
    pkg = copy_case("search", work)
    before = tree(pkg)
    code, out, err = run(["ssr", "$a + $b", "pkg"], work)
    check(code == 0, f"search: exit status {code}\n{err}")
    golden("search/matches.golden", out)
    check("ssr: 2 matches" in err, f"search: {err}")
    check(tree(pkg) == before, "search: files changed")

    # Nothing matches: exit status 1.
    code, out, err = run(["ssr", "$a * 7", "pkg"], work)
    check(code == 1 and "ssr: 0 matches" in err, f"search with no match: {code}\n{err}")

    # A replacement that does not compile is skipped and reported.
    code, out, err = run(["ssr", "$a + $b ==>> $a.nope()", "pkg"], work)
    check(code == 0, f"skipped: exit status {code}\n{err}")
    check("warning: skipped the match" in err and "nothing changed" in err, f"skipped: {err}")
    check(tree(pkg) == before, "skipped: files changed")

# ---- usage --------------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-ssr-") as raw:
    work = Path(raw)
    copy_case("search", work)
    code, out, err = run(["ssr", "$a +", "pkg"], work)
    check(code == 2 and "not one expression" in err, f"invalid pattern: {code}\n{err}")
    code, out, err = run(["ssr"], work)
    check(code == 2 and "missing pattern" in err and "usage: burdin ssr" in err, f"no pattern: {code}\n{err}")
    code, out, err = run(["ssr", "$a", "pkg", "--bogus"], work)
    check(code == 2 and "unknown option `--bogus`" in err, f"bad option: {code}\n{err}")

if UPDATE:
    print("rewrote the goldens and after trees under tests/ssr")
if problems:
    sys.exit(harness.report_problems(problems))
print(f"check_ssr: {checks} checks passed")
