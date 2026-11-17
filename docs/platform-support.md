# Platform support and known limitations

Burdin is a personal experiment, not a production compiler, and it makes no stability promise. This page records
the operating systems and architectures it supports and tests, which runtime features the test suite exercises on
each one, and the gaps that are known today. Behavior is defined by the conformance suite and the specifications;
this page describes how far that evidence reaches.

The page is checked in CI. `tests/check_platform_support.py` requires the support matrix to match the jobs in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml), every test named below to exist, every script it names to
be a CI step, its checked example to compile and print the documented output, and binary-safe standard output and
file I/O to hold byte-for-byte on the running platform. [`tests/check_docs.py`](../tests/check_docs.py) checks the
links. When a platform, a check, or a gap changes, update this page in the same change.

## Support matrix

| OS | Architecture | Target triple | Support | Evidence |
| --- | --- | --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-gnu` | Tested | `linux` job on `ubuntu-24.04`: m01-m11 in the debug and release profiles, every check script, LLVM 21 from apt.llvm.org, and `cc` (gcc and glibc) linking |
| macOS | arm64 | `aarch64-apple-darwin` | Tested | `macos` job on `macos-latest` (Apple Silicon): the same suite and scripts, Homebrew `llvm@21`, and Apple `cc` (clang and libSystem) linking |
| Windows | x86-64 | `x86_64-pc-windows-msvc` | Tested | `windows` job on `windows-latest` (MSVC): the same suite and scripts, LLVM 21 from the official archive, MSVC `cl` linking, and a `BURDIN_CC=cl` rerun of m01 and m11 |
| Linux | arm64 | `aarch64-unknown-linux-gnu` | Untested | The compiler uses the host triple and `tests/check_platform.py` recognizes the host, but no CI job runs the suite there |
| macOS | x86-64 | `x86_64-apple-darwin` | Untested | Same: the `macos` job runs on Apple Silicon, and no job runs the suite on Intel |
| Windows | arm64 | `aarch64-pc-windows-msvc` | Untested | Same; the official LLVM 21 Windows archive that CI uses is x86-64, so a native arm64 LLVM build would come first |
| other OS or 32-bit | — | — | Unsupported | `tests/check_platform.py` reports a skip, and nothing in the build or the suite exercises it |

The compiler targets the host it runs on (`TargetMachine::get_default_triple` in `burdin_codegen`); there is no
cross-compilation flag. "Tested" means the full conformance suite (m01-m11) passes in the debug and release profiles
and every check script runs on that host in CI. "Untested" means the code path is expected to work because nothing
in it is architecture-specific, but no job proves it, so a failure there is not a CI regression. "Unsupported" means
no claim at all.

## What each platform proves

| Runtime feature | Checked by | Linux x86-64 | macOS arm64 | Windows x86-64 |
| --- | --- | --- | --- | --- |
| Language, runtime, and code generation (m01-m11) | `tests/run_tests.py` (debug and release) | yes | yes | yes |
| Host triple and 64-bit `usize`/`isize` | `tests/check_platform.py` | yes | yes | yes |
| Binary-safe standard output and file I/O | `tests/check_platform_support.py`, `tests/m09_stdlib/file_io_binary.bd` | yes | yes | yes |
| Buffered output flushed on return, `process.exit`, and panics | `tests/m02_basics/panic_flushes_stdout.bd`, `tests/m09_stdlib/exit_code.bd` | yes | yes | yes |
| Panic message and backtrace | `tests/m02_basics/panic_backtrace.bd` | yes; names in debug | yes; names in debug | yes; addresses only |
| Stack-overflow guard | `tests/m02_basics/stack_overflow_aborts.bd` | yes (SIGSEGV) | yes (SIGSEGV) | yes (vectored handler, 64 KiB stack guarantee) |
| `burdin test` runner | `tests/check_test_runner.py` | yes | yes | yes |
| `-g` debug information | `tests/check_debug_info.py` | yes (DWARF; gdb when installed) | yes (DWARF in `.dSYM`; gdb when installed) | metadata only; no PDB |
| `--strip` and the size budget | `tests/check_build_flags.py` | yes | yes | flags only; no size budget |
| `--lto`, `--pgo-generate`, `--pgo-use` | `tests/check_build_flags.py` | yes, with an LLVM driver | yes, with an LLVM driver | `--lto`/`--pgo-generate` rejected by `cl`; `--pgo-use` untested |
| Diagnostics formats, color, width, and limits | `tests/check_diagnostics.py` | yes | yes | yes |
| Error-code reference and `burdin explain` | `tests/check_error_docs.py` | yes | yes | yes |
| C bridge (`extern "C"`, `CPtr`, `CString`) | `tests/m08_modules/extern_libc.bd`, `tests/m08_modules/cstr_roundtrip.bd` | yes | yes | yes |
| LLVM C API (`--link-lib LLVM-C`) | `tests/m11_llvm/llvm_return_42.bd` | yes | yes | yes, with `LLVM-C.dll` on `PATH` |
| Parallel parsing and deterministic diagnostics | `tests/check_parallel.py` | yes | yes | yes |
| Lexer/parser fuzzing | `cargo test --workspace` in every job; `tests/fuzz.py` in the scheduled `fuzz` job | corpus replay and the long session | corpus replay | corpus replay |

The `windows` job runs every script in the table through Git Bash. `tests/check_platform_support.py` is the check
that these columns stay true: it fails when a script or a conformance test named here disappears, when a script
stops being a CI step, or when a milestone it names leaves the workflow's conformance run.

## Known limitations

### Windows

- **No source-level debugging and no symbolized backtraces.** `-g` emits LLVM debug metadata, but MSVC's `link.exe`
  keeps no DWARF sections in the image and the driver produces no PDB, so a debugger has no source lines and a
  panic backtrace prints bare addresses. `tests/check_debug_info.py` skips its DWARF-section and gdb checks on
  Windows, and `tests/m02_basics/panic_backtrace_names.bd` is skipped there. The metadata is still emitted
  (`--emit-ir` shows it), so the gap is the debug-information format (CodeView/PDB), not the front end.
- **`--lto` and `--pgo-generate` need an LLVM compiler driver.** MSVC `cl` rejects both, and the Windows CI job does
  not set `BURDIN_CC=clang`, so neither path is tested there. `--pgo-use` compiles the profile through `opt` and
  `llc` and then links normally; it is not exercised on Windows either.
- **The size budget is not enforced.** `--strip` maps to `/OPT:REF /OPT:ICF` and the program runs the same, but
  `tests/check_build_flags.py` enforces its 144 KiB and 512 KiB budgets only on Linux and macOS.
- **Exit statuses are truncated to 8 bits**, and an abort is only required to be abnormal. `burdin run` passes a
  Windows status through after truncation (a non-zero status stays non-zero); the `//@ run-abort` tests assert an
  abnormal end on every platform rather than a particular status, because POSIX's SIGABRT status has no Windows
  equivalent.
