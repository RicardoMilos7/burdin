#!/usr/bin/env python3
"""Checks named build profiles (`spec/profiles.md`, issue #339).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * the default `dev` build panics on integer overflow, and `--profile release` and `--release` wrap;
  * a custom profile that inherits `dev` and turns `overflow-checks` off wraps;
  * `debug-assertions` decides whether `debug_assert` runs, in a build and in `burdin test`;
  * a per-package override changes only that package's functions;
  * `debug = true` and `opt-level` change the generated IR, and the same profile produces the same IR twice;
  * `-O0` to `-O3` (and `-O <n>`) replace a profile's `opt-level` for one build, and `dev -O3` keeps the overflow
    check while `--release -O0` wraps (issue #289);
  * an unknown profile, an unknown setting, a bad value, `panic = "unwind"`, a per-package `opt-level`, and an
    inheritance cycle fail with the documented message and exit status;
  * `--help` documents `--profile`.
"""

import sys
from pathlib import Path

import harness

problems = []
checks = 0


def check(condition: bool, message: str) -> None:
    global checks
    checks += 1
    if not condition:
        problems.append(message)


burdin = harness.require_burdin()

OVERFLOW_MAIN = """fn main() {
    let x: i32 = 2147483647
    let y: i32 = x + 1
    println("{}", y)
}
"""

DEBUG_ASSERT_MAIN = """fn main() {
    debug_assert(false)
    println("ok")
}
"""


def run(*arguments, cwd=None):
    return harness.run([burdin, *arguments], cwd=cwd)


