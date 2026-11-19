# Burdin agent guide

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. The compiler is written in Rust and stays in Rust. This file is the shared
working agreement for any AI agent contributing to the repository; `CLAUDE.md` is the Claude-specific entry point.

Behavior is defined by `tests/`, `spec/grammar.md`, and `spec/memory-rules.md`, in that order. If this file and a spec
disagree about behavior, the spec wins and the disagreement should be raised instead of guessed.

## Read order

1. `tests/README.md` — the test format, the error-code registry, and the 16 settled decisions.
2. `spec/grammar.md`, `spec/memory-rules.md` — syntax and the exact memory semantics.
3. `spec/stdlib.md`, `spec/derives.md`, `spec/format.md`, `spec/errors.md`, `spec/arguments.md`,
   `spec/extensions.md`, `spec/operators.md`, `spec/allocators.md`, `spec/tasks.md`, `spec/channels.md`, `spec/shared.md`, `spec/build-scripts.md`, `spec/bindgen.md`, `spec/rust-interop.md`, `spec/diagnostics.md`, `spec/query.md`, `spec/lsp.md`, `spec/packages.md`, `spec/toolchain.md`, `spec/watch.md` — the standard-library
   surface, the exact derived members, formatting, error handling, default and named arguments, extension methods,
   operator overloading with `Index`/`IndexMut`, allocators and `Shared<T>`, tasks and threads, channels and `select`, shared state (`Send`, `Sync`, atomics, locks), `build.bd`, `burdin bindgen`, the JSON and SARIF diagnostic formats, the query-based compiler library, the language server, dependencies with `burdin.lock` and `burdin add`, and the toolchain manager.
4. `spec/stage1-subset.md` — the milestone list and its decisions. `spec/iterators.md`, `spec/existentials.md`,
   `spec/test-runner.md`, and `spec/bench.md` describe the iteration protocol, `any`/`impl`, `burdin test`, and
   `burdin bench`.
5. `spec/stage0-subset.md` (complete) and `HANDOFF.md` for the current state; `Burdin Language Design.md` for the
   full rationale.
6. `CHANGELOG.md` for what has shipped; the GitHub roadmap and milestones for what is planned.

The files in this repository are the only maintained copies. Documents hosted on claude.ai are historical: they are
not updated and may disagree with the repository.

## Commands

