# TOML

The TOML package (stage 3, issue #256; `packages/toml`), with its subset decided by issue #377. The package is an
ordinary official package (`spec/packages.md`, PKG15-PKG18) and a format package of the serialization framework
(`spec/serialization.md`, SER1-SER16), so this page says what it reads and writes, where it deliberately differs from
the TOML specification, and which tests witness each rule. The rule IDs are TOML1-TOML16. The package documentation,
`packages/toml/README.md`, repeats the decisions for users.

Rejected input is a `DeError` returned from a `Result`, not a compiler diagnostic, so the rejections are pinned by the
golden fixtures under `tests/toml/` rather than by registry codes (`tests/README.md`). The accepted behavior is pinned
by the same fixtures and by the conformance case `tests/s3_serialize/toml`.

## TOML1. The version

The package reads and writes **TOML 1.0.0**. A construct that TOML 1.0.0 does not have (the 1.1 drafts add newlines in
inline tables, `\e`, and optional seconds in times) is an error in the same place 1.0.0 reports one.

## TOML2. The document

A document is a table: key/value pairs and table headers in any order, and an empty document is the empty table. Lines
end with a line feed or a carriage return and line feed; a carriage return that is not followed by a line feed is an
error. Whitespace is a space or a tab. A comment starts with `#` and runs to the end of its line; a control character
other than tab is an error in a comment. A UTF-8 byte order mark is an error, because it is not whitespace in TOML; the
package does not skip it.

## TOML3. Keys

A bare key is one or more of `A-Z a-z 0-9 _ -`. A quoted key is a basic string (TOML5) or a literal string. A dotted key
`a.b.c` names the table `a`, then `b`, then the key `c`, with optional spaces around each dot. A key may appear once in
its table: a second definition is `duplicate key 'k'`, reported at the second key.

## TOML4. Tables, arrays of tables, and their redefinition rules

A header `[a.b]` defines the table at that path, and `[[a.b]]` appends a new element to the array of tables at that
path. The package follows the TOML 1.0.0 rules for what may be defined where:

- A header may define a table that an earlier header or dotted key only implied (`[a.b]` after `[a.b.c]`). It may not
  define a table twice: `table 'k' is defined twice` at the second header's key.
- A header may not define a table that a dotted key defined: `table 'k' is defined twice`.
- A dotted key creates the tables it names and may extend a table that a header only implied. It may not extend a
  table that a header defined (`table 'k' is defined twice`).
- An inline table is closed: a header or a dotted key may not add to it (`inline table 'k' cannot be extended`).
- `[a]` where `a` is an array of tables is `'a' is an array of tables; use [[a]]`. `[[a]]` where `a` is a table is
  `'a' is a table, not an array of tables`. `[[a]]` or a header through `a` where `a` is a static array is
  `'a' is a static array`.
- A header or dotted key whose path passes through a value that is not a table (a scalar, or a static array) is
  `'a' is not a table`.
- A key that already holds a value (a scalar or a static array) is `duplicate key 'k'`.

Each error is reported at the key that caused it, with its line and column (TOML16).

## TOML5. Strings

There are the four TOML forms: basic `"..."`, literal `'...'`, multi-line basic `"""..."""`, and multi-line literal
`'''...'''`. A basic string is one line. A basic string's escapes are `\b \t \n \f \r \" \\`, `\uXXXX`, and
`\UXXXXXXXX`; any other backslash is `invalid escape`, at the backslash. A `\u` or `\U` escape must name a Unicode scalar
value (no surrogate, nothing past `U+10FFFF`); otherwise, or with a non-hex digit, it is `invalid unicode escape`, at the
backslash. A control character other than tab is `control character in a string` at that character, and in a single-line
string so is a line break, which ends the string as `unterminated string`.

In a multi-line string a line break (line feed, or carriage return and line feed) is allowed. A line break right after
the opening delimiter is removed. In a multi-line basic string a backslash at the end of a line removes the line break
and the white space (spaces, tabs, and line breaks) that follows it. Up to two adjacent quotes may appear inside a
multi-line string, and up to two more may come before its closing delimiter. A carriage return and line feed inside a
multi-line string reads as a line feed. A string that is still open at the end of the input is `unterminated string`,
at its opening delimiter.

## TOML6. Integers

An integer is 64-bit and signed: from `-9223372036854775808` to `9223372036854775807`. A decimal integer has an optional
`+` or `-`, digits with single underscores between them, and no leading zero (except `0` itself). A hexadecimal (`0x`),
octal (`0o`), or binary (`0b`) integer has no sign and digits with single underscores between them. A value outside the
range is `integer out of range`, including a hexadecimal, octal, or binary value that would set the sign bit: the package
does not wrap it into a negative number. A malformed integer is `invalid number`.

## TOML7. Floats

A float is an IEEE 754 binary64 number. The literal has an optional sign, digits on both sides of the point, and an
optional exponent (`e` or `E`, an optional sign, and digits), with single underscores between digits; `inf` and `nan`
take an optional sign. A finite literal whose value is past the range of `f64` (`1e400`) is `float out of range`; one
below it rounds to zero, as the C library does. A malformed float is `invalid number`.

The writer emits a finite float as the shortest text that reads back as the same value, the form `f64` displays: a
float whose text has no point or exponent gets `.0` appended, so `1.0` stays a float. Negative zero is written `-0.0`.
A NaN is written `nan` and the infinities `inf` and `-inf`. The sign of a NaN is not kept.

## TOML8. Booleans

`true` and `false`, and nothing else.

## TOML9. Datetimes

There are the four TOML forms, and the package reads each into a `Datetime` value:

| Form | Example | `Datetime` variant |
| --- | --- | --- |
| offset date-time | `1979-05-27T07:32:00.999-07:00` | `OffsetDateTime(Date, Time, offset)` |
| local date-time | `1979-05-27T07:32:00` | `LocalDateTime(Date, Time)` |
| local date | `1979-05-27` | `LocalDate(Date)` |
| local time | `07:32:00` | `LocalTime(Time)` |

The delimiter between date and time is `T`, `t`, or a space; an offset is `Z` or `z`, or a sign and `HH:MM`. The offset
is kept as its minutes east of UTC, so `Z` and `+00:00` are the same value. A fraction of the seconds has up to nine
digits (nanoseconds). A tenth digit is `invalid date or time`, because a value finer than a nanosecond cannot be held.
Seconds may be 60 (a leap second). The fields must be valid: month 1 to 12, a day that exists in its month (February 29
only in a leap year), hour 0 to 23, minute 0 to 59, and an offset hour 0 to 23 and minute 0 to 59. Any other value is
`invalid date or time`, at the value.

The canonical text (TOML13) writes the `T` delimiter, `Z` for a zero offset and `+HH:MM`/`-HH:MM` otherwise, and a
fraction without trailing zeros, omitted when it is zero. The spelling of the input is not kept.

## TOML10. Arrays

A static array is `[ value, ... ]`. Its elements may have different types (TOML 1.0.0 allows a mixed array). Commas
separate the elements, a trailing comma is allowed, and line breaks and comments may appear between the elements. A
missing separator is `expected ',' or ']'` at the token where the separator was expected. An array still open at the end
of the input is `unterminated array`, at its opening bracket.

## TOML11. Inline tables

An inline table is `{ key = value, ... }` on one line; `{}` is empty. A trailing comma is `a trailing comma is not
allowed in an inline table`, at the closing brace. A line break inside one is `a line break is not allowed in an inline
table`, at the break. An inline table still open at the end of the input is `unterminated inline table`, at its opening
brace. Dotted keys are allowed inside, and the table is closed (TOML4).

## TOML12. What is deliberately not supported

- Anything from TOML 1.1 (TOML1).
- An integer or a datetime outside the ranges of TOML6 and TOML9: an error, never a wrap or a truncation.
- A byte order mark (TOML2).
- More than nine fraction digits of a datetime (TOML9).
- Comments, blank lines, and the spelling of a value (`0xff`, `+00:00`, `1e3`) are not kept by `parse` and `render`:
  the canonical form (TOML13) is what a document reads back as. A document is not edited in place.
- Serialization of a datetime. The framework has no datetime shape (SER1), so a struct with a datetime field is read
  and written through `parse` and `render` (the `Value` tree), not through `from_str` and `to_string`.
- Reading a datetime into a `String` field (TOML15).

## TOML13. The canonical form

`render` (and therefore `to_string`) writes this form, and `parse` followed by `render` is the identity on it:

- The scalar and inline values of a table come first, each as `key = value`, in the table's order. Then each
  sub-table follows as a `[path]` section, in order, and each section is preceded by a blank line (except the first
  line of the document).
- A non-empty array whose elements are all tables is a sequence of `[[path]]` sections, one per element, each with a
  blank line before it. Any other array is written inline.
- A key is written bare when it is one or more of `A-Z a-z 0-9 _ -`; otherwise it is a basic string. A header path
  joins its keys with `.`.
- A basic string escapes `"` and `\`, writes `\b \t \n \f \r`, writes any other control character and DEL as `\uXXXX`
  with lowercase hex digits, and writes every other character as it is.
- An inline array is `[1, 2]` (empty: `[]`); an inline table is `{ a = 1, b = "x" }` (empty: `{}`).
- A map's entries are written sorted by key, in byte order (the map's own iteration order is unspecified). A struct's
  fields keep their declaration order. Two calls with equal values write the same text.
- Floats follow TOML7 and datetimes follow TOML9.

## TOML14. The format side of the framework

The package implements `Serializer` for a writer and `Deserializer` for a reader, and nothing else (SER15). The writer
builds a `Value` tree; `to_string` renders it. Each shape maps as follows:

| Shape | TOML |
| --- | --- |
| unit | an error: `TOML has no unit value` |
| bool | `true` / `false` |
| every signed integer, and `u8`, `u16`, `u32`, `usize` and `u64` up to `i64::MAX` | an integer |
| `u64` or `usize` above `i64::MAX` | an error: `an unsigned integer that does not fit in a TOML integer` |
| `f32`, `f64` | a float (TOML7) |
| `char` | a one-character string |
| `str`, `String` | a string |
| bytes | an array of integers |
| sequence | an array |
| map with `String` keys | a table, its keys sorted |
| struct | a table, its fields in declaration order |
| `Option.Some(value)` | the value itself |
| `Option.None` | the key is omitted from its table (TOML15 says what that means when reading) |
| `Option.None` in an array or in a variant's payload | an error: `an Option.None has no TOML form here` |
| variant with no payload | its name as a string: `"Dot"` |
| variant with one value | `{ Circle = 1.5 }` |
| variant with several values | `{ Rect = [1.5, 2.5] }` |

A top-level value must be a table: a struct, a map, or a variant with a payload. Any other top-level value is
`a TOML document must be a table`.

## TOML15. The reader

The reader works on the `Value` tree that `parse` returns. It is strict about types, because TOML's types are distinct:

- An integer is not read as a float, and a float is not read as an integer (`expected a float, found an integer`).
- A datetime is not read as a string, and a string is not read as a datetime. Reading a datetime through the framework
  is an error (TOML12).
- An integer is checked against its width: `a number that does not fit in i8`. A negative number read into an unsigned
  width is the same error.
- A `char` is a string of exactly one character (`expected one character`).
- Bytes are an array whose elements are integers from 0 to 255.
- An enum is a string (a unit variant) or a table with exactly one key, which names the variant; `{ Rect = [1.5, 2.5] }`
  is the variant `Rect` with two payload values. Any other form is `expected a variant: a string or a table with one key`.
- An `Option` reads a present value as `Option.Some`. An absent key is not a value at all: a derived struct reports
  `missing field` for it (SER6). A hand-written reader may treat an absent key as `Option.None`. This is the one place
  where the TOML form of `Option.None` (its omission, TOML14) and the framework's rule for an absent field disagree, so the
  case is pinned by `tests/toml/write/sample.expected` (the second sample has no `backup`, and reading it back is
  `missing field 'backup'`) and by `tests/toml/config/missing_option`, and is an open decision for the framework, not
  for TOML.

A mismatch is `expected <what>, found <what>` (for example `expected a string, found an integer`), reported at the
line and column of the value (TOML16), with the path of the value added by the framework (SER9).

## TOML16. Errors and their locations

A syntax error has the form `line L, column C: message` (SER10): lines and columns are 1-based and a column counts
Unicode scalar values. The position is the place where the parser found the problem, with these exceptions: an
unterminated string, array, or inline table is reported at its opening character; a duplicate key or a redefined table at its key; a
bad value at its first character; and a position past the last character (the end of the input) is one column after
it. A reader's type error names the line and column of the value it rejects.

## Tests

`tests/check_toml.py` runs the golden fixtures against the package and the conformance case:

| Folder | Mode | Rules |
| --- | --- | --- |
| `tests/toml/roundtrip` | `roundtrip`: parse, then render, and print the canonical text | TOML1-TOML11, TOML13 |
| `tests/toml/spans` | `spans`: print each key and each value with its line and column | TOML2-TOML4, TOML16 |
| `tests/toml/bad` | `roundtrip`: print the syntax error, one malformed document per file | TOML3-TOML11, TOML16 |
| `tests/toml/config` | `config`: read a `Config` with `from_str` and print its fields, or the error | TOML14, TOML15 |
| `tests/toml/write` | `write`: write a `Config` with `to_string`, print it, and read it back | TOML13, TOML14, TOML15 |
| `tests/s3_serialize/toml` | conformance (`//@ run`): the same mapping inside the suite | TOML5, TOML9, TOML13-TOML15 |

Every fixture has a `.expected` file next to it. The check also reads each valid `roundtrip` and `spans` fixture with
CRLF line endings and expects the same output, which covers TOML2 without a second set of fixtures.
