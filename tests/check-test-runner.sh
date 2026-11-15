#!/usr/bin/env bash
# Checks the black-box behavior of `burdin test`: attributes, parallel execution, output capture, isolation, and
# package discovery. Needs a built driver. Usage: tests/check-test-runner.sh
#
# BURDIN selects the driver, like tests/run-tests.sh. The fixtures under tests/runner/ are ordinary packages with
# no `//@` directives, so the conformance runner ignores them.
#
# What it checks (issues #438, #439, #440, #441, #442, #443):
#   * [ignore], [should_panic], [should_panic("text")], [should_fail], and [timeout(ms)] decide pass/fail;
#     --ignored and --include-ignored select ignored tests and the summary counts what was skipped;
#   * -j runs independent tests at once (a test that waits for another only finishes with -j 2), and the report
#     keeps declaration order;
#   * a test's stdout and stderr are captured and printed only when it fails, and --verbose always prints them;
#   * every test runs in its own fresh working directory with a clean environment, and the runner removes every
#     temporary directory it created;
#   * -p and --workspace select packages, --filter selects tests, and dependency tests do not run by default.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
FIXTURES=$SCRIPT_DIR/runner

if [ -n "${BURDIN:-}" ]; then
    :
else
    for candidate in "$ROOT/target/debug/burdin" "$ROOT/target/debug/burdin.exe" \
                     "$ROOT/target/release/burdin" "$ROOT/target/release/burdin.exe"; do
        if [ -x "$candidate" ]; then BURDIN=$candidate; break; fi
    done
fi
if [ -z "${BURDIN:-}" ]; then
    echo "No driver found: build it with \`cargo build\` or set BURDIN" >&2
    exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-runner.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/scratch"

problems=()
checks=0
add_problem() { problems+=("$1"); }

status=0
out=""
err=""

# Runs the driver from the scratch working directory, so a build artifact never lands in the fixture.
run_driver() {
    ( cd "$work" && "$@" ) >"$work/stdout" 2>"$work/stderr"
    status=$?
    out=$(tr -d '\r' < "$work/stdout")
    err=$(tr -d '\r' < "$work/stderr")
}

expect_status() {   # $1 description, $2 expected status
    checks=$((checks + 1))
    [ "$status" -eq "$2" ] || add_problem "$1: exit status $status, expected $2"
}

expect_contains() {   # $1 description, $2 text, $3 output
    checks=$((checks + 1))
    case $3 in
        *"$2"*) ;;
        *) add_problem "$1: output does not contain: $2" ;;
    esac
}

expect_not_contains() {   # $1 description, $2 text, $3 output
    checks=$((checks + 1))
    case $3 in
        *"$2"*) add_problem "$1: output should not contain: $2" ;;
    esac
}

# The first and last `FAIL` lines, for the declaration-order check.
expect_fail_order() {   # $1 description, $2 first name, $3 last name
    local first last
    checks=$((checks + 1))
    first=$(grep '^FAIL ' "$work/stdout" | head -n 1)
    last=$(grep '^FAIL ' "$work/stdout" | tail -n 1)
    case $first in
        "FAIL $2") ;;
        *) add_problem "$1: first failure is '$first', expected 'FAIL $2'" ;;
    esac
    case $last in
        "FAIL $3") ;;
        *) add_problem "$1: last failure is '$last', expected 'FAIL $3'" ;;
    esac
}

# --- attributes -----------------------------------------------------------------

run_driver "$BURDIN" test "$FIXTURES/attributes" -j 4
expect_status "attributes (default)" 0
expect_contains "attributes (default)" "test result: 4 passed, 0 failed, 1 ignored" "$out"
expect_not_contains "attributes (default)" "FAIL" "$out"

run_driver "$BURDIN" test "$FIXTURES/attributes" --ignored -j 4
expect_status "attributes (--ignored)" 0
expect_contains "attributes (--ignored)" "test result: 1 passed, 0 failed" "$out"
expect_not_contains "attributes (--ignored)" "ignored" "$out"

run_driver "$BURDIN" test "$FIXTURES/attributes" --include-ignored -j 4
expect_status "attributes (--include-ignored)" 0
expect_contains "attributes (--include-ignored)" "test result: 5 passed, 0 failed" "$out"
expect_not_contains "attributes (--include-ignored)" "ignored" "$out"

# --- timeouts -------------------------------------------------------------------

run_driver "$BURDIN" test "$FIXTURES/timeout" -j 2
expect_status "timeout" 1
expect_contains "timeout" "timed out after 100 ms" "$out"
expect_contains "timeout" "FAIL hangs" "$out"
expect_contains "timeout" "test result: 1 passed, 1 failed" "$out"

# --- output capture -------------------------------------------------------------

run_driver "$BURDIN" test "$FIXTURES/output" -j 4
expect_status "output capture (default)" 1
expect_contains "output capture (default)" "visible stdout" "$out"
expect_contains "output capture (default)" "visible stderr" "$out"
expect_not_contains "output capture (default)" "hidden pass output" "$out"
expect_contains "output capture (default)" "test result: 1 passed, 2 failed" "$out"
expect_fail_order "output capture (default)" "fails loudly" "also fails"

