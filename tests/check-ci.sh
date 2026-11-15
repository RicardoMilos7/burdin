#!/usr/bin/env bash
# Checks that the CI workflow still covers everything the project requires.
# It does not compile anything. Usage: tests/check-ci.sh
#
# This guards the invariants behind the CI jobs (issues #11, #12, #16, #17, #18, #47):
#   * the linux, macos, and windows jobs, and every required step, are present;
#   * Linux installs LLVM 21 from apt.llvm.org; macOS installs Homebrew's llvm@21 and points
#     LLVM_SYS_211_PREFIX and PATH at it; Windows loads the MSVC environment, installs the official LLVM 21
#     archive, and puts its DLL directory on PATH;
#   * macOS runs on macos-latest (Apple Silicon) and Windows on windows-latest; every job checks the host target;
#   * every job passes every milestone folder under tests/ to the conformance runner in both the debug and
#     release profiles; the release step uses --release, and the runner skips tests marked debug-only
#     (issue #47);
#   * the Windows job also runs the suite with BURDIN_CC=cl;
#   * the scripts CI runs exist, are executable, and use LF endings;
#   * the Rust toolchain is pinned to an exact version with clippy and rustfmt,
#     and CI installs and verifies that pin (issue #19);
#   * both jobs cache the cargo registry and target/ with a key built from the pinned
#     toolchain and Cargo.lock, and cache LLVM 21: the installed tree on Linux, the
#     Homebrew bottle downloads on macOS (issue #46);
#   * the compiler benchmarks exist and CI runs their smoke budgets (issue #41);
#   * every job runs the workspace unit tests and the front-end unit-test guard (issue #45);
#   * every job runs the checks for debug information (#422), the code-generation optimizations (#426-#428),
#     the build flags and metadata (#429-#431), and parallel parsing (#432), and each script still tests what
#     its issue requires;
#   * every job runs the advisory benchmark regression check, keeps its baseline in a cache scoped to
#     the runner, the pinned toolchain, and Cargo.lock, and records a new baseline on a push to main
#     (issue #446);
#   * a conformance test still exercises --link-lib LLVM-C;
#   * the documentation check (which reads the driver contract in README.md) runs in both jobs.

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

profile_conformance() {   # $1 block, $2 debug|release
    local steps
    # The BURDIN_CC step runs a subset, so it is never the full-suite step for either profile.
    steps=$(grep -E 'tests/run-tests\.sh' <<< "$1" | grep -v 'BURDIN_CC')
    if [ "$2" = "release" ]; then
        grep -F -- '--release' <<< "$steps" | tail -n 1
    else
        grep -vF -- '--release' <<< "$steps" | tail -n 1
    fi
}

check_conformance() {   # $1 job, $2 block, $3 debug|release
    local job=$1 block=$2 profile=$3 conformance label folder name word dir found
    case "$profile" in
        release) label="release-profile" ;;
        *) label="debug-profile" ;;
    esac
    conformance=$(profile_conformance "$block" "$profile")
    if [ -z "$conformance" ]; then
        add_problem "ci.yml: $job job: no $label conformance step (tests/run-tests.sh)"
        return
    fi
    for dir in "$SCRIPT_DIR"/m[0-9]*_*; do
        [ -d "$dir" ] || continue
        folder=$(basename "$dir")
        name=${folder%%_*}
        grep -qE "(^| )$name( |$)" <<< "$conformance" \
            || add_problem "ci.yml: $job job: $label conformance step omits $name ($folder)"
    done
    for word in $conformance; do
        case "$word" in
            m[0-9]*)
                found=0
                for dir in "$SCRIPT_DIR/$word"_*; do [ -d "$dir" ] && found=1; done
                [ "$found" = 1 ] || add_problem "ci.yml: $job job: $label conformance step names missing milestone $word"
                ;;
        esac
    done
}

