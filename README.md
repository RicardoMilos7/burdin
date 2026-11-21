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
| [`docs/lints.md`](docs/lints.md) | The reference for every `burdin lint` lint, with a checked example and a fix for each. `burdin lint --explain <lint>` prints one entry. |
| [`docs/dependencies.md`](docs/dependencies.md) | The dependency and license policy, the review of every crate, and how to add one. `tests/check_deps.py` enforces it. |
| [`docs/platform-support.md`](docs/platform-support.md) | The tested OS/architecture pairs, the runtime features each CI job proves, and the known limitations. |
| [`docs/releasing.md`](docs/releasing.md) | The version scheme, release channels, artifact and checksum protocol, reproducibility rules, and the rehearsal checklist. |
| [`spec/grammar.md`](spec/grammar.md) | The syntax of stage 0. |
| [`spec/memory-rules.md`](spec/memory-rules.md) | The exact semantics of moves, conventions, exclusivity, references, and drops. |
| [`spec/stage0-subset.md`](spec/stage0-subset.md) | What stage 0 must support, what it must not, and its milestones. Complete. |
| [`spec/stage1-subset.md`](spec/stage1-subset.md) | The stage 1 plan and the roadmaps for later stages. |
| [`spec/build-tool.md`](spec/build-tool.md) | The build tool's command surface: every command, the Cargo commands it does not copy and their issues, the command line and environment rules, and the exit statuses. |
| [`spec/profiles.md`](spec/profiles.md) | Named build profiles (`dev`, `release`, `test`, `bench`, and custom), inheritance, `opt-level`, debug information, assertions, overflow checks, the panic strategy, and per-package overrides. |
| [`spec/test-runner.md`](spec/test-runner.md) | The behavior of `burdin test`: test attributes, the doc tests of doc comments, parallelism, output capture, isolation, and discovery. |
| [`spec/bench.md`](spec/bench.md) | The behavior of `burdin bench`: bench items, warmup and calibration, statistics, baselines, and the profiler hook. |
| [`spec/fuzz.md`](spec/fuzz.md) | The behavior of `burdin fuzz`: fuzz items, the libFuzzer build, the corpus and the artifacts, minimizing a crash, and `burdin test --fuzz-replay`. |
| [`spec/sanitizers.md`](spec/sanitizers.md) | The behavior of `--sanitize`: AddressSanitizer, ThreadSanitizer, LeakSanitizer, the undefined checks, the platforms, and how a run reports. |
| [`spec/fmt.md`](spec/fmt.md) | The behavior of `burdin fmt`: the one style (4-space indentation, 120 columns), comments, blank lines, the magic trailing comma, `use` sorting and grouping, doc-comment code blocks, range formatting, and the command line. |
| [`spec/build-scripts.md`](spec/build-scripts.md) | The behavior of `build.bd`: trust and grants, generated modules, caching, and the `bindgen` a script may invoke. |
| [`spec/bindgen.md`](spec/bindgen.md) | `burdin bindgen`: C headers to Burdin declarations through libclang, the layout check, wrappers for `static inline` functions, and `--export` for a C header of what a package exports. |
| [`spec/rust-interop.md`](spec/rust-interop.md) | Rust interop: `burdin bindgen --rust` reads a crate's rustdoc JSON and writes a C-ABI shim and its Burdin module; `[rust.crates]` builds and links the shim with a pinned toolchain; layout checks on both sides. |
| [`spec/tasks.md`](spec/tasks.md) | Tasks, the work-stealing scheduler, `yield`, scoped tasks that borrow, panics in tasks, and OS threads (stage 2.1). |
| [`spec/channels.md`](spec/channels.md) | `Channel<T>` (bounded and unbounded, closing, ownership of sent values) and the `select` expression, with its priority, `default`, and guard rules (stage 2.2). |
| [`spec/allocators.md`](spec/allocators.md) | Allocator parameters, default type parameters, `Arena`, `Shared<T>`, and the checking allocator used by `burdin test`. |
| [`spec/fix.md`](spec/fix.md) | The one edit model: suggestions and their applicability, the applier, `burdin fix` with its rounds and conflicts, editions, and `burdin fix --migrate` (stage 3). |
| [`spec/refactor.md`](spec/refactor.md) | `burdin ssr` and `burdin refactor`: structural search and replace, extract variable and function, inline variable and function, change signature, and move item, each checked before it is written (stage 3). |
| [`spec/lint.md`](spec/lint.md) | `burdin lint`: the lints, levels and configuration, suppression comments, `--fix`, and the rules the tests cover (stage 3). |
| [`spec/doc.md`](spec/doc.md) | `burdin doc`: doc comments, the Markdown subset, intra-doc links and their diagnostics, the HTML site and its search, the JSON schema, the coverage report, and the command line (stage 3). |
| [`spec/check.md`](spec/check.md) | `burdin check`: the type-check-only command, what it shares with a build, `--tests`, and the rules the tests cover (stage 3). |
| [`spec/watch.md`](spec/watch.md) | `burdin watch` and `--watch`: what is watched and ignored, the debounce, restarting `run`, and the changed-modules-first `test` run (stage 3). |
| [`spec/workspaces.md`](spec/workspaces.md) | Workspaces: members, default members, excludes, the selection flags, inherited package fields and dependencies, and one lockfile at the root (stage 3). |
| [`spec/inspect.md`](spec/inspect.md) | Inspecting what the compiler generated: `burdin expand`, `--emit`, `burdin asm`, `burdin bloat`, and `burdin check --print-instances` (stage 3). |
| [`spec/debug.md`](spec/debug.md) | Debugging: the panic hook, symbolized panic frames, variable lifetimes, the gdb/lldb printers, the Windows Natvis file, `burdin debug`, and the `burdin dap` debug adapter (stage 3). |
| [`AGENTS.md`](AGENTS.md) | The working agreement: definition of done, verification policy, and issue workflow. |
| [`CLAUDE.md`](CLAUDE.md) | The Claude-specific entry point and working rules. |
| [`HANDOFF.md`](HANDOFF.md) | The current state, open decisions, and what is next. |
| [`CHANGELOG.md`](CHANGELOG.md) | Shipped milestones. |
| [`crates/burdin_fuzz/`](crates/burdin_fuzz/) | The lexer/parser fuzzer and its committed corpus. |
| [`crates/burdin_fmt/`](crates/burdin_fmt/) | The formatter behind `burdin fmt`, with its unit, corpus, and fuzz tests. |
| [`crates/burdin_refactor/`](crates/burdin_refactor/) | The refactoring engine behind `burdin ssr` and `burdin refactor`, over the query library. |
| [`crates/burdin_doc/`](crates/burdin_doc/) | The documentation generator behind `burdin doc`, with the fixtures it is checked against. |
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
happens in WSL Ubuntu first; `CLAUDE.md` has the exact setup and the PowerShell wrapper.
[`docs/platform-support.md`](docs/platform-support.md) records which configurations CI tests, which are untested, and
the known gaps. Build from the repository root:

```console
$ cargo build
```

The driver is then `target/debug/burdin` (or `target\debug\burdin.exe`). The examples below use that path; if the binary
is on `PATH`, drop the prefix.

### Dependencies and licenses

The compiler has two direct external Rust dependencies: `inkwell` 0.11 (Apache-2.0), which binds LLVM 21
through `llvm-sys` (MIT), and `libc` 0.2 (MIT OR Apache-2.0), whose C type layouts the generated runtime
reads for directory listings and metadata. Every other crate in [`Cargo.lock`](Cargo.lock) is transitively
required by them and is reviewed in [`tests/dependencies.txt`](tests/dependencies.txt); the policy and the
review are in [`docs/dependencies.md`](docs/dependencies.md).

- External requirements are exact (`=x.y.z`), and the committed `Cargo.lock` records the resolved
  version, source, and checksum of every crate, so builds are reproducible.
- Only permissive licenses are accepted (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, and similar);
  a copyleft identifier fails the check.
- LLVM itself is supplied by the host toolchain under Apache-2.0 WITH LLVM-exception and is not
  vendored.

`tests/check_deps.py` checks all of this without a compiler and re-verifies each license through
`cargo metadata --locked --offline` after a build. Every CI job runs it:

