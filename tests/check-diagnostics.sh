#!/usr/bin/env bash
# Checks diagnostics presentation and tooling (issues #434, #435, #436). Needs a built driver.
# Usage: tests/check-diagnostics.sh
#
# BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.
#
# What it checks:
#   * default output is plain when stderr is not a terminal, and --color=always/--no-color/NO_COLOR behave as
#     documented (an explicit --color always wins over NO_COLOR; the last color flag wins);
#   * --width (and COLUMNS) wraps the human format, with no line over the width, and --width 0 does not wrap;
#   * --diagnostic-format=short is one plain line per diagnostic, and --diagnostic-format=json is one JSON
#     object per diagnostic plus a summary object;
#   * a diagnostic's note and suggestion reach the human output (`= note:`, `= help:`) and the JSON output;
#   * --error-limit prints the first N errors after sorting and reports how many were hidden;
#   * the output is byte-for-byte deterministic between runs;
#   * the flags parse as `--option value` and `--option=value`, and bad values exit 2 with a usage error.
#
# The sorting, wrapping, color decision, and JSON shape are also unit-tested in `burdin_diag` and
# `burdin_driver` (`cargo test --workspace`).

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")

problems=()
checks=0
add_problem() { problems+=("$1"); }
check() { checks=$((checks + 1)); }

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

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-diagnostics.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT

cat > "$work/two_errors.bd" <<'EOF'
fn main() {
    let a: i32 = "x"
    let b: i32 = "y"
}
EOF

cat > "$work/long_message.bd" <<'EOF'
fn main() {
    this_identifier_is_far_too_long_to_fit_on_one_terminal_line(1)
}
EOF

cat > "$work/suggestion.bd" <<'EOF'
struct Counter {
    n: i32
}

fn bump(mut c: Counter) {
    c.n = c.n + 1
}

fn main() {
    let mut c = Counter { n: 0 }
    bump(c)
}
EOF

cat > "$work/note.bd" <<'EOF'
struct Thing {
    id: i32
}

fn take(own t: Thing) {}

fn main() {
    let t = Thing { id: 1 }
    take(t)
    let u = t
}
EOF

cat > "$work/run_bad.bd" <<'EOF'
fn main() {
    let n: i32 = "x"
}
EOF

status=0
stdout=""
stderr=""

run_driver() {   # $1 fixture, the rest are driver options
    local fixture=$1
    shift
    ( cd "$work" && "$BURDIN" build "$fixture" "$@" ) > "$work/stdout" 2> "$work/stderr"
    status=$?
    stdout=$(cat "$work/stdout")
    stderr=$(cat "$work/stderr")
}

expect_status() {   # $1 description, $2 expected
    check
    [ "$status" -eq "$2" ] || add_problem "$1: exit status $status, expected $2"
}

expect_contains() {   # $1 description, $2 text, $3 haystack
    check
    case $3 in
        *"$2"*) ;;
        *) add_problem "$1: output does not contain: $2" ;;
    esac
}

diagnostic_lines() {   # the primary diagnostic lines in the last stderr
    sed -n '1,/^error: could not compile/p' "$work/stderr" | sed '$d'
}

expect_no_color() {   # $1 description
    check
    if grep -q $'\x1b\[' "$work/stderr"; then
        add_problem "$1: output contains ANSI color"
    fi
}

expect_color() {   # $1 description
    check
    if ! grep -q $'\x1b\[' "$work/stderr"; then
        add_problem "$1: output has no ANSI color"
    fi
}

# --- Default output and color (issue #434) -----------------------------------

run_driver two_errors.bd
expect_status "default output" 1
expect_no_color "piped stderr"
check
count=$(grep -cE ':[0-9]+:[0-9]+: error\[E[0-9]{4}\]' "$work/stderr")
[ "$count" -eq 2 ] || add_problem "default output: $count diagnostic lines, expected 2"
expect_contains "default output" "error: could not compile two_errors.bd (2 errors)" "$stderr"
expect_contains "default output" 'burdin explain' "$stderr"

run_driver two_errors.bd --color=always
expect_status "--color=always" 1
expect_color "--color=always"

run_driver two_errors.bd --color never
expect_status "--color never" 1
expect_no_color "--color never"

run_driver two_errors.bd --no-color
expect_status "--no-color" 1
expect_no_color "--no-color"

run_driver two_errors.bd --color=always --no-color
expect_no_color "the last color flag wins (no-color last)"

run_driver two_errors.bd --no-color --color=always
expect_color "the last color flag wins (always last)"

( cd "$work" && NO_COLOR=1 "$BURDIN" build two_errors.bd --color=always ) > "$work/stdout" 2> "$work/stderr"
status=$?
expect_color "NO_COLOR yields to an explicit --color=always"

( cd "$work" && NO_COLOR=1 "$BURDIN" build two_errors.bd ) > "$work/stdout" 2> "$work/stderr"
expect_no_color "NO_COLOR with the default color choice"

# --- Wrapping (issue #434) ---------------------------------------------------

run_driver long_message.bd --width 60
expect_status "--width" 1
check
if awk 'length > 60 { exit 1 }' <<< "$(diagnostic_lines)"; then
    :
else
    add_problem "--width 60: a diagnostic line is wider than 60 characters"
fi
check
grep -qE '^    ' "$work/stderr" || add_problem "--width 60: no continuation line is indented"
check
# Wrapping only breaks at whitespace or inside a word; removing the newlines and indentation reconstructs the
# message, including the long identifier.
joined=$(diagnostic_lines | sed -E 's/^    //' | tr -d '\n')
case $joined in
    *this_identifier_is_far_too_long_to_fit_on_one_terminal_line*) ;;
    *) add_problem "--width 60: the wrapped message does not contain the identifier" ;;
