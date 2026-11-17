# Burdin Language Design

> **Status.** The repository copy of this file is canonical. Copies hosted on claude.ai are historical: they are
> no longer updated and may disagree with this document. Code snippets here are illustrative drafts, not compiled
> examples; the executable specification is the conformance suite in `tests/` (see `tests/README.md`).

## Overview

Burdin (file extension `.bd`) is a statically typed, natively compiled language with Rust-level safety, no garbage collector, and a simpler memory model built on value semantics. It is a personal experiment in building a language with AI assistance, and the author's own ecosystem (UI and TUI libraries, tools) will be written in it. It is not meant for production use.

**Must-haves**

- Compiles to native code, with fast compilation.
- Performance comparable to Rust and Zig, with zero-cost abstractions.
- No garbage collector, and memory safe.
- Strong static type system with `Option` and `Result`; no nulls and no exceptions.
- Owned data and references.
- No object-oriented inheritance.
- A rich standard library: serialization formats, HTTP server, image handling, math.

**Non-goals**

- Production readiness, ABI stability, or backward-compatibility promises.
- Embedded and `no_std` targets.
- A public package registry in the first versions.

**Design principles**

- Costs are visible: heap allocation is spelled `Heap<T>`, and arenas are chosen explicitly.
- One mechanism per job: no `defer`, no macros (comptime covers them), no inheritance.
- Public by default, with a single visibility keyword, `priv`.
- Everything in this document can change; there are no users to break.

## Syntax

The syntax follows Rust where it can, drops semicolons, and uses `:` for both type annotations and return types. Every snippet here is a draft; the grammar is fixed when the spec is written.

**Bindings and constants**

```
let x = 1                // immutable
let mut count = 0        // mutable
let name: str = "burdin"   // explicit type
const MAX: i32 = 100     // compile-time constant
let x = x + 1            // shadowing is allowed
```

There is no `:=` operator and no `var` keyword. A shadowed value is not dropped until the end of its scope.

**Functions**

```
fn add(a: i32, b: i32): i32 { a + b }
fn connect(host: str, port: u16 = 8080): Result<Conn> { ... }
connect("localhost", port: 9000)
```

Return types use `:`. Default and named arguments are supported on functions and methods, but not in interface method signatures. Blocks are expressions, so `if` and `match` produce values.

**Control flow**

`if`, `while`, `for`, and `match` take no parentheses around the condition, as in Rust.

**Closures**

```
let add = |a, b| a + b
let parse = |s: str|: Result<i32> { ... }
users.filter(|u| u.active)
```

A closure passed down to a function and not stored can borrow what it uses. A closure that is returned or stored captures by move.

**Extension methods**

A function declared with a receiver type is called with dot syntax, on any type. Plain functions are never dot-callable automatically.

```
fn String.to_user(self): User { ... }
fn String.shout(mut self) { ... }
fn List<T>.sum(self): T where T: Numeric { ... }

"alice".to_user()
```

Resolution order for `x.f()`: methods defined with the type, then interface methods, then extension functions that are imported. If a type later gains a method with the same name as an extension, the compiler warns that it now shadows the extension.

**Comments and strings**

- `//` line comments, `/* */` block comments, `///` doc comments.
- Strings use `"..."` with interpolation (`"hello {name}"`), `{{` to escape a brace, raw strings, and multi-line strings.

**Naming conventions** (enforced by the linter)

| Kind | Style |
| --- | --- |
| Functions, variables, modules, files | `snake_case` |
| Types, interfaces, enum variants | `PascalCase` |
| Constants | `SCREAMING_SNAKE_CASE` |
| Primitive types | lowercase (`i32`, `bool`, `str`) |

## Types and data

The type system is static and strong, with local inference, no implicit conversions, and numbers that behave as in Rust.

**Primitives** use Rust's names: `i8` to `i128`, `u8` to `u128`, `isize`, `usize`, `f32`, `f64`, `bool`, `char`, and `str`. Integer literals default to `i32` and float literals to `f64`. Indexes and lengths are `usize`.

