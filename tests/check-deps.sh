#!/usr/bin/env bash
# Audits the Rust dependencies of the Burdin compiler (issue #449).
# It needs no compiler, but re-verifies the license fields through `cargo metadata` when the registry
# cache is populated (set BURDIN_DEPS_REQUIRE_METADATA=1 to make that verification mandatory; CI does).
#
# What it checks:
#   * Cargo.lock exists, is tracked by git, and pins every external crate by name, exact version,
#     crates.io source, and checksum;
#   * every workspace crate is under crates/ and every crates/ directory is in the lockfile;
#   * tests/dependencies.txt names every external crate exactly (name and version), with a license and
#     a reason, and nothing that is not in Cargo.lock;
#   * every license is permissive: each identifier in the expression is on the policy allowlist in
#     tests/dependencies.txt (plus the exact term `Apache-2.0 WITH LLVM-exception`);
#   * every direct external dependency in Cargo.toml or a crate manifest requires an exact `=x.y.z`
#     version, and that version is the one Cargo.lock resolves;
#   * when `cargo metadata --locked --offline` succeeds, each crate's license field from its own
#     metadata matches the reviewed inventory, the package count matches, and `--locked` proves the
#     lockfile is current with the manifests.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
LOCK=$ROOT/Cargo.lock
AUDIT=$SCRIPT_DIR/dependencies.txt

problems=()
checks=0
add_problem() { problems+=("$1"); }

# Permissive licenses the project accepts. Every identifier in an expression must be on this list, so
# an OR with a copyleft option is rejected instead of being accepted through the permissive branch.
ALLOWED_LICENSES="MIT MIT-0 Apache-2.0 BSD-2-Clause BSD-3-Clause ISC 0BSD Zlib BSL-1.0 Unlicense CC0-1.0 Unicode-3.0 Unicode-DFS-2016"

if [ ! -f "$LOCK" ]; then
    echo "missing $LOCK" >&2
    exit 1
fi
if [ ! -f "$AUDIT" ]; then
    echo "missing $AUDIT" >&2
    exit 1
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-deps.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT

# --- The lockfile ------------------------------------------------------------

# The pin must be committed, not only present in a working tree.
if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    checks=$((checks + 1))
    git -C "$ROOT" ls-files --error-unmatch Cargo.lock >/dev/null 2>&1 \
        || add_problem "Cargo.lock: not tracked by git (the pin must be committed)"
fi

# One record per [[package]]: name, version, source, checksum. Workspace packages have no source.
awk '
    function flush() {
        if (name != "") printf "%s\t%s\t%s\t%s\n", name, version, source, checksum
        name = ""; version = ""; source = ""; checksum = ""
    }
    /^\[\[package\]\]$/ { flush(); next }
    /^name = / { value = $0; sub(/^name = "/, "", value); sub(/"$/, "", value); name = value; next }
    /^version = / { value = $0; sub(/^version = "/, "", value); sub(/"$/, "", value); version = value; next }
    /^source = / { value = $0; sub(/^source = "/, "", value); sub(/"$/, "", value); source = value; next }
    /^checksum = / { value = $0; sub(/^checksum = "/, "", value); sub(/"$/, "", value); checksum = value; next }
    END { flush() }
' "$LOCK" > "$work/lock.tsv"

: > "$work/lock-external.tsv"
: > "$work/lock-workspace.txt"
while IFS=$'\t' read -r name version source checksum; do
    [ -n "$name" ] || continue
    if [ -z "$source" ]; then
        printf '%s\n' "$name" >> "$work/lock-workspace.txt"
        continue
    fi
    checks=$((checks + 1))
    case "$source" in
        registry+https://github.com/rust-lang/crates.io-index) ;;
        *) add_problem "Cargo.lock: $name $version comes from $source, not crates.io" ;;
    esac
    [ -n "$checksum" ] || add_problem "Cargo.lock: $name $version has no checksum (it is not pinned)"
    printf '%s\t%s\t%s\t%s\n' "$name" "$version" "$source" "$checksum" >> "$work/lock-external.tsv"
done < "$work/lock.tsv"

if [ ! -s "$work/lock.tsv" ]; then
    add_problem "Cargo.lock: no packages (is it empty or malformed?)"
fi

# Every workspace package is a crate directory, and every crate directory is in the lockfile.
while IFS= read -r name; do
    [ -d "$ROOT/crates/$name" ] \
        || add_problem "Cargo.lock: workspace package $name has no crates/$name directory"
