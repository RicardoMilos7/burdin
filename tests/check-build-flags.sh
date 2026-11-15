#!/usr/bin/env bash
# Checks the build flags and binary metadata:
#   * `burdin --version` prints the version, channel, target, LLVM version, commit, and profile (issues #431, #454);
#   * `--strip` removes the symbol table and keeps the program's output, and the stripped sizes stay under the
#     size budget recorded here (issue #430);
#   * `--lto` links bitcode and runs, and `--pgo-generate`/`--pgo-use` round-trip a profile (issue #429), when an
#     LLVM compiler driver and the LLVM tools are installed. On MSVC those two options are rejected.
#
# Needs a built driver. BURDIN selects it, like tests/run-tests.sh.

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

find_tool() {   # $1 name
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

size_of() { wc -c < "$1" 2>/dev/null | tr -d ' '; }

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-flags.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

# --- --version (issue #431) --------------------------------------------------
checks=$((checks + 1))
version=$("$BURDIN" --version 2>err)
status=$?
if [ "$status" -ne 0 ]; then
    add_problem "burdin --version: exit status $status: $(cat err)"
else
    for prefix in "burdin " "channel: " "target: " "llvm: " "commit: " "profile: "; do
        checks=$((checks + 1))
        printf '%s\n' "$version" | grep -q "^$prefix" || add_problem "burdin --version: no '$prefix' line"
    done
    checks=$((checks + 1))
    printf '%s\n' "$version" | grep -Eq '^channel: (dev|nightly|stable)$' \
        || add_problem "burdin --version: the channel is not dev, nightly, or stable"
    checks=$((checks + 1))
    printf '%s\n' "$version" | grep -Eq '^llvm: [0-9]+\.[0-9]+\.[0-9]+$' \
        || add_problem "burdin --version: the LLVM version is not major.minor.patch"
    checks=$((checks + 1))
    printf '%s\n' "$version" | grep -Eq '^target: [A-Za-z0-9_]+-[A-Za-z0-9_.-]+$' \
        || add_problem "burdin --version: the target is not a triple"
fi

# --- Hello world, for --strip and the budgets (issue #430) -------------------
cat > hello.bd <<'EOF'
fn main() {
    println("hello, world")
}
EOF

cat > stdheavy.bd <<'EOF'
struct Item {
    name: String,
    count: i32,
}
fn main() {
    let mut items = List<Item>.new()
    for i in 0..20 {
        items.push(Item { name: String.from("item").clone(), count: i })
    }
    let mut map = Map<String, i32>.new()
    for item in items {
        map.insert(item.name.clone(), item.count)
    }
    println(items.len().to_string())
    println(map.len().to_string())
}
EOF

build_and_run() {   # $1 description, $2 source, $3 output, $4... flags
    checks=$((checks + 1))
    "$BURDIN" build "$2" -o "$3" "${@:4}" >/dev/null 2>err
    local status=$?
    if [ "$status" -ne 0 ]; then
        add_problem "$1: exit status $status: $(cat err)"
        return 1
    fi
    checks=$((checks + 1))
    ./"$3" > out 2>>err
    if [ $? -ne 0 ]; then
        add_problem "$1: the executable failed: $(cat err)"
        return 1
    fi
    return 0
}

if build_and_run "burdin build hello" hello.bd hello_plain; then
    plain_out=$(cat out)
else
    plain_out=""
fi

checks=$((checks + 1))
"$BURDIN" build hello.bd -o hello_strip --strip >/dev/null 2>err
if [ $? -ne 0 ]; then
    add_problem "burdin build --strip: $(cat err)"
else
    checks=$((checks + 1))
    strip_out=$(./hello_strip 2>err)
    [ "$strip_out" = "$plain_out" ] || add_problem "--strip changed the output: '$strip_out'"
    plain_size=$(size_of hello_plain)
    strip_size=$(size_of hello_strip)
    echo "hello: ${plain_size:-?} bytes, stripped: ${strip_size:-?} bytes"
    case "$(uname -s)" in
        Linux|Darwin)
            checks=$((checks + 1))
            [ -n "$strip_size" ] && [ "$strip_size" -le 131072 ] \
                || add_problem "stripped hello is ${strip_size:-?} bytes, over the 128 KiB budget"
            checks=$((checks + 1))
            leftover=$(nm hello_strip 2>/dev/null | grep "burdin\." | head -n 3 | tr '\n' ' ')
            [ -z "$leftover" ] || add_problem "--strip left Burdin symbols in the executable: $leftover"
            ;;
    esac
