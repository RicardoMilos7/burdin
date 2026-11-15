#!/usr/bin/env bash
# Fuzzes the lexer and parser (issues #288 and #447).
#
# Every CI job replays the committed corpus through `cargo test --workspace`; this script runs a longer
# session and is called by the scheduled `fuzz` job. A found panic is written to fuzz-out/crashes/ and the
# exit status is 1; the job uploads that directory as an artifact. Commit a found input under
# crates/burdin_fuzz/corpus/syntax/ together with its fix: the replay test then fails if the panic comes
# back.
#
# Usage: tests/fuzz.sh [OPTIONS]
#   --seconds N       run for about N seconds (default: 600)
#   --iterations N    run a fixed number of mutations instead
#   --seed N          generator seed (print it to reproduce a finding)
#   --corpus PATH     a corpus directory or file; repeatable, added to the committed corpus
#   --seed-dir PATH   an extra seed directory or file; the tests and std are added by default
#   --save DIR        where found inputs are written (default: fuzz-out/crashes)
#   --watchdog N      seconds without progress before the run fails (default: 10; 0 disables)
#
# Exit status: 0 when nothing panicked, 1 on a finding or a stall, 2 on a usage or I/O error.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
cd "$ROOT" || exit 2

has_arg() {   # $1: flag; the rest: arguments
    local flag=$1
    shift
    local arg
    for arg in "$@"; do
        [ "$arg" = "$flag" ] && return 0
    done
    return 1
}

# A bare `tests/fuzz.sh` runs the scheduled budget; callers can choose either kind of budget themselves.
if ! has_arg --seconds "$@" && ! has_arg --iterations "$@"; then
    set -- --seconds 600 "$@"
fi

# Keep found inputs out of the tracked tree unless the caller chooses a directory. The committed corpus is
# always included; the conformance tests and the standard library add richer structural seeds.
if ! has_arg --save "$@"; then
    set -- --save "$ROOT/fuzz-out/crashes" "$@"
fi
if ! has_arg --seed-dir "$@"; then
    set -- --seed-dir "$ROOT/tests" --seed-dir "$ROOT/std" "$@"
fi

exec cargo run --release -p burdin_fuzz --bin burdin-fuzz-syntax -- "$@"
