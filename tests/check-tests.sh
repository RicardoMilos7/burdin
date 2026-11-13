#!/usr/bin/env bash
# Validates the structure of the conformance tests (directives and error codes).
# It does not compile anything. Usage: tests/check-tests.sh
# Same checks as check-tests.ps1.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$SCRIPT_DIR

# error codes registered in the README table
KNOWN=$(tr -d '\r' < "$ROOT/README.md" | grep -oE '^\| E[0-9]{4} \|' | grep -oE 'E[0-9]{4}' | sort -u)

problems=()
total=0
n_run=0; n_abort=0; n_fail=0; n_test=0; n_undecided=0
used=""

add_problem() { problems+=("$1"); }

while IFS= read -r file; do
    total=$((total + 1))
    rel=${file#"$ROOT"/}

    directives=$(tr -d '\r' < "$file" | grep '^//@ ' | sed 's|^//@ ||')
    n_expected=$(tr -d '\r' < "$file" | grep -c '^//> ')
    markers=$(tr -d '\r' < "$file" | grep -no '//~ E[0-9]\{4\}' | sed -E 's|^([0-9]+)://~ (E[0-9]{4})$|\1 \2|')

    # test kinds present
    kinds=""
    while IFS= read -r d; do
        word=${d%%[ :]*}
        case "$word" in
            run|run-abort|compile-fail|burdin-test|undecided) kinds="$kinds $word" ;;
        esac
    done <<< "$directives"
    kinds=${kinds# }

    if [ -z "$kinds" ]; then add_problem "$rel: no test kind directive"; continue; fi

    case " $kinds " in
        *" undecided "*) primary=undecided ;;
        *) primary=${kinds%% *} ;;
    esac
    n_kinds=$(wc -w <<< "$kinds")

    case "$primary" in
        run) n_run=$((n_run + 1)) ;;
        run-abort) n_abort=$((n_abort + 1)) ;;
        compile-fail) n_fail=$((n_fail + 1)) ;;
        burdin-test) n_test=$((n_test + 1)) ;;
        undecided) n_undecided=$((n_undecided + 1)) ;;
    esac

    if [ "$primary" != "undecided" ] && [ "$n_kinds" -gt 1 ]; then add_problem "$rel: more than one test kind"; fi
    [ "$primary" = "undecided" ] && continue

    n_markers=0
    [ -n "$markers" ] && n_markers=$(wc -l <<< "$markers")

    case "$primary" in
        run)
            [ "$n_markers" -gt 0 ] && add_problem "$rel: run test has //~ markers" ;;
        run-abort)
            grep -q '^stderr:' <<< "$directives" || add_problem "$rel: run-abort needs a //@ stderr: line" ;;
        compile-fail)
            [ "$n_markers" -eq 0 ] && add_problem "$rel: compile-fail needs at least one //~ marker"
            [ "$n_expected" -gt 0 ] && add_problem "$rel: compile-fail must not have //> lines" ;;
        burdin-test)
            grep -Eq '^burdin-test passed=[0-9]+ failed=[0-9]+$' <<< "$directives" \
                || add_problem "$rel: burdin-test needs passed=N failed=M" ;;
    esac

    while read -r line code; do
        [ -z "$code" ] && continue
        used="$used $code"
        if ! grep -qx "$code" <<< "$KNOWN"; then add_problem "$rel:$line: unknown error code $code"; fi
    done <<< "$markers"
done < <(grep -rl --include='*.bd' '^//@ ' "$ROOT" | sort)

echo "Checked $total tests:"
printf '  %-13s %s\n' compile-fail "$n_fail" burdin-test "$n_test" run "$n_run" run-abort "$n_abort" undecided "$n_undecided"

unused=""
for code in $KNOWN; do
    case " $used " in *" $code "*) ;; *) unused="$unused $code" ;; esac
done
[ -n "$unused" ] && echo "Error codes in the registry with no test:${unused}"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No structural problems."
