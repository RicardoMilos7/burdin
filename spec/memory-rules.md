# Burdin memory rules (stage 0)

The exact rules the stage 0 compiler enforces for moves, mutability, parameter conventions, exclusivity, references,
returned views, second-class types, and drops. The design document says what the model is; this file says what the
checker does. Each rule has an ID, an error code where it can be violated, and the tests that cover it
(paths relative to `tests/`, without `.bd`).

All checks run on a function body at a time, on the typed tree (`burdin_tir`). Nothing here looks at another
function's body: a call is checked from its signature alone. The compiler has no MIR; the design's MIR steps were
decided against (`stage1-subset.md`, decision 6).

Each check also records the ownership events it observes — declarations, moves, copies, borrows, drops, drop flags,
argument conventions, and derived clones — as `Function::events` (`spec/check.md`, CK10). The command line
(`burdin check --explain-ownership`), the language server's ownership lens, and the REPL render that one list; a tool
never re-derives these facts. `burdin check --explain` prints the path to a memory error and names the rule ID below
(CK11).

## 0. Definitions

**Place.** A local variable or parameter, followed by zero or more struct-field or tuple-field selections:
`x`, `p.a`, `o.inner.b`, `t.0`. A place never continues through indexing, a `Heap` dereference, a reference
dereference, or a method call. `xs[i]`, `(*h).a`, and `r.a` through a reference are each treated as one place rooted at
the whole expression's base (`xs`, `h`, `r`). A user type's `x[i]` is a place too: it lowers to `*x.index(i)` (or
`*x.index_mut(i)` when it is written), so the element is a view of `x` and the ordinary borrow, move, and exclusivity
rules apply to it; a non-`Copy` element cannot be moved out, and two elements of one container are one place
(`spec/operators.md`, OP9-OP10).

**Overlap.** Two places overlap when one is a prefix of the other (including equal). `p` and `p.a` overlap, `p.a` and `p.b` do not.

**Initialization state.** At each program point a place is *initialized*, *moved*, *partially moved* (some field moved
out), or *uninitialized*. Joining paths gives *maybe moved* when the states differ.

**Copy types.** Integers, floats, `bool`, `char`, `&T`, `str`, `[T]`, `CPtr<T>`, `Range<T>`/`RangeInclusive<T>`, `()`,
and tuples, arrays, structs and enums that are all-Copy and derive `Copy`. `&T`, `str`, `[T]`, and `CPtr<T>` are Copy
without a derive (RF4). Everything else (`String`, `List`, `Map`, `Heap`, any type with `Drop`, `&mut T`) is
non-Copy. A type that implements `Drop` can never be Copy.

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

**MV6 Heap.** `Heap.new(v)` moves `v`. `*h` on an owned `Heap<T>` binding moves the pointee out; the allocation is
freed when the `Heap` is dropped (its flag records that the pointee is gone). Reading or assigning through `*h` does not
move. A field of the pointee moves like any field: `(*h).a` leaves the other fields initialized, and the `Heap`'s drop
drops them and frees the allocation. A constant array or `List` index is tracked the same way, as a partial move of the
collection; moving through a runtime index is still E9000 (the checker cannot tell which element it is). An index
expression is evaluated once, when the place's address is computed; a tracked move is written with a constant index,
so there is no expression to re-evaluate.
Tests: `m05_ownership/heap_tree_eval`, `heap_move_whole_then_field`, `move_heap_field`, `move_heap_field_reinit`,
`move_heap_field_after_move` (E0100), `match_heap_pointee`, `match_heap_pointee_partial`, `move_index_array`,
`move_index_list`, `move_index_after_move` (E0100), `index_evaluated_once`,
`m07_generics/generic_stack`, `m05_ownership/copy_and_clone`.

**MV7 Move out of borrowed content.** Moving a non-Copy value out of a place that is only borrowed (a read-only or `mut`
parameter, anything behind a reference, an element of a collection) is E0101.
Test: `m06_conventions/move_out_of_readonly_param`.

