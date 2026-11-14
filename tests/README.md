# Burdin stage 0 conformance tests

These programs are the target for the stage 0 compiler (Rust, LLVM backend). They come from
`Burdin Language Design.md` and [the stage 0 subset](../spec/stage0-subset.md). Each folder is one milestone; a
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
| `//~ E0000` | At the end of a line: that line must carry that diagnostic. Several markers per file are allowed. |
| `//@ burdin-test passed=N failed=M` | Run `burdin test` on the package. Counts must match. |
| `//@ failed-test: name` | That test must be reported as failed. May repeat. |
| `//@ exit-code: N` | Expected exit code with `run`. |
| `//@ args: a b` | Command-line arguments passed after the program name. |
| `//@ debug-only` | Skip in release profile (overflow checks). |
| `//@ link-lib: NAME` | Extra library to link. |
| `//@ undecided: text` | Skipped; documents an open design question. |

Runner rules: run each test in a fresh empty temporary directory as the working directory; compare
stdout after normalizing `\r\n` to `\n`; every `compile-fail` file must report all of its errors (no
stopping at the first error) and no others.

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
pwsh tests/check-tests.ps1            # the same check in PowerShell
```

The runner finds the driver in `BURDIN`, then `target/{debug,release}/burdin`, then `PATH`. It expects `burdin build`,
`burdin run`, and `burdin test` to behave as described at the top of `run-tests.sh`: diagnostics as
`path:LINE:COL: error[E0000]: message`, and a `test result: N passed, M failed` line plus `FAIL <name>` lines from
`burdin test`. If the real driver differs, change the section marked in that file. The exit code is 0 only when every
test passes. On Windows, run it from Git Bash.

## Error codes

The registry the tests use. The compiler adopts these numbers (or the tests are updated).

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

## Assumptions the tests make

The design document leaves these open. The tests use the choices below, so confirm or change them
(a search and replace on the tests is cheap).

1. **Call sites.** A `mut` parameter is passed with a `mut` marker (`push(mut list, 1)`). An `own` parameter is passed
   plainly and moves the value. A read-only parameter is passed plainly and borrows.
2. **One implicit coercion.** A `String` passed where `str` is expected is borrowed as a `str`. All other conversions are explicit.
3. **Views are copyable.** `str`, `[T]`, and `&T` can be used many times. `&mut T` cannot be copied.
4. **String literals are static views.** A function may return a literal as `str` whatever its parameters are. Any other returned
   view or reference must derive from exactly one parameter.
5. **Pattern bindings** from a borrowed scrutinee are read-only places usable like plain values (no explicit `&`).
   `*h` dereferences a `Heap<T>`; on an owned `Heap<T>`, `*h` moves the value out and frees the allocation.
6. **Loops.** `for x in list` (and arrays and slices) borrows the collection, so it is still usable afterwards, and gives
   a read-only element place usable like a value. Maps use `for (k, v) in map` (also a borrow). `for c in s.chars()`.
   Ranges are `a..b` and `a..=b`.
7. **Library names and signatures.** `n.to_string()` (integers, `bool`, `char`, `str`), `i32.parse(s)`,
   `wrapping_add/sub/mul`, `checked_add/sub/mul` (returning `Option<T>`), `min`, `max`, `abs`; `String.new()`,
   `String.from(str)`, `s.push_str(str)`, `s.push(char)`, `s.len(): usize` (bytes), `s.starts_with(str)`,
   `s.find(char): Option<usize>`, `s.find_str(str): Option<usize>`, slicing `s[a..b]`, `.clone()`, `.as_str()`,
   `s.chars()`, and the comparison operators (so `List<String>.sort()` works); `List<T>.new`, `push(v)`,
   `pop(): Option<T>`, `len(): usize`, `last(): Option<&T>`, `insert(i, v)`, `remove(i): T`, `clear()`,
   `contains(v): bool`, `sort()`; `Map<K, V>.new`, `insert(k, v)`, `get(k): Option<&V>`, `remove(k): Option<V>`,
   `contains_key(k): bool`, `len(): usize`; `Heap.new(v)`; `mem.swap(mut a, mut b)`,
   `mem.replace(mut place, value): T`; `read_to_string(path): Result<String, IoError>`,
   `write_file(path, text): Result<(), IoError>`; `process.exit(code)`, `process.run(command)`;
   `env.args(): List<String>` (program name first); `panic(msg)`, `assert(cond)`, `assert_eq(a, b)`;
   `Option`/`Result` `unwrap()`, `expect(msg)`, `orelse`, and `is_some`/`is_none` (`Option`) or `is_ok`/`is_err`
   (`Result`); `CString.from(str).as_ptr()`, `CPtr.null()`, `CStr.to_string(ptr: CPtr<CChar>): String` (`CStr` is
   an alias for `CPtr<CChar>`). The C bridge types are `CPtr<T>`, `CChar` (a signed byte), `CSize` (`size_t`),
   `CVoid` (only behind a pointer), and `CString`; `extern "C"` parameters and results are limited to integers,
   floats, and `CPtr`.
8. **`main`** returns unit in stage 0 (the default-error `Result<T>` is stage 1). Fallible APIs use explicit error types.
9. **Panics** flush stdout before aborting, and print the message, the file name, and the line to stderr.
10. **File I/O is binary-safe** on Windows (no newline translation).
11. **`Drop`** is `Type: Drop { fn drop(mut self) { ... } }`.
12. **Integer literals** have no suffixes. `/` and `%` truncate toward zero.
13. **Syntax shape.** Struct fields and enum variants are one per line. Struct literals use commas. Items need no separators.
14. **Temporaries** are dropped at the end of the statement, and reading a field of a temporary does not extend its life.
15. **Derives** provide `hash(self): u64` for `Hash` and a static `default()` for `Default`.
16. **Exclusivity works on places.** A place is a local or parameter followed by struct fields (`p.a`, `o.inner.b`).
    Different places do not overlap, so `swap_vals(mut p.a, mut p.b)` is allowed. A place never continues through
    indexing, a `Heap` dereference, or a method call, so `xs[0].a` and `xs[1].a` are one place and are rejected (E0110),
    as is a whole value together with one of its fields. (Decided.)

## Review of the expected outputs

The expected outputs and diagnostics were written by hand before the compiler existed, so a passing test only
proves that the implementation matches the expectation. Issue #20 reviewed every test against
[the design](../Burdin%20Language%20Design.md), [the grammar](../spec/grammar.md),
[the memory rules](../spec/memory-rules.md), [the stage 0 subset](../spec/stage0-subset.md), and the assumptions
above on 2026-11-14. Each milestone folder was read in full. No expectation was found to encode an accidental
behavior, so no test was changed; the behavior the tests pin where the design is silent, defers a feature, or
deliberately differs is recorded below.

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
| `m09_stdlib` | 14 | `Option`/`Result`/`?`, `String`, `List`, `Map`, file I/O, process and environment |
| `m10_test_runner` | 5 | `test` blocks, counts, failed names, abort recovery, stripping from builds |
| `m11_llvm` | 1 | the LLVM C API through `extern "C"` and `CPtr` |
| **Total** | **133** | |

## Intentional choices and deviations

"Design" means [the design document](../Burdin%20Language%20Design.md). A **deviation** is a place where stage 0
deliberately does something the design does not, or does not yet, require. The other entries are choices the design
leaves open that the tests now pin. Features that are only deferred have no tests here; the complete list is in
[the stage 0 subset](../spec/stage0-subset.md). Code quoted below comes from tests that compile and run; the one
illustrative fragment, the `Drop` declaration with `...`, is marked as such in assumption 11.

### Language and memory

- **Read-only parameters are borrowed; returning one by value needs `own`.** The design's
  `fn max<T: Ord>(a: T, b: T): T` sketches a signature, but a read-only parameter is a borrow in stage 0, so the test
  writes `fn max_of<T: Ord>(own a: T, own b: T): T` (`m07_generics/generic_functions.bd`). Returning a read-only
  parameter is E0101 (`m06_conventions/move_out_of_readonly_param.bd`; CV1, RT2).
- **One implicit conversion (deviation).** A `String` passed where `str` is expected is borrowed as a `str`; no other
  conversion is implicit. The design says "no implicit conversions". Assumption 2;
  `m06_conventions/str_views.bd`, `m09_stdlib/string_ops.bd`.
- **Views are copyable without deriving `Copy` (deviation).** `&T`, `str`, and `[T]` are Copy; `&mut T` is not. The
  design makes copying explicit except for `Copy` types and does not derive `Copy` on these. Assumption 3;
  `m06_conventions/references_basic.bd`, `str_views.bd`, `slice_views.bd`; RF4.
- **String literals are static views (an exception).** A function may return a literal from any parameter list, the
  one exception to the single-source rule. Assumption 4; `m06_conventions/return_literal_ok.bd`; RT4.
- **Borrowed pattern bindings are read-only places used like values.** No `&` is written on a binding from a borrowed
  scrutinee, and `*h` on an owned `Heap<T>` moves the pointee out and frees it. Assumption 5;
  `m06_conventions/match_borrowed_binding.bd`, `m05_ownership/heap_tree_eval.bd`; MV6, MV8.
- **`for` borrows its collection for the whole loop.** The design's `for` runs through a borrowing iterator, and
  stage 0 makes the borrow last until the loop ends, so a `mut` call on the same collection in the body is E0111.
  Assumption 6; `m09_stdlib/for_borrows_list.bd`. Appending while iterating returns with the stage 1 iteration
  protocol (`HANDOFF.md` known problem 19).
- **Owned parameters drop after locals, in reverse declaration order.** The design fixes the order of locals and
  fields but not parameters. DR3; `m05_ownership/drop_params_and_let_underscore.bd`.
- **Derived `Ord` orders enum variants by declaration.** The design's `Ord` rule covers field order; it does not
  define variant order. `m07_generics/derives.bd` pins `Low < Mid < High`.
- **`Drop` is spelled as an interface block.** `Type: Drop { fn drop(mut self) { ... } }`. Assumption 11; every
  `m05_ownership` drop test.
- **Derive method forms.** `Hash` provides `hash(self): u64`, `Default` a static `default()`, and `Eq`/`Ord` the
  comparison operators. Assumption 15; `m07_generics/derives.bd`.
- **No MIR (deviation).** The design's compiler architecture has a MIR; stage 0 checks the typed tree (`burdin_tir`),
  and `burdin_mir` is planned for stage 1 (`spec/stage1-subset.md`, issue #493). The header and RF2 of
  [the memory rules](../spec/memory-rules.md) now say so.
- **Borrow liveness is conservative (deviation in precision).** The rule is "until the last use", but stage 0
  computes it per top-level statement, so any use inside a loop keeps a loan live for the whole loop. This rejects
  some programs the design would allow; it never accepts a conflicting one. RF2, `HANDOFF.md`;
  `m06_conventions/borrow_ends_at_last_use.bd` pins the last-use rule.
- **`main` returns unit (deviation).** `fn main(): Result<()>` and printing a returned error are stage 1.
  Assumption 8; every `//@ run` test.
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
  `.context(...)`, and traces are stage 1. Assumption 8; `m09_stdlib/question_and_from.bd`.
