#!/usr/bin/env python3
"""Checks the build tool's command surface (`spec/build-tool.md`, issue #338).

Needs a built driver. BURDIN selects it, like tests/run_tests.py.

What it checks:
  * the root `--help` is the golden command list: every command in scope, its summary, and the order;
  * `burdin help <command>` and `burdin <command> --help` work for every command in scope, and a representative
    command page is golden;
  * an unknown command exits 2, names the word, and suggests a near miss;
  * every Cargo command that is planned but not implemented is rejected as an unknown command;
  * `-V`/`--version` prints the documented metadata and takes no arguments;
  * one fixture package goes through `build`, `run`, `check`, and `test`.
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

# BT1: every command in scope, in the order the root help lists them.
COMMANDS = [
    "build",
    "run",
    "test",
    "bench",
    "fuzz",
    "check",
    "fmt",
    "lint",
    "fix",
    "ssr",
    "refactor",
    "doc",
    "new",
    "add",
    "repl",
    "watch",
    "debug",
    "expand",
    "asm",
    "bloat",
    "explain",
    "bindgen",
    "lsp",
    "dap",
    "help",
]

# BT2: Cargo commands that are planned but not implemented, and must be unknown today.
OUT_OF_SCOPE = [
    "init",
    "remove",
    "update",
    "tree",
    "metadata",
    "publish",
    "yank",
    "login",
    "search",
    "install",
    "clean",
    "package",
    "vendor",
    "owner",
    "logout",
    "uninstall",
    "info",
]

ROOT_HELP = """The Burdin compiler and toolchain

Usage: burdin [OPTIONS] [COMMAND]

Commands:
  build     Compile a file or package into an executable
  run       Build a program and run it
  test      Build and run the `test` blocks of a file or package
  bench     Build and measure the `bench` blocks of a file or package
  fuzz      Build a `fuzz` target with libFuzzer and AddressSanitizer, and fuzz it
  check     Type-check a program and run the memory checks, without generating code
  fmt       Format source files
  lint      Report common mistakes and style issues, and fix them
  fix       Apply the machine-applicable fixes of the compiler and the linter across a package
  ssr       Structural search and replace: match a pattern written as Burdin code across a package
  refactor  Refactor code: extract, inline, change a signature, or move an item
  doc       Generate the documentation of a package
  new       Create a new package
  add       Add a path, git, or official dependency to burdin.toml
  repl      Start an interactive session
  watch     Rerun check, build, test, or run whenever the sources change
  debug     Build a program with debug info and start a debugger on it
  expand    Print the source after comptime, derives, default arguments, and operators
  asm       Print the assembly or LLVM IR of one function, with its source
  bloat     Report the size of each function, module, package, and generic in the executable
  explain   Print the reference entry for an error code
  bindgen   Generate bindings: a module from a C header or Rust crate, or a C header for a package
  lsp       Run the language server on standard input and output
  dap       Run the debug adapter on standard input and output
  help      Print the help of a command

Options:
  -V, --version  Print version info and exit
  -h, --help     Print help

See `burdin help <command>` for more information on a specific command.
"""

NEW_HELP = """Create a new package

Usage: burdin new <name>

Arguments:
  <name>  The package directory to create

Options:
  -h, --help  Print help

