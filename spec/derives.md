# Burdin derives (stage 0)

A struct, enum, or error enum lists the interfaces it implements by itself after its name and generic parameters,
the way an impl block names its interface (`grammar.md`, issue #825):

```
struct Point: Copy, Clone, Eq { x: i32, y: i32 }
struct Pair<T>: Clone, Debug { a: T, b: T }
struct Rgb: Copy, Clone repr(C) { r: u8, g: u8, b: u8 }
enum Color: Copy, Eq { Red, Green }
error enum ParseError: Eq { Empty "empty input" }
struct Config: Default {}
```

`: Iface` on a declaration means "implement this interface with its derived or default implementation". Each name
is one of:

- a built-in derive: the compiler knows exactly nine, `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Debug`,
  `Serialize`, and `Deserialize`, described below;
- a `derive` declaration in scope, which generates the impl block (`reflection.md`, RFL8-RFL9);
- an interface in scope (by name or by a module path) whose methods all have default bodies. The declaration
  implements it exactly as the empty impl block `Type: Iface {}` would, so the usual impl rules apply (`Send` and
  `Sync` still need an `unsafe` block, E0281). To override a default, write the impl block by hand instead.

An interface with a method that has no default (`Drop`, `From`, `TryFrom`, `Display`, or a user interface) cannot be listed:
that is E0234 at the name, because the method has to be written in an impl block. A name that is none of the three
is E0002. The names are separated by `,`, with no trailing comma, and `repr(C)` comes after the list. On a generic
type, each type parameter needs the interface as well (`Pair<T>: Describe` holds for `Pair<A>` when `A: Describe`),
the rule of every derive; a const parameter needs nothing.

The old `derives (...)` clause is gone, and `derives` is an ordinary identifier, so `struct P derives (Eq) { ... }`
is a syntax error (E0001).

Each built-in derive is independent: deriving `Copy` does not provide `clone()`, and deriving `Ord` does not provide
`==`; list every one a program uses. A derive whose requirement is not met is E0130, reported at the offending field
(or at the name in the list for `Copy` on a type that implements `Drop`).

`Drop` is not derivable (E0234). It is declared only as `Type: Drop { fn drop(mut self) { ... } }`
(`memory-rules.md`, DR8). A type that implements `Drop` cannot be `Copy` (E0130).

## `Copy`

A `Copy` value is copied by assignment, by `let`, by passing, and by returning; the source stays usable. The derive
requires every field to be `Copy` and the type not to implement `Drop`. Integers, floats, `bool`, `char`, `()`,
tuples and arrays of `Copy` types, `&T`, `str`, `[T]`, and `CPtr<T>` are `Copy` without a derive.

## `Clone`

`x.clone()` produces an independent deep copy and leaves `x` usable. The derive requires every field to be
`Clone`. `Copy` types clone by copying; `String` copies its buffer; `Heap<T>` allocates and clones the pointee; a
`List` clones its elements; structs, tuples, arrays, and enums clone field by field.

## `Eq`

`==` and `!=` compare values. The derive requires every field to support `==`. Structs and tuples compare fields in
declaration order and are equal when every field is equal. Enums are equal only in the same variant with equal
payload fields. `Option` and `Result` support `==` when their parts do. A fixed-size array `[T; N]` supports `==` when
`T` does, comparing element by element (milestone 24, issue #484), and a `Map` supports `==` when its keys and values
do, comparing its entries whatever the iteration order. A `List` supports `==` when its elements do: two lists are
equal when they have the same length and equal elements in the same order (issue #928). Slices, references, `Heap`,
and `Chars` do not support `==`. `PriorityHeap` derives `Eq` over its list, and `Deque` implements `==` by hand in the
standard library, since two deques with the same elements can split them differently between their ends (`stdlib.md`).

## `Ord`

`<`, `<=`, `>`, and `>=` compare values by the order below. The derive requires every field to support `<`.
Comparison is lexicographic:

- a struct or tuple compares its fields in declaration order, the first unequal field deciding;
- a fixed-size array `[T; N]` compares its elements in index order, the first unequal element deciding;
- an enum compares variant declaration index first (`0` is the first variant written), then, for the same variant,
  its payload fields in declaration order;
- integers compare numerically (signed or unsigned as written), `bool` as `false < true`, `char` by scalar value,
  `String` and `str` byte by byte.

`Ord` does not imply `Eq` for a derive: a type that needs both lists both. A generic bound `T: Ord` is different: the
body of `fn f<T: Ord>(a: T, b: T)` may use `a == b`, because the bound adds `Eq` (`bound_closure`). The derive and the bound
disagree today; #918 decides one rule for both. A float field has no derived ordering, so a struct with
an `f64` field cannot derive `Ord` (E0130), even though `<` compares two floats directly. `Option` orders `None`
before `Some` and then by payload; `Result` orders `Ok` before `Err` (its declared variant order) and then by the
payload of the same variant. References and `List` do not support `<`.

## `Hash`

`x.hash()` returns `u64` with the FNV-1a mixing function. Start with the offset basis

```
acc = 0xcbf29ce484222325
```

and fold the value's scalar parts in order. For each part `x` (an integer, `bool`, or `char`), let `w` be its value as
a 64-bit word: signed integers are sign-extended, unsigned integers, `bool` (`0`/`1`), and `char` (its scalar value)
are zero-extended. Then

```
acc = (acc XOR w) * 0x00000100000001b3        (mod 2^64)
```

The parts are: a struct's or tuple's fields in declaration order; an array's elements in index order; a list's length
and then its elements in order; a map's entry count and then each entry, where each entry folds its key and value from
the offset basis and the entries are combined with XOR (so insertion order does not matter); an enum's variant
declaration index (as an `i32` word) followed by the active variant's payload fields in declaration order; an
`Option`'s tag (`false` = `None`, `true` = `Some`) followed by the payload when `Some`; a `Result`'s tag (`true` =
`Ok`) followed by the payload of the active variant. `str` and `String` fold their bytes in order. The derive requires
every field to be `Hash`. Equal values hash equally; the values themselves are pinned by
`m07_generics/derive_values`. `x.hash_from(state)` runs the same fold from `state` instead of the offset basis, and
`std.hash` builds `Hasher` on it (`stdlib.md`, "Hashing").

Worked examples (these are the expected values in that test):

| Value | `hash()` |
| --- | --- |
| `Pair { a: 0, b: 0 }` | `590684067820433389` |
| `Pair { a: 1, b: 2 }` | `589729691727335466` |
| `Pair { a: -1, b: 2 }` | `17855101231749507460` |
| `E.A` (first variant) | `12638153115695167455` |
| `E.B(2)` (second variant) | `589729691727335466` |

`Pair` has two `i32` fields; `E` is `enum E { A, B(i32) }`.

## `Default`

`Type.default()` is the associated function that returns the type's default value. The derive is for structs only;
on an enum it is not implemented in stage 0 (`E9000`, stage 1). The derive requires every field to have a default:

- integers and floats: `0` / `0.0`; `bool`: `false`; `char`: `'\0'`; `()`: `()`;
- a nested struct that derives `Default`: its `default()`;
- a tuple whose parts all have a default: elementwise;
- `String`: an empty string; `Option`: `None`;
- `List` and `Map`: the empty container, when they use the default allocator `Global` (a stored allocator has no value
  to build a default from); `Deque` and `PriorityHeap` derive `Default` from their empty list;
- arrays, `Heap`, and references do not have a default (E0130).

A default value is built as an ordinary value at the use site; there is no hidden storage.

## `Debug`

`: Debug` implements `Debug`, the interface `{:?}` needs (`format.md`, FM4). The derive requires every
field to implement `Debug` (E0130 at the field) and needs the same bound on a generic type's parameters. The
compiler writes the impl block as ordinary Burdin source (`crates/burdin_syntax/src/derive.rs`) and parses it
with the same parser as a hand-written block, so a derived `Debug` and a written one behave identically:

- a struct writes `Name { field: value, ... }`, every field in declaration order, or `Name` for a struct with no
  fields;
- an enum writes the active variant's name: `Variant` with no payload, `Variant(a, b)`, or
  `Variant { field: value }`;
- each value is written with its own `Debug`, so a nested derived type nests its braces the same way;
- every token of the generated block carries the span of the `Debug` in the derive clause, and the checker does not
  report diagnostics of its own inside it: a field that is not `Debug` is already E0130 at the field.

`Display` is not derived; write `Type: Display { fn fmt(self, mut f: Formatter) { ... } }`.

## `Serialize` and `Deserialize`

`: Serialize, Deserialize` generates the two serialization impl blocks in the standard representation: a struct
writes its fields by name, and an enum writes an externally tagged variant with a positional payload. The generated
impls call the per-shape helpers, write the built-in container shapes out themselves, and name their format parameter
`__S`/`__D`. The exact generated code, the container handling, and the field requirement still to come are in
[`serialization.md`](serialization.md), SER12. The derive is separate from the representation: a type can implement
either interface by hand instead (SER3).

## Tests

| Derive | Tests |
| --- | --- |
| `Copy`, `Clone` | `m05_ownership/copy_and_clone`, `m07_generics/derives` |
| `Eq`, `Ord` | `m07_generics/derives`, `derive_values` |
| `Eq` on `List` | `m24_stdlib/list_equality`, `list_equality_needs_eq` |
| `Hash` | `m07_generics/derives`, `derive_values` |
| `Default` | `m07_generics/derives`, `derive_values` |
| Requirement failures | `m07_generics/derive_field_not_eq` (E0130), `m05_ownership/copy_with_drop` (E0130) |
| Interface coverage | `m24_stdlib/interfaces_text`, `interfaces_list`, `interfaces_map`, `interfaces_option_result`, `interfaces_tuples_arrays`, `interfaces_ranges_ordering`, `interfaces_std_containers`, `interfaces_time_path`, `interfaces_iterable`, `interfaces_missing` |
| `Debug` | `m14_format/debug_derive`, `debug_manual` |
| `Serialize`, `Deserialize` | `s3_serialize/derive_struct`, `derive_enum`, `derive_generic`, `derive_errors` |
| `Drop` not derivable | `m05_ownership/drop_derive` (E0234) |
| Interfaces with default methods | `m07_generics/derive_default_interface`, `derive_default_bound` (E0141) |
| Not derivable, unknown name | `m07_generics/derive_not_derivable` (E0234, E0002) |
| The old `derives (...)` clause | `m07_generics/derives_old_syntax` (E0001) |
