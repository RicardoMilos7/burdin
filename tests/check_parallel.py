#!/usr/bin/env python3
"""Checks that the parallel compiler is deterministic:
  * the parallel front end (issue #432): a package with many modules builds and runs with any BURDIN_JOBS value,
    and a package with errors reports byte-identical diagnostics whether the modules are parsed sequentially
    (BURDIN_JOBS=1) or in parallel;
  * parallel code generation (issue #334): a package large enough to be split into codegen units builds and runs
    with `-j 1`, `-j 2`, and `-j 8` in both profiles and, on Linux, links the same executable bytes every time,
    and a forced split (BURDIN_CODEGEN_UNITS=8) passes the
    code-generation-heavy conformance milestones;
  * `-j` (issue #352): every compiling command accepts it, and a bad value is a usage error;
  * `burdin fmt -j` (issue #773): the output, the written files, and the exit status do not depend on it.

Needs a built driver. BURDIN selects it, like tests/run_tests.py.
"""

import difflib
import os
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()


def make_package(directory, count: int, body: str) -> bool:
    try:
        source = directory / "src"
        source.mkdir(parents=True)
        (directory / "burdin.toml").write_bytes(b'[package]\nname = "app"\n')
        uses = ""
        terms = []
        for index in range(count):
            name = f"m{index:02d}"
            harness.write_text(source / f"{name}.bd", f"fn f(): i32 {{\n    {body}\n}}\n")
            uses += f"\nuse {name}"
            terms.append(f"{name}.f()")
        harness.write_text(
            source / "main.bd",
            f"{uses}\n\nfn main() {{\n    println(({' + '.join(terms)}).to_string())\n}}\n",
        )
        return True
    except OSError:
        return False


with harness.temporary_dir("burdin-par-") as work:
    # --- A large package builds and runs with any number of workers ---------------
    if make_package(work / "good", 24, "1"):
        for jobs in (1, 2, 8):
            checks += 1
            env = {**os.environ, "BURDIN_JOBS": str(jobs)}
            result = harness.run([burdin, "run", "good"], cwd=work, env=env)
            output = harness.text(result.stdout).rstrip("\n")
            if result.returncode != 0:
                detail = "\n".join(harness.text(result.stderr).split("\n")[:5])
                add_problem(f"BURDIN_JOBS={jobs}: exit status {result.returncode}: {detail}")
            elif output != "24":
                add_problem(f"BURDIN_JOBS={jobs}: printed '{output}', expected '24'")
    else:
        add_problem("could not create the multi-module package")

    # --- Diagnostics are identical in both modes ---------------------------------
    # Every module has its own unknown name, so each contributes one E0002 at the same line.
    if make_package(work / "bad", 12, "missing_name"):
        sequential = harness.run(
            [burdin, "build", "bad"], cwd=work, env={**os.environ, "BURDIN_JOBS": "1"}
        )
        parallel = harness.run(
            [burdin, "build", "bad"], cwd=work, env={**os.environ, "BURDIN_JOBS": "8"}
        )
        checks += 1
        if sequential.returncode == 0 or parallel.returncode == 0:
            add_problem(
                "the failing package compiled "
                f"(sequential {sequential.returncode}, parallel {parallel.returncode})"
            )
        checks += 1
        if sequential.stderr != parallel.stderr:
            diff = difflib.unified_diff(
                harness.text(sequential.stderr).split("\n"),
                harness.text(parallel.stderr).split("\n"),
                fromfile="sequential",
                tofile="parallel",
                lineterm="",
            )
            detail = "\n".join(list(diff)[:20])
            add_problem(f"diagnostics differ between sequential and parallel parsing: {detail}")
        checks += 1
        sequential_errors = harness.count_lines_containing(sequential.stderr, b"error[E0002]")
        if sequential_errors < 12:
            add_problem("the sequential run did not report every module's error")
        checks += 1
        parallel_errors = harness.count_lines_containing(parallel.stderr, b"error[E0002]")
        if parallel_errors != sequential_errors:
            add_problem("the parallel run reported a different number of errors")
    else:
        add_problem("could not create the failing package")

