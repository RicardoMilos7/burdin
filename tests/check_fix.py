#!/usr/bin/env python3
"""Checks `burdin fix` (stage 3, issues #607 and #609; spec/fix.md).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). `git` on PATH is needed for the dirty-tree checks;
without it they are skipped with a note.

Each case is a package under tests/fix/<case>/before; the expected result is tests/fix/<case>/after and the golden
outputs beside them. BURDIN_UPDATE_GOLDEN=1 rewrites the `after` trees and the goldens from the current driver.

What it checks:
  * `--dry-run` prints the unified diff of every fix (compiler and linter) and changes nothing (FX9);
  * with uncommitted changes under the package, `burdin fix` refuses with exit status 2 and changes nothing, and
    `--allow-dirty` lets it write (FX7);
  * a fix run applies every machine-applicable suggestion, round after round, prints one `fixed` line each, formats
    the touched files, and the result equals the `after` tree and passes `burdin check`; a second run has nothing
    to fix (FX3, FX4, FX8);
  * a package with an error no fix removes keeps it: the fixes are written, the error is printed, and the exit
    status is 1 (FX6);
  * a maybe-incorrect suggestion (`.clone()` after a move) is never applied (FX1);
  * `--migrate` applies the deprecation fixes, moves the manifest to the next edition, and the result checks under
    it; a second `--migrate` says the package is already at the newest edition; a one-file target is refused (FX10,
    FX11);
  * usage errors exit with status 2.
"""

import os
import shutil
import sys
from pathlib import Path

import harness

FIX = harness.TESTS / "fix"
UPDATE = os.environ.get("BURDIN_UPDATE_GOLDEN") == "1"

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()
git = harness.which("git")


def run(args, cwd):
    result = harness.run([burdin, *args], cwd=cwd)
    return result.returncode, harness.text(harness.normalize(result.stdout)), harness.text(harness.normalize(result.stderr))


def tree(directory: Path):
    """Every file under `directory` as `relative path -> text`, without version-control files."""
    out = {}
    for path in sorted(directory.rglob("*")):
        if path.is_file() and ".git" not in path.relative_to(directory).parts:
            out[path.relative_to(directory).as_posix()] = harness.read_text(path)
    return out


def golden(name: str, actual: str) -> None:
    path = FIX / name
    if UPDATE:
        path.parent.mkdir(parents=True, exist_ok=True)
        harness.write_text(path, actual)
        return
    if not path.is_file():
        problems.append(f"missing golden tests/fix/{name} (BURDIN_UPDATE_GOLDEN=1 writes it)")
        return
    expected = harness.read_text(path)
    check(expected == actual, f"tests/fix/{name} differs:\n--- expected\n{expected}\n--- actual\n{actual}")


def expect_tree(case: str, actual_dir: Path) -> None:
    expected_dir = FIX / case / "after"
    actual = tree(actual_dir)
    if UPDATE:
        shutil.rmtree(expected_dir, ignore_errors=True)
        for rel, content in actual.items():
            (expected_dir / rel).parent.mkdir(parents=True, exist_ok=True)
            harness.write_text(expected_dir / rel, content)
        return
    expected = tree(expected_dir)
    check(expected == actual, f"{case}: the fixed package differs from tests/fix/{case}/after: {sorted(set(expected.items()) ^ set(actual.items()))[:4]}")


def copy_case(case: str, work: Path) -> Path:
    destination = work / "pkg"
    shutil.copytree(FIX / case / "before", destination)
    return destination


def git_commit_all(directory: Path) -> None:
    def g(*args):
        harness.run([git, *args], cwd=directory)

    g("init", "-q")
    g("config", "user.email", "fix@example.invalid")
    g("config", "user.name", "check_fix")
    g("config", "core.autocrlf", "false")
    g("add", "-A")
    g("commit", "-q", "-m", "before")


def checks_cleanly(directory: Path, label: str) -> None:
    code, out, err = run(["check", directory.name], directory.parent)
    check(code == 0, f"{label}: the fixed package does not pass `burdin check`:\n{err[:1500]}")


# ---- basic: the dry run, the dirty tree, the fix, and idempotence ------------------------------------------------

