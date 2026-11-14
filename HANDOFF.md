# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second, third, and fourth. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the fourth session (2026-11-14). **Milestones 1 to 4 are done**: all tests in `tests/m01_hello`
through `tests/m04_data` pass in the debug and release profiles.

| What | Where | Status |
| --- | --- | --- |
| Full language design | `Burdin Language Design.md` | Written. Local copy is newer than the one on claude.ai |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written. `Burdin Stage 0 Subset.md` in the root is an old export, safe to delete |
| Grammar | `spec/grammar.md` | Parser follows it except generic arguments in expressions (rule 8.2). Items other than `interface`, `use`, `extern`, and `test` parse |
| Memory rules | `spec/memory-rules.md` | Written, not implemented yet (milestones 5 and 6) |
| Conformance tests | `tests/` | 131 tests. 37 pass today: m01 to m04, and 3 in m06 |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_sema`, `burdin_codegen`, `burdin_driver` (`burdin` binary) |
| Runtime | `crates/burdin_codegen/src/runtime.rs` | `main`, output, panics (`burdin_rt_panic`, `burdin_rt_panic_bounds`), number/char/str to `String`. Generated as LLVM IR into every program; calls libc (`write`, `malloc`, `free`, `abort`). `cc` only links |
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
  integers (`isize`/`usize` are 64-bit), `f32`, `f64`, `str`, `String`, `[T; N]`, `Option<T>` (matching only), `!`.
  Integer and float literals take their type from context (`expect` is passed down; a literal on the left of a binary
  operator takes the right side's type), else `i32` / `f64`; out-of-range literals are E0008, and a `-` before a literal
  is part of it. Built-in methods: `to_string` (integers, `bool`, `char`, `str`), `wrapping_*`, `checked_*` (add, sub,
  mul), `min`, `max`, `abs`. `match` works on integers, `char`, `bool`, and `Option` with literal, range, `|`, `@`, and
  guard patterns, and checks exhaustiveness (E0006). Constants are checked once and expanded at every use.
- Milestone 4 added tuples, structs, enums (unit, tuple, and struct variants), type aliases, and method blocks.
  Types and values are separate namespaces; `Point.new` and `Shape.Circle` resolve the left side as a type when no
  local or global value has that name. Methods are ordinary functions named `Type.method` with `self` first; `self` is
  read-only and passed by value, `mut self` is passed by address (the callee's `self` is the caller's place) and needs
  a mutable receiver (E0113). `own self` and `mut`/`own` named parameters are still E9000 (milestones 5 and 6).
  `Some(x)` and `None` build `Option` values (`None` needs an expected type). `if let` is checked as a two-arm `match`;
  `let` takes any pattern, which must be irrefutable (E0006) unless it has an `else` block that diverges. `for` also
  iterates over arrays (by place, binding each element through the pattern). `==` works on tuples, `Option`, and on
  structs and enums that derive `Eq` (all their fields must support `==`). Derive names are checked, but nothing else
  about derives yet (milestone 7). Struct patterns may leave fields out (the grammar has no `..`).
- Exhaustiveness (`burdin_sema/src/exhaust.rs`) is the usefulness algorithm over a pattern matrix, with integer and
  `char` ranges split at their boundaries. It handles nested tuples, structs, enums, and `Option`.
- `burdin_codegen`: typed tree to LLVM IR to an object file. `str` is `{ ptr, usize }`, `String` is `{ ptr, len, cap }`,
  `Option<T>` is `{ i1, T }`. Functions are `burdin.<name>`, and the generated C `main` calls the program's `main`. Stack slots live in the
  `entry` block; after a terminator, generation continues in a fresh unreachable block. Overflow of `+ - *` and unary
  `-` panics in debug (LLVM `*.with.overflow`) and wraps in release; division by zero and out-of-range shifts always
  panic; `MIN / -1` panics in debug and wraps in release; float-to-int casts saturate (`fpto*i.sat`). Panics print
  `path:LINE:COL: panic: message` to stderr and `abort()`. Tuples and structs are LLVM structs (a `()` field is `{}`);
  an enum is `{ i32 tag, [N x iA] payload }` sized for its largest variant, whose fields are read through the payload
  pointer as the variant's own struct. Pattern tests on variants use `select`, so they may load another variant's bytes
  but never branch on them. Values are still copied freely: moves and drops come in milestone 5.
- **`String` memory, temporary:** only `String` temporaries that are borrowed as `str` (`println(n.to_string())`) are
  freed, at the end of their statement. A `String` stored in a local is leaked. Drop elaboration in milestone 5
  replaces this.
- `burdin_driver`: `build` (writes `<stem>` in the current directory, `-o` to change it, `--emit-ir file.ll`), `run`
  (builds into a temporary directory, passes the exit code through, 128+signal for a signal). `test` is not implemented.
- Diagnostics are printed in source order as `path:LINE:COL: error[E0000]: message`. Semantic checks run only if there
  were no syntax errors.

## Decisions already made (do not reopen without a reason)

- Compiler: Rust first, LLVM only (no Cranelift; lowest optimization level for debug), then rewritten in Burdin.
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

## What is left to do, in order

1. Milestone 5 (`tests/m05_ownership`): moves, `Drop` (which needs `Type: Drop { ... }` interface blocks, today E9000),
   drop order and drop flags, and `Heap`. This replaces the temporary `String` freeing in codegen.
2. Then milestones 6 to 11 in order. After each one, add its folder to the conformance step in `.github/workflows/ci.yml`.
3. Known parser gaps to close in their milestones: generic arguments in expressions (`List<i32>.new()`, grammar rule 8.2,
   needed by m07 and m09) and the `interface`, `use`, `extern`, and `test` items (m07, m08, m10). `Self` is not yet
   accepted in expressions (`Self.new()`, `Self { ... }`); no test needs it so far.
4. Push the repository to a remote so CI runs, and add a Windows job once the `cl` link path is tried.
5. After the first real runs, review failing tests together with the assumptions list and settle each assumption in the spec.
6. Before stage 1: the MIR interpreter question (comptime needs one), the design document's own open-questions list (derive syntax,
   `select`, extern details), and the stage 1 order (closures and iterators first).

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md and HANDOFF.md. Continue with milestone 2.

If the session has no repository access to the folder, attach `CLAUDE.md`, `HANDOFF.md`, and the `spec/` folder.