```
python3 tests/run_tests.py               run the conformance tests (Python 3; parallel by default)
python3 tests/run_tests.py m05 -v        one milestone, with diffs
python3 tests/run_tests.py --release     release profile (skips debug-only tests)
python3 tests/check_tests.py             validate directives and error codes (Python 3; no compiler needed)
python3 tests/check_e9000.py             check the E9000 audit against the compiler (no compiler needed)
python3 tests/check_unit_tests.py        check the front-end unit tests and their CI coverage (no compiler needed)
python3 tests/check_ci.py                validate the CI workflow, its caches, and the pinned Rust toolchain (no compiler needed)
python3 tests/check_deps.py              audit the dependency pins and licenses (no compiler needed)
python3 tests/check_platform.py          check the host triple and 64-bit isize/usize (needs a built driver)
python3 tests/check_platform_support.py  check the platform support page against CI and binary-safe output (needs a built driver)
python3 tests/check_docs.py              check documentation links and the driver contract (needs a built driver)
python3 tests/check_debug_info.py        check -g's DWARF, line table, and a gdb breakpoint (needs a built driver)
python3 tests/check_debug_printers.py    check the panic hook, the symbolized frames, the gdb/lldb printers, the Natvis file, and `burdin debug`
python3 tests/check_dap.py               check `burdin dap`, the debug adapter, against a stub backend
python3 tests/check_codegen_opt.py       check drop-flag elimination, bounds-check elision, and shared glue
python3 tests/check_build_flags.py       check --version, --strip's size budget, --lto, and PGO
python3 tests/check_parallel.py          check parallel parsing and deterministic diagnostics
python3 tests/check_diagnostics.py       check color, width, formats, error limits, and ordering (needs a built driver)
python3 tests/check_error_docs.py        check the error-code reference and `burdin explain` (needs a built driver)
python3 tests/check_test_runner.py       check `burdin test` attributes, parallelism, output, isolation, and discovery
python3 tests/check_lint.py              check `burdin lint`: the documented lints, the fixtures in tests/lint, `--fix`, levels, and the manifest
python3 tests/check_doc.py               check `burdin doc`: the site and its golden pages, links and their diagnostics, JSON, coverage, and output
python3 tests/check_fmt.py               check `burdin fmt`: golden outputs, idempotence, --check, --stdin, --range, grouping
python3 tests/check_build_scripts.py     check build-script caching, declared inputs, `--emit-deps`, and `bindgen`
python3 tests/check_bindgen.py           check `burdin bindgen`: golden modules, compiled and run, wrappers, `--export`, and E0263
python3 tests/check_rust_interop.py      check Rust interop: the fixture crate's golden shim and module, layout checks, `[rust.crates]` builds, and E0264 (needs cargo)
python3 tests/check_new.py               check `burdin new`: the generated package, refusals, invalid names, and the command line
python3 tests/check_packages.py          check `burdin add` and `burdin.lock`: path and git dependencies, golden files, `--locked`, checksums
python3 tests/check_toolchain.py         check the toolchain manager and the `burdin` proxy against fixture releases (needs rustc and curl)
python3 tests/check_check.py             check `burdin check`: fixtures and goldens, equivalence with a build over the conformance suite, no codegen
python3 tests/check_inspect.py           check `burdin expand` (goldens, and every m17/m19/m20/m21 expansion re-checked and run), `--emit`, `burdin asm`, `burdin bloat`, `check --print-instances`
python3 tests/check_bench.py             check `burdin bench`: measurement, statistics, formats, selection, baselines, and `--profile-time`
python3 tests/check_lsp.py               check `burdin lsp`: the driver's server, a test run through a lens, and the editor files
python3 tests/check_repl.py              check `burdin repl`: the golden sessions, :ir, and :compile
python3 tests/check_scripts.py           check single-file scripts: the shebang, the inline manifest, and the cache
python3 tests/check_watch.py             check `burdin watch`: reruns on edits, ignored files, `run` restarts, changed-modules-first `test`
python3 tests/check_release.py           check the release workflow, archives, checksums, installers, and version policy
python3 tests/build_release.py           build, smoke-test, and package one release target (--verify-reproducible)
python3 tests/release_notes.py           generate release notes from CHANGELOG.md and check their links
cargo fmt --all --check            formatting
cargo clippy --all-targets -- -D warnings
cargo test                         unit tests
cargo test -p burdin_lsp           the language server's JSON-RPC integration tests (no LLVM)
cargo test -p burdin_dap           the debug adapter's scripted-peer tests (no LLVM, no debugger)
cargo bench -p burdin_bench        compiler benchmarks (lex, parse, sema, codegen)
cargo bench -p burdin_bench -- --smoke   benchmark budgets CI runs
cargo bench -p burdin_bench -- --save FILE       record a benchmark baseline
cargo bench -p burdin_bench -- --baseline FILE   compare with a baseline (exit 1 on a regression)
python3 tests/bench_regression.py        benchmark regression check CI runs (advisory, issue #446)
python3 tests/fuzz.py                    long lexer/parser fuzzing session (the scheduled CI job; issue #447)
python3 tests/fuzz.py --seconds 60       a short local session; found inputs go to fuzz-out/crashes/
```

Development happens in WSL Ubuntu first. Every check and test tool under `tests/` is Python 3, so it behaves the
same on every host (invoke it with `python3` on Unix and `python` on Windows); the remaining shell scripts
(`install.sh` and the CI helpers) run from Git Bash on Windows.

## Working rules

- **Tests are the spec.** Never change a test to make it pass. If a test is wrong, explain why and ask; changing the
  spec means updating the spec file and the tests first, then the code.
- **Every change ships with tests.** A behavior change is done when a test demonstrates it and CI runs it, not when it
  compiles. Register a new diagnostic in `tests/README.md` before using it.
- **Work one milestone at a time**, in order. A milestone is done when its folder and every earlier folder pass in
  debug and release.
- **Stay inside the current stage.** Stage 0 is complete; stage 1 is the current plan. Do not implement stage 2 or
  stage 3 features early. Reserved words stay reserved until their milestone.
- **Diagnostics are part of the product.** Every error has a registry code, a span, and reports at the line the tests
  mark. Report every error in a file, not just the first.
- **Keep the checker local.** Memory checks look at one function body at a time, from signatures only. No whole-program
  analysis and no lifetime annotations.
- **Prefer simple and correct over clever.** The compiler will be read and extended for a long time.
- **The crate layout is not fixed.** The map below is the current shape, not a constraint. When a milestone needs a new
  responsibility with its own boundary (an LSP server, a package resolver, a formatter), add a new crate under
  `crates/` for it and extend the workspace rather than piling code into an existing crate. Keep the one-way dependency
  direction: syntax to sema to check to codegen, `burdin_tir` as the shared vocabulary, `burdin_diag` under everything,
  and nothing before codegen touching LLVM. Update this file's map when a crate is added, removed, or renamed.
- **Update the docs with the code.** `HANDOFF.md`, the relevant `spec/*.md`, and `CHANGELOG.md` change in the same
  milestone as the behavior.

## Definition of done

### A change

- The behavior is implemented (or the decision is written down).
- A test proves it: conformance, unit, CLI, integration, or a benchmark, depending on the area.
- New diagnostic codes are registered in `tests/README.md` first.
- The suite passes in debug and release.
- The closing comment names the test paths and the exact command(s) that pass, plus any manual verification.

