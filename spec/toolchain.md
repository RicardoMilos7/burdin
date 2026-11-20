# The toolchain manager

This is the behavior of `burdin toolchain` and the `burdin` proxy (stage 3, issue #624: #626, #628, #630, #631). The
crate is `crates/burdin_toolchain`; `tests/check_toolchain.py` is the black-box check, and the crate's unit tests
cover the parsers, version order, the receipt, and resumed downloads.

**The name is `burdin toolchain`, not `burdinup` (decided).** One executable plays both parts. The release archive
carries it as `burdin-toolchain`; the installers copy it into the install directory as `burdin` and `burdin-lsp`.
Invoked as `burdin`, it handles `burdin [+<toolchain>] toolchain ...` itself and runs the selected toolchain's
compiler for everything else; invoked as `burdin-lsp`, it runs the selected toolchain's `burdin-lsp`. It never links
the compiler or LLVM: downloads go through `curl`, archives are unpacked with the system `tar` (on Windows the
system's `tar.exe`), and checksums are verified with `burdin_sha256`. The installers (`install.sh`, `install.ps1`)
stay as its bootstrap (TC13).

## Rules

**TC1. Toolchain names.** A toolchain is `stable`, `nightly`, or an exact version (`0.4.1`,
`0.5.0-nightly.20261118`). A channel follows its latest release through `update`; an exact version never changes.
The name is also the toolchain's directory. Anything else is refused with "`x` is not a toolchain". Versions order by
SemVer: a prerelease sorts before its release, and numeric prerelease identifiers compare as numbers.

**TC2. Targets.** The host target is one of the six release targets (`docs/releasing.md`), derived from the
manager's own architecture and operating system. On any other host the manager refuses to install.

**TC3. Where state lives.** Under the Burdin home (`BURDIN_HOME`, else the platform directory of `spec/repl.md` RP14):

```
<home>/toolchains/<name>/            one toolchain: the unpacked release archive, plus
<home>/toolchains/<name>/.installed.toml      its version, target, and components (and the mark that the manager made it)
<home>/toolchains/<name>/.components/<c>.files   the files each component added
<home>/toolchains/settings.toml      the default toolchain
<home>/toolchains/downloads/         partial downloads, kept for resuming
<home>/toolchains/receipt            what was created outside <home>/toolchains (TC14)
```

The rest of the Burdin home (the git cache, the REPL history, the script cache) is not the manager's.

**TC4. Where releases come from.** By default, the GitHub releases of the repository: a channel's latest version
comes from the releases API (`releases/latest` for `stable`, the newest tag containing `nightly` for `nightly`, as
`install.sh` resolves them), and a release's files from `releases/download/v<version>/<file>`.
`BURDIN_DIST_ROOT` names another distribution root (a directory, a `file://` URL, or an `http(s)://` URL) laid out as
`channels/<channel>` (a file holding the version) and `v<version>/<file>`. URLs are fetched with `curl`.

**TC5. Downloads are resumable and verified before they replace anything.** Every archive is checked against the
release's `SHA256SUMS`. A release that does not list the archive is refused ("release V has no archive for T", "release
V has no C component for T"). A download goes to `downloads/<file>.part`; a failed transfer keeps what arrived, and
the next attempt resumes it (an HTTP range request through `curl -C -`, or the remaining bytes of a local file). If a
resumed file does not verify, it is discarded and fetched again from the start ("did not verify; downloading it
again"). A file that still does not verify is deleted and is an error ("checksum mismatch for F: expected X, got Y").
The archive is unpacked into a staging directory beside the toolchains and moved into place only when it is complete,
so a failed install, update, or component leaves every installed toolchain as it was and nothing behind.

**TC6. Commands.** Results go to standard output, progress (`info: ...`) and errors (`error: ...`) to standard
error. Exit status 0 is success, 1 a failure, 2 a usage error.

| Command | Behavior |
| --- | --- |
| `install <toolchain>... [--component <c>]...` | installs each (`installed stable 0.4.0`); an installed one is reported (`stable 0.4.0 is already installed`) and gets any missing requested component; the first becomes the default when none is set (`default toolchain: stable`) |
| `update [<toolchain>...]` | without names, every installed channel; `stable: updated 0.4.0 -> 0.4.1` keeping the components, `stable: 0.4.1 is up to date`, or `0.3.0: 0.3.0 is an exact version and never changes`; a failure is reported per toolchain and makes the status 1 |
| `list` | `<name> <version>` per toolchain, sorted by name, the default marked ` (default)`; `no toolchains are installed` |
| `default [<toolchain>]` | prints the default (status 1 when none), or sets it, installing it first if needed |
| `remove <toolchain>...` | removes the toolchain directory; removing the default clears it and says so |
| `show` | `active toolchain:`, `selected by:`, and `installed:` lines (TC7) |
| `which [burdin\|burdin-lsp]` | the path of the active toolchain's binary |
| `component list\|add\|remove` | TC10 |
| `target list\|add\|remove` | TC11 |
| `self show\|update\|uninstall` | TC12, TC15, TC14 |

**TC7. Selection.** The first of these selects the toolchain: `burdin +<toolchain> ...`, the `BURDIN_TOOLCHAIN`
environment variable, the nearest `burdin-toolchain.toml` in the current directory or one above it, the default.
`show` names the source: `the +toolchain argument on the command line`, `the BURDIN_TOOLCHAIN environment variable`,
the file's path, or `the default toolchain`; with none it prints `active toolchain: none`. An invalid name is an error
that names its source (`+beta: ...`, `BURDIN_TOOLCHAIN: ...`, `<file>: line N: ...`). Running a tool with nothing
selected is an error that says to run `burdin toolchain install stable`; a selected toolchain that is not installed
is an error with the install command, except one selected by a file (TC8).

**TC8. `burdin-toolchain.toml`.** One `[toolchain]` table with `channel` (a toolchain name, required), `components`,
and `targets` (arrays of strings). Any other table or key, an unknown component, or a target that is not a release
target is an error with the file and line. When the proxy (or `which`) uses the file's toolchain, it installs it on
first use with the file's components, and installs a listed component that is missing; a listed target other than
the host is refused as in TC11.

**TC9. The proxy.** `burdin [+t] <args>` runs `<toolchain>/burdin` with `<args>` unchanged, and `burdin-lsp [+t]
<args>` runs `<toolchain>/burdin-lsp`. It sets `BURDIN_TOOLCHAIN` to the selected name, so a nested `burdin` uses the
same toolchain, and `BURDIN_TOOLCHAIN_DIR` to the toolchain directory. On Unix it replaces itself with the tool
(`exec`); on Windows it waits and exits with the tool's status. A compiler run directly (not through the proxy)
answers `burdin toolchain ...` and `burdin +t ...` with a usage error that points at the installers.

**TC10. Components.** Optional parts installed on demand, each a separate verified archive
`burdin-<component>-<version>-<target>.tar.gz` in the release, whose root `burdin-<component>-<version>-<target>/`
holds paths relative to the toolchain:

| Component | Holds |
| --- | --- |
| `debuggers` | `debuggers/`: the gdb and lldb printers and the Windows Natvis file |
| `docs` | `share/doc/docs/` and `share/doc/spec/`: the offline documentation |
| `lsp` | `burdin-lsp`, the language server without the LLVM back end (what the `burdin-lsp` proxy runs) |
| `std-src` | `lib/std/src/`: the standard library's sources |

`component list` prints `<name> (installed|available): <description>`; `add` installs into the selected toolchain (or
`--toolchain`), `remove` deletes exactly the files its list recorded and the directories it emptied. `update` reinstalls
a toolchain's components at the new version. The language server opens a standard-library file from the toolchain's
`lib/std/src` when the `std-src` component has exactly the embedded text (found through `BURDIN_TOOLCHAIN_DIR`), and
otherwise writes it to its temporary directory as before.

**TC11. Targets.** Every toolchain has its host target. `target list` prints the six release targets with
`(installed, host)` or `(not available)`; `target add <host>` says it is already installed; any other release target
is refused until cross-compilation exists (issue #282): "cannot add the target T: cross-compilation is not available
yet (issue #282)". The host cannot be removed. When cross-compilation lands, a target becomes a component carrying its
runtime pieces.

**TC12. `self show`.** Prints the manager's version and where each piece lives: the home, the toolchains directory and
each toolchain's directory, the settings, the downloads, the receipt, each proxy, and each recorded startup line or
user `PATH` entry (`proxy: none recorded` when the manager was not installed by an installer).

**TC13. The installers are the bootstrap.** `install.sh` and `install.ps1` download and verify the release as before.
When the archive carries `burdin-toolchain`, they run `burdin-toolchain toolchain self install --from <unpacked
release> --name <name> --bin-dir <install dir> --default`, which installs the release as the toolchain `<name>` (the
version when `BURDIN_VERSION`/`-Version` is given, else the channel), makes it the default, copies the manager into the
install directory as the `burdin` and `burdin-lsp` proxies, and records them in the receipt. The LLVM runtime stays in
the toolchain directory, beside its compiler. A release without the manager is installed as the compiler alone, as
before. When an installer adds the install directory to `PATH`, it records that change with `burdin toolchain self
record --startup-line <file> <line>` (`install.sh`) or `--user-path <dir>` (`install.ps1`).

**TC14. `self uninstall`.** Removes exactly what the installers and the manager created: the recorded proxies (and a
leftover `.old`/`.new` beside them), every toolchain directory that has an `.installed.toml`, the downloads, the
settings, staging leftovers, the receipt, each recorded startup line (with the blank line the installer wrote before
it), each recorded user `PATH` entry (Windows, through the raw `HKCU\Environment` value as `install.ps1` writes it), and
the toolchains directory when it is then empty. Anything else is left and reported (`left <path> (not created by
burdin toolchain)`); nothing outside the receipt and `<home>/toolchains` is touched. Without `--yes` it prints what it
would remove and exits with status 1, removing nothing. On Windows the running proxy is deleted by a short-lived
`cmd` after the process exits.

**TC15. `self update`.** Resolves the latest stable release; when it is newer than the manager, downloads and
verifies that release's compiler archive, takes its `burdin-toolchain`, and replaces every recorded proxy through a
temporary file (on Windows the running executable is renamed to `<name>.old` first and deleted by the next run).
Otherwise it prints `burdin toolchain V is up to date (the latest stable release is W)`. Without a recorded proxy it is
an error.

## Tests

`python3 tests/check_toolchain.py` (every CI job) builds `tests/toolchain/fake_burdin.rs` as the compiler of fixture
releases and checks TC1-TC15 with exact output: nothing installed, install/list/default/remove/which, the proxy's
arguments and exit status, the four selection sources and `show`, first-use installs from a file and from a later
component, bad files, update (kept components, exact versions, a failed verification leaving the old toolchain),
resumed and re-fetched downloads, missing archives and releases, HTTP through `curl` with a range request, components
and their removal, targets, `install.sh` (or `install.ps1` on Windows) bootstrapping from a release with the manager,
`self show`, `self update`, and `self uninstall` leaving foreign files alone. `tests/check_release.py` checks the
release packaging: the manager in the compiler archive and the four component archives in `SHA256SUMS.<target>`.

## Manual checklist

These need the real network or the real user environment and are checked by hand before a release:

- `burdin toolchain install stable` against the GitHub releases (TC4's API path) on Linux, macOS, and Windows.
- `install.ps1` without `-NoModifyPath`, then `burdin toolchain self uninstall --yes`: the user `PATH` entry is gone
  and `burdin.exe` is deleted after the command exits.
- Interrupting a large download (Ctrl-C) and rerunning the install resumes it.
