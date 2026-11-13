# Burdin memory rules (stage 0)

The exact rules the stage 0 compiler enforces for moves, mutability, parameter conventions, exclusivity, references,
returned views, second-class types, and drops. The design document says what the model is; this file says what the
checker does. Each rule has an ID, an error code where it can be violated, and the tests that cover it
(paths relative to `tests/`, without `.bd`).

All checks run on a function body at a time, on the MIR control-flow graph. Nothing here looks at another function's body:
a call is checked from its signature alone.

## 0. Definitions

**Place.** A local variable or parameter, followed by zero or more struct-field or tuple-field selections:
`x`, `p.a`, `o.inner.b`, `t.0`. A place never continues through indexing, a `Heap` dereference, a reference
dereference, or a method call. `xs[i]`, `(*h).a`, and `r.a` through a reference are each treated as one place rooted at
the whole expression's base (`xs`, `h`, `r`).

**Overlap.** Two places overlap when one is a prefix of the other (including equal). `p` and `p.a` overlap, `p.a` and `p.b` do not.

**Initialization state.** At each point of the CFG a place is *initialized*, *moved*, *partially moved* (some field moved
out), or *uninitialized*. Joining paths gives *maybe moved* when the states differ.

**Copy types.** Integers, floats, `bool`, `char`, `&T`, `str`, `[T]`, `()`, and tuples, arrays, structs and enums that are
all-Copy and derive `Copy`. Everything else (`String`, `List`, `Heap`, any type with `Drop`, `&mut T`) is non-Copy. A type
that implements `Drop` can never be Copy.

**Second-class types.** `&T`, `&mut T`, `str`, `[T]`, and any type that contains one (`Option<&i32>`, `(str, i32)`, a
generic instantiated with one). They may live in locals, parameters, return values, and temporaries only (SC rules).

## 1. Moves (MV)

**MV1 Move contexts.** Reading a place of non-Copy type in one of these contexts moves it out and leaves the place *moved*:
`let x = P`, `x = P`, passing `P` to an `own` parameter or as `own self`, returning `P`, putting `P` in a struct, tuple, array,
or variant constructor, and a binding in a pattern that matches an owned place (MV8). Reading a Copy place copies it.
Everything else (a read-only or `mut` argument, field access, a method call with `self` or `mut self`, a comparison)
borrows and does not move.
Tests: `m05_ownership/move_and_reinit`, `m05_ownership/copy_and_clone`.

**MV2 Use of a moved value.** Any use of a place that is *moved* or *maybe moved* is E0100, reported at the use.
Tests: `use_after_move`, `use_after_let_move`, `maybe_moved_branch` (all in `m05_ownership`).

**MV3 Moves in loops.** A move of a place declared outside a loop, inside that loop, with no reinitialization before the
next iteration, is E0104 at the move.
Test: `m05_ownership/moved_in_loop`.

**MV4 Reinitialization.** A moved binding declared `let mut` may be assigned again and is then initialized.
Assigning to a place that is initialized drops the old value first (DR4). An immutable binding can be moved but not reassigned (MU1).
Tests: `m05_ownership/move_and_reinit`, `m09_stdlib/word_count` (reinitializes `word` after moving it).

**MV5 Partial moves.** Moving a field out of an owned place makes the parent *partially moved*. The other fields stay
usable, and the parent as a whole can no longer be used (E0100). If the parent's type implements `Drop`, moving a field out of it
is E0103.
Tests: `m05_ownership/partial_move_ok`, `m05_ownership/partial_move_from_drop`.

**MV6 Heap.** `Heap.new(v)` moves `v`. `*h` on an owned `Heap<T>` binding moves the pointee out, frees the allocation, and
leaves `h` moved. Reading or assigning through `*h` does not move.
Tests: `m05_ownership/heap_tree_eval`, `m07_generics/generic_stack`, `m05_ownership/copy_and_clone`.

**MV7 Move out of borrowed content.** Moving a non-Copy value out of a place that is only borrowed (a read-only or `mut`
parameter, anything behind a reference, an element of a collection) is E0101.
Test: `m06_conventions/move_out_of_readonly_param`.

