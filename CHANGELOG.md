# Changelog

Burdin has no backward-compatibility promise (see `Burdin Language Design.md`). Entries are written per milestone; the
GitHub milestones and the roadmap track the work in progress. A milestone closes with an entry here (see the
close-out checklist in `AGENTS.md`).

## Unreleased — Infrastructure

- Retire the claude.ai copies of the design and stage 0 documents (issue #44): the repository files are canonical and
  say so, the external copies are recorded as historical, and the design document marks its snippets as illustrative.
- The driver contract is documented in a new root `README.md`: `burdin build`, `run`, and `test`, the diagnostic
  format, `--emit-ir`, `--link-lib` (including how `LLVM-C` resolves through `llvm-config`), `BURDIN_CC`, and the
  test output format. The file is the documentation index, and `tests/check-docs.sh` checks its links and the
  documented behavior; every CI job runs it (issue #42).
- Pin the Rust toolchain to `1.100.0` with the `clippy` and `rustfmt` components in `rust-toolchain.toml`, so
  `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are reproducible from a clean machine.
  CI installs and verifies that toolchain, and `tests/check-ci.sh` fails if the pin or its verification goes missing
  (issue #19).
- CI gained a `macos` job on `macos-latest` (Apple Silicon): Homebrew's `llvm@21`, the pinned Rust toolchain, and the
  same checks as Linux, including every conformance milestone m01-m11. `tests/check-platform.sh` proves the emitted
  LLVM triple follows the host and that `usize`/`isize` are 64-bit; it runs on every job. `tests/check-ci.sh` guards
  all three jobs. The driver resolves `--link-lib LLVM-C` for Homebrew's `libLLVM.dylib` layout, finds `llvm-config`
  through `LLVM_SYS_211_PREFIX`, and its resolution logic has unit tests (issues #16, #17, #18).
- CI gained a `windows` job on `windows-latest`: the MSVC environment through `ilammy/msvc-dev-cmd`, the official
  LLVM 21 Windows archive, and the same checks as the other jobs, including every conformance milestone m01-m11 and a
  `BURDIN_CC=cl` run. LLVM is linked as a shared library on every platform, because the static Windows archives crash
  in the dynamic-CRT Rust binary; the driver maps `--link-lib LLVM-C` to `LLVM-C.lib` and reports a missing `cl` with
  an actionable message. `process.run` exit codes are covered on both the POSIX and Windows CRTs (issues #12, #13,
  #14, #15).
- Add `cargo bench -p burdin_bench`: generated inputs measure the lexer, the parser, name resolution and type
  checking, and code generation at three sizes, reporting median time per iteration. `--smoke` runs every stage
  once with generous time budgets and is part of CI, and `HANDOFF.md` records the first baseline (issue #41).
- CI caches the cargo registry, `target/`, and LLVM 21 in all three jobs. The cargo cache is keyed by
  `rust-toolchain.toml` and `Cargo.lock`, so a toolchain or dependency change starts fresh; Linux caches the installed
  LLVM tree (including the multiarch `libLLVM` it symlinks to), macOS caches the Homebrew bottle downloads, and
  Windows caches the unpacked official archive. `tests/check-ci.sh` fails if any of those caches goes missing
  (issue #46).
- The compiler benchmarks gained regression tracking (issue #446): `cargo bench -p burdin_bench -- --save FILE`
  records a run's medians in a plain-text baseline, and `--baseline FILE` compares a later run with it, reporting
  every case more than `--threshold` percent slower (default 25) and exiting 1. `tests/bench-regression.sh` runs the
  full suite, and every CI job records and compares a baseline kept in `actions/cache`, keyed by the runner OS and
  architecture, the pinned toolchain, and `Cargo.lock`; a main push updates it and a pull request compares against
  the last main run. The comparison is advisory until the runner numbers are stable. `tests/check-ci.sh` fails if
  the step, the script, the cache, or the harness modes go missing, and `cargo test -p burdin_bench` covers the
  baseline format and the threshold arithmetic.

## Unreleased — Stage 1

Planned in `spec/stage1-subset.md`; tracked by milestones m12-m24 and the GitHub roadmap.


## Stage 0 — 2026-11-14

- Milestones 1-11 complete: lexer and parser, primitives and control flow, type checking, structs/enums/match,
  moves/Drop/Heap, conventions and references, generics/interfaces/derives, modules/extern/unsafe, and the stage 0
  standard library.
- 134 conformance tests pass in debug; 133 pass in release plus one debug-only test. `process.run` reports the child's
  exit code on the POSIX and Windows CRTs.
- LLVM 21 backend through `inkwell`, linked as a shared library on every platform (`crates/burdin_codegen/build.rs`);
  the runtime is generated LLVM IR calling libc directly.
- `burdin build`, `burdin run`, and `burdin test`; the test runner runs one process per test and reports
  `test result: N passed, M failed`.
- `test` blocks, test-mode `assert`/`assert_eq` returning `Result<(), AssertError>`, and abort recovery.
- `std` embedded in the driver: `String`, `List`, `Map`, `Result` with `?`, file I/O, process/environment helpers,
  `CString`/`CPtr`/`CStr`, and the test-mode assertions.
- The m11 LLVM C API program builds and verifies a function through `extern "C"` and `--link-lib LLVM-C`.
- CI runs on the remote: the `linux` job on `ubuntu-24.04` installs LLVM 21 from apt.llvm.org and passes
  check-tests, fmt, clippy, unit tests, build, and every conformance milestone m01-m11. `tests/check-ci.sh`
  guards the workflow's milestone coverage and prerequisites.
