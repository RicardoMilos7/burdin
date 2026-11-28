# Releasing Burdin

This page is the version and release policy for the `burdin` compiler, and the checklist that proves the pipeline
(issues #452-#456). The pipeline is implemented; **no release has been cut yet**. `tests/check_release.py` runs in
every CI job and fails when the workflow, the artifacts, the installers, the checksums, or this policy regress.

## Version scheme

Burdin uses [SemVer](https://semver.org/) for the compiler: `MAJOR.MINOR.PATCH`.

- The workspace version in `Cargo.toml` (`[workspace.package] version`) is the single source of truth. A build
  prints it as the first line of `burdin --version`.
- A stable release is a tag named `vMAJOR.MINOR.PATCH`. The release workflow refuses a tag whose version does not
  equal the manifest, so tagging a commit with a stale `Cargo.toml` fails before anything is published.
- **Before 1.0** the language has no compatibility promise. A breaking change bumps `MINOR`, and a compatible
  addition or a fix bumps `PATCH`. `0.1.0` is the first published version.
- **From 1.0** a breaking change bumps `MAJOR`.
- A prerelease is a SemVer prerelease such as `0.1.0-nightly.20261118`. It sorts before the stable version and is
  never reported as the latest stable release.

**Language editions are versioned separately from the compiler.** An edition is named by year (`2024` is the current
one) and defines the meaning of the language; the compiler version defines the implementation. A compiler release
supports one or more editions and has a default, and a new edition is opt-in, so upgrading the compiler does not
silently change what an existing program means. The `burdin.toml` field that selects an edition is not implemented
yet (it arrives with the stage 3 manifest work), so every package currently uses the `2024` edition.

The channel is reported on the second line of `burdin --version` (`channel: dev|nightly|stable`). `BURDIN_VERSION`
overrides the version printed (nightly builds use it), and `BURDIN_COMMIT` overrides the commit, so a binary built
outside a git checkout still reports where it came from.

## Channels

| Channel | Built by | Version | Published? |
| --- | --- | --- | --- |
| `dev` | a local `cargo build`, or `tests/build_release.py` without `--channel` | the manifest version | never |
| `nightly` | the weekly schedule or a manual workflow run | `<manifest>-nightly.YYYYMMDD` | only when a manual run sets `publish`, as a prerelease |
| `stable` | a push of a `vMAJOR.MINOR.PATCH` tag | exactly the manifest version | yes |

- The scheduled workflow is a nightly **build** of all six targets. It uploads artifacts and checksums but never
  creates a release, so the schedule cannot publish by accident.
- A manual run defaults to `publish: false`. It builds, smoke-tests, verifies the checksums, writes them to the job
  summary, and uploads the bundle; that is the dry run.
- A tag push publishes. A manual run can publish a nightly or a stable release only when `publish` is explicitly
  checked.
- `install.sh` and `install.ps1` default to the stable channel and accept `BURDIN_CHANNEL=nightly` with an explicit
  `BURDIN_VERSION` (a nightly is never the "latest release").

## Editions and migration

`[package] edition` in `burdin.toml` names the language edition new code is written in; a manifest without the line,
and a one-file target, is the 2024 edition. The compiler knows 2024 and 2026.

A breaking change to the language does not reach every package at once:

1. In the edition before the change, the old form is deprecated: a warning with a machine-applicable fix that writes
   the new form.
2. `burdin fix --migrate` applies those fixes and moves the manifest to the next edition, after checking the package
   under it; a migration that would not compile writes nothing.
3. In the new edition the old form is an error, with the same fix.

So every deprecation ships at least one release before the edition that removes it, and the release notes say which
edition removes it. `spec/fix.md` (FX10, FX11) is the exact behavior.

## Breaking changes and packages

A change is **breaking** when it changes the meaning or validity of a program, the observable behavior of the
runtime, or the documented driver contract:

- syntax or a standard-library name is removed, renamed, or given a different meaning;
- a program that compiled and ran before no longer compiles, or produces different output;
- `burdin build`, `run`, `test`, `explain`, or `--version` changes its contract, flags, or exit statuses.

Diagnostic wording, the order of equal diagnostics, and optimization details are not covered.

Before 1.0 every release may break; the `MINOR` bump is the signal. A breaking release lists the affected forms in
its changelog entry. Packages depend on each other by path today (`[dependencies] name = { path = ... }`), so there
is no version resolution to reconcile; when the stage 3 registry adds versions, a package will record the compiler
range and the edition it was written against, and the resolver will enforce them. Until then, a package's
`burdin.toml` names no compiler version and the release notes are the compatibility statement.

## Artifacts

Six native targets are built, each on a hosted runner of the same architecture (there is no cross-compilation):

| Target | Runner |
| --- | --- |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` |
| `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |
| `x86_64-apple-darwin` | `macos-15-intel` |
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-pc-windows-msvc` | `windows-2025` |
| `aarch64-pc-windows-msvc` | `windows-11-arm` |

Each job runs `tests/build_release.py --target <triple> --out dist`, which

1. builds the compiler, the toolchain manager, and `burdin-lsp` with `cargo build --release --locked -p burdin_driver
   -p burdin_toolchain -p burdin_lsp` and the pinned toolchain;
2. checks that the binary's `--version` reports the planned version, channel, and target;
3. smoke-tests `burdin build`, `burdin run`, and `burdin test` on a small program;
4. writes `burdin-<version>-<target>.tar.gz`, one archive per optional component, and their SHA-256 checksums.

The archive is uniform across platforms (`.tar.gz`, not `.zip`) so one packaging path and one installer shape cover
all six. Its layout is:

```
burdin-<version>-<target>/
  burdin            (burdin.exe on Windows)
  burdin-toolchain  the toolchain manager, installed as the burdin proxy (spec/toolchain.md)
  LLVM-C.dll        (Windows only, beside burdin.exe)
  README.md
  docs/error-codes.md
  docs/lints.md
  packages/         (the official packages, spec/packages.md; the compiler finds them beside itself)
```

Each target also gets the optional components of `spec/toolchain.md` (TC10) as separate archives,
`burdin-<component>-<version>-<target>.tar.gz` for `debuggers`, `docs`, `lsp`, and `std-src`, listed in the same
`SHA256SUMS`. The release job requires all six compiler archives and every component archive.

**Runtime requirement.** Development builds link LLVM 21 as a shared library (the static Windows archives crash in a
dynamic-CRT Rust binary). Release builds on Linux and macOS link it statically: `tests/build_release.py` sets
`BURDIN_STATIC_LLVM=1`, `crates/burdin_driver/build.rs` passes the archives `llvm-config --link-static --libs` names
and the C++ runtime as link arguments of the `burdin` binary (they must come after `inkwell` and `llvm-sys` on the
link line; a native library declared by `burdin_codegen` comes before them and leaves symbols unresolved on arm64
Linux), and the script fails the build if `ldd` or `otool -L` still lists an LLVM library. The binary is about 70 MB
and needs only the system C and C++ libraries (`libz`, `libzstd`, `libstdc++` on Linux), plus the `cc` that linking
programs needs anyway; the archive stays under 30 MB. The Windows archives carry the `LLVM-C.dll` from the LLVM 21
release the build linked against (found through `LLVM_SYS_211_PREFIX`, or `--llvm-dll`; `build_release.py` refuses to
package a Windows build without it), and `install.ps1` installs it beside `burdin.exe`. Linking programs on Windows
still needs the MSVC `cl`. A static Windows build is wired but unproven: `BURDIN_STATIC_LLVM=1` there builds with
`-C target-feature=+crt-static` and links the `LLVM*.lib` archives, so try it in a dry-run release before relying on it.

## Checksums

- Each build job writes `<archive>.sha256` and `SHA256SUMS.<target>` in the `sha256sum -c` format.
- The release job concatenates the per-target files into one `SHA256SUMS`, verifies it with `sha256sum -c`, and
  requires all six archives to be present before it does anything else.
- `install.sh` and `install.ps1` download `SHA256SUMS` and the archive, find the archive's entry, and compare the
  SHA-256 before extracting anything. A tampered archive, a release without a matching entry, and a target that is
  not one of the six are refused; `tests/check_release.py` proves the first two against a local release directory.

There is no signing or provenance attestation yet; a checksum proves the download matches the published file, not
who built it. The release job is the only writer, and it runs from the tagged commit.

## Reproducibility

A release build is made reproducible rather than merely repeatable:

- `CARGO_INCREMENTAL=0`, so no incremental state leaks into the output;
- the checkout path and the cargo target directory are remapped with
  `--remap-path-prefix=<root>=/burdin` and `--remap-path-prefix=<target>=/burdin/target`, so two checkouts at
  different paths compile the same source text;
- `SOURCE_DATE_EPOCH` is the commit time and pins `BURDIN_GIT_COMMIT` through the same commit;
- archive entries are sorted, every mtime is pinned to `SOURCE_DATE_EPOCH`, uid/gid are `0` (GNU tar) or
  `0`/`root` (bsdtar), the tar format is `ustar`, and gzip is run with `-n`, so the archive has no timestamp or
  builder identity;
- `python3 tests/build_release.py --verify-reproducible` copies the tree to a second path, builds both copies into separate
  target directories, and requires the two archives to have the same SHA-256. The release workflow's
  `reproducible` job runs it on Linux for every release.

The guarantee is **same pinned Rust, same LLVM, same system linker, same `Cargo.lock`, same target**. A different
LLVM build or a different linker may still produce a different binary; the archive metadata is normalized on every
platform, but byte equality of Mach-O and PE binaries is not claimed across builders and is not checked in CI.

## Installers

Unix (Linux and macOS):

```console
$ curl -fsSL https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.sh | sh
```

Windows PowerShell:

```console
> irm https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.ps1 | iex
```

Both installers:

- detect the platform and architecture and select one of the six targets (`BURDIN_TARGET` overrides it);
- resolve the latest stable release (or a nightly when `BURDIN_CHANNEL=nightly`), or take `BURDIN_VERSION`;
- verify `SHA256SUMS` before unpacking;
- install to `$HOME/.local/bin` (Unix) or `%LOCALAPPDATA%\Programs\burdin\bin` (Windows); `BURDIN_INSTALL_DIR`
  overrides it;
- install the archive's `packages/` folder (the official packages) in the install directory, beside the compiler, where
  `burdin` looks for it; an upgrade replaces the folder, and a `packages/` folder that is not Burdin's (its `README.md`
  lacks the `# Official packages` heading) is refused before anything is installed;
- run `burdin --version` and warn, with the platform's fix, when the LLVM 21 runtime is missing;
- put the install directory on `PATH` once: `install.ps1` appends it to the user's `PATH` (`HKCU\Environment`), and
  `install.sh` appends a PATH line to the startup file of zsh (`~/.zshrc`), bash (`~/.bashrc` on Linux,
  `~/.bash_profile` on macOS), or fish (`~/.config/fish/config.fish`). Other shells get a hint instead. `BURDIN_NO_MODIFY_PATH` (or `install.ps1 -NoModifyPath`)
  leaves `PATH` and the startup files alone.

When the archive carries the toolchain manager, both installers install through it (`spec/toolchain.md` TC13): the
release becomes a toolchain under the Burdin home and the default, the install directory gets the `burdin` and
`burdin-lsp` proxies, and the PATH change is recorded so `burdin toolchain self uninstall` can undo exactly that.

`BURDIN_BASE_URL` points the download at another mirror, and `BURDIN_RELEASE_DIR` installs from a local directory —
the rehearsal path used by `tests/check_release.py` and the release job. A package-manager formula is a later work
item; there is no Homebrew tap, `winget` package, or distro package yet.

## Release notes

Release notes are generated automatically by GitHub (`gh release create --generate-notes`) from the pull requests merged since the previous release tag.

## Cutting a release

1. Set `[workspace.package] version` in `Cargo.toml` to `<version>`, then run a build so `Cargo.lock` records the
   new workspace versions, and commit both.
2. Run the full local gate: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
   `cargo test --workspace`, `tests/run_tests.py` and `tests/run_tests.py --release`, `tests/check_release.py`, and
   `python3 tests/build_release.py --verify-reproducible`.
3. Run the release workflow by hand with `publish: false` and inspect the six archives, `SHA256SUMS`, and the
   reproducibility job. Install the Linux archive with `install.sh` and run `burdin build/run/test` on a sample
   package.
4. Commit, tag `v<version>`, and push the tag. The workflow builds the six targets, verifies the checksums and
   reproducibility, generates the release notes from merged PRs, and creates the GitHub release. Re-running the workflow on the same tag
   uploads the assets again (`gh release upload --clobber`) instead of creating a second release.
5. After publication, verify the installer one-liners on a clean machine and record the versions and checksums in
   the release notes.

## Rehearsal checklist

Every item below is automated; run it before the first real tag and after a change to the pipeline.

| Step | Command | Expected |
| --- | --- | --- |
| Policy and scripts | `tests/check_release.py` | `No problems.` |
| Version metadata | `target/debug/burdin --version` | six lines, and `channel:` is `dev` for a local build |
| Local package | `python3 tests/build_release.py --target <host> --out dist` | an archive and two checksum files; the script's smoke test passes |
| Reproducibility | `python3 tests/build_release.py --verify-reproducible` | `reproducible: <same SHA-256>` for two checkout paths |
| Install rehearsal | `BURDIN_RELEASE_DIR=$PWD/dist BURDIN_VERSION=<v> BURDIN_INSTALL_DIR=$PWD/bin sh install.sh` | the installed compiler prints `<v>`; a sample `build`, `run`, and `test` pass |
| Checksum refusal | append a byte to the archive and rerun the installer | `checksum mismatch`, non-zero exit, nothing installed |
| Dry run | Actions → Release → Run workflow, `publish` unchecked | six artifacts, a verified `SHA256SUMS`, no GitHub release |
| Stable publish | push a `v<version>` tag whose version matches `Cargo.toml` | a GitHub release with six archives, `SHA256SUMS`, and changelog notes |

The pipeline does not yet provide code signing, an SBOM or provenance attestation, a package-manager formula, a
bundled LLVM runtime, or Windows PDB symbols. The arm64 Windows and Intel macOS runner labels are GitHub-hosted
images; the first real rehearsal confirms they are available to the repository.
