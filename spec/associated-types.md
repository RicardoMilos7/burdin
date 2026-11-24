# Associated types

An interface may name a type that each implementation chooses. The interface writes `type Name` and a projection
(`Self.Name`, `T.Name`, or `<T as Interface>.Name`) reads the implementation's choice back. Issue #393 settles the
feature; milestone 21 (`tests/m21_reflect/`) carries its tests. Rule IDs are AT1-AT7.

The stage 1 iteration decision (`spec/stage1-subset.md`, item 2) keeps `Iterator<T>` generic and `Iterable<T>` a
marker. Associated types are the general feature the design's `Iterator` sketch implies; they are implemented here
without changing `Iterator<T>`, so a later milestone may still decide whether the pointer-like iterators use them.

```
interface Container {
    type Item
    fn get(self): Self.Item
    fn again(self): Self.Item { self.get() }   // a default method may use Self.Item
}

struct Counter { n: i32 }

Counter: Container {
    type Item = i32
    fn get(self): Self.Item { self.n }          // an impl method may too
}

struct Holder<T> { value: T }

Holder<T>: Container where T: Copy {
    type Item = T                              // a generic impl binds it to its own parameter
}

fn describe<T: Container>(c: T): T.Item { c.get() }
fn make(): <Counter as Container>.Item { 3 }
```

## AT1. Declaration

An interface body may contain `type Name`. It is a member of the interface, in the same namespace as its methods: a
name may not be both a method and an associated type, nor two associated types, and the second occurrence is E0007.
Stage 1 gives an associated type no bound and no default. `type Name: Bound`, `type Name = Ty`, and a bound or
default written any other way are a parse error (E0001) with a message saying to write `type Name`. The names are
stored on `InterfaceInfo::assoc` in declaration order.

## AT2. Binding

A `Type: Interface` block may contain `type Name = Ty` entries. Every associated type the interface declares is bound
exactly once. The errors are:

| Situation | Code | Where |
| --- | --- | --- |
| A declared associated type has no binding | E0144 | the impl block line |
| A binding names an associated type the interface does not declare | E0144 | that binding |
| The same associated type is bound twice | E0007 | the second binding (the first is kept) |
| A `type` entry in a block with no interface (inherent, `Drop`, `From`, `TryFrom`, a built-in interface) | E0144 | each entry |

Each binding is resolved in the impl's scope: the impl's type parameters stand for themselves (`Ty::Param`) and
`Self` stands for the impl's target type. A non-generic impl resolves to its one type; a generic impl resolves to
the instance whose arguments are the impl's pattern (`Holder<Ty::Param("T")>` for `Holder<T>`). The bindings live on
`ImplInfo::assoc`, parallel to the impl's `args`, and are substituted with the instance's arguments when the impl is
instantiated. A missing binding does not produce a second error at a use: a projection whose implementation exists
but does not bind the type resolves to `Ty::Error` silently, because E0144 was already reported on the block.

## AT3. Projection

The syntax has three forms:

```
Self.Name                    // inside an interface method or an impl block
<T as Interface>.Name        // any type T and a written interface path, with optional type arguments
T.Name                       // shorthand when T is a type parameter or a type path segment
```

`Self.Name` and `<T as Interface>.Name` parse as `TypeKind::Projection`. `T.Name` parses as the two-segment
`TypeKind::Path([T, Name])`; the checker reinterprets it as a projection when the prefix is not a module path and the
first segment is a type parameter or a type name. A module path takes precedence, so `geometry.shapes.Square` is
still a module-qualified type.

Resolution starts with the self type. `Ty::Error` gives `Ty::Error` and no further diagnostic.

- **Qualified** (`<T as I>.Name`): `I` is the written interface. If it does not declare `Name`, the projection is
  E0145. On a type parameter, the parameter's bounds (their `requires` closure included) must provide `I`; otherwise
  it is E0141.
- **Unqualified on a type parameter** (`Ty::Param(p)`): the candidate interfaces are the ones among the bounds of `p`
  in the current bound environment, with their `requires` closure, that declare `Name`. None is E0145; more than one
  is E0145 (ambiguous), and the message names the interfaces and says to write `<p as Interface>.Name`.
- **Unqualified on any other type**: the candidate interfaces are the ones the program implements for that type
  (the impl blocks whose `head` and argument pattern match, exactly as `impl_blocks_cover`/`match_impl_args` decide,
  with their `where` clauses met) and that declare `Name`. None is E0145; more than one is E0145 (ambiguous).

On a type parameter the result is `Ty::Projection { self_ty, iface, iface_name, args, name }`, where `args` are the
bound's interface arguments. On any other type the projection is resolved at once: the impl of the target interface
is found, its type arguments are substituted into its binding, and that type is returned. A qualified projection on a
type with no impl is E0141. An unqualified projection on a concrete type with no candidate is E0145.

`Ty::Projection` is opaque in symbolic code. Its display is `<self_ty as iface<args>>.name` (the `<...>` is omitted
when the interface has no arguments). `ty_has_param` is true for it and it is not second-class. It must never reach
code generation: normalization resolves it as soon as the self type is concrete.