**Numeric behavior**

- Integer overflow panics in debug builds and wraps in release builds. A profile setting (`overflow_checks = true`) turns checking on in release.
- Explicit variants are methods: `wrapping_add`, `checked_add`, `saturating_add`, `overflowing_add`.
- Indexing is always bounds-checked; only an `unsafe` accessor such as `get_unchecked` skips the check.
- Division by zero and out-of-range shifts always panic.
- `as` converts between numeric types; checked conversions use `try_from` and return a `Result`.

**Text, lists, and arrays**

| Type | Meaning |
| --- | --- |
| `String` | Owned, growable, UTF-8 |
| `str` | A view into text owned elsewhere (second-class, see Memory model) |
| `List<T>` | Owned, growable array |
| `[T; N]` | Fixed-size array, a plain value |
| `[T]` | A view of a slice (second-class) |
| `Map<K, V>`, `Set<T>`, `Deque<T>` | Standard collections |

**Structs**

```
struct Circle derives (Copy, Clone, Eq) {
    r: f64
}

let c = Circle { r: 2.0 }
let r = 2.0
let c2 = Circle { r }          // field shorthand

Circle {                       // methods with no interface
    fn new(r: f64): Circle { Circle { r } }
}
```

**Enums** carry data, as in Rust, and are matched exhaustively.

```
enum Shape {
    Circle(f64)
    Rect { w: f64, h: f64 }
    Point
}
```

Recursive types use `Heap<T>` explicitly:

```
enum Tree {
    Leaf(i32)
    Node(Heap<Tree>, Heap<Tree>)
}
```

**Option and Result** follow Rust's design and API (`map`, `and_then`, `unwrap_or`, `ok_or`, and so on):

```
enum Option<T> { Some(T), None }
enum Result<T, E = Error> { Ok(T), Err(E) }
```

`Result<T>` with one parameter uses the dynamic `Error` type (see Error handling). There are no nulls and no exceptions.

**Operator overloading** goes through interfaces (`Add`, `Sub`, `Mul`, `Div`, `Neg`, `Index`, and so on). There are no custom operators, and `&&`, `||`, and `=` cannot be overloaded.

## Memory model

Memory safety comes from value semantics: every value has one owner, borrows last only for the duration of a call, and references can never be stored. This removes lifetime annotations entirely and keeps the borrow checker local to one function body. It follows the mutable value semantics of Hylo, with Rust-style references restricted to be second-class.

**Ownership and moves**

- Every value has a single owner. Assigning or passing a value moves it.
- Copying is explicit (`.clone()`), except for types that derive `Copy` (small plain types such as integers and points).
- A moved-out value is not dropped by its old owner.

**Parameter conventions** state what a function may do with an argument, and the borrow ends when the call returns:

```
fn len(list: List<i32>): usize           // read-only access (default)
fn push(mut list: List<i32>, x: i32)     // exclusive mutable access
fn consume(own list: List<i32>)          // takes ownership (move in)
```

The same markers apply to `self` in methods (`self`, `mut self`, `own self`).

**Law of exclusivity.** While a call has mutable access to a value, no other argument may touch it. `swap(mut a, mut a)` and `push(mut list, list.len())` are rejected. The check is local to the function.

Exclusivity is checked on places. A place is a local or parameter followed by struct fields (`p.a`, `o.inner.b`). Different places do not overlap, so `swap(mut p.a, mut p.b)` is allowed. A place never continues through indexing, a `Heap` dereference, or a method call, so `swap(mut xs[0].a, mut xs[1].a)` is rejected, because the two indices might be equal. A whole value and one of its fields overlap.

**Second-class references.** `&T` and `&mut T` exist, but with restrictions:

