#!/usr/bin/env bash
# Builds, smoke-tests, packages, and checksums one Burdin compiler release archive (issues #452-#456).
#
# Usage: tests/build-release.sh [options]
#   --target <triple>       target triple for the archive name (default: the host triple)
#   --version <x.y.z>       version (default: the workspace version in Cargo.toml)
#   --channel <channel>     dev, nightly, or stable (default: BURDIN_CHANNEL, else dev)
#   --out <dir>             output directory (default: dist/)
#   --binary <path>         package this binary instead of building; its --version must match
#   --skip-smoke            do not run build/run/test with the binary
#   --verify-reproducible   build twice from two copies and require identical archives
#   --print-version         print the workspace version and exit
#   -h, --help              show this help
#
# The archive is `burdin-<version>-<target>.tar.gz`; its root directory is `burdin-<version>-<target>/` and holds the
# compiler, README.md, CHANGELOG.md, and docs/error-codes.md. Next to it the script writes
# `<archive>.sha256` and `SHA256SUMS.<target>`, both in the `sha256sum -c` format. The release workflow aggregates the
# per-target files into one `SHA256SUMS` (docs/releasing.md).
#
# Reproducibility: builds set CARGO_INCREMENTAL=0, remap the checkout and target paths to /burdin, and use the git
# commit time as SOURCE_DATE_EPOCH; archives sort their entries, pin every mtime to SOURCE_DATE_EPOCH, use uid/gid 0,
# and are compressed with `gzip -n`. `--verify-reproducible` proves the result byte for byte on the host.
#
# Environment: BURDIN_VERSION and BURDIN_CHANNEL are read through when the flags are not given. BURDIN_COMMIT
# overrides the commit embedded in the binary (the release workflow checks out with a .git directory, so normally it
# is derived from git).

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")

# The workspace version is the compiler version (docs/releasing.md).
manifest_version() {
    sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1
}

usage() {
    sed -n '2,28s/^# \{0,1\}//p' "$0"
}

die() {
    echo "error: $*" >&2
    exit 1
}

say() {
    echo "$*"
}

TARGET=""
VERSION=""
CHANNEL=""
OUT=$ROOT/dist
BINARY=""
SKIP_SMOKE=0
REPRODUCIBLE=0

while [ $# -gt 0 ]; do
    case "$1" in
        --target) [ $# -ge 2 ] || die "--target needs a triple"; TARGET=$2; shift 2 ;;
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION=$2; shift 2 ;;
        --channel) [ $# -ge 2 ] || die "--channel needs a value"; CHANNEL=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || die "--out needs a directory"; OUT=$2; shift 2 ;;
        --binary) [ $# -ge 2 ] || die "--binary needs a path"; BINARY=$2; shift 2 ;;
        --skip-smoke) SKIP_SMOKE=1; shift ;;
        --verify-reproducible) REPRODUCIBLE=1; shift ;;
        --print-version) manifest_version; exit 0 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

host_target() {
    rustc -vV 2>/dev/null | sed -n 's/^host: //p'
}

sha256_of() {   # $1 file -> the hex digest on stdout
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "no SHA-256 tool: install sha256sum or shasum"
    fi
}

valid_target() {   # the six supported release targets
    case "$1" in
        x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|\
        x86_64-apple-darwin|aarch64-apple-darwin|\
        x86_64-pc-windows-msvc|aarch64-pc-windows-msvc) return 0 ;;
        *) return 1 ;;
    esac
}

exe_suffix() {
    case "$1" in
        *-pc-windows-msvc) printf '%s' ".exe" ;;
        *) printf '%s' "" ;;
    esac
}

archive_name() {
    printf 'burdin-%s-%s.tar.gz' "$VERSION" "$TARGET"
}

[ -n "$TARGET" ] || TARGET=$(host_target)
[ -n "$TARGET" ] || die "cannot determine the host triple; pass --target"
valid_target "$TARGET" || die "unsupported release target: $TARGET (see docs/releasing.md)"
[ -n "$VERSION" ] || VERSION=${BURDIN_VERSION:-}
[ -n "$VERSION" ] || VERSION=$(manifest_version)
[ -n "$VERSION" ] || die "cannot read the workspace version from Cargo.toml; pass --version"
[ -n "$CHANNEL" ] || CHANNEL=${BURDIN_CHANNEL:-dev}
case "$CHANNEL" in
    dev|nightly|stable) ;;
    *) die "the channel must be dev, nightly, or stable, not: $CHANNEL" ;;
esac
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
    die "not a MAJOR.MINOR.PATCH version: $VERSION"
fi
if [ "$REPRODUCIBLE" = 1 ] && [ -n "$BINARY" ]; then
    die "--verify-reproducible builds the compiler; it cannot be combined with --binary"
fi
if [ -n "$BINARY" ] && [ ! -x "$BINARY" ]; then
    die "the binary is missing or not executable: $BINARY"
fi

