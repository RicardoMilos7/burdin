# Serialization

Stage 3's serialization framework (issues #297, #298, #299, and #300). This page settles the data model a format sees, the two
sides (`Serialize`/`Deserialize` and the format-side `Serializer`/`Deserializer`), the error type and its path, the
zero-copy story, and the compiler-generated derives. The framework core is ordinary Burdin in `std/src/serialize.bd`;
the format packages (json, toml, csv, yaml, xml) are ordinary packages under `packages/` that implement only the
format side.

The rule IDs are SER1-SER16, by the convention of the other stage pages. The tests are in `tests/s3_serialize/`. The
`Serialize`/`Deserialize` derives are specified and implemented here (SER12). The rest of the page is true of the
code in `std/src/serialize.bd` today.

## SER1. The data model

A format sees one of twelve shapes. They are the whole vocabulary; there is no generic "value" tree and no schema
object.

| Shape | Data |
| --- | --- |
| unit | nothing |
| bool | `false` or `true` |
| signed integer | the value, and the width it was written with (i8, i16, i32, i64, or isize) |
| unsigned integer | the value, and the width (u8, u16, u32, u64, or usize) |
| float | the value, and the precision (f32 or f64) |
| char | one Unicode scalar value |
| string | a run of UTF-8 text |
| bytes | a run of bytes |
| sequence | a length and that many values, in order |
| map | a length and that many `(String, value)` entries, in order |
| struct | a type name, a length, and that many `(String, value)` fields, in order |
| enum variant | a type name, a variant name, the variant's declaration index, and a payload of zero or more values, in order |

The width is part of the call, not of a single "integer" shape, so a binary format can write an `i32` in four bytes
and a JSON format can write every integer as a JSON number. A format may reject a shape or a width it cannot
represent and report it as a `DeError` (SER9); the framework does not force every shape on every format.

A variant's payload is a sequence of values. A unit variant has an empty payload, a tuple (newtype) variant has one or
more values, and a struct variant's fields are written **positionally in declaration order** (their names are not
part of the payload in v1; named variant fields are deferred, SER16). The declaration index is the variant's position
in the `enum` declaration, from 0; it is metadata a format such as a binary encoding may use.

## SER2. Which Burdin type is which shape

| Burdin type | Shape |
| --- | --- |
| `()` | unit |
| `bool` | bool |
| `i8`, `i16`, `i32`, `i64`, `isize` | signed integer, at that width |
| `u8`, `u16`, `u32`, `u64`, `usize` | unsigned integer, at that width |
| `f32`, `f64` | float, at that precision |
| `char` | char |
| `str`, `String` | string |
| `List<u8>` (and `[u8]` as a view argument) | bytes |
| `List<T>` for any other `T` | sequence |
| `Map<String, V>` | map |
| a struct | struct |
| an enum | enum variant |
| `Option<T>` | enum variant: type `Option`, `Some` (index 0) with one payload value, `None` (index 1) with none |
| `Result<T, E>` | enum variant: type `Result`, `Ok` (index 0) with one payload value, `Err` (index 1) with one |
| a tuple `(A, B)`, `(A, B, C)`, `(A, B, C, D)` | sequence of its parts, in order |
| an array `[T; N]` | sequence of its elements, in order |
| a reference or view (`&T`, `&mut T`, `str`, `[T]`) | not serializable (E0120 at a derive; a hand-written impl cannot name it in a field) |
| `any I`, a closure, `Heap<T>` | not serializable in v1 (SER16) |
| `Map<K, V>` where `K` is not `String` | rejected when the impl is written (SER7) |

The mapping is a property of the type's implementation, not of the shape vocabulary: a type that is not in this table
participates by implementing `Serialize`/`Deserialize` by hand (SER12). The compiler-provided impls that make the
table's mappings automatic are the next phase (SER12). Until they land, an impl block selects the shape by calling the
matching helper (SER14), and a derived impl calls the scalar helpers directly and writes the container shapes out
itself (SER12). A format may narrow the mapping: JSON has no
integer width (every integer is one JSON number) and no char (a char is a one-character JSON string); TOML has no unit
and no top-level scalar; CSV has only flat records of scalars. Those choices belong to the format package and are
reported as a `DeError` when a value cannot be written.

## SER3. The two sides

`std.serialize` declares the user-facing interfaces:

```
interface Serialize {
    fn serialize<S: Serializer>(self, mut s: S): Result<(), DeError>
}

interface Deserialize {
    fn deserialize<D: Deserializer>(mut d: D): Result<Self, DeError>
}
```

