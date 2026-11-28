# Burdin

Burdin is a native-compiled systems programming language with compile-time memory safety, no garbage collector, and value semantics. The compiler is written in Rust and targets LLVM 21.

- **Memory safety without a GC:** Deterministic ownership, move semantics, parameter conventions, and lexical borrow checking enforced at compile time.
- **Native performance:** Compiles directly to native executables via LLVM 21, with support for link-time optimization (LTO), profile-guided optimization (PGO), and sanitizers.
- **Integrated toolchain:** A single `burdin` driver provides building, running, testing, benchmarking, fuzzing, formatting, linting, documentation generation, C/Rust FFI bindgen, a REPL, an LSP server, and a DAP debug adapter.
- **Concurrency:** Lightweight tasks on a work-stealing scheduler, typed channels with `select`, and asynchronous I/O without function coloring.

## Quick start

Write `hello.bd`:

<!-- check-docs: stdout=hello, world -->
```burdin
fn main() {
    println("hello, world")
}
```
<!-- /check-docs -->

Compile and run it:

```console
$ ./target/debug/burdin run hello.bd
hello, world
```

The example above is verified by `tests/check_docs.py`.

## Installing a release

Prebuilt binaries are published for six targets: Linux, macOS, and Windows on x86-64 and arm64.

On Unix (Linux and macOS):

```console
$ curl -fsSL https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.sh | sh
```

On Windows PowerShell:

```console
> irm https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.ps1 | iex
```

Both `install.sh` and `install.ps1` verify the release `SHA256SUMS` before unpacking, install into `$HOME/.local/bin` (`%LOCALAPPDATA%\Programs\burdin\bin` on Windows) alongside the official `packages/` directory, and add the directory to `PATH` (pass `BURDIN_NO_MODIFY_PATH=1` or `-NoModifyPath` to skip modifying `PATH`).

Installer environment variables:
- `BURDIN_VERSION`: pin a specific release version.
- `BURDIN_CHANNEL`: select `stable` (default) or `nightly`.
- `BURDIN_INSTALL_DIR`: override the installation directory.

Prebuilt compilers install through `burdin toolchain`, which acts as a proxy for managing multiple toolchains (`stable`, `nightly`, or pinned versions via `burdin-toolchain.toml`) and optional components (`lsp`, `std-src`, `docs`, `debuggers`). See [`spec/toolchain.md`](spec/toolchain.md) and [`docs/releasing.md`](docs/releasing.md) for details.

## Building from source

### Requirements

- **Rust:** Pinned in [`rust-toolchain.toml`](rust-toolchain.toml) (installed via `rustup`, including `clippy` and `rustfmt`).
- **LLVM 21:** Development libraries and headers, discovered via `LLVM_SYS_211_PREFIX` or `llvm-config`.
  - On Ubuntu: `build-essential llvm-21-dev libpolly-21-dev clang-21 libzstd-dev zlib1g-dev`.
- **C linker:** `cc` on Linux/macOS or MSVC `cl` on Windows (override with `BURDIN_CC`).

Build the compiler driver:

```console
$ cargo build
```

This produces `target/debug/burdin` (or `target\debug\burdin.exe` on Windows).

### Dependencies and platform support

The compiler has minimal direct Rust dependencies (`inkwell`, `libc`, and `clap`) with exact version pins in [`Cargo.lock`](Cargo.lock). All transitive crates are audited for permissive licenses in [`tests/dependencies.txt`](tests/dependencies.txt) and verified by `tests/check_deps.py`. See [`docs/dependencies.md`](docs/dependencies.md) and [`docs/platform-support.md`](docs/platform-support.md) for the dependency policy and tested platform matrix.

## Repository overview