**MV8 Matching.** The scrutinee of `match`, `if let`, and `let ... else` is a place or a temporary.
If it is an owned place (an owned local, an `own` parameter) or a temporary, bindings in the arm that bind non-Copy parts move
those parts out (the scrutinee becomes partially moved or moved) and Copy parts are copied. If it is a borrowed place (a read-only
parameter, a `mut` parameter, behind a reference), bindings are views of the parts: read-only for a read-only place,
mutable for a `mut` place; nothing moves. A binding is dropped at the end of its arm.
Tests: `m05_ownership/match_moves_owned_scrutinee`, `m05_ownership/match_moves_then_use`, `m06_conventions/match_borrowed_binding`,
`m07_generics/generic_stack`.

## 2. Mutability (MU)

**MU1** Assigning to a place, or taking `&mut` of it, requires the place to be mutable: a `let mut` binding, a `mut` parameter,
or a place reached through a `&mut`. Assigning to anything else is E0005. Mutability is inherited by fields:
`p.a = 1` needs `p` mutable.
Tests: `m03_typecheck/assign_immutable`, `m06_conventions/immutable_let_field_assign`.

**MU2** A read-only parameter is never mutable (E0005). An `own` parameter is an immutable binding too; rebind it
(`let mut c = c`) to change it.
Tests: `m06_conventions/mutate_readonly_param`, `m06_conventions/own_param_immutable`.

**MU3** `&mut P` requires `P` mutable; otherwise E0113.
Test: `m06_conventions/mut_ref_of_immutable`.

## 3. Parameter conventions (CV)

**CV1 Read-only (default).** The callee may read the argument and pass it on to other read-only parameters. It may not mutate
(E0005), move out of it (E0101), or pass it on as `mut`/`own`. The caller's place is borrowed for the duration of the call.
The same value may be passed to several read-only parameters in one call.
Tests: `m06_conventions/conventions_basic`, `m06_conventions/exclusivity_ok`.

**CV2 `mut` parameter.** Exclusive mutable access for the duration of the call. The argument must be a mutable place (E0113 otherwise) and
must be written with the marker `mut` (`f(mut x)`; E0114 if missing). A `mut` marker on an argument whose parameter is not
`mut` is E0115. The callee cannot move out of a `mut` parameter (E0101). It may pass it on as `mut` (`bump(mut c)` inside a
function whose own parameter is `mut c`).
Tests: `m06_conventions/conventions_basic`, `forward_mut_param`, `mut_needs_mut_binding`, `missing_mut_marker`, `unneeded_mut_marker`.

**CV3 `own` parameter.** The argument is moved in (MV1) and the callee owns it and drops it at the end of the call (DR3). No marker at the call site.

**CV4 Receivers.** `self` is read-only, `mut self` needs a mutable place and borrows it exclusively, `own self` moves.
The receiver of a `mut self` method is borrowed mutably without a marker (`c.bump()`).
Tests: `m06_conventions/conventions_basic`, `m04_data/struct_and_methods`.

## 4. Exclusivity (EX)

**EX1** For each call (including the receiver as the first argument), if an argument is passed as `mut P`, then no other
argument expression of that call may mention a place that overlaps `P`, whether it reads, borrows, or moves it. Violation: E0110,
reported at the call. This includes reads nested inside other argument expressions (`add_to(mut c, c.n)`) and the same value in
two `mut` arguments.

**EX2** Overlap uses the place definition above, so different fields of one local or parameter do not overlap
(`swap_vals(mut p.a, mut p.b)` is allowed, also through nested fields and a parameter), while a whole value and one of its fields do,
and anything reached through indexing or a `Heap` dereference is one place.

**EX3** Each call is checked on its own. A `mut` borrow ends when the call returns, unless the call's result is a reference derived
from that argument (RF5).
Tests (allowed): `m06_conventions/exclusivity_ok`, `disjoint_field_borrows`.
Tests (E0110): `exclusivity_same_twice`, `exclusivity_arg_reads_value`, `exclusivity_mut_and_read`, `exclusivity_same_field`,
`exclusivity_struct_and_field`, `exclusivity_through_index`, `exclusivity_through_heap` (all in `m06_conventions`).