`Serialize::serialize` borrows the value (`self`) and borrows the serializer exclusively for the call (`mut s`). It
writes exactly one value and returns `Ok(())`. `Deserialize::deserialize` is an associated function: an impl block for
`T` returns `Result<T, DeError>`, and a call is written `T.deserialize(mut d)`. Both methods are generic over the
format type, so dispatch is static and a format is monomorphized like any other generic argument. The interfaces are
ordinary `std` interfaces; a user or a package writes an impl block by hand, and the compiler generates one for a
type that derives the interface (SER12). The compiler-provided impls for the built-in types are the next phase
(SER14).

A generic type adds one bound per type parameter, as every derive does:

```
Syslog<T>: Serialize where T: Serialize { ... }
Syslog<T>: Deserialize where T: Deserialize { ... }
```

## SER4. `Serializer`: how a format writes

A format implements `Serializer`. Every method takes `mut self` (the format is state) and returns `Result<(), DeError>`.

```
interface Serializer {
    fn serialize_unit(mut self): Result<(), DeError>
    fn serialize_bool(mut self, value: bool): Result<(), DeError>

    fn serialize_i8(mut self, value: i8): Result<(), DeError>
    fn serialize_i16(mut self, value: i16): Result<(), DeError>
    fn serialize_i32(mut self, value: i32): Result<(), DeError>
    fn serialize_i64(mut self, value: i64): Result<(), DeError>
    fn serialize_isize(mut self, value: isize): Result<(), DeError>

    fn serialize_u8(mut self, value: u8): Result<(), DeError>
    fn serialize_u16(mut self, value: u16): Result<(), DeError>
    fn serialize_u32(mut self, value: u32): Result<(), DeError>
    fn serialize_u64(mut self, value: u64): Result<(), DeError>
    fn serialize_usize(mut self, value: usize): Result<(), DeError>

    fn serialize_f32(mut self, value: f32): Result<(), DeError>
    fn serialize_f64(mut self, value: f64): Result<(), DeError>

    fn serialize_char(mut self, value: char): Result<(), DeError>
    fn serialize_str(mut self, value: str): Result<(), DeError>
    fn serialize_bytes(mut self, value: [u8]): Result<(), DeError>

    fn begin_seq(mut self, len: usize): Result<(), DeError>
    fn end_seq(mut self): Result<(), DeError>

    fn begin_map(mut self, len: usize): Result<(), DeError>
    fn map_key(mut self, key: str): Result<(), DeError>
    fn end_map(mut self): Result<(), DeError>

    fn begin_struct(mut self, name: str, len: usize): Result<(), DeError>
    fn struct_field(mut self, name: str): Result<(), DeError>
    fn end_struct(mut self): Result<(), DeError>

    fn begin_variant(
        mut self,
        type_name: str,
        variant: str,
        index: usize,
        arity: usize,
    ): Result<(), DeError>
    fn end_variant(mut self, arity: usize): Result<(), DeError>
}
```

- A scalar method writes exactly one value. A sequence is `begin_seq(len)`, then one `serialize`-call per element,
  then `end_seq()`. A map is `begin_map(len)`, then for each entry `map_key(key)` followed by the value's
  `serialize`, then `end_map()`. A struct is `begin_struct(name, len)`, then for each field `struct_field(name)`
  followed by the value's `serialize`, then `end_struct()`.
- A variant is `begin_variant(type_name, variant, index, arity)`, then `arity` payload values, then
  `end_variant(arity)`. `arity` is `0` for a unit variant.
- `len` and `arity` are exact. A format that writes a length prefix uses them; a format that does not (JSON, the
  test format) ignores them. An implementation that writes a different number of elements than it announced is a
  bug in that implementation; the framework does not check it.
- The methods must be called in the structure above. In particular `map_key` comes immediately before the value it
  names and `struct_field` immediately before the field's value. The test format's writer checks this and reports a
  `DeError` for a violation; other formats may rely on it.

## SER5. `Deserializer`: how a format reads

A format implements `Deserializer`. The data type drives the parse: it asks the deserializer for the next token and
builds itself. Every method takes `mut self` and returns a `Result`.