cache_steps() {   # $1 job block: only the steps that use actions/cache
    awk '
        function flush() { if (step ~ /actions\/cache@/) printf "%s", step }
        /^      - / { flush(); step = $0 "\n"; next }
        { step = step $0 "\n" }
        END { flush() }
    ' <<< "$1"
}

check_caches() {   # $1 job, $2 block
    local job=$1 block=$2 caches
    caches=$(cache_steps "$block")
    if [ -z "$caches" ]; then
        add_problem "ci.yml: $job job: no actions/cache step"
        return
    fi
    grep -qF 'uses: actions/cache@v4' <<< "$caches" \
        || add_problem "ci.yml: $job job: caches do not use actions/cache@v4"
    grep -qF '~/.cargo/registry' <<< "$caches" \
        || add_problem "ci.yml: $job job: cargo cache omits ~/.cargo/registry"
    grep -qE '^[[:space:]]+target$' <<< "$caches" \
        || add_problem "ci.yml: $job job: cargo cache omits target/"
    grep -qF "hashFiles('rust-toolchain.toml', 'Cargo.lock')" <<< "$caches" \
        || add_problem "ci.yml: $job job: cargo cache key is not built from rust-toolchain.toml and Cargo.lock"
    grep -qE 'key:.*llvm-21' <<< "$caches" \
        || add_problem "ci.yml: $job job: no LLVM 21 cache key"
    case "$job" in
        linux)
            grep -qF '~/.cache/burdin/llvm-21' <<< "$caches" \
                || add_problem "ci.yml: linux job: LLVM cache does not use a runner-writable path"
            grep -qF 'cache=~/.cache/burdin/llvm-21' <<< "$block" \
                || add_problem "ci.yml: linux job: install step does not use the cached LLVM path"
            grep -qF 'steps.cache-llvm.outputs.cache-hit' <<< "$block" \
                || add_problem "ci.yml: linux job: LLVM install ignores the cache hit"
            grep -qF 'readlink -f' <<< "$block" \
                || add_problem "ci.yml: linux job: LLVM cache does not follow libLLVM symlinks outside the tree"
            ;;
        macos)
            grep -qF '~/Library/Caches/Homebrew' <<< "$caches" \
                || add_problem "ci.yml: macos job: LLVM bottle downloads are not cached"
            ;;
    esac
}

# Benchmark regression tracking (issue #446). The baseline is restored from and saved to actions/cache with a
# key that names the runner, the pinned toolchain, and Cargo.lock; the comparison is advisory, and only a push
# to main records a new baseline.
check_bench_regression() {   # $1 job, $2 block
    local job=$1 block=$2
    grep -qF 'actions/cache/restore@v4' <<< "$block" \
        || add_problem "ci.yml: $job job: no step restores the benchmark baseline"
    grep -qF 'actions/cache/save@v4' <<< "$block" \
        || add_problem "ci.yml: $job job: no step saves the benchmark baseline"
    grep -qF 'bench-${{ runner.os }}-${{ runner.arch }}-' <<< "$block" \
        || add_problem "ci.yml: $job job: the benchmark baseline cache key does not name the runner"
    grep -qF "hashFiles('rust-toolchain.toml', 'Cargo.lock')" <<< "$block" \
        || add_problem "ci.yml: $job job: the benchmark baseline is not scoped to the toolchain and Cargo.lock"
    grep -qF 'tests/bench-regression.sh' <<< "$block" \
        || add_problem "ci.yml: $job job: does not run tests/bench-regression.sh"
    grep -qF 'continue-on-error: true' <<< "$block" \
        || add_problem "ci.yml: $job job: the benchmark regression step is not advisory"
    grep -qF 'BURDIN_BENCH_UPDATE' <<< "$block" \
        || add_problem "ci.yml: $job job: a main-branch push does not record the benchmark baseline"
    grep -qF "github.ref == 'refs/heads/main'" <<< "$block" \
        || add_problem "ci.yml: $job job: the benchmark baseline is saved outside main"
}

