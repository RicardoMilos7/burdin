#!/usr/bin/env python3
"""Fuzzes the lexer and parser (issues #288 and #447).

Every CI job replays the committed corpus through `cargo test --workspace`; this script runs a longer
session and is called by the scheduled `fuzz` job. A found panic is written to fuzz-out/crashes/ and the
exit status is 1; the job uploads that directory as an artifact. Commit a found input under
crates/burdin_fuzz/corpus/syntax/ together with its fix: the replay test then fails if the panic comes
back.

Usage: tests/fuzz.py [OPTIONS]
  --seconds N       run for about N seconds (default: 600)
  --iterations N    run a fixed number of mutations instead
  --seed N          generator seed (print it to reproduce a finding)
  --corpus PATH     a corpus directory or file; repeatable, added to the committed corpus
  --seed-dir PATH   an extra seed directory or file; the tests and std are added by default
  --save DIR        where found inputs are written (default: fuzz-out/crashes)
  --watchdog N      seconds without progress before the run fails (default: 10; 0 disables)

Exit status: 0 when nothing panicked, 1 on a finding or a stall, 2 on a usage or I/O error.
"""

import subprocess
import sys

import harness

arguments = sys.argv[1:]

# A bare `tests/fuzz.py` runs the scheduled budget; callers can choose either kind of budget themselves.
if "--seconds" not in arguments and "--iterations" not in arguments:
    arguments = ["--seconds", "600"] + arguments

# Keep found inputs out of the tracked tree unless the caller chooses a directory. The committed corpus is
# always included; the conformance tests and the standard library add richer structural seeds.
if "--save" not in arguments:
    arguments = ["--save", str(harness.ROOT / "fuzz-out" / "crashes")] + arguments
if "--seed-dir" not in arguments:
    arguments = ["--seed-dir", str(harness.TESTS), "--seed-dir", str(harness.ROOT / "std")] + arguments

command = [
    "cargo",
    "run",
    "--release",
    "-p",
    "burdin_fuzz",
    "--bin",
    "burdin-fuzz-syntax",
    "--",
    *arguments,
]

try:
    result = subprocess.run(command, cwd=str(harness.ROOT))
except OSError as error:
    print(f"error: cannot run cargo: {error}", file=sys.stderr)
    sys.exit(2)
sys.exit(result.returncode)