```
interface Deserializer {
    fn deserialize_unit(mut self): Result<(), DeError>
    fn deserialize_bool(mut self): Result<bool, DeError>

    fn deserialize_i8(mut self): Result<i8, DeError>
    fn deserialize_i16(mut self): Result<i16, DeError>
    fn deserialize_i32(mut self): Result<i32, DeError>
    fn deserialize_i64(mut self): Result<i64, DeError>
    fn deserialize_isize(mut self): Result<isize, DeError>

    fn deserialize_u8(mut self): Result<u8, DeError>
    fn deserialize_u16(mut self): Result<u16, DeError>
    fn deserialize_u32(mut self): Result<u32, DeError>
    fn deserialize_u64(mut self): Result<u64, DeError>
    fn deserialize_usize(mut self): Result<usize, DeError>

    fn deserialize_f32(mut self): Result<f32, DeError>
    fn deserialize_f64(mut self): Result<f64, DeError>

    fn deserialize_char(mut self): Result<char, DeError>
    fn deserialize_str(mut self): Result<String, DeError>
    fn deserialize_bytes(mut self): Result<List<u8>, DeError>

    fn begin_seq(mut self): Result<(), DeError>
    fn has_next_seq_element(mut self): Result<bool, DeError>
    fn end_seq(mut self): Result<(), DeError>

    fn begin_map(mut self): Result<(), DeError>
    fn has_next_map_entry(mut self): Result<bool, DeError>
    fn map_key(mut self): Result<String, DeError>
    fn end_map(mut self): Result<(), DeError>

    fn begin_struct(mut self, name: str): Result<(), DeError>
    fn has_next_field(mut self): Result<bool, DeError>
    fn field_name(mut self): Result<String, DeError>
    fn end_struct(mut self): Result<(), DeError>

    fn begin_enum(mut self, name: str): Result<(), DeError>
    fn variant_name(mut self): Result<String, DeError>
    fn begin_variant_payload(mut self, arity: usize): Result<(), DeError>
    fn variant_payload_field(mut self, index: usize): Result<(), DeError>
    fn end_variant_payload(mut self, arity: usize): Result<(), DeError>
}
```

- `begin_seq()` consumes the sequence's opening; `has_next_seq_element()` answers whether another element follows and
  consumes the separator between elements; `end_seq()` consumes the closing. A reader loops
  `while has_next_seq_element()? { element }`. The same shape is used for maps (`begin_map`, `has_next_map_entry`,
  `map_key`, `end_map`) and structs (`begin_struct`, `has_next_field`, `field_name`, `end_struct`).
- `begin_struct(name)` and `begin_enum(name)` are given the expected type name; a self-describing format checks it and
  reports a mismatch, and a format without type names ignores it.
- An enum reader calls `begin_enum(type_name)`, then `variant_name()`, then
  `begin_variant_payload(arity)`, then `variant_payload_field(i)` and one value for each payload index
  `i` from 0 to `arity - 1`, then `end_variant_payload(arity)`. `variant_payload_field(0)` consumes nothing;
  a later index consumes the separator before its value. The writer needs no matching call because it places the
  separator itself.
- A missing value (the input ends, a sequence ends early, a field value is absent) is reported by the method that
  needed it, as a `DeError` with the format's line and column (SER10).

The framework provides free functions that wrap each scalar and the built-in containers, so an impl block and the
generated derives call the same code (SER14):

```
fn serialize_bool<S: Serializer>(mut s: S, value: bool): Result<(), DeError>          // and one per scalar
fn deserialize_bool<D: Deserializer>(mut d: D): Result<bool, DeError>                 // and one per scalar
fn serialize_list<T: Serialize, S: Serializer>(value: List<T>, mut s: S): Result<(), DeError>
fn deserialize_list<T: Deserialize, D: Deserializer>(mut d: D): Result<List<T>, DeError>
fn serialize_map<V: Serialize, A: Allocator, S: Serializer>(value: Map<String, V, A>, mut s: S): Result<(), DeError>
fn deserialize_map<V: Deserialize, D: Deserializer>(mut d: D): Result<Map<String, V>, DeError>
fn serialize_option<T: Serialize, S: Serializer>(value: Option<T>, mut s: S): Result<(), DeError>
fn deserialize_option<T: Deserialize, D: Deserializer>(mut d: D): Result<Option<T>, DeError>
fn serialize_result<T: Serialize, E: Serialize, S: Serializer>(value: Result<T, E>, mut s: S): Result<(), DeError>
fn deserialize_result<T: Deserialize, E: Deserialize, D: Deserializer>(mut d: D): Result<Result<T, E>, DeError>
fn serialize_tuple2<A: Serialize, B: Serialize, S: Serializer>(value: (A, B), mut s: S): Result<(), DeError>
fn deserialize_tuple2<A: Deserialize, B: Deserialize, D: Deserializer>(mut d: D): Result<(A, B), DeError>
// tuple3 and tuple4 likewise; serialize_bytes and deserialize_bytes exist for the bytes shape
```

## SER6. Structs, fields, and the three field errors

A struct is three calls: `begin_struct(name, len)`, one `struct_field(name)` + value for each field, and
`end_struct()`. The names are the field names as written, and a derived impl writes them in declaration order. There
is no field index in the data model; a format that numbers fields uses its own counter.