linux=$(job_block linux)
macos=$(job_block macos)
windows=$(job_block windows)
[ -n "$linux" ] || add_problem "ci.yml: no linux job"
[ -n "$macos" ] || add_problem "ci.yml: no macos job"
[ -n "$windows" ] || add_problem "ci.yml: no windows job"

grep -qx '    runs-on: ubuntu-24.04' <<< "$linux" || add_problem "ci.yml: linux does not run on ubuntu-24.04"
grep -qx '    runs-on: macos-latest' <<< "$macos" || add_problem "ci.yml: macos does not run on macos-latest"
grep -qx '    runs-on: windows-latest' <<< "$windows" || add_problem "ci.yml: windows does not run on windows-latest"
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

# Windows: the MSVC environment, the official LLVM 21 archive, and its DLL directory on PATH.
grep -qF 'ilammy/msvc-dev-cmd' <<< "$windows" \
    || add_problem "ci.yml: windows job: does not load the MSVC environment (cl would not be on PATH)"
grep -qF 'LLVM_SYS_211_PREFIX: C:/llvm' <<< "$windows" \
    || add_problem "ci.yml: windows job: LLVM_SYS_211_PREFIX is not set"
grep -qF 'llvmorg-21.1.8/clang+llvm-21.1.8-x86_64-pc-windows-msvc.tar.xz' <<< "$windows" \
    || add_problem "ci.yml: windows job: does not install LLVM 21 from the official archive"
grep -qF 'C:/llvm/lib/LLVM-C.lib' <<< "$windows" \
    || add_problem "ci.yml: windows job: never checks that the LLVM C API import library exists"
grep -qF 'C:\llvm\bin' <<< "$windows" \
    || add_problem "ci.yml: windows job: does not put LLVM's DLL directory on PATH"
grep -qF 'BURDIN_CC=cl' <<< "$windows" \
    || add_problem "ci.yml: windows job: does not exercise the BURDIN_CC=cl link path"
# Git for Windows ships a coreutils `link.exe`; rustc must be pointed at MSVC's linker explicitly.
grep -qF 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER' <<< "$windows" \
    || add_problem "ci.yml: windows job: does not pin the MSVC linker (Git's link.exe can shadow it)"

for job in linux macos windows; do
    case "$job" in
        linux) block=$linux ;;
        macos) block=$macos ;;
        windows) block=$windows ;;
    esac
    for command in \
        'tests/check-tests.sh' \
        'tests/check-unit-tests.sh' \
        'tests/check-ci.sh' \
        'tests/check-platform.sh' \
        'tests/check-docs.sh' \
        'tests/check-debug-info.sh' \
        'tests/check-codegen-opt.sh' \
        'tests/check-build-flags.sh' \
        'tests/check-parallel.sh' \
        'tests/bench-regression.sh' \
        'rustup toolchain install' \
        'rustc --version' \
        'cargo fmt --all --check' \
        'cargo clippy --all-targets -- -D warnings' \
        'cargo test --workspace' \
        'cargo build' \
        'cargo bench -p burdin_bench -- --smoke'
    do
        require_command "$job" "$block" "$command"
    done
    check_conformance "$job" "$block" debug
    check_conformance "$job" "$block" release
    check_caches "$job" "$block"
    check_bench_regression "$job" "$block"
done

# The release run (issue #47) is only meaningful when the runner selects the profile and skips
# debug-only tests, and the suite must contain one so that skip path stays exercised.
runner=$ROOT/tests/run-tests.sh
if [ -f "$runner" ]; then
    grep -qF -- '--release' "$runner" \
        || add_problem "tests/run-tests.sh: no --release option"
    grep -qF 'debug-only' "$runner" \
        || add_problem "tests/run-tests.sh: the release profile does not skip debug-only tests"
    grep -rqs --include='*.bd' '^//@ debug-only' "$SCRIPT_DIR" \
        || add_problem "tests/: no test is marked //@ debug-only, so the release profile never exercises the skip"
fi