- **The terminal path of the output buffer is not exercised.** Standard output is block buffered, and line
  buffered when fd 1 is a terminal. CI always redirects stdout to a pipe or a file, so the `_isatty(1)` decision
  and the line-buffered path are untested everywhere.
- **MinGW and clang-cl are untested.** The driver recognizes an MSVC-style driver by its name (`cl` or `cl.exe`)
  and passes MSVC arguments; another Windows toolchain is neither supported nor tested.
- **Binary mode is implemented and checked.** The runtime switches fds 0-2 with `_setmode` and opens files with
  `rb`/`wb`, so there is no newline translation. `tests/check_platform_support.py` proves both on the Windows job
  by inspecting the raw bytes. The conformance suite alone could not: a text-mode round-trip through the same API,
  and the runner's `\r\n` normalization of stdout, hide the translation.

### arm64

- **Only macOS arm64 is tested end to end.** The `macos` job proves the target triple, the LLVM backend, the C
  bridge, and the runtime on arm64, and `tests/check_platform.py` asserts that the emitted triple starts with the
  host architecture and that `usize`/`isize` are 64-bit.
- **Linux arm64 and Windows arm64 are untested.** The driver uses the host triple and `tests/check_platform.py`
  recognizes `aarch64`, so no code change is expected, but no job runs the suite there. Windows arm64 would also
  need an arm64 LLVM build; the official archive that CI uses ships x86-64 binaries.
- **macOS x86-64 (Intel) is untested.** `macos-latest` is Apple Silicon, so the Intel path through the same
  toolchain and libc is not exercised.

### Untested libc assumptions

The runtime is generated LLVM IR that calls libc directly, and linking is delegated to the platform's C compiler
driver. Only these libc and CRT implementations are tested:

| Platform | Libc / CRT | Exercised by |
| --- | --- | --- |
| Ubuntu 24.04 | glibc (gcc as `cc`) | `linux` job |
| macOS | libSystem (Apple clang as `cc`) | `macos` job |
| Windows | MSVC UCRT (`cl`) | `windows` job |

Assumptions that only those implementations back:

- **Symbolized backtraces** use `backtrace` and `backtrace_symbols_fd` on Unix. A libc without them fails to link;
  glibc and macOS libSystem provide them, and musl and other libcs are untested.
