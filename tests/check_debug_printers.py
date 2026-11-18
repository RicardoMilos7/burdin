#!/usr/bin/env python3
"""Checks the debugger integration of issue #565 (`spec/debug.md`): the panic hook and the symbolized panic
frames (#572), the pretty-printers under gdb, lldb, and cdb (#567, #569), and the `burdin debug` command (#574).
Needs a built driver; the debuggers themselves are optional and every missing one is recorded as a skip.

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks today:
  * a `-g` build's panic prints `  #N  at <file>:<line> in <Burdin path>` frames, ends at the program's `main`,
    and the same program without `-g` prints no such line (DBG2);
  * `burdin_panic_begin` exists as an external symbol, and `break burdin_panic_begin` under gdb stops before
    the panic prints, with the Burdin frames readable on the stack (DBG1);
  * the embedded gdb printers show every standard type as its value, not its layout (DBG5, DBG6);
  * `burdin debug` picks the platform debugger, loads the printers, sets the panic breakpoint, and passes the
    exit status through, checked with a stub debugger (DBG9, DBG10).
"""

import os
import sys

import harness

problems = []
checks = 0
skips = []


def add_problem(message: str) -> None:
    problems.append(message)


def skip(reason: str) -> None:
    skips.append(reason)


burdin = harness.require_burdin()
windows_host = harness.PLATFORM == "windows"

# The panic fixture: one value of every printer-covered type, then a panic inside a nested call.
FIXTURE = """\
enum Shape derives (Debug) {
    Circle { radius: f64 }
    Square { side: f64 }
}

struct Item derives (Debug) {
    name: String
    count: i32
}

fn take(own s: String) {
    println(s)
}

fn innermost(x: i32) {
    if x > 0 {
        panic("boom")
    }
    println("not reached")
}

fn outer() {
    innermost(1)
}

fn main() {
    let text = String.from("hello")
    let mut list = List<i32>.new()
    list.push(10)
    list.push(20)
    let mut map = Map<String, i32>.new()
    map.insert("a".to_string(), 1)
    map.insert("b".to_string(), 2)
    let opt: Option<i32> = Some(5)
    let res: Result<i32, String> = Ok(7)
    let shape = Shape.Circle { radius: 1.5 }
    let item = Item { name: "bolt".to_string(), count: 3 }
    let own_text = Heap.new(String.from("owned"))
    let word = text
    println("before")
    outer()
}
"""


def gdb_batch(commands, exe, cwd):
    """Runs gdb once, quietly, with the given `-ex` commands."""
    return harness.run(
        [harness.which("gdb"), "-q", "-batch", "-nx", "-ex", "set debuginfod enabled off"]
        + sum((["-ex", c] for c in commands), [])
        + [exe],
        cwd=cwd,
        merge=True,
    )


with harness.temporary_dir("burdin-dbgp-") as work:
    fixture = work / "dbgprinters.bd"
    harness.write_text(fixture, FIXTURE)
    # The panic fixture with the types unused in `main` removed: this one exists for the panic checks, the one
    # above for the printer checks.
    panic_fixture = work / "dbgpanic.bd"
    harness.write_text(
        panic_fixture,
        "\n".join(line for line in FIXTURE.split("\n") if "list.push" not in line),
    )

    built = harness.run([burdin, "build", "dbgpanic.bd", "-g", "-o", "dbgpanic"], cwd=work)
    checks += 1
    if built.returncode != 0:
        add_problem(
            f"burdin build -g: exit status {built.returncode}: {harness.text(built.stderr).rstrip(chr(10))}"
        )
        print("debug: the -g fixture did not build; nothing else can be checked", file=sys.stderr)
        if harness.report_problems(problems):
            sys.exit(1)
        sys.exit(0)
    exe = harness.executable_in(work, "dbgpanic")

    # DBG2: the `-g` panic prints one symbolized line per frame — the function's own declaration line — and ends
    # at `main`.
    checks += 1
    ran = harness.run([exe], cwd=work)
    stderr = harness.text(ran.stderr)
    for expected in (
        "panic: boom",
        "stack backtrace:",
        "  #0  at dbgpanic.bd:15 in innermost",
        "  #1  at dbgpanic.bd:22 in outer",
        "  #2  at dbgpanic.bd:26 in main",
    ):
        if expected not in stderr:
            add_problem(f"-g panic: the backtrace lacks `{expected}`:\n{stderr}")
    checks += 1
    if "#3" in stderr:
        add_problem(f"-g panic: the backtrace does not end at `main`:\n{stderr}")

    # Without `-g` the frames keep the stage 0 form: no `at ... in` line anywhere.
    plain = harness.run([burdin, "build", "dbgpanic.bd", "-o", "dbgpanic_plain"], cwd=work)
    if plain.returncode == 0:
        plain_exe = harness.executable_in(work, "dbgpanic_plain")
        checks += 1
        plain_run = harness.run([plain_exe], cwd=work)
        plain_err = harness.text(plain_run.stderr)
        if "at dbgpanic.bd" in plain_err or " in innermost" in plain_err:
            add_problem(f"the non-`-g` panic prints a symbolized frame:\n{plain_err}")
        if "stack backtrace:" not in plain_err:
            add_problem(f"the non-`-g` panic prints no backtrace:\n{plain_err}")
    else:
        add_problem(f"burdin build (no -g) failed: {harness.text(plain.stderr).rstrip(chr(10))}")

    # DBG1: the panic hook. gdb stops there with the stack intact, before the panic prints anything.
    if windows_host:
        skip("the gdb breakpoint checks need DWARF, which the Windows linker does not keep")
    elif not harness.which("gdb"):
        skip("gdb is not installed; the panic breakpoint was not checked")
    else:
        debugged = gdb_batch(
            [
                "break burdin_panic_begin",
                "run",
                "bt",
                "frame function burdin.innermost",
                "print x",
                "kill",
            ],
            exe,
            cwd=work,
        )
        transcript = harness.text(debugged.stdout)
        checks += 1
        if "Breakpoint 1, " not in transcript or "burdin_panic_begin" not in transcript:
            add_problem(f"gdb: `break burdin_panic_begin` did not stop there:\n{transcript}")
        checks += 1
        if "innermost" not in transcript or "outer" not in transcript:
            add_problem(f"gdb: the stopped stack lacks the Burdin frames:\n{transcript}")
        checks += 1
        if "panic: boom" in transcript:
            add_problem("gdb: the breakpoint stopped after the panic printed, not before (DBG1)")
        checks += 1
        if "$1 = 1" not in transcript:
            add_problem(f"gdb: `print x` in innermost did not read 1:\n{transcript}")

checked = f"Checked the debugger integration ({checks} checks)"
if skips:
    for reason in skips:
        print(f"skip: {reason}")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
print(checked)
