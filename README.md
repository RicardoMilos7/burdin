# Burdin

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. It is not for production. The compiler is written in Rust and stays in Rust:
stage 0 is the core subset, and stage 1 adds the rest of the core language to the same Rust compiler. Burdin is not
bootstrapped into itself.

This file is the entry point for building the compiler and using the `burdin` driver. Behavior is defined by the
conformance suite and the specs, not by this file; see the index below.

## Documentation index

| Document | Contents |
| --- | --- |
| [`tests/README.md`](tests/README.md) | The conformance suite: test format, error-code registry, and the settled language decisions. Read this first. |
| [`docs/error-codes.md`](docs/error-codes.md) | The user-facing reference for every error code, with a checked example and a fix for each. `burdin explain <code>` prints one entry. |
| [`spec/grammar.md`](spec/grammar.md) | The syntax of stage 0. |
| [`spec/memory-rules.md`](spec/memory-rules.md) | The exact semantics of moves, conventions, exclusivity, references, and drops. |
| [`spec/stage0-subset.md`](spec/stage0-subset.md) | What stage 0 must support, what it must not, and its milestones. Complete. |
| [`spec/stage1-subset.md`](spec/stage1-subset.md) | The stage 1 plan and the roadmaps for later stages. |
| [`AGENTS.md`](AGENTS.md) | The working agreement: definition of done, verification policy, and issue workflow. |
| [`CLAUDE.md`](CLAUDE.md) | The Claude-specific entry point and working rules. |
| [`HANDOFF.md`](HANDOFF.md) | The current state, open decisions, and what is next. |
| [`CHANGELOG.md`](CHANGELOG.md) | Shipped milestones. |
| [`crates/burdin_fuzz/`](crates/burdin_fuzz/) | The lexer/parser fuzzer and its committed corpus. |
| [`std/README.md`](std/README.md) | The stage 0 standard library. |
| [`Burdin Language Design.md`](Burdin%20Language%20Design.md) | The full language design, including features after stage 0. |

## Building the compiler

Requirements:

- the Rust toolchain pinned by `rust-toolchain.toml` (currently `1.100.0`, edition 2024), installed with `rustup` so the
  `clippy` and `rustfmt` components are available;
- LLVM 21 with its development files; `inkwell` 0.11 uses the `llvm21-1` feature and finds LLVM through
  `LLVM_SYS_211_PREFIX` or a matching `llvm-config`;
- a system C compiler driver to link against libc: `cc` on Linux and macOS, MSVC `cl` on Windows.

On Ubuntu, the packages are `build-essential llvm-21-dev libpolly-21-dev clang-21 libzstd-dev zlib1g-dev`. Development
happens in WSL Ubuntu first; `CLAUDE.md` has the exact setup and the PowerShell wrapper. Build from the repository root:

```console
$ cargo build
```

The driver is then `target/debug/burdin` (or `target\debug\burdin.exe`). The examples below use that path; if the binary
is on `PATH`, drop the prefix.

## Hello, world

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

The program above and this command are checked by `tests/check-docs.sh`. Other command transcripts in this file are
illustrative; the behavior they show is covered by the conformance tests and by that script.

## The driver contract

The `burdin` command has the subcommands `build`, `run`, `test`, and `explain`. This section is the contract they
implement; the conformance runner (`tests/run-tests.sh`) and the documentation check depend on it.

```
usage:
  burdin build <file.bd | package directory> [--release] [-o <output>] [--emit-ir <file.ll>]
               [--link-lib <name>]... [diagnostics]
  burdin run <file.bd | package directory> [--release] [--link-lib <name>]... [diagnostics]
             [-- <program arguments>...]
  burdin test <file.bd | package directory> [--release] [diagnostics]
  burdin explain [<error code>]

diagnostics:
  --diagnostic-format <human|short|json>  human (default) colors and wraps; short prints one plain line per
                                          diagnostic; json prints one JSON object per line
  --color <auto|always|never>             color (default auto: only when stderr is a terminal)
  --no-color                              same as --color never
  --width <columns>                       wrap human diagnostics at this width (0 disables wrapping)
  --error-limit <n>                       print at most n errors after sorting (0 prints all)
```

Long options also accept `--option=value`.

