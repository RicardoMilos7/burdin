#!/usr/bin/env python3
"""Checks the artifact cache of a package's target directory (stage 3, issue #353, `spec/target.md`).

Needs a built driver. BURDIN selects it, like tests/run_tests.py. Every package is a fixture in a temporary directory.

The cache is shown the way check_scripts.py shows its binary cache: with a deliberately broken linker, a build that
hits the cache still succeeds, and a build that must link fails. What it checks:
  * the layout: `build` and `run` keep the executable and its fingerprint under `target/burdin/<profile>/bin/`, `test`
    keeps its binary under `test/`, and a release build lives under `release/` without disturbing the debug one;
  * a hit: an unchanged package builds, runs, and copies to `-o` with no linker, the fingerprint file is not rewritten,
    and a relative and an absolute spelling of one package share an entry;
  * the content key: touching a file is a hit, and a changed source, path dependency, manifest, build-script input, or
    flag must link, and the build that follows publishes the new program;
  * the failure paths: a type error is reported while a cached program exists, and the cache is left as it was; a link
    that fails publishes nothing; a truncated executable, a missing fingerprint, or a missing executable each relink; a
    target directory that cannot be created does not fail the build;
  * the bypass: a debug build (`-g`) always links and writes nothing to the cache;
  * the object cache (issue #243, TG11-TG16): with `BURDIN_CODEGEN_UNITS` the program is several units, each kept under
    `target/burdin/<profile>/objects/<kind>/`; an edit that does not change the generated code (a trailing comment)
    reuses every object, an edit to one function keeps the objects of the other units, objects of an earlier build
    are removed, and a debug build writes none.
"""

import os
import re
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
BROKEN = "/definitely-not-a-compiler"
MANIFEST = '[package]\nname = "{name}"\nversion = "0.1.0"\n'
FINGERPRINT_FORMAT = "burdin-artifact-v2"


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    harness.write_text(path, content)


def program(text: str) -> str:
    return 'fn main() {\n    println("' + text + '")\n}\n'


def make_package(root: Path, name: str, source: str) -> Path:
    package = root / name
    write(package / "burdin.toml", MANIFEST.format(name=name))
    write(package / "src" / "main.bd", source)
    return package


def burdin_run(args, cwd: Path, home: Path, broken: bool = False):
    env = dict(os.environ)
    env["BURDIN_HOME"] = str(home)
    env.pop("BURDIN_CC", None)
    if broken:
        env["BURDIN_CC"] = BROKEN
    return harness.run([burdin, *[str(a) for a in args]], cwd=cwd, env=env)


def bin_dir(package: Path, profile: str = "debug", kind: str = "bin") -> Path:
    return package / "target" / "burdin" / profile / kind


def cached_exe(package: Path, name: str, profile: str = "debug", kind: str = "bin") -> Path:
    return harness.executable_in(bin_dir(package, profile, kind), name)


def output_exe(directory: Path, name: str) -> Path:
    """An executable written with `-o <name>`: the linker adds `.exe` to a name with no extension on Windows."""
    return harness.executable_in(directory, name)


def fingerprint_of(package: Path, name: str, profile: str = "debug", kind: str = "bin") -> Path:
    return bin_dir(package, profile, kind) / f"{name}.fingerprint"


