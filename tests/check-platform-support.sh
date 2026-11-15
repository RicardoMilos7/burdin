#!/usr/bin/env bash
# Checks the platform support matrix and its known limitations (issue #457). Needs a built driver.
# Usage: tests/check-platform-support.sh
#
# BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin[.exe] is used.
#
# What it checks:
#   * docs/platform-support.md exists and is linked from README.md (the documentation index) and
#     CHANGELOG.md (the release notes);
#   * the support matrix names the three CI jobs and the runner image each one uses, keeps the untested
#     configurations marked untested, and names only tests and scripts that exist and run in CI;
#   * the row for the running host names the target triple `burdin --version` prints;
#   * the claims under "Known limitations" still match the code and the checks (the Windows DWARF/gdb
#     skip, the skipped backtrace-name test, the LTO/PGO platform split, the size budgets, the binary-mode
#     implementation, and the glibc backtrace and stack-guard calls);
#   * the checked example in the page compiles and prints the documented output;
#   * standard output and file I/O are binary-safe on this host: the redirected stdout and the file the
#     program writes contain exactly the bytes it asked for, with no CRLF translation.
#
# The script is written for the bash 3.2 that ships with macOS as well as newer bash.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
DOC=$ROOT/docs/platform-support.md
README=$ROOT/README.md
CHANGELOG=$ROOT/CHANGELOG.md
WORKFLOW=$ROOT/.github/workflows/ci.yml

problems=()
checks=0
add_problem() { problems+=("$1"); }
check() { checks=$((checks + 1)); }

if [ ! -f "$DOC" ]; then
    echo "missing $DOC" >&2
    exit 1
fi
doc=$(tr -d '\r' < "$DOC")
workflow=$(tr -d '\r' < "$WORKFLOW")

resolve_burdin() {
    if [ -n "${BURDIN:-}" ]; then return 0; fi
    local c
    for c in "$ROOT/target/debug/burdin" "$ROOT/target/debug/burdin.exe" \
             "$ROOT/target/release/burdin" "$ROOT/target/release/burdin.exe"; do
        if [ -x "$c" ]; then BURDIN=$c; return 0; fi
    done
    BURDIN=$(command -v burdin 2>/dev/null) && return 0
    return 1
}

if ! resolve_burdin; then
    echo "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin" >&2
    exit 2
