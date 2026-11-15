#!/usr/bin/env bash
# Checks the documentation: relative links resolve, and the checked example and the documented driver
# contract still hold. Needs a built driver. Usage: tests/check-docs.sh
#
# BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.
#
# What it checks:
#   * every relative link in the repository's Markdown files points at a file, and every `#anchor`
#     matches a heading;
#   * the `burdin` program marked `check-docs` in README.md compiles and prints the documented output;
#   * `build` output naming, `-o`, `--emit-ir`, `--release`, `-g`, `--strip`, and `--link-lib` (including
#     LLVM-C when llvm-config is available), and `--version`'s six metadata lines;
#   * `run` passes arguments after `--` and the program's exit status through;
#   * `test` prints the documented `FAIL` lines and summary and exits 1 on a failure;
#   * diagnostics have the documented `path:LINE:COL: error[CODE]: message` shape;
#   * BURDIN_CC names the linker and its absence is reported.
#
# The script is written for the bash 3.2 that ships with macOS as well as newer bash.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
README=$ROOT/README.md

problems=()
checks=0
add_problem() { problems+=("$1"); }

# --- Links -------------------------------------------------------------------

# GitHub's heading anchor: lowercase, drop punctuation except spaces, hyphens, and underscores,
# then turn spaces into hyphens.
heading_slugs() {
    awk '/^#+[ \t]/' "$1" \
        | sed -E 's/^#+[ \t]+//; s/[[:space:]]+$//; s/\[([^]]*)\]\([^)]*\)/\1/g' \
        | tr '[:upper:]' '[:lower:]' \
        | sed -E 's/[^a-z0-9 _-]//g; s/ /-/g'
}

