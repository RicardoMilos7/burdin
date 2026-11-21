# The build tool: command surface and Cargo parity

Burdin's build tool is the `burdin` executable. This page records which commands it has, which Cargo commands it
deliberately does not have, and what the command line promises. Issue #338 settled the surface; the behavior of a
command is its own specification (`spec/profiles.md`, `spec/packages.md`, `spec/test-runner.md`, ...). The tests
are `tests/check_cli.py`.

## BT1. The command list

A command is in scope when `burdin <command>` is listed by `burdin --help` and has a `burdin <command> --help` page.
The complete list:

| Command | What it does | Specification |
| --- | --- | --- |
| `build` | compile a file or package into an executable | `README.md`, "The driver contract" |
| `run` | build and run a program, passing the program's arguments after `--` | `README.md` |
| `test` | build and run the `test` blocks (and doc tests) of a package | `spec/test-runner.md` |
| `bench` | build and measure the `bench` blocks | `spec/bench.md` |
| `fuzz` | build a `fuzz` target with libFuzzer and fuzz it | `spec/fuzz.md` |
| `check` | type-check and run the memory and comptime checks, without code generation | `spec/check.md` |
| `fmt` | format source files, with one style and nothing to configure | `spec/fmt.md` |
| `lint` | report and fix common mistakes | `spec/lint.md` |
| `fix` | apply the compiler's and the linter's machine-applicable fixes | `spec/fix.md` |
| `ssr` | structural search and replace | `spec/refactor.md` |
| `refactor` | extract, inline, change a signature, or move an item | `spec/refactor.md` |
| `doc` | generate documentation as a site, JSON, or a coverage report | `spec/doc.md` |
| `new` | create a package directory (`--lib` or `--bin`) | `tests/check_new.py` |
| `init` | write a package into an existing directory | `tests/check_new.py` |
| `add` | add a path, git, or official dependency and update `burdin.lock` | `spec/packages.md` |
| `clean` | remove a package's build output, and with `--cache` the caches | `spec/packages.md`, PKG19 |
| `package` | write the package's reproducible archive to `target/package` | `spec/packages.md`, PKG20-PKG22 |
| `vendor` | copy the git dependencies into `vendor/` for offline builds | `spec/packages.md`, PKG23-PKG24 |
| `repl` | start an interactive session | `spec/repl.md` |
| `watch` | rerun `check`, `build`, `test`, or `run` when the sources change | `spec/watch.md` |
| `debug` | build with debug information and start a debugger | `spec/debug.md` |
| `expand` | print the source after comptime, derives, default arguments, and operators | `spec/inspect.md` |
| `asm` | print one function's assembly or LLVM IR with its source | `spec/inspect.md` |
| `bloat` | report where a binary's size comes from | `spec/inspect.md` |
| `bindgen` | generate a module from a C header (or a Rust crate), or a C header for a package | `spec/bindgen.md`, `spec/rust-interop.md` |
| `explain` | print the reference entry for an error code | `docs/error-codes.md` |
| `lsp` | run the language server on standard input and output | `spec/lsp.md` |
| `dap` | run the debug adapter on standard input and output | `spec/debug.md` |
| `help` | print the help of a command | this page |

The `burdin` **proxy** (`spec/toolchain.md`) adds `toolchain` and `+<toolchain>` before it runs a compiler. A
compiler run directly refuses them with a message naming the proxy.

## BT2. Cargo commands that are planned but not copied yet

These Cargo commands are in the plan, each owned by its issue. Until that lands, the command is an unknown command
(exit status 2, with the closest command suggested), not a silent no-op.

| Cargo command | Burdin plan |
| --- | --- |
| `remove` | issue #360 |
| `update`, `tree`, `metadata` | issue #346 |
| `publish`, `yank`, `login`, `search` | issue #347, with the registry of #361 and #252 |
| `install` | issue #348 |
| `owner`, `logout`, `uninstall`, `info` | issue #473 |
| feature selection (`--features`, `--all-features`) | issue #340 |
| workspace member selection | issues #342 and #240 |
| `--lib`/`--bin`/`--example`/`--test`/`--bench` target selection | issue #355 |
| semver compatibility checking | issue #359 |
| shell completions and man pages | issue #363 |
| build timing reports | issue #472 |
| `--quiet` and a global `--verbose` output control (the `-v` of `test` and `bench` shows passing output) | issue #799 |

