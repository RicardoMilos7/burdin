---
title: Status and limitations
description: What is finished in Burdin, which platforms it is tested on, and what it does not do yet.
---

Burdin is a personal experiment in a language with memory safety and no garbage collector. It is not for production, and
it makes no stability promise. Programs can change from one version to the next, and the language is still being built.

## Where the work stands

The record below is from `HANDOFF.md` in the repository, as of 2026-11-24. That file is updated with each session, so the
[repository](https://github.com/RicardoMilos7/burdin/blob/main/HANDOFF.md) has the latest state.

| Part | Status |
| --- | --- |
| Stage 0 (milestones 1 to 11): the core language, ownership, generics, modules, the first standard library, and the test runner | Complete |
| Stage 1 (milestones 12 to 24): closures, iterators, formatting, `any`, errors, arguments, extension methods, operators, constant evaluation, reflection and derives, build scripts, allocators, and the standard library | Complete |
| Stage 2 tasks (2.1 to 2.4): the task scheduler, channels, shared state, and asynchronous I/O | Complete |
| Tooling: the formatter, linter, documentation generator, language server, debug adapter, REPL, and toolchain manager | Available; see the [command line](/reference/cli/) |
| Official packages: `json`, `toml`, `crypto`, `math`, `http`, and `tls` | Available |

The conformance suite is the measure of what works. Each milestone has tests in `tests/`, and the suite runs in debug and
release builds on every change.

## Platforms

| Platform | Support |
| --- | --- |
| Linux, x86-64 | Tested on every change, including the sanitizers and `burdin fuzz` |
| macOS, arm64 (Apple Silicon) | Tested on every change, without the address, leak, and thread sanitizers and without fuzzing |
| Windows, x86-64 (MSVC) | Tested on every change, without the LTO and PGO flags and without the sanitizers except `undefined` |
| Linux, arm64 | Not tested. The compiler uses the host triple, but no job runs the suite |
| macOS, x86-64 | Not tested |
| Windows, arm64 | Not tested. The LLVM build that CI uses is x86-64 |
| Other systems and 32-bit targets | Not supported |

The full matrix, with the test behind each row, is in
[`docs/platform-support.md`](https://github.com/RicardoMilos7/burdin/blob/main/docs/platform-support.md).

## Known limitations

- **Not for production.** The compiler, the standard library, and the tools are an experiment and may change without
  notice.
- **Packages come from paths, git, and the official set.** There is no package registry. A dependency is a directory or a
  git repository in `burdin.toml`. An official package is not declared at all: a `use` of its name loads it.
- **`Option.as_ref` is not provided.** A reference to a `match` binding is copied into a slot by the code generator, so
  `Some(&v)` is accepted and then dangles. That is a compiler bug, recorded in the specification and not yet fixed.
- **Statics cannot hold atomics or `Mutex`.** A `static` is built at compile time, and those types cannot be built then.
  Pass a shared counter to the tasks that use it instead. See [global variables](/guide/globals/).
- **Some borrows last a whole statement.** `s = s + "x"` does not compile (E0111). Write `let t = s + "x"` and then
  `s = t`.
- **HTTPS is not in the `http` package.** The `tls` package is the way to secure a connection, and it needs OpenSSL 3 to
  link.
- **The cryptography is a demonstration.** The `crypto` package implements the algorithms in pure Burdin and checks them
  against known answers. It is not a security claim, and it should not protect real data.
- **Stack traces in `main` are rough.** An error returned from `fn main(): Result<T>` prints a trace that names an internal
  frame. The message itself is correct.
- **Libraries are a first step.** A `[lib]` table builds a static or a shared library for C callers, on Linux as the
  tested platform. The Windows and macOS artifacts are not yet tested in CI, and a Burdin program cannot link a built
  artifact (it compiles the library's source). `--lto` and the sanitizers do not apply to a library, and there is no
  `root` key or per-profile `[lib]`. See [packages and modules](/guide/packages-and-modules/#libraries).
- **LLVM 21 is required.** Building the compiler from source, and running `burdin build`, need the LLVM 21 libraries. The
  [installation guide](/getting-started/installation/) describes each platform.

## Reporting a problem

A problem with a program, a message, or a missing feature is worth reporting in the
[issue tracker](https://github.com/RicardoMilos7/burdin/issues). Include the smallest program that shows the problem, the
`burdin --version` output, and the platform.