- They may be local variables, parameters, and return types.
- They may not be stored in struct fields, collections, or anything that outlives the function.
- A function may return a reference (or a view such as `str` or `[T]`) only if it derives from exactly one parameter, so `fn trim(s: str): str` works with no copy. With two possible sources, the compiler reports an error.
- Borrows last until their last use and are checked inside one function. There are no lifetime annotations.
- Types that contain a reference, such as borrowing iterators, are second-class too. The compiler tracks this automatically through generics.

```
List<T> {
    fn index(self, i: usize): &T { ... }
    fn index_mut(mut self, i: usize): &mut T { ... }
}
let x = list[2]
list[2] = 5
```

Structs that need to point at other data store an index or a handle instead (parsers store offsets, graphs use arenas with handles).

**Drop.** Resources clean themselves up when they leave scope, so there is no `defer`.

- Locals drop in reverse declaration order, struct fields in declaration order, temporaries at the end of the statement.
- A `?` early return drops everything alive, which replaces `errdefer`.
- Moving a field out of a struct that implements `Drop` is forbidden.
- `Drop` cannot fail. Types such as files offer an explicit `close(own self): Result` for callers who need the error.
- Ad hoc cleanup uses a library scope-guard type that runs a closure when dropped.
- A shadowed binding is dropped at the end of its scope, not when it is shadowed.

**Heap and shared ownership**

- `Heap<T>` puts one value on the heap, and is how recursive types are written.
- `Shared<T>` is an opt-in reference-counted owner. Reference cycles leak by design. Stage 1's count was not atomic; since stage 2.3 it is, and `Shared<T>` is `Send` when its value is `Send` and `Sync` (`spec/shared.md`). There are no weak handles yet.

**Allocators.** Every container buffer comes from an allocator value. Allocator-aware types take a defaulted type argument, so most code never mentions allocators; a function that needs to choose one takes an ordinary generic parameter bound by `Allocator`:

```
let a = List<i32>.new()                 // List<i32, Global>
let arena = Arena.new()
let b = List<i32>.new_in(arena)         // List<i32, Arena>
fn build<A: Allocator>(own a: A): List<i32, A> { List<i32>.new_in(a) }
```

`Global` is zero-sized and stored nowhere; any other allocator is stored in the container as a value the container owns, so an arena and everything it allocated live exactly as long as the containers that use it. `Arena` owns its memory and frees everything when dropped or reset. Stage 1 has no replaceable program-wide allocator and no per-arena handles yet; those remain later-stage ideas. A `burdin test` build uses a checking allocator that reports leaks and double frees (not use-after-free yet), and ordinary builds use the C heap.

**Failure behavior.** Panics abort the process (no unwinding). Out-of-memory also aborts; fallible `try_` variants can be added later if a program needs them.

**`unsafe`.** `unsafe { ... }` blocks and `unsafe fn` allow raw pointer dereferences, calls to `extern` functions, unchecked accessors, and low-level casts. Safe wrappers are responsible for upholding the invariants.

## Interfaces and generics

Shared behavior comes from interfaces and bounded generics, with static dispatch by default and no inheritance.

**Interfaces** list method signatures (with parameter conventions), and may include default methods, associated types, and constants. An interface can require another (`requires`), which composes constraints without inheriting behavior.

```
interface Shape {
    fn area(self): f64
    fn scale(mut self, factor: f64)
    fn describe(self): String { "shape with area {self.area()}" }
}

interface Drawable requires Shape { fn draw(self) }
```

**Implementations** attach an interface to a type with `Type: Interface { ... }`. Plain methods with no interface go in a bare `Type { ... }` block. Bounds on a generic implementation use `where`.

```
Circle: Shape {
    fn area(self): f64 { 3.14159 * self.r * self.r }
    fn scale(mut self, factor: f64) { self.r = self.r * factor }
}

Circle {
    fn new(r: f64): Circle { Circle { r } }
}

List<T>: Clone where T: Clone { ... }
```

**Generics** are bounded: a generic function declares what its type parameters support, and the compiler checks the body against those bounds.

```
fn max<T: Ord>(a: T, b: T): T
```

Const generics take compile-time values (`Matrix<T, const R: usize, const C: usize>`), and `comptime` is used for type-level computation, not for checking generic bodies.