| Path | Description |
| --- | --- |
| [`crates/`](crates/) | Rust workspace implementing the compiler (`burdin_syntax`, `burdin_tir`, `burdin_sema`, `burdin_check`, `burdin_interp`, `burdin_codegen`, `burdin_driver`), tooling (`burdin_fmt`, `burdin_lint`, `burdin_doc`, `burdin_refactor`, `burdin_lsp`, `burdin_dap`, `burdin_repl`, `burdin_pkg`, `burdin_toolchain`, `burdin_bindgen`, `burdin_inspect`), and fuzz/bench harnesses. |
| [`std/`](std/README.md) | The Burdin standard library, written in Burdin. |
| [`packages/`](packages/README.md) | Official packages bundled with the compiler (`json`, `toml`, `csv`, `xml`, `yaml`, `regex`, `crypto`, `math`, `http`, `tls`). |
| [`spec/`](spec/) | Language and tooling specifications (grammar, memory rules, stdlib, concurrency, CLI, diagnostics, and packages). |
| [`tests/`](tests/README.md) | Conformance test suite and verification scripts for every compiler phase and tool. |
| [`docs/`](docs/) | User-facing reference documentation: error codes, lints, platform support, dependencies, diagnostics, and releasing. |
| [`website/`](website/) | Source for the Burdin documentation website (Astro / Starlight). |
| [`editors/`](editors/) | Editor integrations for VS Code, Neovim, and Helix. |
| [`debuggers/`](debuggers/) | Pretty-printers for GDB, LLDB, and Windows Natvis. |

## Documentation index

### User & reference docs
- [`tests/README.md`](tests/README.md) — Conformance suite format, error-code registry, and settled language decisions.
- [`docs/error-codes.md`](docs/error-codes.md) — Reference for every compiler error code (`burdin explain <code>`).
- [`docs/lints.md`](docs/lints.md) — Reference for every `burdin lint` rule (`burdin lint --explain <lint>`).
- [`docs/platform-support.md`](docs/platform-support.md) — Supported targets, CI matrix, and known limitations.
- [`docs/dependencies.md`](docs/dependencies.md) — Dependency and license policy.
- [`docs/releasing.md`](docs/releasing.md) — Versioning, release channels, reproducibility, and release process.
- [`docs/diagnostics.md`](docs/diagnostics.md) — Machine-readable JSON and SARIF diagnostic formats ([schema](docs/diagnostic.schema.json)).
- [`std/README.md`](std/README.md) — Standard library overview.
- [`packages/README.md`](packages/README.md) — Official packages overview.

### Language & tooling specifications
- [`spec/grammar.md`](spec/grammar.md) — Language syntax.
- [`spec/memory-rules.md`](spec/memory-rules.md) — Ownership, moves, conventions, exclusivity, references, and drops.
- [`spec/stdlib.md`](spec/stdlib.md) — Standard library surface.
- [`spec/errors.md`](spec/errors.md) — Error handling (`error` declarations, `Result<T>`, `context`, traces).
- [`spec/iterators.md`](spec/iterators.md) — Iteration protocol, adapters, and ranges.
- [`spec/format.md`](spec/format.md) — String interpolation, `Display`, and `Debug`.
- [`spec/derives.md`](spec/derives.md) — `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default`.
- [`spec/existentials.md`](spec/existentials.md) — `any` and `impl` existential types.
- [`spec/arguments.md`](spec/arguments.md) — Default and named arguments.
- [`spec/extensions.md`](spec/extensions.md) — Extension methods.
- [`spec/operators.md`](spec/operators.md) — Operator overloading (`Index`, `IndexMut`).
- [`spec/const-eval.md`](spec/const-eval.md) — Compile-time evaluation, `const`, and `embed`.
- [`spec/allocators.md`](spec/allocators.md) — Allocators, `Arena`, and `Shared<T>`.
- [`spec/tasks.md`](spec/tasks.md), [`spec/channels.md`](spec/channels.md), [`spec/shared.md`](spec/shared.md), [`spec/io.md`](spec/io.md) — Tasks, channels, `select`, shared state, and asynchronous I/O.
- [`spec/packages.md`](spec/packages.md), [`spec/workspaces.md`](spec/workspaces.md), [`spec/build-scripts.md`](spec/build-scripts.md) — Packages, lockfiles, features, workspaces, and `build.bd`.
- [`spec/bindgen.md`](spec/bindgen.md), [`spec/rust-interop.md`](spec/rust-interop.md) — C and Rust FFI binding generation.
- [`spec/build-tool.md`](spec/build-tool.md), [`spec/profiles.md`](spec/profiles.md), [`spec/toolchain.md`](spec/toolchain.md) — CLI surface, build profiles, and toolchain management.
- [`spec/check.md`](spec/check.md), [`spec/fmt.md`](spec/fmt.md), [`spec/lint.md`](spec/lint.md), [`spec/fix.md`](spec/fix.md), [`spec/refactor.md`](spec/refactor.md), [`spec/doc.md`](spec/doc.md) — Static checking, formatting, linting, migrations, refactoring, and doc generation.
- [`spec/test-runner.md`](spec/test-runner.md), [`spec/bench.md`](spec/bench.md), [`spec/fuzz.md`](spec/fuzz.md), [`spec/sanitizers.md`](spec/sanitizers.md) — Testing, benchmarking, fuzzing, and sanitizers.
- [`spec/watch.md`](spec/watch.md), [`spec/inspect.md`](spec/inspect.md), [`spec/debug.md`](spec/debug.md), [`spec/lsp.md`](spec/lsp.md) — File watching, IR/assembly inspection, debugging, and LSP.
- [`spec/stage0-subset.md`](spec/stage0-subset.md), [`spec/stage1-subset.md`](spec/stage1-subset.md) — Language milestones and roadmap.
- [`AGENTS.md`](AGENTS.md), [`CLAUDE.md`](CLAUDE.md) — Contributor and agent guidelines.