```console
$ tests/check_deps.py
No problems.
```

## Installing a release

The release workflow builds and publishes prebuilt compilers for six targets: Linux, macOS, and Windows on x86-64
and arm64. **No release has been cut yet**; the one-liners below work from the first published release onward. On
Unix:

```console
$ curl -fsSL https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.sh | sh
```

On Windows PowerShell:

```console
> irm https://raw.githubusercontent.com/RicardoMilos7/burdin/main/install.ps1 | iex
```

The installers verify the release's `SHA256SUMS` before unpacking, install into `$HOME/.local/bin`
(`%LOCALAPPDATA%\Programs\burdin\bin` on Windows) together with the official packages in a `packages/` folder beside the
compiler, and accept `BURDIN_VERSION` to pin a version,
`BURDIN_CHANNEL=nightly` for a nightly, and `BURDIN_INSTALL_DIR` to change the destination. `install.ps1` adds that
directory to your user `PATH`. `install.sh` adds a PATH line to `~/.zshrc`, `~/.bashrc` (`~/.bash_profile` on macOS), or
`~/.config/fish/config.fish`; other shells get a hint. Set `BURDIN_NO_MODIFY_PATH` (or pass `-NoModifyPath` to `install.ps1`) to skip the PATH
step. A prebuilt compiler needs
the LLVM 21 shared library at run time (`libLLVM-21.so`, `libLLVM.dylib`, or `LLVM-C.dll`); it is not bundled. The
artifact names, checksums, reproducibility rules, and release steps are in [`docs/releasing.md`](docs/releasing.md).

A release that carries the toolchain manager installs through it: the installed `burdin` is a proxy that runs the
selected toolchain, and `burdin toolchain` installs several toolchains side by side (`stable`, `nightly`, `0.4.1`),
updates them, pins one per project with `burdin-toolchain.toml`, selects one per command with `burdin +nightly ...`,
adds components (`lsp`, `std-src`, `docs`, `debuggers`), and updates or uninstalls itself. See
[`spec/toolchain.md`](spec/toolchain.md).

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

The program above and this command are checked by `tests/check_docs.py`. Other command transcripts in this file are
illustrative; the behavior they show is covered by the conformance tests and by that script.

## The driver contract

The `burdin` command has the subcommands `build`, `run`, `test`, `bench`, `check`, `fmt`, `doc`, `debug`, `explain`,
`bindgen`, `new`, `repl`, `lsp`, and `dap`.
This section is the
contract they implement; the conformance runner (`tests/run_tests.py`) and the documentation check depend on it.

`burdin` alone (or `burdin --help`) lists the commands with a one-line summary each. `burdin <command> --help`, or
`burdin help <command>`, prints everything about one command: its usage, arguments, options grouped by topic, and
notes. The command tree and its help are declared with clap in `crates/burdin_driver/src/cli.rs`; a mistyped command
gets a suggestion (`a similar subcommand exists: 'check'`). The synopsis of the main commands:

```
usage:
  burdin build [file.bd | package directory] [--release | --profile <name>] [-g] [--strip] [--lto] [--tests] [-j <n>]
               [--pgo-generate | --pgo-use <profile>] [-o <output>] [--emit-ir <file.ll>]
               [-p <member>]... [--workspace [--exclude <member>]...] [--link-lib <name>]...
               [--comptime-steps <n>] [--emit-deps <file>] [--sanitize <list>] [diagnostics]
  burdin run [file.bd | package directory] [--release | --profile <name>] [-g] [-j <n>] [--link-lib <name>]...
             [-p <member>]... [--workspace [--exclude <member>]...] [--comptime-steps <n>] [--sanitize <list>]
             [diagnostics] [-- <program arguments>...]
  burdin test [file.bd | package directory] [<filter>...] [--release | --profile <name>] [-g] [-j <n>] [--verbose]
              [-p <package>]... [--workspace [--exclude <member>]...] [--exact] [--skip <pattern>]...
              [--ignored | --include-ignored]
              [--failed] [--shard <i>/<n>] [--timeout <ms>] [--slow-threshold <ms>] [--list]
              [--format <human|json>] [--junit <file>] [--fuzz-replay] [--bless | --review] [--interpret]
              [--sanitize <list>] [--comptime-steps <n>] [diagnostics]
  burdin bench <file.bd | package directory> [<filter>...] [--release | --profile <name>] [-g] [-j <n>] [--verbose]
               [-p <package>]... [--workspace [--exclude <member>]...] [--exact] [--skip <pattern>]... [--list]
               [--format <human|json>] [--timeout <ms>] [--warmup <ms>] [--sample-count <n>] [--sample-time <ms>]
               [--save-baseline <name>] [--baseline <name>] [--fail-on-regression <percent>]
               [--profile-time <seconds>] [--comptime-steps <n>] [diagnostics]
  burdin fuzz <file.bd | package directory> <name> [--max-time <seconds>] [--minimize <file>]
              [--release | --profile <name>] [-g] [-j <n>]
              [--comptime-steps <n>] [diagnostics] [-- <libFuzzer options>...]
  burdin check <file.bd | package directory> [--release | --profile <name>] [--tests] [-j <n>] [--comptime-steps <n>]
               [--explain-ownership [--fn <name>] [--format <text|json>]] [--explain] [diagnostics]
  burdin watch [check|build|test|run] <file.bd | package directory> [options of that command] [--no-clear]
  burdin <build|run|test|check> <target> --watch [--no-clear] ...
  burdin fmt [<file.bd | directory>]... [--check] [-j <n>] [diagnostics]
  burdin fmt -p <member>... | --workspace [--exclude <member>]... [--check] [-j <n>] [diagnostics]
  burdin fmt --stdin [--stdin-filepath <path>] [--range <lo>:<hi>] [--check] [diagnostics]
  burdin doc <file.bd | package directory> [-o <dir | file>] [--output-format html|json] [--private]
             [--coverage] [--deny-warnings] [-j <n>] [diagnostics]
  burdin doc <workspace> -p <member>... | --workspace [--exclude <member>]... [the same options]
  burdin debug <file.bd | package directory> [--debugger <name | path>] [--test <name | index>]
               [--break-on-panic] [--release | --profile <name>] [-j <n>] [diagnostics] [-- <program arguments>...]
  burdin explain [<error code>]
  burdin bindgen <header> --out <file> [-I <dir>]... [-D <name[=value]>]... [--clang-arg <arg>]...
                 [--wrappers <file.c>] [--wrapper-include <text>] [diagnostics]
  burdin bindgen --export <target> --out <header.h> [--guard <NAME>] [diagnostics]
  burdin bindgen --rust <crate> --out <file> --shim <dir> [--rust-toolchain <version>] [--rustdoc-json <file>]
  burdin new <name> [--lib | --bin]
  burdin init [<dir>] [--lib | --bin]
  burdin repl [--release] [-j <n>] [--comptime-steps <n>] [diagnostics]
  burdin lsp [--stdio]
  burdin dap [--stdio]
  burdin --version

compile options:
  -j, --jobs <n>       run at most n threads: parsing, code generation, and formatting (default: BURDIN_JOBS,
                       then the number of CPUs); `burdin test` also runs at most n tests at once
  --profile <name>     build with a named profile: `dev` (the default of build, run, check, fuzz, and debug),
                       `release`, `test` (the default of test), `bench` (the default of bench), or one that
                       burdin.toml defines in [profile.<name>]; `--release` is `--profile release`
                       (the behavior is spec/profiles.md)
  --comptime-steps <n> the step budget of each compile-time evaluation (default: 10000000)
  --emit-deps <file>   write the files the build read besides the sources (`embed`), one path per line
  --tests              with `build`: compile the package's `test` blocks and their dispatcher instead of `main`,
                       as `burdin test` does, without running anything (the debug adapter builds a test this way)

check options (type-check only: the front end and every memory check, no code generation or linking):
  --tests              also check the package's `test` blocks, and do not require a `main` (a library package)
  --explain-ownership  print each function's source with margin annotations: its moves, copies, borrows, drops,
                       drop flags, argument conventions, and derived clones (issue #587)
  --fn <name>          with --explain-ownership: print only this function (or qualified name)
  --format <text|json> with --explain-ownership: annotated source (default) or JSON events with spans
  --explain            print the path that leads to each memory error, with its rule ID and the suggested fix

test options:
  -j, --jobs <n>       tests to run at once, and compilation threads (default: the number of CPUs)
  -p, --package <name> run the tests of one package of the build, or one member of a workspace; may repeat
  --workspace          run the tests of the entry package and every path dependency, or of every workspace member
  --exclude <member>   with --workspace, leave out this workspace member; may repeat
  <filter>...          run only tests whose full path (`module.test name`) contains any filter; --filter <text>
                       is the same as a positional filter
  --exact              filters and --skip patterns must equal the full path instead of being contained in it
  --skip <pattern>     do not run tests whose full path contains pattern; may repeat
  --ignored            run only tests marked [ignore]
  --include-ignored    run ignored tests too
  --failed             run only the tests that failed in the previous run (kept under <package>/target/burdin)
  --shard <i>/<n>      run the i-th of n deterministic slices of the selected tests (1 <= i <= n)
  --timeout <ms>       per-test timeout (default: 60000; 0 disables it)
  --slow-threshold <ms>  list tests that ran at least this long (default: 5000; 0 disables it)
  --list               print the selected tests and their attributes without running them
  --format <human|json>  human (default) prints the report; json streams one event per line (with --list, one
                       test per line)
  --junit <file>       also write a JUnit XML report of the run
  -v, --verbose        print the output of passing tests too

bench options (the behavior is spec/bench.md; benchmarks build in the release profile, so --release is the
default and changes nothing):
  -j, --jobs <n>       compilation threads only: benches are measured one at a time
  -p, --package <name> measure the benches of one package of the build, or one member of a workspace; may repeat
  --workspace          measure the entry package and every path dependency, or every workspace member
  --exclude <member>   with --workspace, leave out this workspace member; may repeat
  <filter>...          measure only benches whose full path contains any filter; --filter <text> is the same as a
                       positional filter
  --exact              filters and --skip patterns must equal the full path instead of being contained in it
  --skip <pattern>     do not measure benches whose full path contains pattern; may repeat
  --timeout <ms>       per-bench timeout (default: 600000; 0 disables it)
  --warmup <ms>       how long the warmup of each bench runs (default: 100)
  --sample-count <n>   how many samples each bench takes (default: 20)
  --sample-time <ms>  the target duration of one sample (default: 5)
  --list               print the selected benches without measuring them
  --format <human|json>  human (default) prints the report; json streams one event per line (with --list, one
                       bench per line)
  --save-baseline <name>  store the measurements under <package>/target/burdin/bench/<name>.json
  --baseline <name>    compare each measurement with the named baseline and report what changed beyond noise
  --fail-on-regression <percent>  with --baseline, fail when a bench is this much slower and the change is
                       beyond noise
  --profile-time <seconds>  run one selected bench in a loop for this long without measuring, for an external
                       profiler
  -v, --verbose        print the output of passing benches too

fmt options:
  --check                  change nothing; list the files that are not formatted and exit with status 1
  --stdin                  format standard input to standard output
  --stdin-filepath <path>  the path standard input stands for (names it in diagnostics, finds its package)
  --range <lo>:<hi>        with --stdin, format only the items that overlap these byte offsets

doc options:
  -o, --out <path>         where to write: a directory for html (default: <package>/target/doc), a file for json
                           (default: standard output)
  --output-format <f>      html (default) writes a static site; json writes the documented API as JSON
  --private                document `priv` items, fields, and methods too
  --coverage               print how much of the public interface is documented, per module, and write nothing else
  --deny-warnings          exit with status 1 when a doc comment has a broken link

diagnostics:
  --diagnostic-format <human|short|json|sarif>
                                          human (default) colors and wraps; short prints one plain line per
                                          diagnostic; json prints one version 1 JSON object per line; sarif
                                          prints one SARIF 2.1.0 log (docs/diagnostics.md)
  --color <auto|always|never>             color (default auto: only when stderr is a terminal)
  --no-color                              same as --color never
  --width <columns>                       wrap human diagnostics at this width (0 disables wrapping)
  --error-limit <n>                       print at most n errors after sorting (0 prints all)```

