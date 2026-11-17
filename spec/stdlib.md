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
  later panic message is written. `Stderr.flush` does nothing and returns `Ok(())`.
- A write error is ignored, exactly as `print` ignores one, so a stream's `write` always takes all of `data` and
  returns `Ok(data.len())`.
- `Stdin.read` appends up to `max` bytes the runtime has buffered, reading one block from the operating system only
  when the buffer is empty, so it returns as soon as some input is available and never waits for `max` bytes;
  `Ok(0)` is the end of the input. `read_exact`, `read_to_end`, and `read_until` keep reading. `read_line` returns one
  line without its terminator: it drops the `\n`, and the `\r` of a `\r\n` pair. `Ok(None)` is the end of the input,
  and the last line may have no terminator. `read_to_string` returns everything left, as the empty `String` at the
  end. All of them take bytes from the same buffer, so they can be mixed. Unlike the `Read` and `BufRead` defaults,
  `Stdin.read_line` and `Stdin.read_to_string` do not check the bytes for UTF-8, as with files.
- A read error is `IoError.Read` with the path `<stdin>` (`errors.md`, ERR10).
- Because the handles implement interfaces whose methods take `mut self`, a handle is bound with `mut`
  (`let mut out = stdout()`); a temporary such as `stdout().write_line("x")` needs no binding.
- `is_terminal` asks the operating system whether that stream is a terminal. It is false under the conformance
  runner, for a pipe, and for a file.
- `burdin test` gives each test process a closed standard input (`test-runner.md`, Isolation), so a read sees the
  end of the input and never waits for a person.
- None of these runs at compile time: reaching one in a comptime evaluation is E0224 (`comptime.md`, CT9).

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

enum SeekFrom derives (Clone, Copy, Eq, Debug) { Start(u64), End(i64), Current(i64) }

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
  no read allocates a buffer of its own. `Ok(0)` is the end of the input (or a `max` of 0). A reader may append fewer
  bytes than asked for. `read_exact` keeps reading until `count` bytes have been appended, and a source that ends
  first is `IoError.UnexpectedEof`; the bytes it did produce stay in the list. `read_to_end` appends everything left
  and returns the count. `read_to_string` collects it as text, and bytes that are not valid UTF-8 are
  `IoError.InvalidData` (the file functions above do not check, as they return what the file holds).
- **Writing takes a prefix.** `write` takes a prefix of `data` and returns its length. `write_all` calls it until every
  byte is taken, and a sink that takes none of a non-empty `data` is `IoError.WriteZero`. `write_text` writes the
  UTF-8 bytes of a string and `write_line` adds one `\n`. `flush` pushes what the writer holds to what it wraps; a
  writer that holds nothing returns `Ok(())`.
- **`BufRead`.** `read_until` appends up to and including the first `delim` byte, or up to the end of the input, and
  returns how many bytes it appended (`0` at the end). `read_line` is the one `Stdin` provides, for any
  buffered reader: it drops the `\n` and the `\r` of a `\r\n`, returns `Ok(None)` at the end, accepts a last line with
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
`Err(ParseIntError.Invalid)`, whose message is `invalid integer`, for anything else.

Arithmetic is the built-in operators only: `+ - * / %`, `<< >>`, `& | ^`, unary `-`. Overflow panics in debug and
wraps in release for `+ - *` and unary `-`; division by zero and out-of-range shifts always panic. `/` and `%`
truncate toward zero. `as` converts between numeric types (and `char` to an integer, and `u8` or `u32` to `char`), following Rust's cast rules. `u32 as char` does not check that the value is a Unicode scalar value (the result for a surrogate or a value past `0x10FFFF` is unspecified); `u32.to_char()` does.

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
integer type looks `name` up as the std function `__T_name`, and `T.NAME` as the std constant `__T_NAME`. A name that
does not exist is the usual `E0002`. The floating-point functions are a separate issue (#703).

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

Tests: `m24_stdlib/char_ascii`, `char_unicode_classes`, `char_case`, `char_digits_and_code_points`, `char_bad_radix`.

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

interface PartialEq {
    fn eq(self, other: Self): bool
    fn ne(self, other: Self): bool      // default: !self.eq(other)
}

interface PartialOrd {
    fn lt(self, other: Self): bool
    fn le(self, other: Self): bool      // default: !self.gt(other)
    fn gt(self, other: Self): bool      // default: other.lt(self)
    fn ge(self, other: Self): bool      // default: !self.lt(other)
}

interface Index<T>    { fn index(self, i: usize): &T }
interface IndexMut<T> { fn index_mut(mut self, i: usize): &mut T }
```

The built-in operators and the `Eq`/`Ord` derives always win over an implementation, both operands have the same
type, and `&&`, `||`, `=`, compound assignment, and the shifts are not overloadable. `Index` names the element type
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
fn str.char_indices(self): List<(usize, char)>        // each character with its byte offset
fn str.reverse_chars(self): String
fn str.slice_chars(self, start: usize, end: usize): String   // by character index; an index past the end stops there
fn List<char>.collect_string(self): String
```

Tests: `m24_stdlib/text_characters` (a text with one-, two-, and three-byte characters), and the non-ASCII cases of
`string_*` and `char_*`.

### Text helpers (issue #415)

`std.text` (and the case conversion in `std.unicode`) declare extension methods on `str` and `String`
(`extensions.md`), so the prelude makes them visible everywhere and the compiler's own methods (`len`, `find`,
`starts_with`, `chars`, ...) win. Offsets are byte offsets. A match of a valid separator in valid text always starts
and ends on a character boundary, and `trim` and the case conversions walk whole characters, so the functions keep
UTF-8 intact. The pieces they return are `String`s because a `List` cannot hold views; `trim*` return a `str` view
of the receiver.

