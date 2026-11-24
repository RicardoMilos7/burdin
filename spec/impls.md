# Impl blocks for generic types

An impl block on a generic type can name specific arguments, and `Drop` can be implemented for a generic type.
Milestone 21 settles both, together with the rules for the built-in interfaces on generic types. This page is the
exact behavior; the tests are `tests/m21_reflect/impl_*.bd`, `impl_modules/`, and `derive_conditional.bd`, and the
Rust unit tests in `crates/burdin_sema/src/impl_match.rs` and `impl_tests.rs`. Rule IDs are IM1-IM9.

```
Stack<T> { fn push(mut self, value: T) }       // every instance
Stack<i32> { fn sum(self): i32 }               // that instance
Pair<i32, B> { fn double_first(self): i32 }    // every instance whose first argument is i32
Holder<List<T>>: Describe { ... }              // every instance whose argument is a list
Guard<T>: Drop { fn drop(mut self) { ... } }   // every instance
```

## IM1. The arguments of an impl are a pattern

The type in an impl header is written with the arguments of the type. Every name in them that is not a type is a
parameter of the impl, and it stands for whatever the argument is: in `Pair<i32, B>`, `B` is any type. A name may
appear nested (`Holder<List<T>>`), and a const argument may be a literal (`Grid<T, 3>`) or a name. A name stands for one
argument only, so a name written twice in one header is E0007 (`Pair<T, T>`); an impl for two equal arguments is not
expressible. A header that names every argument with its own name (`Stack<T>`, `Pair<A, B>`) covers every instance,
as it did before this milestone. The wrong number of arguments is E0003.

## IM2. Which instances an impl covers

An impl covers the instances whose arguments match its pattern: a concrete part of the pattern must be equal and a name
binds to what it matches. `Stack<i32>` covers `Stack<i32>` only, `Pair<i32, B>` covers `Pair<i32, bool>` and
`Pair<i32, i32>`, and `Holder<List<T>>` covers `Holder<List<i32>>`. Inside the impl, each name stands for the type it
matched, as the parameters of `Stack<T>` do, with the bounds the type declares for the argument it was written for.
`where` clauses (`Pair<i32, B> where B: Eq`) restrict the instances further, as for a generic impl: the impl covers
the instance only when the bounds hold.

## IM3. The arguments meet the type's bounds

The arguments of the pattern meet the bounds of the type's parameters, as at any use of the type: `Sorted<Plain>` is
E0141 when `Sorted<T: Ord>` and `Plain` has no `Ord`.

## IM4. Coherence

Two impls of one interface for one generic type overlap when some instance matches both patterns and both
interface arguments (`Stack<T>: Describe` and `Stack<i32>: Describe`; `Pair<i32, B>: Describe` and
`Pair<A, i32>: Describe`). The later one is E0143 and is left out. `where` clauses do not make two impls apart: the
compiler does not decide whether two bounds can hold together. Inherent methods have no such rule of their own: two
impls that define a method of one name for one instance are E0007 at the second method, the rule that two methods of a
type cannot share a name (`m07_generics/duplicate_member.bd`). There is no specialization: an impl on specific
arguments adds methods, it never replaces another impl's.

## IM5. Method resolution

A method call finds the methods of the instance's type: the methods of every impl that covers it. A method added by
`Stack<i32>` is not found on `Stack<bool>` (E0002), and it is not found in a generic body whose `T` is only known to be
`Copy`, since the body is checked once for every `T`. A bound `T: Describe` is met by an instance when an impl of
`Describe` covers it.

## IM6. The body is checked wherever it is used or not

The body of an impl for specific arguments is checked even when no value of the instance exists, as the bodies of
a generic impl are: the instance the pattern names is checked on its own. With names left in the pattern
(`Pair<i32, B>`), the names are opaque there, so a body can use only what the bounds of `B` promise.

## IM7. `Drop` for a generic type

`Type<T>: Drop { fn drop(mut self) { ... } }` implements `Drop` for every instance of the type, a struct or an enum,
and the drop glue of each instance calls it. The block is the usual `Drop` block of stage 0 (`spec/memory-rules.md`,
exactly one method `drop(mut self)`) and a second block for the same type is E0007. Because the glue of an instance
cannot differ from the type's others, the header names every argument with its own name and has no `where` clause:
`Guard<i32>: Drop` and `Guard<T>: Drop where T: Copy` are E0131. The body sees the type's parameters with the bounds
the type declares.

## IM8. Built-in interfaces on a generic type

A derive on a generic type holds for the instances whose parts allow it: `Wrapper<T>: Eq, Clone` makes
`Wrapper<i32>` `Eq` and `Clone` because `i32` is, and `Wrapper<NoEq>` neither, so `==`, `.clone()`, or a bound that
needs them is an error at the use (E0003, E0002, E0141). A requirement that mentions no parameter is checked once, at the
derive (E0130). The rule for `Debug`, `Hash`, `Ord`, and `Default` is the same (`spec/derives.md`).

The built-in derive-only interfaces are `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, and `Default` (`spec/derives.md`). `Eq` and `Ord` are also ordinary interfaces: a block `Wrapper<T>: Eq where T: Eq { fn eq(...) }` is a
hand-written comparison, and next to a derive of the same name it is E0007 (`spec/derives.md`, issue #918). Writing
`Copy`, `Clone`, `Hash`, or `Default` by hand (`Wrapper<T>: Clone where T: Clone { ... }`) is not implemented, for a generic type or a plain one: it is E9000 under
issue #402, whose rule applies to both. The other interfaces are written by hand as ordinary interfaces: `Debug`,
`Display`, `From`, `TryFrom`, `Drop` (IM7), and `Serialize`/`Deserialize`, which a program writes by hand as
`serialization.md` describes. When #402 lands, the `where` clauses
of such a block follow IM2: the block covers an instance when the bounds hold, which is the conditional rule (`T: Eq`).

## IM9. Errors

| Code | Meaning |
| --- | --- |
| E0003 | The wrong number of arguments, or an impl written through an alias (`spec/type-aliases.md` AL5) |
| E0007 | A name repeated in an impl header, or two methods of one name on one instance |
| E0131 | A `Drop` impl for specific arguments or with a `where` clause, or one that is not exactly one `drop` |
| E0141 | An argument that does not meet a bound of the type |
| E0143 | An impl that overlaps an earlier impl of the same interface |

## Tests

`impl_specific` (specific methods, an interface for specific instances, mixed and nested patterns), `impl_modules`
(the type in another module, named through a module path), `impl_specific_drop` (`Drop` for a generic struct and
enum), `impl_specific_invalid` (E0003, E0007, E0141, E0143, bodies of unused impls), `impl_drop_invalid` (E0007,
E0131), and `derive_conditional` (IM8).
