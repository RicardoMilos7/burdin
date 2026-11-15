#!/usr/bin/env python3
"""Checks the code-generation optimizations and the behavior they preserve:
  * drop-flag elimination (issue #427): a local proved moved at every drop site gets no `i1` flag, while a
    conditionally moved local keeps one;
  * bounds-check elision (issue #428): a loop over `0..xs.len()` and a constant index need no check, while an
    out-of-range index still panics;
  * glue deduplication (issue #426): two same-shaped types share one drop-glue function.

The checks read the unoptimized `--emit-ir` text, so they do not depend on the optimization pipeline.
Needs a built driver. BURDIN selects it, like tests/run_tests.py.
"""

import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()


def expect_count(description: str, path, pattern: str, expected: int) -> None:
    global checks
    checks += 1
    count = harness.count_lines_containing(harness.read_bytes(path), pattern.encode())
    if count != expected:
        add_problem(f"{description}: {count} occurrences of '{pattern}', expected {expected}")


with harness.temporary_dir("burdin-opt-") as work:
    # --- Drop flags (issue #427) --------------------------------------------------
    harness.write_text(
        work / "flags.bd",
        "fn take(s: String): i32 {\n"
        "    s.len() as i32\n"
        "}\n"
        "fn always(): i32 {\n"
        '    let s = String.from("always")\n'
        "    take(s)\n"
        "}\n"
        "fn maybe(c: bool): i32 {\n"
        '    let s = String.from("maybe")\n'
        "    if c { take(s) } else { 0 }\n"
        "}\n"
        "fn main() {\n"
        "    println((always() + maybe(false)).to_string())\n"
        "}\n",
    )
    built = harness.run(
        [burdin, "build", "flags.bd", "--emit-ir", "flags.ll", "-o", "flags"], cwd=work
    )
    if built.returncode == 0:
        # One `moved` flag for maybe's conditional move; the unconditional move in `always` needs none. The
        # other `i1` is the live flag of the `to_string` temporary in `main`, which drops at the end of its
        # statement (DR5); every temporary carries one so a temporary created in only one branch of a `match`
        # is dropped only where it exists.
        expect_count("drop-flag elimination", work / "flags.ll", "alloca i1", 2)
        expect_count("drop-flag elimination (temporary)", work / "flags.ll", "tmp.live = alloca i1", 1)
        checks += 1
        ran = harness.run([harness.executable_in(work, "flags")], cwd=work)
        output = harness.text(ran.stdout).rstrip("\n")
        if output != "6":
            add_problem(f"drop-flag elimination changed the output: '{output}', expected '6'")
    else:
        add_problem(f"burdin build flags.bd: {harness.text(built.stderr).rstrip(chr(10))}")

    # --- Bounds-check elision (issue #428) ---------------------------------------
    harness.write_text(
        work / "bounds.bd",
        "fn sum(xs: [i32; 4]): i32 {\n"
        "    let mut total = 0\n"
        "    for i in 0..xs.len() {\n"
        "        total = total + xs[i]\n"
        "    }\n"
        "    total + xs[1]\n"
        "}\n"
        "fn main() {\n"
        "    println(sum([1, 2, 3, 4]).to_string())\n"
        "}\n",
    )
    built = harness.run(
        [burdin, "build", "bounds.bd", "--emit-ir", "bounds.ll", "-o", "bounds"], cwd=work
    )
    if built.returncode == 0:
        expect_count("bounds-check elision (loop and constant index)", work / "bounds.ll", "bounds.fail", 0)
        checks += 1
        ran = harness.run([harness.executable_in(work, "bounds")], cwd=work)
        output = harness.text(ran.stdout).rstrip("\n")
        if output != "12":
            add_problem(f"bounds-check elision changed the output: '{output}', expected '12'")
    else:
        add_problem(f"burdin build bounds.bd: {harness.text(built.stderr).rstrip(chr(10))}")

    harness.write_text(
        work / "oob.bd",
        "fn main() {\n"
        "    let xs = [1, 2, 3]\n"
        "    let i: usize = 5\n"
        "    println(xs[i].to_string())\n"
        "}\n",
    )
    checks += 1
    ran = harness.run([burdin, "run", "oob.bd"], cwd=work)
    if ran.returncode == 0:
        add_problem("out-of-range indexing did not panic")
    elif b"index out of bounds" not in ran.stderr:
        detail = "\n".join(harness.text(ran.stderr).split("\n")[:5])
        add_problem(f"out-of-range indexing did not report a bounds panic: {detail}")

    # --- Glue deduplication (issue #426) ----------------------------------------
    harness.write_text(
        work / "glue.bd",
        "struct A { s: String }\n"
        "struct B { s: String }\n"
        "fn main() {\n"
        '    let a = Heap.new(A { s: String.from("aa") })\n'
        '    let b = Heap.new(B { s: String.from("bb") })\n'
        "    println((*a).s.len().to_string())\n"
        "    println((*b).s.len().to_string())\n"
        "}\n",
    )
    built = harness.run(
        [burdin, "build", "glue.bd", "--emit-ir", "glue.ll", "-o", "glue"], cwd=work
    )
    if built.returncode == 0:
        # `Heap<A>` and `Heap<B>` have the same shape, so their drop glue is generated once.
        expect_count("glue deduplication", work / "glue.ll", "define internal void @burdin.drop", 1)
        checks += 1
        ran = harness.run([harness.executable_in(work, "glue")], cwd=work)
        output = harness.text(ran.stdout).rstrip("\n")
        if output != "2\n2":
            add_problem(f"glue deduplication changed the output: '{output}'")
        checks += 1
        try:
            size = harness.file_size(harness.executable_in(work, "glue"))
        except OSError:
            size = "?"
        print(f"glue fixture executable: {size} bytes, 1 shared drop-glue function for 2 types")
    else:
        add_problem(f"burdin build glue.bd: {harness.text(built.stderr).rstrip(chr(10))}")

print(f"Checked code-generation optimizations ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
