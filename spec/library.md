# Libraries: the `[lib]` table, static and shared artifacts

A package with `src/lib.bd` and no `src/main.bd` is a library: today `burdin build` of it fails with "has no entry
point", and nothing it compiles can be linked by another program (issue #1000). The `[lib]` table of `burdin.toml`
says what a build of the library writes: nothing (the default, today's behavior), a static archive, or a shared
library, for callers written in C or any language with a C ABI. This page is the rule set, with a rule ID for each
point and the tests that cover it (`tests/check_library.py`, fixtures in `tests/library/`).

The pieces: the manifest table is read by the package loader (`crates/burdin_query/src/package.rs`, which keeps the
raw entries); the driver validates it and reports E0265 (`crates/burdin_driver/src/library.rs`); the artifact is
linked in the driver (`link_library` and `link` in `crates/burdin_driver/src/main.rs`); the codegen has a library mode
(`burdin_codegen::Options::library`, `crates/burdin_codegen/src/runtime.rs` and `emit/function.rs`). A library is a
package, so the dependency rules (PKG25-PKG34) apply to it unchanged.

## The decisions

- **Kinds.** `source` (the default) writes no artifact. `static` and `shared` write one. There is no `root` key (the
  library's root module is the one `use <package>` reaches, as today), and no per-profile `[lib]`.
- **Burdin callers compile the source.** A Burdin package that depends on a library (PKG25) compiles the library's
  source into its own program, as it does now. The artifact is for callers that link a C ABI; a Burdin program cannot
  read a built artifact, since there is no interface file.
- **A library has no `main`.** A static or shared library with `src/main.bd` is an error, so the artifact is never
  ambiguous between a program and a library.
- **The C surface is the `extern "C" fn` items.** A library exports the `extern "C" fn` items that have a body and are
  not `priv`, under their plain names, with the C ABI. The same set goes in the C header (BG10). A `priv extern "C" fn`
  has no C symbol in any build, which is a change for executables too: `priv` hides the function from C as well as
  from other packages, as the header rule already did.

## The `[lib]` table

**LB1. The table.** `[lib]` has two keys: `kind` and `c-header`.

```toml
[lib]
kind = "shared"             # "source" (the default), "static", or "shared"
c-header = "include/app.h"  # optional: the C header of the exports, relative to the package
```

`kind` is a string, one of `source`, `static`, and `shared`. `c-header` is a string that is a relative path with no
`..` and no leading `/`. Any other key, a `kind` that is not one of the three, a value of the wrong type, and a
`c-header` on a `source` library are E0265 at the line of the entry, and the build stops before anything is written.
A `c-header` on a `source` library is refused because a source library writes no C surface. A package with no `[lib]`
table is a `source` library with no header.

**LB2. `source`.** A `source` library behaves exactly as before: `burdin build` and `burdin run` fail with "has no entry
point" and write nothing; `burdin check` fails the same way unless it is given `--tests` (`spec/check.md`), and
`burdin check --tests` and `burdin test` check and run its modules. `burdin.lock`, `--locked`, and the target cache are
unaffected.

**LB3. Static and shared libraries.** `kind = "static"` or `"shared"` makes `burdin build` (and `burdin build -p` and a
workspace build, LB10) write the artifact. Such a package is checked the way `burdin check --tests` checks a library
(no `main` is required, and its tests are checked), `burdin check` and `burdin test` accept it, and `burdin test` runs
its tests. `burdin run` refuses it with "has no entry point", since a library has nothing to run. A build writes the
artifact into `target/burdin/<profile>/bin/`, where a program's executable is cached, and copies it to where the
executable would go: the current directory for a single package, or the path `-o` names. Its file name is the package
name `NAME`:

| Kind | Windows (MSVC) | macOS | Linux and other Unix |
| --- | --- | --- | --- |
| `static` | `NAME.lib` | `libNAME.a` | `libNAME.a` |
| `shared` | `NAME.dll`, with the import library `NAME.lib` beside it | `libNAME.dylib` | `libNAME.so` |

A static or shared library with `src/main.bd` is E0265, reported at the `[lib]` table's `kind` line.

**LB4. The root module.** A static or shared library needs no `main`. Its root module is the one `use <package>`
reaches, as today. A library whose root has no `extern "C" fn` still builds: its artifact has no exports, and a header
(LB8) declares nothing and warns.

**LB5. Exports.** Every `extern "C" fn` with a body that is not `priv`, in any module of the package, is exported
under its plain name with the C ABI. Every other function is local to the artifact: a static library's and a shared
library's local symbols do not collide with another library's, and a shared library exports nothing else. The
program's static initializer (LB6) is local too. A `priv extern "C" fn` has no symbol (LB5 is the rule for every build;
`tests/bindgen/export` already left it out of the header). The task runtime's context switch is module assembly that
names two symbols globally, `burdin_rt_ctx_asm_switch` and `burdin_rt_ctx_boot`, so a library's `burdin_rt_ctx_*` globals
are the only non-export global symbols allowed.

**LB6. Initialization and the host process.** A library's static initializer (`spec/globals.md`, GL1) runs when the
artifact is loaded, before any exported function runs: a static library's initializer is a global constructor of its
object, and a shared library's runs when the loader maps it (`.init_array` on Unix, the CRT initializer section on
Windows). The standard output buffer is flushed when the process exits, as a program's is. `env.args()` of a library
returns no arguments, since the process's arguments belong to the host. A library installs no signal handler, so a
stack overflow or fault in it is the host's, and a panic ends the host process as a panic ends a program. Test
blocks, benches, and fuzz targets are not part of a library artifact (they are not roots of its code).

