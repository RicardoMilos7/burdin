# Dependencies and licenses

Burdin's compiler depends on a small, reviewed set of Rust crates and on LLVM. This page records the
policy and the review; the checked, per-crate record is [`tests/dependencies.txt`](../tests/dependencies.txt),
and [`tests/check_deps.py`](../tests/check_deps.py) enforces it in every CI job (issue #449).

## Policy

- **Two direct external dependencies, both reviewed.** `inkwell` wraps LLVM 21, and `libc` supplies
  the host's C type layouts: `burdin_codegen` reads `struct stat` and `struct dirent` through it for
  the directory and metadata operations (issue #411). Every other external crate is a transitive
  dependency of one of them, and adding a direct dependency is a deliberate change reviewed like any
  other.
- **Exact pins in the manifests, one locked version per crate.** An external requirement is written
  `=x.y.z`, never a range. `Cargo.lock` is committed and records the resolved version, the crates.io
  source, and the checksum of every crate, so a build is reproducible and a changed crate cannot pass
  unnoticed. No git or path patches exist outside the workspace.
- **Permissive licenses only.** Every identifier in a crate's SPDX license expression must be MIT,
  MIT-0, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Zlib, BSL-1.0, Unlicense, CC0-1.0,
  Unicode-3.0, or Unicode-DFS-2016, or the expression must contain the exact term
  `Apache-2.0 WITH LLVM-exception`. A copyleft license (GPL, LGPL, AGPL, MPL, EPL, ...) fails the check,
  including when it appears as one branch of an `OR`, so the project never has to choose that branch.
- **The license strings are not taken on trust.** CI runs the audit after `cargo build`, when the
  registry cache is populated, and requires `cargo metadata --locked --offline` to succeed. That
  re-reads each crate's license from its own metadata, proves the lockfile is still current with the
  manifests (`--locked`), and fails when the reviewed inventory differs.
- **No vendoring.** LLVM, the C library, and the C compiler driver come from the host toolchain and
  are not copied into the repository. The build links the LLVM 21 shared library, so the distributed
  program depends on it at run time; `burdin --version` prints the linked LLVM version.

## Review

The crate families, from [`tests/dependencies.txt`](../tests/dependencies.txt):

| Crates | License | Why |
| --- | --- | --- |
| `inkwell`, `inkwell_internals` | Apache-2.0 | The LLVM backend bindings and their derive macros. `burdin_codegen` is the only crate that uses them, and they are the reason the compiler needs LLVM. |
| `llvm-sys` | MIT | The raw LLVM C API under `inkwell`; its build script finds `llvm-config` and probes the toolchain. |
| `anyhow`, `cc`, `find-msvc-tools`, `lazy_static`, `regex-lite`, `semver`, `shlex` | MIT OR Apache-2.0 | Build-time support for `llvm-sys`: error types, the C toolchain probe, process-wide configuration, `llvm-config` parsing, version comparison, and flag quoting. None of it is linked into the compiler. |
| `bitflags`, `libc`, `thiserror`, `thiserror-impl` | MIT OR Apache-2.0 | Runtime support inside `inkwell` and `llvm-sys`: attribute flags, C types, and error types. `libc` is also a direct dependency of `burdin_codegen`, which reads the host's `struct stat` and `struct dirent` layouts from it so metadata and directory listings use the exact C offsets, and a dev-dependency of `burdin_tir`, whose tests check the I/O error tables against the host's `errno` constants. |
| `proc-macro2`, `quote`, `syn`, `unicode-ident` | (MIT OR Apache-2.0), the identifier tables also Unicode-3.0 | Macro plumbing of the two derive crates. Compile-time only. |

LLVM itself, which is not a Rust crate and is not in `Cargo.lock`, is distributed under
Apache-2.0 WITH LLVM-exception. The Linux and macOS jobs install LLVM 21 from apt.llvm.org and
Homebrew; the Windows job uses the official LLVM release archive.

## Adding or updating a dependency

1. Write the exact version in `Cargo.toml` (or `[workspace.dependencies]`), and build so `Cargo.lock`
   records the resolution.
2. Review the crate: maintenance, license, build scripts, and whether the standard library or an
   existing crate would do instead.
3. Add or change its line in [`tests/dependencies.txt`](../tests/dependencies.txt) with its license
   and why the compiler needs it.
4. Run `tests/check_deps.py` (CI runs it with `BURDIN_DEPS_REQUIRE_METADATA=1` after `cargo build`).

The audit covers provenance, pinning, and licenses. It is not a vulnerability scanner: it does not
fetch the RustSec advisory database. A security review of a dependency change is still a review step,
and running `cargo audit` locally is a reasonable extra check (it is deliberately not a CI dependency
because it needs the advisory database over the network).
