# Burdin

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. It is not for production. The compiler is written in Rust and stays in Rust:
stage 0 is the core subset, and stage 1 adds the rest of the core language to the same Rust compiler. Burdin is not
bootstrapped into itself. A self-hosted compiler may be reconsidered once the language is more mature and at least its
core features are complete, but it is not planned.

Read `HANDOFF.md` for the current state, the open decisions, and what to do next. Read `AGENTS.md` for the shared
working agreement: the definition of done, the verification policy, the milestone close-out checklist, and the issue
workflow. Process rules live in `AGENTS.md`; behavior lives in `tests/` and `spec/`.

## Sources of truth, in this order

The files in this repository are the only maintained copies. Documents hosted on claude.ai are historical: they are
not updated and may disagree with the repository.

1. `tests/` is the conformance suite and the most precise statement of behavior. Read `tests/README.md` first: it defines
   the test format, the error-code registry, and the 16 settled language decisions.
2. `spec/grammar.md` is the syntax of stage 0. `spec/memory-rules.md` is the exact semantics of moves, conventions,
   exclusivity, references, and drops, with rule IDs and the tests that cover each. `spec/stdlib.md` is the standard
   library surface (names and signatures), `spec/iterators.md` is the iteration protocol (`Iterator<T>`/
   `Iterable<T>`, `iter()`/`into_iter()`, user `for`, the adapters, and ranges), `spec/format.md` is interpolation,
   `Display`/`Debug`, the format specifiers, and the output functions, `spec/derives.md` is the exact
   behavior of `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default`, `spec/existentials.md` is the behavior of `any`
   and `impl` existential types, object safety, boxing, and downcasts, `spec/errors.md` is `error` declarations, the
   dynamic `Error`, `Result<T>`, `context`, traces, and `main(): Result<T>`, `spec/arguments.md` is default and named
   arguments, `spec/extensions.md` is extension methods and their resolution order, `spec/operators.md` is operator
   overloading and `Index`/`IndexMut`, `spec/comptime.md` is compile-time evaluation (`comptime` expressions,
   parameters, `if`, `for`, `assert`, constants, and `embed`), `spec/reflection.md` is `Type.fields`/`variants`/`name`,
   `x.{name}`, and user `derive` declarations, `spec/const-generics.md` is const parameters, const arguments, and array
   lengths, `spec/type-aliases.md` is generic type aliases, `spec/impls.md` is impl blocks for specific instances,
   `Drop` on a generic type, and the built-in interfaces on one, `spec/allocators.md` is allocator parameters,
   default type parameters, `Arena`, `Shared<T>`, and the checking allocator in `burdin test`, `spec/test-runner.md`
   is the behavior of `burdin test`, `spec/fmt.md` is the behavior of `burdin fmt` (the one style, comments, `use`
   sorting, doc-comment code blocks, and the command line), `spec/tasks.md` is tasks, the scheduler, scoped tasks, panics in tasks, and
   threads, `spec/channels.md` is `Channel<T>` and `select`, `spec/shared.md` is `Send`, `Sync`, atomics, `Mutex<T>`, `RwLock<T>`, and
   the atomic `Shared<T>`, `spec/io.md` is the reactor, the helper pool, `blocking` extern functions, sockets, name
   resolution, and timers, and `spec/build-scripts.md` is `build.bd`, dependency grants, generated
   modules, caching, and the minimal `bindgen`; `spec/lint.md` is `burdin lint` (the `[lint]` table, suppression
   comments, and `--fix`); `spec/doc.md` is `burdin doc`, the doc comments, the Markdown subset, intra-doc links and
   their diagnostics, the site and its search, the JSON schema, and the coverage report; and `spec/diagnostics.md` is
   the versioned JSON diagnostic format, the SARIF log, and the links to the error reference.
3. `spec/stage0-subset.md` is what stage 0 must support, what it must not, and the 11 milestones (all complete).
   `spec/stage1-subset.md` is the stage 1 plan: what each milestone adds, the decisions it needs, and the
   roadmaps for later stages.