**Dispatch**

- **Static** (monomorphized) by default: the compiler generates a specialized copy per type.
- `impl Shape` as a parameter or return type means one concrete type known at compile time.
- `any Shape` means any type implementing the interface, resolved at runtime. It is an owned type that the compiler places on the heap, so `List<any Shape>` works without a visible wrapper.

**Coherence.** Rust's orphan rule: an interface can be implemented for a type only in the package that defines the interface or the type.

**Derive.** Common interfaces are derived with a clause on the declaration:

```
struct Point derives (Copy, Clone, Eq, Hash) {
    x: f64
    y: f64
}
```

| Interface | Rule |
| --- | --- |
| `Clone` | All fields are `Clone` |
| `Copy` | All fields are `Copy`, and the type has no `Drop` |
| `Eq` | All fields are `Eq` |
| `Ord` | The type is `Eq`; field order defines the comparison |
| `Hash` | All fields are `Hash`, consistent with `Eq` |
| `Default` | All fields are `Default` |
| `Debug` | All fields are `Debug` |
| `Serialize`, `Deserialize` | All fields support them |

Derives are written in the language with `comptime` and reflection (see Comptime), not as macros. There is no `macro_rules`. Hand-written implementations are always allowed. A derive on a generic type requires the same bound on its parameters, and a failing derive reports the offending field.

**Compile-speed measures.** Monomorphized instances are generated in parallel and cached between builds, identical instantiations are deduplicated, and overlapping implementations are disallowed to keep resolution simple.

## Error handling

Errors are values: `Result` and `Option` carry them, `?` propagates them, and the language has built-in support for typed errors (like `thiserror`) and dynamic errors with context (like `anyhow`).

**Typed errors** use the `error` keyword. It generates the message formatting and the source chain, and `from` lets `?` convert a lower-level error automatically.

```
error enum ConfigError {
    Io(IoError)            from   "cannot read config: {0}"
    Syntax { line: i32 }          "syntax error on line {line}"
    Missing(String)               "missing key {0}"
}
```

**Dynamic errors.** `Result<T>` with no second parameter uses a boxed `Error` type that any error converts into, so applications can use `?` everywhere without naming error types. An `Error` can be inspected or downcast with `match`.

**Context** wraps an error with a message and keeps the original as its source:

```
let text = read_file(path).context("loading settings")?
// loading settings: cannot read config: file not found
```

**Propagation and defaults**

- `expr?` returns early with the error (converted through `from` when declared).
- `x orelse default` supplies a value when an `Option` is `None` or a `Result` is an error.
- `res orelse |e| fallback(e)` handles the error with a closure. There is no `catch` keyword.

**Traces.** Debug builds record where an error was created and where it propagated, so an error carries a useful trace without panics or exceptions.

**Inferred error sets** (Zig-style `!T`) are not included, because they leak implementation details into signatures.

**Panics** are for bugs only (out-of-bounds access, overflow in debug builds, failed internal assumptions). They abort the process.

## Pattern matching

`match` is exhaustive and supports destructuring, guards, and bindings, following Rust's design.

```
match shape {
    Shape.Circle(r) if r > 10.0 => "big circle",
    Shape.Circle(_)             => "circle",
    Shape.Rect { w, h }         => "rect {w}x{h}",
    Shape.Point                 => "point",
}

match code {
    200 => "ok",
    404 => "not found",
    else => "other",
}
```

- **Exhaustiveness.** The compiler reports a missing case.
- **Destructuring** of structs, enums, tuples, and arrays.
- **Guards** (`if` after a pattern), **or-patterns** (`A | B`), **ranges** (`1..=9`), **bindings** (`n @ 1..=9`), and `_` wildcards inside patterns.
- **`else` arm** as the final catch-all. A lint warns when `else` is used on an enum from the same package, because it hides newly added variants.
- **Short forms:** `if let Some(x) = opt { ... }` and `let Some(x) = opt else { return ... }`.

