#!/usr/bin/env bash
# Checks that the CI workflow still covers everything the project requires.
# It does not compile anything. Usage: tests/check-ci.sh
#
# This guards the invariants behind the first green CI run (issue #11) and the Windows job (issue #12):
#   * the linux job and the windows job are present, on the expected runners;
#   * the linux job installs LLVM 21 from apt.llvm.org and points LLVM_SYS_211_PREFIX at it;
#   * the windows job loads the MSVC environment, installs LLVM 21 from the official archive, and puts its
#     DLL directory on PATH;
#   * every required step is present in both jobs;
#   * every milestone folder under tests/ is passed to each conformance runner;
#   * the Windows job also runs the suite with BURDIN_CC=cl;
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

# The lines of one top-level job, without the `  name:` line itself.
job_block() {
    awk -v job="$1" '
        $0 == "  " job ":" { inside = 1; next }
        inside && /^  [^ ]/ { inside = 0 }
        inside { print }
    ' <<< "$text"
}

linux=$(job_block linux)
windows=$(job_block windows)
[ -n "$linux" ] || add_problem "ci.yml: no linux job"
[ -n "$windows" ] || add_problem "ci.yml: no windows job"

grep -qx '  push:' <<< "$text" || add_problem "ci.yml: does not run on push"
grep -qx '  pull_request:' <<< "$text" || add_problem "ci.yml: does not run on pull_request"

for command in \
    'tests/check-tests.sh' \
    'tests/check-ci.sh' \
    'cargo fmt --all --check' \
    'cargo clippy --all-targets -- -D warnings' \
    'cargo test' \
    'cargo build'
do
    grep -qF -- "$command" <<< "$linux" || add_problem "ci.yml: linux job is missing step: $command"
    grep -qF -- "$command" <<< "$windows" || add_problem "ci.yml: windows job is missing step: $command"
done

# The conformance step names milestone filters (m01), which match the folders (m01_hello).
check_conformance() {
    local block=$1 label=$2 conformance dir folder name word found
    # The BURDIN_CC step runs a subset; the full step is the one without an override.
    conformance=$(grep -E 'tests/run-tests\.sh' <<< "$block" | grep -v 'BURDIN_CC' | tail -n 1)
    if [ -z "$conformance" ]; then
        add_problem "ci.yml: $label job has no conformance step (tests/run-tests.sh)"
        return
    fi
    for dir in "$SCRIPT_DIR"/m[0-9]*_*; do
        [ -d "$dir" ] || continue
        folder=$(basename "$dir")
        name=${folder%%_*}
        grep -qE "(^| )$name( |$)" <<< "$conformance" \
            || add_problem "ci.yml: $label conformance step omits $name ($folder)"
    done
    for word in $conformance; do
        case "$word" in
            m[0-9]*)
                found=0
                for dir in "$SCRIPT_DIR/$word"_*; do [ -d "$dir" ] && found=1; done
                [ "$found" = 1 ] || add_problem "ci.yml: $label conformance step names missing milestone $word"
                ;;
        esac
    done
}

check_conformance "$linux" linux
check_conformance "$windows" windows

# --- the linux job ----------------------------------------------------------

grep -qx '    runs-on: ubuntu-24.04' <<< "$linux" || add_problem "ci.yml: linux does not run on ubuntu-24.04"
grep -qF 'LLVM_SYS_211_PREFIX: /usr/lib/llvm-21' <<< "$linux" \
    || add_problem "ci.yml: linux LLVM_SYS_211_PREFIX is not set"
grep -qF 'llvm.sh 21' <<< "$linux" || add_problem "ci.yml: LLVM 21 is not installed from apt.llvm.org"
grep -qF 'llvm-21-dev' <<< "$linux" || add_problem "ci.yml: llvm-21-dev is not installed"
grep -qF 'libpolly-21-dev' <<< "$linux" || add_problem "ci.yml: libpolly-21-dev is not installed"

# --- the windows job --------------------------------------------------------

grep -qx '    runs-on: windows-latest' <<< "$windows" || add_problem "ci.yml: windows does not run on windows-latest"
grep -qF 'ilammy/msvc-dev-cmd' <<< "$windows" \
    || add_problem "ci.yml: windows does not load the MSVC environment (cl would not be on PATH)"
grep -qF 'LLVM_SYS_211_PREFIX: C:/llvm' <<< "$windows" \
    || add_problem "ci.yml: windows LLVM_SYS_211_PREFIX is not set"
grep -qF 'llvmorg-21.1.8/clang+llvm-21.1.8-x86_64-pc-windows-msvc.tar.xz' <<< "$windows" \
    || add_problem "ci.yml: windows does not install LLVM 21 from the official archive"
grep -qF 'C:/llvm/lib/LLVM-C.lib' <<< "$windows" \
    || add_problem "ci.yml: windows never checks that the LLVM C API import library exists"
grep -qF 'C:\llvm\bin' <<< "$windows" \
    || add_problem "ci.yml: windows does not put LLVM's DLL directory on PATH"
grep -qF 'BURDIN_CC=cl' <<< "$windows" \
    || add_problem "ci.yml: windows does not exercise the BURDIN_CC=cl link path"

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