Long options also accept `--option=value`, and `-j4` is `-j 4`. `-j`/`--jobs` is an option of every command that
compiles or formats (`build`, `run`, `test`, `check`, `lint`, `doc`, and `fmt`): it bounds the threads the command
uses, and the output never depends on it. ``-p`, `--workspace`, `--filter`, `--exact`, `--skip`, `--timeout`, `--list`, `--format`,
and `--verbose` are options of `burdin test` and `burdin bench` (and so are the filters after the target);
`--ignored`/`--include-ignored`, `--failed`, `--shard`, `--slow-threshold`, and `--junit` are options of `burdin test`
only, and `--save-baseline`, `--baseline`, `--fail-on-regression`, `--profile-time`, `--warmup`, `--sample-count`,
and `--sample-time` are options of `burdin bench` only; any other command rejects them.

`burdin watch` (or `--watch` on `build`, `run`, `test`, and `check`) reruns the command whenever a source, `burdin.toml`, or a
build input changes, ignoring `target/` and version-control metadata, and clears the screen first unless `--no-clear`; `run` restarts the
program and `test` runs the changed modules' tests first. `spec/watch.md` is the exact behavior.

A target is either a single `.bd` file, compiled as a one-module package, or a directory containing a `burdin.toml`.

`burdin check <target>` runs the front end of `burdin build` (build scripts, parsing, name resolution and type checking, the
move and borrow checks, and compile-time evaluation) and stops before code generation: it needs no linker, writes no
file, and prints the same diagnostics in the same formats as a build. A clean human-format check prints nothing and exits 0; errors exit
1 and end with `error: could not check <target> (N errors)`. `--tests` also checks the package's `test` blocks and accepts
a library package without `src/main.bd`; `--release` checks as `build --release` does. It is meant for CI gates and editor
feedback. `--explain-ownership` prints each function's source with margin annotations for the moves, copies, borrows,
drops, drop flags, argument conventions, and derived clones the checks recorded (`--fn` selects one function and
`--format json` lists the events with spans), and `--explain` traces each memory error from the value's declaration to
the failing use, with its rule ID and suggested fix (text formats only; the JSON diagnostics carry the same facts). `burdin check <target> --print-instances [--format json]` also
prints every generic instance the program created and the call that required it
([`spec/inspect.md`](spec/inspect.md), INS12-INS15). [`spec/check.md`](spec/check.md) is the specification and
`tests/check_check.py` checks it: the fixtures under `tests/check/`, and the whole conformance suite, whose
`compile-fail` programs must report exactly their markers and whose other programs must be accepted.

**Machine-readable result.** `burdin check <target> --diagnostic-format json` prints one JSON object per diagnostic
(fields in [`docs/diagnostics.md`](docs/diagnostics.md), schema [`docs/diagnostic.schema.json`](docs/diagnostic.schema.json))
and always ends with the check summary, clean or not:

```
{"type":"check-summary","version":1,"errors":0,"warnings":0,"shown":0,"limit":null,"files":1,"elapsed_ms":612}
```

`errors` and `warnings` count each severity; `shown` and `limit` are the `--error-limit` figures; `files` is the number of
source files read from disk (the embedded standard library is not counted); `elapsed_ms` is the run's time in
milliseconds and differs between runs. The exit status is `0` when there are no errors (warnings allowed), `1` when
there are errors or the package cannot be read, and `2` for a usage error. A failure before the checks prints a plain
`error:` line and no summary. The human format is unchanged. [`spec/check.md`](spec/check.md) CK12 is the contract.

`burdin expand <target> [--item <path>] [--instances]` prints Burdin source for the program after compile-time evaluation,
`comptime if` and `comptime for` unrolling, user derives, default and named arguments, and operators: the text of each
file with only the rewritten parts replaced, through `burdin fmt`. The output is a program: `burdin check` accepts it
wherever it accepted the input. `--item` prints one function, type, constant, or method, and `--instances` adds each
instance of a generic function. `burdin build --emit <kinds>` writes `tir`, `llvm-ir`, `llvm-ir-opt`, `llvm-bc`, `asm`,
`obj`, and `deps` files next to the output (`--intel` and `--att` choose the dialect of `asm`), `burdin asm <target>
[<function>]` prints one function's assembly (or LLVM IR with `--llvm`) with the source lines interleaved, and `burdin
bloat <target>` reports the size of each function, module, package, and generic in the release build. The behavior is
[`spec/inspect.md`](spec/inspect.md) and `tests/check_inspect.py` checks it.

`burdin new <name>` creates the package directory `<name>` (a path; its last component is the package name, which must be
an identifier that is not a reserved word or `std`) with a `burdin.toml`, a `src/main.bd` that prints a greeting, and a
`.gitignore` for `target/` and the executable `burdin build .` leaves in the package directory. It refuses, with exit
status 2, to touch a path that already exists. `--lib` creates a library instead: `src/lib.bd` (one function and a `test`
for it, which `burdin test` runs; other packages reach the library as `use <name>`), and a `.gitignore` for
`target/` only, since a library builds no program. `burdin init [<dir>]` writes the same package into an existing
directory (the working directory by default; the package takes the directory's real name) and leaves its other files
alone. It refuses, with exit status 2 and before writing anything, when `burdin.toml`, the root module of its kind, or
`.gitignore` already exists, when the directory does not exist or is a file, and when the name is not valid.
`tests/check_new.py` checks both.

A dependency in `burdin.toml` is a directory (`lib = { path = "../lib" }`) or a git repository (`json = { git = "<url>",
tag = "v1" }`, or `branch`, or `rev`). `burdin add [<name>] --path <dir>` and `burdin add [<name>] --git <url> [--tag <t> |
--branch <b> | --rev <commit>]` add one to the nearest `burdin.toml`, resolve it, and write `burdin.lock`, which pins
every git dependency to an exact commit and the SHA-256 checksum of its files; a failed `add` changes neither file. Every
build keeps `burdin.lock` current (git checkouts are cached under `BURDIN_HOME`), and `--locked` on `build`, `run`,
`test`, `bench`, and `check` fails with exit status 1, naming each difference, when the lockfile does not match the
manifests. A third source is the official packages that ship with the compiler, which live in
[`packages/`](packages/README.md): `json = true` in `[dependencies]` (or `burdin add json`) declares one, it is versioned
with the compiler, and `burdin.lock` pins the checksum of its files. The first one is `json`, the JSON format on the
serialization framework (`packages/json/README.md`). The compiler finds them through `BURDIN_PACKAGES`, then
`packages/` beside the directory of its executable (or one level up), and the release archive and the installers put the folder
there. There is no package registry yet.

The dependency tables select which packages a build loads. `[features]` declares named features (each a list of other
features, `dep:x` to enable an optional dependency, or `x/feature` to forward to a dependency), with `default` enabled
unless `--no-default-features`; `--features <list>` and `--all-features` add more, on `build`, `run`, `check`, `test`,
`bench`, and `fuzz` (and `burdin watch` passes them to each run). A package with a `[features]` table or an optional
dependency gets a generated `features` module of `const <name>: bool` constants, so code branches with `comptime if
features.<name>`. Features unify across the graph: one package has one feature set, the union of every request.
`[dev-dependencies]` are loaded only by `test`, `bench`, `fuzz`, and `check --tests`, and a test reaches one by its
package name (`testkit.answer()`); a normal build does not read them. `[target.'cfg(...)'.dependencies]` applies only on
the host platform (`unix`, `windows`, `target_os`, `target_arch`, `target_family`, `all`/`any`/`not`). `burdin.lock`
pins the potential graph — optional, dev, and every target's dependencies — so features and the platform never rewrite
it. [`spec/packages.md`](spec/packages.md) is the specification, `tests/check_packages.py` checks the sources and the
lockfile, and `tests/check_features.py` checks the tables and features.

`burdin lsp` is the language server: it speaks the Language Server Protocol over standard input and output (`--stdio` is
accepted and changes nothing) and exits 0 after `shutdown` and `exit`, 1 otherwise. Editors start it themselves;
[`editors/`](editors/README.md) has the VS Code extension and the settings for Neovim and Helix, and
[`spec/lsp.md`](spec/lsp.md) is the specification. `tests/check_lsp.py` drives it.

`burdin debug <target> [-- args]` builds the target with `-g` in a temporary directory and starts the platform debugger on
it: gdb when it is installed, lldb otherwise (lldb on macOS; cdb on Windows), or whatever `--debugger` names. The
pretty-printers load with the program (the gdb ones are embedded in a `-g` binary, the lldb ones are written beside the
session), `--break-on-panic` starts stopped on `burdin_panic_begin` before the panic prints anything, and `--test <name |
index>` debugs one `test` block. The debugger is interactive and its exit status passes through. `tests/check_debug_printers.py`
checks the command line with a stub debugger.

`burdin dap` is the Debug Adapter Protocol server: it speaks DAP on standard input and output and forwards to `lldb-dap`,
adding the Burdin behavior on top — a `launch` request may name a `target` (and a `test`) that the adapter builds with
`-g`, the "Burdin panic" exception filter breaks on the panic hook, and steps that stop in generated code are skipped.
Editors register the `burdin` debug type against it. `spec/debug.md` (DBG11-DBG12) is the specification,
[`editors/`](editors/README.md) the editor configuration, and `tests/check_dap.py` plus
`crates/burdin_dap/tests/protocol.rs` drive it with a stub backend.

Environment:

| Variable | Meaning |
| --- | --- |
| `BURDIN_CC` | The C compiler driver used to link against libc (default `cc`, `cl` on Windows). |
| `BURDIN_CHANNEL` | The release channel `--version` prints: `dev` (default), `nightly`, or `stable`. |
| `BURDIN_COMMIT` | Overrides the git commit `--version` prints (for builds without a `.git` directory). |
| `BURDIN_CODEGEN_UNITS` | Split code generation into exactly this many units instead of choosing by program size (a testing aid; `tests/check_parallel.py` uses it). |
| `BURDIN_JOBS` | The default of `-j`: the most threads parsing, code generation, and `fmt` use; `1` is sequential. Default: the machine's parallelism. |
| `BURDIN_LD` | The linker to use: `mold`, `lld`, another name, a path (clang), or `platform` (default: `mold`/`lld` when installed on Linux, else the compiler driver's linker; sanitizer and fuzz builds always keep that linker). |
| `BURDIN_VERSION` | Overrides the version `--version` prints (nightly builds). |
| `LLVM_SYS_211_PREFIX` | The LLVM 21 installation; its `bin/llvm-config` drives `--link-lib LLVM-C`. |

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
- `[build] allow = ["lib"]` grants permission to run the named dependencies' `build.bd` scripts (see
  [Build scripts](#build-scripts-and-burdin-bindgen)).
- Unknown sections and keys are ignored, so a newer manifest does not break an older compiler.

A single `.bd` file is a package with one module (its module path is empty) and no manifest.

### Build scripts and `burdin bindgen`

`build.bd` next to a package's `burdin.toml` is an optional program the driver compiles and runs before reading the
package's sources. The package's own script is trusted; a dependency's runs only when the entry package's manifest
names it in `[build] allow` (otherwise E0260). The script runs with the package directory as its working directory
and `argv[1]`, the output directory `<package>/target/burdin/<profile>` as `argv[2]`, and the driver's directory
first on `PATH`; its output goes to the compiler's stderr. Any `.bd` file under `<out>/src/` becomes a module of the
package under that relative path, and a generated module that repeats a source module's name is E0262. A failing
script is E0261. The script reruns only when its source, the manifest, the compiler, or an input listed in
`<out>/inputs` (or read by the script's `embed` calls) changes; the cache key records content per package and
profile. `spec/build-scripts.md` is the exact rule.

`burdin bindgen <header> --out <file>` reads the header through libclang, so `#include`, the preprocessor's state,
and the system headers behave as they do for the C compiler, and writes a Burdin module of raw declarations:
`extern "C"` prototypes, `repr(C)` structs with a compile-time size and alignment check, typedefs, enumerations,
constants from object-like macros, and C wrappers for `static inline` functions. A declaration it cannot represent
is skipped and named in a comment at the top of the module. `--export` goes the other way and writes a C header for
a package's exports. [`spec/bindgen.md`](spec/bindgen.md) is the exact rule; [the command](#burdin-bindgen) has the
options.

