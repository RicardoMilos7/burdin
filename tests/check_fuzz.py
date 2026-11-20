#!/usr/bin/env python3
"""Checks `burdin fuzz` and `burdin test --fuzz-replay` (spec/fuzz.md, issue #618). Needs a built driver and, on
Linux, clang with libFuzzer's runtimes. Usage: tests/check_fuzz.py

BURDIN selects the driver, like tests/run_tests.py. The package under tests/fuzz/planted has one target with a
planted out-of-bounds read: an input that starts with the magic `FUZZ` reaches a length byte the header does not
have. The check fuzzes it for at most 60 seconds, which must find the crash and save the input in the artifacts
directory; it then replays the committed crash through `burdin test --fuzz-replay`, which must report it with the
source location, and shows that the package's ordinary test still passes. A normal build strips fuzz targets, an
unknown target name and the options that do not apply are usage errors, and a host without libFuzzer's runtime
(macOS and Windows, whose sanitizer runtime does not start; `spec/sanitizers.md`, SAN2) rejects `burdin fuzz` with an
error that names it.
"""

import platform
import shutil
import sys
import tempfile
from pathlib import Path

import harness

problems = []
checks = 0


def add_problem(message: str) -> None:
    problems.append(message)


burdin = harness.require_burdin()
fixture = harness.TESTS / "fuzz" / "planted"
crash = fixture / "fuzz" / "artifacts" / "header" / "crash-planted"


# libFuzzer needs AddressSanitizer's runtime, and the runtime starts on Linux only (`spec/sanitizers.md`, SAN2).
fuzzing_supported = platform.system() == "Linux"


def run(*args, cwd, timeout=300):
    result = harness.run(args, cwd=cwd, timeout=timeout)
    return (
        result.returncode,
        harness.text(harness.normalize(result.stdout)),
        harness.text(harness.normalize(result.stderr)),
    )


def expect(description: str, condition: bool, detail: str = "") -> None:
    global checks
    checks += 1
    if not condition:
        add_problem(f"{description}{': ' + detail if detail else ''}")


# A working copy of the package, so the run does not touch the committed corpus or artifacts.
work = Path(tempfile.mkdtemp(prefix="burdin-fuzz-"))
try:
    package = work / "planted"
    shutil.copytree(fixture, package)
    artifacts_dir = package / "fuzz" / "artifacts" / "header"

    # The package's own test passes, and a normal build ignores the fuzz target.
    code, out, err = run(burdin, "test", str(package), cwd=work)
    expect("the package's test passes", code == 0 and "1 passed" in out, out + err)
    code, out, err = run(burdin, "build", str(package), "-o", str(work / "planted.out"), cwd=work)
    expect("a build strips the fuzz target", code == 0, err)

    # The committed crash replays: the target still fails on it, at the source line that reads past the end.
    code, out, err = run(burdin, "test", str(package), "--fuzz-replay", cwd=work)
    expect("--fuzz-replay reports the saved crash", code == 1, f"exit status {code}: {out}")
    expect("--fuzz-replay names the case", "fuzz header: crash-planted" in out, out)
    expect("--fuzz-replay shows the panic and its location", "main.bd:9:" in out and "index out of bounds" in out, out)
    expect("--fuzz-replay counts the case", "1 passed, 1 failed" in out, out)
    code, out, err = run(burdin, "test", str(package), "--fuzz-replay", "--list", cwd=work)
    expect("--list names the replay", code == 0 and "fuzz header: crash-planted" in out, out)

    # A replay reads the absolute path of the input, so a relative invocation works from any directory.
    code, out, err = run(burdin, "test", "planted", "--fuzz-replay", cwd=work)
    expect("--fuzz-replay works from the package's parent", code == 1 and "fuzz header: crash-planted" in out, out)

    # With no saved input, `--fuzz-replay` runs exactly the tests it runs without the flag.
    for saved in artifacts_dir.iterdir():
        saved.unlink()
    code, out, err = run(burdin, "test", str(package), "--fuzz-replay", cwd=work)
    expect("--fuzz-replay with no saved input runs the tests", code == 0 and "1 passed, 0 failed" in out, out + err)

    # `burdin fuzz` must find the planted bug within the time limit.
    code, out, err = run(burdin, "fuzz", str(package), "header", "--max-time", "60", cwd=package, timeout=240)
    artifacts = list(artifacts_dir.iterdir())
    if "is not supported on" in err:
        expect("burdin fuzz needs libFuzzer's runtime on this host", "burdin fuzz" in err, err)
    else:
        expect("burdin fuzz finds the planted crash", code != 0, f"exit status {code}")
        expect("burdin fuzz saves the crashing input", bool(artifacts), err[-1500:])
        expect("burdin fuzz names the artifacts directory", "artifacts" in err, err[-800:])
        if artifacts:
            found = artifacts[0]
            expect(
                "the saved input starts with the magic",
                found.read_bytes().startswith(b"FUZZ"),
                repr(found.read_bytes()),
            )
        corpus = package / "fuzz" / "corpus" / "header"
        expect("burdin fuzz keeps a corpus", corpus.is_dir() and any(corpus.iterdir()), str(corpus))

    # Usage errors. A host without libFuzzer's runtime rejects every fuzz build before it reads the target's name
    # or its options, so those three cases are checked where fuzzing itself is (spec/sanitizers.md, SAN2).
    cases = [
        (("fuzz", str(package)), "missing the fuzz target's name"),
        (("test", str(package), "--max-time", "5"), "only valid for `burdin fuzz`"),
        (("build", str(package), "--fuzz-replay"), "only valid for `burdin test`"),
    ]
    if fuzzing_supported:
        cases += [
            (("fuzz", str(package), "missing"), "has no fuzz target `missing`"),
            (("fuzz", str(package), "header", "--sanitize=address"), "do not apply"),
            (("fuzz", str(package), "header", "--lto"), "do not apply"),
        ]
    for args, message in cases:
        code, _, err = run(burdin, *args, cwd=work)
        expect(f"usage error for `{' '.join(args)}`", code == 2, f"exit status {code}: {err}")
        expect(f"usage error for `{' '.join(args)}`", message in err, err)
finally:
    shutil.rmtree(work, ignore_errors=True)

print(f"Checked {checks} fuzz behaviors")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
