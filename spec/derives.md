# Burdin derives (stage 0)

A struct or enum may list `derives (...)` after its name (`grammar.md`). The compiler knows exactly seven derives:
`Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, and `Debug`. Each is independent: deriving `Copy` does not provide
`clone()`, and deriving `Ord` does not provide `==`; list every one a program uses. A derive whose requirement is
not met is E0130, reported at the offending field (or at the derive clause for `Drop` and `Copy`).

`Drop` is not derivable (E0002). It is declared only as `Type: Drop { fn drop(mut self) { ... } }`
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
do, comparing its entries whatever the iteration order. Slices, references, `List`, `Heap`, and `Chars` do not
support `==`.

## `Ord`

`<`, `<=`, `>`, and `>=` compare values by the order below. The derive requires every field to support `<`.
Comparison is lexicographic:

- a struct or tuple compares its fields in declaration order, the first unequal field deciding;
- a fixed-size array `[T; N]` compares its elements in index order, the first unequal element deciding;
- an enum compares variant declaration index first (`0` is the first variant written), then, for the same variant,
  its payload fields in declaration order;
- integers compare numerically (signed or unsigned as written), `bool` as `false < true`, `char` by scalar value,
  `String` and `str` byte by byte.

`Ord` does not imply `Eq`; a type that needs both lists both. A float field has no derived ordering, so a struct with
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

`derives (Debug)` implements `Debug`, the interface `{:?}` needs (`format.md`, FM4). The derive requires every
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

## Tests

| Derive | Tests |
| --- | --- |
| `Copy`, `Clone` | `m05_ownership/copy_and_clone`, `m07_generics/derives` |
| `Eq`, `Ord` | `m07_generics/derives`, `derive_values` |
| `Hash` | `m07_generics/derives`, `derive_values` |
| `Default` | `m07_generics/derives`, `derive_values` |
| Requirement failures | `m07_generics/derive_field_not_eq` (E0130), `m05_ownership/copy_with_drop` (E0130) |
| Interface coverage | `m24_stdlib/interfaces_text`, `interfaces_list`, `interfaces_map`, `interfaces_option_result`, `interfaces_tuples_arrays`, `interfaces_ranges_ordering`, `interfaces_std_containers`, `interfaces_time_path`, `interfaces_iterable`, `interfaces_missing` |
| `Debug` | `m14_format/debug_derive`, `debug_manual` |
| `Drop` not derivable | `m05_ownership/drop_derive` (E0002) |