```console
$ ./target/debug/burdin bindgen c/strings.h --out src/bindings.bd
```

### `burdin build`

```
burdin build <target> [--release] [-g] [--strip] [--lto]
             [--pgo-generate | --pgo-use <profile>] [-o <output>] [--emit-ir <file.ll>]
             [--link-lib <name>]... [--comptime-steps <n>] [--emit-deps <file>] [diagnostics]```

Compiles the target to a native executable. On success it prints nothing and exits 0.

- Without `-o`, the executable is written to the current directory as `<stem>` plus the platform suffix (`.exe` on
  Windows). `<stem>` is the file stem for a `.bd` target and the package name for a package directory. `-o` names the
  output file exactly; an existing directory is an error, so building a package from its parent when the directory and
  the package share a name needs `-o`.
- `--release` enables optimization (`-O2` through LLVM). Debug builds panic on overflow of `+`, `-`, `*`, and
  `MIN / -1`; release builds wrap. Division by zero always panics. This is language behavior, not only an
  optimization choice.
- `-g` (or `--debug-info`) emits source-level debug information: DWARF in the executable on ELF, DWARF in a
  `<output>.dSYM` bundle on macOS (ld64 keeps it out of the executable and writes a debug map), and the LLVM debug
  metadata the COFF backend carries for Windows. Every Burdin function is a `DW_TAG_subprogram`, every parameter and
  local a `DW_TAG_formal_parameter`/`DW_TAG_variable`, and every expression sets the line table, so a debugger can
  stop on a Burdin line and read a local. `burdin run -g` compiles with it too. `tests/check_debug_info.py` builds a
  program, reads the sections with `llvm-dwarfdump` (from the `.dSYM` on macOS), and, when `gdb` is installed, stops
  on a Burdin line and prints the locals.