with harness.temporary_dir("burdin-fix-") as raw:
    work = Path(raw)
    pkg = copy_case("basic", work)
    before = tree(pkg)

    code, out, err = run(["fix", "pkg", "--dry-run"], work)
    check(code == 0, f"basic --dry-run: exit status {code}\n{err}")
    golden("basic/dry-run.golden", out)
    check("fix: would apply 4 fixes to 2 files" in err, f"basic --dry-run: no summary line:\n{err}")
    check(tree(pkg) == before, "basic --dry-run: files changed")

    if git is not None:
        git_commit_all(pkg)
        harness.write_text(pkg / "notes.txt", "uncommitted\n")
        code, out, err = run(["fix", "pkg"], work)
        check(code == 2, f"basic dirty: exit status {code}, expected 2")
        check("uncommitted changes" in err and "--allow-dirty" in err and "notes.txt" in err, f"basic dirty: {err}")
        check(tree(pkg) == {**before, "notes.txt": "uncommitted\n"}, "basic dirty: files changed")
        (pkg / "notes.txt").unlink()
    else:
        print("note: git is not on PATH; the dirty-tree checks are skipped")

    code, out, err = run(["fix", "pkg"], work)
    check(code == 0, f"basic fix: exit status {code}\n{err}")
    golden("basic/fix.golden", out)
    check("fix: applied 4 fixes to 2 files" in err, f"basic fix: no summary line:\n{err}")
    expect_tree("basic", pkg)
    checks_cleanly(pkg, "basic")

    code, out, err = run(["fix", "pkg", "--allow-dirty"], work)
    check(code == 0 and "fix: nothing to fix" in err and out == "", f"basic second run: {code}\n{out}\n{err}")

# ---- remaining: an error no fix removes ---------------------------------------------------------------------------

with harness.temporary_dir("burdin-fix-") as raw:
    work = Path(raw)
    pkg = copy_case("remaining", work)
    code, out, err = run(["fix", "pkg", "--allow-dirty"], work)
    check(code == 1, f"remaining: exit status {code}, expected 1\n{err}")
    check("error[E0003]" in err and "fix: 1 error and 0 warnings remain" in err, f"remaining: {err}")
    golden("remaining/fix.golden", out)
    expect_tree("remaining", pkg)

# ---- maybe-incorrect: never applied -------------------------------------------------------------------------------

with harness.temporary_dir("burdin-fix-") as raw:
    work = Path(raw)
    pkg = copy_case("maybe_incorrect", work)
    before = tree(pkg)
    code, out, err = run(["fix", "pkg", "--allow-dirty"], work)
    check(code == 1, f"maybe_incorrect: exit status {code}, expected 1")
    check("fix: nothing to fix" in err and "error[E0100]" in err and ".clone()" in err, f"maybe_incorrect: {err}")
    check(tree(pkg) == before, "maybe_incorrect: a maybe-incorrect suggestion was applied")

# ---- migrate: the deprecation fix and the next edition ------------------------------------------------------------

with harness.temporary_dir("burdin-fix-") as raw:
    work = Path(raw)
    pkg = copy_case("migrate", work)
    code, out, err = run(["check", "pkg"], work)
    check(code == 0 and "warning[E0301]" in err, f"migrate: the package should check with a deprecation warning:\n{err}")
    code, out, err = run(["fix", "pkg", "--migrate", "--dry-run"], work)
    check(code == 0, f"migrate --dry-run: exit status {code}\n{err}")
    golden("migrate/dry-run.golden", out)
    code, out, err = run(["fix", "pkg", "--migrate", "--allow-dirty"], work)
    check(code == 0, f"migrate: exit status {code}\n{err}")
    golden("migrate/fix.golden", out)
    expect_tree("migrate", pkg)
    check('edition = "2026"' in harness.read_text(pkg / "burdin.toml"), "migrate: the manifest names no new edition")
    code, out, err = run(["check", "pkg"], work)
    check(code == 0 and "warning" not in err, f"migrate: the migrated package does not check cleanly:\n{err}")
    code, out, err = run(["fix", "pkg", "--migrate", "--allow-dirty"], work)
    check(code == 0 and "already at edition 2026" in err, f"migrate again: {code}\n{err}")

    # A one-file target has no manifest to record an edition in.
    harness.write_text(work / "one.bd", "fn main() {\n    println(\"hi\")\n}\n")
    code, out, err = run(["fix", "one.bd", "--migrate", "--allow-dirty"], work)
    check(code == 2 and "one-file target" in err, f"migrate a file: {code}\n{err}")

    # An unknown edition in a manifest is an error that names the known ones.
    harness.write_text(pkg / "burdin.toml", '[package]\nname = "migrate"\nedition = "1999"\n')
    code, out, err = run(["check", "pkg"], work)
    check(code != 0 and "unknown edition `1999`" in err and "2024 and 2026" in err, f"unknown edition: {code}\n{err}")

# ---- usage errors -------------------------------------------------------------------------------------------------

with harness.temporary_dir("burdin-fix-") as raw:
    work = Path(raw)
    code, out, err = run(["fix"], work)
    # Without a target the working directory is the target: outside a package it is an error of the package.
    check(code == 1 and "burdin.toml" in err, f"no target outside a package: {code}\n{err}")
    code, out, err = run(["fix", "x.bd", "--bogus"], work)
    check(code == 2 and "unknown option `--bogus`" in err, f"bad option: {code}\n{err}")

if UPDATE:
    print("rewrote the goldens under tests/fix")
if problems:
    sys.exit(harness.report_problems(problems))
print(f"check_fix: {checks} checks passed")
