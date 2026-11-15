#!/usr/bin/env python3
"""Checks that the compiler targets the machine it runs on (issue #17). Usage: tests/check_platform.py

It drives the built `burdin` binary, so build the compiler first. It proves three things:
  * the emitted LLVM module names the host triple (arch and OS), not a hardcoded one;
  * `usize` and `isize` carry 64-bit values, on every supported host;
  * the linked executable runs on the host (the object and the `cc` linker agree on the target).
The conformance suite itself is the check that no test assumes x86-64: it runs on the macOS arm64 job.

Environment:
  BURDIN   path to the compiler driver (default: target/{debug,release}/burdin[.exe], then PATH)
"""

import platform as host
import re
import sys

import harness

problems = []


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin(
    2, "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin"
)

system = host.system()
machine = host.machine()
lowered = machine.lower()

if system == "Darwin" and lowered == "arm64":
    expected_arch, expected_os = "arm64", "apple-darwin"
elif system == "Darwin" and lowered == "x86_64":
    expected_arch, expected_os = "x86_64", "apple-darwin"
elif system == "Linux" and lowered == "x86_64":
    expected_arch, expected_os = "x86_64", "linux"
elif system == "Linux" and lowered in ("aarch64", "arm64"):
    expected_arch, expected_os = "aarch64", "linux"
elif system == "Windows" and lowered in ("x86_64", "amd64"):
    expected_arch, expected_os = "x86_64", "windows"
elif system == "Windows" and lowered in ("aarch64", "arm64"):
    expected_arch, expected_os = "aarch64", "windows"
else:
    print(
        f"SKIP  unsupported host {system}/{machine} "
        "(expected macOS, Linux, or Windows on x86_64/aarch64)"
    )
    sys.exit(0)

exe_name = "host.exe" if harness.PLATFORM == "windows" else "host"

with harness.temporary_dir("burdin-platform-") as work:
    harness.write_text(
        work / "host.bd",
        "fn main() {\n"
        "    let wide: usize = 18446744073709551615\n"
        "    println(wide.to_string())\n"
        "    let signed: isize = 9223372036854775807\n"
        "    println(signed.to_string())\n"
        "}\n",
    )

    built = harness.run(
        [burdin, "build", work / "host.bd", "--emit-ir", work / "host.ll", "-o", work / exe_name],
        merge=True,
    )
    if built.returncode != 0:
        print(f"error: the compiler could not build for {system}/{machine}", file=sys.stderr)
        sys.stdout.write(harness.text(built.stdout))
        sys.exit(1)

    # The module must name the host triple, and the triple must agree with the C linker that links it.
    triple = ""
    for line in harness.read_text(work / "host.ll").split("\n"):
        match = re.match(r'^target triple = "([^"]*)"', line)
        if match:
            triple = match.group(1)
            break
    if not triple:
        add_problem("the emitted IR has no target triple")
    else:
        if not triple.startswith(expected_arch + "-"):
            add_problem(f"IR triple '{triple}' does not start with the host arch '{expected_arch}'")
        if expected_os not in triple:
            if "apple-darwin" in triple or "apple-macos" in triple:
                if expected_os != "apple-darwin":
                    add_problem(f"IR triple '{triple}' is not a {expected_os} target")
            else:
                add_problem(f"IR triple '{triple}' is not a {expected_os} target")

    linker = harness.run(["cc", "--print-target-triple"])
    if linker.returncode != 0:
        linker = harness.run(["cc", "-dumpmachine"])
    linker_target = harness.text(linker.stdout).strip() if linker.returncode == 0 else ""
    if linker_target and not linker_target.startswith(expected_arch + "-"):
        add_problem(f"the linker cc targets '{linker_target}', not the host arch '{expected_arch}'")

    # `usize` and `isize` are 64-bit on every supported platform. A narrower type either rejects the
    # literal (E0008) or prints a truncated value, so this checks the width observably.
    ran = harness.run([work / exe_name], merge=True)
    got = harness.text(ran.stdout).rstrip("\n")
    want = "18446744073709551615\n9223372036854775807"
    if ran.returncode != 0:
        add_problem("the built program did not run on the host")
    elif got != want:
        add_problem(f"usize/isize are not 64-bit: expected '{want}', got '{got}'")

print(f"Checked host {system}/{machine} (triple {triple}).")

if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