done < "$work/lock-workspace.txt"
for dir in "$ROOT"/crates/*/; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    checks=$((checks + 1))
    grep -Fxq "$name" "$work/lock-workspace.txt" \
        || add_problem "crates/$name: not a workspace package in Cargo.lock (run cargo build after adding it)"
done

# --- The reviewed inventory --------------------------------------------------

licenses_allowed() {   # $1: SPDX license expression
    local expr=$1 opens closes token stable
    opens=$(printf '%s' "$expr" | tr -cd '(' | wc -c | tr -d ' ')
    closes=$(printf '%s' "$expr" | tr -cd ')' | wc -c | tr -d ' ')
    [ "$opens" -eq "$closes" ] || return 1
    # Only the LLVM exception is accepted as a WITH term, and only on Apache-2.0.
    case "$expr" in
        *WITH*)
            case "$expr" in
                *'Apache-2.0 WITH LLVM-exception'*)
                    expr=${expr//Apache-2.0 WITH LLVM-exception/Apache-2.0}
                    ;;
                *) return 1 ;;
            esac
            ;;
    esac
    expr=$(printf '%s' "$expr" | tr -d '()')
    # Drop the AND/OR operators; everything left must be a license identifier.
    stable=0
    while [ "$stable" -lt 4 ]; do
        case "$expr" in
            *' AND '*|*' OR '*) expr=$(printf '%s' "$expr" | sed -E 's/[[:space:]]+(AND|OR)[[:space:]]+/ /g') ;;
            *) break ;;
        esac
        stable=$((stable + 1))
    done
    case "$expr" in
        *AND*|*OR*) return 1 ;;
    esac
    expr=$(printf '%s' "$expr" | tr -s '[:space:]' ' ' | sed -E 's/^ | $//g')
    [ -n "$expr" ] || return 1
    for token in $expr; do
        case " $ALLOWED_LICENSES " in
            *" $token "*) ;;
            *) return 1 ;;
        esac
    done
    return 0
}

: > "$work/audit.tsv"
tr -d '\r' < "$AUDIT" | grep -v '^#' | grep -v '^[[:space:]]*$' \
    | while IFS=$'\t' read -r name version license role; do
        printf '%s\t%s\t%s\t%s\n' "$name" "$version" "$license" "$role"
    done > "$work/audit-raw.tsv"

while IFS=$'\t' read -r name version license role; do
    checks=$((checks + 1))
    if [ -z "$name" ] || [ -z "$version" ] || [ -z "$license" ] || [ -z "$role" ]; then
        add_problem "tests/dependencies.txt: '$name' needs four tab-separated fields (name, version, license, why)"
        continue
    fi
    case "$name" in
        *[!A-Za-z0-9_-]*) add_problem "tests/dependencies.txt: invalid crate name '$name'" ;;
    esac
    if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        add_problem "tests/dependencies.txt: $name: '$version' is not an exact x.y.z version"
    fi
    if ! licenses_allowed "$license"; then
        add_problem "tests/dependencies.txt: $name: license '$license' is not in the permissive policy"
    fi
    printf '%s\t%s\t%s\n' "$name" "$version" "$license" >> "$work/audit.tsv"
done < "$work/audit-raw.tsv"

cut -f1,2 "$work/audit-raw.tsv" | LC_ALL=C sort > "$work/audit.keys"
cut -f1,2 "$work/lock-external.tsv" | LC_ALL=C sort > "$work/lock.keys"

# The same crate at two versions needs two reviewed lines; sort alone would not catch that.
duplicates=$(cut -f1,2 "$work/audit-raw.tsv" | LC_ALL=C sort | LC_ALL=C uniq -d)
if [ -n "$duplicates" ]; then
    while IFS= read -r duplicate; do
        add_problem "tests/dependencies.txt: duplicate entry: $(printf '%s' "$duplicate" | tr '\t' ' ')"
    done <<< "$duplicates"
fi

while IFS= read -r key; do
    [ -n "$key" ] || continue
    add_problem "tests/dependencies.txt: $(printf '%s' "$key" | tr '\t' ' ') is not in Cargo.lock"
done < <(LC_ALL=C comm -23 "$work/audit.keys" "$work/lock.keys")

while IFS= read -r key; do
    [ -n "$key" ] || continue
    add_problem "Cargo.lock: $(printf '%s' "$key" | tr '\t' ' ') is not reviewed in tests/dependencies.txt"
done < <(LC_ALL=C comm -13 "$work/audit.keys" "$work/lock.keys")

# --- Exact pins in the manifests --------------------------------------------

# One line per external dependency declaration: file, name, version requirement.
awk '
    function emit(requirement) {
        printf "%s\t%s\t%s\n", FILENAME, dependency, requirement
    }
    FNR == 1 { section = ""; dependency = ""; next }
    /^[[:space:]]*#/ { next }
    /^[[:space:]]*\[/ {
        header = $0
        sub(/^[[:space:]]*\[/, "", header)
        sub(/\][[:space:]]*$/, "", header)
        section = header
        dependency = ""
        next
    }
    section !~ /^(dependencies|dev-dependencies|build-dependencies|workspace\.dependencies)$/ \
        && section !~ /\.(dependencies|dev-dependencies|build-dependencies)$/ { next }
    dependency != "" {
        if (match($0, /version[[:space:]]*=[[:space:]]*"[^"]*"/)) {
            requirement = substr($0, RSTART, RLENGTH)
            sub(/^version[[:space:]]*=[[:space:]]*"/, "", requirement)
            sub(/"$/, "", requirement)
            emit(requirement)
        }
        if ($0 ~ /}/) dependency = ""
        next
    }
    /^[A-Za-z0-9_-]+[[:space:]]*=[[:space:]]*"/ {
        dependency = $0
        sub(/[[:space:]]*=.*$/, "", dependency)
        requirement = $0
        sub(/^[^"]*"/, "", requirement)
        sub(/".*$/, "", requirement)
        emit(requirement)
        dependency = ""
        next
    }
    /^[A-Za-z0-9_-]+[[:space:]]*=[[:space:]]*\{/ {
        dependency = $0
        sub(/[[:space:]]*=.*$/, "", dependency)
        if (match($0, /version[[:space:]]*=[[:space:]]*"[^"]*"/)) {
            requirement = substr($0, RSTART, RLENGTH)
            sub(/^version[[:space:]]*=[[:space:]]*"/, "", requirement)
            sub(/"$/, "", requirement)
            emit(requirement)
        }
        if ($0 !~ /}/) next
        dependency = ""
    }
' "$ROOT/Cargo.toml" "$ROOT"/crates/*/Cargo.toml > "$work/pins.tsv"