A target is either a single `.bd` file, compiled as a one-module package, or a directory containing a `burdin.toml`.

### Targets and packages

A package directory is described by a minimal `burdin.toml`:

```toml
[package]
name = "app"

[dependencies]
lib = { path = "../lib" }
```

- `[package] name` is required and must be a Burdin identifier. It names the executable and the package in `use`
  paths.
- Every `.bd` file under `src/` is a module named by its path: `src/geometry/shapes.bd` is `geometry.shapes`.
- The entry point is `src/main.bd`. A dependency is reached as `use <package>`, which names its `src/<package>.bd`, or
  `src/lib.bd` when that file does not exist.
- `[dependencies]` entries are path-only and are loaded recursively. A diamond reuses the package; a cycle is an
  error; two packages may not share a name.
- Unknown sections and keys are ignored, so a newer manifest does not break an older compiler.

A single `.bd` file is a package with one module (its module path is empty) and no manifest.

### `burdin build`

```
burdin build <target> [--release] [-o <output>] [--emit-ir <file.ll>] [--link-lib <name>]... [diagnostics]
```

Compiles the target to a native executable. On success it prints nothing and exits 0.

- Without `-o`, the executable is written to the current directory as `<stem>` plus the platform suffix (`.exe` on
  Windows). `<stem>` is the file stem for a `.bd` target and the package name for a package directory. `-o` names the
  output file exactly; an existing directory is an error, so building a package from its parent when the directory and
  the package share a name needs `-o`.
- `--release` enables optimization (`-O2` through LLVM). Debug builds panic on overflow of `+`, `-`, `*`, and
  `MIN / -1`; release builds wrap. Division by zero always panics. This is language behavior, not only an
  optimization choice.
- `--emit-ir <file.ll>` also writes the module's textual LLVM IR to that path, as generated and before the release
  optimization passes.

```console
$ ./target/debug/burdin build hello.bd
$ ./hello
hello, world
$ ./target/debug/burdin build hello.bd -o hello_copy --emit-ir hello.ll
$ head -1 hello.ll
; ModuleID = 'hello'
```

### `burdin run`

```
burdin run <target> [--release] [--link-lib <name>]... [diagnostics] [-- <program arguments>...]
```

Compiles into a temporary directory and runs the program. The program's stdout and stderr pass through unchanged, and
the command exits with the program's exit status. On Unix a signal becomes `128 + signal`; Windows statuses are
truncated to 8 bits, with a non-zero status kept non-zero. A compilation failure with diagnostics exits 1. Arguments
after `--` become the program's command-line arguments after the program name; `env.args()` returns them with the
program name first.

```console
$ ./target/debug/burdin run args.bd -- x y
3
```

### `burdin test`

```
burdin test <target> [--release] [diagnostics]
```

Builds the entry package in test mode and runs every `test "name" { ... }` block, each in its own process, so a panic
or abort fails only the running test. Tests are stripped from a normal build: `burdin build` and `burdin run` never
declare or check them. Test mode also replaces the prelude's `assert` and `assert_eq` with versions that return
`Result<(), AssertError>` instead of panicking, so `assert(...)?` reports a failed test and a helper can propagate it.
A `main` function is not required in test mode. Only the entry package's tests run, not its dependencies'.

The output is one `FAIL <test name>` line on stdout per failed test, followed by a summary line:

```console
$ ./target/debug/burdin test .
FAIL second one fails
test result: 2 passed, 1 failed
```

The exit status is 0 only when every test passes, and 1 when any test fails or compilation fails. `tests/run-tests.sh`
reads exactly this format: the `FAIL ` prefix and the `test result: N passed, M failed` summary.

### Diagnostics

Compilation diagnostics go to stderr. The default (`human`) format starts each diagnostic on its own line:

```
path:LINE:COL: error[E0003]: expected `i32`, found `str`
```

`LINE` and `COL` are 1-based; `COL` counts characters, not bytes. A diagnostic may add related notes that point at
another span and a suggested fix:

```
bad.bd:10:9: error[E0114]: this parameter of `bump` is `mut`; pass the argument as `mut ...`
  = help: add `mut` before this argument
bad.bd:12:9: error[E0100]: use of moved value `t`
  = note: bad.bd:11:10: value moved here
```

