# Type aliases

A type alias gives a type another name. Milestone 21 lets an alias take parameters. This page is the exact behavior of
generic aliases; the tests are `tests/m21_reflect/alias_generic.bd` and `alias_invalid.bd`, and the Rust unit tests
in `crates/burdin_sema/src/impl_tests.rs`. Rule IDs are AL1-AL7.

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
and no implementations of its own, but an `impl` block may name the type through one, generic or not: the alias in
the block's header is expanded like any other use (AL2), and the block is the aliased type's. For
`type Stacked<T> = Stack<T>`, `Stacked<i32> { ... }` is `Stack<i32> { ... }`, `Stacked<T>: Describe { ... }` is
`Stack<T>: Describe { ... }`, and for `type Ints = Stack<i32>`, `Ints { ... }` is `Stack<i32> { ... }`. The names in the
written arguments that are not types are the impl's own, as in any impl header (`spec/impls.md`, IM1); each stands
for what it is written for, whatever the alias calls its parameter. The alias's arity (E0003), its bounds on a
concrete argument (E0141), and its cycles (E0009) are checked as at any other use, and the expanded type is held to
the rules of an impl header written directly: methods go only on a struct or enum (`Pair<i32> { ... }` is E0003, a
tuple), the orphan rule applies to the aliased type (E0142), and a method added through an alias and again directly is
E0007. A name in the header keeps the bounds the aliased type and the block's `where` clause give it, not the
alias's. The one place an alias is looked through by name is `main`'s return type (AL7).

## AL6. Errors

| Code | Meaning |
| --- | --- |
| E0003 | The wrong number of arguments, or an `impl` block through an alias of a type that cannot have methods |
| E0007 | A parameter name repeated |
| E0009 | An alias that expands into itself |
| E0141 | An argument that does not meet a parameter's bound |
| E0250 | A type argument for a const parameter or the reverse (`spec/const-generics.md`) |

## AL7. `main`'s return type

`fn main(): Name` is accepted when `Name` is an alias declared in the file that declares `main` and it expands, through
any other aliases of that file, generic or not, to `Result<...>` (`spec/errors.md`, ERR8). The arguments of the alias
are not looked at: `Outcome<()>` for `type Outcome<T> = Result<T>` is the same return type as `Result<()>`. The rule is
applied when the file is parsed, because the generated wrapper for an `Result.Err` is written then, before imports resolve. So
an alias of a `Result` that is imported from another module is E0003, the same error as any other alias that does not
expand to `Result`. An alias cycle reached from `main`'s return type is E0009 at the declaration (AL4).

## Tests

`alias_generic` (tuples, arrays with a const parameter, a map, alias of alias, a generic struct field, `Table<i32>.new()`,
and impl blocks through a generic alias, an alias of an instance, an alias of an alias, and an alias of a struct)
and `alias_invalid` (E0003, E0009, E0141, and an impl through an alias with the wrong arity, a cycle, or a tuple). The
Rust unit tests `an_impl_through_an_alias_*` in `crates/burdin_sema/src/impl_tests.rs` cover the same cases and the
alias's bounds. AL7 is `tests/m16_errors/main_result_alias.bd`
(run), `main_result_alias_generic.bd` (a generic alias and an alias of it, run-abort), `main_alias_not_result.bd` (E0003),
`main_result_alias_cycle.bd` (E0009), and `main_result_alias_imported/` (an imported alias, E0003).