### A milestone

- [ ] Every test in its folder and every earlier folder passes in debug and release.
- [ ] `.github/workflows/ci.yml` lists the finished milestone's folder.
- [ ] `HANDOFF.md` describes the new behavior and the remaining gaps.
- [ ] The relevant `spec/*.md` is updated (rule IDs and test lists included).
- [ ] `CHANGELOG.md` has an entry for the milestone.
- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass.
- [ ] Every issue in the milestone is closed with its verification result, and the GitHub milestone is closed.

## Verification by area

| Area | Required evidence |
| --- | --- |
| Language / runtime behavior | Conformance tests in `tests/<milestone>/`: `//@ run` with exact `//>` output, `//@ compile-fail` with `//~ E....`, `//@ run-abort`, `//@ exit-code`, `//@ burdin-test` |
| Compiler internals | Rust unit tests in the owning crate plus an end-to-end conformance test |
| Diagnostics | Compile-fail test with the registry code; golden message test where wording matters |
| Driver / CLI / build tool | Black-box tests with fixture packages, success and failure paths, golden output where stable |
| LSP / editors | JSON-RPC integration tests over stdio, grammar fixtures, manual checklist for packaging |
| Performance | Benchmark with before/after numbers plus a correctness test that behavior (including panics) is unchanged |
| Documentation | Snippets that compile/run, commands run from a clean checkout, link checks |
| Release / infrastructure | CI check or script; a rehearsal checklist for installers and external services |
| Decisions | The rule written in `spec/` plus the tests that witness it, before implementation |

## Issue workflow

- Every GitHub issue ends with a `## Verification` section. If it is missing, add it before starting.
- **Every pull request states which issue it closes**, in the body, with a closing keyword and the issue number
  (`Closes #12`), one line per issue when it closes several. A PR that closes no issue says why in its body instead of
  leaving it unstated.
- Decision issues are closed only when the rule and its tests land; implementation follows in the feature issue.
- Close an issue only with a comment naming the test paths and the command(s) that pass.
- Process rules (this file, the close-out checklist, CI policy, changelog policy) are not issues. They live here and in
  `CLAUDE.md`; an issue that only states a rule should be folded into this file and closed.
- The milestones and the roadmap are the plan of record; `spec/stage1-subset.md` is the written rationale.

## Repository map

```
README.md                 the entry point: building, hello world, and the driver contract
CLAUDE.md                 Claude-specific entry point and working rules
AGENTS.md                 this shared working agreement
HANDOFF.md                current state, decisions, and what is next
CHANGELOG.md              shipped milestones
spec/                     grammar, memory rules, standard-library surface, derives, comptime, the linter, the
                          documentation generator, the query library, the language server, stage 0 scope, stage 1 plan
docs/                     the user-facing error-code reference (checked against tests/README.md), the diagnostics-for-tools
                          reference and JSON schema (docs/diagnostics.md, docs/diagnostic.schema.json), the lint
                          reference (docs/lints.md, checked by tests/check_lint.py), the platform support matrix
                          and known limitations, and the version/release policy (docs/releasing.md)
install.sh, install.ps1   the one-line installers for Unix and Windows, with SHA-256 verification
.github/workflows/        ci.yml (pull requests and main; each OS split into four parallel parts) and release.yml (tag, nightly schedule, or manual dry run)
tests/                    conformance suite, README with the format and error codes, and the release scripts
crates/                   burdin_diag, burdin_syntax, burdin_tir, burdin_sema,
                          burdin_check, burdin_comptime, burdin_codegen, burdin_driver, burdin_fmt (the formatter),
                          burdin_lint (the linter), burdin_doc (the documentation generator),
                          burdin_query (the query-based compiler library),
                          burdin_pkg (the package resolver: git dependencies, `burdin.lock`, `burdin add`),
                          burdin_toolchain (`burdin toolchain` and the `burdin` proxy), burdin_sha256 (SHA-256),
                          burdin_lsp (the language server behind `burdin lsp`),
                          burdin_json (the JSON value and framing both protocol servers share),
                          burdin_dap (the debug adapter behind `burdin dap`),
                          burdin_repl (the REPL behind `burdin repl`),
                          burdin_inspect (`burdin expand`, `asm`, `bloat`, and the instance report),
                          burdin_bench (generated inputs and `cargo bench` targets),
                          burdin_fuzz (lexer/parser fuzzing and the committed corpus)
std/                      the standard library, written in Burdin
editors/                  the VS Code extension and the Neovim and Helix setup for `burdin lsp`
debuggers/                the pretty-printers: gdb (`gdb/`, embedded in a `-g` binary, `spec/debug.md` DBG6), lldb
                          (`lldb/`, written beside the session, DBG7), and the Windows Natvis file (`windows/`, DBG8)
```