- **The stack-overflow guard** uses `pthread_getattr_np` and `pthread_attr_getstack` on Linux and
  `pthread_get_stackaddr_np` on macOS, together with `sigaltstack`. These are glibc and libSystem calls; another
  pthread implementation is untested. Windows uses `AddVectoredExceptionHandler` and `SetThreadStackGuarantee`
  instead: the system's default guarantee leaves the filter only a few kilobytes once the exception dispatcher has
  run, so the guard reserves 64 KiB for its own flush and write before it installs the filter.
- **File paths go through `fopen` as bytes.** On Windows that is the CRT's ANSI path API, so a non-ASCII path
  depends on the active code page; UTF-8 paths are not guaranteed to work and are not tested.
- **Output and arguments are bytes.** UTF-8 output is written with `write`/`_write`, and `env.args()` returns the
  CRT's narrow `argv`. A Windows console decodes the output in its active code page, and a non-ASCII argument
  depends on the same code page; neither is tested, because CI redirects output and passes ASCII arguments.
- **`process.run` uses `system`**, so the command is interpreted by the host shell (`/bin/sh -c` or `cmd.exe /C`);
  quoting, built-ins, and available commands differ. The tests use `exit N`, which both shells provide.
- **Allocation, string, and output calls** are `malloc`, `realloc`, `free`, `memcpy`, `memcmp`, `strlen`,
  `write`/`_write`, and `abort`; the test dispatcher reads its index with `atoi`. These are portable C, but only
  the CRTs above are exercised.
- **The stack guard covers the main thread only.** The tasks of stage 1 (not implemented) would need their own
  guard.
- **Static linking, musl, BSD, and other Unix systems are untested.** The link step is the platform `cc` with its
  default flags, so its behavior there is unknown.

### Tasks and threads

`std.task` and `std.thread` (stage 2.1, `spec/tasks.md`) sit on a small runtime written as LLVM IR text
(`crates/burdin_codegen/src/taskrt/`): POSIX threads, locks, and `mmap`-ed task stacks with an inline-assembly
context switch on Linux and macOS (x86-64 and arm64), and threads, slim locks, and fibers on Windows. The Linux
x86-64 path is the one that has been run; the macOS, arm64, and Windows paths follow the same contracts but their first
run is the CI jobs for those platforms, and `tests/s2_tasks` is the check. Constants that differ per system (the
`mmap` flags, the layout of `stack_t`, the name of the processor-count query) are filled in per target by
`taskrt.rs`.

`std.channel` (stage 2.2, `spec/channels.md`) adds no runtime of its own: a channel is Burdin code that waits through the
scheduler's lock, parking, and condition variable, so it runs wherever tasks do. `tests/s2_channels` is its check, and
its stress tests run 20 times in every CI job's debug and release profiles.

The atomics, `Mutex<T>`, `RwLock<T>`, and the atomic `Shared<T>` count (stage 2.3, `spec/shared.md`) use LLVM's atomic
instructions, which every supported target lowers natively, and the scheduler's lock and waiters, so they run wherever
tasks do. `tests/s2_shared` is their check, and its stress tests run 20 times in every CI job's debug and release
profiles.

### Not platform-specific

Language-level gaps and correctness issues that apply everywhere are tracked under "Known problems and risks" in
[`HANDOFF.md`](../HANDOFF.md#known-problems-and-risks) (for example, moving a non-`Copy` value through a runtime
index is rejected with E9000, assigned in [`spec/e9000-audit.md`](../spec/e9000-audit.md)). They are not repeated here.

## The checked example

A `usize` is 64 bits on every supported platform, which is how the runtime sizes pointers, lengths, and indexes:

<!-- check-docs: stdout=18446744073709551615 -->
```burdin
fn main() {
    let max: usize = 18446744073709551615
    println(max.to_string())
}
```
<!-- /check-docs -->

`tests/check_platform_support.py` compiles and runs this program on every CI job and compares the output, and
`tests/check_platform.py` asserts the same width through a literal that only a 64-bit type can hold.

## Checking a host

From a clean checkout, after `cargo build`:

```console
$ python3 tests/check_platform_support.py
$ python3 tests/check_platform.py
$ python3 tests/run_tests.py
```

`tests/check_platform_support.py` checks this page, binary-safe output and file I/O, and the example above;
`tests/check_platform.py` checks the host triple and the pointer width; `tests/run_tests.py` runs the conformance
suite in the debug profile (add `--release` for the release profile). CI runs all three, and the suite in both
profiles, on every platform. The console blocks on this page are illustrative; the scripts are the checked part.
