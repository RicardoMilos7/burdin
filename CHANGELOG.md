# Changelog

Burdin has no backward-compatibility promise (see `Burdin Language Design.md`). Entries are written per milestone; the
GitHub milestones and the roadmap track the work in progress. A milestone closes with an entry here (see the
close-out checklist in `AGENTS.md`).

## Unreleased — Infrastructure

- Every check and test tool under `tests/` is Python 3 (`tests/run_tests.py`, `tests/check_*.py`,
  `tests/build_release.py`, `tests/release_notes.py`, `tests/bench_regression.py`, `tests/fuzz.py`, and the shared
  `tests/harness.py`), one implementation for Linux, macOS, and Windows instead of the former shell scripts and the
  `check-tests.ps1` duplicate. The conformance runner keeps the same directives, diagnostics, output order, and exit
  codes, and runs tests in parallel by default (`-j N`; results are still printed in test order). CI invokes Python
  directly on all three platforms. This removes the Git Bash process-spawn cost that dominated the Windows CI job:
  the full suite takes 3 s instead of 31 s on WSL with the same compiler, and `check_docs.py` walks links in-process.
- The release pipeline (issue #452, sub-issues #453-#456): a `Release` workflow builds the six supported targets
  natively (`x86_64`/`aarch64` Linux, macOS, and Windows), smoke-tests `build`, `run`, and `test`, writes
  `burdin-<version>-<target>.tar.gz` archives and SHA-256 checksums, verifies that two checkout paths produce the
  same Linux archive, and publishes only from a `v<version>` tag or an explicit manual `publish`. A manual run
  defaults to a dry run, and the nightly schedule never publishes. `install.sh` and `install.ps1` install a release
  after verifying `SHA256SUMS`, with `BURDIN_VERSION`, `BURDIN_CHANNEL`, `BURDIN_INSTALL_DIR`, and
  `BURDIN_RELEASE_DIR` for the rehearsal path. `burdin --version` gained a `channel:` line (`dev`, `nightly`, or
  `stable`) backed by `BURDIN_CHANNEL` and `BURDIN_VERSION`; the workspace version became `0.1.0`. The version
  scheme (SemVer for the compiler, editions versioned separately), channel policy, breaking-change rule, artifact
  protocol, reproducibility rules, and rehearsal checklist are in `docs/releasing.md`.
  `tests/build-release.sh` builds and packages a target (and proves reproducibility), `tests/release-notes.sh`
  extracts a changelog section and checks its links, and `tests/check-release.sh` guards the workflow, the archive,
  the checksums, both installers, and the metadata in every CI job. No release has been cut.
- Dependency and license audit (issue #449). `Cargo.lock` is committed and pins every external crate by exact
  version, crates.io source, and checksum, and every direct requirement is written `=x.y.z`.
  `tests/dependencies.txt` is the reviewed inventory: one line per external crate with its license and why the
  compiler needs it (18 crates today, all transitive through `inkwell` 0.11 and `llvm-sys` 211). `tests/check-deps.sh`
  fails on an unreviewed crate, a version change, a lockfile without a checksum, a non-exact requirement, a
  non-permissive license, or a license that disagrees with the crate's own metadata. Every CI job runs it after
  `cargo build` with `BURDIN_DEPS_REQUIRE_METADATA=1`; `tests/check-ci.sh` guards the step, its position, and the
  inventory. `docs/dependencies.md` records the policy and the review, including LLVM's host-supplied
  Apache-2.0 WITH LLVM-exception license.

- The platform support matrix and its known limitations are documented in
  [`docs/platform-support.md`](docs/platform-support.md) (issue #457): the tested and untested OS/architecture pairs,
  the runtime features each CI job proves, and the gaps (no PDB and no symbolized backtraces on Windows, the untested
  Linux and Windows arm64 hosts, the untested libc assumptions, and the terminal-only output path). The page is linked
  from this file and the documentation index. `tests/check-platform-support.sh` checks the matrix against
  `ci.yml`, compiles and runs the page's example, and proves binary-safe standard output and file I/O byte-for-byte;
  every CI job runs it.

- Compiler and runtime debug/observability/performance work (issue #421, sub-issues #422-#432):
  `-g` emits source-level DWARF for functions, parameters, locals, and every expression's line and column
  (`tests/check-debug-info.sh`, which also proves a gdb breakpoint and local read); panics print a symbolized
  `stack backtrace:` after the unchanged message line in both profiles; standard output is block buffered (line
  buffered on a terminal) and flushed by `atexit`, `process.exit`, panics, and the stack-overflow handler; the main
  thread has an alternate-signal-stack guard that turns unbounded recursion into `panic: stack overflow`; generic
  glue is shared by structural type shape; drop flags are omitted for locals the move checker proves dead at every
  exit; constant and `0..len` loop indices skip their bounds checks; `--lto`, `--pgo-generate`, and `--pgo-use`
  expose LLVM LTO and PGO; `--strip` and size budgets (128 KiB hello, 512 KiB std-heavy) cover release binaries;
  `burdin --version` prints the version, target, LLVM version, commit, and profile; and modules are parsed in
  parallel with byte-identical diagnostics (`BURDIN_JOBS`). Each has a CI check: `check-debug-info.sh`,
  `check-codegen-opt.sh`, `check-build-flags.sh`, and `check-parallel.sh`.
- `burdin test` gained the runner ergonomics of issue #438 (sub-issues #439-#443, #445). Tests may carry
  `[ignore]`, `[timeout(ms)]`, `[should_panic]`/`[should_panic("text")]`, and `[should_fail]` attributes; an invalid
  attribute is E0170. Tests run concurrently (`-j`/`--jobs`, default: the CPU count) with a deterministic report,
  each in its own fresh temporary working directory with a clean environment and cleaned up afterwards. Output is
  captured and printed only for failures, with `--verbose` to show it always. `-p`, `--workspace`, and `--filter`
  select packages and tests, and dependency tests do not run by default. The rules are in `spec/test-runner.md`;
  `tests/m10_test_runner` adds four conformance tests and `tests/check-test-runner.sh` checks the runner black-box;
  every CI job runs it (issues #438-#443). `tests/check-tests.sh` and `.ps1` now fail when a registered error code
  has no compile-fail test and no documented exception, so a new diagnostic cannot ship untested (issue #445).
  Differential tests between comptime and compiled code (issue #444) remain blocked on the comptime interpreter
  (milestone 20); `spec/test-runner.md` records their intended shape.
- Settle the 16 assumptions in `tests/README.md` (issue #21, sub-issues #22-#40). The decisions are now rules in
  `spec/grammar.md`, `spec/memory-rules.md`, two new spec files (`spec/stdlib.md` for the standard-library surface
  and `spec/derives.md` for the exact derived members and FNV-1a hash values), and `spec/stage0-subset.md` /
  `spec/stage1-subset.md`; `tests/README.md` links each decision to its section. 40 conformance tests witness them.
  Four implementation gaps closed with the decisions: `Drop` rejects a read-only or `own` receiver (E0003),
  `CPtr.null` rejects arguments (E0004), the ordering operators work on `str` and `String`, and an invalid
  `extern "C"` type or convention is E0162.
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
- The front-end crates have focused Rust unit tests next to the conformance suite: the lexer token stream, parser
  recovery, type unification, exhaustiveness, the type and place questions in `burdin_tir`, the origin and move
  state in `burdin_check`, and diagnostics. CI runs `cargo test --workspace` in every job, and
  `tests/check-unit-tests.sh` fails when a front-end crate or one of the four named areas loses its tests
  (issue #45).
- Every CI job runs the finished conformance milestones in the release profile as well as debug, covering release
  code generation; tests marked `//@ debug-only` (the m02 overflow abort test) are skipped. `tests/check-ci.sh` fails
  if a job loses its release step, if the runner stops selecting the profile or skipping debug-only tests, or if no
  test is marked debug-only (issue #47).
- Diagnostics gained presentation and tooling (issues #433-#437). The human format colors the location, severity,
  and code when stderr is a terminal, honors `--color`, `--no-color`, and `NO_COLOR`, and wraps to `--width`,
  `COLUMNS`, or 80; `--diagnostic-format short` is the one-line machine format and `json` is one JSON object per
  diagnostic plus a summary object. Diagnostics carry notes and suggestions with spans and replacement edits
  (E0100/E0110 attach notes; E0114/E0115 attach the suggested `mut` edit). Output is sorted deterministically, and
  `--error-limit` truncates after sorting while still counting every error (unlimited by default). The new
  `docs/error-codes.md` documents every registry code with a compiled example and a fix, `burdin explain <code>`
  prints an entry, and every CI job runs `tests/check-diagnostics.sh` and `tests/check-error-docs.sh`. Unit tests
  cover the formats, color decision, wrapping, JSON shape, ordering, and limit in `burdin_diag` and `burdin_driver`.
- The compiler benchmarks gained regression tracking (issue #446): `cargo bench -p burdin_bench -- --save FILE`
  records a run's medians in a plain-text baseline, and `--baseline FILE` compares a later run with it, reporting
  every case more than `--threshold` percent slower (default 25) and exiting 1. `tests/bench-regression.sh` runs the
  full suite, and every CI job records and compares a baseline kept in `actions/cache`, keyed by the runner OS and
  architecture, the pinned toolchain, and `Cargo.lock`; a main push updates it and a pull request compares against
  the last main run. The comparison is advisory until the runner numbers are stable. `tests/check-ci.sh` fails if
  the step, the script, the cache, or the harness modes go missing, and `cargo test -p burdin_bench` covers the
  baseline format and the threshold arithmetic.
- The lexer and parser are fuzzed (issues #288 and #447). `burdin_fuzz` is a dependency-free, deterministic harness
  on the pinned stable toolchain: `burdin-fuzz-syntax` replays the committed corpus and then mutates it with a fixed
  seed, catching panics and watching for an input that stops making progress. `crates/burdin_fuzz/corpus/syntax/`
  holds the hand-written edge cases and every input that found a panic, and `cargo test --workspace` replays them
  plus 20,000 deterministic mutations in every CI job, so a reintroduced panic fails. A scheduled `fuzz` job runs
  a ten-minute session seeded by the run id (so each run explores new inputs) and uploads any found input as the
  `fuzz-crashes` artifact; committing it with the fix keeps the regression covered. `tests/check-ci.sh` fails if
  the schedule, the fuzz job, the long session, the artifact upload, or the corpus goes missing.

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
