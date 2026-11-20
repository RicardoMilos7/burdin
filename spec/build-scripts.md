# Build scripts

Milestone 22 adds `build.bd`, a program a package runs before compilation to generate sources or bind C headers.
This page is the exact behavior; the tests are `tests/m22_build/`. Rule IDs are BS1-BS8. The decisions it records
(issue #174) are: the entry package's own script is trusted and a dependency's needs an explicit `[build] allow`
grant, a script is an ordinary Burdin program run with the package and output directories as arguments, generated
modules are loaded from `<out>/src`, and reruns are decided by a content-keyed per-package/per-profile cache.

## BS1. What a build script is

`build.bd` is an optional file next to a package's `burdin.toml`. It is **not** a module of the package and is not
part of the program: it is a standalone Burdin program with its own `fn main`, compiled by the driver as a one-file
target and run before the package's sources are read. It has the standard library, no imports of the package, and
the ordinary `main` rules (unit, or `Result<T>` with the `main` printing an `Err`). A one-file target has no package
directory and therefore no script; the embedded standard library has none either.

Only the entry package and the packages loaded as its dependencies can have scripts.

## BS2. Trust and grants

Running a script is arbitrary code execution, so the trust boundary is the manifest:

- The **entry package's** own `build.bd` is trusted and runs without a grant.
- A **dependency's** `build.bd` runs only when the entry package's manifest names that package in a `[build]`
  section:

  ```toml
  [build]
  allow = ["lib", "crypto"]
  ```

  Every package in the build that has a `build.bd` and is not the entry package must be named, including a
  transitive dependency. Otherwise the build stops before any script runs with E0260 at the entry package's first
  source line.
- An `allow` entry that names no dependency in the build is also E0260, so a typo cannot silently disable a grant.
  An allowed dependency without a `build.bd` is harmless.
- When `burdin test -p <name>` selects a package, that package is the entry package for its own compilation, so its
  script is trusted and its own `[build] allow` list controls its dependencies.

The grant is per build, not stored: deleting the line stops the script from running. A dependency's manifest cannot
grant itself or another package; only the entry package decides.

## BS3. Running the script

The driver compiles `build.bd` in the same profile as the build (`dev` by default; `--release` is `--profile
release`, [`spec/profiles.md`](profiles.md)), then runs it:

- working directory: the package directory;
- `argv[1]`: the package directory (absolute);
- `argv[2]`: the output directory (absolute);
- `PATH`: the directory of the running `burdin` executable first, so the script finds the same compiler (and
  `burdin bindgen`, BS7);
- environment: `BURDIN_MANIFEST_DIR`, `BURDIN_OUT_DIR`, `BURDIN_PROFILE` (the profile's output directory: `debug`,
  `release`, or a custom profile's name), and `BURDIN` (the
  driver's path), in addition to the inherited environment;
- standard output and standard error: forwarded to the compiler's **stderr**, never its stdout, so `burdin run`'s
  program output stays exact.

The output directory is `<package>/target/burdin/<profile>`. The driver creates it, recreates its `src/` directory
before every script run (so a stale generated module cannot survive), and leaves the script's other files alone. The
script may write anywhere its process can; only `src/` is read by the compiler (BS4).

A script that does not compile, cannot be started, or ends with a non-zero status fails the build with E0261. The
script's own diagnostics are reported for `build.bd` first; E0261 is reported at the entry package's first source
line and names the package.

```
fn main() {
    let args = env.args()      // argv, program name first
    let out = args[2]
    match write_file("{out}/src/generated.bd", "const ANSWER: i32 = 42\n") {
        Ok(_) => {}
        Err(_) => panic("cannot write the generated module"),
    }
}
```

## BS4. Generated modules

Every `.bd` file under `<out>/src/` becomes a module of the package, with the same path it would have under the
package's own `src/`: `<out>/src/generated.bd` is module `generated`, and `<out>/src/geometry/shapes.bd` is
`geometry.shapes`. Generated modules are ordinary modules: they can be imported, can import each other and the
package's source modules, and are visible to another package exactly as far as the package's root re-exports them
(`use lib.generated.X` when the root has `pub use generated.X`).

- A generated module whose path repeats a source module's path is E0262: a script may add module names, never
  replace the package's own source.
- A generated path that is not a Burdin identifier (a folder or file stem) is E0262.
- A package still needs at least one source module under `src/`; generation adds modules, it does not replace the
  package.
- Moving generated files into the source tree is not supported and not tracked; a script writes only to its output
  directory.

## BS5. Caching

The driver reruns a script only when its inputs changed. The cache lives in `<out>/build.key` and is keyed per
package and profile settings (`spec/profiles.md`, BP6). The key contains the hashes of:

- `build.bd` and `burdin.toml`;
- the compiler binary (its version, commit, LLVM version, length, and modification time), so rebuilding the compiler
  reruns the script;
- every input the script declared in `<out>/inputs` (one path per line, relative to the package directory or
  absolute; `#` starts a comment);
- every file the script's own `embed` calls read (the same list `--emit-deps` uses for the script);
- every generated file under `<out>/src/`.

The comparison is by content, not modification time: touching a file does not rerun the script, and changing or
deleting an output does. A declared input that does not exist is E0261, because otherwise every later build would
rerun. The driver clears `<out>/src/` before a rerun, so an output that disappears from a script does not linger.
Two builds of the same package in one profile are not synchronized; like a compiler's object directory, the output
directory belongs to one build at a time. Two profiles that write the same directory (`dev` and `test`, `release`
and `bench`) also share it, and their cache keys agree exactly when their settings do.

The inputs a run declared are also added to the program's `--emit-deps` list, after the program's own `embed`
inputs, so an external build system rebuilds when a header the script declared changes.

## BS6. Diagnostics

The three new codes are reported at the entry package's first source line, because the failure is about the package
rather than one expression. They are preceded by any diagnostics of the script itself.

| Code | Meaning |
| --- | --- |
| E0260 | Invalid build configuration: an un-granted dependency script, or an `allow` entry that names no dependency |
| E0261 | A build script failed: it did not compile, could not be run, or ended non-zero; or a declared input is missing |
| E0262 | Invalid generated modules: a bad module path, a conflict with a source module, or an unreadable output directory |

## BS7. `bindgen` from a build script

A build script may invoke the full `burdin bindgen`, which reads the C header through libclang and writes a Burdin
module of raw declarations: `extern "C"` prototypes, `repr(C)` structs with a compile-time layout check, typedefs,
enumerations, constants from object-like macros, and C wrappers for `static inline` functions
([`spec/bindgen.md`](bindgen.md)). Nothing about the tool changes when a script runs it; the driver has already put
`burdin` first on `PATH`, and the script's working directory is its package, so relative paths resolve there.

The script invokes it through `process.run` and passes the failure on:

```
match process.run("burdin bindgen c/strings.h --out \"{out}/src/bindings.bd\"") {
    Ok(0) => {}
    Ok(code) => process.exit(code)
    Err(_) => process.exit(1)
}
```

A wrapper file (`--wrappers <out>/src/wrappers.c`) is not a module: the script compiles it itself and the package
links it, or the script leaves `static inline` functions out by not passing the option. The generated module's notes
(the declarations that were skipped, and why) are printed to stderr, which is the compiler's stderr, so a build that
skips something says so.

## BS8. What is not sandboxed

A build script is native code running as the user, exactly like Rust's `build.rs`. The grant is the trust boundary;
there is no operating-system sandbox, no filesystem jail, and no command allowlist in stage 1. The generated-source
directory is the only output the compiler tracks, and the cache records only the inputs the script declares and
`embed` reads. A script that needs another input must list it in `<out>/inputs`; a file the script reads but does
not declare does not trigger a rerun.

A failure can leave the output directory behind; the key is written only after a successful run, so a failed or
interrupted run is never treated as fresh.

## Tests

`tests/m22_build/`:

| Test | Kind | Covers |
| --- | --- | --- |
| `generated_module` | run | the package's own script writes a module with a constant, a function, and a struct that `main` imports |
| `granted_dependency` | run | the entry package grants a dependency's script; the dependency's root re-exports the generated names |
| `missing_grant` | compile-fail E0260 | a dependency's script without a grant |
| `invalid_grant` | compile-fail E0260 | an `allow` entry that names no dependency |
| `script_failure` | compile-fail E0261 | a script that exits non-zero |
| `script_compile_error` | compile-fail E0261 | a script with a source error |
| `generated_conflict` | compile-fail E0262 | a generated module that repeats the source `main` module |
| `generated_test` | burdin-test | a `test` block in a generated module runs with `burdin test` |
| `bindgen` | run | a script invokes `burdin bindgen`; the generated constants, struct size, enum, and libc `strlen` all work |

The cache logic the conformance suite cannot observe in one run (content keys, a changed or deleted input or output,
the profile, a missing declared input, the merge and its conflict) is covered by unit tests in
`crates/burdin_driver/src/build.rs` and black-box by `tests/check_build_scripts.py` (rerun and reuse, touching,
`--emit-deps`, and the `bindgen` command line). The generator itself is covered by `tests/check_bindgen.py`
(`spec/bindgen.md`).
