# toml

TOML 1.0.0 reading and writing (stage 3, issue #256; `spec/toml.md`) on the serialization framework
(`spec/serialization.md`). Use it with `use toml`; no `burdin.toml` entry is needed.

```burdin
use toml

let text = toml.to_string(config)?
let config = toml.from_str(text.as_str())?
```

## Surface

```
fn parse(text: str): Result<Value, DeError>
fn render(value: Value): Result<String, DeError>
fn to_value<T: Serialize>(value: T): Result<Value, DeError>
fn from_value<T: Deserialize>(value: Value): Result<T, DeError>
fn to_string<T: Serialize>(value: T): Result<String, DeError>
fn from_str<T: Deserialize>(text: str): Result<T, DeError>
```

`parse` reads a document into the `Value` tree (`Str`, `Int`, `Float`, `Bool`, `Datetime`, `Array`, `Table`), keeping
where each key and each value starts (`Key`/`Entry` and `Item` carry a `Span`). `render` writes a `Value` in the
canonical form below. `to_string` is `to_value` followed by `render`; `from_str` is `parse` followed by `from_value`.

The types `Value`, `Item`, `Entry`, `Span`, `Datetime`, `Date`, and `Time` are public, and so are `Writer` and `Reader`,
so a hand-written impl can drive them directly.

A datetime has no shape in the serialization framework, so a struct with a datetime field is read and written through
`parse` and `render`, not through `from_str` and `to_string`.

## Mapping

| Framework shape | TOML |
| --- | --- |
| unit | an error: `TOML has no unit value` |
| `bool` | `true` / `false` |
| every signed integer, and unsigned integers up to `i64::MAX` | an integer |
| an unsigned integer above `i64::MAX` | an error |
| `f32`, `f64` | a float (a non-finite one is `nan`, `inf`, or `-inf`) |
| `char` | a one-character string |
| `str`, `String` | a string |
| bytes | an array of integers |
| sequence | an array |
| map with `String` keys | a table, its keys sorted |
| struct | a table, its fields in declaration order |
| `Option.Some(value)` | the value itself |
| `Option.None` | the key is left out of its table |
| variant with no payload | its name as a string: `"Dot"` |
| variant with one value | `{ Circle = 1.5 }` |
| variant with several values | `{ Rect = [1.5, 2.5] }` |

A top-level value must be a table: a struct, a map, or a variant with a payload.

`Option.None` is written by omitting the key. Reading reports it as `missing field`, because an absent key is not a value: the
framework's derived reader has no way to tell `Option.None` from a field the document never mentioned. Write the field as a
variant, or hand-write the impl, when the distinction matters.

## The subset

The package reads and writes **TOML 1.0.0** and rejects everything TOML 1.0.0 rejects. In particular:

- Integers are 64-bit and signed. A hexadecimal, octal, or binary value that would set the sign bit is
  `integer out of range`, never a wrapped negative.
- Floats are IEEE 754 binary64. A finite literal past the range (`1e400`) is `float out of range`.
- A datetime keeps up to nine fraction digits. A tenth is `invalid date or time`. The four forms are `OffsetDateTime`,
  `LocalDateTime`, `LocalDate`, and `LocalTime`; an offset is kept in minutes east of UTC.
- Comments, blank lines, and the spelling of a value (`0xff`, `+00:00`, `1e3`) are not kept. `parse` followed by
  `render` yields the canonical form, and a document is not edited in place.
- A UTF-8 byte order mark is an error, because TOML does not treat it as white space.

## The canonical form

`render` writes scalars and inline values of a table first, then its sub-tables as `[path]` sections, each preceded by a
blank line. A non-empty array whose elements are all tables becomes one `[[path]]` section per element. A key is written
bare when it can be, and as a basic string otherwise. A basic string escapes `"` and `\`, uses `\b \t \n \f \r`, and
writes other control characters as `\uXXXX`. A finite float always has a point or an exponent, so `1.0` stays a float;
negative zero is `-0.0`. Datetimes write `T` between date and time, `Z` for a zero offset, and a fraction without
trailing zeros. A map's keys are sorted; a struct's keep their declaration order. Two equal values always write the same
text.

## Errors

Every error is a `DeError`. A syntax error reads `line L, column C: message`, with a 1-based column counted in Unicode
scalar values. Reading never panics on malformed text. A type error in the reader is `expected <one>, found <other>`
reported at the value's line and column, and the framework adds the path of the value as it unwinds
(`expected an integer, found a string at servers[0].port`).

The reader is strict, because TOML's types are distinct: an integer is not read as a float or a float as an integer, a
datetime is not read as a string, an integer is checked against the width it is read into, and a `char` must be a string
of exactly one character. An enum is a string (a unit variant) or a table with exactly one key.

## Tests

`tests/check_toml.py` drives this package through fixture files: `tests/toml/roundtrip` (parse, then render),
`tests/toml/spans` (each key and value with its position), `tests/toml/bad` (one malformed document per file with its
exact message), `tests/toml/config` (reading a `Config` through the framework, one fixture per rule), and
`tests/toml/write` (writing a `Config` and reading it back). `tests/s3_serialize/toml` is the conformance case that runs
the mapping inside the conformance suite.