run_driver "$BURDIN" test "$FIXTURES/output" --verbose -j 4
expect_status "output capture (--verbose)" 1
expect_contains "output capture (--verbose)" "hidden pass output" "$out"
expect_contains "output capture (--verbose)" "PASS passes quietly" "$out"
expect_contains "output capture (--verbose)" "FAIL fails loudly" "$out"

# --- isolation and cleanup ------------------------------------------------------

# The driver's temporary directories (the test binaries and each test's working directory) go under TMPDIR/TMP/TEMP,
# so an empty scratch directory afterwards proves the runner cleaned up after itself.
scratch_tmp=$work/scratch
if command -v cygpath >/dev/null 2>&1; then
    scratch_tmp=$(cygpath -w "$work/scratch")
fi
( cd "$work" && env TMPDIR="$scratch_tmp" TMP="$scratch_tmp" TEMP="$scratch_tmp" \
    "$BURDIN" test "$FIXTURES/isolation" -j 1 ) >"$work/stdout" 2>"$work/stderr"
status=$?
out=$(tr -d '\r' < "$work/stdout")
err=$(tr -d '\r' < "$work/stderr")
expect_status "isolation" 0
expect_contains "isolation" "test result: 2 passed, 0 failed" "$out"
checks=$((checks + 1))
if [ -n "$(ls -A "$work/scratch" 2>/dev/null)" ]; then
    add_problem "isolation: the runner left temporary files behind: $(ls -A "$work/scratch" | tr '\n' ' ')"
fi

# --- parallelism ----------------------------------------------------------------

# `waiter` only finishes if `creator` runs at the same time, which the isolated directories make a real test of
# concurrency: a sequential runner starts `waiter` first and it times out.
run_driver "$BURDIN" test "$FIXTURES/parallel" -j 2
expect_status "parallel (-j 2)" 0
expect_contains "parallel (-j 2)" "test result: 2 passed, 0 failed" "$out"

run_driver "$BURDIN" test "$FIXTURES/parallel" -j 1
expect_status "parallel (-j 1)" 1
expect_contains "parallel (-j 1)" "FAIL waiter" "$out"
expect_contains "parallel (-j 1)" "test result: 1 passed, 1 failed" "$out"

# --- discovery and selection ----------------------------------------------------

workspace=$FIXTURES/workspace
run_driver "$BURDIN" test "$workspace" -j 4
expect_status "discovery (default)" 0
expect_contains "discovery (default)" "test result: 2 passed, 0 failed" "$out"
expect_not_contains "discovery (default)" "lib one" "$out"

run_driver "$BURDIN" test "$workspace" -p util -j 4
expect_status "discovery (-p util)" 0
expect_contains "discovery (-p util)" "test result: 1 passed, 0 failed" "$out"

# A library package has no src/main.bd; its tests still run when it is the target.
run_driver "$BURDIN" test "$workspace/util" -j 4
expect_status "discovery (library target)" 0
expect_contains "discovery (library target)" "test result: 1 passed, 0 failed" "$out"

run_driver "$BURDIN" test "$workspace" --workspace -j 4
expect_status "discovery (--workspace)" 0
expect_contains "discovery (--workspace)" "test result: 3 passed, 0 failed" "$out"

run_driver "$BURDIN" test "$workspace" --workspace --filter "lib" -j 4
expect_status "discovery (--filter)" 0
expect_contains "discovery (--filter)" "test result: 1 passed, 0 failed" "$out"

run_driver "$BURDIN" test "$workspace" --filter "no such test" -j 4
expect_status "discovery (empty filter)" 0
expect_contains "discovery (empty filter)" "test result: 0 passed, 0 failed" "$out"

run_driver "$BURDIN" test "$workspace" -p missing -j 4
expect_status "discovery (unknown package)" 2
expect_contains "discovery (unknown package)" "no package named \`missing\`" "$err"

run_driver "$BURDIN" test "$workspace" -p std -j 4
expect_status "discovery (embedded package)" 2
expect_contains "discovery (embedded package)" "embedded" "$err"

# --- job control ----------------------------------------------------------------

run_driver "$BURDIN" test "$FIXTURES/attributes" -j 0
expect_status "job control (-j 0)" 2
expect_contains "job control (-j 0)" "at least 1" "$err"

run_driver "$BURDIN" test "$FIXTURES/attributes" --ignored --include-ignored
expect_status "job control (conflicting ignore flags)" 2
expect_contains "job control (conflicting ignore flags)" "cannot be used together" "$err"

run_driver "$BURDIN" test "$FIXTURES/attributes" -p attributes --workspace
expect_status "job control (conflicting selection flags)" 2
expect_contains "job control (conflicting selection flags)" "cannot be used together" "$err"

echo "Checked $checks runner behaviors"
if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
