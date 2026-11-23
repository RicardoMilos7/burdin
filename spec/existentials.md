# Existential types: `any` and `impl`

Stage 1 milestone 15 (issue #117). This is the decision that the milestone's code and tests implement: the
representation of `any Interface`, when a value is boxed, which interfaces may be used existentially (object
safety), the meaning of `impl Interface` in a signature, and the downcast. It extends the stage 0 decisions; it does
not replace any of them.

The design document says `impl Shape` in a signature is one concrete type known at compile time, and `any Shape` is
a type "the compiler places on the heap, so `List<any Shape>` works without a visible wrapper"
(`Burdin Language Design.md`, "Interfaces and generics", "Dispatch"). This page makes that precise.

The rules are numbered `AN1`-`AN12`, by the same convention as the other stage 1 rule sets. The tests are in
`tests/m15_any/`.

## AN1: `any Interface` is an owned heap box

`any I` is an ordinary, owned, first-class type. Every value is two words:

```
{ data: ptr, vtable: ptr }
```

`data` is the address of a heap allocation that holds one value of the concrete type `T` that was boxed, and `vtable`
points at the constant table for `(T, I)`. The box owns the allocation: dropping it runs `T`'s drop through the vtable
and then frees the allocation. `any I` is never `Copy`, never `Clone`, has no `==`, and cannot be derived.

`any I` is not a view (`spec/memory-rules.md`, SC2): it is stored in locals, fields, arrays, `List`, `Map` values,
returned, and moved like any other owned value. Because the data is behind a pointer, a type may contain `any I`
directly without becoming infinitely sized.

## AN2: Object safety: which interfaces `any` accepts

`any I` is allowed only for an **object-safe** interface (E0202). `I` is object-safe when all of these hold:

1. no method of `I` has its own type parameters (`fn map<U, F: ...>`), because a vtable slot cannot be
   monomorphized at run time;
2. no method mentions `Self` outside its receiver (in a parameter or in the return type), because the vtable does
   not know the concrete type's spelling;
3. no method takes `own self`: an `own` receiver is passed by value, and a boxed value is reached through a
   pointer (every other receiver is already passed by address);
4. every interface `I requires` is itself object-safe.

`Display` and `Debug` (`std/src/fmt.bd`) are object-safe; `Iterator<T>` is not, because of its generic adapters.

A method whose signature mentions the interface's own type parameters is fine: `interface Store<T> { fn get(self): T }`
is object-safe, and `any Store<i32>` fixes `T = i32`.

The vtable contains one slot for every method of `I`, including default methods, followed by the methods of every
interface `I requires` (transitively), deduplicated by name. `any I` therefore satisfies `I` and every interface `I`
requires, so the methods reached through `requires` can be called through the box and `any I` can be passed where a
bound `J` is expected when `I requires J`.

## AN3: Boxing is implicit where an expected type is known

A concrete value of type `T` used where `any I` is expected is boxed by the compiler:

```
let shape: any Shape = circle            // a local with an annotation
takes_any(circle)                        // a call argument
fn make(): any Shape { circle }          // a return
let list: List<any Shape> = ...
list.push(circle)                       // the element type is the expected type
S { shape: circle }                      // a struct or enum field
```

This is an implicit conversion that produces an **owned** value, so unlike the `String`-to-`str` borrow (IC1) it is
allowed in every position where an expected type is known, not only in argument-like positions. Boxing moves a
non-`Copy` value and copies a `Copy` value; the source is left moved.

The value must implement `I` (E0141) and must be first-class: boxing a view (`str`, `[T]`, `&T`, `&mut T`, a closure
with borrow captures, or a type containing one) is E0120, because the box would outlive the borrow. Boxing an
`any I` value where `any I` is expected is the identity, not a second box. Boxing an `any J` where `any I` is
expected is E0003; there is no re-boxing conversion.

The compiler implements `Display` and `Debug` for the primitive types and the built-in containers directly, without a
method (spec/format.md). Those implementations have no vtable entry, so a value whose implementation is built in this
way cannot be boxed as `any Display`/`any Debug`: E0204. A user type with `Type: Display { ... }` or
`: Debug` can be boxed; `any Display` itself implements `Display` (AN9), so a boxed user value formats through
the vtable.

## AN4: `impl Interface` is a hidden generic parameter

`impl I` is accepted in a parameter list and as a return type (`fn make(): impl Shape`, which the caller sees through
`Shape`; the compiler reports E0201 for a return type until #926 lands). It is E0201 anywhere else, including a local
annotation, a field, a container element, a bound, or an `extern` signature. In a parameter it is sugar for one hidden
type parameter with the interface as its bound:

```
fn draw(shape: impl Shape)          // fn draw<T: Shape>(shape: T)
fn scale(mut shape: impl Shape)     // fn scale<T: Shape>(mut shape: T)
fn take(own shape: impl Shape)      // fn take<T: Shape>(own shape: T)
```

The hidden parameter is inferred from the argument at the call site and monomorphized, so dispatch is static and
there is one copy of the body per concrete argument type. The parameter's convention is written as usual and the
call-site marker rules are unchanged. `impl I` does not require `I` to be object-safe, because no vtable is built.
A call whose argument type does not implement `I` is E0141. Passing an `any I` where `impl I` is expected uses
`T = any I` and is static in the parameter, dynamic in the calls the body makes through `T: I`.

In a return type, `impl I` is one concrete type known at compile time, as the design document says
(`fn make(): impl Shape`). The function's body determines that type, and the caller sees only the interface. Each
`return` and the tail expression of the body must have the same concrete type (E0003 otherwise), and that type must
implement `I` (E0141). A value of the return type:

- calls the methods of `I` and of the interfaces `I` requires, which run the concrete type's own methods, statically
  (a method of the concrete type that `I` does not name is E0002);
- is passed where `I` or one of its requirements is required, including a type parameter bound by `I`;
- is boxed as `any J` only when `J` is `I` or one of its requirements, and the box holds the concrete value;
- cannot be named, and has none of the concrete type's own operators or built-in methods (`==`, `clone`); `{}` works
  when `I` requires `Display`, through that requirement's method.

A local cannot be annotated `impl I` (E0201). A function whose concrete type depends on itself, through a recursive
call whose result decides its own return, has no concrete type to find (E0205). Each function with an `impl` return
is given its concrete type before any body that calls it is checked, so a caller sees the same type the body
returns, whatever the order of the functions in the file.

## AN5: Calling a method through `any`

`x.m(args)` where `x` has type `any I` resolves `m` among the interface's vtable methods (after the transitive
`requires` methods) and lowers to an indirect call through the vtable slot:

- the slot's function is the concrete type's method implementation, which is the same function a static call would
  use (a default method is instantiated per concrete type like any other);
- the receiver is passed as the `data` pointer: every object-safe receiver is passed by address already;
- the other arguments follow the interface's parameter conventions (`mut` and `own`), and the call-site marker,
  exclusivity, and `unsafe` rules are the same as for a static method call;
- `self` borrows the box shared and `mut self` borrows it exclusively for the duration of the call.

An unknown method name is E0002, a wrong argument type or count is E0003/E0004, and the interface's
`mut`/`own`/`unsafe` rules apply as written. `x.m(args)` on an `any I` value can never reach a method that `I` does
not declare, even if the concrete type has it: the static type is the interface.

## AN6: `downcast` and `is`

Two compiler-known methods are available on every `any I`:

```
x.is<T>(): bool             // does the box hold exactly a T?
x.downcast<T>(): Option<T>  // the value, or None when the box holds another type
```

`T` must implement `I` (E0141) and be first-class (E0120). Type identity is the identity of the **monomorphized**
type: `Pair<i32>` and `Pair<i64>` are different types, and each generic instance has its own identity.

`is` borrows the box and reads its type identity. `downcast` consumes the box (`own self`): on a match it moves the
value out of the allocation and frees the allocation without running the drop, so the value is dropped exactly once
by its new owner; on a mismatch it drops the boxed value through the vtable and frees the allocation, and returns
`None`. There is no borrow-returning downcast: a value moved out of the box is owned, so `List<any I>` can be
drained.

Method-level type arguments are written after the member name (`x.downcast<Circle>()`), following the type-argument
spelling used for `Type<i32>.new()` (grammar rule 8.2).

## AN7: Moving and dropping

Boxing a value is a move context (`spec/memory-rules.md`, MV1): `let b: any I = x` moves `x`, and using `x`
afterwards is E0100. A `Copy` value is copied. The box's own moves are ordinary moves of an owned value; there is no
copy of the allocation. Drop runs the concrete destructor once, through the vtable, and frees the allocation, whether
the box is dropped by its scope, by an assignment overwrite, by a `?`/`return`/`break`, or as a temporary. A value
downcast out of a box is not dropped by the box.

## AN8: Interfaces that `any I` satisfies

`any I` satisfies the bound `I` and every interface `I requires` (AN2), so:

- `fn f<T: Shape>(x: T)` accepts an `any Shape`;
- `x.clone()` is E0130/E0002 and `x == y` is E0003: `any I` is not `Clone`, `Eq`, `Ord`, `Hash`, or `Default`;
- `Display` and `Debug` are reached through the vtable, so `println("{}", box)` works for `any Display` and
  `println("{:?}", box)` for `any Debug`.

## AN9: Naming and display

The type is written `any I` and `impl I`, where `I` is an interface path with optional type arguments
(`any Store<i32>`, `impl Shape`). Diagnostics print the type as `any I`.

`any` and `impl` remain keywords: they are no longer reserved words rejected in type position, but they are still
not identifiers. After a `.` they are ordinary member names, as for every keyword (`x.any(...)` is the iterator
adapter).

## AN10: What `any` cannot be

`any I` cannot be used in an `extern "C"` signature (E0162), because it is not a C ABI value. It cannot be the target
of an `impl` block or a `Drop` implementation, and it cannot be a bound (`T: any I` is E0200). It cannot be matched
with a pattern: there is no pattern that names a boxed type; use `is`/`downcast` instead. It cannot be a type
argument that makes a type second-class (it never does, since it is owned). `impl I` outside a parameter list and a
return type is E0201, including a `self` type and a bound. `impl I` in an interface method's parameter list is an ordinary generic
method, which makes that interface non-object-safe (AN2).

## AN11: The representation of a vtable

The compiler defines, for each `(T, I)` pair that a program boxes:

```
vtable = { type_id: i64, drop: ptr, method_0: ptr, ..., method_n: ptr }
```

`type_id` is a program-wide integer assigned to `T` (the same integer for every interface; distinct for distinct
monomorphized types), `drop` is `T`'s drop glue (`void(ptr)`), and each method slot is the concrete implementation's
function, whose first parameter is the `self` pointer. The compiler emits one private constant per pair and reuses
it. An interface with no methods (a marker) needs only the type id and drop slots, so any type that satisfies it
can be boxed. A box's `data` pointer is the address of the value; a `self` or `mut self` method receives it as its `self`
parameter. No run-time support beyond `malloc`/`free` is needed.

## AN12: Object safety is reported where the existential appears

E0200-E0204 are reported at the type that names the interface, in every position:

| Situation | Code |
| --- | --- |
| `any X` where `X` is not an interface (a type, or a built-in bound such as `Copy`) | E0200 |
| `impl I` outside a parameter list or a return type | E0201 |
| `any I` where `I` is not object-safe (AN2) | E0202 |
| boxing a view, or downcasting to one (AN3, AN6) | E0120 |
| `any Display`/`any Debug` of a built-in type the compiler formats directly (AN3) | E0204 |

A concrete type that does not implement the interface is E0141, whether boxed (AN3), named as a downcast target
(AN6), or passed to an `impl I` parameter (AN4).

## Tests

The executable form is `tests/m15_any/`:

| Rule | Tests |
| --- | --- |
| AN1, AN5, AN7 | `any_basics.bd`, `any_return_and_field.bd`, `heterogeneous_list.bd`, `containers.bd`, `drop_glue.bd` |
| AN2, AN12 | `object_safety.bd`, `requires_methods.bd`, `any_default_method.bd` |
| AN3 | `any_return_and_field.bd`, `heterogeneous_list.bd`, `box_view.bd` (E0120), `not_implementing.bd` (E0141), `builtin_display_box.bd` (E0204), `moved_into_box.bd` (E0100) |
| AN4 | `impl_parameter.bd`, `impl_method.bd`, `impl_not_parameter.bd` (E0201); the return-type run test is issue #926 |
| AN5 | `any_basics.bd`, `generic_interface.bd`, `requires_methods.bd`, `any_default_method.bd`, `any_exclusivity.bd` (E0110/E0113), `unknown_any_method.bd` (E0002) |
| AN6 | `downcast.bd`, `downcast_generic.bd`, `marker_interface.bd`, `not_implementing.bd` (E0141) |
| AN8, AN9 | `display_any.bd`, `debug_any.bd`, `requires_methods.bd`, `not_interface.bd` (E0200) |
| AN11 | `drop_glue.bd`, `downcast.bd` (the runtime paths) |