```
fn str.is_empty(self): bool
fn str.contains(self, needle: str): bool                // "" is contained everywhere
fn str.ends_with(self, suffix: str): bool
fn str.trim(self): str                                  // also trim_start, trim_end; Unicode white space
fn str.split(self, sep: str): List<String>              // keeps empty pieces; an empty `sep` panics
fn str.split_char(self, c: char): List<String>
fn str.rsplit(self, sep: str): List<String>             // the same pieces, last first
fn str.rsplit_char(self, c: char): List<String>
fn str.split_whitespace(self): List<String>             // runs of non-white-space characters
fn str.lines(self): List<String>                        // at line feeds; a carriage return before one is removed
fn str.replace(self, from: str, to: str): String        // an empty `from` panics
fn str.strip_prefix(self, prefix: str): Option<String>
fn str.strip_suffix(self, suffix: str): Option<String>
fn str.repeat(self, count: usize): String
fn str.to_lowercase(self): String                       // char.to_lowercase on each character
fn str.to_uppercase(self): String
fn str.to_ascii_lowercase(self): String
fn str.to_ascii_uppercase(self): String
fn str.eq_ignore_ascii_case(self, other: str): bool
fn List<String>.join(self, sep: str): String
```

`String` has the same methods with the same behavior. When two standard modules extend different receivers under one
name (`Option.replace`, `str.replace`), the prelude keeps all of them; a module's own declaration or `use` of that
name still hides the prelude's.

Tests: `m24_stdlib/string_split`, `string_trim`, `string_lines_and_words`, `string_replace_join`,
`string_search_and_affixes`, `string_case`, `string_empty_separator`, `string_empty_pattern`.

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
`Bytes index out of bounds`), `BytesMut` also `IndexMut<u8>` (`b[i] = x`), and both implement `PartialEq` (same bytes)
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
`Copy`. The rules are [`allocators.md`](allocators.md) AL1-AL8.

## `List<T>`

```
struct List<T, A = Global> {
    fn new(): List<T, A>              // only when A is `Global`: the allocator-free constructor
    fn new_in(alloc: A): List<T, A>   // takes the allocator value to store (`allocators.md`, AL4)
    fn push(mut self, value: T)       // takes ownership of the value
    fn pop(mut self): Option<T>
    fn len(self): usize
    fn last(self): Option<&T>
    fn insert(mut self, index: usize, value: T)
    fn remove(mut self, index: usize): T
    fn clear(mut self)
    fn contains(self, value: T): bool // needs == on T
    fn sort(mut self)                 // ascending; needs < on T
    fn iter(self): [T]                // borrows: `T` if Copy, else `&T`
    fn into_iter(own self): ListIntoIter<T, A>   // consumes the list, carrying the allocator
    fn map<U, F: FnMut(&T): U>(self, f: F): List<U>
    fn filter<F: FnMut(&T): bool>(self, f: F): List<T>   // needs Clone on T
    fn fold<A, F: FnMut(A, &T): A>(self, init: A, f: F): A
    fn any<F: FnMut(&T): bool>(self, f: F): bool
    fn all<F: FnMut(&T): bool>(self, f: F): bool
    fn find<F: FnMut(&T): bool>(self, f: F): Option<T>   // needs Clone on T
}
```

Indexing `xs[i]` is a place: it reads or writes one element, takes a `usize`, and panics when out of bounds.
`for x in xs` borrows the list for the loop and yields read-only element places (`memory-rules.md`, IT). `T` must be
storable, so `List<str>` and `List<&T>` are rejected (E0120). The last six methods are the eager iteration helpers:
they borrow the list, hand the closure a view of each element, and return owned results (`iterators.md`, IT10);
`.any` is resolved to the `std` function named `__list_any` and spelled with the reserved word. Every method
except `new` exists for every allocator argument; `List<T, A>.new()` where `A` is not `Global` is E0272 because
the constructor has no allocator value to store (`allocators.md`, AL4).

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
`Hash`.

### Map entry operations (issue #482)

