# Existential types: `any` and `impl`

Stage 1 milestone 15 (issue #117). This is the decision that the milestone's code and tests implement: the
representation of `any Interface`, when a value is boxed, which interfaces may be used existentially (object
safety), the meaning of `impl Interface` in a signature, and the downcast. It extends the stage 0 decisions; it does
not replace any of them.

The design document says `impl Shape` in a signature is one concrete type known at compile time, and `any Shape` is
a type "the compiler places on the heap, so `List<any Shape>` works without a visible wrapper"
(`Burdin Language Design.md`, "Interfaces and generics", "Dispatch"). This page makes that precise.

The rules are numbered `AN1`-`AN15`, by the same convention as the other stage 1 rule sets. The tests are in
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

This is an implicit conversion that produces an **owned** value, so like the `String`-to-`str` borrow (IC1) it is
allowed in every position where an expected type is known. Boxing moves a
non-`Copy` value and copies a `Copy` value; the source is left moved.

The value must implement `I` (E0141) and must be first-class: boxing a view (`str`, `[T]`, `&T`, `&mut T`, a closure
with borrow captures, or a type containing one) is E0120, because the box would outlive the borrow. Boxing an
`any I` value where `any I` is expected is the identity, not a second box. An `any J` where `any I` is
expected converts to it when `J` requires `I` (AN13), and is E0003 otherwise.

The compiler implements `Display` and `Debug` for the primitive types and the built-in containers directly, without a
method (spec/format.md). Those values are boxed too (AN14). A user type with `Type: Display { ... }` or
`: Debug` is boxed with its own method; `any Display` itself implements `Display` (AN9), so a boxed value formats
through the vtable.

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
with an ordinary pattern: no destructuring pattern names the boxed value's type. A `match` can test it with a type
pattern (AN15), and `is`/`downcast` remain (AN6). It cannot be a type
argument that makes a type second-class (it never does, since it is owned). `impl I` outside a parameter list and a
return type is E0201, including a `self` type and a bound. `impl I` in an interface method's parameter list is an ordinary generic
method, which makes that interface non-object-safe (AN2).

## AN11: The representation of a vtable

The compiler defines, for each `(T, I)` pair that a program boxes:

```
vtable = { type_id: i64, drop: ptr, name: ptr, name_len: i64, method_0: ptr, ..., method_n: ptr,
           super_0: ptr, ..., super_k: ptr }
```

`type_id` is a program-wide integer assigned to `T` (the same integer for every interface; distinct for distinct
monomorphized types), `drop` is `T`'s drop glue (`void(ptr)`), and each method slot is the concrete implementation's
function, whose first parameter is the `self` pointer. Each `super_i` points at the `(T, J_i)` vtable for one interface `J_i`
that `I` requires (AN13). The compiler emits one private constant per pair and reuses
it. An interface with no methods (a marker) needs only the type id and drop slots, so any type that satisfies it
can be boxed. A box's `data` pointer is the address of the value; a `self` or `mut self` method receives it as its `self`
parameter. No run-time support beyond `malloc`/`free` is needed.

## AN12: Object safety is reported where the existential appears

E0200-E0202 are reported at the type that names the interface, in every position:

| Situation | Code |
| --- | --- |
| `any X` where `X` is not an interface (a type, or a built-in bound such as `Copy`) | E0200 |
| `impl I` outside a parameter list or a return type | E0201 |
| `any I` where `I` is not object-safe (AN2) | E0202 |
| boxing a view, or downcasting to one (AN3, AN6) | E0120 |

A concrete type that does not implement the interface is E0141, whether boxed (AN3), named as a downcast target
(AN6), or passed to an `impl I` parameter (AN4).

## AN13: An `any I` converts to `any J` when `I requires J`

`any I` satisfies every interface `I` requires (AN2, AN8), so it is also accepted where `any J` is expected, for each
`J` that `I` requires, directly or transitively (`interface Drawable requires Scaled`, `Scaled requires Named`:
an `any Drawable` is accepted as `any Scaled` and as `any Named`). For a generic requirement the arguments are the
ones `I` writes (`I requires Store<i32>` gives `any Store<i32>`):

```
fn read(x: any Named): i32 { x.value() }
let d: any Drawable = Box { v: 5 }
read(d)                                 // an `any Drawable` where `any Named` is expected
let n: any Named = d                    // d is moved
```

The conversion applies at every position that boxes (AN3). It is a move of the box, like boxing: the source is left
moved (E0100 on a later use). It does not allocate or copy: the result owns the same allocation, holds the same
concrete value, and drops it once, through the vtable of `(T, J)`. `x.is<T>()` and `x.downcast<T>()` on the result
see the same type identity. Each vtable for `(T, I)` holds the `(T, J)` vtable of every interface `I` requires
(AN11), so the conversion loads a pointer and does not look at `T`.

Only the requires relation converts. An `any J` where `any I` is expected and `J` does not require `I` is E0003, as
is the opposite direction (`any Named` where `any Drawable` is expected). An `impl I` value converts as before (AN4).
Every interface involved is object-safe, because `any I` already requires it (AN2, rule 4).

## AN14: The compiler-provided `Display` and `Debug` are boxable

Every type that satisfies `Display` or `Debug` can be boxed as `any Display` or `any Debug`, including the types
whose implementation the compiler provides: the integers, floats, `bool`, `char`, `String`, and the built-in
containers and tuples whose elements are formattable (`Option<T>`, `Result<T, E>`, `List<T>`, `Map<K, V>`, `Heap<T>`,
tuples, the unit type) (spec/format.md). A user type's own `fmt`/`debug` (or a derived `Debug`) is still the
vtable entry for that type.

```
let one: any Display = 3
let mut items: List<any Display> = List<any Display>.new()
items.push(4.5)
items.push(String.from("hi"))
println("{one} {items[0]}")             // 3 4.5
let d: any Debug = Some(5)              // `{d:?}` prints Some(5)
```

The vtable entry of such a type is a compiler-generated thunk: the instance, for the boxed type `T`, of a standard
library function that takes the value by borrow and a `mut Formatter` and makes the same formatting call a
placeholder makes for `T`. It has the signature the vtable slot has (AN11), so `any Display` formats exactly as the
value would directly (the specifier of the placeholder applies as before), `is<T>`/`downcast<T>` see the same type
identity (AN6), and the box drops the value through `T`'s drop glue. A view (`str`, `[T]`, a reference) is not
boxable, whatever implements its `Display` (E0120, AN3): box a `String` instead.

The code E0204, which reported a compiler-provided implementation with no function to dispatch to, is retired: no
implementation lacks one now. Every other implementation reaches a vtable through an impl block's method, and an
interface cannot be implemented for a type the program does not declare.

## AN15: `match` on an `any I`: type patterns

An arm of a `match` whose scrutinee is an `any I` (or the dynamic `AnyError`, spec/errors.md ERR4) may be a **type
pattern**, written `name: T` or `_: T`:

```
match err {
    e: ParseError => println("parse: {e}"),
    e: IoError => println("io: {e}"),
    other => println("unknown: {other}"),     // the box itself, still an `any Error`
}
```

`T` is the concrete type the box must hold exactly (the same identity as `is<T>`, AN6). `name` is a binding that owns
the value moved out of the box, with type `T`; `_` binds nothing, and the value is dropped at the end of the arm.

1. **The scrutinee is consumed.** A `match` that has a type arm moves its scrutinee into a hidden local, like `let`
   (AN7), so a named `any I` is moved and using it afterwards is E0100. A borrowed `any I` (a parameter that is not
   `own`) cannot be matched this way: moving out of a borrow is already an error. There is no borrowing type pattern,
   for the reason there is no borrowing `downcast` (AN6).
2. **Only the arm that matches moves out of the box.** The first type arm, in order, whose `T` the box holds runs
   with `name` bound to the moved value, and the box's allocation is freed without running the drop (as `downcast`
   does). The arms are tried in order, so the first matching type wins.
