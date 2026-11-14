#!/usr/bin/env bash
# Checks that the CI workflow still covers everything the project requires.
# It does not compile anything. Usage: tests/check-ci.sh
#
# This guards the invariants behind the first green CI run (issue #11):
#   * the linux job and every required step are present;
#   * LLVM 21 is installed from apt.llvm.org and pointed at;
#   * every milestone folder under tests/ is passed to the conformance runner;
#   * the scripts CI runs exist, are executable, and use LF endings;
#   * a conformance test still exercises --link-lib LLVM-C.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
WORKFLOW=$ROOT/.github/workflows/ci.yml

problems=()
add_problem() { problems+=("$1"); }

if [ ! -f "$WORKFLOW" ]; then
    echo "missing $WORKFLOW" >&2
    exit 1
fi
text=$(tr -d '\r' < "$WORKFLOW")

grep -qx '  linux:' <<< "$text" || add_problem "ci.yml: no linux job"
grep -qx '    runs-on: ubuntu-24.04' <<< "$text" || add_problem "ci.yml: linux does not run on ubuntu-24.04"
grep -qx '  push:' <<< "$text" || add_problem "ci.yml: does not run on push"
grep -qx '  pull_request:' <<< "$text" || add_problem "ci.yml: does not run on pull_request"
grep -qF 'LLVM_SYS_211_PREFIX: /usr/lib/llvm-21' <<< "$text" \
    || add_problem "ci.yml: LLVM_SYS_211_PREFIX is not set"
grep -qF 'llvm.sh 21' <<< "$text" || add_problem "ci.yml: LLVM 21 is not installed from apt.llvm.org"
grep -qF 'llvm-21-dev' <<< "$text" || add_problem "ci.yml: llvm-21-dev is not installed"
grep -qF 'libpolly-21-dev' <<< "$text" || add_problem "ci.yml: libpolly-21-dev is not installed"

for command in \
    'tests/check-tests.sh' \
    'tests/check-ci.sh' \
    'cargo fmt --all --check' \
    'cargo clippy --all-targets -- -D warnings' \
    'cargo test' \
    'cargo build'
do
    grep -qF -- "$command" <<< "$text" || add_problem "ci.yml: missing step: $command"
done

# The conformance step names milestone filters (m01), which match the folders (m01_hello).
conformance=$(grep -E 'tests/run-tests\.sh' <<< "$text" | tail -n 1)
if [ -z "$conformance" ]; then
    add_problem "ci.yml: no conformance step (tests/run-tests.sh)"
else
    for dir in "$SCRIPT_DIR"/m[0-9]*_*; do
        [ -d "$dir" ] || continue
        folder=$(basename "$dir")
        name=${folder%%_*}
        grep -qE "(^| )$name( |$)" <<< "$conformance" \
            || add_problem "ci.yml: conformance step omits $name ($folder)"
    done
    for word in $conformance; do
        case "$word" in
            m[0-9]*)
                found=0
                for dir in "$SCRIPT_DIR/$word"_*; do [ -d "$dir" ] && found=1; done
                [ "$found" = 1 ] || add_problem "ci.yml: conformance step names missing milestone $word"
                ;;
        esac
    done
fi

# The m11 test is the one that proves --link-lib LLVM-C resolves in CI.
if ! grep -rqs --include='*.bd' -- '//@ link-lib: LLVM-C' "$SCRIPT_DIR/m11_llvm"; then
    add_problem "tests/m11_llvm: no test uses //@ link-lib: LLVM-C"
fi

# Scripts CI invokes must exist, be executable, and have LF endings.
for rel in tests/check-tests.sh tests/run-tests.sh tests/check-ci.sh; do
    path=$ROOT/$rel
    if [ ! -f "$path" ]; then
        add_problem "$rel: missing"
        continue
    fi
    mode=$(git -C "$ROOT" ls-files -s "$rel" 2>/dev/null | awk '{print $1}')
    if [ -n "$mode" ]; then
        [ "$mode" = "100755" ] || add_problem "$rel: git mode $mode, expected 100755"
    elif [ ! -x "$path" ]; then
        add_problem "$rel: not executable"
    fi
    if grep -q $'\r' "$path"; then add_problem "$rel: CRLF line endings"; fi
done

echo "Checked .github/workflows/ci.yml against tests/"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
