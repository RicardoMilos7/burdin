# Burdin

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. It is not for production. The compiler is written in Rust first (stage 0), then
rewritten in Burdin (stage 1) as soon as stage 0 can compile it.

Read `HANDOFF.md` for the current state, the open decisions, and what to do next.

## Sources of truth, in this order

1. `tests/` is the conformance suite and the most precise statement of behavior. Read `tests/README.md` first: it defines
   the test format, the error-code registry, and 16 assumptions the tests make.
2. `spec/grammar.md` is the syntax of stage 0. `spec/memory-rules.md` is the exact semantics of moves, conventions,
   exclusivity, references, and drops, with rule IDs and the tests that cover each.
3. `spec/stage0-subset.md` is what stage 0 must support, what it must not, and the 11 milestones.
4. `Burdin Language Design.md` is the full language design, including features that come after stage 0.

If these disagree, stop and say so. Do not pick one silently. `Burdin Stage 0 Subset.md` in the repo root is an old
export of the stage 0 document; use `spec/stage0-subset.md` instead.

## Working rules

- **Do not change a test to make it pass.** A test is the spec. If you think a test is wrong, explain why and ask. Changing
  the spec means updating the spec file and the tests first, then the code.
- **Work one milestone at a time**, in order (`spec/stage0-subset.md`). A milestone is done when all tests in its folder and
  all earlier folders pass. Do not start the next milestone with failing tests in the current one.
- **Stay inside stage 0.** Do not implement deferred features (closures, string interpolation, `any`, comptime, tasks,
  `error` declarations, extension methods, and so on) even when it would be convenient. Reserved words must still be rejected.
- **Diagnostics are part of the product.** Every error has a code from the registry in `tests/README.md`, a source span, and
  reports at the line the tests mark. Report every error in a file, not just the first.
- **Keep the checker local.** Memory checks look at one function body at a time, from signatures only. No whole-program
  analysis and no lifetime annotations.
- **Prefer simple and correct over clever.** This compiler will be read and rewritten in Burdin, so avoid Rust features
  that have no easy equivalent in the stage 0 subset where it costs little.

## Commands

```
tests/run-tests.sh                 run the conformance tests against the compiler (BURDIN=path overrides the binary)
tests/run-tests.sh m05 -v          only milestone 5, with diffs for failures
tests/check-tests.sh               check test directives and error codes (no compiler needed; also check-tests.ps1)
```

On Windows run the `.sh` scripts from Git Bash. The runner expects `burdin build`, `burdin run`, and `burdin test` as described at
the top of `tests/run-tests.sh`.

## Planned layout

```
Cargo.toml                workspace
crates/
  burdin_diag/              spans, diagnostics, error codes
  burdin_syntax/            lexer, parser, syntax tree (follows spec/grammar.md)
  burdin_sema/              name resolution, types, interfaces, generics, exhaustiveness
  burdin_mir/               MIR, lowering, drop elaboration
  burdin_check/             move, mutability, exclusivity, borrow, and return-origin checks (spec/memory-rules.md)
  burdin_codegen/           LLVM backend
  burdin_driver/            the `burdin` binary: build, run, test
std/                      the standard library, written in Burdin
runtime/                  the small C or Burdin runtime (start-up, panic path)
spec/                     grammar, memory rules, stage 0 scope
tests/                    conformance tests and scripts
```

The crate boundaries are a plan, not a rule. Keep the front end (syntax, sema) free of LLVM so the checks can be tested
without a backend.

## Toolchain

Rust 1.100 (edition 2024, pinned in `rust-toolchain.toml`), **LLVM 21** through `inkwell` 0.11 (feature `llvm21-1`), and the
system C compiler to build `runtime/rt.c` and link (`cc` on Linux and macOS, MSVC `cl` on Windows; `BURDIN_CC` overrides).
Platforms: Linux, macOS, Windows; x86-64 and arm64.

**Development happens in WSL Ubuntu 26.04 first** (decided). The repository stays on `F:`; build and test from WSL at
`/mnt/f/projects/burdin-lang`. Packages: `build-essential llvm-21-dev libpolly-21-dev clang-21 libzstd-dev zlib1g-dev`, and
Rust through rustup in `~/.cargo`. From PowerShell, run WSL commands with `wsl -d Ubuntu -e bash -lc '...'` (`-e` so the
outer shell does not expand `$?`), and keep Windows entries out of `PATH` inside WSL. Windows builds come later; the driver
already has an untested `cl` link path.

Diagnostics that are not in the registry: `E9000` means "not implemented in this compiler yet". It marks constructs from
later milestones and must be gone when stage 0 is complete.
