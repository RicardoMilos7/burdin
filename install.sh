#!/bin/sh
# Installs a prebuilt Burdin compiler from a GitHub release (issues #452-#455).
#
# Install the latest stable release:
#   curl -fsSL https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.sh | sh
#
# Environment:
#   BURDIN_VERSION       version to install (default: the latest release of the channel)
#   BURDIN_CHANNEL       stable (default) or nightly
#   BURDIN_INSTALL_DIR   destination directory (default: $HOME/.local/bin)
#   BURDIN_TARGET        target triple override (default: detected from uname)
#   BURDIN_BASE_URL      directory that holds the archive and SHA256SUMS
#   BURDIN_RELEASE_DIR   local directory to install from, instead of the network (a rehearsal)
#
# Every download is verified against the release's SHA256SUMS before it is unpacked. The compiler needs the LLVM 21
# shared library at run time (libLLVM-21.so on Linux, libLLVM.dylib on macOS, LLVM-C.dll on Windows); this script
# warns when the installed compiler cannot start (docs/releasing.md).

set -eu

REPO="RicardoMilos7/burdin"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
    -h|--help)
        sed -n '2,21s/^# \{0,1\}//p' "$0"
        exit 0
        ;;
    "") ;;
    *) die "unknown argument: $1" ;;
esac

# --- platform ----------------------------------------------------------------------------------------------------

os=$(uname -s 2>/dev/null || echo unknown)
cpu=$(uname -m 2>/dev/null || echo unknown)

case "$os" in
    Linux*) platform=unknown-linux-gnu; exe="" ;;
    Darwin*) platform=apple-darwin; exe="" ;;
    MINGW*|MSYS*|CYGWIN*) platform=pc-windows-msvc; exe=".exe" ;;
    *) die "unsupported operating system: $os (use install.ps1 on Windows PowerShell)" ;;
esac

case "$cpu" in
    x86_64|amd64|x64) cpu=x86_64 ;;
    arm64|aarch64) cpu=aarch64 ;;
    *) die "unsupported architecture: $cpu" ;;
esac

target=${BURDIN_TARGET:-"$cpu-$platform"}
case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|\
    x86_64-apple-darwin|aarch64-apple-darwin|\
    x86_64-pc-windows-msvc|aarch64-pc-windows-msvc) ;;
    *) die "unsupported target: $target (expected one of the six release targets in docs/releasing.md)" ;;
esac
case "$target" in
    *-pc-windows-msvc) exe=".exe" ;;
    *) exe="" ;;
esac

channel=${BURDIN_CHANNEL:-stable}
case "$channel" in
    stable|nightly) ;;
    *) die "the channel must be stable or nightly, not: $channel" ;;
esac

# --- fetching ----------------------------------------------------------------------------------------------------

download() {   # $1 URL, $2 destination
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$1" -O "$2"
    else
        die "curl or wget is required"
    fi
}

fetch_text() {   # $1 URL -> stdout
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$1"
    else
        die "curl or wget is required"
    fi
}

latest_version() {   # $1 stable|nightly
    local api json tag
    api="https://api.github.com/repos/$REPO/releases"
    if [ "$1" = stable ]; then
        json=$(fetch_text "$api/latest") || return 1
    else
        json=$(fetch_text "$api?per_page=30") || return 1
    fi
    tag=$(printf '%s' "$json" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | sed -n 's/.*"\([^"]*\)"$/\1/p' \
        | while IFS= read -r candidate; do
            case "$candidate" in
                *nightly*) [ "$1" = nightly ] && { printf '%s\n' "$candidate"; break; } ;;
                *) [ "$1" = stable ] && { printf '%s\n' "$candidate"; break; } ;;
            esac
        done \
        | head -n 1)
    [ -n "$tag" ] || return 1
    printf '%s' "${tag#v}"
}

version=${BURDIN_VERSION:-}
if [ -z "$version" ] && [ -n "${BURDIN_RELEASE_DIR:-}" ] && [ -f "$BURDIN_RELEASE_DIR/VERSION" ]; then
    version=$(tr -d '[:space:]' < "$BURDIN_RELEASE_DIR/VERSION")
fi
if [ -z "$version" ]; then
    [ -z "${BURDIN_RELEASE_DIR:-}" ] || die "set BURDIN_VERSION when installing from BURDIN_RELEASE_DIR"
    version=$(latest_version "$channel") \
        || die "cannot resolve the latest $channel release; set BURDIN_VERSION"
fi
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' \
    || die "not a MAJOR.MINOR.PATCH version: $version"

archive="burdin-$version-$target.tar.gz"
if [ -n "${BURDIN_RELEASE_DIR:-}" ]; then
    base=$BURDIN_RELEASE_DIR
    fetch() { cp "$base/$1" "$2"; }
elif [ -n "${BURDIN_BASE_URL:-}" ]; then
    base=${BURDIN_BASE_URL%/}
    fetch() { download "$base/$1" "$2"; }
else
    base="https://github.com/$REPO/releases/download/v$version"
    fetch() { download "$base/$1" "$2"; }
fi

# --- checksum and install ----------------------------------------------------------------------------------------

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "no SHA-256 tool: install sha256sum or shasum"
    fi
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/burdin-install.XXXXXX") || die "cannot create a temporary directory"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading burdin $version ($channel) for $target"
fetch SHA256SUMS "$tmp/SHA256SUMS" || die "cannot download $base/SHA256SUMS"
fetch "$archive" "$tmp/$archive" || die "cannot download $base/$archive"

expected=$(awk -v name="$archive" '$2 == name { print $1; exit }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS does not list $archive"
actual=$(sha256_of "$tmp/$archive")
[ "$expected" = "$actual" ] \
    || die "checksum mismatch for $archive: expected $expected, got $actual"

mkdir -p "$tmp/unpacked"
tar -xzf "$tmp/$archive" -C "$tmp/unpacked" || die "cannot unpack $archive"
source=$tmp/unpacked/burdin-$version-$target/burdin$exe
[ -f "$source" ] || die "the archive does not contain burdin-$version-$target/burdin$exe"

dir=${BURDIN_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$dir" || die "cannot create $dir"
if command -v install >/dev/null 2>&1; then
    install -m 755 "$source" "$dir/burdin$exe" || die "cannot install into $dir"
else
    cp "$source" "$dir/burdin$exe" && chmod 755 "$dir/burdin$exe" || die "cannot install into $dir"
fi

say "installed burdin $version ($channel) for $target in $dir"
if "$dir/burdin$exe" --version >/dev/null 2>&1; then
    say "checksum verified; the compiler runs"
else
    warn "$dir/burdin$exe did not start. Burdin needs the LLVM 21 shared library at run time:"
    warn "  Linux: install the LLVM 21 runtime (libLLVM-21.so)"
    warn "  macOS: brew install llvm@21"
    warn "  Windows: put LLVM-C.dll on PATH or beside burdin.exe"
fi
case ":${PATH:-}:" in
    *":$dir:"*) ;;
    *) say "add $dir to PATH to run burdin from anywhere" ;;
esac
