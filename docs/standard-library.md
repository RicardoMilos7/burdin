# The standard library

The standard library is written in Burdin and built into the compiler. This guide is the map: what each module holds,
which spec section defines its behavior, and which conformance folder tests it. The API reference is generated from the
doc comments in `std/src/` by the website's build (`website/scripts/build-api-docs.mjs`), so nothing generated is
committed.

## Reading the reference

- `burdin doc --std` renders the site, and `burdin doc --std --output-format json` prints the same API as JSON. The site lists every public item of every module, with its signature, its doc comment, and its examples.
- Every public item has a doc comment and at least one example. Each example is a standalone program, built and run
  with the doc-test attributes (`ignore`, `no_run`, `should_panic`, `compile_fail`): `burdin doc --std` shows them, and
  `tests/check_std_docs.py` runs them.
- Examples are shown with `fn main` left out; the checker wraps them in one and hoists their `use` lines and their
  top-level declarations above it.
- Names that begin with `__` are compiler-internal: the language desugars `select` into calls to them. They stay public for that reason and are left out of the site (spec/doc.md, DC7).
- A module is named as in `std/src/` (`src/iter.bd` is the `iter` module). Most names resolve unqualified: `Map`,
  `swap`, `assert_eq`, `format`, and `read_to_string` need no `use`. The module name is a namespace for the calls that
  would clash, such as `thread.spawn` and `task.spawn`.

## Modules

Each row links the module's page in the reference, the spec sections that define it, and the conformance folders that
test it. A folder under `tests/` named `m` or `s` followed by a number is a milestone or a stage-1 group (see
`spec/stage0-subset.md` and `spec/stage1-subset.md`).

| Module | What it holds | Spec | Tests |
|---|---|---|---|
| `std` | The prelude: `assert`, `format`, `Error`, `Result` helpers, output | [stdlib](../spec/stdlib.md), [errors](../spec/errors.md) | [m24_stdlib](../tests/m24_stdlib/), [m16_errors](../tests/m16_errors/) |
| `alloc` | Allocators, `Arena`, `AllocError` | [allocators](../spec/allocators.md) | [m23_allocators](../tests/m23_allocators/) |
| `atomic` | `AtomicBool` and the integer atomics | [shared](../spec/shared.md) | [s2_shared](../tests/s2_shared/) |
| `bytes` | `Bytes` and `BytesMut`, byte buffers | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `channel` | `Channel<T>`, `select` helpers | [channels](../spec/channels.md) | [s2_channels](../tests/s2_channels/) |
| `cmp` | `Ordering`, `Reverse`, `min`, `max`, and `clamp` | [derives](../spec/derives.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `deque` | `Deque<T>`, a double-ended queue | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `encoding` | Base64, hex, URL percent-encoding, and UTF-8 | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `env` | Environment variables, arguments, and the current directory | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `float` | `f32` and `f64` methods and parsing | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `fmt` | `Display`, `Debug`, `Formatter`, and format specifiers | [format](../spec/format.md) | [m14_format](../tests/m14_format/) |
| `fs` | Files, directories, metadata, and `OpenOptions` | [io](../spec/io.md) | [s2_io](../tests/s2_io/), [m24_stdlib](../tests/m24_stdlib/) |
| `hash` | `Hash`, `Hasher`, and hashing | [derives](../spec/derives.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `heap` | `PriorityHeap<T>`, ordered by `Ord` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `int` | Integer methods: checked, wrapping, saturating, bits | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `io` | `IoError`, `IoErrorKind`, the standard streams, and whole-file helpers | [io](../spec/io.md) | [s2_io](../tests/s2_io/) |
| `iter` | `Iterator`, `Iterable`, and the adapters | [iterators](../spec/iterators.md) | [m13_iterators](../tests/m13_iterators/) |
| `lazy` | `Lazy` and `Once`, lazily computed values | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `list` | `List<T>` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `mapops` | The rest of `Map<K, V>`: entries, views, construction | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `marker` | The `Send` and `Sync` markers | [shared](../spec/shared.md) | [s2_shared](../tests/s2_shared/) |
| `mem` | `swap`, `replace`, `take`, and `drop` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `net` | `SocketAddr`, TCP and UDP sockets | [io](../spec/io.md) | [s2_io](../tests/s2_io/) |
| `ops` | The operator interfaces and `Index` | [operators](../spec/operators.md) | [m19_operators](../tests/m19_operators/) |
| `option` | `Option<T>` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `path` | Paths, with their extension methods on `Path` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `process` | `Command`, `Child`, `Output`, and exit status | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `random` | `Random`, a seeded generator | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `reactor` | The I/O reactor and the helper pool | [io](../spec/io.md) | [s2_io](../tests/s2_io/) |
| `rw` | `Read`, `Write`, `Seek`, `BufRead`, `Cursor`, and buffered I/O | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `serialize` | `Serialize` and `Deserialize`, the test format | [serialization](../spec/serialization.md) | [s3_serialize](../tests/s3_serialize/) |
| `set` | `Set<T>` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `slice` | Slice methods, `join`, `concat`, and `chunks` | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `sort` | Sorting | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `sync` | `Mutex<T>`, `RwLock<T>`, and `Shared<T>` | [shared](../spec/shared.md) | [s2_shared](../tests/s2_shared/) |
| `target` | The operating system and architecture the program is compiled for | [comptime](../spec/comptime.md) | [m20_comptime](../tests/m20_comptime/) |
| `task` | Tasks, scopes, and the scheduler | [tasks](../spec/tasks.md) | [s2_tasks](../tests/s2_tasks/) |
| `text` | `String`, `str`, and the text methods | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `thread` | Threads and their join handles | [tasks](../spec/tasks.md) | [s2_tasks](../tests/s2_tasks/) |
| `time` | `Duration`, `Instant`, clocks, and dates | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |
| `unicode` | Character classes and case conversion | [stdlib](../spec/stdlib.md) | [m24_stdlib](../tests/m24_stdlib/) |

Packages that ship with the compiler but are not part of `std` have their own specs and folders: `math`
([spec](../spec/math.md)), `crypto` ([spec](../spec/crypto.md)), `toml` ([spec](../spec/toml.md)), `http`
([spec](../spec/http-client.md)), and `tls` ([spec](../spec/tls.md)). Their reference is built with the package, not
with `--std`.

## Keeping the reference current

`tests/check_std_docs.py` is the gate (spec/doc.md, DC25). It fails when a public item has no doc comment, when an item
has no example and is not in `tests/std_docs/no_example.txt` (a list that may only shrink), when an example does not
build or run as its attributes say. After a change to `std/src/`, rebuild the driver (the sources are compiled
in) and run the gate:

```
python3 tests/check_std_docs.py
```

Generated files come from a template, not from their output: `std/src/int.bd` from `tests/gen_int_std.py`,
`std/src/atomic.bd` from `tests/gen_atomic_std.py`. Their examples live in the template, so edit the template and
regenerate the file.