## Collections and iteration

Collections have Kotlin-style methods that run eagerly and return owned lists, and `.iter()` starts a Rust-style lazy chain for performance-critical code.

**`Iterable` and `Iterator`**

- `Iterable` is implemented by `List`, `Map`, `Set`, arrays, ranges, and so on. It provides `iter(self)` (borrowing) and `into_iter(own self)` (consuming), plus default methods such as `map`, `filter`, `sum`, `fold`, and `any`.
- `Iterator` is the stateful object returned by `iter()`, with `fn next(mut self): Option<Item>`.
- A collection is not itself an `Iterator`, because `next` would consume it.

**Eager methods** on collections borrow the collection and return owned results, which can be stored and returned freely. Closures receive references, so moving a field out needs a clone or a consuming call:

```
let names = users.filter(|u| u.active).map(|u| u.name.clone())         // borrows users
let names = users.into_iter().filter(|u| u.active).map(|u| u.name)     // consumes users
```

**Lazy chains** start with `.iter()`, allocate no intermediate lists, and end with a terminal call such as `sum()`, `to_list()`, or a `for` loop:

```
let total = orders.iter().filter(|o| o.paid).map(|o| o.amount).sum()
```

A chain that borrows is a second-class type: it can be a local, a parameter, or part of a chain in one function, but it cannot be stored in a field or collection. Chains that own their data, such as ranges and `into_iter()`, have no such restriction. A lint suggests `.iter()` when it finds a long chain of eager calls.

**`for` loops** run over any `Iterable` through `iter()`:

```
for order in orders { ... }
```

## Comptime and build.bd

`comptime` runs ordinary code during compilation and replaces macros, `#ifdef`, and most code generation, while `build.bd` is an optional script for anything that needs real side effects.

**What comptime does**

- `comptime` parameters, blocks, and values; types are ordinary values at compile time.
- `comptime if` and `comptime for` for conditional and unrolled code.
- `const` values are evaluated at compile time.
- Reflection is exposed as members of the type: `Self.fields`, `Self.variants`, `Self.name`, `Self.size`, `Self.align`. These names are reserved on types, and the compiler reports a conflict if a type defines its own.
- `comptime assert(...)` and `embed("file")` (the compiler records the file as a build input).
- Comptime may call any normal function and may allocate with a comptime allocator.

**Constraints.** Comptime is pure and deterministic. It has no file, network, command, or environment access apart from `embed`, so caching and reproducible builds stay simple and the language server can run it safely. A configurable step budget turns an infinite loop into a compile error. Comptime cannot generate new declarations (types or functions); it fills in bodies and computes values. Declaration generation may be added later.

**Examples**

A lookup table built at compile time:

```
const CRC_TABLE: [u32; 256] = comptime {
    let mut table = [0; 256]
    for i in 0..256 {
        let mut c = i as u32
        for _ in 0..8 {
            c = if c & 1 == 1 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 }
        }
        table[i] = c
    }
    table
}
```

A derive written with reflection (`x.{name}` is the access to a field by a compile-time name; milestone 21, `spec/reflection.md`):

```
derive Eq {
    fn eq(self, other: Self): bool {
        comptime for field in Self.fields {
            if self.{field.name} != other.{field.name} { return false }
        }
        true
    }
}
```

Compile-time checks, const generics, platform branches, and embedded files:

```
comptime assert(Self.size <= 64, "packet must fit in a cache line")

struct Matrix<T, const R: usize, const C: usize> { data: [T; R * C] }

fn path_separator(): str {
    comptime if Target.os == Os.Windows { "\\" } else { "/" }
}

const LICENSE: str = embed("LICENSE.txt")
```

**`build.bd`** is an optional program that runs before compilation, like Rust's `build.rs`. It can run commands, generate files, compile C code, and run `burdin bindgen`. The package's own `build.bd` is trusted. A dependency's `build.bd` needs an explicit grant in the manifest.

## Concurrency

