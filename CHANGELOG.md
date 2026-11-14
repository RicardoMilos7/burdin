# Changelog

Burdin has no backward-compatibility promise (see `Burdin Language Design.md`). Entries are written per milestone; the
GitHub milestones and the roadmap track the work in progress. A milestone closes with an entry here (see the
close-out checklist in `AGENTS.md`).

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
- CI runs on the remote on two platforms. The `linux` job on `ubuntu-24.04` installs LLVM 21 from apt.llvm.org; the
  `windows` job on `windows-latest` loads the MSVC environment and installs the official LLVM 21 Windows archive.
  Both pass check-tests, fmt, clippy, unit tests, build, and every conformance milestone m01-m11; the Windows job also
  exercises `BURDIN_CC=cl`, binary-safe CRT output and files, abort exit codes, and `--link-lib LLVM-C` against
  `LLVM-C.lib`. `tests/check-ci.sh` guards both jobs' milestones and prerequisites.