A derived reader accepts the fields in **any order**: it reads `field_name()`, matches the name against the
declaration, deserializes the value with the matching field's type, and remembers that the field was seen. After
`has_next_field()` is false it checks that every field was seen. A hand-written impl may do the same or may require
declaration order; nothing in the interface forces either.

The three failures have fixed messages and are produced by helpers in `std.serialize`:

| Situation | Message | Helper | Path |
| --- | --- | --- | --- |
| a name that is not a field | `unknown field '<name>'` | `unknown_field(name)` | extended with that field |
| a field that appears twice | `duplicate field '<name>'` | `duplicate_field(name)` | extended with that field |
| a field that never appeared | `missing field '<name>'` | `missing_field(name)` | not extended |

The path of an unknown or duplicate field includes the field name, because the name was read. A missing field's path
is the struct's own path: the caller that reads the struct extends the error with the field or index that holds the
struct. A derived reader reports the first violation in input order.

## SER7. Sequences, maps, and string keys

A sequence is `begin_seq(len)` / elements / `end_seq()`. A list is written by `serialize_list` and read by
`deserialize_list`; a tuple and an array use the same shape. An element failure is extended with its index
(`DeError.index(i)`).

A map is `begin_map(len)` / (`map_key(key)`, value) / `end_map()`. **Map keys are `String` in v1.** `map_key` takes a
`str` and the reader returns a `String`, so a format writes the key as a string shape and reads it back as one. An
impl block for a `Map<K, V>` whose `K` is not `String` is rejected when it is checked: the compiler has no
`Serialize` for `K`, and the helper is typed `Map<String, V>`. Other key types (integers, enums, a `Map` of a user
key) are deferred (SER16); a map with a non-string key is not representable in JSON or TOML anyway.

A map value failure is extended with its key (`DeError.key(k)`). Map iteration order is unspecified, so a writer that
wants deterministic text sorts its keys itself; the framework does not.

## SER8. Enums and externally tagged variants

The default representation is **externally tagged**: the variant name is the tag and the payload is beside it. An
enum writer calls `begin_variant(type_name, variant, index, arity)`, writes the payload values, and calls
`end_variant(arity)`. A unit variant has `arity` 0. A derived writer uses the variant's declaration order for the
index and writes its fields in declaration order.

An enum reader calls `begin_enum(type_name)`, reads `variant_name()`, then `begin_variant_payload(arity)`, then
`variant_payload_field(i)` and value `i` for each payload index, then `end_variant_payload(arity)`. It matches the
name against the declaration; a name it does not know is
`unknown variant '<name>' of <Type>` (`unknown_variant(type_name, name)`). A payload value failure is extended
with its index and then the variant name (`DeError.index(i).variant(name)`), so a tuple variant's payload paths look
like `Shape.Circle[0].x`; a struct variant's payload is positional, so its fields are reached the same way.

Because v1 writes a variant's payload positionally, a struct variant loses its field names in the payload (SER1).
Formats that need named variant fields are deferred (SER16).

## SER9. Errors with a path

There is one error type for both sides:

```
struct DeError {
    message: String
    path: String
}
```

`message` is the format's or the data model's description; `path` is the location of the offending value inside the
value being written or read, built from the outside in. `path` is empty when the error is at the value itself.

The operations that extend a path take the error by ownership and return a new one. Each prepends its segment to the
existing path:

| Operation | Segment | Example |
| --- | --- | --- |
| `e.field("port")` | `port` | `port` |
| `e.index(2)` | `[2]` | `[2].port` |
| `e.key("host")` | `["host"]` | `["host"].port` |
| `e.variant("Circle")` | `Circle` | `Circle.x` |