# The commit recorded in the binary. Derived once so both reproducibility builds see the same value even when one
# copy has no .git directory.
COMMIT=${BURDIN_COMMIT:-}
if [ -z "$COMMIT" ]; then
    COMMIT=$(git -C "$ROOT" rev-parse --short=12 HEAD 2>/dev/null || true)
fi
[ -n "$COMMIT" ] || COMMIT=unknown
export BURDIN_COMMIT=$COMMIT

# The epoch that pins file mtimes in the archive. It defaults to the commit time, so identical sources produce
# identical archives regardless of when the build ran.
SOURCE_EPOCH=${SOURCE_DATE_EPOCH:-}
if [ -z "$SOURCE_EPOCH" ]; then
    SOURCE_EPOCH=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || true)
fi
[ -n "$SOURCE_EPOCH" ] || SOURCE_EPOCH=315532800   # 1980-01-01, the zip epoch
export SOURCE_DATE_EPOCH=$SOURCE_EPOCH

check_binary_version() {   # $1 binary
    local output
    output=$("$1" --version 2>&1) || die "$1 --version failed: $output"
    printf '%s\n' "$output" | grep -Fxq "burdin $VERSION" \
        || die "$1 --version does not report version $VERSION: $output"
    printf '%s\n' "$output" | grep -Fxq "channel: $CHANNEL" \
        || die "$1 --version does not report channel $CHANNEL: $output"
    printf '%s\n' "$output" | grep -Fxq "target: $TARGET" \
        || die "$1 --version does not report target $TARGET: $output"
}

# Set by build_binary. The path is returned through a variable, not stdout: a `die` inside a command substitution
# would only exit the subshell and the caller would continue with an empty path.
BUILT_BINARY=""