- `--strip` passes the platform strip option to the linker (`-s` on ELF/Mach-O; `/OPT:REF /OPT:ICF` with MSVC) and
  keeps the program's behavior. `tests/check_build_flags.py` enforces a 144 KiB budget for a stripped hello-world
  and 512 KiB for a program using `List` and `Map` on Linux and macOS.
- `--lto` emits LLVM bitcode and links it with `-flto`. It needs an LLVM compiler driver: set `BURDIN_CC=clang` (MSVC
  `cl` cannot link bitcode).
- `--pgo-generate` compiles with IR-level profile instrumentation and links the profile runtime (again with an LLVM
  driver). Run the program with `LLVM_PROFILE_FILE=default.profraw`, then `--pgo-use default.profraw` merges the
  profile with `llvm-profdata`, applies `pgo-instr-use` with `opt`, and compiles the result with `llc`. The LLVM 21
  command-line tools must be on `PATH` or under `LLVM_SYS_211_PREFIX`. `--lto` cannot be combined with PGO.
- `--comptime-steps <n>` sets the step budget of each compile-time evaluation (a constant, a comptime expression, or
  a specialization; `spec/comptime.md`, CT11). The default is 10,000,000; an evaluation that runs out is E0225. `n`
  must be a positive integer. `burdin run` and `burdin test` accept it too.
- `--emit-ir <file.ll>` also writes the module's textual LLVM IR to that path, as generated and before the release
  optimization passes.
- `--emit-deps <file>` writes the files the build read besides the sources, one path per line: the files `embed`
  embedded (`spec/comptime.md`, CT15). A build system uses it to rebuild when one of them changes.
- A package directory keeps the executable it links under `<package>/target/burdin/<profile>/bin/` (`burdin test`
  keeps its binary under `test/`), beside a fingerprint of every input. An unchanged package is copied rather than
  linked again, and the front end still runs, so the diagnostics are the same. `-g`, `--emit-ir`, `--emit`, and
  `--pgo-use` builds are not cached. `spec/target.md` has the rules.

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

Compiles into a temporary directory and runs the program. The target may be left out: in a package directory,
`burdin run` runs that package, as `burdin run .` does. `burdin r` is the same command (the aliases are in
`spec/build-tool.md`, BT4). The program's stdout and stderr pass through unchanged, and
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
burdin test <target> [<filter>...] [--release] [-j <n>] [--verbose] [-p <package>]... [--workspace]
            [--exact] [--skip <pattern>]... [--ignored | --include-ignored] [--failed] [--shard <i>/<n>]
            [--timeout <ms>] [--slow-threshold <ms>] [--list] [--doc] [--format <human|json>] [--junit <file>]
            [diagnostics]
```

Builds the selected packages in test mode and runs every `test "name" [attrs] { ... }` block, each in its own
process, so a panic or abort fails only the running test. Tests are stripped from a normal build: `burdin build` and
`burdin run` never declare or check them. Test mode also replaces the prelude's `assert` and `assert_eq` with
versions that return `Result<(), AssertError>` instead of panicking, so `assert(...)?` reports a failed test and a
helper can propagate it. A `main` function is not required in test mode.

Test attributes are written after the name: `[ignore]` skips a test by default (`--ignored` runs only ignored tests,
`--include-ignored` runs all); `[timeout(ms)]` bounds one test (`0` disables the limit); `[should_panic]` and
`[should_panic("text")]` expect a panic abort (and the text on stderr); `[should_fail]` expects the test to return
an error; `[differential]` runs the body through the comptime interpreter too and requires the compiled test to
produce the same `Ok`/`Err` outcome. An invalid attribute is E0170.

Tests run concurrently (`-j`, default: the number of CPUs), each in a fresh temporary working directory with a
clean environment. Their output is captured and printed only when they fail; `--verbose` prints passing output too.
The report is deterministic: failures and the summary follow declaration order, not completion order.

By default only the entry package's tests run. `-p <package>` selects a package of the build (it may repeat), and
`--workspace` selects the entry package and every path dependency. With more than one package, names are displayed as
`<package>::<test>`. In a workspace, `-p`, `--workspace`, and `--exclude` select members instead, and the selected
members run together with one report (`spec/workspaces.md`, WS11).

Doc tests run with the `test` blocks: every Burdin code block in a `///` or `//!` comment of the selected packages
is built as its own program against the package as a dependency (its public API only) and run like a `test` block;
`--doc` runs only them. A doc test's name is `<owner> (line N)` — the documented item (or the file, for module
documentation) and the line of the block's opening fence — and a failure is reported at the doc comment's file and
line, with the compile's diagnostics moved from the generated program back to the comment. The block's fence takes
attributes (`ignore`, `no_run`, `should_panic`, `compile_fail`, `compile_fail E0123`); an unknown attribute warns.
A line the marker `# ` starts is setup: compiled and hidden from the rendered documentation (`burdin doc` hides it).
The full behavior is [`spec/test-runner.md`](spec/test-runner.md), "Doc tests".

Every test has a full path, `module.submodule.test name` (the module is the file's path inside `src`, empty for
`src/main.bd`; with more than one package it starts with `<package>::`). The filters after the target keep the tests
whose full path contains any of them (`--filter <text>` is the same thing), `--exact` requires an equal path, and
`--skip <pattern>` leaves tests out. The summary says how many tests were filtered out. `--list` prints the selected
tests and their attributes without running them (`--list --format json` is one JSON object per test, with file and
line, for editors and CI). `--format json` streams one JSON event per line while the tests run, and `--junit <file>`
writes a JUnit XML report. `--failed` reruns the tests that failed in the previous run (kept in
`<package>/target/burdin`), `--shard <i>/<n>` runs one deterministic slice for CI parallelism, and tests that ran at
least `--slow-threshold` milliseconds are listed before the summary.

The output is one `---- test "name" stdout ----` / `stderr` section and `FAIL <test name>` line per failed test,
followed by a summary line; a passing test prints nothing by default:

```console
$ ./target/debug/burdin test .
---- test "second one fails" stderr ----
<the captured stderr>
FAIL second one fails
test result: 2 passed, 1 failed
```

When tests are marked `[ignore]`, the summary ends with `, N ignored`, and `, N filtered out` follows when the filters
left tests out. The exit status is 0 only when every run test
passes, 1 when a test fails or compilation fails, and 2 for a usage error (an unknown package, a bad `-j`,
conflicting flags) or a driver failure after loading. `tests/run_tests.py` reads the `FAIL ` prefix and the
`test result: N passed, M failed` summary; the full behavior, including isolation and discovery, is
[`spec/test-runner.md`](spec/test-runner.md), and `tests/check_test_runner.py` checks it black-box.