**LB7. Linking.** A static library is an archive: `ar rcs` on Unix and `lib /OUT:` on MSVC, of the program's objects.
An archive records no system libraries, so a C program that links it names the runtime's libraries itself: `-lm` on
Unix, and on Windows the libraries the program's own link names (`kernel32.lib`, `ws2_32.lib`, and the C runtime). A
shared library is linked the way a program is linked (`cc -shared` on Unix and macOS, `cl /LD` with MSVC), with the
package's `[link]` libraries (PKG34) recorded as its dependencies, so a loader finds them. The Windows exports are
marked `dllexport`. `--link-lib` names libraries for a library's link as for a program's.

**LB8. The C header.** `c-header = "<path>"` makes `burdin build` also write the C header of the library's exports,
exactly as `burdin bindgen --export` writes it (`spec/bindgen.md`, BG10 and BG11): the same prototypes, structs, and
constants, the same E0263 for a signature C cannot spell, and a guard named from the file's stem. The header is
written when the library is built, after the artifact; a header that fails leaves the artifact in place and fails the
build. A header with no exports is written and a warning says so, as `--export` does.

**LB9. Options.** The `[lib]` table is the same in every profile (`spec/profiles.md`). `--release`, `-g`, `--strip`,
`-o`, `--emit`, and `-v` apply to a library as to a program. `--lto`, `--pgo-generate`, `--pgo-use`, and `--sanitize`
do not: a library build with one of them is a usage error (exit status 2) that names the option.

**LB10. Workspaces.** `burdin build` of a workspace builds every selected member that has `src/main.bd` or a static or
shared `[lib]`. A member with neither is skipped with a note, as today. `-p <name>` of a member with neither is an
error, as today. A workspace build writes each member's artifact under its own `target/`.

**LB11. What a library does not change.** `[lib]` is not part of the dependency graph: `burdin.lock` does not record
it, and `--locked` does not check it (PKG8). A library's artifact is not stored in the target cache (`spec/target.md`):
each `burdin build` writes it again. The table of a dependency is ignored: a package's build reads only the `[lib]` of
the package it builds.

## Diagnostics

E0265 is an invalid library configuration: a `[lib]` key or `kind` that is not known, a value of the wrong type, a
`c-header` that is not a relative path, a `c-header` on a `source` library, or a static or shared library with a
`src/main.bd`. The message names the key or file and gives the fix. A link failure is the linker's error, as an
executable's is, with the tool's output.

## Tests

`tests/check_library.py` builds the fixtures in `tests/library/` with the driver and checks, for each rule:

- LB1-LB4: a `source` library still fails `build` with "has no entry point" (LB2); a static and a shared library build
  (LB3, with the file names of the table, checked on the host's platform); each invalid `[lib]` entry, a `main` next to a
  library kind, and a `c-header` on a `source` library are E0265 with the line number (LB1, LB4).
- LB5: the exports are the non-`priv` `extern "C" fn` items (`nm` reports them on Unix and macOS; the priv function is
  absent), and a C program that calls an exported function links and runs.
- LB6: a C program calls an exported function whose result comes from a `static` (the initializer ran before the
  call), and an exported function that prints through `println` shows the output after the C program returns (the
  flush at exit).
- LB7: the C program links the static library with `-lm` and the shared library by its name, and runs with the shared
  library found through `LD_LIBRARY_PATH`.
- LB8: the `c-header` file matches `burdin bindgen --export`'s output for the same package, and the C program compiles
  against it.
- LB9: `--lto` and `--sanitize` on a library exit with status 2.
- LB10: a workspace build writes the artifacts of its library members and skips the `source` member.
- LB11: `--locked` accepts a changed `[lib]` table.

The `nm` checks run on Unix and macOS. The C-program checks run on Linux only, where they were verified; on macOS and
Windows they are skipped, because the macOS and MSVC links are not run in the checker's CI job yet. The Rust side is `crates/burdin_query/src/package.rs`'s unit tests for the parse (`lib_*`) and
`crates/burdin_driver/src/library.rs`'s unit tests for the validation and the file names.

## Not yet

- A `root` key; a per-profile `[lib]`; and `--lto` and the sanitizers for a library.
- Burdin programs that link a built artifact (an interface file is needed first).
- Two multi-unit static libraries (`BURDIN_CODEGEN_UNITS`, or a program large enough to be split) linked into one
  program may both define the split's hidden helper symbols, since they are named by the unit split, not by the
  package. A single-unit library has no such symbols.
- A library's runtime is not initialized with the host's arguments, stack guard, or signal handlers (LB6).
- The Windows and macOS artifact paths (`lib.exe`, `/LD`, `dllexport`, `-shared` on macOS) are not yet exercised by the
  checker's CI job.
