#!/usr/bin/env bash
# Checks the release pipeline (issues #452-#456):
#   * .github/workflows/release.yml builds the six supported targets on native runners, smoke-tests each one,
#     uploads archives and per-target checksums, verifies reproducibility, and publishes only from a tag push or an
#     explicit manual `publish`;
#   * tests/build-release.sh prints the manifest version, packages a versioned archive whose contents and checksum
#     are deterministic, and rejects a binary whose --version disagrees with the archive name;
#   * install.sh and install.ps1 verify SHA256SUMS before installing, and install.sh installs from a local release
#     directory (the rehearsal path) and refuses a tampered archive or an unsupported platform;
#   * tests/release-notes.sh extracts a changelog section and fails on a broken relative link;
#   * docs/releasing.md records the version scheme, channels, artifact protocol, reproducibility, and rehearsal;
#   * `burdin --version` reports the version and channel the policy requires.
#
# Needs a built driver for the last section. BURDIN selects it, like tests/run_tests.py.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(dirname "$SCRIPT_DIR")
WORKFLOW=$ROOT/.github/workflows/release.yml
RELEASE_SCRIPT=$SCRIPT_DIR/build-release.sh
INSTALL_SH=$ROOT/install.sh
INSTALL_PS1=$ROOT/install.ps1
NOTES=$SCRIPT_DIR/release-notes.sh
POLICY=$ROOT/docs/releasing.md

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

problems=()
checks=0
add_problem() { problems+=("$1"); }

work=$(mktemp -d "${TMPDIR:-/tmp}/burdin-release.XXXXXX") || exit 1
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

expect_file() {   # $1 description, $2 path
    checks=$((checks + 1))
    [ -f "$2" ] || add_problem "$1: missing $2"
}

expect_status() {   # $1 description, $2 actual, $3 expected
    checks=$((checks + 1))
    [ "$2" -eq "$3" ] || add_problem "$1: exit status $2, expected $3"
}

expect_contains() {   # $1 description, $2 text, $3 output
    checks=$((checks + 1))
    case "$3" in
        *"$2"*) ;;
        *) add_problem "$1: output does not contain: $2" ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        echo "none"
    fi
}

# --- The release workflow (issues #453, #455, #456) ----------------------------------

checks=$((checks + 1))
if [ ! -f "$WORKFLOW" ]; then
    add_problem ".github/workflows/release.yml: missing"
else
    text=$(tr -d '\r' < "$WORKFLOW")
    for needle in \
        'name: Release' \
        'tags:' \
        "'v[0-9]*'" \
        'workflow_dispatch:' \
        'x86_64-unknown-linux-gnu' \
        'aarch64-unknown-linux-gnu' \
        'x86_64-apple-darwin' \
        'aarch64-apple-darwin' \
        'x86_64-pc-windows-msvc' \
        'aarch64-pc-windows-msvc' \
        'ubuntu-24.04' \
        'ubuntu-24.04-arm' \
        'macos-15-intel' \
        'macos-15' \
        'windows-2025' \
        'windows-11-arm' \
        'tests/build-release.sh' \
        '--verify-reproducible' \
        'tests/release-notes.sh' \
        'tests/check-release.sh' \
        'BURDIN_VERSION' \
        'BURDIN_CHANNEL' \
        'SHA256SUMS' \
        'sha256sum -c' \
        'gh release create' \
        'actions/upload-artifact@v4' \
        'actions/download-artifact@v4' \
        'install.sh'
    do
        checks=$((checks + 1))
        grep -qF -- "$needle" <<< "$text" || add_problem "release.yml: missing: $needle"
    done

    # Publishing is opt-in: a manual dispatch defaults to false, and the publish step checks the output.
    checks=$((checks + 1))
    grep -A4 '^      publish:$' <<< "$text" | grep -qF 'default: false' \
        || add_problem "release.yml: a manual run publishes by default (publish must default to false)"
    checks=$((checks + 1))
    grep -qF "if: needs.plan.outputs.publish == 'true'" <<< "$text" \
        || add_problem "release.yml: the GitHub release is not gated on the publish output"
    checks=$((checks + 1))
    grep -qF "contents: write" <<< "$text" \
        || add_problem "release.yml: no job has contents: write for the publish step"
fi

# --- The policy and the documentation index (issue #456) -----------------------------

checks=$((checks + 1))
if [ ! -f "$POLICY" ]; then
    add_problem "docs/releasing.md: missing (the version and release policy)"
