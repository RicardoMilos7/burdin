# The target directory and the artifact cache

Stage 3 (issue #353). A package's `target` directory holds everything a build writes besides the sources, and the
artifact cache keeps the executables that `build`, `run`, and `test` link so an unchanged package is not linked again.
This file is the exact behavior, with a rule ID for each point and the tests that cover it. The end-to-end rules are
covered by `tests/check_target.py`, which runs a built driver with a deliberately broken linker: a build that hits the
cache still succeeds, and a build that must link fails. The pure parts are unit tests in
`crates/burdin_driver/src/artifact.rs`.

## The layout

**TG1. What lives where.** The output of a package's builds is `<package>/target/burdin/<profile>/`, where `<profile>`
is `debug` or `release`. Its entries are:

- `bin/<name>[.exe]` and `bin/<name>.fingerprint`: the executable of `build` and `run`, and the fingerprint it was built
  from (TG3);
- `test/<name>[.exe]` and `test/<name>.fingerprint`: the test binary of `test`, and its fingerprint;
- `src/` and `build.key`: the output of the package's build script and its cache key (`spec/build-scripts.md`, BS5);
- `rust/<crate>/`: the Rust crates' shims (`spec/rust-interop.md`, RI8);
- `test-failed.txt`: the record of failed tests that `--failed` reads (`spec/test-runner.md`).

Inside a workspace the members share the root's `target/` (`spec/workspaces.md`, WS19). The benchmark baselines are the one exception: `target/burdin/bench/` is not per profile (`spec/bench.md`). Each kind of
artifact has its own directory, so the program and the test binary of one package never overwrite each other. There is
no `--target` option, so the directories are the only per-target split. Other entries under `target/` are not a
contract. A new package's `.gitignore` lists `/target/` (`burdin new`).
Tests: `tests/check_target.py` (the layout, the profiles, and the kinds).

**TG2. What is cached.** `build`, `run`, and `test` of a package directory (a directory with a `burdin.toml`) use the
cache. Every other build links into its output as it always did, without reading or writing the cache:

- a build for another platform (`--target`, `spec/cross-compilation.md`, XC9), whose executable is not the host's;
- a debug build (`-g`), because the linker writes sidecar files beside the executable (the PDB, the Natvis file, and the
  dSYM bundle on macOS), and a copy would leave them behind;
- a build with `--emit` or `--emit-ir`, which writes files of its own;
- a build with `--pgo-use`, whose profile file is not an input the fingerprint reads;
- `bench` and `fuzz` builds, which are not programs the user runs again;
- a one-file target. A script's binary is cached by content instead (`spec/scripts.md`, SC5).

Tests: `tests/check_target.py` (`-g` links every time, and the cache is left alone); the unit test
`only_build_and_test_of_a_package_are_cached`.

**TG3. The fingerprint.** A fingerprint file is text. Its first line is `burdin-artifact-v2`, so a change to the format
makes every older fingerprint mismatch. Then come one line per input, in this order:

- `compiler <identity>`: the compiler's version, commit, LLVM version, and its own file's length and modification time
  (the identity that BS5 uses for build scripts);
- `host <os> <arch>`;
- `profile <profile>`, `kind <kind>`, and `name <name>`, the package's name;
- `settings <key>`: the profile's code generation settings as `spec/profiles.md` spells the cache key
  (`opt=<n>;debug=<b>;debug-assertions=<b>;overflow-checks=<b>;panic=<strategy>`), so two profiles that write the
  same directory are still told apart;
- `package <name> overflow-checks=<b> debug-assertions=<b>` for each per-package override, in the manifest's order;
- `options strip=<b> lto=<b> pgo-generate=<b> sanitize=<list>`, where `<list>` is `none` or the enabled
  sanitizers, comma-separated;
- `link-lib <name>` for each `--link-lib`, in the order given;
- `manifest <sha256> <path>` for the `burdin.toml` of each package in the closure;
- `source <sha256> <path>` for each source file the front end read, hashed as it was checked, so the generated modules
  of a build script and the output of `bindgen` are sources too. The embedded standard library is not listed; the
  compiler identity covers it;
- `input <sha256|missing> <path>` for each build input: the files `embed` read, the inputs a build script declared, and
  the sources of a Rust crate (the same list `--emit-deps` writes);
- `rust-lib <sha256|missing> <name>` for each Rust shim library the program links. A system library is a name, not a
  file, so it reads as `missing`.

The last line is `artifact <sha256>`, the SHA-256 of the executable. A path inside the entry package is written relative
to its directory, and any other path is written absolute. A path is resolved before it is written, so two spellings of
one file name it the same way. Hashes are of file contents, never of modification times.
Tests: `tests/check_target.py` (the fingerprint's shape and lines; a relative and an absolute spelling of one package
share an entry); the unit test `publishing_moves_the_staged_program_and_records_it`.

**TG4. Freshness.** The front end always runs in full, so a cached build reports exactly the diagnostics, warnings,
and build-script output an uncached build does. A type error with a cached program present fails the build and leaves
the cache as it was. Only code generation and linking are skipped on a hit. A hit requires that the fingerprint file
hold exactly the body computed for this build, and that the executable hash to the `artifact` line.
Tests: `tests/check_target.py` (a type error with a cached program; a hit copies the program and does not rewrite the
fingerprint).

**TG5. Publishing.** A build that must link writes its program to `bin/<name>.staging-<pid>[.exe]` (or `test/`), in the
cache directory, which is created then. Then:

1. If the link fails, the staging file is removed, the cache is left as it was, and the build fails as a link failure
   does (exit status 2). Nothing is published for a failed link.
2. Otherwise the old fingerprint is removed first, so it never vouches for the new executable.
3. The staging file is renamed over the executable.
4. The new fingerprint is written under a temporary name and renamed into place. A failure here is not an error: the
   next build links again.

The fingerprint is written last, so an interrupted build never leaves a fingerprint that vouches for a missing or
unfinished executable. A staging file left by an interrupted build stays where it is; the next build does not remove it.
Tests: `tests/check_target.py` (a failed link leaves no executable, no fingerprint, and no staging file; the next build
publishes); the unit test `a_failed_move_publishes_nothing`.

**TG6. Copying out.** The executable a command asked for is a copy of the cached executable: the `-o` file, the default
`./<name>` of `build`, or the temporary executable `run` and `test` start. A copy onto the cached executable itself is
nothing to do. A cache that cannot be created is not a failed build: the program links straight into its output, as an
uncached build does. Tests: `tests/check_target.py` (a `target` file where the directory belongs still builds and runs).

**TG7. When a rebuild happens.** The program is linked again when any of these holds:

- there is no executable, or no fingerprint;
- the fingerprint's body differs from this build's: an input in TG3 changed, was added, or was removed. This includes a
  changed compiler, a changed flag, a changed manifest, a changed source, a changed path dependency, and a changed build
  input;
- the executable does not hash to the `artifact` line (it was truncated, replaced, or damaged).

Modification times are not inputs: touching a file without changing it is a hit. The output path is not an input.
There is one executable per profile and kind, so a build whose inputs differ from the last cached build replaces it,
and going back to an earlier source or to other flags links again. Keeping several executables per profile is not done.
Tests: `tests/check_target.py` (each case above, with the broken linker proving the link happened); the unit test
`a_changed_executable_or_fingerprint_is_not_fresh`.

**TG8. What is not an input.** These change the executable's bytes or the command's output, but not what the program
does, so they do not rebuild: the linker (`BURDIN_CC`), the number of object files the code is split into
(`BURDIN_CODEGEN_UNITS`), the thread count (`-j`), the compile-time step budget (a budget that is too small fails the
front end, which always runs), the diagnostic options, the program's arguments, and `burdin.lock`. The lockfile's commits
are checked out into the Burdin home and their files are sources, so the lockfile adds no input of its own.
Tests: `tests/check_target.py` (a build with a broken `BURDIN_CC` still hits).

## The object cache

Issue #243. The artifact cache above skips code generation when nothing changed. The object cache makes a build that
did change cheaper: each codegen unit's object file (`crates/burdin_codegen/src/split.rs`, issue #334) is kept under
a hash of its module, so only the units whose code changed are optimized and compiled again. The linker still runs
whenever the artifact cache misses. The decision on the parallel code generation model is that it stays as it is:
code generation is whole-program (the typed tree is lowered to one LLVM module on one thread), the module is cut into
units by function, and the units are compiled in parallel on `-j` threads. The object cache sits on top of that split;
it does not change it.

**TG11. Where.** A cacheable build (TG2) keeps the objects of its units in `<package>/target/burdin/<profile>/objects/<kind>/`
(`kind` is `bin` or `test`, as in TG1), one file per unit named `<key>.o` (`<key>.obj` with MSVC), where `<key>` is
the 64-hex-digit hash of TG12. A build that is not cacheable never reads or writes it. So `-g`, `--emit`, `--pgo-use`,
bench, fuzz, one-file targets, and every build that always uses one unit for its own reasons (`--lto`, `--sanitize`,
`--pgo-generate`) do not use it.
Tests: `tests/check_target.py` (the layout; a debug build writes no objects).

**TG12. The key.** The key hashes, in order: the format (`burdin-object-v1`), the compiler's identity (TG3's
`compiler` line), the optimization level, the target triple, and the unit's module as LLVM bitcode, taken after the
unit has been cut out of the whole module and before the optimizer runs. That module is the whole input of the
optimizer and the backend, so equal keys give equal objects. It contains the unit's own functions, a declaration of
everything another unit owns, and in a release build the small functions of other units that may be inlined; it also
carries the panic messages' file names and line numbers. Hashes are of contents, never of modification times.
Settings that change the code (debug assertions, overflow checks, per-package overrides) are already in the module, and
the linker and the thread count are not inputs (TG8).
Tests: `tests/check_target.py` (an edit that does not change the generated code reuses every object).