4. `Burdin Language Design.md` is the full language design, including features that come after stage 0.

If these disagree, stop and say so. Do not pick one silently.

## Working rules

- **Do not change a test to make it pass.** A test is the spec. If you think a test is wrong, explain why and ask. Changing
  the spec means updating the spec file and the tests first, then the code.
- **Every change ships with tests.** A behavior change is done when a test demonstrates it and CI runs it, not when it
  compiles. Register new diagnostics in `tests/README.md` before using them. `AGENTS.md` has the full definition of done;
  every issue in the GitHub plan carries a `## Verification` section saying how it is proven.
- **Work one milestone at a time**, in order (`spec/stage0-subset.md` for stage 0, `spec/stage1-subset.md` for stage 1).
  A milestone is done when all tests in its folder and all earlier folders pass in debug and release.
- **Stay inside the current stage.** Stage 0 is complete; stage 1 is the current plan. Do not implement deferred features
  (stage 2 tasks, tooling, ecosystem) early, even when it would be convenient. Reserved words must still be rejected.
- **Diagnostics are part of the product.** Every error has a code from the registry in `tests/README.md`, a source span, and
  reports at the line the tests mark. Report every error in a file, not just the first.
- **Keep the checker local.** Memory checks look at one function body at a time, from signatures only. No whole-program
  analysis and no lifetime annotations.
- **Prefer simple and correct over clever.** This compiler will be read and extended for a long time, so keep it plain
  and easy to follow.

## Commands

```
python3 tests/run_tests.py               run the conformance tests against the compiler (Python 3; BURDIN=path overrides the binary)
python3 tests/run_tests.py m05 -v        only milestone 5, with diffs for failures
python3 tests/check_tests.py             check test directives and error codes (Python 3; no compiler needed)
python3 tests/check_e9000.py             check the E9000 audit against the compiler (no compiler needed)
python3 tests/check_unit_tests.py        check the front-end unit tests and their CI coverage (no compiler needed)
python3 tests/check_deps.py              audit the dependency pins and licenses (no compiler needed)
python3 tests/check_docs.py              check documentation links and the driver contract (needs a built driver)
python3 tests/check_debug_info.py        check -g, the line table, and a gdb breakpoint (needs a built driver)
python3 tests/check_codegen_opt.py       check drop-flag elimination, bounds-check elision, and shared glue
python3 tests/check_build_flags.py       check --version, --strip's size budget, --lto, and PGO
python3 tests/check_parallel.py          check parallel parsing and deterministic diagnostics
python3 tests/check_diagnostics.py       check color, width, formats, error limits, and ordering (needs a built driver)
python3 tests/check_error_docs.py        check the error-code reference and `burdin explain` (needs a built driver)
python3 tests/check_test_runner.py       check `burdin test` attributes, parallelism, output, isolation, and discovery
python3 tests/check_lint.py              check `burdin lint`: the documented lints, the fixtures in tests/lint, `--fix`, levels, and the manifest
python3 tests/check_doc.py               check `burdin doc`: the site and its golden pages, links, JSON, coverage, and the output rules
python3 tests/check_fmt.py               check `burdin fmt`: golden outputs, idempotence, --check, --stdin, --range, grouping
python3 tests/check_build_scripts.py     check build-script caching, declared inputs, `--emit-deps`, and `bindgen`
python3 tests/check_new.py               check `burdin new`: the generated package, refusals, invalid names, and the command line
python3 tests/check_release.py           check the release workflow, archives, checksums, installers, and version policy
python3 tests/build_release.py           build, smoke-test, and package one release target (--verify-reproducible)
python3 tests/release_notes.py           generate release notes from CHANGELOG.md and check their links
python3 tests/check_platform_support.py  check the platform support page against CI and binary-safe output (needs a built driver)
python3 tests/fuzz.py                    long lexer/parser fuzzing session; the scheduled CI job runs it (issue #447)
```

