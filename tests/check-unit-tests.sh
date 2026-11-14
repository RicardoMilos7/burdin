#!/usr/bin/env bash
# Checks that the compiler front end keeps its focused unit tests (issue #45).
# It does not compile anything. Usage: tests/check-unit-tests.sh
#
# `cargo test --workspace` in CI is what runs the unit tests. This script guards the
# structure that step depends on:
#   * every front-end crate (diag, syntax, tir, sema, check) has a test module with at
#     least one #[test];
#   * the four components issue #45 calls out keep their tests: the lexer token stream,
#     parser recovery, type unification, and exhaustiveness;
#   * every CI job runs the workspace unit tests.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
WORKFLOW=$ROOT/.github/workflows/ci.yml

problems=()
add_problem() { problems+=("$1"); }

has_unit_tests() {   # $1: file that must hold a test module with at least one test
    [ -f "$1" ] && grep -q '#\[cfg(test)\]' "$1" && grep -q '#\[test\]' "$1"
}

crate_has_unit_tests() {   # $1: crate: any source file holds a test module with a test
    local crate=$1 file
    while IFS= read -r file; do
        if has_unit_tests "$file"; then return 0; fi
    done < <(find "$ROOT/crates/$crate/src" -name '*.rs' 2>/dev/null | sort)
    return 1
}

for crate in burdin_diag burdin_syntax burdin_tir burdin_sema burdin_check; do
    if [ ! -d "$ROOT/crates/$crate" ]; then
        add_problem "crates/$crate: missing"
    elif ! crate_has_unit_tests "$crate"; then
        add_problem "crates/$crate: no unit tests (#[cfg(test)] module with a #[test])"
    fi
done

# The components issue #45 names. A test elsewhere in the crate does not cover these.
for file in \
    crates/burdin_syntax/src/lexer.rs \
    crates/burdin_syntax/src/parser/mod.rs \
    crates/burdin_sema/src/generics.rs \
    crates/burdin_sema/src/exhaust.rs
do
    has_unit_tests "$ROOT/$file" || add_problem "$file: missing unit tests"
done

# Every job runs the tests of every crate, so losing a crate's tests fails CI.
if [ ! -f "$WORKFLOW" ]; then
    add_problem ".github/workflows/ci.yml: missing"
else
    text=$(tr -d '\r' < "$WORKFLOW")
    for job in linux macos windows; do
        block=$(awk -v job="$job:" '
            $0 == "  " job { inside = 1; next }
            inside && /^  [A-Za-z0-9_-]+:$/ { exit }
            inside { print }
        ' <<< "$text")
        if [ -z "$block" ]; then
            add_problem "ci.yml: no $job job"
        elif ! grep -qF 'cargo test --workspace' <<< "$block"; then
            add_problem "ci.yml: $job job: does not run cargo test --workspace"
        fi
    done
fi

if [ ${#problems[@]} -gt 0 ]; then
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "Unit tests: every front-end crate and every CI job is covered."