**MV8 Matching.** The scrutinee of `match`, `if let`, `while let`, and `let ... else` is a place or a temporary.
If it is an owned place (an owned local, an `own` parameter) or a temporary, bindings in the arm that bind non-Copy parts move
those parts out (the scrutinee becomes partially moved or moved) and Copy parts are copied. If it is a borrowed place (a read-only
parameter, a `mut` parameter, behind a reference), bindings are views of the parts: read-only for a read-only place,
mutable for a `mut` place; nothing moves. A binding from a borrowed scrutinee is a read-only place used like a plain
value, with no `&` written: reading it, passing it to a read-only parameter, and taking fields of it work; assigning
to it is E0005 and moving out of it is E0101. A binding is dropped at the end of its arm.
A temporary scrutinee is moved into a compiler-made local of the same statement, so its untouched parts still drop
once and a failed guard restores the parts an earlier arm bound.
A `..` in a pattern (`spec/rest-patterns.md`, RP2) binds nothing, so the parts it stands for stay in the scrutinee
exactly as a `_` would leave them. A struct update's taken fields are moved out of its base the same way
(`spec/struct-update.md`, SU4 and SU5), and a base that is a temporary is held in a compiler-made local of the literal.
Tests: `m05_ownership/match_moves_owned_scrutinee`, `m05_ownership/match_moves_then_use`, `match_temporary_fields_drop`,
`match_guard_temporary`, `match_guard_temporary_fields`, `match_heap_pointee_partial`, `m06_conventions/match_borrowed_binding`, `match_borrowed_binding_readonly`,
`m07_generics/generic_stack`.

**MV9 Block values.** The value of a block, an `if`, or a `match` (including the one a `?` lowers to) is an owned
temporary whatever uses it. Its tail or arm moves the value out, even where the expression is only borrowed: a read-only
argument, a method receiver, a comparison operand, a `let _`. A `loop`'s value (what its `break` gives back) is owned the
same way. A temporary scrutinee of such a value is moved into its
compiler-made local as MV8 says, so the value drops exactly once, at the end of the statement that created it (DR5),
and a local the tail moves out is not usable afterwards (E0100).
Tests: `m05_ownership/temporary_question_arg` (run), `m05_ownership/move_block_value_borrowed` (E0100).

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

**MU4 Slices.** Indexing a slice is read-only: an element of a `[T]` is not a mutable place, because a slice view
carries no mutability of its own. The compiler's slice methods that write (`swap`, `fill`, `copy_from_slice`,
`clone_from_slice`, and the sort) take `mut self`, so the slice must be a mutable place (MU1) and the call borrows
it exclusively. A slice expression or a `List.as_slice()` call may be the receiver when the place it views is
mutable: `xs[0..2].sort()` needs a mutable `xs` (E0113 otherwise). A `mut` slice parameter may call the writing
methods, but `xs[0] = v` is still E0005, so the mutation surface is exactly the methods that take `mut self`.
Tests: `m24_stdlib/slice_write`, `slice_reverse_immutable` (E0113), `slice_view_immutable_list` (E0113).

## 3. Parameter conventions (CV)

**CV1 Read-only (default).** The callee may read the argument and pass it on to other read-only parameters. It may not mutate
(E0005), move out of it (E0101), or pass it on as `mut`/`own`. The caller's place is borrowed for the duration of the call.
The same value may be passed to several read-only parameters in one call.
Tests: `m06_conventions/conventions_basic`, `m06_conventions/exclusivity_ok`.

**CV2 `mut` parameter.** Exclusive mutable access for the duration of the call. The argument must be a mutable place (E0113 otherwise) and
must be written with the marker `mut` (`f(mut x)`; E0114 if missing). A `mut` marker on an argument whose parameter is not
`mut` (including an `own` parameter) is E0115. The callee cannot move out of a `mut` parameter (E0101). It may pass it on as `mut` (`bump(mut c)` inside a
function whose own parameter is `mut c`).
Tests: `m06_conventions/conventions_basic`, `forward_mut_param`, `mut_needs_mut_binding`, `missing_mut_marker`, `unneeded_mut_marker`,
`call_site_markers`, `own_mut_marker`.

**CV3 `own` parameter.** The argument is moved in (MV1) and the callee owns it and drops it at the end of the call (DR3). No marker at the call site; a `mut` marker is E0115.

**CV4 Receivers.** `self` is read-only, `mut self` needs a mutable place and borrows it exclusively, `own self` moves.
The receiver of a `mut self` method is borrowed mutably without a marker (`c.bump()`); a `mut` marker in receiver
position is a syntax error (E0001).
Tests: `m06_conventions/conventions_basic`, `call_site_markers`, `receiver_mut_marker`, `m04_data/struct_and_methods`.

