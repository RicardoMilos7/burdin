# Handoff: where the project stands

Written at the end of the first session. Read `CLAUDE.md` first, then this file. Delete this file once its open items are done.

## State

Burdin is a personal experiment: a native-compiled language with Rust-level memory safety, no garbage collector, and value
semantics. **No compiler code exists yet.** What exists is the design, the specs, and a test suite for the first compiler
(stage 0, written in Rust).

| What | Where | Status |
| --- | --- | --- |
| Full language design | `Burdin Language Design.md` | Written. Local copy is newer than the one on claude.ai |
| Stage 0 scope and 11 milestones | `spec/stage0-subset.md` | Written. `Burdin Stage 0 Subset.md` in the root is an old export, safe to delete |
| Grammar | `spec/grammar.md` | Written, not yet tried against a parser |
| Memory rules | `spec/memory-rules.md` | Written. About 40 numbered rules, each linked to its tests |
| Conformance tests | `tests/` | 131 tests in 11 milestone folders (62 run, 61 compile-fail, 4 run-abort, 4 `burdin test`) |
| Test scripts | `tests/run-tests.sh`, `tests/check-tests.sh`, `tests/check-tests.ps1` | Written. The runner was tried only against a stand-in driver |
| AI instructions | `CLAUDE.md` | Written |
| Git repository | none | Not initialized |
| Cargo workspace, CI | none | Not created |
| LLVM | not installed | Blocking, see below |

Machine facts (Windows 11, folder `F:\projects\burdin-lang`): Rust nightly 1.101 (stable and 1.98.1 also installed), MSVC build
tools with `link.exe`, cmake 4.4, git 2.45, Python 3.15, 16 cores, 191 GB free on `F:`. WSL has an Ubuntu distro (stopped).
No LLVM, clang, `llvm-config`, or ninja.

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

## Open decision that blocks the compiler

**How to get LLVM** (needed for the backend through the `inkwell` crate, version 0.9.0 supports LLVM 11 to 22):

1. WSL Ubuntu: `apt install llvm-NN-dev`, build and test on Linux first, add Windows later. Recommended.
2. Windows with a third-party prebuilt LLVM that includes `llvm-config` (for example vovkos/llvm-package-windows). Must match the
   Rust MSVC toolchain.
3. Build LLVM from source on Windows. Works, takes hours.

Official Windows LLVM releases do not include `llvm-config`, which `llvm-sys` needs. Also pending: which LLVM version to pin
(20 or 21 suggested). `CLAUDE.md` tells the AI not to add `inkwell` until this is decided.

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
5. **The runner assumes a driver interface that does not exist yet:** `burdin build|run|test <target>`, diagnostics as
   `path:LINE:COL: error[E0000]: message`, and a `test result: N passed, M failed` line plus `FAIL <name>` lines. The contract is at the top
   of `tests/run-tests.sh`. Build the driver to match it, or change that one block.
6. **`m11_llvm/llvm_return_42.bd` stops short** of emitting an object file and linking, because the target-initialization functions
   are architecture-specific. The stage 0 milestone says "link and run", so extend it once the compiler can run the current version.
7. **Windows-specific things are untested:** binary-safe file I/O (no newline translation), `_write` versus `write`, abort exit codes
   (`run-abort` only checks "non-zero"), and the `run-tests.sh` script needs Git Bash.
8. **The grammar has not been tried.** Two rules are the most likely to need adjustment when the parser is written: generic
   arguments in expressions (`List<i32>.new()` versus `a < b`), and newline handling inside nested braces.
9. **Two documents on claude.ai are stale.** The design doc there lacks the exclusivity rule, and the stage 0 doc there is the old
   export. The local files are the source of truth.
10. **The task-list tool was unavailable in the last session**, so progress was tracked only in the conversation.

## What is left to do, in order

1. Decide the LLVM route and version (above).
2. `git init`, first commit of everything that exists now. Attribution lines for commits are in the session setup; ask the user.
3. Create the Cargo workspace per the layout in `CLAUDE.md`, a CI workflow that runs `tests/check-tests.sh` and the conformance tests,
   a pinned toolchain file, and an empty `burdin` driver that satisfies the runner's CLI contract.
4. Milestone 1 (`tests/m01_hello`): lexer, parser, LLVM codegen, linking, `println`. Then milestones 2 to 11 in order. Each is
   finished only when its folder and all earlier folders pass.
5. After the first real runs, review the failing tests together with the assumptions list and settle each assumption in the spec.
6. Before stage 1: the MIR interpreter question (comptime needs one), the design document's own open-questions list (derive syntax,
   `select`, extern details), and the stage 1 order (closures and iterators first).

## Starting the next session

Open the folder and say something like:

> Read CLAUDE.md and HANDOFF.md. I chose LLVM route <1, 2, or 3> and version <N>. Continue with the next step.

If the session has no repository access to the folder, attach `CLAUDE.md`, `HANDOFF.md`, and the `spec/` folder.
