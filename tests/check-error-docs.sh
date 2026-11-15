#!/usr/bin/env bash
# Checks the user-facing error-code reference (issue #437). Needs a built driver.
# Usage: tests/check-error-docs.sh
#
# BURDIN selects the driver, like tests/run-tests.sh; otherwise target/{debug,release}/burdin is used.
#
# What it checks:
#   * docs/error-codes.md has exactly one section for every code in the tests/README.md registry, no extra
#     codes, and no duplicate headings;
#   * every section has a "How to fix" paragraph and at least one checked example;
#   * every `burdin` snippet on the page is inside an `error-example`, and the page has no unchecked snippets;
#   * every example is a complete program (or package) that the driver compiles and rejects with exactly the
#     `//~ E0000` diagnostics marked in it, all of the section's code;
#   * the page is linked from README.md and from tests/README.md;
#   * `burdin explain <code>` prints every section, `burdin explain` lists every code, and an unknown code
#     exits 2.
#
# The page is also embedded in the driver, so `burdin explain` cannot disagree with it.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
DOCS=$ROOT/docs/error-codes.md
REGISTRY=$SCRIPT_DIR/README.md

problems=()
checks=0
examples=0
add_problem() { problems+=("$1"); }

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

if [ ! -f "$DOCS" ]; then
    echo "missing $DOCS" >&2
    exit 1
fi
if ! resolve_burdin; then
    echo "error: compiler driver not found. Build it first or set BURDIN=/path/to/burdin" >&2
    exit 2
