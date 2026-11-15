# Burdin stage 0 conformance tests

These programs are the target for the stage 0 compiler (Rust, LLVM backend). They come from
[the design document](../Burdin%20Language%20Design.md) and [the stage 0 subset](../spec/stage0-subset.md). The design
document's snippets are illustrative; these programs are the executable form. Each folder is one milestone; a
milestone is done when every test in its folder and in all earlier folders passes.

Anything the stage 0 subset defers (closures, iterators, interpolation, `any`, `error` declarations,
comptime, tasks, leak detection, allocator parameters) has **no tests here**.

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
| `//@ debug-only` | Skip in release profile (overflow checks). |
| `//@ link-lib: NAME` | Extra library to link. |
| `//@ undecided: text` | Skipped; documents an open design question. |

Runner rules: run each test in a fresh empty temporary directory as the working directory; compare
stdout after normalizing `\r\n` to `\n`; every `compile-fail` file must report all of its errors (no
stopping at the first error) and no others. The `test` item's attributes (`[ignore]`, `[timeout(ms)]`,
`[should_panic]`, `[should_fail]`) and the `burdin test` flags are specified in
[`spec/test-runner.md`](../spec/test-runner.md).

## Running the tests

```
tests/run-tests.sh                    # every test, debug profile
tests/run-tests.sh m05 m06            # only paths containing m05 or m06
tests/run-tests.sh -v drop_order      # show diffs and compiler output for failures
tests/run-tests.sh --release          # release profile (skips debug-only tests)
tests/run-tests.sh --list             # list tests and their kinds
BURDIN=/path/to/burdin tests/run-tests.sh # choose the compiler driver
tests/check-tests.sh                  # check directives and error codes (no compiler needed)
tests/check-ci.sh                     # check the CI workflow and the Rust toolchain pin (no compiler needed)
tests/check-platform.sh               # check the host triple and 64-bit isize/usize (needs a built driver)
tests/check-docs.sh                   # check documentation links and the driver contract (needs a built driver)
tests/check-debug-info.sh             # -g: DWARF, line table, and a gdb breakpoint when gdb is installed
tests/check-codegen-opt.sh            # drop-flag elimination, bounds-check elision, shared glue
tests/check-build-flags.sh            # --version, --strip's budget, --lto, and PGO
tests/check-parallel.sh               # parallel parsing and deterministic diagnostics
tests/check-diagnostics.sh            # check color, width, formats, error limits, and ordering (needs a built driver)
tests/check-error-docs.sh             # check the error-code reference and `burdin explain` (needs a built driver)
tests/check-test-runner.sh            # check `burdin test` attributes, parallelism, output, isolation, discovery
pwsh tests/check-tests.ps1            # the same check in PowerShell
```

CI runs every finished milestone in both profiles, so a milestone is done only when its folder passes in debug and
release. The release run skips tests marked `//@ debug-only`. The `burdin test` features themselves (attributes,
`-j`, output capture, isolation, `-p`/`--workspace`) are checked black-box by `tests/check-test-runner.sh`, with
fixtures under `tests/runner/` that carry no `//@` directives and are therefore not conformance tests.

The compiler and runtime work of issue #421 is covered in two places. The observable runtime behavior has conformance
tests: `m02_basics/panic_backtrace.bd` (both profiles), `m02_basics/panic_backtrace_names.bd` (debug-only),
`m02_basics/stack_overflow_aborts.bd`, and the existing flush tests
(`m02_basics/panic_flushes_stdout.bd`, `m09_stdlib/exit_code.bd`). The flags and code-generation effects have the
four scripts above: they read the `--emit-ir` text (drop flags, bounds checks, glue) and compare parallel and
sequential diagnostics. `BURDIN_JOBS=1` forces sequential parsing for a comparison; the default is the machine's
parallelism.

