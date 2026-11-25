# Rust interop

A Burdin package can call a Rust crate. The crate is not compiled by Burdin and is not linked to the compiler:
`burdin bindgen --rust` reads the crate's public API from **rustdoc JSON** and writes two things, a small Rust crate
of `extern "C"` functions (the **shim**) and a Burdin module of declarations for them. The shim is built as a static
library and the program links it. Everything that crosses the boundary is the C ABI, so the Burdin side needs nothing
beyond the C interop it already has (`extern "C"`, `repr(C)`, `CPtr<T>`), and the Rust side runs its own code with its
own allocator, destructors, and panics.

The generated module is declarations only, like the one `burdin bindgen` writes for a C header (`spec/bindgen.md`):
`unsafe` at every call, no wrappers, no ownership tracking. A safe wrapper is the programmer's work on top of it.

The first version binds what has an exact C form and **names what it skips**. It never guesses: a signature it cannot
represent is left out with a note that says why, and the rest of the crate is still bound.

## RI1. The command and the build step

```
burdin bindgen --rust <crate> --out <file> --shim <dir> [--rust-toolchain <version>] [--rustdoc-json <file>]
```

`<crate>` is a crate directory or its `Cargo.toml` (a workspace manifest with no `[package]` is refused). `--out` is
the Burdin module, `--shim` the directory that receives the shim crate (`Cargo.toml`, `rust-toolchain.toml`,
`src/lib.rs`). `--rust-toolchain` pins the Rust version the layouts are computed for (RI2); `--rustdoc-json` reads a
JSON file instead of running `cargo doc`, which is how the format checks are tested without a toolchain run.
`--rust` takes no C compiler options and no wrapper options, and `--shim`, `--rust-toolchain`, and `--rustdoc-json`
are refused without `--rust`.

A package gets the same result with no command: a `[rust.crates]` entry in `burdin.toml` makes every `burdin build`,
`run`, and `test` generate, build, and link the shim (RI7-RI9). The command is for
looking at the output and for projects that commit it.

## RI2. The API comes from rustdoc JSON, on one pinned toolchain

rustdoc's JSON output is unstable: its `format_version` changes with the compiler. The tool therefore reads exactly
one version (`61`, produced by Rust `1.100.0`) and refuses every other (exit 1, naming both numbers), and it runs the
Rust tools under one pinned toolchain, selected through `RUSTUP_TOOLCHAIN`. Without a `--rust-toolchain`, the pin is
the exact version in this repository's `rust-toolchain.toml`; with a package, it is `[rust] toolchain` (RI7). The
installed `rustc` must report the pinned version exactly, or the command stops with exit 2 and says how to install it.
The JSON is requested on a stable compiler through `RUSTC_BOOTSTRAP=1` and `-Z unstable-options --output-format json`,
which is why the version is pinned and not a channel.

The tool reads the crate's public items, their attributes (`repr`, `no_mangle`), signatures, constants, and the spans
that name each item in a note (`src/lib.rs:199`, always with `/`). A document that is not JSON, or JSON that lacks what
the tool needs, is exit 1 with a message; it is never a panic.

## RI3. Which items are bound