The test scripts are Python 3 and run unchanged on every platform (invoke them with `python3` on Unix and `python`
on Windows); on Windows the shell scripts that remain (`install.sh` and the CI helpers) run from Git Bash. The
conformance runner expects `burdin build`, `burdin run`, and `burdin test` as described at the top of
`tests/run_tests.py`.

## Layout

```
README.md                 the entry point: building, hello world, and the driver contract
CLAUDE.md                 the Claude-specific entry point and working rules
AGENTS.md                 the shared working agreement: definition of done, close-out checklist, verification policy
HANDOFF.md                current state, decisions, and what is next
CHANGELOG.md              shipped milestones
Cargo.toml                workspace
crates/
  burdin_diag/              spans, diagnostics, error codes
  burdin_syntax/            lexer, parser (parser/: items, types, statements, patterns, expressions), syntax tree
  burdin_tir/               the typed tree every later stage reads: types, expressions, programs, signatures, and the
                            questions about types (`TypeDefs`: Copy, Clone, Drop) and places
  burdin_sema/              name resolution and type checking, modules and imports, monomorphization of generics,
                            exhaustiveness (declare, imports, impls, instances, resolve, bodies, validate; fncx/
                            checks one function body)
  burdin_check/             return-origin, move, and borrow checks over the typed tree (spec/memory-rules.md)
  burdin_comptime/          the compile-time interpreter over the typed tree: constants, comptime expressions,
                            specialization of comptime parameters (spec/comptime.md)
  burdin_lint/              the linter (`burdin lint`, spec/lint.md): syntax-tree lints (names, imports, unused and
                            unreachable code, dead code, `else` on a local enum) and typed-tree lints (clones, copies,
                            `mut`, allocations in loops), suppression comments, levels, and the `--fix` edits; reads
                            what the compiler built and changes nothing
  burdin_codegen/           LLVM backend (emit/: one file per theme) and the generated runtime; taskrt/ holds the task
                            runtime (threads, locks, context switch) as LLVM IR text, linked in only when used
  burdin_fmt/               the formatter behind `burdin fmt`: its own lossless layout parser, the document printer,
                            comment and `use` handling, doc-comment code blocks, and the checks of its own output
                            (spec/fmt.md); it depends only on burdin_diag and burdin_syntax
  burdin_doc/               the documentation generator behind `burdin doc`: doc comments and signatures from the syntax
                            trees, intra-doc link resolution, the HTML site with search, the versioned JSON schema, and
                            the coverage report (spec/doc.md); it depends only on burdin_diag and burdin_syntax
  burdin_driver/            the `burdin` binary: build, run, test, fmt, doc, bindgen; package loading (`package.rs`: burdin.toml,
                            modules, dependencies), build scripts (`build.rs`), and the minimal C bindgen
                            (`bindgen.rs`)
  burdin_bench/             generated benchmark inputs and the `cargo bench` targets for lexing, parsing, name
                            resolution and type checking, and code generation
  burdin_fuzz/              deterministic lexer/parser fuzzing, the `burdin-fuzz-syntax` binary, and the committed
                            corpus the replay test parses (issue #447)
std/                      the standard library, written in Burdin
install.sh, install.ps1   the one-line installers for Unix and Windows, with SHA-256 verification
docs/                     the user-facing error-code reference (checked by tests/check_error_docs.py), the
                          diagnostics-for-tools reference and the JSON schema (docs/diagnostics.md,
                          docs/diagnostic.schema.json, checked by tests/check_diagnostics.py), the lint
                          reference (docs/lints.md, checked by tests/check_lint.py), the platform
                          support matrix and known limitations (checked by tests/check_platform_support.py), and the
                          version/release policy (docs/releasing.md)
.github/workflows/        ci.yml (every push) and release.yml (tag, nightly schedule, or manual dry run)
spec/                     grammar, memory rules, standard-library surface, iteration protocol, derives, extension
                          methods, test runner, stage 0 scope, stage 1 plan
tests/                    conformance tests and scripts
```