Registry protocols and workspaces have their own entries in `spec/stage1-subset.md`; offline mode landed with
`burdin vendor` (BT1, issue #350). The command surface will grow with the rest, and this table is the list of what
is still missing.

## BT3. What is deliberately not copied from Cargo

- **No plugin or external-subcommand dispatch.** `burdin frobnicate` is an unknown command even if a
  `burdin-frobnicate` executable is on `PATH`. Every command in BT1 is part of the tool.
- **No proc-macros, `links`/`-sys` native-build protocol, or Cargo `build.rs` conventions.** A package's generated
  code comes from `build.bd` (`spec/build-scripts.md`) or from `burdin bindgen`; there is no other native build
  hook.
- **No registry client in the compiler.** Dependencies come from a path, a git repository, or the packages bundled
  with the compiler (`spec/packages.md`). The registry and its authentication are later issues (#252, #361).
- **No arbitrary installation.** `burdin install`-style placement of third-party binaries is issue #348; `new` and
  `init` create a package and nothing else.
- **No configuration layered from the environment beyond the documented variables.** The build tool reads
  `burdin.toml` and the environment listed in BT5; there is no `cargo config`-style file or `RUSTFLAGS`
  equivalent.
- **No formatting configuration.** `burdin fmt` has one style and no options (`spec/fmt.md`).
- **No `--manifest-path` on build commands.** The target is the positional argument; `burdin add` is the one
  command with `--manifest-path`, because it edits a file (`spec/packages.md`).

## BT4. The command line

- `burdin` with no arguments, `burdin --help`, and `burdin help` print the command list and exit 0.
- `burdin help <command>` and `burdin <command> --help` print the command's page and exit 0 (`burdin help help`, like
  `burdin help`, prints the command list). `--help` or `-h` after `--` belongs to the program being run, not to
  `burdin`.
- An unknown command, an unknown option, and a missing required argument are usage errors: a message, the
  command's usage line, and ``For more information, try `burdin <command> --help`.``. The exit status is 2.
- `burdin -V`/`--version` prints the version, channel, target, LLVM version, commit, and build profile, and exits
  0. It takes no arguments.
- The options of a command are documented only on that command's page; the root page lists commands and their
  one-line summaries.
- A command that has a target takes it as a positional argument, before any options and before `--`. `--` ends
  `burdin`'s options: `run` and `debug` pass what follows to the program, `fuzz` passes it to libFuzzer.
- Long options accept `--option value` and `--option=value`. A flag that takes no value rejects `--flag=value`.
  `-j4` is `-j 4`.

## BT5. Environment variables

The variables the tool reads are listed on the root help page: `BURDIN_CC`, `BURDIN_JOBS`, `BURDIN_HOME`,
`BURDIN_CODEGEN_UNITS`, `LLVM_SYS_211_PREFIX`, `COLUMNS`, `FORCE_HYPERLINK`, and `NO_COLOR`. The build-script
environment adds `BURDIN`, `BURDIN_MANIFEST_DIR`, `BURDIN_OUT_DIR`, and `BURDIN_PROFILE`
(`spec/build-scripts.md`, BS3); the release build adds `BURDIN_VERSION` and `BURDIN_CHANNEL`
(`docs/releasing.md`).

## BT6. Exit statuses

| Status | Meaning |
| --- | --- |
| 0 | success |
| 1 | a compile, check, test, lint, doc, or package error; a dependency that cannot be resolved; a manifest or profile error |
| 2 | a usage error, an invalid option value, an unreadable build output directory, or a refused operation such as `burdin new` on an existing path |
| the program's | `burdin run` and `burdin debug` pass the program's exit status through; a signal is `128 + signal` on Unix |
| 101 | the compiler itself panicked (a bug) |

## BT7. One flow through the build commands

`build`, `run`, `test`, and `check` share one front end and one set of diagnostics; they differ only in what they
do after checking (`spec/check.md`, CK1). A package that passes `check` gets as far as code generation in `build`,
and `test` runs the same checked program's test dispatcher. `tests/check_cli.py` builds a fixture package, runs
it, checks it, and tests it in one flow, and checks that the unknown-command and out-of-scope-command behavior of
BT2 holds.

## BT8. Quiet and verbose output

`build`, `run`, and `check` take two flags for what they print besides their results (issue #799):

- `-q`/`--quiet` prints only errors. Warnings and informational notes (such as `note: skipped` for a workspace member
  with no `src/main.bd`) are left out, in every diagnostic format. Errors print as they do without the flag, and a
  SARIF log is still written under `build` and `check`, holding only the errors. A result a command asks for is
  unchanged: a program's output under `run`, and `check --print-instances` or `--explain-ownership`.
- `-v`/`--verbose` prints, to standard error, `compiling <target>`, then each phase of the compilation with its
  wall time, in this order: `dependencies` (the lockfile and git packages), `load` (the package and its modules),
  `build scripts`, `parse and check` (parsing, name resolution, type checking, and the memory checks), and, for
  `build` and `run` only, `codegen` followed by one `codegen unit <i> of <n>` line per unit, in unit order, and
  `link`. `check` stops after `parse and check`. A fresh cached executable replaces `codegen` and `link` with
  `phase cache: the executable is up to date, so codegen and link are skipped`. A failed compilation prints the
  phases that finished, then its errors. The program's output and exit status are unchanged. The unit lines do not
  depend on `-j`: the units are numbered by the split, not by the order their threads finish.
- The two flags conflict (exit status 2). `-q` is valid only for `build`, `run`, and `check`; `-v` is also the test
  and bench runners' flag for their own output, and `burdin fuzz` rejects it. The timings vary from run to run, so
  the checks compare the phase names and the unit numbers, and golden text only for the quiet error output.

The flags change only what is printed: `--error-limit`, `--color`, `--width`, and `--diagnostic-format` keep their
meaning under both.

## Tests

`tests/check_cli.py` checks:

- every command in BT1 is listed by `burdin --help`, and `burdin help <command>` and `burdin <command> --help`
  exit 0 and name the command;
- the root help is the golden command list (summaries and order), and a representative command page is golden;
- an unknown command exits 2 with `unknown command` and a suggestion for a near miss;
- every out-of-scope command in BT2 is rejected as unknown with exit status 2;
- `burdin -V` and `burdin --version build` behave as BT4 says;
- one fixture package goes through `build`, `run`, `check`, and `test` (BT7).

`tests/check_verbosity.py` checks BT8 with the fixture packages under `tests/verbosity/`: the quiet output of a
warning-only package and of a failing one (golden text under `tests/verbosity/golden/`), in every format; the phases
and the codegen units of `-v` with `-j 1` and `-j 8`; the cached-executable line; and the usage errors.
