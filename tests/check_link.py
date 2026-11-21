#!/usr/bin/env python3
"""Checks the `[link]` table of a manifest: native libraries a package asks the linker for (stage 3, issue #322;
spec/packages.md, PKG34).

Needs a built driver (BURDIN selects it, like tests/run_tests.py). Every package is written into a temporary directory,
and the libraries are made up (`burdin_no_such_lib_*`), so the check needs no native library and runs the same on every
platform: a library that reached the linker fails the link and the linker's message names it.

What it checks:
  * a dependency's `[link] libs` reach the link of the program that depends on it: the build fails and names the library
    (PKG34), and so does a dependency's dependency (every package of the graph contributes);
  * the entry package's own `[link]` table works the same way;
  * a `[target.'cfg(...)'.link]` table whose predicate does not hold on this host adds nothing: the build succeeds and
    the program runs, and the table of the host's own platform (`cfg(unix)` or `cfg(windows)`) fails it by name;
  * a `[link]` library and a matching target table's are both requested (the merge and its order are the loader's unit test);
  * `burdin check` does not link, so the same package checks cleanly;
  * a package with no table builds as before;
  * the manifest errors name the line: an unknown key, a value that is not an array of strings, a name that is a flag or
    holds a space, and a bad predicate.
"""

import sys

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()

HOST = "windows" if harness.PLATFORM.startswith("windows") else "unix"
OTHER = "unix" if HOST == "windows" else "windows"


