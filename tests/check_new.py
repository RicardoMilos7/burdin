#!/usr/bin/env python3
"""Checks `burdin new` and `burdin init` (stage 3, issues #228 and #354).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * `burdin new app` creates exactly burdin.toml, src/main.bd, and .gitignore, with the documented contents, and
    prints one line naming the package;
  * the new package builds, runs, and prints its greeting (`burdin run`), and `burdin test` accepts it;
  * the executable `burdin build .` leaves in the package directory is covered by its .gitignore (the cached copy
    under `target/` is ignored by `/target/`, `spec/target.md` TG1);
  * an existing directory (empty or not), an existing file, and an existing package are refused with exit status 2,
    and the refusal leaves what was there untouched;
  * a nested path creates its parent directories and names the package after the last component;
  * an invalid name (not an identifier, a reserved word, `std`, no name) is refused with exit status 2 and creates
    nothing;
  * a missing name, an extra argument, and an unknown option are usage errors (exit status 2); `--help` exits 0;
  * `burdin new --lib` creates src/lib.bd instead of src/main.bd, with a .gitignore that covers only target/; the
    library's tests pass under `burdin test`, `burdin build` refuses it for lacking an entry point (the unchanged
    error), and a program that `use`s it runs (`burdin add --path`); `--lib` with `--bin` is a usage error that
    creates nothing;
  * `burdin init [<dir>]` writes the package into an existing directory and keeps its other files (the golden
    contents, the working directory by default, `..` named by its real name, `--lib`, and the program it writes runs);
  * `burdin init` refuses, before writing anything, when burdin.toml, the root module of its kind, or .gitignore
    already exists, when the directory does not exist or is a file, when `src` is a file, and when the name is
    invalid; the command line errors are the same as `new`'s.
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
LIB = """// The root module of the library: other packages reach it as `use <name>`, and `burdin test` runs the
// tests below.

/// Adds two numbers.
fn add(a: i32, b: i32): i32 {
    a + b
}

