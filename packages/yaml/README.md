# yaml

YAML reading and writing on the serialization framework (issue #258; the framework is `spec/serialization.md`).
Declare it in `burdin.toml` and `use yaml`:

```toml
[dependencies]
yaml = true
```

```burdin
use yaml

let text = yaml.to_string(config)?
let config = yaml.from_str(text.as_str())?
```

This is a **safe subset** of YAML 1.2 for configuration and data files: it can read what people write by hand, it can
never build a value of a type the program did not ask for, and everything outside the subset is an error with a
line and column, never a silent guess. The subset is listed below, with what is rejected.

## Surface

```
fn to_string<T: Serialize>(value: T): Result<String, DeError>   // block style, ends in a line feed
fn from_str<T: Deserialize>(text: str): Result<T, DeError>
```

`from_str` reads one document and consumes all the text. The types `Loader` (text to a tree), `Reader` (the tree as a
`Deserializer`), and `Writer` (a `Serializer`) are public so that a hand-written impl can drive them directly;
`to_string` and `from_str` are the usual entry points.

## What is supported

- **One document.** An optional `---` first line (a scalar, a flow collection, an alias, or a block scalar may follow
  it on the same line) and an optional `...` last line. Comments, from a `#` at the start of a line or after white
  space, anywhere between tokens. LF, CRLF, and CR line breaks, and a byte order mark at the start.
- **Block mappings and sequences**, nested by indentation (spaces only), including the compact forms `- key: value`,
  `- - item`, and a sequence that sits at the same indentation as the key that owns it:

  ```yaml
  servers:
  - host: a
    port: 80
  ```

- **Flow mappings and sequences**: `[a, b]` and `{a: 1, b: 2}`, nested, spread over several lines, with comments and a
  trailing comma. A mapping key with no value (`{a, b: 1}`) has a null value.
- **Plain scalars**, which may continue over following lines that are indented more than their parent (a line break
  folds into a space and a blank line is a line feed); **single-quoted** scalars (`''` is a quote); **double-quoted**
  scalars with every YAML 1.2 escape (`\0 \a \b \t \n \v \f \r \e \" \/ \\ \N \_ \L \P \xNN \uNNNN \UNNNNNNNN` and the
  escaped space and line break), where a line break folds like it does in a plain scalar.
- **Block scalars**, literal `|` and folded `>`, with the indentation indicator (`|2`) and the chomping indicator
  (`|-` strips the final line breaks, `|+` keeps them, and the default clips to one). A more-indented line inside a
  folded scalar keeps its line breaks.
- **Anchors and aliases** (`&name`, `*name`), as values in block and flow collections. An alias is resolved while the
  text is loaded: it becomes a copy of the anchored node, and the copying is bounded (see Limits). An alias names an
  anchor that was defined earlier and is complete, so a node cannot contain itself; defining a name again replaces it
  for the aliases that follow. An anchor goes before the node on its line, or on a line of its own before a block
  collection (`key: &a` and then the indented collection).
- **The `!!str` tag**, which makes a scalar a string whatever it looks like (`!!str 123`).
- Keys are plain or quoted scalars; a key that looks like a number or a boolean is just its text.

## What is rejected

Each of these is an error that names the line and column, and none is skipped or guessed:

| Input | Why |
| --- | --- |
| a tag other than `!!str` (`!!python/object:...`, `!!binary`, `!!set`, `!Custom`, `!<verbatim>`) | no tag constructs an arbitrary type; reading is driven by the type the program asked for |
| a `%` directive (`%YAML 1.2`, `%TAG`) | directives change how the rest is read; not part of the subset |
| a second document (`---` after the first, or content after `...`) | `from_str` reads one value |
| a merge key `<<` | it would silently add entries; a quoted `"<<"` is an ordinary key |
| a complex key (`? ` and a flow collection or alias as a key) | keys are strings |
| an anchor or tag on the same line as a mapping key (`&a key: v`) | it is ambiguous whether it names the key or the mapping; put it on its own line |
| `a: b: c`, `a: - b`, and `[a: 1]` | a block mapping or sequence where a value belongs, and a single-pair mapping in a flow sequence |
| `*name` of an anchor that was not defined earlier, `&a [*a]` | no forward or recursive references |
| a duplicate key in one mapping, for a struct field or a map key alike | |
| a tab in indentation, a control character anywhere (other than tab and line feed) | |
| an unterminated quote or flow collection, an invalid escape, a `\u` that is a surrogate | |
| the reserved indicators `@` and `` ` `` at the start of a node, a `|` or `>` inside a flow collection | |
| nesting deeper than 100 levels | a bound on recursion |
| aliases that copy more than 100000 nodes or 4 MiB of scalar text in one document | an alias bomb ("billion laughs") |

Also not supported: types such as timestamps, sets, ordered maps, and binary data (an untagged timestamp reads as a
string, and the tags that name the others are rejected above), and input that is not UTF-8 (`from_str` takes a `str`).

## Mapping

| Framework shape | YAML |
| --- | --- |
| unit | `null` |
| `bool` | `true` / `false` |
| every integer width | a decimal number |
| `f32`, `f64` | a number; `.inf`, `-.inf`, and `.nan` for the values that are not finite |
| `char`, `str`, `String` | a string |
| bytes (`[u8]`) | a sequence of numbers |
| sequence, tuple | a sequence |
| map with `String` keys | a mapping, its keys in sorted order (a map has no order of its own) |
| struct | a mapping, its fields in declaration order; the struct's name is not written |
| `Option.None` | `null` |
| `Option.Some(value)` | the value itself |
| variant with no payload | its name as a string: `Dot` |
| variant with one value | `Circle: 1.5` |
| variant with several values | `Rect:` and a sequence of the values |

Reading accepts the other spellings of the same thing: `Option` takes any null (below) for `None` and any other value
for `Some`.

### How a scalar is read

A scalar is converted when the type asks for a number or a boolean, so the same text reads as a string in one field and
as a number in another. Only an **unquoted** scalar converts: a quoted scalar, a block scalar, and a `!!str` scalar are
always strings, and a `"80"` where a number is expected is an error.

- **null**: `~`, `null`, `Null`, `NULL`, or no value at all (`key:`). It reads as `None` and as `()`.
- **bool**: `true`, `True`, `TRUE`, `false`, `False`, `FALSE`. YAML 1.1's `yes`, `no`, `on`, and `off` are *not*
  booleans, so a `bool` field given `yes` is an error. (The writer quotes those words in a string, so a file that
  other YAML 1.1 tools read gets a string back.)
- **integer**: an optional sign and decimal digits (`-12`, `+5`, `007` is 7), `0x` and hexadecimal digits, or `0o`
  and octal digits (those two without a sign). No `_` separators. A number outside the type's range is
  `a number that does not fit in u16`.
- **float**: a decimal with an optional fraction and exponent (`1`, `.5`, `5.`, `-1.5e3`), `.inf`, `-.inf`, `.nan` (also
  `.Inf`, `.INF`, `.NaN`, `.NAN`), and any integer. A decimal that overflows the type is an error, not an infinity.
- **string**: any scalar, as its text: `123` reads as `"123"` and `null` as `"null"` for a `String` field. A missing
  value reads as the empty string.
- **char**: a scalar of exactly one character.

A collection is never converted to a scalar, a scalar is never converted to a collection (`items:` with no value is
not an empty list), and a mapping that is not a struct's fields is not a struct.

### How a string is written

A string is written plain when it would read back as the same string, and double-quoted otherwise: when it is empty,
starts with anything but a letter, `_`, `/`, or a non-ASCII character (so every number, `-`, `?`, `:`, `#`, `&`, `*`,
`!`, `|`, `>`, `'`, `"`, `%`, `@`, and `.inf` is quoted), ends in white space or `:`, holds `: `, ` #`, a flow
indicator (`,[]{}`), or a control character, or is one of `null`, `true`, `false`, `yes`, `no`, `on`, `off`, `y`, `n`
in any of their usual letter cases. The quoted form escapes `"`, `\`, line breaks, tabs, and other control characters.
The writer never uses block scalars, anchors, tags, or flow collections (except `[]` and `{}` for empty ones), and
never writes the `---` line.

## Errors

Every error is a `DeError` with a message, and an error in the text also has the line and a 1-based column, counted
in Unicode scalar values (`line 3, column 7: a number that does not fit in u16`). The path of the offending value inside
the data is added as the error unwinds (`missing field 'y' at servers[1]`, `expected a boolean at servers[0].tls`).
A reading error is located at the start of the node it is about; for a node that came from an alias that is the
anchored node, not the alias. The framework's own errors (a missing, unknown, or duplicate field, and an unknown
variant) carry the path but no line and column, as in the other formats.

Reading never panics on malformed text: truncated input, a bad escape, wrong indentation, an alias bomb, and a very deep
document are all reports, and `tests/check_yaml.py` runs a fixture for each.

## Limits

| Limit | Value |
| --- | --- |
| nesting depth of nodes, block and flow together (down to the innermost scalar) | 100 |
| nodes added by aliases in one document | 100000 |
| scalar text added by aliases in one document | 4 MiB |

The document itself, outside aliases, has no size limit. The first two limits apply to the loaded text, so they hold
whatever type the program reads.

## Tests

`tests/check_yaml.py` drives this package through fixture files: `tests/yaml/roundtrip`, `tests/yaml/scalars`
(block scalars, quoted scalars, and what looks like another type), `tests/yaml/anchors`, `tests/yaml/nested`
(tuples, a list of lists, the float cases), and `tests/yaml/bad` (one malformed document per file), each with its exact
output in an `.expected` file. It also reads the same documents with CRLF line breaks and with a byte order mark, and
two large documents, and checks `burdin add yaml` and the lockfile pin. `tests/s3_serialize/yaml` is the conformance
case that runs the mapping inside the conformance suite.