**Ordering and completeness.** Diagnostics are collected while checking and printed once, sorted by file (in
package load order) and byte offset; ties are broken by code, message, and the order the checks found them. The
output is byte-for-byte deterministic, and it is never streamed: sorting needs the whole list. Every error is
reported by default; the semantic and memory checks keep going after a failure so one run shows as many errors as
they can find. `--error-limit <n>` prints the first `n` diagnostics after sorting and adds a
`note: <hidden> more errors not shown (--error-limit <n>)` line; the summary still counts every error. `0` (the
default) means no limit, so the "report every error" rule is unchanged unless a limit is asked for.

After the diagnostics the driver prints a summary line and exits with status 1:

```
error: could not compile bad.bd (1 error)
```

The count is pluralized (`1 error`, `2 errors`). Semantic checks run only when there were no syntax errors, so a file
with a syntax error may report fewer later diagnostics. Errors before or outside source checking (a malformed
manifest, a missing dependency, a dependency cycle, a missing target) are printed as `error: <message>` without an
error code.

**Color and width.** `--color auto|always|never` (and `--no-color`, the same as `never`) colors the location, the
severity, and the code. `auto` is the default and colors only when stderr is a terminal; a non-empty `NO_COLOR`
disables color for `auto`, while an explicit `always` or `never` wins over it, and the last color option wins.
`--width <n>` wraps the human format to `n` columns (`0` disables wrapping); without it the driver uses `COLUMNS`,
then 80. Wrapping only inserts line breaks and indentation, and the first line always keeps the
`path:LINE:COL: error[E0000]: message` shape.

**Formats.** `--diagnostic-format short` prints exactly one plain line per diagnostic, with no notes,
suggestions, color, or wrapping; it is the stable format the test scripts parse. `--diagnostic-format json` prints
one JSON object per line, and the last line is a summary object:

```json
{"type":"diagnostic","severity":"error","code":"E0114","span":{"file":"bad.bd","line":10,"column":9,"end_line":10,"end_column":10,"byte_start":140,"byte_end":141},"message":"this parameter of `bump` is `mut`; pass the argument as `mut ...`","notes":[],"suggestions":[{"message":"add `mut` before this argument","span":{"file":"bad.bd","line":10,"column":9,"end_line":10,"end_column":9,"byte_start":140,"byte_end":140},"replacement":"mut "}]}
{"type":"summary","errors":1,"shown":1,"limit":null}
```

Each diagnostic carries its primary `span`, `notes` (a message and an optional span), and `suggestions` (a
message, a span, and the replacement text). A suggestion with `byte_start == byte_end` inserts its replacement;
an empty replacement deletes the span. Together they are what an editor turns into a diagnostic and a quick-fix
code action. JSON and short output are never colored or wrapped. `--color` and `--width` only affect `human`.