def output_of(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def stderr_of(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


def run_output(path: Path) -> str:
    """The standard output of a built program, or `<missing>` when the build left no executable there."""
    if not path.is_file():
        return "<missing>"
    return output_of(harness.run([str(path)]))


def staged_leftovers(package: Path) -> list:
    return sorted(bin_dir(package).glob("*.staging-*")) if bin_dir(package).is_dir() else []


with harness.temporary_dir("burdin-target-") as work:
    home = work / "home"

    # ---- the layout and a hit -------------------------------------------------
    hit = make_package(work, "hit", program("first"))
    built = burdin_run(["build", "."], hit, home)
    check(built.returncode == 0, f"`burdin build .` failed: {stderr_of(built)[:400]}")
    check(cached_exe(hit, "hit").is_file(), "the build left no executable under target/burdin/debug/bin/")
    check(fingerprint_of(hit, "hit").is_file(), "the build left no fingerprint beside its executable")
    check(staged_leftovers(hit) == [], "a published build left a staging file behind")
    lines = fingerprint_of(hit, "hit").read_text(encoding="utf-8").splitlines()
    check(lines and lines[0] == FINGERPRINT_FORMAT, "the fingerprint does not start with the format line")
    check(
        lines and re.fullmatch(r"artifact [0-9a-f]{64}", lines[-1]) is not None,
        "the fingerprint does not end with the executable's SHA-256",
    )
    for name in (
        "compiler",
        "host",
        "profile debug",
        "kind bin",
        "name hit",
        "settings opt=0;debug=false;debug-assertions=true;overflow-checks=true;panic=abort",
        "options strip=false",
    ):
        check(any(line.startswith(name) for line in lines), f"the fingerprint has no `{name}` line")
    main_source = os.path.join("src", "main.bd")
    check(
        any(line.startswith("source ") and line.endswith(main_source) for line in lines),
        "no source line for main.bd",
    )
    check(any(line.startswith("manifest ") and line.endswith(" burdin.toml") for line in lines), "no manifest line")
    ran = run_output(cached_exe(hit, "hit"))
    check(ran == "first\n", f"the built program printed {ran!r}")
    settled = fingerprint_of(hit, "hit").read_bytes()

    hit_copy = burdin_run(["build", ".", "-o", "copy"], hit, home, broken=True)
    check(hit_copy.returncode == 0, f"a cached build needed the linker: {stderr_of(hit_copy)[:300]}")
    check(run_output(output_exe(hit, "copy")) == "first\n", "the cached copy does not print the program's output")
    check(fingerprint_of(hit, "hit").read_bytes() == settled, "a cache hit rewrote the fingerprint")

    hit_run = burdin_run(["run", "."], hit, home, broken=True)
    check(hit_run.returncode == 0, f"`burdin run` did not hit the cache: {stderr_of(hit_run)[:300]}")
    check(output_of(hit_run) == "first\n", f"the cached run printed {output_of(hit_run)!r}")

    spelled = burdin_run(["build", hit.resolve(), "-o", "spelled"], work, home, broken=True)
    check(spelled.returncode == 0, "an absolute spelling of the package missed the cache it made with `.`")

    touch = hit / "src" / "main.bd"
    os.utime(touch, None)
    touched = burdin_run(["build", "."], hit, home, broken=True)
    check(touched.returncode == 0, "touching a source without changing it broke the cache hit")

    # ---- profiles and kinds are separate directories ---------------------------
    profiles = make_package(work, "profiles", program("profiles"))
    check(burdin_run(["build", "."], profiles, home).returncode == 0, "the debug build of `profiles` failed")
    check(burdin_run(["build", "--release", "."], profiles, home).returncode == 0, "the release build failed")
    check(
        cached_exe(profiles, "profiles", "release").is_file(),
        "a release build did not use target/burdin/release/bin/",
    )
    check(
        burdin_run(["build", "."], profiles, home, broken=True).returncode == 0,
        "a release build disturbed the debug cache",
    )
    check(
        burdin_run(["build", "--release", "."], profiles, home, broken=True).returncode == 0,
        "the release cache did not hit",
    )

    kinds = make_package(
        work,
        "kinds",
        'fn main() {\n    println("kinds")\n}\n\ntest "adds" {\n    assert(1 + 1 == 2)?\n}\n',
    )
    tested = burdin_run(["test", "."], kinds, home)
    check(tested.returncode == 0, f"`burdin test .` failed: {stderr_of(tested)[:400]}")
    check(cached_exe(kinds, "kinds", kind="test").is_file(), "`burdin test` did not keep its binary under test/")
    check(not cached_exe(kinds, "kinds").is_file(), "`burdin test` wrote a program into bin/")
    check(
        burdin_run(["test", "."], kinds, home, broken=True).returncode == 0,
        "a second `burdin test` did not hit the test cache",
    )
    check(
        burdin_run(["build", "."], kinds, home, broken=True).returncode != 0,
        "the test binary was used as the program: `build` hit an empty bin/",
    )
    check(burdin_run(["build", "."], kinds, home).returncode == 0, "the build of `kinds` failed")
    check(
        burdin_run(["test", "."], kinds, home, broken=True).returncode == 0,
        "building the program disturbed the test cache",
    )

    # ---- what makes a rebuild happen -------------------------------------------
    rebuild = make_package(work, "rebuild", program("first"))
    check(burdin_run(["build", "."], rebuild, home).returncode == 0, "the first build of `rebuild` failed")
    manifest = rebuild / "burdin.toml"
    write(manifest, manifest.read_text(encoding="utf-8") + "# an edit\n")
    edited = burdin_run(["build", "."], rebuild, home, broken=True)
    check(edited.returncode != 0 and "linker" in stderr_of(edited), "a changed manifest did not rebuild")
    check(burdin_run(["build", "."], rebuild, home).returncode == 0, "the rebuild after a manifest change failed")

    write(rebuild / "src" / "main.bd", program("second"))
    changed = burdin_run(["build", ".", "-o", "changed"], rebuild, home, broken=True)
    check(changed.returncode != 0 and "linker" in stderr_of(changed), "a changed source did not rebuild")
    check(burdin_run(["build", ".", "-o", "changed"], rebuild, home).returncode == 0, "the rebuild failed")
    check(output_of(harness.run([str(output_exe(rebuild, "changed"))])) == "second\n", "the rebuilt program is stale")

    stripped = burdin_run(["build", "--strip", "."], rebuild, home, broken=True)
    check(stripped.returncode != 0 and "linker" in stderr_of(stripped), "a changed flag (--strip) did not rebuild")
    check(burdin_run(["build", "--strip", "."], rebuild, home).returncode == 0, "the --strip build failed")
    unstripped = burdin_run(["build", "."], rebuild, home, broken=True)
    check(unstripped.returncode != 0, "the stripped cache was reused for an unstripped build")
    check(burdin_run(["build", "."], rebuild, home).returncode == 0, "the rebuild without --strip failed")

    # One executable per profile and kind: going back to an earlier source relinks, it does not remember.
    write(rebuild / "src" / "main.bd", program("first"))
    back = burdin_run(["build", "."], rebuild, home, broken=True)
    check(back.returncode != 0, "a source that is not the cached one was reused")

    # ---- a path dependency is an input -----------------------------------------
    greet = work / "greet"
    write(greet / "burdin.toml", MANIFEST.format(name="greet"))
    write(greet / "src" / "greet.bd", 'fn hello(): str {\n    "first dependency"\n}\n')
    dependent = work / "dependent"
    write(
        dependent / "burdin.toml",
        MANIFEST.format(name="dependent") + '\n[dependencies]\ngreet = { path = "../greet" }\n',
    )
    write(dependent / "src" / "main.bd", 'use greet.hello\n\nfn main() {\n    println("{hello()}")\n}\n')
    check(burdin_run(["build", "."], dependent, home).returncode == 0, "the build with a path dependency failed")
    check(burdin_run(["build", "."], dependent, home, broken=True).returncode == 0, "the dependency build missed")
    write(greet / "src" / "greet.bd", 'fn hello(): str {\n    "second dependency"\n}\n')
    stale = burdin_run(["build", "."], dependent, home, broken=True)
    check(stale.returncode != 0, "changing a path dependency did not invalidate the program")
    fresh = burdin_run(["build", ".", "-o", "dependent-out"], dependent, home)
    check(fresh.returncode == 0, "the rebuild with a changed dependency failed")
    check(
        run_output(output_exe(dependent, "dependent-out")) == "second dependency\n",
        "the dependency rebuild is stale",
    )

    # ---- a build script's inputs are part of the program -----------------------
    scripted = work / "scripted"
    write(scripted / "burdin.toml", MANIFEST.format(name="scripted"))
    write(
        scripted / "build.bd",
        """fn main() {
    let args = env.args()
    let out = args[2]
    let header = embed("header.h")
    let source = "const N: i32 = {header.len()}\\n"
    match write_file("{out}/src/generated.bd", source) {
        Ok(_) => {}
        Err(_) => panic("cannot write the generated module"),
    }
    match write_file("{out}/inputs", "header.h\\n") {
        Ok(_) => {}
        Err(_) => panic("cannot write the input list"),
    }
}
""",
    )
    write(scripted / "src" / "main.bd", "use generated.N\n\nfn main() {\n    println(\"{}\", N)\n}\n")
    write(scripted / "header.h", "first header\n")
    check(burdin_run(["build", ".", "-o", "scripted-out"], scripted, home).returncode == 0, "the scripted build failed")
    check(run_output(output_exe(scripted, "scripted-out")) == "13\n", "the generated module has the wrong value")
    write(scripted / "header.h", "a longer second header\n")
    regenerated = burdin_run(["build", ".", "-o", "scripted-out"], scripted, home, broken=True)
    check(regenerated.returncode != 0, "a changed build-script input did not reach the program")
    check(burdin_run(["build", ".", "-o", "scripted-out"], scripted, home).returncode == 0, "the rebuild failed")
    # "a longer second header" is 22 characters, and the header ends with a newline.
    check(
        run_output(output_exe(scripted, "scripted-out")) == "23\n",
        "the rebuilt program does not use the regenerated module",
    )

    # ---- the failure paths ------------------------------------------------------
    typed = make_package(work, "typed", program("fine"))
    check(burdin_run(["build", "."], typed, home).returncode == 0, "the first build of `typed` failed")
    before = (cached_exe(typed, "typed").read_bytes(), fingerprint_of(typed, "typed").read_bytes())
    write(typed / "src" / "main.bd", 'fn main() {\n    let n: i32 = "text"\n    println("{n}")\n}\n')
    wrong = burdin_run(["build", "."], typed, home, broken=True)
    check(wrong.returncode == 1, f"a type error exited {wrong.returncode}, expected 1")
    check("E0003" in stderr_of(wrong), "a type error with a cached program does not report its code")
    after = (cached_exe(typed, "typed").read_bytes(), fingerprint_of(typed, "typed").read_bytes())
    check(before == after, "a failed check changed the cached program")
    write(typed / "src" / "main.bd", program("fine"))
    check(
        burdin_run(["build", "."], typed, home, broken=True).returncode == 0,
        "restoring the cached source did not hit the cache",
    )

    failing = make_package(work, "failing", program("never"))
    failed_link = burdin_run(["build", "."], failing, home, broken=True)
    check(failed_link.returncode != 0 and "linker" in stderr_of(failed_link), "a broken linker did not fail a build")
    check(not cached_exe(failing, "failing").is_file(), "a failed link left an executable in the cache")
    check(not fingerprint_of(failing, "failing").is_file(), "a failed link left a fingerprint")
    check(staged_leftovers(failing) == [], "a failed link left its staging file behind")
    check(burdin_run(["build", "."], failing, home).returncode == 0, "the build after a failed link failed")
    check(cached_exe(failing, "failing").is_file(), "the build after a failed link did not publish")

    damaged = make_package(work, "damaged", program("damaged"))
    check(burdin_run(["build", "."], damaged, home).returncode == 0, "the first build of `damaged` failed")
    exe = cached_exe(damaged, "damaged")
    fingerprint = fingerprint_of(damaged, "damaged")
    exe.write_bytes(b"truncated")
    check(
        burdin_run(["build", "."], damaged, home, broken=True).returncode != 0,
        "a truncated cached executable was trusted",
    )
    check(burdin_run(["build", "."], damaged, home).returncode == 0, "the rebuild of a truncated executable failed")
    fingerprint.unlink()
    check(
        burdin_run(["build", "."], damaged, home, broken=True).returncode != 0,
        "an executable without its fingerprint was trusted",
    )
    check(burdin_run(["build", "."], damaged, home).returncode == 0, "the rebuild without a fingerprint failed")
    cached_exe(damaged, "damaged").unlink()
    check(
        burdin_run(["build", "."], damaged, home, broken=True).returncode != 0,
        "a fingerprint without its executable was trusted",
    )

    blocked = make_package(work, "blocked", program("blocked"))
    write(blocked / "target", "a file where the target directory belongs\n")
    unwritable = burdin_run(["build", ".", "-o", "blocked-out"], blocked, home)
    check(unwritable.returncode == 0, f"an unwritable cache failed the build: {stderr_of(unwritable)[:300]}")
    check(run_output(output_exe(blocked, "blocked-out")) == "blocked\n", "the build without a cache did not run")
    check((blocked / "target").is_file(), "the build replaced the file that blocked the cache")

    # ---- the bypass: debug builds always link -----------------------------------
    debug_built = burdin_run(["build", ".", "-g", "-o", "debug-out"], hit, home, broken=True)
    check(debug_built.returncode != 0, "a debug build hit the cache; `-g` must always link")
    settled = fingerprint_of(hit, "hit").read_bytes()
    check(burdin_run(["build", ".", "-g", "-o", "debug-out"], hit, home).returncode == 0, "the debug build failed")
    check(fingerprint_of(hit, "hit").read_bytes() == settled, "a debug build rewrote the cache's fingerprint")

    # ---- the object cache -----------------------------------------------------
    def objects_of(package: Path, profile: str = "debug", kind: str = "bin") -> dict:
        directory = package / "target" / "burdin" / profile / "objects" / kind
        if not directory.is_dir():
            return {}
        return {f.name: (f.stat().st_mtime_ns, f.read_bytes()) for f in directory.iterdir() if f.is_file()}

    def units_build(package: Path, *args):
        args = args or (".",)
        env = dict(os.environ)
        env["BURDIN_HOME"] = str(home)
        env["BURDIN_CODEGEN_UNITS"] = "3"
        env.pop("BURDIN_CC", None)
        return harness.run([burdin, "build", *[str(a) for a in args]], cwd=package, env=env)

    def many_functions(tag: str, comment: str = "") -> str:
        parts = []
        for i in range(12):
            parts.append(
                f"fn f{i}(n: i64): i64 {{\n    let mut total = n\n    for _ in 0..{i + 3} {{\n"
                f"        total = total + n * {i + 1}\n    }}\n    total\n}}\n\n"
            )
        calls = "".join(f"    println(\"{{f{i}({i})}}\")\n" for i in range(12))
        return "".join(parts) + f'fn main() {{\n    println("{tag}")\n{calls}}}\n{comment}'

    objs = make_package(work, "objs", many_functions("one"))
    check(units_build(objs).returncode == 0, "the first build with three units failed")
    first = objects_of(objs)
    check(len(first) >= 2, f"a build with three codegen units kept {len(first)} object(s) in the cache")
    check(all(re.fullmatch(r"[0-9a-f]{64}\.(o|obj)", name) for name in first), f"unexpected cache entries: {sorted(first)}")
    expected = run_output(cached_exe(objs, "objs"))

    write(objs / "src" / "main.bd", many_functions("one", "// a trailing comment\n"))
    check(units_build(objs).returncode == 0, "the build after a comment edit failed")
    second = objects_of(objs)
    check(second == first, "an edit that does not change the generated code did not reuse every cached object")
    check(run_output(cached_exe(objs, "objs")) == expected, "the program built from cached objects prints differently")

    write(objs / "src" / "main.bd", many_functions("two"))
    check(units_build(objs).returncode == 0, "the build after a code edit failed")
    third = objects_of(objs)
    kept = [name for name in third if name in first and third[name] == first[name]]
    check(len(kept) >= 1, "an edit to one function rebuilt every unit's object")
    check(set(third) != set(first), "an edit to one function reused every cached object")
    check(len(third) <= 3, f"objects of an earlier build were not removed: {len(third)} entries for three units")
    check(run_output(cached_exe(objs, "objs")) == expected.replace("one", "two"), "the edited program printed wrongly")

    fresh = make_package(work, "objsdebug", many_functions("one"))
    debug_units = units_build(fresh, ".", "-g", "-o", "dbg-out")
    check(debug_units.returncode == 0, f"the `-g` build with units failed: {stderr_of(debug_units)[:300]}")
    check(objects_of(fresh) == {}, "a debug build wrote objects to the cache")

    # ---- --locked is an input-free flag: a package without dependencies hits ---
    locked = make_package(work, "locked", program("locked"))
    check(burdin_run(["build", "--locked", "."], locked, home).returncode == 0, "`build --locked` failed")
    check(
        burdin_run(["build", "--locked", "."], locked, home, broken=True).returncode == 0,
        "`build --locked` did not hit the cache",
    )

if problems:
    print("\n".join(problems))
    print(f"{len(problems)} problem(s) in {checks} checks")
    sys.exit(1)
print(f"all {checks} target-cache checks passed")