def chunk(tag: str, index: int) -> str:
    """One generated unit of the codegen package: a struct with methods, an enum and a `match`, a generic
    function, a loop, a `List`, and a `String`."""
    n = f"{tag}{index}"
    return f"""struct V{n} derives (Copy, Clone, Eq) {{
    a: i32
    b: i64
    on: bool
}}

enum K{n} {{
    Unit
    Pair(i32, i64)
    Record {{ x: i32, on: bool }}
}}

V{n} {{
    fn new(a: i32, b: i64, on: bool): V{n} {{
        V{n} {{ a, b, on }}
    }}

    fn total(self): i64 {{
        self.a as i64 + self.b
    }}
}}

fn kind{n}(value: K{n}): i64 {{
    match value {{
        K{n}.Unit => 0,
        K{n}.Pair(x, y) => x as i64 + y,
        K{n}.Record {{ x, on }} => if on {{ x as i64 }} else {{ 0 - x as i64 }},
    }}
}}

fn maxv{n}<T: Ord>(own a: T, own b: T): T {{
    if a > b {{ a }} else {{ b }}
}}

fn fold{n}(n: i32): i64 {{
    let mut total: i64 = 0
    let mut k = 0
    while k < n {{
        let v = V{n}.new(k, k as i64, k % 2 == 0)
        total = total + v.total()
        let w = maxv{n}(k, 0 - k)
        total = total + w as i64
        k = k + 1
    }}
    let mut xs = List<i32>.new()
    xs.push(3)
    for x in xs {{
        total = total + x as i64
    }}
    let mut s = String.new()
    s.push_str("ab")
    total + s.len() as i64 + kind{n}(K{n}.Pair(1, 2))
}}

"""


def make_codegen_package(directory, modules: int, units: int) -> bool:
    try:
        source = directory / "src"
        source.mkdir(parents=True)
        (directory / "burdin.toml").write_bytes(b'[package]\nname = "big"\n')
        uses = ""
        terms = ""
        for module in range(modules):
            text = "".join(chunk(f"M{module}_", index) for index in range(units))
            text += "fn total(): i64 {\n    let mut a: i64 = 0\n"
            text += "".join(f"    a = a + foldM{module}_{index}(3)\n" for index in range(units))
            text += "    a\n}\n"
            harness.write_text(source / f"m{module}.bd", text)
            uses += f"use m{module}\n"
            terms += f"    t = t + m{module}.total()\n"
        harness.write_text(
            source / "main.bd",
            f"{uses}\nfn main() {{\n    let mut t: i64 = 0\n{terms}    println(t.to_string())\n}}\n",
        )
        return True
    except OSError:
        return False


def make_fmt_tree(directory) -> None:
    """Twelve files, four of them not formatted, and one with a syntax error."""
    for index in range(12):
        if index % 3 == 0:
            body = f"fn f{index}( ): i32 {{ {index} }}\n"
        else:
            body = f"fn f{index}(): i32 {{\n    {index}\n}}\n"
        harness.write_text(directory / f"f{index:02d}.bd", body)
    harness.write_text(directory / "broken.bd", "fn broken( {\n")


