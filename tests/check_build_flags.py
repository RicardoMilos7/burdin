#!/usr/bin/env python3
"""Checks the build flags and binary metadata:
  * `burdin --version` prints the version, channel, target, LLVM version, commit, and profile (issues #431, #454);
  * `--strip` removes the symbol table and keeps the program's output, and the stripped sizes stay under the
    size budget recorded here (issue #430);
  * `--lto` links bitcode and runs, and `--pgo-generate`/`--pgo-use` round-trip a profile (issue #429), when an
    LLVM compiler driver and the LLVM tools are installed. On MSVC those two options are rejected.

Needs a built driver. BURDIN selects it, like tests/run_tests.py.
"""

import os
import re
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()


def size_of(path):
    try:
        return harness.file_size(path)
    except OSError:
        return None


with harness.temporary_dir("burdin-flags-") as work:
    # --- --version (issue #431) --------------------------------------------------
    checks += 1
    version = harness.run([burdin, "--version"])
    if version.returncode != 0:
        add_problem(
            f"burdin --version: exit status {version.returncode}: "
            f"{harness.text(version.stderr).rstrip(chr(10))}"
        )
    else:
        lines = harness.text(version.stdout).rstrip("\n").split("\n")
        for prefix in ("burdin ", "channel: ", "target: ", "llvm: ", "commit: ", "profile: "):
            checks += 1
            if not any(line.startswith(prefix) for line in lines):
                add_problem(f"burdin --version: no '{prefix}' line")
        checks += 1
        if not any(re.match(r"^channel: (dev|nightly|stable)$", line) for line in lines):
            add_problem("burdin --version: the channel is not dev, nightly, or stable")
        checks += 1
        if not any(re.match(r"^llvm: [0-9]+\.[0-9]+\.[0-9]+$", line) for line in lines):
            add_problem("burdin --version: the LLVM version is not major.minor.patch")
        checks += 1
        if not any(re.match(r"^target: [A-Za-z0-9_]+-[A-Za-z0-9_.-]+$", line) for line in lines):
            add_problem("burdin --version: the target is not a triple")

    # --- Hello world, for --strip and the budgets (issue #430) -------------------
    harness.write_text(
        work / "hello.bd", 'fn main() {\n    println("hello, world")\n}\n'
    )
    harness.write_text(
        work / "stdheavy.bd",
        "struct Item {\n"
        "    name: String,\n"
        "    count: i32,\n"
        "}\n"
        "fn main() {\n"
        "    let mut items = List<Item>.new()\n"
        "    for i in 0..20 {\n"
        '        items.push(Item { name: String.from("item").clone(), count: i })\n'
        "    }\n"
        "    let mut map = Map<String, i32>.new()\n"
        "    for item in items {\n"
        "        map.insert(item.name.clone(), item.count)\n"
        "    }\n"
        "    println(items.len().to_string())\n"
        "    println(map.len().to_string())\n"
        "}\n",
    )

    def build_and_run(description: str, source: str, output: str, *flags: str):
        global checks
        checks += 1
        built = harness.run([burdin, "build", source, "-o", output, *flags], cwd=work)
        if built.returncode != 0:
            add_problem(
                f"{description}: exit status {built.returncode}: "
                f"{harness.text(built.stderr).rstrip(chr(10))}"
            )
            return None
        checks += 1
        ran = harness.run([harness.executable_in(work, output)], cwd=work)
        if ran.returncode != 0:
            add_problem(
                f"{description}: the executable failed: {harness.text(ran.stderr).rstrip(chr(10))}"
            )
            return None
        return harness.text(ran.stdout).rstrip("\n")

    plain_out = build_and_run("burdin build hello", "hello.bd", "hello_plain")
    if plain_out is None:
        plain_out = ""

    checks += 1
    stripped = harness.run([burdin, "build", "hello.bd", "-o", "hello_strip", "--strip"], cwd=work)
    if stripped.returncode != 0:
        add_problem(f"burdin build --strip: {harness.text(stripped.stderr).rstrip(chr(10))}")
    else:
        checks += 1
        ran = harness.run([harness.executable_in(work, "hello_strip")], cwd=work)
        strip_out = harness.text(ran.stdout).rstrip("\n")
        if strip_out != plain_out:
            add_problem(f"--strip changed the output: '{strip_out}'")
        plain_size = size_of(harness.executable_in(work, "hello_plain"))
        strip_size = size_of(harness.executable_in(work, "hello_strip"))
        print(
            f"hello: {plain_size if plain_size is not None else '?'} bytes, "
            f"stripped: {strip_size if strip_size is not None else '?'} bytes"
        )
        if harness.PLATFORM in ("linux", "macos"):
            checks += 1
            if strip_size is None or strip_size > 131072:
                add_problem(
                    f"stripped hello is {strip_size if strip_size is not None else '?'} bytes, "
                    "over the 128 KiB budget"
                )
            checks += 1
            nm = harness.which("nm")
            leftover = ""
            if nm is not None:
                symbols = harness.run([nm, str(harness.executable_in(work, "hello_strip"))])
                matches = [
                    line
                    for line in harness.text(symbols.stdout).split("\n")
                    if "burdin." in line
                ][:3]
                leftover = "".join(line + " " for line in matches)
            if leftover:
                add_problem(f"--strip left Burdin symbols in the executable: {leftover}")

    std_output = build_and_run("burdin build std-heavy", "stdheavy.bd", "std_plain", "--strip")
    if std_output is not None:
        std_size = size_of(harness.executable_in(work, "std_plain"))
        print(f"std-heavy (stripped): {std_size if std_size is not None else '?'} bytes")
        if harness.PLATFORM in ("linux", "macos"):
            checks += 1
            if std_size is None or std_size > 524288:
                add_problem(
                    f"stripped std-heavy is {std_size if std_size is not None else '?'} bytes, "
                    "over the 512 KiB budget"
                )

    # --- LTO and PGO (issue #429) ------------------------------------------------
    clang_tool = harness.find_llvm_tool("clang")
    opt_tool = harness.find_llvm_tool("opt")
    llc_tool = harness.find_llvm_tool("llc")
    profdata_tool = harness.find_llvm_tool("llvm-profdata")

    if harness.PLATFORM in ("linux", "macos"):
        if clang_tool:
            checks += 1
            lto = harness.run(
                [burdin, "build", "hello.bd", "--lto", "-o", "hello_lto"],
                cwd=work,
                env={**os.environ, "BURDIN_CC": clang_tool},
            )
            if lto.returncode == 0:
                checks += 1
                ran = harness.run([harness.executable_in(work, "hello_lto")], cwd=work)
                output = harness.text(ran.stdout).rstrip("\n")
                if output != "hello, world":
                    add_problem(f"--lto changed the output: '{output}'")
            else:
                detail = "\n".join(harness.text(lto.stderr).split("\n")[:5])
                add_problem(f"burdin build --lto: {detail}")
            if opt_tool and llc_tool and profdata_tool:
                checks += 1
                pgo = harness.run(
                    [burdin, "build", "hello.bd", "--pgo-generate", "-o", "hello_pgo"],
                    cwd=work,
                    env={**os.environ, "BURDIN_CC": clang_tool},
                )
                if pgo.returncode == 0:
                    profile = work / "default.profraw"
                    profile.unlink(missing_ok=True)
                    checks += 1
                    ran = harness.run(
                        [harness.executable_in(work, "hello_pgo")],
                        cwd=work,
                        env={**os.environ, "LLVM_PROFILE_FILE": "default.profraw"},
                    )
                    if ran.returncode != 0:
                        detail = "\n".join(harness.text(ran.stderr).split("\n")[:5])
                        add_problem(f"the instrumented build failed: {detail}")
                    elif not profile.is_file() or profile.stat().st_size == 0:
                        add_problem("--pgo-generate wrote no profile")
                    checks += 1
                    used = harness.run(
                        [burdin, "build", "hello.bd", "--pgo-use", "default.profraw", "-o", "hello_pgo_used"],
                        cwd=work,
                    )
                    if used.returncode == 0:
                        checks += 1
                        ran = harness.run([harness.executable_in(work, "hello_pgo_used")], cwd=work)
                        output = harness.text(ran.stdout).rstrip("\n")
                        if output != "hello, world":
                            add_problem(f"--pgo-use changed the output: '{output}'")
                    else:
                        detail = "\n".join(harness.text(used.stderr).split("\n")[:5])
                        add_problem(f"burdin build --pgo-use: {detail}")
                else:
                    detail = "\n".join(harness.text(pgo.stderr).split("\n")[:5])
                    add_problem(f"burdin build --pgo-generate: {detail}")
            else:
                print("note: opt, llc, or llvm-profdata is missing; skipping the PGO round-trip")
        else:
            print("note: clang is missing; skipping the LTO and PGO checks")

    # --- Rejected combinations and missing profiles ------------------------------
    checks += 1
    rejected = harness.run([burdin, "build", "hello.bd", "--lto", "--pgo-generate", "-o", "nope"], cwd=work)
    if rejected.returncode == 0 or b"cannot be combined" not in rejected.stderr:
        add_problem("--lto with --pgo-generate was not rejected")
    checks += 1
    missing = harness.run([burdin, "build", "hello.bd", "--pgo-use", "missing.profraw", "-o", "nope"], cwd=work)
    if missing.returncode == 0 or b"does not exist" not in missing.stderr:
        add_problem("a missing PGO profile was not reported")

print(f"Checked build flags and metadata ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