The error codes come from the registry in [`tests/README.md`](tests/README.md#error-codes). `E9000` is the temporary
code for "not implemented in this compiler yet"; the conformance suite does not use it.

### `burdin explain` and the error-code reference

[`docs/error-codes.md`](docs/error-codes.md) explains every registry code, with a program that triggers it and how
to fix it. The page is checked in CI: `tests/check-error-docs.sh` compiles every snippet and requires exactly the
marked diagnostics, and the driver embeds the page, so the two cannot drift.

```
burdin explain E0003      print the reference entry for E0003
burdin explain            list every code
```

`burdin explain` writes to stdout and exits 0; an unknown code exits 2. The human diagnostics end with a note
pointing at the command.

### Emitting LLVM IR

`--emit-ir <file.ll>` also writes the textual LLVM IR, both for inspecting the backend and for debugging code
generation:

```console
$ ./target/debug/burdin build hello.bd --emit-ir hello.ll
$ grep -m1 'define.*@burdin.main' hello.ll
define internal void @burdin.main() {
```

The file is written before the release optimization passes, so `--release --emit-ir` shows pre-optimization IR. The
IR is the module that is handed to LLVM for the object file; it is not a stable format.

### Linking with `--link-lib`

`--link-lib <name>` adds one library to the link step. It may repeat.

- On Linux and macOS the driver passes `-l<name>` to the C compiler driver.
- With MSVC (`cl`) it passes `<name>.lib`.

`LLVM-C` is the one special name. On Ubuntu the LLVM C API is exported by `libLLVM-21.so`, not by a `libLLVM-C.so`, so
the driver resolves the library through `llvm-config` instead of leaving `-lLLVM-C` to the linker:

1. It looks for `llvm-config` first in `$LLVM_SYS_211_PREFIX/bin/`, then on `PATH` as `llvm-config`,
   `llvm-config-21`, `llvm-config-20`, and so on down to `llvm-config-17`.
2. It asks that tool for `--libdir`, then prefers, in that directory, `libLLVM-C.so`, `libLLVM.so`, `libLLVM-C.dylib`,
   or `libLLVM.dylib`.
3. Failing that, it uses the single `-l` flag from `llvm-config --link-shared --libs`, then a versioned
   `libLLVM-<version>.so` or `.dylib`.
4. It adds `-L<libdir>` and an rpath for that directory, so the executable finds the shared library at run time.

All other names go to the linker unchanged. The m11 conformance test
([`tests/m11_llvm/llvm_return_42.bd`](tests/m11_llvm/llvm_return_42.bd)) exercises `--link-lib LLVM-C`; the resolution
order is also covered by unit tests in `burdin_driver`.

```console
$ ./target/debug/burdin run tests/m11_llvm/llvm_return_42.bd --link-lib LLVM-C
module verified
found ret
```

### Choosing the C compiler

The compiler emits an object file and leaves linking to libc to the system C compiler driver, which is `cc` on Linux
and macOS and `cl` on Windows. `BURDIN_CC` overrides it:

```console
$ BURDIN_CC=clang ./target/debug/burdin build hello.bd
```

An MSVC-style driver is recognized when the name ends in `cl` or `cl.exe`; the driver then passes MSVC arguments,
names the static CRT libraries, and appends `.lib` to `--link-lib` names. If the driver cannot be started, the error
names the compiler and suggests `BURDIN_CC`:

```
error: cannot run the linker `cc`: No such file or directory (os error 2) (set BURDIN_CC)
```

### Exit status

| Status | Meaning |
| --- | --- |
| 0 | `build` succeeded; `run` exited 0; every test passed. |
| 1 | Diagnostics were reported, a source file or manifest could not be read, or one or more tests failed. |
| 2 | A usage error, or a driver failure after loading: the output path is a directory, code generation failed, the linker could not be started or failed, or a temporary directory could not be created. |
| program status | `run` exits with the program's status; 128+signal on Unix. |

Usage errors print `error: <message>` followed by the usage block.

## Fuzzing the lexer and parser

`crates/burdin_fuzz` lexes and parses a committed corpus and deterministic mutations of it. `cargo test --workspace`
replays the corpus in every CI job, so a panic reintroduced by a change fails CI. The scheduled `fuzz` job runs a
longer session through `tests/fuzz.sh`; a found input is written under `fuzz-out/crashes` and is committed under
`crates/burdin_fuzz/corpus/syntax/` together with its fix. See
[`crates/burdin_fuzz/corpus/syntax/README.md`](crates/burdin_fuzz/corpus/syntax/README.md).

```console
$ tests/fuzz.sh --seconds 60
```

## Checking the documentation

`tests/check-docs.sh` verifies that the links in the repository's Markdown files resolve (files and heading anchors),
that the checked example above still compiles and prints the shown output, and that the documented driver behavior
holds: `build` output naming, `-o`, `--emit-ir`, `run` exit-status passthrough, `-- args`, the `test` summary format,
the diagnostic format, `--link-lib` (including `LLVM-C` where LLVM is available), and `BURDIN_CC`. Run it after
building the driver:

```console
$ tests/check-docs.sh
```

Two checks cover the diagnostics themselves, and every CI job runs them after the build:

- `tests/check-diagnostics.sh` checks the default and short and JSON formats, color and `NO_COLOR`, wrapping and
  `--width`, notes and suggestions, `--error-limit`, and deterministic ordering.
- `tests/check-error-docs.sh` checks that [`docs/error-codes.md`](docs/error-codes.md) mirrors the registry, that
  every snippet fails with exactly the marked diagnostics, and that `burdin explain` prints every entry.
