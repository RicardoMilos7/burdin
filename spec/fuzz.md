# Fuzz testing

`burdin fuzz <target> <name>` builds one fuzz target with libFuzzer's coverage instrumentation and
AddressSanitizer, links libFuzzer's `main`, and lets it search for inputs that crash the target (issue #618). A crash
is a panic: a failed `assert`, an index out of bounds, an arithmetic overflow, or a memory error that
AddressSanitizer reports (`spec/sanitizers.md`). The black-box check is
[`tests/check_fuzz.py`](../tests/check_fuzz.py), with a package that has a planted bug in
[`tests/fuzz/planted/`](../tests/fuzz/planted).

## The item

- **FZ1. Syntax.** `fuzz name(data: [u8]) { ... }` is a contextual item, like `test` and `bench`: `fuzz` starts one
  only when an identifier and a parameter list follow it, and is an ordinary identifier everywhere else. The
  parameter must be exactly one, read-only and typed `[u8]`; the target returns nothing and takes no type
  parameters or default arguments. The parser accepts any signature and the checker reports what is wrong with it as
  E0173, so a bad target never becomes a confusing follow-on error. Two targets of one package cannot share a name
  (also E0173). The item compiles as the private function `__burdin_fuzz_<name>` taking the `[u8]` by address.
- **FZ2. Where it lives.** Like `test` and `bench`, the item stays out of `File.items`: a normal build and `burdin
  run` strip it, so a fuzz target costs a released program nothing. `burdin fuzz` declares the entry package's
  targets and checks the body of the one it builds; `burdin test` declares and checks them all, because `--fuzz-replay`
  runs them (FZ6). A dependency package's targets are never built: a fuzz target belongs to the package it tests.
- **FZ3. The entry point.** A `burdin fuzz` build emits libFuzzer's `LLVMFuzzerTestOneInput(data, size)` — it wraps
  the input as a `[u8]` and calls the target — and `LLVMFuzzerInitialize`, which prepares the process the way `main`
  does (the arguments, binary-safe output, the flush at exit, the stack guard). libFuzzer's own `main` drives them.
  The build carries `sanitize_address` on every function the compiler defines and the coverage instrumentation
  libFuzzer reads (edge counters, the PC table, comparison tracing), and it links `-fsanitize=fuzzer,address` through
  clang (`spec/sanitizers.md`, SAN3) with the compiler driver's own linker, since the coverage starts up from a module
  constructor in `.ctors` that lld would leave unrun.
- **FZ4. The directories.** A target's corpus is `fuzz/corpus/<name>/` and its crashes are `fuzz/artifacts/<name>/`,
  next to the entry package's `burdin.toml`, or next to a single-file target. Both are created when missing and
  should be committed (the corpus makes the next run start where the last one stopped; a crash input is the bug's
  record). `burdin fuzz` passes `-artifact_prefix` so libFuzzer writes its crash files there.
- **FZ5. The command line.** `burdin fuzz <target> <name>` takes `--max-time <seconds>` (libFuzzer stops on its own
  after that long), `--minimize <file>` (shrink a crashing input instead of fuzzing; the result is written to the
  artifacts directory as `minimized-<file>`), `--release`, `-g`, `--locked`, `-j`, `--comptime-steps`, and, after
  `--`, libFuzzer options of its own (`-jobs=4`, `-dict=file`, ...). A name no target of the package has is a usage
  error that lists the names it does have. `--sanitize`, `--lto`, PGO, `-o`, `--emit`, and `--tests` do not apply: the
  build is already instrumented, and its binary is libFuzzer's. The exit status is libFuzzer's, and a crash prints a
  note naming the artifacts directory. Linux only (SAN2): libFuzzer needs AddressSanitizer's runtime, which starts on
  Linux but not on macOS or Windows, so a fuzz build elsewhere is a usage error that says so.
- **FZ6. Replay.** `burdin test --fuzz-replay` adds one case per saved input to the run: `<package>::fuzz <target>:
  <file>`, listed and reported like a test. Each case starts the package's test binary with the target's dispatcher
  index and the absolute path of the input, so it runs in the same isolated process as any test; the dispatcher reads
  the file and calls the target on its bytes. The case passes when the target finishes, and fails when it panics or
  AddressSanitizer reports — the input is then named in the failure. A passing replay is checked for leaked
  allocations like a passing test (`spec/allocators.md`, AL9), and an unreadable input fails the case. A package with
  no saved inputs runs exactly the tests it runs without the flag.
- **FZ7. What is not here.** No `Arbitrary` derive and no typed inputs: a target takes bytes and decodes them. A
  fuzz target is not a conformance test — the conformance suite has no directive for it — and `burdin fuzz` is not
  part of `cargo test`.

## Tests

| Behavior | Test |
| --- | --- |
| The signature rules and the duplicate name (FZ1) | `tests/m10_test_runner/fuzz_invalid.bd` |
| A planted out-of-bounds read found by `burdin fuzz --max-time 60` (FZ3, FZ4) | `tests/fuzz/planted/src/main.bd`, `tests/check_fuzz.py` |
| The saved crash, replayed by `burdin test --fuzz-replay` (FZ6) | `tests/fuzz/planted/fuzz/artifacts/header/crash-planted`, `tests/check_fuzz.py` |
| Fuzz targets stripped from a build, and `--fuzz-replay` on a package with none | `tests/check_fuzz.py` |
| Formatting a target (FZ1) | `crates/burdin_fmt/tests/regressions.rs` |
| The usage errors (FZ5) | `tests/check_fuzz.py` |
