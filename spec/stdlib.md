# Burdin standard library (stage 0)

The names and signatures a stage 0 program may use. They are a language decision, not an implementation detail: a
change here changes the tests and the compiler together. Issues #28, #29, #30, and #31 settled this surface; the
tests named under each section witness it.

Signatures are written in Burdin. A parameter without `mut` or `own` is read-only and borrowed for the call; `mut`
takes `mut` at the call site; `own` moves and takes no marker (`memory-rules.md`, CV). `T` is a value type that can be
stored (not a second-class view). The one implicit conversion applies throughout: a `String` passed where a `str`
is expected is borrowed as a `str` (IC1).

## Output

```
fn print(s: str)
fn println(s: str)
fn eprint(s: str)
fn eprintln(s: str)
```

Each writes to stdout or stderr. `println`/`eprintln` add one `\n`. A `String` argument is converted by IC1.
Standard output is buffered and standard error is not; the buffer is flushed when it fills, after a line when
stdout is a terminal, and before the process ends through a normal return, `process.exit`, a panic, or the
stack-overflow guard. A program never sees the difference except through the timing of the bytes (there is no
explicit output handle in stage 0); the flush points are what keep the order of stdout and stderr correct before
an abnormal end.

## Numbers and parsing

Every integer type (`i8`..`i64`, `isize`, `u8`..`u64`, `usize`) has:

```
fn to_string(self): String
fn wrapping_add(self, other: Self): Self
fn wrapping_sub(self, other: Self): Self
fn wrapping_mul(self, other: Self): Self
fn checked_add(self, other: Self): Option<Self>
fn checked_sub(self, other: Self): Option<Self>
fn checked_mul(self, other: Self): Option<Self>
fn min(self, other: Self): Self
fn max(self, other: Self): Self
fn abs(self): Self                    // signed types only
fn parse(s: str): Result<Self, ParseIntError>
```

`bool`, `char`, and `str` have `to_string(self): String`. `bool` also works in `if`/`while` conditions. `parse` is
the associated function `i32.parse(s)`; it accepts an optional leading `-` and digits in the base 10 and returns
`Err(())` (`ParseIntError` is a unit alias in stage 0) for anything else. Floats have no methods in stage 0.

Arithmetic is the built-in operators only: `+ - * / %`, `<< >>`, `& | ^`, unary `-`. Overflow panics in debug and
wraps in release for `+ - *` and unary `-`; division by zero and out-of-range shifts always panic. `/` and `%`
truncate toward zero. `as` converts between numeric types (and `char` to an integer), following Rust's cast rules.

## `String` and `str`

A string literal has type `str`. `str` is a copied, second-class view; `String` is the owning type.

```
struct String {
    fn new(): String                  // associated function: String.new()
    fn from(s: str): String           // associated function: String.from(s); copies
    fn push_str(mut self, s: str)
    fn push(mut self, c: char)
    fn len(self): usize               // bytes, not characters
    fn starts_with(self, prefix: str): bool
    fn find(self, c: char): Option<usize>        // byte offset of the first occurrence
    fn find_str(self, needle: str): Option<usize> // byte offset of the first occurrence
    fn as_str(self): str
    fn chars(self): Chars             // Unicode scalar values
    fn clone(self): String            // deep copy of the buffer
}
```

`str` has the same read methods (`len`, `starts_with`, `find`, `find_str`, `as_str`, `chars`, `to_string`), without
`push*`, `from`, or `clone`. `Chars` is only iterable: `for c in s.chars()`.

Slicing `s[a..b]` and `s[a..=b]` yields a `str` view of the bytes in the range; it works on `str`, `String`, arrays,
and slices. `a` and `b` are `usize`; a reversed or out-of-range range panics. `len` counts bytes, and a slice may
split a multi-byte character: the language does not check UTF-8 boundaries.

`==` and `!=` compare two `str` values or two `String` values byte for byte. `<`, `<=`, `>`, `>=` compare two `str`
values or two `String` values lexicographically by bytes. The operands must have the same type; `s == "x"` does not
convert, and there is no `Ord` for a mixed pair.

## `List<T>`

```
struct List<T> {
    fn new(): List<T>                 // associated function: List<T>.new()
    fn push(mut self, value: T)       // takes ownership of the value
    fn pop(mut self): Option<T>
    fn len(self): usize
    fn last(self): Option<&T>
    fn insert(mut self, index: usize, value: T)
    fn remove(mut self, index: usize): T
    fn clear(mut self)
    fn contains(self, value: T): bool // needs == on T
    fn sort(mut self)                 // ascending; needs < on T
}
```

Indexing `xs[i]` is a place: it reads or writes one element, takes a `usize`, and panics when out of bounds.
`for x in xs` borrows the list for the loop and yields read-only element places (`memory-rules.md`, IT). `T` must be
storable, so `List<str>` and `List<&T>` are rejected (E0120).

## `Map<K, V>`

```
struct Map<K, V> {
    fn new(): Map<K, V>               // associated function: Map<K, V>.new()
    fn insert(mut self, key: K, value: V)   // replaces an existing key's value
    fn get(self, key: K): Option<&V>
    fn remove(mut self, key: K): Option<V>
    fn contains_key(self, key: K): bool
    fn clear(mut self)
    fn len(self): usize
}
```

Keys need `==` (`Eq`). `for (k, v) in map` borrows the map for the loop; iteration order is not specified, so a
program must not depend on it. The stage 0 implementation is a linear scan, but that is not part of the interface.