## 5. References and borrows (RF)

**RF1** `&P` creates a shared reference to place `P`, `&mut P` a mutable one (MU3). `*r` reads or writes the referent. Field access
and method calls auto-dereference, so `r.n` works for `r: &Counter` and `r: &mut Counter`.
Tests: `m06_conventions/references_basic`.

**RF2 Liveness.** A borrow is live from where it is created to the last use of the reference or of any value derived from it,
on any path. It is not tied to a scope. Liveness is computed on the CFG.
Test: `m06_conventions/borrow_ends_at_last_use`.

**RF3 Conflicts.** While a shared borrow of `P` is live, an overlapping place may not be assigned, moved, borrowed with `&mut`,
or passed as `mut`. While a mutable borrow of `P` is live, an overlapping place may not be read, assigned, borrowed, or moved
at all. Violation: E0111, reported at the conflicting access.
Tests: `write_while_borrowed`, `two_mut_refs`, `shared_and_mut_ref`, `move_while_borrowed`, `derived_borrow_keeps_arg_borrowed`
(all in `m06_conventions`).

**RF4 Copying.** `&T` is Copy. `&mut T` is not: using it moves it, except that passing it where a `&mut` is expected, or
dereferencing it, reborrows it, so it stays usable afterwards.
Test: `m06_conventions/references_basic` (`*m = *m + 5` then use of `x`).

**RF5 Borrows through a call.** An argument is borrowed for the duration of the call (shared for a read-only parameter,
mutable for a `mut` parameter). If the result type is second-class and the callee's signature derives it from that parameter (RT),
the borrow lasts until the last use of the result.
Tests: `m06_conventions/return_ref_single_source` (`*first_mut(mut p) = 9`), `derived_borrow_keeps_arg_borrowed`.

**RF6 Reference parameters.** A parameter of type `&T` or `&mut T` is allowed. The call site passes `&x` or `&mut x`, with no `mut` marker. Inside the
callee it is a reference like any other.
Test: `m06_conventions/reference_parameters`.

## 6. Returned references and views (RT)

A function whose return type is second-class (contains `&`, `str`, `[T]`) must say, implicitly, which parameter its result
comes from. The compiler infers an *origin set* for each returned expression by data flow through the body: a reference to
`p.a` or `xs[i]` has origin `p` or `xs`; `s[0..i]` has origin `s`; calling a function with a second-class result gives that
call's origins mapped back to its arguments; `if` and `match` give the union of their branches.

**RT1** The union of origins over all returned expressions must contain **exactly one parameter**, or be empty with every
returned expression static (RT4).

**RT2** An origin must be a read-only, `mut`, or reference/view-typed parameter. An `own` parameter, a local, or a temporary is dropped at return,
so a result derived from one is E0122, reported at the returned expression.

**RT3** Two or more origin parameters is E0121, reported at the function signature.

**RT4 Static.** String literals are static views. A literal can be returned from any function, whatever its parameters.

Tests: `m06_conventions/return_ref_single_source`, `str_views`, `slice_views`, `return_literal_ok`, `option_of_reference_ok`;
errors: `return_ref_two_sources`, `return_str_two_sources` (E0121), `return_ref_to_local`, `return_view_of_local_string`,
`return_ref_from_own_param` (E0122).

## 7. Second-class types (SC)

**SC1** A second-class value may be a local, a parameter, a return value, or a temporary. It cannot be stored.

**SC2 Forbidden positions (E0120).** A second-class type may not appear as: a struct field type, an enum payload type, a type argument
of a stored-container type (`List`, `Map`, `Heap`, `Shared`, and the user generics that need a first-class parameter), a `const`
type. Tuples, arrays, `Option`, and `Result` inherit second-class-ness from their parts and may hold them as locals.