## The driver contract

The `burdin` CLI provides `build`, `run`, `test`, `bench`, `fuzz`, `check`, `watch`, `fmt`, `doc`, `debug`, `explain`, `bindgen`, `new`, `init`, `repl`, `lsp`, `dap`, `expand`, `asm`, `bloat`, `add`, and `toolchain`. Run `burdin --help` or `burdin <command> --help` for full option details.

```
usage:
  burdin build [file.bd | package directory] [--release | --profile <name>] [-g] [--strip] [--lto] [--tests] [-j <n>]
               [--pgo-generate | --pgo-use <profile>] [-o <output>] [--emit-ir <file.ll>] [--emit <kinds>]
               [-p <member>]... [--workspace [--exclude <member>]...] [--link-lib <name>]...
               [--const-steps <n>] [--emit-deps <file>] [--sanitize <list>] [diagnostics]
  burdin run [file.bd | package directory] [--release | --profile <name>] [-g] [-j <n>] [--link-lib <name>]...
             [-p <member>]... [--workspace [--exclude <member>]...] [--const-steps <n>] [--sanitize <list>]
             [diagnostics] [-- <program arguments>...]
  burdin test [file.bd | package directory] [<filter>...] [--release | --profile <name>] [-g] [-j <n>] [--verbose]
              [-p <package>]... [--workspace [--exclude <member>]...] [--exact] [--skip <pattern>]...
              [--ignored | --include-ignored] [--failed] [--shard <i>/<n>] [--timeout <ms>] [--slow-threshold <ms>]
              [--list] [--format <human|json>] [--junit <file>] [--fuzz-replay] [--bless | --review] [--interpret]
              [--sanitize <list>] [--const-steps <n>] [diagnostics]
  burdin bench <file.bd | package directory> [<filter>...] [--release | --profile <name>] [-g] [-j <n>] [--verbose]
               [-p <package>]... [--workspace [--exclude <member>]...] [--exact] [--skip <pattern>]... [--list]
               [--format <human|json>] [--timeout <ms>] [--warmup <ms>] [--sample-count <n>] [--sample-time <ms>]
               [--save-baseline <name>] [--baseline <name>] [--fail-on-regression <percent>]
               [--profile-time <seconds>] [--const-steps <n>] [diagnostics]
  burdin fuzz <file.bd | package directory> <name> [--max-time <seconds>] [--minimize <file>]
              [--release | --profile <name>] [-g] [-j <n>] [--const-steps <n>] [diagnostics] [-- <libFuzzer options>...]
  burdin check <file.bd | package directory> [--release | --profile <name>] [--tests] [-j <n>] [--const-steps <n>]
               [--explain-ownership [--fn <name>] [--format <text|json>]] [--explain] [--print-instances] [diagnostics]
  burdin watch [check|build|test|run] <file.bd | package directory> [options] [--no-clear]
  burdin fmt [<file.bd | directory>]... [--check] [-j <n>] [diagnostics]
  burdin doc <file.bd | package directory> [-o <dir | file>] [--output-format html|json] [--private]
             [--coverage] [--deny-warnings] [-j <n>] [diagnostics]
  burdin debug <file.bd | package directory> [--debugger <name | path>] [--test <name | index>]
               [--break-on-panic] [--release | --profile <name>] [-j <n>] [diagnostics] [-- <program arguments>...]
  burdin explain [<error code>]
  burdin bindgen <header> --out <file> [-I <dir>]... [-D <name[=value]>]... [--clang-arg <arg>]...
                 [--wrappers <file.c>] [--wrapper-include <text>] [diagnostics]
  burdin bindgen --export <target> --out <header.h> [--guard <NAME>] [diagnostics]
  burdin bindgen --rust <crate> --out <file> --shim <dir> [--rust-toolchain <version>] [--rustdoc-json <file>]
  burdin new <name> [--lib | --bin]
  burdin init [<dir>] [--lib | --bin]
  burdin repl [--release] [-j <n>] [--const-steps <n>] [diagnostics]
  burdin lsp [--stdio]
  burdin dap [--stdio]
  burdin --version
```

