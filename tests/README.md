# Burdin stage 0 conformance tests

These programs are the target for the stage 0 compiler (Rust, LLVM backend). They come from
[the design document](../Burdin%20Language%20Design.md) and [the stage 0 subset](../spec/stage0-subset.md). The design
document's snippets are illustrative; these programs are the executable form. Each folder is one milestone; a
milestone is done when every test in its folder and in all earlier folders passes.

Stage 1 starts at `m12_closures`; everything the stage 1 plan still defers (the standard library's remaining
containers, tasks, and the concurrency stage) has **no tests here** yet.

## Layout

```
tests/
  m01_hello/           hello world, lexer, syntax errors
  m02_basics/          integers, control flow, functions, panics
  m03_typecheck/       inference and type errors
  m04_data/            structs, tuples, enums, match
  m05_ownership/       moves, Drop, Heap
  m06_conventions/     mut/own, exclusivity, references, views
  m07_generics/        generics, interfaces, derives
  m08_modules/         files as modules, priv, extern "C", unsafe
  m09_stdlib/          Option, Result, ?, String, List, Map, I/O
  m10_test_runner/     `test` blocks and `burdin test`
  m11_llvm/            calling the LLVM C API from Burdin
  m12_closures/        closures, captures, closure types and bounds
  m13_iterators/       the iteration protocol, adapters, ranges, user `for` loops
  m14_format/          interpolation, `Display`/`Debug`, format specifiers, `format`, output
  m15_any/             `any`/`impl` existentials, boxing, dispatch, downcast
  m16_errors/          `error` declarations, messages, `from`, dynamic `Error`, `context`, traces, `main`
  m17_arguments/       default arguments, named arguments at call sites, conventions and evaluation order
  m18_extension/       extension methods, resolution order, imports, shadowing diagnostics
  m19_operators/       operator overloading, PartialEq/PartialOrd, Index/IndexMut, built-in regressions
  m20_comptime/        comptime expressions and parameters, comptime if/for, const evaluation, comptime assert
  m21_reflect/         reflection members, user derives, const generics, embed, generic aliases, impls for
                       specific instances
  m22_build/           build.bd, dependency grants, generated modules, the minimal bindgen
  m23_allocators/      allocator parameters, Arena, Shared<T>, leak detection, allocator-aware containers
  m24_stdlib/          the standard-library roundout: Ordering, containers, text, numbers, I/O, time, random
```

A test for a design question that is still open can be marked `//@ undecided: question`. The runner skips it.
Put it in the folder of the milestone it belongs to, and remove the directive once the question is settled.

A test is either one `.bd` file (compiled as a one-file package whose root is that file) or a directory
that is a package (`burdin.toml` plus `src/`).

## Directives

Directives are line comments at any position in the root file.

| Directive | Meaning |
| --- | --- |
| `//@ run` | Compile and run. Exit code must be 0 (or `exit-code`). Stdout must equal the `//>` lines, in order. |
| `//> text` | One expected stdout line. The text after `//> ` is exact, with no trailing space. |
| `//@ run-abort` | Compile and run. The process must end abnormally (panic abort, non-zero exit). Stdout before the abort must equal the `//>` lines. |
| `//@ stderr: text` | Stderr must contain `text`. May repeat; all must match. |
| `//@ compile-fail` | Compilation must fail. The set of (line, code) diagnostics must equal the `//~` markers exactly. |
| `//@ test-compile-fail` | Like `compile-fail`, but the package is compiled in test mode (`burdin test`), so diagnostics inside `test` items and their attributes are reported. |
| `//~ E0000` | At the end of a line: that line must carry that diagnostic. Several markers per file are allowed. |
| `//@ burdin-test passed=N failed=M` | Run `burdin test` on the package. Counts must match. |
| `//@ burdin-test passed=N failed=M ignored=I` | The same, with I tests marked `[ignore]`. |
| `//@ failed-test: name` | That test must be reported as failed. May repeat. |
| `//@ exit-code: N` | Expected exit code with `run`. |
| `//@ args: a b` | Command-line arguments passed after the program name. |
| `//@ stdin: text` | One line of standard input for a `run`/`run-abort` test, followed by `\n`. May repeat; the lines are joined in file order. `\n`, `\r`, and `\\` are escapes. |
| `//@ stdin-eof: text` | Text appended after the `stdin` lines with no terminator. May repeat. A test with neither directive reads an empty standard input. |
| `//@ debug-only` | Skip in release profile (overflow checks). |
| `//@ skip-platform: os` | Skip on that host: `linux`, `macos`, or `windows`; several may be listed. |
| `//@ link-lib: NAME` | Extra library to link. |
| `//@ undecided: text` | Skipped; documents an open design question. |

Runner rules: run each test in a fresh empty temporary directory as the working directory; give standard input
the `//@ stdin:`/`//@ stdin-eof:` bytes (an empty pipe when neither is present, so a test never waits for input);
compare stdout after normalizing `\r\n` to `\n`; every `compile-fail` file must report all of its errors (no
stopping at the first error) and no others. The `test` item's attributes (`[ignore]`, `[timeout(ms)]`,
`[should_panic]`, `[should_fail]`, `[differential]`) and the `burdin test` flags are specified in
[`spec/test-runner.md`](../spec/test-runner.md).

## Running the tests

```
python3 tests/run_tests.py                # every test, debug profile (python on Windows)
python3 tests/run_tests.py m05 m06        # only paths containing m05 or m06
python3 tests/run_tests.py -v drop_order  # show diffs and compiler output for failures
python3 tests/run_tests.py --release      # release profile (skips debug-only tests)
python3 tests/run_tests.py --list         # list tests and their kinds
python3 tests/run_tests.py -j 4           # four tests at a time (default: CPU count, at most 8)
BURDIN=/path/to/burdin python3 tests/run_tests.py  # choose the compiler driver
python3 tests/check_tests.py              # check directives and error codes (no compiler needed)
python3 tests/check_e9000.py              # check the E9000 audit and its milestone owners (no compiler needed)
python3 tests/check_ci.py                 # check the CI workflow and the Rust toolchain pin (no compiler needed)
python3 tests/check_platform.py           # check the host triple and 64-bit isize/usize (needs a built driver)
python3 tests/check_platform_support.py   # check the platform support page against CI and binary-safe output (needs a built driver)
python3 tests/check_docs.py               # check documentation links and the driver contract (needs a built driver)
python3 tests/check_deps.py               # dependency, pin, and license audit (no compiler needed)
python3 tests/check_debug_info.py         # -g: DWARF, line table, and a gdb breakpoint when gdb is installed
python3 tests/check_codegen_opt.py        # drop-flag elimination, bounds-check elision, shared glue
python3 tests/check_build_flags.py        # --version, --strip's budget, --lto, and PGO
python3 tests/check_parallel.py           # parallel parsing and deterministic diagnostics
python3 tests/check_diagnostics.py        # check color, width, formats, error limits, and ordering (needs a built driver)
python3 tests/check_error_docs.py         # check the error-code reference and `burdin explain` (needs a built driver)
python3 tests/check_test_runner.py        # check `burdin test` attributes, parallelism, output, isolation, discovery
python3 tests/check_build_scripts.py      # check build-script caching, declared inputs, --emit-deps, and bindgen
```

CI runs every finished milestone in both profiles, so a milestone is done only when its folder passes in debug and
release. The release run skips tests marked `//@ debug-only`. The `burdin test` features themselves (attributes,
`-j`, output capture, isolation, `-p`/`--workspace`) are checked black-box by `tests/check_test_runner.py`, with
fixtures under `tests/runner/` that carry no `//@` directives and are therefore not conformance tests.

