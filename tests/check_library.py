#!/usr/bin/env python3
"""Checks libraries: the `[lib]` table and the static and shared artifacts (spec/library.md, LB1-LB11).

Needs a built driver (BURDIN selects it, like tests/run_tests.py) and a C compiler for the parts that link C
programs. Each case copies a fixture from tests/library into a temporary directory first, so the header and the
`target/` directory a build writes never touch the repository.

  * LB2: a `source` library (no `[lib]`, or `kind = "source"`) still fails `build` with "has no entry point";
  * LB3, LB6: a `static` and a `shared` library build to the artifact the table names, with the platform's file name,
    and `burdin test` runs the library's test block;
  * LB1, LB4: a `src/main.bd` next to a library kind, an unknown key, a `kind` that is unknown or not a string, a
    `c-header` on a `source` library, and a `c-header` that is not a relative path are E0265 at the line of the
    entry, from `build` and from `check`;
  * LB5: on Linux and macOS the static archive defines the two C exports and no other global symbol, so the `priv`
    export is absent and the helpers are local;
  * LB6, LB7: on Linux a C program compiled against the header links the static library with `-lm` and the shared
    library by name, calls both exports, and gets their results; the text an export prints appears (the flush at
    exit);
  * LB8: `c-header` writes the header `burdin bindgen --export` writes for the same package and name;
  * LB9: `--lto` on a library is a usage error (exit status 2);
  * LB10: a workspace build writes the library member's artifact, builds the program member, and skips the `source`
    member with a note;
  * LB11: `--locked` accepts a changed `[lib]` table, and a package without dependencies writes no lockfile.

The C programs run only on Linux: their link flags (`-Wl,-rpath`, and the MSVC `cl /LD` link) are not checked on the
other platforms here. The artifact names are checked everywhere.
"""

import os
import shutil
import sys
from pathlib import Path

import harness

FIXTURES = harness.TESTS / "library"
C_DIR = FIXTURES / "c"
EXPORTS = {"greet_base", "greet_print"}

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


def output(result) -> str:
    return harness.text(result.stdout) + harness.text(result.stderr)


def c_compiler():
    """A `cc`-style compiler: the one `BURDIN_CC` names, or the first of cc, clang, and gcc on `PATH`."""
    named = os.environ.get("BURDIN_CC")
    if named:
        return named
    for name in ("cc", "clang", "gcc"):
        found = harness.which(name)
        if found:
            return found
    return None


burdin = harness.require_burdin()
cc = c_compiler()


def build(directory: Path, *extra: str, command: str = "build"):
    return harness.run([burdin, command, str(directory), *extra], cwd=directory)


def copy_fixture(name: str, destination: Path) -> Path:
    shutil.copytree(FIXTURES / name, destination, ignore=shutil.ignore_patterns("target", "include", "burdin.lock"))
    return destination


def write_manifest(root: Path, text: str) -> None:
    harness.write_text(root / "burdin.toml", text)


def lib_manifest(kind: str, extra: str = "") -> str:
    """The `greet` manifest with a `[lib]` table of `kind`, the C header of `static_ok`, and `extra` lines."""
    return (
        '[package]\nname = "greet"\nversion = "0.1.0"\n\n[lib]\n'
        f'kind = "{kind}"\nc-header = "include/greet.h"\n{extra}'
    )


def artifact_name(kind: str) -> str:
    if harness.PLATFORM == "windows":
        return "greet.lib" if kind == "static" else "greet.dll"
    if kind == "static":
        return "libgreet.a"
    return "libgreet.dylib" if harness.PLATFORM == "macos" else "libgreet.so"


def find_artifact(root: Path, name: str):
    """The one file called `name` under `root/target`, where a build writes its artifacts."""
    found = [path for path in (root / "target").rglob(name) if path.is_file()]
    return found[0] if len(found) == 1 else None


def defined_globals(path: Path) -> set:
    """The global symbols an object or archive defines, without the leading `_` Mach-O adds."""
    nm = harness.which("nm")
    if nm is None:
        return set()
    result = harness.run([nm, "-g", "--defined-only", str(path)])
    names = set()
    for line in harness.text(result.stdout).splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[1] in "TDBRSV":
            name = parts[2]
            if harness.PLATFORM == "macos" and name.startswith("_"):
                name = name[1:]
            names.add(name)
    return names