with harness.temporary_dir("burdin-cgu-") as work:
    # --- Parallel code generation: the same program and the same bytes with any -j ------------------
    # 16 modules of 12 units is about 65,000 instructions, which the compiler splits into several units.
    clean_env = {key: value for key, value in os.environ.items() if key not in ("BURDIN_JOBS", "BURDIN_CODEGEN_UNITS")}
    if make_codegen_package(work / "big", 16, 12):
        for profile in ([], ["--release"]):
            label = "release" if profile else "debug"
            binaries = {}
            for jobs in ("1", "2", "8"):
                checks += 1
                out = work / f"big-{label}-{jobs}"
                build = harness.run([burdin, "build", "big", *profile, "-j", jobs, "-o", out], cwd=work, env=clean_env)
                if build.returncode != 0:
                    detail = "\n".join(harness.text(build.stderr).split("\n")[:8])
                    add_problem(f"{label} -j {jobs}: the build failed ({build.returncode}): {detail}")
                    continue
                exe = harness.executable_in(work, out.name)
                result = harness.run([exe], cwd=work)
                output = harness.text(result.stdout).strip()
                if result.returncode != 0 or output != "3264":
                    add_problem(f"{label} -j {jobs}: printed '{output}' (status {result.returncode}), expected '3264'")
                binaries[jobs] = harness.read_bytes(exe)
            # The GNU linker is deterministic; link.exe stamps the time, and ld64 is not compared.
            if harness.platform_name() == "linux" and len(binaries) == 3:
                checks += 1
                if len(set(binaries.values())) != 1:
                    add_problem(f"{label}: the executable depends on -j (the codegen units must not)")
    else:
        add_problem("could not create the codegen package")

    # --- A forced split runs the code-generation-heavy milestones unchanged -------------------------
    # Every program is split into 8 units, so functions, globals, vtables, closures, task entry points, and
    # the runtime cross unit boundaries, and each unit is compiled on its own thread.
    checks += 1
    tests_dir = os.path.dirname(os.path.abspath(__file__))
    forced = harness.run(
        [sys.executable, os.path.join(tests_dir, "run_tests.py"), "m11_llvm", "m12_closures", "m15_any", "s2_tasks"],
        env={**clean_env, "BURDIN_CODEGEN_UNITS": "8", "BURDIN": str(burdin)},
        merge=True,
    )
    lines = harness.text(forced.stdout).split("\n")
    summary = [line for line in lines if " passed, " in line]
    if forced.returncode != 0 or not summary:
        failures = [line for line in lines if line.startswith("FAIL")]
        add_problem(f"BURDIN_CODEGEN_UNITS=8: conformance tests failed: {failures[:10]} {summary}")

    # --- -j is an option of every compiling command ---------------------------------------------------
    harness.write_text(work / "one.bd", "fn main() {\n    println(\"one\")\n}\n")
    commands = (
        ["build", "one.bd", "-o", work / "one"],
        ["run", "one.bd"],
        ["bench", "one.bd"],
        ["check", "one.bd"],
        ["lint", "one.bd"],
        ["doc", "one.bd", "--coverage"],
        ["fmt", "--check", "one.bd"],
    )
    for command in commands:
        for jobs, usage_error in (("-j3", False), ("--jobs=1", False), ("-j0", True), ("-jmany", True)):
            checks += 1
            result = harness.run([burdin, command[0], jobs, *command[1:]], cwd=work, env=clean_env)
            # Only the usage matters here: lint, doc, and fmt may report findings (status 1) about the file.
            if (result.returncode == 2) != usage_error:
                add_problem(
                    f"`burdin {command[0]} {jobs}` exited with {result.returncode}: "
                    + harness.text(result.stderr).split("\n")[0]
                )

    # --- burdin fmt -j: the same report, files, and status with any thread count ----------------------
    runs = {}
    for jobs in ("1", "8"):
        tree = work / f"fmt-{jobs}"
        tree.mkdir()
        make_fmt_tree(tree)
        check = harness.run([burdin, "fmt", "--check", "-j", jobs, "--color", "never", "."], cwd=tree, env=clean_env)
        write = harness.run([burdin, "fmt", "-j", jobs, "--color", "never", "."], cwd=tree, env=clean_env)
        files = {path.name: harness.read_bytes(path) for path in sorted(tree.iterdir())}
        runs[jobs] = (check.returncode, check.stdout, check.stderr, write.returncode, write.stdout, write.stderr, files)
    checks += 1
    if runs["1"] != runs["8"]:
        add_problem("`burdin fmt -j 1` and `-j 8` differ in output, status, or the files they wrote")
    checks += 1
    if runs["1"][0] != 2 or runs["1"][3] != 2:
        add_problem(f"fmt over a tree with a syntax error exited {runs['1'][0]} and {runs['1'][3]}, expected 2")
    checks += 1
    if harness.count_lines_containing(runs["1"][4], b".bd") != 4:
        add_problem("`burdin fmt` did not list exactly the four unformatted files")


print(f"Checked parallel compilation determinism ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
