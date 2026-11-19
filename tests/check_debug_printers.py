#!/usr/bin/env python3
"""Checks the debugger integration of issue #565 (`spec/debug.md`): the panic hook and the symbolized panic
frames (#572), the pretty-printers under gdb and lldb (#567), and the embedding of the gdb printers (#567).
Needs a built driver; the debuggers themselves are optional and every missing one is recorded as a skip.

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks today:
  * a `-g` build's panic prints `  #N  at <file>:<line> in <Burdin path>` frames, ends at the program's `main`,
    and the same program without `-g` prints no such line (DBG2);
  * `burdin_panic_begin` exists as an external symbol, and `break burdin_panic_begin` under gdb stops before
    the panic prints, with the Burdin frames readable on the stack (DBG1);
  * a `-g` binary carries the gdb printers in `.debug_gdb_scripts` and a non-`-g` one does not (DBG6);
  * the gdb printers show every standard type — `String`, `str`, `List`, `Map`, `Option`, `Result`, an enum, a
    struct, `Heap`, `Shared` with its strong count, `any I` with the concrete type from the vtable, a closure,
    and a range — as its value, not its layout (DBG5, DBG6);
  * the lldb formatters show the same values (DBG5, DBG7).
"""

import os
import sys
from xml.etree import ElementTree

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

# The panic fixture: a panic inside a nested call, for the panic hook and the symbolized frames.
FIXTURE = """\
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
    println("before")
    outer()
}
"""

# The printer fixture: one value of every printer-covered type, live in `main` when the panic stops the program
# inside `show`. `show` prints its `str` parameter first: without a use before the panic the parameter has no
# location of its own in the DWARF (the panic call is unreachable-terminated), and the printers are not what that
# check is about. The code after the panic keeps the call's return address inside `show`: a call that ends the
# function leaves it one past the end, and older gdb versions then find no `burdin.show` frame.
PRINTER_FIXTURE = """\
interface Measured {
    fn size(self): i32
}

struct Block {
    side: i32
}

Block: Measured {
    fn size(self): i32 {
        self.side
    }
}

enum Shape: Debug {
    Circle { radius: f64 }
    Square { side: f64 }
}

struct Item: Debug {
    name: String
    count: i32
}

fn show(word: str) {
    println(word)
    if word.len() > 0 {
        panic("show")
    }
    println("not reached")
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
    let none: Option<i32> = None
    let res: Result<i32, String> = Ok(7)
    let shape = Shape.Circle { radius: 1.5 }
    let unit = Shape.Square { side: 2.0 }
    let item = Item { name: "bolt".to_string(), count: 3 }
    let owned = Heap.new(String.from("owned"))
    let shared = Shared.new(String.from("shared"))
    let shared2 = shared.clone()
    let boxed: any Measured = Block { side: 4 }
    let step = |n: i32| n + 1
    let range = 0..10
    println("before")
    show(text)
}
"""

# What `burdin.main`'s locals print in a gdb that auto-loaded the embedded printers. Each entry is the text after
# `$N = `; a printer may append its children (`= {...}`), so the check is a prefix.
GDB_LOCALS = [
    ("text", '"hello"'),
    ("list", "{10, 20}"),
    ("map", '{"a": 1, "b": 2}'),
    ("opt", "Some(5)"),
    ("none", "None"),
    ("res", "Ok(7)"),
    ("shape", "Circle { radius = 1.5 }"),
    ("unit", "Square { side = 2 }"),
    ("item", '{name = "bolt", count = 3}'),
    ("owned", 'Heap("owned")'),
    ("shared", 'Shared("shared") (strong 2)'),
    ("boxed", "any Block = {side = 4}"),
    ("step", "closure { code, env, drop }"),
    ("range", "0..10"),
]

# The same values as lldb's formatters print them (the lldb summary replaces the whole value, children included).
# `item` is a plain struct with no printer, checked on its own below.
LLDB_LOCALS = [
    '(String) text = "hello"',
    "(List<i32>) list = {10, 20}",
    '(Map<String, i32>) map = {"a": 1, "b": 2}',
    "(Option<i32>) opt = Some(5)",
    "(Option<i32>) none = None",
    "(Result<i32, String>) res = Ok(7)",
    "(Shape) shape = Circle { radius = 1.5 }",
    "(Shape) unit = Square { side = 2 }",
    '(Heap<String>) owned = Heap("owned")',
    '(Shared<String>) shared = Shared("shared") (strong 2)',
    "(any Measured) boxed = any Block = {side = 4}",
    "(Fn(i32): i32) step = closure { code, env, drop }",
    "(Range<i32>) range = 0..10",
]

# Selects a frame by function name: both debuggers number the panic frames differently, and the name is stable.
LLDB_SELECT = """\
import lldb


def select(name):
    thread = lldb.debugger.GetSelectedTarget().GetProcess().GetSelectedThread()
    for index in range(thread.GetNumFrames()):
        if thread.GetFrameAtIndex(index).GetFunctionName() == name:
            thread.SetSelectedFrame(index)
            return
    print("no frame named", name)
"""


