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
   is the behavior of `burdin test`, `spec/bench.md` is the behavior of `burdin bench` (bench items, the harness,
   statistics, baselines, `black_box`, and the profiler hook), `spec/fuzz.md` is the behavior of `burdin fuzz`
   (fuzz items, the libFuzzer build, the corpus and artifacts, and `burdin test --fuzz-replay`), `spec/sanitizers.md`
   is `--sanitize` (AddressSanitizer, ThreadSanitizer, LeakSanitizer, the undefined checks, and the platforms),
   statistics, baselines, `black_box`, and the profiler hook), `spec/fmt.md` is the behavior of `burdin fmt` (the one style, comments, `use`
   sorting, doc-comment code blocks, and the command line), `spec/tasks.md` is tasks, the scheduler, scoped tasks, panics in tasks, and
   threads, `spec/channels.md` is `Channel<T>` and `select`, `spec/shared.md` is `Send`, `Sync`, atomics, `Mutex<T>`, `RwLock<T>`, and
   the atomic `Shared<T>`, `spec/io.md` is the reactor, the helper pool, `blocking` extern functions, sockets, name
   resolution, and timers, `spec/build-scripts.md` is `build.bd`, dependency grants, generated
   modules, caching, and the `bindgen` a script may invoke, `spec/bindgen.md` is `burdin bindgen`: libclang, the
   import rules, layout checks, wrappers, and `--export`; `spec/rust-interop.md` is Rust interop: rustdoc JSON, the C-ABI
   shim, `[rust.crates]`, the pinned toolchain, and the layout checks; `spec/fix.md` is the suggestion model, `burdin fix`, and editions;
   `spec/refactor.md` is `burdin ssr` and the refactorings (`burdin refactor`, the language server's structural search
   and extract/inline actions); `spec/lint.md` is `burdin lint` (the `[lint]` table,
   suppression comments, and `--fix`); `spec/doc.md` is `burdin doc`, the doc comments, the Markdown subset,
   intra-doc links and their diagnostics, the site and its search, the JSON schema, and the coverage report;
   `spec/check.md` is `burdin check`, the type-check-only command (what it shares with a build, and `--tests`);
   `spec/inspect.md` is the inspection commands: `burdin expand`, `--emit`, `burdin asm`, `burdin bloat`, and
   `burdin check --print-instances`;
   `spec/diagnostics.md` is the versioned JSON diagnostic format, the SARIF log, and the links to the error
   reference; `spec/query.md` is the query-based compiler library: the memo, its granularity, cancellation, the
   budget, the tool API, and the parity between the library and the command line; and `spec/lsp.md` is `burdin lsp`,
   the language server: transport, lifecycle, positions, packages, every feature, the editor targets, and their tests,
   `spec/repl.md` is `burdin repl`, the session model, the meta-commands, and the editor, `spec/scripts.md` is
   single-file scripts, the shebang, the inline manifest, and the binary cache, `spec/packages.md` is path, git, and
   official dependencies (`name = true`), `burdin.lock`, `--locked`, and `burdin add`, `spec/serialization.md` is the
   serialization framework: the data model, `Serialize`/`Deserialize` and the format-side `Serializer`/`Deserializer`,
   path-aware errors, the derives, and the test format, `spec/toolchain.md` is the toolchain manager: `burdin
   toolchain`, the `burdin` proxy, `burdin-toolchain.toml`, components, and `self update`/`uninstall`, and `spec/debug.md` is debugging: the
   panic hook, symbolized panic frames, variable lifetimes, the gdb/lldb printers, the Windows Natvis file,
   `burdin debug`, and the `burdin dap` debug adapter, and `spec/watch.md` is `burdin watch` and `--watch`: what is
   watched, the debounce, restarting `run`, and the changed-modules-first `test` run.
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
python3 tests/check_debug_printers.py    check the panic hook, symbolized frames, the gdb/lldb printers, the Natvis file, and `burdin debug`
python3 tests/check_dap.py               check `burdin dap` against a stub backend
python3 tests/check_codegen_opt.py       check drop-flag elimination, bounds-check elision, and shared glue
python3 tests/check_build_flags.py       check --version, --strip's size budget, --lto, and PGO
python3 tests/check_parallel.py          check parallel parsing and deterministic diagnostics
python3 tests/check_diagnostics.py       check color, width, formats, error limits, and ordering (needs a built driver)
python3 tests/check_error_docs.py        check the error-code reference and `burdin explain` (needs a built driver)
python3 tests/check_test_runner.py       check `burdin test` attributes, parallelism, output, isolation, and discovery
python3 tests/check_fuzz.py              check `burdin fuzz` and `burdin test --fuzz-replay` (needs clang and
                                         libFuzzer's runtime)
python3 tests/check_interpret.py         check `burdin test --interpret`: one fixture per strict-check class,
                                         and std code run in the interpreter
python3 tests/check_lint.py              check `burdin lint`: the documented lints, the fixtures in tests/lint, `--fix`, levels, and the manifest
python3 tests/check_fix.py               check `burdin fix`: suggestions, rounds, conflicts, the dirty tree, and `--migrate`
python3 tests/check_ssr.py               check `burdin ssr`: patterns, type constraints, resolution, search, and skipped matches
python3 tests/check_refactor.py          check `burdin refactor`: every refactoring and its refusal
python3 tests/check_doc.py               check `burdin doc`: the site and its golden pages, links, JSON, coverage, and the output rules
python3 tests/check_fmt.py               check `burdin fmt`: golden outputs, idempotence, --check, --stdin, --range, grouping
python3 tests/check_sanitizers.py        check `--sanitize`: one program per bug class under tests/sanitize/
                                         (needs clang and compiler-rt)
python3 tests/check_build_scripts.py     check build-script caching, declared inputs, `--emit-deps`, and `bindgen`
python3 tests/check_bindgen.py           check `burdin bindgen`: golden modules, compiled and run, wrappers, `--export`, and E0263
python3 tests/check_rust_interop.py      check Rust interop: the fixture crate's golden shim and module, layout checks, `[rust.crates]` builds, and E0264 (needs cargo)
python3 tests/check_new.py               check `burdin new`: the generated package, refusals, invalid names, and the command line
python3 tests/check_packages.py          check `burdin add` and `burdin.lock`: path and git dependencies, golden files, `--locked`, checksums
python3 tests/check_json.py              check the `json` official package: the tests/json fixtures, `burdin add json`, and its official pin
python3 tests/check_toolchain.py         check the toolchain manager and the `burdin` proxy against fixture releases (needs rustc and curl)
python3 tests/check_check.py             check `burdin check`: fixtures and goldens, equivalence with a build over the conformance suite, no codegen
python3 tests/check_inspect.py           check `burdin expand` (goldens, and every m17/m19/m20/m21 expansion re-checked and run), `--emit`, `burdin asm`, `burdin bloat`, `check --print-instances`
python3 tests/check_bench.py             check `burdin bench`: measurement, statistics, formats, selection, baselines, and `--profile-time`
python3 tests/check_lsp.py               check `burdin lsp`: the driver's server, a test run through a lens, and the editor files
python3 tests/check_repl.py              check `burdin repl`: the golden sessions, :ir, and :compile
python3 tests/check_scripts.py           check single-file scripts: the shebang, the inline manifest, and the cache
python3 tests/check_watch.py             check `burdin watch`: reruns on edits, ignored files, `run` restarts, changed-modules-first `test`
python3 tests/check_target.py            check the target directory's artifact cache: the layout, fingerprints, invalidation, the failure paths (spec/target.md)
python3 tests/check_release.py           check the release workflow, archives, checksums, installers, and version policy
python3 tests/build_release.py           build, smoke-test, and package one release target (--verify-reproducible)
python3 tests/release_notes.py           generate release notes from CHANGELOG.md and check their links
python3 tests/check_platform_support.py  check the platform support page against CI and binary-safe output (needs a built driver)
python3 tests/fuzz.py                    long lexer/parser fuzzing session; the scheduled CI job runs it (issue #447)
cargo test --workspace                   the Rust unit and integration tests, including `burdin_query` and the
                                         driver's parity test (`crates/burdin_driver/tests/parity.rs`)
cargo test -p burdin_lsp                 the language server's JSON-RPC integration tests, against `burdin-lsp` (no LLVM)
cargo test -p burdin_dap                 the debug adapter's scripted-peer tests (no LLVM, no debugger)
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
  burdin_bindgen/           `burdin bindgen` (spec/bindgen.md): libclang loaded at run time (`clang.rs`), the C
                            header walk (`parse.rs`), macro evaluation (`macros.rs`), the module's rendering
                            (`model.rs`), and C's spelling of an exported type (`export.rs`); depends only on libc
  burdin_rustgen/           the generator behind `burdin bindgen --rust` (spec/rust-interop.md): rustdoc JSON parsing
                            (`rustdoc.rs`), the exposed API and its classification (`api.rs`), and the Burdin module and shim
                            source (`render.rs`); it depends only on burdin_json and runs no tool, so the driver runs cargo
                            around it (`rust_interop.rs`, `rust_build.rs`)
  burdin_doc/               the documentation generator behind `burdin doc`: doc comments and signatures from the syntax
                            trees, intra-doc link resolution, the HTML site with search, the versioned JSON schema, and
                            the coverage report (spec/doc.md); it depends only on burdin_diag and burdin_syntax
  burdin_query/             the query-based compiler library (spec/query.md): the database of file texts, the parse
                            memo and the body-only reparse, the check cache's orchestration, cancellation and the
                            budget, the tool API (syntax tree with trivia, resolve/type/signature/ownership at a
                            position), the package loader (`package.rs`), and the `burdin.lock` format
                            (`lockfile.rs`); the driver and a language server share it, and it never links LLVM
  burdin_pkg/               the package resolver (spec/packages.md): git dependencies and their cache (`git.rs`),
                            resolution, the checksum, and keeping `burdin.lock` current or `--locked` (`resolve.rs`),
                            `burdin add`'s manifest edit (`add.rs`), and SHA-256 (`sha256.rs`); it depends on
                            burdin_query and burdin_syntax, runs only `git`, and the driver is its only caller
  burdin_toolchain/         the toolchain manager and the `burdin` proxy (spec/toolchain.md): names and versions
                            (`version.rs`), the home, settings, and receipt (`home.rs`), releases, resumable verified
                            downloads, and unpacking (`dist.rs`), installs and components (`install.rs`), selection and
                            `burdin-toolchain.toml` (`select.rs`), the proxy (`proxy.rs`), the commands (`cli.rs`,
                            `selfcmd.rs`); it links no compiler crate and no LLVM, only burdin_json and burdin_sha256
  burdin_sha256/            SHA-256, shared by the package resolver and the toolchain manager
  burdin_json/              the JSON value, its parser, and the `Content-Length` framing: shared by the language
                            server and the debug adapter
  burdin_dap/               the debug adapter behind `burdin dap` (spec/debug.md, DBG11): the lldb-dap child
                            (`backend.rs`), the `target` build and formatter import (`launch.rs`), and the four
                            augmentations (`adapter.rs`); its `burdin-dap` binary is the same server without LLVM
  burdin_refactor/          the refactoring engine behind `burdin ssr` and `burdin refactor` (spec/refactor.md):
                            structural search and replace, extract, inline, change signature, and move item, over
                            burdin_query; every result is checked before it is returned
  burdin_lsp/               the language server behind `burdin lsp` (spec/lsp.md): positions and URIs
                            (`text.rs`), one query database per package (`project.rs`),
                            the loop, lifecycle, cancellation, and progress (`server.rs`), and the features (`diagnostics.rs`,
                            `nav.rs`, `edit.rs`, `outline.rs`, `actions.rs`); its `burdin-lsp` binary is the same server
                            without LLVM, which the integration tests in `tests/` drive over JSON-RPC
  burdin_repl/              the REPL behind `burdin repl` (spec/repl.md): the session model and the input classifier
                            (`text.rs`), the check-evaluate-print flow and the meta-commands (`lib.rs`), completion
                            (`complete.rs`), the raw terminal (`terminal.rs`), the line editor with history
                            (`editor.rs`), and the highlighter (`highlight.rs`); it never links LLVM, and the driver
                            hands it `:compile` and `:ir`
  burdin_inspect/           the inspection commands (spec/inspect.md): the source expander behind `burdin expand`
                            (`expand.rs`), the monomorphization report (`instances.rs`), the assembly view (`asm.rs`),
                            the binary-size analysis and the ELF/Mach-O/COFF symbol reader behind `burdin bloat`
                            (`bloat.rs`, `object.rs`), and the typed-tree dump of `--emit tir` (`tir.rs`); it reads
                            what the compiler built, never links LLVM, and the driver is its only caller
  burdin_driver/            the `burdin` binary: build, run, test, bench, check, expand, asm, bloat, fmt, lint, doc, bindgen, new, add, repl, lsp; it runs its
                            front end through `burdin_query`, and keeps build scripts (`build.rs`), the bench runner
                            (`bench_runner.rs`), the linker choice (`linker.rs`), the bindgen command line
                            (`bindgen.rs`), and the export header built from the checked program (`export.rs`)
  burdin_bench/             generated benchmark inputs and the `cargo bench` targets for lexing, parsing, name
                            resolution and type checking, code generation, and the query library's keystroke edits
  burdin_fuzz/              deterministic lexer/parser fuzzing, the `burdin-fuzz-syntax` binary, and the committed
                            corpus the replay test parses (issue #447)
std/                      the standard library, written in Burdin
packages/                 the official packages, each an ordinary Burdin package declared with `name = true`; they ship
                          with the compiler (spec/packages.md, PKG15-PKG18); `tests/packages/official` has the fixtures
                          the tests use instead
editors/                  the VS Code extension (vscode/) and the Neovim and Helix setup for `burdin lsp`
debuggers/                the printers and visualizers (spec/debug.md, DBG6-DBG8): gdb (embedded in a `-g` binary),
                          lldb (written beside the session), and the Windows Natvis file
install.sh, install.ps1   the one-line installers for Unix and Windows, with SHA-256 verification
docs/                     the user-facing error-code reference (checked by tests/check_error_docs.py), the
                          diagnostics-for-tools reference and the JSON schema (docs/diagnostics.md,
                          docs/diagnostic.schema.json, checked by tests/check_diagnostics.py), the lint
                          reference (docs/lints.md, checked by tests/check_lint.py), the platform
                          support matrix and known limitations (checked by tests/check_platform_support.py), and the
                          version/release policy (docs/releasing.md)
.github/workflows/        ci.yml (pull requests and main; each OS split into four parallel parts) and release.yml (tag, nightly schedule, or manual dry run)
spec/                     grammar, memory rules, standard-library surface, iteration protocol, derives, extension
                          methods, test runner, the query library, stage 0 scope, stage 1 plan
tests/                    conformance tests and scripts
```

Data flows one way: `burdin_syntax` to `burdin_sema` to `burdin_check` to `burdin_comptime` to `burdin_codegen`, with
`burdin_tir` as the shared vocabulary and `burdin_diag` under everything. `burdin_query` sits beside the stages: it
drives them for a whole package, memoizes what an edit cannot affect, and is the one front end the driver and a tool
share. `burdin_lsp` is such a tool: it depends on `burdin_query`, `burdin_fmt`, `burdin_lint`, `burdin_syntax`, and
`burdin_diag`, never on LLVM, and the driver calls it for `burdin lsp`. `burdin_lint` is a side branch: it reads the
syntax tree and the typed tree the compiler built, depends on `burdin_syntax`, `burdin_tir`, and `burdin_diag` only, and
is called from the driver, never from the compiler stages. `burdin_bindgen` is another: it depends on nothing in the
compiler (only libc, for loading libclang), and the driver calls it for `burdin bindgen`. `burdin_check`, `burdin_comptime`, and
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