```
fn get_mut(mut self, key: K): Option<&mut V>        // built in: a mutable view of the value, or None
fn get_or_insert(mut self, key: K, value: V): &mut V    // built in
fn entry(mut self, own key: K): MapEntry<K, V, A>       // built in
fn MapEntry<K, V, A>.or_insert(own self, own value: V): &mut V                  // built in
fn MapEntry<K, V, A>.or_insert_with<F: FnOnce(): V>(own self, own make: F): &mut V   // built in
fn get_or_insert_with<F: FnOnce(): V>(mut self, key: K, make: F): &mut V
fn update<F: FnOnce(&mut V)>(mut self, key: K, change: F): bool
fn upsert<F: FnOnce(&mut V)>(mut self, key: K, value: V, change: F)
```

`entry(key)` borrows the map exclusively, takes ownership of the key, and probes the map once. It returns a
`MapEntry<K, V, A>` that holds the key and the slot the probe found. The entry is second-class (`memory-rules.md`,
SC2): it views the map, so it cannot be stored in a field or a container, is neither `Copy` nor `Clone`, may not
outlive the map (E0123), and dropping it without filling it drops the key. `or_insert(value)` stores the key and the
value when the key is absent and returns a mutable view of the value; when the key is present it keeps the stored
value and drops the incoming value and the entry's key. `or_insert_with(make)` does the same, but calls `make` for
the value only when the key is absent; the closure is consumed either way, and a closure that is not called is
dropped with what it captured. Neither operation looks the key up again: they fill the slot the entry probed for,
and probe again only when the insert has to grow the map. A stored allocator (`allocators.md`, AL3) carries through
`MapEntry`, so a fill that grows allocates through the map's own allocator.

The `&mut V` keeps the map exclusively borrowed until its last use (a `m.insert(...)` while it is live is E0111),
because inserting may move the table; an entry that has not been filled holds the map the same way. The key needs
`==`, like every map operation (E0141), and neither the entry nor its operations run at compile time
(`comptime.md`, CT9).

`get_or_insert` returns the value stored under `key`; when the key is absent it first stores `value`. A value that is
already there is kept, and the incoming key and value are dropped on the spot. It looks the key up once.
`get_or_insert_with` is `self.entry(key).or_insert_with(make)`, so it probes once as well. `update` runs `change` on
the value for `key` and returns whether the key was there. `upsert` stores `value` for an absent key and otherwise
runs `change` on the value already there, dropping `value`.

The counting idiom is

```
let count = map.entry(word).or_insert(0)
*count = *count + 1
```

Tests: `m24_stdlib/map_entry`, `map_entry_drops`, `map_entry_generic`, `map_entry_allocator`, `map_entry_borrow`
(E0111), `map_entry_needs_eq` (E0141), `map_entry_second_class` (E0120, E0123), and `map_entry_comptime` (E0224).

## `Set<T>` (issue #190)

`std.set` is a thin layer over `Map<T, bool, A>`, so it follows the map's rules: an element needs only `==` (`Eq`),
lookup, insert, and remove are expected O(1), and the iteration order is not specified. Like `List` and `Map`, a set
takes an allocator as its last type argument and stores it (`allocators.md`).