Concurrency uses goroutine-style lightweight tasks on a cooperative scheduler, with channels for communication and no `async`/`await`, so there is no function coloring.

**Tasks.** `spawn(|| ...)` creates a task. A work-stealing scheduler runs many tasks on a pool of OS threads. The scheduler is part of `std`.

**Channels.** `Channel<T>` is typed, and sending a value moves it into the channel, so nothing is shared. A `select` construct waits on several channels at once.

**I/O inside tasks looks blocking** but pauses only that task. The standard library wraps sockets with the operating system's event facility (epoll, kqueue, IOCP). File access runs on a helper thread pool. An `extern` function that blocks can be marked `blocking`, so the scheduler moves it to a separate thread.

**Sharing data** needs explicit types: `Shared<T>` (reference-counted), `Mutex<T>`, and atomics. A `Send`-style marker is derived automatically and decides which types can move to another task or thread.

```
let handle = spawn(|| { work(data) })      // `data` moves into the task

scope(|s| {                                // scoped tasks may borrow; they finish before the scope ends
    s.spawn(|| process(mut left_half))
    s.spawn(|| process(mut right_half))
})
```

Plain `spawn` closures capture by move. Scoped tasks may borrow disjoint pieces.

**Scheduling is cooperative.** Tasks yield at channel operations, I/O, locks, and an explicit `yield()`. A CPU-heavy loop with none of these can monopolize a worker thread, so CPU-bound work uses `thread.spawn` (a real OS thread). Compiler-inserted preemption checks can be added later without changing user code.

**Stacks.** Each task gets a stack reserved in virtual memory (for example 256 KB) that the operating system commits as it is used. This supports hundreds of thousands of tasks on 64-bit systems. A stack overflow hits a guard page and aborts.

Panics in a task abort the process, as everywhere else.

## Modules and packages

Every file is a module, folders group modules, and a package is the unit that is published and depended on. There are no `mod` declarations.

```
src/
  main.bd            -> module main
  parser.bd          -> module parser
  net/
    http.bd          -> module net.http
    tcp.bd           -> module net.tcp
```

**Imports**

```
use std.json
use net.http.{Client, Request}
use net.http as web
```

A small prelude (`Option`, `Result`, `List`, `String`, and similar) is imported automatically.

**Visibility.** Everything is public by default, and `priv` is the only visibility keyword. `priv` means package-private: the item is visible to every file in the same package and hidden from other packages.

- Functions, structs, enums, interfaces, constants, and modules marked `priv` are not exported.
- A `priv` struct field can be read and written by any file in the package. A struct with `priv` fields cannot be built with a literal by other packages, which use the package's constructor functions.
- A `priv` method is callable only inside the package.
- Enum variants cannot be private, because they are part of the type's shape; the whole enum can be `priv`.
- `priv` does not protect invariants between files of the same package, only from dependents.

**Entry point.** `fn main()` or `fn main(): Result<()>`. If `main` returns an error, the runtime prints it with its context chain and exits with a non-zero code. Arguments come from `std.env.args()`.

**Packages** are described by `burdin.toml` and locked by `burdin.lock`. Dependencies come from a path or a git URL; a central registry is a later possibility.

## Interop

C interop is done through `extern` declarations and a bindgen tool that generates Burdin declarations, in the style of Kotlin's cinterop. Generated bindings are the raw layer only.

**Manual declarations**

```
extern "C" {
    fn strlen(s: CPtr<CChar>): CSize
}
```

Raw C pointers (`CPtr<T>`) can be used only inside `unsafe`.

**Bindgen from C headers.** `burdin bindgen sqlite3.h --out src/sqlite3.bd` reads a header (through libclang) and generates:

- functions as `extern "C"` declarations
- structs as Burdin structs with C-compatible layout
- C enums as enums with explicit values
- typedefs as type aliases
- `#define` constants as `const`
- function pointers as function types

It can be run from `build.bd` so bindings regenerate when a header changes. Safe wrappers (for example a `Drop` type around a database handle) are written by hand on top of the raw layer. Automatic wrapper generation is not planned for now.

