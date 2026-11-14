# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second, third, fourth, fifth, sixth, seventh, eighth, ninth, tenth, eleventh, twelfth, thirteenth, fourteenth, and fifteenth. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the fifteenth session (2026-11-14). **Milestones 1 to 11 are done, so stage 0 is complete**: all
134 tests pass in the debug profile and 133 in release (one m02 test is debug-only). `burdin test` runs `test` blocks,
and the m11 program calls the LLVM C API. The eleventh session added the macOS CI job (issues #16, #17, #18); the
twelfth deleted the stale `Burdin Stage 0 Subset.md` root export (issue #43) and documented the driver contract in a
root `README.md`, checked by `tests/check-docs.sh` (issue #42); the thirteenth retired the claude.ai copies of the
design and stage 0 documents, added the compiler benchmarks, and cached the cargo registry, `target/`, and LLVM 21 in
CI (#44, #41, #46); the fourteenth added the Windows CI job and the shared LLVM link (issue #12); the fifteenth runs
the finished conformance milestones in the release profile in every CI job (issue #47) and adds benchmark regression
tracking: the harness saves and compares a baseline, and each CI job keeps one in a cache and reports regressions
advisably (issue #446).

| What | Where | Status |
| --- | --- | --- |
| Driver contract | `README.md` | Written: targets and `burdin.toml`, `build`/`run`/`test`, diagnostics, `--emit-ir`, `--link-lib` and the `LLVM-C` resolution, `BURDIN_CC`, and exit statuses. `tests/check-docs.sh` checks the links and the documented behavior, and both CI jobs run it |
| Full language design | `Burdin Language Design.md` | Written. The repository copy is canonical; the copy on claude.ai is historical |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written and complete |
| Stage 1 plan | `spec/stage1-subset.md` | Written at the end of the tenth session: milestones 12+, open decisions, and the roadmaps for later stages |
| Process and verification | `AGENTS.md`, `CHANGELOG.md` | Shared definition of done, milestone close-out checklist, verification policy, and shipped milestones. Process rules live here, not as GitHub issues |
| Grammar | `spec/grammar.md` | Parser follows it, including `test` items, generic arguments in expressions (rule 8.2), `interface`, `use`, and `extern` items |
| Memory rules | `spec/memory-rules.md` | Moves, `Drop`, drop order and flags, and `Heap` (milestone 5); conventions, exclusivity, references, views, and return origins (milestone 6). All stage 0 memory rules are implemented |
| Conformance tests | `tests/` | 134 tests. All pass: m01 to m11 on Linux, macOS, and Windows |
| Unit tests | `crates/*/src` | Focused tests for the front end: the lexer token stream (`burdin_syntax/src/lexer.rs`), parser recovery (`burdin_syntax/src/parser/mod.rs`), type unification (`burdin_sema/src/generics.rs`), and exhaustiveness (`burdin_sema/src/exhaust.rs`), plus type and place questions in `burdin_tir`, origin and move state in `burdin_check`, and diagnostics in `burdin_diag`. CI runs `cargo test --workspace` in every job, and `tests/check-unit-tests.sh` fails when a front-end crate or one of those four areas loses its tests (issue #45) |
| Standard library | `std/` | The stage 0 std, written in Burdin and embedded in the driver: `std.bd` (error aliases, `CStr`, `assert`, `assert_eq`, `unreachable`, the test-mode assertions), `io.bd`, `env.bd`, `process.bd`, `mem.bd`. Every package gets it as an implicit dependency and every module gets its public names as a prelude |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_tir`, `burdin_sema`, `burdin_check`, `burdin_codegen`, `burdin_driver` (`burdin` binary), `burdin_bench`; see the layout in `CLAUDE.md` |
| Runtime | `crates/burdin_codegen/src/runtime.rs` | `main`, output, panics (`burdin_rt_panic`, `burdin_rt_panic_bounds`), `String` allocation/clone/push/push-char, heap allocation (`burdin_rt_alloc`, `burdin_rt_realloc`, `burdin_rt_free`), `CString` allocation (`burdin_rt_cstring_from`), number/char/str to `String`, `str` `starts_with`, `find`, `find_str`, `eq`, `cmp`, integer parsing, file reads/writes (`fopen`/`fread`/`fwrite`/`fclose`), `CStr.to_string` (`strlen` plus the string allocator), `env.args()`, `process.exit`, and `process.run` (`system`). Generated as LLVM IR into every program; calls libc (`write`, `malloc`, `realloc`, `free`, `memcmp`, `strlen`, `abort`, `exit`, `system`, the `fopen` family). `cc` only links. In a `burdin test` build the generated entry point also calls `atoi` to read the test index from `argv[1]` |
| Benchmarks | `crates/burdin_bench/` | Generated inputs for three sizes; `cargo bench -p burdin_bench` times lexing, parsing, name resolution/type checking, and code generation (median per iteration). `--smoke` checks every stage against generous budgets (issue #41); `--save` and `--baseline` record and compare medians (issue #446) |
| CI | `.github/workflows/ci.yml` | `linux` (`ubuntu-24.04`, LLVM 21 from apt.llvm.org), `macos` (`macos-latest`, Apple Silicon, Homebrew `llvm@21`), and `windows` (`windows-latest`, MSVC through `ilammy/msvc-dev-cmd`, LLVM 21 from the official `clang+llvm-21.1.8-x86_64-pc-windows-msvc` archive). All run check-ci, check-tests, docs checks, fmt, clippy, unit tests, build, benchmark smoke budgets, `tests/check-platform.sh`, and conformance m01-m11 in both the debug and release profiles (`tests/run-tests.sh --release` skips the debug-only overflow test); the Windows job also reruns m01 and m11 with `BURDIN_CC=cl`. All three cache the cargo registry and `target/` under a key built from `rust-toolchain.toml` and `Cargo.lock`, and cache LLVM 21 (the installed tree on Linux, the Homebrew bottle downloads on macOS, the unpacked archive on Windows); `tests/check-ci.sh` guards all three jobs, both profiles, and their caches. Every job also runs `tests/bench-regression.sh`: it compares the full benchmark suite with a baseline cached per runner, toolchain, and lockfile, and records a new one on a main push, advisory for now (issue #446) |
| LLVM | WSL: `/usr/lib/llvm-21`; macOS: Homebrew `llvm@21`; Windows: `C:\llvm` | LLVM 21.1.8. Linked as a shared library on every platform by `crates/burdin_codegen/build.rs` (`llvm21-1-no-llvm-linking`): the static Windows archives crash in a binary that uses the dynamic CRT, so MSVC links `LLVM-C.lib` and needs `LLVM-C.dll` on `PATH` |

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
- Milestone 9 added the standard library. `Result<T, E>`, `List<T>`, `Map<K, V>`, and `Chars` are built-in types
  (`Ty::Result`, `Ty::List`, `Ty::Map`, `Ty::Chars`): `Ok`/`Err` construct and match like `Some`/`None`, `?` lowers
  in sema to a match that returns early through the enclosing `Option`/`Result` and converts the error through a
  user `Type: From<Source>` block, and `orelse` lowers to a match. Built-in methods cover `Option`/`Result`
  (`unwrap`, `expect`, `is_some`, `is_none`, `is_ok`, `is_err`), `String` (`push`, `find_str`, `chars`, comparison
  with `==`), the integer `parse` associated function, and the containers (`List`: `new`, `push`, `pop`, `last`,
  `insert`, `remove`, `clear`, `contains`, `sort`, `len`, indexing, `for`; `Map`: `new`, `insert`, `get`, `remove`,
  `contains_key`, `clear`, `len`, `for` over `(k, v)`). `List` and `Map` use the three-word `String` layout in
  the C heap; `Map` is a linear-scan map, not a hash table. Drop and clone glue, `==`, `Ord`, and the move,
  borrow, and origin checkers understand the new types. `Chars` is only an iterable: `for c in s.chars()` decodes
  UTF-8 in code generation. The prelude also provides file I/O (`read_to_string`, `write_file`), `env.args()`,
  `process.exit`, `process.run`, `assert`, `assert_eq`, `unreachable`, `mem.swap`, and `mem.replace`. The
  `IoError`, `ParseIntError`, and `AssertError` names are aliases for unit in `std`; the `error` declarations of
  stage 1 will give them payloads. The compiler's own primitives behind the std wrappers are the underscored
  built-ins `__read_to_string`, `__write_file`, `__args`, `__exit`, `__run`, `__swap`, and `__replace`, plus a
  small runtime of C calls (`fopen`/`fread`/`fwrite`/`fclose`, `system`, `exit`, `strlen`, `memcmp`, `realloc`).
  The driver embeds `std/` with `include_str!` and adds it as a dependency of every package; `prelude.rs` copies
  its public names and module aliases into every module, keeping a module's own declarations.
- Milestone 10 added the test runner. The parser reads `test "name" { ... }` into `ast::File.tests`, apart from the
  other items, and builds each one as a private function `__burdin_test_N` with no parameters returning
  `Result<(), AssertError>` whose body is the block followed by `Ok(())`. Normal builds never declare or check them
  (tests are stripped); with `burdin_sema::check(..., test_mode, ...)`, the entry package's tests are declared and
  checked, the prelude replaces `assert`/`assert_eq` with `std`'s `__test_assert`/`__test_assert_eq` (which return
  `Err(())` instead of panicking), and `main` is not required. `Program.tests` carries each test's display name and
  `FuncId`. Code generation then emits a dispatcher `main` instead of calling the program's `main`: it reads the test
  index from `argv[1]` with `atoi`, calls that test, and returns 0 when its `Result` is `Ok` and 1 when it is `Err`.
  `burdin test` compiles once into a temporary directory and runs the binary once per test, so a panic aborts only
  the running test; it prints `FAIL <name>` for failures and `test result: N passed, M failed` at the end. The
  panicking `assert` of normal builds is unchanged.
- Milestone 11 added `CStr.to_string`: `CStr` is a `std` alias for `CPtr<CChar>`, and a `CPtr<CChar>` has an
  associated `to_string` that reads the bytes with libc `strlen` and copies them into a `String`
  (`Builtin::CStrToString`). `burdin` links `--link-lib LLVM-C` through `llvm-config`: Ubuntu installs the LLVM C API as
  `libLLVM-21.so`, not `libLLVM-C.so`, so the driver asks `llvm-config --libdir` and `--link-shared --libs` and adds
  `-L` plus an rpath. The m11 program builds and verifies `define i32 @answer() { ret i32 42 }` through the LLVM C API
  and prints the IR. (Emitting an object file from the LLVM C API is still a later step in the stage 1 compiler.)
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
  `run` (builds into a temporary directory, passes the exit code through, 128+signal for a signal), and `test` (builds
  in test mode and runs one process per test; it owns the counts and `FAIL` lines). A target is a `.bd` file or a
  package directory; dependency cycles, missing paths, invalid module names, and malformed manifests are reported as
  `error: ...` without diagnostics codes. `--link-lib LLVM-C` resolves the LLVM shared library through
  `llvm-config`, which it finds through `LLVM_SYS_211_PREFIX` and then `PATH`; it accepts Linux's `libLLVM-C.so` /
  `libLLVM-21.so` and Homebrew's `libLLVM-C.dylib` / `libLLVM.dylib`, and adds `-L` plus an rpath. On MSVC it passes
  `LLVM-C.lib` and adds `/LIBPATH:<LLVM_SYS_211_PREFIX>/lib` when that import library is there. A missing `cl` is
  reported with the fix (a Developer Command Prompt, or `BURDIN_CC`).
- Diagnostics are printed in source order as `path:LINE:COL: error[E0000]: message`. Semantic checks run only if there
  were no syntax errors.

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
fails. CI runs it after `cargo build`, and `tests/check-ci.sh` requires the step and the bench target.

For tighter tracking, `--save FILE` writes a run's medians in a plain-text baseline (`case<TAB>nanoseconds`, one line
per case), and `--baseline FILE` compares a later run with it: each case gets a row with the baseline median, the
current median, the change, and a verdict, and the harness exits 1 when a case is more than `--threshold` percent
slower (default 25; exactly at the threshold is within it). A case with no baseline entry is reported as new and a
baseline entry the run did not measure as missing; neither is a regression. `tests/bench-regression.sh` runs the full
suite and does both, and every CI job runs it. The baseline lives in `~/.cache/burdin/bench`, restored and saved with
`actions/cache` under a key that names the runner OS and architecture, the pinned toolchain, and `Cargo.lock`, so a
development machine is never compared with a runner and a toolchain or dependency change starts a fresh baseline. A
push to main records the new baseline (even when the run flagged a regression, so the baseline follows main); a pull
request compares against the last main run on the same platform and never writes the cache. The comparison is
advisory (`continue-on-error`) until the runner numbers are stable: a regression is shown in the job log without
failing the job. A missing baseline (including one evicted after seven days without a run) is recorded and passes.
`tests/check-ci.sh` fails if the step, the script, the cache, or the harness modes go missing, and
`cargo test -p burdin_bench` covers the format and the threshold arithmetic.

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

- LLVM route 1 (WSL Ubuntu first) and LLVM 21, through `inkwell` 0.11.0. The feature was `llvm21-1-prefer-dynamic`; in
  the fourteenth session it became `llvm21-1-no-llvm-linking` with the link step in `crates/burdin_codegen/build.rs`,
  because `llvm-sys` falls back to the static Windows archives, which crash in a binary built with the dynamic CRT.
- No Rust toolchain pin for now: build with current stable (edition 2024 needs 1.85 or later). *Superseded on
  2026-11-14 by issue #19: `rust-toolchain.toml` pins `1.100.0` with `clippy` and `rustfmt` (see problem 12).*
- `E9000` is a temporary code for "not implemented in this compiler yet". It is not in the registry and no test uses it.

## Known problems and risks

1. **The expected outputs were reviewed on 2026-11-14 (issue #20).** Every test in every milestone folder was read
   against the design, the grammar, the memory rules, the stage 0 subset, and the assumptions list. No expectation was
   found to encode an accidental behavior, so no test changed; every intentional choice or deviation the suite pins is
   recorded in `tests/README.md` under "Intentional choices and deviations". A new test still needs the same review: a
   passing test proves the implementation matches the expectation, not the design document.
2. **Many tests depend on standard-library names I chose** (`String.from`, `push_str`, `i32.parse`, `find`, `find_str`, `env.args()`,
   `CString.from`, `CPtr.null()`, and others). They are listed in assumption 7 of `tests/README.md`. They are guesses, not decisions.
3. **15 other assumptions in `tests/README.md` are mine, not yours.** The risky ones: call-site `mut` for `mut` parameters,
   `String` coercing to `str` as the only implicit conversion, views being copyable, `for` borrowing the collection, and string
   literals being static.
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
   tested.
8. **The grammar is mostly tried now.** Generic arguments in expressions (`Stack<i32>.new()`, rule 8.2), `use`,
   `extern "C"`, and `test` items are exercised by m07, m08, and m10. Not covered by a conformance test yet:
   `unsafe fn` declarations (they work; tested by hand) and `interface` methods that are `unsafe`.
9. **The claude.ai copies are historical (issue #44).** The design document there lacks the exclusivity rule, and the
   stage 0 document there is the old export. The repository files are the only maintained copies and say so at their
   tops (`CLAUDE.md`, `AGENTS.md`, `Burdin Language Design.md`, and `spec/stage0-subset.md`). Deleting or archiving the
   claude.ai pages themselves is a manual action for the owner, who has the account access; the repository has no link
   to them, so nothing here depends on those pages.
10. **Ownership gaps that no test exercises yet.** Moving a field out of a temporary `match` scrutinee is still not
    tracked, so the temporary's other fields are leaked. The move forms the checker cannot follow are rejected with
    E9000 rather than miscompiled: moving a non-`Copy` value through indexing or a `Heap` dereference (a field of an
    array element or of the pointee), moving a value out of `match *h`, and a moving pattern with a guard on a
    temporary scrutinee. A `match` on an owned local or an `own` parameter is tracked normally. Milestone 9's
    `List`/`Map` make moving a non-`Copy` element through indexing reachable (`let x = xs[0]`); it is still E9000,
    so move the element with `remove` instead.
11. **A view can outlive the storage it borrows from without a diagnostic.** No registry code covers it, and the spec
    only states the function-return case (RT2). Examples: `let r = first_of(Pair { a: 1, b: 2 })` (the temporary drops
    at the end of the `let`, DR5), `let r = { let p = Pair { a: 1, b: 2 }; first_of(p) }` (the source local dies with
    the block), and `let t = "x".to_string().as_str()`. The borrow checker creates no loan for a temporary and has no
    notion of a source's scope ending, and origin inference gives a temporary argument an empty origin. Decide the
    rule and its error code first, then implement. E0122 at the binding is the closest code, but its registry text says
    "returned".
12. **The Rust toolchain is pinned** (issue #19, 2026-11-14) to `1.100.0` with `clippy` and `rustfmt` in
    `rust-toolchain.toml`, so `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are reproducible
    from a clean machine. CI installs that toolchain (`rustup toolchain install` reads the file) and verifies
    `rustc --version` and both components; `tests/check-ci.sh` fails if the pin or CI's verification step goes missing.
    Bumping the version means running `cargo fmt --all` after the change.
13. **The module and C bridge is narrower than the design.** There is no syntax for a private module, no module
    visibility check, and no check that public signatures only use public types. `CPtr` supports only `null()` and
    `CString.as_ptr()`, plus `CStr.to_string` (no dereference, null test, pointer arithmetic, or comparison).
    `extern` accepts only integers, floats, and `CPtr` (no C `bool`/`char`/structs/unions/function pointers/varargs),
    and only the `C` ABI. `CString` has no clone. `unsafe` is required only for calls, not for holding or moving a
    `CPtr`. There is no way to re-export an imported name (no `pub use`), and no module visibility. The manifest parser
    is deliberately minimal (path dependencies, no versions, no lockfile). A user `extern` whose symbol collides with
    a runtime libc declaration of a different type is a backend error printed without an error code or source span.
14. **`Map` is a linear-scan map**, not the hash map the stage 0 table names: the observable API and ordering are
    the same, but lookup is O(n) and keys only need `Eq`. A hash table can replace the implementation without
    changing the type or its methods; `Hash` and `hash()` already exist.
15. **The error types are aliases for unit.** `std` declares `IoError`, `ParseIntError`, and `AssertError` as
    `type ... = ()`; `read_to_string`, `write_file`, `process.run`, and the integer parsers return
    `Result<T, ()>` under those names. Tests only match `Err(_)`, and the `error` declarations of stage 1 are the
    place to give them payloads. This is also why the names were removed from the compiler's `builtin_type`.
16. **Ranges are only iterable, not values.** `a..b` and `a..=b` are handled by `for`; a range stored in a binding
    or passed to a function is still E9000. The stage 0 core-type table lists ranges, so this needs a decision
    before it can be called complete.
17. **The standard library is embedded with `include_str!`.** Changing `std/` requires rebuilding `burdin`; a
    project that declares its own `std` package replaces the embedded one (and then must provide every prelude
    name it uses). The prelude copies public names into every module including dependency packages, so a
    dependency that declares `args` or `read_to_string` keeps its own and the program may see two different
    meanings for the same spelling.
18. **`assert` has two modes.** A normal build uses the panicking `assert`/`assert_eq` from `std`. A `burdin test`
    build replaces those prelude names with `__test_assert`/`__test_assert_eq`, which return
    `Result<(), AssertError>`, so `assert(...)?` reports a failed test and helper functions can propagate it. Stage 1's
    `error` declarations replace the unit aliases and make the failure messages part of the error type.
19. **`for` now borrows its collection for the whole loop.** A `mut` method on the same collection inside the body
    is E0111, which also rejects useful patterns such as appending while iterating a list; that is deliberate
    until the iteration protocol of stage 1.

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

## Decided in the tenth session

- A `test` item is parsed into `File.tests`, separate from `File.items`, and compiled as a private function
  `__burdin_test_N()` that returns `Result<(), AssertError>`: the block's statements followed by `Ok(())`. A normal
  build never declares or checks tests, so stripping them needs no code-generation work and a broken test body cannot
  fail `burdin build`. Test items are only declared for the entry package's modules, not for dependencies.
- Test mode is a flag on `burdin_sema::check`. It makes the prelude replace `assert` and `assert_eq` with
  `std`'s `__test_assert`/`__test_assert_eq`, which return `Err(())` instead of panicking; a module's own `assert`
  keeps its meaning. `main` is not required and `Program.main` is `None`.
- The test binary is one executable with a generated dispatcher `main`: it reads the test index from `argv[1]`
  (`atoi`), calls that test, and returns 0 for `Ok` and 1 for `Err`. `burdin test` runs one process per test, so a
  panic abort fails only that test without restarting or skipping lists. This matches the design's observable
  behavior ("marks the running test failed, the others still run") with less machinery. The name/`FuncId` pairs travel
  in `Program.tests`.
- `CStr` is a `std` type alias for `CPtr<CChar>`, not a new view type: `CStr.to_string(ptr)` is the associated
  function `to_string` of a `CPtr<CChar>`, implemented with libc `strlen` plus the existing string allocator. This
  avoids a second implicit conversion while giving the m11 test its name.
- `--link-lib LLVM-C` is resolved by the driver through `llvm-config` because Ubuntu ships the C API inside
  `libLLVM-21.so`, not as `libLLVM-C.so`. The driver adds the libdir and an rpath; other library names still go to the
  linker unchanged. This is the one platform-specific exception in the driver.

## Decided in the eleventh session

- CI has a `macos` job on `macos-latest` (Apple Silicon) next to `linux`. It installs Homebrew's keg-only `llvm@21`,
  exports `LLVM_SYS_211_PREFIX`, and adds `<prefix>/bin` to `PATH` so the driver's `llvm-config` lookup and the
  `--link-lib LLVM-C` resolution work without hardcoded paths. No test assumes x86-64: the whole suite runs on arm64.
- `tests/check-platform.sh` is the regression check for the host target. It builds a small program, requires the emitted
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
- `tests/check-ci.sh` now fails if either job loses a cache: the cargo step must include `~/.cargo/registry` and
  `target/` with the toolchain-and-lockfile key, and an LLVM 21 cache must be present with the platform's path. The
  Linux job must also handle a libLLVM symlink that leaves the tree, and the macOS job must cache its Homebrew
  downloads.

## Decided in the fourteenth session

- CI has a `windows` job on `windows-latest` next to `linux` and `macos`. `ilammy/msvc-dev-cmd` loads the MSVC
  environment, the official `clang+llvm-21.1.8-x86_64-pc-windows-msvc` archive is unpacked into `C:\llvm`, and its
  `bin` goes on `PATH` so `LLVM-C.dll` is found. The job runs every check plus m01-m11 and a `BURDIN_CC=cl` smoke run,
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
  release profile, where it wraps (`tests/run-tests.sh --release`). The release run skips tests marked
  `//@ debug-only`; the m02 overflow test is the one such test. `tests/check-ci.sh` requires a release step covering
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
- `tests/check-ci.sh` guards the tracking the same way it guards the other CI invariants: every job must restore and
  save the baseline cache with the runner-and-lockfile key, run `tests/bench-regression.sh`, keep the step advisory,
  and record the baseline only on main.

## What is left to do, in order

1. Read `spec/stage1-subset.md` and settle its "Decisions needed before milestone 12" list. The MIR interpreter
   question (comptime), the closure representation, the iterator protocol (associated types or a generic
   `Iterator<T>`), `any`/`impl`, the error representation, and allocator parameters all shape milestones 12 to 24.
2. Write the m12 tests before the code, the same way stage 0 was built: `tests/m12_closures` and its `//@` expectations
   are the spec for the first stage 1 milestone.
3. Close the stage 0 correctness gaps that stage 1 inherits, each with a registry code and tests first: a view outliving
   its source (problem 11), ranges as values (16), the move forms rejected as E9000 (10), the generic gaps (the old
   item 4 of this list), and the module and C gaps (13).
4. CI runs on `ubuntu-24.04` (LLVM 21 from apt.llvm.org), `macos-latest` (Apple Silicon, Homebrew `llvm@21`), and
   `windows-latest` (MSVC and the official LLVM 21 archive, issue #12); all three pass every step and m01-m11 in both
   the debug and release profiles (issues #11, #16, #47, and #12), and cache cargo and LLVM 21 (issue #46).
   `tests/check-ci.sh` guards that the workflow keeps covering the suite in both profiles, every job, and their
   caches; `tests/check-platform.sh` guards the host triple. Every job also records and compares benchmark baselines,
   advisory until the runner numbers are stable (issue #446); when they are, drop `continue-on-error` from the step.
   Next: an Intel macOS job and another arm64 host if they are wanted.
5. Review the failing-test assumptions in `tests/README.md` with the author and settle each one in the spec; the
   passing suite only shows the implementation matches the hand-written expectations, not the design document.

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md, AGENTS.md, HANDOFF.md, and spec/stage1-subset.md. Settle the decisions for milestone 12, then write its tests.

If the session has no repository access to the folder, attach `CLAUDE.md`, `AGENTS.md`, `HANDOFF.md`, `spec/`, and `tests/README.md`.