if [ ! -s "$work/pins.tsv" ]; then
    add_problem "Cargo.toml: no external dependency was found to pin (the manifest parser may be out of date)"
fi

while IFS=$'\t' read -r file dependency requirement; do
    [ -n "$dependency" ] || continue
    checks=$((checks + 1))
    case "$requirement" in
        '='*)
            version=${requirement#=}
            if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
                add_problem "${file#$ROOT/}: $dependency: '$requirement' is not an exact =x.y.z pin"
                continue
            fi
            ;;
        *)
            add_problem "${file#$ROOT/}: $dependency: '$requirement' is not an exact pin (write '=x.y.z')"
            continue
            ;;
    esac
    if ! awk -F'\t' -v name="$dependency" -v version="$version" \
        '$1 == name && $2 == version { found = 1 } END { exit !found }' "$work/lock-external.tsv"; then
        add_problem "${file#$ROOT/}: $dependency is pinned to $version, which is not the version in Cargo.lock"
    fi
done < "$work/pins.tsv"

# --- The crates' own metadata (optional, mandatory in CI) --------------------

metadata_note=""
if ! command -v cargo >/dev/null 2>&1; then
    metadata_note="cargo is not on PATH"
elif (cd "$ROOT" && cargo metadata --locked --offline --format-version 1 >"$work/metadata.json" 2>"$work/metadata.err"); then
    # A package is `"name":"...","version":"...","id":"...","license":...`; dependencies in the same
    # JSON have no "version" field, so only packages match. Parse failures are caught by the count check.
    grep -oE '"name":"[^"]+","version":"[^"]+","id":"[^"]*","license":("[^"]*"|null)' "$work/metadata.json" \
        | sed -E 's/^"name":"([^"]+)","version":"([^"]+)".*"license":("([^"]*)"|null)$/\1\t\2\t\4/' \
        | awk -F'\t' '$3 != "" { print $1 "@" $2 "@" $3 }' | LC_ALL=C sort > "$work/metadata.licenses"

    awk -F'\t' '{ print $1 "@" $2 "@" $3 }' "$work/audit.tsv" | LC_ALL=C sort > "$work/audit.licenses"

    while IFS= read -r line; do
        [ -n "$line" ] || continue
        add_problem "tests/dependencies.txt: license does not match the crate metadata: $line"
    done < <(LC_ALL=C comm -3 "$work/audit.licenses" "$work/metadata.licenses")

    metadata_count=$(wc -l < "$work/metadata.licenses" | tr -d ' ')
    lock_count=$(wc -l < "$work/lock-external.tsv" | tr -d ' ')
    [ "$metadata_count" -eq "$lock_count" ] \
        || add_problem "cargo metadata: $metadata_count licensed packages, but Cargo.lock has $lock_count external crates (is the metadata parser out of date?)"
else
    metadata_note=$(head -n 1 "$work/metadata.err" 2>/dev/null)
    metadata_note="cargo metadata --locked --offline failed${metadata_note:+: $metadata_note}"
fi

if [ -n "$metadata_note" ]; then
    if [ "${BURDIN_DEPS_REQUIRE_METADATA:-0}" = "1" ]; then
        add_problem "$metadata_note (BURDIN_DEPS_REQUIRE_METADATA=1)"
    else
        echo "note: $metadata_note; the license fields were not re-verified against the crates" >&2
    fi
fi

echo "Checked $checks dependency records and pins in Cargo.lock, the manifests, and tests/dependencies.txt"
if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
