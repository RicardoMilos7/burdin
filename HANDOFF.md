# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second, third, fourth, fifth, and sixth. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the sixth session (2026-11-14). **Milestones 1 to 6 are done**: all tests in `tests/m01_hello`
through `tests/m06_conventions` pass in the debug and release profiles (95 tests; one m06 test is debug-only).

| What | Where | Status |
| --- | --- | --- |
| Full language design | `Burdin Language Design.md` | Written. Local copy is newer than the one on claude.ai |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written. `Burdin Stage 0 Subset.md` in the root is an old export, safe to delete |
| Grammar | `spec/grammar.md` | Parser follows it except generic arguments in expressions (rule 8.2). Items other than `interface`, `use`, `extern`, and `test` parse |
| Memory rules | `spec/memory-rules.md` | Moves, `Drop`, drop order and flags, and `Heap` (milestone 5); conventions, exclusivity, references, views, and return origins (milestone 6). All stage 0 memory rules are implemented |
| Conformance tests | `tests/` | 131 tests. 95 pass today: m01 to m06 |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_sema`, `burdin_codegen`, `burdin_driver` (`burdin` binary) |
| Runtime | `crates/burdin_codegen/src/runtime.rs` | `main`, output, panics (`burdin_rt_panic`, `burdin_rt_panic_bounds`), `String` allocation/clone/push, heap allocation (`burdin_rt_alloc`, `burdin_rt_free`), number/char/str to `String`, `str` `starts_with` and `find` (byte offsets, UTF-8 aware). Generated as LLVM IR into every program; calls libc (`write`, `malloc`, `realloc`, `free`, `memcmp`, `abort`). `cc` only links |
| CI | `.github/workflows/ci.yml` | Linux only: check-tests, fmt, clippy, unit tests, conformance tests of finished milestones. Never run yet (no remote) |
| LLVM | WSL: `/usr/lib/llvm-21` | LLVM 21.1.8 from Ubuntu 26.04 |

Dev environment: WSL Ubuntu 26.04, stable Rust through rustup (1.100 at the time), LLVM 21. See the Toolchain section of `CLAUDE.md` for the
exact packages and how to call WSL from PowerShell.

### How the compiler is built so far

- `burdin_syntax`: complete lexer; parser for functions, constants, structs, enums, type aliases, method blocks
  (`Type { ... }` and `Type: Interface { ... }`), types, statements, struct literals (`P { x }`, `Shape.Rect { w: 1 }`),
  `if let`, `let ... else`, and the full expression precedence table with the newline rules of section 2 and
  statement-level and item-level error recovery. `interface`, `use`, `extern`, and `test` items are reported as E9000
  and skipped whole.
- `burdin_sema`: resolves functions, constants, locals, and the built-ins (`print`, `println`, `eprint`, `eprintln`, `panic`),
  and produces a typed tree (`tir`) where every expression has a type and a span. Types: `()`, `bool`, `char`, all
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
  type is never `Copy`; deriving both is E0130. `burdin_sema/src/moves.rs` is the move checker: one function body at a
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
  `burdin_sema/src/origins.rs` computes each signature's return origin (the single parameter a second-class result
  derives from, if any) to a fixpoint and stores it in `ParamSig.ret_origin`; a result the signature cannot attribute
  is checked against the body: E0121 at the signature for two or more sources, E0122 at the returned expression for a
  local, an `own` parameter, or a temporary.
  `burdin_sema/src/borrows.rs` is the borrow checker (RF2, RF3, RF5): a loan records the place, whether it is mutable,
  and the locals that hold the reference, a copy of it, or a view derived from it; liveness is the last top-level
  statement that uses a holder (conservative inside loops, no CFG needed); an overlapping read of a mutable loan, or
  any overlapping write, `&mut`, or move while a loan is live is E0111. A call whose second-class result derives from
  an argument keeps that argument borrowed until the result's last use. Exclusivity (EX1, EX2) is checked in
  `burdin_sema` per call, receiver included, over the places each argument mentions (E0110). `match` on a reference or
  a slice binds views (MV8), and the move checker rejects moving out of borrowed content with E0101.
  Slicing `p[lo..hi]` works on arrays, `str`, `String`, and slices (bounds-checked, panicking on a bad range); `for`
  iterates slices; `as_str`, `len`, `starts_with`, and `find` are built-in methods.
- Exhaustiveness (`burdin_sema/src/exhaust.rs`) is the usefulness algorithm over a pattern matrix, with integer and
  `char` ranges split at their boundaries. It handles nested tuples, structs, enums, and `Option`.
- `burdin_codegen`: typed tree to LLVM IR to an object file. `str` is `{ ptr, usize }`, `String` is `{ ptr, len, cap }`,
  `Heap<T>` is a `ptr`, `&T` and `&mut T` are a `ptr`, `[T]` is `{ ptr, usize }` like `str`, `Option<T>` is `{ i1, T }`. Functions are `burdin.<name>`, and the generated `main` calls the
  program's `main`. Stack slots live in the `entry` block; after a terminator, generation continues in a fresh unreachable block.
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
- `burdin_driver`: `build` (writes `<stem>` in the current directory, `-o` to change it, `--emit-ir file.ll`), `run`
  (builds into a temporary directory, passes the exit code through, 128+signal for a signal). `test` is not implemented.
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

1. **The tests have never run against a compiler.** Expected outputs were worked out by hand. Expect a few wrong expectations and
   some syntax slips in the first runs. Fix them in the tests only after checking the spec says the same thing.
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
8. **The grammar is only partly tried.** Functions, statements, expressions, and the newline rules work on every test file. Still
   untried: generic arguments in expressions (`List<i32>.new()` versus `a < b`, rule 8.2), which today make those lines fail with
   "comparison operators cannot be chained".
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
12. **`cargo fmt --all --check` fails on files this milestone did not touch** (for example
    `crates/burdin_syntax/src/parser.rs`): the rustfmt from Rust 1.100 lays out some `matches!` calls differently from
    when the tree was formatted. The five files touched in the sixth session are formatted. Either reformat the whole
    tree in one commit or pin the toolchain, so the CI Format step can pass.

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
- The move checker is a separate pass (`burdin_sema/src/moves.rs`), not part of type checking, and it keeps the checker
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

## Decided in the sixth session

- Calling convention: every parameter except `own` is passed by address; `own` is passed by value. That makes a
  returned view of a read-only, `mut`, or `self` parameter refer to the caller's storage (RT1), and a `mut` or `own`
  parameter still sees the caller's place. `ParamSig` keeps `by_ref` (how the value is passed) separate from
  `exclusive` (whether the call borrows the argument exclusively), so the ABI does not make every call exclusive.
- The borrow checker (`burdin_sema/src/borrows.rs`) is statement-granular and holder-based: a loan lives to the last
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

1. Milestone 7 (`tests/m07_generics`): monomorphization, interfaces, `requires`, derives, and the orphan rule; the
   generic stack and `max_of`; derived `Eq`, `Ord`, and `Hash`. The parser still needs generic arguments in expressions
   (`List<i32>.new()`, grammar rule 8.2) for this milestone and m09.
2. Then milestones 8 to 11 in order. After each one, add its folder to the conformance step in `.github/workflows/ci.yml`.
3. Known parser and front-end gaps to close in their milestones: the `interface`, `use`, `extern`, and `test` items
   (m07, m08, m10). `Self` is not yet accepted in expressions (`Self.new()`, `Self { ... }`); no test needs it so far.
4. Push the repository to a remote so CI runs, and add a Windows job once the `cl` link path is tried.
5. After the first real runs, review failing tests together with the assumptions list and settle each assumption in the spec.
6. Before stage 1 (more features in the same Rust compiler, not a rewrite): the MIR interpreter question (comptime needs one),
   the design document's own open-questions list (derive syntax, `select`, extern details), and the stage 1 order (closures
   and iterators first).

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md and HANDOFF.md. Continue with milestone 7.

If the session has no repository access to the folder, attach `CLAUDE.md`, `HANDOFF.md`, and the `spec/` folder.
