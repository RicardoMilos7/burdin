# Reflection and user derives

Milestone 21 adds reflection over types and derives written in Burdin. This page is the exact behavior; the tests are
`tests/m21_reflect/`. Rule IDs are RFL1-RFL10. The decisions it records (issue #167) are: the reflection API is a set of
compile-time members of every type, the access to a field by a compile-time name is `x.{name}`, and a derive is
declared with the `derive` keyword.

## RFL1. What reflection is

Reflection is not a library and not a macro system. It is five members that every type has, evaluated by the
compiler while it checks a function body, and two loop and access forms that make them useful:

- the members `Type.name`, `Type.size`, `Type.align`, `Type.fields`, `Type.variants` (RFL2),
- `for item in Type.fields { ... }` and the same over `Type.variants`, which is unrolled (RFL4),
- `x.{name}`, a field access whose name is known while compiling (RFL5).

Reflection runs when a function body is checked for a concrete type, so a derive body sees the type it is applied to
(RFL8). A generic body is checked symbolically too, where a type parameter has no members yet (RFL7).

## RFL2. The members

| Member | Type | Meaning |
| --- | --- | --- |
| `Type.name` | `str` | the type as written, without the module it lives in; a generic instance includes its arguments: `Point`, `Pair<Leaf>`, `[i32; 4]` |
| `Type.size` | `usize` | the size in bytes (the existing `size`) |
| `Type.align` | `usize` | the alignment in bytes (the existing `align`) |
| `Type.fields` | `[FieldInfo; N]` | the fields of a struct, in declaration order |
| `Type.variants` | `[VariantInfo; M]` | the variants of an enum, in declaration order |

`Type` is any type path: a name, a generic instance, `Self`, or a type parameter. `FieldInfo` and `VariantInfo` are
structs of the standard library (`spec/stdlib.md`):

```
struct FieldInfo: Clone, Eq, Debug {
    name: String        // the field's name
    type_name: String   // its type, as `Type.name` spells it
    index: usize        // its position, from 0
}

struct VariantInfo: Clone, Eq, Debug {
    name: String        // the variant's name
    index: usize        // its position, from 0
    arity: usize        // how many fields it carries
}
```

The names are `String`, not `str`, because a struct field cannot be `str` (E0120).

A struct has no variants and an enum has no fields: `Enum.fields` and `Struct.variants` are empty arrays, so a derive
written for both kinds can loop over both. Any other type (a number, `bool`, `char`, `str`, a tuple, an array, and
the other built-in types) has neither, and asking is E0230.

The members are ordinary values: `let fields = Point.fields` binds an array that can be indexed, measured, and
looped over at run time too.

## RFL3. Reserved names

`name`, `size`, `align`, `fields`, and `variants` are reserved on every type. An associated function without `self`
named one of them, or an enum variant named one of them, would be ambiguous with the member and is E0231. A method
with `self` (`fn name(self): String`) is called on a value and does not conflict.

## RFL4. `for` over fields and variants

`for item in Type.fields { body }` and `for item in Type.variants { body }` are unrolled while
the body is checked. The rule is syntactic: the loop is unrolled when its iterable is literally `Type.fields` or
`Type.variants`, that is, a type path, `Self`, or a type parameter followed by `.fields` or `.variants`. Any other `for`
is the ordinary run-time loop, so `Type.fields` bound first (`let fs = Point.fields; for f in fs { ... }`) loops over
the array at run time. Unrolling works like this: the body is checked once per item, in order, with `item` standing for that item's value. The
loop variable is a plain name or `_`. In each copy `item.name`, `item.type_name`, `item.index` (a field) and
`item.name`, `item.index`, `item.arity` (a variant) are literals, so they fold into the code, and `item` alone is
the item's `FieldInfo` or `VariantInfo` value. An empty array unrolls to nothing. `break` and `continue` cannot leave
an unrolled loop (E0220, because the unrolled body is no loop); a loop written inside the body may use them for itself.
The body is checked like a loop body, so moving an outer value in it is E0104, as in any loop.

The unrolled loop is the only `for` evaluated at compile time (`spec/const-eval.md`, CE5).

## RFL5. Computed field access

`x.{name}` reads the field whose name is the compile-time string in the braces. The name is a string literal or the
`name` of the loop variable of a `for` over `Type.fields`; the access then means exactly `x.that_name`, with
the same rules (a missing field is E0002, a private field E0150, the borrow and move rules of a field). It can be
assigned (`x.{field.name} = v`) and borrowed like any field. A name that is not a compile-time string is E0232.

## RFL6. Where the members are legal

The members are expressions of the checked body, so they work anywhere an expression does, including in `const`
initializers. `Type.size` and `Type.align` keep their existing rules. Reading `Type.fields`
costs nothing at run time beyond building the array it is bound to.

## RFL7. Reflection in generic code

A generic function or impl is checked once symbolically, against its bounds, and once per instance. In the symbolic
check a type parameter has no members yet: `T.fields` is an empty array and `T.name` is the parameter's name. In an
instance the members are the concrete type's, so `Self.fields` of `Pair<Leaf>` lists `first` and `second`.

## RFL8. Derive declarations

```
derive Describe {
    fn describe(self): String { ... }
}
```

A `derive Name { fn ... }` item declares the methods a derive generates. It is syntax for the methods of an impl
block: applying the derive to `Type` (RFL9) registers the impl block `Type: Name { <the methods> }` and checks the
methods as for a hand-written one, so every error in them is an ordinary error. Rules:

- `Name` must name an interface in scope where the derive is applied, and the methods must match it (the usual impl
  rules). A derive with no interface is E0233 at the use.
- The name of a derive cannot be the name of a built-in derive (`Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`,
  `Debug`); declaring one is E0233. The built-in derives keep working as before.
- Derives and interfaces are separate namespaces, so a derive and its interface have the same name. `use m.Name`
  imports the interface and the derive that have that name; `priv derive` is package-private like any item.
- The names in the body resolve where the derive is applied, not where it is declared, so a derive that calls a
  helper needs the helper imported in the module of the type (derives are not hygienic).
- Methods may be `priv`.

## RFL9. Applying a derive

`struct T: Name` (or `enum T: Name`) applies a user derive, alongside the built-in ones (`derives.md`). For a
generic type the generated impl is `Type<A, B>: Name where A: Name, B: Name` (one bound per type parameter; a const parameter takes
none), the rule of the built-in derives: `Pair<Leaf>` works when `Leaf` has the derive too. A derive whose generated
impl does not check is reported at the derive's own lines, once, with the type's name in the message.

## RFL10. Errors

| Code | Meaning |
| --- | --- |
| E0230 | `Type.fields` or `Type.variants` on a type that is neither a struct nor an enum |
| E0231 | An associated function or enum variant named `name`, `size`, `align`, `fields`, or `variants` |
| E0232 | A computed field access whose name is not a compile-time string |
| E0233 | A derive named like a built-in derive, or `struct T: Name` with a user derive and no interface `Name` in scope |

An unknown name after `:` is E0002 (as before), a missing field in `x.{"z"}` is E0002, and E0220 and E0003 apply as
everywhere.

## Tests

`tests/m21_reflect/` covers RFL2-RFL5 and RFL8-RFL9: `reflect_members` (the members on a struct and an enum),
`reflect_invalid` (E0230), `reserved_members` (E0231), `computed_field_invalid` (E0232), `derive_describe` (a derive
that formats any struct), `derive_generic` (a derive on a generic struct and an enum), `derive_modules` (a derive
imported with its interface), and `derive_invalid` (E0233).