else
    policy=$(tr -d '\r' < "$POLICY")
    for heading in \
        '## Version scheme' \
        '## Channels' \
        '## Breaking changes and packages' \
        '## Artifacts' \
        '## Checksums' \
        '## Reproducibility' \
        '## Installers' \
        '## Release notes' \
        '## Cutting a release' \
        '## Rehearsal checklist'
    do
        checks=$((checks + 1))
        grep -qF -- "$heading" <<< "$policy" || add_problem "docs/releasing.md: missing section: $heading"
    done
    for word in 'SemVer' 'stable' 'nightly' 'dev' 'SHA-256' 'SOURCE_DATE_EPOCH' 'LLVM 21'; do
        checks=$((checks + 1))
        grep -qF -- "$word" <<< "$policy" || add_problem "docs/releasing.md: does not mention: $word"
    done
fi
checks=$((checks + 1))
grep -qF 'docs/releasing.md' "$ROOT/README.md" || add_problem "README.md: does not link docs/releasing.md"
checks=$((checks + 1))
grep -qF 'install.sh' "$ROOT/README.md" || add_problem "README.md: does not document install.sh"
checks=$((checks + 1))
grep -qF 'install.ps1' "$ROOT/README.md" || add_problem "README.md: does not document install.ps1"

# --- Packaging and checksums (issue #455) -------------------------------------------

expect_file "tests/build-release.sh" "$RELEASE_SCRIPT"
expect_file "tests/release-notes.sh" "$NOTES"
expect_file "install.sh" "$INSTALL_SH"
expect_file "install.ps1" "$INSTALL_PS1"

if [ -f "$RELEASE_SCRIPT" ]; then
    # A rewritten script must keep building the release profile, smoke-testing the binary before packaging, and
    # normalizing the build and the archive (issues #453, #455).
    for needle in 'smoke_test' 'cargo build --release --locked' '--remap-path-prefix' \
                  'SOURCE_DATE_EPOCH' 'gzip -n' 'sha256_of'; do
        checks=$((checks + 1))
        grep -qF -- "$needle" "$RELEASE_SCRIPT" \
            || add_problem "build-release.sh: no longer contains: $needle"
    done

    checks=$((checks + 1))
    manifest=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)
    printed=$("$RELEASE_SCRIPT" --print-version 2>/dev/null)
    [ "$printed" = "$manifest" ] \
        || add_problem "build-release.sh --print-version: '$printed', expected the manifest version '$manifest'"

    checks=$((checks + 1))
    printf '%s' "$manifest" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$' \
        || add_problem "Cargo.toml: '$manifest' is not a MAJOR.MINOR.PATCH version"

    cat > "$work/fake-burdin" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
    cat <<'VERSION'
burdin 9.9.9
channel: stable
target: x86_64-unknown-linux-gnu
llvm: 21.1.8
commit: test
profile: release
VERSION
    exit 0
