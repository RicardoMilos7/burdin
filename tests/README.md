# Burdin stage 0 conformance tests

These programs are the target for the stage 0 compiler (Rust, LLVM backend). They come from
`Burdin Language Design.md` and the Stage 0 Subset document. Each folder is one milestone; a
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
7. **Library names.** `n.to_string()`, `i32.parse(s): Result<i32, ParseIntError>`, `String.new()`, `String.from(str)`,
   `s.push_str(str)`, `s.push(char)`, `s.len(): usize` (bytes), `s.starts_with(str)`, `s.find(char): Option<usize>`,
   `s.find_str(str): Option<usize>`, slicing `s[a..b]`, `.clone()`, `.as_str()`,
   `List<T>.new/push/pop/len/last/insert/remove/clear/contains/sort`, `Map<K, V>.new/insert/get/remove/contains_key/len`,
   `Heap.new(v)`, `read_to_string(path)`, `write_file(path, text)`, `process.exit(code)`, `env.args()`
   (program name first), `panic(msg)`, `assert(cond)`, `assert_eq(a, b)`, `CString.from(str).as_ptr()`, `CPtr.null()`.
   The C bridge types are `CPtr<T>`, `CChar` (a signed byte), `CSize` (`size_t`), `CVoid` (only behind a pointer),
   and `CString`; `extern "C"` parameters and results are limited to integers, floats, and `CPtr`.
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