### `burdin bench`

```
$ cat src/main.bd
fn main() { println("hello") }

bench "sum a range" {
    let mut total = 0
    for i in 0..100 {
        total = total + black_box(i)
    }
    black_box(total)
}

$ ./target/debug/burdin bench .
sum a range: 12.40 ns (mean 12.51 ns, ci 95% 12.44 ns to 12.58 ns, mad 0.21 ns, 2/20 outliers)
bench result: 1 measured, 0 failed
```

A `bench "name" { ... }` block is measured work: `burdin bench` compiles it in the release profile (that is the
default; `--release` changes nothing), warms it up, times growing batches to choose how many iterations one sample
holds, takes `--sample-count` samples of about `--sample-time` milliseconds each, and reports the median, the mean,
a 95% confidence interval, the median absolute deviation, and the outliers. One call of the bench body is one
iteration, so anything the body does is measured; `black_box(value)` hands the value back through a barrier the
optimizer cannot see through, so the work cannot be deleted or hoisted. Every other command strips bench blocks
from the program, the way they strip `test` blocks.

```
sum a range: 12.40 ns (mean ..., 2/20 outliers)
  baseline nightly: -3.1% faster (beyond noise)
bench result: 1 measured, 0 failed
```

`--save-baseline <name>` stores the statistics under `<package>/target/burdin/bench/<name>.json`, `--baseline
<name>` compares each measurement with them and reports `unchanged (within noise)`, `N% slower/faster (beyond
noise)`, or `new`; `--fail-on-regression <percent>` makes a bench that is slower by more than that percent, beyond
noise, fail the run for CI. `--profile-time <seconds>` runs exactly one selected bench in a loop, unmeasured, for a
profiler to attach to. Filters, `--exact`, `--skip`, `-p`, `--workspace`, `--list`, and `--format json` behave as
they do for `burdin test`, and a bench is always measured alone. The exit status is 0 when every bench was
measured, 1 when one failed or regressed past the gate, and 2 for a usage error. The full behavior, including the
harness protocol and the statistics' definitions, is [`spec/bench.md`](spec/bench.md), and
`tests/check_bench.py` checks it black-box.

### `burdin fmt`

```
burdin fmt [<file.bd | directory>]... [--check] [diagnostics]
burdin fmt --stdin [--stdin-filepath <path>] [--range <lo>:<hi>] [--check] [diagnostics]
```

Formats Burdin source in the one style `burdin fmt` has: it takes no options. Each `.bd` file named on the command
line is rewritten in place, and a directory means every `.bd` file under it (hidden directories and `target` are
skipped); with no path it formats the current directory. The path of every file it changed is printed. A file that is
already formatted is not written, and the result is always a fixed point: running `burdin fmt` again changes nothing.
Comments and blank lines are kept, lines are at most 120 columns, a list with a trailing comma stays one element
per line, `use` declarations are sorted and grouped (the standard library, the dependencies in `burdin.toml`, then the
package), and the code in the fenced blocks of doc comments is formatted too (a block with hidden `# ` setup lines
keeps the shape its author gave it). The formatter checks its own output
with the compiler's parser: the formatted program must be the same program with the same comments, or the file is
left alone and the failure is reported as a bug.

`--check` writes nothing: it lists the files that are not formatted and exits 1 when there is one, for CI.
`--stdin` reads one source from standard input and writes the formatted source to standard output, for editors;
`--stdin-filepath` says which file it is (for diagnostics and the package whose dependencies group the `use`
declarations) and `--range lo:hi` formats only the items that overlap those byte offsets. A source with a syntax error
is not formatted: the compiler's diagnostics are printed and the exit status is 2. The status is 0 when everything is
formatted, 1 when `--check` finds a file that is not, and 2 for a usage error, an unreadable file, a syntax error, or
an internal formatter error. The full behavior is [`spec/fmt.md`](spec/fmt.md), and `tests/check_fmt.py` checks it
black-box.

### `burdin --version`

```
burdin 0.1.0
channel: dev
target: x86_64-unknown-linux-gnu
llvm: 21.1.8
commit: b011782b7f2a
profile: debug
```

The version comes from `Cargo.toml` (unless `BURDIN_VERSION` overrides it), the channel from `BURDIN_CHANNEL`
(`dev` for a local build, `nightly` or `stable` for a release), the target and profile from the build (`target` is
the host triple the compiler was built for), the LLVM version from the linked LLVM library, and the commit from git
at build time (`unknown` when the build had no `.git`, unless `BURDIN_COMMIT` overrides it). The version scheme and
channels are in [`docs/releasing.md`](docs/releasing.md). Diagnostics do **not** print the compiler version: their
format stays `path:LINE:COL: error[E0000]: message`. Include the `--version` output in a bug report.

### Runtime behavior

These properties are part of the generated runtime and are checked by the conformance suite and the scripts below.

- **Buffered output.** Standard output is block buffered in 8 KiB, or line buffered when fd 1 is a terminal
  (`isatty`). Standard error is unbuffered. The buffer is flushed at a normal exit (through `atexit`), by
  `process.exit`, before a panic message, and by the stack-overflow handler, so output is never lost before an
  abnormal end. `tests/m02_basics/panic_flushes_stdout.bd` and `tests/m09_stdlib/exit_code.bd` witness it.
- **Panic backtraces.** A panic prints its stable message line and then `stack backtrace:` with symbolized frames,
  hiding the runtime's own frames. Every generated function carries an unwind table (`uwtable`), and Burdin
  functions keep their `burdin.<name>` symbol, so the trace names them in both profiles; an optimized build may
  inline the frames around the panic. `tests/m02_basics/panic_backtrace.bd` checks both profiles and
  `panic_backtrace_names.bd` (debug-only) checks the names.
- **Stack overflow.** The main thread gets an alternate signal stack and a `SIGSEGV` handler on Unix, and on Windows
  a vectored exception handler for `EXCEPTION_STACK_OVERFLOW` with a 64 KiB stack guarantee reserved so the handler
  can still flush and report. Unbounded recursion ends with `panic: stack overflow` and status 134 instead of a raw
  fault; a fault that is not in the stack guard region ends with `panic: invalid memory access`.
  `tests/m02_basics/stack_overflow_aborts.bd` covers the recursion case.
- **Parallel parsing.** Modules are independent, so `burdin build`/`run`/`test` parse them in parallel. Each worker
  keeps its own diagnostics and the driver merges them in file order, so the output is byte-identical to a
  sequential run. `-j` (or `BURDIN_JOBS`) sets the worker count (1 is sequential); `tests/check_parallel.py` compares
  the two modes on a multi-module package, including its errors.
- **Parallel code generation.** The program is lowered to one LLVM module, unused runtime and glue functions are
  dropped, and a program of more than about 8,000 instructions is split into codegen units (at most 16) that are
  optimized and compiled to separate objects on up to `-j` threads and linked together. The number of units depends
  only on the program, so `-j 1` and `-j 16` link the same executable. In a release build a small function stays
  inlinable across units. `--lto`, `--pgo-generate`, and `--pgo-use` keep one unit. `tests/check_parallel.py` checks
  the result and its determinism, and runs the code-generation-heavy milestones with the split forced.

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

The count is pluralized (`1 error`, `2 errors`). A file with a syntax error is not type checked, and the other modules
are checked against the declarations the broken file did read; the follow-on errors of a declaration that did not parse
are left out, so a file with a syntax error may report fewer later diagnostics (`spec/check.md`, CK4). Errors before or outside source checking (a malformed
manifest, a missing dependency, a dependency cycle, a missing target) are printed as `error: <message>` without an
error code.

**Color and width.** `--color auto|always|never` (and `--no-color`, the same as `never`) colors the location, the
severity, and the code. `auto` is the default and colors only when stderr is a terminal; a non-empty `NO_COLOR`
disables color for `auto`, while an explicit `always` or `never` wins over it, and the last color option wins.
`--width <n>` wraps the human format to `n` columns (`0` disables wrapping); without it the driver uses `COLUMNS`,
then 80. Wrapping only inserts line breaks and indentation, and the first line always keeps the
`path:LINE:COL: error[E0000]: message` shape.

