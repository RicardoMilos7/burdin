# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second, third, fourth, fifth, sixth, seventh, eighth, ninth, tenth, eleventh, twelfth, thirteenth, fourteenth, fifteenth, sixteenth, seventeenth, eighteenth, nineteenth, twentieth, twenty-first, twenty-second, twenty-third, twenty-fourth, twenty-fifth, twenty-sixth, twenty-seventh, twenty-eighth, twenty-ninth, thirtieth, thirty-first, thirty-second, thirty-third, thirty-fourth, thirty-fifth, thirty-sixth, thirty-seventh, thirty-eighth, thirty-ninth, and fiftieth. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the fifty-fourth session (2026-11-19). **Milestones 1 to 11 are done, so stage 0 is complete,
and the stage 0 gaps of issue #48 are closed; milestones 12 (closures), 13 (iterators), 14 (formatting), 15
(`any` and `impl`), 16 (errors), 17 (default and named arguments), 18 (extension methods), 19 (operator
overloading and `Index`), 20 (comptime), 21 (reflection, user derives, const generics, `embed`, generic aliases, and impls for specific instances), 22 (build scripts, dependency grants, generated modules, and the minimal `bindgen`), and 23 (allocators, `Arena`, `Shared<T>`, leak detection, and allocator-aware containers) are done**, milestone 24
(`m24_stdlib`) is done, and stage 2 is complete through milestone 2.4 (asynchronous I/O)**: 1073 conformance tests;
1070 pass in debug (three are skipped) and 1061 in release (the release run skips the debug-only and platform-specific
tests). Stage 3's bench blocks (issue #251) are done: `bench` items, `black_box`, and `burdin bench` with statistics,
baselines, filters, `--list`, and `--profile-time`.
`burdin test` runs `test` blocks concurrently with attributes, captured output, and isolation
(issues #16, #17, #18); the twelfth deleted the stale
`Burdin Stage 0 Subset.md` root export (issue #43) and documented the driver contract in a root `README.md`, checked
by `tests/check_docs.py` (issue #42); the thirteenth retired the claude.ai copies of the design and stage 0
documents, added the compiler benchmarks, and cached the cargo registry, `target/`, and LLVM 21 in CI (#44, #41,
#46); the fourteenth added the Windows CI job and the shared LLVM link (issue #12); the fifteenth runs the finished
conformance milestones in the release profile in every CI job (issue #47) and adds benchmark regression tracking:
the harness saves and compares a baseline, and each CI job keeps one in a cache and reports regressions advisably
(issue #446); the sixteenth settled the 16 assumptions of issue #21 (sub-issues #22-#40) in `spec/`, added
`spec/stdlib.md` and `spec/derives.md`, 40 decision tests, and four small fixes: `Drop` now rejects a read-only or
`own` receiver, `CPtr.null` rejects arguments, the ordering operators work on `str`/`String`, and an invalid
`extern "C"` type or convention is E0162; the seventeenth added the diagnostics presentation and tooling of issue
#433 (sub-issues #434-#437) — colors, terminal width, and `--no-color`; notes and suggestions with spans and edits;
deterministic ordering and `--error-limit`; and the checked `docs/error-codes.md` reference with `burdin explain` —
and deterministic lexer/parser fuzzing: a committed corpus plus a 20,000-mutation replay in every CI job, a
scheduled `fuzz` job that runs a ten-minute session and uploads new inputs, and a watchdog for an input that stops
returning (issues #288 and #447); the eighteenth added the test-runner ergonomics of issue #438 (sub-issues
#439-#443, #445): test attributes (E0170), `-j` and package selection, output capture, isolation, a black-box check
wired into every CI job, and an error-code coverage audit that fails when a registered code has no test. Issue #444
(differential tests) waited for the comptime interpreter, milestone 20, and was implemented in the thirty-sixth
session; the nineteenth implemented the compiler and
runtime work of issue #421 (sub-issues #422-#432): source-level debug information, panic backtraces, buffered output,
a main-thread stack-overflow guard, glue deduplication, drop-flag elimination, bounds-check elision, LTO and PGO
flags, stripping and a size budget, `--version` with build metadata, and parallel parsing. The twentieth session
added the dependency and license audit of issue #449: `Cargo.lock` is committed with exact pins and checksums,
`tests/dependencies.txt` records every external crate with its license and role, and `tests/check_deps.py` fails on
an unreviewed crate, a changed version, a non-permissive license, a non-exact direct requirement, or a stale
lockfile; every CI job runs it after `cargo build` and requires the crates' own `cargo metadata` license fields. The
twenty-first session documented the platform support matrix and known limitations (issue #457) in
`docs/platform-support.md` and added `tests/check_platform_support.py`, which every CI job runs. The twenty-second
session implemented the release pipeline of issue #452 (sub-issues #453-#456) without cutting a release: a six-target
release workflow, versioned archives with SHA-256 checksums, reproducible builds and archives, the installers, the
version/channel metadata, and the policy and rehearsal page in `docs/releasing.md`. The twenty-third session
implemented the stage 0 correctness gaps of issue #48 (sub-issues #49, #53, #58, #64, #71, #77, #85, #86, #87):
the view-lifetime rule (E0123), ranges as values, the tracked move forms, the generic inference/bounds/signature
gaps, `pub use` and the module visibility rule, the C bridge (`CPtr` operations, `unsafe` for every `CPtr` value,
`CString.clone`, out-parameters, `repr(C)` layout, variadic declarations, exported `extern "C" fn`), conformance
tests for `unsafe fn` and unsafe interface methods, and a memory-rule coverage audit. The twenty-fourth session
settled issue #83: `CStr` stays a `std` alias for `CPtr<CChar>` and is not a second-class view type; the rule is in
`spec/stdlib.md` and `spec/memory-rules.md` SC1, and `m08_modules/cstr_is_an_alias.bd`,
`cstr_no_implicit_conversion.bd`, and `cstr_is_not_a_str.bd` witness it. It also replaced the linear-scan `Map`
with an open-addressing hash table (issue #82): lookup, insert, and remove are expected O(1), keys still need only
`==`, and the unspecified iteration order is unchanged. The same session audited every `Code::UNSUPPORTED` site
(issue #84): `spec/e9000-audit.md` assigns each remaining E9000 to a milestone and an issue, each site carries a
`// e9000:` id, and `tests/check_e9000.py` keeps the table and the compiler in agreement. The stage 0 diagnostics
that had no future milestone got permanent codes (E0007, E0009, E0131, E0163, E0164, E0180) with compile-fail
tests; string literal patterns and inclusive slice ranges are implemented, and a dead "comparing strings" branch was
removed. The remaining E9000 sites are assigned to m20, m21, or the cross-cutting Stage 1 work (the hand-written
built-in-interface question moved there with #402 when milestone 19 made the operator interfaces ordinary `std`
interfaces).

The twenty-fifth session implemented milestone 12, closures (issue #88, sub-issues #89-#97): `|a, b| ...` and `||`
expressions, the `Fn`/`FnMut`/`FnOnce` interface types and generic `Fn` bounds with monomorphization, borrow and
move capture analysis, calls that borrow or move the closure, storing, returning, and passing closures, function
values (`add`, builtins, and `Some` used where a closure type is expected), drop and clone glue for closure values,
the rules CL1-CL11, and `tests/m12_closures` with 45 tests.

The twenty-sixth session implemented milestone 13, iterators (issue #98, sub-issues #99-#108): generic interfaces
and generic methods in sema (`interface Iterator<T>`, `fn map<U, F: FnMut(T): U>` default methods, monomorphized per
instantiation), the protocol for arrays, slices, `List`, `Map`, `Chars`, and ranges in the compiler, `iter()`,
`into_iter()`, and `next()`, user-defined `for` loops lowered to `next` calls, the lazy adapters in `std`, the eager
`List` methods, compiler-known `sum`, ranges through the protocol with no overflow at an inclusive upper bound, the
rules IT4-IT12 in `spec/iterators.md`, and `tests/m13_iterators` with 21 tests. The E9000 rows owned by m13 (generic
interfaces, generic methods, generic `From`, and the two range-pattern sites) are removed with their sites.

The twenty-seventh session implemented milestone 14, formatting (issue #109, sub-issues #110-#116), deciding first
that format strings are a compile-time construct (`spec/format.md`): a literal's placeholders are checked against
the call's arguments and each value's type, and the string lowers to `Formatter` calls, so nothing is parsed at run
time. The lexer produces an `Interp` token whose holes carry their own token streams, spans, and raw specifiers, and
parses raw strings; the parser builds each hole's expression and the `FormatSpec`; sema lowers interpolation,
`format`, and multi-argument `print`/`println`/`eprint`/`eprintln` to one block of `Formatter` calls, formats the
built-in types with helpers in the new `std/src/fmt.bd`, and checks E0190-E0194. `derives (Debug)` is generated as
Burdin source at parse time. `tests/m14_format` adds 17 tests. Code generation now emits a body only for reachable
functions (`burdin_codegen/src/emit/reach.rs`), so the embedded standard library stops adding dead drop flags and
bounds checks to every program; the same session fixed a `-g` verifier failure where a function's parameter stores
inherited the previous function's debug location.

The twenty-eighth session implemented milestone 15, `any` and `impl` (issue #117, sub-issues #119-#125). It decided
first (`spec/existentials.md`, AN1-AN12) that `any Interface` is an owned, first-class two-word heap box
`{data, vtable}`, that a concrete value is boxed implicitly wherever `any I` is the expected type, that an interface
used as `any` must be object-safe (E0202), that `impl Interface` is a hidden generic parameter allowed only in a
parameter list (E0201), and that `x.is<T>()`/`x.downcast<T>()` are compiler-known methods with member type arguments.
The syntax is `any`/`impl` as keywords and `[ GenericArgs ]` after a member name; sema interns interface instances,
their vtable method lists, type identities, and vtables, defers object-safety reports until every interface signature
is resolved, and boxes at every expected-type position; the move, borrow, and origin checks understand the four new
expression forms; code generation emits `{ ptr, ptr }` values, one private constant vtable per `(T, I)` pair
`{ i64 type_id, ptr drop, ptr method... }`, indirect calls, and type-id-checked downcasts that move the value out and
free the allocation without dropping it again. The new diagnostics are E0200, E0201, E0202, and E0204. The tests are
`tests/m15_any/` with 24 tests. The remaining milestones are m21 (reflection, derives, const generics) onward.

The twenty-ninth session implemented milestone 16, errors (issue #126). It decided first (`spec/errors.md`,
ERR1-ERR10) that an `error enum` lowers to an ordinary enum plus generated `Display`, `Error`, and `From<Source>`
impls, that `Result<T>` means `Result<T, Error>`, where the dynamic `Error` is
`{ value: any Error, frames: List<TraceFrame>, context: String }`, and that an error type is boxed implicitly
wherever `Error` is expected. A `?` converts through `from` or into the dynamic error, `.context(message)` prefixes
the chain while keeping the boxed source, `is`/`downcast` inspect the box, and a debug build records creation and
propagation frames while a release build records none. The parser lowers `error` declarations in the new
`crates/burdin_syntax/src/error_item.rs` before derive expansion and wraps `fn main(): Result<T>`; it also splits
each test body into a `Result<()>` body plus a wrapper that prints the error, so a failing typed assertion reports
its message while `?` still accepts other error types in a test; sema checks the one-argument `Result<T>`, the
implicit conversion, `context`, and E0212. `std/src/std.bd` declares the `Error` interface and struct,
`TraceFrame`, the `IoError`, `ParseIntError`, and `AssertError` error enums, and `__test_assert`/`__test_assert_eq`;
`orelse` now accepts a closure handler for a `Result` and a no-argument one for an `Option` (issue #488). The new
diagnostics are E0210, E0211, and E0212. The session also fixed a code-generation bug the release profile exposed:
a temporary created inside one branch had an uninitialized drop flag at the statement's drop site, which an
optimizer could treat as an undefined free and use to delete the rest of the function; the flag now starts `false`
in the entry block. The tests are `tests/m16_errors/` with 25 tests.

The thirty-fourth session implemented milestone 21, reflection, user derives, const generics, and `embed` (issue
#166, sub-issues #167-#172). It decided first (`spec/reflection.md` RFL1-RFL10, `spec/const-generics.md` CG1-CG9,
`spec/comptime.md` CT15) that reflection is five compile-time members of every type that sema evaluates while it
checks a body, that a `comptime for` over `Type.fields`/`variants` is unrolled in sema rather than in
`burdin_comptime` (because `x.{field.name}` needs the name to type the access), that a user derive is `derive Name {
fn ... }` registered as the impl block `Type: Name` for each type that lists it in `derives (...)`, that a const
argument is a `Ty::Const` value so the instance, substitution, and inference code for types serves values too, and
that `embed` reads the file in sema and records it as a build input. New in the compiler: the `derive` item and the
`x.{name}` postfix in `burdin_syntax`; `crates/burdin_sema/src/fncx/reflect.rs` (members, the unrolled loop, computed
access), `user_derives.rs` (the generated impl), `consteval.rs` (const expressions, symbolic lengths interned as
`Ty::Const`/array lengths from `SYM_CONST`/`SYM_LEN`), and `fncx/embed.rs`; `Ty::Const`, `Bound::ConstOf`, and the
`Program::build_inputs` list with the `--emit-deps <file>` driver flag. `std` gained `FieldInfo` and `VariantInfo`.
The new diagnostics are E0230-E0233, E0240-E0241, E0250-E0252, and E0143. The tests are `tests/m21_reflect/` with 25
tests. The milestone did not split. Five E9000 sites are gone (`type-array-length`, `array-repeat-length`, and the
three type-system rows the audit had moved to `Stage 1`: generic aliases #392, impls for specific instances #395, and
built-in interfaces for generic types #401). Those three were then implemented in the same session: `aliases.rs`
expands `type Pair<T> = (T, T)` where it is used (`spec/type-aliases.md` AL1-AL6); `impl_match.rs` matches an impl
block's argument pattern against an instance (`Stack<i32>`, `Pair<i32, B>`, `Holder<List<T>>`), reports the later of
two overlapping impls of one interface as E0143, and `Drop` covers every instance (E0131 when it names specific
arguments) (`spec/impls.md` IM1-IM9); a derive on a generic type holds for the instances whose parts allow it (IM8),
and writing a built-in interface by hand is the one m21-family site left, E9000 under #402.

The thirty-fifth session implemented milestone 22, build scripts (issue #173, sub-issues #174-#179). It decided first
(`spec/build-scripts.md` BS1-BS8) that `build.bd` is a standalone program at the package root that the driver
compiles in the build profile and runs before any source is read, that the entry package's own script is trusted
while a dependency's run only when the entry package's manifest lists it in `[build] allow` (E0260), that the script
receives the package directory (working directory and `argv[1]`) and `<package>/target/burdin/<profile>` (`argv[2]`)
and writes modules to `<out>/src` (E0262 on a path that repeats a source module), and that reruns are decided by a
content key per package and profile over the script, manifest, compiler binary, the `<out>/inputs` list, the script's
`embed` files, and the generated outputs (BS5). The driver gained `build.rs` (grants, running, cache, generated
modules) and a minimal `bindgen.rs` (`burdin bindgen <header> --out <file>`: `extern "C"` declarations, `repr(C)`
structs, typedefs, enum constants, and object-like `#define`s; `#include`, function pointers, and unions are skipped
for stage 3). The manifest parser gained `[build] allow`. The new diagnostics are E0260 (invalid build configuration),
E0261 (a failed script), and E0262 (invalid generated modules), reported at the entry package's first source line.
The tests are `tests/m22_build/` with nine conformance tests, driver unit tests for the cache key and the header
parser, and `tests/check_build_scripts.py` for the cache, `--emit-deps`, and the `bindgen` command line.

The thirty-sixth session implemented milestone 23, allocators, `Arena`, and `Shared<T>` (issue #180, sub-issues
#181-#188). It decided first (`spec/allocators.md` AL1-AL10) that the allocator is an ordinary value and an
ordinary generic parameter — `A: Allocator`, or `own a: impl Allocator` — that `List<T, A = Global>`,
`Map<K, V, A = Global>`, and `Heap<T, A = Global>` take as their trailing type argument, and that `Global` is a
zero-sized value stored nowhere, so the stage 0 spellings and layouts do not change. Any other allocator is
stored in the container as an owned field and given to `new_in`; `new` has no value to store there (E0272), and a
type or value that is not an allocator is E0270. Default type parameters are written on trailing struct/enum
parameters (`struct Buffer<A: Allocator = Global>`), resolved where the type is declared, with E0271 for a
non-trailing default, a const parameter's default, and a default on a function, interface, or alias; a bare
inherent or `Drop` impl covers every instance. `Arena` is a first-class bump allocator with 4096-byte blocks,
`bytes_allocated`, and `reset`; dropping it frees every block, and it is not second-class because every safe way
to get an arena-backed buffer makes the container own the arena. `Shared<T>` is a non-atomic reference-counted
cell allocated through `Global`: `.clone()` shares it, `.get()` borrows its value, `.strong_count()` counts, the
last handle drops the value and frees the cell, and it is `Clone` for every `T` but never `Copy`; stage 2 makes the
count atomic and adds `Send`. A `burdin test` build allocates through a checking allocator with a 40-byte header
and a live list: a double free panics, and an allocation still live when an otherwise passing test returns is
reported as `test leak: <n> allocation(s) leaked, <bytes> byte(s)` with the source site the compiler recorded, and
fails that test; `burdin build`/`run` use the plain C heap. In the compiler, `burdin_tir` gained the `Alloc`
marker on `List`/`Map`/`Heap` and the owning iterators, `Ty::Shared`, the allocator builtins and methods, and
`Program::allocator_fns`; sema resolves allocator arguments, the default type parameters, and the `new_in` calls;
`burdin_codegen/src/emit/alloc.rs` calls the allocator implementation (every argument by address) and the runtime
gained the checking allocator and the leak check; `burdin_comptime` models `Shared` as its value and rejects
explicit allocation as impure (E0224). `std/src/alloc.bd` adds `Allocator`, `Global`, and `Arena`. The new
diagnostics are E0270, E0271, and E0272. The tests are `tests/m23_allocators/` with 28 conformance tests, the
`tests/runner/leaks/` fixture, and the leak-detection checks in `tests/check_test_runner.py`; the suite is 577
tests. The checking allocator also exposed a latent temporary-drop leak in the non-consuming `Option`/`Result`
methods (`is_some`, `is_ok`, ...), fixed by reading the receiver through a place so its temporary is registered;
two runner fixtures reported it before the fix.

The thirty-sixth session implemented the differential tests of issue #444, the last sub-issue of #438: a test
item may carry `[differential]`, and the runner then compares the `Ok`/`Err` outcome of the same body run by the
comptime interpreter while the package is compiled with the compiled test's outcome.
`burdin_comptime::evaluate_differential_tests` runs each `[differential]` body after the rewrite of CT13, through
the interpreter's function-call entry point, under the ordinary purity, panic, budget, and profile rules; the
outcome travels in `tir::TestEntry.comptime` to the runner, which treats a different outcome or an aborted compiled
run as a divergence with a note naming both outcomes.
`differential` cannot be combined with `should_panic` or `should_fail` (E0170). The behavior is in
`spec/test-runner.md`, the conformance tests are `tests/m10_test_runner/attributes_differential.bd`,
`differential_invalid.bd`, and `differential_impure.bd`, and `tests/check_test_runner.py` gains a fixture
(`tests/runner/differential/`) whose third test depends on the unspecified `Map` iteration order, so the interpreter
and compiled code really do disagree; the check reports it instead of passing. Every CI job already runs the script.

The thirty-seventh session implemented issue #419, `process.Command`. `std/src/process.bd` adds `Command` (a program
with arguments, environment, working directory, and standard-stream modes), `Stdio`, `Child`, `ChildStdin`,
`ChildStdout`, `ChildStderr`, `ExitStatus`, and `Output`. `Command.new` starts nothing; `spawn` starts the child,
`status` starts it and waits, and `output` pipes both output streams, reads them, and waits. A new set of compiler
primitives reaches the operating system: `__env_list`, `__process_spawn`, `__process_wait`, `__process_close`,
`__pipe_read_to_string`, `__pipe_read_line`, `__pipe_read_both`, `__pipe_write`, and `__pipe_close`. Unix forks and
calls `execvp` with the child's own `environ`, so a changed `PATH` applies; Windows calls `CreateProcessW` with a
command line quoted by the C runtime's rules, a UTF-16 environment block built from the final entry list, and
inheritable pipes converted to CRT descriptors. `wait_with_output` drains both pipes together (`poll` on Unix,
`PeekNamedPipe` on Windows), so a child writing a lot to one of them cannot block the other; a `ChildStdin` write is
unbuffered, and every handle and the `Child` close their operating-system resources on drop. The tests are
`tests/m24_stdlib/process_command`, `process_pipes`, `process_windows`, `process_bad_calls`, and `process_test_mode`,
plus `tests/m20_comptime/purity_process`; the suite is 713 tests (711 run in debug).

The thirty-eighth session implemented milestone 2.1, tasks and the scheduler (issue #198, sub-issues #199-#206,
#301-#304, and #405), the first stage 2 milestone. Tasks are stackful coroutines scheduled cooperatively on a pool of
worker threads; the scheduler (per-worker queues, a shared queue, stealing, parking) is Burdin code in
`std/src/task.bd` over about two dozen small runtime primitives (`Builtin::Rt(RtOp)`, `__closure_own`), which are
LLVM IR text in `crates/burdin_codegen/src/taskrt/` linked in only when used. The surface is `task.spawn`/`JoinHandle`,
`task.yield`, `task.scope`/`Scope.spawn`, `task.set_workers`, and `thread.spawn`/`Thread` (`std/src/thread.bd`). The
decisions are `spec/tasks.md` TK1-TK12 (representation, 256 KiB stacks with a 64 KiB guard, `Send` deferred to 2.3, a
panic in a task aborts the process, threads, shutdown) and the checker rules for scoped tasks are `spec/memory-rules.md`
SK1-SK3 (loans from `Scope.spawn` last to the end of the scope body; a task borrows only the body's captures, E0171;
a mutable borrow in a loop is E0111). `tests/s2_tasks/` has 27 tests, run by every CI job in both profiles. The
runtime work touched shared state: the output buffer and the checking allocator's live list are protected by spin
locks, the stack-overflow handler recognizes a task-stack guard, and a test run's leak check ignores the scheduler (its
records come from `calloc`, and worker-thread environments are `untrack`ed). Code generation now emits a closure body
only when a reachable body creates it (a closure in an unreachable function used to be emitted, which pulled the whole
scheduler into every program). The Linux x86-64 path is the only one run so far: the macOS, arm64, and Windows paths
are written but untested (`docs/platform-support.md`).

The forty-fourth session implemented stage 3's `burdin test` selection and reporting (issue #235, sub-issues #638-#641).
`burdin test` takes positional name filters, `--exact`, and `--skip` over a test's full path
(`module.submodule.test name`); `--list` prints the selected tests with their attributes (human or `--format json`);
`--format json` streams events and `--junit <file>` writes JUnit XML; `--failed` reruns the previous failures from
`<package>/target/burdin/<profile>/test-failed.txt`, `--shard <i>/<n>` takes a round-robin slice, and tests that ran at
least `--slow-threshold` milliseconds are listed before the summary. The rules, the JSON schema (version 1), and the
choices that are not obvious are in `spec/test-runner.md`; `tests/check_test_runner.py` proves them. The driver's test
runner is now `test_runner.rs` plus `test_runner/select.rs` and `test_runner/report.rs`, and `Args` derives `Default`
and `Clone`. Not done: the language server's test integration (issue #374), which consumes `--list --format json`.

The forty-fifth session implemented stage 3's machine-readable diagnostics (issue #236, sub-issues #642-#644). One
`Diagnostic` value (`crates/burdin_diag`) now feeds four formats. `json.rs` renders the version 1 object (a small
ordered `Json` value, so the key order is in the code) and `sarif.rs` renders the SARIF 2.1.0 log; the driver builds the
SARIF tool and its rules from the embedded `docs/error-codes.md` headings and the linter's table. The model gained
`labels`, `help`, and `children`, and `Diagnostic::new`; no compiler diagnostic produces them yet (the human format
prints `help` and children, not labels), so the fields exist for the schema and the next producers. The format is
documented in `docs/diagnostics.md` and `spec/diagnostics.md` (DG1-DG14); `docs/diagnostic.schema.json` is closed on
purpose. **The hosted error reference does not exist yet.** The link root, `https://ricardomilos7.github.io/burdin`
(`URL_BASE`), is where the repository's GitHub Pages would serve `/errors/<code>` and `/lints/<code>`; the scheme is
pinned by the tests but nothing publishes the pages, so the links 404 until someone does. Changing the root is one edit
of `URL_BASE` plus the two scripts that pin it. SARIF columns are Unicode scalar values (`columnKind`
`unicodeCodePoints`), and a relative path is `%SRCROOT%`-relative. Existing assertions changed only where the JSON
summary gained `"version":1`.

The forty-sixth session implemented stage 3's `burdin check` (issue #237): the type-check-only command. It is not a new
pipeline: `check.rs` calls `front_end` (`main.rs`), the one function that `build`, `run`, `test`, and `lint` use to load
a package, run its build script, parse it, check it, run the memory checks, and evaluate `comptime`, and stops there. The
issue says to reuse the query-based library; that library does not exist yet (it is a later stage 3 item), so the
command reuses `front_end`, and the query-based library will replace `front_end` for every command at once. The output
is a build's (the failure text moved from `compile` into `report_errors`; a clean check prints nothing), and the exit
statuses are a build's. Two choices came with it: `--tests` is how test blocks are checked and how a library package
without `src/main.bd` is checked (it uses the test-mode front end, which does not look up `main`), and a build's
output-only options are usage errors. The decisions are `spec/check.md` CK1-CK8. `tests/check_check.py` proves it,
including by running `burdin check` over all 1053 runnable conformance tests and comparing with their markers (about a
minute and a half with eight workers on a debug build; every CI job runs it). Not done: watch mode and an editor
protocol (the language server is its own item), checking a workspace's members with one command, and `check` of a
package with a `build.bd` without running the script (a generated module has to exist first).

The forty-first session implemented stage 3's `burdin lint` (issue #230, sub-issues #231-#233, #464-#466, #661). The
linter is the new crate `burdin_lint`, a side branch of the pipeline: it reads the syntax tree and the typed tree the
compiler built, never changes them, and the driver (`crates/burdin_driver/src/lint.rs`) is the only caller. Syntax-tree
lints (names, imports, unused variables and parameters, unreachable code, dead `priv` items, `else` over a local enum)
and typed-tree lints (eager chains, redundant clones, `mut` never written, large `Copy` in a loop, borrow parameters
every caller consumes, allocation in a loop, `to_list()` iterated at once) are `L0001`-`L0016`; `L0000` is an unknown
lint name. Generic functions are checked per instance and a finding is kept only when every instance agrees; generated
code (single-token spans) is skipped. Closure capture modes in the typed tree are unreliable (an `IndexMut` write leaves
a capture as a borrow), so `unused_mut` analyzes closure bodies to a fixed point. `--fix` applies rounds of
machine-applicable edits, rechecks that the file still compiles, and writes all files or none. Levels, the manifest
`[lint]` table, and the suppression comments are in `docs/lints.md`; the decisions, including the ones that are not
obvious, are `spec/lint.md` LN1-LN32; `tests/check_lint.py` and `tests/lint/` prove them and every CI job runs the
script. Scanning the 665 conformance programs for false positives drove most of the rules in "Choices that are not
obvious" in the spec. Not done: `missing_docs` and `burdin doc --coverage` (issue #657) are not sub-issues of #230.

The thirty-ninth session implemented milestone 2.2, channels and `select` (issue #207, sub-issues #208-#212).
`Channel<T>` (`std/src/channel.bd`) is a cloneable handle to a bounded or unbounded multi-producer, multi-consumer
queue: `send` (returns the value back when the channel is closed), `recv` (`None` when closed and empty), `try_send`,
`try_recv`, `close`, `len`, `capacity`, `clone`, and `for v in ch`. All channel state, the waiter lists, and a change
counter live under the scheduler's one lock; a waiter is a task (parked through the existing park-and-unlock-after-switch
path) or a thread (the scheduler's condition variable), so no new runtime primitive was needed. `select` is a contextual
keyword that the **parser rewrites** into a loop over `__try_recv_slot`/`__try_send_slot` calls plus a `match` on the
chosen arm (`crates/burdin_syntax/src/parser/select.rs`), so sema, the checks, and codegen have no `select` of their own;
the first ready arm in source order runs, `default` makes it non-blocking, and an arm guard (`if cond`) switches an arm
off (a closed channel is always ready, so a fan-in loop needs guards). The decisions are `spec/channels.md` CH1-CH10 and
`tests/s2_channels/` has 36 tests, including stress tests that CI repeats 20 times in both profiles (`tests/run_tests.py
--repeat`, checked by `tests/check_ci.py`). `Channel<&T>` is E0120. The derived `Send` marker is still 2.3.

The fortieth session implemented milestone 2.3, shared state (issue #213, sub-issues #214-#219, #406, #407). `Shared<T>`'s
count is atomic through new compiler builtins (`Builtin::Atomic(AtomicOp, AtomicWidth)`, emitted in
`crates/burdin_codegen/src/emit/atomic.rs`); `std/src/atomic.bd` (generated by `tests/gen_atomic_std.py`, checked by
`tests/check_tests.py`) builds seven atomic types on them, and `std/src/sync.bd` builds `Mutex<T>` and `RwLock<T>` on
the scheduler's lock and waiter primitives, with a closure API (`m.lock(|v| ...)`) instead of a guard. `Send` and `Sync`
(`std/src/marker.bd`) are empty interfaces derived structurally in `crates/burdin_sema/src/markers.rs`; a closure type
carries an identity-ignored `send: FnSend` computed at the closure expression from its captures (`Ty::Fn`), and
`unsafe Type: Send where ... { }` impl blocks (parsed in `parser/items.rs`, checked in `impls.rs`) override the
derivation. `task.spawn`, `thread.spawn`, `Scope.spawn`, a task's result, and `Channel<T>` require them: E0280 (the
message names the field or capture) and E0281. One checker change came with it: the view made by dereferencing a raw
pointer takes the origin of the storage that holds the pointer, so a handle's method can return `&T` (`origins.rs`). The
decisions are `spec/shared.md` SH1-SH11 and `tests/s2_shared/` has 26 tests, including stress tests that CI repeats 20
times in both profiles.

The forty-first session implemented milestone 2.4, asynchronous I/O (issue #220, sub-issues #221-#225 and #408). The
decision (`spec/io.md` IO1-IO8) is that socket, file, name-resolution, and `blocking` extern calls look blocking but
park only the task: a reactor thread waits on the operating system's event facility and wakes the waiting task, and a
small helper pool runs the calls that block. The runtime primitives (`__io_*`, `__pool_submit`,
`__pool_submit_raw`) are LLVM IR in `crates/burdin_codegen/src/taskrt/reactor_epoll.ll`, `reactor_kqueue.ll`, and
`reactor_iocp.ll`, linked only when used; the timers, the wait records, and the integration with the scheduler's
waiters are Burdin in `std/src/reactor.bd` (`__wait_io`, `__offload`, `__offload_raw`, `__sleep_async`). The socket
layer (`std/src/net.bd`, plus `__net_*` and `__dns_lookup` primitives in `crates/burdin_codegen/src/runtime.rs`) has
`TcpListener`, `TcpStream`, `UdpSocket`, `SocketAddr`, and `lookup_host`; a descriptor is an `i64` (a Windows `SOCKET`
is pointer-sized), the calls are non-blocking at the OS level with one-shot reactor registrations, and timeouts are
deadlines on the reactor's timer list. File and directory operations now go through `__offload`, which is sound
without a `Send` bound because the caller waits for the helper. A contextual `blocking` marker on an `extern "C"`
declaration (`FnDecl.is_blocking`, `tir::ExternFn.blocking`) makes a call marshal its arguments into an 8-byte slot
block and run a generated thunk on the pool through `std.__offload_blocking`; reachability roots that shim only when
a blocking extern exists. `time.sleep` parks a task instead of holding a worker. `IoError` gained a `Net` variant and
`IoErrorKind` the connection, address, and reachability kinds, with the errno and Winsock tables extended
(`burdin_tir::ioerror`). The new code is E0290 (invalid `blocking`), and `tests/s2_io/` has 21 tests, including a TCP
echo server, UDP, DNS, read timeouts, a FIFO whose blocking open would deadlock without the pool, and two stress tests
that CI repeats 20 times in both profiles. The Windows reactor uses a completion port for finished helper calls, a `WSAPoll` readiness wait, and a loopback
UDP wakeup sent from a second socket (Winsock serializes the socket a thread is polling, so the polled socket cannot
wake itself); the macOS path uses `kqueue`. Every CI job runs `tests/s2_io` on its own platform, and the Windows
socket and pool paths were also run locally on Windows x86-64 while fixing that job.

The forty-second session started stage 3 with `burdin new` (issue #228). `crates/burdin_driver/src/new.rs` creates the
package directory (`burdin.toml`, `src/main.bd`, `.gitignore`), takes the package name from the last path component
(an identifier, not a reserved word, not `std`), and refuses an existing path with exit status 2 before writing
anything. The `.gitignore` lists `/target/` and the executable `burdin build .` leaves in the package directory,
because the driver writes it to the working directory. Unit tests in `new.rs` and `tests/check_new.py` (run in every
CI job) cover it. There is no `burdin new --lib` or template choice, no `git init`, and no `--name` override yet.

The forty-eighth session implemented stage 3's C bindgen (issue #244, sub-issues #245-#248, #475, #476, #663). The
import side is a new crate, `crates/burdin_bindgen`, which knows nothing about the compiler: `clang.rs` finds and loads
libclang at run time with hand-written C API bindings over `dlopen`/`dlsym` (no LLVM link, no new Rust dependency),
`parse.rs` walks the header, `macros.rs` evaluates macro bodies with C's typing rules, and `model.rs` renders the
module. `SKIP_FUNCTION_BODIES` is deliberately not passed to clang: with it, every function looks like a declaration,
and `static inline` functions then cannot be told from declarations (the trap the first version fell into). The
generated module is `extern "C"` declarations, `repr(C)` structs with `comptime assert` size and alignment checks, a
byte blob with the member list for a union, packed struct, bit-field, flexible array member, or anonymous member, an
alias plus constants per enumeration, constants from object-like macros, and `CPtr<CVoid>` for a function pointer
(BG1-BG9 in `spec/bindgen.md`). An enumeration's integer type is chosen from its values rather than from clang, because
the Itanium and MSVC ABIs pick different signedness for the same enum and the choice only has to keep C's size.
`--wrappers` writes C for `static inline` functions as `<name>__extern`, and `--link-lib` now accepts a file path so
that C joins the link. The export side (`crates/burdin_driver/src/export.rs`) reads the checked program, writes a
header for the package's `extern "C" fn`s, their `repr(C)` structs, and their constants, and reports E0263 for a
declaration C cannot spell. `tests/check_bindgen.py` (every CI job) checks goldens, compiles and runs each generated
module, builds and calls the wrappers, compiles C and C++ against the export header, and on Linux runs the round trip
of a C shared library calling Burdin exports; `tests/m22_build/bindgen` invokes the tool from a build script. Linux CI
now installs `libclang-21-dev`.
The forty-fourth session implemented `burdin doc` (issue #234, sub-issues #467-#469, #656-#658): a documentation
generator that reads `///` and `//!` comments from the syntax trees (`burdin_syntax::docs`, shared with the
`missing_docs` lint so the two always agree), builds the documented API in the new crate `burdin_doc` (depending only
on `burdin_diag` and `burdin_syntax`), and writes it as a self-contained HTML site, versioned JSON, or a coverage
report. The site documents the public interface, with `--private` for `priv` items; a type's page lists its fields and
variants, methods, operators written as `a + b`/`-a`, extension methods with the module and `use` they need, derived
and implemented interfaces, and the methods of impl blocks for a specific instance (`Stack<i32>`). Intra-doc links
(`[Type]`, `[module.fn]`, `[`Type::method`]`, and explicit targets) resolve from the comment's module and report `D0001`
(nothing named) and `D0002` (a `priv` item) as warnings, which `--deny-warnings` turns into a failure; `--coverage`
counts the same items `missing_docs` (`L0017`, allow by default) reports. The site embeds its stylesheet and search
script and writes `search-index.js` as a script, so it works from disk; the output directory is refused unless it is
empty or was written by `burdin doc` (`.burdin-doc` lists what a run wrote, and a later run removes what is gone).
`spec/doc.md` is the specification (DC1-DC23), `tests/check_doc.py` runs the fixtures under `tests/doc/` against the
golden pages in `tests/doc/expect/` in every CI job; the doc tests are in the fifty-first session's paragraph below.

The thirty-third session implemented milestone 20, comptime (issue #157, sub-issues #158-#165 and #305-#308). It
decided first (issue #158, `spec/comptime.md` CT1) that the engine is an interpreter over the checked typed tree, in
the new crate `crates/burdin_comptime` (depending only on `burdin_tir` and `burdin_diag`), run by the driver after
the memory checks and before code generation; there is no MIR, and #159 is closed as not chosen. `comptime` is a
keyword: `comptime E` and `comptime { ... }`, `comptime name: T` parameters, `comptime if`, `comptime for`, and
`comptime assert(cond, "msg")` (also as a module item). Sema checks the visibility rule (only constants, comptime
parameters, `comptime for` bindings, and the expression's own locals; E0220), the materializable types (E0221), and
the forms that cannot be comptime (E0220); a `const` initializer is now any materializable expression (the
`const-expression` E9000 site is gone), and the memory checks also run on const initializers. The interpreter
(`interp.rs`, `prims.rs`) runs every expression form with owned values and places for `&mut`/`mut` parameters, moves
where the tree moves, runs user `Drop` implementations where compiled code would, follows the profile's overflow
rule, and matches the runtime's float text, integer parsing, hashing, and `size()`/`align()` layout; output, files,
the environment, processes, and C are E0224, panics are E0222, a failed assertion is E0223, and the step budget
(10,000,000, `--comptime-steps N`), call depth 512, and 1024 specializations per function are E0225. The pass
(`lib.rs`) evaluates every constant in declaration order (a cycle through calls is E0009), replaces each comptime
expression by its materialized literal (`reify.rs`), keeps only the selected `comptime if` branch, unrolls
`comptime for`, and specializes each call of a function with comptime parameters per argument value.
`std/src/target.bd` adds `target.os()`/`target.arch()` for platform branches. The driver runs the compiler on a
thread with a 1 GiB stack (reserved, not committed) so deep comptime recursion does not overflow. The tests are
`tests/m20_comptime/` with 40 tests, including `interp_*` tests that compare a comptime result with the same code
run at run time.

The thirty-second session implemented milestone 19, operator overloading and `Index` (issue #150, sub-issues
#151-#156). It decided first (`spec/operators.md`, OP1-OP13) that the operator interfaces are ordinary `std`
interfaces declared in `std/src/ops.bd` — `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Neg`,
`Not`, `PartialEq`, `PartialOrd`, `Index<T>`, and `IndexMut<T>` — visible through the prelude, with same-type
operands and at most one impl per type. The stage 0 built-in rule and a derived `Eq`/`Ord` are always tried first;
a user struct/enum then uses its interface impl and a type parameter its bounds. Comparisons use the new
`PartialEq`/`PartialOrd` rather than the derive-only `Eq`/`Ord`, so every derive keeps its behavior. `Index<T>` and
`IndexMut<T>` name the element type, take a `usize`, and return `&T`/`&mut T`; `x[i]` lowers to `*x.index(i)` and a
written place (`=`, `&mut x[i]`, `mut x[i]`, `x[i].f = v`, nested `g[i][j] = v`) selects `index_mut`. The
implementation lowers operators and indexing to ordinary `Call` nodes in sema, so codegen needed only two fixes:
a reference dereference is always a place, and `clone` reads it without a temporary drop (a non-`Copy` element was
being freed twice). The borrow checker now skips the result loan of a reference consumed by `*`, tracks a
re-borrow through the call's origin, and the closure classifier follows calls under a place, so `v[i] = v[j] + 1`
works, `&mut v[i]` still borrows `v`, and a closure that writes `v[i]` is `FnMut`. A literal never adopts a user
type (`v + 1` is E0003), and compound assignment and shifts stay non-overloadable. No new error code was needed:
E0141 covers a missing operator/index implementation, E0113 an immutable `index_mut` receiver, E0101 a move out of
an indexed element, and E0003 wrong operand types (including a failed derive). The E9000 row for hand-written
built-in interfaces moved to the #402 Stage 1 work, because the operator interfaces are now ordinary. The tests are
`tests/m19_operators/` with 36 tests; the suite is 472 tests.

The thirty-first session implemented milestone 18, extension methods (issue #143, sub-issues #144-#149). It
decided first (`spec/extensions.md`, EX1-EX8) that `fn Type.name(self, ...)` at module scope adds a method to any
type name in scope (local, imported, or built-in); the receiver's type arguments declare the extension's type
parameters and a `<...>` after the method name adds method-level ones, with bounds from inline parameters and
`where` predicates. An extension is visible in its declaring module and in modules that import it **by name**
(`use text.shout`, `use text.{shout}`); importing the module or another item does not bring it, an import is not
re-exported, `pub use` re-exports it, and `priv` is package-private (E0150). `x.f()` tries the type's methods
(inherent and interface, including the methods the compiler provides for built-in types) before a visible
extension, so a method always wins; a same-named extension gets the new warning E0215 at its declaration and the
program still compiles. A second extension for the same receiver in one module, and a second import of one
extension name, are E0007; a declaration without `self` first, a malformed receiver path, a `where` predicate
naming a non-parameter, and an import under another name are the new E0216. The parser reads the receiver path
without backtracking (`generic_list_end`), and sema registers each extension as an ordinary function whose first
parameter is the receiver, so `self`/`mut self`/`own self`, drop and clone behavior, views, and code generation are
the ordinary ones; a generic extension is monomorphized per instantiation. The driver now prints warnings on a
successful build, and `//@ stderr:` works on `run` tests so the warning is asserted. `docs/error-codes.md` gained
the first `warning-example` block, checked by `tests/check_error_docs.py`. The tests are `tests/m18_extension/`
with 32 tests, including package tests for imports, a dependency's extension and receiver type, a `pub use`
re-export, duplicates, an alias, and `priv`.

The thirtieth session implemented milestone 17, default and named arguments (issue #137, sub-issues #138-#142). It
decided first (`spec/arguments.md`, AG1-AG7) that a parameter written `name: Type = expr` may be omitted at the
call site and that the default is a constant expression checked once at the declaration, in the declaration's
module scope, where the function's parameters and `self` are not in scope; every call that omits the argument
materializes the value, so nothing is captured and no code runs at the declaration. A default cannot depend on a
type parameter, cannot be on a `mut` parameter, cannot appear in an interface method signature or an `extern`
declaration, and the parameters with defaults must be trailing. A call may write `name: value` for any function,
method, interface method, associated function, or generic function/method; positional arguments fill the leading
parameters in order, named ones may be reordered, a positional argument may not follow a named one, and built-in
functions and methods, the constructors, closure values, and `extern` declarations take no named arguments.
Arguments and defaults are evaluated left to right in parameter order. The parser reads both forms
(`Param.default`, `Arg.name`); `burdin_sema` records the checked default in `ParamSig.default` and fills it in
`args_pre`, matches names to parameters before generic inference, and produces the new codes E0213 (invalid
default) and E0214 (invalid named argument). The tests are `tests/m17_arguments/` with 25 tests, and parser unit
tests cover the syntax.

The fifty-first session implemented stage 3's doc tests (issue #250, sub-issues #650-#652). `burdin test` runs the
Burdin code blocks of the selected packages' `///` and `//!` comments with the `test` blocks, and `burdin test --doc`
runs only them: every untagged (or `burdin`/`bd`-tagged) fenced block becomes a one-file package whose only
dependency is the documented package, so an example sees its public API only. An example with no `fn main` is
wrapped in one, its leading declarations (the longest prefix that parses as module items) are hoisted above the
wrapper, and padding comment lines put each example line on the line of its doc comment — so a panic reports the line
a reader sees. A compile failure's diagnostics are rewritten from the generated file to the original one (the query
library's source map is rebuilt with the doc comment's file in the generated file's place) before they are shown;
the headings read `---- doc test "<owner> (line N)" at <file>:<line>: <reason> ----`. The 13 new driver tests and
the extraction/generation/mapping tests in `test_runner/doc_tests.rs` cover the pieces; `tests/check_test_runner.py`
gains about 30 checks over the fixtures `tests/runner/doctests/`, `tests/runner/doctests-bin/`, and
`tests/runner/doctest_one.bd`. The block attributes (#651) are `ignore`, `no_run`, `should_panic`, `compile_fail`,
and `compile_fail E0123` (the note names the code it did fail with); an unknown attribute warns at the fence's line
and the block still runs. A hidden `# ` (or bare `#`) line (#652) is compiled by the doc test and never shown by
`burdin doc` — `burdin_doc::markdown::block_lines`/`visible_code` are the one reader of the marker, and the
`tests/doc/geometry` example now hides its `use` line and runs green (it constructs points with a new `Point.new`).
`Block::Code` gained the fence's line so a tool can map a block back to its source line, `burdin fmt` leaves a block
with hidden lines as written and now treats `burdin,no_run` as Burdin, and `spec/test-runner.md` documents the whole
behavior in its "Doc tests" section (`spec/doc.md` DC6, `spec/fmt.md` FM10). Every check script passes, and the
conformance suite is unchanged at 1052 passed (debug) with 1055 tests.

| What | Where | Status |
| --- | --- | --- |
| Driver contract | `README.md` | Written: targets and `burdin.toml`, `build`/`run`/`test`/`explain`, diagnostics and their formats, `--emit-ir`, `--link-lib` and the `LLVM-C` resolution, `BURDIN_CC`, and exit statuses. The nineteenth session added `-g`, `--strip`, `--lto`, PGO, `--version`, `BURDIN_JOBS`, and the runtime behavior; the twenty-second added `BURDIN_VERSION`/`BURDIN_CHANNEL`, the `channel:` line, the installers, and `docs/releasing.md`. `tests/check_docs.py` checks the links and the documented behavior, and every CI job runs it and the other check scripts |
| Full language design | `Burdin Language Design.md` | Written. The repository copy is canonical; the copy on claude.ai is historical |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written and complete |
| Stage 1 plan | `spec/stage1-subset.md` | Written at the end of the tenth session: milestones 12+, open decisions, and the roadmaps for later stages |
| Test runner | `spec/test-runner.md` | The behavior of `burdin test`: attributes (E0170), `-j`, `-p`/`--workspace`/`--filter`, output capture, isolation, and the `[differential]` comparison between the comptime interpreter and the compiled test (issue #444) |
| Process and verification | `AGENTS.md`, `CHANGELOG.md` | Shared definition of done, milestone close-out checklist, verification policy, and shipped milestones. Process rules live here, not as GitHub issues |
| Grammar | `spec/grammar.md` | Parser follows it, including `test` items, generic arguments in expressions (rule 8.2), member type arguments (`x.downcast<T>()`), `any`/`impl` types, `interface`, `use`/`pub use`, `extern` items, `repr(C)`, ranges as values, `Self` in expressions and patterns, the permissive declaration layout, the call-site marker rules, `...` in an `extern` signature, interpolated and raw strings, and the format-specifier grammar |
| Memory rules | `spec/memory-rules.md` | Moves, `Drop`, drop order and flags, and `Heap` (milestone 5); conventions, exclusivity, references, views, and return origins (milestone 6); the one implicit conversion (IC1), iteration (IT1-IT3), the stored-view lifetime RT5 (E0123), the extended Unsafe rule US, and the settled place and temporary rules (issue #21). All stage 0 memory rules are implemented |
| Standard-library and derive surface | `spec/stdlib.md`, `spec/derives.md` | The names and signatures a stage 0 program may use (issues #28-#31), and the exact `Copy`/`Clone`/`Eq`/`Ord`/`Hash`/`Default`/`Debug` behavior, including the FNV-1a values (issue #39) |
| Formatting | `spec/format.md`, `std/src/fmt.bd`, `crates/burdin_syntax/src/derive.rs`, `crates/burdin_sema/src/fncx/format.rs` | Milestone 14 (issue #109). Interpolated literals, `Display`/`Debug`, the specifiers, `format` and the generalized output functions, all checked at compile time and lowered to `Formatter` calls; `derives (Debug)` generates an impl block as Burdin source. The tests are `tests/m14_format/` |
| Existential types | `spec/existentials.md`, `crates/burdin_syntax`, `crates/burdin_tir`, `crates/burdin_sema`, `crates/burdin_check`, `crates/burdin_codegen` | Milestone 15 (issue #117). `any Interface` is an owned two-word heap box `{data, vtable}` with implicit boxing where an expected type is known, dynamic method calls, type-id `is`/`downcast`, and object safety (E0202). `impl Interface` in a parameter list is a hidden generic parameter with that bound. New codes E0200, E0201, E0202, and E0204; the tests are `tests/m15_any/` |
| Errors | `spec/errors.md`, `crates/burdin_syntax/src/error_item.rs`, `crates/burdin_sema`, `std/src/std.bd` | Milestone 16 (issue #126). `error enum` declarations lower to an ordinary enum plus generated `Display`, `Error`, and `From<Source>` impls; `Result<T>` means `Result<T, Error>`, error types box implicitly, `.context` prefixes the chain, debug builds record traces, `fn main(): Result<T>` prints an `Err`, and test failures are typed. New codes E0210, E0211, and E0212; the tests are `tests/m16_errors/` |
| Arguments | `spec/arguments.md`, `crates/burdin_syntax`, `crates/burdin_tir` (`ParamSig.name`/`.default`), `crates/burdin_sema/src/fncx/{call,generic,item}.rs` | Milestone 17 (issue #137). `name: Type = expr` declares a constant default checked at the declaration and inserted at each call that omits it; `f(name: value)` matches a parameter by name in any order before generic inference; arguments evaluate in parameter order. New codes E0213 and E0214; the tests are `tests/m17_arguments/` |
| Extension methods | `spec/extensions.md`, `crates/burdin_syntax/src/ast.rs` (`Item::Extension`), `crates/burdin_sema/src/extensions.rs`, `crates/burdin_sema/src/{declare,imports,prelude,resolve}.rs`, `crates/burdin_sema/src/fncx/call.rs` | Milestone 18 (issue #143). `fn Type.name(self, ...)` adds a method to a local, imported, or built-in type; visible in its module and where imported by name; methods win over a same-named extension, which gets the warning E0215 at its declaration. Generic receivers and method parameters are monomorphized. New codes E0215 (warning) and E0216; the tests are `tests/m18_extension/` |
| Closures | `crates/burdin_tir`, `burdin_syntax`, `burdin_sema`, `burdin_check`, `burdin_codegen`, `spec/memory-rules.md` CL1-CL11 | Milestone 12 (issue #88). `Ty::Fn { sig, kind, borrowed }` is the closure interface type; `Fn`, `FnMut`, and `FnOnce` are contextual type names and generic bounds, monomorphized per closure type. A closure value is `{ code, env, drop }` with a heap capture record; `ExprKind::Closure` builds it, `ExprKind::ClosureCall` calls it, and `Program::closures` holds the bodies. Capture analysis (borrow vs move), the kind, the loans and move checks, function values, and the drop glue are implemented; the tests are `tests/m12_closures/` |
| Conformance tests | `tests/` | 1055 tests. All pass: m01 to m24 and s2_tasks, s2_channels, s2_shared, and s2_io (debug and release; the release run skips the debug-only and platform-specific tests) |
| Comptime | `spec/comptime.md`, `crates/burdin_comptime/`, `crates/burdin_sema/src/{comptime.rs,fncx/comptime.rs}`, `std/src/target.bd` | Milestone 20 (issue #157). An interpreter over the typed tree evaluates constants, comptime expressions, `comptime if` conditions, `comptime for` iterables, `comptime assert`, and `comptime` arguments, then rewrites the program so code generation sees no comptime construct (CT13). Codes E0220-E0225; `--comptime-steps N`; tests in `tests/m20_comptime/` |
| Diagnostics | `crates/burdin_diag`, `crates/burdin_driver`, `docs/error-codes.md`, `docs/diagnostics.md`, `spec/diagnostics.md` | Four formats (`human`, `short`, a versioned `json` with `docs/diagnostic.schema.json`, and `sarif`), color and width control, hyperlinks to the error reference, notes and suggestions with spans and edits, deterministic ordering, `--error-limit`, and `burdin explain`; `tests/check_diagnostics.py` and `tests/check_error_docs.py` run in every CI job |
| Test runner | `spec/test-runner.md`, `crates/burdin_driver/src/test_runner.rs` and `test_runner/{select,report,doc_tests}.rs`, `tests/check_test_runner.py` | `[ignore]`, `[timeout(ms)]`, `[should_panic]`, `[should_fail]`, `-j`/`--jobs`, `-p`/`--workspace`/`--filter`, output capture, and per-test isolation. An invalid attribute is E0170. The doc tests of the doc comments run with the blocks (or alone with `--doc`): each Burdin block becomes a one-file package depending on the documented package, wrapped in `fn main` when it has none, with its attributes (`ignore`, `no_run`, `should_panic`, `compile_fail`) and its hidden `# ` setup lines; a failure reports the doc comment's file and line (issues #250, #650-#652). The rules are in `spec/test-runner.md`; four m10 conformance tests and the black-box script (fixtures in `tests/runner/`) prove them, and every CI job runs the script (issues #438-#443) |
| Linter | `spec/lint.md`, `crates/burdin_lint/`, `crates/burdin_driver/src/lint.rs`, `docs/lints.md`, `tests/check_lint.py` | `burdin lint`: lints `L0000`-`L0016`, levels (`allow`/`warn`/`deny`, the `all` and `warnings` groups), the manifest `[lint]` table, `// lint: allow(...)` comments, and `--fix` with a diff preview and an all-or-nothing write. Fixtures in `tests/lint/`; every CI job runs the script (issues #230-#233, #464-#466, #661) |
| Type-check command | `spec/check.md`, `crates/burdin_driver/src/check.rs`, `tests/check_check.py`, `tests/check/` | `burdin check`: `front_end` without code generation or linking, a build's diagnostics and exit statuses, `--tests` for `test` blocks and library packages, `--release`, `--comptime-steps`. The script runs fixtures with goldens and every conformance test, and every CI job runs it (issue #237) |
| Error-code coverage | `tests/check_tests.py`, `tests/README.md` | Every registered code needs a `//~` marker or a row in the documented-exceptions table; CI fails otherwise (issue #445) |
| E9000 audit | `spec/e9000-audit.md`, `tests/check_e9000.py` | Every remaining `Code::UNSUPPORTED` site has a `// e9000:` marker, an owning milestone, and the issue that removes it. The stage 0 diagnostics that had no future milestone (duplicate names, cyclic declarations, invalid `Drop`/`extern`/`repr` declarations, non-generic type arguments) now have permanent codes and tests; string patterns and inclusive slicing are implemented (issue #84) |
| Platform support | `docs/platform-support.md`, `tests/check_platform_support.py` | The tested OS/architecture pairs (Linux x86-64, macOS arm64, Windows x86-64), the untested ones (Linux arm64, macOS Intel, Windows arm64), the runtime features each CI job proves, and the known gaps. The script checks the page against `ci.yml`, compiles and runs its example, and proves binary-safe standard output and file I/O byte-for-byte; every CI job runs it (issue #457) |
| Unit tests | `crates/*/src` | Focused tests for the front end: the lexer token stream (`burdin_syntax/src/lexer.rs`), parser recovery (`burdin_syntax/src/parser/mod.rs`), type unification (`burdin_sema/src/generics.rs`), and exhaustiveness (`burdin_sema/src/exhaust.rs`), plus type and place questions in `burdin_tir`, origin and move state in `burdin_check`, diagnostics in `burdin_diag`, and the driver's argument, selection, verdict, isolation, diagnostics-presentation, build-metadata, and parse-job logic (`burdin_driver`). CI runs `cargo test --workspace` in every job, and `tests/check_unit_tests.py` fails when a front-end crate or one of those four areas loses its tests (issue #45) |
| Standard library | `std/` | The std, written in Burdin and embedded in the driver: `std.bd` (the `Error` interface and dynamic `Error`, `TraceFrame`, the `IoError`, `ParseIntError`, and `AssertError` error enums, `CStr`, `assert`, `assert_eq`, `__test_assert`/`__test_assert_eq`, `unreachable`), `io.bd`, `env.bd`, `process.bd`, `mem.bd`, `iter.bd`, `fmt.bd` (formatting), `target.bd` (the compilation target, for `comptime if`), `reactor.bd` (the reactor thread, the timers, `__offload`, and the blocking-extern shim support), `net.bd` (TCP, UDP, `SocketAddr`, and `lookup_host`). Every package gets it as an implicit dependency and every module gets its public names as a prelude |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_tir`, `burdin_sema`, `burdin_check`, `burdin_comptime`, `burdin_codegen`, `burdin_driver` (`burdin` binary), `burdin_bench`, `burdin_fuzz` (the `burdin-fuzz-syntax` binary); see the layout in `CLAUDE.md` |
| Dependencies and licenses | `Cargo.lock`, `tests/dependencies.txt`, `tests/check_deps.py`, `docs/dependencies.md` | 18 external crates (`inkwell` plus everything it pulls in through `llvm-sys` 211); all licenses permissive (mostly MIT OR Apache-2.0; `inkwell` is Apache-2.0); exact `=x.y.z` pins with crates.io source and checksum; `tests/check_deps.py` checks all of it offline and re-verifies the licenses through `cargo metadata` after the build. Every CI job runs it (issue #449) |
| Reachability | `crates/burdin_codegen/src/emit/reach.rs` | Only functions reachable from `main`, the tests, exported functions, `Drop` implementations, and closure bodies get a real body; the rest are emitted as `unreachable` so the standard library does not bloat every program's IR |
| Runtime | `crates/burdin_codegen/src/runtime.rs` | `main`, buffered output with an explicit flush, panic backtraces, the stack-overflow guard, panics (`burdin_rt_panic`, `burdin_rt_panic_bounds`), `String` allocation/clone/push/push-char, heap allocation (`burdin_rt_alloc`, `burdin_rt_realloc`, `burdin_rt_free`), `CString` allocation (`burdin_rt_cstring_from`), number/char/str to `String`, `str` `starts_with`, `find`, `find_str`, `eq`, `cmp`, integer parsing, file reads/writes (`fopen`/`fread`/`fwrite`/`fclose`), `CStr.to_string` (`strlen` plus the string allocator), `env.args()`, `process.exit`, and `process.run` (`system`). Generated as LLVM IR into every program; calls libc (`write`, `malloc`, `realloc`, `free`, `memcmp`, `strlen`, `abort`, `exit`, `system`, the `fopen` family). `cc` only links. In a `burdin test` build the generated entry point also calls `atoi` to read the test index from `argv[1]` |
| Benchmarks | `crates/burdin_bench/` | Generated inputs for three sizes; `cargo bench -p burdin_bench` times lexing, parsing, name resolution/type checking, and code generation (median per iteration). `--smoke` checks every stage against generous budgets (issue #41); `--save` and `--baseline` record and compare medians (issue #446) |
| CI | `.github/workflows/ci.yml` | `linux` (`ubuntu-24.04`, LLVM 21 from apt.llvm.org), `macos` (`macos-latest`, Apple Silicon, Homebrew `llvm@21`), and `windows` (`windows-latest`, MSVC through `ilammy/msvc-dev-cmd`, LLVM 21 from the official `clang+llvm-21.1.8-x86_64-pc-windows-msvc` archive). All run check-ci, the directive check, docs checks, the debug-info/codegen-optimization/build-flag/parallel checks, fmt, clippy, unit tests, build, benchmark smoke budgets, `tests/check_platform.py`, `tests/check_platform_support.py`, and conformance m01-m24 and s2_tasks, s2_channels, s2_shared, and s2_io in both the debug and release profiles, plus the repeated stress tests of the stage 2 folders (`tests/run_tests.py --release` skips the debug-only overflow test); the Windows job also reruns m01 and m11 with `BURDIN_CC=cl`. All three cache the cargo registry and `target/` under a key built from `rust-toolchain.toml` and `Cargo.lock`, and cache LLVM 21 (the installed tree on Linux, the Homebrew bottle downloads on macOS, the unpacked archive on Windows); `tests/check_ci.py` guards all three jobs, both profiles, and their caches. Every job also runs `tests/bench_regression.py`: it compares the full benchmark suite with a baseline cached per runner, toolchain, and lockfile, and records a new one on a main push, advisory for now (issue #446). A `fuzz` job runs on the weekly schedule and by hand: it replays the corpus, runs `tests/fuzz.py --seconds 600`, and uploads `fuzz-out/crashes` on a finding; it builds only `burdin_fuzz` and its front-end dependencies, so it needs no LLVM (issues #288, #447) |
| Fuzzing | `crates/burdin_fuzz/` | Dependency-free deterministic lexer/parser fuzzing. `corpus/syntax/` holds the edge cases and every input that found a panic; `tests/corpus.rs` replays it plus 20,000 mutations during `cargo test --workspace`, which is the check that fails when a panic is reintroduced. `tests/fuzz.py` runs a longer session (the watchdog fails a stalled input) and the scheduled `fuzz` CI job uploads found inputs; a found input is committed under the corpus with its fix (issues #288, #447) |
| Release pipeline | `.github/workflows/release.yml`, `tests/build_release.py`, `tests/release_notes.py`, `tests/check_release.py` | Implemented, no release cut (issue #452, sub-issues #453-#456). The workflow builds the six supported targets on native runners, smoke-tests each, writes `burdin-<version>-<target>.tar.gz` plus SHA-256 checksums, verifies Linux reproducibility from two checkout paths, and publishes only from a tag or an explicit manual `publish`; the schedule and a plain manual run are dry runs. `tests/build_release.py` also packages an existing binary (`--binary`) for the check, and its archive is deterministic (sorted entries, fixed mtimes, uid/gid 0, `gzip -n`). `tests/check_release.py` runs in every CI job |
| Installers | `install.sh`, `install.ps1`, `docs/releasing.md` | One-line installers for Unix and Windows. Both verify `SHA256SUMS` before unpacking, install to a user directory, run `--version` and warn when the LLVM 21 runtime is missing, and support `BURDIN_VERSION`, `BURDIN_CHANNEL`, `BURDIN_INSTALL_DIR`, `BURDIN_BASE_URL`, and `BURDIN_RELEASE_DIR` (the local rehearsal path). A package-manager formula is not written yet |
| Version and release policy | `docs/releasing.md`, `Cargo.toml`, `crates/burdin_driver/build.rs` | SemVer for the compiler (`0.1.0`), language editions versioned separately, channels `dev`/`nightly`/`stable`, and the breaking-change rule for packages. `burdin --version` prints `burdin <version>`, `channel:`, `target:`, `llvm:`, `commit:`, and `profile:`; `BURDIN_VERSION` and `BURDIN_CHANNEL` override the version and channel and are checked by `tests/check_build_flags.py`, `tests/check_docs.py`, and `tests/check_release.py` |
| LLVM | WSL: `/usr/lib/llvm-21`; macOS: Homebrew `llvm@21`; Windows: `C:\llvm` | LLVM 21.1.8. Linked as a shared library on every platform by `crates/burdin_codegen/build.rs` (`llvm21-1-no-llvm-linking`): the static Windows archives crash in a binary that uses the dynamic CRT, so MSVC links `LLVM-C.lib` and needs `LLVM-C.dll` on `PATH`. Release builds on Linux and macOS set `BURDIN_STATIC_LLVM=1` and link LLVM statically (no `libLLVM` at run time); Windows release archives bundle `LLVM-C.dll`, and a static Windows build (`BURDIN_STATIC_LLVM=1` plus the static CRT) is wired in `crates/burdin_driver/build.rs` (the static archives are link arguments of the `burdin` binary, after every crate; the helpers are in `crates/burdin_codegen/llvm_link.rs`) but unproven (`docs/releasing.md`) |

Dev environment: WSL Ubuntu 26.04, Rust 1.100.0 pinned by `rust-toolchain.toml` through rustup, LLVM 21; the macOS and
Windows jobs are the reference for native setups. See the Toolchain section of `CLAUDE.md` for the exact packages and
how to call WSL from PowerShell.

### How the compiler is built so far

- `burdin_syntax`: complete lexer; parser for functions, constants, structs, enums, type aliases, method blocks
  (`Type { ... }` and `Type: Interface { ... }`), types, statements, struct literals (`P { x }`, `Shape.Rect { w: 1 }`),
  `if let`, `let ... else`, and the full expression precedence table with the newline rules of section 2 and
  statement-level and item-level error recovery. `use` items (path, `as`, and `.{...}`), `extern "C"` blocks, and
  `test` items are parsed; tests are kept in `File.tests` so a normal build strips them.
- `burdin_sema`: resolves functions, constants, locals, and the built-ins (`print`, `println`, `eprint`, `eprintln`, `panic`),
  and produces a typed tree (`burdin_tir`) where every expression has a type and a span. Types: `()`, `bool`, `char`, all
  integers (`isize`/`usize` are 64-bit), `f32`, `f64`, `str`, `String`, `Heap<T>`, `[T; N]`, `Option<T>` (matching
  only), `!`.
  Integer and float literals take their type from context (`expect` is passed down; a literal on the left of a binary
  operator takes the right side's type), else `i32` / `f64`; out-of-range literals are E0008, and a `-` before a literal
  is part of it. Built-in methods: `to_string` (integers, `bool`, `char`, `str`), `wrapping_*`, `checked_*` (add, sub,
  mul), `min`, `max`, `abs`. `match` works on integers, `char`, `bool`, and `Option` with literal, range, `|`, `@`, and
  guard patterns, and checks exhaustiveness (E0006). Constants are checked once and expanded at every use.
- Milestone 4 added tuples, structs, enums (unit, tuple, and struct variants), type aliases, and method blocks.
  Types and values are separate namespaces; `Point.new` and `Shape.Circle` resolve the left side as a type when no
  local or global value has that name. Methods are ordinary functions named `Type.method` with `self` first; `self` is
  read-only and passed by address, `mut self` is passed by address (the callee's `self` is the caller's place) and needs
  a mutable receiver (E0113). `own self` and `mut` named parameters were E9000 until milestone 6; `own` named
  parameters pass ownership and are implemented (milestone 5).
  `Some(x)` and `None` build `Option` values (`None` needs an expected type). `if let` is checked as a two-arm `match`;
  `let` takes any pattern, which must be irrefutable (E0006) unless it has an `else` block that diverges. `for` also
  iterates over arrays (by place, binding each element through the pattern). `==` works on tuples, `Option`, and on
  structs and enums that derive `Eq` (all their fields must support `==`). Derive names are checked, but nothing else
  about derives yet (milestone 7). Struct patterns may leave fields out (the grammar has no `..`).
- Milestone 5 added ownership. `Type: Drop { fn drop(mut self) }` interface blocks register a drop method, and a `Drop`
  type is never `Copy`; deriving both is E0130. `burdin_check/src/moves.rs` is the move checker: one function body at a
  time, from signatures only. It keeps an initialization state per local and field path (struct/tuple fields, enum
  variant fields, `Option` payload), computes the MV1 move contexts, and reports use of a moved or maybe-moved place
  (E0100), a move out of borrowed content (E0101), moving a field out of a `Drop` type (E0103), and moving an outer
  local inside a loop (E0104). The loop condition is checked inside the loop, only branches that can fall through are
  merged, and a guard that fails restores the drop flags of the bindings its pattern moved. The twentieth session
  extended the tracked forms (see the #48 bullet): heap fields, constant array and `List` indices, `match *h`, and a
  guarded moving pattern on a temporary scrutinee all work now. The move still rejected with E9000 is a non-`Copy`
  element through a runtime index (`let x = xs[i]` with a non-constant `i`). It does
  not rewrite the tree; it returns the list of places read in a move context (`Function.moved`) for code generation. `Heap.new` builds `Heap<T>`; `String.new`, `String.from`, `push_str`, and
  `clone` are built-in methods.
- Milestone 6 added the conventions, references, views, and exclusivity. Every parameter that is not `own` is passed by
  address (`ParamSig.by_ref`); `own` is passed by value. That is what makes a returned view of a read-only, `mut`, or
  `self` parameter refer to the caller's storage (RT1). `ParamSig.exclusive` separately marks the parameters whose
  argument is borrowed exclusively for the call (`mut`, `own`, `mut self`), so passing by address does not make every
  call exclusive (EX1).
  `&T` and `&mut T` are pointers in code generation; `&T`, `str`, and `[T]` are `Copy`, `&mut T` is not (RF4: reading
  through it or passing it where `&mut` is expected does not move it).
  `burdin_check/src/origins.rs` computes each signature's return origin (the single parameter a second-class result
  derives from, if any) to a fixpoint and stores it in `ParamSig.ret_origin`; a result the signature cannot attribute
  is checked against the body: E0121 at the signature for two or more sources, E0122 at the returned expression for a
  local, an `own` parameter, or a temporary.
  `burdin_check/src/borrows.rs` is the borrow checker (RF2, RF3, RF5): a loan records the place, whether it is mutable,
  and the locals that hold the reference, a copy of it, or a view derived from it; liveness is the last top-level
  statement that uses a holder (conservative inside loops, no CFG needed); an overlapping read of a mutable loan, or
  any overlapping write, `&mut`, or move while a loan is live is E0111. A call whose second-class result derives from
  an argument keeps that argument borrowed until the result's last use. Exclusivity (EX1, EX2) is checked in
  `burdin_sema` per call, receiver included, over the places each argument mentions (E0110). `match` on a reference or
  a slice binds views (MV8), and the move checker rejects moving out of borrowed content with E0101.
  Slicing `p[lo..hi]` works on arrays, `str`, `String`, and slices (bounds-checked, panicking on a bad range); `for`
  iterates slices; `as_str`, `len`, `starts_with`, and `find` are built-in methods.
- Milestone 7 added generics, interfaces, and the rest of the derives (`burdin_sema/src/generics.rs` and the driver in
  `check`). Generics are monomorphized inside sema, so the typed tree, the move and borrow checkers, and code generation
  only ever see concrete types. A generic struct or enum becomes one ordinary struct or enum per distinct argument list
  (`Stack<i32>`), a generic function one ordinary function (`max_of<i32>`), and the methods of `Stack<T> { ... }` and
  `Stack<T>: Iface { ... }` are instantiated with each instance. Instances are allocated on request: `resolve_type`
  and the call checker record a `Request` (a `RefCell` in `Globals`) and continue with `Ty::Error`; `check` allocates
  what was asked for and repeats signature resolution and body checking until nothing new is requested, keeping only
  the last pass's diagnostics. Every generic function, every impl of a generic type, and every default interface method
  is also checked once against its declared bounds, with the parameters as `Ty::Param` and bounds from `FnItem.bounds`:
  that is the "symbolic" check (E0141 for an operation the bounds do not give). Its code is discarded (the function
  becomes a stub with `symbolic: true`, skipped by the other passes); only diagnostics are kept, deduplicated by
  code and span with `Diagnostics::absorb`. Type arguments are inferred by unifying the parameter types of the symbolic
  instance (the "identity instance") with the argument types, and from the expected type for what is left open
  (`Stack.Empty`, `let z: i32 = pick()`).
  Interfaces are `InterfaceInfo` (`requires`, method signatures with `Self` as `Ty::Param("Self")`, default flags).
  `Type: Iface { ... }` registers its methods as ordinary methods of the type and instantiates the defaults it does not
  override. E0140 is a missing method without a default, E0141 a `requires` the type does not meet (or a bound a call
  does not meet), E0142 a built-in interface (`From`, `Eq`, ...) implemented for a type from the standard library.
  Bounds are `Bound` (`Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, or a user interface). Derives: `Eq` and `Ord` on
  structs, tuples, and enums (fields in order, variants in declaration order; codegen `cmp_at` returns -1/0/1 and
  `<`, `>`, `<=`, `>=` compare it with 0), `Hash` (`x.hash(): u64`, FNV-1a over the fields, codegen `hash_at`), and
  `Default` (`Type.default()`, built as a literal in sema; structs only), plus the earlier `Copy` and `Clone`. A derive
  on a generic type holds per instance (`Pair<A, B>` is `Eq` when `A` and `B` are); E0130 is reported only for fields
  that do not mention a type parameter.
- Milestone 8 added modules, packages, `priv`, `extern "C"`, `unsafe`, and the C bridge (`burdin_driver/src/package.rs`,
  `burdin_sema/src/imports.rs`, and the module-aware declarations in `globals.rs`/`session.rs`). The driver reads a
  minimal `burdin.toml` (`[package] name`, `[dependencies] name = { path = "..." }`) and loads every `.bd` under
  `src/` as a module named by its path (`geometry/shapes.bd` is `geometry.shapes`); a package's entry point is
  `src/main.bd`, and other packages reach it as `use <package>` through `src/<name>.bd` or `src/lib.bd`. A single
  `.bd` target is a one-module package. Every module has its own value, type, interface, and module namespaces, so
  two files may declare the same simple name; items are displayed internally with qualified names (`math.add`,
  `lib.Gadget`) which keeps function symbols and generic instances unique. `use` resolves a module (own package
  first, then the package's dependencies) and copies names into the importing module: `use math.add`,
  `use math.{add, mul}`, `use geometry.shapes as sh`, `use lib`. Qualified paths work in expressions, types, struct
  literals, patterns, and enum variants (`sh.Square { ... }`, `sh.Color.Red`, `let p: sh.Square`). `priv` is
  package-private: another package importing a private item, reading or writing a private field, or calling a
  private method gets E0150. `pub use` re-exports a name, and a dependency's non-root modules are reachable only when
  its root re-exports them; a public signature or field may not name a package-private type (E0150). `extern "C" { fn ... }` registers C functions by their plain symbol; calls require an
  `unsafe` block or `unsafe fn` (E0161) and use the C ABI with by-value parameters limited to integers, floats, and
  `CPtr<T>`. `unsafe` blocks are ordinary block expressions with an unsafe depth, and `unsafe fn` bodies are
  unsafe. `CPtr<T>` is a raw, `Copy` pointer; `CPtr.null()` takes its type from context (the twenty-third session added
  the pointer operations and made every `CPtr` value require `unsafe`; see the #48 bullet). `CString` is an owning,
  NUL-terminated buffer (`CString.from(str)`, `.as_ptr(): CPtr<CChar>`) with `Drop` and a deep `clone`; `CChar`,
  `CSize`, and `CVoid` are the C names (`i8`, `usize`, and unit behind a pointer). Extern declarations are
  deduplicated by symbol, and a second declaration with a different signature is an error.
- Milestone 9 added the standard library. `Result<T, E>`, `List<T>`, `Map<K, V>`, and `Chars` are built-in types
  (`Ty::Result`, `Ty::List`, `Ty::Map`, `Ty::Chars`): `Ok`/`Err` construct and match like `Some`/`None`, `?` lowers
  in sema to a match that returns early through the enclosing `Option`/`Result` and converts the error through a
  user `Type: From<Source>` block, and `orelse` lowers to a match. Built-in methods cover `Option`/`Result`
  (`unwrap`, `expect`, `is_some`, `is_none`, `is_ok`, `is_err`), `String` (`push`, `find_str`, `chars`, comparison
  with `==`), the integer `parse` associated function, and the containers (`List`: `new`, `push`, `pop`, `last`,
  `insert`, `remove`, `clear`, `contains`, `sort`, `len`, indexing, `for`; `Map`: `new`, `insert`, `get`, `remove`,
  `contains_key`, `clear`, `len`, `for` over `(k, v)`). `List` and `Map` use the three-word `String` layout in
  the C heap; `Map` is an open-addressing hash table (twenty-fourth session, issue #82), not a linear scan.
  Drop and clone glue, `==`, `Ord`, and the move, borrow, and origin checkers understand the new types. `Chars`
  is only an iterable: `for c in s.chars()` decodes
  UTF-8 in code generation. The prelude also provides file I/O (`read_to_string`, `write_file`), `env.args()`,
  `process.exit`, `process.run`, `assert`, `assert_eq`, `unreachable`, `mem.swap`, and `mem.replace`. The
  `IoError`, `ParseIntError`, and `AssertError` names were aliases for unit in `std` at milestone 9; milestone 16
  replaced them with real error types (see the milestone 16 note). The compiler's own primitives behind the std
  wrappers are the underscored built-ins `__read_to_string`, `__write_file`, `__args`, `__exit`, `__run`, `__swap`,
  and `__replace`, plus a small runtime of C calls (`fopen`/`fread`/`fwrite`/`fclose`, `system`, `exit`, `strlen`,
  `memcmp`, `realloc`).
  The driver embeds `std/` with `include_str!` and adds it as a dependency of every package; `prelude.rs` copies
  its public names and module aliases into every module, keeping a module's own declarations.
- Milestone 10 added the test runner. The parser reads `test "name" [attrs] { ... }` into `ast::File.tests`, apart
  from the other items, and builds each one as a private function `__burdin_test_N` with no parameters returning
  `Result<(), AssertError>` whose body is the block followed by `Ok(())`. Normal builds never declare or check them
  (tests are stripped); with `burdin_sema::check(..., test_mode, ...)`, the entry package's tests are declared and
  checked, the prelude replaces `assert`/`assert_eq` with `std`'s `__test_assert`/`__test_assert_eq` (which return
  an error instead of panicking; milestone 16 made it the typed `AssertError`), and `main` is not required.
  `Program.tests` carries each test's display name,
  `FuncId`, and validated attributes (`tir::TestAttrs`). Code generation then emits a dispatcher `main` instead of
  calling the program's `main`: it reads the test index from `argv[1]` with `atoi`, calls that test, and returns 0
  when its `Result` is `Ok` and 1 when it is `Err`.
  Issue #438 added the runner ergonomics (`crates/burdin_driver/src/test_runner.rs`). A test item may carry
  `[ignore]`, `[timeout(ms)]` (`0` means no limit), `[should_panic]`/`[should_panic("text")]`, and `[should_fail]`
  in one bracket list; `burdin_sema` validates them and E0170 reports an unknown name, a repeat, a value the
  attribute does not take, or `should_panic` with `should_fail`. `burdin test` compiles one test binary per selected
  package (the entry package by default; `-p` selects a package of the graph and `--workspace` the entry package and
  every path dependency), runs each test in its own process with `-j` workers (default: the CPU count), and reports
  in declaration order. Each process gets a fresh temporary working directory, a clean environment (only `PATH`,
  `TMPDIR`/`TMP`/`TEMP`, `BURDIN_TEST`, and the Windows CRT variables), and a captured stdout/stderr shown only on
  failure (`--verbose` shows it always). A timeout kills the process and marks the test failed; `should_panic`
  requires an abnormal ending and `should_fail` requires the dispatcher's error exit (1). The summary is
  `test result: N passed, M failed`, with `, I ignored` when tests were skipped; `--filter` selects by name. The
  panicking `assert` of normal builds is unchanged. The rules are in `spec/test-runner.md` and the black-box checks
  in `tests/check_test_runner.py`.
- Milestone 11 added `CStr.to_string`: `CStr` is a `std` alias for `CPtr<CChar>`, and a `CPtr<CChar>` has an
  associated `to_string` that reads the bytes with libc `strlen` and copies them into a `String`
  (`Builtin::CStrToString`). `burdin` links `--link-lib LLVM-C` through `llvm-config`: Ubuntu installs the LLVM C API as
  `libLLVM-21.so`, not `libLLVM-C.so`, so the driver asks `llvm-config --libdir` and `--link-shared --libs` and adds
  `-L` plus an rpath. The m11 program builds and verifies `define i32 @answer() { ret i32 42 }` through the LLVM C API
  and prints the IR. (Emitting an object file from the LLVM C API is still a later step in the stage 1 compiler.)
- **Issue #48 closed the stage 0 gaps.** The work, by theme:
  - *View lifetime (#49).* `burdin_check/src/origins.rs` was rewritten around an `Origin` with parameter, local, and
    temporary sources plus the block tree. RT5 is the new rule (E0123): a second-class value stored in a `let` or an
    assignment may derive only from static data, a parameter, or a local of the same or an enclosing block; a
    temporary or an inner-block local is rejected at the binding. `match`/`if let`/`for` pattern bindings are
    exempt, and a synthetic scrutinee local (`LocalKind::Scrutinee`) carries the temporary's origin.
  - *Ranges as values (#53).* `Ty::Range(T, inclusive)` holds the endpoints; `ExprKind::Range` builds one and
    `ForRangeValue` iterates a stored one. `Range<T>` and `RangeInclusive<T>` are builtin names, the endpoints are
    integers of one type (the `for` forms keep their inference), and the structural `==`, ordering, `Hash`, `clone`,
    and drop paths cover them. `for i in r` evaluates `r` once and does not borrow it.
  - *Moves (#58).* `burdin_check/src/moves.rs` tracks constant array and `List` indices, heap pointee fields (the
    `MOVED_DEREF` path step), `match *h`, and a moving pattern with a guard on a temporary scrutinee (the scrutinee
    is wrapped in a compiler-made local of the same statement). A runtime index is still E9000. Code generation
    follows the same paths (`place_path`, `drop_at_address`), and a whole `*h` move leaves the `Heap`'s drop to free
    the allocation once.
  - *Generics (#64).* A static method of a generic type infers its arguments from the method's own parameters; a
    generic type's bounds are checked at instantiation (E0141); a view type argument makes the instance
    second-class (SC3) and is rejected in a stored position; an impl method is compared with the interface method
    (E0003, including `unsafe`); `==`/`<` on symbolic instances follow the bounds; `Self` resolves in expressions,
    struct literals, patterns, and type paths.
  - *Modules (#77).* `pub use` re-exports a module or item, imports resolve to a fixpoint so a re-export may come
    later in module order, a dependency's non-root modules are hidden unless the root re-exports them, and a public
    signature or field naming a package-private type is E0150.
  - *C bridge (#71: #72-#76, #489, #490).* `CPtr<T>` gained load/store through `*p` (`ExprKind::PtrDeref`), `.is_null()`,
    `==`/`!=`, `p ± n`, `p - q` (an element count), and casts to/from integers and other `CPtr` types; every creation,
    hold, use, or move of a `CPtr` now requires `unsafe` (E0161), and a reference converted to a pointer ends its loan
    at the cast.
    `CString.clone` copies the NUL-terminated buffer in the runtime. Out-parameters work through
    `&mut x as CPtr<...>`. `struct Name repr(C)` has the C layout and `Type.size()`/`Type.align()`; `...` declares a
    variadic C function; `extern "C" fn` exports a Burdin function under its plain symbol with the C ABI rules
    (E0162). Aggregates cross the ABI through `CPtr<T>` only (by-value lowering is not implemented), C `_Bool` is
    declared `u8`, and C `char` is `CChar`.
  - *Coverage (#85-#87).* `unsafe fn` and unsafe interface methods have run and compile-fail tests, and the
    memory-rule coverage matrix in `spec/memory-rules.md` was audited against the suite (every referenced test
    exists, and US now lists the new tests).
- **Milestone 12 added closures.** The parser reads `|a, b| expr`, `|a: i32|: T { ... }`, and `||`, and the type
  `Fn(A, B): R`/`FnMut`/`FnOnce` in types and bounds; `generic_args_ahead` now tracks parentheses and brackets, so
  `List<Fn(i32): i32>` parses. In `burdin_tir`, `Ty::Fn { sig, kind, borrowed }` is the interface type (`borrowed`
  marks a second-class closure), `ExprKind::Closure { closure, captures }` builds a value, `ExprKind::ClosureCall`
  calls one, and `Program::closures` holds each body as a `Function` whose locals include its captures. `burdin_sema`
  checks a closure by capturing the body's free names from the enclosing `FnCx` (eagerly, so nested closures capture
  through the outer record), infers parameter types from the expected `Fn(...)` type or an `Fn` bound, and
  classifies the body's uses of the captures into a capture mode and `Fn`/`FnMut`/`FnOnce`. `burdin_check` moves a
  moved-in capture and reads a borrowed one at the construction, takes loans for borrow captures held by the
  closure, and applies the call's borrow or move to the callee; the closure body is checked as its own function.
  `burdin_codegen` builds `{ code, env, drop }`, allocates the capture record, generates the body function with the
  record as its first parameter, and emits an indirect call; the record's destructor drops moved-in captures and
  frees it, and a `FnOnce` body frees the record after dropping what it did not move out. `tests/m12_closures/`
  covers the behavior.
- **Milestone 15 added `any` and `impl`.** `any` and `impl` are keywords (`TokenKind::Any`/`Impl`) and ordinary
  member names after `.`; `TypeKind::Any`/`Impl` parse the interface after the keyword. `fn_decl_opt` desugars a
  parameter written `impl I` into a hidden generic parameter (`impl0`, `impl1`, ...) with `I` as its bound and
  replaces the parameter's type with a path to it, so inference, monomorphization, bounds, and the symbolic check
  are the ordinary generic ones; any remaining `TypeKind::Impl` is E0201 at `resolve_type_in`. `ty.rs` adds
  `Ty::Any(AnyRef { id, name, iface, args })`, `tree.rs` adds `ExprKind::BoxAny`, `AnyCall` (receiver, interface id,
  slot, parameter conventions, arguments), `AnyIs`, and `AnyDowncast`, and `Program` carries `any_ifaces`,
  `any_vtables`, and `any_type_ids`. `burdin_sema/src/any.rs` interns interface instances by `(interface, args)`,
  computes their vtable slots lazily (own methods, then the transitive `requires` methods, deduplicated by name),
  checks object safety, interns type identities and vtables, and records every written `any` for the deferred
  E0202 report at the end of `resolve_all`; `has_eq_in`/`is_copy`/`is_clone` are false and `needs_drop` is true.
  `fncx/expr.rs` boxes a value whenever the expected type is `Ty::Any` (`coerce_any`: E0120 for a view, E0141 for a
  missing implementation, E0204 when the compiler provides the implementation without a method), and
  `fncx/any.rs` checks dynamic calls and `x.is<T>()`/`x.downcast<T>()`; the parser accepts `[ GenericArgs ]` after a
  member name using the rule-8.2 look-ahead without the uppercase restriction. `burdin_check` moves the boxed value,
  consumes the box on `downcast`, takes the receiver's shared/exclusive loan on a dynamic call, and treats a box as
  an owned value for origins. `burdin_codegen/src/emit/any.rs` emits the value as `{ ptr, ptr }`, one private
  unnamed-addr constant `{ i64 type_id, ptr drop, ptr method... }` per vtable (memoized by `Program::any_vtables`
  index), the indirect call `(data, args...)`, the type-id test, and the downcast (match: load and free without
  drop; mismatch: vtable drop then free); `drop_value` runs the vtable's drop and frees, `reach.rs` roots every
  vtable entry, and the module layout documentation is updated. `tests/m15_any/` covers the behavior.
- **Milestone 16 added errors.** `error enum` declarations parse into `ast::ErrorEnumDecl` and lower in
  `crates/burdin_syntax/src/error_item.rs`, before `derive::expand_derives`, into an ordinary `EnumDecl` plus
  generated `Display`, `Error`, and `From<Source>` impl blocks written as Burdin source with the declaration name's
  span. The lowering checks each message template (a placeholder must name a payload field) and each `from` (a
  one-field tuple variant, one conversion per source type, and none on a generic error enum), reporting E0210 and
  E0211. At parse time the same module renames a written `fn main(): Result<...>` to `__burdin_main_result` and
  appends a `main` that prints an `Err` with `Display` to standard error and exits 1; it also wraps each test body
  as `__burdin_test_N_body` with a `__burdin_test_N` that prints the `AssertError` before returning it, and the
  compiler appends the condition's source text and the call's location to every test-mode assertion. Sema resolves
  the one-argument `Result<T>` as `Result<T, Error>`, boxes an error type implicitly where `Error` is expected,
  checks `.context` and the `?` conversion (E0212), and `orelse` takes a closure handler (issue #488). The checker
  and code generation see only ordinary values. `std/src/std.bd` declares the `Error` interface and struct
  `{ value: any Error, frames: List<TraceFrame>, context: String }`, `TraceFrame`, the `context`/`is`/`downcast`
  helpers, and the `IoError`, `ParseIntError`, and `AssertError` enums; a debug build records creation and
  propagation frames and a release build records none. `tests/m16_errors/` covers the behavior.
- **Milestone 18 added extension methods.** `ast::Item::Extension` holds an `ExtensionDecl { recv, func }`; the
  parser recognizes `fn Type.name` by scanning the receiver path (including a generic list whose closing `>` is
  followed by `.`) before consuming anything, so an ordinary function and an extension never need backtracking, and
  `fn_rest` is shared by both. `Globals::extensions` holds one `ExtensionInfo` per declaration (receiver pattern,
  parameters, bounds, optional `generic_fns` template, function), and each module has an extension namespace copied
  by `use module.name` exactly like other items (`pub use`, `priv`, E0150, E0007, E0216 for an alias). The receiver
  is resolved in `declare_values`/`register_extensions` and again in each `resolve_all` pass, because a generic
  receiver's instance may not exist yet; `GenericFn.recv` makes `allocate_fn` give a generic extension instance its
  `self` type. A call checks the type's methods and the compiler-provided methods first and falls back to
  `FnCx::extension_call`, which matches the receiver pattern (`extensions::extension_applies`) and then calls the
  ordinary function or `generic_call_with_receiver`. `check_extension_problems` reports a duplicate same-receiver
  declaration (E0007) and a shadowed extension (E0215 warning) once the methods exist. Code generation needs no
  change: the extension is an ordinary function with a `self` parameter, reachable through the call.
- **Milestone 17 added default and named arguments.** `ast::Param.default` and `ast::Arg.name` carry the syntax,
  and `tir::ParamSig` gained `name` and `default`. In `resolve_all`, `fncx::check_defaults` checks each default in
  a fresh `FnCx` with the function's module, subst, and `self_ty` but no parameters or locals, stores the typed
  constant in the signature, and reports E0213 for the invalid placements (before a required parameter, on `mut`,
  on a type-parameter type, non-constant); the interface loop and `extern_signature` report the two declaration
  cases. `FnCx::arg_slots` matches written arguments to parameters (positional fills the next slot, a named
  argument fills the parameter with that name) and reports E0214; `args_pre` checks the arguments and inserts each
  omitted default in parameter order, so code generation is unchanged. Generic calls match names and fill defaults
  through the same helper before inference and then `args_pre`; the same applies to generic methods. `tests/m17_arguments/`
  covers the behavior.
- Exhaustiveness (`burdin_sema/src/exhaust.rs`) is the usefulness algorithm over a pattern matrix, with integer and
  `char` ranges split at their boundaries. It handles nested tuples, structs, enums, and `Option`.
- `burdin_codegen`: typed tree to LLVM IR to an object file. `str` is `{ ptr, usize }`, `String` is `{ ptr, len, cap }`,
  `Heap<T>`, `CPtr<T>`, `CString`, `&T`, and `&mut T` are a `ptr`, `[T]` is `{ ptr, usize }` like `str`, `Option<T>` is `{ i1, T }`.
  Functions are `burdin.<name>`, and the generated `main` calls the program's `main`. Extern functions keep their plain
  C symbols and share a symbol with the runtime's own libc declarations when the types match. Stack slots live in the `entry` block; after a terminator, generation continues in a fresh unreachable block.
  Overflow of `+ - *` and unary `-` panics in debug (LLVM `*.with.overflow`) and wraps in release; division by zero and
  out-of-range shifts always panic; `MIN / -1` panics in debug and wraps in release; float-to-int casts saturate
  (`fpto*i.sat`). Panics print `path:LINE:COL: panic: message` to stderr and `abort()`. Tuples and structs are LLVM
  structs (a `()` field is `{}`); an enum is `{ i32 tag, [N x iA] payload }` sized for its largest variant, whose
  fields are read through the payload pointer as the variant's own struct. Pattern tests on variants use `select`, so
  they may load another variant's bytes but never branch on them.
- **Ownership and drops:** every place read in a move context gets a runtime `i1` drop flag in a stack slot, set when
  the place is initialized, cleared when it is moved, and checked at every drop site. The flags make branch and loop
  merges automatic: no static state is needed at each program point, and a partially moved aggregate is dropped field
  by field (DR7). Scopes drop locals in reverse declaration order, own parameters after the body's locals, statement
  temporaries at the end of their statement, and early `return`/`break`/`continue` drop everything they leave. Drop and
  clone glue are internal functions (`burdin.drop.N`, `burdin.clone.N`), memoized by type and declared before the
  bodies that use them, so a type that contains itself through `Heap` terminates. `String` drop frees the buffer;
  dropping a `Heap` drops the pointee and frees the allocation; moving the pointee out of a `Heap` frees it. `clone`
  is deep: `Copy` values are copied bitwise, `String` copies its buffer, `Heap` allocates and clones the pointee, and
  aggregates clone field by field.
- `burdin_driver`: `build` (writes `<package name>` in the current directory, `-o` to change it, `--emit-ir file.ll`,
  and `-g`, `--strip`, `--lto`, `--pgo-generate`, `--pgo-use`), `--version` (version, target, LLVM, commit, profile),
  `run` (builds into a temporary directory, passes the exit code through, 128+signal for a signal), and `test` (builds
  in test mode and runs one process per test; it owns the counts and `FAIL` lines). A target is a `.bd` file or a
  package directory; dependency cycles, missing paths, invalid module names, and malformed manifests are reported as
  `error: ...` without diagnostics codes. `--link-lib LLVM-C` resolves the LLVM shared library through
  `llvm-config`, which it finds through `LLVM_SYS_211_PREFIX` and then `PATH`; it accepts Linux's `libLLVM-C.so` /
  `libLLVM-21.so` and Homebrew's `libLLVM-C.dylib` / `libLLVM.dylib`, and adds `-L` plus an rpath. On MSVC it passes
  `LLVM-C.lib` and adds `/LIBPATH:<LLVM_SYS_211_PREFIX>/lib` when that import library is there. A missing `cl` is
  reported with the fix (a Developer Command Prompt, or `BURDIN_CC`).
- Issue #21 settled the decisions and closed four small gaps: `check_drop_signatures` (`burdin_sema/src/session.rs`)
  now requires an exclusive `mut self` receiver, `CPtr.null` (`burdin_sema/src/fncx/call.rs`) rejects arguments with
  E0004, `<`/`<=`/`>`/`>=` on `str` and `String` go through `cmp_at` instead of the integer path
  (`burdin_codegen/src/emit/ops.rs`), and an invalid `extern "C"` type or convention is E0162
  (`burdin_sema/src/resolve.rs`).
- Diagnostics are collected, sorted by file and byte offset (ties by code, message, and discovery order), and
  rendered once, so the output is deterministic and never streamed. The human format starts each diagnostic with
  `path:LINE:COL: error[E0000]: message`, colors the location, severity, and code when enabled, wraps to the width,
  and prints one line per note (`= note:`) and suggestion (`= help:`); `short` is the plain one-line format the
  test scripts parse; `json` is one object per diagnostic plus a summary object with structured spans, notes, and
  suggestions (a span and a replacement). `--color`, `--no-color`, `NO_COLOR`, `--width`, and `COLUMNS` control the
  human format; `--error-limit N` truncates after sorting and reports how many were hidden, and the default prints
  every error. `burdin explain <code>` prints the entry from `docs/error-codes.md`, which is embedded in the driver
  and checked by compiling every snippet; `tests/check_diagnostics.py` covers the presentation and ordering.
  Semantic checks run only if there were no syntax errors.

### Compiler benchmarks and regression tracking (issues #41, #446)

`crates/burdin_bench` generates its inputs (`src/lib.rs`) and the `benches/compiler.rs` target times one stage at a
time. A generated chunk is a struct with methods, an enum with a `match`, a generic function, an arithmetic loop, a
`List` loop, and a `String` loop; `main` calls the arithmetic and enum functions. The programs use only built-in
types and methods, so sema does not spend each iteration checking `std`. The parser case clones a pre-lexed token
stream because the parser takes tokens by value; that copy is part of the number.

First baseline, 2026-11-14, `cargo bench -p burdin_bench`, medians per iteration, `bench` profile, WSL Ubuntu 26.04
on x86-64 with Rust 1.100.0 and LLVM 21.1.8 (a different machine will differ; compare a run against itself, not
against this table):

| Case | small (8 chunks, ~600 lines) | medium (32, ~2,400 lines) | large (128, ~9,600 lines) |
| --- | --- | --- | --- |
| lex | 125 us | 417 us | 1.55 ms |
| parse | 345 us | 1.30 ms | 4.92 ms |
| sema | 492 us | 1.78 ms | 7.15 ms |
| codegen (debug) | 16.8 ms | 53.3 ms | 201 ms |

`cargo bench -p burdin_bench -- --smoke` runs 16 chunks through all four stages, checks that the generated program
compiles, and fails when a stage exceeds 1 s (lex, parse), 2 s (sema), or 10 s (codegen). Those buy roughly two to
three orders of magnitude of headroom, so a busy runner passes and only a hang or an accidental super-linear pass
fails. CI runs it after `cargo build`, and `tests/check_ci.py` requires the step and the bench target.

For tighter tracking, `--save FILE` writes a run's medians in a plain-text baseline (`case<TAB>nanoseconds`, one line
per case), and `--baseline FILE` compares a later run with it: each case gets a row with the baseline median, the
current median, the change, and a verdict, and the harness exits 1 when a case is more than `--threshold` percent
slower (default 25; exactly at the threshold is within it). A case with no baseline entry is reported as new and a
baseline entry the run did not measure as missing; neither is a regression. `tests/bench_regression.py` runs the full
suite and does both, and every CI job runs it. The baseline lives in `~/.cache/burdin/bench`, restored and saved with
`actions/cache` under a key that names the runner OS and architecture, the pinned toolchain, and `Cargo.lock`, so a
development machine is never compared with a runner and a toolchain or dependency change starts a fresh baseline. A
push to main records the new baseline (even when the run flagged a regression, so the baseline follows main); a pull
request compares against the last main run on the same platform and never writes the cache. The comparison is
advisory (`continue-on-error`) until the runner numbers are stable: a regression is shown in the job log without
failing the job. A missing baseline (including one evicted after seven days without a run) is recorded and passes.
`tests/check_ci.py` fails if the step, the script, the cache, or the harness modes go missing, and
`cargo test -p burdin_bench` covers the format and the threshold arithmetic.

### Debug information, observability, and performance (issues #421-#432)

The non-feature work of issue #421 is implemented and checked by four scripts that every CI job runs.

- **Source-level debug information (#422, `-g`/`--debug-info`).** `crates/burdin_codegen/src/emit/debug.rs` emits
  DWARF (and the LLVM debug metadata the COFF backend carries) for every function, parameter, local, and expression.
  Instructions get the line and column of their expression's span, so a breakpoint on a Burdin line stops there and
  the locals are readable. Debug types model the scalars, references, arrays, tuples, structs, `Option`, `Result`,
  `str`, `String`, and the containers; an enum with a payload is an opaque named type. On macOS ld64 keeps DWARF out
  of the executable, so the driver runs `dsymutil` and writes `<output>.dSYM`. `tests/check_debug_info.py` builds a
  program with `-g`, compares its output, reads the sections with `llvm-dwarfdump` (from the `.dSYM` on macOS), and,
  when `gdb` is installed, breaks on `dbg.bd:7` and reads `sum`, `a`, and `b` (3, 1, 2). The optimized profile keeps
  the line table and runs the same. Cost: the hello-world executable grows from 27992 to 30480 bytes in debug and
  from 18232 to 21424 in release.
- **Panic backtraces (#423).** After the unchanged `path:LINE:COL: panic: message` line, a panic prints
  `stack backtrace:` and the symbolized frames above the runtime. Unix uses libc `backtrace_symbols_fd`; Windows uses
  `RtlCaptureStackBackTrace` and links `kernel32.lib`, and gives addresses because no PDB is produced. The two runtime
  frames between the panic function and the caller are skipped; `notail` on the panic call keeps the panicking
  function's frame; every defined function gets `uwtable`, and on Darwin `frame-pointer=all` because libSystem's
  `backtrace` needs a frame chain; Burdin functions keep external `burdin.<name>` symbols, with `-rdynamic` on ELF,
  so names resolve. Decision: backtraces are always on, in both profiles. An optimized build may inline the frames
  around the panic, so the name test is `debug-only` (not on Windows) and the release test only requires a frame
  address.
- **Buffered standard output (#424).** `burdin_rt_out_write` appends to an 8 KiB block buffer, line buffered when fd 1
  is a terminal (`isatty`); standard error stays unbuffered. `burdin_rt_flush` runs from `atexit` (normal return and
  `process.exit`), before a panic message, and from the stack-overflow handler, so no output is lost. The existing
  `m02_basics/panic_flushes_stdout.bd` and `m09_stdlib/exit_code.bd` tests now witness the buffer, not the old
  unbuffered path. Measured: 200,000 lines of 70 bytes (14 MB) reach `/dev/null` in 8 ms in a release build; a
  std-heavy program links in 26680 bytes stripped.
- **Main-thread stack overflow guard (#425).** On Unix the runtime installs a 64 KiB alternate signal stack and a
  `SA_SIGINFO` `SIGSEGV` handler, compares the fault address with the main stack's guard region (recorded through
  `pthread_getattr_np`/`pthread_attr_getstack`, or `pthread_get_stackaddr_np` on macOS), prints
  `panic: stack overflow` or `panic: invalid memory access`, flushes, and exits 134. Windows registers an
  `AddVectoredExceptionHandler` filter for `EXCEPTION_STACK_OVERFLOW`. `m02_basics/stack_overflow_aborts.bd` recurses
  a million frames in both profiles. The handler is `SA_RESETHAND` and has a reentrancy guard.
- **Monomorphization and code size (#426).** Generic instances were already unique by type arguments. Drop and clone
  glue are now keyed by a structural type key (`Codegen::structural_key`), so two distinct types with the same shape
  share one `burdin.drop.N`/`burdin.clone.N`; recursion through `Heap` is cut off with `rec`. The IR check sees one
  drop glue for `Heap<A>` and `Heap<B>` where `A` and `B` have the same fields. LLVM's `MergeFunctions` pass was
  evaluated and rejected: Burdin functions must stay externally visible for backtrace symbols, the remaining
  identical bodies are already shared, and enabling it changed neither the symbol count nor the binary size. The
  fixture executable is 28032 bytes.
- **Drop-flag elimination (#427).** `burdin_check` now records every point where a local's scope can end (block
  fallthrough, `return`, `break`, `continue`) and marks a whole-local move `always_dead` when all observations see it
  moved. Code generation allocates no `i1` flag and emits no drop for such a local. A conditional move
  (`if c { take(s) }`) still gets its flag. `tests/check_codegen_opt.py` pins the IR count: one flag for the
  conditional fixture and none for the unconditional one. Adjacent drops of one aggregate were already merged
  field-by-field by DR7.
- **Bounds-check elision (#428).** A constant array index that is in range, and `for i in 0..xs.len() { xs[i] }`,
  skip the check; every other dynamic index keeps the panic. The elision lives in `place.rs` with a fact stack
  (`bounds_facts`) that is pushed only around the loop body and popped afterwards. `tests/check_codegen_opt.py`
  requires zero `bounds.fail` blocks in the elidable fixture and runs an out-of-range index that still panics; the
  existing `m02_basics/index_out_of_bounds_aborts.bd` keeps the other side.
- **LTO and PGO (#429).** `--lto` writes LLVM bitcode and links with `-flto`; `--pgo-generate` runs
  `pgo-instr-gen,instrprof` and links `-fprofile-instr-generate`; `--pgo-use FILE` runs `llvm-profdata merge`,
  `opt -passes=pgo-instr-use[,default<O2>] -pgo-test-profile-file=...`, then `llc -relocation-model=pic`, and links
  normally. Both need an LLVM compiler driver (`BURDIN_CC=clang`); MSVC rejects them, and `--lto` cannot be combined
  with PGO. Measured on `fib(34)` with `--release`: plain 26 ms, LTO 28 ms, PGO 28 ms; sizes 18256, 18528, and 22456
  bytes. The compiler emits one module, so LTO has nothing to merge across objects today; the flags and their
  correctness are what this milestone delivers, and PGO instrumentation is visible (an IR profile with 54 functions
  and 131 blocks on the hello-world fixture).
- **Release size and stripping (#430).** `--strip` passes `-s` on ELF/Mach-O; with MSVC it passes `/OPT:REF` and
  `/OPT:ICF` (cl writes no debug directory without `/DEBUG`). `tests/check_build_flags.py` enforces 128 KiB for a
  stripped hello world and 512 KiB for a std-heavy program on Linux and macOS. Measured on WSL x86-64: hello 27992
  bytes / stripped 22584; std-heavy stripped 26680.
- **`--version` and build metadata (#431).** `crates/burdin_driver/build.rs` embeds the target, profile, and git
  commit (with `BURDIN_COMMIT` as an override), and `burdin_codegen::llvm_version()` reads the linked LLVM version
  through the C API. `--version` prints five lines (`burdin`, `target`, `llvm`, `commit`, `profile`), covered by a
  unit test and by `check_docs.py` and `check_build_flags.py`. Decision: diagnostics do not print the compiler
  version; their format is unchanged, and a bug report should include `burdin --version`.
- **Parallel front-end analysis (#432).** The driver parses modules in `BURDIN_JOBS` worker threads (default: the
  machine's parallelism; 1 is sequential). Each worker keeps its own `Diagnostics`, and the driver merges them in
  file order, so a parallel run is byte-identical to a sequential one. `tests/check_parallel.py` builds a 24-module
  package with each job count and compares the diagnostics of a 12-module failing package between 1 and 8 workers.
  **Checking is still sequential**: the query library exists now (`spec/query.md`), but its memo is used from one
  thread at a time and its stages are not parallel, so parsing is still the phase that scales with `BURDIN_JOBS`.
- **Windows stack-overflow determinism.** The Windows vectored filter ran on the system's default stack guarantee,
  which left only a few kilobytes once the exception dispatcher had run; the handler's flush and write could then
  fault, so the process occasionally ended without `panic: stack overflow`
  (`m02_basics/stack_overflow_aborts` was flaky on the `windows` CI job). `burdin_rt_install_stack_guard` now calls
  `SetThreadStackGuarantee` for 64 KiB before it adds the filter, matching the Unix alternate signal stack; a
  runtime unit test pins the call and the size.

## The forty-seventh session: the query-based compiler library (stage 3, issue #241)

The front end is now a library with a memo, `crates/burdin_query` (`spec/query.md`, QL1-QL9), and the command line
runs on it. `burdin build`, `run`, `test`, and `lint` call `Db::from_workspace` and `Db::analyze` in
`front_end`; the driver keeps only the build phase (scripts, grants), code generation, and the tool commands, and
its `burdin_sema`, `burdin_check`, and `burdin_comptime` dependencies are gone. `package.rs` moved from the driver to
`burdin_query::package` so the library can open a package by itself.

The engine is an in-house memo, not salsa (QL1): `burdin_tir::Program` and the checker's registries are `Rc`/`RefCell`
graphs, salsa's storage wants `Send + Sync` values, and the repository takes no new external dependency without a
decision. Parsing is memoized per file by its text, and a body edit inside one function reparses only that body
(`burdin_syntax::parse_block_range`, `spans::map_spans`). Type checking is memoized per item in
`burdin_sema::CheckCache`, keyed by the item's declaration-span text, a declaration fingerprint (every file's text
with the parser-recorded body spans elided), and the pass chain; a hit replays the body's diagnostics, closures,
instantiation requests, and registry changes with their spans moved to the new text. A declaration edit rechecks
every body, because ids are baked into the typed tree; the memory checks and compile-time evaluation are
whole-program passes on every analysis. Both limits are documented in `spec/query.md` QL2-QL3 and measured by the
benchmark rather than hidden.

What the tests pin: `crates/burdin_query/tests/incremental.rs` (counters: a body edit rechecks one body, a
signature edit rechecks callers, a cached diagnostic follows its function), `reparse.rs` (the memoized tree equals a
full parse after each edit, and the `main`/syntax-error/signature fallbacks), `equivalence.rs` (random edits over
the conformance corpus against a fresh database; the CI release job runs a larger sweep),
`api.rs` (the tool API against `fixtures/api.golden`), `stress.rs` (cancellation, budget eviction, a thousand
keystroke edits), `memory.rs` (a counting allocator over a thousand edits), and `crates/burdin_driver/tests/parity.rs`
(every conformance compile-fail test through the binary and through the library, same file, line, and code). The
benchmark's query cases (`cargo bench -p burdin_bench -- query`) measure a keystroke edit and a cold analysis on a
generated package of one, two, and four modules, and `--smoke` fails unless a keystroke rechecks exactly one body.

What is left from the issue's spirit: checking is still sequential (the memo is used from one thread; the
`BURDIN_JOBS` parallelism covers parsing only), the memory checks and compile-time evaluation are not yet per
function, and the language server came in the forty-ninth session (below).

## The forty-ninth session: the language server (stage 3, issue #242)

`burdin lsp` is the language server (`spec/lsp.md`, LS1-LS20; sub-issues #309-#313, #365-#376, #470, #662), in a new
crate, `crates/burdin_lsp`, that the driver calls. It answers everything from `burdin_query`: one `Db` per package root
(loaded the way `burdin test` loads a package), edits applied to every database that contains the file, a debounced
background analysis that an edit or `$/cancelRequest` stops through the library's `CancelHandle`, and the last
analysis that had a program kept for answers while the file does not parse (positions mapped through the edit). The
query library gained the questions a server needs and the tool API did not answer: `Analysis::occurrences` (every
resolved name), `locals`, `calls` (with the argument written for each parameter), `functions`, `tests`, and
`Db::modules`/`Db::packages` (`navigate.rs`). Name-based features (outline, folding, completion of scope and `use`)
read a per-file index of the syntax tree and tokens (`index.rs`).

What the tests pin: `crates/burdin_lsp/tests/` starts the crate's `burdin-lsp` binary (the server without LLVM) and
speaks JSON-RPC to it, one file per area (lifecycle, diagnostics, navigation, editing, outline, actions, workspace),
including a non-ASCII position in both encodings and a cancellation, with golden files for the capabilities,
diagnostics, symbols, semantic tokens, inlay hints, lenses, and type hierarchy (`BURDIN_LSP_UPDATE=1` rewrites them).
`tests/check_lsp.py` (every CI job) drives the driver's `burdin lsp`, runs a passing and a failing test through the
lenses with the real `burdin test --format json`, and checks the editor files, including that the TextMate grammar's
keywords are exactly the lexer's.

Editors: `editors/vscode` (extension with `vscode-languageclient` pinned to 9.0.1, grammar, language configuration),
`editors/zed` (Zed extension with `zed_extension_api` pinned to 0.7.0; `burdin` from `PATH` or `lsp.burdin.binary.path`;
highlighting from `editors/tree-sitter-burdin`, a lexical tree-sitter grammar Zed fetches at the commit in
`extension.toml`'s `rev`, which must be bumped when the grammar changes), and the Neovim and Helix setup in
`editors/README.md`. Using them in an editor is the manual checklist in `spec/lsp.md` LS20; it was not run in this
session (no editor here), so run it before calling the editor side done.

Follow-ups, none started: `burdin dap` (#576), which the `Debug` lenses already target with a DAP `launch` configuration
(the VS Code extension falls back to building with `-g` and CodeLLDB for a program); bench blocks (#251), which would
get lenses like tests; Helix and Neovim queries for `editors/tree-sitter-burdin`; Zed runnables for `test` items;
publishing the VS Code extension; per-file instead of per-package analysis if large packages make the debounce feel
slow; and the `burdin doc` JSON as a richer hover source.

## The fiftieth session: parallel compilation and `-j` (issues #334, #352, #773)

Profiled first, on a generated 16-module package (about 65,000 LLVM instructions) and on hello world, in WSL with a
clean `PATH` (Windows entries on `PATH` make each `cc` call take half a second there, which is the environment, not
the compiler). Debug build of the package: parse 23 ms, sema 140 ms, memory checks 9 ms, lowering to LLVM 30 ms,
LLVM object emission 300 ms, link 20 ms. Release: LLVM's `default<O2>` 650 ms and emission 450 ms. So the LLVM
backend was 60% of a debug build and 80% of a release build, and it ran on one thread. Hello world spends 90 ms in
sema, almost all of it checking the standard library, and 70 ms emitting a runtime most of which it never calls.

- **Parallel code generation (#334, `burdin_codegen/src/split.rs`).** The typed tree is still lowered to one module
  on one thread. Then `globaldce` drops what nothing reaches (every build; hello world's debug executable went from
  171 KB to 24 KB), and a module of more than `UNIT_INSTRUCTIONS` (8,000) instructions is split into up to 16 codegen
  units the way LLVM's `SplitModule` does it: internal definitions become external, hidden, and renamed
  `burdin.cgu.<name>`; private `unnamed_addr` constants are copied into every unit; each function is owned by one unit
  (contiguous runs in module order, balanced by instruction count); globals, the inline assembly, and appending
  globals belong to the first unit. Each unit reads the whole module from bitcode in its own `Context` and thread,
  turns what it does not own into `available_externally` and runs `elim-avail-extern,globaldce`; in release, functions
  of at most 40 instructions owned elsewhere stay `available_externally` through `default<O2>` so they can still be
  inlined. The objects are linked in unit order. The unit count depends only on the program, never on `-j`, so the
  executable is byte-identical across thread counts. `--lto`, `--pgo-generate`, and `--pgo-use` keep one unit.
  Result: the package's debug build went from 0.63 s to 0.46 s and its release build from 1.40 s to 0.62 s.
  `BURDIN_CODEGEN_UNITS=n` forces a split; the whole conformance suite passes with it set to 8 in both profiles, and
  so do the debug-info, build-flag, and codegen-optimization checks.
- **`-j`/`--jobs` (#352).** Every command that compiles or formats takes `-j <n>` (also `-jN` and `--jobs=n`):
  `build`, `run`, `test`, `check`, `lint`, `doc`, and `fmt`. It bounds the parser threads and the codegen threads;
  for `burdin test` it still also bounds the test processes. The default is `BURDIN_JOBS`, then the machine's
  parallelism; `burdin_query::Options::jobs` carries it into the front end. Parsing now hands files out from a shared
  counter instead of fixed chunks.
- **`burdin fmt -j` (#773).** Files are read and formatted on worker threads; reports, writes, and the exit status
  follow the sorted file order afterwards, so the output does not depend on the thread count. `cargo bench -p
  burdin_bench -- fmt` measures the formatter, and `--smoke` gives it a budget.
- `tests/check_parallel.py` builds the generated package with `-j 1/2/8` in both profiles (identical bytes on Linux),
  runs `m11_llvm`, `m12_closures`, `m15_any`, and `s2_tasks` with `BURDIN_CODEGEN_UNITS=8`, checks `-j` on every
  command, and compares `fmt -j 1` with `fmt -j 8`.

What is left: sema and the memory checks are still sequential. The typed tree uses `Rc` (`Ty` holds `Rc<str>` and
`Rc<FnSig>`), so it is not `Send`; checking function bodies on several threads needs `Arc` there (or interned
names) and a sema whose shared state is read-only while bodies are checked. That is the next big win for small
programs, whose time is mostly sema over the standard library. A `burdin test --workspace` still compiles its
packages one after another. Build timing reports (#472) and cached objects (#243) remain open.

## The fifty-third session: debugger integration (issue #565, sub-issues #567, #569, #571, #572, #574, #576)

`spec/debug.md` (DBG1-DBG12) holds the twelve decisions this session settled; the checks are
`tests/check_debug_info.py` (variable lifetimes), `tests/check_debug_printers.py` (the panic hook, the symbolized
frames, both printer sets, the Natvis file, and `burdin debug`), `tests/check_dap.py`, and
`crates/burdin_dap/tests/protocol.rs`.

- **Symbolized frames (#572).** `burdin_panic_begin` is an empty external function every panic path calls first, and
  a `-g` module carries a frame table plus `burdin_rt_frame_info(pc)`; the backtrace prints
  `  #N  at file:line in path` for program functions, and the non-`-g` path is unchanged. `TraceFrame` carries the
  function, `Display` is `at {func} ({file}:{line}:{col})`, and `Error.print_trace()` prints the frames (the generated
  wrappers call it; a `for` over `self.frames` inside them tripped E0111, so the loop moved into `std`).
- **Variable lifetimes (#571).** `dbg.declare` pins an alloca for the whole function and LLVM then drops every later
  `dbg.value`, so the design is `dbg.value`-only: parameters and captures reopen their slot after the store, moves
  and scope ends kill the location, and the kill list travels from `burdin_check::moves` to codegen in
  `tir::Function::debug_kills`. Conditional moves and early exits keep the location; both gaps are in DBG4.
- **Printers (#567).** gdb's script is embedded in `.debug_gdb_scripts` (type byte `4`, the name and a newline, the
  text, a NUL; there is no version byte, and gdb needs `set auto-load safe-path /`). lldb has no such section, so
  `burdin debug` and `burdin dap` write the formatter file beside the session. Facts that cost time: lldb 21 has no
  Python API for type categories (drive `type category` commands), lldb applies the *last* matching summary (so the
  catch-all enum summary is registered first), an lldb summary cannot decline a type (returning `None` prints
  `None`; returning `""` falls back to the fields), synthetic children shadow the value's own members
  (`GetNonSyntheticValue()`), `SBType.GetFieldsArray` is not a method, and a bare pointer enumerates no children
  (make element values with `SBValue.CreateValueFromAddress`). On the gdb side gdb prints `to_string = {children}`
  for a printer that has both, and a summary needs `display_hint`.
- **`any` and named types.** The vtable now holds the concrete type's name at byte 16 with its length at 24, and the
  concrete type is kept in the debug info by an artificial `any.value` variable (`dbg.value` + `DW_OP_deref` over
  the box), because a type DIE only referenced from a subprogram is still emitted but is not findable by name: named
  aggregate types are now built at compile-unit scope, which is what makes `type lookup Block` work.
- **Windows (#569).** `/DEBUG` and `/NATVIS:<output>.natvis` under `-g` with MSVC, with the Natvis text embedded in
  the driver; natvis has no catch-all, so enums and closures show their raw members (documented in the file).
- **`burdin debug` (#574).** A temp-dir `-g` build and the platform debugger with the right command line per
  debugger; `--test` reuses the whole test build (`burdin build --tests`, new) and resolves names like the runner.
- **`burdin dap` (#576).** `burdin_json` (the JSON value and framing, moved out of `burdin_lsp`, which re-exports
  it) and `burdin_dap`: an lldb-dap child, the four augmentations, and a `Peer` trait so
  `crates/burdin_dap/tests/protocol.rs` drives the adapter against a scripted backend. The VS Code extension
  registers the `burdin` debug type, and the lenses now send `target` (not `program`) in their launch configuration.

What is left: the Natvis file is checked as XML and by the linker arguments, not by a Windows debugger (cdb is not in
CI); the DAP adapter is checked against a stub backend, not against lldb-dap; and `--test` resolution runs
`burdin test --list` before the build, so it compiles twice.

## The fifty-first session: bench blocks and `burdin bench` (stage 3, issue #251)

`bench "name" { ... }` is a module-scope item beside `test` (`spec/bench.md` BN1-BN14, sub-issues #653-#655): a
contextual keyword followed by a string and a block, no attributes, kept out of every build but `burdin bench` the
way tests are kept out of every build but `burdin test`. The body compiles as `__burdin_bench_N` with a unit tail
appended (so `black_box(x)` can end a measurement), and one call of it is one iteration. `black_box(value)`
(`spec/stdlib.md`, "The optimizer barrier") is a compiler-known call of any first-class type: the value is spilled
to a stack slot whose address crosses an empty side-effecting inline-asm call, so the optimizer cannot delete,
forward, or hoist the work; at compile time it is the identity, and a view argument is E0120. The checker, the
memory checks, and the code generator see `tir::Builtin::BlackBox`, which moves its argument and returns it.

`burdin bench` (new `crates/burdin_driver/src/bench_runner.rs`) mirrors the test runner's package selection,
discovery, filters, and isolation, and compiles in the release profile by default (`--release` changes nothing).
Each bench runs in its own process with the harness in `burdin_codegen/src/runtime.rs::define_bench_main`: a
warmup until `--warmup` milliseconds pass, a growing-batch calibration (batches of 1, 4, 16, ... calls until one
takes a millisecond, because timing a single call would be swamped by the clock's ~25 ns granularity), then
`--sample-count` samples of about `--sample-time` milliseconds each, printed on one `burdin-bench-result:` line.
The runner computes the median, the mean, a 95% normal confidence interval, the median absolute deviation, and
the outliers beyond three deviations, and reports in the human format or as a version 1 JSON event stream;
`--list` prints the selection. Benches run one at a time (`-j` bounds compilation only), a panicking bench is a
failure with its captured output, and a hung one is killed at `--timeout` (default 600 s). `--save-baseline`
stores statistics under `<package>/target/burdin/bench/<name>.json` keyed by package-local path; `--baseline`
compares (beyond noise = medians differ by more than three times the larger spread) and `--fail-on-regression`
fails CI on a slower bench that is also beyond noise; `--profile-time <seconds>` runs one bench unmeasured for a
profiler. The syntax and semantics are `spec/bench.md`; `crates/burdin_syntax/src/parser/mod.rs`,
`crates/burdin_driver/src/bench_runner.rs`, and `crates/burdin_driver/src/main.rs` carry the unit tests;
`tests/s3_bench/` has seven conformance tests with two new directives (`bench-compile-fail`, `burdin-bench`), and
`tests/check_bench.py` (every CI job) checks the command black-box.

What is left: the language server has no bench integration (lenses or `--list` consumption; the issue's
follow-up list already names it), there is no way to bench a debug build, and bench bodies cannot take setup or
teardown code beyond what a `let` outside the loop can express.

## The fifty-second session: the REPL and single-file scripts (stage 3, issue #578, sub-issues #580, #582, #584, #586)

`burdin repl` is a new crate, `crates/burdin_repl` (`spec/repl.md`, RP1-RP15), that the driver calls. The session is
one growing program held in memory: `fn main() {` first, the statements of every accepted input inside it, then the
accepted items below — a layout chosen so the interpreter's closure and vtable indices stay stable as items are
added, which lets binding values (closures included) be carried from input to input. Every input is classified by
the parser (module content, statements, incomplete, or bad), checked by the query library in test mode, and
evaluated by `burdin_comptime`, which gained `session.rs`: a public boundary over the typed tree that evaluates a
body's statements from an index with the bindings pre-bound, carries the values out, and (for `:reset`) runs their
drops. Printing in a session is real: the interpreter's output builtins write the process's streams in `Output::Live`
mode, so an expression's value prints through the standard library's own `Debug` code and matches the compiled
program byte for byte. The trigger for printing is elegant and worth remembering: a bare non-unit expression is not
a statement in Burdin, so the checker answers with exactly one E0003 at it, and the REPL turns that answer into the
rewrite `let __repl_v = <expr>` / `println("{(__repl_v):?}")`. An input that fails checking or evaluation is
discarded and the bindings restored (a fault reports E0222/E0224/E0225 and says so). The meta-commands are `:type`
and `:ownership` (throwaway probe analyses over `Analysis::statements_of` and `Analysis::expr_facts`, both new in
`burdin_query`'s tool API), `:explain` (the reference text moved into `burdin_diag` so `burdin explain` and the REPL
share it), `:load`, `:reset`, `:help`, `:quit`, and the driver-provided `:ir` and `:compile`. Interactive mode
(only when stdin is a terminal) is a raw-mode editor with no new dependency — libc termios on Unix, hand-declared
kernel32 calls on Windows — with per-keystroke highlighted redraws using the editor grammar's token classes,
history in the Burdin home, Tab completion through the query library, and multi-line input decided by the parser.
What the tests pin: `tests/check_repl.py` with golden sessions under `tests/repl/` (values, moves across lines, type
errors, items, closures across item inputs, panic/impure rollback, drops on `:reset`, multi-line input, the
meta-commands), plus `:ir`/`:compile`; the crate's unit tests cover the session text, the classifier, the
highlighter, history, and completion. The interactive path was exercised through a pty (RP15 records the checklist)
but not on Windows or macOS consoles — do that before calling the editor finished.

Single-file scripts (`spec/scripts.md`, SC1-SC7, issue #586) reuse the language's own pieces: the lexer skips `#!`
at byte 0 and reports E0300 anywhere else; the loader reads a leading `//!` block's `[dependencies]` fragment into a
hidden in-memory package, so `build`, `check`, and `test` resolve a script's dependencies like any package's; and
`burdin run script.bd` caches the binary by a 128-bit content key (script text, the dependency closure's manifests
and sources, the compiler's identity, and the build flags) under `<burdin home>/cache/scripts/<key>/`, published
with one rename so an interrupted build is never trusted. `tests/s3_scripts/` covers the shebang and the fragment;
`tests/check_scripts.py` proves the cache with a deliberately broken linker (an unchanged script still runs, a
changed one cannot rebuild), and the conformance runner now points `BURDIN_HOME` into each test's scratch
directory so no test writes to the user's home. A plain one-file target (no shebang, no fragment) is exactly what it
was and is never cached.

What is left: the interactive editor on Windows and macOS (untested consoles), completion after `.` for
expressions that are not bare bindings and for `use` paths, re-running `burdin test` over a session (#583's idea
became `:compile`), the REPL's `--history`-style options and prompt customization (deliberately none), and
per-function memory checks and comptime evaluation (the same open item the query library records).

## The fifty-fourth session: Rust interop through C-ABI shims (stage 3, issue #249, sub-issue #477)

`spec/rust-interop.md` (RI1-RI11) is the design; this is what is worth remembering about how it landed. The generator
is a new crate, `crates/burdin_rustgen`, that is pure text-to-text (rustdoc JSON in, Burdin module and shim source
out) and depends only on `burdin_json`; every tool run (`cargo doc`, `cargo rustc`, `rustc -vV`) is in the driver's
`rust_interop.rs`, and the manifest step is `rust_build.rs`. Rustdoc JSON is unstable, so the tool reads exactly
`format_version` 61 and the toolchain is pinned to an exact `x.y.z` (the manifest's `[rust] toolchain`, or the
repository's own pin for the bare command); the JSON is produced on stable with `RUSTC_BOOTSTRAP=1` and
`-Z unstable-options --output-format json`. A type is a **value** only when it is `#[repr(C)]`, all fields are public
and C-compatible, and it has no `Drop`; everything else is an opaque boxed **handle**. Layouts are asserted on both
sides: `comptime assert` in the Burdin module (E0223 when wrong) and `const _` asserts with `offset_of!` in the shim
(rustc E0080, reported as E0264). Panics abort because the shim functions are `extern "C"`. Generation is keyed on
the crate's `.rs`, `Cargo.toml`, and `Cargo.lock` contents plus the compiler fingerprint, profile, and toolchain; the
cargo build always runs (cargo knows when it is fresh), and `rustc --print native-static-libs` supplies the system
libraries, saved in `native-libs.txt` for the runs where cargo prints nothing. `build::run` now returns an `Outcome`
(inputs and link libs), and `emit_program` appends the Rust libraries after `--link-lib`.

What the tests pin: `tests/check_rust_interop.py` (golden module and shim for `tests/rust/geometry`, determinism,
`--rustdoc-json` refusals, a real shim build, both layout checks failing on a corrupted size, usage errors, a fake
`rustc` of another version, and the build-step cache, `--emit-deps`, `--release`, and E0264 on a broken crate);
the `tests/m22_build/rust_*` packages (a program that runs through the shim, `burdin test`, and five failures); the
unit tests of `burdin_rustgen`, `rust_interop.rs`, `rust_build.rs`, and the manifest tables in `burdin_query`.

What is left: the Windows and macOS paths of the build step (system libraries, `crt-static` on MSVC) were written
from the Linux design and never run; a fallible API needs a non-`Result` function in the crate because `Result`,
`Option`, tuples, generics, traits, closures, and `async` are skipped (the mapping of errors is the next design
decision); a crate's own dependencies on other bound crates are not modeled; and two shims in one program each carry
their own Rust runtime.

## Decisions already made (do not reopen without a reason)

- Compiler: Rust, LLVM only (no Cranelift; lowest optimization level for debug). It stays in Rust: no self-hosting, at least
  until the language is mature and its core features are complete (decided in the sixth session, 2026-11-14).
- Platforms: Linux, macOS, Windows; x86-64 and arm64; MSVC toolchain on Windows.
- Memory model: value semantics, single owner, parameter conventions (read-only default, `mut`, `own`), second-class references
  and views, automatic `Drop`, `Heap<T>`, global allocator, abort on panic.
- Exclusivity works on places: field paths of a local or parameter are separate places. Indexing, `Heap` dereference, and method
  calls are not (option C). Recorded in the design document and as decision 16 in `tests/README.md`.
- Stage 0 has no closures, string interpolation, `any`/`impl`, `error` declarations, comptime, tasks, or leak detection.
- Syntax choices: `fn f(): T`, `let` / `let mut`, `Type: Interface { }`, `derives (...)`, `priv` means package-private,
  `?` and `orelse`, `else` arm in `match`, call-site `mut` marker.

## Decided in the second session

- LLVM route 1 (WSL Ubuntu first) and LLVM 21, through `inkwell` 0.11.0. The feature was `llvm21-1-prefer-dynamic`; in
  the fourteenth session it became `llvm21-1-no-llvm-linking` with the link step in `crates/burdin_codegen/build.rs`,
  because `llvm-sys` falls back to the static Windows archives, which crash in a binary built with the dynamic CRT.
- No Rust toolchain pin for now: build with current stable (edition 2024 needs 1.85 or later). *Superseded on
  2026-11-14 by issue #19: `rust-toolchain.toml` pins `1.100.0` with `clippy` and `rustfmt` (see problem 12).*
- `E9000` is a temporary code for "not implemented in this compiler yet". It is not in the registry and no test uses it.

## Known problems and risks

1. **The expected outputs were reviewed on 2026-11-14 (issue #20).** Every test in every milestone folder was read
   against the design, the grammar, the memory rules, the stage 0 subset, and the decision list. No expectation was
   found to encode an accidental behavior, so no test changed; every intentional choice or deviation the suite pins is
   recorded in `tests/README.md` under "Intentional choices and deviations". Issue #21 then added 37 decision tests,
   each written with the rule it witnesses. A new test still needs the same review: a passing test proves the
   implementation matches the expectation, not the design document.
2. **The standard-library names are decided (issue #21, 7a-7d).** `spec/stdlib.md` is the surface, and the tests
   witness it. It is no longer a list of guesses.
3. **The other assumptions are decided (issue #21).** Call-site markers, the `String`-to-`str` conversion, copyable
   views, `for` borrowing, static literals, and the rest are rules in `spec/`, with the tests named there. The
   `tests/README.md` decision list links each one.
4. **Rules I made up while writing the specs** (see the end of the last session's summary): newline continuation only before
   `.`, `?`, `else`, `orelse`, `as`, `&&`, `||`; `own` parameters are immutable bindings; a block's tail-expression temporaries drop
   before its locals; `orelse` binds looser than `||`; comparison operators do not chain.
5. **The driver implements the runner's contract** for `build`, `run`, and `test`.
6. **`m11_llvm/llvm_return_42.bd` stops short** of emitting an object file and linking from Burdin, because the
   target-initialization functions are architecture-specific. The test as written builds, verifies, and prints the IR;
   emitting an object and linking it from Burdin is the next compiler step in stage 1.
7. **Windows and Apple Silicon macOS are covered by CI; Intel macOS and other arm64 hosts are not.** The `windows` job
   exercises the CRT paths: binary-safe file I/O (no newline translation), `_write` and `_setmode` versus `write`, abort
   exit codes (still only "non-zero" in `run-abort`), and the test dispatcher's `atoi`/`strlen` path. The `macos` job
   covers the arm64 triple, `cc` linking, libc calls, the test dispatcher, and `--link-lib LLVM-C`. Intel macOS is not
   tested. `docs/platform-support.md` is the full matrix, the per-platform evidence, and the known gaps (issue #457).
8. **The grammar is tried now.** Generic arguments in expressions (`Stack<i32>.new()`, rule 8.2), `use`/`pub use`,
   `extern "C"` blocks and exported functions, `repr(C)`, ranges as values, `Self` in expressions, and `test` items
   are exercised by m06, m07, m08, and m10; `unsafe fn` declarations and unsafe interface methods have their own
   tests (`m08_modules/unsafe_fn*`, `m07_generics/unsafe_interface_method`).
9. **The claude.ai copies are historical (issue #44).** The design document there lacks the exclusivity rule, and the
   stage 0 document there is the old export. The repository files are the only maintained copies and say so at their
   tops (`CLAUDE.md`, `AGENTS.md`, `Burdin Language Design.md`, and `spec/stage0-subset.md`). Deleting or archiving the
   claude.ai pages themselves is a manual action for the owner, who has the account access; the repository has no link
   to them, so nothing here depends on those pages.
10. **One ownership gap remains.** Moving a non-`Copy` value through a **runtime** index (`let x = xs[i]` with a
    non-constant `i`) is still E9000: the checker cannot tell which element moves, so it rejects the program rather
    than miscompiling it. Moving a constant array or `List` index, a heap pointee field, out of `match *h`, and a
    guarded moving pattern on a temporary scrutinee are tracked (twenty-third session, `spec/memory-rules.md` MV6 and
    MV8). Use `remove` for a dynamic `List` element until stage 1 extends the path model (`stage1-subset.md`). The two
    runtime-index sites are assigned to Stage 1 in `spec/e9000-audit.md` (issues #84 and #493).
11. **A stored view could outlive its source; that is closed (#49).** RT5 with E0123 rejects a view of a temporary, of
    an inner-block local, or of a dropped `String` in a `let` or assignment, reported at the stored value. The old
    examples (`let r = first_of(Pair { .. })`, `let r = { let p = ...; first_of(p) }`,
    `let t = "x".to_string().as_str()`) are compile-fail tests in `m06_conventions`; the rule is in
    `spec/memory-rules.md` RT5. Match/`if let`/`for` bindings remain exempt because their source lives to the end of
    the statement.
12. **The Rust toolchain is pinned** (issue #19, 2026-11-14) to `1.100.0` with `clippy` and `rustfmt` in
    `rust-toolchain.toml`, so `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are reproducible
    from a clean machine. CI installs that toolchain (`rustup toolchain install` reads the file) and verifies
    `rustc --version` and both components; `tests/check_ci.py` fails if the pin or CI's verification step goes missing.
    Bumping the version means running `cargo fmt --all` after the change.
13. **The module and C bridge still has deliberate edges.** There is no `priv` marker on a module: the rule is
    positional, so a dependency's non-root modules are hidden unless its root re-exports them with `pub use`. There
    are no function-pointer types and no `union` declarations, and a `repr(C)` struct crosses the C ABI only through
    `CPtr<T>` (passing or returning one by value is E0162 because the platform coercions are not implemented). C
    `_Bool` is declared `u8`, since a Burdin `bool` is one bit, and C `char` is `CChar`. The manifest parser is
    deliberately minimal (path dependencies, no versions, no lockfile). A user `extern` whose symbol collides with a
    runtime libc declaration of a different type is a backend error printed without an error code or source span.
14. **`Map` is a hash table now (#82).** Stage 0 shipped a linear scan; the twenty-fourth session replaced the
    implementation with open addressing (linear probing, a 3/4 load factor, backward-shift deletion, and structural
    hashing of keys) without changing the type, its methods, or the unspecified iteration order. Lookup, insert, and
    remove are expected O(1); a key still needs only `==`, not `Hash`. `spec/stdlib.md` states the implementation and
    `m09_stdlib/map_key_types`, `map_remove_and_clear`, and `map_many_keys` witness it.
15. **The error types are real error enums now; milestone 16 closed this.** `std` declares `IoError`,
    `ParseIntError`, and `AssertError` as `error enum`s with payloads and messages (`spec/errors.md`, ERR9 and
    ERR10). `read_to_string`, `write_file`, `process.run`, and the integer parsers return `Result<T, E>` under
    those names; the compiler primitives behind them produce a placeholder error value, and a program calls the
    `std` wrappers, which build the real variant. The assertion errors carry the call text, the values, and the
    location.
16. **Ranges are values now (#53).** `a..b` is a `Range<T>` and `a..=b` a `RangeInclusive<T>`: they are stored,
    passed, put in fields and containers, compared, cloned, and hashed, and `for` iterates a stored range without
    borrowing it. Endpoints are integers of one type; the exclusive/inclusive bounds and the overflow behavior of
    `for` are unchanged. Stage 1's iterator protocol makes ranges implement `Iterable`/`Iterator`; the value types
    themselves no longer change.
17. **The standard library is embedded with `include_str!`.** Changing `std/` requires rebuilding `burdin`; a
    project that declares its own `std` package replaces the embedded one (and then must provide every prelude
    name it uses). The prelude copies public names into every module including dependency packages, so a
    dependency that declares `args` or `read_to_string` keeps its own and the program may see two different
    meanings for the same spelling.
18. **`assert` has two modes, and the test mode is typed.** A normal build uses the panicking `assert`/`assert_eq`
    from `std`. A `burdin test` build replaces those prelude names with `__test_assert`/`__test_assert_eq`, which
    return `Result<(), AssertError>`, so `assert(...)?` reports a failed test and helper functions can propagate it.
    The compiler appends the call's source text and location at the call site, `assert_eq` stores both values
    formatted with `Debug`, and the test binary prints the message to standard error before reporting the failure
    (`spec/errors.md`, ERR9).
19. **`for` now borrows its collection for the whole loop.** A `mut` method on the same collection inside the body
    is E0111, which also rejects useful patterns such as appending while iterating a list; that is deliberate
    until the iteration protocol of stage 1.
20. **A `match` on a Copy `Option` field can crash code generation.** Found while writing the decision tests and not
    caused by them: with `struct D { a: i32, o: Option<i32> }`, `let d = D { a: 1, o: None }` followed by
    `match d.o { Some(v) => ..., None => ... }` prints the arm and then aborts in `free()` (reproduced on Linux
    x86-64, debug profile). Copying the field into a local first (`let o = d.o; match o { ... }`) also crashes, while
    `d.o.is_none()` and a struct whose only field is the `Option` work. No milestone test matches a Copy `Option`
    field, so the suite is green; this needs a codegen investigation before stage 1 relies on it. The same shape
    with `Option<String>` works.
21. **A compiler-provided `Display`/`Debug` implementation cannot be boxed (E0204).** `Display` and `Debug` for the
    primitive types and the built-in containers are answered from the type (`builtin_formattable`) and lowered to
    `std` helpers; there is no method function, so there is no vtable entry to dispatch to. Boxing such a type as
    `any Display`/`any Debug` is therefore E0204, while `any Display` of a user type (an impl block, or
    `derives (Debug)`) works and formats through the vtable. A user type that wraps the primitive restores it. A
    later milestone can generate the thunk once it has a way to lower the compiler's format calls into a function.
22. **Explicitly typed generic struct literals keep their arguments only through inference.** `Pair<i64> { ... }` is
    parsed with the generic arguments, but the parser's struct-literal path drops them (`struct_literal` takes only
    the identifiers), so the instance is inferred from the field values and the annotation rather than from the
    written arguments. `tests/m15_any/downcast_generic.bd` writes a typed local and pins the field (`2 as i64`) to
    avoid it. This is a pre-existing parser gap (the grammar's rule 8.2 says a `{` continues the expression), not a
    milestone 15 rule; fixing it means carrying `Vec<Type>` on `ExprKind::StructLit` and resolving it in
    `fncx::aggregate`.
23. **`any` cannot be matched with a pattern.** A `match` on an `any I` value accepts only `_` and a binding; there is
    no type pattern, because a struct or enum pattern cannot name the boxed type. `is` and `downcast` are the
    supported forms. A future `match` downcast would extend the pattern grammar and is not needed by m15.

24. **User derives are not hygienic, and derive impls are checked per type.** The methods of `derive Name` are
    copied into the impl block of each type that applies it and checked in that type's module, so names in the body
    resolve there (the derive's own module is not consulted). An error in a derive body is reported at the derive's
    lines. A derive on a generic type needs `T: Name` for every type parameter, so a derive whose body formats a
    field with `{}` is only usable where the field types implement `Display` too: write such a derive over a type that
    implements what the body needs, or make the derive's interface require it.
25. **Symbolic array lengths compare by spelling.** While a generic body is checked, `[T; R * C]` and `[T; C * R]`
    are different types (CG5); only the instances, where the numbers are known, are compared by value. A
    `where R > 0`-style predicate and calls inside a const expression do not exist.
26. **Moving a non-`Copy` field out of an untracked temporary frees the whole temporary.** `println(make().name)` or
    `[a, b][0].name`, with a `String` field, double-frees: `place_ptr`'s `_` arm registers the temporary for drop and
    `expr_place` only marks tracked places as moved. It is a pre-existing code generation bug that m21 worked around
    (a reflection loop variable's `name` is folded to a literal, and the test binds `Shape.variants` to a local
    first). Fixing it means marking the moved field of a temporary slot, or dropping only the other fields.
27. **`derives (Debug)` on a type with an array field prints an empty value for a generic type.** A non-generic type
    is E0130, but the derived `debug` of `Gen<T> { data: [T; 3] }` prints `data: ` and nothing else (arrays have no
    `Debug`). It predates m21; `derive_generic` and `const_params` avoid printing arrays.
28. **Moving an element out of a temporary collection double-frees.** `let out = env.args()[2]` (a `List<String>`
    temporary), `let x = make()[0]`, and `let s = [String.from("x")][0]` move the element's struct but the
    temporary's drop glue still frees the element, so the moved value points at freed memory (the failure is silent
    corruption or `free(): double free detected`). The same move from a **named** local is tracked and works: the
    fix is to bind the collection first (`let args = env.args(); let out = args[2]`). This is the same temporary
    bookkeeping gap as problem 26 and was not introduced by m22; the build-script tests and `spec/build-scripts.md`
    use the bound form. Fixing it means registering the temporary as a tracked place (or marking its moved element)
    in the memory checker and code generation.
29. **Leak detection is a `burdin test` feature and does not detect every error.** The checking allocator catches a
    leak and a double free or a pointer it did not hand out; it does not catch a use-after-free, and an allocator
    that reaches the OS heap through its own `extern` calls is invisible to it. `burdin build`/`run` use the plain
    C heap with no tracking (`spec/allocators.md` AL9).
30. **`Shared<T>` is a non-atomic `Rc` in stage 1.** The count is a plain `usize`, there are no weak handles, a
    `Shared` cycle leaks, and the cell always comes from `Global` (no arena-backed `Shared`). Stage 2 makes the
    count atomic, adds `Send`, and revisits sharing across threads (`spec/allocators.md` AL8).
31. **An arena reset cannot be checked.** `Arena` is first-class and `allocate` returns a raw `CPtr`; the compiler
    does not track which buffers a `reset` invalidates. Safe container code keeps the arena alive by owning it, but
    manual allocation and reset are `unsafe` (`spec/allocators.md` AL6).
32. **A temporary used as a `for` iterable is not dropped.** `for x in make_list() { ... }` over a `List<String>`
    result leaks the list and its elements under the checking allocator: the iterable's temporary is not registered
    for the drop at the end of the loop statement. Binding it first (`let items = make_list(); for x in items`) is
    the workaround, and `std/src/process.bd` uses it. The failure is a leak, not corruption.
33. **Generic inference of `T = !` crashes the checker.** `task.spawn(|| panic("x"))` makes `T` the never type and
    `origins.rs` panics. It is not specific to tasks (any generic function whose argument closure ends in a call that
    never returns); write the closure's last expression as a call to a function that returns `()` (the s2 tests do).
34. **Tasks cannot be preempted, and a blocking call blocks its worker.** A task that never yields, waits, or finishes
    keeps its worker, and a file or standard-input read inside a task blocks the thread (`spec/tasks.md` TK1, TK12).
    With `task.set_workers(1)` this can starve every other task.
35. **A lock closure moves what it captures.** `lock`, `read`, and `write` take an `FnOnce`, and an `FnOnce` bound means
    its captures move (CL2). A non-`Copy` local used inside the closure and again outside is a use of a moved value,
    and a scoped task cannot use a borrowed capture inside a lock closure (E0101). The way around is a second `Shared`
    handle moved into the closure (`spec/shared.md` SH8). `FnMut` would borrow instead but could not move a value
    in; revisit if this proves too tight.
36. **`task.scope` starts the worker pool even when no task is spawned**, because waiting for the scope reads the
    scheduler's lock. Harmless, but `set_workers` after a `scope` call panics.

## Decided in the third session

- Diagnostics without a registry code use these for now: `break`/`continue` outside a loop is E0001; assigning to
  something that is not a place is E0005; a constant that depends on itself and a non-constant expression in a `const`
  are E9000. Give them registry codes when the spec settles them. *The self-cycle is E0009 and the non-constant
  expression is `const-expression` in `spec/e9000-audit.md` (issue #84).*

## Decided in the fourth session

- Tuples were implemented with milestone 4, which closed the last milestone 3 gap (`let (p, q) = (1, "two")`). No test
  was changed.
- More diagnostics without a registry code: duplicate definitions (types, variants, fields, methods), a type that
  contains itself by value, and a type alias cycle are E9000; a missing or repeated field in a struct literal, an
  `else` block of `let ... else` that can finish normally, and an irrefutable pattern with `else` are E0003; an unknown
  field or variant is E0002; a refutable `let` or `for` pattern without `else` is E0006; calling a `mut self` method on
  an immutable place is E0113 (the same rule as a `mut` argument). *Issue #84 gave duplicate definitions E0007 and the
  cycles E0009; none of them is E9000 any more.*

## Decided in the fifth session

- `Drop` is declared with the interface-block syntax `Type: Drop { fn drop(mut self) }`; the block is accepted only for
  `Drop` (m07 interfaces are still E9000). The same `Type: Drop` route registers the drop method. Deriving both `Drop`
  and `Copy` is E0130.
- The move checker is a separate pass (`burdin_check/src/moves.rs`), not part of type checking, and it keeps the checker
  local: one function body, signatures only. It produces `Function.moved`, the places that need drop flags, instead of
  rewriting the tree with explicit drops. Drop elaboration is entirely in code generation.
- Code generation uses runtime drop flags, one `i1` stack slot per moved path, rather than tracking initialization
  statically point by point. Moves clear flags; bindings, assignments, and re-initializations set them. This resolves
  `Maybe` states from branches and loops for free and keeps the drop code small.
- A partially moved aggregate is dropped field by field (DR7), except that a type with a user `Drop` cannot be
  partially moved (E0103), so its `Drop::drop` always sees a whole value.
- `Heap<T>` is a raw allocated pointer; `Heap.new` allocates, `*h` moves the pointee out and frees the allocation in a
  move context, and reading or assigning through `*h` does not move. `String` is a three-word owning builtin:
  `String.new()` is empty, `String.from` copies, `push_str` grows, and `clone` copies the buffer.
- `x.clone()` is a built-in deep clone for every type that satisfies the `Clone` requirements (`Copy` types,
  `String`, `Heap`, and aggregates that derive `Clone`), not a user-overridable method.
- The move checker rejects the move forms it cannot follow with E9000 rather than accepting them and misplacing
  drops. A move is tracked only for fields of a local or parameter and for the whole pointee `*h`; moving a field
  through indexing or a `Heap` dereference, moving bindings out of `match *h`, and guarded moving patterns on a
  temporary scrutinee are E9000. The same rule keeps code generation honest: a `match` arm whose guard fails sets the
  drop flags of the paths its pattern moved back to initialized, so the scrutinee is dropped exactly once.
- Temporaries drop in reverse creation order (DR5). `let _ = place` reads the place instead of moving it.

## Decided after the seventh session (structure)

- The compiler is split by stage so each crate has one job and a one-way dependency: `burdin_tir` (the typed tree and
  the type questions every stage asks), `burdin_sema` (checking and monomorphization), `burdin_check` (return origins,
  moves, borrows), `burdin_codegen`. The driver runs `burdin_sema::check` and then `burdin_check::check`; sema returns
  the program even when it reported errors, because the memory checks still run to report every error in a file.
- `burdin_sema` keeps the state of a run in one `Sema` (`session.rs`); the phases are methods in `declare.rs`,
  `imports.rs`, `impls.rs`, `instances.rs`, `resolve.rs`, and `bodies.rs`. `Globals` (`globals.rs`) holds one
  `ModuleInfo` per file with its own namespaces, the package and dependency tables, and extern declarations. One
  function body is checked by `fncx/` (`FnCx`), with one file per theme (expressions, calls, patterns, operators,
  aggregates, control flow, generics).
- `burdin_codegen` is one `Codegen` in `emit/`, with one file per theme (types, scopes, expressions, places, patterns,
  operators, built-ins, glue, panics, strings, structural comparison and hashing).
- Visibility is `pub(crate)` between modules of a crate and `pub(super)` for the methods of a split `impl`. Private
  fields of the central struct (`FnCx`, `Codegen`, `Parser`) are visible to the theme files because they are children
  of the module that defines it.

## Decided in the eighth session

- Every file is a module named by its path inside `src/` (`geometry/shapes.bd` is `geometry.shapes`). There is no
  `mod` declaration and no root module special case inside a package: `use` paths are written from `src/`. The package
  being built has its entry point in `src/main.bd`; a dependency is reached as `use <package>`, which names its
  `src/<package>.bd` or `src/lib.bd`. A single `.bd` target is a one-module package whose module path is empty.
- `burdin.toml` is read by a small purpose-built parser: `[package] name`, `[dependencies] name = { path = ... }`,
  comments, and unknown sections/keys ignored. Dependencies are path-only and loaded recursively; diamonds reuse a
  package, cycles are an error, and two packages may not share a name (qualified names are built from it). Path
  resolution is relative to the manifest. Package and module names must be Burdin identifiers. A package's root
  module is `src/<package>.bd`, with `src/lib.bd` as the conventional fallback.
- Each module owns separate value, type, interface, and module namespaces. Declarations are stored with qualified
  internal names (`math.add`, `lib.Gadget`, `shapes.Stack<i32>`) so LLVM symbols and generic instances are unique;
  diagnostics show those names. `use` copies names into the importing module, and all `use` items apply module-wide
  regardless of position. **Imports are not re-exported**: only items a module declares can be imported from it or
  reached through a qualified path, and imports never affect another module's resolution (there is no `pub use`).
  Types are resolved by the same rule, including type aliases: an alias that names another alias through a module
  path is resolved first, so file order never changes what a program means.
- A module path resolves an imported module alias first, then a module of the same package, then a direct dependency
  of the current package. Imported aliases may be extended with submodule names that live under the aliased module's
  file path; aliasing a package root names that one file, not its siblings.
- `priv` is package-private, not module-private, and there is no syntax for a private module. It is enforced on
  imports, qualified paths to private types and values, field reads and writes, struct-literal construction, enum
  struct-variant fields in patterns, methods, and extern functions. A private item imported with `use` still binds its
  name after the E0150, so later uses do not cascade into E0002. A method's visibility belongs to the package of the
  impl block (or interface default) that defines it, which may differ from the type's package.
- Impl blocks are coherent (E0142): inherent methods and `Drop` may only be added to a type the current package
  declares, and an interface may be implemented for a foreign type only when the current package declares the
  interface (Rust's orphan rule). `impl` blocks on generic types and interfaces may be written through a module path
  (`shapes.Box<T> { ... }`, `Box<T>: api.Describe { ... }`), and bounds may name an imported or qualified interface.
- `extern "C"` functions are the only C ABI declarations. Every parameter is passed by value and must be an integer,
  a float, or a `CPtr<T>` (the return may be unit); `mut`, `own`, `self`, generics, other ABIs, and `main` or
  `burdin_*` symbols are rejected. A symbol is declared once: a second declaration with the same signature reuses it,
  and a different signature is E9000. The first declaration's types define the ABI; code generation reuses a matching
  libc declaration from the runtime (`free`, `malloc`, ...).
- `unsafe` is a depth counter in body checking: an `unsafe` block increments it, and an `unsafe fn` body starts at
  one. Calling an `extern` function, an `unsafe` function (including an instantiated generic one), or an `unsafe`
  interface method with the depth at zero is E0161.
- `CPtr<T>` is a raw `Copy` pointer (`ptr` in LLVM) with no drop; `CPtr.null()` takes its type from the expected type.
  `CString` owns a NUL-terminated `malloc`ed buffer (`ptr` in LLVM): `CString.from(str)` allocates, `.as_ptr()` yields
  `CPtr<CChar>`, and its drop frees the buffer. A `CString` temporary whose `.as_ptr()` is taken lives to the end of
  its statement, like a temporary `String` behind a `str` view. `CChar` is `i8`, `CSize` is `usize`, and `CVoid` is
  the unit type used only behind a pointer.
- Package-loading failures (manifest syntax, missing dependency, invalid module name, cycle) are driver errors printed
  as `error: ...`; source diagnostics keep their registry codes.

## Decided in the seventh session

- Generics are monomorphized in sema by checking the AST again with a type substitution (`FnItem.subst`), not in a MIR.
  Instances are named `Stack<i32>` and `max_of<i32>`; a name only has to be unique.
- Generic bodies are checked against their bounds with `Ty::Param`, and each instantiation is checked again with concrete
  types, so an instantiation can report an error the symbolic check did not (a move error in a generic body). Duplicates
  are merged by code and span.
- The impl block of a generic type names the type's arguments by writing them: `Wrapper<T>: Describe where T: Describe`.
  There is no separate `<T>` list; the arguments must be distinct plain identifiers.
- `Ord` implies `Eq` and `Copy` implies `Clone`; derives are conditional on the field types for generic types.
- Interface method implementations are registered as ordinary methods of the type (one namespace per type). Two
  interfaces with a method of the same name on one type are reported as a duplicate member (E9000).
- E9000 for: generic interfaces, generic methods (method-level `<U>`), generic type aliases, an impl for specific
  type arguments (`Stack<i32> { ... }`), `Drop` or any built-in interface implemented by hand, `where` clauses on
  something other than a type parameter, a user interface implemented for a type that is not a local struct or enum
  (`i32: Shape`), struct-like variants of generic enums built with a literal, and `derives (Default)` on enums.

## Decided in the sixth session

- Calling convention: every parameter except `own` is passed by address; `own` is passed by value. That makes a
  returned view of a read-only, `mut`, or `self` parameter refer to the caller's storage (RT1), and a `mut` or `own`
  parameter still sees the caller's place. `ParamSig` keeps `by_ref` (how the value is passed) separate from
  `exclusive` (whether the call borrows the argument exclusively), so the ABI does not make every call exclusive.
- The borrow checker (`burdin_check/src/borrows.rs`) is statement-granular and holder-based: a loan lives to the last
  top-level statement that uses a local holding the reference, a copy of it, or a view derived from it. There is no
  CFG and no scope tracking; it is conservative in loops (a loan used in a loop body is live for the whole loop). It
  runs after the move checker and reports only E0111.
- A call whose second-class result derives from an argument creates a derived loan (RF5) only when that parameter is
  not `own`. A result derived from an `own` parameter is already E0122 in the callee, so skipping the loan avoids a
  spurious E0111 in callers of code that is already rejected.
- `str.starts_with` and `str.find` are generated runtime functions using `memcmp`. `find` encodes the char and returns
  the byte offset of its first occurrence, or -1, so slicing with the result stays UTF-8 correct.
- `match` on a reference or a slice binds its pattern as views (MV8): nothing is moved, and moving out of borrowed
  content is E0101 from the move checker.
- Assignment to a whole view-typed local (`r = &y`) ends the loans the local used to hold and gives it the new ones,
  the same way a `let` does.

## Decided in the tenth session

- A `test` item is parsed into `File.tests`, separate from `File.items`, and compiled as a private function
  `__burdin_test_N()` that returns `Result<(), AssertError>`: the block's statements followed by `Ok(())`. A normal
  build never declares or checks tests, so stripping them needs no code-generation work and a broken test body cannot
  fail `burdin build`. Test items are only declared for the entry package's modules, not for dependencies.
- Test mode is a flag on `burdin_sema::check`. It makes the prelude replace `assert` and `assert_eq` with
  `std`'s `__test_assert`/`__test_assert_eq`, which return an error instead of panicking (the typed `AssertError`
  since milestone 16); a module's own `assert` keeps its meaning. `main` is not required and `Program.main` is
  `None`.
- The test binary is one executable with a generated dispatcher `main`: it reads the test index from `argv[1]`
  (`atoi`), calls that test, and returns 0 for `Ok` and 1 for `Err`. `burdin test` runs one process per test, so a
  panic abort fails only that test without restarting or skipping lists. This matches the design's observable
  behavior ("marks the running test failed, the others still run") with less machinery. The name/`FuncId` pairs travel
  in `Program.tests`.
- `CStr` is a `std` type alias for `CPtr<CChar>`, not a new view type: `CStr.to_string(ptr)` is the associated
  function `to_string` of a `CPtr<CChar>`, implemented with libc `strlen` plus the existing string allocator. This
  avoids a second implicit conversion while giving the m11 test its name. Issue #83 confirmed the decision and
  wrote it down: the rule is in `spec/stdlib.md` ("`CStr` is an alias, not a view type") and `spec/memory-rules.md`
  SC1, and `m08_modules/cstr_is_an_alias.bd` (run), `cstr_no_implicit_conversion.bd`, and `cstr_is_not_a_str.bd`
  (E0003, E0002) are its witnesses.
- `--link-lib LLVM-C` is resolved by the driver through `llvm-config` because Ubuntu ships the C API inside
  `libLLVM-21.so`, not as `libLLVM-C.so`. The driver adds the libdir and an rpath; other library names still go to the
  linker unchanged. This is the one platform-specific exception in the driver.

## Decided in the eleventh session

- CI has a `macos` job on `macos-latest` (Apple Silicon) next to `linux`. It installs Homebrew's keg-only `llvm@21`,
  exports `LLVM_SYS_211_PREFIX`, and adds `<prefix>/bin` to `PATH` so the driver's `llvm-config` lookup and the
  `--link-lib LLVM-C` resolution work without hardcoded paths. No test assumes x86-64: the whole suite runs on arm64.
- `tests/check_platform.py` is the regression check for the host target. It builds a small program, requires the emitted
  `target triple` to match `uname` and `cc`'s target, and runs the program to show `usize` and `isize` are 64-bit. It
  runs on all three jobs, so a change that hardcodes a triple or narrows the pointer types fails CI on any platform.
- The driver resolves `--link-lib LLVM-C` for both `.so` and `.dylib`, prefers `llvm-config` from
  `LLVM_SYS_211_PREFIX` (then `PATH`), and has unit tests for the resolution order. Homebrew's `libLLVM.dylib` has an
  absolute install name, so no rpath is required; the driver still passes `-Wl,-rpath` as on Linux.

## Decided in the thirteenth session

- Both CI jobs restore the cargo registry and `target/` from `actions/cache` with the key
  `${{ runner.os }}-${{ runner.arch }}-cargo-${{ hashFiles('rust-toolchain.toml', 'Cargo.lock') }}`. There are no
  `restore-keys`: a cache is reused only when the pinned toolchain and `Cargo.lock` both match, which is the guarantee
  issue #46 asks for. The OS and architecture are in the key so the linux and macos jobs never share build artifacts.
- LLVM caching differs by platform because `actions/cache` restores as the runner user. On Linux the installed tree is
  kept in `~/.cache/burdin/llvm-21` and copied into `/usr/lib/llvm-21` with sudo. When a distribution splits libLLVM
  into the multiarch directory, `/usr/lib/llvm-21/lib/libLLVM*.so` points outside the tree, so the resolved real files
  are cached under their absolute paths and restored the same way; caching the tree alone would leave the symlinks
  dangling. A cache hit still installs `libzstd-dev` and `zlib1g-dev`, which are not part of the LLVM tree. On macOS
  the keg is not self-contained, so the cache holds Homebrew's bottle downloads (`~/Library/Caches/Homebrew`) and
  `brew install llvm@21` still runs to resolve the keg's dependencies.
- `tests/check_ci.py` now fails if either job loses a cache: the cargo step must include `~/.cargo/registry` and
  `target/` with the toolchain-and-lockfile key, and an LLVM 21 cache must be present with the platform's path. The
  Linux job must also handle a libLLVM symlink that leaves the tree, and the macOS job must cache its Homebrew
  downloads.

## Decided in the fourteenth session

- CI has a `windows` job on `windows-latest` next to `linux` and `macos`. `ilammy/msvc-dev-cmd` loads the MSVC
  environment, the official `clang+llvm-21.1.8-x86_64-pc-windows-msvc` archive is unpacked into `C:\llvm`, and its
  `bin` goes on `PATH` so `LLVM-C.dll` is found. The job runs every check plus m01-m12 and a `BURDIN_CC=cl` smoke run,
  and caches the cargo registry, `target/`, and the unpacked LLVM archive with the same key shape as the other jobs.
- Git for Windows ships a coreutils `link.exe`, and the runner image made rustc pick it instead of MSVC's linker, so
  the Windows job names MSVC's `link.exe` through `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` instead of relying on
  PATH order.
- LLVM is linked as a shared library on every platform: `inkwell` uses `llvm21-1-no-llvm-linking` and
  `crates/burdin_codegen/build.rs` emits the link. The static LLVM archives from the official Windows release are
  built with the static CRT and crash inside the Rust binary, which uses the dynamic CRT; `llvm-sys` also refuses
  dynamic linking on MSVC, so the script links `LLVM-C.lib` directly.
- Code generation keeps `RelocMode::PIC` on COFF. A static relocation model makes LLVM address string constants with
  `IMAGE_REL_AMD64_ADDR32` relocations, which `link.exe` rejects (`LNK2017`).
- The driver maps `--link-lib LLVM-C` to `LLVM-C.lib` plus `/LIBPATH` from `LLVM_SYS_211_PREFIX`, and a missing `cl`
  names both the Developer Command Prompt and `BURDIN_CC`.

## Decided in the fifteenth session

- Every CI job runs the finished conformance milestones twice: the debug profile, where overflow panics, and the
  release profile, where it wraps (`tests/run_tests.py --release`). The release run skips tests marked
  `//@ debug-only`; the m02 overflow test is the one such test. `tests/check_ci.py` requires a release step covering
  every milestone folder in every job, the runner's `--release` handling, and at least one `//@ debug-only` test so
  the skip path stays exercised (issue #47).
- The benchmark baseline lives in the CI cache, not in the repository (issue #446). Numbers from a development
  machine and from a hosted runner are not comparable, and committing one machine's numbers would make every CI run
  report a false regression. The cache key includes the runner OS and architecture, `rust-toolchain.toml`, and
  `Cargo.lock`, so a run is only compared with a run of the same kind, and a missing or expired cache records a fresh
  baseline instead of failing. The trade-off is that the baseline cannot be reviewed in a diff (the run that records
  it is in its log) and a cache evicted after seven days without a run restarts the trend.
- The regression comparison is advisory (`continue-on-error: true`), as issue #446 asks until the numbers are stable.
  The harness keeps the exit status (1 over the threshold, 2 on an error), so tightening the policy is a workflow
  change, not a code change.
- The default threshold is 25% and `--threshold` overrides it. Hosted runners are noisy enough that a tighter default
  would flag noise; the baseline format and the comparison live in `burdin_bench`'s library, so the arithmetic is
  unit-tested without a backend or a runner.
- A push to main records the new baseline even when that run flagged a regression, so the baseline follows main and a
  persistent regression does not keep firing on every pull request while the numbers are still settling.
- `tests/check_ci.py` guards the tracking the same way it guards the other CI invariants: every job must restore and
  save the baseline cache with the runner-and-lockfile key, run `tests/bench_regression.py`, keep the step advisory,
  and record the baseline only on main.

## Decided in the sixteenth session

- **Issue #21 is settled.** The 16 assumptions in `tests/README.md` are now decisions, one sub-issue each (#22-#40),
  stated in the spec and witnessed by tests. The list below is the short form; the rules are in `spec/grammar.md`,
  `spec/memory-rules.md`, `spec/stdlib.md`, `spec/derives.md`, and `spec/stage0-subset.md`, and `tests/README.md`
  links each decision to its section.
- **Call sites (#22).** A named `mut` parameter takes `mut` at every call site (E0114 missing, E0115 on a parameter
  that is not `mut`); `own` and read-only parameters take no marker; a `mut self` receiver takes no marker, and a
  marker in receiver position is a syntax error.
- **One implicit conversion (#23).** `String` to `str`, as a borrow, only in argument-like positions (call
  arguments, `Some`/`Ok`/`Err`, `assert_eq`, `mem.replace`). `let`, assignment, returns, array/tuple elements, and
  operators do not convert; there are no reference or user-defined conversions.
- **Views are Copy (#24).** `&T`, `str`, `[T]`, and `CPtr<T>`; `&mut T` moves on assignment and reborrows when passed
  to `&mut` or dereferenced.
- **Static literals (#25).** A string literal is static in any returned expression, and a `str` constant of a literal
  is static too; no other expression is.
- **Pattern bindings and `*h` (#26).** Bindings from a borrowed scrutinee are read-only places usable like values
  (E0005 assignment, E0101 move-out); `*h` moves the whole pointee and frees it. Moving a field through `*h` stays an
  E9000 stage 1 gap; move the pointee into a local first.
- **`for` and ranges (#27).** `for` borrows its collection shared for the whole loop (E0111 on mutation), the binding
  is read-only (E0005), maps iterate `(k, v)`, `chars()` iterates `char`, and `a..b`/`a..=b` are iterable. (Issue #53
  later made ranges ordinary values; the loop forms kept this behavior.)
- **Standard-library surface (#28-#31).** Written with signatures in `spec/stdlib.md`, including `unreachable`,
  `String`/`str` ordering, `CPtr.null()` taking no arguments, and the C bridge names.
- **`main` (#32).** Unit, no parameters, E0003 otherwise. Decided for stage 1 (m16): a returned error is printed with
  its context chain to stderr and the process exits non-zero.
- **Panics (#33).** `<file>:<line>:<column>: panic: <message>` on stderr, stdout flushed first, non-zero exit, no
  drops.
- **File I/O (#34).** Byte-for-byte, no newline translation on Windows, no UTF-8 normalization.
- **`Drop` (#35).** `Type: Drop { fn drop(mut self) { ... } }` only; `mut self` and a unit return are required
  (E0003), and `Drop` is not derivable (E0002) and not combinable with `Copy` (E0130).
- **Literals and division (#36).** No suffixes (E0001); context, then the other operand, then `i32`/`f64`; `/` and
  `%` truncate toward zero.
- **Declaration layout (#37).** The parser accepts commas or line breaks in brace lists; the tests and style use one
  field/variant per line; struct literals use commas; items need no separators. (This keeps the existing
  `Shape.Rect { w, h }` declaration in `m04_data` valid.)
- **Temporary drops (#38).** End of statement, reverse creation order, field reads do not extend, block
  tail-expression temporaries before the block's locals.
- **Derives (#39).** Exact `Eq`/`Ord` field and variant orders, `Hash` as FNV-1a over scalar values with the offset
  basis and prime recorded, `Default` for structs, and worked hash values pinned in `m07_generics/derive_values.bd`.
- **Exclusivity places (#40).** Confirmed as the design states: field paths only, `xs[0].a` and `xs[1].a` one place
  (E0110), different nested fields separate.
- **Four implementation fixes came with the decisions.** `check_drop_signatures` now requires an exclusive `mut self`
  receiver (`crates/burdin_sema/src/session.rs`); `CPtr.null` rejects arguments with E0004
  (`crates/burdin_sema/src/fncx/call.rs`); the ordering operators handle `str`/`String` through `cmp_at` instead of
  the integer path (`crates/burdin_codegen/src/emit/ops.rs`), which previously crashed code generation; and an
  invalid `extern "C"` type or convention is E0162 (`crates/burdin_sema/src/resolve.rs`).
- **New tests.** 40 conformance tests cover the decisions; `tests/README.md` has the per-decision table. The suite is
  174 tests, 173 in release.

## Decided in the seventeenth and eighteenth sessions

- **Test attributes are one bracket list after the name.** `test "name" [ignore, timeout(500), should_panic("boom")]`
  fits the grammar without a new token, and the names stay contextual. The accepted set is `ignore`, `timeout(ms)`,
  `should_panic[(message)]`, and `should_fail`; anything else in the list is E0170, and so is a repeat, a value an
  attribute does not take, or `should_panic` with `should_fail`. The grammar is in `spec/grammar.md`; the behavior
  in `spec/test-runner.md`; the tests are `m10_test_runner/attributes_*.bd`.
- **`should_panic` and `should_fail` are separate endings.** The dispatcher returns 0 for `Ok` and 1 for `Err`, so a
  returned error is exit 1, a panic is any other abnormal ending, and `should_fail` accepts only exit 1. A test that
  calls `process.exit(n)` with another value is indistinguishable from an abort; that is documented rather than
  given a new protocol. At milestone 10 an expected error *value* waited for milestone 16: `AssertError` was a unit
  alias, so `should_fail` took no payload. Milestone 16 made the failure typed (`spec/errors.md`, ERR9), while
  `should_fail` still checks only the exit status.
- **Tests run in one process each, concurrently.** `-j`/`--jobs` defaults to the CPU count, `-j 1` is sequential,
  and the report is collected and printed in declaration order. One test binary is compiled per selected package,
  so a dependency's tests are checked against their own package.
- **Isolation is a fresh directory and a clean environment.** Each test runs in an empty temporary directory that is
  removed afterwards; the environment holds only `PATH`, `TMPDIR`/`TMP`/`TEMP` (the test's directory),
  `BURDIN_TEST`, and the Windows CRT variables. `argv` stays reserved for the dispatcher index, so a test must not
  rely on `env.args()`.
- **`--workspace` means the build graph.** Stage 1 has no manifest-level workspace (multiple-package workspaces are
  stage 3), so `--workspace` selects the entry package and every path dependency; `-p` selects one package of that
  graph, and dependency tests never run by default. Names are shown as `<package>::<test>` when more than one package
  is selected.
- **Default timeout is 60 seconds.** `--timeout <ms>` overrides it (0 disables it) and a `[timeout(ms)]` attribute
  overrides that per test. A timeout kills the process and is a failure; `should_panic` does not accept a timeout.
- **The runner has a black-box check.** `tests/check_test_runner.py` drives fixture packages in `tests/runner/`
  through every mode, checks the deterministic order and the cleanup, and every CI job runs it. The fixtures carry no
  `//@` directives, so the conformance runner ignores them.
- **Every registry code needs a test.** `tests/check_tests.py` now fails on a registered code with no `//~`
  marker; a code that cannot have a compile-fail test is listed with its reason in `tests/README.md`
  (issue #445). The list is empty today.
- **Differential tests are implemented (issue #444).** A `[differential]` body runs at compile time through the
  interpreter and as the compiled test; the runner compares the `Ok`/`Err` outcomes and reports a divergence with
  both. The compile-time run follows the ordinary CT9-CT11 rules, so an impure or panicking body is a compile error
  (E0224/E0222), and `differential` cannot combine with `should_panic` or `should_fail` (E0170). The check is
  `tests/check_test_runner.py` with `tests/runner/differential/`, whose divergent case uses the unspecified `Map`
  iteration order, plus the `attributes_differential.bd`, `differential_invalid.bd`, and `differential_impure.bd`
  conformance tests. The `interp_*` tests in `tests/m20_comptime/` remain the per-construct comparisons.
- **Diagnostics are sorted once, never streamed** (issue #436). The semantic and memory checks already collect the
  whole list before the driver prints it, so the renderer sorts by file (load order), byte offset, code, message,
  and discovery order. That makes equal spans stable and the output byte-for-byte deterministic. The "report every
  error in a file" rule therefore holds by default; `--error-limit N` is opt-in, applies to the sorted list, and
  only hides output (the summary counts every error). `0` means no limit.
- **Three formats** (issues #434, #435). `human` is the default and is the only one that colors or wraps; `short`
  is exactly one plain `path:LINE:COL: error[E0000]: message` line per diagnostic, which is what the test scripts
  parse; `json` is one object per diagnostic plus a final summary object, with `span` (file, line, column, end,
  byte offsets), `notes`, and `suggestions` (message, span, replacement). An insert is a span with equal ends, and
  an empty replacement deletes. The JSON is the interface an editor uses for diagnostics and quick-fix code
  actions; there is no LSP yet.
- **Color precedence**: an explicit `--color always`/`never` (or `--no-color` for `never`) wins over a non-empty
  `NO_COLOR`; `auto` is the default and requires stderr to be a terminal and `NO_COLOR` to be absent. The last
  color option on the command line wins. The decision is a pure function unit-tested in the driver.
- **Width precedence**: `--width`, then a positive `COLUMNS`, then 80; `--width 0` disables wrapping. Wrapping
  only inserts breaks and a four-space hanging indent and never changes the message characters, so a hard-split
  word can be reconstructed by removing the breaks.
- **Notes and suggestions are part of the diagnostic model, not of rendering.** `Diagnostic` carries
  `notes: Vec<Note>` (optional span) and `suggestions: Vec<Suggestion>` (span + replacement). E0100 now notes where
  the value was moved from, E0110 notes the conflicting argument, and E0114/E0115 attach the `mut` insertion or
  deletion (the parser records the marker's span in `ast::Arg.mut_span`). The human renderer prints `= note:` and
  `= help:`; `short` omits both.
- **The error-code reference is a checked page, embedded in the driver** (issue #437). `docs/error-codes.md` has one
  section per registry code with a complete example and a fix. The markers `<!-- error-example: E0000 -->` delimit
  each snippet, which `tests/check_error_docs.py` extracts, compiles, and requires to fail with exactly the marked
  `//~` diagnostics (a multi-file snippet uses `// --- path ---` lines and `target=`). The driver embeds the page
  with `include_str!`, so `burdin explain <code>` and the page cannot drift; a unit test compares the page headings
  with the registry in `tests/README.md`.
- **CI runs two new checks in every job**: `tests/check_diagnostics.py` (formats, color, width, notes/suggestions,
  limit, determinism, flag syntax) and `tests/check_error_docs.py` (registry coverage, every example compiled,
  `burdin explain`). `tests/check_ci.py` fails if either step, either script, or the reference page goes missing,
  and checks that the page's code set equals the registry without compiling anything.
- **No new diagnostic codes.** The registry is unchanged; #433 is presentation and tooling.
- **The fuzzer is dependency-free and deterministic (issues #288, #447).** `crates/burdin_fuzz` has its own
  SplitMix64 generator and mutation operators rather than `cargo-fuzz`/libFuzzer, so it builds on the pinned stable
  toolchain, adds no dependency to `Cargo.lock`, and needs no backend or LLVM. A run is a function of the corpus,
  the seed, and the iteration count: the same pair reproduces every input on every platform. The fuzzer parses
  source text (`&str`), because that is what the driver's `read_to_string` hands the lexer; invalid UTF-8 never
  reaches the parser.
- **The corpus is committed, and the fast replay is the regression check.** `crates/burdin_fuzz/corpus/syntax/` holds
  hand-written edge cases (truncated constructs, unbalanced brackets, `>>` splitting, struct-literal lookahead,
  recovery points, unicode) plus every input that found a panic. `tests/corpus.rs` replays it and then runs 20,000
  deterministic mutations from a fixed seed inside `cargo test --workspace`, so every CI job fails when a committed
  input panics again. The run takes about a second; the unit tests also prove that a deliberate panic is reported and
  a saved input is named by its content hash.
- **The long session is scheduled.** `burdin-fuzz-syntax` replays the corpus, then mutates it for a time budget,
  writes each failing input to `fuzz-out/crashes/`, and exits 1. A watchdog fails the run when no input finishes for
  ten seconds and saves the input that stalled, so a recovery loop that stops consuming tokens is caught rather than
  hanging the job. The `fuzz` job runs `tests/fuzz.py --seconds 600` on the weekly schedule
  (`cron: '17 4 * * 1'`) and by hand, with the run id as the generator seed so every run explores a different
  sequence, and uploads the directory as the `fuzz-crashes` artifact. It builds only the fuzz crate and its
  front-end dependencies, so it needs no LLVM. The script also seeds the session with the conformance tests and
  `std`, which are structurally richer than the small edge cases.
- **A found input is committed with its fix.** The scheduled job cannot commit by itself; the artifact holds the
  input, and the fix commits it under `corpus/syntax/`, where the replay keeps the panic from coming back. The
  corpus README states that rule. `tests/check_ci.py` fails if the schedule trigger, the gated `fuzz` job, its
  `cargo test -p burdin_fuzz` replay, `tests/fuzz.py`, the artifact upload, or the corpus itself goes missing.
- **Fuzzing found no panic in the current front end.** More than 20 million mutations over several seeds (corpus,
  conformance tests, and `std` as seeds) and the deliberately broken-parser fixture used to prove the harness reports
  findings both behaved as intended; no input is committed yet, so the corpus is the hand-written seed set. A
  4,000-character input of nested delimiters, the mutation cap, parses on both the main and a test thread's stack.

## Decided in the nineteenth session

- **Debug information is opt-in (`-g`).** It is not tied to the profile: a debug build without `-g` stays small, and
  `--release -g` works when a debugger is needed for an optimized binary. The metadata is emitted in both cases, and
  the type model deliberately leaves payload enums opaque rather than guessing at their union layout.
- **Backtraces are always on.** A panic ends the process, and the trace is the diagnostic that makes the abort
  useful; there is no environment switch. The first line keeps the stage 0 format exactly. Runtime frames are hidden
  by skipping the two frames between the panic function and its caller; `notail` keeps the panicking function's own
  frame. Windows prints addresses because symbolization needs a PDB, which the driver does not produce.
- **Standard output is block buffered (8 KiB), line buffered on a terminal, and stderr is unbuffered.** `atexit`
  covers normal returns and `process.exit`; the panic path and the stack-overflow handler flush explicitly. The
  stage 0 stdlib has no explicit output handle, so there is nothing user-visible to flush yet; a stage 1 handle
  would call the same `burdin_rt_flush`.
- **The stack guard distinguishes faults by address.** Unix records the main stack's bounds at startup and only
  reports a stack overflow when the fault lies in the 1 MiB below the stack; other faults report
  `panic: invalid memory access`. Both paths flush and exit 134. The Windows filter checks the exception code.
- **Glue is shared by structural shape, and LLVM's `MergeFunctions` is not used.** Two types with the same field
  types generate one drop/clone glue; a user `Drop` method keeps its own type's glue. MergeFunctions cannot merge the
  externally visible Burdin functions (their symbols are needed for backtraces), and the remaining duplicates are
  already shared, so enabling it changed nothing measurable.
- **Drop-flag elimination is limited to whole locals the move checker proves dead at every exit.** Partial moves and
  conditional moves keep their runtime flags, which is what makes the analysis small and auditable. Adjacent drops
  are not merged by hand: eliminating a statically dead drop removes it entirely, which is stronger than merging its
  check, and release `-O2` merges the remaining branch structure. Hand-merging would complicate DR7's field-by-field
  partial drops for little gain.
- **Bounds checks are elided only for an in-range constant index and for an index from `for i in 0..xs.len()` over
  the same collection.** Every other dynamic index keeps the panic.
- **LTO and PGO are opt-in and need an LLVM compiler driver and tools.** `--lto` emits bitcode and links with
  `-flto`; `--pgo-generate` instruments with `pgo-instr-gen,instrprof`; `--pgo-use` runs the profile pipeline through
  `opt` and `llc` because the LLVM C API does not expose the pass builder's profile options. MSVC `cl` rejects LTO
  and instrumentation, and `--lto` and PGO cannot be combined. Release does not turn them on by default: the
  compiler emits one module, so LTO currently has nothing to combine across objects, and the measured fib benchmark
  showed no gain (26/28/28 ms, 18256/18528/22456 bytes).
- **`--strip` uses the platform's linker flag and has a size budget.** `-s` on ELF/Mach-O; `/OPT:REF`/`/OPT:ICF`
  with MSVC. CI enforces 128 KiB for a stripped hello-world and 512 KiB for a stripped std-heavy program on Linux and
  macOS. The budget is deliberately generous (the measured sizes are 22584 and 26680 bytes) so it catches an
  accidental runtime or library growth, not compiler noise.
- **`--version` prints stable lines, and diagnostics do not carry the version.** Keeping every diagnostic line
  in the registry format matters more than embedding a version; a bug report includes `burdin --version` (documented
  in README.md). The twenty-second session added a `channel:` line, so a release build now prints six.
- **Parsing and code generation are parallel by default; checking is not.** `-j` (default `BURDIN_JOBS`, then the
  CPU count) controls the workers (1 is sequential), and the codegen unit count never depends on it. Each
  worker keeps its own diagnostics and the driver merges them in file order, so output is byte-identical. The query
  library (forty-seventh session) made checking incremental but not parallel: `Sema` is still one shared mutable state,
  and the memo is not shared between threads.

## Decided in the twentieth session

- **The dependency audit is a checked inventory, not a third-party scanner (issue #449).** `tests/dependencies.txt`
  lists every external crate with its exact version, license, and role, and `tests/check_deps.py` compares it with
  `Cargo.lock` and the manifests. It is offline and deterministic, so it runs in every job without network or an
  unpinned tool. A vulnerability scan (`cargo audit` and the RustSec advisory database) is a manual extra, not a CI
  dependency; `docs/dependencies.md` says so.
- **External requirements are exact `=x.y.z` pins, and `Cargo.lock` is committed.** The lockfile records the
  crates.io source and checksum of every crate, so a changed dependency cannot slip in with a matching inventory
  line; CI's `cargo metadata --locked --offline` also proves the lockfile is current with the manifests.
- **The policy is permissive-only, and every identifier in an expression must be allowed.** An `OR` with one
  copyleft branch is rejected rather than accepted through the permissive side. The accepted set is stated in
  `tests/dependencies.txt` and enforced by the script; only `Apache-2.0 WITH LLVM-exception` is accepted as a
  `WITH` term (for LLVM itself, which is host-provided and not vendored).
- **The crates' own metadata is the source of truth in CI.** After `cargo build`, every job runs the audit with
  `BURDIN_DEPS_REQUIRE_METADATA=1`, so each license is re-read from the crate's metadata and a failure to do so is
  an error. Locally, before a build populates the registry cache, the same failure is a note, so the offline checks
  still run on an unpacked tree.

## Decided in the twenty-second session

- **SemVer, with editions versioned separately (issue #456).** The compiler version is the workspace version in
  `Cargo.toml`, a stable release is a `vMAJOR.MINOR.PATCH` tag that must match it, and the workspace is `0.1.0` for
  the first published release. Before 1.0 a breaking change bumps `MINOR`; from 1.0 it bumps `MAJOR`. Language
  editions are named by year (`2024`) and versioned independently, so upgrading the compiler does not silently change
  what an existing program means; the manifest field that selects an edition arrives with the stage 3 manifest work,
  so every package uses `2024` today. The rules and the rehearsal are in `docs/releasing.md`.
- **Three channels, and publishing is opt-in (issues #454, #456).** `dev` is a local build and is never published;
  `nightly` is `<manifest>-nightly.YYYYMMDD` and is built by the weekly schedule (artifacts only) or by a manual run;
  `stable` is a tag and is published. A manual run defaults to `publish: false`, and a plain `workflow_dispatch` can
  therefore rehearse the whole pipeline without creating a release. The tag path publishes, and the publish step is
  the only `gh release` call.
- **Native runners, one per target (issue #453).** The six targets are built on `ubuntu-24.04` and
  `ubuntu-24.04-arm`, `macos-15-intel` and `macos-15`, and `windows-2025` and `windows-11-arm`; there is no
  cross-compilation, so every job can run the compiler it just built. `tests/build_release.py` refuses a `--target`
  that is not the host in build mode, so a mislabelled runner fails instead of producing a mislabelled archive.
- **Archives are uniform `.tar.gz` with a single named root (issue #453).** `burdin-<version>-<target>.tar.gz`
  contains the compiler, `README.md`, `CHANGELOG.md`, and `docs/error-codes.md`, on every platform, so packaging
  and the installers have one shape. The compiler still needs the LLVM 21 shared library at run time; the archive
  does not bundle it, and the installers warn with the platform's fix.
- **Checksums are per-target files aggregated into one `SHA256SUMS` (issue #455).** Each build job writes
  `<archive>.sha256` and `SHA256SUMS.<target>`; the release job concatenates and verifies them with `sha256sum -c`
  and requires all six archives before publishing. Both installers verify before extracting; a tampered archive, a
  missing entry, or an unsupported target is refused, which `tests/check_release.py` proves against a local release.
- **Reproducibility is a build property plus a normalized archive (issue #455).** Builds set
  `CARGO_INCREMENTAL=0`, remap the checkout and target paths to `/burdin`, and pin `SOURCE_DATE_EPOCH` to the commit
  time; archives sort entries, pin mtimes, zero uid/gid, and use `gzip -n`. `--verify-reproducible` builds two
  copies at different paths and requires the same archive SHA-256; the release workflow runs that on Linux. Byte
  equality across different LLVM builds or linkers is explicitly not claimed.
- **Release notes come from the changelog (issue #456).** `tests/release_notes.py --version X.Y.Z` requires a
  `## X.Y.Z — <date>` section and checks every relative link; `--unreleased` is the dry-run form. The release job
  fails on a tag without a section, so the changelog has to be updated before tagging.
- **`burdin --version` reports the channel.** The output is `burdin <version>`, `channel:`, `target:`, `llvm:`,
  `commit:`, and `profile:`. `BURDIN_VERSION` and `BURDIN_CHANNEL` are compiled in by `crates/burdin_driver/build.rs`,
  which rejects a malformed version and a channel other than `dev`, `nightly`, or `stable`; the release workflow
  passes both, and a local build defaults to the manifest version and `dev`.
- **The release rehearsal is automated where it can be.** `tests/check_release.py` runs in every CI job and checks
  the workflow, the six targets, the publish gate, packaging determinism, checksums, both installers, the notes
  generator, and the version metadata; the release job installs from its own bundle with `install.sh` and runs the
  installed compiler. The manual checklist in `docs/releasing.md` covers the parts CI cannot (the first real tag,
  a clean machine, and the external download URLs).

## Decided in the twenty-third session

- **A stored view's source rule is RT5 (E0123), checked in `origins.rs`.** A second-class value stored in a `let`, an
  assignment, or a `let` pattern binding may derive only from static data, a parameter, or a local declared in the
  same or an enclosing block. A temporary, or a local of an inner block, is E0123 at the stored value. The rules for
  `match`, `if let`, and `for` bindings are exempt: their source lives to the end of the same statement and the
  binding does not. Origin is now a set of parameters, locals, and a "temporary" flag, with the block ancestry
  tracked, and the check stays body-local.
- **Ranges are ordinary `Copy` values with integer endpoints.** `a..b` and `a..=b` produce `Range<T>` /
  `RangeInclusive<T>`, where both endpoints have the same integer type (inferred from context or the other
  endpoint, as before). A range is evaluated once and `for i in r` does not borrow it; equality, ordering, hashing,
  cloning, and dropping are the structural versions of the two fields. `..` is the lowest-precedence operator, so a
  range used as an operand is written in parentheses (`(4..=6) == r`); open-ended forms (`..b`, `a..`, `..`) do not
  exist.
- **The move checker tracks constant indices and heap fields, not runtime indices.** A constant array or `List`
  index (`xs[0]`) is a path step, and a field of a heap pointee (`(*h).a`) extends the path through the `MOVED_DEREF`
  step. A moving pattern on a temporary `match` scrutinee is safe: the scrutinee is moved into a compiler-made local
  of the statement, so untouched fields drop once and a failed guard restores what an earlier arm bound. An index
  expression is evaluated once. A runtime index remains E9000: without knowing the element, the drop flag cannot be
  placed, and miscompiling is worse.
- **Generic gaps use the existing rules.** A static method of a generic type unifies `Self`'s arguments with the
  method's parameters (so `Holder.make(x)` infers `T`); a generic type's bounds are checked when it is instantiated
  (E0141), not only in the symbolic check; a view type argument makes the instance second-class (SC3), so it is
  allowed as a local but rejected in a field; an impl method is compared with the interface method, including
  `unsafe` (E0003); `==`/`<` on symbolic instances are accepted when the bounds provide `Eq`/`Ord`; and `Self`
  resolves in expressions, struct literals, patterns, and type paths of an impl block.
- **`pub use` re-exports, and module visibility is positional.** `pub use` copies a module or item into the
  re-exporting module's exported names; import resolution iterates to a fixpoint so a re-export may appear later in
  module order. A dependency's root module (`src/<package>.bd` or `src/lib.bd`) is what `use <package>` names; its
  non-root modules are hidden unless the root re-exports them (E0002 when reached directly). `pub use` of a
  package-private item and a public signature or field naming a package-private type are E0150. There is no `priv`
  marker on a module because a file has no item position to carry it.
- **Every `CPtr` value requires `unsafe`.** Creating (`CPtr.null()`), holding, passing, moving, dereferencing,
  testing, comparing, offsetting, or casting a raw pointer is E0161 outside `unsafe`/`unsafe fn`. This closes the
  design's rule that raw pointers are only usable in unsafe code; the memory rules still apply inside. A reference
  converted to a `CPtr` (`&mut x as CPtr<T>`) ends its borrow at the conversion, because the pointer leaves the safe
  domain; that is what makes out-parameters work without a spurious E0111.
- **C aggregates cross the ABI through `CPtr<T>` only.** `struct Name repr(C)` fixes field order, padding, and
  alignment (`Type.size()`/`Type.align()` report the LLVM ABI layout), and a pointer to it is an ordinary
  `CPtr<Struct>`. Passing or returning an aggregate by value is E0162: the SysV/MSVC coercions (packing a small
  struct into a register, `sret`) do not match LLVM's lowering of a literal struct return, and a silent ABI
  mismatch is worse than a rejection. C `_Bool` is a one-byte integer, so it is declared `u8` (0 or 1); C `char` is
  `CChar` (`i8`).
- **`extern "C" fn` exports a Burdin function under its plain symbol.** The declaration rules apply (ABI "C", `own`
  parameters, no generics/`...`/receiver), and the C symbol is reused if an `extern "C"` declaration of the same
  type exists, so a caller in another module or language links to the same function. Variadic declarations take
  `...`; each extra argument is checked against the C ABI.
- **The coverage issues are verification work.** `unsafe fn` and unsafe interface methods have run and compile-fail
  tests (#85, #86), and the memory-rule matrix was audited with a script over every referenced test (#87); `US` now
  names the pointer and unsafe-function tests.

## Decided in the twenty-fifth session

- **Closures are boxed capture records with interface types.** A closure expression has one of the compiler-known
  types `Fn(A, B): R`, `FnMut(...)`, or `FnOnce(...)`, and a value is `{ code, env, drop }`: a code pointer, a
  heap-allocated capture record, and the record's destructor. This makes stored, returned, interface-typed, and
  container-held closures uniform at the cost of one allocation; an inline anonymous type was rejected because a
  struct field or return type cannot name it before milestone 15's `impl`/`any`.
- **`Fn`/`FnMut`/`FnOnce` are contextual type names and bounds.** `Fn(i32): i32` is a type; `F: Fn(i32): i32` is a
  generic bound resolved into `Bound::Fn(signature, capability)` and monomorphized per closure type. Capabilities
  order `Fn` > `FnMut` > `FnOnce`: a value may be used at a lower capability, never a higher one (E0141).
- **Captures are whole variables, borrowed in place and moved into owned positions.** A closure captures every
  variable its body uses and does not bind. The context decides the mode: an expected owned closure type (a written
  annotation, a field or element type, a return, an own argument, or an `Fn(...)` parameter) moves the captures in;
  any other use borrows. A generic `F: Fn`/`FnMut` keeps the concrete mode, so a borrowing closure crosses that call
  and remains second-class (E0120/E0123/E0171); an `FnOnce` bound moves because the call consumes the closure. A
  body write makes a borrowed capture `&mut`. A closure that moves a view (`str`, `&T`, a slice, or an aggregate
  containing one) into its record is second-class as well: the record holds bits that point at storage the closure
  does not own, so it cannot escape either (E0171).
- **The body's use of captures decides the kind.** Moving a moved-in non-Copy capture out makes the closure
  `FnOnce`; writing a capture makes it `FnMut`; otherwise it is `Fn`. The sema-side classification is a typed-tree
  walk (`fncx/closure.rs`); the move checker still enforces the move and borrow rules themselves. Calling borrows
  `Fn` shared, borrows `FnMut` exclusively (E0113), and moves `FnOnce` (E0100). A closure value may be called at a
  lower capability, so a `Fn`/`FnMut` value called through a weakened `FnOnce` type consumes and destroys its
  capture record (on the normal and on an early `return`). A closure that must outlive the call cannot capture a
  read-only or `mut` parameter by move (E0101).
- **Free names are captured eagerly.** The body's free-name scan runs before the closure is checked, so a nested
  closure's names are captured through the enclosing closure's record. A closure body is a separate function with
  its own return, loops, and locals; `break`/`continue` do not cross it, and a second-class return type is E0171
  because the result would point into the record.
- **Function values are captureless closures.** A non-generic function name, a built-in function name, or `Some`
  used where a closure type is expected synthesizes a closure with no captures whose body forwards its owned
  arguments to the function with the function's own conventions. A function returning a view is E0171 (the view
  would point into the wrapper's argument), an `unsafe fn` is E0161 (a closure call has no `unsafe` boundary), and
  a generic or `extern` function as a value stays E9000 under Stage 1. The E9000 audit rows owned by m12 were
  removed with their sites.
- **Known limits of the milestone.** A closure cannot return a view (`str`, `&T`, `[T]`) at all; parameter types are
  inferred only from an expected closure type or an `Fn` bound, so a bare `let f = |x| ...` needs an annotation; there
  is no `move` keyword because the context selects move captures; a closure *literal* passed to a bound whose
  signature names another type parameter of the callee (`F: Fn(T): T`) is not inferred, because the arguments are
  checked in order and the closure's signature is fixed from the bound before `T` is known (pass a variable of a
  concrete closure type instead); passing a closure parameter through a generic function by returning it is rejected
  by the existing move/origin rules (returning a closure created inside the function works); a generic function
  cannot be used as a value (annotate a concrete instantiation by wrapping it in a closure); and every closure value
  allocates its record.
- **An array drop/clone bug was fixed with the milestone.** The drop and clone glue walked arrays with a one-index
  GEP, which strides by the whole array instead of one element; an array of closures exposed it. The glue now uses
  `[0, i]`, and arrays of any droppable element drop and clone correctly.

## Decided in the twenty-seventh session

- **Format strings are a compile-time construct.** A literal's placeholders are checked against the call's
  arguments (E0191, E0192), the value's interface (E0193), and the specifier's applicability (E0190) when the
  program is compiled, and the compiler lowers the string to `Formatter` calls. Nothing parses a format string at
  run time, so a placeholder cannot fail while the program runs. The rule is `spec/format.md` FM1-FM10.
- **`{}` numbering is positional, `{expr}` is not.** `{}` takes the next argument and `{n}` an argument by index;
  any other placeholder holds an expression evaluated where the string is. A string literal outside a format call
  has no argument list, so `{}` in one is E0191; this keeps `println(text)` (one argument, no format string)
  unchanged, so the `String`-to-`str` conversion and every stage 0 program behave as before.
- **An argument used twice is evaluated once.** A place is borrowed and a `Copy` value is copied into a hidden
  local; a value that is neither is E0191, because re-evaluating a call would run it twice. This is a deliberate
  limit: it needs no hidden temporary for an owned value.
- **`Display` and `Debug` are ordinary `std` interfaces with compiler-known implementations.** Built-in types have
  no impl blocks; the checker answers "does this type implement it" from its type (`builtin_formattable`), and code
  generation uses `priv` helpers in `std/src/fmt.bd`. A user type writes the block, and `Debug` may be derived.
  `Formatter` applies the specifier in `pad`/`pad_number`; a user `fmt` chooses what to honor.
- **`derives (Debug)` is generated source, not a special code path.** The parser appends an impl block written as
  Burdin text, parsed with the same parser, with every token's span pointing at the `Debug` in the derive clause.
  The checker drops diagnostics raised inside that block so the field's E0130 is the one a programmer sees.
- **Floats print the shortest round-trip text.** A new runtime function formats with `snprintf("%.*e")` and reads
  the text back with `strtod` until it matches, then `std` turns the digits into positional notation; a precision
  uses `snprintf("%.*f")`. No exponent notation is printed, `NaN`/`inf`/`-inf` are spelled out, and a float in
  `Debug` shows a `.0`.
- **A signed integer in a radix shows its two's complement in its own width**, so `(-1 as i8) as {x}` is `ff`. The
  milestone keeps stage 1's promise to extend stage 0 rules rather than add parallel ones: `to_string` is `Display`,
  the output functions keep their stage 0 flush behavior, and the new codes slot into the existing registry.

## Decided in the twenty-eighth session

- **`any I` is an owned heap box `{data, vtable}`.** The design's "owned type the compiler places on the heap" is
  two words: a `malloc`ed concrete value and a pointer to a constant vtable. The box owns the allocation; drop runs
  the concrete destructor through the vtable and frees, and a successful `downcast` moves the value out and frees
  without dropping it again. This is `spec/existentials.md` AN1 and AN7.
- **Boxing is implicit at every expected type, and it is the second implicit conversion.** Unlike the
  `String`-to-`str` borrow (IC1), boxing produces an owned value and allocates, so it is allowed wherever an
  expected type is known: annotations, arguments, returns, fields, elements, `Some`/`Ok`/`Err` payloads, and match
  arms. A view cannot be boxed (E0120), because the box would outlive the borrow. This extends decision 2 rather
  than adding a parallel rule (AN3).
- **Object safety is the vtable's requirement, not the interface's.** The rule list is AN2: no generic methods, a
  `self`/`mut self` receiver, no `Self` outside the receiver, no view return, and object-safe `requires`
  interfaces. The reason for the view-return rule is the second-class model: a view out of the box would borrow the
  allocation, which the receiver's `self` already covers, and rejecting it keeps every dynamic result owned. A
  generic interface instance is object-safe once its arguments are fixed (`any Store<i32>`).
- **`impl I` is a parameter-only hidden generic, not an opaque return type.** It desugars in the parser to one
  hidden type parameter with `I` as its bound, so all of inference, bounds, and monomorphization are the existing
  generic machinery, and `I` does not need object safety. Return-position `impl I` would be an opaque type, a much
  larger feature; it is E0201 with a note to use `any I` (AN4).
- **Downcast is a consuming method with a type argument.** `x.downcast<T>(): Option<T>` and `x.is<T>(): bool`, with
  the type written after the member name (`x.downcast<Circle>()`), extend rule 8.2's look-ahead to member names.
  `downcast` consumes the box so a `List<any I>` can be drained; on a mismatch it drops the value, so a failed
  downcast loses the value, which the `Option` return makes explicit. Type identity is per monomorphized type, so
  generic instances are distinct (AN6).
- **The `Display`/`Debug` built-in implementations are the one absent vtable.** The compiler formats primitives and
  built-in containers directly and has no method function for them, so boxing those is E0204 rather than a silent
  wrong call. A user impl or `derives (Debug)` supplies the method and works (`any Display` itself formats through
  the vtable). This is the milestone's documented limitation (known problem 21).
- **The vtable is a private constant with a type id first.** `{ i64 type_id, ptr drop, ptr method... }`; the type
  id is a program-wide index shared across interfaces, the drop slot is the existing drop glue, and each method
  slot is the concrete implementation's function (object-safe receivers are already passed by address, so no thunk
  is needed). Code generation emits one per `(T, I)` and `is`/`downcast` compare the id (AN11). Vtable entry
  functions are reachability roots, because no direct call names them.

## Decided in the thirty-eighth session

- **A task is a stackful coroutine on a fixed 256 KiB stack, scheduled cooperatively (`spec/tasks.md` TK1, TK3).** A
  state-machine transform would need a function coloring; one OS thread per task does not scale. A 64 KiB `PROT_NONE`
  guard below each stack turns an overflow into `panic: stack overflow`. There is no preemption in 2.1.
- **The scheduler is Burdin code over runtime primitives, not Rust or generated IR (TK2).** The language already has
  raw pointers, `repr(C)` structs, closures, and `unsafe`, so only threads, locks, the context switch, and two
  slots need the compiler. The contract with the runtime is that the first word of the worker record is its own
  context (`main_ctx`), which `ctx_entry` switches to when a task body returns.
- **Yielding goes to the back of the shared queue (TK5, TK6).** Putting a yielded task on the worker's own queue starved
  every other task behind a yielding one; the shared queue is FIFO across workers.
- **A panic in a task aborts the process (TK8, issue #205).** No unwinding exists, and delivering it to the joiner would
  make `join` return a `Result` for a bug.
- **`Send` waited for 2.3 (TK11, issue #199)** and is now derived (SH4).
- **Scoped tasks reuse the closure borrow rules, with loans pinned to the end of the scope body (TK7, SK1-SK3).** The
  checker recognizes `std.task.Scope.spawn` by name. A task may borrow only the body's captures, and a closure that
  borrows must be written at the call. Disjointness is by captured variable, since a closure captures whole variables.
- **`__closure_own` is the one unsafe relabeling (TK7).** `Scope.spawn` takes `FnMut`, so its closure borrows, and the
  primitive that starts a task wants an owned `FnOnce` of the same representation; the checker rules, not the type,
  make that sound.

## Decided in the thirty-ninth session

- **`Channel<T>` is one cloneable handle type, multi-producer and multi-consumer, closed explicitly (`spec/channels.md`
  CH1-CH3).** Separate `Sender`/`Receiver` types would need two names and a pair-returning constructor; the design
  asks for one `Channel<T>`. A loop `for v in rx` ends because somebody called `close`, as in Go. `send` on a closed
  channel hands the value back (`Some(value)`) instead of panicking, because a close races with senders by design.
- **Bounded (capacity at least 1) and unbounded; no rendezvous channel (CH2).** A capacity-1 channel is the nearest;
  `bounded(0)` panics.
- **`select` is a contextual keyword rewritten by the parser (CH6).** The arm bodies sit outside the generated loop, so
  `break`/`continue`/`return`/`?` mean what they always mean. An arm is `[v =] ch.recv()`, `[r =] ch.send(x)`, or
  `default`, with the binding the same `Option<T>` the method returns, and an optional `if guard`.
- **Source-order priority, with guards instead of nil channels (CH7, CH8).** Deterministic on purpose. A closed channel
  is always ready, so an earlier arm on a closed channel starved the later arms until a guard was added to switch it off;
  an all-switched-off `select` without `default` panics rather than wait forever.
- **A send arm's value moves when the `select` starts (CH8).** The value is evaluated once, up front, like the channel
  expression; an arm that does not run drops it.
- **All channel state is guarded by the scheduler's `state_lock` (CH10).** It makes `select` a few steps under one lock
  with no lock order, at the cost that channel operations serialize; a per-channel lock is a later optimization. A change
  counter read before a `select` looks at its arms is what makes a lost wake-up impossible.
- **`clone` is an ordinary method, not `Clone`.** Writing a built-in interface by hand is still not implemented
  (issue #402); the E9000 says so. `Channel<T>: Iterable<T>` works.
- **Not done, on purpose:** a timeout arm and timers (2.4), nil channels, and rendezvous channels.

## Decided in the forty-third session (`burdin fmt`, issue #229)

- **The formatter has its own parser.** The compiler's syntax tree is lossy (a `select` is rewritten, impl parameters
  are desugared, `use` declarations are separated, parentheses and comments are gone), so `burdin_fmt` reads the lexer's
  tokens and lays them out itself, sharing only the look-ahead rules (`burdin_syntax::lookahead`). The compiler's parser
  is the judge: the source must parse, and the output must parse to the same span-free tree and the same comment
  multiset, or the result is an internal error and the file is left alone (`crates/burdin_fmt/src/verify.rs`). The
  layout is then repeated until it stops changing (at most four rounds), which makes idempotence hold by construction
  for the odd comment placements that move on the first pass.
- **One style, written down in `spec/fmt.md` (FM1-FM13).** 4 spaces, 120 columns, magic trailing comma, leading
  `&&`/`||`/`orelse` and trailing other operators, method chains broken before `.` from two calls, one member per line
  in type bodies, `;` dropped, short single-expression blocks inline only inside expressions. These are taste; change
  them in the spec and the goldens together (`tests/fmt/*.golden`).
- **`use` merging is conservative.** Only brace lists with the same path merge; `use a.b` (maybe a module) and
  `use a.{b}` (an item) stay apart; a declaration with a comment is never merged. Groups are std (`std` or a standard
  module), the dependencies named in `burdin.toml`, then the package.
- **Doc-comment code blocks** (FM10) are formatted as items, else as a function body, at the width left after the
  indentation, and only when the result passes the same program and idempotence checks.
- **Range formatting** (FM11) reformats whole top-level items overlapping the range and copies the rest verbatim; it does
  not sort `use` declarations.
- **Known limits.** A comment between two tokens where the grammar allows no line break moves to the end of its line;
  two such line comments go on separate lines. `1.` followed by a line break and an integer keeps the break so it is not
  read as a float. Nothing formats a file with syntax errors. `burdin fmt` is not wired to the language server
  (stage 3); `--range` and `--stdin` are its contract.
- **Tests:** `tests/check_fmt.py` (fixtures in `tests/fmt/`, black-box), and in `crates/burdin_fmt`: unit tests, the
  repository corpus, a fuzz round trip (`BURDIN_FMT_FUZZ_SEED` and `BURDIN_FMT_FUZZ_MUTATIONS` choose a longer session),
  and the regressions the fuzzer found. A change to the style regenerates the goldens with
  `burdin fmt --stdin < tests/fmt/<name>.input > tests/fmt/<name>.golden` and the diff is reviewed.

## Decided in the fortieth session

- **`Shared<T>` is always atomic (SH1, issue #214).** One type, no non-atomic variant: relaxed increment on clone, release
  decrement on drop, an acquire fence before the value is freed. A handle still cannot cross to a task unless `T` is
  `Send` and `Sync` (SH2), so the count never protects an unsafe value.
- **`Send` and `Sync` are derived, with `unsafe` impl blocks as the override (SH4, SH5).** A raw pointer and `any I` are
  never either; a closure is `Send` when what it moves is `Send` and what it borrows is `Sync`, and is never `Sync`; a
  closure type written as an annotation is not known to be `Send`. E0280 is "not `Send`/`Sync`", E0281 an invalid marker
  impl. An error from a failed requirement is not repeated inside the library code that uses the type.
- **Atomics are 32- and 64-bit and sequentially consistent (SH7).** No weaker orders, no 8- or 16-bit types, no atomic
  pointers. `fetch_max` and `fetch_min` are compare-exchange loops.
- **Locks use closures, not guards, and there is no poisoning (SH8).** A view cannot be stored, so a guard could not be
  a value; a panic aborts the process, so no lock is ever left poisoned. A waiter parks (a thread blocks) and is woken by
  the releaser; the lock state is under the scheduler's lock (SH9). A waiting writer blocks new readers (SH10).
- **`Weak<T>`, condition variables, barriers, and `Once` are deferred.** Nothing in the tests needs them.

## Decided in the forty-ninth session (the language server, issue #242)

- **Rust, no new dependency, stdio only (#365).** JSON-RPC needs a small JSON value and `Content-Length` framing,
  which are under 600 lines with their tests; `lsp-server`/`lsp-types`/`serde` would be the first serialization stack in the compiler. Every
  target editor starts servers as child processes, so there is no socket transport.
- **One thread owns the databases.** `burdin_query::Db` is `Rc`/`RefCell`; the server answers in order on the main
  thread, a reader thread records cancellations and stops the running analysis, and test runs are the only work on
  other threads (their results come back as messages).
- **One database per package root, not per workspace.** It matches `burdin test` and the query library's unit; a path
  dependency is part of each dependent's database, and the memo is what keeps an edit cheap.
- **The test lenses carry the exact command line** (`burdin test <root> --exact <path>`), and the server runs it with
  `--format json`, so the editor, the terminal, and CI run the same thing. `burdin.run` and `burdin.debug` are client
  commands; debugging targets `burdin dap` (#576) through a DAP `launch` configuration of type `burdin`.
- **Formatting is the `burdin fmt` library**, refusing a file with syntax errors (`null`) instead of guessing.

## Decided in the forty-seventh session

- **The memo is in-house, not salsa (QL1, issue #645).** `burdin_tir::Program` and the checker's registries are
  `Rc`/`RefCell` graphs; salsa's storage wants `Send + Sync` values, and making every stage thread-safe to hold a
  cache that is used from one thread at a time is not worth it. The memo is a table keyed by 128-bit fingerprints in
  `burdin_sema::CheckCache` (bodies) and `burdin_query::Db` (parsing and the pipeline). No external dependency was
  added.
- **The unit of reuse is the function or constant body, and a declaration edit is global (QL2, QL3).** A body is
  keyed by its declaration-span text, the declaration fingerprint (file texts with the parser-recorded bodies
  elided), and the pass chain; a declaration edit rechecks every body because ids are baked into the typed tree.
  The memory checks and compile-time evaluation run over the whole program on every analysis. Both are documented
  limits, measured by the benchmark, not hidden behavior; making them per function is a later refinement.
- **The body-only reparse is a narrow, checked shortcut (QL6).** It applies only when the edit is strictly inside
  one body, the new text is exactly one block, the old file had no syntax errors, and the body is not `main`'s and
  does not mention `select`; the result is compared with a full parse in `tests/reparse.rs` for every case.
- **The tool API returns plain data (QL7).** A tool depends on `burdin_query` and not on the typed tree; positions
  are byte offsets, and types are printed as source writes them. `spec/query.md` names each answer and the fixture
  golden that pins it.

## Decided in the forty-fourth session (`burdin doc`, issue #234)

- **The generator reads syntax trees, not the typed tree.** Documentation is a property of the source, and a package
  that compiles is documented whether or not its types are monomorphized; this also keeps `burdin doc` free of the
  compiler stages and lets `missing_docs` share the exact reading of the comments (`burdin_syntax::docs`).
- **The site is one set of static files and needs no server.** The stylesheet and the search script are compiled into
  the binary and copied into `static/`, and the index is `search-index.js` (`window.BURDIN_DOC_INDEX`) rather than a
  JSON fetched at run time, because a `fetch` from `file://` is blocked in browsers. Pages load nothing from another
  server.
- **Links are Markdown links with Burdin paths, resolved like code.** One or two `.`- or `::`-separated segments are
  looked up the way a path in the module would be; anything that leaves the package is `external` and not an error.
  The warning codes are `D0001` (unresolved) and `D0002` (a `priv` item), registered in `tests/README.md`.
- **The JSON schema is versioned (`format_version: 1`)** and is the API for tools that do not read HTML; a field is
  removed or changes meaning only with a version bump. It carries spans, link resolutions, operator displays, member
  origins, and instances.
- **`--private` is the only switch over the public interface**, and the default never writes a `priv` item. The
  coverage report and the `missing_docs` lint are one definition seen twice, so they cannot disagree.
- **The output directory is guarded.** `burdin doc` writes a `.burdin-doc` marker listing its files, refuses a
  directory that holds other files (status 2), and removes the pages of items that are gone; `-o .` cannot overwrite a
  project.
- **Not done, on purpose:** doc tests (#250, #650-#652), a `--document-private-items` alias, a theme switch beyond the
  system setting, and wiring the JSON into an LSP symbol view (stage 3 tooling).

## What is left to do, in order

0. Milestones 2.1 (tasks, thirty-eighth session), 2.2 (channels and `select`, thirty-ninth session), and 2.3 (shared
   state, fortieth session) are complete. The next stage 2 milestone is 2.4 (I/O). The 2.3 atomics and locks add
   no runtime of their own beyond the atomic builtins, whose arm64 forms are LLVM's; run them with the rest of
   `tests/s2_*` on macOS and Windows. Before that, run the CI jobs for macOS and
   Windows (the channels add no runtime, so they run wherever tasks do): `tests/s2_tasks` is the first run of the arm64 context-switch assembly, the Darwin constants, and
   `taskrt/windows.ll` (fibers); fix what they find. Possible follow-ups: a per-task stack-size option, cooperative
   preemption points, and making blocking calls suspend the task (2.4).
1. Milestone 23 is complete (thirty-sixth session): `spec/allocators.md` AL1-AL10 is the decision and
   `tests/m23_allocators/` has 28 tests. The next milestone is 24 (`m24_stdlib`: `Set`, `Deque`, a priority heap,
   Unicode helpers, paths, time, random, and the formatting foundation), which may split as the standard library
   grows. Issue #444 (differential tests between comptime and compiled code) is done (thirty-sixth session);
   `spec/test-runner.md` holds its behavior. The C bindgen is done (forty-third session, `spec/bindgen.md`); Burdin
   function-pointer types and `union` declarations of its own remain on the stage 1 "Fixes" list.
2. The remaining stage 1 work is milestone 24 and the open type-system item: writing a built-in interface by
   hand (#402). Known limits to keep in mind: the comptime ones (a closure inside a function with comptime
   parameters is shared by its specializations; `Map` iteration at compile time is insertion order, CT9;
   comptime results are not cached, CT14), the m21 ones (problems 24-27 above), the temporary-move and temporary-drop bugs (problems
   26, 28, and 32), and the allocator limits (problems 29-31).
3. The stage 0 correctness gaps of issue #48 are closed (twenty-third session): the view lifetime (E0123), ranges as
   values, the tracked move forms, the generic gaps, `pub use` and module visibility, and the C bridge. What stage 1
   still inherits is listed in `spec/stage1-subset.md` "Fixes that ship with stage 1": moves through a runtime index,
   C function-pointer and `union` types, by-value C aggregates, and a `priv` module marker if a project needs one.
4. CI runs on `ubuntu-24.04` (LLVM 21 from apt.llvm.org), `macos-latest` (Apple Silicon, Homebrew `llvm@21`), and
   `windows-latest` (MSVC and the official LLVM 21 archive, issue #12); all three pass every step and m01-m23 in both
   the debug and release profiles (issues #11, #16, #47, and #12), and cache cargo and LLVM 21 (issue #46).
   `tests/check_ci.py` guards that the workflow keeps covering the suite in both profiles, every job, and their
   caches; `tests/check_platform.py` guards the host triple. Every job also records and compares benchmark baselines,
   advisory until the runner numbers are stable (issue #446); when they are, drop `continue-on-error` from the step.
   The weekly `fuzz` job runs the longer lexer/parser session and uploads a found input (issues #288, #447); the
   corpus replay runs in every job, so no further wiring is needed for a reintroduced panic.
   Next: an Intel macOS job and another arm64 host if they are wanted.
5. The 16 assumptions in `tests/README.md` are settled (issue #21, sixteenth session); their rules are in `spec/` and
   their tests are in the suite. A new open question still gets an `//@ undecided` test until it is decided.
6. The issue #421 work is done, with these follow-ups if they ever matter: a PDB/CodeView path for Windows so
   backtraces and debuggers get symbol names (the COFF object carries LLVM debug metadata today, but `cl` produces no
   PDB); multi-object emission so LTO has something to combine; parallel checking on top of the query library (its
   memo is single-threaded today); and a stable PGO benchmark where instrumented profiles show a measurable gain. The code-size
   budgets in `tests/check_build_flags.py` are the regression guard for the release binary.
7. The release pipeline is implemented (issue #452, twenty-second session), but no release has been cut. Next: run the
   manual dry run from `docs/releasing.md` (it builds all six targets and creates no release), confirm the `windows-11-arm`
   and `macos-15-intel` images on this repository, then tag the first stable release. Follow-ups the pipeline does not
   cover yet: a package-manager formula (Homebrew/winget), code signing and provenance attestations, bundling the LLVM
   21 runtime so a prebuilt compiler is self-contained, and a PDB/CodeView path for Windows.
8. The query-based compiler library is in (forty-seventh session, issue #241, `spec/query.md` QL1-QL9). Follow-ups it
   leaves: parallel checking on top of it and per-function memory checks and compile-time evaluation. The language
   server is in (forty-ninth session, issue #242, `spec/lsp.md`); its follow-ups are in that session's section. The rest of stage 3's compiler item — a lockfile, multiple-package workspaces, cached
   object files — and its tooling (`doc`, `add`) have not started; `burdin fmt`, `burdin lint`, `burdin new`, and the
   bindgen are done.

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md, AGENTS.md, HANDOFF.md, and spec/stage1-subset.md. Settle the decisions for milestone 24, then write its tests.

If the session has no repository access to the folder, attach `CLAUDE.md`, `AGENTS.md`, `HANDOFF.md`, `spec/`, and `tests/README.md`.