def stdout(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def stderr(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


def package(root: Path, name: str, main: str, manifest: str = "") -> Path:
    directory = root / name
    (directory / "src").mkdir(parents=True)
    harness.write_text(directory / "burdin.toml", f'[package]\nname = "{name}"\nversion = "0.1.0"\n{manifest}')
    harness.write_text(directory / "src" / "main.bd", main)
    return directory


def expect_ok(result, what: str) -> None:
    check(result.returncode == 0, f"{what} failed ({result.returncode}): {stderr(result)[:500]}")


def expect_failure(result, status: int, needle: str, what: str) -> None:
    check(result.returncode == status, f"{what} exited {result.returncode}, expected {status}")
    check(needle in stderr(result), f"{what} does not report `{needle}`:\n{stderr(result)[:600]}")


with harness.temporary_dir("burdin-profiles-") as work:
    overflow = package(work, "overflow", OVERFLOW_MAIN)

    # BP1: dev checks overflow; release wraps.
    dev = run("run", overflow, cwd=work)
    check(dev.returncode != 0, "the dev profile did not fail on overflow")
    expect_failure(dev, dev.returncode, "attempt to add with overflow", "the dev overflow run")

    release = run("run", overflow, "--profile", "release", cwd=work)
    expect_ok(release, "`--profile release`")
    check(stdout(release).strip() == "-2147483648", f"release printed {stdout(release)!r}")

    spelled = run("run", overflow, "--release", cwd=work)
    expect_ok(spelled, "`--release`")
    check(stdout(spelled) == stdout(release), "`--release` and `--profile release` disagree")

    # BP2-BP3: a custom profile inherits dev and changes one setting.
    fast = package(
        work,
        "fast",
        OVERFLOW_MAIN,
        '\n[profile.fast]\ninherits = "dev"\noverflow-checks = false\nopt-level = 3\n',
    )
    custom = run("run", fast, "--profile", "fast", cwd=work)
    expect_ok(custom, "`--profile fast`")
    check(stdout(custom).strip() == "-2147483648", f"the custom profile printed {stdout(custom)!r}")

    # BP1/BP5: debug-assertions decides whether debug_assert runs.
    asserts = package(work, "asserts", DEBUG_ASSERT_MAIN)
    kept = run("run", asserts, cwd=work)
    check(kept.returncode != 0 and "assertion failed" in stderr(kept), f"dev kept no assertion: {stderr(kept)[:300]}")
    dropped = run("run", asserts, "--profile", "release", cwd=work)
    expect_ok(dropped, "the release debug_assert run")
    check(stdout(dropped).strip() == "ok", f"release printed {stdout(dropped)!r}")

    # BP4: the test profile is the default of `burdin test`, and a manifest can change it.
    test_program = """test "the sum" {
    let x: i32 = 2147483647
    let y: i32 = x + 1
    assert_eq(y, -2147483648)?
}
"""
    test_default = package(work, "testdefault", test_program)
    failing = run("test", test_default, cwd=work)
    check(failing.returncode != 0, "the default test profile did not fail on overflow")
    test_wrap = package(
        work,
        "testwrap",
        test_program,
        "\n[profile.test]\noverflow-checks = false\n",
    )
    passing = run("test", test_wrap, cwd=work)
    expect_ok(passing, "`burdin test` with `[profile.test] overflow-checks = false`")
    check("1 passed" in stdout(passing), f"`burdin test` printed {stdout(passing)!r}")

    # BP7: a per-package override applies to that package's functions only.
    dep = work / "perapp" / "dep"
    (dep / "src").mkdir(parents=True)
    harness.write_text(dep / "burdin.toml", '[package]\nname = "dep"\nversion = "0.1.0"\n')
    harness.write_text(
        dep / "src" / "dep.bd",
        """fn add(a: i32, b: i32): i32 {
    a + b
}
""",
    )
    perapp = work / "perapp" / "app"
    (perapp / "src").mkdir(parents=True)
    harness.write_text(
        perapp / "burdin.toml",
        """[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = { path = "../dep" }

[profile.dev.package.dep]
overflow-checks = false
""",
    )
    harness.write_text(
        perapp / "src" / "main.bd",
        """fn main() {
    let a: i32 = dep.add(2147483647, 1)
    println("{}", a)
    let b: i32 = 2147483647 + 1
    println("{}", b)
}
""",
    )
    mixed = run("run", perapp, cwd=work)
    check(mixed.returncode != 0, "the entry package's own overflow check disappeared")
    check(stdout(mixed).strip() == "-2147483648", f"the dependency did not wrap: {stdout(mixed)!r}")
    check("attempt to add with overflow" in stderr(mixed), f"the entry package did not panic:\n{stderr(mixed)[:400]}")

    # The other direction: false for the dependency by default, true for it through the override.
    dep2 = work / "assertdep" / "dep"
    (dep2 / "src").mkdir(parents=True)
    harness.write_text(dep2 / "burdin.toml", '[package]\nname = "dep"\nversion = "0.1.0"\n')
    harness.write_text(
        dep2 / "src" / "dep.bd",
        """fn check() {
    debug_assert(false)
}
""",
    )
    app2 = work / "assertdep" / "app"
    (app2 / "src").mkdir(parents=True)
    harness.write_text(
        app2 / "burdin.toml",
        """[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = { path = "../dep" }

[profile.dev.package.dep]
debug-assertions = false
""",
    )
    harness.write_text(app2 / "src" / "main.bd", 'fn main() {\n    dep.check()\n    println("ok")\n}\n')
    dep_quiet = run("run", app2, cwd=work)
    expect_ok(dep_quiet, "a dependency with `debug-assertions = false`")
    check(stdout(dep_quiet).strip() == "ok", f"the dependency printed {stdout(dep_quiet)!r}")

    # BP5: a profile changes codegen deterministically.
    ir_program = """fn sum(n: i32): i32 {
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + i
        i = i + 1
    }
    total
}

fn main() {
    println("{}", sum(10))
}
"""
    ir_package = package(work, "ir", ir_program)
    out = work / "irbin"
    opt_ll = {}
    for profile in ["dev", "release", "dev2"]:
        target = work / f"irbin-{profile}"
        built = run("build", ir_package, "--profile", profile.replace("dev2", "dev"), "--emit", "llvm-ir-opt",
                    "-o", str(target), cwd=work)
        expect_ok(built, f"`build --profile {profile}`")
        opt_ll[profile] = harness.read_text(Path(str(target) + ".opt.ll"))
    check(opt_ll["dev"] != opt_ll["release"], "dev and release optimized IR are identical")
    check(opt_ll["dev"] == opt_ll["dev2"], "two dev builds wrote different IR")

    # `debug = true` adds debug metadata; the default `dev` has none.
    traced = package(
        work,
        "traced",
        "fn main() {\n    println(\"ok\")\n}\n",
        "\n[profile.traced]\ninherits = \"dev\"\ndebug = true\n",
    )
    plain_ir = work / "plain.ll"
    traced_ir = work / "traced.ll"
    plain = run("build", traced, "--emit-ir", str(plain_ir), "-o", str(work / "plainbin"), cwd=work)
    expect_ok(plain, "the default IR build")
    with_debug = run("build", traced, "--profile", "traced", "--emit-ir", str(traced_ir), "-o",
                     str(work / "tracedbin"), cwd=work)
    expect_ok(with_debug, "the `debug = true` IR build")
    check("DILocation" not in harness.read_text(plain_ir), "the default build emitted debug metadata")
    check("DILocation" in harness.read_text(traced_ir), "`debug = true` emitted no debug metadata")

    # BP3-BP5 (issue #289): `-O<n>` replaces the optimization level and nothing else. The IR embeds the source path,
    # so every comparison here is between builds of one package.
    levels = package(work, "levels", ir_program)

    def emit_opt(*flags, name):
        target = work / f"levels-{name}"
        built = run("build", levels, *flags, "--emit", "llvm-ir-opt", "-o", str(target), cwd=work)
        expect_ok(built, f"`build {' '.join(flags)}`")
        return harness.read_text(Path(str(target) + ".opt.ll"))

    default_ir = emit_opt(name="default")
    o0_ir = emit_opt("-O0", name="o0")
    o2_ir = emit_opt("-O2", name="o2")
    o2_spaced_ir = emit_opt("-O", "2", name="o2s")
    check(default_ir == o0_ir, "`-O0` is not the default `dev` IR")
    check(o2_ir != o0_ir, "`-O2` did not change the optimized IR")
    check(o2_ir == o2_spaced_ir, "`-O 2` and `-O2` wrote different IR")

    # BP3/BP4: `opt-level` in a manifest is the level a build uses, and `-O` replaces it for one invocation.
    configured = package(work, "configured", ir_program, "\n[profile.dev]\nopt-level = 2\n")

    def emit_configured(*flags, name):
        target = work / f"configured-{name}"
        built = run("build", configured, *flags, "--emit", "llvm-ir-opt", "-o", str(target), cwd=work)
        expect_ok(built, f"`build {' '.join(flags)}` with `[profile.dev] opt-level = 2`")
        return harness.read_text(Path(str(target) + ".opt.ll"))

    configured_ir = emit_configured(name="default")
    check(configured_ir == emit_configured("-O2", name="o2"), "`[profile.dev] opt-level = 2` is not level 2")
    check(configured_ir != emit_configured("-O0", name="o0"), "`-O0` did not replace the manifest's level")
    check(
        emit_configured("-O0", name="o0b") == emit_configured("--profile", "test", name="test"),
        "`-O0` is not the `test` profile's level 0 build",
    )

    # BP5: the level leaves the checks alone, in both directions.
    overflow_o3 = run("run", overflow, "-O3", cwd=work)
    check(
        overflow_o3.returncode != 0 and "attempt to add with overflow" in stderr(overflow_o3),
        f"`dev -O3` did not keep the overflow check:\n{stderr(overflow_o3)[:300]}",
    )
    wrap_o0 = run("run", overflow, "--release", "-O0", cwd=work)
    expect_ok(wrap_o0, "`--release -O0`")
    check(stdout(wrap_o0).strip() == "-2147483648", f"`--release -O0` printed {stdout(wrap_o0)!r}")
    test_o1 = run("test", test_wrap, "-O1", cwd=work)
    expect_ok(test_o1, "`burdin test -O1`")
    check("1 passed" in stdout(test_o1), f"`burdin test -O1` printed {stdout(test_o1)!r}")

    # BP4: the usage errors.
    expect_failure(run("build", levels, "-O4", cwd=work), 2, "needs an optimization level", "`-O4`")
    expect_failure(run("build", levels, "-O", cwd=work), 2, "needs an optimization level", "`-O` with no level")
    expect_failure(run("build", levels, "-O1", "-O2", cwd=work), 2, "conflicts with `-O1`", "`-O1 -O2`")

    # BP3/BP4: the failure paths.
    unknown = run("build", overflow, "--profile", "nope", cwd=work)
    expect_failure(unknown, 2, "unknown profile `nope`", "an unknown profile")
    check(
        all(name in stderr(unknown) for name in ("dev", "release", "test", "bench")),
        f"the unknown profile does not list the built-ins:\n{stderr(unknown)[:400]}",
    )

    conflict = run("build", overflow, "--release", "--profile", "fast", cwd=work)
    expect_failure(conflict, 2, "conflicts with `--profile fast`", "`--release --profile fast`")

    bad_opt = package(work, "badopt", OVERFLOW_MAIN, '\n[profile.fast]\ninherits = "dev"\nopt-level = 9\n')
    expect_failure(run("build", bad_opt, cwd=work), 1, "opt-level", "an out-of-range opt-level")

    bad_key = package(work, "badkey", OVERFLOW_MAIN, '\n[profile.fast]\ninherits = "dev"\nthreads = 4\n')
    expect_failure(run("build", bad_key, cwd=work), 1, "not a profile setting", "an unknown profile setting")

    unwind = package(work, "unwind", OVERFLOW_MAIN, '\n[profile.fast]\ninherits = "dev"\npanic = "unwind"\n')
    expect_failure(run("build", unwind, cwd=work), 1, "only strategy", "`panic = \"unwind\"`")

    cycle = package(
        work,
        "cycle",
        OVERFLOW_MAIN,
        '\n[profile.a]\ninherits = "b"\n\n[profile.b]\ninherits = "a"\n',
    )
    expect_failure(run("build", cycle, "--profile", "a", cwd=work), 1, "cycle", "an inheritance cycle")

    narrowed = package(
        work,
        "narrowed",
        OVERFLOW_MAIN,
        '\n[profile.fast]\ninherits = "dev"\n\n[profile.fast.package.other]\nopt-level = 3\n',
    )
    expect_failure(run("build", narrowed, "--profile", "fast", cwd=work), 1, "setting of the whole build",
                   "a per-package opt-level")

    no_parent = package(work, "noparent", OVERFLOW_MAIN, "\n[profile.fast]\nopt-level = 3\n")
    expect_failure(run("build", no_parent, "--profile", "fast", cwd=work), 1, "inherits", "a custom profile without a parent")

    # BP4: the command line documents the option.
    help_text = stdout(run("build", "--help"))
    check("--profile <name>" in help_text, "`burdin build --help` does not document `--profile`")
    check("-O <n>" in help_text, "`burdin build --help` does not document `-O`")

print(f"check_profiles: {checks} checks")
sys.exit(harness.report_problems(problems))
