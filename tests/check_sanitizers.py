#!/usr/bin/env python3
"""Checks `--sanitize` (spec/sanitizers.md, issue #620). Needs a built driver; on Linux it also needs clang 21 with
compiler-rt, which links the sanitizer runtimes. Usage: tests/check_sanitizers.py

BURDIN selects the driver, like tests/run_tests.py. The programs under tests/sanitize/ have no `//@` directives, so
the conformance runner ignores them. Each one has one bug in `unsafe` code or in a C library call:

  * an out-of-bounds write and a use after free of a C allocation, which `address` reports;
  * a C allocation that is never freed, which `leak` (and `address` on Linux) reports;
  * a data race between two threads on a C cell, which `thread` reports;
  * a null and a misaligned `CPtr` dereference and an integer overflow in a release build, which `undefined`
    reports as panics at the Burdin source location.

`temporaries.bd` has no bug: it borrows temporaries the code generator once leaked, and must run clean under
`address` and `leak`.

Without the sanitizer each program finishes normally (a null dereference faults either way, so that fixture is
checked for the message instead), so the report comes from the sanitizer and not from the program. `burdin test --sanitize` instruments a test binary the same way. The command line rejects unknown
sanitizers, `thread` together with `address` or `leak`, `--sanitize` with `--lto`, and `burdin bench --sanitize`.
On a platform without a sanitizer the driver says so (checked on every host for the sanitizers it lacks).
"""

import os
import platform
import subprocess
import sys

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()
fixtures = harness.TESTS / "sanitize"
system = platform.system()
# The sanitizers this host supports (docs/platform-support.md); `undefined` needs no runtime library.
supported = {
    "Linux": {"address", "thread", "leak", "undefined"},
    "Darwin": {"address", "thread", "undefined"},
}.get(system, {"undefined"})


def call(args, cwd, timeout, env=None):
    """Runs a command, reporting a timeout as exit status 124 so one hung case does not take the whole job down
    with a traceback (the sanitizer runtimes can wait a long time on a runner)."""
    try:
        result = harness.run(args, cwd=cwd, timeout=timeout, env=env)
    except subprocess.TimeoutExpired as expired:
        return 124, harness.text(harness.normalize(expired.stdout or b"")), harness.text(
            harness.normalize(expired.stderr or b"")
        )
    return (
        result.returncode,
        harness.text(harness.normalize(result.stdout)),
        harness.text(harness.normalize(result.stderr)),
    )


def run(*args, cwd, timeout=300):
    return call(args, cwd, timeout)


def sanitizer_env():
    """The environment a sanitized program runs in. On macOS, AddressSanitizer symbolizes through `atos`, a
    separate process that can hang on a runner where nothing is watching, so it is asked not to symbolize; it also
    reports what it initialized, because a sanitizer that never initialized shows up as a silent crash or a hang and
    its first lines on standard error are what says so."""
    env = dict(os.environ)
    if system == "Darwin":
        env["ASAN_OPTIONS"] = "symbolize=0:verbosity=1"
        env["TSAN_OPTIONS"] = "symbolize=0:verbosity=1"
    return env


def brief(code, err):
    """Exit status and the first lines of standard error, for a failure's detail."""
    lines = [line for line in err.strip().split("\n") if line.strip()][:8]
    return f"exit {code}: " + " | ".join(lines) if lines else f"exit {code} with no standard error"


def expect(description: str, condition: bool, detail: str = "") -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(f"{description}{': ' + detail if detail else ''}")


# Each program, its sanitizer, the text the report must contain, and the profile it is built in. The optimizer is
# free to delete a dead store to freed memory, so the memory errors are built in the debug profile; the overflow is
# built in release, where it would otherwise wrap.
CASES = [
    ("heap_overflow", "address", "ERROR: AddressSanitizer: heap-buffer-overflow", False),
    ("use_after_free", "address", "ERROR: AddressSanitizer: heap-use-after-free", False),
    ("leak", "leak", "ERROR: LeakSanitizer: detected memory leaks", False),
    ("data_race", "thread", "WARNING: ThreadSanitizer: data race", False),
    ("null_deref", "undefined", "null_deref.bd:4:14: panic: null pointer dereference", False),
    ("misaligned", "undefined", "misaligned.bd:12:9: panic: misaligned pointer dereference: `i64` needs 8-byte alignment", False),
    ("overflow", "undefined", "overflow.bd:4:5: panic: attempt to add with overflow", True),
]
if system == "Linux":
    # LeakSanitizer is part of AddressSanitizer on Linux.
    CASES.append(("leak", "address", "ERROR: LeakSanitizer: detected memory leaks", False))