fi
case "$BURDIN" in /*|[A-Za-z]:*) ;; *) BURDIN="$(pwd)/$BURDIN" ;; esac

# --- The page and its links --------------------------------------------------

check
grep -qF 'docs/platform-support.md' "$README" \
    || add_problem "README.md: does not link docs/platform-support.md"
check
grep -qF 'platform-support.md' "$CHANGELOG" \
    || add_problem "CHANGELOG.md: the release notes do not link docs/platform-support.md"

# --- The matrix agrees with CI -----------------------------------------------

check_tested_row() {   # $1 OS, $2 architecture, $3 CI job, $4 runner image
    local row
    check
    row=$(grep -E "^\| $1 \| $2 \| [^|]+ \| Tested \|" <<< "$doc" | head -n 1)
    if [ -z "$row" ]; then
        add_problem "docs/platform-support.md: no Tested row for $1 $2"
        return
    fi
    check
    case "$row" in
        *"\`$3\`"*"$4"*) ;;
        *) add_problem "docs/platform-support.md: the $1 $2 row does not name the '$3' job on $4" ;;
    esac
    check
    grep -qx "    runs-on: $4" <<< "$workflow" \
        || add_problem "ci.yml: no job runs on $4 (the matrix says the $3 job does)"
}

check_tested_row Linux x86-64 linux ubuntu-24.04
check_tested_row macOS arm64 macos macos-latest
check_tested_row Windows x86-64 windows windows-latest

for spec in "Linux|arm64" "macOS|x86-64" "Windows|arm64"; do
    unlisted_os=${spec%%|*}
    unlisted_arch=${spec#*|}
    check
    grep -qE "^\| $unlisted_os \| $unlisted_arch \| [^|]+ \| Untested \|" <<< "$doc" \
        || add_problem "docs/platform-support.md: no Untested row for $unlisted_os $unlisted_arch"
done
check
grep -qE '^\| other OS or 32-bit \|' <<< "$doc" \
    || add_problem "docs/platform-support.md: no Unsupported row for other hosts"

# The row for this host must carry the triple the driver prints.
host_os=$(uname -s)
host_machine=$(uname -m)
case "$host_os:$host_machine" in
    Linux:x86_64)     host_key="Linux|x86-64" ;;
    Darwin:arm64)     host_key="macOS|arm64" ;;
    Darwin:x86_64)    host_key="macOS|x86-64" ;;
    Linux:aarch64|Linux:arm64)
                      host_key="Linux|arm64" ;;
    MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64)
                      host_key="Windows|x86-64" ;;
    MINGW*:aarch64|MSYS*:aarch64|CYGWIN*:aarch64)
                      host_key="Windows|arm64" ;;
    *)                host_key="" ;;
esac
if [ -n "$host_key" ]; then
    host_name=${host_key%%|*}
    host_arch=${host_key#*|}
    target=$("$BURDIN" --version 2>/dev/null | tr -d '\r' | sed -n 's/^target: //p' | head -n 1)
    check
    if [ -z "$target" ]; then
        add_problem "burdin --version: no 'target:' line"
    else
        row=$(grep -E "^\| $host_name \| $host_arch \| [^|]+ \| (Tested|Untested) \|" <<< "$doc" | head -n 1)
        check
        case "$row" in
            *"$target"*) ;;
            *) add_problem "docs/platform-support.md: the $host_name $host_arch row does not name the host triple '$target'" ;;
        esac
    fi
fi

# Every test the page names must exist, and every script it names must be a CI step.
refs=$(grep -oE 'tests/[A-Za-z0-9_./-]+\.(sh|bd)' <<< "$doc" | sort -u)
for ref in $refs; do
    check
    if [ ! -e "$ROOT/$ref" ]; then
        add_problem "docs/platform-support.md: names a missing test: $ref"
        continue
    fi
    case "$ref" in
        *.sh)
            check
            grep -qF -- "$ref" <<< "$workflow" \
                || add_problem "ci.yml: does not run $ref (the platform page says CI checks it)"
            ;;
    esac
done

# Every conformance test the page names belongs to a milestone the workflow runs.
milestones=$(grep -oE 'tests/m[0-9]+_[A-Za-z0-9_]+/' <<< "$doc" | sed -E 's#tests/(m[0-9]+)_.*#\1#' | sort -u)
for milestone in $milestones; do
    check
    grep -qE "tests/run_tests\.py.*[ ]$milestone([ ]|$)" <<< "$workflow" \
        || add_problem "ci.yml: no conformance step runs $milestone (the platform page relies on it)"
done

# The known limitations are claims about the checks and the runtime, so tie each one to what makes it true.
check_claim() {   # $1 file, $2 text, $3 the claim it backs
    check
    grep -qF -- "$2" "$ROOT/$1" \
        || add_problem "$1: no longer contains '$2' (the platform page says: $3)"
}
check_claim tests/check-debug-info.sh 'the Windows linker kept no DWARF' \
    "Windows has no DWARF sections and the debug check skips them"
check_claim tests/m02_basics/panic_backtrace_names.bd '//@ skip-platform: windows' \
    "named backtrace frames are not checked on Windows"
check_claim tests/check-build-flags.sh 'On MSVC those two options are rejected' \
    "LTO and PGO generation are not exercised with MSVC"
check_claim tests/check-build-flags.sh 'Linux|Darwin' \
    "the size budgets are enforced only on Linux and macOS"
check_claim crates/burdin_codegen/src/runtime.rs '_setmode' \
    "the Windows standard handles are switched to binary mode"
check_claim crates/burdin_codegen/src/runtime.rs 'b"rb\0"' \
    "files are opened in binary mode"
check_claim crates/burdin_codegen/src/runtime.rs 'b"wb\0"' \
    "files are written in binary mode"
check_claim crates/burdin_codegen/src/runtime.rs 'pthread_getattr_np' \
    "the Linux stack guard uses glibc's pthread calls"
check_claim crates/burdin_codegen/src/runtime.rs 'backtrace_symbols_fd' \
    "symbolized backtraces use libc's backtrace calls"
check_claim tests/check-platform.sh 'aarch64' \
    "check-platform.sh recognizes Linux and Windows on arm64"

# --- The checked example and binary-safe I/O ---------------------------------

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-platform.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

expected=$(sed -n 's/^<!-- check-docs: stdout=\(.*\) -->$/\1/p' "$DOC")
if [ -z "$expected" ]; then
    add_problem "docs/platform-support.md: no 'check-docs' example with expected stdout"
else
    awk '
        /^<!-- check-docs: stdout=/ { inside = 1; next }
        /^<!-- \/check-docs -->$/ { inside = 0 }
        inside && /^```burdin$/ { code = 1; next }
        inside && /^```$/ { code = 0; next }
        inside && code { print }
    ' "$DOC" > example.bd
    check
    [ -s example.bd ] || add_problem "docs/platform-support.md: the checked example has no \`\`\`burdin block"

    "$BURDIN" run example.bd > example.out 2> example.err
    status=$?
    check
    [ "$status" -eq 0 ] \
        || add_problem "the platform page's example did not run: exit status $status: $(head -n 5 example.err)"
    check
    [ "$(cat example.out)" = "$expected" ] \
        || add_problem "the platform page's example printed '$(cat example.out)', expected '$expected'"
fi

# Binary mode: a redirected stdout and a written file must carry exactly the requested bytes. A text-mode
# CRT would turn each LF into CRLF, and a round-trip through the same API would hide it; the byte count
# does not. This is the check the Windows binary-mode row of the page refers to.
cat > binary_io.bd <<'EOF'
fn main() {
    match write_file("raw_bytes.bin", "a\nb") {
        Ok(_) => {},
        Err(_) => println("write failed"),
    }
    match read_to_string("raw_bytes.bin") {
        Ok(text) => println(text.len().to_string()),
        Err(_) => println("read failed"),
    }
    println("a")
    println("b")
}
EOF

"$BURDIN" run binary_io.bd > raw.out 2> raw.err
status=$?
check
[ "$status" -eq 0 ] \
    || add_problem "the binary I/O example did not run: exit status $status: $(head -n 5 raw.err)"
check
out_size=$(wc -c < raw.out | tr -d ' ')
[ "$out_size" = "6" ] \
    || add_problem "redirected stdout is $out_size bytes, expected 6: newline translation is on"
check
file_size=$(wc -c < raw_bytes.bin | tr -d ' ')
[ "$file_size" = "3" ] \
    || add_problem "the written file is $file_size bytes, expected 3: newline translation is on"
check
[ "$(cat raw.out)" = "$(printf '3\na\nb')" ] \
    || add_problem "raw stdout is '$(cat raw.out)', expected the lines 3, a, b"

echo "Checked the platform support page ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
