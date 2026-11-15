#!/usr/bin/env python3
"""Checks the platform support matrix and its known limitations (issue #457). Needs a built driver.
Usage: tests/check_platform_support.py

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin[.exe] is used.

What it checks:
  * docs/platform-support.md exists and is linked from README.md (the documentation index) and
    CHANGELOG.md (the release notes);
  * the support matrix names the three CI jobs and the runner image each one uses, keeps the untested
    configurations marked untested, and names only tests and scripts that exist and run in CI;
  * the row for the running host names the target triple `burdin --version` prints;
  * the claims under "Known limitations" still match the code and the checks (the Windows DWARF/gdb
    skip, the skipped backtrace-name test, the LTO/PGO platform split, the size budgets, the binary-mode
    implementation, and the glibc backtrace and stack-guard calls);
  * the checked example in the page compiles and prints the documented output;
  * standard output and file I/O are binary-safe on this host: the redirected stdout and the file the
    program writes contain exactly the bytes it asked for, with no CRLF translation.
"""

import platform
import re
import sys

import harness

README = harness.ROOT / "README.md"
CHANGELOG = harness.ROOT / "CHANGELOG.md"
WORKFLOW = harness.ROOT / ".github" / "workflows" / "ci.yml"
DOC = harness.ROOT / "docs" / "platform-support.md"

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


def check() -> None:
    global checks
    checks += 1


if not DOC.is_file():
    print(f"missing {DOC}", file=sys.stderr)
    sys.exit(1)
doc = harness.read_text(DOC)
workflow = harness.read_text(WORKFLOW)

burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)

# --- The page and its links --------------------------------------------------

check()
if "docs/platform-support.md" not in harness.read_text(README):
    add_problem("README.md: does not link docs/platform-support.md")
check()
if "platform-support.md" not in harness.read_text(CHANGELOG):
    add_problem("CHANGELOG.md: the release notes do not link docs/platform-support.md")

# --- The matrix agrees with CI -----------------------------------------------


def check_tested_row(os_name: str, architecture: str, job: str, image: str) -> None:
    global checks
    check()
    row = next(
        (
            line
            for line in doc.split("\n")
            if re.match(rf"^\| {re.escape(os_name)} \| {re.escape(architecture)} \| [^|]+ \| Tested \|", line)
        ),
        "",
    )
    if not row:
        add_problem(f"docs/platform-support.md: no Tested row for {os_name} {architecture}")
        return
    check()
    marker = f"`{job}`"
    if marker not in row or image not in row or row.find(marker) > row.find(image):
        add_problem(
            f"docs/platform-support.md: the {os_name} {architecture} row does not name "
            f"the '{job}' job on {image}"
        )
    check()
    if f"    runs-on: {image}" not in workflow.split("\n"):
        add_problem(f"ci.yml: no job runs on {image} (the matrix says the {job} job does)")


check_tested_row("Linux", "x86-64", "linux", "ubuntu-24.04")
check_tested_row("macOS", "arm64", "macos", "macos-latest")
check_tested_row("Windows", "x86-64", "windows", "windows-latest")

for os_name, architecture in (("Linux", "arm64"), ("macOS", "x86-64"), ("Windows", "arm64")):
    check()
    if not re.search(
        rf"^\| {re.escape(os_name)} \| {re.escape(architecture)} \| [^|]+ \| Untested \|",
        doc,
        re.MULTILINE,
    ):
        add_problem(f"docs/platform-support.md: no Untested row for {os_name} {architecture}")
check()
if not re.search(r"^\| other OS or 32-bit \|", doc, re.MULTILINE):
    add_problem("docs/platform-support.md: no Unsupported row for other hosts")

# The row for this host must carry the triple the driver prints.
system = platform.system()
machine = platform.machine().lower()
if system == "Linux" and machine == "x86_64":
    host_key = "Linux|x86-64"
elif system == "Darwin" and machine == "arm64":
    host_key = "macOS|arm64"
elif system == "Darwin" and machine == "x86_64":
    host_key = "macOS|x86-64"
elif system == "Linux" and machine in ("aarch64", "arm64"):
    host_key = "Linux|arm64"
elif system == "Windows" and machine in ("x86_64", "amd64"):
    host_key = "Windows|x86-64"
elif system == "Windows" and machine in ("aarch64", "arm64"):
    host_key = "Windows|arm64"
else:
    host_key = ""
if host_key:
    host_name, host_arch = host_key.split("|", 1)
    version = harness.run([burdin, "--version"])
    target = ""
    for line in harness.text(version.stdout).split("\n"):
        if line.startswith("target: "):
            target = line[len("target: "):].strip()
            break
    check()
    if not target:
        add_problem("burdin --version: no 'target:' line")
    else:
        row = next(
            (
                line
                for line in doc.split("\n")
                if re.match(
                    rf"^\| {re.escape(host_name)} \| {re.escape(host_arch)} \| [^|]+ \| (Tested|Untested) \|",
                    line,
                )
            ),
            "",
        )
        check()
        if target not in row:
            add_problem(
                f"docs/platform-support.md: the {host_name} {host_arch} row does not name "
                f"the host triple '{target}'"
            )

# Every test the page names must exist, and every script it names must be a CI step.
refs = sorted(set(re.findall(r"tests/[A-Za-z0-9_./-]+\.(?:py|sh|bd)", doc)))
for ref in refs:
    check()
    if not (harness.ROOT / ref).exists():
        add_problem(f"docs/platform-support.md: names a missing test: {ref}")
        continue
    if ref.endswith((".py", ".sh")):
        check()
        if ref not in workflow:
            add_problem(f"ci.yml: does not run {ref} (the platform page says CI checks it)")

