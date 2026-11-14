# The Burdin standard library

The stage 0 standard library, written in Burdin. The compiler embeds these files in `burdin` and makes the
package a dependency of every program, then copies its public names into every module (the prelude). A
project that declares its own package named `std` replaces this one.

| Module | Contents |
| --- | --- |
| `std.bd` | `IoError`, `ParseIntError`, `AssertError`; `assert`, `assert_eq`, `unreachable` |
| `io.bd` | `read_to_string`, `write_file` |
| `env.bd` | `args` |
| `process.bd` | `exit`, `run` |
| `mem.bd` | `swap`, `replace` |

Some names in the stage 0 table are compiler built-ins rather than Burdin code, because the language cannot
express them yet: `Option`, `Result`, `Heap`, `List`, `Map`, `String`, `str`, the `str`/`[T]` intrinsics,
the number methods (`to_string`, `parse`, `wrapping_*`, `checked_*`, `min`, `max`, `abs`), the
`__read_to_string`, `__write_file`, `__args`, `__exit`, `__run`, `__swap`, and `__replace` primitives the
modules above call, and `print`/`println`/`eprint`/`eprintln`/`panic`.

Because the sources are embedded at compile time, changing a file here requires rebuilding `burdin`
(`cargo build`). To use a modified standard library without rebuilding, point a package at its own `std`
package: packages that declare `std` themselves keep it.
