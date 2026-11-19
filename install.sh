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
#   BURDIN_NO_MODIFY_PATH  set to any value to leave the shell startup file alone
#
# Every download is verified against the release's SHA256SUMS before it is unpacked. The compiler needs the LLVM 21
# shared library at run time (libLLVM-21.so on Linux, libLLVM.dylib on macOS, LLVM-C.dll on Windows); this script
# warns when the installed compiler cannot start (docs/releasing.md). Unless told not to, it adds the install directory
# to PATH in the startup file of zsh, bash, or fish (~/.zshrc; ~/.bashrc on Linux, ~/.bash_profile on macOS;
# ~/.config/fish/config.fish). Other shells get a hint instead. A running shell keeps its PATH, so open a new one afterwards.
#
# A release that carries the toolchain manager (burdin-toolchain, spec/toolchain.md) is installed through it: the
# release becomes a toolchain under the Burdin home (named after BURDIN_VERSION when it is set, else the channel) and
# the default, and the install directory gets the `burdin` and `burdin-lsp` proxies that select a toolchain per
# command. `burdin toolchain self uninstall` later removes exactly what this script and the manager created.

set -eu

REPO="RicardoMilos7/burdin"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
    -h|--help)
        sed -n '2,26s/^# \{0,1\}//p' "$0"
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
    api="https://api.github.com/repos/$REPO/releases"
    if [ "$1" = stable ]; then
        json=$(fetch_text "$api/latest") || return 1
    else
        json=$(fetch_text "$api?per_page=30") || return 1
    fi
    # A `for` loop over the tags keeps this inside POSIX sh: macOS's /bin/sh (bash 3.2 in POSIX mode) rejects a
    # `{ ...; }` group followed by `;;` inside a case branch. Tag names contain no whitespace.
    tags=$(printf '%s' "$json" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | sed -n 's/.*"\([^"]*\)"$/\1/p')
    tag=""
    for candidate in $tags; do
        if [ "$1" = nightly ]; then
            case "$candidate" in
                *nightly*) tag=$candidate; break ;;
            esac
        else
            case "$candidate" in
                *nightly*) ;;
                *) tag=$candidate; break ;;
            esac
        fi
    done
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
manager=$tmp/unpacked/burdin-$version-$target/burdin-toolchain$exe
if [ -f "$manager" ]; then
    # The toolchain manager installs the release as a toolchain and puts its proxies in $dir (spec/toolchain.md TC13).
    if [ -n "${BURDIN_VERSION:-}" ]; then name=$version; else name=$channel; fi
    chmod 755 "$manager" 2>/dev/null || true
    "$manager" toolchain self install --from "$tmp/unpacked/burdin-$version-$target" --name "$name" \
        --bin-dir "$dir" --default || die "the toolchain manager could not install burdin $version"
    managed=1
else
    # A release without the manager: install the compiler alone.
    managed=0
    if command -v install >/dev/null 2>&1; then
        install -m 755 "$source" "$dir/burdin$exe" || die "cannot install into $dir"
    else
        cp "$source" "$dir/burdin$exe" && chmod 755 "$dir/burdin$exe" || die "cannot install into $dir"
    fi
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
if [ -n "$exe" ]; then
    say "on Windows, linking needs Build Tools for Visual Studio with the \"Desktop development with C++\" workload"
fi

# --- PATH --------------------------------------------------------------------------------------------------------------

# Prints the startup file that the user's shell reads for PATH; fails for shells this script does not know.
startup_file() {
    case "${SHELL:-}" in
        */zsh) printf '%s\n' "$HOME/.zshrc" ;;
        */bash)
            case "$os" in
                Darwin*) printf '%s\n' "$HOME/.bash_profile" ;;
                *) printf '%s\n' "$HOME/.bashrc" ;;
            esac
            ;;
        */fish) printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" ;;
        *) return 1 ;;
    esac
}

# Prints the startup-file line that puts $1 on PATH. fish has no `export`; its line skips $1 when PATH already has it.
startup_line() {
    case "${SHELL:-}" in
        */fish) printf '%s\n' "contains -- \"$1\" \$PATH; or set -gx PATH \"$1\" \$PATH" ;;
        *) printf '%s\n' "export PATH=\"$1:\$PATH\"" ;;
    esac
}

# Appends the PATH line for $1 to the startup file, once. Fails when the shell is unknown or the file is not writable.
add_to_startup_file() {
    profile=$(startup_file) || return 1
    line=$(startup_line "$1")
    if [ -f "$profile" ] && grep -Fqx -- "$line" "$profile"; then
        say "$profile already puts $1 on PATH"
        return 0
    fi
    mkdir -p "$(dirname "$profile")" 2>/dev/null || return 1
    printf '\n%s\n' "$line" >> "$profile" || return 1
    say "added $1 to PATH in $profile; open a new shell to run burdin from anywhere"
    # The manager records the line, so `burdin toolchain self uninstall` removes exactly what was added here.
    if [ "$managed" = 1 ]; then
        "$1/burdin$exe" toolchain self record --startup-line "$profile" "$line" \
            || warn "could not record the PATH line for burdin toolchain self uninstall"
    fi
}

case ":${PATH:-}:" in
    *":$dir:"*) on_path=1 ;;
    *) on_path=0 ;;
esac
# A directory with a quote, a dollar sign, a backquote, or a backslash cannot be written safely into a startup file.
case "$dir" in
    *[\"\$\`\\]*) safe_for_profile=0 ;;
    *) safe_for_profile=1 ;;
esac
if [ -n "${BURDIN_NO_MODIFY_PATH:-}" ] || [ "$safe_for_profile" = 0 ] || ! add_to_startup_file "$dir"; then
    [ "$on_path" = 1 ] || say "add $dir to PATH to run burdin from anywhere"
fi