3. **A catch-all arm is required.** An `any` is never exhaustive over concrete types, so the arms must end with a
   catch-all: `else`, `_`, or a binding `name`. The binding receives the whole box (an `any I`, or the `AnyError`),
   still owned, for an arm that did not match any type. Without one the `match` is E0006 (`match` does not cover every
   value), reported at the `match`. Arms after the catch-all are unreachable and not accepted: the catch-all is last
   (E0003).
4. **Each type must be possible.** `T` must implement `I` (E0141, as for `downcast`) and be first-class (E0120).
5. **A type arm takes no guard** (E0003): the value leaves the box when its type matches, so a failed guard would
   have nothing left to fall through with. Nest a `match` or an `if` in the arm instead.
6. **Mixing is restricted.** In a `match` with a type arm, every other arm must be the catch-all of rule 3. A type
   pattern is accepted only as the whole pattern of an arm (not under `|`, `@`, a tuple, or `if let`/`let`), and only
   on a scrutinee that is an `any I` or `AnyError`; elsewhere it is E0003.
7. **Result type.** The arms are unified as in any `match`; the arm bodies are ordinary expressions with `name` in
   scope. All the moves of the box happen on separate paths, so the catch-all's binding and a type arm's binding are
   never both live.

A `match` with type arms is the chain of `is<T>`/`downcast<T>` it stands for: `is<T>` borrows to test, and the
`downcast` that follows consumes the box only in the arm taken (AN6).

## Tests

The executable form is `tests/m15_any/`:

| Rule | Tests |
| --- | --- |
| AN1, AN5, AN7 | `any_basics.bd`, `any_return_and_field.bd`, `heterogeneous_list.bd`, `containers.bd`, `drop_glue.bd` |
| AN2, AN12 | `object_safety.bd`, `requires_methods.bd`, `any_default_method.bd` |
| AN3 | `any_return_and_field.bd`, `heterogeneous_list.bd`, `box_view.bd` (E0120), `not_implementing.bd` (E0141), `builtin_display_view.bd` (E0120), `moved_into_box.bd` (E0100) |
| AN4 | `impl_parameter.bd`, `impl_method.bd`, `impl_not_parameter.bd` (E0201); the return-type run test is issue #926 |
| AN5 | `any_basics.bd`, `generic_interface.bd`, `requires_methods.bd`, `any_default_method.bd`, `any_exclusivity.bd` (E0110/E0113), `unknown_any_method.bd` (E0002) |
| AN6 | `downcast.bd`, `downcast_generic.bd`, `marker_interface.bd`, `not_implementing.bd` (E0141) |
| AN8, AN9 | `display_any.bd`, `debug_any.bd`, `requires_methods.bd`, `not_interface.bd` (E0200) |
| AN14 | `builtin_display_box.bd`, `builtin_debug_box.bd`, `builtin_display_downcast.bd`, `builtin_display_view.bd` (E0120) |
| AN13 | `any_upcast.bd`, `any_upcast_not_required.bd` (E0003) |
| AN15 | `match_any.bd`, `match_any_error.bd`, `match_any_nomatch.bd`, `match_any_no_catchall.bd` (E0006), `match_any_not_implementing.bd` (E0141), `match_any_misuse.bd` (E0003, E0100) |
| AN11 | `drop_glue.bd`, `downcast.bd` (the runtime paths) |
| Keyword member names (`grammar.md`, issue #933) | `keyword_member_declaration.bd`, `keyword_fn_name_free.bd`, `keyword_member_other.bd`, `any_item_removed.bd` |
