# Burdin agent guide

Burdin is a personal, AI-assisted experiment in building a native-compiled language with Rust-level memory safety, no
garbage collector, and value semantics. The compiler is written in Rust and stays in Rust. This file is the shared
working agreement for any AI agent contributing to the repository; `CLAUDE.md` is the Claude-specific entry point.

Behavior is defined by `tests/`, `spec/grammar.md`, and `spec/memory-rules.md`, in that order. If this file and a spec
disagree about behavior, the spec wins and the disagreement should be raised instead of guessed.

## Read order

1. `tests/README.md` — the test format, the error-code registry, and the 16 settled decisions.
2. `spec/grammar.md`, `spec/memory-rules.md` — syntax and the exact memory semantics.
3. `spec/stdlib.md`, `spec/derives.md` — the standard-library surface and the exact derived members.
4. `spec/stage0-subset.md` (complete) and `spec/stage1-subset.md` (the current plan).
5. `HANDOFF.md` for the current state; `Burdin Language Design.md` for the full rationale.
6. `CHANGELOG.md` for what has shipped; the GitHub roadmap and milestones for what is planned.

The files in this repository are the only maintained copies. Documents hosted on claude.ai are historical: they are
not updated and may disagree with the repository.

## Commands

```
tests/run-tests.sh                 run the conformance tests
tests/run-tests.sh m05 -v          one milestone, with diffs
tests/run-tests.sh --release       release profile (skips debug-only tests)
tests/check-tests.sh               validate directives and error codes (no compiler needed)
tests/check-unit-tests.sh          check the front-end unit tests and their CI coverage (no compiler needed)
tests/check-ci.sh                  validate the CI workflow, its caches, and the pinned Rust toolchain (no compiler needed)
tests/check-platform.sh            check the host triple and 64-bit isize/usize (needs a built driver)
tests/check-docs.sh                check documentation links and the driver contract (needs a built driver)
cargo fmt --all --check            formatting
cargo clippy --all-targets -- -D warnings
cargo test                         unit tests
cargo bench -p burdin_bench        compiler benchmarks (lex, parse, sema, codegen)
cargo bench -p burdin_bench -- --smoke   benchmark budgets CI runs
cargo bench -p burdin_bench -- --save FILE       record a benchmark baseline
cargo bench -p burdin_bench -- --baseline FILE   compare with a baseline (exit 1 on a regression)
tests/bench-regression.sh          benchmark regression check CI runs (advisory, issue #446)
```

Development happens in WSL Ubuntu first; the shell scripts also run from Git Bash on Windows.

## Working rules

- **Tests are the spec.** Never change a test to make it pass. If a test is wrong, explain why and ask; changing the
  spec means updating the spec file and the tests first, then the code.
- **Every change ships with tests.** A behavior change is done when a test demonstrates it and CI runs it, not when it
  compiles. Register a new diagnostic in `tests/README.md` before using it.
- **Work one milestone at a time**, in order. A milestone is done when its folder and every earlier folder pass in
  debug and release.
- **Stay inside the current stage.** Stage 0 is complete; stage 1 is the current plan. Do not implement stage 2 or
  stage 3 features early. Reserved words stay reserved until their milestone.
- **Diagnostics are part of the product.** Every error has a registry code, a span, and reports at the line the tests
  mark. Report every error in a file, not just the first.
- **Keep the checker local.** Memory checks look at one function body at a time, from signatures only. No whole-program
  analysis and no lifetime annotations.
- **Prefer simple and correct over clever.** The compiler will be read and extended for a long time.
- **The crate layout is not fixed.** The map below is the current shape, not a constraint. When a milestone needs a new
  responsibility with its own boundary (an LSP server, a package resolver, a formatter), add a new crate under
  `crates/` for it and extend the workspace rather than piling code into an existing crate. Keep the one-way dependency
  direction: syntax to sema to check to codegen, `burdin_tir` as the shared vocabulary, `burdin_diag` under everything,
  and nothing before codegen touching LLVM. Update this file's map when a crate is added, removed, or renamed.
- **Update the docs with the code.** `HANDOFF.md`, the relevant `spec/*.md`, and `CHANGELOG.md` change in the same
  milestone as the behavior.

## Definition of done

### A change

- The behavior is implemented (or the decision is written down).
- A test proves it: conformance, unit, CLI, integration, or a benchmark, depending on the area.
- New diagnostic codes are registered in `tests/README.md` first.
- The suite passes in debug and release.
- The closing comment names the test paths and the exact command(s) that pass, plus any manual verification.

### A milestone

- [ ] Every test in its folder and every earlier folder passes in debug and release.
- [ ] `.github/workflows/ci.yml` lists the finished milestone's folder.
- [ ] `HANDOFF.md` describes the new behavior and the remaining gaps.
- [ ] The relevant `spec/*.md` is updated (rule IDs and test lists included).
- [ ] `CHANGELOG.md` has an entry for the milestone.
- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass.
- [ ] Every issue in the milestone is closed with its verification result, and the GitHub milestone is closed.

## Verification by area

| Area | Required evidence |
| --- | --- |
| Language / runtime behavior | Conformance tests in `tests/<milestone>/`: `//@ run` with exact `//>` output, `//@ compile-fail` with `//~ E....`, `//@ run-abort`, `//@ exit-code`, `//@ burdin-test` |
| Compiler internals | Rust unit tests in the owning crate plus an end-to-end conformance test |
| Diagnostics | Compile-fail test with the registry code; golden message test where wording matters |
| Driver / CLI / build tool | Black-box tests with fixture packages, success and failure paths, golden output where stable |
| LSP / editors | JSON-RPC integration tests over stdio, grammar fixtures, manual checklist for packaging |
| Performance | Benchmark with before/after numbers plus a correctness test that behavior (including panics) is unchanged |
| Documentation | Snippets that compile/run, commands run from a clean checkout, link checks |
| Release / infrastructure | CI check or script; a rehearsal checklist for installers and external services |
| Decisions | The rule written in `spec/` plus the tests that witness it, before implementation |

## Issue workflow

- Every GitHub issue ends with a `## Verification` section. If it is missing, add it before starting.
- **Every pull request states which issue it closes**, in the body, with a closing keyword and the issue number
  (`Closes #12`), one line per issue when it closes several. A PR that closes no issue says why in its body instead of
  leaving it unstated.
- Decision issues are closed only when the rule and its tests land; implementation follows in the feature issue.
- Close an issue only with a comment naming the test paths and the command(s) that pass.
- Process rules (this file, the close-out checklist, CI policy, changelog policy) are not issues. They live here and in
  `CLAUDE.md`; an issue that only states a rule should be folded into this file and closed.
- The milestones and the roadmap are the plan of record; `spec/stage1-subset.md` is the written rationale.

## Repository map

```
README.md                 the entry point: building, hello world, and the driver contract
CLAUDE.md                 Claude-specific entry point and working rules
AGENTS.md                 this shared working agreement
HANDOFF.md                current state, decisions, and what is next
CHANGELOG.md              shipped milestones
spec/                     grammar, memory rules, standard-library surface, derives, stage 0 scope, stage 1 plan
tests/                    conformance suite, README with the format and error codes
crates/                   burdin_diag, burdin_syntax, burdin_tir, burdin_sema,
                          burdin_check, burdin_codegen, burdin_driver,
                          burdin_bench (generated inputs and `cargo bench` targets)
std/                      the standard library, written in Burdin
```
