#!/usr/bin/env bash
# Checks that the CI workflow still covers everything the project requires.
# It does not compile anything. Usage: tests/check-ci.sh
#
# This guards the invariants behind the CI jobs (issues #11, #16, #17, #18):
#   * the linux and macos jobs, and every required step, are present;
#   * Linux installs LLVM 21 from apt.llvm.org; macOS installs Homebrew's llvm@21 and points
#     LLVM_SYS_211_PREFIX and PATH at it, which is what `--link-lib LLVM-C` resolves through;
#   * macOS runs on macos-latest (Apple Silicon) and checks the host target;
#   * every job passes every milestone folder under tests/ to the conformance runner;
#   * the scripts CI runs exist, are executable, and use LF endings;
#   * the Rust toolchain is pinned to an exact version with clippy and rustfmt,
#     and CI installs and verifies that pin (issue #19);
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

# The steps of one job: from `  <name>:` to the next job at the same indentation.
job_block() {
    awk -v job="$1:" '
        $0 == "  " job { inside = 1; next }
        inside && /^  [A-Za-z0-9_-]+:$/ { exit }
        inside { print }
    ' <<< "$text"
}

# The Rust toolchain is pinned so fmt and clippy are reproducible (issue #19).
toolchain_file=$ROOT/rust-toolchain.toml
if [ ! -f "$toolchain_file" ]; then
    add_problem "rust-toolchain.toml: missing (the Rust toolchain is not pinned)"
else
    toolchain=$(tr -d '\r' < "$toolchain_file")
    channel=$(sed -n 's/^channel = "\([0-9][0-9.]*\)"$/\1/p' <<< "$toolchain")
    if [ -z "$channel" ]; then
        add_problem "rust-toolchain.toml: no channel (an exact x.y.z version is required, not stable/nightly)"
    elif ! grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' <<< "$channel"; then
        add_problem "rust-toolchain.toml: channel '$channel' is not an x.y.z version"
    fi
    for component in clippy rustfmt; do
        grep -qF "\"$component\"" <<< "$toolchain" \
            || add_problem "rust-toolchain.toml: components do not include $component"
    done
fi

if grep -qE 'rustup (toolchain install|default)[^#]*stable' <<< "$text"; then
    add_problem "ci.yml: installs a floating 'stable' toolchain instead of the pinned one"
fi
grep -qF 'rust-toolchain.toml' "$ROOT/CLAUDE.md" \
    || add_problem "CLAUDE.md: does not document the rust-toolchain.toml pin"

require_command() {   # $1 job, $2 block, $3 command
    grep -qF -- "$3" <<< "$2" || add_problem "ci.yml: $1 job: missing step: $3"
}

check_conformance() {   # $1 job, $2 block
    local job=$1 block=$2 conformance folder name word dir found
    conformance=$(grep -E 'tests/run-tests\.sh' <<< "$block" | tail -n 1)
    if [ -z "$conformance" ]; then
        add_problem "ci.yml: $job job: no conformance step (tests/run-tests.sh)"
        return
    fi
    for dir in "$SCRIPT_DIR"/m[0-9]*_*; do
        [ -d "$dir" ] || continue
        folder=$(basename "$dir")
        name=${folder%%_*}
        grep -qE "(^| )$name( |$)" <<< "$conformance" \
            || add_problem "ci.yml: $job job: conformance step omits $name ($folder)"
    done
    for word in $conformance; do
        case "$word" in
            m[0-9]*)
                found=0
                for dir in "$SCRIPT_DIR/$word"_*; do [ -d "$dir" ] && found=1; done
                [ "$found" = 1 ] || add_problem "ci.yml: $job job: conformance step names missing milestone $word"
                ;;
        esac
    done
}

linux=$(job_block linux)
macos=$(job_block macos)
[ -n "$linux" ] || add_problem "ci.yml: no linux job"
[ -n "$macos" ] || add_problem "ci.yml: no macos job"

grep -qx '    runs-on: ubuntu-24.04' <<< "$linux" || add_problem "ci.yml: linux does not run on ubuntu-24.04"
grep -qx '    runs-on: macos-latest' <<< "$macos" || add_problem "ci.yml: macos does not run on macos-latest"
grep -qx '  push:' <<< "$text" || add_problem "ci.yml: does not run on push"
grep -qx '  pull_request:' <<< "$text" || add_problem "ci.yml: does not run on pull_request"

# Linux: LLVM 21 from apt.llvm.org.
grep -qF 'LLVM_SYS_211_PREFIX: /usr/lib/llvm-21' <<< "$linux" \
    || add_problem "ci.yml: linux job: LLVM_SYS_211_PREFIX is not set"
grep -qF 'llvm.sh 21' <<< "$linux" || add_problem "ci.yml: linux job: LLVM 21 is not installed from apt.llvm.org"
grep -qF 'llvm-21-dev' <<< "$linux" || add_problem "ci.yml: linux job: llvm-21-dev is not installed"
grep -qF 'libpolly-21-dev' <<< "$linux" || add_problem "ci.yml: linux job: libpolly-21-dev is not installed"

# macOS: Homebrew's keg-only LLVM 21, exported for inkwell and for the driver's llvm-config lookup.
grep -qF 'brew install llvm@21' <<< "$macos" || add_problem "ci.yml: macos job: llvm@21 is not installed"
grep -qF 'LLVM_SYS_211_PREFIX=$(brew --prefix llvm@21)' <<< "$macos" \
    || add_problem "ci.yml: macos job: LLVM_SYS_211_PREFIX is not set from Homebrew"
grep -qF '$(brew --prefix llvm@21)/bin' <<< "$macos" \
    || add_problem "ci.yml: macos job: llvm-config is not added to PATH"

for job in linux macos; do
    case "$job" in
        linux) block=$linux ;;
        macos) block=$macos ;;
    esac
    for command in \
        'tests/check-tests.sh' \
        'tests/check-ci.sh' \
        'tests/check-platform.sh' \
        'rustup toolchain install' \
        'rustc --version' \
        'cargo fmt --all --check' \
        'cargo clippy --all-targets -- -D warnings' \
        'cargo test' \
        'cargo build'
    do
        require_command "$job" "$block" "$command"
    done
    check_conformance "$job" "$block"
done

# The m11 test is the one that proves --link-lib LLVM-C resolves in CI.
if ! grep -rqs --include='*.bd' -- '//@ link-lib: LLVM-C' "$SCRIPT_DIR/m11_llvm"; then
    add_problem "tests/m11_llvm: no test uses //@ link-lib: LLVM-C"
fi

# Scripts CI invokes must exist, be executable, and have LF endings.
for rel in tests/check-tests.sh tests/run-tests.sh tests/check-ci.sh tests/check-platform.sh; do
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