**Rust interop** goes through the C ABI, because Rust has no stable ABI. `burdin bindgen --rust some_crate` reads the crate's public API (using rustdoc's JSON output) and generates Burdin types for `extern "C"` functions and `#[repr(C)]` types. Other Rust items (generics, traits, `Vec`) would need generated C-ABI shims on the Rust side, which is a later step.

## Testing

Tests are written next to the code with `test` blocks, run with `burdin test`, and fail through errors instead of panics.

```
test "add works" {
    assert_eq(add(2, 3), 5)?
}

fn check_valid(user: User): Result<()> {
    assert(user.name.len() > 0)?
    Ok(())
}

test "user is valid" {
    check_valid(make_user())?
}
```

- Test blocks are stripped from normal builds, and they can reach package-private items because `priv` is package-wide.
- A test may carry attributes after its name: `[ignore]`, `[timeout(ms)]`, `[should_panic]`/`[should_panic("text")]`, `[should_fail]`, and `[differential]`. They are specified in `spec/test-runner.md`.
- A test body is implicitly a function returning `Result<()>`. `assert`, `assert_eq`, and similar return an error, and the explicit `?` propagates it, including from helper functions that return `Result`.
- Every test runs with the leak-detecting allocator, so a leaked allocation fails the test.
- Real panics (out-of-bounds access, overflow in debug builds, an explicit `panic()`) still abort the process. The runner notes which test was running, marks it failed, and the remaining tests still run. Stage 0 runs each test in its own process, which gives the same result without restarting the binary.
- Doc-comment examples that run as tests, and `bench` blocks for benchmarks, can be added later.

## Standard library and ecosystem

`std` is meant to be as complete as possible and assumes a heap and an operating system, with official packages for heavier use cases that are versioned separately from the language. The scope is large and will be built over a long time, in the order the author's own projects need.

**`std`**

- Collections: `List`, `Map`, `Set`, `Deque`, priority heap.
- Text: `String`, `str`, Unicode, formatting.
- I/O: files, paths, processes, environment, time.
- Concurrency: tasks, channels, `Mutex`, atomics, `Shared<T>`.
- Memory: allocators, `Heap<T>`, `Arena`.
- Basic math: numeric functions, constants, random numbers, hashing.
- Errors and testing support.
- The serialization framework.

**Official packages**

- Formats: JSON, XML, TOML, YAML, CSV.
- Networking: HTTP client and server, TLS, WebSockets.
- Image: PNG, JPEG, and basic processing.
- Math: linear algebra, statistics, vectors and matrices.
- Others: regex, compression, cryptography, database drivers.

**Serialization framework.** `Serialize` and `Deserialize` are derivable interfaces in `std`, in the spirit of Serde and Kotlin serialization. Each format package implements only the format side, so a struct with `derives (Serialize, Deserialize)` works with every format.

**Personal ecosystem.** Long-term goals are a UI library and a TUI library written in Burdin. The TUI library needs raw terminal I/O, tasks, and channels. The UI library mostly wraps an existing graphics or windowing library through bindgen. Neither is planned to be production-ready.

## Tooling and compiler

A single command-line tool, `burdin`, handles building, testing, formatting, and linting, and the compiler is written in Rust and uses LLVM for every build. It stays in Rust at least until the language is mature and its core features are complete.

**The `burdin` tool**

| Command | Purpose |
| --- | --- |
| `burdin new <name>` | Create a package |
| `burdin build`, `burdin run` | Compile, and compile and run |
| `burdin test` | Run `test` blocks |
| `burdin fmt` | Format code (opinionated, no options) |
| `burdin lint` | Run the built-in linter |
| `burdin doc` | Generate documentation |
| `burdin add <package>` | Add a dependency |
| `burdin bindgen <header>` | Generate C bindings |

