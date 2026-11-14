# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second, third, fourth, fifth, sixth, seventh, and eighth. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the eighth session (2026-11-14). **Milestones 1 to 8 are done**: all tests in `tests/m01_hello`
through `tests/m08_modules` pass in the debug and release profiles (113 tests in debug; 112 in release, because one
m06 test is debug-only).

| What | Where | Status |
| --- | --- | --- |
| Full language design | `Burdin Language Design.md` | Written. Local copy is newer than the one on claude.ai |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written. `Burdin Stage 0 Subset.md` in the root is an old export, safe to delete |
| Grammar | `spec/grammar.md` | Parser follows it, including generic arguments in expressions (rule 8.2), `interface`, `use`, and `extern` items. `test` items are still skipped as E9000 |
| Memory rules | `spec/memory-rules.md` | Moves, `Drop`, drop order and flags, and `Heap` (milestone 5); conventions, exclusivity, references, views, and return origins (milestone 6). All stage 0 memory rules are implemented |
| Conformance tests | `tests/` | 131 tests. 113 pass today: m01 to m08 |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_tir`, `burdin_sema`, `burdin_check`, `burdin_codegen`, `burdin_driver` (`burdin` binary); see the layout in `CLAUDE.md` |
| Runtime | `crates/burdin_codegen/src/runtime.rs` | `main`, output, panics (`burdin_rt_panic`, `burdin_rt_panic_bounds`), `String` allocation/clone/push, heap allocation (`burdin_rt_alloc`, `burdin_rt_free`), `CString` allocation (`burdin_rt_cstring_from`), number/char/str to `String`, `str` `starts_with` and `find` (byte offsets, UTF-8 aware). Generated as LLVM IR into every program; calls libc (`write`, `malloc`, `realloc`, `free`, `memcmp`, `abort`). `cc` only links |
| CI | `.github/workflows/ci.yml` | Linux only: check-tests, fmt, clippy, unit tests, conformance tests of finished milestones. Never run yet (no remote) |
| LLVM | WSL: `/usr/lib/llvm-21` | LLVM 21.1.8 from Ubuntu 26.04 |

Dev environment: WSL Ubuntu 26.04, stable Rust through rustup (1.100 at the time), LLVM 21. See the Toolchain section of `CLAUDE.md` for the
exact packages and how to call WSL from PowerShell.

### How the compiler is built so far

- `burdin_syntax`: complete lexer; parser for functions, constants, structs, enums, type aliases, method blocks
  (`Type { ... }` and `Type: Interface { ... }`), types, statements, struct literals (`P { x }`, `Shape.Rect { w: 1 }`),
  `if let`, `let ... else`, and the full expression precedence table with the newline rules of section 2 and
  statement-level and item-level error recovery. `use` items (path, `as`, and `.{...}`) and `extern "C"` blocks are
  parsed; `test` items are still reported as E9000 and skipped whole.
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
  merged, and a guard that fails restores the drop flags of the bindings its pattern moved. Moves it cannot follow are
  rejected with E9000 instead of being miscompiled: moving a non-`Copy` value through indexing or a `Heap`
  dereference, moving a value out of `match *h`, and a moving pattern with a guard on a temporary scrutinee. It does
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
  private method gets E0150. `extern "C" { fn ... }` registers C functions by their plain symbol; calls require an
  `unsafe` block or `unsafe fn` (E0161) and use the C ABI with by-value parameters limited to integers, floats, and
  `CPtr<T>`. `unsafe` blocks are ordinary block expressions with an unsafe depth, and `unsafe fn` bodies are
  unsafe. `CPtr<T>` is a raw, `Copy` pointer; `CPtr.null()` takes its type from context. `CString` is an owning,
  NUL-terminated buffer (`CString.from(str)`, `.as_ptr(): CPtr<CChar>`) with `Drop`; `CChar`, `CSize`, and `CVoid`
  are the C names (`i8`, `usize`, and unit behind a pointer). Extern declarations are deduplicated by symbol, and a
  second declaration with a different signature is an error.
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
- `burdin_driver`: `build` (writes `<package name>` in the current directory, `-o` to change it, `--emit-ir file.ll`),
  `run` (builds into a temporary directory, passes the exit code through, 128+signal for a signal). A target is a
  `.bd` file or a package directory; dependency cycles, missing paths, invalid module names, and malformed manifests
  are reported as `error: ...` without diagnostics codes. `test` is not implemented.
- Diagnostics are printed in source order as `path:LINE:COL: error[E0000]: message`. Semantic checks run only if there
  were no syntax errors.

## Decisions already made (do not reopen without a reason)

- Compiler: Rust, LLVM only (no Cranelift; lowest optimization level for debug). It stays in Rust: no self-hosting, at least
  until the language is mature and its core features are complete (decided in the sixth session, 2026-11-14).
- Platforms: Linux, macOS, Windows; x86-64 and arm64; MSVC toolchain on Windows.
- Memory model: value semantics, single owner, parameter conventions (read-only default, `mut`, `own`), second-class references
  and views, automatic `Drop`, `Heap<T>`, global allocator, abort on panic.
- Exclusivity works on places: field paths of a local or parameter are separate places. Indexing, `Heap` dereference, and method
  calls are not (option C). Recorded in the design document and as assumption 16 in `tests/README.md`.
- Stage 0 has no closures, string interpolation, `any`/`impl`, `error` declarations, comptime, tasks, or leak detection.
- Syntax choices: `fn f(): T`, `let` / `let mut`, `Type: Interface { }`, `derives (...)`, `priv` means package-private,
  `?` and `orelse`, `else` arm in `match`, call-site `mut` marker.

## Decided in the second session

- LLVM route 1 (WSL Ubuntu first) and LLVM 21, through `inkwell` 0.11.0 with the `llvm21-1-prefer-dynamic` feature.
- No Rust toolchain pin for now: build with current stable (edition 2024 needs 1.85 or later).
- `E9000` is a temporary code for "not implemented in this compiler yet". It is not in the registry and no test uses it.

## Known problems and risks

1. **The expected outputs were written by hand** before the compiler existed; milestones 1 to 8 now pass against it, but a
   passing test still only reflects the implementation matching those expectations, not the design document. Review them
   with the assumptions list before trusting the suite as a spec of intent.
2. **Many tests depend on standard-library names I chose** (`String.from`, `push_str`, `i32.parse`, `find`, `find_str`, `env.args()`,
   `CString.from`, `CPtr.null()`, and others). They are listed in assumption 7 of `tests/README.md`. They are guesses, not decisions.
3. **15 other assumptions in `tests/README.md` are mine, not yours.** The risky ones: call-site `mut` for `mut` parameters,
   `String` coercing to `str` as the only implicit conversion, views being copyable, `for` borrowing the collection, and string
   literals being static.
4. **Rules I made up while writing the specs** (see the end of the last session's summary): newline continuation only before
   `.`, `?`, `else`, `orelse`, `as`, `&&`, `||`; `own` parameters are immutable bindings; a block's tail-expression temporaries drop
   before its locals; `orelse` binds looser than `||`; comparison operators do not chain.
5. **The driver now implements the runner's contract** for `build` and `run`; `test` waits for milestone 10.
6. **`m11_llvm/llvm_return_42.bd` stops short** of emitting an object file and linking, because the target-initialization functions
   are architecture-specific. The stage 0 milestone says "link and run", so extend it once the compiler can run the current version.
7. **Windows-specific things are untested:** binary-safe file I/O (no newline translation), `_write` versus `write`, abort exit codes
   (`run-abort` only checks "non-zero"), and the `run-tests.sh` script needs Git Bash.
8. **The grammar is mostly tried now.** Generic arguments in expressions (`Stack<i32>.new()`, rule 8.2), `use`, and
   `extern "C"` are exercised by m07 and m08. Not covered by a conformance test yet: `unsafe fn` declarations (they
   work; tested by hand), `interface` methods that are `unsafe`, and the `test` item (m10).
9. **Two documents on claude.ai are stale.** The design doc there lacks the exclusivity rule, and the stage 0 doc there is the old
   export. The local files are the source of truth.
10. **Ownership gaps that no test exercises yet.** Moving a field out of a temporary `match` scrutinee is still not
    tracked, so the temporary's other fields are leaked. The move forms the checker cannot follow are rejected with
    E9000 rather than miscompiled: moving a non-`Copy` value through indexing or a `Heap` dereference (a field of an
    array element or of the pointee), moving a value out of `match *h`, and a moving pattern with a guard on a
    temporary scrutinee. A `match` on an owned local or an `own` parameter is tracked normally. Revisit when m09's
    `List`/`Map` make these reachable.
11. **A view can outlive the storage it borrows from without a diagnostic.** No registry code covers it, and the spec
    only states the function-return case (RT2). Examples: `let r = first_of(Pair { a: 1, b: 2 })` (the temporary drops
    at the end of the `let`, DR5), `let r = { let p = Pair { a: 1, b: 2 }; first_of(p) }` (the source local dies with
    the block), and `let t = "x".to_string().as_str()`. The borrow checker creates no loan for a temporary and has no
    notion of a source's scope ending, and origin inference gives a temporary argument an empty origin. Decide the
    rule and its error code first, then implement. E0122 at the binding is the closest code, but its registry text says
    "returned".
12. **`cargo fmt --all --check` now passes** (the tree was reformatted in the eighth session with the current rustfmt), but
    the toolchain is still not pinned, so a future rustfmt can break the CI Format step. Pinning the toolchain or running
    `cargo fmt --all` in each milestone are the two options.
13. **The module and C bridge is narrower than the design.** There is no syntax for a private module, no module
    visibility check, and no check that public signatures only use public types. `CPtr` supports only `null()` and
    `CString.as_ptr()`: no dereference, null test, pointer arithmetic, or comparison. `extern` accepts only integers,
    floats, and `CPtr` (no C `bool`/`char`/structs/unions/function pointers/varargs), and only the `C` ABI. `CString`
    has no clone and there is no `CStr` yet (m11's `CStr.to_string` needs it). `unsafe` is required only for calls, not
    for holding or moving a `CPtr`. There is no way to re-export an imported name (no `pub use`), and no module
    visibility. The manifest parser is deliberately minimal (path dependencies, no versions, no lockfile). A user
    `extern` whose symbol collides with a runtime libc declaration of a different type is a backend error printed
    without an error code or source span.

## Decided in the third session

- Diagnostics without a registry code use these for now: `break`/`continue` outside a loop is E0001; assigning to
  something that is not a place is E0005; a constant that depends on itself and a non-constant expression in a `const`
  are E9000. Give them registry codes when the spec settles them.

## Decided in the fourth session

- Tuples were implemented with milestone 4, which closed the last milestone 3 gap (`let (p, q) = (1, "two")`). No test
  was changed.
- More diagnostics without a registry code: duplicate definitions (types, variants, fields, methods), a type that
  contains itself by value, and a type alias cycle are E9000; a missing or repeated field in a struct literal, an
  `else` block of `let ... else` that can finish normally, and an irrefutable pattern with `else` are E0003; an unknown
  field or variant is E0002; a refutable `let` or `for` pattern without `else` is E0006; calling a `mut self` method on
  an immutable place is E0113 (the same rule as a `mut` argument).

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

## What is left to do, in order

1. Milestone 9 (`tests/m09_stdlib`): `List<T>`, `Map<K, V>`, `Result<T, E>`, `?`, `String` methods, file I/O, and
   processes. `List` and `Map` resolve to `Ty::Error` today, and `for x in list` panics in `moves.rs` ("a `for` array
   has an array type"); fix both when the containers arrive. They can reuse the generic machinery: a built-in generic
   type is a `GenericAdt` whose declaration comes from the standard library. m11 additionally needs `CStr.to_string`
   and out-parameters through `CPtr<CPtr<CChar>>`, which the current `CPtr` support already types.
2. Then milestones 10 and 11 in order. After each one, add its folder to the conformance step in
   `.github/workflows/ci.yml`.
3. Known parser and front-end gaps to close in their milestones: the `test` item (m10). `Self` is not yet accepted in
   expressions (`Self.new()`, `Self { ... }`); no test needs it so far.
4. Generic gaps no test exercises yet: a static method of a generic type cannot infer its arguments from its own
   arguments (`Holder.make(x)`; write `Holder<T>.make(x)` or give an expected type), bounds written on a generic type's
   parameters are not checked when the type is instantiated (they only feed the symbolic check of its impls), a view
   type as a type argument is not second-class (memory-rules SC3), interface method signatures in an impl are not
   compared with the interface's, and `==` or `<` on a generic type's symbolic instance (`Wrapper<T> == Wrapper<T>` in
   a generic body) is rejected.
5. Module and C gaps no test exercises yet: no pointer intrinsics beyond `CPtr.null()` and `.as_ptr()` (no
   dereference, null test, or pointer comparison); no C `bool`, `char`, structs, unions, function pointers, or
   varargs across `extern`; no `unsafe` requirement for merely holding a `CPtr`; no check that public signatures use
   public types; a dependency's non-root modules are reachable by path (`use lib.helpers.x`) even though there is no
   module visibility syntax.
6. Push the repository to a remote so CI runs, and add a Windows job once the `cl` link path is tried.
7. After the first real runs, review failing tests together with the assumptions list and settle each assumption in the spec.
8. Before stage 1 (more features in the same Rust compiler, not a rewrite): the MIR interpreter question (comptime needs one),
   the design document's own open-questions list (derive syntax, `select`, extern details), and the stage 1 order (closures
   and iterators first).

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md and HANDOFF.md. Continue with milestone 9.

If the session has no repository access to the folder, attach `CLAUDE.md`, `HANDOFF.md`, and the `spec/` folder.
