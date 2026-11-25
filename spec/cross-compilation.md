# Cross-compilation

`burdin build --target <triple>` builds a program for a platform other than the one the compiler runs on, and
`burdin check --target <triple>` checks the program as it is for that platform (issue #282). This file is the
behavior: what `--target` accepts, which facts follow it, how the object and the link are chosen, and what it does not
do. The tests are `tests/check_cross.py` and the unit tests named in each rule.

Without `--target` nothing changes: the platform is the machine the compiler runs on.

## The targets

XC1. `--target` takes one of six triples, and nothing else:

| Triple                       | `target.os()` | `target.arch()` |
| ---------------------------- | ------------- | --------------- |
| `x86_64-unknown-linux-gnu`   | `Linux`       | `X86_64`        |
| `aarch64-unknown-linux-gnu`  | `Linux`       | `Aarch64`       |
| `x86_64-apple-darwin`        | `MacOs`       | `X86_64`        |
| `aarch64-apple-darwin`       | `MacOs`       | `Aarch64`       |
| `x86_64-pc-windows-msvc`     | `Windows`     | `X86_64`        |
| `aarch64-pc-windows-msvc`    | `Windows`     | `Aarch64`       |

Any other triple is a usage error (exit status 2) that names the six. `--target` is a `build` and `check` option: on
`run`, `test`, `bench`, and `fuzz` it is a usage error, because a program built for another platform cannot run here.
It cannot be combined with `--sanitize` or profile-guided optimization, whose runtimes and profiles belong to the host.
`--target <the host's own triple>` is a native build and behaves exactly as a build without `--target`.

## What follows the target

XC2. The compile-time facts follow the target, not the host: `target.os()` and `target.arch()` (`spec/const-eval.md`,
CE4) in a compile-time `if` and at run time, the `cfg(...)` predicates of a manifest's `[target.'cfg(...)'...]` tables
(`spec/packages.md`), and the `[link]` tables chosen by them. The driver records the target once, before the front end
starts (`burdin_tir::target`); the checker, the compile-time interpreter, and the code generator all read it from
there, so they cannot disagree. Compile-time evaluation still runs on the host, so what it *does* (reading a file with
`embed`, a `const` initializer that calls the file system) is the host's; only what it reports about the target is the target's.

XC3. Code generation uses the target's machine: the module's triple and data layout are the target's, and the object
file is in the target's format (ELF, Mach-O, or COFF) for the target's architecture. `--emit obj`, `--emit asm`,
`--emit llvm-ir`, and `--emit llvm-bc` write the target's artifacts. `--emit asm`'s `--intel` and `--att` need an
x86-64 target. The object file's extension is the target's (`.obj` for Windows).

XC4. Every supported target is 64-bit and little-endian, so `usize`, `isize`, and pointers are 8 bytes and the layout of
a Burdin type is the same on all six. The C ABI of an `extern` function or a `repr(C)` type is the target's, as LLVM
lays it out for the triple.

XC5. The generated runtime is the target's. It reads the operating system's structures through fixed offsets, and those
are tables per platform (`crates/burdin_codegen/src/platform.rs`), not the host's headers: `struct stat`
(glibc x86-64 and arm64 differ; macOS and Windows are the same on both architectures), `struct dirent`,
`struct addrinfo`, the socket option numbers, the `errno` values that mean "try again", and the `open` flags. The
file-metadata symbol is `lstat` on Linux and arm64 macOS, `lstat$INODE64` on Intel macOS, and `_stat64` on Windows.
Unit tests check the host's table against the host's C headers; the other tables follow the same headers and are
checked by the cross build's IR (`tests/check_cross.py`), not by running the program.

XC6. The task runtime (context switch and thread entry) and the stack-overflow and fault handlers are the target's: the
architecture selects the assembly and the operating system the signal or exception code, exactly as in a native build
for that platform (`spec/tasks.md`).

## Output and link

XC7. The default name of an executable has the target's extension: `app.exe` for a Windows target, `app` elsewhere.
`-o app` on a Windows target is `app.exe` when it has no extension. A library's file names follow the target as well
(`spec/library.md`, LB3).

XC8. A cross link runs the compiler driver told to target the platform. The driver is, in order, `BURDIN_CC_<TRIPLE>`
(the triple in upper case with `-` as `_`, for example `BURDIN_CC_AARCH64_UNKNOWN_LINUX_GNU`), `BURDIN_CC`, then the
first of `clang` and `clang-21` that starts. The host's `cc` is never used: it targets the host. If none is found the
error names `BURDIN_CC_<TRIPLE>` and `BURDIN_SYSROOT_<TRIPLE>`, and an `--emit obj` file already written stays.

The command line is:

- `--target=<triple>` first;
- `--sysroot=<dir>` when `BURDIN_SYSROOT_<TRIPLE>` or `BURDIN_SYSROOT` is set: the target's C library and startup
  files (a Linux sysroot, or the macOS SDK);
- `-fuse-ld=lld`, or `-fuse-ld=<name>` when `BURDIN_LD` names another linker, because the host's own linker cannot link
  another architecture;
- then the same objects, `-o`, `-g`, `-rdynamic` (Linux), `-shared`, `-flto`, `-s`, and libraries as a native link, with
  `-lm` omitted on Windows targets, where `-lkernel32 -lws2_32` take its place.

An MSVC-style driver (`clang-cl`) is the one form for Windows targets that has the Windows SDK: it gets
`--target=<triple>`, `/winsysroot <dir>` for the SDK, and the MSVC-style arguments of a native Windows link; it picks its
linker itself.

A static library is archived by `llvm-ar` (or `BURDIN_AR`), which indexes every object format. A failed cross link
prints the driver's output and names the sysroot variable.

XC9. A cross build is not served from the artifact cache and does not write to it (`spec/target.md`, TG2): the cached
executable would be one of another platform under the host's name. The object cache is keyed by the triple, so
codegen units are still reused between cross builds of the same target. `--target <host triple>` is native and caches.

XC10. A build script (`build.bd`) runs on the host, so it is compiled for the host and sees the host as the target
(`spec/build-scripts.md`). The package it generates code for is built for `--target`.

XC11. `burdin check --target <triple>` type-checks, evaluates `const`s, and runs the memory checks with the target's
facts (XC2). It generates no code and links nothing, so it needs no cross toolchain.

## What is not supported

XC12. A `[rust.crates]` package is built with `cargo` for the host (`spec/rust-interop.md`), so `--target` with one is
an error. `--sanitize` and PGO are refused (XC1). A cross build cannot be run, tested, or benchmarked by Burdin; run the
executable on the target. `burdin asm`, `burdin bloat`, and `burdin expand` have no `--target`: use
`burdin build --target <triple> --emit asm`.

No toolchain is installed or located by the compiler. A cross link needs a clang driver, lld, and the target's
sysroot, all supplied by the user (XC8).

## Tests

- `tests/check_cross.py`: the six objects' formats and machines (XC3), the target facts in the IR and in
  `burdin check` (XC2, XC11), the runtime symbols and structure sizes per platform (XC5), the file names (XC7), the link
  command line with a stand-in driver (XC8), the cache (XC9), and the usage errors (XC1).
- `crates/burdin_tir/src/target.rs`, `crates/burdin_codegen/src/platform.rs`, and `crates/burdin_driver/src/platform.rs`:
  the triple table, the host's layout against the C headers, and the variable names.