Data flows one way: `burdin_syntax` to `burdin_sema` to `burdin_check` to `burdin_comptime` to `burdin_codegen`, with
`burdin_tir` as the shared vocabulary and `burdin_diag` under everything. `burdin_lint` is a side branch: it reads the
syntax tree and the typed tree the compiler built, depends on `burdin_syntax`, `burdin_tir`, and `burdin_diag` only, and
is called from the driver, never from the compiler stages. `burdin_check`, `burdin_comptime`, and
`burdin_codegen` depend only on `burdin_tir` (and `burdin_diag`), not on sema. Keep everything before codegen free of
LLVM so the checks can be tested without a backend. There is no MIR (`burdin_mir`), decided in milestone 20 (issue
#158): sema monomorphizes, the checks read the typed tree, and the comptime engine interprets the same tree and
rewrites it before code generation.

## Toolchain

Rust is pinned by `rust-toolchain.toml` to an exact stable version with the `clippy` and `rustfmt` components (currently
`1.100.0`; edition 2024), so `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` give the same
verdict on a clean machine. Bump the version deliberately, together with a `cargo fmt --all` pass and a HANDOFF.md note;
`tests/check_ci.py` fails if the pin, its components, or CI's verification step goes missing. **LLVM 21** through
`inkwell` 0.11 (feature `llvm21-1-no-llvm-linking`), and the system C compiler driver as the linker against libc (`cc`
on Linux and macOS, MSVC `cl` on Windows; `BURDIN_CC` overrides). `crates/burdin_codegen/build.rs` links the shared LLVM
library on every platform: the static LLVM archives in the official Windows release are built with the static CRT and
crash when linked into the Rust binary, which uses the dynamic CRT. On Windows/MSVC the compiler links `LLVM-C.lib`, so
`LLVM-C.dll` must be on `PATH` for `burdin`, `burdin test`, and programs built with `--link-lib LLVM-C`.
There is no C runtime: `burdin_codegen/src/runtime.rs` generates `main`, output, panics, string allocation and
conversions, and heap allocation as LLVM IR that calls libc directly.
Platforms: Linux, macOS, Windows; x86-64 and arm64.

External Rust crates are exact-pinned, committed in `Cargo.lock`, and reviewed in `tests/dependencies.txt`:
`tests/check_deps.py` enforces the permissive-license policy and the pins (see `docs/dependencies.md`).

**Development happens in WSL Ubuntu 26.04 first** (decided). The repository stays on `F:`; build and test from WSL at
`/mnt/f/projects/burdin-lang`. Packages: `build-essential llvm-21-dev libpolly-21-dev clang-21 libzstd-dev zlib1g-dev`, and
Rust through rustup in `~/.cargo`. From PowerShell, run WSL commands with `wsl -d Ubuntu -e bash -lc '...'` (`-e` so the
outer shell does not expand `$?`), and keep Windows entries out of `PATH` inside WSL.

Windows builds are covered by the `windows` CI job (issue #12). Locally, install LLVM 21 from the official
`clang+llvm-21.1.8-x86_64-pc-windows-msvc.tar.xz` archive, set `LLVM_SYS_211_PREFIX` to it, put its `bin` on `PATH`,
and run from a Visual Studio Developer Command Prompt so `cl` is on `PATH` (or set `BURDIN_CC`). `tests/run_tests.py`
runs with Python 3 from any shell, Git Bash included.

Diagnostics that are not in the registry: `E9000` means "not implemented in this compiler yet". After stage 0 it marks
constructs that belong to stage 1 or later; each one gets a real diagnostic when its milestone arrives. Every site is
listed with its owning milestone and issue in [`spec/e9000-audit.md`](spec/e9000-audit.md), and
`tests/check_e9000.py` fails when a site is unassigned or belongs to a milestone that has shipped.