**IC1 Implicit conversion.** A `String` value at a position whose type is `str` is borrowed as a `str`; the `String`
is not moved, and the view follows the lifetime rules of an explicit `.as_str()` (a view of a temporary or a local
that is returned is E0122, one that outlives its storage is E0123). The positions are: a call argument whose parameter
type is `str`, the payload of `Option.Some`, `Result.Ok`, and `Result.Err`, the value of `mem.replace`, `assert_eq`'s second operand when the
first is a `str`, a `let` with a `str` annotation, the right side of `=`, a `return` value and a function body's tail
whose return type is `str`, an element of an array or tuple whose element type is `str`, and a struct-literal field
of a generic struct instantiated with `str` (a concrete `str` field is E0120). The conversion applies to the `String`
itself, never inside another type (`Option<String>` is not `Option<str>`, E0003), and a built-in name that is not
in one of these positions keeps the type `String` and is E0003 when `str` is expected. A comparison
(`==`, `!=`, `<`, `<=`, `>`, `>=`) with a `String` on one side and a `str` on the other borrows the `String` as a `str`
too, and so does the `String` operand of `String + str` (`operators.md` OP14). `.as_str()` is the explicit conversion. These are
the **only** implicit conversions in the language. There are no `&mut T` to `&T`, `&T` to `&mut T`, `&String` to
`str`, `String` to `[u8]`, numeric widening, or user-defined conversions (all E0003).
Tests: `m06_conventions/string_str_conversion`, `string_str_positions`, `let_string_to_str`, `m09_stdlib/string_ops`,
`m19_operators/string_str_operators`; errors: `string_str_not_nested`, `return_temp_string_as_str`, `mut_ref_to_shared_ref`,
`ref_string_to_str`, `string_to_slice`.

## 4. Exclusivity (EX)

**EX1** For each call (including the receiver as the first argument), if an argument is passed as `mut P`, then no other
argument expression of that call may mention a place that overlaps `P`, whether it reads, borrows, or moves it. Violation: E0110,
reported at the call. This includes reads nested inside other argument expressions (`add_to(mut c, c.n)`) and the same value in
two `mut` arguments.

**EX2** Overlap uses the place definition above, so different fields of one local or parameter do not overlap
(`swap_vals(mut p.a, mut p.b)` is allowed, also through nested fields and a parameter), while a whole value and one of its fields do,
and anything reached through indexing or a `Heap` dereference is one place. The receiver and the arguments of a
method call form one call, and a method call used as an argument is a whole place rooted at its base (its fields,
indices, and dereferences do not make it a smaller place). Two elements of one array are one place, so
`swap_vals(mut xs[0].a, mut xs[1].a)` is E0110, as is `swap_vals(mut p, mut p.a)`; `xs[0].a` and `xs[1].a` are not
separate places even though they are different fields of different elements.

**EX3** Each call is checked on its own. A `mut` borrow ends when the call returns, unless the call's result is a reference derived
from that argument (RF5).
Tests (allowed): `m06_conventions/exclusivity_ok`, `disjoint_field_borrows`.
Tests (E0110): `exclusivity_same_twice`, `exclusivity_arg_reads_value`, `exclusivity_mut_and_read`, `exclusivity_same_field`,
`exclusivity_struct_and_field`, `exclusivity_through_index`, `exclusivity_through_heap` (all in `m06_conventions`).

## 5. References and borrows (RF)

**RF1** `&P` creates a shared reference to place `P`, `&mut P` a mutable one (MU3). `*r` reads or writes the referent. Field access
and method calls auto-dereference, so `r.n` works for `r: &Counter` and `r: &mut Counter`.
Tests: `m06_conventions/references_basic`.

**RF2 Liveness.** A borrow is live from where it is created to the last use of the reference or of any value derived
from it. It is not tied to a scope. Stage 0 computes liveness per top-level statement, so a use anywhere inside a loop
keeps the loan live for the whole loop; that is conservative, not a full CFG analysis.
Test: `m06_conventions/borrow_ends_at_last_use`.

**RF3 Conflicts.** While a shared borrow of `P` is live, an overlapping place may not be assigned, moved, borrowed with `&mut`,
or passed as `mut`. While a mutable borrow of `P` is live, an overlapping place may not be read, assigned, borrowed, or moved
at all. Violation: E0111, reported at the conflicting access.
Tests: `write_while_borrowed`, `two_mut_refs`, `shared_and_mut_ref`, `move_while_borrowed`, `derived_borrow_keeps_arg_borrowed`
(all in `m06_conventions`).

**RF4 Copying.** `&T`, `str`, `[T]`, and `CPtr<T>` are Copy without deriving anything: reading one copies it, and it
stays usable. `&mut T` is not: assigning it to another local, returning it, or storing it in a struct moves it, and
using the moved value is E0100. Passing it where a `&mut T` is expected, or dereferencing it with `*`, reborrows it,
so it stays usable afterwards (`*m = *m + 5` then `x`; passing `m` to another `&mut` parameter).
Tests: `m06_conventions/references_basic`, `views_are_copy`, `mut_ref_move_then_use`, `str_views`, `slice_views`.