with harness.temporary_dir("burdin-sanitize-") as work:
    for name, sanitizer, report, release in CASES:
        program = str(fixtures / f"{name}.bd")
        profile = ["--release"] if release else []
        what = f"{name} ({sanitizer})"
        if sanitizer not in supported:
            code, _, err = run(burdin, "run", program, f"--sanitize={sanitizer}", cwd=work)
            expect(f"{what}: unsupported on {system}", code == 2, f"exit status {code}")
            expect(f"{what}: unsupported on {system}", "is not supported on" in err, err)
            continue
        # Without the sanitizer the bug goes unnoticed: the program finishes. A null dereference is the exception:
        # the hardware faults on it too, so that case is about the message `undefined` gives, not about detection.
        if name != "null_deref":
            code, out, err = run(burdin, "run", program, *profile, cwd=work)
            expect(
                f"{what}: runs without the sanitizer", code == 0 and "finished" in out, f"exit {code}: {err[-800:]}"
            )
        # Built first, then run: a sanitizer that hangs on this host says which half hung.
        exe = work / f"{name}-{sanitizer}.out"
        code, _, err = run(
            burdin, "build", program, *profile, f"--sanitize={sanitizer}", "-o", str(exe), cwd=work, timeout=300
        )
        expect(f"{what}: builds", code == 0, f"exit status {code}: {err[-1500:]}")
        if code == 0:
            code, out, err = call([exe], work, timeout=60, env=sanitizer_env())
            expect(f"{what}: reported", code != 0, brief(code, err))
            expect(f"{what}: reported", report in err, brief(code, err))

    # Safe code that borrows temporaries frees them: the program runs clean under every leak detector the host has.
    expected = "false true\n2 2\n1\n2\n1\n2\nk 1\n8\ntrue\ntrue\nfinished\n"
    for sanitizer in sorted({"address", "leak"} & supported):
        code, out, err = run(
            burdin, "run", str(fixtures / "temporaries.bd"), f"--sanitize={sanitizer}", cwd=work, timeout=180
        )
        expect(f"temporaries ({sanitizer}): no report", code == 0 and out == expected, f"exit {code}: {out}{err[-1500:]}")

    # `burdin build --sanitize` writes the instrumented program, and `burdin test --sanitize` runs instrumented tests.
    if "address" in supported:
        exe = work / "heap_overflow.out"
        code, _, err = run(burdin, "build", str(fixtures / "heap_overflow.bd"), "--sanitize=address", "-o", str(exe), cwd=work)
        expect("build --sanitize=address", code == 0, err)
        if code == 0:
            code, _, err = call([exe], work, timeout=60, env=sanitizer_env())
            expect(
                "build --sanitize=address: the program reports",
                code != 0 and "heap-buffer-overflow" in err,
                brief(code, err),
            )
        code, out, err = run(
            burdin, "test", str(fixtures / "tests.bd"), "--sanitize=address", cwd=work, timeout=300
        )
        expect("test --sanitize=address", code == 1, f"exit status {code}")
        expect("test --sanitize=address", "test result: 1 passed, 1 failed" in out, out)
        expect("test --sanitize=address", "FAIL writes past the end" in out, out)
        expect("test --sanitize=address", "heap-buffer-overflow" in out, out[-1500:])
        code, out, err = run(burdin, "test", str(fixtures / "tests.bd"), cwd=work)
        expect("test without --sanitize", code == 0 and "test result: 2 passed, 0 failed" in out, out + err)

    # Usage errors.
    program = str(fixtures / "overflow.bd")
    for args, message in [
        (("run", program, "--sanitize=memory"), "unknown sanitizer `memory`"),
        (("run", program, "--sanitize=thread,address"), "cannot be combined with `address` or `leak`"),
        (("run", program, "--sanitize=undefined,undefined"), "named twice"),
        (("build", program, "--sanitize=undefined", "--lto"), "cannot be combined with `--lto`"),
        (("bench", program, "--sanitize=undefined"), "only valid for `burdin build`, `burdin run`, and `burdin test`"),
    ]:
        code, _, err = run(burdin, *args, cwd=work)
        expect(f"usage error for {' '.join(args[2:])}", code == 2, f"exit status {code}")
        expect(f"usage error for {' '.join(args[2:])}", message in err, err)

print(f"Checked {checks} sanitizer behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
