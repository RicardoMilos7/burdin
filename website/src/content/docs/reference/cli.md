---
title: Command line
description: Every burdin command, what it does, and where to read more. Output from burdin --help for the installed version.
---

The `burdin` program builds, runs, tests, checks, formats, and documents Burdin code. Every command takes a single `.bd`
file or a package directory (one with a `burdin.toml`), unless it says otherwise.

```console
$ burdin --help
$ burdin help build
```

`burdin help <command>` prints the full options of one command, and `burdin commands` lists every command in alphabetical
order.

## Building and running

| Command | What it does |
| --- | --- |
| `build` | Compile a file or package into an executable. `-o` names the output, `--release` selects the release profile. |
| `run` | Build a program and run it. |
| `check` | Type-check a program and run the memory checks, without generating code. It is the fastest way to see errors. |
| `clean` | Remove a package's build output. With `--cache`, also remove its caches. |
| `debug` | Build a program with debug information and start a debugger on it. |
| `watch` | Rerun `check`, `build`, `test`, or `run` whenever the sources change. |
| `repl` | Start an interactive session that checks and runs what you type. |

```console
$ burdin run hello.bd
$ burdin build -o hello hello.bd
$ burdin check .
```

### Building for another platform

`burdin build --target <triple>` builds for a platform other than the one you are on, and `burdin check --target
<triple>` checks the program as it is there. The targets are `x86_64` or `aarch64` with `unknown-linux-gnu`,
`apple-darwin`, or `pc-windows-msvc`. `target.os()` and `target.arch()` report the target, so a `comptime if` on them
keeps the target's code.

```console
$ burdin build --target aarch64-unknown-linux-gnu --emit obj,asm -o hello hello.bd
$ BURDIN_CC=clang BURDIN_SYSROOT=/opt/aarch64-sysroot \
    burdin build --target aarch64-unknown-linux-gnu -o hello hello.bd
```

The link needs a clang driver and the target's system root, which you provide: `BURDIN_CC_<TRIPLE>` or `BURDIN_CC`
names the driver (`<TRIPLE>` is the triple in capitals with `_` for `-`, such as `BURDIN_CC_AARCH64_UNKNOWN_LINUX_GNU`),
and `BURDIN_SYSROOT_<TRIPLE>` or `BURDIN_SYSROOT` names the root. The linker is lld unless `BURDIN_LD` names another. A
Windows target links with `clang-cl` and the Windows SDK. Without a cross toolchain, `--emit obj` still writes the
object file. A program built for another platform cannot be run, tested, or benchmarked by Burdin, so `--target` is
only for `build` and `check`; it does not combine with `--sanitize`, profile-guided optimization, or a `[rust.crates]`
package. See `spec/cross-compilation.md` for the exact rules.

## Testing and measuring

| Command | What it does |
| --- | --- |
| `test` | Build and run the `test` blocks of a file or package. |
| `bench` | Build and measure the `bench` blocks, with statistics and baselines. |
| `fuzz` | Build a `fuzz` target with libFuzzer and AddressSanitizer, and fuzz it. |

```console
$ burdin test .
$ burdin test --list .
```

## Code quality and changes

| Command | What it does |
| --- | --- |
| `fmt` | Format source files in the one style Burdin uses. There are no options to choose from. |
| `lint` | Report common mistakes and style issues, and with `--fix`, apply the safe fixes. |
| `fix` | Apply the machine-applicable fixes of the compiler and the linter across a package. |
| `ssr` | Structural search and replace: match a pattern written as Burdin code across a package. |
| `refactor` | Extract a function, inline one, change a signature, or move an item. |

## Packages and dependencies

| Command | What it does |
| --- | --- |
| `new` | Create a new package in a new directory. |
| `init` | Create a package in an existing directory. |
| `add` | Add a path or git dependency to `burdin.toml`, for example `burdin add geo --path ../geo`. Official packages are not added: `use json` loads one. |
| `vendor` | Copy the git dependencies into `vendor/` for offline builds. |
| `package` | Write the package's reproducible archive to `target/package`. |
| `bindgen` | Generate a Burdin module from a C header or a Rust crate, or a C header from a package. |

## Documentation and inspection

| Command | What it does |
| --- | --- |
| `doc` | Generate the documentation of a package as a site, with search, JSON, and a coverage report. |
| `explain` | Print the reference entry for an error code, such as `burdin explain E0100`. |
| `expand` | Print the source after compile-time evaluation, derives, default arguments, and operators. |
| `asm` | Print the assembly or LLVM IR of one function, next to its source. |
| `bloat` | Report the size of each function, module, package, and generic in the executable. |

## Editors

| Command | What it does |
| --- | --- |
| `lsp` | Run the language server on standard input and output. The [editor setup](/getting-started/editors/) uses it. |
| `dap` | Run the debug adapter on standard input and output. |

## Where to read more

The complete behavior of each command is in the repository's specification:
[build tool](https://github.com/RicardoMilos7/burdin/blob/main/spec/build-tool.md),
[check](https://github.com/RicardoMilos7/burdin/blob/main/spec/check.md),
[test runner](https://github.com/RicardoMilos7/burdin/blob/main/spec/test-runner.md),
[formatter](https://github.com/RicardoMilos7/burdin/blob/main/spec/fmt.md),
[lint](https://github.com/RicardoMilos7/burdin/blob/main/spec/lint.md), and
[packages](https://github.com/RicardoMilos7/burdin/blob/main/spec/packages.md). The
[language reference](/reference/language-spec/) lists every specification file.
