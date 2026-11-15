#!/usr/bin/env bash
# Conformance test runner for Burdin. The test format is described in tests/README.md.
#
# Usage: tests/run-tests.sh [options] [filter ...]
#   -r, --release       use the release profile (tests marked debug-only are skipped)
#   -v, --verbose       show diffs and compiler output for failures
#   -l, --list          list the tests that would run, without running them
#   -t, --timeout SEC   seconds allowed per test (default 30)
#   -h, --help          show this help
#   filter              run only tests whose path contains one of these strings
#                       (for example: m05, drop_order, m06_conventions/exclusivity)
#
# Environment:
#   BURDIN   path to the compiler driver (default: target/{debug,release}/burdin[.exe], then PATH)
#
# ---------------------------------------------------------------------------
# Driver contract assumed by this script. If your driver differs, change only this section.
# The full contract, including --emit-ir, --link-lib (and LLVM-C), and BURDIN_CC, is documented in
# README.md at the repository root (`The driver contract`), and tests/check-docs.sh checks it.
#
#   burdin build <target> [--release]
#       Compiles. Diagnostics go to stderr or stdout, one per line, in the form
#           path:LINE:COL: error[E0000]: message
#       and the exit code is non-zero when compilation fails.
#   burdin run <target> [--release] [--link-lib NAME]... [-- args...]
#       Compiles and runs. The program's stdout and stderr pass through, and its exit code
#       is the command's exit code. A panic abort must give a non-zero exit code.
#   burdin test <target> [--release]
#       Prints "test result: N passed, M failed" (with ", I ignored" when any test is marked [ignore]) and
#       one "FAIL <test name>" line per failed test.
#
# <target> is the test's .bd file, or the package directory (the nearest folder with a burdin.toml).
# ---------------------------------------------------------------------------

burdin_build() { run_limited "$BURDIN" build "$@"; }
burdin_run()   { run_limited "$BURDIN" run "$@"; }
burdin_test()  { run_limited "$BURDIN" test "$@"; }

DIAGNOSTIC_REGEX=':[0-9]+:[0-9]+: error\[E[0-9]{4}\]'
TEST_SUMMARY_REGEX='test result: ([0-9]+) passed, ([0-9]+) failed(, ([0-9]+) ignored)?'
FAILED_TEST_PREFIX='FAIL '
# ---------------------------------------------------------------------------

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TESTS_DIR=$SCRIPT_DIR
REPO_DIR=$(dirname "$SCRIPT_DIR")

PROFILE_FLAG=""
VERBOSE=0
LIST_ONLY=0
TIMEOUT=30
FILTERS=()

usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
    case "$1" in
        -r|--release) PROFILE_FLAG="--release" ;;
        -v|--verbose) VERBOSE=1 ;;
        -l|--list) LIST_ONLY=1 ;;
        -t|--timeout) shift; TIMEOUT="$1" ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
        *) FILTERS+=("$1") ;;
    esac
    shift
done

run_limited() {
    if command -v timeout >/dev/null 2>&1; then
        timeout "$TIMEOUT" "$@"
    else
        "$@"
    fi
}

resolve_burdin() {
    if [ -n "${BURDIN:-}" ]; then return 0; fi
    local c
    for c in "$REPO_DIR/target/debug/burdin" "$REPO_DIR/target/debug/burdin.exe" \
             "$REPO_DIR/target/release/burdin" "$REPO_DIR/target/release/burdin.exe"; do
        if [ -x "$c" ]; then BURDIN=$c; return 0; fi
    done
    BURDIN=$(command -v burdin 2>/dev/null) && return 0
    return 1
}

