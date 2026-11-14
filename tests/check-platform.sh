#!/usr/bin/env bash
# Checks that the compiler targets the machine it runs on (issue #17). Usage: tests/check-platform.sh
#
# It drives the built `burdin` binary, so build the compiler first. It proves three things:
#   * the emitted LLVM module names the host triple (arch and OS), not a hardcoded one;
#   * `usize` and `isize` carry 64-bit values, on every supported host;
#   * the linked executable runs on the host (the object and the `cc` linker agree on the target).
# The conformance suite itself is the check that no test assumes x86-64: it runs on the macOS arm64 job.
#
# Environment:
#   BURDIN   path to the compiler driver (default: target/{debug,release}/burdin[.exe], then PATH)

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO_DIR=$(dirname "$SCRIPT_DIR")

problems=()
add_problem() { problems+=("$1"); }

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

if ! resolve_burdin; then
    echo "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin" >&2
    exit 2
fi

host_os=$(uname -s)
host_machine=$(uname -m)
case "$host_os:$host_machine" in
    Darwin:arm64)     expected_arch=arm64;  expected_os='apple-darwin' ;;
    Darwin:x86_64)    expected_arch=x86_64; expected_os='apple-darwin' ;;
    Linux:x86_64)     expected_arch=x86_64; expected_os='linux' ;;
    Linux:aarch64|Linux:arm64)
                      expected_arch=aarch64; expected_os='linux' ;;
    *)
        echo "SKIP  unsupported host $host_os/$host_machine (expected macOS arm64/x86_64 or Linux x86_64/aarch64)"
        exit 0
        ;;
esac

case "$host_os" in
    MINGW*|MSYS*|CYGWIN*) exe_name=host.exe ;;
    *) exe_name=host ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cat > "$tmp/host.bd" <<'EOF'
fn main() {
    let wide: usize = 18446744073709551615
    println(wide.to_string())
    let signed: isize = 9223372036854775807
    println(signed.to_string())
}
EOF

if ! "$BURDIN" build "$tmp/host.bd" --emit-ir "$tmp/host.ll" -o "$tmp/$exe_name" > "$tmp/build.out" 2>&1; then
    echo "error: the compiler could not build for $host_os/$host_machine" >&2
    cat "$tmp/build.out" >&2
    exit 1
fi

# The module must name the host triple, and the triple must agree with the C linker that links it.
triple=$(sed -n 's/^target triple = "\([^"]*\)".*/\1/p' "$tmp/host.ll" | head -n 1)
if [ -z "$triple" ]; then
    add_problem "the emitted IR has no target triple"
else
    case "$triple" in
        "$expected_arch"-*) ;;
        *) add_problem "IR triple '$triple' does not start with the host arch '$expected_arch'" ;;
    esac
    case "$triple" in
        *"$expected_os"*) ;;
        *apple-darwin*|*apple-macos*)
            [ "$expected_os" = 'apple-darwin' ] || add_problem "IR triple '$triple' is not a $expected_os target" ;;
        *) add_problem "IR triple '$triple' is not a $expected_os target" ;;
    esac
fi

linker_target=$(cc --print-target-triple 2>/dev/null || cc -dumpmachine 2>/dev/null)
if [ -n "$linker_target" ]; then
    case "$linker_target" in
        "$expected_arch"-*) ;;
        *) add_problem "the linker cc targets '$linker_target', not the host arch '$expected_arch'" ;;
    esac
fi

# `usize` and `isize` are 64-bit on every supported platform. A narrower type either rejects the
# literal (E0008) or prints a truncated value, so this checks the width observably.
want=$'18446744073709551615\n9223372036854775807'
if ! got=$("$tmp/$exe_name" 2>&1); then
    add_problem "the built program did not run on the host"
elif [ "$got" != "$want" ]; then
    add_problem "usize/isize are not 64-bit: expected '$want', got '$got'"
fi

echo "Checked host $host_os/$host_machine (triple $triple)."

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