fi
exit 1
EOF
    chmod +x "$work/fake-burdin"

    target=x86_64-unknown-linux-gnu
    archive_name="burdin-9.9.9-$target.tar.gz"
    checks=$((checks + 1))
    if ! "$RELEASE_SCRIPT" --binary "$work/fake-burdin" --target "$target" --version 9.9.9 \
            --channel stable --out "$work/dist-a" > "$work/package-a.log" 2>&1; then
        add_problem "build-release.sh --binary: $(cat "$work/package-a.log")"
    else
        expect_file "build release archive" "$work/dist-a/$archive_name"
        expect_file "build release checksum" "$work/dist-a/$archive_name.sha256"
        expect_file "build release per-target checksums" "$work/dist-a/SHA256SUMS.$target"

        hash=$(sha256_of "$work/dist-a/$archive_name")
        checks=$((checks + 1))
        grep -qx "$hash  $archive_name" "$work/dist-a/$archive_name.sha256" \
            || add_problem "the .sha256 file does not match the archive (expected $hash)"
        checks=$((checks + 1))
        grep -qx "$hash  $archive_name" "$work/dist-a/SHA256SUMS.$target" \
            || add_problem "SHA256SUMS.$target does not match the archive"

        # The archive layout: one root directory holding the compiler and the documentation.
        listing=$(tar -tzf "$work/dist-a/$archive_name" | LC_ALL=C sort)
        for entry in "burdin-9.9.9-$target/" "burdin-9.9.9-$target/burdin" \
                     "burdin-9.9.9-$target/README.md" "burdin-9.9.9-$target/CHANGELOG.md" \
                     "burdin-9.9.9-$target/docs/error-codes.md"; do
            expect_contains "archive layout" "$entry" "$listing"
        done

        # Packaging is deterministic: the same binary, version, and epoch make the same bytes.
        checks=$((checks + 1))
        if ! "$RELEASE_SCRIPT" --binary "$work/fake-burdin" --target "$target" --version 9.9.9 \
                --channel stable --out "$work/dist-b" > "$work/package-b.log" 2>&1; then
            add_problem "build-release.sh --binary (second run): $(cat "$work/package-b.log")"
        else
            second=$(sha256_of "$work/dist-b/$archive_name")
            [ "$hash" = "$second" ] \
                || add_problem "packaging is not deterministic: $hash vs $second"
        fi

        # A binary whose --version disagrees with the archive name is refused.
        sed 's/burdin 9\.9\.9/burdin 9.9.8/' "$work/fake-burdin" > "$work/fake-wrong"
        chmod +x "$work/fake-wrong"
        checks=$((checks + 1))
        if "$RELEASE_SCRIPT" --binary "$work/fake-wrong" --target "$target" --version 9.9.9 \
                --channel stable --out "$work/dist-wrong" > /dev/null 2>&1; then
            add_problem "build-release.sh packaged a binary whose --version reports another version"
        fi

        # The version and channel are validated before anything is built.
        checks=$((checks + 1))
        if "$RELEASE_SCRIPT" --binary "$work/fake-burdin" --target "$target" --version 9.9 \
                --channel stable --out "$work/dist-bad" > /dev/null 2>&1; then
            add_problem "build-release.sh accepted a version that is not MAJOR.MINOR.PATCH"
        fi
        checks=$((checks + 1))
        if "$RELEASE_SCRIPT" --binary "$work/fake-burdin" --target "$target" --version 9.9.9 \
                --channel beta --out "$work/dist-bad" > /dev/null 2>&1; then
            add_problem "build-release.sh accepted an unknown channel"
        fi
        checks=$((checks + 1))
        if "$RELEASE_SCRIPT" --binary "$work/fake-burdin" --target riscv64-unknown-linux-gnu \
                --version 9.9.9 --channel stable --out "$work/dist-bad" > /dev/null 2>&1; then
            add_problem "build-release.sh accepted an unsupported target"
        fi
    fi
fi

# --- The installers (issues #454, #455) ---------------------------------------------

if [ -f "$INSTALL_SH" ]; then
    checks=$((checks + 1))
    sh -n "$INSTALL_SH" 2> "$work/sh-syntax.log" \
        || add_problem "install.sh: not valid POSIX sh: $(cat "$work/sh-syntax.log")"
    for needle in 'SHA256SUMS' 'sha256sum' 'shasum' 'BURDIN_INSTALL_DIR' 'BURDIN_RELEASE_DIR' \
                  'install.ps1' 'LLVM'; do
        checks=$((checks + 1))
        grep -qF -- "$needle" "$INSTALL_SH" || add_problem "install.sh: missing: $needle"
    done
fi

if [ -f "$INSTALL_PS1" ]; then
    for needle in 'Get-FileHash' 'SHA256SUMS' 'BURDIN_INSTALL_DIR' 'BURDIN_RELEASE_DIR' 'LLVM-C.dll'; do
        checks=$((checks + 1))
        grep -qF -- "$needle" "$INSTALL_PS1" || add_problem "install.ps1: missing: $needle"
    done
fi

