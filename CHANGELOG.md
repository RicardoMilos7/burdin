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
- Audit every E9000 site and assign it to a milestone (issue #84).
  - `spec/e9000-audit.md` lists every remaining `Code::UNSUPPORTED` site with the construct, the owning milestone,
    and the issue that removes it. Each site carries a `// e9000: <id>` marker, and `tests/check_e9000.py` fails
    when the source and the table disagree, when a row is stale, or when a site names a milestone that CI already
    runs. Every CI job runs it.
  - The stage 0 diagnostics that had no future milestone now have permanent codes and compile-fail tests: E0007 (a
    name declared or imported more than once; `duplicate_values`, `duplicate_type`, `duplicate_type_parameter`,
    `duplicate_member`, `duplicate_import`), E0009 (a cyclic or infinitely sized declaration; `constant_cycle`,
    `cyclic_types`), E0131 (invalid `Drop`; `drop_impl_shape`), E0163 (invalid `extern`; `extern_invalid`,
    `extern_signature_conflict`), E0164 (unsupported `repr`; `repr_other`), and E0180 (type arguments on a
    non-generic type; `type_args_on_non_generic`). The registry, `docs/error-codes.md`, and their examples are
    updated.
  - String literal patterns are implemented: a `str` scrutinee compares byte for byte, including slice views, the
    empty string, or-patterns, and guards. The domain is open, so a `match` on strings still needs an `else` or `_`
    arm (`m04_data/string_patterns`, `nonexhaustive_string`).
  - An inclusive slice range (`s[a..=b]`) is implemented; `spec/stdlib.md` already promised it. The slice end is
    checked as `hi < len` before it is advanced, so the maximum inclusive end panics as out of bounds rather than
    wrapping, and a reversed inclusive range (`a[2..=1]`) still panics. `m09_stdlib/slice_inclusive`,
    `slice_inclusive_bounds`, and `slice_inclusive_reversed` witness it. A dead "comparing strings" branch was
    removed.
  - The remaining E9000 sites are assigned to m19 (hand-built built-in interfaces), m20 (constant expressions),
    m21 (the remaining type-system forms), or Stage 1 for the cross-cutting pattern, bound, `where`,
    function-value, and runtime-index move forms.
- Close the stage 0 correctness gaps carried into stage 1 (issue #48, sub-issues #49, #53, #58, #64, #71, #77,
  #85, #86, #87).
  - **View lifetime (#49).** RT5 and the new E0123 reject a view stored in a `let` or an assignment that derives
    from a temporary or from a local of an inner block; static data, parameters, and same-or-enclosing-block locals
    are allowed. `match`/`if let`/`for` bindings are exempt. `burdin_check/src/origins.rs` now tracks parameter,
    local, and temporary origins with block ancestry; the rule is in `spec/memory-rules.md` RT5 and
    `docs/error-codes.md`, with `m06_conventions/view_lifetime_*` covering both outcomes.
  - **Ranges as values (#53).** `a..b` is a `Range<T>` and `a..=b` a `RangeInclusive<T>`: `Copy`, equal, ordered,
    hashable, cloneable, droppable, storable in fields and containers, and iterable with `for i in r` without
    borrowing the range. Endpoints are integers of one type; `..` has the lowest precedence. The type and its
    structural behavior are in `spec/grammar.md`, `spec/stdlib.md`, and `Ty::Range`; open-ended forms (`..b`, `a..`,
    `..`) do not exist. `range_values` (storage, clone, equality, hash, Map key, iteration),
    `range_endpoints_are_integers`, and `range_forms` (loop forms and the inclusive maximum) witness it.
  - **Move checker (#58).** Constant array and `List` indices, heap pointee fields (`(*h).a`), `match *h`, and a
    moving pattern with a guard on a temporary scrutinee are tracked instead of E9000. A temporary scrutinee is
    moved into a compiler-made local, so its other fields drop once and a failed guard restores the arm's moves. An
    index expression is evaluated once; a tracked move uses a constant index. A runtime index is still E9000.
    `spec/memory-rules.md` MV6/MV8 and `m05_ownership/move_index_*`, `index_evaluated_once`, `move_heap_field*`,
    `match_heap_pointee*`, `match_guard_temporary*`, `match_temporary_fields_drop`.
  - **Generic gaps (#64).** A generic static method infers its type arguments from its own parameters; a generic
    type's bounds are checked at instantiation (E0141); a view type argument makes the instance second-class (SC3);
    an impl method is compared with the interface method including `unsafe` (E0003); `==`/`<` work on symbolic
    instances whose bounds provide them; `Self` resolves in expressions, literals, patterns, and type paths. Tests
    in `m07_generics` (`infer_static_method`, `type_bounds_at_instantiation`, `view_instance_*`,
    `impl_signature_mismatch`, `symbolic_derives`, `self_in_expressions`, `self_outside_method`).
  - **Modules (#77).** `pub use` re-exports modules and items, with import resolution iterated to a fixpoint; a
    dependency's non-root modules are hidden unless its root re-exports them; `pub use` of a package-private item
    and a public signature or field naming a package-private type are E0150. `m08_modules/reexports`,
    `reexports_hidden`, `pub_use_private`, `public_signature_leaks`.
  - **C bridge (#71: #72-#76, #489, #490).** `CPtr<T>` gained load/store (`*p`), `.is_null()`, equality, element
    offsets and difference, and casts to/from integers and other pointers, all inside `unsafe`; creating, holding,
    passing, moving, or using a `CPtr` outside `unsafe` is E0161. `CString.clone` deep-copies the buffer.
    Out-parameters work through `&mut x as CPtr<...>`, and the cast ends the loan. `struct Name repr(C)` has the C
    layout and `Type.size()`/`Type.align()`; `...` declares a variadic C function; `extern "C" fn` exports a Burdin
    function under its plain C symbol with the C ABI rules (E0162). Aggregates cross the ABI through `CPtr<T>` only
    (by value is E0162), C `_Bool` is declared `u8`, and C `char` is `CChar`. There are no `union` declarations or
    function-pointer types: a union is bytes behind `CPtr<T>` and a function pointer is stored as `CPtr<CVoid>`
    (`spec/stdlib.md`). Tests: `cptr_operations`, `cptr_requires_unsafe`, `c_out_parameter`, `c_abi_structs`,
    `cstring_clone`, `export_c_abi`, `export_c_abi_invalid`, `export_c_abi_panics`; docs in `spec/stdlib.md`,
    `spec/grammar.md`, and `docs/error-codes.md`.
  - **`CStr` is an alias, not a view type (#83).** `CStr` remains a `std` alias for `CPtr<CChar>`: one type
    under two names, first-class and `Copy`, with no length, origin, or conversion rules of its own. There is no
    implicit conversion between text and a C string in either direction (E0003), `p.to_string()` and every other
    string method are E0002, and the only conversions are the explicit `CString.from(s).as_ptr()` and the
    associated `CStr.to_string(p)`. Every use keeps requiring `unsafe`. The rule and its rationale are in
    `spec/stdlib.md`; `m08_modules/cstr_is_an_alias` (run), `cstr_no_implicit_conversion`, and
    `cstr_is_not_a_str` witness it.
  - **Coverage (#85, #86, #87).** Run and compile-fail tests for `unsafe fn` declarations and unsafe interface
    methods (`unsafe_fn`, `unsafe_fn_call_requires_unsafe`, `unsafe_interface_method`,
    `unsafe_interface_mismatch`), and an audit of the `spec/memory-rules.md` test matrix against the suite, with US
    extended to the new pointer and unsafe-function tests.
  - The suite is 247 tests (245 in release); `tests/check_tests.py`, `check_unit_tests.py`, `check_docs.py`,
    `check_error_docs.py`, `check_diagnostics.py`, `check_codegen_opt.py`, `check_build_flags.py`,
    `check_parallel.py`, `check_test_runner.py`, and `check_debug_info.py` are green, and `cargo fmt --check`,
    `cargo clippy -- -D warnings`, and `cargo test` pass.
- `Map` is an open-addressing hash table (issue #82). The linear scan became linear probing in the map's existing
  three-word buffer: a zero hash marks an empty slot, the table doubles and rehashes at a 3/4 load factor, and a
  removal shifts the rest of the probe cluster back instead of leaving tombstones. Keys still need only `==`: they
  are hashed structurally (integers, floats, text by its bytes, tuples, structs, enums, `Option`, `Result`, and
  ranges), so `Hash` is not required. The API, the unspecified iteration order, and the iteration, drop, and clone
  behavior are unchanged. A 200,000-key insert-and-lookup program drops from 9.8 s to 0.01 s in a release build, and
  the million-key `m09_stdlib/map_many_keys` witness takes 0.07 s; `map_key_types` and `map_remove_and_clear` cover
  aggregate keys, deletion, reinsertion, and `clear`. The implementation is described in `spec/stdlib.md`.
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

### Milestone 12 — closures (issue #88, sub-issues #89-#97)

- Closure expressions: `|a, b| expr`, `|a: i32|: bool { ... }`, and `||` (zero parameters), parsed at primary
  precedence and callable with the ordinary `f(args)` syntax. Parameter types come from annotations or from an
  expected closure type; the return type is written after the closing `|` or inferred from the body, and must be
  written when the body uses `return`.
- Closure interface types `Fn(A, B): R`, `FnMut(...)`, and `FnOnce(...)` in types, parameters, fields, containers,
  returns, and as generic bounds (`fn apply<F: Fn(i32): i32>(f: F, x: i32)`), monomorphized per closure type. A
  closure value may be used at a lower capability (`Fn` as `FnMut` as `FnOnce`), never the other way.
- Capture analysis: a closure captures the whole variables it uses and does not bind. It borrows when used in place
  and moves when an owned closure type is expected (annotation, field, element, return, or closure-interface
  parameter); a `Fn`/`FnMut` bound keeps its concrete borrow captures. Borrow captures are shared or mutable
  according to how the body writes them; a `FnOnce` bound or a move context captures by move. A closure that moves a
  captured non-Copy value is `FnOnce`, one that writes a capture is `FnMut`, otherwise `Fn`.
- Function values: a non-generic function name (including a built-in function) or `Some` used where a closure type
  is expected becomes a captureless `Fn` closure that forwards its owned arguments to the function with the
  function's own conventions. A function returning a view is E0171 and an `unsafe fn` is E0161; a generic or
  `extern` function as a value stays E9000 (Stage 1). The E9000 audit rows owned by m12 are removed.
- Memory rules CL1-CL11 in `spec/memory-rules.md`: borrowed captures are second-class, and so is a closure that
  moves a view into its record; neither can escape (E0171, registered in `tests/README.md` and
  `docs/error-codes.md`); their loans live until the closure's last use (E0111); `Fn` calls borrow, `FnMut` calls
  borrow exclusively (E0113), and `FnOnce` calls move the closure (E0100); moving a read-only parameter into a
  stored closure is E0101.
- Representation and code generation: a closure value is `{ code, env, drop }`; the capture record is heap-allocated
  and holds pointers for borrow captures and values for move captures. A body function takes the record and the
  call's consumed flag first, the record's destructor drops moved-in captures, and a `FnOnce` body frees the record
  it consumes. A `Fn`/`FnMut` value called through a weakened `FnOnce` type destroys the record after the call,
  including on an early `return`. Calls are indirect through the code pointer; drop and clone glue, `Option`/`Result`,
  arrays, `List`, and `Map` handle closure values. Fixed a latent array drop/clone GEP that strided by the whole
  array instead of one element (found by an array of closures).
- Tests: `tests/m12_closures/` adds 45 conformance tests (run, `burdin test`, and compile-fail) covering borrow and
  move captures, view captures, stored and returned closures, generic `Fn`/`FnMut`/`FnOnce`, weakened capabilities,
  function values, fields, arrays, lists, nesting, `self`, branches, drops, and every new diagnostic. The suite is
  292 tests (290 in release).
- Documentation: `spec/grammar.md` sections 1, 4, and 6, `spec/memory-rules.md` section 12, `spec/stage1-subset.md`,
  `spec/e9000-audit.md`, `tests/README.md`, `docs/error-codes.md`, and `HANDOFF.md`.

### Milestone 13 — iterators (issue #98, sub-issues #99-#108)

- Generic interfaces and generic methods in sema: `interface Name<T>` declares type parameters and bounds, an impl
  supplies the interface's arguments (`Type: Iterator<i32>`), and a method may have its own parameters
  (`fn map<U, F: FnMut(T): U>`). Each call instantiates the method for the types it infers, with method parameters
  first so a default method's `F` wins over an impl parameter of the same name. `Fn` bounds infer the parameters
  inside them (`F: FnMut(T): U` gives `U` from the closure), and a `where` clause is checked with the impl's own
  arguments substituted. A generic `From` method is E0003.
- The protocol in `std`: `interface Iterator<T> { fn next(mut self): Option<T> }` and the marker
  `interface Iterable<T> { }`, with lazy default methods `map`, `filter`, `take`, `skip`, `fold`, `count`,
  `any_item` (the reserved `any` is spelled `.any` at the call), `all`, `find`, and `to_list`. The adapter types
  `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter` are generic structs whose `next` forwards to the source and
  the closure. Default methods are instantiated on demand for each receiver type, so a chain never instantiates
  every adapter's own adapters.
- Built-in implementations in the compiler: `iter()` and `into_iter()` on arrays, slices, `List`, `Map`, `Chars`,
  and ranges, and the compiler-known iterator types `ArrayIntoIter<T, N>`, `ListIntoIter<T>`, `MapIter<K, V>`,
  `MapIntoIter<K, V>`, and `RangeIter<T>`. A borrowing collection yields `T` when it is `Copy` and `&T` otherwise; a
  map yields `(&K, &V)` and `MapIntoIter` yields owned `(K, V)` and clears each entry it moves; `Chars.next`
  decodes one UTF-8 scalar value; an inclusive range iterator stops at its upper endpoint without computing
  `upper + 1`. `it.sum()` is compiler-known for integer items, and `next` on a non-mutable place is E0113.
- `for` over a user type or an iterator value lowers to `let mut __iter = e; loop { match __iter.next() { Some(pat)
  => body, None => break } }`. An `Iterable<T>` is iterated through `iter()` and its result must be an
  `Iterator<T>` (E0141); a type that is both an `Iterable` and an `Iterator` is iterated through `iter()`, because
  `for` borrows its source. The pattern must match every item (E0006). The stage 0 built-in loop forms and their
  rules are unchanged.
- Eager `List` methods `map`, `filter`, `fold`, `any`, `all`, and `find` in `std`: they borrow the list, pass the
  closure a view of each element, and return owned results (`filter` and `find` clone the element).
- Memory rules IT4-IT12 in the new `spec/iterators.md`: a chain is second-class (E0120, E0123), a view-typed
  parameter may be returned (RT2 now covers an `own` parameter whose type is a view), and a return-origin union
  counts only values that carry a view. Temporary drop flags (`tmp.live`) make a temporary created on one branch of
  a `match` drop only where it exists.
- Tests: `tests/m13_iterators/` adds 21 conformance tests (run and compile-fail) covering `for` over a user type,
  `iter`/`into_iter`/`next`, list, array, slice, map, `Chars`, and range iteration including the inclusive maximum,
  the lazy adapters and their laziness, the eager `List` methods, `sum`, generic iterator/iterable functions, and
  the second-class rejections. The suite is 313 tests (311 in release).
- Documentation: `spec/iterators.md` (new), `spec/memory-rules.md` section 7, `spec/stdlib.md`,
  `spec/stage1-subset.md`, `spec/e9000-audit.md`, `tests/README.md` decision 18, `CLAUDE.md`, and `HANDOFF.md`.


### Milestone 14 — formatting (issue #109, sub-issues #110-#116)

- The decision (`spec/format.md`, issue #110): format strings are a compile-time construct. A string literal's
  placeholders are checked against the call's arguments and each value's type when the program is compiled, and the
  whole string lowers to ordinary `Formatter` calls; nothing is parsed at run time. The page fixes the forms
  (`{expr}`, `{name}`, `{}`, `{n}`, `{expr:spec}`), the escapes (`{{`, `}}`), raw strings (`r"..."`, `r#"..."#`),
  multi-line literals, the specifier grammar, the output format of `Display` and `Debug`, floats, and the radixes.
- Lexer and parser (issue #111): a string literal lexes to a `Str` or an `Interp` token whose pieces are literal
  text and holes; each hole carries its own token stream and the raw specifier with its span, so a diagnostic points
  inside the placeholder. A placeholder's expression may hold brackets, character literals, and string literals, and
  the specifier starts at the first `:` outside brackets. An unterminated `{` or a stray `}` is E0001, reported once
  the string is known to end, and `r"..."` / `r#"..."#` are raw. The AST keeps each placeholder's parsed expression,
  the parsed `FormatSpec`, and spans; `crates/burdin_syntax/src/parser/format.rs` parses the specifier.
- `Display` and `Debug` (issue #112): `std/src/fmt.bd` declares the two interfaces, the `Formatter`, the built-in
  helpers (`__fmt_i64`, `__fmt_u64`, `__fmt_float`, `__fmt_str`, `__fmt_char`, `__fmt_bool`, `__fmt_*` debug forms,
  and the generic `__debug_option`/`__debug_result`/`__debug_list`/`__debug_heap`/`__debug_tuple2..4`), and the
  `__float_str` runtime binding. The compiler implements `Display`/`Debug` for integers, floats, `bool`, `char`,
  `str`, `String`, and `Debug` for `()`, tuples of 2 to 4, `Option`, `Result`, `List`, `Heap`, and `&T`, without an
  impl block. `derives (Debug)` generates the impl block as Burdin source at parse time
  (`crates/burdin_syntax/src/derive.rs`), so a derived `Debug` and a written one are the same; its diagnostics are
  suppressed in favor of E0130 at the offending field, and `Debug` joins the derive requirement table.
- Lowering (issue #113): an interpolated literal, `format(...)`, and `print`/`println`/`eprint`/`eprintln` with more
  than one argument lower to one block that builds the text in a `Formatter`. A user type writes to the formatter
  with `x.fmt(mut f)` / `x.debug(mut f)`; a specifier becomes `set_spec(...)` before the value and `reset_spec()`
  after it. `x.to_string()` formats any type that implements `Display` through the same path (floats included).
  A single-argument `println(text)` is unchanged, so `String`-to-`str` and existing programs behave as before.
- Specifiers (issue #114): width, fill, alignment (`<`, `^`, `>`), `+`, `#`, `0`, precision, and the `x`/`X`/`b`/`o`
  radixes, with `Formatter::pad`, `pad_plain`, and `pad_number` applying them. A signed value in a radix shows its
  two's complement in its own width. Floats are the shortest round-trip text (`snprintf("%.*e")` read back with
  `strtod`, in a new runtime function), or `snprintf("%.*f")` with a precision; `NaN`, `inf`, and `-inf` print as
  such. Precision counts characters for text and digits for floats.
- Compile-time checking (issue #115): every placeholder in a string is reported, not just the first. A missing
  argument, an argument no placeholder uses, a value that does not implement the interface, a specifier that does
  not apply, and a call with arguments whose format string is not a literal are the new codes E0191, E0192, E0193,
  E0190, and E0194. An argument used by more than one placeholder is evaluated once: a place is borrowed and a
  `Copy` value is copied; anything else is E0191. The code that resolves an ordinary method call was split so that
  an already-checked receiver can be used (`method_call_on`), which the lowering needs.
- Code generation emits only reachable functions (`crates/burdin_codegen/src/emit/reach.rs`): the embedded
  standard library used to be generated into every program, so new `std` code (as `fmt.bd` is) added drop flags and
  bounds checks to unrelated programs and made the IR proportional to the library rather than the program. A
  function nothing reaches now gets a two-line `unreachable` body; the roots are `main`, the tests, exported
  functions, `Drop` implementations, and closure bodies. Fixed with it: a function's prologue parameter stores used
  to inherit the previous function's debug location, which LLVM's verifier rejects under `-g`; the location is now
  cleared at each function start.
- Tests (issue #116): `tests/m14_format/` adds 17 conformance tests — interpolation with expressions, escapes, raw
  and multi-line strings, closures that capture into a placeholder, positional arguments and the shared-argument
  rule, every specifier for integers, floats, and text, a user `Display` (including one that honors `pad`), a
  derived `Debug` (structs, enums, generics, `Heap`), a hand-written `Debug` next to a `Display`, and compile-fail
  tests for every new code and for an unterminated brace. The suite is 330 tests (328 in release). The specs are
  `spec/format.md` (new), `spec/grammar.md`, `spec/stdlib.md`, `spec/derives.md`, and `spec/stage1-subset.md`; the
  registry, `docs/error-codes.md`, CI's milestone list, `tests/README.md`, `CLAUDE.md`, `HANDOFF.md`, and this file
  are updated with them.

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