**Formats.** `--diagnostic-format short` prints exactly one plain line per diagnostic, with no notes,
suggestions, link, color, or wrapping; it is the stable format the test scripts parse. `--diagnostic-format json`
prints one version 1 JSON object per line (a diagnostic with its spans, labels, notes, help, children, related
information, suggestions with edits, the link to its entry in the error reference, and the `rendered` human text),
and the last line is a summary object. `--diagnostic-format sarif` prints one SARIF 2.1.0 log for the whole run, for
code-scanning services; `burdin lint` supports it too, and `burdin build` writes a log even when it found nothing.
JSON, SARIF, and short output are never colored or wrapped; `--color` and `--width` only affect `human`. The fields,
the schema (`docs/diagnostic.schema.json`), and the SARIF mapping are in [`docs/diagnostics.md`](docs/diagnostics.md).

**Links.** Every diagnostic has a stable link to its entry in the hosted error reference,
`https://ricardomilos7.github.io/burdin/errors/E0003` for E0003 (`/lints/L0007` for a lint). It is the `url` of the
JSON format and the `helpUri` of the SARIF rule, and in colored `human` output the code is a terminal hyperlink
(OSC 8) when the terminal supports it. `FORCE_HYPERLINK=1` turns the hyperlink on and `FORCE_HYPERLINK=0` turns it off;
without color there is none.

The error codes come from the registry in [`tests/README.md`](tests/README.md#error-codes). `E9000` is the temporary
code for "not implemented in this compiler yet"; the conformance suite does not use it, and
[`spec/e9000-audit.md`](spec/e9000-audit.md) assigns every remaining site to the milestone that removes it.

### `burdin explain` and the error-code reference

[`docs/error-codes.md`](docs/error-codes.md) explains every registry code, with a program that triggers it and how
to fix it. The page is checked in CI: `tests/check_error_docs.py` compiles every snippet and requires exactly the
marked diagnostics, and the driver embeds the page, so the two cannot drift.

```
burdin explain E0003      print the reference entry for E0003
burdin explain            list every code
```

`burdin explain` writes to stdout and exits 0; an unknown code exits 2. The human diagnostics end with a note
pointing at the command.

### `burdin doc`

`burdin doc` builds the documented interface of a package from its `///` comments and `//!` module comments and writes
it as a static HTML site, as JSON, or as a coverage report. The site is self-contained: the stylesheet and the search
script are compiled into the driver, so a page loads nothing from another server and the site works when opened from
disk. Doc comments are Markdown (headings, lists, tables, quotes, fenced code) and may link to the API with `[Circle]`,
`[shapes.Shape]`, or ``[`Circle.area`]`` (a Markdown link `[text]` with a target in parentheses works too); a link
that resolves to nothing is warning `D0001`, and one
that names a `priv` item is `D0002`. [`spec/doc.md`](spec/doc.md) is the specification.

```
burdin doc <file.bd | package directory> [-o <dir | file>] [--output-format html|json] [--private]
           [--coverage] [--deny-warnings] [diagnostics]
```

The HTML site documents the public interface: by default `priv` items, fields, and methods are left out and `--private`
shows them, marked. A type's page lists its fields and variants, its methods, the operators it implements written as
they are written (`a + b`, `-a`), its extension methods with the `use` a caller needs, the interfaces it implements
(derived ones marked), and the methods of impl blocks for a specific instance (`Stack<i32>`). The JSON output has a
schema version (`format_version`), so other tools can read the API. `--coverage` prints the documented and undocumented
items per module; they are the ones the `missing_docs` lint (`L0017`, off by default) reports.

### `burdin lint`

`burdin lint` is the built-in linter. It checks a file or a package that already compiles for code that is legal but
probably unintended: names that break the conventions, an `else` that hides new variants of a local enum, long eager
`List` chains, unused imports, variables and parameters, unreachable and dead code, and clones, copies, `mut`s and
allocations that cost for no reason. [`docs/lints.md`](docs/lints.md) has one entry per lint, each with a checked
example; [`spec/lint.md`](spec/lint.md) is the specification.

```
burdin lint <file.bd | package directory> [-A|-W|-D <lint>]... [--deny-warnings] [--fix [--dry-run]]
burdin lint --list                 print every lint with its code and default level
burdin lint --explain <lint>       print the reference entry of one lint, by name or by code
```

Every lint is `allow`, `warn` (the default), or `deny`. `-A`, `-W`, and `-D` set a level from the command line (`all`
names every lint); a `[lint]` table in `burdin.toml` (`unused_imports = "deny"`) sets the levels of a package, and the
command line wins. A finding is silenced by a comment above the code (`// lint: allow(dead_code)`) or for a whole file
(`// lint: allow-file(non_snake_case)`). The diagnostics use the same formats as the compiler's (`--diagnostic-format`);
a fix is attached to a diagnostic as a suggestion with `"applicability":"machine"` in the JSON.

`--fix` applies the fixes that cannot change what the program does (an unused import, an unused variable's `_`, a
needless `mut`, a clone of a value that is not used again). It prints a unified diff, checks that the fixed package
still compiles, and writes every file or none; `--fix --dry-run` writes nothing. The exit status is 0 when no lint is at
`deny`, 1 when one is reported (or the package has compile errors), and 2 for a usage error.

### `burdin bindgen`

```
burdin bindgen <header> --out <file> [-I <dir>]... [-D <name[=value]>]... [--clang-arg <arg>]...
               [--wrappers <file.c>] [--wrapper-include <text>] [diagnostics]
burdin bindgen --export <target> --out <header.h> [--guard <NAME>] [diagnostics]
burdin bindgen --rust <crate> --out <file> --shim <dir> [--rust-toolchain <version>] [--rustdoc-json <file>]
```

The first form reads a C header through libclang and writes a Burdin module of raw declarations. libclang is loaded
when the command runs and is never linked into the compiler, so every other command works without it; when it is
missing the error names the places that were searched and how to install it. `BURDIN_LIBCLANG` and `LIBCLANG_PATH`
override the search, and `LLVM_SYS_211_PREFIX` is honored.

`-I`, `-D`, and `--clang-arg` are passed to the C front end, so the preprocessor sees the same configuration the C
compiler would; on macOS the SDK from `xcrun --show-sdk-path` is added as `-isysroot` unless one is given, because a
Homebrew libclang has no sysroot of its own. `--wrappers <file.c>` also writes C that calls the header's `static inline` functions and exports
them as `<name>__extern`, because they have no symbol of their own; `--wrapper-include <text>` is the `#include` line
of that file (the header path by default). Link the compiled wrapper with `--link-lib <file.o>`.

What was skipped, and why, is printed to stderr and repeated as a `// bindgen:` comment at the top of the module.
The exit status is 0 when the header parsed, 1 when clang reports an error in it, and 2 for a usage or I/O error or
a libclang that cannot be loaded.

`--export <target> --out <header.h>` writes the other direction: a C header with a prototype for every
`extern "C" fn` the package exports, the `repr(C)` structs they use, and their public constants, inside an include
guard and an `extern "C"` block for C++. A declaration a C header cannot spell is E0263, and no header is written.

`--rust <crate>` binds a Rust crate. It reads the crate's public API from rustdoc JSON (which needs the exact Rust
toolchain the manifest or `--rust-toolchain` pins, because the format is unstable) and writes a **shim**, a small
crate of `extern "C"` functions that you build as a static library, and a Burdin module of declarations for it.
`repr(C)` structs with public fields are bound by value with size, alignment, and offset checks on both sides; every
other struct or enum is an opaque handle with a generated `_free`; fieldless enums are their integer; `&str`, `String`,
and slices are a pointer and a length; generics, traits, `async`, and `Result` are skipped and named in a
`// bindgen:` note. A package gets the shim without the command: `[rust] toolchain = "1.100.0"` and
`[rust.crates] geometry = { path = "../geometry" }` in `burdin.toml` make `build`, `run`, and `test` generate, build, and
link it (a failure is E0264). [`spec/rust-interop.md`](spec/rust-interop.md) is the exact rule.