esac

run_driver long_message.bd --width 0
check
[ "$(diagnostic_lines | wc -l)" -eq 1 ] || add_problem "--width 0: the message was wrapped"

( cd "$work" && COLUMNS=60 "$BURDIN" build long_message.bd ) > "$work/stdout" 2> "$work/stderr"
check
if awk 'length > 60 { exit 1 }' <<< "$(diagnostic_lines)"; then
    :
else
    add_problem "COLUMNS=60: a diagnostic line is wider than 60 characters"
fi

# --- Short and JSON formats (issues #434, #435) ------------------------------

run_driver suggestion.bd --diagnostic-format=short
expect_status "--diagnostic-format=short" 1
expect_no_color "short format ignores color"
check
[ "$(grep -cE ':[0-9]+:[0-9]+: error\[E[0-9]{4}\]' "$work/stderr")" -eq 1 ] \
    || add_problem "short format: not one diagnostic line"
check
if grep -qF '= help:' "$work/stderr"; then
    add_problem "short format: contains a help line"
fi
check
if grep -E 'error\[' "$work/stderr" | grep -vE '^[^:]+:[0-9]+:[0-9]+: error\[E[0-9]{4}\]: ' | grep -q .; then
    add_problem "short format: a diagnostic line does not match path:LINE:COL: error[CODE]: message"
fi

run_driver two_errors.bd --diagnostic-format=json
expect_status "--diagnostic-format=json" 1
check
if grep -vE '^\{.*\}$' "$work/stderr" | grep -q .; then
    add_problem "json format: a line is not a JSON object"
fi
check
[ "$(grep -c '"type":"diagnostic"' "$work/stderr")" -eq 2 ] || add_problem "json format: expected two diagnostics"
expect_contains "json format" '"code":"E0003"' "$stderr"
expect_contains "json format" '"span":{"file":"two_errors.bd","line":2,"column":18' "$stderr"
expect_contains "json format" '"type":"summary","errors":2,"shown":2,"limit":null' "$stderr"

run_driver two_errors.bd --diagnostic-format=json --color=always
expect_no_color "json format ignores color"

run_driver suggestion.bd --diagnostic-format=json
expect_status "json suggestion" 1
expect_contains "json suggestion" '"suggestions":[{"message":"add `mut` before this argument"' "$stderr"
expect_contains "json suggestion" '"replacement":"mut "' "$stderr"

run_driver suggestion.bd --no-color
expect_contains "human suggestion" '= help: add `mut` before this argument' "$stderr"

run_driver note.bd --diagnostic-format=json
expect_status "json note" 1
expect_contains "json note" '"notes":[{"message":"value moved here","span":{"file":"note.bd"' "$stderr"

run_driver note.bd --no-color
expect_contains "human note" '= note: note.bd:' "$stderr"
expect_contains "human note" 'value moved here' "$stderr"

# --- Error limit and ordering (issue #436) -----------------------------------

run_driver two_errors.bd --error-limit 1
expect_status "--error-limit 1" 1
check
[ "$(grep -cE ':[0-9]+:[0-9]+: error\[E[0-9]{4}\]' "$work/stderr")" -eq 1 ] \
    || add_problem "--error-limit 1: more than one diagnostic line"
expect_contains "--error-limit 1" "note: 1 more error not shown (--error-limit 1)" "$stderr"
expect_contains "--error-limit 1" "error: could not compile two_errors.bd (2 errors)" "$stderr"

run_driver two_errors.bd --error-limit 0
check
[ "$(grep -cE ':[0-9]+:[0-9]+: error\[E[0-9]{4}\]' "$work/stderr")" -eq 2 ] \
    || add_problem "--error-limit 0: did not print every diagnostic"
check
if grep -qF 'not shown' "$work/stderr"; then
    add_problem "--error-limit 0: reported hidden errors"
fi

run_driver two_errors.bd --error-limit 5
check
if grep -qF 'not shown' "$work/stderr"; then
    add_problem "--error-limit 5: reported hidden errors"
fi

run_driver two_errors.bd --diagnostic-format=short
mv "$work/stderr" "$work/first"
run_driver two_errors.bd --diagnostic-format=short
check
if ! cmp -s "$work/first" "$work/stderr"; then
    add_problem "determinism: two runs of the same input differ"
fi

# The diagnostics are sorted by position even though the checker may report them in another order.
run_driver two_errors.bd --diagnostic-format=short
check
lines=$(sed -nE 's/^.*:([0-9]+):[0-9]+: error\[E[0-9]{4}\].*$/\1/p' "$work/stderr" | tr '\n' ' ')
[ "$lines" = "2 3 " ] || add_problem "ordering: diagnostic lines are $lines, expected 2 3"

# --- Flag syntax and usage errors -------------------------------------------

run_driver two_errors.bd --diagnostic-format xml
expect_status "unknown diagnostic format" 2
expect_contains "unknown diagnostic format" "unknown diagnostic format" "$stderr"

run_driver two_errors.bd --color sparkles
expect_status "unknown color choice" 2

run_driver two_errors.bd --width wide
expect_status "bad width" 2

run_driver two_errors.bd --error-limit -1
expect_status "bad error limit" 2

run_driver two_errors.bd --release=yes
expect_status "a value on --release" 2

# `run` accepts the same options and still exits 1 on a compilation failure.
( cd "$work" && "$BURDIN" run run_bad.bd --no-color --error-limit 1 ) > "$work/stdout" 2> "$work/stderr"
status=$?
expect_status "run with diagnostic options" 1
expect_no_color "run with diagnostic options"
expect_contains "run with diagnostic options" "error: could not compile run_bad.bd (1 error)" "$(cat "$work/stderr")"

echo "Checked $checks diagnostics behaviors from $work"
if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
