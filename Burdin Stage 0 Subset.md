# Burdin Stage 0 Subset

## Purpose and inclusion rule

Stage 0 is a Rust compiler for the smallest subset of Burdin that can express the Burdin compiler itself, and it enforces the memory-model checks from the start. Everything else waits for stage 1, when the compiler is rewritten in Burdin.

**A feature is in stage 0 only if one of these is true:**

1. The stage 1 compiler needs it to be written (data structures, control flow, text handling, file I/O, calling LLVM).
2. It is part of the memory-model checks (moves, parameter conventions, exclusivity, drops), which stage 0 must enforce.
3. It is cheaper to build now than to avoid (for example shadowing and tuples).

**Everything else is deferred**, even if it is already designed. Deferred features come back in stage 1 or later, and the compiler is written without them in the meantime.

**Targets and backend.** One backend, LLVM, at its lowest optimization level by default. Linux, macOS, and Windows from the start, with a thin platform layer for the system calls that differ. The compiler links with the system C compiler or linker.

**What "done" means.** Stage 0 is finished when it compiles a Burdin program of compiler size, passes the full conformance suite (programs with expected output, and programs that must be rejected with a specific error code), and rejects the memory-rule violations in the test set.

## Language features in stage 0

Stage 0 covers 20 areas, and six of them (match, interfaces, moves, conventions, references, drops) carry most of the work. Weight is a rough size of the implementation: S small, M medium, L large.

| Area | Included in stage 0 | Weight |
| --- | --- | --- |
| Lexing and syntax | Identifiers; integer, float, char, and string literals with escapes; all three comment forms; no semicolons; blocks as expressions | S |
| Bindings | `let`, `let mut`, `const` with simple constant expressions, shadowing, type annotations, local inference | M |
| Primitive types | `i8` to `i64`, `u8` to `u64`, `isize`, `usize`, `f32`, `f64`, `bool`, `char`, `str`, `()`; `as` casts; debug overflow panic, release wrap; bounds checks | M |
| Functions | `fn` with `:` return types, `mut` and `own` parameters, `self` forms, recursion | M |
| Control flow | `if`/`else`, `while`, `loop`, `for` over built-in iterables (ranges, arrays, `List`, `Map`), `break`, `continue`, `return` | S |
| Structs and tuples | Declarations, literals with shorthand, tuples, field access, `Type { ... }` method blocks, type aliases | M |
| Enums and match | Tuple, struct, and unit variants; `match` with destructuring, literals, ranges, guards, or-patterns, `@` bindings, `else` arm; `if let`, `let else`; exhaustiveness checking | L |
| Generics and interfaces | Bounded generic functions, structs, and enums; `interface` with `requires` and default methods; `Type: Interface { ... }` with `where`; monomorphization; orphan rule | L |
| Built-in interfaces | `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Drop`, `From` | M |
| Derive | `derives (Copy, Clone, Eq, Ord, Hash, Default)`, implemented inside the Rust compiler, not through comptime | M |
| Ownership and moves | Single owner, moves by default, use-after-move errors, partial moves out of non-`Drop` structs, conditional moves | L |
| Conventions and exclusivity | Read-only default, `mut`, `own`; exclusivity check at each call | L |
| References and views | `&T`, `&mut T`, `str`, `[T]` as second-class types; single-source return rule; no stored references; propagation through generics | L |
| Drop | Scope-end drops in reverse order, fields, temporaries, drops on `?`, `return`, and `break`, drop flags | L |
| Heap | `Heap<T>` on the C `malloc` and `free` as the global allocator | S |
| Errors | `Option`, `Result<T, E>` with an explicit `E`, `?` with `From` conversion, `orelse` with a default value, `unwrap` and `expect`; panics abort with a message, file, and line | M |
| Modules | Files as modules, `use` with braces and `as`, `priv`, a prelude; one package with a minimal `burdin.toml` | M |
| Unsafe and C | `unsafe` blocks and functions, `extern "C"` blocks, `CPtr<T>`, raw pointer operations; used for the LLVM C API and libc | M |
| Tests | `test` blocks, `assert` and `assert_eq` returning errors, a runner that restarts after an abort | M |
| Diagnostics and driver | Source spans, error codes (`E0001` and up) with a code snippet, used by the tests; `burdin build`, `burdin run`, `burdin test` | M |

**Notes on a few choices**

- Operators on primitives are built in. `==`, `!=`, `<`, and the other comparisons work through `Eq` and `Ord`. Arithmetic overloading is deferred.
- Indexing (`list[i]`, `map[k]`) is built in for `List`, arrays, and `Map`, with the same borrow rules the `Index` interface will use later.
- Derives are hard-coded in the compiler for the listed interfaces. They are replaced by the comptime versions in stage 1 or later.