Creates <name>/burdin.toml, <name>/src/main.bd, and <name>/.gitignore.
"""


def run(*arguments, cwd=None):
    return harness.run([burdin, *arguments], cwd=cwd)


def stdout(result) -> str:
    return harness.text(result.stdout).replace("\r", "")


def stderr(result) -> str:
    return harness.text(result.stderr).replace("\r", "")


# BT4 and BT1: the root page is the golden command list, and every command has a page.
root = run("--help")
check(root.returncode == 0, f"`burdin --help` failed: {stderr(root)[:400]}")
check(stdout(root) == ROOT_HELP, f"`burdin --help` is not the golden command list:\n{stdout(root)}")

command_section = stdout(root).split("Commands:\n", 1)[1].split("\n\nOptions:", 1)[0]
listed = [line.strip().split()[0] for line in command_section.splitlines() if line.strip()]
check(listed == COMMANDS, f"the root help lists {listed}, expected {COMMANDS}")

for command in COMMANDS:
    # `help` has no page of its own: `burdin help help` and `burdin help` both print the command list.
    if command == "help":
        check(stdout(run("help", "help")) == stdout(root), "`burdin help help` is not the command list")
        continue
    page = run("help", command)
    check(page.returncode == 0, f"`burdin help {command}` failed: {stderr(page)[:300]}")
    check(
        f"usage: burdin {command}" in stdout(page).lower(),
        f"`burdin help {command}` does not name the command's usage:\n{stdout(page)[:300]}",
    )
    page = run(command, "--help")
    check(page.returncode == 0, f"`burdin {command} --help` failed: {stderr(page)[:300]}")
    check(
        f"usage: burdin {command}" in stdout(page).lower(),
        f"`burdin {command} --help` does not name the command's usage:\n{stdout(page)[:300]}",
    )

# A representative page is golden, so a change to the page format is visible.
new_help = run("help", "new")
check(stdout(new_help) == NEW_HELP, f"`burdin help new` is not the golden page:\n{stdout(new_help)}")

# BT4: help after `--` belongs to the program, not to `burdin`.
program_help = run("run", "--", "--help")
check(program_help.returncode != 0, "`burdin run -- --help` ran without a target")

# BT4: an unknown command is a usage error with a suggestion for a near miss.
unknown = run("burld")
check(unknown.returncode == 2, f"`burdin burld` exited {unknown.returncode}, expected 2")
check("burld" in stderr(unknown), f"`burdin burld` does not name the word:\n{stderr(unknown)[:400]}")
check("build" in stderr(unknown), f"`burdin burld` gives no suggestion:\n{stderr(unknown)[:400]}")

# BT2: every planned-but-missing Cargo command is unknown today.
for command in OUT_OF_SCOPE:
    result = run(command)
    check(result.returncode == 2, f"`burdin {command}` exited {result.returncode}, expected 2")
    check(
        command in stderr(result) or command in stdout(result),
        f"`burdin {command}` does not name the command:\n{stderr(result)[:300]}",
    )

# BT4: the version.
version = run("--version")
check(version.returncode == 0, f"`burdin --version` failed: {stderr(version)[:300]}")
lines = stdout(version).splitlines()
for prefix in ["burdin ", "channel: ", "target: ", "llvm: ", "commit: ", "profile: "]:
    check(any(line.startswith(prefix) for line in lines), f"`burdin --version` is missing `{prefix}`:\n{stdout(version)}")
version_argument = run("--version", "build")
check(version_argument.returncode == 2, f"`burdin --version build` exited {version_argument.returncode}, expected 2")

# BT7: one fixture package through build, run, check, and test.
MANIFEST = '[package]\nname = "app"\nversion = "0.1.0"\n'
MAIN = """fn double(x: i32): i32 {
    x * 2
}

fn main() {
    println("{}", double(21))
}

test "doubles" {
    assert_eq(double(2), 4)?
}
"""

with harness.temporary_dir("burdin-cli-") as work:
    app = work / "app"
    (app / "src").mkdir(parents=True)
    harness.write_text(app / "burdin.toml", MANIFEST)
    harness.write_text(app / "src" / "main.bd", MAIN)

    exe = work / ("appbin.exe" if harness.PLATFORM == "windows" else "appbin")
    built = run("build", "app", "-o", str(exe), cwd=work)
    check(built.returncode == 0, f"`burdin build app` failed: {stderr(built)[:600]}")
    check(exe.is_file(), f"`burdin build app` wrote no {exe.name}")

    ran = run("run", "app", cwd=work)
    check(ran.returncode == 0, f"`burdin run app` failed: {stderr(ran)[:400]}")
    check(stdout(ran).strip() == "42", f"`burdin run app` printed {stdout(ran)!r}")

    checked = run("check", "app", cwd=work)
    check(checked.returncode == 0, f"`burdin check app` failed: {stderr(checked)[:400]}")
    check(stdout(checked) == "", f"`burdin check app` printed {stdout(checked)!r}")

    tested = run("test", "app", cwd=work)
    check(tested.returncode == 0, f"`burdin test app` failed: {stderr(tested)[:400]}")
    check("1 passed" in stdout(tested), f"`burdin test app` printed {stdout(tested)!r}")

    # BT4: an unknown option and a missing target are usage errors.
    bogus = run("build", "app", "--bogus", cwd=work)
    check(bogus.returncode == 2, f"`burdin build app --bogus` exited {bogus.returncode}, expected 2")
    check("unknown option" in stderr(bogus), f"`--bogus` is not reported:\n{stderr(bogus)[:300]}")
    missing = run("build", cwd=work)
    check(missing.returncode == 2, f"`burdin build` exited {missing.returncode}, expected 2")

print(f"check_cli: {checks} checks")
sys.exit(harness.report_problems(problems))