fi
case "$BURDIN" in /*|[A-Za-z]:*) ;; *) BURDIN="$(pwd)/$BURDIN" ;; esac

work=""
trap 'rm -f "${normalized:-}"; rm -rf "${work:-}"' EXIT
# A CRLF checkout must not break the line-by-line parser. Read from a temporary copy rather than writing next to
# the repository copy.
if grep -q $'\r' "$DOCS"; then
    normalized=${TMPDIR:-/tmp}/burdin-error-docs.$$
    tr -d '\r' < "$DOCS" > "$normalized"
    DOCS=$normalized
fi

# --- Registry and page coverage ---------------------------------------------

registry_codes=$(tr -d '\r' < "$REGISTRY" | grep -oE '^\| E[0-9]{4} \|' | grep -oE 'E[0-9]{4}' | sort -u)
doc_codes=$(grep -oE '^## E[0-9]{4}:' "$DOCS" | grep -oE 'E[0-9]{4}' | sort -u)
checks=$((checks + 1))
if [ "$registry_codes" != "$doc_codes" ]; then
    add_problem "docs/error-codes.md and tests/README.md disagree about the codes: registry [$(echo $registry_codes)], page [$(echo $doc_codes)]"
fi
duplicate=$(grep -oE '^## E[0-9]{4}:' "$DOCS" | sort | uniq -d)
if [ -n "$duplicate" ]; then
    add_problem "docs/error-codes.md: duplicate sections: $(echo $duplicate)"
fi

section_of() {   # $1 code: the page section, without its heading's neighbours
    awk -v code="$1" '
        $0 ~ "^## " code ":" { inside = 1; print; next }
        inside && /^## / { exit }
        inside { print }
    ' "$DOCS"
}

for code in $registry_codes; do
    checks=$((checks + 1))
    section=$(section_of "$code")
    if [ -z "$section" ]; then
        add_problem "docs/error-codes.md: no section for $code"
        continue
    fi
    grep -qF '**How to fix:**' <<< "$section" \
        || add_problem "docs/error-codes.md: $code has no '**How to fix:**' paragraph"
    grep -qF "error-example: $code" <<< "$section" \
        || add_problem "docs/error-codes.md: $code has no checked example"
done

for index in "$ROOT/README.md" "$REGISTRY"; do
    checks=$((checks + 1))
    grep -qF 'docs/error-codes.md' "$index" \
        || add_problem "${index#$ROOT/}: does not link docs/error-codes.md"
done

# --- Example extraction ------------------------------------------------------

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-error-docs.XXXXXX") || exit 1

state=idle
code=""
target=""
mode=""
dir=""
path=""

check_example() {   # $1 example directory, $2 code, $3 target, $4 mode (build|test)
    local dir=$1 code=$2 target=$3 mode=$4 status file line marker actual_file line_no matched candidate
    if [ ! -s "$dir/.files" ]; then
        add_problem "example $code: no source files"
        return
    fi
    if [ -z "$target" ]; then
        if [ "$(wc -l < "$dir/.files")" -ne 1 ]; then
            add_problem "example $code: a multi-file example needs target=<path>"
            return
        fi
        target=$(cat "$dir/.files")
    fi
    if [ ! -e "$dir/$target" ]; then
        add_problem "example $code: target $target is not one of the example files"
        return
    fi
    if [ "$mode" = "test" ]; then
        # Test attributes are checked only in test mode; a normal build strips test items.
        ( cd "$dir" && "$BURDIN" test "$target" --no-color --diagnostic-format=short ) > "$dir/out" 2>&1
    else
        ( cd "$dir" && "$BURDIN" build "$target" --no-color --diagnostic-format=short ) > "$dir/out" 2>&1
    fi
    status=$?
    if [ "$status" -eq 0 ]; then
        add_problem "example $code: compiled successfully, but the section expects $code"
        return
    fi

    : > "$dir/expected"
    while IFS= read -r file; do
        [ -f "$dir/$file" ] || continue
        grep -nE '//~ E[0-9]{4}' "$dir/$file" \
            | sed -E "s|^([0-9]+):.*//~ (E[0-9]{4}).*|${file} \\1 \\2|" >> "$dir/expected"
    done < "$dir/.files"
    sort -o "$dir/expected" "$dir/expected"
    if [ ! -s "$dir/expected" ]; then
        add_problem "example $code: no //~ marker"
        return
    fi
    while read -r file line marker; do
        [ "$marker" = "$code" ] \
            || add_problem "example $code: $file:$line marks $marker instead of $code"
    done < "$dir/expected"

    : > "$dir/actual"
    while IFS= read -r line; do
        actual_file=$(sed -E 's|:[0-9]+:[0-9]+: error\[E[0-9]{4}\].*$||' <<< "$line")
        line_no=$(sed -E 's|^.*:([0-9]+):[0-9]+: error\[E[0-9]{4}\].*$|\1|' <<< "$line")
        marker=$(sed -E 's|.*error\[(E[0-9]{4})\].*|\1|' <<< "$line")
        actual_file=${actual_file//\\//}
        actual_file=${actual_file#./}
        matched=""
        while IFS= read -r candidate; do
            case $actual_file in
                "$candidate"|*"/$candidate") matched=$candidate; break ;;
            esac
        done < "$dir/.files"
        if [ -z "$matched" ]; then
            add_problem "example $code: diagnostic outside the example files: $line"
            continue
        fi
        printf '%s %s %s\n' "$matched" "$line_no" "$marker" >> "$dir/actual"
    done < <(grep -E ':[0-9]+:[0-9]+: error\[E[0-9]{4}\]' "$dir/out")
    sort -o "$dir/actual" "$dir/actual"
    if ! diff -u "$dir/expected" "$dir/actual" > "$dir/diff"; then
        add_problem "example $code: diagnostics differ from the //~ markers: $(tr '\n' ';' < "$dir/diff")"
    fi
    examples=$((examples + 1))
}