## Standard library and runtime

The stage 0 library is the minimum that the stage 1 compiler needs, and almost all of it is written in Burdin itself using `unsafe` and C's `malloc`, so it also serves as a large test of the compiler.

| Area | Contents |
| --- | --- |
| Core types | `Option<T>`, `Result<T, E>`, `Heap<T>`, ranges |
| `String` and `str` | UTF-8; `len`, `push`, `push_str`, `clone`, comparison, byte-range slicing, `starts_with`, `find`, iteration over `char`s with `for`, integer to string and string to integer |
| `List<T>` | `new`, `push`, `pop`, `len`, `last`, indexing, `insert`, `remove`, `clear`, `contains`, `sort` for `Ord` types, `for` iteration |
| `Map<K, V>` | Hash map for keys that are `Eq` and `Hash`: `new`, `insert`, `get`, `remove`, `contains_key`, `len`, `for` iteration over entries |
| Memory helpers | `swap`, `replace`, which are needed to move values out through a `mut` reference |
| Numbers | `min`, `max`, `abs`, `wrapping_*` and `checked_*` arithmetic, integer parsing |
| Output | `print`, `println`, `eprint`, `eprintln` taking `str` |
| Files and process | `read_to_string`, `write_file`, `env.args()`, `process.exit`, and running a child process (used to call the linker) |
| Errors | `IoError`; `panic(msg)`, `unreachable()`, `assert` and `assert_eq` returning errors |
| C bridge | `CPtr<T>`, `CChar`, C string conversion, hand-written `extern` declarations for the libc and LLVM C API functions the compiler uses |

**What the Rust compiler must know about by name** (the "language items"): `Copy`, `Clone`, `Drop`, `Eq`, `Ord`, `Hash`, `From`, `Option` and `Result` (for `?`, `orelse`, and `if let`), the range types (for `for`), and the intrinsics behind `str` and `[T]`. Everything else, including `List`, `String`, and `Map`, is ordinary library code.

**Runtime.** A small start-up wrapper calls `main`, turns an error returned from `main` into a message and a non-zero exit code, and implements the panic path: print the message with file and line, then abort.

**Platform layer.** Files, processes, paths, and environment access go through one small module with an implementation per operating system (POSIX calls for Linux and macOS, the Windows C runtime for Windows), so the rest of the library is portable.

**Not in stage 0:** `Set`, `Deque`, `Shared`, `Arena`, allocator type parameters, threads, tasks, channels, time, random numbers, string formatting and interpolation, and serialization.

## Deferred features

These are designed in the main document and left out of stage 0, so the stage 1 compiler is written without them and they are added back once it self-hosts.

| Feature | Why it waits | Returns in |
| --- | --- | --- |
| Closures and lazy iterator chains (`map`, `filter`, `iter()`) | Capture analysis and second-class types through generics are a large piece; loops are enough for the compiler | Stage 1, first |
| `Iterable`, `Iterator`, associated types, user-defined `for` | Depends on closures and generic second-class types | Stage 1, first |
| `any Interface` and `impl Interface` in signatures | Needs vtables and boxing; not needed by the compiler | Stage 1 |
| String interpolation, `Display`, `Debug`, formatting | The compiler can build messages by concatenation; painful, but cheap to add early | Stage 1, early |
| `error` declarations, dynamic `Error`, `Result<T>` default, `.context(...)`, traces | Needs `any` and closures | Stage 1 |
| Extension methods, default and named arguments | Syntax sugar over functions | Stage 1 |
| Arithmetic operator overloading, user-defined `Index` | Built-in operators and indexing cover the compiler | Stage 1 |
| Comptime, const generics, `embed`, comptime derives | Needs a MIR interpreter; derives are hard-coded in stage 0 | Stage 1, after the MIR interpreter |
| `build.bd` | Needs the driver to compile and run a Burdin program during the build | Stage 1 |
| Allocator parameters, `Arena`, `Shared<T>`, leak-detecting allocator | Global `malloc` is enough | Stage 1 |
| Tasks, channels, `select`, `Mutex`, atomics, `thread.spawn` | The compiler is single-threaded at first | After self-hosting |
| Bindgen for C and Rust, `blocking` externs | Hand-written `extern` declarations cover the LLVM and libc subset | After self-hosting |
| `fmt`, `lint`, `doc`, `add`, language server, lockfile, multiple packages | One package is enough while the language changes daily | After self-hosting |
| `i128` and `u128`, `try_from`, labeled loops, `break` with a value | Not needed by the compiler | When wanted |
| `Set`, `Deque`, doc tests, `bench` blocks | Not needed by the compiler | When wanted |