```console
$ ./target/debug/burdin bindgen c/strings.h --out src/bindings.bd
$ ./target/debug/burdin bindgen --export . --out build/exports.h
```

### Emitting LLVM IR

`--emit-ir <file.ll>` also writes the textual LLVM IR, both for inspecting the backend and for debugging code
generation:

```console
$ ./target/debug/burdin build hello.bd --emit-ir hello.ll
$ grep -m1 'define.*@burdin.main' hello.ll
define void @burdin.main() {
```

The file is written before the release optimization passes, so `--release --emit-ir` shows pre-optimization IR. The
IR is the module that is handed to LLVM for the object file; it is not a stable format.

### Linking with `--link-lib`

`--link-lib <name>` adds one library to the link step. It may repeat.

- On Linux and macOS the driver passes `-l<name>` to the C compiler driver.
- With MSVC (`cl`) it passes `<name>.lib`.
- A value that names a file (it contains a path separator, or ends in `.a`, `.o`, `.obj`, `.lib`, `.so`, `.dylib`,
  or `.dll`) is passed to the linker as it is. That is how the C wrappers `burdin bindgen --wrappers` writes, or a
  shared library that calls exported Burdin functions, joins the link: `--link-lib build/wrappers.o`.

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

### Choosing the linker

The C compiler driver still supplies the C runtime and the libraries, but the link itself can be done by a faster
linker. On Linux the driver names one when it is installed — `mold` when `ld.mold` is on `PATH`, then `lld` when
`ld.lld` is — by passing `-fuse-ld=<name>` to the compiler driver. Otherwise the driver's own linker is used, as on
macOS (ld64) and Windows (`link.exe`). `BURDIN_LD` overrides the choice: a linker name (`mold`, `lld`, `gold`), a path
to the linker (clang accepts a path; gcc wants the `ld.<name>` name), or `platform`/`default` to force the compiler
driver's own linker. An empty value is the same as unset.

```console
$ BURDIN_LD=mold ./target/debug/burdin build hello.bd
$ BURDIN_LD=platform ./target/debug/burdin test
```

`--lto` keeps the compiler driver's linker and refuses mold: LLVM bitcode needs lld, and the installed lld may not be
the LLVM version that produced it. A sanitizer or fuzz build (`--sanitize`, `burdin fuzz`) keeps it too: those runtimes
start up from module constructors that LLVM emits into `.ctors`, which lld leaves in a section libc never runs, so
naming another linker there is a usage error. An MSVC-style driver (`cl`, `cl.exe`, `clang-cl`) always uses `link.exe`
and has no `-fuse-ld`, so a `BURDIN_LD` value other than `platform`, `default`, `link`, or `msvc` is a usage error.
When a link fails, the error says whether `BURDIN_LD` or the automatic choice supplied the linker, and how to go back
to the default. `tests/check_linker.py` checks all of it.

### Exit status

| Status | Meaning |
| --- | --- |
| 0 | `build` succeeded; `run` exited 0; every test passed. |
| 1 | Diagnostics were reported, a source file or manifest could not be read, or one or more tests failed. |
| 2 | A usage error, or a driver failure after loading: the output path is a directory, code generation failed, the linker could not be started or failed, or a temporary directory could not be created. |
| program status | `run` exits with the program's status; 128+signal on Unix. |

Usage errors print `error: <message>`, the command's usage line, and a pointer to `burdin <command> --help`.

## Fuzzing the lexer and parser

`crates/burdin_fuzz` lexes and parses a committed corpus and deterministic mutations of it. The formatter has its own
fuzz test, `crates/burdin_fmt/tests/fuzz.rs`, which formats the same inputs and requires that everything that parses
formats to a fixed point. `cargo test --workspace`
replays the corpus in every CI job, so a panic reintroduced by a change fails CI. The scheduled `fuzz` job runs a
longer session through `tests/fuzz.py`; a found input is written under `fuzz-out/crashes` and is committed under
`crates/burdin_fuzz/corpus/syntax/` together with its fix. See
[`crates/burdin_fuzz/corpus/syntax/README.md`](crates/burdin_fuzz/corpus/syntax/README.md).

```console
$ tests/fuzz.py --seconds 60
```

## Checking the documentation

`tests/check_docs.py` verifies that the links in the repository's Markdown files resolve (files and heading anchors),
that the checked example above still compiles and prints the shown output, and that the documented driver behavior
holds: `build` output naming, `-o`, `--emit-ir`, `--version`, `-g`, `--strip`, `--release`, `run` exit-status
passthrough, `-- args`, the `test` summary format, the diagnostic format, `--link-lib` (including `LLVM-C` where
LLVM is available), and `BURDIN_CC`. Run it after building the driver:

```console
$ tests/check_docs.py
```

The other checks need a built driver too and cover the runtime and code-generation work of issues #421-#432:

```console
$ tests/check_debug_info.py     # -g: DWARF, line table, and a gdb breakpoint when gdb is installed
$ tests/check_codegen_opt.py    # drop-flag elimination, bounds-check elision, shared glue
$ tests/check_build_flags.py    # --version, --strip's budget, --lto, and PGO
$ tests/check_parallel.py       # parallel parsing and deterministic diagnostics
$ tests/check_fmt.py            # burdin fmt: golden outputs, idempotence, --check, --stdin, --range, grouping
$ tests/check_release.py        # release workflow, archives, checksums, installers, and version channels
$ tests/check_doc.py            # burdin doc: the site and its goldens, links, JSON, coverage, output
```

The release pipeline has three scripts of its own: `tests/build_release.py` builds and packages one target (and
`--verify-reproducible` proves two checkout paths produce the same archive), `tests/release_notes.py` extracts a
changelog section and checks its links, and `.github/workflows/release.yml` builds the six targets, verifies the
checksums, and publishes only from a tag push or an explicit manual `publish`. See
[`docs/releasing.md`](docs/releasing.md).
`tests/check_check.py` covers `burdin check`: the fixtures under `tests/check/` with their golden outputs, the equivalence with
`burdin build`, every conformance test (a `compile-fail` program reports exactly its markers, every other program is
accepted), the absence of code generation and linking, and the command line. Every CI job runs it after the build.
`tests/check_inspect.py` covers the inspection commands: the golden expansion of fixtures from `m17_arguments`, `m19_operators`,
`m20_comptime`, and `m21_reflect`, the expansion of every program of those folders going back through `burdin check` and
printing what the original prints, `--emit`, `burdin asm`, `burdin bloat`, and the instance report. Every CI job runs it
after the build.
`tests/check_lint.py` covers `burdin lint`: every documented example, the fixtures under `tests/lint/` (each with the
diagnostics its `//~` markers name), `--fix` and `--dry-run` against golden files, the all-or-nothing write, levels, the
manifest `[lint]` table, and the output formats. Every CI job runs it after the build. `tests/check_doc.py` covers `burdin doc`: the fixture package under `tests/doc/geometry` against the golden pages in
`tests/doc/expect`, the integrity of every link in the site, the intra-doc link diagnostics of `tests/doc/links`, private
items, the JSON schema, the coverage report and its agreement with `missing_docs`, and the output directory rules.

Two checks cover the diagnostics themselves, and every CI job runs them after the build:

- `tests/check_diagnostics.py` checks the default and short and JSON formats, color and `NO_COLOR`, wrapping and
  `--width`, notes and suggestions, `--error-limit`, and deterministic ordering. It also validates the JSON the driver
  prints for every failing conformance test against [`docs/diagnostic.schema.json`](docs/diagnostic.schema.json), the
  golden files in `tests/diagnostics/`, the SARIF log of `build` and `lint`, and the hyperlinks
  ([`docs/diagnostics.md`](docs/diagnostics.md)).
- `tests/check_error_docs.py` checks that [`docs/error-codes.md`](docs/error-codes.md) mirrors the registry, that
  every snippet fails with exactly the marked diagnostics, that every diagnostic links to its entry at the stable
  address, and that `burdin explain` prints every entry.

The dependency audit needs no built compiler: `tests/check_deps.py` reads `Cargo.lock`, the manifests, and
[`tests/dependencies.txt`](tests/dependencies.txt), and re-verifies the licenses through `cargo metadata` when it can.
Every CI job runs it after `cargo build` with `BURDIN_DEPS_REQUIRE_METADATA=1`.