**TG13. A hit.** The unit's object is copied from the cache to where the unit's object would have been written, and the
optimizer and the backend do not run for that unit. The cached file is not rewritten. A build whose units all hit writes
the same executable as one that compiled them.
Tests: `tests/check_target.py` (a trailing comment keeps every object's name, bytes, and modification time; the program
prints what it printed).

**TG14. A miss.** The unit is compiled and its object is stored under its key: written to a temporary name in the cache
directory and renamed, so an interrupted build or two builds at once never leave a half-written object under a key. A
cache that cannot be created or written is a miss, never a failed build.

**TG15. What a change rebuilds.** An edit to one function changes the module of the unit that owns it, and of any unit
that inlined it in a release build; every other unit's key is the same, so only those units are compiled. The split is
by contiguous runs of functions balanced by instruction count (`split.rs`), so an edit that changes a function's size
enough to move a unit boundary, and one that changes the number of units, also rebuilds the neighbouring units. After a
build the cache keeps exactly the objects that build used and removes the others, as TG7 keeps one executable: going
back to an earlier source compiles again.
Tests: `tests/check_target.py` (an edit to one function keeps the objects of other units, and the objects of the
earlier build are gone).

**TG16. Constant evaluation.** Evaluation results are not cached themselves (CE11): the front end always runs and its results are
in the module, so a result that changes changes the key of the units it reaches.

## Notes

**TG9. Dependencies share no build output.** Git dependencies are shared through the Burdin home's checkouts (PKG3 in
`spec/packages.md`). Code generation is whole-program, so no dependency has an object file of its own to share, and
the build cache is per package. The object cache is per package too. The per-package build script cache (BS5) and the Rust shims (RI8) are unchanged. A
cache of per-dependency objects is not built; it would need code generation that is separate per package.

**TG10. Concurrent builds.** Two builds of one package may run at once. Each stages under its own process id, and the
last publish wins; two builds of the same inputs publish the same program. A rename over an executable that another
process is reading can fail on Windows; the build then copies its staged program to its output instead (TG5, TG6).

**Not done.** There is no `burdin clean`, and staging files of interrupted builds are not removed. Bench, fuzz, and
debug builds are not cached (TG2). There is no `--offline` mode, and there are no workspaces or registries to publish
to, so the parts of the issue's verification that name them do not apply to this compiler.