build_binary() {   # $1 source root, $2 cargo target dir
    local root=$1 target_dir=$2 binary
    case "$target_dir" in
        /*) ;;
        *) target_dir=$root/$target_dir ;;
    esac
    local flags="--remap-path-prefix=$root=/burdin --remap-path-prefix=$target_dir=/burdin/target"
    [ -z "${RUSTFLAGS:-}" ] || flags="${RUSTFLAGS} $flags"
    say "building $VERSION ($CHANNEL) for $TARGET from $root" >&2
    ( cd "$root" && \
        CARGO_INCREMENTAL=0 \
        RUSTFLAGS="$flags" \
        CARGO_TARGET_DIR="$target_dir" \
        BURDIN_VERSION="$VERSION" \
        BURDIN_CHANNEL="$CHANNEL" \
        cargo build --release --locked -p burdin_driver >&2 ) || die "cargo build failed"
    binary=$target_dir/release/burdin$(exe_suffix "$TARGET")
    [ -f "$binary" ] || die "cargo did not produce $binary"
    BUILT_BINARY=$binary
}

smoke_test() {   # $1 binary
    local binary=$1 dir program
    dir=$(mktemp -d "${TMPDIR:-/tmp}/burdin-release-smoke.XXXXXX") || die "cannot create a temporary directory"
    cat > "$dir/smoke_main.bd" <<'EOF'
fn main() {
    println("release smoke")
}
EOF
    cat > "$dir/smoke_test.bd" <<'EOF'
test "release smoke" {
    assert_eq(1 + 1, 2)
}
EOF
    say "smoke-testing build, run, and test" >&2
    ( cd "$dir" && "$binary" build smoke_main.bd -o smoke_program ) \
        || die "burdin build failed in the smoke test"
    program=$dir/smoke_program
    [ -f "$program" ] || program=$dir/smoke_program.exe
    [ -x "$program" ] || die "burdin build did not produce a program"
    [ "$("$program")" = "release smoke" ] || die "the smoke program printed the wrong output"
    [ "$( cd "$dir" && "$binary" run smoke_main.bd )" = "release smoke" ] \
        || die "burdin run printed the wrong output"
    ( cd "$dir" && "$binary" test smoke_test.bd ) > "$dir/test.log" 2>&1 \
        || die "burdin test failed: $(cat "$dir/test.log")"
    grep -q 'test result: 1 passed, 0 failed' "$dir/test.log" \
        || die "burdin test printed an unexpected summary: $(cat "$dir/test.log")"
    rm -rf "$dir"
}

normalize_mtimes() {   # $1 directory, $2 epoch
    local dir=$1 epoch=$2 stamp
    if date -u -d "@$epoch" +%Y%m%d%H%M.%S >/dev/null 2>&1; then
        stamp=$(date -u -d "@$epoch" +%Y%m%d%H%M.%S)
    elif date -u -r "$epoch" +%Y%m%d%H%M.%S >/dev/null 2>&1; then
        stamp=$(date -u -r "$epoch" +%Y%m%d%H%M.%S)
    else
        die "cannot format the source epoch $epoch"
    fi
    find "$dir" -exec env TZ=UTC touch -t "$stamp" {} + || die "cannot normalize file times"
}

make_archive() {   # $1 staging directory, $2 root directory name, $3 archive path, $4 epoch
    local stage=$1 root=$2 archive=$3 epoch=$4 plain list
    plain=$stage.tar
    if tar --version 2>/dev/null | grep -q 'GNU tar'; then
        ( cd "$stage" && tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
            --format=ustar -cf "$plain" "$root" ) || die "tar failed"
    else
        # bsdtar has no --sort; hand it the entries in byte order and disable recursion so each appears once.
        list=$stage.list
        ( cd "$stage" && find "$root" -print | LC_ALL=C sort > "$list" ) || die "cannot list the archive"
        ( cd "$stage" && tar --format=ustar --uid 0 --gid 0 --uname root --gname root -n \
            -cf "$plain" -T "$list" ) || die "tar failed"
        rm -f "$list"
    fi
    gzip -n -9 -c "$plain" > "$archive" || die "gzip failed"
    rm -f "$plain"
}

# Set by package_binary, for the same reason as BUILT_BINARY.
PACKAGED_ARCHIVE=""

package_binary() {   # $1 binary, $2 output dir
    local binary=$1 out=$2 stage root name archive hash
    root="burdin-$VERSION-$TARGET"
    name=$(archive_name)
    archive=$out/$name
    mkdir -p "$out" || die "cannot create $out"
    stage=$(mktemp -d "${TMPDIR:-/tmp}/burdin-release-stage.XXXXXX") || die "cannot create a temporary directory"
    mkdir -p "$stage/$root/docs" || die "cannot create the archive layout"
    cp "$binary" "$stage/$root/burdin$(exe_suffix "$TARGET")" || die "cannot copy the compiler"
    chmod 755 "$stage/$root/burdin$(exe_suffix "$TARGET")"
    cp "$ROOT/README.md" "$ROOT/CHANGELOG.md" "$stage/$root/" || die "cannot copy the documentation"
    cp "$ROOT/docs/error-codes.md" "$stage/$root/docs/" || die "cannot copy the error-code reference"
    normalize_mtimes "$stage" "$SOURCE_EPOCH"
    make_archive "$stage" "$root" "$archive" "$SOURCE_EPOCH"
    [ -s "$archive" ] || die "the archive is empty: $archive"
    hash=$(sha256_of "$archive")
    ( cd "$out" && printf '%s  %s\n' "$hash" "$name" > "$name.sha256" )
    ( cd "$out" && printf '%s  %s\n' "$hash" "$name" > "SHA256SUMS.$TARGET" )
    rm -rf "$stage"
    say "archive: $archive" >&2
    say "sha256:  $hash" >&2
    PACKAGED_ARCHIVE=$archive
}

if [ -n "$BINARY" ]; then
    check_binary_version "$BINARY"
    package_binary "$BINARY" "$OUT"
    exit 0
fi

if [ "$REPRODUCIBLE" = 1 ]; then
    work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-repro.XXXXXX") || die "cannot create a temporary directory"
    copy=$work/source
    mkdir -p "$copy" || die "cannot create the second source copy"
    trap 'rm -rf "$work"' EXIT
    # Copy the tree except the build outputs and .git; BURDIN_COMMIT already pins the commit, so the binary does not
    # depend on the checkout's git state.
    ( cd "$ROOT" && tar -cf - --exclude=./target --exclude=./dist --exclude=./.git . ) \
        | ( cd "$copy" && tar -xf - ) || die "cannot copy the source tree"
    say "reproducibility run 1" >&2
    build_binary "$ROOT" "$work/target-a"
    binary_a=$BUILT_BINARY
    check_binary_version "$binary_a"
    [ "$SKIP_SMOKE" = 1 ] || smoke_test "$binary_a"
    package_binary "$binary_a" "$work/out-a"
    archive_a=$PACKAGED_ARCHIVE
    say "reproducibility run 2 (different checkout path)" >&2
    build_binary "$copy" "$work/target-b"
    binary_b=$BUILT_BINARY
    check_binary_version "$binary_b"
    [ "$SKIP_SMOKE" = 1 ] || smoke_test "$binary_b"
    package_binary "$binary_b" "$work/out-b"
    archive_b=$PACKAGED_ARCHIVE
    hash_a=$(sha256_of "$archive_a")
    hash_b=$(sha256_of "$archive_b")
    [ "$hash_a" = "$hash_b" ] || die "archives differ: $hash_a ($archive_a) vs $hash_b ($archive_b)"
    say "reproducible: $hash_a"
    rm -rf "$work"
    exit 0
fi

if [ "$TARGET" != "$(host_target)" ]; then
    die "--target $TARGET is not the host triple ($(host_target)); release jobs use a native runner per target"
fi

build_binary "$ROOT" "${CARGO_TARGET_DIR:-$ROOT/target}"
binary=$BUILT_BINARY
check_binary_version "$binary"
[ "$SKIP_SMOKE" = 1 ] || smoke_test "$binary"
package_binary "$binary" "$OUT"