if [ -f "$INSTALL_SH" ] && [ -f "$work/dist-a/burdin-9.9.9-x86_64-unknown-linux-gnu.tar.gz" ]; then
    release=$work/release
    mkdir -p "$release"
    cp "$work/dist-a/burdin-9.9.9-x86_64-unknown-linux-gnu.tar.gz" "$release/"
    cp "$work/dist-a/SHA256SUMS.x86_64-unknown-linux-gnu" "$release/SHA256SUMS"
    printf '9.9.9\n' > "$release/VERSION"

    checks=$((checks + 1))
    if ! BURDIN_RELEASE_DIR="$release" BURDIN_TARGET=x86_64-unknown-linux-gnu \
            BURDIN_INSTALL_DIR="$work/install" sh "$INSTALL_SH" > "$work/install.log" 2>&1; then
        add_problem "install.sh (local release): $(cat "$work/install.log")"
    else
        expect_file "install.sh installs the compiler" "$work/install/burdin"
        checks=$((checks + 1))
        installed=$("$work/install/burdin" --version 2>/dev/null | head -n 1)
        [ "$installed" = "burdin 9.9.9" ] \
            || add_problem "install.sh installed a binary that reports '$installed'"
    fi

    # A tampered archive is refused and leaves nothing installed.
    tampered=$work/tampered
    cp -a "$release" "$tampered"
    printf 'x' >> "$tampered/burdin-9.9.9-x86_64-unknown-linux-gnu.tar.gz"
    checks=$((checks + 1))
    if BURDIN_RELEASE_DIR="$tampered" BURDIN_TARGET=x86_64-unknown-linux-gnu \
            BURDIN_INSTALL_DIR="$work/install-tampered" sh "$INSTALL_SH" > "$work/tampered.log" 2>&1; then
        add_problem "install.sh accepted an archive whose checksum does not match"
    else
        checks=$((checks + 1))
        [ ! -e "$work/install-tampered/burdin" ] \
            || add_problem "install.sh installed a binary from a tampered archive"
        expect_contains "install.sh checksum failure" "checksum mismatch" "$(cat "$work/tampered.log")"
    fi

    # A release without a checksum entry is refused too.
    missing=$work/missing
    cp -a "$release" "$missing"
    : > "$missing/SHA256SUMS"
    checks=$((checks + 1))
    if BURDIN_RELEASE_DIR="$missing" BURDIN_TARGET=x86_64-unknown-linux-gnu \
            BURDIN_INSTALL_DIR="$work/install-missing" sh "$INSTALL_SH" > /dev/null 2>&1; then
        add_problem "install.sh installed a release with no SHA256SUMS entry"
    fi

    checks=$((checks + 1))
    if BURDIN_RELEASE_DIR="$release" BURDIN_TARGET=riscv64-unknown-linux-gnu BURDIN_VERSION=9.9.9 \
            BURDIN_INSTALL_DIR="$work/install-bad" sh "$INSTALL_SH" > /dev/null 2>&1; then
        add_problem "install.sh accepted an unsupported target"
    fi
    checks=$((checks + 1))
    if BURDIN_RELEASE_DIR="$release" BURDIN_TARGET=x86_64-unknown-linux-gnu BURDIN_VERSION=9.9.9 \
            BURDIN_CHANNEL=dev BURDIN_INSTALL_DIR="$work/install-dev" sh "$INSTALL_SH" > /dev/null 2>&1; then
        add_problem "install.sh accepted the dev channel, which is never published"
    fi
fi

# On Windows the PowerShell installer is exercised against the same fake release. The archive holds a shell script
# named burdin.exe, so only the checksum, extraction, and placement are proven; running it is not.
# The behavior test needs a Windows shell: on WSL, powershell.exe may be reachable through interop but cannot read a
# Unix path, so only run it where cygpath exists (Git Bash on Windows).
if [ -f "$INSTALL_PS1" ] && command -v cygpath >/dev/null 2>&1; then
    ps=$(command -v powershell.exe 2>/dev/null || command -v powershell 2>/dev/null || true)
    if [ -n "$ps" ]; then
        checks=$((checks + 1))
        win_path=$(cygpath -w "$INSTALL_PS1" 2>/dev/null || printf '%s' "$INSTALL_PS1")
        if ! "$ps" -NoProfile -NonInteractive -Command \
                "[void][scriptblock]::Create((Get-Content -Raw -LiteralPath '$win_path'))" > "$work/ps-syntax.log" 2>&1; then
            add_problem "install.ps1: PowerShell cannot parse it: $(head -n 5 "$work/ps-syntax.log")"
        fi

        cat > "$work/fake-burdin-windows" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
    cat <<'VERSION'
burdin 9.9.9
channel: stable
target: x86_64-pc-windows-msvc
llvm: 21.1.8
commit: test
profile: release
VERSION
    exit 0
fi
exit 1
EOF
        chmod +x "$work/fake-burdin-windows"
        win_target=x86_64-pc-windows-msvc
        win_archive="burdin-9.9.9-$win_target.tar.gz"
        checks=$((checks + 1))
        if ! "$RELEASE_SCRIPT" --binary "$work/fake-burdin-windows" --target "$win_target" \
                --version 9.9.9 --channel stable --out "$work/dist-win" > "$work/package-win.log" 2>&1; then
            add_problem "build-release.sh --binary (Windows archive): $(cat "$work/package-win.log")"
        else
            win_release=$work/release-win
            mkdir -p "$win_release"
            cp "$work/dist-win/$win_archive" "$win_release/"
            cp "$work/dist-win/SHA256SUMS.$win_target" "$win_release/SHA256SUMS"
            checks=$((checks + 1))
            install_dir=$(cygpath -w "$work/install-win" 2>/dev/null || printf '%s' "$work/install-win")
            release_dir=$(cygpath -w "$win_release" 2>/dev/null || printf '%s' "$win_release")
            if "$ps" -NoProfile -NonInteractive -ExecutionPolicy Bypass \
                    -File "$win_path" -Version 9.9.9 -Target "$win_target" -ReleaseDir "$release_dir" \
                    -InstallDir "$install_dir" > "$work/install-ps.log" 2>&1; then
                checks=$((checks + 1))
                [ -f "$work/install-win/burdin.exe" ] \
                    || add_problem "install.ps1 did not install burdin.exe"
            else
                add_problem "install.ps1 (local release): $(head -n 5 "$work/install-ps.log")"
            fi
        fi
    fi
