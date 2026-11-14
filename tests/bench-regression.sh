#!/usr/bin/env bash
# Runs the compiler benchmarks and compares them with a stored baseline (issue #446).
# Usage: tests/bench-regression.sh [BASELINE]
#
# The baseline is the plain-text file `cargo bench -p burdin_bench -- --save` writes: one
# `case<TAB>median nanoseconds` line per benchmark case. This script measures the full suite and compares
# it with the baseline; a case more than BURDIN_BENCH_THRESHOLD percent slower is a regression and the
# exit status is 1.
#
# Where the baseline lives:
#   * the first argument, else $BURDIN_BENCH_BASELINE, else ~/.cache/burdin/bench/baseline.txt;
#   * CI caches that directory with actions/cache, keyed by the runner OS and architecture, the pinned
#     Rust toolchain, and Cargo.lock, so only numbers from the same kind of machine are compared;
#   * a push to main sets BURDIN_BENCH_UPDATE=1 and records the new baseline after the comparison; a pull
#     request compares against the last main run on the same platform and never writes the cache.
#
# The comparison is advisory until the runner numbers are stable: the CI step sets continue-on-error, so
# an exit status of 1 is reported without failing the job. The harness itself exits 1, so tightening the
# policy later is a workflow change, not a code change.
#
# Environment:
#   BURDIN_BENCH_BASELINE   the baseline file (default ~/.cache/burdin/bench/baseline.txt)
#   BURDIN_BENCH_THRESHOLD  percent a case may be slower before it is flagged (default 25)
#   BURDIN_BENCH_UPDATE=1   replace the baseline with this run, regressed or not
#   BURDIN_BENCH_FILTER     measure only cases whose name contains this text (development aid)
#
# Exit status: 0 when every measured case is within the threshold (or no baseline existed), 1 when a
# case is over it, and 2 when the benchmark or a file operation fails.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")

case ${1:-} in
    -h | --help)
        echo "usage: tests/bench-regression.sh [BASELINE]"
        exit 0
        ;;
esac

if [ $# -gt 1 ]; then
    echo "usage: tests/bench-regression.sh [BASELINE]" >&2
    exit 2
fi

BASELINE=${1:-${BURDIN_BENCH_BASELINE:-$HOME/.cache/burdin/bench/baseline.txt}}
THRESHOLD=${BURDIN_BENCH_THRESHOLD:-25}
FILTER=${BURDIN_BENCH_FILTER:-}

case $THRESHOLD in
    '' | *[!0-9]*)
        echo "bench-regression: BURDIN_BENCH_THRESHOLD must be a whole percentage, got '$THRESHOLD'" >&2
        exit 2
        ;;
esac

cd "$ROOT" || exit 2

CURRENT=$(mktemp "${TMPDIR:-/tmp}/burdin-bench.XXXXXX") || exit 2
trap 'rm -f "$CURRENT"' EXIT

run_bench() {
    if [ -n "$FILTER" ]; then
        cargo bench -p burdin_bench -- "$@" "$FILTER"
    else
        cargo bench -p burdin_bench -- "$@"
    fi
}

if [ -f "$BASELINE" ]; then
    printf 'bench-regression: comparing this run with %s (threshold %s%%)\n' "$BASELINE" "$THRESHOLD"
    run_bench --save "$CURRENT" --baseline "$BASELINE" --threshold "$THRESHOLD"
else
    printf 'bench-regression: no baseline at %s; recording this run\n' "$BASELINE"
    run_bench --save "$CURRENT"
fi
status=$?

if [ "${BURDIN_BENCH_UPDATE:-0}" = 1 ] && [ -s "$CURRENT" ]; then
    mkdir -p "$(dirname "$BASELINE")" || exit 2
    cp "$CURRENT" "$BASELINE" || exit 2
    printf 'bench-regression: baseline updated: %s\n' "$BASELINE"
fi

case $status in
    0) ;;
    1) printf 'bench-regression: at least one case is over the %s%% threshold\n' "$THRESHOLD" >&2 ;;
    *) printf 'bench-regression: the benchmark run failed (exit %s)\n' "$status" >&2 ;;
esac
exit "$status"
