#!/usr/bin/env python3
"""Runs the compiler benchmarks and compares them with a stored baseline (issue #446).

Usage: tests/bench_regression.py [BASELINE]

The baseline is the plain-text file `cargo bench -p burdin_bench -- --save` writes: one
`case<TAB>median nanoseconds` line per benchmark case. This script measures the full suite and compares
it with the baseline; a case more than BURDIN_BENCH_THRESHOLD percent slower is a regression and the
exit status is 1.

Where the baseline lives:
  * the first argument, else $BURDIN_BENCH_BASELINE, else ~/.cache/burdin/bench/baseline.txt;
  * CI caches that directory with actions/cache, keyed by the runner OS and architecture, the pinned
    Rust toolchain, and Cargo.lock, so only numbers from the same kind of machine are compared;
  * a push to main sets BURDIN_BENCH_UPDATE=1 and records the new baseline after the comparison; a pull
    request compares against the last main run on the same platform and never writes the cache.

The comparison is advisory until the runner numbers are stable: the CI step sets continue-on-error, so
an exit status of 1 is reported without failing the job. The harness itself exits 1, so tightening the
policy later is a workflow change, not a code change.

Environment:
  BURDIN_BENCH_BASELINE   the baseline file (default ~/.cache/burdin/bench/baseline.txt)
  BURDIN_BENCH_THRESHOLD  percent a case may be slower before it is flagged (default 25)
  BURDIN_BENCH_UPDATE=1   replace the baseline with this run, regressed or not
  BURDIN_BENCH_FILTER     measure only cases whose name contains this text (development aid)

Exit status: 0 when every measured case is within the threshold (or no baseline existed), 1 when a
case is over it, and 2 when the benchmark or a file operation fails.
"""

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import harness

USAGE = "usage: tests/bench_regression.py [BASELINE]"

arguments = sys.argv[1:]
if arguments and arguments[0] in ("-h", "--help"):
    print(USAGE)
    sys.exit(0)
if len(arguments) > 1:
    print(USAGE, file=sys.stderr)
    sys.exit(2)

baseline = (
    Path(arguments[0])
    if arguments
    else Path(
        os.environ.get("BURDIN_BENCH_BASELINE")
        or Path.home() / ".cache" / "burdin" / "bench" / "baseline.txt"
    )
)
threshold = os.environ.get("BURDIN_BENCH_THRESHOLD", "25")
bench_filter = os.environ.get("BURDIN_BENCH_FILTER", "")

if not threshold.isdigit():
    print(
        "bench-regression: BURDIN_BENCH_THRESHOLD must be a whole percentage, "
        f"got '{threshold}'",
        file=sys.stderr,
    )
    sys.exit(2)

os.chdir(harness.ROOT)

handle, current_name = tempfile.mkstemp(prefix="burdin-bench.")
os.close(handle)
current = Path(current_name)
try:
    if baseline.is_file():
        print(f"bench-regression: comparing this run with {baseline} (threshold {threshold}%)")
        command = [
            "cargo",
            "bench",
            "-p",
            "burdin_bench",
            "--",
            "--save",
            str(current),
            "--baseline",
            str(baseline),
            "--threshold",
            threshold,
        ]
    else:
        print(f"bench-regression: no baseline at {baseline}; recording this run")
        command = ["cargo", "bench", "-p", "burdin_bench", "--", "--save", str(current)]
    if bench_filter:
        command.append(bench_filter)

    try:
        status = subprocess.run(command).returncode
    except OSError:
        status = 2

    if os.environ.get("BURDIN_BENCH_UPDATE") == "1" and current.is_file() and current.stat().st_size > 0:
        baseline.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(current, baseline)
        print(f"bench-regression: baseline updated: {baseline}")

    if status == 1:
        print(
            f"bench-regression: at least one case is over the {threshold}% threshold",
            file=sys.stderr,
        )
    elif status not in (0, 1):
        print(f"bench-regression: the benchmark run failed (exit {status})", file=sys.stderr)
    sys.exit(status)
finally:
    current.unlink(missing_ok=True)