The compiler and runtime work of issue #421 is covered in two places. The observable runtime behavior has conformance
tests: `m02_basics/panic_backtrace.bd` (both profiles), `m02_basics/panic_backtrace_names.bd` (debug-only, Linux
and macOS),
`m02_basics/stack_overflow_aborts.bd`, and the existing flush tests
(`m02_basics/panic_flushes_stdout.bd`, `m09_stdlib/exit_code.bd`). The flags and code-generation effects have the
four scripts above: they read the `--emit-ir` text (drop flags, bounds checks, glue) and compare parallel and
sequential diagnostics. `BURDIN_JOBS=1` forces sequential parsing for a comparison; the default is the machine's
parallelism.

The runner finds the driver in `BURDIN`, then `target/{debug,release}/burdin`, then `PATH`. It expects `burdin build`,
`burdin run`, and `burdin test` to behave as documented in [the driver contract](../README.md#the-driver-contract) at
the repository root (summarized at the top of `run_tests.py`): diagnostics as
`path:LINE:COL: error[E0000]: message`, and a `test result: N passed, M failed` line plus `FAIL <name>` lines from
`burdin test`. If the real driver differs, change the section marked in that file. The exit code is 0 only when every
test passes. All the scripts under `tests/` are Python 3 (invoke them with `python3` on Unix and `python` on Windows).
The runner runs tests in parallel by default; `-j N` sets the worker count, and results are printed in the same order
either way.

## Benchmarks

The compiler benchmarks are not part of the conformance run:

```
cargo bench -p burdin_bench                       # medians per stage and size
cargo bench -p burdin_bench -- --smoke            # the time budgets CI runs
cargo bench -p burdin_bench -- lex                # only cases whose name contains `lex`
cargo bench -p burdin_bench -- --save bench.txt   # record the medians as a baseline
cargo bench -p burdin_bench -- --baseline bench.txt   # compare; exit 1 when a case is over the threshold
python3 tests/bench_regression.py         # the advisory check CI runs
```

`crates/burdin_bench` generates its inputs, so a size is the only parameter of a measurement. A baseline is a
plain text file of `case<TAB>median nanoseconds` lines, and `--threshold` sets the percentage (default 25) a case
may get slower before `--baseline` reports it. CI keeps the baseline in `actions/cache`, keyed by the runner OS
and architecture, the pinned toolchain, and `Cargo.lock`, so only numbers from the same kind of machine are
compared; a push to main records a new baseline, and a pull request compares against the last main run on that
platform. `HANDOFF.md` records the first baseline and the meaning of the `--smoke` budgets.

## Dependencies and licenses

[`dependencies.txt`](dependencies.txt) is the reviewed record of every external crate: one
`name<TAB>version<TAB>license<TAB>why` line per crate in `Cargo.lock`. `tests/check_deps.py` fails when the
lockfile and the record disagree, when a crate has no checksum, when a direct requirement is not an exact
`=x.y.z` pin, or when a license is outside the permissive policy stated at the top of that file. It also
re-reads each crate's license and proves the lockfile is current with `cargo metadata --locked --offline`;
a missing registry cache makes that verification a note, unless `BURDIN_DEPS_REQUIRE_METADATA=1` (which
every CI job sets, after `cargo build`). The policy and the review are
[`docs/dependencies.md`](../docs/dependencies.md).

## Error codes

The registry the tests use. The compiler adopts these numbers (or the tests are updated). Every code also has a
user-facing entry in [the error-code reference](../docs/error-codes.md), which `tests/check_error_docs.py` checks
against this table and whose examples it compiles; `burdin explain <code>` prints the entry.

`E9000` is not a registry code: it means "not implemented in this compiler yet". Every remaining site is assigned
to a milestone in [`spec/e9000-audit.md`](../spec/e9000-audit.md), and `tests/check_e9000.py` fails when a site is
unassigned, stale, or owned by a milestone that has shipped.

| Code | Meaning |
| --- | --- |
| E0001 | Syntax error (lexing or parsing) |
| E0002 | Unknown name (variable, function, type, module, or imported item) |
| E0003 | Type mismatch |
| E0004 | Wrong number of arguments |
| E0005 | Assignment to an immutable binding, or mutation through a read-only parameter |
| E0006 | Non-exhaustive `match` (reported at the `match` line) |
| E0007 | A name is declared or imported more than once (type, interface, function, constant, test, type parameter, field, variant, member, or import) |
| E0008 | Literal out of range for its type |
| E0009 | A declaration depends on itself: a constant (also through the calls of its compile-time evaluation), a type alias, or a type that contains itself |
| E0100 | Use of a moved or possibly moved value |
| E0101 | Move out of borrowed content (for example returning a read-only parameter by value) |
| E0103 | Partial move out of a type that implements `Drop` |
| E0104 | Value moved in a previous loop iteration |
| E0110 | Exclusivity violation in a call (a `mut` argument also reachable through another argument) |
| E0111 | Conflicting borrow (write, `&mut`, or move while a reference is still used later) |
| E0113 | Mutable use of an immutable place: a `mut` argument or `&mut` taken from an immutable binding |
| E0114 | Missing `mut` marker at the call site for a `mut` parameter |
| E0115 | `mut` marker on an argument whose parameter is not `mut` |
| E0120 | Reference or view stored in a struct field, enum variant, or collection |
| E0121 | Returned reference or view derives from more than one parameter (reported at the signature) |
| E0122 | Returned reference or view points to a local |
| E0123 | A view stored in a binding outlives the storage it borrows from (a temporary or a local of an inner block) |
| E0130 | Derive requirement not met (reported at the offending field or derive clause) |
| E0131 | A `Drop` implementation that is not exactly one method named `drop` |
| E0140 | Interface implementation is missing a method (reported at the `impl` line) |
| E0141 | Interface bound or `requires` not satisfied, a generic body uses an operation its bounds do not provide, or a user type has no implementation of an operator or `Index`/`IndexMut` interface |
| E0142 | Orphan rule violation |
| E0143 | Two implementations of one interface for one generic type that apply to a common instance |
| E0150 | Use of a `priv` item from another package |
| E0160 | `?` used in a function whose return type is not `Option` or `Result` |
| E0161 | Operation that requires `unsafe` used outside `unsafe` |
| E0162 | Type or convention not allowed in an `extern "C"` declaration (parameters and results are integers, floats, and `CPtr`) |
| E0163 | Invalid `extern` declaration: a runtime-reserved symbol, a generic function, or a signature that conflicts with an earlier declaration of the symbol |
| E0164 | Unsupported `repr`: only `repr(C)` exists |
| E0170 | Invalid test attribute: an unknown name, a repeated attribute, a value the attribute does not take, `should_panic` with `should_fail`, or `differential` with `should_panic` or `should_fail` |
| E0171 | A closure whose captures or result would outlive the storage they borrow: a second-class closure used as an owned one, or a closure returning a view |
| E0180 | Type arguments given to a type that declares no type parameters |
| E0190 | Format specifier that is malformed or does not apply to the formatted value |
| E0191 | Placeholder naming an argument the call does not have |
| E0192 | Argument of a format call that the format string does not use |
| E0193 | Value in a placeholder whose type does not implement `Display` (`Debug` for `{:?}`) |
| E0194 | A format call with arguments whose format string is not a string literal |
| E0200 | `any` or `impl` applied to something that is not an interface (a type, a type parameter, or a built-in bound such as `Copy`) |
| E0201 | `impl Interface` outside a parameter list (a return type, a local annotation, a field, an element, a bound, or an `extern` signature) |
| E0202 | An interface that cannot be used as `any` because it is not object-safe |
| E0204 | Boxing a type whose implementation of the interface is compiler-provided (`Display`/`Debug` for primitives and built-in containers), so there is no function to dispatch to |
| E0210 | Invalid `error` declaration: a `from` variant that is not a tuple variant with exactly one field, `from` on a generic error enum, or a message placeholder that does not name a payload field |
| E0211 | Two variants of one error enum convert from the same source type |
| E0212 | An error conversion is missing: `?` between two error types with no `From` block, an implicit conversion into `Error` from a type that does not implement `Error`, or `.context` on a `Result` whose error type is not an error type |
| E0213 | Invalid default argument: a default before a required parameter, on a `mut` parameter, in an interface signature or an `extern` declaration, depending on a type parameter, or not a constant expression |
| E0214 | Invalid named argument: a name no parameter has, a parameter named more than once, a positional argument after a named one, or a name on a callee that takes none |
| E0215 | Warning: an extension method is shadowed by a method of its receiver type; calls use the method |
| E0216 | Invalid extension method declaration: no `self` first parameter, a receiver path with type arguments on a segment other than the type, a `where` clause naming something that is not one of its type parameters, or an extension imported under another name |
| E0220 | Invalid comptime construct: a run-time local, `self`, or capture used in a compile-time context; `comptime` on a parameter that cannot take it (`mut`/`own`, `self`, an interface method or its implementation, an `extern` declaration); a comptime-parameter function used as a value; `break`/`continue` leaving a `comptime for`; a `mut` `comptime for` binding; or a `comptime assert` message that is not a string literal |
| E0221 | A compile-time value whose type cannot be materialized in the program (a `List`, `Map`, `Heap`, `any`, closure, reference, slice, C type, or iterator, or an aggregate holding one) |
| E0222 | Compile-time evaluation panicked |
| E0223 | A `comptime assert` failed |
| E0224 | An impure operation (output, files, the environment, processes, or C) during compile-time evaluation |
| E0225 | Compile-time evaluation exceeded its step budget, the comptime call depth, or the specialization limit of a function with comptime parameters |
| E0230 | Invalid reflection: `Type.fields` or `Type.variants` on a type that is neither a struct nor an enum (a number, `bool`, `str`, a tuple, ...) |
| E0231 | A name the reflection members reserve (`name`, `size`, `align`, `fields`, `variants`) used for an associated function or an enum variant |
| E0232 | Invalid computed field access `x.{name}`: the name is not a string literal or the `name` of a `comptime for` variable over `Type.fields` |
| E0233 | Invalid derive declaration or use: a user derive named like a built-in derive, or `derives (Name)` with no interface `Name` in scope |
| E0240 | `embed` could not read its file: it is missing, unreadable, or not valid UTF-8 text |
| E0241 | Invalid `embed` use: the path is not a string literal, is absolute, or has `..`, or the expected `[u8; N]` is not the file's length |
| E0250 | Invalid const generic declaration or argument: a type where a value is expected or the other way round, a const parameter of a type that is not an integer, `bool`, or `char`, or a value that does not fit |
| E0251 | A const expression (an array length or const argument) that cannot be computed: an unknown name, overflow, a division by zero, or a length out of range |
| E0252 | A const generic argument that cannot be inferred from the call |
| E0260 | Invalid build configuration: a dependency has a `build.bd` that the entry package's `[build] allow` list does not name, or the list names a package that is not a dependency |
| E0261 | A build script failed: it did not compile, could not be run, or ended with a non-zero status |
| E0262 | Invalid generated modules: a generated path is not a module name, a generated module repeats a source module's name, or the output directory cannot be read |
| E0270 | An allocator-aware container whose allocator type does not implement `Allocator` |
| E0271 | Invalid default type parameter: a parameter without a default after one with a default, a const parameter's default, or a default on a declaration that takes none (a function, an interface, or a type alias) |
| E0272 | A container with a stored allocator created with `new`, `from_iter`, or `from`, which have no allocator value to store; it is made with `new_in` |

### Codes without a compile-fail test

Every code in the registry above needs a `//~` marker in at least one conformance test. A code that cannot be
witnessed by a compile-fail test (for example, a rule only a passing test can show) is listed here with the reason;
`tests/check_tests.py` fails when a registered code is in neither place, and when a row
here names an unknown code, a code that already has a test, or no reason.

| Code | Why it has no compile-fail test |
| --- | --- |
| E0215 | A warning, so compilation continues. `m18_extension/shadowed_method.bd`, `shadowed_builtin_method.bd`, and `shadowed_interface_method.bd` assert it with `//@ stderr: warning[E0215]`. |

## Decisions the tests encode

These were the 16 open assumptions of the original suite. Issue #21 settled them in sub-issues #22-#40: each is now a
language decision recorded in `spec/`, and the tests below (and the decision tests in the next section) are its
executable form. The design document leaves them open; where a rule says "deviation", stage 0 deliberately differs.

1. **Call sites.** A named `mut` parameter is passed with a `mut` marker (`push(mut list, 1)`); a missing marker is
   E0114 and a marker on a non-`mut` (including `own`) parameter is E0115. An `own` parameter is passed plainly and
   moves the value. A read-only parameter is passed plainly and borrows. A `mut self` receiver takes no marker (a
   marker there is a syntax error, E0001). `spec/grammar.md` 8.4, `spec/memory-rules.md` CV2-CV4.
2. **One implicit coercion.** A `String` passed where `str` is expected is borrowed as a `str` and stays usable.
   This is the only implicit conversion, and it applies only to argument-like positions (call arguments, `Some`/`Ok`/
   `Err` payloads, `assert_eq`, `mem.replace`); a `let`, assignment, return, array or tuple element, or comparison
   does not convert. There are no `&mut T`-to-`&T`, `&T`-to-`&mut T`, `&String`-to-`str`, or user-defined conversions.
   `spec/memory-rules.md` IC1.
3. **Views are copyable.** `str`, `[T]`, `&T`, and `CPtr<T>` can be used many times without a derive. `&mut T` cannot
   be copied: assigning it moves it (E0100), while passing it to `&mut` or dereferencing reborrows it.
   `spec/memory-rules.md` RF4.
4. **String literals are static views.** A function may return a literal as `str` whatever its parameters are, in a
   tail expression, `return`, block, `if` branch, or `match` arm. A `str` constant whose value is a literal is static
   too. Any other returned view or reference must derive from exactly one parameter.
   `spec/memory-rules.md` RT1-RT4.
5. **Pattern bindings** from a borrowed scrutinee are read-only places usable like plain values (no explicit `&`):
   assigning to one is E0005 and moving out of one is E0101. `*h` dereferences a `Heap<T>`; on an owned `Heap<T>`,
   `*h` moves the whole pointee out and frees the allocation, and a field of the pointee (`(*h).a`) is a tracked
   partial move (issue #58). `spec/memory-rules.md` MV6, MV8.
6. **Loops.** `for x in list` (and arrays and slices) borrows the collection for the whole loop, so it is still usable
   afterwards, and gives a read-only element place usable like a value. Maps use `for (k, v) in map` (also a borrow).
   `for c in s.chars()`. Mutating the collection in the body is E0111. Ranges are ordinary values of
   `Range<T>`/`RangeInclusive<T>` (issue #53), iterated by `for i in r` without borrowing `r`.
   `spec/memory-rules.md` IT1-IT3. `for x in mut xs` (issue #709) borrows exclusively and yields `&mut T`;
   `spec/memory-rules.md` IT15.
7. **Library names and signatures.** The surface is written down with signatures in `spec/stdlib.md`: numeric
   methods, `String` and `str` methods and slicing, `List`, `Map`, `Heap`, `Option`/`Result`, file, process, and
   environment functions, the C bridge names, and `unreachable()`. Unknown names are E0002 and wrong arities E0004.
8. **`main`** takes no parameters and returns unit in stage 0; either violation is E0003. Stage 1's
   `fn main(): Result<...>` (m16) prints the returned error chain to stderr and exits non-zero.
   `spec/stage0-subset.md`, `spec/stage1-subset.md`.
9. **Panics** print the message, file, line, and column to stderr, flush stdout first, and abort with a non-zero
   status; no drops run. `spec/memory-rules.md` DR10.
10. **File I/O is binary-safe** on Windows (no newline translation) and treats text as UTF-8 bytes without
    normalization. `spec/stdlib.md`.
11. **`Drop`** is `Type: Drop { fn drop(mut self) { ... } }` and nothing else: `mut self` and a unit return are
    required (E0003), and `Drop` cannot be derived (E0002) or combined with `Copy` (E0130).
    `spec/grammar.md`, `spec/memory-rules.md` DR8.
12. **Integer literals** have no suffixes (E0001 if one is written). Their type comes from context, else the other
    operand of a binary operator, else `i32` (`f64` for floats). `/` and `%` truncate toward zero.
    `spec/grammar.md`, `spec/stage0-subset.md`.
13. **Syntax shape.** Struct fields and enum variants may be separated by a comma or a line break; the tests and the
    recommended style put one per line, and the parser accepts both. Struct literals use commas. Items need no
    separators. `spec/grammar.md` "Declaration layout".
14. **Temporaries** are dropped in reverse creation order at the end of the statement that created them, and reading a
    field of a temporary does not extend its life. A block's tail-expression temporaries drop before its locals.
    `spec/memory-rules.md` DR5.
15. **Derives** provide `hash(self): u64` (FNV-1a, exact values pinned), a static `default()` for structs, and the
    documented `Eq`/`Ord` field and variant orders. `spec/derives.md`.
16. **Exclusivity works on places.** A place is a local or parameter followed by struct fields (`p.a`, `o.inner.b`).
    Different places do not overlap, so `swap_vals(mut p.a, mut p.b)` is allowed. A place never continues through
    indexing, a `Heap` dereference, a reference dereference, or a method call, so `xs[0].a` and `xs[1].a` are one
    place and are rejected (E0110), as is a whole value together with one of its fields.
    `spec/memory-rules.md` EX1-EX3.

## Stage 1 decisions

These extend the list above; they are settled in `spec/` and witnessed by the stage 1 tests.

17. **Closures.** A closure is `|a, b| expr` and has one of the compiler-known interface types `Fn(A, B): R`,
    `FnMut(A, B): R`, or `FnOnce(A, B): R`. It captures whole variables: by borrow when used in place, by move when
    an owned closure type is expected (a written annotation, field, return, or closure-interface parameter). A
    closure with a borrow capture is second-class and cannot escape; its captures are loans held until the closure's
    last use. Its kind follows the body: `FnOnce` if it moves a captured value out, `FnMut` if it mutates one, else
    `Fn`; a value may be used at a lower capability (`Fn` as `FnMut` as `FnOnce`). Calling borrows `Fn` shared,
    `FnMut` exclusively, and moves `FnOnce`. The representation is a boxed capture record behind a code pointer, so
    closure values are never `Copy`, `Clone`, or derived. A non-generic function, a built-in function, or `Some` is
    a value of a closure type when the context expects one: it becomes a captureless `Fn` closure (CL11).
    `spec/grammar.md` section 4, `spec/memory-rules.md` CL1-CL11, `tests/m12_closures/`.

18. **Iteration.** `std` declares `interface Iterator<T> { fn next(mut self): Option<T> }` and the marker
    `interface Iterable<T> { }`. The compiler knows the protocol for arrays, slices, `List`, `Map`, `Chars`, and
    ranges: `iter()` borrows and yields `T` when it is `Copy` and `&T` otherwise (a map yields `(&K, &V)`), while
    `into_iter()` consumes and yields owned items (a map yields `(K, V)`). `for` over a user type uses `iter()` when
    the type is `Iterable<T>` and the iterator value directly when it is an `Iterator<T>`; a type that is both is
    iterated through `iter()`, because `for` borrows its source. The loop lowers to `let mut __iter = e; loop { match
    __iter.next() { Some(pattern) => body, None => break } }`, keeps the stage 0 binding/drop/`break` semantics, and
    requires the pattern to match every item. The lazy adapters (`map`, `filter`, `take`, `skip`, `fold`, `count`,
    `any`, `all`, `find`, `to_list`) are default methods that allocate nothing, and the eager `List` methods borrow
    the list and return owned results. A chain is second-class and may not be stored (E0120) or kept past a
    temporary's statement (E0123). `spec/iterators.md` IT4-IT12, `tests/m13_iterators/`.
19. **Formatting is checked and lowered at compile time.** A string literal's `{...}` placeholders are part of the
    type-checked program: `{expr}` holds an expression evaluated where the string is, `{}` and `{n}` name the
    arguments of the enclosing `format`/`print`/`println`/`eprint`/`eprintln` call, and `{{`/`}}` are literal
    braces. The checker reports a missing argument (E0191), an unused one (E0192), a value that does not implement
    `Display`/`Debug` (E0193), a specifier that does not apply (E0190), and a call with arguments whose format
    string is not a literal (E0194); the compiler then lowers the string to `Formatter` calls, so no format string
    is parsed at run time. An argument used by two placeholders is evaluated once (a place by borrowing, a `Copy`
    value by copying). `Display` and `Debug` are ordinary `std` interfaces implemented by the compiler for the
    built-in types and by an impl block (or `derives (Debug)`) for a user type. A one-argument output call takes no
    format string, so stage 0 calls are unchanged. `spec/format.md` FM1-FM10, `tests/m14_format/`.
20. **Existential types.** `any Interface` is an owned two-word heap box `{data, vtable}`; a concrete value is boxed
    implicitly wherever an expected type is known (an annotation, an argument, a return, a field, an element, a
    `Some`/`Ok`/`Err` payload, a match arm). `any I` satisfies `I` and every interface `I requires`, is never
    `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, or `Default`, and calls `I`'s methods through the vtable (so `any Display`
    prints through it). It is allowed only for an object-safe `I`: no generic method, no method without a `self`
    receiver, no `own self`, no `Self` outside the receiver, no view return, and object-safe `requires` interfaces
    (E0202). `impl Interface` is accepted only in a parameter list (E0201 anywhere else) and is sugar for one hidden
    generic parameter with the interface as its bound. `x.is<T>(): bool` and `x.downcast<T>(): Option<T>` are
    compiler-known on `any I`, written with member type arguments; the downcast consumes the box. The boxed type
    must implement the interface (E0141) and be first-class (E0120); boxing a primitive's compiler-provided
    `Display`/`Debug` implementation is E0204, because there is no function to dispatch to.
    `spec/existentials.md` AN1-AN12, `tests/m15_any/`.

21. **Errors.** `error enum` is an ordinary enum plus generated impls: `Display` from each variant's message
    template (whose placeholders name payload fields), `Error` (the std marker interface, which requires
    `Display`), and one `From<Source>` per `from` variant (a tuple variant with exactly one field). `Result<T>`
    is `Result<T, Error>`, where `std.Error` is an owned box (`any Error`) plus the recorded trace frames. An
    error type is boxed implicitly wherever `Error` is expected; `?` converts through `from` or into the dynamic
    error; `.context("...")` wraps the source and keeps the chain; `is`/`downcast` inspect the box. Debug builds
    record creation and propagation frames; release builds record none. `fn main(): Result<T>` prints an `Err`
    with `Display` to stderr and exits 1. In a `burdin test` build, `assert`/`assert_eq` return a typed
    `AssertError` carrying the call text, the values, and the location, and the test binary prints it when the
    test fails.
    `spec/errors.md` ERR1-ERR10, `tests/m16_errors/`.

22. **Default and named arguments.** A parameter written `name: Type = expr` may be omitted at the call site. A
    default is a constant expression checked once at the declaration (parameters and `self` are not in scope),
    it cannot depend on a type parameter, and only trailing parameters may have one; a `mut` parameter, an
    interface method signature, and an `extern` declaration cannot declare one (E0213). A call may write
    `name: value` for any function, method, interface method, associated function, or generic function/method;
    positional arguments fill the leading parameters and may not follow a named one, an unknown or repeated
    name is E0214, and a missing required argument is E0004. Built-in functions and methods, constructors,
    closure values, and `extern` declarations take no named arguments. Every argument is evaluated left to
    right in parameter order, a default in the position of its parameter.
    `spec/arguments.md` AG1-AG7, `tests/m17_arguments/`.

23. **Extension methods.** `fn Type.name(self, ...)` at module scope declares a method on any type name in scope,
    local, imported, or built-in; the receiver's type arguments declare its type parameters (`fn List<T>.sum(self)`),
    and a `<...>` after the method name adds method-level ones. `self` must be first, and `mut self`/`own self`
    follow the method conventions. An extension is visible in its declaring module and in modules that import it by
    name (`use text.shout`, `use text.{shout}`); importing the module, an alias, or `use text.other` does not bring
    it, and `priv` is package-private (E0150). `x.f()` tries the type's methods first (inherent and interface, and
    the methods the compiler provides for built-in types) and a visible extension last; a same-named extension
    always loses and gets E0215 at its declaration. A second extension for the same receiver in one module, and a
    second import of one extension name, are E0007; an extension without `self`, a malformed receiver path, a
    `where` naming a non-parameter, and an import under another name are E0216.
    `spec/extensions.md` EX1-EX8, `tests/m18_extension/`.

24. **Operator overloading and `Index`.** The operator interfaces are the ordinary `std` interfaces `Add`, `Sub`,
    `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Neg`, `Not`, `PartialEq`, `PartialOrd`, `Index<T>`, and
    `IndexMut<T>` in `std.ops`, visible through the prelude. Both operands of an operator have the same type, and a
    type implements each interface at most once. The stage 0 built-in rule and a derived `Eq`/`Ord` are always tried
    first; a user struct or enum then uses its interface implementation, and a type parameter its bounds. Comparisons
    use the new `PartialEq`/`PartialOrd` interfaces, not the derive-only `Eq`/`Ord`, so every stage 0 derive keeps
    its exact behavior. `Index<T>`/`IndexMut<T>` name the element type, take a `usize`, and return `&T`/`&mut T`, so
    `x[i]` is a place; a write (assignment, `&mut x[i]`, `mut x[i]`, or an index inside a written place) selects
    `index_mut` and needs a mutable receiver. A literal never adopts a user type, so `v + 1` is E0003. A missing
    operator or index implementation is E0141, `index_mut` through an immutable place is E0113, and moving a
    non-`Copy` element out is E0101. Compound assignment and the shifts are not overloadable; `&&`, `||`, and `=`
    remain non-overloadable.
    `spec/operators.md` OP1-OP13, `tests/m19_operators/`.

25. **Comptime.** The comptime engine interprets the checked typed tree; there is no MIR (issue #158, #159 not
    chosen). It runs after the memory checks and before code generation, in its own crate, and behaves like
    compiled code of the same profile (overflow panics in debug and wraps in release). `comptime E` and `comptime
    { ... }` evaluate while compiling and may use only constants, `comptime` parameters, `comptime for` bindings,
    and their own locals (E0220). The value must be materializable: scalars, `str`, `String`, and tuples, arrays,
    structs, enums, `Option`, `Result`, and ranges of those (E0221). `comptime name: T` parameters specialize each
    call per distinct argument value (at most 1024 per function). `comptime if` keeps only the selected branch, and
    `comptime for` unrolls its body over a range, array, or `List`. A `const` initializer is any materializable
    expression, evaluated once in declaration order; a cycle through calls is E0009. `comptime assert(cond, "msg")`
    fails with E0223. Evaluation is pure (E0224 for output, files, the environment, processes, and C), panics are
    E0222, and each evaluation has a budget of 10,000,000 steps (`--comptime-steps N`), a call depth of 512, and
    E0225 beyond any limit. `target.os()` and `target.arch()` name the compilation target.
    `spec/comptime.md` CT1-CT14, `tests/m20_comptime/`.

26. **Reflection and user derives.** Reflection is five compile-time members of every type: `Type.name` (`str`),
    `.size` and `.align` (`usize`), `.fields` (an array of the standard library's `FieldInfo`), and `.variants` (an
    array of `VariantInfo`). A struct has no variants and an enum no fields (both empty arrays), any other type has
    neither (E0230), and a function without `self` or a variant with one of the five names is E0231.
    `comptime for item in Type.fields` (or `.variants`) is unrolled while the body is checked, with `item.name`,
    `item.type_name`, `item.index`, `item.arity` as literals. `x.{name}` is a field access whose name is a string
    literal or `item.name` (E0232 otherwise). `derive Name { fn ... }` declares the methods of a user derive, and
    `derives (Name)` applies it as the impl block `Type: Name`, with `where P: Name` on each type parameter. A derive
    cannot take a built-in derive's name (E0233). `spec/reflection.md` RFL1-RFL10, `tests/m21_reflect/`.

27. **Const generics and `embed`.** `const N: usize` (an integer, `bool`, or `char` type) is a generic parameter of
    a struct, enum, or function; its argument is a literal, a name, or `{ expr }`. A const argument is a value in
    `Ty::Const`, so instances, inference, and keys work as for types. Array lengths (`[T; R * C]`, `[0; N]`) are
    const expressions over numbers, const parameters, and constants (E0250 for a type/value mix-up, E0251 when the
    expression cannot be computed, E0252 for an argument nothing determines). `embed("path")` is the contents of a
    file read at compile time, as `str` or, with the expected type `[u8; N]`, as bytes; the file is a build input
    (`--emit-deps`), a missing one is E0240 and a bad path or length E0241. `spec/const-generics.md` CG1-CG9,
    `spec/comptime.md` CT15, `tests/m21_reflect/`.

28. **Generic aliases, impls for specific instances, and built-in interfaces on generic types.** `type Pair<T> =
    (T, T)` is a template expanded where it is used; an alias is the aliased type, so an `impl` block written
    through one is E0003 (`spec/type-aliases.md` AL1-AL6). The arguments of an impl block are a pattern: every name
    in them that is not a type stands for what it matches (`Stack<i32>`, `Pair<i32, B>`, `Holder<List<T>>`), and
    the impl covers the instances it matches. The later of two overlapping impls of one interface is E0143;
    two methods of one name on one instance are E0007. `Drop` on a generic type covers every instance, so its
    header has no concrete arguments and no `where` clause (E0131). A derive on a generic type holds for the
    instances whose parts allow it, and a built-in interface written by hand is still E9000 (#402).
    `spec/impls.md` IM1-IM9, `tests/m21_reflect/`.

29. **Build scripts.** `build.bd` sits next to `burdin.toml` and is compiled and run by the driver before the
    package's sources are read. The package's own script is trusted; a dependency's script runs only when the
    **entry package's** manifest names it in `[build] allow`, because running a script is arbitrary code execution
    (E0260 otherwise, and for an allow entry that names no dependency). The script is an ordinary Burdin program
    compiled in the build's profile and run with the package directory as its working directory and `argv[1]`,
    the output directory as `argv[2]`, and the driver's own directory first on `PATH`. Its stdout and stderr go to
    the compiler's stderr. The output directory is `<package>/target/burdin/<profile>`; the driver clears its `src/`
    before a run, and every `.bd` file under it becomes a module of the package under its path relative to `src/`
    (E0262 when it repeats a source module). A script failure is E0261 at the entry package's first source line. The
    driver reruns the script only when the script, the manifest, the compiler binary, or an input changed; the
    inputs are the files the script lists in `<out>/inputs` plus the files its `embed` calls read, and the cache key
    records their contents per package and profile. `burdin bindgen <header> --out <file>` is the minimal C
    declaration generator a script may invoke: functions as `extern "C"`, `repr(C)` structs, typedefs, enums and
    object-like `#define`s. `spec/build-scripts.md` BS1-BS8, `tests/m22_build/`.

30. **Allocators, arenas, and `Shared`.** The allocator is an ordinary value and an ordinary generic parameter:
    `std.alloc.Allocator` has `allocate`, `reallocate`, and `deallocate` (all `mut self`), and
    `List<T, A = Global>`, `Map<K, V, A = Global>`, and `Heap<T, A = Global>` take it as their last type
    argument. `Global` is zero-sized and stored nowhere, so the stage 0 spellings and layouts are unchanged;
    any other allocator (`Arena`) is stored in the container as a field the container owns. A container with a
    stored allocator is made with `new_in`, which moves the allocator value in; `new` exists only for `Global`,
    and using it where an allocator would have to be stored is E0272; an allocator argument or value that does
    not implement `Allocator` is E0270. A type may give its trailing type parameters defaults
    (`struct Buffer<A: Allocator = Global>`), resolved where the type is declared; a non-trailing default, a
    const parameter's default, and a default on a function, interface, or alias are E0271. An inherent or
    `Drop` impl written with no arguments covers every instance, defaults included. `Arena` is a first-class
    bump allocator with 4096-byte blocks, `bytes_allocated`, and `reset`; dropping it frees every block, and it
    is not second-class, because every safe way to get an arena-backed buffer makes the container own the
    arena. Dropping a container frees its buffer through its allocator and then drops the allocator; cloning
    requires the allocator to be `Clone` and allocates the copy through the cloned value; owning iterators
    carry the allocator. `Shared<T>` is a non-atomic reference-counted cell `{count, value}` from `Global`:
    `.clone()` shares it, `.get()` reads it, `.strong_count()` counts, the last owner drops the value and frees
    the cell; it is `Clone` for every `T` and never `Copy`, and stage 2 makes the count atomic and adds `Send`.
    A `burdin test` build uses a checking allocator with a 40-byte header: a double free panics, and an
    allocation still live when an otherwise passing test returns is reported (`test leak: ...`,
    `allocated at path:line:column`) and fails that test. At compile time containers are modeled, `Shared` is a
    transient model of its value, and explicit allocation is impure (E0224).
    `spec/allocators.md` AL1-AL10, `tests/m23_allocators/`, `tests/runner/leaks/`.

## Review of the expected outputs

The expected outputs and diagnostics were written by hand before the compiler existed, so a passing test only
proves that the implementation matches the expectation. Issue #20 reviewed every test against
[the design](../Burdin%20Language%20Design.md), [the grammar](../spec/grammar.md),
[the memory rules](../spec/memory-rules.md), [the stage 0 subset](../spec/stage0-subset.md), and the decisions
above on 2026-11-14. Each milestone folder was read in full. No expectation was found to encode an accidental
behavior, so no test was changed; the behavior the tests pin where the design is silent, defers a feature, or
deliberately differs is recorded below.

The counts in the table are the 134 tests issue #20 reviewed. Issue #21 then added the decision tests in the next
section; the suite is larger than the table.

| Folder | Tests | What was reviewed |
| --- | --- | --- |
| `m01_hello` | 5 | comments and layout, escapes, E0001 syntax errors |
| `m02_basics` | 10 | arithmetic and casts, short-circuiting, control flow, recursion, the three abort paths |
| `m03_typecheck` | 10 | inference from context, `as`, E0002/E0003/E0004/E0005/E0008 |
| `m04_data` | 9 | structs, methods, tuples, aliases, enums, patterns, E0006 exhaustiveness |
| `m05_ownership` | 18 | moves, reassignment, partial and conditional moves, drop order and flags, `Heap` |
| `m06_conventions` | 43 | `mut`/`own`, exclusivity, borrows, views, return origins, E0120/E0121/E0122 |
| `m07_generics` | 12 | monomorphization, bounds, interfaces, defaults, `requires`, derives, E0140/E0141/E0142 |
| `m08_modules` | 6 | files and folders as modules, `use`, `priv` in and across packages, `extern "C"`, E0150/E0161 |
| `m09_stdlib` | 15 | `Option`/`Result`/`?`, `String`, `List`, `Map`, file I/O, process and environment |
| `m10_test_runner` | 5 | `test` blocks, counts, failed names, abort recovery, stripping from builds |
| `m11_llvm` | 1 | the LLVM C API through `extern "C"` and `CPtr` |
| **Total** | **134** | |

### Decision tests (issue #21)

The tests added when the 16 decisions were settled. Every decision also links to the `spec/` section that states the
rule; the existing tests listed in that section remain the main coverage, and these fill the gaps.

| Decision | Tests added |
| --- | --- |
| 1. Call-site markers (#22) | `m06_conventions/call_site_markers`, `receiver_mut_marker`, `own_mut_marker` |
| 2. One implicit conversion (#23) | `m06_conventions/string_str_conversion`, `let_string_to_str`, `mut_ref_to_shared_ref`, `ref_string_to_str`, `string_to_slice` |
| 3. Views are copyable (#24) | `m06_conventions/views_are_copy`, `mut_ref_move_then_use` |
| 4. Static literals (#25) | `m06_conventions/static_literals`, `const_is_not_a_place` |
| 5. Pattern bindings and `*h` (#26) | `m05_ownership/heap_move_whole_then_field`, `match_borrowed_binding_readonly`, `match_borrowed_binding_move` |
| 6. `for` borrows; ranges (#27) | `m06_conventions/for_borrow_forms`, `range_forms`, `for_mutate_array`, `for_mutate_map`, `for_binding_readonly` |
| 7a. Numeric and `String` names (#28) | `m09_stdlib/string_order`, `unknown_stdlib_names` |
| 7b. `List`, `Map`, `Heap` names (#29) | `m09_stdlib/unknown_stdlib_names` (and the existing `list_ops`, `map_ops`) |
| 7c. File, process, environment names (#30) | `m09_stdlib/file_io_binary`, `unreachable_aborts`, `unknown_stdlib_names` |
| 7d. C bridge names (#31) | `m08_modules/cptr_null_arguments`, `cstr_roundtrip`, `extern_signature` |
| 8. `main` returns unit (#32) | `m02_basics/main_returns_value`, `main_takes_parameter` |
| 9. Panic output and flush (#33) | `m02_basics/panic_reports_location`, `panic_runs_no_drops` |
| 10. Binary-safe file I/O (#34) | `m09_stdlib/file_io_binary` |
| 11. `Drop` declaration (#35) | `m05_ownership/drop_signature`, `drop_derive` |
| 12. Literals and division (#36) | `m02_basics/division_truncates`, `literal_suffix`, `literal_inference` |
| 13. Declaration layout (#37) | `m04_data/declaration_layout` |
| 14. Temporary drop timing (#38) | `m05_ownership/drop_block_tail` |
| 15. Derive outputs (#39) | `m07_generics/derive_values` |
| 16. Exclusivity places (#40) | the existing `m06_conventions/exclusivity_*` and `disjoint_field_borrows` |

Settling them also fixed four implementation gaps: `Drop` now rejects a read-only or `own` receiver (E0003),
`CPtr.null` rejects arguments (E0004), `<`/`<=`/`>`/`>=` work on `str` and `String` (they previously crashed in
code generation; `m09_stdlib/string_order`), and an invalid `extern "C"` type or convention has its own code, E0162
(`m08_modules/extern_signature`).

## Intentional choices and deviations

"Design" means [the design document](../Burdin%20Language%20Design.md). A **deviation** is a place where stage 0
deliberately does something the design does not, or does not yet, require. The other entries are choices the design
leaves open that the tests now pin. Features that are only deferred have no tests here; the complete list is in
[the stage 0 subset](../spec/stage0-subset.md). Code quoted below comes from tests that compile and run; the one
illustrative fragment, the `Drop` declaration with `...`, is marked as such in decision 11.

### Language and memory

- **Read-only parameters are borrowed; returning one by value needs `own`.** The design's
  `fn max<T: Ord>(a: T, b: T): T` sketches a signature, but a read-only parameter is a borrow in stage 0, so the test
  writes `fn max_of<T: Ord>(own a: T, own b: T): T` (`m07_generics/generic_functions.bd`). Returning a read-only
  parameter is E0101 (`m06_conventions/move_out_of_readonly_param.bd`; CV1, RT2).
- **One implicit conversion (deviation).** A `String` passed where `str` is expected is borrowed as a `str`; no other
  conversion is implicit, and it applies only to argument-like positions. The design says "no implicit conversions".
  Decision 2; [memory rules](../spec/memory-rules.md) IC1; `m06_conventions/str_views.bd`,
  `m06_conventions/string_str_conversion.bd`, `m09_stdlib/string_ops.bd`.
- **Views are copyable without deriving `Copy` (deviation).** `&T`, `str`, `[T]`, and `CPtr<T>` are Copy; `&mut T` is
  not. The design makes copying explicit except for `Copy` types and does not derive `Copy` on these. Decision 3;
  `m06_conventions/references_basic.bd`, `views_are_copy.bd`, `str_views.bd`, `slice_views.bd`; RF4.
- **String literals are static views (an exception).** A function may return a literal from any parameter list, the
  one exception to the single-source rule, and a `str` constant of a literal is static too. Decision 4;
  `m06_conventions/return_literal_ok.bd`, `static_literals.bd`; RT4.
- **Borrowed pattern bindings are read-only places used like values.** No `&` is written on a binding from a borrowed
  scrutinee, and `*h` on an owned `Heap<T>` moves the whole pointee out and frees it. A field of the pointee
  (`(*h).a`) is tracked as a partial move of the `Heap` (issue #58). Decision 5;
  `m06_conventions/match_borrowed_binding.bd`, `m05_ownership/match_borrowed_binding_readonly.bd`,
  `heap_tree_eval.bd`, `heap_move_whole_then_field.bd`, `move_heap_field.bd`; MV6, MV8.
- **`for` borrows its collection for the whole loop.** The design's `for` runs through a borrowing iterator, and
  stage 0 makes the borrow last until the loop ends, so a `mut` call on the same collection in the body is E0111.
  Decision 6; `m09_stdlib/for_borrows_list.bd`, `m06_conventions/for_borrow_forms.bd`. Appending while iterating
  returns with the stage 1 iteration protocol (`HANDOFF.md` known problem 19).
- **Owned parameters drop after locals, in reverse declaration order.** The design fixes the order of locals and
  fields but not parameters. DR3; `m05_ownership/drop_params_and_let_underscore.bd`.
- **Derived `Ord` orders enum variants by declaration.** The design's `Ord` rule covers field order; it does not
  define variant order. `m07_generics/derives.bd` pins `Low < Mid < High`.
- **`Drop` is spelled as an interface block.** `Type: Drop { fn drop(mut self) { ... } }`. `mut self` and a unit
  return are required (E0003), and `Drop` cannot be derived. Decision 11; every `m05_ownership` drop test,
  `drop_signature.bd`, `drop_derive.bd`.
- **Derive method forms.** `Hash` provides `hash(self): u64` with the exact FNV-1a values in
  [the derives spec](../spec/derives.md), `Default` a static `default()` for structs, and `Eq`/`Ord` the comparison
  operators with fields in declaration order and variants in declaration order. Decision 15;
  `m07_generics/derives.bd`, `derive_values.bd`.
- **No MIR (deviation).** The design's compiler architecture has a MIR; stage 0 checks the typed tree (`burdin_tir`),
  and `burdin_mir` is planned for stage 1 (`spec/stage1-subset.md`, issue #493). The header and RF2 of
  [the memory rules](../spec/memory-rules.md) now say so.
- **Borrow liveness is conservative (deviation in precision).** The rule is "until the last use", but stage 0
  computes it per top-level statement, so any use inside a loop keeps a loan live for the whole loop. This rejects
  some programs the design would allow; it never accepts a conflicting one. RF2, `HANDOFF.md`;
  `m06_conventions/borrow_ends_at_last_use.bd` pins the last-use rule.
- **`main` returns unit (deviation).** A parameter or a non-unit return type is E0003. `fn main(): Result<()>` and
  printing a returned error are stage 1 (m16 prints the error chain to stderr and exits non-zero). Decision 8;
  every `//@ run` test, `m02_basics/main_returns_value.bd`, `main_takes_parameter.bd`.
- **`unsafe` gates C calls and every `CPtr` value.** Calling an `extern` function, an `unsafe fn`, or an `unsafe`
  interface method requires `unsafe`, and so does creating, holding, passing, moving, dereferencing, testing,
  comparing, offsetting, or casting a `CPtr`. Inside `unsafe` the memory rules still apply. US;
  `m08_modules/cptr_requires_unsafe.bd`, `cptr_operations.bd`, `unsafe_fn.bd`,
  `unsafe_fn_call_requires_unsafe.bd`, `m07_generics/unsafe_interface_method.bd`.
- **A stored view may not outlive its source.** A second-class value in a `let` or assignment may derive only from
  static data, a parameter, or a local of the same or an enclosing block; a temporary or an inner-block local is
  E0123. Match/`if let`/`for` pattern bindings are exempt because their source lives to the end of the statement.
  RT5; `m06_conventions/view_lifetime_ok.bd`, `view_of_earlier_local_ok.bd`,
  `view_outlives_temporary.bd`, `view_outlives_inner_local.bd`, `view_outlives_string.bd`.
- **Ranges are ordinary values.** `a..b` is `Range<T>`, `a..=b` is `RangeInclusive<T>`; the endpoints are integers
  of the same type, and the type is `Copy`, `Clone`, `Eq`, and droppable like a two-field struct. A stored range is
  evaluated once and iterated with `for i in r` without borrowing it. Issue #53; [standard library](../spec/stdlib.md),
  `m06_conventions/range_values.bd`, `range_forms.bd`, `m09_stdlib/for_borrow_forms.bd`.
- **Array and slice patterns are stage 1 (deviation).** The design lists arrays under destructuring; stage 0 matches
  structs, enums, tuples, literals, and ranges. No test uses array patterns (issue #486).
- **No `priv` marker on modules; a dependency's non-root modules are hidden (deviation).** The design allows `priv`
  on modules and checks that public signatures use public types. Stage 0 has no module item to mark, so the rule is
  positional: a dependency's root module is reachable as `use <package>` and its other modules are hidden unless the
  root re-exports them with `pub use` (`m08_modules/reexports`, `reexports_hidden`). `pub use` cannot widen a
  package-private item (E0150), and a public signature or field may not name a package-private type (E0150;
  `public_signature_leaks.bd`, `pub_use_private`).
- **`as` also converts `char` to an integer.** The design says `as` converts between numeric types; stage 0 follows
  Rust and allows `c as i32` (`m09_stdlib/tokenizer.bd`, `m02_basics/casts_and_numeric_methods.bd`).
- **No linter (deviation).** The design's warnings (`else` on a same-package enum, "use `.iter()`") are deferred with
  `burdin lint`. No test depends on a warning; `m04_data/match_patterns.bd` uses `else` only on integers.

### Errors and testing

- **Error types are real.** `Result<T>` is `Result<T, Error>` (a boxed dynamic error), and `Result<T, E>` names a
  typed error; `error enum` declarations generate their `Display`, `Error`, and `From` impls, `.context(...)`
  keeps a chain, and debug builds record trace frames. `spec/errors.md` ERR1-ERR10; `m16_errors/*`.
- **`?` converts through `from` or into the dynamic `Error`.** An `error enum` variant written `from` registers the
  conversion automatically, a hand-written `Type: From<Source>` block still works, and an error type converts into
  `Result<T>`'s dynamic error without a block (`m16_errors/error_from_conversion.bd`, `error_missing_conversion.bd`).
  The stage 0 hand-written example remains in `m09_stdlib/question_and_from.bd`.
- **`assert` has two modes.** In a normal build it panics; in a `burdin test` build the prelude binds
  `__test_assert`/`__test_assert_eq`, which build a typed `AssertError` carrying the call text, the values, and the
  location, so a failed test is reported with its message instead of aborting. The design describes the test form
  only. Decision 9, `spec/errors.md` ERR9, `m10_test_runner/*`, `m16_errors/assert_typed.bd`.
- **No leak-detecting allocator (deviation).** The design runs every test under one; allocators are stage 1, so no
  test asserts that a leaked allocation fails. `spec/stage0-subset.md`.
- **Panic wording is chosen by the runtime.** The design requires a message, the file, and the line; it does not fix
  the text. The abort tests match substrings (`division by zero`, `out of bounds`, `overflow`, the `panic` argument,
  and the file name). `m02_basics/*_aborts.bd`.

### Standard library and C bridge

- **Names and signatures are the tests' choices.** The design names some operations and says `Option`/`Result`
  follow Rust "and so on"; stage 0's exact surface is [the standard-library spec](../spec/stdlib.md), including the
  signatures the tests exercise. In particular `List.last` and `Map.get` return `Option<&T>`/`Option<&V>`, `List.pop`
  returns `Option<T>`, `List.remove` and `Map.remove` return owned values, and `List.contains` takes a value.
  Unknown names are E0002 and wrong arities E0004.
- **`String` length and slicing are byte-based.** `s.len()` is bytes and `s[a..b]` slices bytes; `chars()` iterates
  Unicode scalar values. The design says UTF-8 but does not define `len`. `m09_stdlib/string_ops.bd`,
  `m09_stdlib/file_io.bd`, `file_io_binary.bd`; decision 7.
- **`String` and `str` compare by bytes.** `==`/`!=` and `<`/`<=`/`>`/`>=` require both operands to have the same
  type; there is no conversion in an operator. `List<String>.sort()` uses the same order.
  `m09_stdlib/string_order.bd`, `string_ops.bd`; [standard library](../spec/stdlib.md).
- **`Map` is a hash table; its iteration order is unspecified.** The design names a hash map, and the implementation
  is open addressing with linear probing in the map's three-word buffer: a zero hash marks an empty slot, growth
  rehashes at a 3/4 load factor, and removal shifts the rest of the probe cluster back. Any type with `==` is a key
  (no `Hash` requirement): integers, floats, text, tuples, structs, enums, `Option`, `Result`, and ranges hash
  structurally. Iteration scans the slots, so tests sum or sort instead of depending on order
  (`m09_stdlib/map_ops.bd`, `map_key_types.bd`, `map_remove_and_clear.bd`, `map_many_keys.bd`, `word_count.bd`).
  `HANDOFF.md` known problem 14.
- **`Option` and `Result` expose a subset (deviation).** The tests use `unwrap`, `expect`, `is_some`, `is_none`,
  `orelse`, and `?` (and `is_ok`/`is_err` exist); the design's `map`, `and_then`, `unwrap_or`, `ok_or`, and the rest
  are not implemented until stage 1 (issue #481). `m09_stdlib/option_result_basics.bd`.
- **Only `wrapping_*` and `checked_*` exist.** The design also lists `saturating_*` and `overflowing_*`; the stage 0
  subset narrows the set. `m02_basics/casts_and_numeric_methods.bd`.
- **No `i128`/`u128` (deviation).** Stage 0 stops at `i64`/`u64`, as the stage 0 subset says; the design lists all of
  them. No test uses the wider types.
- **File I/O is binary-safe.** No newline translation on Windows and no UTF-8 normalization. Decision 10;
  `m09_stdlib/file_io.bd`, `file_io_binary.bd`.
- **`env.args()` puts the program name first.** The design names `std.env.args()` but not its contents. Decision 7;
  `m09_stdlib/env_args.bd`.
- **C bridge surface.** `CPtr<T>` (with `null`, `is_null`, load/store through `*p`, `==`/`!=`, offsets `p ± n` and
  `p - q`, and casts to/from integers and other `CPtr` types), `CChar` (`i8`), `CSize` (`usize`), `CVoid` (only
  behind a pointer), `CString` (with a deep `clone`), `CStr` (an alias for `CPtr<CChar>`), `CString.from(str)`,
  `.as_ptr()`, `CPtr.null()` (no arguments), and `CStr.to_string(ptr): String`. `struct Name repr(C)` has the C field
  order/padding/alignment and reports it with `Type.size()`/`Type.align()`. `extern "C"` parameters and results are
  integers, floats, and `CPtr`; `...` passes extra C-ABI arguments; `extern "C" fn` exports a Burdin function under
  its plain symbol. Aggregates cross the ABI only through `CPtr<T>` (by value is E0162), C `_Bool` is declared `u8`,
  and C `char` is `CChar`. Issues #72-#76, #489, #490; [standard library](../spec/stdlib.md);
  `m08_modules/cptr_operations.bd`, `c_abi_structs.bd`, `c_out_parameter.bd`, `cstring_clone.bd`,
  `export_c_abi`, `extern_signature.bd`, `export_c_abi_invalid.bd`.
- **`CStr` stays an alias and is not a view type (issue #83).** It is the same type as `CPtr<CChar>`, a
  first-class `Copy` value that may be stored in a field and returned without an origin rule. There is no implicit
  conversion in either direction (`str`/`String`/`CString` to `CStr` is E0003, and `CStr` to `str`/`String` is
  E0003), no string method on it (`p.to_string()` and `p.len()` are E0002), and only the explicit
  `CString.from(s).as_ptr()` and the associated `CStr.to_string(p)` conversions. The `CString` must outlive the
  pointer, because a `CPtr` is not a tracked view. [Standard library](../spec/stdlib.md), SC1;
  `m08_modules/cstr_is_an_alias.bd`, `cstr_no_implicit_conversion.bd`, `cstr_is_not_a_str.bd`.

### Not covered yet

These are absences, not deviations; they are listed so the next review does not mistake silence for a decision. No
test currently exercises: release-mode wrapping (the runner can only skip a test in release, not change its
expectation), out-of-range shift panics, a `main(): Result<()>` (stage 1), bounds panics for `List`/`Map`/slice
indexing (only arrays are covered), `priv` methods, `match` on `bool` or `char`, struct patterns that omit fields,
moving through a runtime index (E9000, assigned to Stage 1 in `spec/e9000-audit.md`), C function-pointer types and
`union` declarations, or passing a `repr(C)` struct by value across the C ABI (E0162).