The separator is chosen from the segment being prepended and the existing path: a field or variant name uses `.`
unless the existing path begins with `[`, an index or key adds nothing before a path that already begins with `[`.
Unwinding a value therefore composes to `config.servers[2].port`, `table["host"].name`, and `Shape.Circle[0].x`.
A key containing `"` or `\` is escaped in the path with the same rules as text (SER13), so the path is unambiguous.

The path is attached where the failure is caught, not where it is raised: a reader that reads a field wraps the
error the field produced with `e.field(name)`, a sequence reader wraps with `e.index(i)`, a map reader with
`e.key(k)`, and a variant reader with `e.index(i).variant(name)`. An error that the reader itself raises (a syntax error, a
missing field) is wrapped by its caller the same way. A hand-written impl does the wrapping explicitly; a generated
impl receives the wrapping code (SER12).

`DeError` is an error type: it implements `Display` and the `std.Error` marker, so it boxes into the dynamic `Error`
and `?` converts it in a `fn main(): Result<()>`. `Display` writes `message`, then ` at ` and the path when the path
is non-empty:

```
syntax error: expected a number at config.servers[2].port
```

`e.to_string()` is that text. `DeError.new(message)` makes an error with an empty path, and
`DeError.syntax(message, line, column)` makes one whose message starts with `line L, column C: ` (SER10).

## SER10. A format's own syntax errors

A format reports a syntax error of its input with `DeError.syntax(message, line, column)`, which produces the message
`line <line>, column <column>: <message>`. Lines and columns are 1-based; a column counts Unicode scalar values, not
bytes. The format raises the error where it sees the problem, with an empty path, and the enclosing readers attach
the path as they unwind (SER9). The two compose without a second message field:

```
line 1, column 47: expected a number at config.servers[2].port
```

A format that reads bytes rather than text (a binary format) has no line or column; it uses `DeError.new`.

## SER11. Zero-copy views

Version 1 produces **owned** values. A struct field cannot be a view (`spec/grammar.md`; E0120), a `List` cannot hold
one, and a function cannot return a view derived from a deserializer it consumed, so a deserialized `String` is a new
`String` and a bytes field is a `List<u8>`. The interfaces reflect that: `deserialize_str` returns `String` and
`deserialize_bytes` returns `List<u8>`, not `str`/`[u8]`.

The places where a borrowed view helps are kept:

- `TextDeserializer.new(text)` copies the input once into the `String` it owns, because the reader is a value that
  outlives the call. `from_text` therefore makes one copy of the input text; the values it builds own their own
  storage.
- `TextDeserializer.remaining(self): str` exposes the unread part of the input as a `str` view, so a caller that wants
  to inspect or hand off the rest of a document does not copy it.
- A format may expose a `str` accessor on a value it has already read (for example a map value that is known to be a
  string) as an ordinary method on its own type; the framework's interfaces do not carry it.

Deferred: borrowed `&str`/`&[u8]` outputs, `Cow`-like types, `from_bytes` without a copy, and deserializing into a type
whose fields are views. Those need a view type that can live in a struct, which the language does not have.

## SER12. The derives

The interfaces are ordinary `std` interfaces, so a user writes an impl block by hand (SER3). For a type that wants the
standard representation, the compiler generates the two impl blocks, the same way it generates `Debug`
(`spec/derives.md`):

```
struct Server: Serialize, Deserialize {
    host: String
    port: u16
}
enum Shape: Serialize, Deserialize { Circle(f64), Empty }
```

`: Serialize, Deserialize` is a **built-in derive**, not a user `derive` declaration. It must be, because of two
language facts:

- an impl block's type must be a struct or enum of the package (`crates/burdin_sema/src/impls.rs`), so `std` cannot
  write `impl i32: Serialize` or `impl List<T>: Serialize`; the compiler must provide those impls itself (SER14);
- a user `derive` is checked as ordinary Burdin in the module of the applied type, and reflection has no way to
  construct a struct from a compile-time field list (`x.{name}` reads a field, it does not build one). The derive for
  `Deserialize` must synthesize the field reads and the constructor, which only the compiler can do.

The derives are implemented in two places. `serialize` and `deserialize` are flags on `tir::Derives`
(`crates/burdin_tir/src/defs.rs`), read by `BUILTIN_DERIVES` and `parse_derives`
(`crates/burdin_sema/src/derives.rs`). `expand_derives` (`crates/burdin_syntax/src/derive.rs`) generates one impl
block per name as ordinary Burdin source text and parses it with the same parser as a hand-written block, so a derived
impl is indistinguishable from a written one.

The generated impls are:

- `Name<A, B>: Serialize where A: Serialize, B: Serialize`, and the same with `Deserialize`; each type parameter gets
  one bound, and a const parameter gets none.
- `fn serialize<__S: Serializer>(self, mut s: __S): Result<(), DeError>` and
  `fn deserialize<__D: Deserializer>(mut d: __D): Result<Self, DeError>`. The method's format parameter is always
  `__S`/`__D`, never `S`/`D`: the checker currently lets a method's generic parameter shadow an impl parameter of the
  same name, so a field of the type's own `D` parameter would bind the wrong one. Fixing the scoping is a compiler
  change, not a serialization one.
- A struct writer emits `begin_struct(name, N)`, then `struct_field(name)` and the value of each field in declaration
  order, then `end_struct()`.
- An enum writer emits a `match self` with one arm per variant: `begin_variant(type_name, variant, index, arity)`,
  the payload values (a unit variant has none; a struct variant's fields are written positionally in declaration
  order), then `end_variant(arity)`.
- A struct reader holds every field as an `Option<T>` (there is no default value to hold an unread field of any type),
  loops on `has_next_field()`/`field_name()`, reports `duplicate_field(name)` for a repeated name and
  `unknown_field(name)` for an unknown one, calls `end_struct()`, reports `missing_field(name)` for each remaining
  `None` in declaration order, and builds the value. A field value's error is wrapped with `.field(name)`.
- An enum reader calls `begin_enum(name)` and `variant_name()`, then one branch per variant:
  `begin_variant_payload(arity)`, `variant_payload_field(i)` and value `i`, `end_variant_payload(arity)`, and the
  constructor. A payload value's error is wrapped with `.index(i).variant(name)`; an unknown name is
  `unknown_variant(type_name, name)`.
- A field's type decides the reader and writer. `bool`, every integer and float width, `char`, `String`, unit, and
  `List<u8>` call their helper (`serialize_i32`, `serialize_str`, `serialize_bytes`, `deserialize_i32`, ...); a type
  path (a user type, a type alias, or a type parameter) calls `value.serialize(mut s)` or
  `Type.deserialize(mut d)`. The built-in containers (`List`, `Map<String, V>`, `Option`, `Result`, and tuples of
  2-4) are written out recursively by the generator instead of through the SER14 helpers, because those helpers
  require their element or part type to implement the interface and the compiler does not provide the primitive
  impls yet. The generator issues the same serializer calls and reads the same shapes, so the representation is
  identical; a map value's error is extended with `.key`, and a fixed-arity sequence consumes the separator its
  format places between values.

The fields' requirement is not checked yet: `check_derives` does not add `Serialize`/`Deserialize` to its requirement
list, because a concrete `i32` field does not implement the interface until the compiler-provided impls of SER14 land.
The generated body is correct for such a field today because it calls that field's helper directly. That check, the
compiler-provided impls, and switching the container paths back to the SER14 helpers are the next phase. The
requirement will then be the same as `Debug`: a non-generic type's field must implement the interface (E0130 at the
field), and a generic type gets one `where P: Serialize` / `P: Deserialize` per type parameter.

A hand-written impl may do anything the interfaces allow: a different field order, a renamed field, a different
number representation, a map with another key type (when a future interface supports it), or a completely different
representation. The only requirement is that the impl calls the serializer/deserializer methods in the structure of
SER4-SER8.

## SER13. The test format

`std.serialize` also provides a tiny human-readable format, so the framework can be exercised and a failure can be
inspected before a real format exists. It is not a production format and it is not a package: it lives in `std` with
the interfaces. It is also the format the conformance tests use.

```
value    := unit | bool | integer | float | char | string | bytes | sequence | map | struct | variant
unit     := "()"
bool     := "true" | "false"
integer  := ["-"] digit+
float    := integer [ "." digit+ ] [ ("e" | "E") ["+" | "-"] digit+ ]
          | ["+" | "-"] ("inf" | "infinity" | "NaN" | "nan")      // any letter case