**RF5 Borrows through a call.** An argument is borrowed for the duration of the call (shared for a read-only parameter,
mutable for a `mut` parameter). If the result type is second-class and the callee's signature derives it from that parameter (RT),
the borrow lasts until the last use of the result. Converting a reference to a raw pointer (`&mut x as CPtr<T>`) ends
the loan at the conversion: the pointer is only usable inside `unsafe`, where the programmer is responsible for its
validity (US).
Tests: `m06_conventions/return_ref_single_source` (`*first_mut(mut p) = 9`), `derived_borrow_keeps_arg_borrowed`,
`m08_modules/c_out_parameter`.

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

**RT3** Two or more origin parameters is E0121, reported at the function signature, unless the result is a tuple or an
instance of a struct or enum (RT3a): a result that is a view (`&T`, `&mut T`, `str`, `[T]`) or a built-in wrapper of one
(`Option<&T>`, `Result`, an array, a list) derives from exactly one parameter.

**RT3a Several origins in an aggregate.** A result that is a tuple, or an instance of a generic struct or enum, that
holds views (`(&i32, &i32)`, `Pair<&i32, &i32>`, a chain such as `ZipIter<[i32], i32, [i32], i32>`) may derive from several
parameters. The signature's return origin is then the set of those parameters (`Signature.ret_origins`), a call's
result takes the union of the storage of the matching arguments, and the borrow checker keeps every one of them
borrowed while the result is live (RF5): writing or moving any source is E0111 or E0100. The reason for the
asymmetry with RT3 is that a result that is a single view would force a caller to guess which source it came from,
while an aggregate that holds several views is exactly their union. An origin that is a local or a temporary is still
E0122 (RT2).
Tests: `m06_conventions/return_aggregate_two_sources` (run); errors: `return_aggregate_two_sources_borrow`,
`return_aggregate_first_source_borrow` (E0111), `return_option_ref_two_sources` (E0121);
`m24_stdlib/iter_zip`, `iter_chain` (the chains), `iter_chain_while_mutating_second`, `iter_zip_while_mutating_first`.

**RT4 Static.** String literals are static views. A literal can be returned from any function, whatever its parameters,
in any returned expression: a tail expression, `return`, a block, an `if` branch, or a `match` arm. The same holds for
an `embed("path")` whose expected type is `str` or `[u8]` (`const-eval.md`, CE12): its bytes are static data. A `const`
of type `str` whose value is a string literal, directly or through another such constant, is static too (a constant may
not have type `[u8]`, E0221). No other expression is static in stage 0: a view of a local, parameter, temporary, `String`, or non-`str` constant follows
RT1-RT3, and `&` of a constant is E0003 because a constant is not a place.
Tests: `m06_conventions/return_literal_ok`, `static_literals`, `const_is_not_a_place`, `return_ref_single_source`,
`str_views`, `slice_views`, `option_of_reference_ok`, `m21_reflect/embed_slice`;
errors: `return_ref_two_sources`, `return_str_two_sources` (E0121), `return_ref_to_local`, `return_view_of_local_string`,
`return_ref_from_own_param` (E0122).

**RT5 View lifetime in a local.** A value of a second-class type stored in a local (`let`, an assignment, or a
pattern binding of a `let`) may derive only from static data (RT4), from a parameter, or from a local that is
declared in the same block or in an enclosing block. It may not derive from a temporary, which is dropped at the
end of the statement that created it, or from a local of an inner block, which is dropped before the binding
escapes. Violation: E0123, reported at the stored value. Bindings of a `match`, `if let`, or `for` pattern are
not subject to RT5: their scrutinee's temporaries live until the end of the same statement, and the binding lives
no longer than the arm or loop body. The same holds for a `while let` pattern: each pass evaluates the scrutinee
again, and its bindings and temporaries end with that pass (`spec/grammar.md`, rule 8.11).
Tests: `m06_conventions/view_lifetime_ok`, `view_of_earlier_local_ok`;
errors: `view_outlives_temporary`, `view_outlives_inner_local`, `view_outlives_string` (all in `m06_conventions`,
E0123).

## 7. Iteration (IT)

**IT1 Borrow.** `for pattern in C { ... }` borrows `C` shared for the whole loop, from the start of the loop to the
end of its body. The borrow ends after the loop, so `C` is usable afterwards. It is the same loan RF2/RF3 describe:
writing, moving, `&mut`-borrowing, or passing `C` (or an overlapping place) as `mut` inside the body is E0111. There
is no way to consume or mutate the collection through the `for` header; to consume a collection, write
`C.into_iter()` explicitly (IT4-IT12).

**IT2 Bindings.** The loop binding is a read-only place used like a plain value, with no `&` written. Assigning to it
is E0005; moving a non-Copy element out of it is E0101.

