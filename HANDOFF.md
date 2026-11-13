# Handoff: where the project stands

Written at the end of the first session, updated at the end of the second. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Updated at the end of the second session (2026-11-13). **Milestone 1 is done**: all 5 tests in `tests/m01_hello` pass.

| What | Where | Status |
| --- | --- | --- |
| Full language design | `Burdin Language Design.md` | Written. Local copy is newer than the one on claude.ai |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written. `Burdin Stage 0 Subset.md` in the root is an old export, safe to delete |
| Grammar | `spec/grammar.md` | Parser follows it except generic arguments in expressions (rule 8.2), struct literals, `match`, and patterns beyond bindings and tuples |
| Memory rules | `spec/memory-rules.md` | Written, not implemented yet (milestones 5 and 6) |
| Conformance tests | `tests/` | 131 tests. 6 pass today (m01 plus `m03_typecheck/unknown_variable.bd`) |
| Git repository | `main` | Initialized. `.gitattributes` keeps LF line endings |
| Cargo workspace | `Cargo.toml`, `crates/` | `burdin_diag`, `burdin_syntax`, `burdin_sema`, `burdin_codegen`, `burdin_driver` (`burdin` binary) |
| C runtime | `runtime/rt.c` | `main`, `burdin_rt_print`. Embedded in the driver and compiled with every program |
| CI | `.github/workflows/ci.yml` | Linux only: check-tests, fmt, clippy, unit tests, conformance tests of finished milestones. Never run yet (no remote) |
| LLVM | WSL: `/usr/lib/llvm-21` | LLVM 21.1.8 from Ubuntu 26.04 |

Dev environment: WSL Ubuntu 26.04, Rust 1.100 through rustup, LLVM 21. See the Toolchain section of `CLAUDE.md` for the
exact packages and how to call WSL from PowerShell.

### How the compiler is built so far

- `burdin_syntax`: complete lexer; parser for functions, types, statements, and the full expression precedence table with the
  newline rules of section 2 and statement-level and item-level error recovery. Items other than `fn` are reported as E9000
  and skipped whole.
- `burdin_sema`: resolves functions, locals, and the output built-ins (`print`, `println`, `eprint`, `eprintln`), checks
  argument counts (E0004) and types (E0003), unknown names (E0002), and produces a typed tree (`tir`). Types so far: `()`,
  `bool`, `str`. Anything else is E9000.
- `burdin_codegen`: typed tree to LLVM IR to an object file. `str` is `{ ptr, usize }`. Functions are `burdin.<name>`, and
  `burdin_entry` calls `main`.
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
- Rust 1.100 pinned in `rust-toolchain.toml`.
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

## What is left to do, in order

1. Milestone 2 (`tests/m02_basics`): integer and float types, arithmetic, bindings with `let mut` and assignment, control flow,
   recursion, and the panic path (overflow, bounds, divide by zero abort with a message, flushing stdout first). Extend
   `burdin_sema::tir::Ty` and the codegen; add a panic function to `runtime/rt.c`.
2. Then milestones 3 to 11 in order. After each one, add its folder to the conformance step in `.github/workflows/ci.yml`.
3. Known parser gaps to close in their milestones: generic arguments in expressions (`List<i32>.new()`, grammar rule 8.2,
   needed by m07 and m09), path struct literals (`Shape.Rect { w: 3 }`, m04), `match` and refutable patterns (m04), items
   other than `fn` (m04, m07, m08, m10).
4. Push the repository to a remote so CI runs, and add a Windows job once the `cl` link path is tried.
5. After the first real runs, review failing tests together with the assumptions list and settle each assumption in the spec.
6. Before stage 1: the MIR interpreter question (comptime needs one), the design document's own open-questions list (derive syntax,
   `select`, extern details), and the stage 1 order (closures and iterators first).

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md and HANDOFF.md. Continue with milestone 2.

If the session has no repository access to the folder, attach `CLAUDE.md`, `HANDOFF.md`, and the `spec/` folder.
