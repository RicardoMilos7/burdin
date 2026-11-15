#!/usr/bin/env bash
# Checks the code-generation optimizations and the behavior they preserve:
#   * drop-flag elimination (issue #427): a local proved moved at every drop site gets no `i1` flag, while a
#     conditionally moved local keeps one;
#   * bounds-check elision (issue #428): a loop over `0..xs.len()` and a constant index need no check, while an
#     out-of-range index still panics;
#   * glue deduplication (issue #426): two same-shaped types share one drop-glue function.
#
# The checks read the unoptimized `--emit-ir` text, so they do not depend on the optimization pipeline.
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

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-opt.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

expect_count() {   # $1 description, $2 file, $3 pattern, $4 expected
    checks=$((checks + 1))
    local count
    count=$(grep -c -- "$3" "$2" 2>/dev/null || true)
    [ "$count" -eq "$4" ] || add_problem "$1: $count occurrences of '$3', expected $4"
}

# --- Drop flags (issue #427) --------------------------------------------------
cat > flags.bd <<'EOF'
fn take(s: String): i32 {
    s.len() as i32
}
fn always(): i32 {
    let s = String.from("always")
    take(s)
}
fn maybe(c: bool): i32 {
    let s = String.from("maybe")
    if c { take(s) } else { 0 }
}
fn main() {
    println((always() + maybe(false)).to_string())
}
EOF
if "$BURDIN" build flags.bd --emit-ir flags.ll -o flags >/dev/null 2>err; then
    # One `moved` flag for maybe's conditional move; the unconditional move in `always` needs none.
    expect_count "drop-flag elimination" flags.ll "alloca i1" 1
    checks=$((checks + 1))
    out=$(./flags)
    [ "$out" = "6" ] || add_problem "drop-flag elimination changed the output: '$out', expected '6'"
else
    add_problem "burdin build flags.bd: $(cat err)"
fi

# --- Bounds-check elision (issue #428) ---------------------------------------
cat > bounds.bd <<'EOF'
fn sum(xs: [i32; 4]): i32 {
    let mut total = 0
    for i in 0..xs.len() {
        total = total + xs[i]
    }
    total + xs[1]
}
fn main() {
    println(sum([1, 2, 3, 4]).to_string())
}
EOF
if "$BURDIN" build bounds.bd --emit-ir bounds.ll -o bounds >/dev/null 2>err; then
    expect_count "bounds-check elision (loop and constant index)" bounds.ll "bounds.fail" 0
    checks=$((checks + 1))
    out=$(./bounds)
    [ "$out" = "12" ] || add_problem "bounds-check elision changed the output: '$out', expected '12'"
else
    add_problem "burdin build bounds.bd: $(cat err)"
fi

cat > oob.bd <<'EOF'
fn main() {
    let xs = [1, 2, 3]
    let i: usize = 5
    println(xs[i].to_string())
}
EOF
checks=$((checks + 1))
out=$("$BURDIN" run oob.bd 2>oob.err)
status=$?
if [ "$status" -eq 0 ]; then
    add_problem "out-of-range indexing did not panic"
elif ! grep -Fq "index out of bounds" oob.err; then
    add_problem "out-of-range indexing did not report a bounds panic: $(head -n 5 oob.err)"
fi

# --- Glue deduplication (issue #426) ----------------------------------------
cat > glue.bd <<'EOF'
struct A { s: String }
struct B { s: String }
fn main() {
    let a = Heap.new(A { s: String.from("aa") })
    let b = Heap.new(B { s: String.from("bb") })
    println((*a).s.len().to_string())
    println((*b).s.len().to_string())
}
EOF
if "$BURDIN" build glue.bd --emit-ir glue.ll -o glue >/dev/null 2>err; then
    # `Heap<A>` and `Heap<B>` have the same shape, so their drop glue is generated once.
    expect_count "glue deduplication" glue.ll "define internal void @burdin.drop" 1
    checks=$((checks + 1))
    out=$(./glue)
    [ "$out" = "2
2" ] || add_problem "glue deduplication changed the output: '$out'"
    checks=$((checks + 1))
    size=$(wc -c < glue 2>/dev/null | tr -d ' ')
    echo "glue fixture executable: ${size:-?} bytes, 1 shared drop-glue function for 2 types"
else
    add_problem "burdin build glue.bd: $(cat err)"
fi

echo "Checked code-generation optimizations ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
