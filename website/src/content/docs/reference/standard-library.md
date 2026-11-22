---
title: Standard library
description: The modules and types the standard library provides, grouped by what they are for, with links to the full signatures.
---

The standard library is written in Burdin and ships with the compiler. Most of it is available without an import: the
core types (`String`, `List`, `Map`, `Option`, `Result`) and the output functions are part of every program. Names that
live in a module are reached through it, as in `fs.read_to_string` or `time.now()`.

The full names and signatures are in the
[standard library specification](https://github.com/RicardoMilos7/burdin/blob/main/spec/stdlib.md). This page is the map:
what each area is for, and the type to start with.

## Printing and input

- **Output.** `println`, `print`, `eprintln`, `eprint`, and `format`. Each takes a literal with `{...}` placeholders. See
  [strings and formatting](/guide/strings-and-formatting/).
- **Streams.** `stdout()`, `stderr()`, and `stdin()` return handles that implement the reader and writer interfaces.
- **Readers and writers.** `Read`, `Write`, and `BufRead`, with `BufReader`, `BufWriter`, and `copy`.

## Values

- **Numbers and parsing.** Integer and float functions, checked and wrapping arithmetic, and `parse` on each numeric type.
  Parsing returns a `Result`.
- **Characters.** Classification and conversion of `char` values.
- **Operators.** Operator interfaces that a user type can implement; see
  [generics and interfaces](/guide/generics-and-interfaces/).
- **Conversions.** `From`, `Into`, `TryFrom`, and the rules for `as` between numeric types.
- **Hashing and ordering.** `Ordering`, `cmp`, `Reverse<T>`, and the `Hash` interface.

## Text and bytes

- **`String` and `str`.** Building, editing, searching, splitting, and trimming text. A `str` is a view, and a `String`
  owns its text.
- **Encodings and byte buffers.** UTF-8 checks, checked slicing, and byte buffers for binary data.

## Collections

- **`List<T>`** and **slices** `[T]`. Growable lists, sorting with a comparator, and read-only or mutable views.
- **`Map<K, V>`** and **`Set<T>`**. Hash-based collections with entry operations and views. A map has no order.
- **`Deque<T>`** and **`PriorityHeap<T>`**. A double-ended queue and a priority queue.
- **`Once<T>`** and **`Lazy<T, F>`**. Values computed once, on first use.
- **Collecting.** Building a `List`, `Map`, or `Set` from an iterator, and extending a collection from one.

## Iteration

`Iterable<T>` and `Iterator<T>`, `iter()` and `into_iter()`, and the lazy adapters `map`, `filter`, `take`, and `fold`.
`Range<T>` and `RangeInclusive<T>` are iterators too. See [closures and iterators](/guide/closures-and-iterators/).

## Errors and results

- **`Option<T>` and `Result<T, E>`.** Combinators such as `map`, `and_then`, and `unwrap_or`, and the `?` operator.
- **`Error` and `context`.** The dynamic error type, the `Result<T>` shorthand, and adding context to a chain of errors.
  See [errors](/guide/errors/).

## Memory

- **`Heap<T>`.** A single owned value on the heap.
- **Allocators.** Fallible allocation, `Arena`, and allocator parameters for containers.
- **Memory helpers.** Helpers and assertions for memory, and an optimizer barrier for benchmarks. The `mem` module adds no
  `size_of` or `align_of`.

## Time, randomness, and the environment

- **Time.** Instants, durations, wall-clock time, and `sleep`.
- **Random numbers.** `Random.seeded(seed)` for a repeatable sequence, `Random.from_entropy()` for a fresh one, and
  `shuffle` for lists.
- **Paths, directories, and files.** Path manipulation, directory walking, metadata, and file handles that read and
  write text and bytes.
- **Environment and processes.** `env.get`, `env.set`, and the working directory, `process.exit`, and child processes.

## Networking

TCP sockets and their addresses. Inside a task, socket operations wait without blocking a thread. See
[concurrency](/guide/concurrency/).

## Tasks, channels, and shared state

- **Tasks and threads.** `task.spawn` and `task.scope` run work on the task pool; `thread.spawn` starts a thread for
  blocking calls.
- **Channels.** `Channel<T>` with `send`, `recv`, and `select` over several channels.
- **Shared state.** `Send` and `Sync`, atomics, `Mutex<T>`, `RwLock<T>`, and `Shared<T>`.

## Serialization

The serialization framework: `Serialize` and `Deserialize`, derives for structs and enums, and the format-side
serializers. The `json` and `toml` official packages build on it. See
[packages and modules](/guide/packages-and-modules/).

## The C bridge

`extern "C"` declarations, `CPtr<T>` for raw pointers at the boundary, and `CString` and `CStr` for C strings. Every call
to a C function is `unsafe`. See [coming from C and C++](/coming-from/c-and-cpp/).

## Official packages

Some libraries ship with the compiler and are added with `name = true` in `burdin.toml`, or with `burdin add name`:
`json`, `toml`, `crypto`, `math`, `http`, and `tls`. Their specifications are in the repository:
[packages](https://github.com/RicardoMilos7/burdin/blob/main/spec/packages.md),
[http](https://github.com/RicardoMilos7/burdin/blob/main/spec/http-client.md),
[crypto](https://github.com/RicardoMilos7/burdin/blob/main/spec/crypto.md),
[math](https://github.com/RicardoMilos7/burdin/blob/main/spec/math.md), and
[tls](https://github.com/RicardoMilos7/burdin/blob/main/spec/tls.md).