`Globals::subst_ty` substitutes inside `self_ty` and `args`; if the new self type is not a bare `Ty::Param` (and not
`Ty::Error`), it normalizes the projection through the implementation's binding. This is what makes an interface's
own signature, written with `Self`, compare with an implementation's after `Self` is substituted
(`validate.rs::check_impl_signatures`). With no impl known, the projection is kept.

## AT4. Monomorphization

The binding is part of the impl and is substituted with the instance's arguments. Every written type is resolved
again for an instance, so a projection normalizes to the binding before code generation. No projection reaches the
typed tree of a concrete function, and the compiler crates after `burdin_sema` treat a projection as having no
runtime value (it is unreachable there).

## AT5. Object safety

An interface that declares any associated type cannot be used as `any I`: a vtable is a fixed set of method slots and
cannot answer which type `Self.Item` is for a value whose concrete type it does not name. `any I` on such an interface
is E0202, and the message names the interface and its first associated type. The check is part of
`any_not_object_safe_in` in `crates/burdin_sema/src/any.rs` (see `spec/existentials.md`, AN2).

## AT6. Default methods

An interface default method may use `Self.Name` in its signature and its body. The method is copied into an
implementation as usual (`register_methods` → `add_method` with `self_ty` the implementing type), so the projection
resolves to that implementation's binding there. The interface's own symbolic check sees the opaque
`Ty::Projection`, checked against the bounds it declares.

## AT7. Where clauses on projections

A `where` predicate may bound a projection of one of the declaration's own type parameters, written with a projection
form from AT3: `where I.Item: Eq`, `where <I as Container>.Item: Describe`. Issue #394 settles the rule; a predicate
on a projection is a bound on the projection, not on the parameter, so `I` itself does not gain the bound.

- **In the body**, the projection meets the bounds its predicate states. `==`, `<`, a method of the bound's interface
  (`c.get().describe()`), and `{}` formatting all accept a projection value that way. A value of the projection type
  without such a predicate is not usable as that bound's value (E0141 for `==`, a method call, or an operator).
- **At a call** (a generic function, a generic method, or an impl block that is instantiated), the projection under the
  instance's arguments must meet the bounds. The argument that does not is reported at the call: E0141, with the
  projection named (`` `Plain` does not implement `Eq`, which `<C as Container>.Item` requires ``).
- **On an impl block**, a predicate on a projection of an impl parameter decides whether the impl applies, as its other
  `where` clauses do (IM2). A method call on an instance whose projection does not meet the predicate finds no method
  (E0002).
- **Order**: the plain predicates of a declaration (`where C: Container`) are in scope when a projection predicate is
  resolved, so the two may come in either order. The projection must resolve: an unknown associated type, or one no
  bound declares, is E0145, as anywhere else.
- **Not covered** (still E9000 in `spec/e9000-audit.md`, owner Stage 1, issue #394): a predicate whose subject is not a
  parameter or a projection of one (`where i32: Eq`, `where Vec<T>: Eq`), a `where` clause on a non-generic impl
  block, and a predicate on `Self` inside an interface's default method.

Tests: `tests/m21_reflect/associated_where.bd` (a function, a method, a generic impl, the `==`, `<`, method, and format
uses) and `associated_where_errors.bd` (E0141 at a call, in a body, and in a method, E0145 for an unknown projection,
and E0002 for an impl whose predicate fails); the unit test `satisfies_reads_the_projected_bounds_of_a_projection` in
`crates/burdin_sema/src/generics.rs`.

## Errors

| Code | Meaning |
| --- | --- |
| E0001 | A bound or a default written on an associated type declaration |
| E0007 | A member name declared twice in one interface; an associated type bound twice in one impl |
| E0141 | A qualified projection on a type that does not implement the named interface |
| E0144 | A missing binding, a binding of a name the interface does not declare, or a binding in a block with no interface |
| E0145 | A projection no interface can answer, or one more than one interface answers |
| E0202 | `any` on an interface that declares an associated type |

## Tests

- `tests/m21_reflect/associated_types.bd` — a concrete binding, `Self.Item` in an interface default method and in an
  impl method, a generic impl, a generic function returning `T.Item`, a qualified `<Counter as Container>.Item`, a
  projection in a generic struct's field and in a generic alias, a generic interface's associated type, a
  `requires`-chain projection, the qualified form disambiguating two interfaces, and `type Item = Self`.
- `tests/m21_reflect/associated_types_errors.bd` — E0007 (duplicate declaration and duplicate binding) and E0144
  (missing, unknown, and inherent binding).
- `tests/m21_reflect/associated_type_projection_errors.bd` — E0145 (unknown and ambiguous), E0141 (a qualified
  projection with no impl), and E0202 (`any` on an interface with an associated type).
- Rust unit tests: the `Ty::Projection` display cases in `crates/burdin_tir/src/ty.rs`, and the symbolic test in
  `crates/burdin_sema/src/generics.rs`.