def out(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def err(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


def write(path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def package(root, name: str, manifest_tail: str = "", dependencies=(), source: str = "fn value(): i32 {\n    7\n}\n"):
    """A library package `name` under `root`, depending on the packages in `dependencies` (names)."""
    deps = "".join(f'{dep} = {{ path = "../{dep}" }}\n' for dep in dependencies)
    table = f"\n[dependencies]\n{deps}" if deps else ""
    write(root / name / "burdin.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\n{table}\n{manifest_tail}')
    write(root / name / "src" / f"{name}.bd", source)


def app(root, manifest_tail: str = "", dependencies=()):
    deps = "".join(f'{dep} = {{ path = "../{dep}" }}\n' for dep in dependencies)
    table = f"\n[dependencies]\n{deps}" if deps else ""
    write(root / "app" / "burdin.toml", f'[package]\nname = "app"\nversion = "0.1.0"\n{table}\n{manifest_tail}')
    uses = "".join(f"use {dep}\n" for dep in dependencies)
    write(root / "app" / "src" / "main.bd", f'{uses}\nfn main() {{\n    println("ok")\n}}\n')
    return root / "app"


def build(directory, *extra):
    return harness.run([burdin, "build", ".", *extra], cwd=directory)


with harness.temporary_dir("burdin-link-") as work:
    # --- A dependency's libraries reach the link, by name (PKG34) ---
    case = work / "dependency"
    package(case, "native", '[link]\nlibs = ["burdin_no_such_lib_dep"]\n')
    directory = app(case, dependencies=["native"])
    result = build(directory)
    check(result.returncode != 0, "a dependency's missing library did not fail the link")
    check("burdin_no_such_lib_dep" in err(result) + out(result),
          f"the link failure does not name the dependency's library: {err(result)[:400]!r}")

    # --- Every package of the graph contributes, a dependency's dependency too ---
    case = work / "transitive"
    package(case, "deep", '[link]\nlibs = ["burdin_no_such_lib_deep"]\n')
    package(case, "middle", dependencies=["deep"], source="use deep\n\nfn value(): i32 {\n    1\n}\n")
    directory = app(case, dependencies=["middle"])
    result = build(directory)
    check(result.returncode != 0 and "burdin_no_such_lib_deep" in err(result) + out(result),
          f"a dependency's dependency did not reach the link: exit {result.returncode}, {err(result)[:400]!r}")

    # --- The entry package's own table ---
    case = work / "entry"
    directory = app(case, '[link]\nlibs = ["burdin_no_such_lib_entry"]\n')
    result = build(directory)
    check(result.returncode != 0 and "burdin_no_such_lib_entry" in err(result) + out(result),
          f"the entry package's own libraries did not reach the link: {err(result)[:400]!r}")

    # --- A table for another platform adds nothing; this platform's table does ---
    case = work / "targets"
    other_table = f"[target.'cfg({OTHER})'.link]\nlibs = [\"burdin_no_such_lib_other\"]\n"
    unknown_os = "[target.'cfg(target_os = \"no_such_os\")'.link]\nlibs = [\"burdin_no_such_lib_unknown_os\"]\n"
    package(case, "native", other_table + unknown_os)
    directory = app(case, dependencies=["native"])
    result = build(directory)
    check(result.returncode == 0, f"a table for another platform failed the build: {err(result)[:400]!r}")
    ran = harness.run([burdin, "run", "."], cwd=directory)
    check(ran.returncode == 0 and out(ran).strip() == "ok", f"the program did not run: {out(ran)!r} {err(ran)[:300]!r}")
    host_table = f"[target.'cfg({HOST})'.link]\nlibs = [\"burdin_no_such_lib_host\"]\n"
    package(case, "native", other_table + host_table)
    result = build(directory)
    check(result.returncode != 0 and "burdin_no_such_lib_host" in err(result) + out(result),
          f"this platform's table did not reach the link: {err(result)[:400]!r}")
    check("burdin_no_such_lib_other" not in err(result) + out(result),
          "a table for another platform reached the linker")

    # --- A `[link]` library and a matching target table's are both requested ---
    case = work / "additive"
    first = '[link]\nlibs = ["burdin_no_such_lib_first", "burdin_no_such_lib_first"]\n\n'
    both = first + host_table.replace("burdin_no_such_lib_host", "burdin_no_such_lib_second")
    package(case, "native", both)
    directory = app(case, dependencies=["native"])
    result = build(directory)
    text = err(result) + out(result)
    check(result.returncode != 0 and "burdin_no_such_lib_first" in text, f"the `[link]` library was not requested: {text[:400]!r}")
    # The linker may stop at the first missing library; the merge itself is the loader's unit test.
    # --- `burdin check` does not link ---
    result = harness.run([burdin, "check", "."], cwd=directory)
    check(result.returncode == 0, f"`burdin check` linked or failed: {err(result)[:400]!r}")

    # --- A package with no table is unchanged ---
    case = work / "plain"
    package(case, "native")
    directory = app(case, dependencies=["native"])
    result = build(directory)
    check(result.returncode == 0, f"a package with no link table stopped building: {err(result)[:400]!r}")

    # --- The manifest errors ---
    def manifest_error(label: str, tail: str, *needles: str) -> None:
        case = work / f"error-{label}"
        directory = app(case, tail)
        result = build(directory)
        text = err(result)
        check(result.returncode == 1, f"{label}: expected exit status 1, got {result.returncode}")
        for needle in needles:
            check(needle in text, f"{label}: the error does not say {needle!r}: {text[:400]!r}")

    manifest_error("unknown-key", '[link]\nsearch = ["/opt/lib"]\n', "unknown key `search`", "burdin.toml:6")
    manifest_error("not-array", '[link]\nlibs = "ssl"\n', "`libs` must be an array", "burdin.toml:6")
    manifest_error("flag", '[link]\nlibs = ["-lssl"]\n', "`-lssl` is not a library name", "burdin.toml:6")
    manifest_error("space", '[link]\nlibs = ["a b"]\n', "is not a library name")
    manifest_error("empty", '[link]\nlibs = [""]\n', "is not a library name")
    manifest_error("predicate", "[target.'cfg(bogus)'.link]\nlibs = [\"x\"]\n", "burdin.toml:5")

print(f"Checked the link table ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
