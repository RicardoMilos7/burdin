# Type aliases

A type alias gives a type another name. Milestone 21 lets an alias take parameters. This page is the exact behavior of
generic aliases; the tests are `tests/m21_reflect/alias_generic.bd` and `alias_invalid.bd`, and the Rust unit tests
in `crates/burdin_sema/src/impl_tests.rs`. Rule IDs are AL1-AL6.

```
type Pair<T> = (T, T)
type Grid<T, const N: usize> = [T; N]
type Table<V> = Map<String, V>
type Sorted<T: Ord> = List<T>
```

## AL1. Declaration

`type Name<P, ...> = Type` declares an alias whose parameters are type parameters, optionally with bounds
(`T: Ord`), or `const` parameters (`const N: usize`, `spec/const-generics.md` CG1). The parameters are in scope in the
aliased type and nowhere else. A parameter name repeated is E0007, like any generic list. An alias without
parameters is the alias of stage 0, unchanged.

## AL2. Use

`Pair<i32>` is the aliased type with the written arguments in place of the parameters: `(i32, i32)`. It is written
wherever a type is written: a parameter, a return type, a field, a local annotation, a generic argument, the argument
of another alias, an `impl Interface` or `any Interface` argument, and a type in a path expression
(`Table<i32>.new()`). The arguments are resolved where they are written, so they can mention the enclosing
declaration's own type parameters.

## AL3. Arity and bounds

A use with the wrong number of arguments, none included (`Pair` alone), is E0003. The arguments of a type
parameter with bounds meet them: `Sorted<Unordered>` is E0141 when `Unordered` has no `Ord`, exactly as for a generic
struct. An argument that still mentions a type parameter is left to the check of the enclosing generic body, as
everywhere else.

## AL4. Nesting and cycles

An alias may name other aliases, generic or not, and its arguments may be aliases (`Twice<Pair<i32>>`). The expansion
is a substitution, so it cannot loop unless the declarations do: an alias that expands into itself, directly or through
others, is E0009 at the declaration, like the aliases of stage 0.

## AL5. An alias is the type, not a new type

`Pair<i32>` and `(i32, i32)` are the same type: a value of one is a value of the other, methods and interface
implementations are those of the aliased type, and diagnostics print the aliased type. An alias adds no methods
and no implementations of its own: an `impl` block names the type itself, and one written through an alias, generic or
not, is E0003 (`Stacked<i32> { ... }` for `type Stacked<T> = Stack<T>`; write `Stack<i32> { ... }`).

## AL6. Errors

| Code | Meaning |
| --- | --- |
| E0003 | The wrong number of arguments, or an `impl` block written through an alias |
| E0007 | A parameter name repeated |
| E0009 | An alias that expands into itself |
| E0141 | An argument that does not meet a parameter's bound |
| E0250 | A type argument for a const parameter or the reverse (`spec/const-generics.md`) |

## Tests

`alias_generic` (tuples, arrays with a const parameter, a map, alias of alias, a generic struct field, `Table<i32>.new()`)
and `alias_invalid` (E0003, E0009, E0141, and the impl through an alias).