char     := "'" { escape | any character but "'" or "\" } "'"
string   := '"' { escape | any character but '"' or "\" } '"'
bytes    := "[" [ u8 { "," u8 } ] "]"                 // the same text as a sequence of u8
sequence := "[" [ value { "," value } ] "]"
map      := "{" [ string ":" value { "," string ":" value } ] "}"
struct   := name "{" [ name ":" value { "," name ":" value } ] "}"
variant  := name "." name [ "(" [ value { "," value } ] ")" ]

escape   := "\n" | "\r" | "\t" | "\\" | "\0" | "\"" | "\'"
```

`name` is `[A-Za-z_][A-Za-z0-9_]*`. Whitespace (space, tab, line feed, carriage return) is allowed between tokens
and is ignored; no trailing comma is accepted. The output of the writer is the canonical spacing below; the reader
accepts any spacing.

| Value | Writer output |
| --- | --- |
| unit, bool | `()`, `true`, `false` |
| integers | decimal with a leading `-` for a negative signed value |
| floats | the shortest text that reads back as the same value (`1.5`, `2`, `inf`, `NaN`); a negative zero loses its sign |
| char | `'x'`, with the escapes above |
| string | `"text"`, with the escapes above |
| bytes | `[1, 2, 3]` |
| sequence | `[1, 2]` (empty: `[]`) |
| map | `{ "a": 1, "b": 2 }` (empty: `{}`) |
| struct | `Point { x: 1, y: 2 }` (empty: `Point {}`) |
| variant | `Shape.Circle(1.5)`, `Shape.Empty` |

The parser reads the number with the grammar above; the integer or float reader decides whether the token is valid
(a `u8` reader rejects `-1`). A float reader also accepts the non-finite words `inf`, `infinity`, and `nan` in any
letter case, with an optional sign, which is what the writer emits and what `f64.parse` accepts. The char reader
requires exactly one character and the string reader decodes escapes;
an unknown escape is an error. The reader does not accept a type name that differs from the expected struct or enum
name. It reports an unterminated string, a missing token, a bad escape, a trailing comma, a number that does not
parse, and trailing text after the value, each with `line` and `column` (SER10).

The public surface is:

```
struct TextSerializer {
    fn new(): TextSerializer
    fn finish(own self): String
}

struct TextDeserializer {
    fn new(text: str): TextDeserializer
    fn remaining(self): str
    fn end(mut self): Result<(), DeError>       // no non-whitespace text left
}

fn to_text<T: Serialize>(value: T): Result<String, DeError>
fn from_text<T: Deserialize>(text: str): Result<T, DeError>
```

`to_text` writes the value and returns the text; `from_text` reads a value, requires the whole input to be consumed,
and returns it. The format treats a map's entries in iteration order, which is unspecified; a test that compares text
uses at most one entry, or compares values instead.

## SER14. The built-in helper functions

`std.serialize` provides one free function per shape. They are what a hand-written impl calls when a field is a
primitive, and what the compiler's generated impls call; having them in `std` means the generated code is small and
the same code is exercised by the tests.

```
fn serialize_unit<S: Serializer>(mut s: S): Result<(), DeError>
fn serialize_bool<S: Serializer>(mut s: S, value: bool): Result<(), DeError>
fn serialize_i8/i16/i32/i64/isize<S: Serializer>(mut s: S, value: T): Result<(), DeError>
fn serialize_u8/u16/u32/u64/usize<S: Serializer>(mut s: S, value: T): Result<(), DeError>
fn serialize_f32/f64<S: Serializer>(mut s: S, value: T): Result<(), DeError>
fn serialize_char<S: Serializer>(mut s: S, value: char): Result<(), DeError>
fn serialize_str<S: Serializer>(mut s: S, value: str): Result<(), DeError>
fn serialize_bytes<S: Serializer>(mut s: S, value: [u8]): Result<(), DeError>
fn serialize_list<T: Serialize, S: Serializer>(mut s: S, value: List<T>): Result<(), DeError>
fn serialize_map<V: Serialize, A: Allocator, S: Serializer>(mut s: S, value: Map<String, V, A>): Result<(), DeError>
fn serialize_option<T: Serialize, S: Serializer>(mut s: S, value: Option<T>): Result<(), DeError>
fn serialize_result<T: Serialize, E: Serialize, S: Serializer>(mut s: S, value: Result<T, E>): Result<(), DeError>
fn serialize_tuple2/3/4<...>(mut s: S, value: (..)): Result<(), DeError>

fn deserialize_unit<D: Deserializer>(mut d: D): Result<(), DeError>
fn deserialize_bool<D: Deserializer>(mut d: D): Result<bool, DeError>
fn deserialize_i8/i16/i32/i64/isize<D: Deserializer>(mut d: D): Result<T, DeError>
fn deserialize_u8/u16/u32/u64/usize<D: Deserializer>(mut d: D): Result<T, DeError>
fn deserialize_f32/f64<D: Deserializer>(mut d: D): Result<T, DeError>
fn deserialize_char<D: Deserializer>(mut d: D): Result<char, DeError>
fn deserialize_str<D: Deserializer>(mut d: D): Result<String, DeError>
fn deserialize_bytes<D: Deserializer>(mut d: D): Result<List<u8>, DeError>
fn deserialize_list<T: Deserialize, D: Deserializer>(mut d: D): Result<List<T>, DeError>
fn deserialize_map<V: Deserialize, D: Deserializer>(mut d: D): Result<Map<String, V>, DeError>
fn deserialize_option<T: Deserialize, D: Deserializer>(mut d: D): Result<Option<T>, DeError>
fn deserialize_result<T: Deserialize, E: Deserialize, D: Deserializer>(mut d: D): Result<Result<T, E>, DeError>
fn deserialize_tuple2/3/4<...>(mut d: D): Result<(..), DeError>
```

Every writer takes the serializer first and the value second, the order `serialize_i32(mut s, self.port)` and
`serialize_list(mut s, self.items)` share. Every reader takes only the deserializer: a reader's element type comes
from the expected type of the call or from a value the caller names. The checker infers a function's type arguments
from an expected type only when the two spellings match exactly, so a reader whose result is used with `?` needs the
annotation on the call rather than on the `?` result:

```
let read: Result<List<Leaf>, DeError> = deserialize_list(mut d)
let items = match read { Ok(value) => value, Err(e) => return Err(e.field("items")) }
```

A hand-written impl that wraps the error itself does not need `?` at that step; the generated impls write the
container shapes out, so they do not depend on this (SER16).

The error helpers are public too: `unknown_field(name)`, `duplicate_field(name)`, `missing_field(name)`, and
`unknown_variant(type_name, variant)`. `DeError` itself provides `new`, `syntax`, `field`, `index`, `key`, and
`variant`.

A tuple reader calls `has_next_seq_element()` before each part: that call consumes the separator between two parts,
and a sequence that ends early has to be reported as a missing element (`expected N elements in a tuple`), not as
whatever the format would parse next (SER7). The writer needs no such call, because it places the separators itself.

Arrays are not in the helper list: building a `[T; N]` one element at a time needs a way to initialize an array
without a default value, which the language does not provide. The derives do not handle an array field yet either;
the compiler-provided array impls are deferred (SER16).

## SER15. Errors in the format packages

A format package implements `Serializer` for its writer and `Deserializer` for its reader and nothing else. It does not
declare a second error type: every failure is a `DeError`, built with `new` for a value-level rejection (a float that
JSON cannot represent exactly, a TOML table that cannot hold a mixed array) and with `syntax` for malformed input.
The path is attached by the data type's impls (SER9), so a format never sees the path of the value it is writing
unless the impl gives it a path through the error it returns. A format that wants to reject a shape does so from the
method for that shape; the format package's own tests pin its messages, the way the test format's messages are pinned
here.

## SER16. Not yet

These are deferred and are not to be implemented early:

- The compiler-provided impls for primitives and built-in containers, and the derive's field requirement
  (SER12). Until they land, the derive writes the built-in container shapes out itself and the scalars call their
  helpers. A hand-written impl calls the helper functions of SER14.
- Arrays as deserialization targets (SER14), arrays in a derived type, and arrays of `List`-like views.
- Named payload fields for struct variants (SER1, SER8).
- Map keys other than `String` (SER7), including integer and enum keys.
- Borrowed outputs (`&str`, `&[u8]`), `from_bytes` without a copy, and zero-copy map values (SER11).
- `Heap<T>`, `any I`, and closures (SER2).
- Skipping an unknown field instead of reporting it, and `deny_unknown_fields` or its opposite.
- The remaining format packages (toml, csv, yaml, xml) and their tests; `json` is written (`packages/json`, issue
  #255) and is the reference for what a format package looks like.
- A reflection-based structural fallback serializer (`Type.fields` at run time), a binary encoding, and a schema.
- Inferring a generic function's type arguments through `?` and through a nested expected type: today
  `deserialize_list(mut d)?` cannot infer its element type, which is why SER14's reader pattern annotates the call's
  own `Result`. When the checker infers it, the pattern shrinks to one line.
- Correct generic-parameter scoping in the checker: a method's generic parameter currently shadows an impl parameter
  of the same name (SER12). Until that is fixed, a hand-written generic impl should not name the receiver parameter
  `D` (or `S` for `Serialize`) when it calls `D.deserialize` on a field of that type; the generated impls use
  collision-free names.

## Tests

`tests/s3_serialize/`:

| Test | Rules |
| --- | --- |
| `all_scalars` | SER1-SER5, SER14: unit, every integer width, both floats, char, string, and bytes round-trip |
| `scalars` | SER1-SER6, SER9, SER13, SER14: a struct of scalars round-trips, including the non-finite floats |
| `nested` | SER1, SER5, SER7, SER13: nested sequences and a map with string keys |
| `enum_hand` | SER1, SER5, SER8, SER13: a hand-written enum impl for a unit, a tuple, and a two-field variant |
| `error_path` | SER6, SER9, SER10: the path in a nested error and the missing/unknown field messages |
| `derive_struct` | SER2, SER12: a derived struct of mixed scalars round-trips, and its text is pinned |
| `derive_enum` | SER8, SER12: derived unit, tuple, and struct variants round-trip |
| `derive_generic` | SER12, SER14: a generic derived type, nested derived user types, and every container shape |
| `derive_errors` | SER6, SER8, SER12: the derived unknown/missing/duplicate field messages and an unknown variant |
| `tuple_hand` | SER7, SER14: `serialize_tuple2`/`deserialize_tuple2` in a hand-written impl, and the short-sequence error |
| `json/` | SER1-SER10 in JSON (`packages/json`): a round trip, the pretty form, and the error message of malformed input |

`tests/json/` holds the format package's own fixtures (`roundtrip`, `pretty`, and one malformed document per file in
`bad`, each with its `.expected` message); `tests/check_json.py` runs them against the package.