Only items reachable by a public path are bound, and a `pub use` re-export binds the item under the path it is
visible at (a private module's `pub use` is fine). Items are visited in a fixed order, so the output is the same on
every run and on every platform.

| Item | Result |
|---|---|
| `pub const` of a primitive or `&str` | a Burdin `const`; one in a nested module is prefixed `MODULE_NAME` |
| a struct with `#[repr(C)]`, only public fields, every field C-compatible, no `Drop` | a **value**: a `repr(C)` struct, with layout checks (RI10) |
| any other struct or enum | a **handle**: an opaque `repr(C)` struct with no fields, used through `CPtr<T>` |
| a fieldless enum with `#[repr(u8..u64)]`, `#[repr(i8..i64)]`, or `#[repr(C)]` | the integer type as an alias, and a constant for every variant (`KIND_POINT`) |
| a free function or an inherent method of a bound type | a shim function (RI4) |
| a `#[no_mangle] pub extern "C" fn` | declared directly; the shim needs no wrapper for it |
| generics, traits, `impl Trait`, `async` functions, `Result`, `Option`, tuples, closures, references to a type with no C form, a type outside the crate | **skipped** with a note |

Every skipped item gets one line at the top of the Burdin module (`// bindgen: the function `first` at src/lib.rs:195
was skipped: it is generic, and C has no generics`) and one `warning:` line on standard error when the build step
generates, so a skipped item is visible without opening the module. The lines are sorted, and a re-run on the same
crate prints the same ones. A skip is never an error: the other items are still bound.

## RI4. Signature shapes

Each parameter and the result has one of these shapes, and a function with any other shape is skipped (RI3).

| Rust | C-ABI form |
|---|---|
| `i8..i64`, `u8..u64`, `isize`, `usize`, `f32`, `f64` | the same Burdin type |
| `bool` | `u8` (`0` or `1`); the shim converts both ways |
| `&str`, `String` (parameter) | `CPtr<u8>` and a `usize` length, named `<name>` and `<name>_len`; the shim checks UTF-8 and panics (so aborts) if it is not |
| `&[T]`, `&mut [T]` for a primitive `T` | `CPtr<T>` and a `usize` length, named `<name>` and `<name>_len` |
| `String` (result) | `CPtr<CChar>`, a NUL-terminated string the shim allocated; release it with the generated `<crate>_string_free` |
| a value struct, by value | `CPtr<T>` to a value the shim reads; as a result, an out parameter named `result` of type `CPtr<T>` that the shim writes |
| `&T` / `&mut T` of a value struct | `CPtr<T>` |
| a handle, by value (`self`, or a parameter) | `CPtr<T>`; the shim takes ownership, so the handle is **consumed** |
| `&self` / `&mut self`, `&T` / `&mut T` of a handle | `CPtr<T>`; the shim checks it is not null |
| a handle returned by value | `CPtr<T>` to a new boxed value; the caller owns it |
| `&T` returned from a method of a handle (primitive `T`) | `CPtr<T>` into the handle, valid until the handle is changed or freed |
| a fieldless enum | its integer; an unknown value in a parameter panics (so aborts), and a result is `as` the integer |
| `unsafe fn` | the same shape; the shim calls it in an `unsafe` block, and the Burdin declaration needs `unsafe` as every other |

A null pointer where the shim requires a pointer is a panic. **Every panic in a shim function aborts the process**:
the functions are `extern "C"`, and Rust aborts when a panic reaches that boundary, because unwinding into Burdin is
undefined. A Rust function that can fail returns `Result`, which is skipped, so a fallible API needs a thin
non-`Result` function in the crate (or its own `#[no_mangle]` function) to be bound; a first-class mapping of errors is
a later decision (`HANDOFF.md`).

## RI5. Names and the Burdin module

A free function is `<crate>_<module path>_<name>` (`geometry_length`, `geometry_util_clamp`); a method is
`<crate>_<type in snake case>_<method>` (`geometry_counter_inc`); a handle has `<crate>_<type>_free`, which drops the
value; `<crate>_string_free` is generated once when a function returns a `String`. The receiver parameter is named
`this`. A Rust name that is a Burdin keyword gains a trailing underscore. The type and constant names keep the Rust names.

The module has a header comment (the crate's name and version and the rustdoc format version, never a path or a
date), the notes, then the constants, types, layout assertions, and one `extern "C"` block, each group in a fixed
order. It imports nothing, so a package uses it as a generated module (RI7): `use geometry.{Vec2, Segment}`.

## RI6. The shim crate

`<dir>/Cargo.toml` is a `staticlib` named `<crate>_burdin_shim` that depends on the bound crate by path (relative to
the shim when it can be, absolute otherwise), in its own `[workspace]` so a workspace around the crate does not adopt it.
`<dir>/rust-toolchain.toml` pins
the toolchain (RI2). `<dir>/src/lib.rs` holds the layout checks (RI10), then one `#[unsafe(no_mangle)] pub extern "C"`
function per bound function, in the same order as the Burdin `extern "C"` block. Nothing in it depends on the
machine, so it is byte-identical across runs; the files are written only when their content changed, so cargo's own
freshness check is not disturbed.

## RI7. The manifest

```toml
[rust]
toolchain = "1.100.0"        # exact major.minor.patch; a channel such as "stable" is refused

[rust.crates]
geometry = { path = "../rust/geometry" }
```

`[rust.crates]` names crates by the module name the package imports them under, and `path` is relative to the
package. `[rust] toolchain` is required when any crate is named: the layouts are computed for it, so it must be a
decision, not a default. A crate that is not found (no `Cargo.toml` at the path), a toolchain that is missing or not
exact, and a toolchain that is not installed are each E0264, reported at the entry package's first source line before
any tool runs. Like a build script (`spec/build-scripts.md`, BS2), a crate runs native code (its `build.rs` and procedural
macros), so a **dependency's** `[rust.crates]` is used only when the entry package's `[build] allow` names that
dependency; otherwise E0260 ("the Rust crates of dependency `lib` are not allowed by this package's manifest").
Dependencies are processed before the packages that use them.

## RI8. Build and cache

For each crate, in the manifest's order, into `<package>/target/burdin/<profile>/rust/<name>/`:

1. **Generate** `shim/`, `bindings.bd`, and `rust.key`, when the key changed. The key is the compiler's fingerprint, the
   profile, the toolchain, the crate's name, location, and package name, and the content hash of every `.rs`,
   `Cargo.toml`, and `Cargo.lock` file in the crate (directories named `target`, and hidden ones, are skipped). Notes
   are printed as warnings only when this step runs. The key is written after the shim built, so a crate that
   failed to build is generated again next time.
2. **Build** the shim: `cargo rustc --lib [--release] -- --print native-static-libs` under the pinned toolchain,
   always, because cargo decides when the archive is fresh. A failure is E0264 with cargo's output.
3. **Register** the module: it is copied to `<out>/rust-src/<name>.bd` and merged as a generated module (BS4), so
   a name that repeats a source module is E0262.

`--emit-deps` lists every crate source as an input, so a build system reruns the build when the crate changes.
The release profile builds the shim in release; a debug build builds it in debug. On MSVC the shim is built with
`RUSTFLAGS=-C target-feature=+crt-static`, the static C runtime Burdin programs link.

## RI9. Linking

The program links the shim's static library, then the system libraries the archive needs, which `rustc` reports as
`native-static-libs` (`-lgcc_s -lutil -lrt -lpthread -lm -ldl -lc` on Linux, `kernel32.lib ntdll.lib ...` on
Windows). They are stored in `native-libs.txt` beside the archive, and a build that reuses the archive without cargo
printing them reads that file, then falls back to the platform's defaults. The libraries come after the `--link-lib`
options and are not repeated. `burdin test` links them too, so a `test` block calls the shim.

Two copies of Rust's runtime in one process are not a concern: the program has one shim per crate, each with its own
runtime and `std`, and nothing but C types crosses between them. A handle belongs to the shim that made it: a pointer
must go back to the `<crate>_<type>_*` functions it came from.

## RI10. Layout is checked on both sides

A value struct is the one place where Burdin and Rust must agree on bytes, so it is checked twice.

- The **Burdin module** carries a `priv const` of type `()` for every value struct, whose initializer is `if T.size() == N { } else { panic(...) }`
  and another for `T.align() == N`, where `N` is what rustc computed. A `const` is evaluated whether or
  not it is used (`spec/const-eval.md`, CE2), so a Burdin compiler whose `repr(C)` layout differs fails the build
  with E0222 (a panic during constant evaluation) and the message "`T` must have size N, as in the Rust crate".
- The **shim** carries `const _: () = { assert!(size_of::<T>() == N) ... }` for the size, the alignment, and
  `offset_of!` of every field, and for an enumeration's width. A crate whose layout changed since the shim was
  generated stops `cargo` with "evaluation panicked: `T` must have size N" (rustc E0080), reported as E0264.

The numbers come from the rustdoc JSON for the pinned toolchain. Nothing is assumed about another toolchain, which
is why RI2 pins one.

## RI11. Diagnostics, exit codes, and tests

**E0264** (Rust interop failed) is reported by the build step for an invalid `[rust]` table, a crate with no
`Cargo.toml`, a toolchain mismatch, a `cargo doc` or `cargo` failure (the tool's output follows the message), and a
shim that does not build, a layout failure included. E0260 is a dependency's crates without a grant (RI7), E0262 a
generated module that repeats a source module. `burdin bindgen --rust` exits 0 on success, 1 for rustdoc or JSON
problems, and 2 for usage errors, I/O errors, and a toolchain mismatch.

Tests:

- `tests/check_rust_interop.py` generates the fixture crate `tests/rust/geometry` and compares the module
  (`tests/rust/expected/geometry.bd`), the shim (`geometry_shim.rs`), and the shim's `rust-toolchain.toml` byte for
  byte, checks the notes and warnings, runs the generator twice for determinism, checks `--rustdoc-json` with
  valid, wrong-version, non-JSON, and missing files, builds the shim, shows that a wrong shim size and a wrong
  Burdin size are each rejected, checks the usage errors and a fake `rustc` of another version, and builds and
  runs a package in a temporary copy: the first build, the cached second build, a changed source, `--emit-deps`,
  `--release`, and a crate that does not compile.
- `tests/m22_build/rust_shim` (run, 32 output lines: constants, layouts, handles, slices, strings, enumerations,
  `#[no_mangle]` functions, nested modules, structs by value) and `rust_shim_test` (`burdin test`) are conformance tests.
- `tests/m22_build/rust_missing_crate`, `rust_unpinned`, `rust_toolchain_missing`, `rust_toolchain_inexact`, and
  `rust_missing_grant` are the E0264 and E0260 packages.
- `docs/error-codes.md` carries the E0264 example, which `tests/check_error_docs.py` runs.
- Unit tests in `burdin_rustgen` (rustdoc parsing, classification, naming), `burdin_driver` (the option parser,
  version checks, source discovery, native-library parsing), and `burdin_query` (the manifest tables).

## Not in this version

Generic functions and types, traits and `dyn Trait`, `Result` and `Option` mapping, tuples, callbacks and closures,
async functions, borrowed return values other than a primitive from a method, Rust crates that depend on other
crates through `[rust.crates]` rather than their own `Cargo.toml`, and a second Rust version in one program are
skipped or refused today. The macOS and Windows paths of the build step (the system libraries, `crt-static`) follow the
Linux design and have not been run.