def gdb_batch(commands, exe, cwd):
    """Runs gdb once, quietly, with the given `-ex` commands.

    The `-iex` settings run before gdb loads the executable: `auto-load safe-path /` is what lets it read the
    `.debug_gdb_scripts` section a `-g` binary carries (DBG6).
    """
    return harness.run(
        [harness.which("gdb"), "-q", "-batch", "-nx"]
        + ["-iex", "set debuginfod enabled off", "-iex", "set auto-load safe-path /"]
        + sum((["-ex", c] for c in commands), [])
        + [exe],
        cwd=cwd,
        merge=True,
    )


def lldb_batch(commands, exe, cwd):
    """Runs lldb once with the given `-o` commands."""
    return harness.run(
        [harness.find_working_llvm_tool("lldb"), "-b"]
        + sum((["-o", c] for c in commands), [])
        + [exe],
        cwd=cwd,
        merge=True,
    )


with harness.temporary_dir("burdin-dbgp-") as work:
    fixture = work / "dbgpanic.bd"
    harness.write_text(fixture, FIXTURE)
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
        "  #0  at dbgpanic.bd:1 in innermost",
        "  #1  at dbgpanic.bd:8 in outer",
        "  #2  at dbgpanic.bd:12 in main",
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
        # DBG6: the printers are in the `-g` binary only.
        checks += 1
        if b"burdin_printers.py" in harness.read_bytes(plain_exe):
            add_problem("a non-`-g` binary carries the embedded gdb printers")
    else:
        add_problem(f"burdin build (no -g) failed: {harness.text(plain.stderr).rstrip(chr(10))}")

    # A macOS binary carries no gdb script: Mach-O has no such section and macOS debugs with lldb (DBG6).
    checks += 1
    carries = b"burdin_printers.py" in harness.read_bytes(exe)
    if sys.platform == "darwin":
        if carries:
            add_problem("a macOS `-g` binary carries the gdb printers (DBG6)")
    elif not carries:
        add_problem("the `-g` binary does not carry the embedded gdb printers (DBG6)")

    # DBG1: the panic hook. gdb stops there with the stack intact, before the panic prints anything.
    has_gdb = harness.which("gdb") is not None
    if windows_host:
        skip("the gdb breakpoint and printer checks need DWARF, which the Windows linker does not keep")
    elif not has_gdb:
        skip("gdb is not installed; the panic breakpoint and the printers were not checked")
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

    # DBG5/DBG6: the printers, auto-loaded from the binary's `.debug_gdb_scripts` section — no `command script
    # import` anywhere below.
    printer_fixture = work / "dbgprinters.bd"
    harness.write_text(printer_fixture, PRINTER_FIXTURE)
    printer_build = harness.run([burdin, "build", "dbgprinters.bd", "-g", "-o", "dbgprinters"], cwd=work)
    checks += 1
    if printer_build.returncode != 0:
        add_problem(
            f"burdin build -g (printers) failed: {harness.text(printer_build.stderr).rstrip(chr(10))}"
        )
    else:
        printer_exe = harness.executable_in(work, "dbgprinters")
        if windows_host or not has_gdb:
            pass
        else:
            commands = [
                "break burdin_panic_begin",
                "run",
                "bt",
                "frame function burdin.show",
                "print word",
                "frame function burdin.main",
            ] + [f"print {name}" for name, _ in GDB_LOCALS]
            debugged = gdb_batch(commands, printer_exe, cwd=work)
            transcript = harness.text(debugged.stdout)
            checks += 1
            if '$1 = "hello"' not in transcript:
                add_problem(f"gdb: the `str` printer did not print the parameter:\n{transcript}")
            for index, (name, expected) in enumerate(GDB_LOCALS, start=2):
                checks += 1
                prefix = f"${index} = {expected}"
                if not any(line.startswith(prefix) for line in transcript.splitlines()):
                    add_problem(f"gdb: `{name}` did not print as `{prefix}`:\n{transcript}")

        # DBG5/DBG7: the lldb formatters, loaded by hand as `burdin debug` does.
        lldb = None if windows_host else harness.find_working_llvm_tool("lldb")
        if lldb is None:
            skip("lldb is not installed; the lldb formatters were not checked")
        else:
            formatters = os.path.join(harness.ROOT, "debuggers", "lldb", "burdin_formatters.py")
            selector = work / "lldb_select.py"
            harness.write_text(selector, LLDB_SELECT)
            names = " ".join(name for name, _ in GDB_LOCALS)
            debugged = lldb_batch(
                [
                    f"command script import {formatters}",
                    f"command script import {selector}",
                    "breakpoint set -n burdin_panic_begin",
                    "run",
                    'script lldb_select.select("burdin.show")',
                    "frame variable word",
                    'script lldb_select.select("burdin.main")',
                    f"frame variable {names}",
                ],
                printer_exe,
                cwd=work,
            )
            transcript = harness.text(debugged.stdout)
            checks += 1
            if '(str) word = "hello"' not in transcript:
                add_problem(f"lldb: the `str` formatter did not print the parameter:\n{transcript}")
            checks += 1
            if "(Item) item = {" not in transcript:
                add_problem(f"lldb: a plain struct printed no members:\n{transcript}")
            for expected in LLDB_LOCALS:
                checks += 1
                if expected not in transcript:
                    add_problem(f"lldb: `{expected}` is missing:\n{transcript}")

    # DBG8: the Windows visualizers. The driver embeds the file with `include_str!` for `/NATVIS:`; it has to be
    # well-formed XML and cover the same types as the two printer files (`spec/debug.md`, DBG5).
    natvis = harness.ROOT / "debuggers" / "windows" / "burdin.natvis"
    checks += 1
    document = None
    try:
        document = ElementTree.parse(natvis).getroot()
    except (OSError, ElementTree.ParseError) as error:
        add_problem(f"debuggers/windows/burdin.natvis does not parse: {error}")
    if document is not None:
        entries = [element.get("Name") for element in document.iter() if element.tag.endswith("Type")]
        for expected in (
            "String",
            "str",
            "List<*>",
            "Map<*>",
            "Option<*>",
            "Result<*>",
            "Heap<*>",
            "Shared<*>",
            "any *",
            "Range<*>",
        ):
            checks += 1
            if expected not in entries:
                add_problem(f"the natvis file has no entry for `{expected}`: {entries}")
    checks += 1
    if b"<AutoVisualizer" not in harness.read_bytes(burdin):
        add_problem("the driver does not carry debuggers/windows/burdin.natvis (DBG8)")

    # DBG9/DBG10: `burdin debug`. A stub debugger stands in for a real one: it records its command line and exits
    # with a status the command has to pass through, which is also how the argument layout is checked.
    if windows_host:
        skip("`burdin debug`'s command line was not checked: the stub-debugger test is a shell script")
    else:
        stub = work / "stub-debugger"
        harness.write_text(
            stub,
            "#!/bin/sh\n"
            'here="$(dirname "$0")"\n'
            'printf "%s\\n" "$@" > "$here/stub-args.txt"\n'
            "for arg in \"$@\"; do\n"
            '  case "$arg" in\n'
            '    "command script import "*) cp "${arg#command script import }" "$here/imported.py" ;;\n'
            "  esac\n"
            "done\n"
            "exit 7\n",
        )
        os.chmod(stub, 0o755)
        args_file = work / "stub-args.txt"
        debugged = harness.run(
            [burdin, "debug", "dbgprinters.bd", "--debugger", str(stub), "--break-on-panic", "--", "hello"],
            cwd=work,
        )
        checks += 1
        if debugged.returncode != 7:
            add_problem(
                f"`burdin debug` did not pass the debugger's exit status through: {debugged.returncode}\n"
                f"{harness.text(debugged.stderr)}"
            )
        recorded = harness.read_text(args_file).splitlines() if args_file.is_file() else []
        checks += 1
        if "break burdin_panic_begin" not in recorded:
            add_problem(f"`burdin debug --break-on-panic` set no breakpoint: {recorded}")
        checks += 1
        if "--args" not in recorded or not recorded or recorded[-1] != "hello":
            add_problem(f"`burdin debug -- hello` did not pass the program's arguments: {recorded}")

        # DBG10: the test index, resolved like the runner's filters.
        tests = work / "dbgtests.bd"
        harness.write_text(tests, 'test "adds" {\n    assert(1 + 1 == 2)\n}\n\ntest "subtracts" {\n    assert(2 - 1 == 1)\n}\n')
        for query, expected in (("1", "1"), ("subtracts", "1"), ("adds", "0")):
            debugged = harness.run([burdin, "debug", "dbgtests.bd", "--debugger", str(stub), "--test", query], cwd=work)
            recorded = harness.read_text(args_file).splitlines() if args_file.is_file() else []
            checks += 1
            if debugged.returncode != 7 or not recorded or recorded[-1] != expected:
                add_problem(f"`burdin debug --test {query}` did not select test {expected}: {recorded}")
        checks += 1
        unknown = harness.run([burdin, "debug", "dbgtests.bd", "--debugger", str(stub), "--test", "nope"], cwd=work)
        if unknown.returncode != 2 or "no test path contains" not in harness.text(unknown.stderr):
            add_problem(
                f"`burdin debug --test nope` did not report an unknown test: {harness.text(unknown.stderr)}"
            )

        # DBG7: an lldb session imports the formatters from the file the command writes beside it.
        lldb_stub = work / "lldb-stub"
        os.replace(stub, lldb_stub)
        debugged = harness.run([burdin, "debug", "dbgprinters.bd", "--debugger", str(lldb_stub)], cwd=work)
        recorded = harness.read_text(args_file).splitlines() if args_file.is_file() else []
        checks += 1
        import_line = next((line for line in recorded if line.startswith("command script import ")), None)
        imported = work / "imported.py"
        if import_line is None:
            add_problem(f"`burdin debug --debugger lldb-…` imported no formatters: {recorded}")
        else:
            checks += 1
            if not imported.is_file() or "__lldb_init_module" not in harness.read_text(imported):
                add_problem(f"the lldb formatters were not written for the session: {import_line}")

checked = f"Checked the debugger integration ({checks} checks)"
if skips:
    for reason in skips:
        print(f"skip: {reason}")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
print(checked)