**IT3 Forms.** `for x in xs` yields each element of an array, slice, or `List`. `for (k, v) in map` yields each entry
of a `Map` (order unspecified). `for c in s.chars()` yields `char` scalar values, and `for i in a..b` / `a..=b`
yields the integers of the range. A range is an ordinary `Copy` value, so `for i in r` over a stored range
evaluates `r` once and does not borrow it.
Tests: `m09_stdlib/for_borrows_list`, `for_borrow_forms`, `range_forms`; errors: `for_mutate_array`, `for_mutate_map`
(E0111), `for_binding_readonly` (E0005).

**IT15 Mutable iteration.** `for pattern in mut C { ... }` calls `C.iter_mut()` (`mut` receiver, so `C` must be a
mutable place, E0113) and walks the iterator, whose items are `&mut T` (`iterators.md`, IT15). The call borrows `C`
exclusively from the start of the loop until the last use of the iterator or of a view derived from it: any other
access to `C` or a place overlapping it inside the body, a `push`, a read, or another loop over `C`, is E0111 (RF3). A
view of an element copied into a local that outlives the loop (`held = x`) holds the loan until its last use. Keys
of a `Map` are `&K`: writing through one is E0005. A `for` without `mut` is unchanged (IT1, IT2).
Tests: `m24_stdlib/iter_mut_list`, `iter_mut_array`, `iter_mut_map`, `iter_mut_deque`; errors: `iter_mut_mutates_collection`,
`iter_mut_nested_loops`, `iter_mut_hold_past_loop`, `iter_mut_immutable` (E0113), `iter_mut_map_key_readonly` (E0005).

**IT4-IT12 The protocol.** Stage 1 adds `Iterator<T>` and `Iterable<T>` in `std`, compiler-known `iter`, `into_iter`,
and `next` for arrays, slices, `List`, `Map`, `Chars`, and ranges, user-defined `for` loops, the lazy adapters, and
the eager `List` methods. The rules are in [`iterators.md`](iterators.md). The protocol preserves IT1-IT3: `for`
borrows its source and prefers `iter()` when a type is both an `Iterable` and an `Iterator`; `next` needs a mutable
place (E0113) and `into_iter` moves the collection (E0100); a chain is second-class (SC2/SC3, E0120/E0123); and a
`for` pattern that does not match every item is E0006. Tests: `tests/m13_iterators/`.

## 8. Second-class types (SC)