**SC3 Propagation.** A generic type instantiated with a second-class type is second-class. The check is applied
at the forbidden positions above, so `Option<&i32>` is accepted as a local and rejected as a field.
Tests: `ref_in_struct_field`, `ref_in_enum_variant`, `ref_in_collection`, `str_in_struct_field`, `option_ref_in_struct_field`
(E0120); `option_of_reference_ok`.

## 8. Drop (DR)

**DR1 When.** A value is dropped when the owner goes out of scope while it is initialized, when it is overwritten by assignment
(DR4), or when it is a temporary at the end of its statement (DR5). Moved-out values are not dropped by the old owner.
Copy values are never dropped.

**DR2 Order.** Locals are dropped in reverse order of declaration (shadowed bindings included, DR9). When a value is dropped, its
`Drop::drop` runs first, then its fields in declaration order (struct and enum payload), tuple elements in order, array and `List`
elements in index order.
Tests: `m05_ownership/drop_order`, `m09_stdlib/list_of_drops`.

**DR3 Parameters.** Owned parameters are dropped when the function returns, after all locals, in reverse order of declaration.
Read-only and `mut` parameters are not dropped by the callee.
Test: `m05_ownership/drop_params_and_let_underscore`, `m05_ownership/drop_conditional_move`.

**DR4 Assignment.** `x = v` evaluates `v`, then drops the old value of `x` if it is initialized, then stores `v`.
Test: `m05_ownership/move_and_reinit`.

**DR5 Temporaries.** A temporary is dropped in reverse creation order at the end of the statement that created it, including the
`let` statement itself. Reading a field of a temporary does not extend its life. `let _ = e` does not bind, so `e` is a
temporary. `let _x = e` binds. The temporaries of a block's tail expression are dropped when that expression finishes, before the block's locals.
Tests: `m05_ownership/drop_temporaries`, `drop_params_and_let_underscore`.

**DR6 Early exits.** `return`, `break`, `continue`, and `?` drop every initialized value in the scopes being left, in reverse
order, before control transfers. A returned value is computed before the drops run.
Tests: `m05_ownership/drop_early_return`, `m09_stdlib/question_and_from`.

**DR7 Partial and conditional moves.** Remaining fields of a partially moved owner are dropped at its scope end (in field order).
When a place is moved on only some paths, the compiler tracks it with a runtime drop flag and drops it only when the flag says it is
initialized.
Tests: `m05_ownership/partial_move_ok`, `m05_ownership/drop_conditional_move`.

**DR8 Types with Drop.** A type that implements `Drop` cannot be Copy (E0130), and its fields cannot be moved out (E0103).
Test: `m05_ownership/copy_with_drop`.

**DR9 Shadowing.** `let x = ...` over an existing `x` does not drop the old value; both live to the end of the scope.
Test: `m05_ownership/drop_shadowing`.

**DR10 Panics.** A panic prints its message, file, and line to stderr, flushes stdout, and aborts. No drops run.
Tests: `m02_basics/*_aborts`.

## 9. Derives that depend on these rules (CP)

`Copy` requires every field to be Copy and the type to have no `Drop`; `Clone`, `Eq`, `Ord`, `Hash`, and `Default` require every field
to implement them. A violation is E0130, reported at the offending field (or the derive clause for the `Drop` case).
Tests: `m05_ownership/copy_with_drop`, `m07_generics/derive_field_not_eq`.

## 10. Unsafe (US)

Calling an `extern` function and any raw-pointer operation require an `unsafe` block or `unsafe fn` (E0161). Inside `unsafe` all
the rules above still apply; `unsafe` only enables those operations.
Test: `m08_modules/unsafe_required`.

## Summary of error codes used here

E0005 MU1, MU2 · E0100 MV2, MV5, MV8 · E0101 MV7, CV1, CV2 · E0103 MV5 · E0104 MV3 · E0110 EX1 · E0111 RF3 ·
E0113 CV2, MU3 · E0114 CV2 · E0115 CV2 · E0120 SC2 · E0121 RT3 · E0122 RT2 · E0130 DR8, CP · E0161 US.