```
struct Set<T: Eq, A: Allocator = Global> {
    fn new(): Set<T>                               // only when A is `Global`
    fn new_in(own alloc: A): Set<T, A>
    fn from_list(own items: List<T>): Set<T>       // a repeated element is stored once
    fn len(self): usize
    fn is_empty(self): bool
    fn contains(self, value: T): bool
    fn insert(mut self, own value: T): bool        // false (and the new value dropped) when already present
    fn remove(mut self, value: T): bool
    fn clear(mut self)
    fn extend(mut self, own items: List<T>)
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

`Set<T>` implements `PartialEq` (same elements, regardless of order), `Debug` (`{1, 2, 3}`, in iteration order), and
`Iterable<&T>`. `MapEntries<K, V>` is the nameable type of the iterator `map.iter()` returns, added so a library type
can wrap it. Operations that copy elements need `Clone`; they return a set that uses the default allocator.

Tests: `m24_stdlib/set_basic`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_needs_eq`.

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
    fn into_list(own self): List<T>                 // front to back
    fn iter(self): DequeIter<&Deque<T>, T>          // yields &T from the front to the back
    fn clone(self): Deque<T>                        // T: Clone
}
```

`Deque<T>` implements `Index<T>` and `IndexMut<T>` (`d[i]` counts from the front and panics with `Deque index out of
bounds` past the end), `PartialEq` (same length and equal elements), `Debug` (`[1, 2, 3]`), and `Iterable<&T>`.
`DequeIter<D, T>` is generic over the view of the deque it walks and needs `D: Index<T> + DequeLen`; for that, a
shared view `&X` satisfies an interface bound that `X` satisfies.

Tests: `m24_stdlib/deque_basic`, `deque_index_iteration`, `deque_strings_and_drops`, `deque_index_out_of_bounds`.

## `PriorityHeap<T>` (issue #192)

`std.heap` is a priority queue ordered by `Ord`. The elements are kept in ascending order in one list, so the greatest
element is the last one: `peek` and `pop` are O(1), `push` finds its place with a binary search (O(log n)
comparisons) and moves the elements above it, and iterating in priority order is walking the list backwards. It is
not a binary heap: swapping two elements of one list needs a placeholder value the language does not have, and the
sorted list gives iteration in priority order for free. Elements that compare equal come out in the order they were
pushed. The element type only needs an `Ord` bound, which a type with a `PartialOrd` impl also satisfies.

```
struct PriorityHeap<T: Ord> {
    fn new(): PriorityHeap<T>
    fn from_list(own items: List<T>): PriorityHeap<T>
    fn len(self): usize
    fn is_empty(self): bool
    fn push(mut self, own value: T)
    fn extend(mut self, own items: List<T>)
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

Tests: `m24_stdlib/heap_basic`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`,
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

**The static/global story.** Burdin has no mutable global variables, so a cache or a piece of configuration is a value
that lives in a local or a struct and is passed to whatever needs it, and `Once` and `Lazy` are ordinary values like
that. A value that is known at compile time is a `const` or a `comptime` expression instead. The concurrency-safe
`Once` and `Lazy` (a cell several threads can race to fill) arrive with threads in stage 2.

## Paths (issue #194)

```
struct Path derives (Clone, Eq) {
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
  `file_name` is the last name, `None` for `..` or no names; trailing separators and `.` pieces are ignored.
- `extension` is the text after the last dot of the file name, unless the name starts with that dot (`.profile` has
  none); `file_stem` is the rest (`archive.tar.gz` is `archive.tar` and `gz`). `with_extension("")` removes it.
- `normalize` removes `.` and empty pieces, cancels `name/..`, keeps a leading `..` of a relative path, drops `..` at a
  root, and writes the rules' separator; an empty result is `.`. It is lexical, so it can change what a path with
  symbolic links names.
- `exists` is true when the system can see the path (the C library's `access`, or `_access` on Windows, with no mode bits). It is impure, so a
  comptime evaluation that reaches it fails with E0224. Directory listing, metadata, and walking are the next section.

Tests: `m24_stdlib/path_unix`, `path_windows`, `path_edit`, `path_exists`.

## Directories, metadata, and walking (issue #411)

```
enum FileKind derives (Copy, Clone, Eq, Debug) { File  Dir  Symlink  Other }

struct Metadata derives (Clone, Copy, Eq, Debug) {
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

Every function is impure: a comptime evaluation that reaches one fails with E0224. A read that only inspects the
file system (`read_dir`, `metadata`, `canonicalize`, `walk`, `walk_glob`) reports `IoError.Read`; one that changes it
(`create_dir`, `create_dir_all`, `remove_dir`, `remove_dir_all`, `remove_file`, `rename`) reports `IoError.Write`.
`rename` names its destination `to` in the error.

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
  because it consumes the handle, the drop that follows closes the descriptor a second time, which fails harmlessly
  on a closed descriptor.
- `read`, `write`, and `append` are the byte-level counterparts of `read_to_string` and `write_file`; `write`
  creates or replaces, and `append` creates a missing file. `copy_file` copies the bytes only, not permissions, and
  returns how many it copied. It is not named `copy` because `std.rw` already exports the reader/writer `copy` under
  that name, and the prelude has only the unqualified name for one of them; write `fs.copy_file(...)` for a file.
  `exists` is `Path.exists`.
- Every operation is impure: a comptime evaluation that reaches one fails with E0224.

Tests: `m24_stdlib/file_handles`, `file_errors`, `file_comptime`.

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
`any_item`, `all`, `find`, and `to_list`; and, from issue #706, `enumerate`, `take_while`, `skip_while`, `map_while`,
`step_by`, `inspect`, and `peekable`; and, from issue #707, `last`, `nth`, `position`, `for_each`, `reduce`,
`partition`, `min_by`, `max_by`, `min_by_key`, and `max_by_key`) and the marker `Iterable<T>`. The compiler implements both for arrays,
slices, `List`, `Map`, `Chars`, and ranges, and adds `sum` for iterators of integers; `std` declares the adapter
types `MapIter`, `FilterIter`, `TakeIter`, and `SkipIter`. A program implements the interfaces to make its own type
work with `for` and the adapters. The rules, the item types, and the second-class restrictions are in
[`iterators.md`](iterators.md).

## `Ordering`, `cmp`, and `Reverse<T>` (issue #705)

`std.cmp` declares the three-way comparison. The prelude makes its names visible in every module, and `cmp.min(a, b)`
names the module.

```
enum Ordering derives (Copy, Clone, Eq, Ord, Hash, Debug) { Less  Equal  Greater }

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

struct Reverse<T> derives (Copy, Clone, Eq, Debug) { value: T }   // `<` is `T`'s `>`; `T: Ord` through `PartialOrd`
```

`a.cmp(b): Ordering` is provided by the compiler for every type that satisfies `Ord`: the integers, `char`, `bool`,
`str`, `String`, `()`, tuples, fixed-size arrays, and ranges of ordered parts, a struct or enum that derives `Ord`, a type parameter with
an `Ord` bound, and `Reverse<T>`. It calls the private `std.cmp.__ord_cmp`, which is `if a < b { Less } else if b < a
{ Greater } else { Equal }`. `Option<T>` and `Result<T, E>` get `cmp` from extension methods in `std.cmp` (`None`
before every `Some`; every `Ok` before every `Err`; contents compared when the kinds match): they are not `Ord`, so
`<` still does not apply to them. Floats have `<` but no total order, so `f64.cmp` is E0002 (`total_cmp` is the float
order, issue #703). The decisions behind it:

- **`cmp` is not an interface method.** `PartialOrd` keeps `lt`/`le`/`gt`/`ge` and `Ord` stays the compiler-known
  bound, so a derive generates nothing new: `cmp` is `<` read twice. `Ord` and `Eq` remain independent derives.
- **A hand-written `PartialOrd` satisfies an `Ord` bound.** A type that implements `PartialOrd` (its `lt` is the
  order) is accepted wherever `T: Ord` is required, which is how `Reverse<T>` works with `cmp.min`, `max_by_key`, and
  later `PriorityHeap` and `sort_by_key`. The built-in `<` does not apply to such a type (`has_ord` is false), so a
  generic body instantiated for it calls `lt`; the compiler-implemented `List.sort` still wants a derived `Ord`
  (E0141). Hand-written `Eq`/`Ord`/`Hash` beyond this stay E9000 under #402.
- A closure comparator or key function sees views of the values (`&T`), so the helpers return an owned `T`: `min`
  and friends cannot be called on two `str` views (E0121, a view cannot come from two parameters); use `String`.

Tests: `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`,
`reverse`, `cmp_float_has_no_cmp`, `cmp_needs_ord`.

## `Option<T>` and `Result<T, E>`

`Option<T>` is built in with the variants `Some(T)` and `None`; `None` takes its type from context. `Result<T, E>`
is built in with `Ok(T)` and `Err(E)`. `Result<T>` written with one argument means `Result<T, Error>`, the dynamic
error type described below (`errors.md`, ERR5).

```
Option<T>:  unwrap(), expect(msg: str): T, is_some(): bool, is_none(): bool
Result<T, E>: unwrap(), expect(msg: str): T, is_ok(): bool, is_err(): bool
Result<T, E>: context(message: str): Result<T, Error>
```

`unwrap` panics on `None`/`Err`; `expect(msg)` panics with `msg`. `expr?` returns early through the enclosing
`Option` or `Result` function (E0160 outside one) and converts the error through a user `Type: From<Source>` block;
in a function returning `Result<U>` / `Result<U, Error>`, any error type is boxed into the dynamic `Error`
(`errors.md`, ERR4). `expr orelse fallback` supplies a value for `None` or `Err`; a closure handler receives the
`Err` payload (`|e| ...`) and takes no argument for `None` (`|| ...`, issue #488). `match` handles both types; `Option` also supports `==`/`!=` when its
payload does, and `Hash` when its payload does. `.context(message)` prefixes a context message and returns
`Result<T, Error>`; it is E0212 when the error type does not implement `Error` (`errors.md`, ERR6). The combinators of the next section are
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
Option<T>:     map, and_then, or_else, or, and, xor, zip, unwrap_or, unwrap_or_else, map_or, map_or_else,
               ok_or, ok_or_else, filter, is_some_and, is_none_or, take, replace
Result<T, E>:  map, map_err, and_then, or_else, unwrap_or, unwrap_or_else, map_or, ok, err, unwrap_err,
               is_ok_and, is_err_and
```

The error-conversion helpers are `map_err` (apply a conversion to the error), `ok_or`/`ok_or_else` (an absent value
becomes an error), `ok`/`err` (drop one side), and the `?` operator, which keeps converting through `From`. The two
`unwrap_or_default` methods, `Option.flatten`, and `as_ref` are not provided: `T.default()` does not exist for the
built-in types, an extension receiver may not nest a type parameter (`Option<Option<T>>`), and a view returned from
an extension would dangle. A closure whose result mentions a type parameter that only the closure fixes
(`and_then(|x| Some(x))`, `F: FnOnce(T): Option<U>`) is typed from its body; a body that cannot say every part of the
type by itself (`or_else(|e| Err(e.len()))`, which leaves the `Ok` type open) needs a named function or a typed
binding.

Tests: `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators`, `unwrap_err_on_ok`,
`option_wrong_default`, `m12_closures/closure_returns_generic_wrapper`.

### `List` convenience methods (issue #483)

`std.list` declares these as extension methods on `List<T>` (a list with the default allocator; `extensions.md`), so
the prelude makes them visible everywhere and the compiler's own methods (`push`, `pop`, `sort`, `contains`, `map`,
`filter`, `find`, ...) win. They move elements with `pop`/`push`, so nothing is cloned unless the name says so, and
every element is dropped exactly once.

```
fn List<T>.is_empty(self): bool
fn List<T>.retain<F: FnMut(&T): bool>(mut self, f: F)      // keeps the accepted elements, in order
fn List<T>.position<F: FnMut(&T): bool>(self, f: F): Option<usize>
fn List<T>.rposition<F: FnMut(&T): bool>(self, f: F): Option<usize>
fn List<T>.reverse(mut self)
fn List<T>.dedup(mut self) where T: Eq                      // removes consecutive duplicates
fn List<T>.extend(mut self, own other: List<T>)             // moves the elements of `other`
fn List<T>.extend_from(mut self, other: List<T>) where T: Clone
fn List<T>.resize(mut self, new_len: usize, own value: T) where T: Clone
fn List<T>.truncate(mut self, len: usize)
fn List<T>.swap(mut self, a: usize, b: usize)              // panics when an index is out of bounds
```

`find` is the eager helper of `iterators.md` IT10 (it clones the element). `dedup_by_key`, `drain`, and `first` are
not provided: a key function would need its result type fixed by a bound the checker cannot yet match against a type
parameter, and views returned from an extension would dangle.

Tests: `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_position`, `list_extend_resize`, `list_drops`,
`list_swap_out_of_bounds`.

## Errors

`std` declares the marker interface `Error requires Display { }` and, under the same name in the type namespace,
the dynamic error value:

```
struct Error {
    value: any Error
    frames: List<TraceFrame>
    context: String
}

struct TraceFrame {
    file: String
    line: i32
    col: i32
}
```

A type is an error type when it implements `Error`, which requires `Display`. The dynamic `Error` is an owned
`any Error` box together with the context messages `.context` added (outermost first) and the frames a debug build
records. An error type is boxed implicitly wherever `Error` is expected; `e.is<T>()` reports whether the box holds
a `T`, `e.downcast<T>()` consumes the box and yields `Some(value)` on a match (both need `T: Error`), and `{}`
prints the wrapped error's `Display` (`errors.md`, ERR4). `TraceFrame` implements `Display` as `file:line:col`, and
`error.frames` is its public `List<TraceFrame>`; a debug build records a creation frame at each conversion into
`Error` and a propagation frame at each `?` that passes a `Result<..., Error>` outward, and a release build records
none (`errors.md`, ERR7).

`std` declares its error types (`errors.md`, ERR10):

```
error enum IoError {
    Read { path: String }    "cannot read {path}"
    Write { path: String }   "cannot write {path}"
    Run { command: String }  "cannot run {command}"
    UnexpectedEof            "unexpected end of input"
    WriteZero                "the writer accepted no bytes"
    InvalidData              "the bytes are not valid text"
    Seek                     "seek to a position before the start"
    InvalidInput             "invalid open options"
}

error enum ParseIntError {
    Invalid  "invalid integer"
}
```

`read_to_string` returns `IoError.Read` on failure, `write_file` returns `IoError.Write`, `process.run` returns
`IoError.Run`, and an integer `parse` returns `ParseIntError.Invalid`. The readers and writers return
`UnexpectedEof` (`read_exact` on a source that ended), `WriteZero` (`write_all` on a sink that takes nothing),
`InvalidData` (text that is not UTF-8), and `Seek` (a position before the start); `OpenOptions.open` returns
`InvalidInput` for an option set it rejects ("Files and file handles"). The set of variants is settled together
with the operating-system error codes in issue #701.

An `error enum` declaration lowers to an ordinary enum plus generated `Display` (from each variant's message
template), `Error`, and `From<Source>` impls, so an error value is an ordinary value (`errors.md`, ERR1-ERR3). A
function written `fn main(): Result<T>` prints an `Err` with `Display` to standard error and exits 1; `Ok` prints
nothing and exits 0 (`errors.md`, ERR8).

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
returned as is. On failure each returns the matching `IoError` variant, and an integer `parse` returns
`ParseIntError.Invalid` (`errors.md`, ERR10). The byte-level helpers (`read`, `write`, `append`, `copy_file`,
`exists`) and the owning `File` handle are in "Files and file handles".

None of these runs at compile time: a comptime evaluation that reaches one fails with E0224 (`comptime.md`, CT9).

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
except NUL. On Unix an environment value and a path are byte strings: `get` and `current_dir` report `None` and
`WorkingDirectory` when the bytes are not valid UTF-8, and the `_bytes` functions return them exactly. Windows stores
the environment and the working directory as UTF-16, so the primitives convert to and from UTF-8; a value that is not
valid Unicode (an unpaired surrogate) is reported as absent and cannot be set, and the byte and text functions agree.

**Behavior.** `get` is `None` when the variable is unset, while an empty value is `Some("")` because the variable is
present. `set` replaces an existing value, and `remove` succeeds when the variable is unset. `set` and `remove` report
`Name` for an invalid name, `Value` for an invalid value, and `Access` when the system refuses. `current_dir` has no
trailing separator except at a root, and `set_current_dir` reports `SetWorkingDirectory` when the path is not a
directory. A change is visible to a child started with `process.run`.

None of these runs at compile time: a comptime evaluation that reaches one fails with E0224 (`comptime.md`, CT9).

Tests: `m24_stdlib/env_vars`, `env_bytes_unix`, `env_bytes_windows`, `env_cwd`, `env_child_unix`, `env_bad_calls`,
`env_test_mode`.

## The compilation target

```
enum target.Os derives (Copy, Clone, Eq) { Linux  MacOs  Windows }
enum target.Arch derives (Copy, Clone, Eq) { X86_64  Aarch64 }
fn target.os(): target.Os                   // the operating system the program is compiled for
fn target.arch(): target.Arch               // the processor architecture the program is compiled for
```

The compiler builds for the machine it runs on, so both are the host's. They are ordinary functions, usable at run
time and at compile time, which is how `comptime if target.os() == target.Os.Windows { ... }` selects a platform
branch (`comptime.md`, CT5). `tests/m20_comptime/comptime_if_platform.bd`, `comptime_target_values.bd`.

## Memory helpers and assertions

```
fn mem.swap(mut a: T, mut b: T)
fn mem.replace(mut place: T, own value: T): T   // moves the value in; returns the old one
fn panic(msg: str): !
fn assert(cond: bool)
fn assert_eq<T: Eq>(a: T, b: T)
fn unreachable(): !
```

`swap` and `replace` take places and are checked for exclusivity like any other call. `panic` prints its message and
location and aborts (`memory-rules.md`, DR10); `unreachable()` panics with `unreachable`; `assert` panics with
`assertion failed`; `assert_eq` panics when the values differ. In a `burdin test` build the prelude replaces
`assert`/`assert_eq` with `__test_assert`/`__test_assert_eq`, which return `Result<(), AssertError>` instead of
panicking: a failure is `AssertError.Failed { condition, location }` or
`AssertError.FailedEq { condition, left, right, location }`, where `condition` is the call's source text, `left`
and `right` are its values formatted with `Debug`, and `location` is its `path:line:col` (`errors.md`, ERR9). A
normal build keeps the panicking assertions.

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

## Tests

| Area | Tests |
| --- | --- |
| Output, numbers, parsing | `m02_basics/arithmetic`, `casts_and_numeric_methods`, `m09_stdlib/string_ops` |
| Operators and `Index` | `m19_operators/*` (`operators.md`) |
| `String`, `str`, slicing, ordering | `m09_stdlib/string_ops`, `string_order`, `slice_inclusive`, `slice_inclusive_bounds`, `m06_conventions/str_views`, `slice_views` |
| `List` | `m09_stdlib/list_ops`, `list_of_drops`, `m06_conventions/for_borrows_list` |
| `Map` | `m09_stdlib/map_ops`, `map_key_types`, `map_remove_and_clear`, `map_many_keys`, `word_count` |
| Iteration | `m13_iterators/*` (`iterators.md`) |
| `Ordering`, `cmp`, `Reverse` | `m24_stdlib/ordering_methods`, `cmp_builtin_types`, `cmp_option_result`, `cmp_generic`, `cmp_functions`, `reverse` |
| `Heap` | `m05_ownership/heap_tree_eval` |
| `Option`, `Result`, `?`, `orelse` | `m09_stdlib/option_result_basics`, `question_and_from`, `m16_errors/dynamic_result`, `orelse_closure` |
| Characters of a text | `m24_stdlib/text_characters` |
| Iterator consumers | `m24_stdlib/iter_last_nth_position`, `iter_for_each_reduce`, `iter_partition`, `iter_min_max` |
| Iterator adapters | `m24_stdlib/iter_enumerate`, `iter_take_skip_while`, `iter_step_by`, `iter_step_by_zero`, `iter_inspect_peekable` |
| Sorting | `m24_stdlib/sort_by`, `sort_by_key`, `sort_unstable`, `sort_inconsistent_comparator`, `sort_drops` |
| Text helpers | `m24_stdlib/string_split`, `string_trim`, `string_lines_and_words`, `string_replace_join`, `string_search_and_affixes`, `string_case`, `string_empty_separator`, `string_empty_pattern` |
| Map entry operations | `m24_stdlib/map_get_mut`, `map_get_or_insert`, `map_get_or_insert_drops`, `map_get_or_insert_borrow`, `map_get_mut_needs_eq`, `map_entry`, `map_entry_drops`, `map_entry_generic`, `map_entry_allocator`, `map_entry_borrow`, `map_entry_needs_eq`, `map_entry_second_class`, `map_entry_comptime` |
| Encodings | `m24_stdlib/encoding_hex`, `encoding_base64`, `encoding_url`, `encoding_utf8` |
| Readers and writers | `m24_stdlib/rw_cursor_read`, `rw_cursor_write_seek`, `rw_read_exact_eof`, `rw_read_chunks`, `rw_write_all`, `rw_bufreader_lines`, `rw_bufreader_buffering`, `rw_bufwriter`, `rw_bufwriter_drop`, `rw_seek_buffered`, `rw_copy`, `rw_bytesmut_write`, `rw_error_propagation`, `rw_reader_needs_read`, `rw_bufreader_capacity_zero`, `rw_bufwriter_capacity_zero` |
| Byte buffers | `m24_stdlib/bytes_mut`, `bytes_share`, `bytes_slice_out_of_bounds`, `bytes_index_out_of_bounds`, `shared_get_view`, `shared_get_view_outlives` |
| `Once` and `Lazy` | `m24_stdlib/once_basic`, `lazy_basic`, `once_lazy_drops` |
| Integer functions | `m24_stdlib/int_constants`, `int_arithmetic`, `int_bits`, `int_bytes`, `int_unknown_names`, `int_ilog_zero`, `int_isqrt_negative`, `int_next_power_overflow` |
| Integer radix and parse range | `m24_stdlib/int_radix`, `int_parse_range`, `int_radix_bad_radix`, `int_parse_radix_bad_radix` |
| Paths | `m24_stdlib/path_unix`, `path_windows`, `path_edit`, `path_exists` |
| Directories, metadata, and walking | `m24_stdlib/dir_basic`, `dir_errors`, `dir_canonicalize`, `dir_walk` |
| Files and file handles | `m24_stdlib/file_handles`, `file_errors`, `file_comptime` |
| Float parsing | `m24_stdlib/float_parse`, `float_round_trip` |
| Characters | `m24_stdlib/char_ascii`, `char_unicode_classes`, `char_case`, `char_digits_and_code_points`, `char_bad_radix` |
| `PriorityHeap` | `m24_stdlib/heap_basic`, `heap_iteration`, `heap_equal_priorities`, `heap_min_heap`, `heap_drops`, `heap_index_out_of_bounds`, `heap_needs_ord` |
| `Deque` | `m24_stdlib/deque_basic`, `deque_index_iteration`, `deque_strings_and_drops`, `deque_index_out_of_bounds` |
| `Set` | `m24_stdlib/set_basic`, `set_algebra`, `set_iteration`, `set_strings_and_drops`, `set_needs_eq` |
| `List` convenience methods | `m24_stdlib/list_retain_reverse`, `list_dedup`, `list_position`, `list_extend_resize`, `list_drops` |
| `Option` and `Result` combinators | `m24_stdlib/option_combinators`, `option_take_replace`, `result_combinators` |
| Files, processes, environment | `m09_stdlib/file_io`, `file_io_binary`, `env_args`, `exit_code`, `process_run`, `m16_errors/std_errors` |
| Environment variables and the working directory | `m24_stdlib/env_vars`, `env_bytes_unix`, `env_bytes_windows`, `env_cwd`, `env_child_unix`, `env_bad_calls`, `env_test_mode` |
| Memory helpers and assertions | `m09_stdlib/std_helpers`, `unreachable_aborts`, `m10_test_runner/*`, `m16_errors/assert_typed` |
| Error declarations and conversions | `m16_errors/error_keyword`, `error_display`, `error_message_specs`, `error_bad_template`, `error_bad_from`, `error_duplicate_from`, `error_from_conversion`, `error_missing_conversion`, `error_not_display` |
| Dynamic errors, `main`, assertions | `m16_errors/dynamic_error_downcast`, `error_context_chain`, `error_context_non_error`, `traces_debug`, `main_result_ok`, `main_result_err`, `main_result_typed`, `main_result_alias`, `assert_typed`, `std_errors` |
| C bridge | `m08_modules/extern_libc`, `cstr_roundtrip`, `cptr_operations`, `cptr_null_arguments`, `c_out_parameter`, `c_abi_structs`, `cstring_clone`, `cstr_is_an_alias`, `export_c_abi`, `m11_llvm/llvm_return_42` |
| Rejected names, arities, and C ABI types | `m09_stdlib/unknown_stdlib_names` (E0002, E0004), `m08_modules/cptr_null_arguments` (E0004), `extern_signature` (E0162), `export_c_abi_invalid` (E0162), `cstr_no_implicit_conversion` (E0003), `cstr_is_not_a_str` (E0002, E0003) |
