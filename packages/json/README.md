# json

JSON reading and writing on the serialization framework (issue #255; the framework is `spec/serialization.md`).
Use it with `use json`; no `burdin.toml` entry is needed.

```burdin
use json

let text = json.to_string(config)?
let config = json.from_str(text.as_str())?
```

## Surface

```
fn to_string<T: Serialize>(value: T): Result<String, DeError>        // compact
fn to_string_pretty<T: Serialize>(value: T): Result<String, DeError> // two-space indentation, final newline
fn from_str<T: Deserialize>(text: str): Result<T, DeError>
```

`from_str` consumes the whole text: anything but whitespace after the value is `trailing characters`. A value the
framework cannot write as JSON is an error, never a panic: a float that is not finite has no JSON number, and an
object member without a name or two names in a row cannot be written at all.

The types `Writer` and `Parser` are public so that a hand-written impl can drive them directly; `to_string`,
`to_string_pretty`, and `from_str` are the usual entry points.

## Mapping

| Framework shape | JSON |
| --- | --- |
| unit | `null` |
| `bool` | `true` / `false` |
| every integer width, `f32`, `f64` | a number |
| `char` | a one-character string |
| `str`, `String` | a string |
| bytes (`[u8]`) | an array of numbers |
| sequence | an array |
| map with `String` keys | an object |
| struct | an object; the struct's name is not written |
| `Option.None` | `null` |
| `Option.Some(value)` | the value itself |
| variant with no payload | its name as a string: `"Dot"` |
| variant with one value | `{"Circle": 1.5}` |
| variant with several values | `{"Rect": [1.5, 2.5]}` |

`Option` is the one enum with a special form, which is what makes `null` mean `Option.None`. Reading `Option` accepts
`null` for `Option.None` and any other value for `Option.Some`. A JSON number that is not finite cannot be represented, so a
`1e999` is an error rather than an infinity.

## Errors

Every reader error is a `DeError` with a message, and a syntax error also has a line and a 1-based column counted in
Unicode scalar values (`line 1, column 12: trailing comma`). The path of the offending value inside the data is
added as the error unwinds (`missing field 'y' at servers[1]`, `a number that does not fit in u16 at
servers[0].port`). Duplicate names are rejected: a struct field and a map key may appear only once in one object.

Reading never panics on malformed text: truncated input, bad escapes, a lone surrogate, and a control character
inside a string are all reports. Writing reports a value JSON has no form for instead of producing invalid text.

## Tests

`tests/check_json.py` drives this package through fixture files: `tests/json/roundtrip` (parse and re-emit),
`tests/json/pretty`, and `tests/json/bad` (one malformed document per file, with its exact message in an
`.expected` file). `tests/s3_serialize/json` is the conformance case that runs the same mapping inside the
conformance suite.