- **Manifest:** `burdin.toml` (TOML). **Lockfile:** `burdin.lock`, for reproducible builds.
- **Language server:** the compiler is built as a query-based library from the start (like rust-analyzer), so the editor and the command line share one analysis.
- **Linter:** built into the compiler (for example the `else`-on-own-enum warning and the "use `.iter()`" suggestion).

**Compiler architecture**

1. Source files are parsed into an AST and then a typed intermediate form.
2. The typed form is lowered to a MIR (mid-level IR), where the parameter-convention checks, drop insertion, and generic instantiation happen.
3. Backends and the comptime interpreter both read the MIR, so comptime behavior matches compiled behavior.
4. LLVM generates code for all builds: the lowest optimization level for debug, higher levels for release. There is no Cranelift backend.

**Build speed.** LLVM at its lowest level is slower than a dedicated fast backend, so speed comes from elsewhere: a fast front end, parallel code generation per module, cached object files so only changed modules rebuild, and a fast linker. A custom debug backend is a possible future project.

**Implementation plan**

1. **Stage 0:** a Rust compiler for the core subset of the language, enough to write compiler-sized programs: structs, enums, `match`, generics with interfaces, `List`, `Map`, `String`, file I/O, `Result` with `?`, and arenas with handles. It enforces the memory-model checks (parameter conventions, exclusivity, drops) from the start and skips tasks and comptime at first.
2. **Stage 1:** the same Rust compiler grows the rest of the core language: closures and iterators, formatting and interpolation, `error` declarations, comptime, allocators, and then tasks.

**No self-hosting for now.** Burdin is not bootstrapped into itself. The compiler is mostly written by AI, which has no prior experience with Burdin, and a self-hosted compiler would mean building every feature twice and debugging two unproven things at once (the program and the compiler compiling it). Self-hosting may be reconsidered once the language is mature and at least its core features are complete. It is not a goal that shapes the language or the compiler before then.

**Working with AI.** A written specification plus a large suite of example programs and tests is the most valuable asset, because it lets each piece the AI writes be checked and keeps regressions visible as the language changes. Build a thin vertical slice first, then widen it.

## Open questions and future ideas

The design is settled at the level of ideas, and these details still need decisions before or during the first implementation.

**Syntax placeholders to fix in the grammar**

- Access to a field by a compile-time name (`self.{field.name}` in the derive example).
- The declaration form for derives (`derive Eq { ... }`) and how user-defined derives are registered.
- Slice and array type syntax (`[T]`, `[T; N]`), tuples, the unit type, ranges, labeled loops, and type aliases.
- Exact syntax for `select`, channel creation, and `extern` blocks (`CPtr`, `blocking`). (Settled for `select` and
  channels in stage 2.2: `spec/channels.md`.)

**Semantics to write down precisely**

- The exact borrow rules: exclusivity checking, two-phase borrows, reborrowing, partial moves, and how closures capture variables.
- Drop rules for temporaries and conditional moves (drop flags).
- The single-source return rule for references and views, and the diagnostics when it is ambiguous.
- How second-class types propagate through generics and interfaces.
- What `unsafe` may do, and what a safe wrapper must guarantee.
- The `Send` marker: name, derivation rules, and how it interacts with `Shared<T>`. Decided in stage 2.3 (`spec/shared.md` SH2, SH4, SH5): `Send` and `Sync`, derived from a type's parts, declared with `unsafe` impl blocks.

**Possible later additions**

- Declaration generation in comptime (new types and functions).
- Compiler-inserted preemption checks for tasks.
- Fallible allocation (`try_` variants) if a program needs to survive out-of-memory.
- Doc tests and `bench` blocks.
- Direct Rust interop through generated C-ABI shims.
- A custom fast debug backend.
- A package registry.

**Next steps**

1. Write the grammar and a first specification from this document.
2. Collect example programs and expected outputs, which become the conformance test suite.
3. Define the stage 0 subset precisely and build the Rust compiler for it end to end (a vertical slice).
4. Grow the same Rust compiler through stage 1 until the core features are complete. Self-hosting is not planned.