fi

# --- Release notes (issue #456) ------------------------------------------------------

if [ -f "$NOTES" ]; then
    checks=$((checks + 1))
    if ! "$NOTES" --unreleased --check-links > "$work/notes.md" 2> "$work/notes.log"; then
        add_problem "release-notes.sh --unreleased --check-links: $(cat "$work/notes.log")"
    else
        expect_contains "release notes title" "# Burdin " "$(cat "$work/notes.md")"
        checks=$((checks + 1))
        [ "$(wc -l < "$work/notes.md" | tr -d ' ')" -gt 2 ] || add_problem "release notes are empty"
    fi

    cat > "$work/CHANGELOG-fixture.md" <<'EOF'
# Changelog

## Unreleased — Test

- See [README](README.md) and [the error codes](docs/error-codes.md).

## 1.2.3 — 2026-01-01

- See [a missing file](does-not-exist.md).
EOF
    checks=$((checks + 1))
    "$NOTES" --changelog "$work/CHANGELOG-fixture.md" --version 1.2.3 \
        > "$work/notes-version.md" 2> /dev/null
    status=$?
    [ "$status" -eq 0 ] && grep -qF '# Burdin 1.2.3' "$work/notes-version.md" \
        || add_problem "release-notes.sh --version did not extract the versioned section"
    checks=$((checks + 1))
    "$NOTES" --changelog "$work/CHANGELOG-fixture.md" --version 1.2.3 --check-links \
        > /dev/null 2>&1
    [ $? -ne 0 ] || add_problem "release-notes.sh --check-links accepted a broken link"
    checks=$((checks + 1))
    "$NOTES" --changelog "$work/CHANGELOG-fixture.md" --version 9.9.9 > /dev/null 2>&1
    [ $? -ne 0 ] || add_problem "release-notes.sh accepted a version with no changelog section"
    checks=$((checks + 1))
    "$NOTES" --changelog "$work/CHANGELOG-fixture.md" --unreleased --check-links \
        > "$work/notes-fixture.md" 2>&1
    [ $? -eq 0 ] || add_problem "release-notes.sh rejected a valid Unreleased section"
fi

# --- The driver reports its version and channel (issues #454, #456) ------------------

checks=$((checks + 1))
version=$("$BURDIN" --version 2> "$work/version.log")
status=$?
if [ "$status" -ne 0 ]; then
    add_problem "burdin --version: exit status $status: $(cat "$work/version.log")"
else
    lines=$(printf '%s\n' "$version" | wc -l | tr -d ' ')
    checks=$((checks + 1))
    [ "$lines" -eq 6 ] || add_problem "burdin --version: $lines lines, expected 6"
    for prefix in "burdin " "channel: " "target: " "llvm: " "commit: " "profile: "; do
        checks=$((checks + 1))
        printf '%s\n' "$version" | grep -q "^$prefix" \
            || add_problem "burdin --version: no '$prefix' line"
    done
    checks=$((checks + 1))
    printf '%s\n' "$version" | grep -Eq '^channel: (dev|nightly|stable)$' \
        || add_problem "burdin --version: the channel is not dev, nightly, or stable"
    manifest=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)
    checks=$((checks + 1))
    if [ -z "${BURDIN_VERSION:-}" ]; then
        printf '%s\n' "$version" | grep -Fxq "burdin $manifest" \
            || add_problem "burdin --version: the version is not the manifest's ($manifest)"
    fi
    checks=$((checks + 1))
    grep -qF 'channel:' "$ROOT/README.md" || add_problem "README.md: does not document the channel line"
    checks=$((checks + 1))
    grep -qF 'BURDIN_CHANNEL' "$ROOT/README.md" || add_problem "README.md: does not document BURDIN_CHANNEL"
fi

# --- Summary -------------------------------------------------------------------------

echo "Checked the release pipeline ($checks checks)"

if [ ${#problems[@]} -gt 0 ]; then
    echo
    echo "Problems:"
    printf '  %s\n' "${problems[@]}"
    exit 1
fi
echo "No problems."
