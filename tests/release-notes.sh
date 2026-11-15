#!/usr/bin/env bash
# Generates release notes from CHANGELOG.md and checks the links in them (issues #452, #456).
#
# Usage: tests/release-notes.sh [options]
#   --version <x.y.z>      use the `## <x.y.z> ...` section
#   --unreleased           use the `## Unreleased ...` section (the default; for a dry run)
#   --changelog <file>     read another changelog (default: CHANGELOG.md)
#   --check-links          fail when a relative link in the notes does not exist
#   --output <file>        write the notes to a file instead of stdout
#   -h, --help             show this help
#
# The notes are the changelog section under a `# Burdin <version>` title. Cutting a stable release means moving the
# Unreleased entries under a `## <version> — <date>` heading first; docs/releasing.md has the steps.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")

usage() {
    sed -n '2,13s/^# \{0,1\}//p' "$0"
}

die() {
    echo "error: $*" >&2
    exit 1
}

manifest_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1
}

VERSION=""
UNRELEASED=0
CHANGELOG=$ROOT/CHANGELOG.md
CHECK_LINKS=0
OUTPUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION=$2; shift 2 ;;
        --unreleased) UNRELEASED=1; shift ;;
        --changelog) [ $# -ge 2 ] || die "--changelog needs a path"; CHANGELOG=$2; shift 2 ;;
        --check-links) CHECK_LINKS=1; shift ;;
        --output) [ $# -ge 2 ] || die "--output needs a path"; OUTPUT=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

[ -f "$CHANGELOG" ] || die "missing changelog: $CHANGELOG"
if [ -n "$VERSION" ] && [ "$UNRELEASED" = 1 ]; then
    die "--version and --unreleased are mutually exclusive"
fi

if [ "$UNRELEASED" = 1 ] || [ -z "$VERSION" ]; then
    pattern='^## Unreleased'
    title_version=$(manifest_version)
    [ -n "$title_version" ] || title_version=unknown
    title="# Burdin $title_version (unreleased)"
else
    escaped=$(printf '%s' "$VERSION" | sed 's/\./\\./g')
    pattern="^## $escaped([[:space:](]|\$)"
    title="# Burdin $VERSION"
fi

body=$(awk -v pattern="$pattern" '
    $0 ~ pattern { inside = 1; next }
    inside && /^## / { exit }
    inside { print }
' "$CHANGELOG" | awk '
    NF { started = 1 }
    started { line[++count] = $0 }
    END {
        while (count > 0 && line[count] ~ /^[[:space:]]*$/) count--
        for (i = 1; i <= count; i++) print line[i]
    }
')

if [ -z "$body" ]; then
    if [ "$UNRELEASED" = 1 ] || [ -z "$VERSION" ]; then
        die "CHANGELOG.md has no Unreleased section"
    fi
    die "CHANGELOG.md has no section for version $VERSION; add '## $VERSION — <date>' before tagging"
fi

notes=$(printf '%s\n\n%s\n' "$title" "$body")

if [ "$CHECK_LINKS" = 1 ]; then
    problems=0
    targets=$(printf '%s\n' "$body" \
        | grep -o '](\([^)]*\))' \
        | sed -e 's/^](//' -e 's/)$//' \
        | sort -u)
    while IFS= read -r target; do
        [ -n "$target" ] || continue
        case "$target" in
            http://*|https://*|mailto:*|tel:*|\#*) continue ;;
        esac
        path=${target%%#*}
        [ -n "$path" ] || continue
        path=$(printf '%s' "$path" | sed 's/%20/ /g')
        case "$path" in
            /*) resolved=$path ;;
            *) resolved=$ROOT/$path ;;
        esac
        if [ ! -e "$resolved" ]; then
            echo "broken link in the release notes: $target" >&2
            problems=$((problems + 1))
        fi
    done <<EOF
$targets
EOF
    [ "$problems" -eq 0 ] || exit 1
fi

if [ -n "$OUTPUT" ]; then
    printf '%s\n' "$notes" > "$OUTPUT" || die "cannot write $OUTPUT"
else
    printf '%s\n' "$notes"
fi
