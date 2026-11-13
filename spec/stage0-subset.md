# Burdin stage 0 subset

This is the maintained copy. `../Burdin Stage 0 Subset.md` is the original export from claude.ai and is out of date:
it predates the milestone folders, the error-code registry, and the decisions in `memory-rules.md`.

Stage 0 is a Rust compiler for the smallest subset of Burdin that can express the Burdin compiler itself. It enforces
the memory-model checks from the start. Everything else waits for stage 1, when the compiler is rewritten in Burdin.
The language is described in `../Burdin Language Design.md`, the syntax in `grammar.md`, the checks in `memory-rules.md`,
and the conformance tests in `../tests/`.

## Inclusion rule

A feature is in stage 0 only if one of these is true:

1. The stage 1 compiler needs it to be written (data structures, control flow, text handling, file I/O, calling LLVM).
2. It is part of the memory-model checks (moves, parameter conventions, exclusivity, drops), which stage 0 must enforce.
3. It is cheaper to build now than to avoid (for example shadowing and tuples).

Everything else is deferred, even if it is already designed.

**Targets.** Linux, macOS, and Windows, on x86-64 and arm64. On Windows the MSVC toolchain, not MinGW. One backend,
LLVM, at its lowest optimization level by default, with a thin platform layer for the system calls that differ.

**Done means:** stage 0 compiles a Burdin program of compiler size, passes the whole conformance suite on all three
systems, and rejects every memory-rule violation in the test set.

## Language features in stage 0

| Area | Included | Weight |
| --- | --- | --- |
| Lexing and syntax | Identifiers; integer, float, char, string literals with escapes; all comment forms; no semicolons; blocks as expressions | S |
| Bindings | `let`, `let mut`, `const` with simple constant expressions, shadowing, annotations, local inference | M |
| Primitive types | `i8`..`i64`, `u8`..`u64`, `isize`, `usize`, `f32`, `f64`, `bool`, `char`, `str`, `()`; `as` casts; debug overflow panic, release wrap; bounds checks | M |
| Functions | `fn` with `:` return types, `mut` and `own` parameters, `self` forms, recursion | M |
| Control flow | `if`/`else`, `while`, `loop`, `for` over built-in iterables, `break`, `continue`, `return` | S |
| Structs and tuples | Declarations, literals with shorthand, tuples, field access, `Type { ... }` method blocks, type aliases | M |
| Enums and match | Tuple, struct, unit variants; `match` with destructuring, literals, ranges, guards, or-patterns, `@`, `else`; `if let`, `let else`; exhaustiveness | L |
| Generics and interfaces | Bounded generics on functions, structs, enums; `interface` with `requires` and default methods; `Type: Interface { ... }` with `where`; monomorphization; orphan rule | L |
| Built-in interfaces | `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Drop`, `From` | M |
| Derive | `derives (Copy, Clone, Eq, Ord, Hash, Default)`, hard-coded in the Rust compiler | M |
| Ownership and moves | Single owner, moves, use-after-move errors, partial and conditional moves | L |
| Conventions and exclusivity | Read-only default, `mut`, `own`; place-based exclusivity check at each call | L |
| References and views | `&T`, `&mut T`, `str`, `[T]` as second-class types; single-source return rule; propagation through generics | L |
| Drop | Scope-end drops, fields, temporaries, early exits, drop flags | L |
| Heap | `Heap<T>` on C `malloc`/`free` | S |
| Errors | `Option`, `Result<T, E>` with explicit `E`, `?` with `From`, `orelse` with a default value, `unwrap`, `expect`; panics abort | M |
| Modules | Files as modules, `use` with braces and `as`, `priv`, a prelude; one package with a minimal `burdin.toml` | M |
| Unsafe and C | `unsafe`, `extern "C"` blocks, `CPtr<T>`, used for libc and the LLVM C API | M |
| Tests | `test` blocks, `assert` and `assert_eq` returning errors, a runner that restarts after an abort | M |
| Diagnostics and driver | Spans, error codes (see `../tests/README.md`), `burdin build`, `run`, `test` | M |

Built-in operators on primitives are compiler built-ins. Indexing is built in for `List`, arrays, slices, and `Map`.

## Standard library in stage 0

Written mostly in Burdin itself on top of `unsafe` and C's `malloc`, so it also tests the compiler.