# The compiler benchmarks (issue #41): CI runs the smoke budgets, so the bench target, the crate that generates
# its inputs, and the `--smoke` mode of the harness must all stay in place.
bench_manifest=$ROOT/crates/burdin_bench/Cargo.toml
bench_target=$ROOT/crates/burdin_bench/benches/compiler.rs
bench_lib=$ROOT/crates/burdin_bench/src/lib.rs
[ -f "$bench_manifest" ] || add_problem "crates/burdin_bench/Cargo.toml: missing"
[ -f "$bench_target" ] || add_problem "crates/burdin_bench/benches/compiler.rs: missing"
[ -f "$bench_lib" ] || add_problem "crates/burdin_bench/src/lib.rs: missing"
if [ -f "$bench_manifest" ] && ! grep -qF 'name = "compiler"' "$bench_manifest"; then
    add_problem "crates/burdin_bench/Cargo.toml: no 'compiler' benchmark target"
fi
if [ -f "$bench_target" ] && ! grep -qF '"--smoke"' "$bench_target"; then
    add_problem "crates/burdin_bench/benches/compiler.rs: no --smoke mode"
fi

# Benchmark regression tracking (issue #446): the harness must keep the modes the script uses to record and
# compare a baseline, and the script must keep running that comparison and updating the baseline.
bench_script=$ROOT/tests/bench-regression.sh
[ -f "$bench_script" ] || add_problem "tests/bench-regression.sh: missing"
if [ -f "$bench_target" ]; then
    for mode in '"--save"' '"--baseline"' '"--threshold"'; do
        grep -qF -- "$mode" "$bench_target" \
            || add_problem "crates/burdin_bench/benches/compiler.rs: no $mode mode"
    done
fi
if [ -f "$bench_script" ]; then
    for word in '--save' '--baseline' '--threshold' 'BURDIN_BENCH_UPDATE'; do
        grep -qF -- "$word" "$bench_script" \
            || add_problem "tests/bench-regression.sh: does not use $word"
    done
fi

# The new checks must keep testing what their issues require, so a rewritten script cannot silently weaken CI.
check_script() {   # $1 relative path, then the strings it must contain
    local rel=$1 path pattern
    path=$ROOT/$rel
    [ -f "$path" ] || return 0   # reported by the existence loop below
    shift
    for pattern in "$@"; do
        grep -qF -- "$pattern" "$path" || add_problem "$rel: no longer contains: $pattern"
    done
}
check_script tests/check-debug-info.sh "llvm-dwarfdump" "gdb" "-g" "DW_TAG_subprogram" "DW_TAG_variable"
check_script tests/check-codegen-opt.sh "alloca i1" "bounds.fail" "burdin.drop" "index out of bounds"
check_script tests/check-build-flags.sh "burdin --version" "--strip" "--lto" "--pgo-use" "131072"
check_script tests/check-parallel.sh "BURDIN_JOBS" "diff -u"

# The m11 test is the one that proves --link-lib LLVM-C resolves in CI.
if ! grep -rqs --include='*.bd' -- '//@ link-lib: LLVM-C' "$SCRIPT_DIR/m11_llvm"; then
    add_problem "tests/m11_llvm: no test uses //@ link-lib: LLVM-C"
fi

# The documentation check reads the driver contract from the root README (issue #42).
if [ ! -f "$ROOT/README.md" ]; then
    add_problem "README.md: missing (the driver contract lives there)"
elif ! grep -qF 'check-docs' "$ROOT/README.md"; then
    add_problem "README.md: does not mention tests/check-docs.sh"
fi

# Scripts CI invokes must exist, be executable, and have LF endings.
for rel in \
    tests/check-tests.sh tests/check-unit-tests.sh tests/run-tests.sh tests/check-ci.sh tests/check-platform.sh \
    tests/check-docs.sh tests/check-debug-info.sh tests/check-codegen-opt.sh tests/check-build-flags.sh \
    tests/check-parallel.sh tests/bench-regression.sh; do
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
