# Sanitizer builds

`--sanitize=<list>` builds a program, or the test binary of `burdin test`, with LLVM's sanitizers (issue #620).
Safe Burdin should never trigger one: the memory rules, the bounds checks, and the overflow checks already rule
out what they look for. Their value is the code the language cannot check: `unsafe` blocks, raw `CPtr` arithmetic
and dereferences, C libraries called through `extern "C"` and `burdin bindgen`, and the stage 2 task and thread
runtime. The black-box checks are [`tests/check_sanitizers.py`](../tests/check_sanitizers.py), with one program per
bug class in [`tests/sanitize/`](../tests/sanitize); a scheduled CI job runs the stage 0 and stage 1 conformance
suite with `--sanitize=address`.

## The option

- **SAN1. Values.** `burdin build`, `burdin run`, and `burdin test` take `--sanitize=<list>` (or `--sanitize
  <list>`), a comma-separated list of:

  | Name | What it finds | How |
  | --- | --- | --- |
  | `address` | out-of-bounds reads and writes of the heap, the stack, and globals, use after free, double free, and (on Linux) leaks at exit | AddressSanitizer: every load and store is instrumented and the runtime replaces `malloc` and `free` |
  | `leak` | memory still allocated and unreachable at exit | LeakSanitizer's runtime alone; nothing is instrumented |
  | `thread` | data races between threads, including the task runtime's worker threads | ThreadSanitizer: every load, store, and atomic is instrumented |
  | `undefined` | integer overflow in every profile, and a null or misaligned `CPtr` dereference | checks in the generated code (SAN4); there is no runtime library |

  A name may appear once. `thread` cannot be combined with `address` or `leak`, because ThreadSanitizer keeps its
  own shadow memory and allocator. `--sanitize` cannot be combined with `--lto` or profile-guided optimization, and
  `burdin bench` does not take it (a sanitized measurement measures the sanitizer). Each of these is a usage error
  (exit status 2).
- **SAN2. Platforms.** The supported combinations are in the sanitizer table of
  [`docs/platform-support.md`](../docs/platform-support.md#sanitizers): Linux (x86-64 and arm64) has all four and
  `burdin fuzz`; macOS and Windows have `undefined` only. Asking for one the platform lacks is a usage error that
  names it and the platform: ``error: `--sanitize=address` is not supported on macos-aarch64; see the sanitizer
  table in docs/platform-support.md``. macOS is limited deliberately: Apple's AddressSanitizer runtime, dynamic
  because Darwin has no static one, looks for the address of its shadow memory at startup, and that search
  (`FindAvailableMemoryRange`, which walks the process's VM regions) does not finish for a Burdin binary on the macOS
  runners, so every instrumented program hangs before `main`. A sanitizer that cannot start is worse than none, so
  macOS gets the checks that need no runtime; the platform moves back to `address` once the runtime starts there.
- **SAN3. Linking.** `address`, `thread`, and `leak` link LLVM's runtime libraries (compiler-rt) with
  `-fsanitize=<list>`, so the linker is clang: `BURDIN_CC` when it names clang, otherwise `clang-21` or `clang` from
  `LLVM_SYS_211_PREFIX/bin` or `PATH`. A `BURDIN_CC` that is not clang, or no clang at all, is an error that says
  what to install. `undefined` alone links like any other build.
- **SAN4. `undefined`.** The checks a debug build has for integer overflow (`+`, `-`, `*`, negation, and the
  `MIN / -1` division) stay in a release build instead of wrapping, and every dereference of a `CPtr<T>` (a read,
  a write, or a field of the pointee) first checks that the pointer is not null and that its address is a multiple
  of `T`'s alignment. A failed check is an ordinary panic at the dereference or the operation, with its source
  location: `panic: null pointer dereference`, ``panic: misaligned pointer dereference: `i64` needs 8-byte
  alignment``, `panic: attempt to add with overflow`. A `[differential]` test compares its compiled run with the
  interpreter, which wraps in a release build (`comptime.md`, CT1), so release-mode overflow under `undefined`
  reports a divergence.
- **SAN5. Instrumentation.** Every function the compiler defines, the generated runtime and the standard library
  included, carries the sanitizer's attribute (`sanitize_address`, `sanitize_thread`), added before the profile's
  optimization pipeline; the instrumentation passes run after it, as clang orders them. An instrumented program is
  compiled as one codegen unit. The release profile still optimizes: a store to memory that is freed before it is
  read, or a C allocation that never escapes, may be removed before the sanitizer sees it, so the debug profile is
  the one to hunt memory errors in.
- **SAN6. Reports.** The sanitizers report on standard error in their own format and end the program with their
  own exit status (1 for AddressSanitizer and LeakSanitizer by default, 66 for ThreadSanitizer); `burdin run`
  passes that status through. In `burdin test`, a sanitizer report in a test process fails the test like any abort
  (`test-runner.md`, "Execution"), and the report is the captured standard error. A sanitized program does not
  install the runtime's stack-overflow guard: that guard is a `SIGSEGV` handler on an alternate stack, the same
  resource the sanitizer uses for its own reporting, so the sanitizer's handler is the one left in place. The runtimes read their usual
  options from the environment of `burdin run` (`ASAN_OPTIONS`, `TSAN_OPTIONS`, ...); a test process has a clean
  environment, so the defaults apply there.

## Tests

| Behavior | Test |
| --- | --- |
| `address`: an `unsafe` out-of-bounds write, a C use after free; `leak` and `address`: a C leak | `tests/sanitize/heap_overflow.bd`, `use_after_free.bd`, `leak.bd`, `tests/check_sanitizers.py` |
| `thread`: a data race between two threads | `tests/sanitize/data_race.bd`, `tests/check_sanitizers.py` |
| `undefined`: a null and a misaligned `CPtr` dereference, release-mode overflow | `tests/sanitize/null_deref.bd`, `misaligned.bd`, `overflow.bd`, `tests/check_sanitizers.py` |
| Statement temporaries a comparison, a borrowing method, a `for`, a closure call, or `hash` reads are freed (DR5): no leak under `address` or `leak` | `tests/sanitize/temporaries.bd`, `tests/check_sanitizers.py` |
| `burdin build` and `burdin test` with `--sanitize` | `tests/sanitize/tests.bd`, `tests/check_sanitizers.py` |
| Parsing, conflicts, and the platform table (SAN1-SAN2) | unit tests in `crates/burdin_driver/src/sanitize.rs`, `tests/check_sanitizers.py` |
| The stage 0 and stage 1 suite under `address` | `python3 tests/run_tests.py --sanitize address m01 ... m24` (the scheduled `sanitize` part of the Linux CI job) |
