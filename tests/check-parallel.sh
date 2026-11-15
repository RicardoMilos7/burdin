#!/usr/bin/env bash
# Checks that the parallel front end (issue #432) is deterministic: a package with many modules builds and runs
# with any BURDIN_JOBS value, and a package with errors reports byte-identical diagnostics whether the modules
# are parsed sequentially (BURDIN_JOBS=1) or in parallel.
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

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-par.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

make_package() {   # $1 directory, $2 module count, $3 body template
    local dir=$1 count=$2 body=$3 i name uses sum
    mkdir -p "$dir/src" || return 1
    printf '[package]\nname = "app"\n' > "$dir/burdin.toml"
    uses=""
    sum=""
    i=0
    while [ "$i" -lt "$count" ]; do
        name=$(printf 'm%02d' "$i")
        printf 'fn f(): i32 {\n    %s\n}\n' "$body" > "$dir/src/$name.bd"
        uses="$uses
use $name"
        if [ -z "$sum" ]; then sum="$name.f()"; else sum="$sum + $name.f()"; fi
        i=$((i + 1))
    done
    printf '%s\n\nfn main() {\n    println((%s).to_string())\n}\n' "$uses" "$sum" > "$dir/src/main.bd"
}

# --- A large package builds and runs with any number of workers ---------------
if make_package good 24 "1"; then
    for jobs in 1 2 8; do
        checks=$((checks + 1))
        out=$(BURDIN_JOBS=$jobs "$BURDIN" run good 2>err)
        status=$?
        if [ "$status" -ne 0 ]; then
            add_problem "BURDIN_JOBS=$jobs: exit status $status: $(head -n 5 err)"
        elif [ "$out" != "24" ]; then
            add_problem "BURDIN_JOBS=$jobs: printed '$out', expected '24'"
        fi
    done
else
    add_problem "could not create the multi-module package"
fi

# --- Diagnostics are identical in both modes ---------------------------------
# Every module has its own unknown name, so each contributes one E0002 at the same line.
if make_package bad 12 "missing_name"; then
    BURDIN_JOBS=1 "$BURDIN" build bad >/dev/null 2>seq.err
    seq_status=$?
    BURDIN_JOBS=8 "$BURDIN" build bad >/dev/null 2>par.err
    par_status=$?
    checks=$((checks + 1))
    [ "$seq_status" -ne 0 ] && [ "$par_status" -ne 0 ] \
        || add_problem "the failing package compiled (sequential $seq_status, parallel $par_status)"
    checks=$((checks + 1))
    if ! diff -u seq.err par.err > diff.txt; then
        add_problem "diagnostics differ between sequential and parallel parsing: $(head -n 20 diff.txt)"
    fi
    checks=$((checks + 1))
    [ "$(grep -c 'error\[E0002\]' seq.err)" -ge 12 ] \
        || add_problem "the sequential run did not report every module's error"
    checks=$((checks + 1))
    [ "$(grep -c 'error\[E0002\]' par.err)" -eq "$(grep -c 'error\[E0002\]' seq.err)" ] \
        || add_problem "the parallel run reported a different number of errors"
else
    add_problem "could not create the failing package"
fi

echo "Checked parallel analysis determinism ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