The runner finds the driver in `BURDIN`, then `target/{debug,release}/burdin`, then `PATH`. It expects `burdin build`,
`burdin run`, and `burdin test` to behave as documented in [the driver contract](../README.md#the-driver-contract) at
the repository root (summarized at the top of `run-tests.sh`): diagnostics as
`path:LINE:COL: error[E0000]: message`, and a `test result: N passed, M failed` line plus `FAIL <name>` lines from
`burdin test`. If the real driver differs, change the section marked in that file. The exit code is 0 only when every
test passes. On Windows, run it from Git Bash.

## Benchmarks

The compiler benchmarks are not part of the conformance run:

```
cargo bench -p burdin_bench                       # medians per stage and size
cargo bench -p burdin_bench -- --smoke            # the time budgets CI runs
cargo bench -p burdin_bench -- lex                # only cases whose name contains `lex`
cargo bench -p burdin_bench -- --save bench.txt   # record the medians as a baseline
cargo bench -p burdin_bench -- --baseline bench.txt   # compare; exit 1 when a case is over the threshold
tests/bench-regression.sh                         # the advisory check CI runs
```

`crates/burdin_bench` generates its inputs, so a size is the only parameter of a measurement. A baseline is a
plain text file of `case<TAB>median nanoseconds` lines, and `--threshold` sets the percentage (default 25) a case
may get slower before `--baseline` reports it. CI keeps the baseline in `actions/cache`, keyed by the runner OS
and architecture, the pinned toolchain, and `Cargo.lock`, so only numbers from the same kind of machine are
compared; a push to main records a new baseline, and a pull request compares against the last main run on that
platform. `HANDOFF.md` records the first baseline and the meaning of the `--smoke` budgets.

## Error codes

The registry the tests use. The compiler adopts these numbers (or the tests are updated). Every code also has a
user-facing entry in [the error-code reference](../docs/error-codes.md), which `tests/check-error-docs.sh` checks
against this table and whose examples it compiles; `burdin explain <code>` prints the entry.

| Code | Meaning |
| --- | --- |
| E0001 | Syntax error (lexing or parsing) |
| E0002 | Unknown name (variable, function, type, module, or imported item) |
| E0003 | Type mismatch |
| E0004 | Wrong number of arguments |
| E0005 | Assignment to an immutable binding, or mutation through a read-only parameter |
| E0006 | Non-exhaustive `match` (reported at the `match` line) |
| E0008 | Literal out of range for its type |
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
| E0130 | Derive requirement not met (reported at the offending field or derive clause) |
| E0140 | Interface implementation is missing a method (reported at the `impl` line) |
| E0141 | Interface bound or `requires` not satisfied, or a generic body uses an operation its bounds do not provide |
| E0142 | Orphan rule violation |
| E0150 | Use of a `priv` item from another package |
| E0160 | `?` used in a function whose return type is not `Option` or `Result` |
| E0161 | Operation that requires `unsafe` used outside `unsafe` |
| E0162 | Type or convention not allowed in an `extern "C"` declaration (parameters and results are integers, floats, and `CPtr`) |
| E0170 | Invalid test attribute: an unknown name, a repeated attribute, a value the attribute does not take, or `should_panic` with `should_fail` |

### Codes without a compile-fail test

Every code in the registry above needs a `//~` marker in at least one conformance test. A code that cannot be
witnessed by a compile-fail test (for example, a rule only a passing test can show) is listed here with the reason;
`tests/check-tests.sh` and `tests/check-tests.ps1` fail when a registered code is in neither place, and when a row
here names an unknown code, a code that already has a test, or no reason.

| Code | Why it has no compile-fail test |
| --- | --- |

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
   `*h` moves the whole pointee out and frees the allocation. Moving a field through `*h` is not supported in stage 0
   (E9000; stage 1). `spec/memory-rules.md` MV6, MV8.
6. **Loops.** `for x in list` (and arrays and slices) borrows the collection for the whole loop, so it is still usable
   afterwards, and gives a read-only element place usable like a value. Maps use `for (k, v) in map` (also a borrow).
   `for c in s.chars()`. Mutating the collection in the body is E0111. Ranges are `a..b` and `a..=b`, and in stage 0
   they are only iterable. `spec/memory-rules.md` IT1-IT3.
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
  scrutinee, and `*h` on an owned `Heap<T>` moves the whole pointee out and frees it; a field of the pointee moves
  only after the whole pointee is in a local. Decision 5; `m06_conventions/match_borrowed_binding.bd`,
  `m05_ownership/match_borrowed_binding_readonly.bd`, `heap_tree_eval.bd`, `heap_move_whole_then_field.bd`; MV6, MV8.
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
- **`unsafe` gates `extern` calls only (deviation).** The design also puts raw-pointer dereferences, unchecked
  accessors, and low-level casts behind `unsafe`. Stage 0 requires it for calls; holding or moving a `CPtr` does not
  need it. `HANDOFF.md` known problem 13; `m08_modules/extern_libc.bd`, `m11_llvm/llvm_return_42.bd`.
- **Array and slice patterns are stage 1 (deviation).** The design lists arrays under destructuring; stage 0 matches
  structs, enums, tuples, literals, and ranges. No test uses array patterns (issue #486).
- **No private modules or module-visibility check (deviation).** The design allows `priv` on modules and checks that
  public signatures use public types; stage 0 has no syntax for a private module and no module-visibility check
  (`HANDOFF.md` known problem 13). The tests cover `priv` items and fields across packages (`m08_modules/*`).
- **`as` also converts `char` to an integer.** The design says `as` converts between numeric types; stage 0 follows
  Rust and allows `c as i32` (`m09_stdlib/tokenizer.bd`, `m02_basics/casts_and_numeric_methods.bd`).
- **No linter (deviation).** The design's warnings (`else` on a same-package enum, "use `.iter()`") are deferred with
  `burdin lint`. No test depends on a warning; `m04_data/match_patterns.bd` uses `else` only on integers.

### Errors and testing

- **Explicit error types (deviation).** `Result<T, E>` always names `E`; dynamic `Result<T>`, the `error` keyword,
  `.context(...)`, and traces are stage 1. Decision 8; `m09_stdlib/question_and_from.bd`.
- **`?` converts through a hand-written `From` impl (deviation).** The design's `from` clause inside an `error enum`
  is not available, so the test writes `AppError: From<ParseError> { fn from(...) }`
  (`m09_stdlib/question_and_from.bd`).
- **Error payloads are unit aliases (deviation).** `IoError`, `ParseIntError`, and `AssertError` are `type ... = ()`
  until stage 1. Tests only match `Err(_)` (`m09_stdlib/*`). `HANDOFF.md` known problem 15.
- **`assert` has two modes (deviation).** In a normal build it panics; in a `burdin test` build the prelude binds
  `__test_assert`/`__test_assert_eq`, which return `Result<(), AssertError>` so a failed test is reported instead of
  aborting. The design describes the test form only. Decision 9, `HANDOFF.md` known problem 18;
  `m10_test_runner/*`.
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
- **`Map` is a linear scan (deviation).** The design and the stage 0 subset name a hash map; stage 0 stores entries in
  a list and needs only `Eq` on keys. The API and the (unspecified) iteration order are the same, so tests sum or
  sort instead of depending on order (`m09_stdlib/map_ops.bd`, `word_count.bd`). `HANDOFF.md` known problem 14.
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
- **C bridge surface.** `CPtr<T>`, `CChar`, `CSize`, `CVoid` (only behind a pointer), `CString`, `CStr` (an alias for
  `CPtr<CChar>`), `CString.from(str).as_ptr()`, `CPtr.null()` (no arguments), and `CStr.to_string(ptr): String`.
  `extern "C"` parameters and results are integers, floats, and `CPtr`. The design sketches `CPtr` only. Decision 7;
  `m08_modules/extern_libc.bd`, `cptr_null_arguments.bd`, `cstr_roundtrip.bd`, `m11_llvm/llvm_return_42.bd`.

### Not covered yet

These are absences, not deviations; they are listed so the next review does not mistake silence for a decision. No
test currently exercises: release-mode wrapping (the runner can only skip a test in release, not change its
expectation), out-of-range shift panics, `unsafe fn` declarations, a `main(): Result<()>` (stage 1), bounds panics
for `List`/`Map`/slice indexing (only arrays are covered), `priv` methods, `match` on `bool` or `char`, or struct
patterns that omit fields.
