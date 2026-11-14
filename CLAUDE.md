# Burdin

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. It is not for production. The compiler is written in Rust and stays in Rust:
stage 0 is the core subset, and stage 1 adds the rest of the core language to the same Rust compiler. Burdin is not
bootstrapped into itself. A self-hosted compiler may be reconsidered once the language is more mature and at least its
core features are complete, but it is not planned.

Read `HANDOFF.md` for the current state, the open decisions, and what to do next. Read `AGENTS.md` for the shared
working agreement: the definition of done, the verification policy, the milestone close-out checklist, and the issue
workflow. Process rules live in `AGENTS.md`; behavior lives in `tests/` and `spec/`.

## Sources of truth, in this order

The files in this repository are the only maintained copies. Documents hosted on claude.ai are historical: they are
not updated and may disagree with the repository.

1. `tests/` is the conformance suite and the most precise statement of behavior. Read `tests/README.md` first: it defines
   the test format, the error-code registry, and 16 assumptions the tests make.
2. `spec/grammar.md` is the syntax of stage 0. `spec/memory-rules.md` is the exact semantics of moves, conventions,
   exclusivity, references, and drops, with rule IDs and the tests that cover each.
3. `spec/stage0-subset.md` is what stage 0 must support, what it must not, and the 11 milestones (all complete).
   `spec/stage1-subset.md` is the stage 1 plan: what the next milestones add, the decisions they need, and the
   roadmaps for later stages.
4. `Burdin Language Design.md` is the full language design, including features that come after stage 0.

If these disagree, stop and say so. Do not pick one silently.

## Working rules

- **Do not change a test to make it pass.** A test is the spec. If you think a test is wrong, explain why and ask. Changing
  the spec means updating the spec file and the tests first, then the code.
- **Every change ships with tests.** A behavior change is done when a test demonstrates it and CI runs it, not when it
  compiles. Register new diagnostics in `tests/README.md` before using them. `AGENTS.md` has the full definition of done;
  every issue in the GitHub plan carries a `## Verification` section saying how it is proven.
- **Work one milestone at a time**, in order (`spec/stage0-subset.md` for stage 0, `spec/stage1-subset.md` for stage 1).
  A milestone is done when all tests in its folder and all earlier folders pass in debug and release.
- **Stay inside the current stage.** Stage 0 is complete; stage 1 is the current plan. Do not implement deferred features
  (stage 2 tasks, tooling, ecosystem) early, even when it would be convenient. Reserved words must still be rejected.
- **Diagnostics are part of the product.** Every error has a code from the registry in `tests/README.md`, a source span, and
  reports at the line the tests mark. Report every error in a file, not just the first.
- **Keep the checker local.** Memory checks look at one function body at a time, from signatures only. No whole-program
  analysis and no lifetime annotations.
- **Prefer simple and correct over clever.** This compiler will be read and extended for a long time, so keep it plain
  and easy to follow.

## Commands

```
tests/run-tests.sh                 run the conformance tests against the compiler (BURDIN=path overrides the binary)
tests/run-tests.sh m05 -v          only milestone 5, with diffs for failures
tests/check-tests.sh               check test directives and error codes (no compiler needed; also check-tests.ps1)
```

On Windows run the `.sh` scripts from Git Bash. The runner expects `burdin build`, `burdin run`, and `burdin test` as described at
the top of `tests/run-tests.sh`.

## Layout

```
CLAUDE.md                 the Claude-specific entry point and working rules
AGENTS.md                 the shared working agreement: definition of done, close-out checklist, verification policy
HANDOFF.md                current state, decisions, and what is next
CHANGELOG.md              shipped milestones
Cargo.toml                workspace
crates/
  burdin_diag/              spans, diagnostics, error codes
  burdin_syntax/            lexer, parser (parser/: items, types, statements, patterns, expressions), syntax tree
  burdin_tir/               the typed tree every later stage reads: types, expressions, programs, signatures, and the
                            questions about types (`TypeDefs`: Copy, Clone, Drop) and places
  burdin_sema/              name resolution and type checking, modules and imports, monomorphization of generics,
                            exhaustiveness (declare, imports, impls, instances, resolve, bodies, validate; fncx/
                            checks one function body)
  burdin_check/             return-origin, move, and borrow checks over the typed tree (spec/memory-rules.md)
  burdin_codegen/           LLVM backend (emit/: one file per theme) and the generated runtime
  burdin_driver/            the `burdin` binary: build, run, test; package loading (`package.rs`: burdin.toml,
                            modules, dependencies)
  burdin_bench/             generated benchmark inputs and the `cargo bench` targets for lexing, parsing, name
                            resolution and type checking, and code generation
std/                      the standard library, written in Burdin
spec/                     grammar, memory rules, stage 0 scope, stage 1 plan
tests/                    conformance tests and scripts
```

Data flows one way: `burdin_syntax` to `burdin_sema` to `burdin_check` to `burdin_codegen`, with `burdin_tir` as the
shared vocabulary and `burdin_diag` under everything. `burdin_check` and `burdin_codegen` depend only on `burdin_tir`,
not on sema. Keep everything before codegen free of LLVM so the checks can be tested without a backend. A MIR
(`burdin_mir`) is not planned for stage 0: sema monomorphizes, and the checks read the typed tree.

## Toolchain

Rust is pinned by `rust-toolchain.toml` to an exact stable version with the `clippy` and `rustfmt` components (currently
`1.100.0`; edition 2024), so `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` give the same
verdict on a clean machine. Bump the version deliberately, together with a `cargo fmt --all` pass and a HANDOFF.md note;
`tests/check-ci.sh` fails if the pin, its components, or CI's verification step goes missing. **LLVM 21** through
`inkwell` 0.11 (feature `llvm21-1`), and the system C compiler driver as the linker against libc (`cc` on Linux and
macOS, MSVC `cl` on Windows; `BURDIN_CC` overrides).
There is no C runtime: `burdin_codegen/src/runtime.rs` generates `main`, output, panics, string allocation and
conversions, and heap allocation as LLVM IR that calls libc directly.
Platforms: Linux, macOS, Windows; x86-64 and arm64.

**Development happens in WSL Ubuntu 26.04 first** (decided). The repository stays on `F:`; build and test from WSL at
`/mnt/f/projects/burdin-lang`. Packages: `build-essential llvm-21-dev libpolly-21-dev clang-21 libzstd-dev zlib1g-dev`, and
Rust through rustup in `~/.cargo`. From PowerShell, run WSL commands with `wsl -d Ubuntu -e bash -lc '...'` (`-e` so the
outer shell does not expand `$?`), and keep Windows entries out of `PATH` inside WSL. Windows builds come later; the driver
already has an untested `cl` link path.

Diagnostics that are not in the registry: `E9000` means "not implemented in this compiler yet". After stage 0 it marks
constructs that belong to stage 1 or later; each one gets a real diagnostic when its milestone arrives.