### `burdin test`

Runs `test` blocks and doc tests in parallel using the `test` profile. Supports filtering (`<filter>...`, `--exact`, `--skip`), sharding (`--shard <i>/<n>`), JUnit XML (`--junit`), JSON streaming (`--format json`), and snapshot review (`--bless`, `--review`). See [`spec/test-runner.md`](spec/test-runner.md).

### `burdin bench`

Measures `bench` items in the `release` profile with warmup, calibration, and statistical comparison against saved baselines (`--save-baseline`, `--baseline`, `--fail-on-regression`). See [`spec/bench.md`](spec/bench.md).

### `burdin fmt`

Formats Burdin source files to the canonical 4-space, 120-column style. Supports `--check` for CI, `--stdin` with `--stdin-filepath`, and `--range` for editor integration. See [`spec/fmt.md`](spec/fmt.md).

### Version metadata

`burdin --version` prints six metadata lines, including the release channel (`channel: dev`, `nightly`, or `stable`), target triple, LLVM version, commit hash, and build profile. `BURDIN_VERSION`, `BURDIN_CHANNEL`, and `BURDIN_COMMIT` can override the embedded metadata at build time.

### Diagnostics and machine-readable output

All compilation and checking commands accept `--diagnostic-format <human|short|json|sarif>`, `--color <auto|always|never>`, `--no-color`, `--width <columns>`, and `--error-limit <n>`.

`burdin check <target> --diagnostic-format json` emits one JSON object per diagnostic ([`docs/diagnostics.md`](docs/diagnostics.md)) and ends with a `check-summary` record:

```json
{"type":"check-summary","version":1,"errors":0,"warnings":0,"shown":0,"limit":null,"files":1,"elapsed_ms":612}
```

### Exit status

| Status | Meaning |
| --- | --- |
| `0` | `build` succeeded; `run` exited 0; every test passed. |
| `1` | Diagnostics were reported, a source file or manifest could not be read, or one or more tests failed. |
| `2` | Usage error, or a driver failure after loading (such as a codegen or linker failure). |
| `<program>` | `run` forwards the executed program's exit status (128+signal on Unix). |

## Verification and testing

Run the test and verification suites from the repository root:

```console
$ cargo test --workspace
$ python3 tests/run_tests.py
$ python3 tests/check_deps.py
$ python3 tests/check_docs.py
$ python3 tests/check_release.py
```

- `tests/check_docs.py` verifies all relative Markdown links and heading anchors, compiles and runs the `check-docs` example above, and checks the driver contract.
- `tests/check_ci.py` validates the CI workflow, pinned Rust toolchain, and test script invariants.
- `tests/fuzz.py --seconds 60` runs a local lexer/parser fuzzing session against [`crates/burdin_fuzz/corpus/syntax/`](crates/burdin_fuzz/corpus/syntax/README.md).
