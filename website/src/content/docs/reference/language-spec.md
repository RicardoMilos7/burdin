---
title: Language specification
description: Where the exact rules of Burdin are written, which file to read for each topic, and which file wins when two disagree.
---

The guide explains Burdin with examples. The exact rules are in the repository, and they are the final word. The
conformance tests are the most precise statement of behavior, the specification files explain each area in prose with
rule numbers, and the design document covers features that come later.

## Sources of truth

1. **The conformance tests** in
   [`tests/`](https://github.com/RicardoMilos7/burdin/tree/main/tests). Each test is a program with its expected output
   or its expected error code. The
   [test README](https://github.com/RicardoMilos7/burdin/blob/main/tests/README.md) defines the format and lists every
   error code.
2. **The specification files** in
   [`spec/`](https://github.com/RicardoMilos7/burdin/tree/main/spec), listed below.
3. **The design document**, `Burdin Language Design.md`, which covers the whole language, including later stages.

When these disagree, the repository's files win over any copy hosted elsewhere, and a disagreement is a bug to report.

## The language

| Topic | File |
| --- | --- |
| Syntax of the core language | [`spec/grammar.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/grammar.md) |
| Moves, conventions, exclusivity, references, and drops, with rule IDs | [`spec/memory-rules.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/memory-rules.md) |
| Errors: `error` declarations, `Result<T>`, `context`, traces, `main` returning a `Result` | [`spec/errors.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/errors.md) |
| Closures | [`spec/memory-rules.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/memory-rules.md) (rules CL1 to CL11) |
| Iteration: `Iterable`, `Iterator`, adapters, and ranges | [`spec/iterators.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/iterators.md) |
| Interpolation, `Display` and `Debug`, specifiers, and output | [`spec/format.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/format.md) |
| `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default` | [`spec/derives.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/derives.md) |
| `any` and `impl` existential types, object safety, and downcasts | [`spec/existentials.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/existentials.md) |
| Default and named arguments | [`spec/arguments.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/arguments.md) |
| Extension methods and their resolution order | [`spec/extensions.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/extensions.md) |
| Operator overloading and `Index` and `IndexMut` | [`spec/operators.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/operators.md) |
| Compile-time evaluation: `comptime` values, parameters, `if`, `for`, and `embed` | [`spec/comptime.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/comptime.md) |
| Declaration generation with `comptime generate` | [`spec/generate.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/generate.md) |
| Reflection: `Type.fields`, `variants`, `name`, and user `derive` declarations | [`spec/reflection.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/reflection.md) |
| Const parameters and array lengths | [`spec/const-generics.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/const-generics.md) |
| Generic type aliases | [`spec/type-aliases.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/type-aliases.md) |
| Impl blocks for specific instances, and `Drop` on a generic type | [`spec/impls.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/impls.md) |
| Associated types, projections, and monomorphization | [`spec/associated-types.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/associated-types.md) |
| Allocator parameters, `Arena`, and `Shared<T>` | [`spec/allocators.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/allocators.md) |

## The standard library and the runtime

| Topic | File |
| --- | --- |
| Names and signatures of the standard library | [`spec/stdlib.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/stdlib.md) |
| Tasks, the scheduler, scoped tasks, and panics in tasks | [`spec/tasks.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/tasks.md) |
| `Channel<T>` and `select` | [`spec/channels.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/channels.md) |
| `Send`, `Sync`, atomics, `Mutex<T>`, and `RwLock<T>` | [`spec/shared.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/shared.md) |
| The reactor, sockets, name resolution, and timers | [`spec/io.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/io.md) |
| Serialization framework | [`spec/serialization.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/serialization.md) |

## The tools

| Topic | File |
| --- | --- |
| The build tool's commands, exit statuses, and differences from Cargo | [`spec/build-tool.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/build-tool.md) |
| Named build profiles | [`spec/profiles.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/profiles.md) |
| Workspaces and the workspace lockfile | [`spec/workspaces.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/workspaces.md) |
| Packages, `burdin.lock`, and `burdin add` | [`spec/packages.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/packages.md) |
| Single-file scripts | [`spec/scripts.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/scripts.md) |
| The test runner | [`spec/test-runner.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/test-runner.md) |
| Benchmarks | [`spec/bench.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/bench.md) |
| Fuzzing | [`spec/fuzz.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/fuzz.md) |
| Sanitizers | [`spec/sanitizers.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/sanitizers.md) |
| Diagnostics, the JSON format, and SARIF | [`spec/diagnostics.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/diagnostics.md) |
| Formatter | [`spec/fmt.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/fmt.md) |
| Linter | [`spec/lint.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/lint.md) |
| Documentation generator | [`spec/doc.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/doc.md) |
| Language server | [`spec/lsp.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/lsp.md) |
| Debugging and the debug adapter | [`spec/debug.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/debug.md) |
| Toolchain manager | [`spec/toolchain.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/toolchain.md) |

## Interop

| Topic | File |
| --- | --- |
| `burdin bindgen`: C headers, libclang, and exported modules | [`spec/bindgen.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/bindgen.md) |
| Rust interop | [`spec/rust-interop.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/rust-interop.md) |
| Build scripts (`build.bd`) | [`spec/build-scripts.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/build-scripts.md) |

## Stages

[`spec/stage0-subset.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/stage0-subset.md) describes the first
stage, which is complete. [`spec/stage1-subset.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/stage1-subset.md)
is the plan for the stage after it. The [status page](/reference/status/) says where the work stands.