while IFS= read -r line; do
    case $state in
        idle)
            case $line in
                '<!-- error-example:'*)
                    attrs=${line#<!-- error-example:}
                    attrs=${attrs%-->}
                    attrs=$(printf '%s' "$attrs" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
                    code=${attrs%% *}
                    target=""
                    mode=build
                    rest=${attrs#"$code"}
                    for attr in $rest; do
                        case $attr in
                            target=*) target=${attr#target=} ;;
                            mode=test) mode=test ;;
                            *) add_problem "docs/error-codes.md: unknown example attribute: $attr" ;;
                        esac
                    done
                    if ! grep -Eqx 'E[0-9]{4}' <<< "$code"; then
                        add_problem "docs/error-codes.md: example without a code: $line"
                    fi
                    checks=$((checks + 1))
                    dir=$work/example-$checks
                    mkdir -p "$dir"
                    : > "$dir/.files"
                    path=""
                    state=await_fence
                    ;;
                '```'*)
                    add_problem "docs/error-codes.md: fenced block outside an error-example: $line"
                    ;;
            esac
            ;;
        await_fence)
            case $line in
                '```burdin') state=in_code ;;
                *)
                    add_problem "docs/error-codes.md: example $code is not followed by a \`\`\`burdin fence"
                    state=idle
                    ;;
            esac
            ;;
        in_code)
            case $line in
                '```')
                    state=await_end
                    ;;
                '// --- '*)
                    path=${line#// --- }
                    path=${path% ---}
                    case $path in
                        ''|/*|*..*)
                            add_problem "docs/error-codes.md: example $code has an invalid path: $path"
                            path=""
                            ;;
                        *)
                            if grep -qxF "$path" "$dir/.files"; then
                                add_problem "docs/error-codes.md: example $code repeats the path $path"
                            fi
                            mkdir -p "$dir/$(dirname "$path")"
                            printf '%s\n' "$path" >> "$dir/.files"
                            : > "$dir/$path"
                            ;;
                    esac
                    ;;
                *)
                    if [ -z "$path" ]; then
                        path="$code.bd"
                        printf '%s\n' "$path" >> "$dir/.files"
                        : > "$dir/$path"
                    fi
                    printf '%s\n' "$line" >> "$dir/$path"
                    ;;
            esac
            ;;
        await_end)
            case $line in
                '<!-- /error-example -->')
                    check_example "$dir" "$code" "$target" "$mode"
                    state=idle
                    ;;
                *)
                    add_problem "docs/error-codes.md: unexpected line after the example fence: $line"
                    ;;
            esac
            ;;
    esac
done < "$DOCS"
if [ "$state" != "idle" ]; then
    add_problem "docs/error-codes.md: unterminated error-example"
fi

# --- burdin explain ----------------------------------------------------------

explain_out=$work/explain.out
explain_err=$work/explain.err

run_explain() {
    "$BURDIN" explain "$@" > "$explain_out" 2> "$explain_err"
}

checks=$((checks + 1))
if ! run_explain; then
    add_problem "burdin explain (list) failed: $(cat "$explain_err")"
else
    for code in $registry_codes; do
        grep -qF "$code:" "$explain_out" || add_problem "burdin explain (list) omits $code"
    done
fi

for code in $registry_codes; do
    checks=$((checks + 1))
    if ! run_explain "$code"; then
        add_problem "burdin explain $code failed: $(cat "$explain_err")"
        continue
    fi
    grep -qF "## $code:" "$explain_out" || add_problem "burdin explain $code does not print its heading"
    grep -qF "//~ $code" "$explain_out" || add_problem "burdin explain $code does not print its example"
done

checks=$((checks + 1))
run_explain E9999
status=$?
if [ "$status" -ne 2 ]; then
    add_problem "burdin explain E9999: exit status $status, expected 2"
elif ! grep -qF 'unknown error code' "$explain_err"; then
    add_problem "burdin explain E9999: does not report an unknown code"
fi

checks=$((checks + 1))
run_explain E0003 extra
status=$?
if [ "$status" -ne 2 ]; then
    add_problem "burdin explain with two arguments: exit status $status, expected 2"
fi

echo "Checked $checks links, sections, and explanations; compiled $examples examples"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
