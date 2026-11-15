#!/usr/bin/env bash
# Checks source-level debug information (issue #422). Needs a built driver; uses llvm-dwarfdump to read the
# DWARF (from the executable on Linux, from the .dSYM bundle on macOS) and gdb, when it is installed, to prove a
# breakpoint on a Burdin line and a local read work.
#
# BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.
#
# What it checks:
#   * `burdin build -g` produces an executable whose output is unchanged;
#   * the DWARF has the source file and line table, a DW_TAG_subprogram per Burdin function, a
#     DW_TAG_formal_parameter for each parameter, and a DW_TAG_variable for each local, with declaration lines;
#   * with gdb installed, `break <file>:<line>` stops in the expected function and `print` reads the locals;
#   * the release profile with -g still carries the line table.
#
# The script is written for the bash 3.2 that ships with macOS as well as newer bash.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")

problems=()
checks=0
add_problem() { problems+=("$1"); }

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

find_tool() {   # $1 name: prints a usable tool, preferring the LLVM 21 spelling
    local name=$1 candidate
    for candidate in "${LLVM_SYS_211_PREFIX:+$LLVM_SYS_211_PREFIX/bin/}$name-21" "$name-21" \
                     "${LLVM_SYS_211_PREFIX:+$LLVM_SYS_211_PREFIX/bin/}$name" "$name"; do
        if command -v "$candidate" >/dev/null 2>&1; then
            command -v "$candidate"
            return 0
        fi
        if command -v "$candidate.exe" >/dev/null 2>&1; then
            command -v "$candidate.exe"
            return 0
        fi
    done
    return 1
}

windows_host=0
case "$(uname -s)" in MSYS*|MINGW*|CYGWIN*) windows_host=1 ;; esac

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-dbg.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT

# Lines 5-8 are `add`; line 7 is the tail expression, where the breakpoint goes.
cat > "$work/dbg.bd" <<'EOF'
struct Point {
    x: i32,
    y: i32,
}
fn add(a: i32, b: i32): i32 {
    let sum = a + b
    sum
}
fn main() {
    let p = Point { x: 1, y: 2 }
    let total = add(p.x, p.y)
    println(total.to_string())
}
EOF

cd "$work" || exit 1
stdout=$("$BURDIN" build dbg.bd -g -o dbg 2>err)
status=$?
checks=$((checks + 1))
[ "$status" -eq 0 ] || add_problem "burdin build -g: exit status $status: $(cat err)"
checks=$((checks + 1))
[ -x ./dbg ] || [ -x ./dbg.exe ] || add_problem "burdin build -g: no executable"
exe=./dbg
[ -f "$exe" ] || exe=./dbg.exe

checks=$((checks + 1))
out=$("$exe" 2>err)
[ "$out" = "3" ] || add_problem "-g changed the program's output: '$out', expected '3'"

dwarfdump=$(find_tool llvm-dwarfdump) || true
if [ -z "${dwarfdump:-}" ]; then
    echo "warning: llvm-dwarfdump not found; skipping the debug-info sections" >&2