# Every conformance test the page names belongs to a milestone the workflow runs.
milestones = sorted(
    {match.group(1) for match in re.finditer(r"tests/(m[0-9]+)_[A-Za-z0-9_]+/", doc)}
)
for milestone in milestones:
    check()
    if not re.search(rf"tests/run_tests\.py.*[ ]{milestone}([ ]|$)", workflow, re.MULTILINE):
        add_problem(f"ci.yml: no conformance step runs {milestone} (the platform page relies on it)")

# The known limitations are claims about the checks and the runtime, so tie each one to what makes it true.


def check_claim(file: str, text: str, claim: str) -> None:
    check()
    if text not in harness.read_text(harness.ROOT / file):
        add_problem(f"{file}: no longer contains '{text}' (the platform page says: {claim})")


check_claim(
    "tests/check_debug_info.py",
    "the Windows linker kept no DWARF",
    "Windows has no DWARF sections and the debug check skips them",
)
check_claim(
    "tests/m02_basics/panic_backtrace_names.bd",
    "//@ skip-platform: windows",
    "named backtrace frames are not checked on Windows",
)
check_claim(
    "tests/check_build_flags.py",
    "On MSVC those two options are rejected",
    "LTO and PGO generation are not exercised with MSVC",
)
check_claim(
    "tests/check_build_flags.py",
    '("linux", "macos")',
    "the size budgets are enforced only on Linux and macOS",
)
check_claim(
    "crates/burdin_codegen/src/runtime.rs",
    "_setmode",
    "the Windows standard handles are switched to binary mode",
)
check_claim(
    "crates/burdin_codegen/src/runtime.rs",
    'b"rb\\0"',
    "files are opened in binary mode",
)
check_claim(
    "crates/burdin_codegen/src/runtime.rs",
    'b"wb\\0"',
    "files are written in binary mode",
)
check_claim(
    "crates/burdin_codegen/src/runtime.rs",
    "pthread_getattr_np",
    "the Linux stack guard uses glibc's pthread calls",
)
check_claim(
    "crates/burdin_codegen/src/runtime.rs",
    "backtrace_symbols_fd",
    "symbolized backtraces use libc's backtrace calls",
)
check_claim(
    "tests/check_platform.py",
    "aarch64",
    "check-platform.py recognizes Linux and Windows on arm64",
)

# --- The checked example and binary-safe I/O ---------------------------------

with harness.temporary_dir("burdin-platform-") as work:
    expected_matches = re.findall(
        r"^<!-- check-docs: stdout=(.*) -->$", doc, re.MULTILINE
    )
    expected = expected_matches[-1] if expected_matches else ""
    if not expected:
        add_problem("docs/platform-support.md: no 'check-docs' example with expected stdout")
    else:
        inside = False
        code = False
        example = []
        for line in doc.split("\n"):
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
        harness.write_text(work / "example.bd", "\n".join(example) + ("\n" if example else ""))
        check()
        if not example:
            add_problem("docs/platform-support.md: the checked example has no ```burdin block")

        ran = harness.run([burdin, "run", "example.bd"], cwd=work)
        check()
        if ran.returncode != 0:
            detail = "\n".join(harness.text(ran.stderr).split("\n")[:5])
            add_problem(
                f"the platform page's example did not run: exit status {ran.returncode}: {detail}"
            )
        check()
        output = harness.text(ran.stdout).rstrip("\n")
        if output != expected:
            add_problem(f"the platform page's example printed '{output}', expected '{expected}'")

    # Binary mode: a redirected stdout and a written file must carry exactly the requested bytes. A text-mode
    # CRT would turn each LF into CRLF, and a round-trip through the same API would hide it; the byte count
    # does not. This is the check the Windows binary-mode row of the page refers to.
    harness.write_text(
        work / "binary_io.bd",
        "fn main() {\n"
        '    match write_file("raw_bytes.bin", "a\\nb") {\n'
        "        Ok(_) => {},\n"
        '        Err(_) => println("write failed"),\n'
        "    }\n"
        '    match read_to_string("raw_bytes.bin") {\n'
        "        Ok(text) => println(text.len().to_string()),\n"
        '        Err(_) => println("read failed"),\n'
        "    }\n"
        '    println("a")\n'
        '    println("b")\n'
        "}\n",
    )
    ran = harness.run([burdin, "run", "binary_io.bd"], cwd=work)
    check()
    if ran.returncode != 0:
        detail = "\n".join(harness.text(ran.stderr).split("\n")[:5])
        add_problem(f"the binary I/O example did not run: exit status {ran.returncode}: {detail}")
    check()
    out_size = len(ran.stdout)
    if out_size != 6:
        add_problem(f"redirected stdout is {out_size} bytes, expected 6: newline translation is on")
    check()
    try:
        written_size = harness.file_size(work / "raw_bytes.bin")
    except OSError:
        written_size = -1
    if written_size != 3:
        add_problem(
            f"the written file is {written_size} bytes, expected 3: newline translation is on"
        )
    check()
    if harness.text(ran.stdout).rstrip("\n") != "3\na\nb":
        add_problem(f"raw stdout is '{harness.text(ran.stdout)}', expected the lines 3, a, b")

print(f"Checked the platform support page ({checks} checks)")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
