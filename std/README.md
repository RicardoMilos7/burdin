# The Burdin standard library

The standard library (stage 0 plus the stage 1 language surface: closures, iterators, formatting, `any`/`impl`,
errors, and allocators), written in Burdin. The compiler embeds these files in `burdin` and makes the package a dependency of every
program, then copies its public names into every module (the prelude). A project that declares its own package named
`std` replaces this one.

| Module | Contents |
| --- | --- |
| `std.bd` | the `Error` marker interface and the dynamic `Error`, `TraceFrame`, the `IoError` error enum and its `IoErrorKind`, `ParseIntError`, and `AssertError`, `CStr`, `assert`, `assert_eq`, `__test_assert`, `__test_assert_eq`, `unreachable` |
| `io.bd` | `read_to_string`, `write_file`, and the `stdin`, `stdout`, and `stderr` handles (`spec/stdlib.md`, Streams) |
| `rw.bd` | the `Read`, `Write`, `Seek`, and `BufRead` interfaces, `SeekFrom`, `Cursor`, `BufReader`, `BufWriter`, `Lines`, and `copy` (`spec/stdlib.md`, Readers and writers) |
| `fs.bd` | `read_dir`, `create_dir`, `create_dir_all`, `remove_dir`, `remove_dir_all`, `remove_file`, `rename`, `metadata`, `canonicalize`, `walk`, and `walk_glob`, with `Metadata` and `FileKind` (`spec/stdlib.md`, "Directories, metadata, and walking") |
| `env.bd` | `args` |
| `process.bd` | `exit`, `run` |
| `mem.bd` | `swap`, `replace` |
| `iter.bd` | `Iterator<T>` and `Iterable<T>`, the lazy adapters and their types, and the eager `List` helpers |
| `fmt.bd` | `Display` and `Debug`, the `Formatter`, and the formatting helpers |
| `alloc.bd` | the `Allocator` interface, the default `Global` allocator, and `Arena` (`spec/allocators.md`) |

An `error enum` declaration lowers to an ordinary enum plus generated `Display`, `Error`, and `From<Source>`
impls (`spec/errors.md`, ERR1-ERR3). The `Error` interface marks a type as an error; the `Error` struct is the
dynamic error `Result<T>` uses, with its context messages and debug trace frames (ERR4-ERR7). `AssertError` is the
type of a failed `assert`/`assert_eq` in a `burdin test` build; a normal build keeps the panicking assertions
(ERR9).

Some names in the stage 0 table are compiler built-ins rather than Burdin code, because the language cannot
express them yet: `Option`, `Result`, `Heap`, `List`, `Map`, `String`, `Shared`, `str`, the `str`/`[T]` intrinsics,
the number methods (`to_string`, `parse`, `wrapping_*`, `checked_*`, `min`, `max`, `abs`), the
`__read_to_string`, `__write_file`, `__read_dir`, `__create_dir`, `__remove_dir`, `__remove_file`, `__rename`,
`__metadata`, `__canonicalize`, `__io_kind`, `__stdout_write`, `__stdout_write_line`, `__stdout_write_bytes`,
`__stdout_flush`, `__stderr_write`, `__stderr_write_line`, `__stderr_write_bytes`, `__is_terminal`,
`__stdin_read_line`, `__stdin_read_to_string`, `__stdin_avail`, `__stdin_next_byte`, `__stdin_error`, `__args`,
`__env_get`, `__env_set`, `__env_set_bytes`, `__env_remove`, `__current_dir`, `__set_current_dir`,
`__set_current_dir_bytes`, `__exit`, `__run`, `__swap`, and `__replace` primitives the modules above call, and
`print`/`println`/`eprint`/`eprintln`/`panic`. The I/O primitives are typed `Result<..., i32>`: the error is the
operating system's code, and the `std` wrappers use `__io_kind` to build the `IoError` with its portable kind
(`spec/errors.md`, ERR10-ERR11). The integer-parse primitives are typed with `ParseIntError` and produce a
placeholder error value; the `std` wrappers replace it with `Invalid`.

Because the sources are embedded at compile time, changing a file here requires rebuilding `burdin`
(`cargo build`). To use a modified standard library without rebuilding, point a package at its own `std`
package: packages that declare `std` themselves keep it.
