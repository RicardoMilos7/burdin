# Burdin standard library (stage 0)

The names and signatures a stage 0 program may use. They are a language decision, not an implementation detail: a
change here changes the tests and the compiler together. Issues #28, #29, #30, and #31 settled this surface; the
tests named under each section witness it.

Signatures are written in Burdin. A parameter without `mut` or `own` is read-only and borrowed for the call; `mut`
takes `mut` at the call site; `own` moves and takes no marker (`memory-rules.md`, CV). `T` is a value type that can be
stored (not a second-class view). The one implicit conversion applies throughout: a `String` passed where a `str`
is expected is borrowed as a `str` (IC1).

## Names the standard library keeps private

A standard library helper has a plain name and is `priv`: it is package-private, so a program cannot name it, and
the documentation and completion leave it out. The compiler reaches some helpers by name: a method on a built-in type
maps onto a helper with a plain name (`u32.from_le_bytes` is `u32_from_le_bytes`, `f64.PI` is `f64_PI`, and `{:?}` of
a pair is `debug_tuple2`), and so do the formatters and the eager `List` methods.

A name beginning with `__` is never such a helper. It is one of three things: a compiler intrinsic, the primitive a
`std` wrapper or an `unsafe` block calls and the compiler provides (`__alloc`, `__mutex_lock`, `__io_kind`;
`crates/burdin_sema/src/builtins.rs` lists them); a name the compiler generates for its own locals and functions
(`__f`, `__arg`, `__burdin_test_0`); or a public member that a program's own code reaches through the compiler, which
the documentation leaves out (DC7). The last are the `select` protocol of `std/src/channel.bd` (`__select_new`,
`__Select`, and the methods `__empty`, `__wrap`, `__take`, `__send_outcome`, `__try_recv_slot`, `__try_send_slot`,
spec/channels.md, CH7) and the parallel protocol methods `__par_len` and `__par_get` (spec/parallel.md, PR5). A
member of that kind stays public because a `select` expression or an implementation of a parallel interface in another
package must reach it.

## Compiler-known types (issue #1017)

The guiding rule: the compiler's built-ins stay as small as possible, and everything the language can express is
written in `std/*.bd`. A type or method is compiler-known only when the language cannot express it yet, and the
reason is written down here. `tests/check_docs.py` fails when a `Ty` variant of `crates/burdin_tir/src/ty.rs` has no
row below. Status: **stays** (a primitive, a layout, or a type-system form: it is not a library type), **moves** (the
language can express it; the issue named moves it), or **blocked** (it needs the language feature named).

