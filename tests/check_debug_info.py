#!/usr/bin/env python3
"""Checks source-level debug information (issue #422). Needs a built driver; uses llvm-dwarfdump to read the
DWARF (from the executable on Linux, from the .dSYM bundle on macOS) and gdb, when it is installed, to prove a
breakpoint on a Burdin line and a local read work.

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * `burdin build -g` produces an executable whose output is unchanged;
  * the DWARF has the source file and line table, a DW_TAG_subprogram per Burdin function, a
    DW_TAG_formal_parameter for each parameter, and a DW_TAG_variable for each local, with declaration lines;
  * with gdb installed, `break <file>:<line>` stops in the expected function and `print` reads the locals;
  * the release profile with -g still carries the line table.
"""

import re
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()
windows_host = harness.PLATFORM == "windows"

with harness.temporary_dir("burdin-dbg-") as work:
    # Lines 5-8 are `add`; line 7 is the tail expression, where the breakpoint goes.
    harness.write_text(
        work / "dbg.bd",
        "struct Point {\n"
        "    x: i32,\n"
        "    y: i32,\n"
        "}\n"
        "fn add(a: i32, b: i32): i32 {\n"
        "    let sum = a + b\n"
        "    sum\n"
        "}\n"
        "fn main() {\n"
        "    let p = Point { x: 1, y: 2 }\n"
        "    let total = add(p.x, p.y)\n"
        "    println(total.to_string())\n"
        "}\n",
    )

    built = harness.run([burdin, "build", "dbg.bd", "-g", "-o", "dbg"], cwd=work)
    checks += 1
    if built.returncode != 0:
        add_problem(
            f"burdin build -g: exit status {built.returncode}: "
            f"{harness.text(built.stderr).rstrip(chr(10))}"
        )
    exe = harness.executable_in(work, "dbg")
    checks += 1
    if not exe.is_file():
        add_problem("burdin build -g: no executable")

    checks += 1
    ran = harness.run([exe], cwd=work)
    output = harness.text(ran.stdout).rstrip("\n")
    if output != "3":
        add_problem(f"-g changed the program's output: '{output}', expected '3'")

    dwarfdump = harness.find_llvm_tool("llvm-dwarfdump")
    if not dwarfdump:
        print("warning: llvm-dwarfdump not found; skipping the debug-info sections", file=sys.stderr)
    else:
        # ld64 keeps DWARF in a .dSYM bundle beside the executable; the driver produces it for `-g` on macOS.
        dwarf_target = exe
        bundle = exe.parent / (exe.name + ".dSYM")
        if bundle.is_dir():
            dwarf_target = bundle / "Contents" / "Resources" / "DWARF" / exe.name
        info = harness.text(harness.run([dwarfdump, "--debug-info", dwarf_target]).stdout)
        line_table = harness.text(harness.run([dwarfdump, "--debug-line", dwarf_target]).stdout)

        if windows_host and "DW_TAG_compile_unit" not in info:
            # MSVC's link.exe does not carry the DWARF sections into the image, and the driver does not produce a
            # PDB; the Linux and macOS jobs are where the DWARF sections are verified.
            print(
                "note: the Windows linker kept no DWARF sections; skipping the section checks",
                file=sys.stderr,
            )
        else:
            if "DW_TAG_compile_unit" not in info:
                # Show what the executable and the IR actually carry, so a platform that drops the sections is
                # diagnosable from the CI log without a second run.
                print("debug: the executable carries no compile unit; diagnosing", file=sys.stderr)
                file_tool = harness.which("file")
                if file_tool:
                    reported = harness.run([file_tool, exe], merge=True)
                    sys.stderr.write(harness.text(reported.stdout))
                otool = harness.which("otool")
                if otool:
                    listing = harness.text(harness.run([otool, "-l", exe]).stdout)
                    sections = [
                        line
                        for line in listing.split("\n")
                        if "segname" in line or "sectname" in line
                    ][:30]
                    sys.stderr.write("\n".join(sections) + ("\n" if sections else ""))
                raw = harness.run([dwarfdump, "--debug-info", dwarf_target], merge=True)
                sys.stderr.write("\n".join(harness.text(raw.stdout).split("\n")[:10]) + "\n")
                diagnostic = harness.run(
                    [burdin, "build", "dbg.bd", "-g", "--emit-ir", "diag.ll", "-o", "diag_exe"],
                    cwd=work,
                )
                if diagnostic.returncode == 0:
                    ir = harness.read_text(work / "diag.ll")
                    print(
                        f"IR: {ir.count('!DICompileUnit')} DICompileUnit, "
                        f"{ir.count('!DISubprogram')} DISubprogram",
                        file=sys.stderr,
                    )
                    llc = harness.find_llvm_tool("llc")
                    if llc:
                        object_file = work / "diag.o"
                        if harness.run([llc, "-filetype=obj", "diag.ll", "-o", "diag.o"], cwd=work).returncode == 0:
                            dumped = harness.text(
                                harness.run([dwarfdump, "--debug-info", object_file]).stdout
                            )
                            interesting = [
                                line
                                for line in dumped.split("\n")
                                if "DW_TAG_compile_unit" in line
                                or "DW_TAG_subprogram" in line
                                or "error" in line
                            ][:6]
                            sys.stderr.write("\n".join(interesting) + ("\n" if interesting else ""))
                else:
                    detail = "\n".join(harness.text(diagnostic.stderr).split("\n")[:5])
                    print(f"IR build failed: {detail}", file=sys.stderr)

            def expect_info(description: str, pattern: str) -> None:
                global checks
                checks += 1
                if pattern not in info:
                    add_problem(f"DWARF: {description} (missing '{pattern}')")

            expect_info("no compile unit", "DW_TAG_compile_unit")
            expect_info("no source file", "dbg.bd")
            expect_info("the add subprogram", '"add"')
            expect_info("the main subprogram", '"main"')
            expect_info("the sum local", '"sum"')
            expect_info("the total local", '"total"')
            expect_info("a formal parameter", "DW_TAG_formal_parameter")
            expect_info("a local variable", "DW_TAG_variable")
            expect_info("a declaration line", "DW_AT_decl_line")
            checks += 1
            if "dbg.bd" not in line_table:
                add_problem("DWARF: the line table never names dbg.bd")

        # An optimized build keeps the line table too.
        release = harness.run([burdin, "build", "dbg.bd", "--release", "-g", "-o", "dbg_release"], cwd=work)
        if release.returncode == 0:
            release_exe = harness.executable_in(work, "dbg_release")
            release_target = release_exe
            bundle = release_exe.parent / (release_exe.name + ".dSYM")
            if bundle.is_dir():
                release_target = bundle / "Contents" / "Resources" / "DWARF" / release_exe.name
            release_lines = harness.text(
                harness.run([dwarfdump, "--debug-line", release_target]).stdout
            )
            if not windows_host:
                checks += 1
                if "dbg.bd" not in release_lines:
                    add_problem("DWARF: --release -g has no line table for dbg.bd")
            checks += 1
            ran = harness.run([release_exe], cwd=work)
            output = harness.text(ran.stdout).rstrip("\n")
            if output != "3":
                add_problem(f"--release -g changed the output: '{output}', expected '3'")
        else:
            add_problem(
                f"burdin build --release -g: exit status {release.returncode}: "
                f"{harness.text(release.stderr).rstrip(chr(10))}"
            )

    gdb = harness.which("gdb")
    if windows_host and gdb:
        # MSVC's link.exe kept no DWARF, so gdb has nothing to read.
        print(
            "note: the Windows linker kept no DWARF sections; skipping the gdb checks", file=sys.stderr
        )
    elif gdb:
        debugged = harness.run(
            [
                gdb,
                "-q",
                "-batch",
                "-nx",
                "-ex",
                "set debuginfod enabled off",
                "-ex",
                "break dbg.bd:7",
                "-ex",
                "run",
                "-ex",
                "print sum",
                "-ex",
                "print a",
                "-ex",
                "print b",
                "-ex",
                "continue",
                exe,
            ],
            cwd=work,
            merge=True,
        )
        transcript = harness.text(debugged.stdout)
        checks += 1
        if not re.search(r"^\$1 = 3$", transcript, re.MULTILINE):
            detail = "\n".join(transcript.split("\n")[-20:])
            add_problem(f"gdb: 'print sum' at dbg.bd:7 did not print 3: {detail}")
        checks += 1
        if not re.search(r"^\$2 = 1$", transcript, re.MULTILINE):
            add_problem("gdb: 'print a' did not print 1")
        checks += 1
        if not re.search(r"^\$3 = 2$", transcript, re.MULTILINE):
            add_problem("gdb: 'print b' did not print 2")
        checks += 1
        if not re.search(r"Breakpoint 1, .*add", transcript):
            add_problem("gdb: the breakpoint did not stop in add")

print(f"Checked debug information ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