else
    # ld64 keeps DWARF in a .dSYM bundle beside the executable; the driver produces it for `-g` on macOS.
    dwarf_target=$exe
    if [ -d "$exe.dSYM" ]; then
        dwarf_target="$exe.dSYM/Contents/Resources/DWARF/$(basename "$exe")"
    fi
    "$dwarfdump" --debug-info "$dwarf_target" > info.txt 2>/dev/null
    "$dwarfdump" --debug-line "$dwarf_target" > lines.txt 2>/dev/null
    if [ "$windows_host" -eq 1 ] && ! grep -Fq "DW_TAG_compile_unit" info.txt; then
        # MSVC's link.exe does not carry the DWARF sections into the image, and the driver does not produce a
        # PDB; the Linux and macOS jobs are where the DWARF sections are verified.
        echo "note: the Windows linker kept no DWARF sections; skipping the section checks" >&2
    else
        if ! grep -Fq "DW_TAG_compile_unit" info.txt; then
            # Show what the executable and the IR actually carry, so a platform that drops the sections is
            # diagnosable from the CI log without a second run.
            {
                echo "debug: the executable carries no compile unit; diagnosing"
                file "$exe" 2>&1
                if command -v otool >/dev/null 2>&1; then
                    otool -l "$exe" 2>&1 | grep -E 'segname|sectname' | head -n 30
                fi
                "$dwarfdump" --debug-info "$dwarf_target" 2>&1 | head -n 10
                if "$BURDIN" build dbg.bd -g --emit-ir diag.ll -o diag_exe >/dev/null 2>diag.err; then
                    echo "IR: $(grep -c '!DICompileUnit' diag.ll) DICompileUnit, $(grep -c '!DISubprogram' diag.ll) DISubprogram"
                    if llc_tool=$(find_tool llc); then
                        if "$llc_tool" -filetype=obj diag.ll -o diag.o 2>/dev/null; then
                            "$dwarfdump" --debug-info diag.o 2>&1 | grep -E 'DW_TAG_compile_unit|DW_TAG_subprogram|error' | head -n 6
                        fi
                    fi
                else
                    echo "IR build failed: $(head -n 5 diag.err)"
                fi
            } >&2
        fi
        expect_info() {   # $1 description, $2 pattern
            checks=$((checks + 1))
            grep -Fq -- "$2" info.txt || add_problem "DWARF: $1 (missing '$2')"
        }
        expect_info "no compile unit" "DW_TAG_compile_unit"
        expect_info "no source file" "dbg.bd"
        expect_info "the add subprogram" '"add"'
        expect_info "the main subprogram" '"main"'
        expect_info "the sum local" '"sum"'
        expect_info "the total local" '"total"'
        expect_info "a formal parameter" "DW_TAG_formal_parameter"
        expect_info "a local variable" "DW_TAG_variable"
        expect_info "a declaration line" "DW_AT_decl_line"
        checks=$((checks + 1))
        grep -Fq "dbg.bd" lines.txt || add_problem "DWARF: the line table never names dbg.bd"
    fi

    # An optimized build keeps the line table too.
    if "$BURDIN" build dbg.bd --release -g -o dbg_release >/dev/null 2>err; then
        release_target=./dbg_release
        if [ -d "$release_target.dSYM" ]; then
            release_target="$release_target.dSYM/Contents/Resources/DWARF/dbg_release"
        fi
        "$dwarfdump" --debug-line "$release_target" > lines_release.txt 2>/dev/null
        if [ "$windows_host" -eq 0 ]; then
            checks=$((checks + 1))
            grep -Fq "dbg.bd" lines_release.txt \
                || add_problem "DWARF: --release -g has no line table for dbg.bd"
        fi
        checks=$((checks + 1))
        out=$(./dbg_release 2>err)
        [ "$out" = "3" ] || add_problem "--release -g changed the output: '$out', expected '3'"
    else
        add_problem "burdin build --release -g: exit status $?: $(cat err)"
    fi
fi

gdb_tool=$(command -v gdb || true)
if [ "$windows_host" -eq 1 ] && [ -n "$gdb_tool" ]; then
    # MSVC's link.exe kept no DWARF, so gdb has nothing to read.
    echo "note: the Windows linker kept no DWARF sections; skipping the gdb checks" >&2
elif [ -n "$gdb_tool" ]; then
    "$gdb_tool" -q -batch -nx \
        -ex "set debuginfod enabled off" \
        -ex "break dbg.bd:7" \
        -ex run \
        -ex "print sum" \
        -ex "print a" \
        -ex "print b" \
        -ex continue \
        "$exe" > gdb.txt 2>&1
    checks=$((checks + 1))
    grep -Eq '^\$1 = 3$' gdb.txt || add_problem "gdb: 'print sum' at dbg.bd:7 did not print 3: $(tail -n 20 gdb.txt)"
    checks=$((checks + 1))
    grep -Eq '^\$2 = 1$' gdb.txt || add_problem "gdb: 'print a' did not print 1"
    checks=$((checks + 1))
    grep -Eq '^\$3 = 2$' gdb.txt || add_problem "gdb: 'print b' did not print 2"
    checks=$((checks + 1))
    grep -Eq 'Breakpoint 1, .*add' gdb.txt || add_problem "gdb: the breakpoint did not stop in add"
fi

echo "Checked debug information ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