check_links() {
    local file dir line target path fragment resolved
    while IFS= read -r -d '' file; do
        dir=$(dirname "$file")
        # Skip fenced code blocks: their contents are examples, not links.
        while IFS= read -r line; do
            while IFS= read -r target; do
                target=${target#']('}
                target=${target%')'}
                [ -n "$target" ] || continue
                case $target in
                    http://*|https://*|mailto:*|tel:*) continue ;;
                esac
                case $target in
                    *'#'*) path=${target%%#*}; fragment=${target#*#} ;;
                    *) path=$target; fragment="" ;;
                esac
                path=${path//%20/ }
                if [ -z "$path" ]; then
                    resolved=$file
                else
                    case $path in
                        /*) resolved=$path ;;
                        *) resolved=$dir/$path ;;
                    esac
                fi
                checks=$((checks + 1))
                if [ ! -e "$resolved" ]; then
                    add_problem "${file#$ROOT/}: broken link: $target"
                    continue
                fi
                if [ -n "$fragment" ] && [ -f "$resolved" ]; then
                    if ! heading_slugs "$resolved" | grep -Fxq -- "$fragment"; then
                        add_problem "${file#$ROOT/}: no heading matches the anchor: $target"
                    fi
                elif [ -n "$fragment" ]; then
                    add_problem "${file#$ROOT/}: anchor on a directory: $target"
                fi
            done < <(grep -oE '\]\([^)]*\)' <<< "$line" || true)
        done < <(awk '/^[[:space:]]*(```|~~~)/ { fence = !fence; next } !fence { print }' "$file")
    done < <(find "$ROOT" -name '*.md' -not -path '*/.git/*' -not -path '*/target/*' -print0)
}

check_links

# --- Driver ------------------------------------------------------------------

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

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-docs.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT

status=0
stdout=""
stderr=""

run_driver() {   # stdout and stderr go to $stdout and $stderr, the status to $status
    "$@" >"$work/stdout" 2>"$work/stderr"
    status=$?
    stdout=$(cat "$work/stdout")
    stderr=$(cat "$work/stderr")
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

expect_file() {   # $1 description, $2 path
    checks=$((checks + 1))
    [ -f "$2" ] || add_problem "$1: expected file $2"
}

# The checked program in README.md: everything between the check-docs markers.
expected=$(sed -n 's/^<!-- check-docs: stdout=\(.*\) -->$/\1/p' "$README")
if [ -z "$expected" ]; then
    add_problem "README.md: no 'check-docs' example with expected stdout"
else
    cd "$work" || exit 1
    awk '
        /^<!-- check-docs: stdout=/ { inside = 1; next }
        /^<!-- \/check-docs -->$/ { inside = 0 }
        inside && /^```burdin$/ { code = 1; next }
        inside && /^```$/ { code = 0; next }
        inside && code { print }
    ' "$README" > hello.bd
    if [ ! -s hello.bd ]; then
        add_problem "README.md: the check-docs example has no \`\`\`burdin block"
    else
        run_driver "$BURDIN" run hello.bd
        expect_status "README example (burdin run)" 0
        checks=$((checks + 1))
        [ "$stdout" = "$expected" ] \
            || add_problem "README example: printed '$stdout', expected '$expected'"

        # build: default output name, -o, --emit-ir
        run_driver "$BURDIN" build hello.bd
        expect_status "burdin build" 0
        exe=hello
        [ -f "$exe" ] || exe=hello.exe
        expect_file "burdin build (default output)" "$exe"

        run_driver "$BURDIN" build hello.bd -o custom_name
        expect_status "burdin build -o" 0
        custom=custom_name
        [ -f "$custom" ] || custom=custom_name.exe
        expect_file "burdin build -o" "$custom"

        run_driver "$BURDIN" build hello.bd --emit-ir hello.ll
        expect_status "burdin build --emit-ir" 0
        expect_file "burdin build --emit-ir" hello.ll
        expect_contains "burdin build --emit-ir" "define" "$(cat hello.ll 2>/dev/null)"

        # --version: the build metadata the README documents.
        run_driver "$BURDIN" --version
        expect_status "burdin --version" 0
        for prefix in "burdin " "channel: " "target: " "llvm: " "commit: " "profile: "; do
            expect_contains "burdin --version" "$prefix" "$stdout"
        done

        # -g and --strip: the documented build flags keep the program's behavior.
        run_driver "$BURDIN" build hello.bd -g -o hello_debug
        expect_status "burdin build -g" 0
        debug_exe=hello_debug
        [ -f "$debug_exe" ] || debug_exe=hello_debug.exe
        run_driver "./$debug_exe"
        expect_status "burdin build -g executable" 0
        checks=$((checks + 1))
        [ "$stdout" = "$expected" ] || add_problem "burdin build -g executable: wrong output"

        run_driver "$BURDIN" build hello.bd --strip -o hello_stripped
        expect_status "burdin build --strip" 0
        stripped_exe=hello_stripped
        [ -f "$stripped_exe" ] || stripped_exe=hello_stripped.exe
        run_driver "./$stripped_exe"
        expect_status "burdin build --strip executable" 0
        checks=$((checks + 1))
        [ "$stdout" = "$expected" ] || add_problem "burdin build --strip executable: wrong output"

        run_driver "$BURDIN" build hello.bd --release -o hello_release
        expect_status "burdin build --release" 0
        release=hello_release
        [ -f "$release" ] || release=hello_release.exe
        run_driver "./$release"
        expect_status "burdin build --release executable" 0
        checks=$((checks + 1))
        [ "$stdout" = "$expected" ] || add_problem "burdin build --release executable: wrong output"

        # run: exit status passthrough and arguments after --
        printf 'fn main() { process.exit(3) }\n' > exit3.bd
        run_driver "$BURDIN" run exit3.bd
        expect_status "burdin run (exit status)" 3

        printf 'fn main() { println(env.args().len().to_string()) }\n' > args.bd
        run_driver "$BURDIN" run args.bd -- x y
        expect_status "burdin run -- args" 0
        checks=$((checks + 1))
        [ "$stdout" = "3" ] || add_problem "burdin run -- args: printed '$stdout', expected '3'"

        # --link-lib: a system library, and LLVM-C through llvm-config on Unix.
        case "$(uname -s)" in
            Linux|Darwin)
                run_driver "$BURDIN" build hello.bd --link-lib m -o hello_m
                expect_status "burdin build --link-lib m" 0
                if command -v llvm-config >/dev/null 2>&1 \
                    || { [ -n "${LLVM_SYS_211_PREFIX:-}" ] && [ -x "$LLVM_SYS_211_PREFIX/bin/llvm-config" ]; }; then
                    run_driver "$BURDIN" build hello.bd --link-lib LLVM-C -o hello_llvm
                    expect_status "burdin build --link-lib LLVM-C" 0
                fi
                ;;
        esac

        # BURDIN_CC is the linker; a missing one is reported with the variable's name.
        run_driver env BURDIN_CC=burdin-cc-that-does-not-exist "$BURDIN" build hello.bd -o ignored
        expect_status "BURDIN_CC (missing linker)" 2
        expect_contains "BURDIN_CC (missing linker)" "BURDIN_CC" "$stderr"

        # Diagnostics: path:LINE:COL: error[CODE]: message, then the summary.
        printf 'fn main() {\n    let x: i32 = "hi"\n}\n' > bad.bd
        run_driver "$BURDIN" build bad.bd
        expect_status "diagnostics" 1
        checks=$((checks + 1))
        case $stderr in
            bad.bd:2:*) ;;
            *) add_problem "diagnostics: no 'bad.bd:2:...' line: $stderr" ;;
        esac
        expect_contains "diagnostics" "error[E0003]" "$stderr"
        expect_contains "diagnostics" "error: could not compile bad.bd (1 error)" "$stderr"

        # test: the FAIL lines and the summary, with a failing test exiting 1.
        # The directory is `app` while the package is `demo`, so the default output `./demo` cannot
        # collide with the package directory.
        mkdir -p app/src
        printf '[package]\nname = "demo"\n' > app/burdin.toml
        cat > app/src/main.bd <<'EOF'
fn main() {}

test "first passes" {
    assert_eq(1 + 1, 2)?
}

test "second one fails" {
    assert_eq(1 + 1, 3)?
}

test "third passes" {
    assert(true)?
}
EOF
        run_driver "$BURDIN" test app
        expect_status "burdin test" 1
        expect_contains "burdin test" "FAIL second one fails" "$stdout"
        expect_contains "burdin test" "test result: 2 passed, 1 failed" "$stdout"

        # Tests are stripped from a normal build: an unknown name inside a test cannot fail it.
        cat > app/src/main.bd <<'EOF'
fn main() {}

test "not checked by a build" {
    let x = no_such_function()
}
EOF
        run_driver "$BURDIN" build app
        expect_status "burdin build (tests stripped)" 0
        demo=demo
        [ -f "$demo" ] || demo=demo.exe
        expect_file "burdin build (package output name)" "$demo"
    fi
fi

echo "Checked $checks links and driver behaviors"
if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