- **`?` converts through a hand-written `From` impl (deviation).** The design's `from` clause inside an `error enum`
  is not available, so the test writes `AppError: From<ParseError> { fn from(...) }`
  (`m09_stdlib/question_and_from.bd`).
- **Error payloads are unit aliases (deviation).** `IoError`, `ParseIntError`, and `AssertError` are `type ... = ()`
  until stage 1. Tests only match `Err(_)` (`m09_stdlib/*`). `HANDOFF.md` known problem 15.
- **`assert` has two modes (deviation).** In a normal build it panics; in a `burdin test` build the prelude binds
  `__test_assert`/`__test_assert_eq`, which return `Result<(), AssertError>` so a failed test is reported instead of
  aborting. The design describes the test form only. Assumption 9, `HANDOFF.md` known problem 18;
  `m10_test_runner/*`.
- **No leak-detecting allocator (deviation).** The design runs every test under one; allocators are stage 1, so no
  test asserts that a leaked allocation fails. `spec/stage0-subset.md`.
- **Panic wording is chosen by the runtime.** The design requires a message, the file, and the line; it does not fix
  the text. The abort tests match substrings (`division by zero`, `out of bounds`, `overflow`, the `panic` argument,
  and the file name). `m02_basics/*_aborts.bd`.

### Standard library and C bridge