**Suggested order for stage 1:** closures and iterators first (they remove the most boilerplate from the compiler's own code), then formatting, then the error extras and `any`, then the MIR interpreter and comptime.

## Milestones

Stage 0 is built as eleven vertical slices, and each one must pass the exit test below plus every earlier test before the next one starts. When a decision changes, update the spec and the tests first, then the code.

1. **Hello world.** Lexer, parser for functions and calls, LLVM code generation, linking, and `println`. *Exit:* a program that prints text runs and its output matches the expected file; a syntax error reports file, line, and column. The CI runs this test on Linux, macOS, and Windows.
2. **Integers and control flow.** Primitives, arithmetic, `let` and `let mut`, `const`, shadowing, `if`, `while`, `loop`, functions, recursion, and the panic path. *Exit:* Fibonacci, GCD, and Collatz programs match their expected output; a debug overflow and an out-of-bounds index abort with a message, file, and line.
3. **Type checking.** Local inference, `as` casts, and error codes. *Exit:* a set of ill-typed programs is rejected with the expected error codes.
4. **Structs, tuples, enums, and `match`.** Destructuring, guards, ranges, or-patterns, `@` bindings, `if let`, `let else`, and exhaustiveness checking. *Exit:* an expression evaluator runs correctly; programs with a missing case are rejected.
5. **Ownership, moves, and drops.** Moves, use-after-move errors, `Drop`, drop order, drop flags, and `Heap<T>`. *Exit:* a program whose `Drop` implementation prints shows the exact expected order (including early `return` and conditional moves); use-after-move is rejected.
6. **Conventions, exclusivity, and references.** `mut`, `own`, `&T`, `&mut T`, `str`, `[T]`, and the single-source return rule. *Exit:* the 15 to 20 tricky programs from the memory rules compile or are rejected exactly as written.
7. **Generics, interfaces, and derive.** Monomorphization, bounds, `requires`, implementations, the orphan rule, built-in interfaces, and hard-coded derives. *Exit:* a generic `Stack<T>` and a generic `max` work; derived `Eq`, `Ord`, and `Hash` behave correctly.
8. **Modules, unsafe, and C.** Files as modules, `use`, `priv`, `extern "C"`, and `CPtr<T>`. *Exit:* a multi-file program runs; a program calls the C library's write function (`_write` on Windows); a `priv` violation across packages is rejected.
9. **Standard library.** `Option`, `Result`, `?`, `orelse`, `String`, `str`, `List`, `Map`, file I/O, and process execution. *Exit:* a word-count program reads a file and prints correct counts; a tokenizer for a small language passes its tests.
10. **Tests and driver.** `test` blocks, assertion errors, the runner that restarts after an abort, and `burdin build`, `run`, and `test`. *Exit:* a package with 20 tests, one of which deliberately aborts, reports 19 passed and 1 failed.
11. **LLVM from Burdin.** Hand-written `extern` declarations for the LLVM C API subset. *Exit:* a Burdin program emits an object file for a function returning 42, links it, and runs it. Stage 0 is then complete, and the stage 1 rewrite starts.

## Decisions needed from you

The subset rests on these assumptions. Tick each one you accept, and comment on the ones you want changed.

- [x] **Target platforms.** Linux, macOS, and Windows from the start; less common systems are out of scope.
- [x] **Architectures and Windows toolchain.** x86-64 and arm64 on all three systems, and on Windows the MSVC toolchain (linking with `lld-link` or `link.exe`) rather than MinGW.
- [x] **No closures in stage 0.** The compiler is written with loops and `for` over built-in types until closures arrive early in stage 1.
- [x] **`for` works only on built-in iterables** (ranges, arrays, `List`, `Map`) in stage 0, and user-defined iterables come with `Iterable` in stage 1.
- [x] **Derives are hard-coded in the Rust compiler** for `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default`, and are replaced by comptime derives later.
- [x] **Errors use explicit types.** `Result<T, E>` needs both parameters, and the compiler uses plain error enums. The `error` keyword, the dynamic `Error`, and `.context(...)` wait for stage 1.
- [x] **No string interpolation or formatting** in stage 0, so compiler messages are built by concatenation.
- [x] **LLVM through `inkwell`** in the Rust compiler, with one pinned LLVM version, and the system C compiler or linker for the final link step. This requires a C toolchain on your machine.
- [x] **Memory checks from the start.** Stage 0 enforces moves, conventions, exclusivity, references, and drops.