if [ "$LIST_ONLY" -eq 0 ]; then
    if ! resolve_burdin; then
        echo "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin" >&2
        exit 2
    fi
    case "$BURDIN" in /*|[A-Za-z]:*) ;; *) BURDIN="$(pwd)/$BURDIN" ;; esac
fi

# ----- helpers --------------------------------------------------------------

directives_of() { grep '^//@ ' "$1" | tr -d '\r' | sed 's|^//@ ||'; }

has_directive() { directives_of "$1" | grep -Eq "^$2( |:|\$)"; }

directive_values() { directives_of "$1" | grep -E "^$2:" | sed -E "s/^$2: ?//"; }

kind_of() {
    local f=$1 k
    if has_directive "$f" undecided; then echo undecided; return; fi
    for k in run-abort run test-compile-fail compile-fail burdin-test; do
        if has_directive "$f" "$k"; then echo "$k"; return; fi
    done
    echo unknown
}

# the package directory (nearest ancestor with burdin.toml inside tests/), or the file itself
target_of() {
    local d
    d=$(dirname "$1")
    while [ "$d" != "$TESTS_DIR" ] && [ "$d" != "/" ] && [ "$d" != "." ]; do
        if [ -f "$d/burdin.toml" ]; then echo "$d"; return; fi
        d=$(dirname "$d")
    done
    echo "$1"
}

expected_stdout() { tr -d '\r' < "$1" | grep '^//> ' | cut -c5-; }

expected_markers() {
    grep -no '//~ E[0-9]\{4\}' "$1" | tr -d '\r' | sed -E 's|^([0-9]+)://~ (E[0-9]{4})$|\1 \2|' | sort -u
}

REASON=""
DETAIL=""

fail() { REASON="$1"; DETAIL="${2:-}"; return 1; }

check_stderr_contains() {
    local root=$1 errfile=$2 text
    while IFS= read -r text; do
        [ -z "$text" ] && continue
        if ! grep -Fq -- "$text" "$errfile"; then
            fail "stderr does not contain: $text" "$(head -c 2000 "$errfile")"
            return 1
        fi
    done < <(directive_values "$root" stderr)
    return 0
}

# ----- test kinds -----------------------------------------------------------

test_run() {   # $1 root, $2 target, $3 logs, $4 workdir, $5 kind
    local root=$1 target=$2 logs=$3 work=$4 kind=$5
    local args="" libs="" l code want_code

    args=$(directive_values "$root" args | head -n 1)
    for l in $(directive_values "$root" link-lib); do libs="$libs --link-lib $l"; done
    want_code=$(directive_values "$root" exit-code | head -n 1)
    want_code=${want_code:-0}

    ( cd "$work" && burdin_run "$target" $PROFILE_FLAG $libs -- $args ) >"$logs/out" 2>"$logs/err"
    code=$?
    tr -d '\r' < "$logs/out" > "$logs/out.norm"

    if [ "$code" -eq 124 ]; then fail "timed out after ${TIMEOUT}s"; return 1; fi
    if grep -Eq 'error\[E[0-9]{4}\]' "$logs/err" "$logs/out" 2>/dev/null && [ "$kind" != "compile-fail" ]; then
        fail "compilation failed" "$(grep -hE 'error\[E[0-9]{4}\]' "$logs/err" "$logs/out" | head -n 10)"
        return 1
    fi

    expected_stdout "$root" > "$logs/expected"
    if [ "$kind" = "run" ]; then
        if [ "$code" -ne "$want_code" ]; then
            fail "exit code $code, expected $want_code" "$(head -c 2000 "$logs/err")"
            return 1
        fi
    else
        if [ "$code" -eq 0 ]; then fail "expected an abort but the program exited with 0"; return 1; fi
        check_stderr_contains "$root" "$logs/err" || return 1
    fi
    if ! diff -u "$logs/expected" "$logs/out.norm" > "$logs/diff"; then
        fail "stdout differs from the //> lines" "$(head -n 40 "$logs/diff")"
        return 1
    fi
    return 0
}

test_compile_fail() {   # $1 root, $2 target, $3 logs, $4 work, $5 build|test
    local root=$1 target=$2 logs=$3 work=$4 mode=${5:-build} code base
    base=$(basename "$root")

    if [ "$mode" = "test" ]; then
        ( cd "$work" && burdin_test "$target" $PROFILE_FLAG ) >"$logs/out" 2>&1
    else
        ( cd "$work" && burdin_build "$target" $PROFILE_FLAG ) >"$logs/out" 2>&1
    fi
    code=$?
    tr -d '\r' < "$logs/out" > "$logs/out.norm"

    if [ "$code" -eq 124 ]; then fail "timed out after ${TIMEOUT}s"; return 1; fi
    if [ "$code" -eq 0 ]; then fail "compilation succeeded but errors were expected"; return 1; fi

    expected_markers "$root" > "$logs/expected"
    grep -E "$DIAGNOSTIC_REGEX" "$logs/out.norm" | grep -F "$base:" \
        | sed -E 's|.*:([0-9]+):[0-9]+: error\[(E[0-9]{4})\].*|\1 \2|' | sort -u > "$logs/actual"

    if ! diff -u "$logs/expected" "$logs/actual" > "$logs/diff"; then
        fail "diagnostics differ from the //~ markers (- expected, + actual; format: LINE CODE)" \
             "$(head -n 30 "$logs/diff")"
        return 1
    fi
    return 0
}

test_burdin_test() {
    local root=$1 target=$2 logs=$3 work=$4
    local want_p want_f want_i got_p got_f got_i name directive

    ( cd "$work" && burdin_test "$target" $PROFILE_FLAG ) >"$logs/out" 2>&1
    tr -d '\r' < "$logs/out" > "$logs/out.norm"

    directive=$(directives_of "$root" | grep -E '^burdin-test passed=[0-9]+ failed=[0-9]+' | head -n 1)
    want_p=$(sed -nE 's/.*passed=([0-9]+).*/\1/p' <<< "$directive")
    want_f=$(sed -nE 's/.*failed=([0-9]+).*/\1/p' <<< "$directive")
    want_i=$(sed -nE 's/.*ignored=([0-9]+).*/\1/p' <<< "$directive")
    want_i=${want_i:-0}
    got_p=$(sed -nE "s/.*$TEST_SUMMARY_REGEX.*/\1/p" "$logs/out.norm" | tail -n 1)
    got_f=$(sed -nE "s/.*$TEST_SUMMARY_REGEX.*/\2/p" "$logs/out.norm" | tail -n 1)
    got_i=$(sed -nE "s/.*$TEST_SUMMARY_REGEX.*/\4/p" "$logs/out.norm" | tail -n 1)
    got_i=${got_i:-0}

    if [ -z "$got_p" ]; then fail "no test summary line in the output" "$(head -c 2000 "$logs/out.norm")"; return 1; fi
    if [ "$got_p" != "$want_p" ] || [ "$got_f" != "$want_f" ] || [ "$got_i" != "$want_i" ]; then
        fail "got $got_p passed, $got_f failed, $got_i ignored; expected $want_p passed, $want_f failed, $want_i ignored" \
             "$(head -c 2000 "$logs/out.norm")"
        return 1
    fi
    while IFS= read -r name; do
        [ -z "$name" ] && continue
        if ! grep -Fq -- "$FAILED_TEST_PREFIX$name" "$logs/out.norm"; then
            fail "test not reported as failed: $name" "$(head -c 2000 "$logs/out.norm")"
            return 1
        fi
    done < <(directive_values "$root" failed-test)
    return 0
}