- **Names and signatures are the tests' choices.** The design names some operations and says `Option`/`Result`
  follow Rust "and so on"; stage 0 uses the set in assumption 7, now including the signatures the tests exercise.
  `HANDOFF.md` known problem 2. In particular `List.last` and `Map.get` return `Option<&T>`/`Option<&V>`, `List.pop`
  returns `Option<T>`, `List.remove` and `Map.remove` return owned values, and `List.contains` takes a value.
- **`String` length and slicing are byte-based.** `s.len()` is bytes and `s[a..b]` slices bytes; `chars()` iterates
  Unicode scalar values. The design says UTF-8 but does not define `len`. `m09_stdlib/string_ops.bd`,
  `m09_stdlib/file_io.bd`; assumption 7.
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
- **File I/O is binary-safe.** No newline translation on Windows. Assumption 10; `m09_stdlib/file_io.bd`.
- **`env.args()` puts the program name first.** The design names `std.env.args()` but not its contents. Assumption 7;
  `m09_stdlib/env_args.bd`.
- **C bridge surface.** `CPtr<T>`, `CChar`, `CSize`, `CVoid` (only behind a pointer), `CString`, `CStr` (an alias for
  `CPtr<CChar>`), `CString.from(str).as_ptr()`, `CPtr.null()`, and `CStr.to_string(ptr): String`. `extern "C"`
  parameters and results are integers, floats, and `CPtr`. The design sketches `CPtr` only. Assumption 7;
  `m08_modules/extern_libc.bd`, `m11_llvm/llvm_return_42.bd`.

### Not covered yet

These are absences, not deviations; they are listed so the next review does not mistake silence for a decision. No
test currently exercises: release-mode wrapping (the runner can only skip a test in release, not change its
expectation), out-of-range shift panics, `unsafe fn` declarations, a `main(): Result<()>`, bounds panics for
`List`/`Map`/slice indexing (only arrays are covered), `priv` methods, `match` on `bool` or `char`, struct patterns
that omit fields, or the exact text of a derived `hash()` value (only equality of hashes is checked).