| Type | Status | Reason, blocker, and issue |
| --- | --- | --- |
| `Ty::Unit`, `Ty::Bool`, `Ty::Char`, `Ty::Int`, `Ty::Float` | stays | Primitives with literals, `as` casts, and built-in operators. Their methods other than intrinsics are `std` extension methods already (`std/src/int.bd`, `float.bd`). |
| `Ty::Str`, `Ty::String` | stays | Literals, the pointer-and-length layout, and the implicit `String` to `str` conversion (IC1). `String` is revisited once `List` has moved (#1077). |
| `Ty::Fn` | stays | Closures are compiler-generated values of the `Fn`/`FnMut`/`FnOnce` interfaces. |
| `Ty::Ref`, `Ty::Slice`, `Ty::Array` | stays | Views and layouts the borrow checker reasons about; only the intrinsics among their methods stay in the compiler. |
| `Ty::Tuple`, `Ty::Never`, `Ty::Param`, `Ty::Projection`, `Ty::TryFromError`, `Ty::Const`, `Ty::Error` | stays | Type-system forms, not library types. |
| `Ty::Struct`, `Ty::Enum` | stays | What `std` declares its own types with. |
| `Ty::Any` | stays | A boxed value behind a vtable (`existentials.md`, AN1). |
| `Ty::CPtr` | stays | The raw pointer every `std` collection is written over. |
| `Ty::Option`, `Ty::Result` | moved | Done in #1017: `enum Option<T>` and `enum Result<T, E = AnyError>` are ordinary enums in `std/src/std.bd`, and their methods are written in Burdin in `std/src/option.bd` (`unwrap` and `expect` call `panic`, so a panic location points into `std`). The compiler finds them by name (`Globals::lang_option`, `lang_result`, and `EnumDef::lang`) for `?`, `orelse`, `context`, the built-ins that return them, and exhaustiveness. `Ty` has no variant for either. |
| `Ty::CString` | moved | Done in #1017: `struct CString { ptr, len }` is an ordinary `std` struct in `std/src/std.bd` over `__alloc`/`__dealloc`, with `from`, `as_ptr`, `clone`, and `Drop` written in Burdin. `Ty` has no variant for it. |
| `Ty::List`, `Ty::ListIntoIter` | moved | Done in #1017: `struct List<T, A: Allocator = Global> { ptr, len, cap, alloc }` and `struct ListIntoIter<T, A>` are ordinary generic `std` structs in `std/src/std.bd` over `CPtr<T>`, `__read`/`__write`, `__slice_raw`, and the allocator interface, with `Drop`, `Eq`, `Debug`, `Clone`, `Hash`, `Default`, and `Iterator<T>` written in Burdin. The compiler finds them by name (`Globals::lang_list`, `lang_list_into_iter`, and `StructDef::lang`) for array and list literals, `collect`, `for` loops, `==`, and the layout; its code generation keeps the faithful `{ptr, len, cap, alloc}` layout so the runtime's helpers still read it. `Ty` has no variant for either; the interpreter runs their methods on its native sequences so `const` evaluation still works. |
| `Ty::Map` | moved | Done in #1017: `struct Map<K, V, A: Allocator = Global> { ptr: CPtr<(K, V, u64)>, len, cap, alloc }` is an ordinary `std` struct in `std/src/std.bd` (lang item `LangItem::Map`, found through `Globals::lang_map`); `Ty` has no variant for it. Its methods (`new`, `new_in`, `with_capacity`, `try_with_capacity`, `insert`, `get`, `get_mut`, `get_or`, `get_or_insert`, `remove`, `remove_entry`, `contains_key`, `retain`, `clear`, `reserve`, `try_reserve`, `shrink_to_fit`, `len`) are written in Burdin over `CPtr`, `__read`/`__write`, the allocator interface, and the `__hash_key` intrinsic (the structural hash glue). The table keeps the layout the runtime's helpers read (slots of `(key, value, hash)`, a zero hash marks an empty slot, FNV-1a with zero moved to one, linear probing, backward-shift deletion, a power-of-two size of at least eight, a 3/4 load factor), so iteration order is unchanged. Like `List`, drop, clone, `==`, hash, `{:?}`, and the three iterators (`iter`, `iter_mut`, `into_iter`) stay compiler glue over that layout; the interpreter runs the methods natively on its `Value::Map`. |
| `Ty::MapEntry` | removed | The entry API (`Map.entry`, `MapEntry`, `or_insert`, `or_insert_with`) is gone by decision: `get_or_insert`, `get_or_insert_with`, `update`, and `upsert` cover it. |
| `Ty::Heap` | stays | Decided in #1017: `*h` is a place with partial-move and drop-flag rules (MV6) that a plain `std` struct cannot express, so `Heap<T>` stays a compiler primitive like references and slices. |
| `Ty::ArrayIntoIter` | moved | Done in #1017: `struct ArrayIntoIter<T, const N: usize> { data: ManuallyDrop<[T; N]>, pos }` is an ordinary `std` struct in `std/src/std.bd` with `Drop` and `Iterator<T>` written in Burdin. `ManuallyDrop<T>` is a lang item (`LangItem::ManuallyDrop`): drop glue never drops its value, so the iterator drops only the elements it did not yield. `array.into_iter()` stays a compiler method (an array cannot be an extension receiver) that returns this struct. |
| `Ty::RangeIter`, `Ty::Range` | moved | Done in #1017: `Range<T>`, `RangeInclusive<T>`, `RangeIter<T>`, and `RangeIterInclusive<T>` are ordinary `std` structs (lang items `Range`, `RangeInclusive`, `RangeIter`, `RangeIterInclusive`). A range literal builds the struct; the iterators step with the `__int_step(x, delta)` intrinsic, so they stop after the last value without overflowing; `for` over a range with a plain binding still lowers to a counted loop. |
| `Ty::MapIntoIter` | moved | `struct MapIntoIter<K, V, A> { ptr, len, cap, pos, alloc }` in `std` (lang item `MapIntoIter`), a plain struct implementing `Iterator<(K, V)>` and `Drop`; `Map.into_iter`, `into_keys`, `into_values`, and `drain` produce it. |
| `Ty::MapIter`, `Ty::MapIterMut`, `Ty::IterMut`, `Ty::SliceChunks`, `Ty::SliceWindows` | stays | They hold a view, and a struct field cannot (SC2, E0120); the language keeps that rule (#1074 was closed as not planned). A stored `CPtr` would let the iterator outlive its collection. |
| `Ty::Chars` | removed | `str.chars()` and `String.chars()` return a `Chars<str>` (`std/src/text.bd`): an ordinary `std` struct generic over its text, `struct Chars<S> { text: S, front: usize, back: usize }`, with `Iterator<char>` and `DoubleEndedIterator<char>` written in Burdin (issue #1083). #1017 first made `chars()` return a `List<char>` copy, because a struct with a `str` field is E0120; a struct generic over its source may hold the view, and its `Chars<str>` instance is second-class (`memory-rules.md` SC3), so the iterator is lazy and allocates nothing. `to_char_list()` gives the list. |

Every type, function, and method the compiler provides is declared once, without a body, in `std/src/builtins.bd`
(`fn List<T>.push(mut self, own x: T)`, `enum Option<T> { Some(T) None }`), each with its `///` documentation. The
compiler never loads that file, and a function or an extension method without a body in a program is E0001. The
language server parses it like a standard-library module, so hover, completion, signature help, and go to definition
read one source, and `crates/burdin_lsp/src/builtins.rs` has no table (issue #1017). Moving a type's representation out of
the compiler (the rows marked "moves") replaces its declaration there with a real one.

The compiler's own tables, measured before the work (the "after" column is filled by #1075-#1077, with the reason
for any row that stays):

| Table | Before |
| --- | --- |
| `crates/burdin_sema/src/builtins.rs` (functions, types, and method signatures) | 1331 lines |
| `crates/burdin_codegen/src/emit/builtins.rs` (code generation of the methods) | 2109 lines |
| `crates/burdin_lsp/src/builtins.rs` (hand-written hover and completion rows) | 352 lines, 169 rows; now 80 lines and no rows (the declarations are `std/src/builtins.bd`, #1017) |
| `Builtin` and `Method` in `crates/burdin_tir/src/ops.rs` | 98 and 98 variants |

What each plan keeps in the compiler is the intrinsics (raw allocation, overflow-checked arithmetic, bit operations,
atomics, context switch) and the layouts above, each declared in `std` as a body-less signature with an intrinsic
marker so that `burdin doc`, `burdin lsp`, and `burdin expand` read one source (#1073).

## Output

```
fn format(fmt: str, ...): String
fn print(fmt: str, ...)
fn println(fmt: str, ...)
fn eprint(fmt: str, ...)
fn eprintln(fmt: str, ...)
```

Each takes a string literal, with or without `{...}` placeholders, followed by the arguments its placeholders use
(`format.md`, FM1 and FM6). The output functions write the text to stdout or stderr; `println`/`eprintln` add one `\n`,
and `format` returns the `String`. A call with one argument takes no format string, so `println(text)`
still writes any `str` value, and a `String` argument is converted by IC1.
Standard output is buffered and standard error is not; the buffer is flushed when it fills, after a line when
stdout is a terminal, and before the process ends through a normal return, `process.exit`, a panic, or the
stack-overflow guard. The `stdout` handle below writes into the same buffer, and its `flush` is the same flush; a
program that does not use the handles sees no difference. The flush points are what keep the order of stdout and
stderr correct before an abnormal end.

## Streams (issue #410)

The three standard streams also have explicit handles. A handle is a zero-sized value, so passing or copying one
costs nothing, and every handle to a stream shares the same runtime buffer. The handles implement the reader and
writer interfaces of "Readers and writers" below, so any code written against `Read`, `Write`, or `BufRead` accepts a
stream, and `BufReader`, `BufWriter`, and `copy` work on them.

```
fn stdout(): Stdout
fn stderr(): Stderr
fn stdin(): Stdin

Stdout: Write { }                    // write(mut self, data: [u8]), flush(mut self), and the Write defaults
Stderr: Write { }
Stdin: Read, BufRead { }             // read(mut self, ...), read_until(mut self, ...), read_line, read_to_string

Stdout { fn is_terminal(self): bool }
Stderr { fn is_terminal(self): bool }
Stdin  { fn is_terminal(self): bool }
```

- `Stdout.write` appends all of `data` to the buffer `print` and `println` use, so the bytes of the handle and of
  `print` stay in the order the program wrote them. `write_text` and `write_line` come from `Write`; on a terminal the
  buffer is flushed after a line, as for `println`. `flush` writes the buffered bytes now, including what `print`
  wrote. The other flush points are the ones in "Output" above.
- `Stderr.write` is unbuffered: the bytes reach the operating system during the call, so they are already out when a
  later panic message is written. `Stderr.flush` does nothing and returns `Result.Ok(())`.
- A write error is ignored, exactly as `print` ignores one, so a stream's `write` always takes all of `data` and
  returns `Result.Ok(data.len())`.
- `Stdin.read` appends up to `max` bytes the runtime has buffered, reading one block from the operating system only
  when the buffer is empty, so it returns as soon as some input is available and never waits for `max` bytes;
  `Result.Ok(0)` is the end of the input. `read_exact`, `read_to_end`, and `read_until` keep reading. `read_line` returns one
  line without its terminator: it drops the `\n`, and the `\r` of a `\r\n` pair. `Result.Ok(Option.None)` is the end of the input,
  and the last line may have no terminator. `read_to_string` returns everything left, as the empty `String` at the
  end. All of them take bytes from the same buffer, so they can be mixed. Unlike the `Read` and `BufRead` defaults,
  `Stdin.read_line` and `Stdin.read_to_string` do not check the bytes for UTF-8, as with files.
- A read error is `IoError.Read` with the path `<stdin>`, the portable kind, and the OS code (`errors.md`,
  ERR10-ERR11).
- Because the handles implement interfaces whose methods take `mut self`, a handle is bound with `mut`
  (`let mut out = stdout()`); a temporary such as `stdout().write_line("x")` needs no binding.
- `is_terminal` asks the operating system whether that stream is a terminal. It is false under the conformance
  runner, for a pipe, and for a file.
- `burdin test` gives each test process a closed standard input (`test-runner.md`, Isolation), so a read sees the
  end of the input and never waits for a person.
- None of these runs at compile time: reaching one in a compile-time evaluation is E0224 (`const-eval.md`, CE6).

## Readers and writers (issue #409)

`Read`, `Write`, `Seek`, and `BufRead` are the interfaces that byte sources and sinks implement; `Cursor`,
`BufReader`, and `BufWriter` are the implementations `std` provides. Every operation returns
`Result<T, IoError>`, so a failure passes out of any depth of wrappers with `?`.

```
interface Read {
    fn read(mut self, mut buf: List<u8>, max: usize): Result<usize, IoError>
    fn read_exact(mut self, mut buf: List<u8>, count: usize): Result<(), IoError>
    fn read_to_end(mut self, mut buf: List<u8>): Result<usize, IoError>
    fn read_to_string(mut self): Result<String, IoError>
}

interface Write {
    fn write(mut self, data: [u8]): Result<usize, IoError>
    fn flush(mut self): Result<(), IoError>
    fn write_all(mut self, data: [u8]): Result<(), IoError>
    fn write_text(mut self, text: str): Result<(), IoError>
    fn write_line(mut self, text: str): Result<(), IoError>
}

enum SeekFrom: Clone, Copy, Eq, Debug { Start(u64), End(i64), Current(i64) }

interface Seek {
    fn seek(mut self, from: SeekFrom): Result<u64, IoError>
    fn position(mut self): Result<u64, IoError>
    fn rewind(mut self): Result<(), IoError>
}

interface BufRead requires Read {
    fn read_until(mut self, delim: u8, mut buf: List<u8>): Result<usize, IoError>
    fn read_line(mut self): Result<Option<String>, IoError>
    fn lines(own self): Lines<Self>                  // Iterator<Result<String, IoError>>
}

fn copy<R: Read, W: Write>(mut from: R, mut to: W): Result<u64, IoError>

struct Cursor {                                      // reads, writes, and seeks in a List<u8>
    fn new(own data: List<u8>): Cursor
    fn empty(): Cursor
    fn from_text(text: str): Cursor
    fn get_ref(self): &List<u8>
    fn into_inner(own self): List<u8>
}

struct BufReader<R: Read> {                          // Read, BufRead, and Seek when R is Seek
    fn new(own inner: R): BufReader<R>               // a 4096-byte buffer
    fn with_capacity(capacity: usize, own inner: R): BufReader<R>
    fn get_ref(self): &R
    fn buffered(self): usize
    fn into_inner(own self): R
}

struct BufWriter<W: Write> {                         // Write, and Seek when W is Seek
    fn new(own inner: W): BufWriter<W>               // a 4096-byte buffer
    fn with_capacity(capacity: usize, own inner: W): BufWriter<W>
    fn get_ref(self): &W
    fn buffered(self): usize
    fn into_inner(own self): Result<W, IoError>
}
```

- **Reading appends.** `read` appends at most `max` bytes to the caller's list and returns how many it appended, so
  no read allocates a buffer of its own. `Result.Ok(0)` is the end of the input (or a `max` of 0). A reader may append fewer
  bytes than asked for. `read_exact` keeps reading until `count` bytes have been appended, and a source that ends
  first is `IoError.UnexpectedEof`; the bytes it did produce stay in the list. `read_to_end` appends everything left
  and returns the count. `read_to_string` collects it as text, and bytes that are not valid UTF-8 are
  `IoError.InvalidData` (the file functions above do not check, as they return what the file holds).
- **Writing takes a prefix.** `write` takes a prefix of `data` and returns its length. `write_all` calls it until every
  byte is taken, and a sink that takes none of a non-empty `data` is `IoError.WriteZero`. `write_text` writes the
  UTF-8 bytes of a string and `write_line` adds one `\n`. `flush` pushes what the writer holds to what it wraps; a
  writer that holds nothing returns `Result.Ok(())`.
- **`BufRead`.** `read_until` appends up to and including the first `delim` byte, or up to the end of the input, and
  returns how many bytes it appended (`0` at the end). `read_line` is the one `Stdin` provides, for any
  buffered reader: it drops the `\n` and the `\r` of a `\r\n`, returns `Result.Ok(Option.None)` at the end, accepts a last line with
  no terminator, and reports bytes that are not valid UTF-8 as `IoError.InvalidData`. `lines` yields each line as a
  `Result`, and stops after the first error.
- **`Cursor`** holds a `List<u8>` and a position. It reads and `read_until`s from the position, writes over the bytes
  at the position (growing the list at the end, and filling the gap with zeros when the position is past the end),
  and seeks. `Seek.position` is `seek(SeekFrom.Current(0))`.
- **`seek`** returns the new position in bytes from the start. A position before the start is `IoError.Seek` and
  leaves the position as it was; a position past the end is allowed. `SeekFrom.End(n)` and `SeekFrom.Current(n)` take
  a signed offset.
- **`BufReader`** asks the wrapped reader for its capacity at a time, so many small reads cost one inner read; a read
  of at least the capacity, with nothing buffered, goes straight to the wrapped reader. `capacity` 0 panics. A seek
  counts the buffered bytes, so `Current` is relative to what the program has read, and then discards the buffer.
  `into_inner` returns the wrapped reader and drops what was buffered.
- **`BufWriter`** holds writes until the next one would overflow the capacity, then writes the buffer to the wrapped
  writer; a write of at least the capacity goes straight through after the buffer is flushed. `capacity` 0 panics.
  `flush` writes the buffer and flushes the wrapped writer. A failed write leaves the bytes the wrapped writer did not
  take in the buffer. **Dropping a `BufWriter` flushes the buffer and ignores a failure**; call `flush` or
  `into_inner` to see one. `into_inner` flushes, returns the writer, and leaves nothing for the drop to write twice.
  A seek flushes first.
- **`BytesMut`** implements `Write`: writing appends to the buffer. The standard-stream handles implement the
  interfaces too (see "Streams"). File handles are issue #702, which will implement `Read`, `Write`, and `Seek`.

The tests are `m24_stdlib/rw_cursor_read`, `rw_cursor_write_seek`, `rw_read_exact_eof`, `rw_read_chunks`, `rw_write_all`,
`rw_bufreader_lines`, `rw_bufreader_buffering`, `rw_bufwriter`, `rw_bufwriter_drop`, `rw_seek_buffered`, `rw_copy`,
`rw_bytesmut_write`, `rw_error_propagation`, `rw_reader_needs_read`, `rw_bufreader_capacity_zero`, and
`rw_bufwriter_capacity_zero`; the stream handles are covered by `io_stdout_handle`, `io_stdout_write_bytes`,
`io_stderr_write_bytes`, `io_stdin_read_line`, `io_stdin_read_to_string`, `io_stdin_read_bytes`, and
`io_stdin_buffered_lines`.

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

Floats have `to_string(self): String`, through `Display` (`format.md`, FM8), and `f64.parse(s)` and `f32.parse(s)`
(below). `bool`, `char`, and `str` have `to_string(self): String`. `bool` also works in `if`/`while` conditions. `parse` is
the associated function `i32.parse(s)`; it accepts an optional leading `-` and digits in the base 10 and returns
`Result.Err(ParseIntError.Invalid)`, whose message is `invalid integer`, for anything else.

Arithmetic is the built-in operators only: `+ - * / %`, `<< >>`, `& | ^`, unary `-`. Overflow panics in debug and
wraps in release for `+ - *` and unary `-`; division by zero and out-of-range shifts always panic. `/` and `%`
truncate toward zero. `as` converts between numeric types (and `char` to an integer, and `u8` or `u32` to `char`), following Rust's cast rules. For the pairs with a built-in `From` impl (FR1 in "Numeric conversions"), `as` is that `From` conversion (FR3). `u32 as char` does not check that the value is a Unicode scalar value (the result for a surrogate or a value past `0x10FFFF` is unspecified); `u32.to_char()` does.

### Integer functions (issue #704)

Beyond the table above, every integer type has the functions below. They are written in Burdin (`std/src/int.bd`,
generated by `tests/gen_int_std.py`), so each one exists once per type; `T` stands for the type, and `U` for its
unsigned counterpart (`T` itself for the unsigned types). The compiler's own methods keep their names.

```
// associated constants and functions (written `T.name`)
const MIN: T                           const MAX: T                           const BITS: u32
fn from_le_bytes(b: [u8; N]): T        fn from_be_bytes(b: [u8; N]): T        fn from_ne_bytes(b: [u8; N]): T

// arithmetic
fn pow(self, exp: u32): T              // overflow panics in debug, wraps in release, like `*`
fn wrapping_pow(self, exp: u32): T     fn checked_pow(self, exp: u32): Option<T>     fn saturating_pow(self, exp: u32): T
fn saturating_add/sub/mul(self, other: T): T
fn overflowing_add/sub/mul(self, other: T): (T, bool)
fn checked_div(self, other: T): Option<T>      fn checked_rem(self, other: T): Option<T>   // None for 0, and for MIN / -1
fn checked_neg(self): Option<T>        fn wrapping_neg(self): T
fn checked_shl/checked_shr(self, n: u32): Option<T>   // None when n >= BITS
fn wrapping_shl/wrapping_shr(self, n: u32): T         // shifts by n % BITS
fn div_euclid(self, other: T): T       fn rem_euclid(self, other: T): T   // the remainder is never negative
fn abs_diff(self, other: T): U         fn unsigned_abs(self): U           // signed types only for unsigned_abs
fn signum(self): T                     fn is_positive(self): bool         fn is_negative(self): bool   // signed only
fn isqrt(self): T                      // negative panics
fn ilog2(self): u32                    fn ilog10(self): u32               // zero or negative panics

// bits
fn count_ones(self): u32               fn count_zeros(self): u32
fn leading_zeros(self): u32            fn trailing_zeros(self): u32
fn rotate_left(self, n: u32): T        fn rotate_right(self, n: u32): T   // n % BITS places
fn swap_bytes(self): T                 fn reverse_bits(self): T
fn is_power_of_two(self): bool         fn next_power_of_two(self): T      fn checked_next_power_of_two(self): Option<T>   // unsigned only

// text in a base
fn to_string_radix(self, radix: u32): String       fn parse_radix(s: str, radix: u32): Result<T, ParseIntError>   // radix 2 to 36

// bytes
fn to_le_bytes(self): [u8; N]          fn to_be_bytes(self): [u8; N]      fn to_ne_bytes(self): [u8; N]
```

`to_string_radix` writes lowercase digits with a leading `-` for a negative number. `parse_radix` takes the same
digits in either case, and for a signed type an optional leading `-`; the empty text, a bare `-`, a `+`, a space, a
digit that is not in the base, and a value outside the type are all `ParseIntError.Invalid`. A radix outside 2 to 36
panics in both. `T.parse(s)` (base 10) rejects a value outside the type the same way, so `i32.parse("4294967297")` is
an error, not `1`.

`N` is the size of the type in bytes (1, 2, 4, or 8). The target is little-endian everywhere Burdin runs, so `ne` is
`le`. The `from_*_bytes` functions are the one case of an associated function that is not `parse`: `T.name(...)` on an
integer type looks `name` up as the std function `T_name`, and `T.NAME` as the std constant `T_NAME`. A name that
does not exist is the usual `E0002`. The floating-point functions are below.

### Floats (issue #417)

```
error enum ParseFloatError { Empty "cannot parse a float from an empty string", Invalid "invalid float" }

fn f64.parse(s: str): Result<f64, ParseFloatError>      // an associated function, like `i32.parse`
fn f32.parse(s: str): Result<f32, ParseFloatError>
```

The grammar is an optional sign, then `digits`, `digits.digits`, `.digits`, or `digits.`, then an optional exponent
(`e` or `E`, an optional sign, digits); or `inf`, `infinity`, `nan` in any letter case after the optional sign. An empty
text is `Empty`; everything else that does not match, including spaces around the number, a second point, a `0x`
prefix, and an exponent without digits, is `Invalid`. A number past the range of the type is infinity and one below it is
zero, as in C. The value is the correctly rounded one (it is computed by the C library's `strtod`/`strtof` after the
grammar check), so a float written with `to_string`, which is the shortest text that reads back as the same number
(`format.md`, FM7), parses back to the same float: `f64.parse(x.to_string()) == x` for every finite `x`.

### Float functions (issue #703)

`f64` and `f32` have the functions below. `T` stands for either type. The compiler provides the ones that lower to
an LLVM intrinsic or to a C math library call, as it does for the integer methods; the rest are extension methods in
`std/src/float.bd` (`extensions.md`), so the prelude makes them visible everywhere.

```
// Exactly rounded: the result is the IEEE 754 result. Compiled code and the constant-evaluation interpreter agree bit for bit.
fn T.abs(self): T                    fn T.sqrt(self): T                 fn T.floor(self): T
fn T.ceil(self): T                   fn T.round(self): T                fn T.round_ties_even(self): T
fn T.trunc(self): T                  fn T.mul_add(self, a: T, b: T): T   fn T.copysign(self, sign: T): T
fn T.to_bits(self): u64              // u32 for `f32`
fn f64.from_bits(bits: u64): f64     fn f32.from_bits(bits: u32): f32    // associated functions, not methods

// The C math library's results, through an LLVM intrinsic where one exists.
fn T.powi(self, exp: i32): T         fn T.powf(self, exp: T): T          fn T.cbrt(self): T
fn T.exp(self): T                    fn T.exp2(self): T                 fn T.ln(self): T
fn T.log2(self): T                   fn T.log10(self): T                fn T.sin(self): T
fn T.cos(self): T                    fn T.tan(self): T                  fn T.asin(self): T
fn T.acos(self): T                   fn T.atan(self): T                 fn T.atan2(self, other: T): T
fn T.sinh(self): T                   fn T.cosh(self): T                 fn T.tanh(self): T
fn T.hypot(self, other: T): T

// Written in `std`.
fn T.is_nan(self): bool              fn T.is_infinite(self): bool       fn T.is_finite(self): bool
fn T.is_sign_negative(self): bool    fn T.signum(self): T               fn T.fract(self): T
fn T.log(self, base: T): T           fn T.min(self, other: T): T        fn T.max(self, other: T): T
fn T.clamp(self, min: T, max: T): T  fn T.to_degrees(self): T           fn T.to_radians(self): T
fn T.total_cmp(self, other: T): Ordering     // the float order; floats have no `cmp`

// The associated constants, each in its type's precision.
f64.PI  f64.E  f64.TAU  f64.SQRT_2  f64.LN_2  f64.LN_10  f64.INFINITY  f64.NEG_INFINITY  f64.NAN
f64.EPSILON  f64.MIN  f64.MAX  f64.MIN_POSITIVE
f32.PI  f32.E  f32.TAU  f32.SQRT_2  f32.LN_2  f32.LN_10  f32.INFINITY  f32.NEG_INFINITY  f32.NAN
f32.EPSILON  f32.MIN  f32.MAX  f32.MIN_POSITIVE
```

The exact semantics:

- `abs` clears the sign bit: `abs(-0.0)` is `0.0`, and a NaN keeps its payload but loses its sign. A NaN payload is
  otherwise unspecified, including the one `NAN` has.
- `round` rounds a half away from zero (`2.5` is `3.0`, `-2.5` is `-3.0`); `round_ties_even` rounds a half to the
  even neighbour (`2.5` is `2.0`, `3.5` is `4.0`); `trunc` rounds toward zero; `floor` and `ceil` round down and
  up. `fract` is `self - self.trunc()`, so it is NaN for an infinite or NaN value.
- `signum` is `1.0`, `-1.0`, or NaN: it follows the sign bit, so `(-0.0).signum()` is `-1.0` and a NaN gives NaN.
  `is_sign_negative` likewise reads the sign bit, so it is true for `-0.0` and for a NaN written with that bit.
- `min` and `max` return the other value when one of them is NaN, and the receiver when the two compare equal. The
  rule for equal values makes the signed zeros deterministic: `0.0.min(-0.0)` is `0.0` and `-0.0.min(0.0)` is
  `-0.0`, and the same for `max`. Both values are bit-identical in compiled code and at compile time.
- `clamp` panics with `clamp: min is NaN, max is NaN, or min is greater than max` when a bound is NaN or `min` is
  greater than `max`; it is the value limited to `min..=max`, and a NaN receiver gives NaN.
- `is_nan` is `self != self`, `is_infinite` is equality with either infinity, and `is_finite` is `abs(self) <
  INFINITY`.
- `log(base)` is `self.ln() / base.ln()`; `to_degrees` is `self * (180.0 / PI)` and `to_radians` is `self * (PI /
  180.0)`, with the constant in the receiver's precision.
- `total_cmp` is the IEEE 754 totalOrder as an `Ordering`: `-NaN < -inf < ... < -0.0 < 0.0 < ... < inf < NaN`, and
  NaNs are ordered by their bit patterns. It is the comparator `sort_by` and `PriorityHeap` use for floats; floats
  have no `cmp` (`E0002`). `Ordering` is described below.
- `to_bits` and `from_bits` are a bit cast: every bit of the value is kept, including a NaN payload and the sign of
  `-0.0`. The integer is `u64` for `f64` and `u32` for `f32`.
- The constants are each type's value, so `f32.PI` is `3.1415927`. `MIN` and `MAX` are the finite extremes,
  `MIN_POSITIVE` is the smallest positive normal value, and `EPSILON` is the gap between `1.0` and the next value.
  `INFINITY`, `NEG_INFINITY`, and `NAN` are written as divisions (`1.0 / 0.0`, `-1.0 / 0.0`, `0.0 / 0.0`) because a
  float literal must be finite; `NAN`'s payload and sign are unspecified.
- Compiled code and the constant-evaluation interpreter agree bit for bit on the exactly rounded operations. The transcendental functions are the
  C library's, through an LLVM intrinsic where LLVM has one (`tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`,
  `cosh`, and `tanh` do in LLVM 21; `cbrt` and `hypot` are direct library calls), so their last bit is the
  library's and is not promised across platforms.

`f64.NAME` is the std constant `f64_NAME` and `f32.NAME` is `f32_NAME`, as for the integer constants. An unknown
method is `E0002`, an unknown constant `E0002`, a wrong argument count `E0004`, and a wrong argument type `E0003`;
`f64.pow` and `f64.cmp` are `E0002`, and the integer functions do not apply to floats.

Tests: `m24_stdlib/float_math`, `float_constants`, `float_predicates`, `float_min_max`, `float_bits`,
`float_clamp_abort`, `float_clamp_nan_abort`, `float_unknown_names`.

## Conversions (issue #274)

`TryFrom<Source>` is the fallible counterpart of `From`, and a built-in interface like it. A block is written on a struct
or enum the package declares:

```
Percent: TryFrom<i32> {
    fn try_from(v: i32): Result<Percent, PercentError> { ... }
}
```

The `try_from` method returns `Result<Type, E>`, where `E` is whatever the block chooses. Every integer type has the
same function for every integer source, written `T.try_from(v)` and returning `Result<T, TryFromIntError>`.

- **TF1.** `Type.try_from(v)` and `Type.from(v)` call the block of `Type` whose source type is the type of `v`. A type
  converts from each source type once: a second block for the same source is E0211. A `from` or `try_from` member
  that is not a block's method shares the name and is E0007. `?` picks a `From` block by its error type (`errors.md`,
  ERR3). Two `From` blocks with different sources are allowed, and so are a `From` and a `TryFrom` for one source,
  since they are different methods.
- **TF2.** A `TryFrom` block gives the method `try_from` (E0140 otherwise), and a `From` block gives `from`. The block
  is for a type the package declares (E0142 otherwise), and its method is not generic (E0003). The method takes one
  parameter of the source type: another count is E0004, and another type is E0003. Its result is the target type for
  `from`, and `Result<target, E>` for `try_from`; another result is E0003. The signature is checked where the block is
  declared, so the callers can rely on it.
- **TF3.** A `Type.try_from(v)` with no block for the type of `v` is E0003 at `v`, and a type with no `TryFrom` block
  has no `try_from` function (E0002). A `String` is borrowed as a `str` where the source is `str` (IC1), as for any
  argument.
- **TF4.** For an integer type `T` and an integer `v` of any type, `T.try_from(v)` is `Result.Ok(v as T)` when `v` is in the
  range of `T`, and `Result.Err(TryFromIntError.OutOfRange)` when it is not. A value of another type, such as a float, is
  E0003, and a call with other than one argument is E0004. `v` is evaluated once. Its source may be any integer, so
  a literal argument is not given `T`: it stays an `i32`, and `u8.try_from(300)` is `Result.Err(OutOfRange)`, not E0008.
- **TF5.** `TryFromIntError` is an error enum in `std` with one variant, `OutOfRange`, whose message is `out of range
  integral type conversion attempted`. It is `Eq` and `Debug`, and it is an ordinary error value (ERR1-ERR2).
- **TF6.** The `Result` of `try_from` is an ordinary `Result`, so `?` converts its error through `From` like any other
  (ERR3). A function whose error type has no `From` block from `TryFromIntError` or from the block's error type is
  E0212 at the `?`.
- **TF7.** `TryFrom<S>` is a bound, as `From<S>` is (FR7). `T: TryFrom<S>` is met by an integer `T` and any integer
  `S` (TF4), or by a type `T` with a `TryFrom` block whose source is `S` (TF1). It takes exactly one type argument, and
  any other count is E0003. A bare `TryFrom` with no argument is E0003. In the body of a function with the bound,
  `T.try_from(v)` with `v` of type `S` is a conversion with the result `Result<T, <T as TryFrom<S>>.Error>`, and `v`
  of another type is E0003 at `v`. `T.try_from` with no such bound is E0141. A call is checked against the bound, with
  E0141 when it is not met.
- **TF9.** `<T as TryFrom<S>>.Error` is the error type of the conversion from `S` into `T`. A type parameter `T` must
  have the bound `TryFrom<S>` (E0141 otherwise), and the projection stays symbolic until `T` is concrete. A concrete
  type resolves at once: `TryFromIntError` for an integer `T` (TF5), otherwise the `E` of the block's `Result<T, E>`,
  and a concrete type with no conversion from `S` is E0141. A type that still mentions a parameter stays symbolic.
  `Error` is the only name the conversion has, and any other name is E0145. The shorthand `T.Error` is not a projection
  of this kind: no interface among the bounds declares it, so it is E0145. Its display is `<T as TryFrom<S>>.Error`.
- **TF10.** `TryFrom` cannot be used as `any` (E0200), and it cannot be listed after a type's name, since its method
  has no default (E0234). `Drop` as a bound is still E9000 (`e9000-audit.md`).

- **TF8.** Not done: a `From` or `TryFrom` block on a generic type (`Holder<i32>: TryFrom<i64>`) is accepted as an
  ordinary impl, so its method is a member of each instance. `?` does not use it, and its signature and source are
  not checked against TF2. The same holds for `From` today.

Conversions from `char`, and a `try_from` for other primitive types, are not part of this section.

Tests: `m24_stdlib/try_from_bound_run`, `try_from_bound_arity`, `try_from_bound_body_mismatch`, `try_from_bound_call`,
`try_from_bound_no_bound`, `try_from_bound_projection`, `try_from_bound_wrong_source`; `m24_stdlib/try_from_int`, `try_from_user`, `try_from_from_sources`, `try_from_str`, `try_from_error`,
`try_from_expect_aborts`, `try_from_bad_return`, `try_from_missing_method`, `try_from_orphan`, `try_from_bad_source`,
`try_from_arity`, `try_from_duplicate`, `try_from_member_clash`, `try_from_int_float`, `try_from_int_arity`,
`try_from_no_source`, `try_from_no_impl`, `try_from_derive`, `try_from_any`, `try_from_question_missing`.

## Numeric conversions: `From`, `into`, and `as`

`From<Source>` is the infallible conversion. A block written on a type the package declares (TF1 and TF2) works as
before. The scalar pairs below also have a built-in `From` impl, with no block to write, and `into` and `as` use the
same rules.

- **FR1.** A pair of scalar types `(S, T)` has a built-in `From` impl when every value of `S` converts to `T` exactly,
  so `as` from `S` to `T` gives back the same value. The pairs are:
  - an integer type `S` to an integer type `T` whose range contains the range of `S`. `isize` and `usize` are 64 bits
    (`POINTER_BITS`), so `u32` converts to `usize`;
  - an integer type `S` to a float type `T` when every value of `S` is exact in `T`: `i8`, `i16`, `u8`, and `u16` to
    `f32`, and those with `i32` and `u32` as well to `f64`;
  - `f32` to `f64`;
  - `bool` to every integer type;
  - `char` to an integer type whose range contains every scalar value (`0` through `0x10FFFF`): `u32`, `i32`, `u64`,
    `i64`, `usize`, and `isize`;
  - `u8` to `char`;
  - every integer, float, `bool`, and `char` type to itself.
  No other pair has a built-in impl. `i64` to `f64`, `f64` to `f32`, and `u32` to `char` do not: the first rounds, the
  second rounds, and the third is not a scalar value for every `u32`.
- **FR2.** `T.from(v)` for a pair of FR1 is that conversion. An unsuffixed integer literal `v` (or a minus sign
  before one) takes the type `T` when `T` is an integer type, as a literal takes its expected type everywhere else:
  `u8.from(5)`, `i64.from(5)`, and `usize.from(5)` are valid. A literal out of `T`'s range is E0008 (`u8.from(300)`),
  and a negative literal for an unsigned `T` is E0003 (`u8.from(-5)`). Any other argument is checked without an
  expected type, so a variable is its own type: `i32.from(big)` with `big: i64` is E0003 at `big`, and a literal
  for a target that is not an integer type is an `i32`, so `char.from(65)` is E0003. `f64.from(5)` is the
  `i32` pair, as it was. `T.from(v)` for any other pair is E0003 at `v`. A built-in impl cannot be written by the
  program, since a `From` block on a built-in type is E0142 (`spec/stdlib.md`, "Conversions", TF2).
- **FR3.** `as` from `S` to `T` for a pair of FR1 is the same conversion as `T.from(v)`: both use one definition, so
  they cannot disagree. Every other `as` keeps its behavior: `i64 as i32` truncates, `f64 as f32` rounds, and
  `i64 as f64` rounds to nearest. None of these pairs has a `From` impl.
- **FR4.** `v.into()` converts `v` to the type the context expects: a typed `let`, an argument, a return value, a
  field, or an element. It is `T.from(v)` for that type `T`, through the built-in pairs of FR1 or the `From` blocks of
  `T`. A receiver whose type has a method named `into` uses that method, as any method of the receiver does.
- **FR5.** `v.into()` with no expected type is E0003 at the call, and a target with no conversion from the type of `v`
  is E0003 at the call too. `into` takes its receiver as `from` takes its argument: a receiver that is not `Copy` is
  moved, so a borrowed value of such a type cannot be converted. The literal rule of FR2 does not reach a literal
  receiver, which is checked before the target is known: `let b: u8 = 5.into()` is E0003 (write `u8.from(5)`).
- **FR6.** Not done: `From` blocks on generic types (TF8). A type parameter as the target of `into` or `from` needs
  a `From` bound (FR7).
- **FR7.** `From<S>` is a bound: `T: From<S>` is met by a type `T` into which a value of `S` converts, through a
  built-in pair (FR1) or a `From` block of `T` whose source is `S`. A bound takes exactly one type argument, and any
  other count is TYPE_MISMATCH (E0003). In the body of a function with the bound, `T.from(v)` with `v` of type `S`
  returns `T`, and `v.into()` with the expected type `T` is the same conversion; `v` of another type is E0003 at `v`,
  and `T.from` or `into` with no such bound is E0141 or E0003. A call of such a function is checked against the bound,
  with E0141 when it is not met.
- **FR8.** The bound's type argument may be a type parameter (`fn f<T: From<U>, U>(...)`); it is substituted when the
  function is instantiated, as any bound argument is.

Tests: `m24_stdlib/from_numeric`, `from_as_lossy`, `into_numeric`, `from_lossy`, `from_literal`, `from_literal_range`,
`from_arity`, `into_no_target`,
`into_no_impl`, `into_method_wins`, `from_bound_run`, `from_bound_call`, `from_bound_wrong_source`, `from_bound_no_bound`,
`from_bound_arity`, `from_bound_into_no_bound`.

## Characters (issue #416)

`std.unicode` declares extension methods on `char` and `u32` (`extensions.md`), so the prelude makes them visible
everywhere.

```
fn char.is_ascii(self): bool
fn char.is_ascii_alphabetic(self): bool       // also _uppercase, _lowercase, _digit, _hexdigit, _alphanumeric,
                                              // _whitespace, _punctuation, _control
fn char.to_ascii_lowercase(self): char
fn char.to_ascii_uppercase(self): char
fn char.eq_ignore_ascii_case(self, other: char): bool

fn char.is_alphabetic(self): bool
fn char.is_numeric(self): bool                // decimal digits of any covered script, and ², ³, ¹, ¼, ½, ¾
fn char.is_alphanumeric(self): bool
fn char.is_whitespace(self): bool             // Unicode White_Space
fn char.is_control(self): bool                // 0-31 and 127-159
fn char.is_uppercase(self): bool
fn char.is_lowercase(self): bool
fn char.to_lowercase(self): char              // one-to-one mapping
fn char.to_uppercase(self): char

fn char.is_digit(self, radix: u32): bool
fn char.to_digit(self, radix: u32): Option<u32>   // radix 2..=36, else panics
fn char.len_utf8(self): usize
fn u32.to_char(self): Option<char>            // None for a surrogate or a value past 0x10FFFF
fn u32.digit_char(self, radix: u32): Option<char>  // the digit or lowercase letter for a value; radix 2..=36
```

The ASCII functions are exact. The Unicode ones work from code point ranges that cover the Latin, Greek, Cyrillic,
Armenian, Hebrew, Arabic, Devanagari, Thai, Hiragana, Katakana, Hangul, CJK, and fullwidth Latin blocks; a character of
another script is not alphabetic and has no case. Case conversion maps one character to one character, so `ß` has no
uppercase form here and `İ`/`ı` are unchanged; Latin-1, Latin Extended-A, Greek, and Cyrillic letters convert.

Tests: `m24_stdlib/char_ascii`, `char_control`, `char_unicode_classes`, `char_case`, `char_digits_and_code_points`,
`char_bad_radix`.

## Operators

`std` declares the operator interfaces in `std.ops`; the prelude makes the names visible in every module, so a user
type takes part in an operator by implementing one of them (`operators.md`, OP1-OP2):

```
interface Add      { fn add(self, other: Self): Self }
interface Sub      { fn sub(self, other: Self): Self }
interface Mul      { fn mul(self, other: Self): Self }
interface Div      { fn div(self, other: Self): Self }
interface Rem      { fn rem(self, other: Self): Self }
interface BitAnd   { fn bitand(self, other: Self): Self }
interface BitOr    { fn bitor(self, other: Self): Self }
interface BitXor   { fn bitxor(self, other: Self): Self }
interface Neg      { fn neg(self): Self }
interface Not      { fn not(self): Self }

interface Eq {
    fn eq(self, other: Self): bool
    fn ne(self, other: Self): bool      // default: !self.eq(other)
}

interface Ord {
    fn lt(self, other: Self): bool
    fn le(self, other: Self): bool      // default: !self.gt(other)
    fn gt(self, other: Self): bool      // default: other.lt(self)
    fn ge(self, other: Self): bool      // default: !self.lt(other)
}

interface Index<T>    { fn index(self, i: usize): &T }
interface IndexMut<T> { fn index_mut(mut self, i: usize): &mut T }
```

The built-in operators and the `Eq`/`Ord` derives always win over an implementation, both operands have the same
type (except a `String` with a `str`, OP14), and `&&`, `||`, `=`, and the shifts are not overloadable. A compound assignment (`a += b`) on a user type is `a = a + b`
through `Add` and the other operator interfaces (`spec/compound-assignment.md`, CA5). `Index` names the element type
and returns a reference; `IndexMut` is selected when the indexed place is written. The exact resolution, the
diagnostics, and the ownership rules are in [`operators.md`](operators.md).

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
    fn clone(self): String            // deep copy of the buffer
}
```

`str` has the same read methods (`len`, `starts_with`, `find`, `find_str`, `as_str`, `chars`, `to_string`), without
`push*`, `from`, or `clone`. `chars()` is an extension method (`text.bd`) on both: `fn str.chars(self): Chars<str>` (and
`fn String.chars(self): Chars<str>`) returns a lazy iterator over the Unicode scalar values (issue #1083). `Chars<S>` is an
ordinary `std` struct, `struct Chars<S>: Clone { priv text: S, priv front: usize, priv back: usize }`, with
`Iterator<char>` and `DoubleEndedIterator<char>` for `Chars<str>`: `next` decodes the UTF-8 at the front, `next_back` steps
back over continuation bytes and decodes the last character, and nothing is allocated, so `for c in s.chars()`,
`s.chars().rev()`, `.filter(...)`, and `.count()` cost no list; `.to_list()` or `s.to_char_list()` builds one. A `Chars<str>`
holds a view of the text, so it is second-class (`memory-rules.md` SC1-SC3): a struct field or enum payload of that type is
E0120, returning one that reads a local `String` is E0122, and the text stays borrowed until the iterator's last use (E0111).

Slicing `s[a..b]` and `s[a..=b]` yields a `str` view of the bytes in the range; it works on `str`, `String`, arrays,
and slices. `a` and `b` are `usize`; a reversed or out-of-range range panics, and so does a slice of `str` or `String`
either of whose ends is not a character boundary (see "Slicing and character boundaries" below). `len` counts bytes.

`==` and `!=` compare two `str` values or two `String` values byte for byte. `<`, `<=`, `>`, `>=` compare two `str`
values or two `String` values lexicographically by bytes. A `String` and a `str` compare with each other in all six
operators, in either order (`s == "x"`, `"x" < s`), by the same bytes rule (`operators.md` OP14). Any other mixed pair is
E0003.

### Bytes, UTF-8, and checked slicing (issue #713)

```
fn as_bytes(self): [u8]                 // a view of the UTF-8 bytes; built in, no copy
fn bytes(self): [u8]                    // the iterator spelling of as_bytes
fn String.into_bytes(own self): List<u8> // the bytes as an owned list; the string is consumed

error enum Utf8Error { Invalid { index: usize } "invalid UTF-8 at byte {index}" }
fn String.from_utf8(own bytes: List<u8>): Result<String, Utf8Error>   // associated function
fn str.from_utf8(bytes: [u8]): Result<str, Utf8Error>                  // associated function; a view
fn String.from_utf8_lossy(bytes: [u8]): String                         // associated function

fn contains_str(self, needle: str): bool           // the same as contains
fn rfind(self, c: char): Option<usize>              // byte offset of the last occurrence
fn rfind_str(self, needle: str): Option<usize>      // "" matches at the end
fn match_indices(self, needle: str): MatchIndices<str> // lazy iterator of `(usize, str)`; "" panics
fn find_all(self, needle: str): MatchIndices<str>      // the same as match_indices
fn get(self, from: usize, to: usize): Option<str>      // the checked, boundary-aware slice
```

`as_bytes` is a `[u8]` view of the string's storage, so it copies nothing and keeps the string borrowed; `bytes`
is the iterator spelling (`for b in s.bytes()`), and `into_bytes` copies the bytes into an owned list. An offset
these functions return is a byte offset.

`String.from_utf8` accepts exactly the well-formed UTF-8 sequences and reports the first byte that is not part of
one, as `Utf8Error.Invalid { index }`: a bad lead or continuation byte, an overlong form, a surrogate, a value past
U+10FFFF, or a sequence the end cuts off. `str.from_utf8` validates the same way and returns a `str` view of the
caller's bytes without copying; the view keeps the bytes borrowed. `String.from_utf8_lossy` keeps every well-formed
sequence and replaces each byte that is not part of one with U+FFFD REPLACEMENT CHARACTER, so it never fails.

`match_indices` reports non-overlapping matches from the left; `find_all` is its other name. `get(from, to)` is the
checked slice: it returns `Option.None` when the range is reversed, past the end, or either end splits a character, and a
view otherwise.

The unchecked reinterpretation behind `str.from_utf8` is the primitive `__view_as_text(view: [u8]): str`, and the
unchecked buffer adoption behind `String.from_utf8` is `__bytes_into_string(own bytes: List<u8>): String`; both
require an `unsafe` block (E0161). `std` calls them only after validating the bytes, so an ordinary program cannot
make a `str` or `String` that is not UTF-8. `str.from_utf8` and `String.from_utf8` are associated functions resolved
by name; `as_bytes` is a compiler-provided method.

**Slicing and character boundaries (issue #916).** On `str` and `String`, `s[a..b]` and `s[a..=b]` panic unless both
ends are character boundaries: the start of the text, the end of the text, or a byte that does not continue a
multi-byte character. A slice therefore never breaks a character, so every `str` stays valid UTF-8, which the
character functions (`chars`, `trim`, the case conversions) and `__view_as_text` rely on. The panic reads
`slice: the index is not a character boundary`. A reversed or out-of-range range panics as before. Slicing an
array or a slice of `[u8]` checks only the range, since its elements are not characters.

`get(from, to)` is the same slice without the panic: it returns `Option.None` when the range is reversed, out of range, or
an end is not a character boundary. `as_bytes()` is the byte view for byte-level work, and `is_char_boundary` tests a
single offset.

Tests: `m24_stdlib/string_bytes`, `string_utf8_validation`, `string_from_utf8_lossy`, `string_utf8_file`,
`string_find_all`, `string_checked_slice`, and the compile-fail cases `string_bytes_second_class` (E0120) and
`string_view_as_text_unsafe` (E0161).
### String capacity and editing (issue #714)

```
fn String.with_capacity(capacity: usize): String   // associated function: an empty string with room for `capacity` bytes
fn capacity(self): usize                           // the buffer's capacity in bytes
fn reserve(mut self, additional: usize)            // room for `additional` more bytes; keeps the contents
fn shrink_to_fit(mut self)                         // shrinks the buffer to the length
fn clear(mut self)                                 // empties the string; keeps the buffer
fn truncate(mut self, byte_len: usize)
fn pop(mut self): Option<char>
fn insert(mut self, byte_idx: usize, c: char)
fn insert_str(mut self, byte_idx: usize, s: str)          // built in
fn remove(mut self, byte_idx: usize): char
fn retain<F: FnMut(char): bool>(mut self, f: F)
fn drain(mut self, from: usize, to: usize): String         // the removed bytes, as a new string
fn replace_range(mut self, from: usize, to: usize, s: str) // built in
fn split_off(mut self, byte_idx: usize): String            // the text from `byte_idx` on
fn repeat(self, count: usize): String                      // the text `count` times (see "Text helpers")
```

The buffer operations keep the `String` layout: `{ ptr, len, cap }`. `with_capacity(n)` allocates `n` bytes and
leaves the length at 0; `reserve(additional)` grows the buffer when the length plus `additional` does not fit, using
the same growth as `push_str` (the larger of `capacity + capacity / 2 + 8` and the needed size); `shrink_to_fit`
reallocates to the length (and frees the buffer when the string is empty). `capacity` may be larger than `len`, and
its exact value is an implementation detail.

Every byte index is a character boundary of the string. `insert`, `insert_str`, `remove`, `truncate`, `drain`,
`replace_range`, and `split_off` panic with a message naming the operation when an index is out of range or not on a
character boundary, so a `String` stays valid UTF-8. `insert` and `insert_str` shift the tail right by the inserted
text's length; `remove`, `truncate`, `drain`, and `replace_range` move it left. `pop` returns the last character and
removes its bytes; `remove` returns the character that starts at its index. `retain` keeps the characters its
predicate accepts, in order. `drain(from, to)` removes the bytes and returns them as a new string; `split_off(i)`
leaves the prefix in the receiver and returns the suffix. `replace_range` takes the two byte offsets rather than a
range value because a `Range<usize>` value does not expose its endpoints to library code.

`String` implements `Default` (the empty string), `Ord` (byte order through `<`), `Clone`, `Display`, and `Debug`;
`Hash` for `String` is part of the interface-coverage work (issue #716).

**`String + str` is built in; other `+` on text is not provided.** `String + str` returns a new `String` holding both
texts and borrows both operands. The comparisons `==`, `!=`, `<`, `<=`, `>`, `>=` accept a `String` on either side of a
`str`. Every other pair stays same-type: `str + str` and `String + String` are E0003 (write `s + t.as_str()`), and
`s += "x"` is E0003. `push_str`, `format`, the interpolated string literal, and `join` are the other ways to build text.
The rule is `operators.md` OP14.

Tests: `m24_stdlib/string_capacity`, `string_editing`, `string_edit_const`, and the abort cases
`string_edit_boundary_aborts` (a non-boundary insert), `string_edit_bounds_aborts` (a reversed range), and
`string_truncate_bounds_aborts` (a length past the end); `string_plus_unsupported` (E0003) pins `"a" + "b"`, and the
mixed forms are in `m19_operators/string_str_operators` and `string_str_mixed_compile_fail`.

### Characters of a text (issue #193)

The scope of Unicode support is the scalar value. These extension methods (also on `String`) count, index, and rebuild
text by `char`, where `len` and slicing count bytes. The library does not normalize and does not group characters
into grapheme clusters, so an `e` followed by a combining accent is two characters. `chars()` stays the basic
UTF-8-correct iterator. Case mapping and classification are in "Characters" above and "Text helpers" below.

```
fn str.char_count(self): usize
fn str.is_ascii(self): bool
fn str.is_char_boundary(self, i: usize): bool         // the start of a character, or the end of the text
fn str.nth_char(self, n: usize): Option<char>
fn str.to_char_list(self): List<char>
fn str.char_indices(self): CharIndices<str>           // lazy double-ended iterator of `(usize, char)`
fn str.reverse_chars(self): String
fn str.slice_chars(self, start: usize, end: usize): String   // by character index; an index past the end stops there
fn List<char>.collect_string(self): String
```

Tests: `m24_stdlib/text_characters` (a text with one-, two-, and three-byte characters), and the non-ASCII cases of
`string_*` and `char_*`.

### Text helpers (issues #415, #1019)

`std.text` (and the case conversion in `std.unicode`) declare extension methods on `str` (`extensions.md`), so the
prelude makes them visible everywhere and the compiler's own methods (`len`, `find`, `starts_with`, `chars`, ...) win.
Read-only `str` extensions also apply directly to `String` receivers through the `String`-to-`str` borrow fallback
(EX3), so `std.text` needs no duplicate `String` wrappers. Offsets are byte offsets. A match of a valid separator in
valid text always starts and ends on a character boundary, and `trim` and the case conversions walk whole characters,
so the functions keep UTF-8 intact.

The split, lines, match, and character-index helpers return lazy iterators (`Split<str>`, `RSplit<str>`,
`SplitChar<str>`, `RSplitChar<str>`, `SplitWhitespace<str>`, `Lines<str>`, `MatchIndices<str>`, `CharIndices<str>`)
that yield `str` views (or `(usize, str)` / `(usize, char)`) without allocating per piece. Each iterator struct is
generic over its source (`Split<S>`, etc.) and second-class when instantiated with `str` (`memory-rules.md`, SC2/SC3),
keeping its source text (and separator) borrowed until its last use (RF5, RT3a). `SplitChar`, `RSplitChar`,
`SplitWhitespace`, `Lines`, and `CharIndices` also implement `DoubleEndedIterator`. Use `.collect<List<String>>()` (or
`let parts: List<String> = s.split(",").collect()`) to materialize owned `String` pieces; `List<str>` remains
forbidden by SC2.

Substring search (`contains`, `split`, `rsplit`, `replace`, `rfind_str`, `match_indices`, `find_all`) uses a linear
byte scan with O(1) extra space and O(n * m) worst-case time (O(n) for single-byte patterns); `rsplit` and
`rsplit_char` scan backward from the end without building an intermediate list. `trim*` return a `str` view of the
receiver (`trim_end` scans backward). `replace`, `repeat`, `reverse_chars`, `slice_chars`, `List<char>.collect_string`,
`List<String>.join`, `String.retain`, and `String.into_bytes` reserve their output capacity up front.

```
fn str.is_empty(self): bool
fn str.contains(self, needle: str): bool                // "" is contained everywhere
fn str.ends_with(self, suffix: str): bool
fn str.trim(self): str                                  // also trim_start, trim_end; Unicode white space
fn str.split(self, sep: str): Split<str>                // keeps empty pieces; an empty `sep` panics
fn str.split_char(self, c: char): SplitChar<str>        // double-ended
fn str.rsplit(self, sep: str): RSplit<str>              // backward scan, last piece first; empty `sep` panics
fn str.rsplit_char(self, c: char): RSplitChar<str>      // double-ended, backward scan
fn str.split_whitespace(self): SplitWhitespace<str>     // double-ended; runs of non-white-space characters
fn str.lines(self): Lines<str>                          // double-ended; at `\n`, stripping preceding `\r`
fn str.replace(self, from: str, to: str): String        // an empty `from` panics
fn str.strip_prefix(self, prefix: str): Option<str>     // a view of the receiver
fn str.strip_suffix(self, suffix: str): Option<str>
fn str.repeat(self, count: usize): String
fn str.to_lowercase(self): String                       // char.to_lowercase on each character
fn str.to_uppercase(self): String
fn str.to_ascii_lowercase(self): String
fn str.to_ascii_uppercase(self): String
fn str.eq_ignore_ascii_case(self, other: str): bool
fn List<String>.join(self, sep: str): String
```

When two standard modules extend different receivers under one name (`Option.replace`, `str.replace`), the prelude
keeps all of them; a module's own declaration or `use` of that name still hides the prelude's.

Tests: `m24_stdlib/string_split`, `string_trim`, `string_lines_and_words`, `string_replace_join`,
`string_search_and_affixes`, `string_case`, `string_empty_separator`, `string_empty_pattern`,
`string_split_alloc`, `string_split_borrow` (E0111), `string_split_escape` (E0122), `string_split_field` (E0120).

## Encodings (issue #414)

```
error enum DecodeError { InvalidLength, InvalidCharacter { index: usize }, InvalidUtf8 { index: usize } }

fn str.to_utf8(self): List<u8>             fn String.to_utf8(self): List<u8>
fn from_utf8(data: [u8]): Result<String, DecodeError>

fn hex_encode(data: [u8]): String          fn hex_encode_upper(data: [u8]): String
fn hex_decode(text: str): Result<List<u8>, DecodeError>

fn base64_encode(data: [u8]): String       fn base64_url_encode(data: [u8]): String
fn base64_decode(text: str): Result<List<u8>, DecodeError>
fn base64_url_decode(text: str): Result<List<u8>, DecodeError>

fn url_encode(text: str): String
fn url_decode(text: str): Result<String, DecodeError>

fn List<u8>.to_hex(self): String           fn List<u8>.to_base64(self): String
fn List<u8>.to_base64_url(self): String    fn List<u8>.to_text(self): Result<String, DecodeError>
```

Encoders take bytes as a `[u8]` view (`bytes[0..bytes.len()]`; the `List<u8>` methods do that for the whole list) and
return a `String`. `hex_encode` writes lowercase digits, two per byte; `hex_decode` accepts either case. `base64_encode`
is RFC 4648 section 4 with `=` padding to a multiple of four characters; `base64_url_encode` is section 5 (`-` and
`_`) **without** padding. `base64_decode` needs the padding and the standard alphabet; `base64_url_decode` takes the
URL-safe alphabet with or without padding. Neither checks that the unused bits of the last character are zero.
`url_encode` writes every byte of the UTF-8 form of the text as `%XX` (uppercase) except `A-Z a-z 0-9 - . _ ~`;
`url_decode` turns `%XX` (either case) back into bytes and leaves every other character, `+` included, as it is.
`from_utf8` accepts exactly the well-formed UTF-8 byte sequences.

A decoder reports the first problem: `InvalidLength` for an odd number of hex digits, a base64 text that cannot be a
whole number of bytes or whose padding is wrong, or a `%` cut off by the end of the text; `InvalidCharacter { index }`
for a character that does not belong (`index` is its byte offset in the text); `InvalidUtf8 { index }` for the byte
offset where the bytes stop being UTF-8.

## Byte buffers (issue #413)

```
struct BytesMut {                         // a growable, owned buffer
    fn new(): BytesMut                    fn from_list(data: List<u8>): BytesMut     fn from_text(text: str): BytesMut
    fn len(self): usize                   fn is_empty(self): bool                    fn get(self, i: usize): Option<u8>
    fn push(mut self, byte: u8)           fn extend(mut self, bytes: [u8])           fn push_text(mut self, text: str)
    fn truncate(mut self, len: usize)     fn clear(mut self)
    fn as_slice(self): [u8]               fn to_list(self): List<u8>                 fn into_list(own self): List<u8>
    fn freeze(own self): Bytes
}

struct Bytes {                            // read-only; clones and slices share storage
    fn new(): Bytes                       fn from_list(data: List<u8>): Bytes        fn from_text(text: str): Bytes
    fn len(self): usize                   fn is_empty(self): bool                    fn get(self, i: usize): Option<u8>
    fn as_slice(self): [u8]               fn to_list(self): List<u8>
    fn slice(self, from: usize, to: usize): Bytes                fn split_at(self, mid: usize): (Bytes, Bytes)
    fn to_text(self): Result<String, DecodeError>                fn to_hex(self): String
}
```

`BytesMut` and `Bytes` implement `Index<u8>` (`b[i]`; an index past the end panics with `BytesMut index out of bounds` /
`Bytes index out of bounds`), `BytesMut` also `IndexMut<u8>` (`b[i] = x`), and both implement `Eq` (same bytes)
and `Debug` (`Bytes[1, 2, 3]`). `Bytes` is `Clone`.

The relationship to the other byte types: `List<u8>` is the plain owned vector, and `[u8]` is a borrowed view
(`as_slice`); both stay. `BytesMut` is a `List<u8>` with a byte-oriented surface, built to be filled and then frozen.
`Bytes` holds one `List<u8>` in a `Shared` handle and an offset and length into it, so `clone`, `slice`, and `split_at`
copy no bytes and the storage lives until the last handle goes. `freeze` moves a `BytesMut`'s list into a `Bytes`
without copying; going back, `to_list` copies and `into_list` (on `BytesMut`) does not. A `slice` or `split_at` that is
reversed or past the end panics with `Bytes slice out of bounds`. The encodings of the previous section take `as_slice()`.

## Allocators

```
interface Allocator {
    fn allocate(mut self, size: usize, align: usize): CPtr<u8>
    fn reallocate(mut self, ptr: CPtr<u8>, old_size: usize, new_size: usize, align: usize): CPtr<u8>
    fn deallocate(mut self, ptr: CPtr<u8>, size: usize, align: usize)
}

struct Global { }                       // the default allocator; zero-sized, stored nowhere
struct Arena { ... }                    // a bump allocator with blocks, reset, and bytes_allocated
Shared<T>                               // a built-in reference-counted cell: Shared.new(value), get, strong_count
```

`Global` and `Arena` live in `std.alloc` and are visible through the prelude; they implement `Allocator`.
`List`, `Map`, and `Heap` take an allocator as their last type argument (`List<T, A = Global>`) and store it
when it is not `Global`; `new_in` is the constructor that takes the value (`List<T>.new_in(alloc)`,
`Map<K, V>.new_in(alloc)`, `Heap.new_in(alloc, value)`). `Arena.new()` makes an empty arena, `bytes_allocated()`
reports the rounded bytes handed out since the last reset, and `reset()` frees every block and leaves the arena
usable. `Shared<T>` is shared by `clone()`, read by `get(): &T`, counted by `strong_count(): usize`, and is not
`Copy`; its count is atomic, and it is `Send` and `Sync` when `T` is both ([`shared.md`](shared.md) SH1-SH2). The rules are [`allocators.md`](allocators.md) AL1-AL8.

### Fallible allocation (issue #278)

`AllocError` is an error enum in `std` with two variants, `CapacityOverflow` and `OutOfMemory`
([`allocators.md`](allocators.md) AL11.1). The `try_` forms return `Result<_, AllocError>` instead of panicking:

```
List<T>.try_with_capacity(n: usize): Result<List<T>, AllocError>
List<T>.try_with_capacity_in(n: usize, alloc: A): Result<List<T, A>, AllocError>
List<T>.try_reserve(additional: usize): Result<(), AllocError>        // grows like reserve
List<T>.try_reserve_exact(additional: usize): Result<(), AllocError>  // asks for exactly len + additional
Map<K, V>.try_with_capacity(n: usize): Result<Map<K, V>, AllocError>
Map<K, V>.try_reserve(additional: usize): Result<(), AllocError>
String.try_with_capacity(n: usize): Result<String, AllocError>
String.try_reserve(additional: usize): Result<(), AllocError>
```

A failed `try_` operation leaves the container unchanged. The rules are AL11.1-AL11.7. `Heap`, `Deque`, `Set`,
`PriorityHeap`, and `Shared<T>` have no `try_` forms yet (AL11.6).

## `List<T>`

```
struct List<T, A = Global> {
    fn new(): List<T, A>              // only when A is `Global`: the allocator-free constructor
    fn new_in(alloc: A): List<T, A>   // takes the allocator value to store (`allocators.md`, AL4)
    fn with_capacity(n: usize): List<T, A>            // only when A is `Global`
    fn with_capacity_in(n: usize, alloc: A): List<T, A>
    fn push(mut self, value: T)       // takes ownership of the value
    fn pop(mut self): Option<T>
    fn len(self): usize
    fn capacity(self): usize
    fn is_empty(self): bool
    fn last(self): Option<&T>
    fn first(self): Option<&T>
    fn first_mut(mut self): Option<&mut T>
    fn last_mut(mut self): Option<&mut T>
    fn get(self, index: usize): Option<&T>
    fn get_mut(mut self, index: usize): Option<&mut T>
    fn reserve(mut self, additional: usize)
    fn reserve_exact(mut self, additional: usize)
    fn shrink_to_fit(mut self)
    fn insert(mut self, index: usize, value: T)
    fn remove(mut self, index: usize): T
    fn swap(mut self, a: usize, b: usize)
    fn swap_remove(mut self, index: usize): T
    fn truncate(mut self, len: usize)
    fn split_off(mut self, index: usize): List<T>
    fn append(mut self, own other: List<T, A>)
    fn drain(mut self, range: Range<usize>): ListIntoIter<T>   // also `lo..=hi`
    fn extend_from_slice(mut self, other: [T])   // needs Clone on T
    fn clear(mut self)
    fn contains(self, value: T): bool // needs == on T
    fn sort(mut self)                 // ascending; needs < on T
    fn iter(self): [T]                // borrows: `T` if Copy, else `&T`
    fn into_iter(own self): ListIntoIter<T, A>   // consumes the list, carrying the allocator
}
```

Indexing `xs[i]` is a place: it reads or writes one element, takes a `usize`, and panics when out of bounds.
`for x in xs` borrows the list for the loop and yields read-only element places (`memory-rules.md`, IT). `T` must be
storable, so `List<str>` and `List<&T>` are rejected (E0120).

`List<T>` implements `Eq` when `T: Eq` (issue #928). Two lists are equal when they have the same length and
their elements are equal in order: `[1, 2] != [2, 1]` and `[1] != [1, 1]`. Equal lists hash alike (`derives.md`,
"Hash"). `List` has no `Ord`. `assert_eq` and `assert_ne` take lists of an `Eq` element type, and `contains` searches
with the same `==`. Tests: `m24_stdlib/list_equality`, `list_equality_needs_eq`.

`List<T>` is `Iterable`, so every lazy `Iterator` adapter and consumer (`map`, `filter`, `fold`, `any`, `all`, `find`,
`zip`, `rev`, `sum`, `to_list`, `collect`, ...) can be called directly on a list without `.iter()` (`iterators.md`,
IT10). The call borrows the list through `iter()` and builds no intermediate list; call `to_list()` or `collect()` when
an eager `List` is wanted, as in `xs.map(f).to_list()`. A prelude constructor is a closure value too: `Option.Some`
maps the items to `Option`s, as in `xs.map(Option.Some).collect()`, while `Result.Ok` and `Result.Err` need the other
type argument written in a closure type (`memory-rules.md` CL11). Every method except `new` and `with_capacity` exists
for every allocator argument.

The capacity methods (`spec/allocators.md`, AL1-AL4): `with_capacity(n)` and `with_capacity_in(n, alloc)` make an
empty list with room for `n` elements, allocating the buffer up front through the default allocator or through the
allocator value the list then stores. `capacity()` reports that room. `reserve(additional)` makes room for at least
`len + additional` elements and may over-allocate; `reserve_exact(additional)` asks for exactly that many and a
smaller capacity is left alone; `shrink_to_fit()` releases the room past the length. A reservation whose length
would overflow `usize` panics (`reserve: capacity overflow`). Every allocation and reallocation goes through the
list's allocator, so an arena-backed list never touches the C heap. `new` and `with_capacity` exist only for
`Global`: `List<T, A>.new()` or `List<T, A>.with_capacity(n)` where `A` is not `Global` is E0272, because the
constructor has no allocator value to store (`allocators.md`, AL4).

The accessors that answer instead of panicking: `get(i)` and `get_mut(i)` are `Option.Some` only when `i < len`, and
`first`, `first_mut`, `last`, `last_mut`, and `is_empty` describe the ends and the emptiness. The `get_mut`,
`first_mut`, and `last_mut` receivers are `mut self`: they hand out a mutable view of an element and keep the list
exclusively borrowed until the view's last use (E0111), so nothing that may move the buffer is allowed while it is
live.

The bulk edits: `swap(a, b)` exchanges two elements and panics when either index is out of bounds
(`swap: index out of bounds`); `swap_remove(i)` returns element `i`, fills its slot with the last element, and
panics past the end (`swap_remove: index out of bounds`); `truncate(n)` drops the elements from `n` on and does
nothing when the list is already shorter; `split_off(i)` moves the elements from `i` on into a new list, and an
index equal to the length yields an empty list (`i > len` panics with `split_off: index out of bounds`);
`append(other)` moves every element of `other` to the end and frees `other`'s buffer; `extend_from_slice(other)`
appends a clone of every element of a `[T]` view and needs `Clone` on `T` (E0141); `drain(range)` removes `lo..hi`
(or `lo..=hi`) and returns an owning `ListIntoIter<T>` that yields the removed elements, so a `for` loop over it or
a `next()` call moves them out. A reversed or out-of-range drain range panics (`drain: range out of bounds`).
Elements are moved, never duplicated: each one is dropped exactly once, by the list that ends up owning it or by
the iterator the drain returned.

`split_off` and `drain` move their elements into a buffer with the default allocator, so their result is
`List<T>`/`ListIntoIter<T>` whatever `A` is. A stored allocator value cannot be duplicated, because `Allocator` does
not require `Clone`; the source keeps its own allocator and the moved elements are freed through `Global`.

Tests: `m24_stdlib/list_capacity`, `list_access`, `list_bulk_edits`, `list_drop_order`, `list_allocator_methods`,
`list_leaks`, `list_const`, `list_get_mut_immutable` (E0113), `list_extend_from_slice_needs_clone` (E0141),
`list_capacity_stored_allocator` (E0272), `list_with_capacity_in_not_allocator` (E0270), `list_drain_not_range`
(E0003), `list_append_wrong_allocator` (E0003), `list_get_mut_borrow` (E0111), `list_swap_remove_out_of_bounds`,
`list_split_off_out_of_bounds`, `list_drain_out_of_bounds`, and `list_reserve_overflow`.

## Slices (issue #710)

A slice `[T]` is a borrowed view of contiguous elements: its reads are methods the compiler implements or
extension methods in `std/src/slice.bd`, and the methods that write go through the compiler's element operations.
An array `[T; N]` has the same methods through an implicit view: a method call whose receiver is an array first
looks for a method of the array, then for a slice method or a slice extension, and takes `arr[0..N]` for that call.
The view is a slice like any other, so it borrows the array and a mutating method needs the array binding to be
mutable. A `List<T>` is viewed explicitly with `as_slice()` or by slicing it (`xs[a..b]`); there is no implicit
`List`-to-slice view: the methods `List` declares itself (`get`, `first`, `last`, `contains`, `sort`, ...) work on a
list, and a slice-only extension such as `xs.binary_search(...)` or `xs.split_at(...)` is E0002 until the list is
viewed, as in `xs.as_slice().binary_search(...)`. That was the decision
the issue asked for: an explicit method for lists, an implicit view for arrays.

```
fn [T].is_empty(self): bool
fn [T].first(self): Option<&T>
fn [T].last(self): Option<&T>
fn [T].get(self, i: usize): Option<&T>             // checked indexing; `None` when out of bounds
fn [T].get(self, range): Option<[T]>               // checked slicing; `None` when out of bounds or reversed
fn [T].contains(self, value: T): bool              // needs `Eq`
fn [T].starts_with(self, prefix: [T]): bool        // needs `Eq`
fn [T].ends_with(self, suffix: [T]): bool          // needs `Eq`
fn [T].split_at(self, mid: usize): ([T], [T])      // panics when `mid > len`
fn [T].split_at_mut(mut self, mid: usize): ([T], [T])
fn [T].split_first(self): Option<(&T, [T])>
fn [T].split_last(self): Option<(&T, [T])>
fn [T].binary_search(self, value: T): Result<usize, usize>                  // needs `Ord`
fn [T].binary_search_by<F: FnMut(&T): Ordering>(self, f: F): Result<usize, usize>
fn [T].binary_search_by_key<K: Ord, F: FnMut(&T): K>(self, key: K, f: F): Result<usize, usize>
fn [T].to_list(self): List<T>                      // needs `Clone`
fn [T].chunks(self, n: usize): chunks iterator     // panics when `n == 0`
fn [T].chunks_exact(self, n: usize): chunks iterator
fn [T].windows(self, n: usize): windows iterator   // panics when `n == 0`
fn [T].swap(mut self, a: usize, b: usize)          // panics when an index is out of bounds
fn [T].reverse(mut self)
fn [T].fill(mut self, value: T)                    // needs `Clone`; drops each old element
fn [T].copy_from_slice(mut self, other: [T])       // needs `Copy`; panics on a length mismatch
fn [T].clone_from_slice(mut self, other: [T])      // needs `Clone`; panics on a length mismatch
fn [T].sort(mut self)                              // needs `Ord`; stable
fn [T].sort_by<F: FnMut(&T, &T): Ordering>(mut self, f: F)                     // stable
fn [T].sort_by_key<K: Ord, F: FnMut(&T): K>(mut self, key: F)                  // stable
fn [T].is_sorted_by<F: FnMut(&T, &T): Ordering>(self, f: F): bool
fn [T].rotate_left(mut self, mid: usize)           // panics when `mid > len`; a no-op at 0 and `len`
fn [T].rotate_right(mut self, mid: usize)
fn [[T]].concat(self): List<T>                     // needs `Clone`
fn [String].join(self, sep: str): String
fn [str].join(self, sep: str): String
fn [u8].is_ascii(self): bool
fn [u8].to_ascii_lowercase(self): List<u8>         // copies non-letters unchanged
fn [u8].to_ascii_uppercase(self): List<u8>
fn [u8].eq_ignore_ascii_case(self, other: [u8]): bool
fn List<T>.as_slice(self): [T]
```

`len`, `iter`, `get`, `last`, `to_list`, `swap`, `fill`, `copy_from_slice`, `clone_from_slice`, `chunks`,
`chunks_exact`, and `windows` are compiler-implemented: they need element addresses or a checked view. The rest
are extension methods in `std/src/slice.bd` (`spec/extensions.md`), visible through the prelude like the other
standard-library extensions. `[T]`'s own `last` and `to_list` win over the `Iterator` defaults of the same name:
the slice's `last` is the view `Option<&T>`, and its `to_list` makes an owned `List<T>` of clones (the iterator
defaults are reachable on an adapter chain, where the receiver is not a slice).

A method that writes takes `mut self`, so the slice must be a mutable place: `let mut view = xs[0..3]`, then
`view.sort()`. A slice expression is accepted as the receiver when the place it views is mutable
(`xs[0..2].sort()` and `xs.as_slice().swap(0, 1)` need a mutable `xs`, E0113 otherwise). The swap-based
algorithms move elements byte for byte, so a non-`Copy` element is never cloned and is dropped exactly once;
`fill` and `clone_from_slice` drop each old element before storing a clone, and `copy_from_slice` copies `Copy`
bytes. The sort is insertion sort: stable, and a comparator that is not a total order cannot lose, duplicate, or
corrupt an element.

`get` takes either an index or a range: `xs.get(3)` is `Option<&T>` and `xs.get(1..4)` / `xs.get(1..=3)` is
`Option<[T]>`. A stored range works when its endpoints are `usize`; a `Range<i32>` at `get` is E0003, because
there is no implicit integer conversion. The checked forms never panic: a reversed or out-of-range range is
`Option.None`. The other panics carry their own messages: `swap: index out of bounds`, `split_at: index out of bounds`,
`rotate_left`/`rotate_right: index out of bounds`, `chunks: chunk size must be non-zero`,
`windows: window size must be non-zero`, and `copy_from_slice`/`clone_from_slice: source and destination lengths
differ`.

Slicing a slice or a `List` of elements wider than one byte moves by elements: `xs[2..5]` of an `[i32]` starts at
the third element. (`String` and `str` slicing stays byte-based.)

The view borrows its source, so growing a `List` (or writing an array element) while a slice view, a `chunks`
iterator, or a `split_at_mut` half is live is E0111. A `chunks`, `chunks_exact`, or `windows` iterator is
second-class (it holds the slice view), and `for chunk in xs.chunks(n)` binds each chunk as a slice view. At
compile time the read methods work (a slice is a copy of the values it views in the interpreter); the writing
methods are E0224, because a compile-time view is not the storage it views.

Tests: `m24_stdlib/slice_read`, `slice_write`, `slice_chunks`, `slice_arrays`, `list_as_slice`,
`slice_bytes_ascii`, `slice_join_concat`, `slice_binary_search`, `slice_strings`, `slice_edges`, `slice_drops`,
`slice_chunks_zero`, `slice_windows_zero`, `slice_split_at_bounds`, `slice_rotate_bounds`, `slice_swap_bounds`,
`slice_copy_length`, `slice_clone_length`, `slice_chunks_borrow` (E0111), `slice_split_at_mut_borrow` (E0111),
`slice_as_slice_borrow` (E0111), `slice_array_view_borrow` (E0111), `slice_fill_needs_clone` (E0141),
`slice_to_list_needs_clone` (E0141), `slice_copy_needs_copy` (E0141), `slice_sort_needs_ord` (E0141),
`slice_reverse_immutable` (E0113), `slice_view_immutable_list` (E0113), `slice_write_exclusivity` (E0110),
`slice_const_write` (E0224), and `slice_const_read` (the reads at compile time).

## `Map<K, V>`

```
struct Map<K, V, A = Global> {
    fn new(): Map<K, V, A>            // only when A is `Global`
    fn new_in(alloc: A): Map<K, V, A> // takes the allocator value to store
    fn insert(mut self, key: K, value: V)   // replaces an existing key's value
    fn get(self, key: K): Option<&V>
    fn remove(mut self, key: K): Option<V>
    fn contains_key(self, key: K): bool
    fn clear(mut self)
    fn len(self): usize
    fn iter(self): MapIter<K, V>          // yields (&K, &V)
    fn into_iter(own self): MapIntoIter<K, V, A>   // consumes the map, yielding (K, V)
}
```

Keys need `==` (`Eq`). `for (k, v) in map` borrows the map for the loop; iteration order is not specified, so a
program must not depend on it. The implementation is an open-addressing hash table: lookup, insert, and remove are
expected O(1), the table keeps a 3/4 load factor and rehashes as it grows, and removing a key shifts the rest of its
probe cluster back instead of leaving tombstones. Keys are hashed structurally — integers, `bool`, `char`, floats,
`String` by its bytes, tuples, fixed-size arrays, structs, enums, `Option`, `Result`, and ranges — so a key needs only `==`, not
`Hash`. The fold is the stable one of `hash_of` (HS1), and the key rule is the structural one (HS5).

### Map in-place operations (issues #482, #1017)

```
fn get_mut(mut self, key: K): Option<&mut V>        // a mutable view of the value, or None
fn get_or_insert(mut self, key: K, value: V): &mut V
fn get_or_insert_with<F: FnOnce(): V>(mut self, key: K, make: F): &mut V
fn update<F: FnOnce(&mut V)>(mut self, key: K, change: F): bool
fn upsert<F: FnOnce(&mut V)>(mut self, key: K, value: V, change: F)
```

The `&mut V` these return keeps the map exclusively borrowed until its last use (a `m.insert(...)` while it is live
is E0111), because inserting may move the table. The key needs `==`, like every map operation (E0141). There is no
entry API: the language has no `Map.entry`, `MapEntry`, `or_insert`, or `or_insert_with` (decided in #1017).

`get_or_insert` returns the value stored under `key`; when the key is absent it first stores `value`. A value that is
already there is kept, and the incoming key and value are dropped on the spot. `get_or_insert_with` does the same,
but calls `make` for the value only when the key is absent; the closure is consumed either way. `update` runs `change` on
the value for `key` and returns whether the key was there. `upsert` stores `value` for an absent key and otherwise
runs `change` on the value already there, dropping `value`.

### Map views, updates, and construction (issue #712)

```
fn is_empty(self): bool

// views: the borrowing iterators are second-class like every iterator over borrowed storage
fn keys(self): MapKeys<...>                    // yields &K, in iteration order
fn values(self): MapValues<...>                // yields &V
fn into_keys(own self): MapIntoKeys<...>       // consumes the map; yields K
fn into_values(own self): MapIntoValues<...>   // consumes the map; yields V

// in-place updates
fn get_mut(mut self, key: K): Option<&mut V>
fn get_or(mut self, key: K, default: V): &V
fn remove_entry(mut self, key: K): Option<(K, V)>
fn retain<F: FnMut(&K, &V): bool>(mut self, f: F)
fn drain(mut self): MapIntoIter<K, V>                             // default allocator only
fn extend<I: Iterator<(K, V)>>(mut self, items: I) where K: Eq

// capacity: the table keeps a power-of-two slot count and the 3/4 load factor
fn with_capacity(capacity: usize): Map<K, V>       // associated function; default allocator only
fn reserve(mut self, additional: usize)
fn shrink_to_fit(mut self)
```

`keys` and `values` borrow the map for the iterator's whole life and yield views, so the map cannot be changed
until the iterator's last use; `into_keys` and `into_values` consume the map and yield owned values. `MapKeys`
and `MapValues` are the nameable iterator types, second-class because they view the map; `MapIntoKeys` and
`MapIntoValues` own the entries they have not handed out.

`get_or(key, default)` is the read-only form of `get_or_insert`: it returns the value stored under `key`, storing
`default` first when the key is absent. A read-only view of a caller's default that is not stored would need the
result to derive from two parameters, which the return-origin rule does not allow (E0121), so the default is
always stored. `get_or` keeps the map borrowed (shared) until the view's last use.

`remove_entry` moves the stored key and value out; `remove` drops the stored key and returns only the value.
`retain` calls its predicate on a view of each entry and drops the ones it rejects, shifting the rest of the probe
cluster back as `remove` does. `drain` empties the map and yields its entries as an owning iterator, leaving the
map empty and usable with its allocator; only a default-allocator map has it (a stored allocator cannot be copied
into the iterator). `extend` inserts every item, replacing the value of a key that is already present, as `insert`
does.

`with_capacity(n)` makes an empty map whose table holds `n` entries without growing; a map with a stored allocator
is made with `new_in` and then `reserve` (E0272 for `with_capacity`). `reserve(additional)` grows the table so it
holds the entries plus `additional` more without passing the load factor, and `shrink_to_fit` rehashes into the
smallest table that holds the entries (and frees the buffer of an empty map). A request that would need more than
2^62 slots panics with `Map capacity overflow`.

`Map<K, V>` implements `Eq` (the same entries with equal values, whatever the insertion order),
`Clone` (when `K` and `V` are `Clone`), `Debug` (when `K` and `V` are `Debug`, as `{k: v, ...}` in iteration
order), and `Default` (an empty map; only for the default allocator). A struct that holds a `Map` derives each of
these when its key and value types allow it.

### Lookups take the key type (issue #1017)

Every lookup takes the map's own key type: `get`, `get_mut`, `contains_key`, `remove`, and `remove_entry` on a
`Map<String, V>` take a `String`, so `map.get("x".to_string())`. There is no borrowed-key lookup (`Map<String, V>.get`
taking a `str`): a method has one signature and the language has no specialization, so the map is written in Burdin
with `key: K` throughout (decided in #1017; it replaced the `str` lookups of #712).

Tests: `m24_stdlib/map_views`, `map_in_place`, `map_capacity_interfaces`, `map_views_allocator`,
and the compile-fail case `map_get_or_needs_eq` (E0141), and
`map_with_capacity_allocator` (E0272).

## `Set<T>` (issue #190)

`std.set` is a thin layer over `Map<T, bool, A>`, so it follows the map's rules: an element needs only `==` (`Eq`),
lookup, insert, and remove are expected O(1), and the iteration order is not specified. Like `List` and `Map`, a set
takes an allocator as its last type argument and stores it (`allocators.md`).

```
struct Set<T: Eq, A: Allocator = Global> {
    fn new(): Set<T>                               // only when A is `Global`
    fn new_in(own alloc: A): Set<T, A>
    fn from_list(own items: List<T>): Set<T>       // a repeated element is stored once
    fn from<const N: usize>(own items: [T; N]): Set<T>   // from an array (issue #708)
    fn len(self): usize
    fn is_empty(self): bool
    fn contains(self, value: T): bool
    fn insert(mut self, own value: T): bool        // false (and the new value dropped) when already present
    fn remove(mut self, value: T): bool
    fn clear(mut self)
    fn extend(mut self, own items: List<T>)
    fn extend_iter<I: Iterator<T>>(mut self, own iter: I)   // issue #708
    fn into_list(own self): List<T>                // consumes the set, in no particular order
    fn iter(self): SetIter<MapEntries<T, bool>, T> // yields &T; `for x in set` borrows the set
}

// with `T: Clone`, the default allocator, and the result uses the default allocator
fn to_list(self): List<T>
fn clone(self): Set<T>
fn union(self, other: Set<T>): Set<T>
fn intersection(self, other: Set<T>): Set<T>
fn difference(self, other: Set<T>): Set<T>             // in `self` and not in `other`
fn symmetric_difference(self, other: Set<T>): Set<T>   // in exactly one of the two

fn is_subset(self, other: Set<T>): bool
fn is_superset(self, other: Set<T>): bool
fn is_disjoint(self, other: Set<T>): bool
```

`Set<T>` implements `Eq` (same elements, regardless of order), `Debug` (`{1, 2, 3}`, in iteration order),
`Iterable<&T>`, and, for the default allocator, `FromIterator<T>` (issue #708). `MapEntries<K, V>` is the nameable type of the iterator `map.iter()` returns, added so a library type
can wrap it. Operations that copy elements need `Clone`; they return a set that uses the default allocator.

Tests: `m24_stdlib/set_basic`, `set_construction`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_allocator`,
`set_needs_eq`.

## `Deque<T>` (issue #191)

`std.deque` is a double-ended queue made of two lists: `front` holds the first elements in reverse order and `back`
holds the rest, so a push at either end is one `List.push`. A pop from an end whose list is empty first moves the other
list over, which makes a run of pops from one end amortized O(1); a program that alternates `pop_front` and `pop_back`
on a deque held in one half pays O(n) for each switch. `List` is unchanged, and `Deque` always uses the default
allocator.

```
struct Deque<T> {
    fn new(): Deque<T>
    fn from_list(own items: List<T>): Deque<T>
    fn from<const N: usize>(own items: [T; N]): Deque<T>      // from an array (issue #708)
    fn len(self): usize
    fn is_empty(self): bool
    fn push_front(mut self, own value: T)
    fn push_back(mut self, own value: T)
    fn pop_front(mut self): Option<T>
    fn pop_back(mut self): Option<T>
    fn front(self): Option<&T>
    fn back(self): Option<&T>
    fn get(self, i: usize): Option<&T>
    fn clear(mut self)
    fn extend(mut self, own items: List<T>)        // at the back, in order
    fn extend_iter<I: Iterator<T>>(mut self, own iter: I)   // at the back, in order (issue #708)
    fn into_list(own self): List<T>                 // front to back
    fn iter(self): DequeIter<&Deque<T>, T>          // yields &T from the front to the back
    fn iter_mut(mut self): DequeIterMut<&mut Deque<T>, T>   // yields &mut T from the front to the back (IT15)
    fn clone(self): Deque<T>                        // T: Clone
}
```

`Deque<T>` implements `Index<T>` and `IndexMut<T>` (`d[i]` counts from the front and panics with `Deque index out of
bounds` past the end), `Eq` (same length and equal elements), `Debug` (`[1, 2, 3]`), and `Iterable<&T>`.
`DequeIter<D, T>` is generic over the view of the deque it walks and needs `D: Index<T> + DequeLen`; for that, a
shared view `&X` satisfies an interface bound that `X` satisfies. `DequeIterMut<D, T>` needs `D: IndexMut<T> + DequeLen`,
and a mutable view `&mut X` satisfies the bounds `X` does as well.

Tests: `m24_stdlib/deque_basic`, `deque_construction`, `deque_index_iteration`, `deque_strings_and_drops`,
`deque_index_out_of_bounds`, `iter_mut_deque`.

## `PriorityHeap<T>` (issue #192)

`std.heap` is a priority queue ordered by `Ord`. The elements are kept in ascending order in one list, so the greatest
element is the last one: `peek` and `pop` are O(1), `push` finds its place with a binary search (O(log n)
comparisons) and moves the elements above it, and iterating in priority order is walking the list backwards. It is
not a binary heap: swapping two elements of one list needs a placeholder value the language does not have, and the
sorted list gives iteration in priority order for free. Elements that compare equal come out in the order they were
pushed. The element type only needs an `Ord` bound, which a type with an `Ord` impl also satisfies.

```
struct PriorityHeap<T: Ord> {
    fn new(): PriorityHeap<T>
    fn from_list(own items: List<T>): PriorityHeap<T>
    fn from<const N: usize>(own items: [T; N]): PriorityHeap<T>   // from an array (issue #708)
    fn len(self): usize
    fn is_empty(self): bool
    fn push(mut self, own value: T)
    fn extend(mut self, own items: List<T>)
    fn extend_iter<I: Iterator<T>>(mut self, own iter: I)   // issue #708
    fn pop(mut self): Option<T>                       // the greatest element
    fn peek(self): Option<&T>
    fn clear(mut self)
    fn into_sorted_list(own self): List<T>            // highest priority first
    fn iter(self): PriorityHeapIter<&PriorityHeap<T>, T>   // yields &T, highest priority first
    fn clone(self): PriorityHeap<T>                   // T: Clone
}
```

`PriorityHeap<T>` implements `Index<T>` (`h[0]` is the element `pop` returns next; past the end it panics with
`PriorityHeap index out of bounds`), `Debug` (`[9, 5, 1]`, highest priority first), and `Iterable<&T>`. To get a
min-heap, wrap the elements in `Reverse<T>`.

`PriorityHeap<T>` derives `Eq` (issue #928, `derives.md`): two heaps are equal when their sorted lists are equal
element by element, so when their elements are equal in priority order. Elements that `Ord` ranks equal keep their push
order, so an `Ord` that ignores a field can make two heaps with the same elements unequal. `Hash` is not implemented.
Tests: `m24_stdlib/heap_equality`.

Tests: `m24_stdlib/heap_basic`, `heap_construction`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`,
`heap_needs_ord`.

## `Once<T>` and `Lazy<T, F>` (issue #420)

```
struct Once<T> {
    fn new(): Once<T>                     fn is_set(self): bool                  fn get(self): Option<&T>
    fn set(mut self, value: T): bool      // false, and `value` dropped, when already set
    fn get_or_init<F: FnOnce(): T>(mut self, init: F): &T
    fn take(mut self): Option<T>          // leaves the cell empty
}

struct Lazy<T, F: FnOnce(): T> {
    fn is_forced(self): bool              fn force(mut self): &T                 fn peek(self): Option<&T>
}
fn lazy<T, F: FnOnce(): T>(init: F): Lazy<T, F>
```

`Once` holds a value after the first `set` (or `get_or_init`) and refuses every later one; `get_or_init` runs `init` only
when the cell is empty, so it runs at most once. `Lazy` runs its closure the first time `force` is called and keeps the
result; `peek` never computes. The views `get`, `get_or_init`, `force`, and `peek` return keep the cell borrowed, and
`force` and `get_or_init` take it `mut`, so a cell that is handed out cannot be forced again until the view is gone.

**The static/global story.** Burdin has global variables, declared with `static` (immutable) and `static mut`
(writable, in `unsafe` code only), with the rules in `globals.md` (issue #485). A `static` holds plain data: a scalar,
`str`, or an aggregate of those, never a `String` or a collection (GL2). A value that owns memory, such as a cache or a
`String` of configuration, is a value that lives in a local or a struct and is passed to whatever needs it, and `Once`
and `Lazy` are ordinary values like that. A value that is known at compile time is a `const`
instead. A cell several threads can race to fill is a value handed to the tasks that share it (`globals.md`, GL6).

## Paths (issue #194)

```
struct Path: Clone, Eq {
    fn new(text: str): Path                 // the rules of the system the program is compiled for
    fn unix(text: str): Path                // Unix rules       fn windows(text: str): Path     // Windows rules
    fn as_str(self): str                    fn separator(self): char
    fn is_absolute(self): bool              fn is_relative(self): bool       fn drive(self): Option<String>
    fn components(self): List<String>       // names only: no separators, empty pieces, or `.`; `..` is kept
    fn join(self, other: str): Path
    fn parent(self): Option<Path>           fn file_name(self): Option<String>
    fn file_stem(self): Option<String>      fn extension(self): Option<String>
    fn with_file_name(self, name: str): Path        fn with_extension(self, extension: str): Path
    fn normalize(self): Path
    fn exists(self): bool                   // the only function that asks the operating system
}
Path: Display    Path: Debug                // `Path("a/b")`
```

**Platform split.** There is one module. A `Path` carries the rules that read its text, so `Path.unix` and
`Path.windows` work on every system and the tests exercise both; `Path.new` chooses the host's. Unix rules: `/` is the
only separator and a path is absolute when it starts with one. Windows rules: `/` and `\` both separate; a drive
(`C:`) is a prefix; a path is absolute only with a drive and a separator (`C:\x`) or as a share (`\\server\share`), so
`\x` and `C:x` are relative; `join` writes `\`. Nothing but `exists` touches the file system, and none of this
resolves symbolic links.

- `join`: an absolute `other` (on Windows also one with a drive) replaces the path; on Windows a rooted `other` such as
  `\x` keeps the drive; an empty path or empty `other` adds no separator; otherwise `other` follows one separator.
- `parent` drops the last name (`a/b` is `a`, `/a` is `/`, `a` is the empty path); a path with no names has none.
  `file_name` is the last name, `Option.None` for `..` or no names; trailing separators and `.` pieces are ignored.
- `extension` is the text after the last dot of the file name, unless the name starts with that dot (`.profile` has
  none); `file_stem` is the rest (`archive.tar.gz` is `archive.tar` and `gz`). `with_extension("")` removes it.
- `normalize` removes `.` and empty pieces, cancels `name/..`, keeps a leading `..` of a relative path, drops `..` at a
  root, and writes the rules' separator; an empty result is `.`. It is lexical, so it can change what a path with
  symbolic links names.
- `exists` is true when the system can see the path (the C library's `access`, or `_access` on Windows, with no mode bits). It is impure, so a
  compile-time evaluation that reaches it fails with E0224. Directory listing, metadata, and walking are the next section.

**Cost (issue #1020).** Every operation reads the text's bytes in place: separators, the drive colon, and dots are
ASCII, so no character is decoded and no `List<char>` is built. A method that returns a value allocates only that
value: `file_name`, `file_stem`, `extension`, `drive`, `parent`, `join`, `with_file_name`, and `with_extension`
allocate one block each; `components` allocates its list once, at its final size, plus one `String` per name;
`normalize` allocates its list of kept names and its result; `is_absolute`, `is_relative`, and `separator` allocate
nothing. The pieces stay owned (`Option<String>`, `List<String>`, `Path`) rather than views of the path, so a result
outlives the `Path` it came from. `m24_stdlib/path_allocations` pins these counts with `__alloc_count()`
(`allocators.md`, AL9), and `benches/std/src/path.bd` measures `components`, `file_name`, `extension`, `parent`,
`join`, and `normalize`.

Tests: `m24_stdlib/path_unix`, `path_windows`, `path_edit`, `path_display_debug`, `path_exists`, `path_allocations`.

## Directories, metadata, and walking (issue #411)

```
enum FileKind: Copy, Clone, Eq, Debug { File  Dir  Symlink  Other }

struct Metadata: Clone, Copy, Eq, Debug {
    size: u64                 // bytes
    mode: u32                 // the system's mode bits
    modified: i64             // seconds since the Unix epoch
    accessed: i64             // seconds since the Unix epoch
    kind: FileKind
}
Metadata { fn is_file(self): bool    fn is_dir(self): bool    fn is_symlink(self): bool }

fn read_dir(path: str): Result<List<String>, IoError>
fn create_dir(path: str): Result<(), IoError>          fn create_dir_all(path: str): Result<(), IoError>
fn remove_dir(path: str): Result<(), IoError>          fn remove_dir_all(path: str): Result<(), IoError>
fn remove_file(path: str): Result<(), IoError>
fn rename(from: str, to: str): Result<(), IoError>
fn metadata(path: str): Result<Metadata, IoError>
fn canonicalize(path: str): Result<String, IoError>
fn walk(root: str): Result<List<String>, IoError>
fn walk_glob(root: str, pattern: str): Result<List<String>, IoError>

Path {   // extension methods, the same operations with the receiver as the path
    fn read_dir(self): Result<List<String>, IoError>        fn metadata(self): Result<Metadata, IoError>
    fn canonicalize(self): Result<String, IoError>          fn walk(self): Result<List<String>, IoError>
    fn walk_glob(self, pattern: str): Result<List<String>, IoError>
}
```

Every function is impure: a compile-time evaluation that reaches one fails with E0224. A read that only inspects the
file system (`read_dir`, `metadata`, `canonicalize`, `walk`, `walk_glob`) reports `IoError.Read`; one that changes it
(`create_dir`, `create_dir_all`, `remove_dir`, `remove_dir_all`, `remove_file`, `rename`) reports `IoError.Write`.
Each error carries the portable kind and the OS code (`errors.md`, ERR10-ERR11). `rename` names its destination `to`
in the error. `remove_dir_all` on a path that is not a directory has kind `NotADirectory` and no OS code, because
`std` saw the kind before any system call failed.

- `read_dir` returns the entry names, not paths, without `.` and `..`, in the order the system returns them; sort the
  list for a deterministic order. A missing path or a file is an error. The names join with `Path.join`.
- `create_dir` creates one directory and requires the parent to exist; an existing directory is an error.
  `create_dir_all` creates every missing parent, treats an existing directory as success, and reports a path that
  exists as a file.
- `remove_dir` removes an empty directory only. `remove_dir_all` removes a directory and everything under it; a
  symbolic link is removed as a link and never followed, and a path that is not a directory is an error.
- `remove_file` removes a file or a symbolic link; a directory is an error.
- `rename` renames a file or a directory and replaces an existing `to`, on Unix and Windows alike.
- `metadata` reads the entry itself and does not follow a symbolic link (`lstat` on Unix), so `kind` is the entry's
  kind. `mode` is the system's mode bits: the `S_IFMT` file-kind bits agree between Unix and Windows for `File`,
  `Dir`, and `Symlink`, and the permission bits are the system's. `modified` and `accessed` are seconds since the
  Unix epoch. On Windows the CRT's `_stat64` follows a link, so a link's size and times may describe its target,
  while `kind` is still `Symlink` when the entry is a reparse point.
- `canonicalize` returns an absolute path with `.` and `..` resolved, using the system's separators. The path must
  exist. On Unix it is `realpath`, which also resolves symbolic links; on Windows it is `_fullpath`, which only
  normalizes the text and keeps a link's name.
- `walk` returns every file and directory under `root`, recursively, as a path relative to `root` with the system's
  separator, sorted, without `root` itself. A symbolic link is listed but not followed. A directory that cannot be
  read makes the whole walk fail.
- `walk_glob` is `walk` filtered by `pattern`, matched against the whole relative path. `*` matches any run of
  characters inside one path segment, `?` matches one character inside a segment, and `**` matches across segments;
  a `**` followed by a separator also matches no segment, so `**/*.bd` finds a top-level file too. Both `/` and `\`
  are separators in a pattern.

Tests: `m24_stdlib/dir_basic`, `dir_errors`, `dir_canonicalize`, `dir_walk`.

## Files and file handles (issue #702)

The file operations live in `std.fs`, beside the directory operations; the prelude makes every name below visible
unqualified. A file is opened through `OpenOptions` or the two shortcuts `File.open` and `File.create`, and the
handle owns the system descriptor. Bytes are never translated: on every system a file is binary, so `0x00` and text
that is not valid UTF-8 come back exactly as they were written.

```
struct OpenOptions {
    fn new(): OpenOptions
    fn read(mut self, on: bool)                fn write(mut self, on: bool)
    fn append(mut self, on: bool)              fn truncate(mut self, on: bool)
    fn create(mut self, on: bool)              fn create_new(mut self, on: bool)
    fn open(self, path: str): Result<File, IoError>
}

struct File {
    fn open(path: str): Result<File, IoError>          // `read` only
    fn create(path: str): Result<File, IoError>        // write, create, and truncate
    fn sync_all(self): Result<(), IoError>
    fn set_len(mut self, size: u64): Result<(), IoError>
    fn close(own self): Result<(), IoError>            // reports the error a drop would discard
}
File: Read     // read(mut buf: List<u8>, max: usize): Result<usize, IoError>
File: Write    // write(data: [u8]): Result<usize, IoError>; flush(): Result<(), IoError> (there is no buffer)
File: Seek     // seek(from: SeekFrom): Result<u64, IoError>; position() and rewind() come from `Seek`
File: Drop     // closes the descriptor

fn read(path: str): Result<List<u8>, IoError>
fn write(path: str, data: [u8]): Result<(), IoError>
fn append(path: str, data: [u8]): Result<(), IoError>
fn copy_file(from: str, to: str): Result<u64, IoError>
fn exists(path: str): bool
```

- `File.open` opens for reading, and `File.create` opens for writing, creating the file when it is missing and
  emptying it when it exists. `OpenOptions.new()` starts with every option off; each setter changes one option.
- `OpenOptions.open` rejects a set with no read, write, or append access, `truncate` without write or append access,
  and `append` together with `truncate`, with `IoError.InvalidInput`, before it reaches the system. `create_new`
  implies `create` and fails when the file exists. A handle opened only for appending is opened for writing, and
  every write through it goes to the end whatever the position.
- A system failure is `IoError.Read` when the options only read and `IoError.Write` otherwise, and names the path
  the handle was opened with; a seek before the start is `IoError.Seek`.
- `read` appends up to `max` bytes to the caller's list, growing it, and returns how many bytes it appended; `0` is
  the end of the file. The `Read` defaults (`read_exact`, `read_to_end`, `read_to_string`) and the `Write` defaults
  (`write_all`, `write_text`, `write_line`) are built on it, and `BufReader` and `BufWriter` wrap a `File` like any
  other reader or writer.
- `flush` succeeds without doing anything, because a file handle has no buffer; `sync_all` is the durability call
  (`fsync` on Unix, `_commit` on Windows). `set_len` cuts the file or extends it with zero bytes.
- Dropping the handle closes the descriptor. `close` closes it too and returns the failure a drop would discard;
  it marks the handle closed, so the drop that follows does not close the descriptor again. Closing a descriptor
  twice is an invalid descriptor, which the Windows CRT treats as a fatal parameter error.
- `read`, `write`, and `append` are the byte-level counterparts of `read_to_string` and `write_file`; `write`
  creates or replaces, and `append` creates a missing file. `copy_file` copies the bytes only, not permissions, and
  returns how many it copied. It is not named `copy` because `std.rw` already exports the reader/writer `copy` under
  that name, and the prelude has only the unqualified name for one of them; write `fs.copy_file(...)` for a file.
  `exists` is `Path.exists`.
- Every operation is impure: a compile-time evaluation that reaches one fails with E0224.

Tests: `m24_stdlib/file_handles`, `file_errors`, `file_const`.

## Time (issue #195)

```
struct Duration: Copy, Clone, Eq, Ord, Hash, Default   // a non-negative length of time
struct Instant: Copy, Clone, Eq, Ord, Hash, Debug      // a reading of the monotonic clock
struct SystemTime: Copy, Clone, Eq, Ord, Hash, Debug   // a reading of the real-time clock
struct DateTime: Copy, Clone, Eq, Ord, Hash, Debug     // a calendar date and time of day, in UTC
enum Weekday: Copy, Clone, Eq, Ord, Hash, Debug        // Monday ... Sunday
interface Clock { fn now(self): Instant   fn system_time(self): SystemTime }
struct SystemClock { }                                          // zero-sized; the operating system's clocks
struct ManualClock                                              // moves only when told to
fn sleep(d: Duration)

Duration {
    fn zero(): Duration                                  fn from_nanos(n: u64): Duration
    fn from_micros(n: u64): Duration                     fn from_millis(n: u64): Duration
    fn from_secs(n: u64): Duration                       fn from_secs_f64(secs: f64): Duration
    fn as_nanos(self): u64      fn as_micros(self): u64  fn as_millis(self): u64     fn as_secs(self): u64
    fn as_secs_f64(self): f64
    fn subsec_nanos(self): u32  fn subsec_micros(self): u32                          fn subsec_millis(self): u32
    fn is_zero(self): bool
    fn checked_add(self, other: Duration): Option<Duration>
    fn checked_sub(self, other: Duration): Option<Duration>
    fn saturating_sub(self, other: Duration): Duration   fn abs_diff(self, other: Duration): Duration
    fn times(self, n: u64): Duration                     fn divide(self, n: u64): Duration
}                                                        // and `+`, `-` through Add and Sub; Display and Debug

Instant {
    fn now(): Instant                                    fn from_start(since_start: Duration): Instant
    fn since_start(self): Duration                       fn elapsed(self): Duration
    fn duration_since(self, earlier: Instant): Duration
    fn checked_add(self, d: Duration): Option<Instant>   fn checked_sub(self, d: Duration): Option<Instant>
}

SystemTime {
    fn now(): SystemTime                                 fn unix_epoch(): SystemTime
    fn from_unix_secs(secs: i64): SystemTime             fn from_unix_nanos(nanos: i64): SystemTime
    fn unix_secs(self): i64     fn unix_nanos(self): i64 fn subsec_nanos(self): u32
    fn duration_since(self, earlier: SystemTime): Option<Duration>
    fn checked_add(self, d: Duration): Option<SystemTime>
    fn checked_sub(self, d: Duration): Option<SystemTime>
    fn to_utc(self): DateTime
}

DateTime {   // the fields year: i32, month, day, hour, minute, second, nanosecond: u32 are public
    fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32, nanosecond: u32): Option<DateTime>
    fn from_unix_secs(secs: i64): DateTime               fn to_unix_secs(self): i64
    fn to_system_time(self): SystemTime
    fn is_leap_year(self): bool   fn ordinal(self): u32  fn weekday(self): Weekday
    fn format(self, pattern: str): String
}                                                        // and Display (ISO 8601)

ManualClock {
    fn new(start: SystemTime): ManualClock
    fn advance(mut self, d: Duration)                    fn set_system_time(mut self, time: SystemTime)
}
```

- **Durations.** A `Duration` is one `u64` of nanoseconds, so it is never negative and tops out at about 584 years.
  Every operation that could leave that range panics in every build profile, not only in debug: `from_micros`,
  `from_millis`, `from_secs`, and `times` when the product does not fit, `+` when the sum does not fit, `-` when
  the right side is longer, `from_secs_f64` for a negative, NaN, or too large value, and `divide(0)`. The `checked_`
  forms return `Option.None` instead, and `saturating_sub` stops at zero. `from_secs_f64` truncates to a whole nanosecond.
  The `as_` functions round down, and `subsec_*` give the part after the last whole second.
- **Writing a duration.** `Display` (and `Debug`, which is the same) writes the largest unit that gives a whole part,
  with the fraction after it and no trailing zeros: `1.5s`, `250ms`, `42µs`, `7ns`, `0ns`. Width, fill, and
  alignment apply; the precision does not.
- **Two clocks.** `Instant` is the monotonic clock: it never goes backward, its zero is unspecified, and only the
  difference between two readings means anything, so it is what measures how long something took.
  `duration_since` is zero when `earlier` is not earlier, and `elapsed` is `Instant.now().duration_since(self)`.
  `SystemTime` is the real-time clock: nanoseconds since 1970-01-01 00:00:00 UTC in an `i64` (the years 1678 to 2262,
  negative before 1970). It can be set, so it can jump backward, which is why `SystemTime.duration_since` returns an
  `Option`. `unix_secs` rounds toward the past, so a time before the epoch still has a `subsec_nanos` from 0 to
  999,999,999. The monotonic clock is `clock_gettime(CLOCK_MONOTONIC)` and `QueryPerformanceCounter`; the real-time
  clock is `clock_gettime(CLOCK_REALTIME)` and `GetSystemTimePreciseAsFileTime`.
- **Calendar.** `DateTime` is the proleptic Gregorian calendar in UTC with no leap seconds and no time zones. The
  conversions with Unix seconds are exact, before 1970 too; `from_unix_secs` panics when the year does not fit the
  `i32` field. `DateTime.new` returns
  `Option.None` for a month outside 1 to 12, a day the month does not have (February 29 only in a leap year: divisible by 4,
  except centuries that are not divisible by 400), an hour above 23, a minute or second above 59, or a nanosecond
  above 999,999,999. The fields are public, so a value built by hand can be invalid; the derived ordering is
  chronological because the fields run from the year down to the nanosecond. `to_system_time` panics outside the
  range of `SystemTime`. `weekday` returns a `Weekday` and `ordinal` is 1 for January 1.
- **Formatting a date.** `Display` writes ISO 8601 in UTC: `2026-11-17T09:41:07Z`, with the fraction of a second
  after the seconds when it is not zero (`2023-11-14T22:13:20.12345Z`, no trailing zeros). `format(pattern)` expands
  `%Y` (the year, at least four digits, with a minus sign when negative), `%m`, `%d`, `%H`, `%M`, `%S` (two digits
  each), `%f` (nine digits of nanoseconds), `%j` (the day of the year, three digits), and `%%`. Any other text is
  copied, including an unknown code such as `%q` and a `%` at the end.
- **Determinism for tests.** Code that needs the time takes a `C: Clock` instead of calling `Instant.now()` or
  `SystemTime.now()`. `SystemClock` forwards to the operating system. `ManualClock` starts its monotonic clock at
  `Instant.from_start(Duration.zero())` and its real-time clock at the time given to `new`; `advance` moves both
  forward (and panics if a reading would not fit), and `set_system_time` moves only the real-time clock, to model the
  system clock being set. Everything else here (`Duration`, `DateTime`, the calendar functions, `ManualClock`) is
  pure.
- **Sleeping.** `sleep(d)` suspends the thread for about `d`. The system may round it up to its timer resolution
  (whole milliseconds on Windows), and on Unix a signal can end it early. A long sleep is made of sleeps of at most
  one hour.
- **Compile time.** `Instant.now`, `SystemTime.now`, `SystemClock`'s methods, and `sleep` reach the operating
  system, so a compile-time evaluation that reaches one fails with E0224 (`const-eval.md`, CE6). The rest is pure and
  works in a `const`.

Tests: `m24_stdlib/time_duration`, `time_instant`, `time_duration_underflow`, `time_duration_negative_float`, `time_calendar`,
`time_clock`, `time_const`, `time_const_impure`.

## Random numbers (issue #195)

```
fn random_seed(): u64                                   // 64 random bits from the operating system
struct Random: Clone, Eq

Random {
    fn seeded(seed: u64): Random                        fn from_entropy(): Random
    fn next_u64(mut self): u64                          fn next_u32(mut self): u32
    fn next_bool(mut self): bool                        fn next_f64(mut self): f64
    fn below(mut self, bound: u64): u64
    fn range(mut self, low: i64, high: i64): i64        fn range_f64(mut self, low: f64, high: f64): f64
    fn index(mut self, len: usize): Option<usize>
    fn shuffle<T>(mut self, mut items: List<T>)
}
```

- **The generator.** `Random` is xoshiro256** with its 256 bits of state filled from the seed by SplitMix64. It is
  written in Burdin and uses only wrapping integer arithmetic, so a seed gives the same numbers on every platform,
  in every build profile, and at compile time. Every seed works, including 0, because SplitMix64 never leaves the
  state all zero for the four draws it makes. `clone` copies the state, so a clone replays the same numbers. It is a
  statistical generator, not a cryptographic one: its output must not be used for keys or tokens.
- **The seed source.** `random_seed()` is the one impure function: 64 bits from `getentropy` on Unix and `rand_s` on
  Windows. If the system refuses, the bits come from the real-time clock, so a seed is never a constant. A test that
  needs determinism seeds with a constant (`Random.seeded(42)`); a program that wants a different sequence each run
  uses `Random.from_entropy()`, which is `Random.seeded(random_seed())`.
- **Draws.** `next_u64` is the generator's output and `next_u32` is its high 32 bits. `next_bool` is the high bit.
  `next_f64` is from 0.0 up to but not including 1.0, in steps of 2^-53. `below(bound)` is uniform over `0..bound`
  with no modulo bias (a draw that would favor the low values is discarded and drawn again) and panics when `bound`
  is zero. `range(low, high)` is uniform over `low..high` for any two `i64` values, including the whole `i64` range,
  and panics unless `low < high`. `range_f64` is `low + next_f64() * (high - low)` and panics unless `low < high`
  and the width is finite. `index(len)` is `Option.Some` of a uniform index below `len`, or `Option.None` for an empty collection.
- **Shuffling.** `shuffle` is Fisher-Yates over `0..len` using `below`, so every order is equally likely and the
  result is a function of the generator's state alone. An empty list and a one-element list are unchanged and
  consume no numbers.
- **Compile time.** `random_seed` and `from_entropy` fail with E0224 at compile time; `seeded` and every draw are
  pure and work there (`const-eval.md`, CE6).

Tests: `m24_stdlib/random_seeded` (known answers from an independent implementation), `random_properties`,
`random_entropy`, `random_below_zero`, `random_range_empty`, `time_const`, `time_const_impure`.

## Roundout tests (issue #196)

The sections above each name the tests that cover one type. Three more tests use the m24 types together, so a
change to one of them is also checked against the others:

- `containers_graph_search`: a `Deque` drives a breadth-first search, a `Set` marks what was visited, and a
  `PriorityHeap` of `Reverse` entries is the frontier of a shortest-path search, with the distances in a `Map`.
- `containers_const`: `Set`, `Deque`, and `PriorityHeap` are ordinary library code, so a `const` initializer
  can build and consume them; only a scalar, `String`, or composite of those leaves it (`const-eval.md`, CE3).
- `roundout_text_and_time`: Unicode case mapping and `split_whitespace` into a `Set`, `Path` edits, a `ManualClock`
  with `DateTime` formatting, and a seeded `Random` shuffle.

Every public function of `Set`, `Deque`, `PriorityHeap`, `Path`, the time types, `Random`, and the character
predicates (apart from the `DequeLen` and `HeapLen` helper interfaces the iterators use) is called by at least one test in `tests/`. The compile-fail cases are `set_needs_eq` and `heap_needs_ord`
(E0141, an element type that does not meet the container's bound) and `time_const_impure` (E0224, the clocks, sleep,
and the entropy source at compile time). There is no new diagnostic.

## `Heap<T>`

```
struct Heap<T, A = Global> {
    fn new(value: T): Heap<T, A>         // only when A is `Global`; moves the value in
    fn new_in(alloc: A, value: T): Heap<T, A>   // takes the allocator value to store
}
```

`Heap<T>` owns one allocation. `*h` on an owned `Heap<T>` moves the whole pointee out and frees the allocation;
reading or assigning through `*h` does not move. A field of the pointee (`(*h).a`) is a partial move tracked like a
field of a local, and dropping the `Heap` drops the remaining fields and frees the allocation (`memory-rules.md`,
MV6).

## `Range<T>` and `RangeInclusive<T>`

```
let r: Range<i32> = 0..4
let q: RangeInclusive<i32> = 0..=4
```

Both are built-in first-class types. `a..b` builds the exclusive form and `a..=b` the inclusive one; the endpoints
must be integers, with the type coming from context or defaulting to `i32`. A range is `Copy`, `Clone`, `Eq`,
`Ord`, and `Hash`, may be stored in a field or collection, and is iterable by `for i in r` (which evaluates `r`
once). `r.iter()` and `r.into_iter()` give a `RangeIter<T>` that yields the endpoints in order; the inclusive form
stops after the upper endpoint without computing `upper + 1`, so it does not overflow (`iterators.md`, IT11).

## Iteration

`std` declares `Iterator<T>` (`next`, plus the default methods `map`, `filter`, `take`, `skip`, `fold`, `count`,
`any`, `all`, `find`, `to_list`, and, from issue #708, `collect`; and, from issue #706, `enumerate`, `zip`, `chain`, `flat_map`, `take_while`,
`skip_while`, `map_while`, `step_by`, `inspect`, and `peekable`; and, from issue #707, `last`, `nth`, `position`,
`for_each`, `reduce`, `partition`, `min_by`, `max_by`, `min_by_key`, `max_by_key`, `try_fold`, and `try_for_each`), the
marker `Iterable<T>`, and `DoubleEndedIterator<T>` (`next_back`). The adapters that need a particular kind of source are
compiler-resolved functions on any iterator: `flatten`, `cloned`, `copied`, `cycle`, and `rev` (`iterators.md`, IT13,
IT14). The compiler implements both for arrays, slices, `List`, `Map`, and ranges, and adds `sum` and `product`
for iterators of integers or floats; `std` also holds the functions behind `min`, `max`, `eq`, and `cmp` between
iterators, which need a bound on the item type; `std` declares the adapter
types `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter`. A program implements the interfaces to make its own type
work with `for` and the adapters. The rules, the item types, and the second-class restrictions are in
[`iterators.md`](iterators.md).

### Mutable iteration (issue #709)

```
fn List<T>.iter_mut(mut self): IterMut<T>               // built in; yields &mut T
fn [T; N].iter_mut(mut self): IterMut<T>                // built in; the array must be a mutable place
fn Map<K, V>.iter_mut(mut self): MapIterMut<K, V>       // built in; yields (&K, &mut V)
fn Deque<T>.iter_mut(mut self): DequeIterMut<&mut Deque<T>, T>   // in std; yields &mut T
for x in mut xs { ... }                                  // the same as `for x in xs.iter_mut() { ... }`
```

The iterator borrows the collection exclusively, its items are `&mut T` (`*x = ...`), a map's keys stay `&K`, and the
adapters of `Iterator<T>` apply to it. A slice, a `Set`, and a `PriorityHeap` have no `iter_mut`. The rules, the
decisions, and the reasons are `iterators.md` IT15 and `memory-rules.md` IT15.

## `Ordering`, `cmp`, and `Reverse<T>` (issue #705)

`std.cmp` declares the three-way comparison. The prelude makes its names visible in every module, and `cmp.min(a, b)`
names the module.

```
enum Ordering: Copy, Clone, Eq, Ord, Hash, Debug { Less  Equal  Greater }

fn Ordering.reverse(self): Ordering
fn Ordering.then(self, other: Ordering): Ordering
fn Ordering.then_with<F: FnOnce(): Ordering>(self, f: F): Ordering
fn Ordering.is_lt(self): bool      is_le   is_eq   is_ne   is_gt   is_ge

fn cmp.min<T: Ord>(a: T, b: T): T                      // `a` when they are equal
fn cmp.max<T: Ord>(a: T, b: T): T                      // `b` when they are equal
fn cmp.clamp<T: Ord>(value: T, low: T, high: T): T     // panics when `low > high`
fn cmp.min_by<T, F: FnMut(&T, &T): Ordering>(a: T, b: T, compare: F): T
fn cmp.max_by<T, F: FnMut(&T, &T): Ordering>(a: T, b: T, compare: F): T
fn cmp.min_by_key<T, K: Ord, F: FnMut(&T): K>(a: T, b: T, key: F): T
fn cmp.max_by_key<T, K: Ord, F: FnMut(&T): K>(a: T, b: T, key: F): T

struct Reverse<T>: Copy, Clone, Eq, Debug { value: T }   // `<` is `T`'s `>`; `Ord` is written by hand
```

`a.cmp(b): Ordering` is provided by the compiler for every type that satisfies `Ord`: the integers, `char`, `bool`,
`str`, `String`, `()`, tuples, fixed-size arrays, and ranges of ordered parts, a struct or enum that derives `Ord`, a type parameter with
an `Ord` bound, and `Reverse<T>`. It calls the private `std.cmp.ord_cmp`, which is `if a < b { Less } else if b < a
{ Greater } else { Equal }`. `Option<T>` and `Result<T, E>` get `cmp` from extension methods in `std.cmp` (`Option.None`
before every `Option.Some`; every `Result.Ok` before every `Result.Err`; contents compared when the kinds match): they are not `Ord`, so
`<` still does not apply to them. Floats have `<` but no total order, so `f64.cmp` is E0002 (`total_cmp` is the float
order, issue #703). The decisions behind it:

- **`cmp` is not an interface method.** The `Ord` interface keeps `lt`/`le`/`gt`/`ge`, so a derive
  generates nothing new: `cmp` is `<` read twice.
- **`Eq` and `Ord` are each one name for the derive and the interface.** A type that implements `Ord` (its `lt` is
  the order) is accepted wherever `T: Ord` is required, which is how `Reverse<T>` works with `cmp.min`,
  `max_by_key`, `PriorityHeap`, and `sort`; a type that implements `Eq` is accepted wherever `T: Eq` is. The compiler-implemented `List.sort`, `contains`, `dedup`, and the
  `Map`/`Set` lookups call the type's `lt` and `eq` (`derives.md`, issue #918); a hand-written-`eq` key must also
  derive `Hash`. Hand-written `Eq`/`Ord`/`Hash` stay E9000 under #402 (`Hash`: HS5).
- A closure comparator or key function sees views of the values (`&T`), so the helpers return an owned `T`: `min`
  and friends cannot be called on two `str` views (E0121, a view cannot come from two parameters); use `String`.

Tests: `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`,
`reverse`, `cmp_float_has_no_cmp`, `cmp_needs_ord`.

## Hashing (issue #717)

`std.hash` gives a program the hash that `Map` uses, so it can build a cache key, a content hash, a bloom filter, or
a hand-written hash table. The prelude makes its names visible in every module, and `hash` also names the module.

```
x.hash(): u64                              // every type that is `Hash`; built in (derives.md)
fn x.hash_from(state: u64): u64            // built in: folds `x` into a running state

struct Hasher: Copy, Clone, Eq, Debug     // one `u64` of state
fn Hasher.new(): Hasher                    // the FNV-1a offset basis
fn Hasher.with_seed(seed: u64): Hasher     // `new()` followed by `write_u64(seed)`
fn Hasher.write<T: Hash>(mut self, value: T)   // folds a whole value, as `value.hash()` would
fn Hasher.write_u8(mut self, x: u8)        fn write_u32(mut self, x: u32)      fn write_u64(mut self, x: u64)
fn Hasher.write_bytes(mut self, bytes: [u8])    // one word per byte, in order
fn Hasher.write_str(mut self, text: str)   // the text's UTF-8 bytes, like `write_bytes`
fn Hasher.finish(self): u64                // the state; `finish` does not consume or reset the hasher

fn hash_of<T: Hash>(value: T): u64                          // `value.hash()`
fn hash_seeded<T: Hash>(seed: u64, value: T): u64           // `Hasher.with_seed(seed)`, `write(value)`, `finish()`
```

**HS1. One stable algorithm.** There is one hash, the FNV-1a fold of `derives.md`, and it is stable: the same value
gives the same `u64` on every run, every platform, and every build profile, in a `const` and at run time. It does not
change between compiler versions without a breaking-change entry in `CHANGELOG.md`, so a program may store it (an
on-disk cache key, a content hash). `Map` and `Set` hash their keys with this same fold; the order they iterate in is
still unspecified, and a program must not depend on it (the table's layout is not part of the contract, only the
fold is).

**HS2. What is `Hash`.** Every type whose `Hash` cell is `yes` in the interface-coverage table (issue #716, below),
and a type parameter bounded by `Hash`. That includes `str` and `String`, which hash their UTF-8 bytes one word per
byte with no length prefix and no terminator, so a `str` and a `String` with the same text hash alike, and `List` and
`Map`, which `derives.md` folds as a length followed by their elements (a map's entries combine so that insertion order
does not matter). A float is not `Hash`, because `==` on floats is not an equivalence (`NaN != NaN`); a struct with a
float field cannot derive `Hash` (E0130), and a `Hash` bound rejects an `f64` argument (E0141). `Map` still accepts a
float key and hashes it structurally; that rule is `Map`'s, not `Hash`'s.

**HS3. `Hasher` is the same fold, one part at a time.** `Hasher.new()` holds the offset basis, and every `write_*`
call folds one more part into the state with `state = (state XOR w) * 0x100000001b3` (mod 2^64), where `w` is the
part as a 64-bit word. `write_u8`, `write_u32`, and `write_u64` fold their argument zero-extended; `write_bytes` and
`write_str` fold one word per byte; `write(value)` folds the value's parts exactly as `value.hash()` does, but
starting from the hasher's current state. So a fresh hasher that writes one value finishes with that value's hash
(`h.write(x)` then `h.finish()` is `x.hash()`), and `x.hash_from(h.finish())` continues a hash without a `Hasher`.
Writes concatenate with no separators: `write_str("ab")` then `write_str("c")` equals `write_str("abc")`. A program
that needs the boundary to matter writes the length first.

**HS4. Seeded hashing is a different sequence, not a keyed one.** `Hasher.with_seed(seed)` folds `seed` before
anything else, so two seeds give two unrelated-looking hash sequences for the same data (the way a bloom filter gets
several hash functions, or a table partitions keys). It is deterministic for a seed, and the seed is the caller's to
choose; `std` has no source of random numbers yet. FNV-1a is not a keyed function: a seed does not protect a table
from a caller who can choose the data. Neither `Hasher` nor `Map` offers collision-attack resistance, and a
checksum or cryptographic hash is issue #560's, built on `Hasher`.

**HS5. A type cannot customize its hash.** Equality and hash are structural for every type, and the only `hash` a
type has is the derived one. A hand-written `Type: Hash { ... }`, like every hand-written built-in interface, is
`E9000` under issue #402, and so is the case-insensitive key it would be needed for; `Map` and `Set` use the one
structural rule for every key, so they and `hash()` cannot disagree. Until #402 lands, a program that needs another
notion of equality keys its table on a normalized value (the lower-cased text), or hashes the part it wants with a
`Hasher` itself. When #402 allows a hand-written `Hash`, it must come with a hand-written `Eq`, `Map` and `Set` must
call the type's own `hash` and `==`, and `a == b` must imply `a.hash() == b.hash()`; that rule is written for #402
and is not implemented by this section.

The primitive behind `Hasher.write` is `x.hash_from(state)`: it is declared for exactly the types `x.hash()` is, it
evaluates `x` once, and `x.hash()` is `x.hash_from(0xcbf29ce484222325)`.

Tests: `m24_stdlib/hash_values` (HS1, HS2), `hash_hasher` (HS3), `hash_seeded` (HS4), `hash_generic` (HS2, bounds),
`hash_map_consistent` (HS1), `hash_case_insensitive` (HS5, workaround), `hash_const` (HS1), `hash_float`
(HS2, E0141), `hash_float_derive` (HS2, E0130).

## `Option<T>` and `Result<T, E>`

`Option<T>` is built in with the variants `Option.Some(T)` and `Option.None`; `Option.None` takes its type from context. `Result<T, E>`
is built in with `Result.Ok(T)` and `Result.Err(E)`. `Result<T>` written with one argument means `Result<T, AnyError>`, the dynamic
error type described below (`errors.md`, ERR5).

```
Option<T>:  unwrap(), expect(msg: str): T, is_some(): bool, is_none(): bool
Result<T, E>: unwrap(), expect(msg: str): T, is_ok(): bool, is_err(): bool
Result<T, E>: context(message: str): Result<T, AnyError>
```

`unwrap` panics on `Option.None`/`Result.Err`; `expect(msg)` panics with `msg`. `expr?` returns early through the enclosing
`Option` or `Result` function (E0160 outside one) and converts the error through a user `Type: From<Source>` block;
in a function returning `Result<U>` / `Result<U, AnyError>`, any error type is boxed into the dynamic `AnyError`
(`errors.md`, ERR4). `expr orelse fallback` supplies a value for `Option.None` or `Result.Err`; a closure handler receives the
`Result.Err` payload (`|e| ...`) and takes no argument for `Option.None` (`|| ...`, issue #488). `match` handles both types; `Option` also supports `==`/`!=` when its
payload does, and `Hash` when its payload does. `.context(message)` prefixes a context message and returns
`Result<T, AnyError>`; it is E0212 when the error type does not implement `Error` (`errors.md`, ERR6). The combinators of the next section are
stage 1 (issue #481).

### Sorting with a comparator (issue #418)

`List.sort` (compiler-implemented) sorts ascending and needs a derived `Ord`. `std.sort` declares extension methods on
`List<T>` that sort with any comparison. The algorithm is a top-down merge sort that moves elements with `push` and
`pop` only, so nothing is cloned and each element is dropped exactly once, and it is stable: elements that compare
equal keep their relative order. The `_unstable` names promise nothing more and use the same algorithm.

```
fn List<T>.sort_by<F: FnMut(&T, &T): Ordering>(mut self, f: F)
fn List<T>.sort_by_key<K: Ord, F: FnMut(&T): K>(mut self, key: F)
fn List<T>.sort_unstable_by<F: FnMut(&T, &T): Ordering>(mut self, f: F)
fn List<T>.sort_unstable_by_key<K: Ord, F: FnMut(&T): K>(mut self, key: F)
fn List<T>.is_sorted_by<F: FnMut(&T, &T): Ordering>(self, f: F): bool
```

The comparator receives two views; write `a.cmp(*b)` for whole elements, or compare fields (`a.age.cmp(b.age)`). A
comparator that is not a total order (it says both `a < b` and `b < a`, or is not transitive) cannot lose, duplicate, or
corrupt an element and cannot panic or loop: the result is some permutation of the input, in an unspecified order.

Tests: `m24_stdlib/sort_by`, `sort_by_key`, `sort_unstable`, `sort_inconsistent_comparator`, `sort_drops`.

### `Option` and `Result` combinators (issue #481)

`std.option` declares them as extension methods (`extensions.md`), which the prelude makes visible in every module;
the methods the compiler provides (`unwrap`, `expect`, `is_some`, `is_none`, `is_ok`, `is_err`, `context`) win.
`own self` consumes the value, so `Option<String>.map(...)` moves the payload into the closure; `is_some_and`,
`is_none_or`, `is_ok_and`, and `is_err_and` borrow the receiver and give the closure a view (`&T`); `take` and
`replace` change the receiver in place.

```
Option<T>:     map, and_then, or_else, or, and, xor, zip, unwrap_or, unwrap_or_else, unwrap_or_default, map_or, map_or_else,
               ok_or, ok_or_else, filter, is_some_and, is_none_or, take, replace
Option<Option<T>>: flatten
Result<T, E>:  map, map_err, and_then, or_else, unwrap_or, unwrap_or_else, map_or, ok, err, unwrap_err,
               is_ok_and, is_err_and
```

The error-conversion helpers are `map_err` (apply a conversion to the error), `ok_or`/`ok_or_else` (an absent value
becomes an error), `ok`/`err` (drop one side), and the `?` operator, which keeps converting through `From`. `Option<T>.unwrap_or_default()` (for a `T` that is `Default`; `T.default()` is the built-in default of `derives.md`) and
`Option<Option<T>>.flatten()` are provided (issue #930): an extension receiver may nest a type parameter, and
`Option<Option<T>>` is one. `Option.as_ref` is **not provided**, and the reason is a compiler bug, not a limit of
extensions. A binding of a borrowed `match` scrutinee is a read-only place (MV8), but the code generator copies its value
into the binding's own slot, so `Option.Some(&v)` in `fn Option<T>.as_ref(self): Option<&T>` passes the return-origin check
(RT2) and returns a pointer into that slot, which is dead when the function returns. A plain function with the same body,
`fn f<T>(o: Option<T>): Option<&T> { match o { Option.Some(v) => Option.Some(&v), Option.None => Option.None } }`, returns garbage too. `as_ref`
stays out until a view of a binding aliases the scrutinee's part; then it is the extension over `Option.Some(v) => Option.Some(&v)`.
A closure whose result mentions a type parameter that only the closure fixes
(`and_then(|x| Option.Some(x))`, `F: FnOnce(T): Option<U>`) is typed from its body; a body that cannot say every part of the
type by itself (`or_else(|e| Result.Err(e.len()))`, which leaves the `Result.Ok` type open) needs a named function or a typed
binding.

Tests: `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators`, `unwrap_err_on_ok`,
`option_wrong_default`, `m12_closures/closure_returns_generic_wrapper`.

### `List` convenience methods (issue #483)

`std.list` declares these as extension methods on `List<T>` (a list with the default allocator; `extensions.md`), so
the prelude makes them visible everywhere and the compiler's own methods (`push`, `pop`, `sort`, `contains`, `map`,
`filter`, `find`, `get`, `first`, `drain`, `truncate`, `swap`, ...) win. They move elements with `pop`/`push`, so
nothing is cloned unless the name says so, and every element is dropped exactly once.

```
fn List<T>.retain<F: FnMut(&T): bool>(mut self, f: F)      // keeps the accepted elements, in order
fn List<T>.rposition<F: FnMut(&T): bool>(self, f: F): Option<usize>
fn List<T>.reverse(mut self)
fn List<T>.dedup(mut self) where T: Eq                      // removes consecutive duplicates
fn List<T>.dedup_by_key<K: Eq, F: FnMut(&T): K>(mut self, own key: F)   // removes a run whose key repeats
fn List<T>.extend(mut self, own other: List<T>)             // moves the elements of `other`
fn List<T, A>.extend_iter<I: Iterator<T>>(mut self, iter: I) where A: Allocator   // issue #708
fn List<T>.extend_from(mut self, other: List<T>) where T: Clone
fn List<T>.resize(mut self, new_len: usize, own value: T) where T: Clone
```

`is_empty` is a method the compiler provides, so it exists for a list with any allocator argument, and so are
`truncate` and `swap`; the extension list above leaves them out. `position` and `find` come from the `Iterable`
protocol (`iterators.md`, IT10). `dedup_by_key` removes an element whose key `==` the key of the element before it (issue #930).
Its key type `K` is a parameter of the method, as `sort_by_key`'s is, so the closure's result fixes it; a key type that
does not support `==` (a struct that does not derive `Eq`) is E0141.

Tests: `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_dedup_by_key`, `list_dedup_by_key_needs_eq`, `list_position`, `list_extend_resize`, `list_drops`,
`list_swap_out_of_bounds`.

## Collecting and extending (issue #708)

`std.iter` declares `FromIterator<T>` and `Iterator<T>.collect`; every collection takes the items of an iterator with
`extend_iter` and an array with `from`. The rules, with IDs, are `iterators.md` IT16-IT22, and the decisions are:

- **Naming the target.** `it.collect` takes its target in one of two ways: the explicit type argument, `it.collect<Set<i32>>()`
  (`arguments.md`, TA5; `iterators.md`, IT17), or the **expected type**: `let s: Set<i32> = it.collect()`, a parameter, a
  return type, a field. With neither it is E0003 ("cannot tell what `C` stands for"); a target that is not `FromIterator`
  for the items is E0141, and an explicit list of another length is E0003. A type can also be named on the call itself,
  `Set<i32>.from_iter(it)`. No other built-in method takes explicit type arguments.
- **The protocol.** `FromIterator<T>` has one method, `from_iter<I: Iterator<T>>(own iter: I): Self`. A user type
  implements it (IT16); `Set`, `Deque`, and `PriorityHeap` do in `std`; `List`, `Map`, `String`, `Option`, and
  `Result` do in the compiler (IT18, IT19).
- **Allocators.** `collect` and `from_iter` build with the default allocator; for another allocator make the container
  with `new_in` and call `extend_iter` (IT22). There is no `collect_in`.
- **Extending.** `extend_iter` takes any `Iterator<T>`; the older `extend(own List<T>)` is unchanged (IT20).

```
interface FromIterator<T> { fn from_iter<I: Iterator<T>>(own iter: I): Self }
fn Iterator<T>.collect<C: FromIterator<T>>(own self): C

List<T>.from_iter(it)       Map<K, V>.from_iter(it)       String.from_iter(it)
Option<C>.from_iter(it)     Result<C, E>.from_iter(it)    Set<T>.from_iter(it)
Deque<T>.from_iter(it)      PriorityHeap<T>.from_iter(it)

List<T>.from(array)         Map<K, V>.from(array of pairs)      // also `List.from(...)` and `Map.from(...)`
Set<T>.from(array)          Deque<T>.from(array)                PriorityHeap<T>.from(array)

fn List<T, A>.extend_iter<I: Iterator<T>>(mut self, iter: I)
fn Map<K, V, A>.extend_iter<I: Iterator<(K, V)>>(mut self, iter: I) where K: Eq
fn String.extend_iter<I: Iterator<char>>(mut self, iter: I)
Set<T, A>.extend_iter    Deque<T>.extend_iter    PriorityHeap<T>.extend_iter     // iter: I where I: Iterator<T>
```

`String.from_iter` accepts items of `char`, `str`, `String`, and `&String`; `String.extend_iter` takes `char` items, because
an extension cannot pick its body by item type. A `Map` built from pairs keeps the last value of a repeated key.

Tests: `m24_stdlib/collect_basic`, `collect_adapters`, `collect_result_option`, `collect_drops`, `collect_user_type`,
`collect_allocator`, `collect_const`, `collect_views`, `extend_iter`, `from_array`, and the compile-fail cases
`collect_needs_target` (E0003), `collect_not_from_iterator` (E0141), `collect_allocator_target` (E0272, E0141),
`collect_map_needs_eq` (E0141), `collect_view_target` (E0141), and `from_array_view_key` (E0120). The explicit target
(`it.collect<C>()`) is `m24_stdlib/collect_explicit` (runs), with the refusals in `collect_explicit_errors` (E0003 for
the arity and a closure value, E0141 for a target that is not `FromIterator`).

## Errors

`std` declares the marker interface `Error requires Display { }` and the dynamic error value `AnyError`, a struct of its own name:

```
struct AnyError {
    value: any Error
    frames: List<TraceFrame>
    context: String
}

struct TraceFrame {
    func: String
    file: String
    line: i32
    col: i32
}
```

A type is an error type when it implements `Error`, which requires `Display`. The dynamic `AnyError` is an owned
`any Error` box together with the context messages `.context` added (outermost first) and the frames a debug build
records. An error type is boxed implicitly wherever `AnyError` is expected; `e.is<T>()` reports whether the box holds
a `T`, `e.downcast<T>()` consumes the box and yields `Option.Some(value)` on a match (both need `T: Error`), `{}`
prints the wrapped error's `Display` (`errors.md`, ERR4), and `e.print_trace()` prints the recorded frames, one
`at {func} ({file}:{line}:{col})` line each, to standard error (`debug.md`, DBG3; nothing when there are none, which
is every release build). `TraceFrame` implements `Display` as `at {func} ({file}:{line}:{col})`, and
`error.frames` is its public `List<TraceFrame>`; a debug build records a creation frame at each conversion into
`AnyError` and a propagation frame at each `?` that passes a `Result<..., AnyError>` outward, each naming the function
the error passed through, and a release build records none (`errors.md`, ERR7).

`std` declares its error types (`errors.md`, ERR10-ERR11):

```
enum IoErrorKind: Copy, Clone, Eq, Debug {
    NotFound  PermissionDenied  AlreadyExists  IsADirectory  NotADirectory  DirectoryNotEmpty
    InvalidInput  InvalidData  UnexpectedEof  Interrupted  BrokenPipe  WouldBlock  TimedOut
    ConnectionRefused  ConnectionReset  ConnectionAborted  NotConnected
    AddrInUse  AddrNotAvailable  NetworkUnreachable  HostUnreachable  Other
}
IoErrorKind: Display { fn fmt(self, mut f: Formatter) }   // "not found", "permission denied", ...

error enum IoError {
    Read { path: String, kind: IoErrorKind, os_code: Option<i32> }      "cannot read {path}: {kind}"
    Write { path: String, kind: IoErrorKind, os_code: Option<i32> }     "cannot write {path}: {kind}"
    Run { command: String, kind: IoErrorKind, os_code: Option<i32> }    "cannot run {command}: {kind}"
    Net { op: String, addr: String, kind: IoErrorKind, os_code: Option<i32> } "cannot {op} {addr}: {kind}"
    UnexpectedEof            "unexpected end of input"
    WriteZero                "the writer accepted no bytes"
    InvalidData              "the bytes are not valid text"
    Seek                     "seek to a position before the start"
    InvalidInput             "invalid open options"
}
IoError {
    fn kind(self): IoErrorKind
    fn os_code(self): Option<i32>
}

error enum ParseIntError {
    Invalid  "invalid integer"
}
```

`read_to_string` returns `IoError.Read` on failure, `write_file` returns `IoError.Write`, `process.run` returns
`IoError.Run`, a socket operation returns `IoError.Net`, and an integer `parse` returns `ParseIntError.Invalid`. The kind is portable and the OS code is the
platform's own (`errno`, or the Win32 error code); the message ends with the kind's text, so a missing file prints
`cannot read missing.txt: not found`. The readers and writers return `UnexpectedEof` (`read_exact` on a source that
ended), `WriteZero` (`write_all` on a sink that takes nothing), `InvalidData` (text that is not UTF-8), and `Seek`
(a position before the start); `OpenOptions.open` returns `InvalidInput` for an option set it rejects ("Files and
file handles"). Those five are failures `std` observed itself, so they carry no OS code; `kind()` still classifies
them (`errors.md`, ERR11).

An `error enum` declaration lowers to an ordinary enum plus generated `Display` (from each variant's message
template), `Error`, and `From<Source>` impls, so an error value is an ordinary value (`errors.md`, ERR1-ERR3). A
function written `fn main(): Result<T>` prints an `Result.Err` with `Display` to standard error and exits 1; `Result.Ok` prints
nothing and exits 0 (`errors.md`, ERR8).

## Networking (issue #220)

```
struct SocketAddr: Copy, Clone, Eq, Debug {
    priv ip: u32       // a.b.c.d, a in the highest byte
    priv port: u16
}
SocketAddr {
    fn new(a: u8, b: u8, c: u8, d: u8, port: u16): SocketAddr
    fn ip(self): u32
    fn port(self): u16
    fn octets(self): (u8, u8, u8, u8)
    fn to_text(self): String                       // "a.b.c.d:port"
    fn parse(text: str): Result<SocketAddr, IoError>   // numeric only
}
SocketAddr: Display

struct TcpListener {
    fn bind(addr: str): Result<TcpListener, IoError>
    fn accept(self): Result<(TcpStream, SocketAddr), IoError>
    fn local_addr(self): Result<SocketAddr, IoError>
    fn close(own self): Result<(), IoError>
}

struct TcpStream {
    fn connect(addr: str): Result<TcpStream, IoError>
    fn connect_timeout(addr: str, timeout: Duration): Result<TcpStream, IoError>   // TimedOut after `timeout`
    fn peer_addr(self): Result<SocketAddr, IoError>
    fn local_addr(self): Result<SocketAddr, IoError>
    fn set_read_timeout(mut self, Option<Duration>)
    fn set_write_timeout(mut self, Option<Duration>)
    fn set_nodelay(self, on: bool): Result<(), IoError>
    fn shutdown_read(mut self): Result<(), IoError>
    fn shutdown_write(mut self): Result<(), IoError>
    fn close(own self): Result<(), IoError>
}
TcpStream: Read      // read, read_exact, read_to_end, read_to_string
TcpStream: Write     // write, flush, write_all, write_text, write_line

struct UdpSocket {
    fn bind(addr: str): Result<UdpSocket, IoError>
    fn recv_from(mut self, mut buf: List<u8>, max: usize): Result<(usize, SocketAddr), IoError>
    fn send_to(self, data: [u8], addr: str): Result<usize, IoError>
    fn local_addr(self): Result<SocketAddr, IoError>
    fn set_read_timeout(mut self, Option<Duration>)
    fn close(own self): Result<(), IoError>
}

fn lookup_host(host: str, port: u16): Result<List<SocketAddr>, IoError>
```

An address is `host:port`; port 0 asks the system for a free port, which `local_addr` reports. The sockets are
IPv4 only. Every call looks blocking and parks the calling task (or blocks the calling thread); the reactor and the
helper pool that make that work are `spec/io.md` IO1-IO8. A socket is `Send` and `Sync` because its state is in the
kernel and its value is a descriptor; `TcpStream` and `UdpSocket` implement `Read`/`Write` where the interface
applies. A failure is `IoError.Net { op, addr, kind, os_code }`, with the network kinds of ERR11.

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
returned as is. On failure each returns the matching `IoError` variant with its kind and OS code, and an integer
`parse` returns `ParseIntError.Invalid` (`errors.md`, ERR10-ERR11). The byte-level helpers (`read`, `write`,
`append`, `copy_file`, `exists`) and the owning `File` handle are in "Files and file handles".

None of these runs at compile time: a compile-time evaluation that reaches one fails with E0224 (`const-eval.md`, CE6).

### Environment variables and the working directory (issue #412)

```
error enum EnvError {
    Name { name: String }        "invalid environment variable name {name}"
    Value { name: String }       "invalid value for environment variable {name}"
    Access { name: String }      "cannot access environment variable {name}"
    WorkingDirectory             "cannot read the working directory"
    SetWorkingDirectory          "cannot set the working directory"
}

fn env.get(name: str): Option<String>              fn env.get_bytes(name: str): Option<Bytes>
fn env.set(name: str, value: str): Result<(), EnvError>
fn env.set_bytes(name: str, value: [u8]): Result<(), EnvError>
fn env.remove(name: str): Result<(), EnvError>
fn env.current_dir(): Result<String, EnvError>     fn env.current_dir_bytes(): Result<Bytes, EnvError>
fn env.set_current_dir(path: str): Result<(), EnvError>
fn env.set_current_dir_bytes(path: [u8]): Result<(), EnvError>
```

**Encoding.** A name is valid when it is not empty and contains neither `=` nor a NUL byte; a value may be any bytes
except NUL. On Unix an environment value and a path are byte strings: `get` and `current_dir` report `Option.None` and
`WorkingDirectory` when the bytes are not valid UTF-8, and the `_bytes` functions return them exactly. Windows stores
the environment and the working directory as UTF-16, so the primitives convert to and from UTF-8; a value that is not
valid Unicode (an unpaired surrogate) is reported as absent and cannot be set, and the byte and text functions agree.

**Behavior.** `get` is `Option.None` when the variable is unset, while an empty value is `Option.Some("")` because the variable is
present. `set` replaces an existing value, and `remove` succeeds when the variable is unset. `set` and `remove` report
`Name` for an invalid name, `Value` for an invalid value, and `Access` when the system refuses. `current_dir` has no
trailing separator except at a root, and `set_current_dir` reports `SetWorkingDirectory` when the path is not a
directory. A change is visible to a child started with `process.run`.

None of these runs at compile time: a compile-time evaluation that reaches one fails with E0224 (`const-eval.md`, CE6).

Tests: `m24_stdlib/env_vars`, `env_bytes_unix`, `env_bytes_windows`, `env_cwd`, `env_child_unix`, `env_bad_calls`,
`env_test_mode`, `env_allocations`.

### Child processes (issue #419)

`Command` starts a program directly: the program and each argument reach the operating system as separate words,
with no shell in between, so nothing in an argument is interpreted.

```
enum Stdio { Inherit  Piped  Null }

Command {
    fn new(program: str): Command
    fn arg(mut self, value: str)
    fn args(mut self, own values: List<String>)
    fn env(mut self, key: str, value: str)
    fn env_remove(mut self, key: str)
    fn env_clear(mut self)
    fn cwd(mut self, directory: str)
    fn stdin(mut self, mode: Stdio)
    fn stdout(mut self, mode: Stdio)
    fn stderr(mut self, mode: Stdio)
    fn spawn(own self): Result<Child, IoError>
    fn status(own self): Result<ExitStatus, IoError>
    fn output(own self): Result<Output, IoError>
}

Child {
    fn id(self): i32
    fn stdin(mut self): Option<ChildStdin>
    fn stdout(mut self): Option<ChildStdout>
    fn stderr(mut self): Option<ChildStderr>
    fn wait(mut self): Result<ExitStatus, IoError>
    fn wait_with_output(mut self): Result<Output, IoError>
}

ChildStdin  { fn write(self, text: str): Result<(), IoError> }
ChildStdout {
    fn read_to_string(self): Result<String, IoError>
    fn read_line(self): Result<Option<String>, IoError>
}
ChildStderr { ... the same two readers ... }

struct ExitStatus { fn code(self): Option<i32>   fn signal(self): Option<i32>   fn success(self): bool }
struct Output { status: ExitStatus   stdout: String   stderr: String }
```

- **Starting.** `Command.new` names the program; `arg` and `args` add arguments, in order. `spawn` starts the child
  and returns its handle, `status` starts it, waits, and returns the exit status, and `output` pipes standard output
  and standard error, gives the child a null standard input, reads both streams to the end, waits, and returns them
  with the status. Nothing runs until one of those three is called. On Unix the program is executed with `execvp`;
  on Windows `CreateProcessW` receives the quoted command line the C runtime parses. An argument keeps any spaces,
  quotes, or shell metacharacters.
- **Streams.** Each standard stream is `Stdio.Inherit` (the default), `Stdio.Piped` (the parent gets a
  `ChildStdin`/`ChildStdout`/`ChildStderr`), or `Stdio.Null` (the system's null device). `output` pipes standard
  output and standard error and gives the child a null standard input. `stdin`, `stdout`, and `stderr` on
  a `Child` hand out the piped handle once and return `Option.None` afterwards.
- **Environment.** The child inherits the parent's environment unless `env_clear` empties it. `env` sets an entry,
  replacing an earlier value for the name, and `env_remove` removes one. Windows compares names without regard to
  case; Unix compares them exactly. The entry is written `key=value`, and an embedded NUL in the program, an
  argument, an environment key or value, or the working directory makes the spawn fail.
- **Working directory.** `cwd` selects it; otherwise the child uses the parent's.
- **Waits.** `wait` closes the child's standard input first, so a program reading it sees the end of the input
  instead of waiting, and then waits for the child. A child that exited normally has `code()` (an `i32`; Windows
  reports a code for every child) and `success()` is `code() == Option.Some(0)`; a child ended by a signal has
  `signal()` (Unix only) and no code. Waiting for the same child twice is an error. `wait_with_output` closes the
  child's standard input, reads the piped output streams together, so a child writing a lot to one of them cannot
  block the other, and then waits; a stream that was not piped is empty in the result.
- **Pipes.** A write to `ChildStdin` is unbuffered and writes every byte. `read_to_string` reads to the end of the
  stream; `read_line` returns one line without its terminator (`\n`, or the `\r` of a `\r\n`), `Result.Ok(Option.None)` at the
  end, and the last line may have no terminator. A handle closes when it is dropped; dropping the `ChildStdin`
  without writing is how the parent signals the end of the input. A `Child` closes its process handle when it is
  dropped, but it does not wait for the child.
- **Errors.** A spawn failure — a program that does not exist, an argument or environment entry the system cannot
  take, or a pipe or process that cannot be created — is `IoError.Run` with the program name. A pipe read failure is
  `IoError.Read` with `<stdout>`, `<stderr>`, or `<child>`, and a write failure is `IoError.Write` with `<stdin>`.
  A program name without a separator is looked up on `PATH`: on Unix the search uses the child's environment, while
  on Windows `CreateProcessW` searches the calling process's `PATH`.
- None of these runs at compile time: a compile-time evaluation that reaches one fails with E0224 (`const-eval.md`, CE6).

Tests: `m24_stdlib/process_command`, `process_pipes`, `process_windows`, `process_bad_calls`, `process_test_mode`,
and `m20_const/purity_process`.

## The compilation target

```
enum target.Os: Copy, Clone, Eq { Linux  MacOs  Windows }
enum target.Arch: Copy, Clone, Eq { X86_64  Aarch64 }
fn target.os(): target.Os                   // the operating system the program is compiled for
fn target.arch(): target.Arch               // the processor architecture the program is compiled for
```

The compiler builds for the machine it runs on, so both are the host's. They are ordinary functions, usable at run
time and at compile time, which is how `if target.os() == target.Os.Windows { ... }` (a compile-time condition) selects a platform
branch (`const-eval.md`, CE4). `tests/m20_const/const_if_platform.bd`, `const_target_values.bd`.

## Memory helpers and assertions

```
fn mem.swap(mut a: T, mut b: T)
fn mem.replace(mut place: T, own value: T): T   // moves the value in; returns the old one
fn mem.take<T: Default>(mut place: T): T        // stores T.default() in the place and returns the old value
fn mem.drop<T>(own value: T)                    // ends the value's life at the call
fn Type.size(): usize                           // the C layout of a type (also `Type.align()`)
fn panic(msg: str): !
fn assert(cond: bool)
fn assert_eq<T: Eq>(a: T, b: T)
fn assert_ne<T: Eq>(a: T, b: T)
fn assert_snapshot<T: Debug>(value: T)          // also (value, @"inline"), and display: true for Display
fn unreachable(): !
fn todo(): !
fn unimplemented(): !
```

`swap`, `replace`, and `take` take places and are checked for exclusivity like any other call. `take` needs
`T: Default` and evaluates to the value that was in the place. `drop` hands the value to a function that drops it at
once, which is the explicit spelling of an early drop; using the value afterwards is E0100. `size` and `align` are
the layout queries, so `mem` adds no `size_of`/`align_of` aliases (issue #715).

`panic` prints its message and location and aborts (`memory-rules.md`, DR10); `unreachable()` panics with
`unreachable`; `todo()` and `unimplemented()` panic with `not yet implemented` and `not implemented`; `assert` panics
with `assertion failed`; `assert_eq` panics with `assertion failed: the two values differ` when the values differ, and
`assert_ne` with `assertion failed: the two values are equal` when they are equal.

**Every assertion has a message form** (`assert(cond, "text {x}")`, `assert_eq(a, b, "text {x}")`,
`assert_ne(a, b, "text {x}")`): the arguments after the condition (or the two values) are formatted exactly as
`println`'s are (`format.md`, FM1-FM5), and a failing assertion panics with that text instead of the fixed one. The
message is evaluated only when the assertion fails. A message whose format string is not a literal is E0194.

**`assert_snapshot`** records the value's `Debug` text (its `Display` text with `display: true`) for `burdin test`,
which compares it with a snapshot kept in a file or, with a second argument `@"..."`, in the call itself; it never
panics and returns `()`. Outside `burdin test` it only formats the value. The rules are
[`test-runner.md`](test-runner.md), SN1-SN9.

**`debug_assert`, `debug_assert_eq`, and `debug_assert_ne`** behave like the assertions above in a debug build
(`burdin build`/`run`/`test` without `--release`). A release build drops the call entirely: neither the condition nor
the values nor the message are evaluated, so side effects in them never run. They are the only assertions with this
behavior, and they keep it in a `burdin test` build (a failure is a panic, which the runner reports as a failed test).

In a `burdin test` build the prelude replaces `assert`, `assert_eq`, and `assert_ne` with `test_assert`,
`test_assert_eq`, and `test_assert_ne`, which return `Result<(), AssertError>` instead of panicking: a failure is
one of `AssertError.Failed`, `FailedMsg`, `FailedEq`, `FailedEqMsg`, `FailedNe`, or `FailedNeMsg`, where `condition`
is the call's source text, `left` and `right` are its values formatted with `Debug`, `message` is the formatted
message (only present when the call wrote one), and `location` is its `path:line:col` (`errors.md`, ERR9). A normal
build keeps the panicking assertions.

## The optimizer barrier

```
fn black_box(value: T): T   // the same value, through a barrier the optimizer cannot see through
```

`black_box` moves its argument in and the same value back out, with no observable effect: it exists so a benchmark's
measured work cannot be deleted (`bench.md`, BN9). At compile time it is the identity. The argument may be any
first-class type; a view is E0120. It is a compiler-known call like `format`: it takes no named arguments and no
bare `black_box` value exists.

## C bridge

```
struct CPtr<T>                       // a raw, Copy pointer; no drop
fn CPtr.null(): CPtr<T>              // associated function; no arguments; type from the expected type
fn CPtr.is_null(self): bool
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

`CPtr<T>` supports `null`, assignment, copying, `==`/`!=`, `.is_null()`, element offsets (`p + n`, `p - n`,
`p - q` as a count of elements), dereference (`*p` as a place to read or write), and casts between `CPtr` types and
integers. Creating, holding, reading, passing, or moving a `CPtr` value requires an `unsafe` context (E0161,
`memory-rules.md` US); a field of a `repr(C)` struct behind a `CPtr` is read and written through `*p` and field
selection. A `CPtr` does not distinguish `const`: writing through the pointer is an `unsafe` operation and is allowed
whatever the parameter convention that produced the pointer. Stage 0 has no function-pointer types and no `union`
declaration: a C function pointer can only be stored and passed as `CPtr<CVoid>` until stage 1 adds a callable type
for the bindgen target, and a union is handled through a `repr(C)` struct over the same bytes or a `CPtr<u8>`.
`CString` owns its buffer, has `Drop`, and `clone` deep-copies the NUL-terminated buffer.

**`CStr` is an alias, not a view type (issue #83).** `CStr` is a `std` type alias for `CPtr<CChar>`: one type
under two names, with no length, no origin, and no conversion rules of its own. A `CPtr<T>` is a plain
first-class `Copy` value (`memory-rules.md`, RF4 and SC1), so a `CStr` may be a local, a parameter, a return
value, a struct field, or a type argument, and no lifetime rule applies to it, unlike `str` and `[T]`.

- There is no implicit conversion between text and a C string in either direction: a `str`, `String`, or
  `CString` is not a `CStr` (E0003), and a `CStr` is not a `str` or a `String` (E0003). `CString.from(s)`
  copies the bytes and appends the NUL, `.as_ptr()` gives the `CStr`, and `CStr.to_string(p)` copies the
  NUL-terminated bytes back. The `CString` must stay alive while its pointer is used; the compiler does not
  track that lifetime, because a raw pointer is not a view.
- `CStr.to_string` is the associated function `to_string` of `CPtr<CChar>`, not a method: `CStr.to_string(p)`
  is the spelling, and `p.to_string()` is E0002, as are `p.len()` and every other string method.
- Every operation on a `CStr` value is a raw-pointer operation and requires `unsafe` (E0161, US); the alias
  adds no safety. A `CStr` promises neither NUL termination nor validity.

The alternative, a second-class `CStr` view with its own conversions, was rejected: it would add a second
implicit conversion (IC1 admits only `String` to `str`), need its own origin and lifetime rules, and present a
C string as a safe borrowed string whose termination and lifetime the compiler cannot check. The alias keeps
the C bridge honest: unvalidated bytes are a pointer (`CStr`), and `CString` is the owning buffer.

`extern "C" { fn name(params): ret }` declares a C function. Parameters are passed by value and may be integers,
floats, or `CPtr<T>`; the result may be one of those or unit. Aggregates cross the ABI through `CPtr<T>` only: a
struct declared `repr(C)` has C field order and alignment (`Type.size()` and `Type.align()` report them), but passing
or returning one by value is E0162 because the platform by-value coercions are not implemented. A Burdin `bool` is a
one-bit value while C `_Bool` is a one-byte integer, so use `u8` (0 or 1); C `char` is `CChar`. `mut`, `own`, `self`,
generics, other ABIs, and a `main` or `burdin_*` symbol are rejected (E0162 for a type or convention, E0001 for the
syntax of an unsupported ABI). A symbol is declared once: a second declaration with the same signature reuses it, a
different signature is an error. The extra arguments of a variadic declaration (`...`) are each checked against the
C ABI; inside the body they are passed as written.

`extern "C" fn name(params): ret { ... }` exports a Burdin function under its plain C symbol, so C code (or another
module that declares it `extern "C"`) can call it. The signature follows the same rules as a declaration: ABI "C",
`own` parameters, no generics, no `...`, no receiver, and only the C-ABI types above. A panic does not unwind across
the boundary; it aborts like any other panic (`memory-rules.md`, DR10).

## Serialization (issues #297, #298, #299, #300)

`std.serialize` is the framework core; the exact data model, the format side, the error path, the derives, and the
test format are in [`serialization.md`](serialization.md). This is the surface, and its names are in the prelude.
The formats themselves are official packages (`spec/packages.md`, PKG15): `packages/json` is the first one, reached
with `use json` and no manifest entry, and used as `json.to_string`, `json.to_string_pretty`, and `json.from_str`
([`packages/json/README.md`](../packages/json/README.md)).

```
interface Serialize {
    fn serialize<S: Serializer>(self, mut s: S): Result<(), DeError>
}
interface Deserialize {
    fn deserialize<D: Deserializer>(mut d: D): Result<Self, DeError>
}

// The format side: a format implements these two (SER4-SER5).
interface Serializer { ... }        // the scalar writers, begin_seq/end_seq, begin_map/map_key/end_map,
                                    // begin_struct/struct_field/end_struct, begin_variant/end_variant
interface Deserializer { ... }      // the scalar readers, begin_seq/has_next_seq_element/end_seq,
                                    // begin_map/has_next_map_entry/map_key/end_map,
                                    // begin_struct/has_next_field/field_name/end_struct,
                                    // begin_enum/variant_name/begin_variant_payload/end_variant_payload

struct DeError { message: String, path: String }        // Display; field/index/key/variant; new/syntax
fn to_text<T: Serialize>(value: T): Result<String, DeError>
fn from_text<T: Deserialize>(text: str): Result<T, DeError>
```

`Serialize` and `Deserialize` are ordinary interfaces: a program implements them by hand, and `struct T: Serialize,
Deserialize` is a built-in derive (SER12) that generates the standard representation. The per-shape helpers
(`serialize_i32`, `deserialize_str`, `serialize_list`, ...) are public and are what a hand-written impl calls; the
derived scalars call them too, while the derived containers are written out until the compiler-provided impls for the
built-in types land (SER12, SER14). Map keys are `String` in v1. The test format is a small human-readable syntax
(`Point { x: 1, y: 2 }`, `Shape.Circle(1.5)`, `[1, 2]`, `{ "k": 1 }`) with line-and-column errors.

Tests: `s3_serialize/all_scalars`, `scalars`, `nested`, `enum_hand`, `error_path`, `derive_struct`, `derive_enum`,
`derive_generic`, `derive_errors`.

## Interface coverage (issue #716)

Which built-in interface each standard type implements. A cell is either `yes` or the condition on the type
arguments; `no` means the type does not implement it. The columns are the built-in derives (`derives.md`) that this
table covers, plus `Display` and `Iterable`; `Serialize`/`Deserialize` are in their own section above.

| Type | Copy | Clone | Eq | Ord | Hash | Default | Debug | Display | Iterable |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `String` | no | yes | yes | yes | yes | empty string | yes | yes | no (`.chars()`) |
| `str` | yes | yes | yes | yes | yes | empty view | yes | yes | no (`.chars()`) |
| `List<T>` | no | `T: Clone` | `T: Eq` | no | `T: Hash` | empty (`Global`) | `T: Debug` | no | yes |
| `Map<K, V>` | no | `K, V: Clone` | `K, V: Eq` | no | `K, V: Hash` | empty (`Global`) | `K, V: Debug` | no | yes |
| `Set<T>` | no | `T: Clone` | `T: Eq` | no | no | no | `T: Eq + Debug` | no | yes |
| `Deque<T>` | no | `T: Clone` | `T: Eq` | no | no | empty | `T: Debug` | no | yes |
| `PriorityHeap<T>` | no | `T: Clone` | `T: Ord` | no | no | empty | `T: Debug` | no | yes |
| `Heap<T>` | no | yes | no | no | no | no | `T: Debug` | no | no |
| `Shared<T>` | no | yes | no | no | no | no | no | no | no |
| `Option<T>` | `T: Copy` | `T: Clone` | `T: Eq` | `T: Ord` | `T: Hash` | `Option.None` | `T: Debug` | no | no |
| `Result<T, E>` | `T, E: Copy` | `T, E: Clone` | `T, E: Eq` | `T, E: Ord` | `T, E: Hash` | no | `T, E: Debug` | no | no |
| tuples | parts | parts | parts | parts | parts | parts | 2 to 4 parts | no | no |
| arrays `[T; N]` | `T: Copy` | `T: Clone` | `T: Eq` | `T: Ord` | `T: Hash` | no | no | no | yes |
| slices `[T]` | yes | yes | no | no | no | no | no | no | yes |
| ranges `Range<T>` | `T: Copy` | `T: Clone` | `T: Eq` | `T: Ord` | `T: Hash` | no | no | no | yes |
| `Ordering` | yes | yes | yes | yes | yes | no | yes | no | no |
| dynamic `AnyError` | no | no | no | no | no | no | yes | yes | no |
| `IoError` | no | no | yes | no | no | no | yes | yes | no |
| `Duration` | yes | yes | yes | yes | yes | zero | yes | yes | no |
| `Instant` | yes | yes | yes | yes | yes | no | yes | no | no |
| `SystemTime` | yes | yes | yes | yes | yes | no | yes | no | no |
| `Weekday` | yes | yes | yes | yes | yes | no | yes | yes | no |
| `DateTime` | yes | yes | yes | yes | yes | no | yes | yes | no |
| `Path` | no | yes | yes | no | no | no | yes | yes | no |

A generic container with a stored allocator (`List<T, A>`, `Map<K, V, A>`, `Set<T, A>`) implements `Clone`, `Eq`,
and `Hash` from its elements, and has a `Default` only when it uses `Global`: a stored allocator has no value to build
a default from.

`Hash` and `Eq` agree wherever both are present: equal values hash alike, including two maps with the same entries in
different insertion order (a map hashes each entry from the offset basis and XORs the results, and a `List` folds its
length and then its elements in order). A float has no `Hash` because `0.0 == -0.0` and a `NaN` never equals itself;
`List` and `Map` have no `Ord`; `String` is not `Copy`; arrays have no `Debug` or `Default`; and slices and references
have no structural `==`.

The cells that are `no` are deliberate: deriving that interface on a struct that holds the type is E0130 at the
field, and the compile-fail test `m24_stdlib/interfaces_missing` pins each one. The cells that became `yes` in this
issue are `Hash` for `str`/`String`/`List`/`Map`, `Default` for `List`/`Deque`/`PriorityHeap`, `Ord` for
`Option`/`Result`, and `Eq`/`Debug` for `IoError` and `ParseIntError`.

## Tasks and threads (issue #198)

`std.task` and `std.thread` are specified in [`tasks.md`](tasks.md); this is the surface.

```
fn task.spawn<T, F: FnOnce(): T>(own f: F): JoinHandle<T>
fn task.scope<R, F: FnMut(Scope): R>(own f: F): R
fn task.yield()
fn task.set_workers(count: usize)
JoinHandle<T> { fn join(own self): T; fn is_finished(self): bool }      // Drop detaches an unjoined task
Scope { fn spawn<F: FnMut(): ()>(self, own f: F) }                       // the closure may borrow; see TK7
fn thread.spawn<T, F: FnOnce(): T>(own f: F): Thread<T>
fn thread.yield_now()
Thread<T> { fn join(own self): T; fn is_finished(self): bool }          // Drop detaches an unjoined thread
```

Tests: `s2_tasks/two_tasks`, `moved_value`, `yield_interleave`, `scope_disjoint`, `thread_spawn`, and the rest of the
folder (`tasks.md` lists them by rule).

## Channels and `select` (issue #207)

`std.channel` is specified in [`channels.md`](channels.md); this is the surface.

```
fn Channel<T>.bounded(capacity: usize): Channel<T>     // capacity >= 1
fn Channel<T>.unbounded(): Channel<T>
Channel<T> {
    fn send(self, own value: T): Result<(), T>         // Ok(()): queued; Err(value): the channel is closed
    fn recv(self): Option<T>                           // None: closed and empty
    fn try_send(self, own value: T): TrySend<T>        // Sent | Full(T) | Closed(T)
    fn try_recv(self): TryRecv<T>                      // Value(T) | Empty | Closed
    fn close(self)
    fn is_closed(self): bool
    fn len(self): usize
    fn capacity(self): Option<usize>
    fn clone(self): Channel<T>                         // another handle to the same channel
    fn iter(self): ChannelIter<T>                      // Iterable<T>: `for v in ch`
}
select { v = rx.recv() => ..., tx.send(x) => ..., default => ... }     // an expression; channels.md CH6-CH8
```

Tests: `s2_channels/producer_consumer`, `close_semantics`, `bounded_capacity`, `select_priority`, `select_default`,
`stress_mpmc`, and the rest of the folder (`channels.md` lists them by rule).

## Shared state: markers, atomics, and locks (issue #213)

`std.marker`, `std.atomic`, and `std.sync` are specified in [`shared.md`](shared.md); this is the surface. All of it is
in the prelude.

```
interface Send { }                                     // derived; `unsafe T: Send { }` declares it (SH4, SH5)
interface Sync { }

AtomicBool, AtomicI32, AtomicU32, AtomicI64, AtomicU64, AtomicIsize, AtomicUsize     // T below is the value type
fn AtomicT.new(value: T): AtomicT
AtomicT {
    fn load(self): T                                   fn store(self, value: T)
    fn swap(self, value: T): T                         fn compare_exchange(self, current: T, new: T): Result<T, T>
    fn fetch_add(self, value: T): T                    fn fetch_sub(self, value: T): T       // integers
    fn fetch_and / fetch_or / fetch_xor(self, value: T): T                                   // all, bool included
    fn fetch_max / fetch_min(self, value: T): T                                              // integers
}

fn Mutex<T>.new(own value: T): Mutex<T>
Mutex<T> {
    fn lock<R, F: FnOnce(&mut T): R>(self, own f: F): R            // waits; runs f with the value
    fn try_lock<R, F: FnOnce(&mut T): R>(self, own f: F): Option<R> // never waits
    fn get_mut(mut self): &mut T                       fn into_inner(own self): T
}
fn RwLock<T>.new(own value: T): RwLock<T>
RwLock<T> {
    fn read<R, F: FnOnce(&T): R>(self, own f: F): R               fn try_read(...): Option<R>
    fn write<R, F: FnOnce(&mut T): R>(self, own f: F): R          fn try_write(...): Option<R>
    fn get_mut(mut self): &mut T                       fn into_inner(own self): T
}
```

Tests: `s2_shared/atomic_ops`, `mutex_basic`, `rwlock_basic`, `send_derived`, and the rest of the folder (`shared.md`
lists them by rule).

## Tests

| Area | Tests |
| --- | --- |
| Output, numbers, parsing | `m02_basics/arithmetic`, `casts_and_numeric_methods`, `m09_stdlib/string_ops` |
| Operators and `Index` | `m19_operators/*` (`operators.md`) |
| `String`, `str`, slicing, ordering | `m09_stdlib/string_ops`, `string_order`, `slice_inclusive`, `slice_inclusive_bounds`, `m06_conventions/str_views`, `slice_views` |
| `List` | `m09_stdlib/list_ops`, `list_of_drops`, `m06_conventions/for_borrows_list` |
| Slices and arrays | `m24_stdlib/slice_read`, `slice_write`, `slice_chunks`, `slice_arrays`, `list_as_slice`, `slice_bytes_ascii`, `slice_join_concat`, `slice_binary_search`, `slice_strings`, `slice_edges`, `slice_drops`, `slice_chunks_borrow`, `slice_split_at_mut_borrow`, `slice_as_slice_borrow`, `slice_array_view_borrow`, `slice_fill_needs_clone`, `slice_to_list_needs_clone`, `slice_copy_needs_copy`, `slice_sort_needs_ord`, `slice_reverse_immutable`, `slice_view_immutable_list`, `slice_write_exclusivity`, `slice_const_write`, `slice_const_read`, `slice_chunks_zero`, `slice_windows_zero`, `slice_split_at_bounds`, `slice_rotate_bounds`, `slice_swap_bounds`, `slice_copy_length`, `slice_clone_length` |
| `List` capacity, access, and bulk edits | `m24_stdlib/list_capacity`, `list_access`, `list_bulk_edits`, `list_drop_order`, `list_allocator_methods`, `list_leaks`, `list_const`, `list_get_mut_immutable`, `list_extend_from_slice_needs_clone`, `list_capacity_stored_allocator`, `list_with_capacity_in_not_allocator`, `list_drain_not_range`, `list_append_wrong_allocator`, `list_get_mut_borrow`, `list_swap_remove_out_of_bounds`, `list_split_off_out_of_bounds`, `list_drain_out_of_bounds`, `list_reserve_overflow` |
| `Map` | `m09_stdlib/map_ops`, `map_key_types`, `map_remove_and_clear`, `map_many_keys`, `word_count` |
| Iteration | `m13_iterators/*` (`iterators.md`); `m06_conventions/return_aggregate_two_sources` and its error tests (RT3a) |
| Tasks, scopes, and threads | `s2_tasks/two_tasks`, `moved_value`, `detach_runs`, `yield_interleave`, `scope_disjoint`, `thread_spawn`, `workers_steal`, `stress_many_tasks` |
| Channels and `select` | `s2_channels/producer_consumer`, `close_semantics`, `bounded_capacity`, `select_priority`, `select_default`, `select_send`, `stress_mpmc`, `select_stress` |
| Networking and asynchronous I/O | `s2_io/net_tcp_echo`, `net_tcp_messages`, `net_tcp_timeout`, `net_tcp_refused`, `net_udp`, `net_lookup_host`, `file_in_task`, `file_fifo_offload`, `blocking_parks_worker`, `timer_parks_task`, `stress_files`, `stress_connections` (`io.md` lists them by rule) |
| `Ordering`, `cmp`, `Reverse` | `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`, `reverse` |
| Hashing | `m24_stdlib/hash_values`, `hash_hasher`, `hash_seeded`, `hash_generic`, `hash_map_consistent`, `hash_case_insensitive`, `hash_const`, `hash_float`, `hash_float_derive` |
| `Heap` | `m05_ownership/heap_tree_eval` |
| `Option`, `Result`, `?`, `orelse` | `m09_stdlib/option_result_basics`, `question_and_from`, `m16_errors/dynamic_result`, `orelse_closure` |
| Characters of a text | `m24_stdlib/text_characters` |
| Iterator consumers | `m24_stdlib/iter_last_nth_position`, `iter_for_each_reduce`, `iter_partition`, `iter_min_max`, `iter_min_max_ord`, `iter_min_max_drops`, `iter_min_needs_ord`, `iter_eq_cmp`, `iter_sum_product`, `iter_product_needs_number`, `iter_sum_overflow`, `iter_product_overflow`, `iter_try_fold`, `iter_consumers_user_iterator` |
| Iterator adapters | `m24_stdlib/iter_enumerate`, `iter_take_skip_while`, `iter_step_by`, `iter_step_by_zero`, `iter_inspect_peekable`, `iter_zip`, `iter_chain`, `iter_flat_map`, `iter_flatten`, `iter_cloned_copied`, `iter_cycle`, `iter_rev`, `iter_adapters_drops`, `iter_adapters_no_leaks`, `iter_adapters_const`; errors `iter_zip_from_temporary`, `iter_zip_in_struct_field`, `iter_chain_in_collection`, `iter_chain_while_mutating_second`, `iter_zip_while_mutating_first`, `iter_rev_needs_double_ended`, `iter_rev_through_adapter`, `iter_cloned_needs_clone`, `iter_cloned_needs_views`, `iter_cycle_needs_clone`, `iter_chain_item_mismatch`, `iter_flatten_needs_iterators`, `iter_zip_needs_iterator` |
| Sorting | `m24_stdlib/sort_by`, `sort_by_key`, `sort_unstable`, `sort_inconsistent_comparator`, `sort_drops` |
| Text helpers | `m24_stdlib/string_split`, `string_trim`, `string_lines_and_words`, `string_replace_join`, `string_search_and_affixes`, `string_case`, `string_empty_separator`, `string_empty_pattern` |
| String bytes, UTF-8, and checked slicing | `m24_stdlib/string_bytes`, `string_utf8_validation`, `string_from_utf8_lossy`, `string_utf8_file`, `string_find_all`, `string_checked_slice`, `string_bytes_second_class`, `string_view_as_text_unsafe` |
| String capacity and editing | `m24_stdlib/string_capacity`, `string_editing`, `string_edit_const`, `string_edit_boundary_aborts`, `string_edit_bounds_aborts`, `string_truncate_bounds_aborts`, `string_plus_unsupported` |
| Map in-place operations | `m24_stdlib/map_get_mut`, `map_get_or_insert`, `map_get_or_insert_drops`, `map_get_or_insert_borrow`, `map_get_mut_needs_eq` |
| Map views, updates, and construction | `m24_stdlib/map_views`, `map_in_place`, `map_capacity_interfaces`, `map_views_allocator`, `map_borrowed_key`, `map_borrowed_key_type`, `map_get_or_needs_eq`, `map_with_capacity_allocator` |
| Encodings | `m24_stdlib/encoding_hex`, `encoding_base64`, `encoding_url`, `encoding_utf8` |
| Readers and writers | `m24_stdlib/rw_cursor_read`, `rw_cursor_write_seek`, `rw_read_exact_eof`, `rw_read_chunks`, `rw_write_all`, `rw_bufreader_lines`, `rw_bufreader_buffering`, `rw_bufwriter`, `rw_bufwriter_drop`, `rw_seek_buffered`, `rw_copy`, `rw_bytesmut_write`, `rw_error_propagation`, `rw_reader_needs_read`, `rw_bufreader_capacity_zero`, `rw_bufwriter_capacity_zero` |
| Byte buffers | `m24_stdlib/bytes_mut`, `bytes_share`, `bytes_slice_out_of_bounds`, `bytes_index_out_of_bounds`, `shared_get_view`, `shared_get_view_outlives` |
| `Once` and `Lazy` | `m24_stdlib/once_basic`, `lazy_basic`, `once_lazy_drops` |
| Integer functions | `m24_stdlib/int_constants`, `int_arithmetic`, `int_bits`, `int_bytes`, `int_unknown_names`, `int_ilog_zero`, `int_isqrt_negative`, `int_next_power_overflow` |
| Integer radix and parse range | `m24_stdlib/int_radix`, `int_parse_range`, `int_radix_bad_radix`, `int_parse_radix_bad_radix` |
| Fallible conversions (`TryFrom`, `try_from`) | `m24_stdlib/try_from_int`, `try_from_user`, `try_from_from_sources`, `try_from_str`, `try_from_error`, `try_from_expect_aborts`, and the `try_from_*` compile-fail cases |
| Fallible allocation (`AllocError`, `try_` forms, AL11) | `m24_stdlib/try_alloc_list`, `try_alloc_out_of_memory`, `try_alloc_reserve`, `try_alloc_map_string`, `try_alloc_arena`, `try_alloc_infallible_aborts`, `try_alloc_stored_aborts`, `try_alloc_test_no_leak` |
| Numeric `From`, `into`, and `as` (FR1-FR6) | `m24_stdlib/from_numeric`, `from_as_lossy`, `into_numeric`, `from_lossy`, `from_arity`, `into_no_target`, `into_no_impl`, `into_method_wins`, `from_bound_*` |
| Interface coverage | `m24_stdlib/interfaces_text`, `interfaces_list`, `interfaces_map`, `interfaces_option_result`, `interfaces_tuples_arrays`, `interfaces_ranges_ordering`, `interfaces_std_containers`, `interfaces_time_path`, `interfaces_iterable`, `interfaces_missing` |
| Paths | `m24_stdlib/path_unix`, `path_windows`, `path_edit`, `path_display_debug`, `path_exists` |
| Directories, metadata, and walking | `m24_stdlib/dir_basic`, `dir_errors`, `dir_canonicalize`, `dir_walk` |
| Files and file handles | `m24_stdlib/file_handles`, `file_errors`, `file_const` |
| Time | `m24_stdlib/time_duration`, `time_instant`, `time_duration_underflow`, `time_duration_negative_float`, `time_calendar`, `time_clock`, `time_const`, `time_const_impure` |
| Random numbers | `m24_stdlib/random_seeded`, `random_properties`, `random_entropy`, `random_below_zero`, `random_range_empty`, `time_const`, `time_const_impure` |
| Float parsing | `m24_stdlib/float_parse`, `float_round_trip` |
| Float functions | `m24_stdlib/float_math`, `float_constants`, `float_predicates`, `float_min_max`, `float_bits`, `float_clamp_abort`, `float_clamp_nan_abort`, `float_unknown_names` |
| Characters | `m24_stdlib/char_ascii`, `char_control`, `char_unicode_classes`, `char_case`, `char_digits_and_code_points`, `char_bad_radix` |
| `PriorityHeap` | `m24_stdlib/heap_basic`, `heap_construction`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`, `heap_needs_ord`, `heap_equality` |
| Mutable iteration | `m24_stdlib/iter_mut_list`, `iter_mut_array`, `iter_mut_map`, `iter_mut_deque`, `iter_mut_adapters`, `iter_mut_drops`, `iter_mut_mutates_collection`, `iter_mut_nested_loops`, `iter_mut_hold_past_loop`, `iter_mut_immutable`, `iter_mut_map_key_readonly`, `iter_mut_no_iter_mut`, `iter_mut_const` |
| `Deque` | `m24_stdlib/deque_basic`, `deque_construction`, `deque_index_iteration`, `deque_strings_and_drops`, `deque_index_out_of_bounds` |
| `Set` | `m24_stdlib/set_basic`, `set_construction`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_allocator`, `set_needs_eq` |
| Roundout (containers, text, paths, time, random together) | `m24_stdlib/containers_graph_search`, `containers_const`, `roundout_text_and_time` |
| `List` convenience methods | `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_position`, `list_extend_resize`, `list_drops` |
| `Option` and `Result` combinators | `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators` |
| Files, processes, environment | `m09_stdlib/file_io`, `file_io_binary`, `env_args`, `exit_code`, `process_run`, `m16_errors/std_errors`, `m24_stdlib/io_error_kinds`, `io_error_kind_display`, `io_error_kind_mapping` |
| Environment variables and the working directory | `m24_stdlib/env_vars`, `env_bytes_unix`, `env_bytes_windows`, `env_cwd`, `env_child_unix`, `env_bad_calls`, `env_test_mode`, `env_allocations` |
| Child processes | `m24_stdlib/process_command`, `process_pipes`, `process_windows`, `process_bad_calls`, `process_test_mode`, `m20_const/purity_process` |
| Memory helpers and assertions | `m09_stdlib/std_helpers`, `unreachable_aborts`, `m24_stdlib/mem_helpers`, `mem_drop_moves`, `mem_take_needs_default`, `assert_messages`, `assert_ne_passes`, `assert_needs_eq`, `assert_message_bad_args`, `assert_ne_aborts`, `assert_message_aborts`, `assert_eq_message_aborts`, `debug_assert_release`, `debug_assert_aborts`, `debug_assert_eq_aborts`, `todo_aborts`, `unimplemented_aborts`, `m10_test_runner/assert_messages_typed`, `m10_test_runner/*`, `m16_errors/assert_typed` |
| Error declarations and conversions | `m16_errors/error_keyword`, `error_display`, `error_message_specs`, `error_bad_template`, `error_bad_from`, `error_duplicate_from`, `error_from_conversion`, `error_missing_conversion`, `error_not_display` |
| Dynamic errors, `main`, assertions | `m16_errors/dynamic_error_downcast`, `error_context_chain`, `error_context_non_error`, `traces_debug`, `main_result_ok`, `main_result_err`, `main_result_typed`, `main_result_alias`, `assert_typed`, `std_errors` |
| C bridge | `m08_modules/extern_libc`, `cstr_roundtrip`, `cptr_operations`, `cptr_null_arguments`, `c_out_parameter`, `c_abi_structs`, `cstring_clone`, `cstr_is_an_alias`, `export_c_abi`, `m11_llvm/llvm_return_42` |
| Rejected names, arities, and C ABI types | `m09_stdlib/unknown_stdlib_names` (E0002, E0004), `m08_modules/cptr_null_arguments` (E0004), `extern_signature` (E0162), `export_c_abi_invalid` (E0162), `cstr_no_implicit_conversion` (E0003), `cstr_is_not_a_str` (E0002, E0003) |
