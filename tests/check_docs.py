#!/usr/bin/env python3
"""Checks the documentation: relative links resolve, and the checked example and the documented driver
contract still hold. Needs a built driver. Usage: tests/check_docs.py

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * every relative link in the repository's Markdown files points at a file, and every `#anchor`
    matches a heading (the website's Markdown is left out: its links are site addresses, checked on
    the built site by website/scripts/check-links.mjs);
  * the `burdin` program marked `check-docs` in README.md compiles and prints the documented output;
  * `build` output naming, `-o`, `--emit-ir`, `--release`, `-g`, `--strip`, and `--link-lib` (including
    LLVM-C when llvm-config is available), and `--version`'s six metadata lines;
  * `run` passes arguments after `--` and the program's exit status through;
  * `test` prints the documented `FAIL` lines and summary and exits 1 on a failure;
  * diagnostics have the documented `path:LINE:COL: error[CODE]: message` shape;
  * BURDIN_CC names the linker and its absence is reported.
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


def heading_slugs(path) -> set:
    """GitHub's heading anchor: lowercase, drop punctuation except spaces, hyphens, and underscores,
    then turn spaces into hyphens."""
    slugs = set()
    for line in harness.read_text(path).split("\n"):
        if not re.match(r"^#+[ \t]", line):
            continue
        text = re.sub(r"^#+[ \t]+", "", line)
        text = re.sub(r"[ \t]+$", "", text)
        text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
        text = text.lower()
        text = re.sub(r"[^a-z0-9 _-]", "", text)
        slugs.add(text.replace(" ", "-"))
    return slugs


def check_links() -> None:
    global checks
    files = [
        path
        for path in sorted(harness.ROOT.rglob("*.md"))
        # `website/` is the published site: its links are addresses in the built site (`/guide/ownership/`), which
        # `website/scripts/check-links.mjs` checks against the build, not files in the tree.
        if ".git" not in path.parts and "target" not in path.parts and "website" not in path.parts
    ]
    for path in files:
        directory = path.parent
        fenced = False
        for line in harness.read_text(path).split("\n"):
            if re.match(r"^[ \t]*(```|~~~)", line):
                fenced = not fenced
                continue
            if fenced:
                continue
            for target in re.findall(r"\]\(([^)]*)\)", line):
                if not target:
                    continue
                if target.startswith(("http://", "https://", "mailto:", "tel:")):
                    continue
                if "#" in target:
                    link, fragment = target.split("#", 1)
                else:
                    link, fragment = target, ""
                link = link.replace("%20", " ")
                if not link:
                    resolved = path
                elif link.startswith("/"):
                    resolved = Path(link)
                else:
                    resolved = directory / link
                checks += 1
                relative = path.relative_to(harness.ROOT).as_posix()
                if not resolved.exists():
                    add_problem(f"{relative}: broken link: {target}")
                    continue
                if fragment and resolved.is_file():
                    if fragment not in heading_slugs(resolved):
                        add_problem(f"{relative}: no heading matches the anchor: {target}")
                elif fragment:
                    add_problem(f"{relative}: anchor on a directory: {target}")


check_links()

burdin = harness.require_burdin()


def check_compiler_known_types() -> None:
    """Every `Ty` variant has a row in the compiler-known types table of spec/stdlib.md (issue #1017)."""
    source = (harness.ROOT / "crates" / "burdin_tir" / "src" / "ty.rs").read_text(encoding="utf-8")
    body = re.search(r"^pub enum Ty \{(.*?)^\}", source, re.M | re.S).group(1)
    variants = re.findall(r"^    ([A-Z][A-Za-z0-9]*)\b", body, re.M)
    spec = (harness.ROOT / "spec" / "stdlib.md").read_text(encoding="utf-8")
    section = re.search(r"^## Compiler-known types.*?(?=^## )", spec, re.M | re.S).group(0)
    for name in variants:
        if f"`Ty::{name}`" not in section:
            add_problem(f'spec/stdlib.md: `Ty::{name}` has no row in "Compiler-known types"')


check_compiler_known_types()


def run_driver(*args, env=None):
    return harness.run([burdin, *args], cwd=work, env=env)


def expect_status(description: str, expected: int) -> None:
    global checks
    checks += 1
    if status != expected:
        add_problem(f"{description}: exit status {status}, expected {expected}")


def expect_contains(description: str, needle: str, output: str) -> None:
    global checks
    checks += 1
    if needle not in output:
        add_problem(f"{description}: output does not contain: {needle}")


def expect_file(description: str, path) -> None:
    global checks
    checks += 1
    if not Path(path).is_file():
        add_problem(f"{description}: expected file {path}")


with harness.temporary_dir("burdin-docs-") as work:
    status = 0
    stdout = ""
    stderr = ""

    def drive(*args, env=None) -> None:
        global status, stdout, stderr
        result = run_driver(*args, env=env)
        status = result.returncode
        stdout = harness.text(result.stdout).rstrip("\n")
        stderr = harness.text(result.stderr).rstrip("\n")

    def drive_program(path) -> None:
        global status, stdout, stderr
        result = harness.run([path], cwd=work)
        status = result.returncode
        stdout = harness.text(result.stdout).rstrip("\n")
        stderr = harness.text(result.stderr).rstrip("\n")

    readme = harness.ROOT / "README.md"
    expected_matches = re.findall(
        r"^<!-- check-docs: stdout=(.*) -->$", harness.read_text(readme), re.MULTILINE
    )
    expected = expected_matches[-1] if expected_matches else ""
    if not expected:
        add_problem("README.md: no 'check-docs' example with expected stdout")
    else:
        inside = False
        code = False
        example = []
        for line in harness.read_text(readme).split("\n"):
            if line.startswith("<!-- check-docs: stdout="):
                inside = True
                continue
            if line == "<!-- /check-docs -->":
                inside = False
            elif inside and line == "```burdin":
                code = True
                continue
            elif inside and line == "```":
                code = False
            elif inside and code:
                example.append(line)
        harness.write_text(work / "hello.bd", "\n".join(example) + ("\n" if example else ""))
        if not example:
            add_problem("README.md: the check-docs example has no ```burdin block")
        else:
            drive("run", "hello.bd")
            expect_status("README example (burdin run)", 0)
            checks += 1
            if stdout != expected:
                add_problem(f"README example: printed '{stdout}', expected '{expected}'")

            # build: default output name, -o, --emit-ir
            drive("build", "hello.bd")
            expect_status("burdin build", 0)
            exe = work / "hello"
            if not exe.is_file():
                exe = work / "hello.exe"
            expect_file("burdin build (default output)", exe)

            drive("build", "hello.bd", "-o", "custom_name")
            expect_status("burdin build -o", 0)
            custom = work / "custom_name"
            if not custom.is_file():
                custom = work / "custom_name.exe"
            expect_file("burdin build -o", custom)

            drive("build", "hello.bd", "--emit-ir", "hello.ll")
            expect_status("burdin build --emit-ir", 0)
            expect_file("burdin build --emit-ir", work / "hello.ll")
            expect_contains(
                "burdin build --emit-ir",
                "define",
                harness.read_text(work / "hello.ll") if (work / "hello.ll").is_file() else "",
            )

            # --version: the build metadata the README documents.
            drive("--version")
            expect_status("burdin --version", 0)
            for prefix in ("burdin ", "channel: ", "target: ", "llvm: ", "commit: ", "profile: "):
                expect_contains("burdin --version", prefix, stdout)

            # -g and --strip: the documented build flags keep the program's behavior.
            drive("build", "hello.bd", "-g", "-o", "hello_debug")
            expect_status("burdin build -g", 0)
            debug_exe = work / "hello_debug"
            if not debug_exe.is_file():
                debug_exe = work / "hello_debug.exe"
            drive_program(debug_exe)
            expect_status("burdin build -g executable", 0)
            checks += 1
            if stdout != expected:
                add_problem("burdin build -g executable: wrong output")

            drive("build", "hello.bd", "--strip", "-o", "hello_stripped")
            expect_status("burdin build --strip", 0)
            stripped_exe = work / "hello_stripped"
            if not stripped_exe.is_file():
                stripped_exe = work / "hello_stripped.exe"
            drive_program(stripped_exe)
            expect_status("burdin build --strip executable", 0)
            checks += 1
            if stdout != expected:
                add_problem("burdin build --strip executable: wrong output")

            drive("build", "hello.bd", "--release", "-o", "hello_release")
            expect_status("burdin build --release", 0)
            release_exe = work / "hello_release"
            if not release_exe.is_file():
                release_exe = work / "hello_release.exe"
            drive_program(release_exe)
            expect_status("burdin build --release executable", 0)
            checks += 1
            if stdout != expected:
                add_problem("burdin build --release executable: wrong output")

            # run: exit status passthrough and arguments after --
            harness.write_text(work / "exit3.bd", "fn main() { process.exit(3) }\n")
            drive("run", "exit3.bd")
            expect_status("burdin run (exit status)", 3)

            harness.write_text(
                work / "args.bd", "fn main() { println(env.args().len().to_string()) }\n"
            )
            drive("run", "args.bd", "--", "x", "y")
            expect_status("burdin run -- args", 0)
            checks += 1
            if stdout != "3":
                add_problem(f"burdin run -- args: printed '{stdout}', expected '3'")

            # --link-lib: a system library, and LLVM-C through llvm-config on Unix.
            if harness.PLATFORM in ("linux", "macos"):
                drive("build", "hello.bd", "--link-lib", "m", "-o", "hello_m")
                expect_status("burdin build --link-lib m", 0)
                llvm_config = harness.which("llvm-config")
                prefix = os.environ.get("LLVM_SYS_211_PREFIX")
                if llvm_config or (
                    prefix and os.access(Path(prefix) / "bin" / "llvm-config", os.X_OK)
                ):
                    drive("build", "hello.bd", "--link-lib", "LLVM-C", "-o", "hello_llvm")
                    expect_status("burdin build --link-lib LLVM-C", 0)

            # BURDIN_CC is the linker; a missing one is reported with the variable's name.
            drive(
                "build",
                "hello.bd",
                "-o",
                "ignored",
                env={**os.environ, "BURDIN_CC": "burdin-cc-that-does-not-exist"},
            )
            expect_status("BURDIN_CC (missing linker)", 2)
            expect_contains("BURDIN_CC (missing linker)", "BURDIN_CC", stderr)

            # Diagnostics: cargo/gleam style by default, and path:LINE:COL: error[CODE]: message in short format.
            harness.write_text(work / "bad.bd", 'fn main() {\n    let x: i32 = "hi"\n}\n')
            drive("build", "bad.bd")
            expect_status("diagnostics", 1)
            expect_contains("diagnostics", "error[E0003]", stderr)
            expect_contains("diagnostics", "--> bad.bd:2:", stderr)
            expect_contains("diagnostics", "error: could not compile bad.bd (1 error)", stderr)
            drive("build", "bad.bd", "--diagnostic-format=short")
            expect_status("diagnostics (short)", 1)
            checks += 1
            if not stderr.startswith("bad.bd:2:"):
                add_problem(f"diagnostics (short): no 'bad.bd:2:...' line: {stderr}")

            # test: the FAIL lines and the summary, with a failing test exiting 1.
            # The directory is `app` while the package is `demo`, so the default output `./demo` cannot
            # collide with the package directory.
            (work / "app" / "src").mkdir(parents=True)
            harness.write_text(work / "app" / "burdin.toml", '[package]\nname = "demo"\n')
            harness.write_text(
                work / "app" / "src" / "main.bd",
                "fn main() {}\n"
                "\n"
                'test "first passes" {\n'
                "    assert_eq(1 + 1, 2)?\n"
                "}\n"
                "\n"
                'test "second one fails" {\n'
                "    assert_eq(1 + 1, 3)?\n"
                "}\n"
                "\n"
                'test "third passes" {\n'
                "    assert(true)?\n"
                "}\n",
            )
            drive("test", "app")
            expect_status("burdin test", 1)
            expect_contains("burdin test", "FAIL second one fails", stdout)
            expect_contains("burdin test", "test result: 2 passed, 1 failed", stdout)

            # Tests are stripped from a normal build: an unknown name inside a test cannot fail it.
            harness.write_text(
                work / "app" / "src" / "main.bd",
                "fn main() {}\n"
                "\n"
                'test "not checked by a build" {\n'
                "    let x = no_such_function()\n"
                "}\n",
            )
            drive("build", "app")
            expect_status("burdin build (tests stripped)", 0)
            demo = work / "demo"
            if not demo.is_file():
                demo = work / "demo.exe"
            expect_file("burdin build (package output name)", demo)

print(f"Checked {checks} links and driver behaviors")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