# ----- main loop ------------------------------------------------------------

matches_filter() {
    local rel=$1 f
    [ ${#FILTERS[@]} -eq 0 ] && return 0
    for f in "${FILTERS[@]}"; do
        case "$rel" in *"$f"*) return 0 ;; esac
    done
    return 1
}

passed=0; failed=0; skipped=0
failed_list=()

while IFS= read -r root; do
    rel=${root#"$TESTS_DIR"/}
    matches_filter "$rel" || continue
    kind=$(kind_of "$root")

    if [ "$LIST_ONLY" -eq 1 ]; then
        printf '%-13s %s\n' "$kind" "$rel"
        continue
    fi

    if [ "$kind" = "undecided" ]; then
        printf 'SKIP  %s (undecided)\n' "$rel"; skipped=$((skipped + 1)); continue
    fi
    if [ "$kind" = "unknown" ]; then
        printf 'FAIL  %s: no test kind directive\n' "$rel"
        failed=$((failed + 1)); failed_list+=("$rel"); continue
    fi
    if [ -n "$PROFILE_FLAG" ] && has_directive "$root" debug-only; then
        printf 'SKIP  %s (debug-only)\n' "$rel"; skipped=$((skipped + 1)); continue
    fi

    target=$(target_of "$root")
    logs=$(mktemp -d)
    work=$(mktemp -d)
    REASON=""; DETAIL=""

    case "$kind" in
        run|run-abort)    test_run "$root" "$target" "$logs" "$work" "$kind" ;;
        compile-fail)     test_compile_fail "$root" "$target" "$logs" "$work" build ;;
        test-compile-fail) test_compile_fail "$root" "$target" "$logs" "$work" test ;;
        burdin-test)      test_burdin_test "$root" "$target" "$logs" "$work" ;;
    esac
    result=$?

    if [ "$result" -eq 0 ]; then
        printf 'PASS  %s\n' "$rel"; passed=$((passed + 1))
    else
        printf 'FAIL  %s: %s\n' "$rel" "$REASON"
        if [ "$VERBOSE" -eq 1 ] && [ -n "$DETAIL" ]; then printf '%s\n' "$DETAIL" | sed 's/^/        /'; fi
        failed=$((failed + 1)); failed_list+=("$rel")
    fi
    rm -rf "$logs" "$work"
done < <(grep -rl --include='*.bd' '^//@ ' "$TESTS_DIR" | sort)

if [ "$LIST_ONLY" -eq 1 ]; then exit 0; fi

echo
echo "$passed passed, $failed failed, $skipped skipped"
if [ "$failed" -gt 0 ]; then
    echo "Failed:"
    printf '  %s\n' "${failed_list[@]}"
    exit 1
fi
exit 0