test "add works" {
    assert_eq(add(2, 3), 5)?
}
"""
LIB_GITIGNORE = "/target/\n"
USER_PROGRAM = 'use greet.add\n\nfn main() {\n    println(add(2, 3).to_string())\n}\n'


def files_under(directory: Path) -> list:
    return sorted(p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file())


def new(run_dir: Path, *arguments):
    return harness.run([burdin, "new", *arguments], cwd=run_dir)


def init(run_dir: Path, *arguments):
    return harness.run([burdin, "init", *arguments], cwd=run_dir)


def out(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def err(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


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
        # `burdin build .` leaves the executable in the package directory, and the .gitignore names it. The build also
        # fills `target/` with the cached artifact (`spec/target.md`, TG1), which `/target/` ignores as a whole.
        built = harness.run([burdin, "build", ".", "--release"], cwd=app)
        check(built.returncode == 0, f"`burdin build .` failed: {harness.text(built.stderr)[:400]}")
        left = [
            name
            for name in files_under(app)
            if name not in (".gitignore", "burdin.toml", "src/main.bd") and not name.startswith("target/")
        ]
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

# `burdin new --lib` and `--bin`: the library layout, its tests, and a program that uses it.
with harness.temporary_dir("burdin-new-lib-") as work:
    created = new(work, "--lib", "greet")
    check(created.returncode == 0, f"`burdin new --lib greet` failed: {err(created)[:400]}")
    check(out(created) == "Created library package `greet` in greet\n", f"`burdin new --lib` printed {out(created)!r}")
    greet = work / "greet"
    check(
        files_under(greet) == [".gitignore", "burdin.toml", "src/lib.bd"],
        f"`burdin new --lib greet` created {files_under(greet) if greet.is_dir() else 'nothing'}",
    )
    if greet.is_dir():
        check(
            harness.read_text(greet / "burdin.toml") == '[package]\nname = "greet"\nversion = "0.1.0"\n',
            "a library's burdin.toml has unexpected contents",
        )
        check(harness.read_text(greet / "src" / "lib.bd") == LIB, "src/lib.bd has unexpected contents")
        check(
            harness.read_text(greet / ".gitignore") == LIB_GITIGNORE,
            "a library's .gitignore has unexpected contents",
        )

        # The library's tests pass; it has no program, so `burdin build` fails with the entry-point error.
        tested = harness.run([burdin, "test", greet], cwd=work)
        check(tested.returncode == 0, f"`burdin test greet` failed: {err(tested)[:400]}")
        check("1 passed, 0 failed" in out(tested), f"`burdin test greet` printed {out(tested)!r}")
        built = harness.run([burdin, "build", greet], cwd=work)
        check(built.returncode == 1, f"`burdin build greet` exited {built.returncode}, expected 1")
        check(
            "package `greet` has no entry point" in err(built) and "src/main.bd" in err(built).replace("\\", "/"),
            f"`burdin build greet` did not report the missing entry point: {err(built)[:400]}",
        )

        # A program uses the library as `use greet`, through a path dependency.
        program = new(work, "app")
        check(program.returncode == 0, f"`burdin new app` failed: {err(program)[:400]}")
        app = work / "app"
        added = harness.run([burdin, "add", "--path", "../greet"], cwd=app)
        check(added.returncode == 0, f"`burdin add --path ../greet` failed: {err(added)[:400]}")
        harness.write_text(app / "src" / "main.bd", USER_PROGRAM)
        ran = harness.run([burdin, "run", app], cwd=work)
        check(ran.returncode == 0, f"a program using the library failed: {err(ran)[:400]}")
        check(out(ran).strip() == "5", f"a program using the library printed {out(ran)!r}")

    flags = new(work, "--lib", "--bin", "both")
    check(flags.returncode == 2, f"`burdin new --lib --bin` exited {flags.returncode}, expected 2")
    check("cannot be used together" in err(flags), "`burdin new --lib --bin` did not say the flags conflict")
    check(not (work / "both").exists(), "`burdin new --lib --bin` created a package")

    explicit = new(work, "--bin", "tool")
    check(out(explicit) == "Created package `tool` in tool\n", f"`burdin new --bin` printed {out(explicit)!r}")
    check((work / "tool" / "src" / "main.bd").is_file(), "`burdin new --bin` did not create src/main.bd")

    refused = new(work, "--lib", "greet")
    check(
        refused.returncode == 2,
        f"`burdin new --lib` over an existing package exited {refused.returncode}, expected 2",
    )

# `burdin init`: the package goes into an existing directory, and nothing that exists is overwritten.
with harness.temporary_dir("burdin-init-") as work:
    # An existing directory with other files: they are kept, and the program it gets runs.
    proj = work / "proj"
    (proj / "src").mkdir(parents=True)
    harness.write_text(proj / "README.md", "notes\n")
    harness.write_text(proj / "src" / "helper.bd", "fn helper() {}\n")
    initialized = init(work, "proj")
    check(initialized.returncode == 0, f"`burdin init proj` failed: {err(initialized)[:400]}")
    check(
        out(initialized) == "Initialized package `proj` in proj\n",
        f"`burdin init proj` printed {out(initialized)!r}",
    )
    check(
        files_under(proj) == [".gitignore", "README.md", "burdin.toml", "src/helper.bd", "src/main.bd"],
        f"`burdin init proj` left {files_under(proj)}",
    )
    if proj.is_dir():
        check(harness.read_text(proj / "README.md") == "notes\n", "`burdin init` changed README.md")
        check(
            harness.read_text(proj / "src" / "helper.bd") == "fn helper() {}\n",
            "`burdin init` changed src/helper.bd",
        )
        check(harness.read_text(proj / "src" / "main.bd") == MAIN, "`burdin init` wrote unexpected src/main.bd")
        check(
            harness.read_text(proj / "burdin.toml") == '[package]\nname = "proj"\nversion = "0.1.0"\n',
            "`burdin init` wrote unexpected burdin.toml",
        )
        check(
            harness.read_text(proj / ".gitignore") == "/target/\n/proj\n/proj.exe\n",
            "`burdin init` wrote unexpected .gitignore",
        )
        ran = harness.run([burdin, "run", proj], cwd=work)
        check(ran.returncode == 0 and out(ran).strip() == "hello, world", f"`burdin run proj` printed {out(ran)!r}")

    # No directory argument: the working directory, named after its own directory.
    here = work / "here"
    here.mkdir()
    library = init(here, "--lib")
    check(library.returncode == 0, f"`burdin init --lib` failed: {err(library)[:400]}")
    check(
        out(library) == "Initialized library package `here` in .\n",
        f"`burdin init --lib` printed {out(library)!r}",
    )
    check(
        files_under(here) == [".gitignore", "burdin.toml", "src/lib.bd"],
        f"`burdin init --lib` created {files_under(here)}",
    )
    tested = harness.run([burdin, "test", "."], cwd=here)
    check(
        tested.returncode == 0 and "1 passed, 0 failed" in out(tested),
        f"the initialized library's tests failed: {out(tested)[:300]}",
    )

    # `..` is the parent directory, and the package takes the parent's real name.
    inner = work / "outer" / "inner"
    inner.mkdir(parents=True)
    parent = init(inner, "..")
    check(parent.returncode == 0, f"`burdin init ..` failed: {err(parent)[:400]}")
    check(out(parent) == "Initialized package `outer` in ..\n", f"`burdin init ..` printed {out(parent)!r}")
    check((work / "outer" / "burdin.toml").is_file(), "`burdin init ..` did not write into the parent")

    # A program and a library root do not clash: `init` (a program) adds src/main.bd beside an existing src/lib.bd.
    both = work / "both"
    (both / "src").mkdir(parents=True)
    harness.write_text(both / "src" / "lib.bd", "fn f() {}\n")
    kept = init(work, "both")
    check(kept.returncode == 0, f"`burdin init` beside a library root failed: {err(kept)[:400]}")
    check((both / "src" / "main.bd").is_file(), "`burdin init` beside a library root did not write src/main.bd")

    # Refusals: a file that would be created already exists. Nothing else is written, and the file is kept.
    for index, (clash, flags) in enumerate(
        [("burdin.toml", []), ("src/main.bd", []), (".gitignore", []), ("src/lib.bd", ["--lib"])]
    ):
        name = f"clash{index}"
        target = work / name
        (target / "src").mkdir(parents=True, exist_ok=True)
        harness.write_text(target / clash, "mine\n")
        refused = init(work, *flags, name)
        check(refused.returncode == 2, f"`burdin init` over {clash} exited {refused.returncode}, expected 2")
        check(
            "already exists" in err(refused) and clash in err(refused).replace("\\", "/"),
            f"the refusal over {clash} did not name it: {err(refused)[:300]}",
        )
        check(harness.read_text(target / clash) == "mine\n", f"a refused `init` changed {clash}")
        check(files_under(target) == [clash], f"a refused `init` over {clash} wrote {files_under(target)}")

    # The directory itself: a missing one, a file, a `src` that is a file, and an invalid name.
    missing = init(work, "missing")
    check(
        missing.returncode == 2 and "does not exist" in err(missing),
        f"`burdin init missing` gave {err(missing)[:200]!r}",
    )
    check(not (work / "missing").exists(), "`burdin init missing` created the directory")
    harness.write_text(work / "afile", "x\n")
    as_file = init(work, "afile")
    check(
        as_file.returncode == 2 and "is not a directory" in err(as_file),
        f"`burdin init afile` gave {err(as_file)[:200]!r}",
    )
    check(harness.read_text(work / "afile") == "x\n", "a refused `init` changed a file")
    blocked = work / "blocked"
    blocked.mkdir()
    harness.write_text(blocked / "src", "not a directory\n")
    refused = init(work, "blocked")
    check(refused.returncode == 2, f"`burdin init` over a `src` file exited {refused.returncode}, expected 2")
    check(files_under(blocked) == ["src"], f"`burdin init` over a `src` file wrote {files_under(blocked)}")
    bad = work / "my-app"
    bad.mkdir()
    refused = init(work, "my-app")
    check(
        refused.returncode == 2 and "not a valid package name" in err(refused),
        f"`burdin init my-app` gave {err(refused)[:200]!r}",
    )
    check(files_under(bad) == [], "a refused `init` of an invalid name wrote files")

    # The command line.
    flags = init(work, "--lib", "--bin", "proj")
    check(
        flags.returncode == 2 and "cannot be used together" in err(flags),
        "`burdin init --lib --bin` was not refused",
    )
    extra = init(work, "proj", "other")
    check(
        extra.returncode == 2 and "unexpected argument `other`" in err(extra),
        "`burdin init proj other` was not refused",
    )
    unknown = init(work, "--bogus")
    check(unknown.returncode == 2, f"`burdin init --bogus` exited {unknown.returncode}, expected 2")
    helped = init(work, "--help")
    check(helped.returncode == 0 and "burdin init" in out(helped), "`burdin init --help` does not print the usage")

print(f"Checked `burdin new` and `burdin init` ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