def run_c_program(root: Path, library: Path, name: str, *extra: str):
    """Builds `tests/library/c/static_main.c` against the header and `library`, then runs it."""
    program = root / name
    header_dir = root / "include"
    compiled = harness.run(
        [cc, "-I", str(header_dir), str(C_DIR / "static_main.c"), str(library), *extra, "-o", str(program)],
        cwd=root,
    )
    check(compiled.returncode == 0, f"the C program did not link against {library.name}: {output(compiled).strip()}")
    if compiled.returncode != 0:
        return None
    ran = harness.run([str(program)], cwd=root)
    check(ran.returncode == 0, f"the C program exited {ran.returncode} with {library.name}")
    return harness.text(ran.stdout)


with harness.temporary_dir("burdin-library-") as work:
    # --- LB2: a source library is what it was --------------------------------------------------------------
    source = copy_fixture("static_ok", work / "source")
    write_manifest(source, '[package]\nname = "greet"\nversion = "0.1.0"\n')
    result = build(source)
    check(
        result.returncode == 1 and "has no entry point" in output(result),
        f"a source library did not fail with 'has no entry point': exit {result.returncode}",
    )
    result = build(source, command="check")
    check(
        result.returncode == 1 and "has no entry point" in output(result),
        "`burdin check` of a source library without `--tests` did not fail as it did before (spec/check.md)",
    )
    result = build(source, "--tests", command="check")
    check(result.returncode == 0, f"`burdin check --tests` of a source library exited {result.returncode}: {output(result)}")
    result = harness.run([burdin, "test", str(source)], cwd=source)
    check(result.returncode == 0, f"`burdin test` of a source library exited {result.returncode}: {output(result)}")

    # --- LB3, LB5-LB8: a static library ----------------------------------------------------------------------
    static = copy_fixture("static_ok", work / "static")
    write_manifest(static, lib_manifest("static"))
    result = build(static)
    check(result.returncode == 0, f"`burdin build` of a static library exited {result.returncode}: {output(result)}")
    library = find_artifact(static, artifact_name("static"))
    check(library is not None, f"the static library {artifact_name('static')} was not written under target/")
    header = static / "include" / "greet.h"
    check(header.is_file(), "`c-header` wrote no header at include/greet.h (LB8)")
    if header.is_file():
        text = harness.read_text(header)
        check("greet_base" in text and "greet_print" in text, "the header does not declare the exports")
        check("hidden" not in text, "the header declares the `priv` export (LB5)")
        # LB8: the header is the one `burdin bindgen --export` writes for the same package and file name.
        exported = work / "export" / "greet.h"
        exported.parent.mkdir()
        result = harness.run([burdin, "bindgen", "--export", ".", "--out", str(exported)], cwd=static)
        check(result.returncode == 0, f"`bindgen --export` exited {result.returncode}: {output(result)}")
        if exported.is_file():
            check(harness.read_text(exported) == text, "the header differs from `burdin bindgen --export` (LB8)")
    if library is not None and harness.PLATFORM in ("linux", "macos") and harness.which("nm"):
        names = defined_globals(library)
        # The task runtime's context switch is module assembly, which names its two symbols globally (LB12).
        extra = {name for name in names - EXPORTS if not name.startswith("burdin_rt_ctx_")}
        check(
            EXPORTS <= names and not extra,
            f"the static library's global symbols are {sorted(names)}: the exports and {sorted(extra)} (LB5)",
        )
    result = harness.run([burdin, "test", str(static)], cwd=static)
    check(result.returncode == 0, f"`burdin test` of a static library exited {result.returncode}: {output(result)}")
    result = harness.run([burdin, "run", str(static)], cwd=static)
    check(
        result.returncode == 1 and "has no entry point" in output(result),
        f"`burdin run` of a static library did not refuse it: exit {result.returncode}",
    )
    if library is not None and cc and harness.PLATFORM == "linux":
        ran = run_c_program(static, library, "static_prog", "-lm")
        if ran is not None:
            words = ran.split()
            check("base" in words and "42" in words, f"the static library's base is not 42: {ran!r} (LB6)")
            check("5" in words, f"the text an export printed is missing: {ran!r} (LB6)")

    # --- LB3, LB6, LB7, LB8: a shared library -----------------------------------------------------------------
    shared = copy_fixture("static_ok", work / "shared")
    write_manifest(shared, lib_manifest("shared"))
    result = build(shared)
    check(result.returncode == 0, f"`burdin build` of a shared library exited {result.returncode}: {output(result)}")
    library = find_artifact(shared, artifact_name("shared"))
    check(library is not None, f"the shared library {artifact_name('shared')} was not written under target/")
    if library is not None and cc and harness.PLATFORM == "linux":
        program = shared / "shared_prog"
        compiled = harness.run(
            [
                cc,
                "-I",
                str(shared / "include"),
                str(C_DIR / "static_main.c"),
                str(library),
                f"-Wl,-rpath,{library.parent}",
                "-o",
                str(program),
            ],
            cwd=shared,
        )
        check(compiled.returncode == 0, f"the C program did not link the shared library: {output(compiled).strip()}")
        if compiled.returncode == 0:
            ran = harness.run([str(program)], cwd=shared)
            words = harness.text(ran.stdout).split()
            detail = f"exit {ran.returncode}, stdout {harness.text(ran.stdout)!r}, stderr {harness.text(ran.stderr)!r}"
            check(ran.returncode == 0 and "42" in words, f"the shared library's export did not answer 42 (LB7): {detail}")
            check("5" in words, f"the text an exported function printed is missing from a shared library (LB6): {detail}")

    # --- LB1, LB4: E0265 -------------------------------------------------------------------------------------
    bad = {
        "an unknown key": (
            '[package]\nname = "greet"\nversion = "0.1.0"\n\n[lib]\nkind = "static"\ncolour = "red"\n',
            None,
            ":7:",
        ),
        "an unknown kind": ('[package]\nname = "greet"\nversion = "0.1.0"\n\n[lib]\nkind = "dylib"\n', None, ":6:"),
        "a kind that is not a string": (
            '[package]\nname = "greet"\nversion = "0.1.0"\n\n[lib]\nkind = 1\n',
            None,
            ":6:",
        ),
        "a c-header on a source library": (
            '[package]\nname = "greet"\nversion = "0.1.0"\n\n[lib]\nc-header = "include/greet.h"\n',
            None,
            ":6:",
        ),
        "a c-header that climbs out of the package": (lib_manifest("static").replace("include/greet.h", "../greet.h"), None, ":7:"),
        "an absolute c-header": (lib_manifest("static").replace("include/greet.h", "/greet.h"), None, ":7:"),
    }
    for label, (text, _, line) in bad.items():
        root = copy_fixture("static_ok", work / ("bad-" + label.replace(" ", "-")))
        write_manifest(root, text)
        result = build(root)
        message = output(result)
        check(result.returncode == 1 and "error[E0265]" in message, f"{label}: expected E0265, got {message.strip()!r}")
        check(line in message, f"{label}: the message does not name line {line!r}: {message.strip()!r}")
        if label == "an unknown key":
            checked = build(root, command="check")
            check(
                checked.returncode == 1 and "error[E0265]" in output(checked),
                f"`burdin check` did not report the unknown key (E0265): {output(checked).strip()!r}",
            )

    # LB4: a static library next to a `src/main.bd` is E0265 at the `kind` line, from `build` and from `check`.
    with_main = copy_fixture("with_main", work / "with-main")
    for command in ("build", "check"):
        result = build(with_main, command=command)
        message = output(result)
        check(
            result.returncode == 1 and "error[E0265]" in message and ":6:" in message,
            f"`burdin {command}` of a library with a main: expected E0265 at line 6, got {message.strip()!r} (LB4)",
        )

    # --- LB9: options a library cannot use -------------------------------------------------------------------
    root = copy_fixture("static_ok", work / "lto")
    write_manifest(root, lib_manifest("static"))
    result = build(root, "--lto")
    check(
        result.returncode == 2 and "--lto" in output(result),
        f"`--lto` on a library exited {result.returncode} without naming it (LB9): {output(result).strip()!r}",
    )

    # --- LB10: a workspace builds its libraries and programs -------------------------------------------------
    workspace = work / "workspace"
    shutil.copytree(FIXTURES / "workspace", workspace)
    greet = copy_fixture("static_ok", workspace / "greet")
    write_manifest(greet, lib_manifest("static"))
    result = harness.run([burdin, "build"], cwd=workspace)
    message = output(result)
    check(result.returncode == 0, f"a workspace build exited {result.returncode}: {message}")
    check("skipped plain" in message, "the source member was not skipped with a note (LB10)")
    check(find_artifact(workspace, artifact_name("static")) is not None, "the workspace wrote no library (LB10)")
    program_name = "app.exe" if harness.PLATFORM == "windows" else "app"
    check(find_artifact(workspace, program_name) is not None, "the workspace wrote no program for `app` (LB10)")

    # --- LB11: the lockfile and --locked ignore the table ----------------------------------------------------
    root = copy_fixture("static_ok", work / "locked")
    write_manifest(root, lib_manifest("static"))
    result = build(root)
    check(result.returncode == 0, f"the static library did not build for the lockfile check: {output(result)}")
    write_manifest(root, lib_manifest("shared"))
    result = build(root, "--locked")
    check(result.returncode == 0, f"`--locked` rejected a changed `[lib]` table (LB11): {output(result).strip()!r}")
    check(not (root / "burdin.lock").exists(), "a package with no dependencies wrote a lockfile (LB11)")

print(f"Checked libraries ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