## `Heap<T>`

```
struct Heap<T> {
    fn new(value: T): Heap<T>         // associated function: Heap.new(value); moves the value in
}
```

`Heap<T>` owns one allocation. `*h` on an owned `Heap<T>` moves the whole pointee out and frees the allocation;
reading or assigning through `*h` does not move. Moving a field through `*h` is not supported in stage 0
(`memory-rules.md`, MV6). Dropping a `Heap` drops the pointee and frees the allocation.

## `Option<T>` and `Result<T, E>`

`Option<T>` is built in with the variants `Some(T)` and `None`; `None` takes its type from context. `Result<T, E>`
is built in with `Ok(T)` and `Err(E)`; stage 0 always names `E`.

```
Option<T>:  unwrap(), expect(msg: str): T, is_some(): bool, is_none(): bool
Result<T, E>: unwrap(), expect(msg: str): T, is_ok(): bool, is_err(): bool
```

`unwrap` panics on `None`/`Err`; `expect(msg)` panics with `msg`. `expr?` returns early through the enclosing
`Option` or `Result` function (E0160 outside one) and converts the error through a user `Type: From<Source>` block.
`expr orelse fallback` supplies a value for `None` or `Err`. `match` handles both types; `Option` also supports
`==`/`!=` when its payload does, and `Hash` when its payload does. Stage 0 has no `map`, `and_then`, `unwrap_or`,
`ok_or`, or other combinators (stage 1, issue #481).

## Files, processes, and environment

```
fn read_to_string(path: str): Result<String, IoError>
fn write_file(path: str, text: str): Result<(), IoError>
fn env.args(): List<String>                 // program name first
fn process.exit(code: i32): !               // ends the process immediately
fn process.run(command: str): Result<i32, IoError>
```

`read_to_string` and `write_file` read and write raw bytes: there is no newline translation on Windows and no UTF-8
normalization, so a string written and read back is byte-identical (`len` is the byte count). `process.run` runs the
command through the system shell and returns its status: the POSIX wait status is shifted, the Windows status is
returned as is. `IoError` is `()` in stage 0.

## Memory helpers and assertions

```
fn mem.swap(mut a: T, mut b: T)
fn mem.replace(mut place: T, value: T): T   // returns the old value
fn panic(msg: str): !
fn assert(cond: bool)
fn assert_eq<T: Eq>(a: T, b: T)
fn unreachable(): !
```

`swap` and `replace` take places and are checked for exclusivity like any other call. `panic` prints its message and
location and aborts (`memory-rules.md`, DR10); `unreachable()` panics with `unreachable`; `assert` panics with
`assertion failed`; `assert_eq` panics when the values differ. In a `burdin test` build the prelude replaces
`assert`/`assert_eq` with versions that return `Err(())` instead of panicking.

## C bridge

```
struct CPtr<T>                       // a raw, Copy pointer; no drop
fn CPtr.null(): CPtr<T>              // associated function; no arguments; type from the expected type
type CChar = i8
type CSize = usize
type CVoid = ()                      // only ever behind a CPtr

struct CString {
    fn from(s: str): CString         // copies the bytes and appends a NUL
    fn as_ptr(self): CPtr<CChar>
}
type CStr = CPtr<CChar>              // a std alias
fn CStr.to_string(ptr: CStr): String // reads the NUL-terminated bytes; associated function
```

`CPtr<T>` supports only `null`, assignment, copying, and passing to/from `extern "C"`; stage 0 has no pointer
dereference, arithmetic, comparison, or null test (stage 1). `CString` owns its buffer, has `Drop`, and has no clone.
`CStr.to_string` is the associated function of `CPtr<CChar>` and copies the bytes with `strlen`.

`extern "C" { fn name(params): ret }` declares a C function. Parameters are passed by value and may be integers,
floats, or `CPtr<T>`; the result may be one of those or unit. `mut`, `own`, `self`, generics, other ABIs, and a
`main` or `burdin_*` symbol are rejected (E0162 for a type or convention, E0001 for the syntax of an unsupported
ABI). A symbol is declared once: a second declaration with the same signature reuses it, a different signature is an
error. Calling an `extern` function needs `unsafe` (E0161); holding or moving a `CPtr` does not.

## Tests

| Area | Tests |
| --- | --- |
| Output, numbers, parsing | `m02_basics/arithmetic`, `casts_and_numeric_methods`, `m09_stdlib/string_ops` |
| `String`, `str`, slicing, ordering | `m09_stdlib/string_ops`, `string_order`, `m06_conventions/str_views`, `slice_views` |
| `List` | `m09_stdlib/list_ops`, `list_of_drops`, `m06_conventions/for_borrows_list` |
| `Map` | `m09_stdlib/map_ops`, `word_count` |
| `Heap` | `m05_ownership/heap_tree_eval` |
| `Option`, `Result`, `?`, `orelse` | `m09_stdlib/option_result_basics`, `question_and_from` |
| Files, processes, environment | `m09_stdlib/file_io`, `file_io_binary`, `env_args`, `exit_code`, `process_run` |
| Memory helpers and assertions | `m09_stdlib/std_helpers`, `unreachable_aborts`, `m10_test_runner/*` |
| C bridge | `m08_modules/extern_libc`, `cstr_roundtrip`, `m11_llvm/llvm_return_42` |
| Rejected names, arities, and C ABI types | `m09_stdlib/unknown_stdlib_names` (E0002, E0004), `m08_modules/cptr_null_arguments` (E0004), `extern_signature` (E0162) |
