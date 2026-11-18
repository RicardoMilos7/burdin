#!/usr/bin/env python3
"""Checks `burdin new` (stage 3, issue #228).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * `burdin new app` creates exactly burdin.toml, src/main.bd, and .gitignore, with the documented contents, and
    prints one line naming the package;
  * the new package builds, runs, and prints its greeting (`burdin run`), and `burdin test` accepts it;
  * the executable `burdin build .` leaves in the package directory is covered by its .gitignore;
  * an existing directory (empty or not), an existing file, and an existing package are refused with exit status 2,
    and the refusal leaves what was there untouched;
  * a nested path creates its parent directories and names the package after the last component;
  * an invalid name (not an identifier, a reserved word, `std`, no name) is refused with exit status 2 and creates
    nothing;
  * a missing name, an extra argument, and an unknown option are usage errors (exit status 2); `--help` exits 0.
"""

import sys
from pathlib import Path

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(message)


burdin = harness.require_burdin()

MANIFEST = '[package]\nname = "app"\nversion = "0.1.0"\n'
MAIN = 'fn main() {\n    println("hello, world")\n}\n'
GITIGNORE = "/target/\n/app\n/app.exe\n"


def files_under(directory: Path) -> list:
    return sorted(p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file())


def new(run_dir: Path, *arguments):
    return harness.run([burdin, "new", *arguments], cwd=run_dir)


with harness.temporary_dir("burdin-new-") as work:
    # The golden case: three files with exact contents, one line of output.
    created = new(work, "app")
    check(created.returncode == 0, f"`burdin new app` failed: {harness.text(created.stderr)[:400]}")
    check(
        harness.text(created.stdout).replace("\r", "") == "Created package `app` in app\n",
        f"`burdin new app` printed {harness.text(created.stdout)!r}",
    )
    app = work / "app"
    check(
        files_under(app) == [".gitignore", "burdin.toml", "src/main.bd"],
        f"`burdin new app` created {files_under(app) if app.is_dir() else 'nothing'}",
    )
    if app.is_dir():
        check(harness.read_text(app / "burdin.toml") == MANIFEST, "burdin.toml has unexpected contents")
        check(harness.read_text(app / "src" / "main.bd") == MAIN, "src/main.bd has unexpected contents")
        check(harness.read_text(app / ".gitignore") == GITIGNORE, ".gitignore has unexpected contents")
        check(
            b"\r" not in harness.read_bytes(app / "burdin.toml")
            and b"\r" not in harness.read_bytes(app / "src" / "main.bd"),
            "a generated file has CRLF line endings",
        )

        # The package works: it runs and prints its greeting, and `burdin test` accepts it (no tests, no failure).
        ran = harness.run([burdin, "run", app], cwd=work)
        check(ran.returncode == 0, f"`burdin run app` failed: {harness.text(ran.stderr)[:400]}")
        check(harness.text(ran.stdout).strip() == "hello, world", f"`burdin run app` printed {harness.text(ran.stdout)!r}")
        tested = harness.run([burdin, "test", app], cwd=work)
        check(tested.returncode == 0, f"`burdin test app` failed: {harness.text(tested.stderr)[:400]}")
        # `burdin build .` leaves the executable in the package directory, and the .gitignore names it.
        built = harness.run([burdin, "build", ".", "--release"], cwd=app)
        check(built.returncode == 0, f"`burdin build .` failed: {harness.text(built.stderr)[:400]}")
        left = [name for name in files_under(app) if name not in (".gitignore", "burdin.toml", "src/main.bd")]
        ignored = harness.read_text(app / ".gitignore").split("\n")
        check(len(left) == 1, f"`burdin build .` left {left}, expected one executable")
        check(
            all("/" + name in ignored for name in left),
            f"the .gitignore does not cover the built executable {left}",
        )

    # Refusals: nothing that exists is overwritten.
    again = new(work, "app")
    check(again.returncode == 2, f"`burdin new app` over an existing package exited {again.returncode}, expected 2")
    check("already exists" in harness.text(again.stderr), "the refusal does not say the path already exists")
    if app.is_dir():
        check(harness.read_text(app / "burdin.toml") == MANIFEST, "a refused `new` changed burdin.toml")

    mine = work / "mine"
    (mine / "src").mkdir(parents=True)
    harness.write_text(mine / "src" / "main.bd", "my own program\n")
    refused = new(work, "mine")
    check(refused.returncode == 2, f"`burdin new` over a non-empty directory exited {refused.returncode}, expected 2")
    check(files_under(mine) == ["src/main.bd"], "a refused `new` added files to an existing directory")
    check(harness.read_text(mine / "src" / "main.bd") == "my own program\n", "a refused `new` overwrote a file")

    empty = work / "empty"
    empty.mkdir()
    refused = new(work, "empty")
    check(refused.returncode == 2, f"`burdin new` over an empty directory exited {refused.returncode}, expected 2")
    check(files_under(empty) == [], "a refused `new` added files to an empty directory")

    file = work / "file"
    harness.write_text(file, "not a directory\n")
    refused = new(work, "file")
    check(refused.returncode == 2, f"`burdin new` over a file exited {refused.returncode}, expected 2")
    check(harness.read_text(file) == "not a directory\n", "a refused `new` changed an existing file")

    # A nested path: the parents are created and the last component names the package.
    nested = new(work, str(Path("tools") / "cli"))
    check(nested.returncode == 0, f"`burdin new tools/cli` failed: {harness.text(nested.stderr)[:400]}")
    manifest = work / "tools" / "cli" / "burdin.toml"
    check(
        manifest.is_file() and 'name = "cli"' in harness.read_text(manifest),
        "`burdin new tools/cli` did not name the package `cli`",
    )

    # Invalid names are refused before anything is created.
    for bad, reason in [
        ("my-app", "not a valid package name"),
        ("2fast", "not a valid package name"),
        ("fn", "reserved word"),
        ("std", "standard library"),
        ("..", "cannot take a package name"),
    ]:
        before = sorted(p.name for p in work.iterdir())
        result = new(work, bad)
        check(result.returncode == 2, f"`burdin new {bad}` exited {result.returncode}, expected 2")
        check(reason in harness.text(result.stderr), f"`burdin new {bad}` did not say: {reason}")
        check(sorted(p.name for p in work.iterdir()) == before, f"`burdin new {bad}` created something")

    # The command line.
    missing = new(work)
    check(missing.returncode == 2, f"`burdin new` without a name exited {missing.returncode}, expected 2")
    check("usage:" in harness.text(missing.stderr), "`burdin new` without a name does not print the usage")
    extra = new(work, "one", "two")
    check(extra.returncode == 2, f"`burdin new one two` exited {extra.returncode}, expected 2")
    check(not (work / "one").exists(), "`burdin new one two` created a package")
    unknown = new(work, "--bogus")
    check(unknown.returncode == 2, f"`burdin new --bogus` exited {unknown.returncode}, expected 2")
    helped = new(work, "--help")
    check(helped.returncode == 0, f"`burdin new --help` exited {helped.returncode}, expected 0")
    check("burdin new <name>" in harness.text(helped.stdout), "`burdin new --help` does not print the usage")

print(f"Checked `burdin new` ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