| Area | Contents |
| --- | --- |
| Core types | `Option<T>`, `Result<T, E>`, `Heap<T>`, ranges |
| `String`, `str` | UTF-8; `len`, `push`, `push_str`, `clone`, comparison, slicing, `starts_with`, `find`, `chars()`, number to string and parse |
| `List<T>` | `new`, `push`, `pop`, `len`, `last`, indexing, `insert`, `remove`, `clear`, `contains`, `sort`, `for` |
| `Map<K, V>` | Hash map: `new`, `insert`, `get`, `remove`, `contains_key`, `len`, `for` |
| Memory helpers | `swap`, `replace` |
| Numbers | `min`, `max`, `abs`, `wrapping_*`, `checked_*`, parsing |
| Output | `print`, `println`, `eprint`, `eprintln` taking `str` |
| Files and process | `read_to_string`, `write_file`, `env.args()`, `process.exit`, running a child process |
| Errors | `IoError`, `AssertError`, `panic`, `unreachable` |
| C bridge | `CPtr<T>`, `CChar`, `CString`, hand-written `extern` declarations for libc and the LLVM C API |

**Language items** the Rust compiler knows by name: `Copy`, `Clone`, `Drop`, `Eq`, `Ord`, `Hash`, `From`, `Option`, `Result`,
the range types, and the intrinsics behind `str` and `[T]`. Everything else is ordinary library code.

**Runtime.** A small start-up wrapper calls `main`, and the panic path prints the message with file and line, flushes stdout,
and aborts. Files, processes, paths, and environment go through one platform module with an implementation per operating system.

## Deferred

| Feature | Returns in |
| --- | --- |
| Closures and lazy iterator chains (`map`, `filter`, `iter()`) | Stage 1, first |
| `Iterable`, `Iterator`, user-defined `for` | Stage 1, first |
| `any` and `impl` types in signatures | Stage 1 |
| String interpolation, `Display`, `Debug`, formatting | Stage 1, early |
| `error` declarations, dynamic `Error`, default `Result<T>`, `.context(...)`, traces | Stage 1 |
| Extension methods, default and named arguments | Stage 1 |
| Operator overloading, user-defined `Index` | Stage 1 |
| Comptime, const generics, `embed`, comptime derives | Stage 1, after a MIR interpreter |
| `build.bd` | Stage 1 |
| Allocator parameters, `Arena`, `Shared<T>`, leak-detecting allocator | Stage 1 |
| Tasks, channels, `select`, `Mutex`, atomics, `thread.spawn` | After self-hosting |
| Bindgen for C and Rust | After self-hosting |
| `fmt`, `lint`, `doc`, `add`, language server, lockfile, multiple-package workspaces | After self-hosting |
| `i128`/`u128`, `try_from`, labeled loops, `break` with a value, compound assignment | When wanted |
| `Set`, `Deque`, doc tests, `bench` blocks | When wanted |

## Milestones

Each milestone is a vertical slice. It is done when every test in its folder (and all earlier folders) passes with
`tests/run-tests.sh`. When a decision changes, update the spec and the tests first, then the code.

| # | Folder | Scope | Exit |
| --- | --- | --- | --- |
| 1 | `m01_hello` | Lexer, parser for functions and calls, LLVM codegen, linking, `println` | Hello world runs; syntax errors report file, line, column |
| 2 | `m02_basics` | Primitives, arithmetic, bindings, control flow, recursion, panic path | Fibonacci, GCD, Collatz match; overflow, bounds, and divide-by-zero abort with a message |
| 3 | `m03_typecheck` | Local inference, casts, error codes | Ill-typed programs rejected with the expected codes |
| 4 | `m04_data` | Structs, tuples, enums, `match`, exhaustiveness | RPN evaluator runs; missing cases rejected |
| 5 | `m05_ownership` | Moves, `Drop`, drop order and flags, `Heap` | Drop order exact, including early return and conditional moves; use after move rejected |
| 6 | `m06_conventions` | `mut`, `own`, `&T`, `str`, `[T]`, exclusivity, single-source return | Every program compiles or is rejected exactly as written |
| 7 | `m07_generics` | Monomorphization, interfaces, `requires`, derives, orphan rule | Generic stack and `max_of` work; derived `Eq`, `Ord`, `Hash` behave |
| 8 | `m08_modules` | Files as modules, `use`, `priv`, `extern "C"`, `CPtr<T>` | Multi-file program runs; `priv` across packages rejected; libc call works |
| 9 | `m09_stdlib` | `Option`, `Result`, `?`, `String`, `List`, `Map`, file I/O, processes | Word count and tokenizer pass |
| 10 | `m10_test_runner` | `test` blocks, assert errors, abort recovery, driver commands | The runner reports passed and failed counts correctly, including after an abort |
| 11 | `m11_llvm` | LLVM C API from Burdin | A Burdin program builds and verifies a function returning 42. Then stage 0 is complete and the stage 1 rewrite starts |