**SC1** A second-class value may be a local, a parameter, a return value, or a temporary. It cannot be stored.
`CPtr<T>` is not second-class: it is a plain `Copy` value that may be stored, with `unsafe` (US) making every
use of it unsafe; `CStr` is an alias for `CPtr<CChar>` (`stdlib.md`, issue #83).

**SC2 Forbidden positions (E0120).** A second-class type may not appear as: a struct field type, an enum payload type, a type argument
of a stored-container type (`List`, `Map`, `Heap`, `Shared`, and the user generics that need a first-class parameter), a `const`
type. Tuples, arrays, `Option`, and `Result` inherit second-class-ness from their parts and may hold them as locals.

**SC3 Propagation.** A generic type instantiated with a second-class type is second-class. The check is applied
at the forbidden positions above, so `Option<&i32>` is accepted as a local and rejected as a field. The same holds
for a user generic (`Wrapper<&i32>`), and its own field may hold the view: the instance is what becomes second-class,
so the field declaration is not itself E0120.
Tests: `ref_in_struct_field`, `ref_in_enum_variant`, `ref_in_collection`, `str_in_struct_field`, `option_ref_in_struct_field`
(E0120); `option_of_reference_ok`, `m07_generics/view_instance_in_field` (E0120), `view_instance_local`.

## 9. Drop (DR)

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
Tests: `m05_ownership/drop_temporaries`, `drop_block_tail`, `drop_params_and_let_underscore`.

**DR6 Early exits.** `return`, `break`, `continue`, and `?` drop every initialized value in the scopes being left, in reverse
order, before control transfers. A returned value is computed before the drops run, and so is a `break` value (which
the loop then gives back). A labeled `break 'l` or `continue 'l`
leaves every loop between it and the loop labeled `'l`, and drops the scopes of all of them, innermost first (issue #275).
Tests: `m02_basics/labeled_loops` (the drops of a labeled `break` and of a labeled `continue`).
Tests: `m05_ownership/drop_early_return`, `m09_stdlib/question_and_from`.

**DR7 Partial and conditional moves.** Remaining fields of a partially moved owner are dropped at its scope end (in field order;
an array or `List` drops its remaining elements in index order, and a `Heap` drops the remaining pointee fields and frees).
When a place is moved on only some paths, the compiler tracks it with a runtime drop flag and drops it only when the flag says it is
initialized.
Tests: `m05_ownership/partial_move_ok`, `m05_ownership/drop_conditional_move`.

**DR8 Types with Drop.** A type that implements `Drop` cannot be Copy (E0130), and its fields cannot be moved out (E0103).
A generic type implements `Drop` once for every instance (`Type<T>: Drop { ... }`), and each instance's drop glue calls
it; the block cannot name specific arguments or take a `where` clause, because the glue of one instance cannot differ
from the type's others (`impls.md` IM7, E0131).
Test: `m05_ownership/copy_with_drop`.

**DR8a Containers with a stored allocator.** A container that stores its allocator (`List<T, A>`, `Map<K, V, A>`,
`Heap<T, A>`) owns that value as an ordinary field. Dropping it drops the elements, frees the buffer through the
allocator with the buffer's size and alignment, and then drops the allocator value; moving the container moves
both. A `Shared<T>` owns one reference: dropping it decrements the count, and the last owner drops the value and
frees the cell. The details are `spec/allocators.md` AL7-AL8.
Tests: `m23_allocators/drop_through_allocator`, `m23_allocators/heap_arena_move_out`, `m23_allocators/shared_drop_once`.

**DR9 Shadowing.** `let x = ...` over an existing `x` does not drop the old value; both live to the end of the scope.
Test: `m05_ownership/drop_shadowing`.

**DR10 Panics.** A panic prints `<file>:<line>:<column>: panic: <message>` to stderr, flushes stdout first, and aborts
with a non-zero status (SIGABRT on POSIX, a non-zero exit on Windows). No drops run, and no further statements or
temporaries are evaluated. `panic(msg)`, `unreachable()`, `assert`, a failed `expect`, overflow in debug, division by
zero, and an out-of-bounds index all take this path. After the message line the runtime prints a symbolized
`stack backtrace:` section, hiding its own frames; the message line and the status are the stable part. The one
exception is a stack overflow, which is not a panic with a source location: the main-thread guard prints
`panic: stack overflow` and exits 134 (`DR10a`). Standard output is buffered; a panic flushes it before writing
anything to stderr, so the observable order is unchanged.

**DR10a Stack overflow.** The main thread has a guard: exceeding its stack prints `panic: stack overflow` to stderr
and ends the process with status 134. On Unix an alternate signal stack and a `SIGSEGV` handler distinguish a fault
in the stack's guard region from another fault (which prints `panic: invalid memory access`); on Windows a vectored
exception filter handles `EXCEPTION_STACK_OVERFLOW`, with a 64 KiB stack guarantee reserved so the filter always has
room to flush and write the message. No drops run.

Tests: `m02_basics/*_aborts`, `panic_reports_location`, `panic_flushes_stdout`, `panic_runs_no_drops`,
`panic_backtrace`, `panic_backtrace_names`, `stack_overflow_aborts`; `tests/check_debug_info.py`.

## 10. Derives that depend on these rules (CP)

`Copy` requires every field to be Copy and the type to have no `Drop`; `Clone`, `Eq`, `Ord`, `Hash`, and `Default` require every field
to implement them. A violation is E0130, reported at the offending field (or the derive clause for the `Drop` case). The exact
derived members, their comparison orders, and the `Hash` algorithm are in `derives.md`.
Tests: `m05_ownership/copy_with_drop`, `m07_generics/derive_field_not_eq`, `m07_generics/derive_values`.

## 11. Unsafe (US)

Calling an `extern` function, calling an `unsafe fn` or an `unsafe` interface method, and any raw-pointer operation
(holding, passing, dereferencing, or casting a `CPtr`) require an `unsafe` block or an `unsafe fn` body (E0161). The
body of an `unsafe fn` is an unsafe context; the caller still needs one. An implementation of an `unsafe` interface
method must repeat the marker (E0003). Inside `unsafe` all the rules above still apply; `unsafe` only enables those
operations.
Tests: `m08_modules/unsafe_required`, `cptr_requires_unsafe`, `cptr_operations`, `unsafe_fn`,
`unsafe_fn_call_requires_unsafe`, `m07_generics/unsafe_interface_method`, `m07_generics/unsafe_interface_mismatch`.

## 12. Closures (CL)

Stage 1, milestone 12. A closure is `|a, b| body`; its type is one of the compiler-known closure interfaces
`Fn(A, B): R`, `FnMut(A, B): R`, or `FnOnce(A, B): R` (`grammar.md` section 4). The representation is a boxed capture
record with a code pointer; these rules are the semantics, not the layout.

**CL1 Capture set.** A closure captures every local or parameter of the enclosing function that its body uses and
that the body does not declare itself. A capture is always a *whole* variable: using `x.y` captures `x`. A nested
closure captures from the closure that encloses it, so a capture chain is allowed. A closure body's own parameters
and `let`/pattern bindings are not captures. Using an unknown name is E0002 as usual.

**CL2 Capture mode.** A closure captures by **borrow** when it is used where it does not have to outlive its
definition: a local binding without a written closure type, an argument to a read-only or `mut` parameter, a call's
callee, or a temporary. It captures by **move** when the context stores or returns it: a `let`/assignment with a
written `Fn(...)` type, a struct or enum field, an array, tuple, `List`, or `Map` element with a known element type, a
`return` expression, an `own` parameter, or any parameter whose type is a closure interface. In short, an owned
closure type in an expected position selects move captures; otherwise captures borrow.

**CL3 Borrow captures.** A borrow capture is shared (`&T`) unless the body writes the captured place (an assignment,
`&mut`, a `mut` argument, a mutating method, `swap`, or `replace`), in which case it is mutable (`&mut T`). Mutating
a capture requires the captured binding to be mutable where it is declared. A closure with at least one borrow
capture is **second-class** (SC2), and so is a closure that moves a second-class value (`str`, `&T`, a slice, or an
aggregate containing one) into its record: that value still points at storage the closure does not own. A
second-class closure may be a local, a parameter, a temporary, or an argument, but it cannot be stored in a field,
variant, or collection (E0120), and it cannot be returned or assigned where an owned closure type is expected
(E0171). Storing it in a local that outlives one of its captured locals is E0123.

**CL4 Move captures.** A move capture moves the captured place into the closure; a Copy capture is copied. Moving out
of a read-only or `mut` parameter is E0101, so a closure that must outlive the call captures `own` parameters (or
copies them into a local first). The closure value owns its captures: dropping the closure drops them, and moving the
closure moves them. A closure with only move captures is a first-class owned value.

**CL5 Kind.** The closure's kind follows how the body uses its captures: **FnOnce** when the body moves a
move-captured non-Copy value out of the closure; **FnMut** when the body mutates a capture but moves none; **Fn**
otherwise. A closure value is usable where the same signature at a lower capability is expected (`Fn` as `FnMut` or
`FnOnce`, `FnMut` as `FnOnce`), never the reverse, which is E0141 at a bound and E0003 elsewhere.

**CL6 Calling.** `f(args)` calls the closure `f`: an `Fn` closure is borrowed shared for the call, an `FnMut`
closure is borrowed exclusively and must be a mutable place (E0113), and an `FnOnce` closure is moved by the call (so
using it again is E0100, and calling one held by a read-only parameter is E0101). The arguments are passed by value,
as if every closure parameter were `own`: a non-Copy argument is moved. A callee of type `Ty::Param` needs an
`Fn`-family bound; calling it without one is E0141.

**CL7 Exclusivity and loans.** Creating a closure with borrow captures takes the same loans as `&`/`&mut` on each
captured place. The loans are held by the closure value: they last until the closure's last use (RF2), so writing a
captured place while the closure value is still used is E0111. An `FnMut` call borrows the closure exclusively for
the call, so no other argument of that call may mention the closure place (EX1). A `Fn`/`FnMut` call does not move
the closure value.

**CL8 Returns, moves, and drops inside a closure body.** `return` inside a closure body returns from the closure, not
from the enclosing function. `break` and `continue` cannot cross a closure boundary (E0001); a labeled one that names a loop
outside the closure is E0002, because the label is not in scope inside it. The closure body is
checked as its own function: its own parameters are owned and dropped at its end, and for a `FnOnce` closure the
captured values that remain are dropped there (the call consumed the capture record). A `Fn`/`FnMut` value called
where a weakened `FnOnce` type is expected (CL5) is consumed by that call as well: once the body returns, the record
and the captures it still owns are destroyed, on the normal path and on an early `return`. A `Fn`/`FnMut` closure's
captures are borrowed from the capture record for each ordinary call, so the body cannot move them out (E0101).

**CL9 Closure types in signatures.** A closure interface written as a type is owned (`borrowed` is not part of its
spelling and it is storable). A closure parameter that is a type parameter (`F` with an `Fn` bound) keeps the
concrete closure type after monomorphization, including whether it borrows; the callee then cannot store the
parameter (SC2/E0120) or return it as an owned closure. Passing a borrowing closure where an owned closure interface
is expected is E0171.

**CL10 Not derived.** A closure value is never `Copy`, `Clone`, `Eq`, `Ord`, `Hash`, or `Default`; calling `clone` is
E0002 and putting one in a derived comparison is E0130. A closure may be stored in a `List`, `Map`, `Heap`,
`Option`, or `Result` only when it owns its captures (CL3).

**CL11 Function values.** A non-generic function name, a built-in function name (`println`, `panic`, the runtime
helpers), or one of the prelude constructors `Option.Some`, `Result.Ok`, or `Result.Err` may be used where a closure type is expected. It
becomes a captureless closure of kind `Fn` whose body forwards its arguments to the function, so CL6's rule applies:
every closure parameter is owned and each argument is moved into the call. Forwarding uses the function's own
conventions (a read-only or `mut` parameter is passed by address, an `own` parameter by value). The value is
first-class and owns only its empty capture record. A function whose return type is a view (`str`, `&T`, `[T]`)
cannot be used this way: the returned view would point into the closure's own argument, which is dropped when the call
returns (E0171). An `unsafe fn` cannot be used as a value either, because a closure call has no `unsafe` boundary
(E0161); write a closure that calls it in an `unsafe` block. A generic function or an `extern` function as a value is
E9000 for now.

A constructor has no signature, so the expected closure type fixes its type arguments. The closure has exactly one
parameter, the payload: `Option.Some`, `Result.Ok`, and `Result.Err` each take one argument, so a closure with another number of parameters
is E0003. `Option.Some` builds `Option<P>` from the payload alone, so its result may be open in the call that takes it (a
`map` whose result is still `U` binds `U` to `Option<P>`). `Result.Ok` and `Result.Err` also need the argument the payload does not
fix: `Result.Ok` takes its error type from the expected result, and `Result.Err` its value type. A result of the wrong shape is
E0003. An argument the expected type leaves open is E0003 as well, so `xs.map(Result.Ok)` whose result is only `U` is
E0003: nothing in the call says what the error type is. A constructor with no expected closure type is E0003.

**Method values.** `Type.name`, where `Type` is a non-generic struct or enum and `name` is one of its associated functions
or methods, is a function value like a named function. Its parameters are the declared ones, and a method's receiver is
its first parameter, so `Point.double` is `Fn(Point): i32`. The value forwards the receiver with the method's own
convention (a read-only or `mut` receiver by address, an `own self` by value), as a named function does.
`recv.name` written without a call is a method value on a bound receiver, where `recv` is a local name or `self`: the
value is the closure `|args| recv.name(args)`, with the method's parameter types. Its captures follow CL2: a read-only
or `mut` method borrows the receiver, an `own self` method moves it, and an owned closure type expected by the context
moves it too. A receiver that is any other expression, and a method with type parameters of its own, are E0002; the
message says to bind the receiver to a name or to write a closure.

Tests: `tests/m12_closures/` (`closure_function_value.bd`, `closure_constructor_value.bd`, `closure_method_value.bd`,
and the compile-fail `closure_constructor_untyped.bd`, `closure_constructor_mismatch.bd`, `closure_constructor_open.bd`,
`closure_method_value_errors.bd`).

## 13. Scoped tasks (SK)

`Scope.spawn(f)` (`std.task`, `spec/tasks.md` TK7) starts a task that runs until the end of its `scope`, so it is the
one place a closure's borrow outlives the call that took it. The checker adds three rules, on one function body (the
scope body is a closure):

* **SK1.** The loans taken by the closure given to `Scope.spawn` last until the end of the scope body, whatever holds
  them. Any later access of a place a task borrows mutably, and any write or `&mut` of a place a task borrows at all, is
  E0111 (RF3). Places that tasks only read may be borrowed by any number of tasks.
* **SK2.** A task may borrow only places that outlive the scope body: the captures of the body. Borrowing a local of
  the body, which is dropped before the scope waits, is E0171. A closure that only copies or moves its captures may
  borrow nothing and can be passed by name.
* **SK3.** A `Scope.spawn` inside a loop is one task per iteration: a mutable loan it takes would be taken once per
  iteration, so it is E0111. A closure that borrows must be written at the `spawn` call (E0171 otherwise), because the
  rules above follow the loans the call site creates.

Tests: `tests/s2_tasks/scope_*.bd`.

## Summary of error codes used here

E0003 IC1, RT4, CL5, CL9 · E0005 MU1, MU2, MU4 · E0100 MV2, MV5, MV8, MV9, CL6 · E0101 MV7, CV1, CV2, CL4, CL6, CL8 ·
E0103 MV5 · E0104 MV3 · E0110 EX1 · E0111 RF3, IT1, CL7, SK1, SK3 · E0113 CV2, MU3, MU4, CL6 · E0114 CV2 · E0115 CV2 ·
E0120 SC2, CL3 · E0121 RT3, RT3a · E0122 RT2 · E0123 RT5, CL3 · E0130 DR8, CP, CL10 · E0141 CL5, CL6 · E0161 US, CL11 ·
E0171 CL3, CL9, CL11, SK2, SK3.