fi

if build_and_run "burdin build std-heavy" stdheavy.bd std_plain --strip; then
    std_size=$(size_of std_plain)
    echo "std-heavy (stripped): ${std_size:-?} bytes"
    case "$(uname -s)" in
        Linux|Darwin)
            checks=$((checks + 1))
            [ -n "$std_size" ] && [ "$std_size" -le 524288 ] \
                || add_problem "stripped std-heavy is ${std_size:-?} bytes, over the 512 KiB budget"
            ;;
    esac
fi

# --- LTO and PGO (issue #429) ------------------------------------------------
clang_tool=$(find_tool clang || true)
opt_tool=$(find_tool opt || true)
llc_tool=$(find_tool llc || true)
profdata_tool=$(find_tool llvm-profdata || true)

case "$(uname -s)" in
    Linux|Darwin)
        if [ -n "$clang_tool" ]; then
            checks=$((checks + 1))
            if BURDIN_CC="$clang_tool" "$BURDIN" build hello.bd --lto -o hello_lto >/dev/null 2>err; then
                checks=$((checks + 1))
                out=$(./hello_lto 2>>err)
                [ "$out" = "hello, world" ] || add_problem "--lto changed the output: '$out'"
            else
                add_problem "burdin build --lto: $(head -n 5 err)"
            fi
            if [ -n "$opt_tool" ] && [ -n "$llc_tool" ] && [ -n "$profdata_tool" ]; then
                checks=$((checks + 1))
                if BURDIN_CC="$clang_tool" "$BURDIN" build hello.bd --pgo-generate -o hello_pgo >/dev/null 2>err; then
                    rm -f default.profraw
                    checks=$((checks + 1))
                    if LLVM_PROFILE_FILE=default.profraw ./hello_pgo >/dev/null 2>>err; then
                        [ -s default.profraw ] || add_problem "--pgo-generate wrote no profile"
                    else
                        add_problem "the instrumented build failed: $(head -n 5 err)"
                    fi
                    checks=$((checks + 1))
                    if "$BURDIN" build hello.bd --pgo-use default.profraw -o hello_pgo_used >/dev/null 2>err; then
                        checks=$((checks + 1))
                        out=$(./hello_pgo_used 2>>err)
                        [ "$out" = "hello, world" ] || add_problem "--pgo-use changed the output: '$out'"
                    else
                        add_problem "burdin build --pgo-use: $(head -n 5 err)"
                    fi
                else
                    add_problem "burdin build --pgo-generate: $(head -n 5 err)"
                fi
            else
                echo "note: opt, llc, or llvm-profdata is missing; skipping the PGO round-trip"
            fi
        else
            echo "note: clang is missing; skipping the LTO and PGO checks"
        fi
        ;;
esac

# --- Rejected combinations and missing profiles ------------------------------
checks=$((checks + 1))
"$BURDIN" build hello.bd --lto --pgo-generate -o nope >/dev/null 2>err
[ $? -ne 0 ] && grep -q "cannot be combined" err \
    || add_problem "--lto with --pgo-generate was not rejected"
checks=$((checks + 1))
"$BURDIN" build hello.bd --pgo-use missing.profraw -o nope >/dev/null 2>err
[ $? -ne 0 ] && grep -q "does not exist" err \
    || add_problem "a missing PGO profile was not reported"

echo "Checked build flags and metadata ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
