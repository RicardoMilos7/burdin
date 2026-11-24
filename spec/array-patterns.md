# Array and slice patterns

Issue #486. A pattern `[p, ...]` destructures a fixed-size array `[T; n]` or a slice view `[T]`. A `..` (or `..name`)
inside the brackets stands for the elements the pattern does not name, as the rest patterns of tuples do
(`spec/rest-patterns.md`). This page is the behavior the compiler implements and the tests that witness it.
`tests/array_patterns/` holds the conformance programs. Decision 39 in `tests/README.md` records the choices; the
grammar is `spec/grammar.md`, rule 7.2.

Every rule below is witnessed by the tests named in it. A rule without a witness is not a rule.

**AP1 Syntax.** `[` begins a pattern wherever a pattern is allowed (`let`, `match` arms, `if let`, `while let`, `for`,
closure parameters). Its elements are patterns, separated by `,` (a trailing `,` is allowed), and at most one of them
is a rest element: `..`, or `..name` to bind the rest. `[]` is a valid pattern. A rest element outside an array or slice
pattern is E0016 (AP6). Tests: `array_patterns/array_basic.bd`, `array_patterns/array_nested.bd`.

**AP2 Types.** An array pattern matches a value of type `[T; n]` or of type `[T]`, and no other type (E0003). It does not
look through a reference: a scrutinee of type `&[T]` or `&[T; n]` is E0003, as a tuple pattern on `&(A, B)` is. A
slice view is matched through its own type `[T]`, the same way a `str` is matched by its type.
Tests: `array_patterns/array_not_array` (E0003), `array_patterns/slice_match.bd` (run).

**AP3 Fixed length.** On an array, a pattern without a rest element names exactly `n` elements, and a pattern with one
names at most `n` (E0003 when it names more, or fewer without a rest). Tests: `array_patterns/array_count_mismatch`
(E0003), `array_patterns/array_rest.bd` (run).

**AP4 Rest on an array.** The rest element stands for the `n - k` elements it does not name, where `k` is the number of
named elements. A rest that stands for no element is E0011 (RP4 in `spec/rest-patterns.md`): `[a, b, ..]` on `[T; 2]`.
A second rest element in one pattern is E0010 (RP3). Tests: `array_patterns/array_rest_ignores_nothing` (E0011),
`array_patterns/array_rest_repeated` (E0010), `array_patterns/array_rest.bd` (run).

**AP5 Rest on a slice.** A slice pattern without a rest element matches exactly its length. With a rest element it matches
every length of at least the number of named elements, and a rest that matches zero elements is allowed (no E0011):
`[..]` matches every slice. Tests: `array_patterns/slice_match.bd` (run), `slice_rest_zero.bd` (run).

**AP6 Named rest.** `..name` binds the rest. On a slice it binds a view `[T]` of the elements the pattern does not name,
in order: a view is second-class (`spec/memory-rules.md`, SC2), so it may not be stored or returned. On an array it binds
an array `[T; n - k]` of those elements, moved or copied out (AP9). A named rest in a tuple or tuple-variant pattern is
E0016: tuples have no rest binding. Tests: `array_patterns/slice_rest_view.bd` (run), `array_patterns/array_rest.bd`
(run), `array_patterns/named_rest_in_tuple` (E0016).

**AP7 Refutability.** An array pattern is irrefutable when each element pattern is, because its length is known. A slice
pattern is refutable unless it is `[..]` or `[..name]` with irrefutable elements, since a slice may have any length. The
rule that decides refutability is the one for every pattern (`spec/memory-rules.md`, exhaustiveness): a refutable `let`
needs an `else` block (E0006 without one) and a refutable `for` pattern is rejected. Tests:
`array_patterns/slice_let_else.bd` (run), `slice_refutable_let` (E0006), `slice_refutable_for` (E0006).

**AP8 Exhaustiveness.** A `match` over a slice is exhaustive when every length is covered. The lengths the arms name are
checked one by one, and every length beyond the longest named one is covered by the rest arms of that length or more.
So `[]` with `[x, ..]` is exhaustive, while `[x]` with `[x, y, ..]` leaves `[]` out (E0006). Tests:
`array_patterns/slice_exhaustive.bd` (run), `slice_not_exhaustive` (E0006).

**AP9 Moves.** A pattern on an owned array place moves only the elements it binds, and each such element is a partial move of
that place by its constant index (`spec/memory-rules.md`, MV8 and the constant-index rule). The elements a rest binds move
as a group, each one a partial move. The rest of the array stays where it was. A pattern on a slice takes its elements
the way any pattern on a borrowed scrutinee does: as views (`spec/memory-rules.md`, the view bindings of a `match` on a
reference), so a `String` element is read through its view and never moved out of the slice. A rest of a slice is a view
of the middle. A binding that would move out of a slice, which a slice that owns its storage would need, is E0101 as an
index into a slice is. Tests: `array_patterns/array_moves.bd` (run), `slice_view_elements.bd` (run).

**AP10 Drops.** The elements an array pattern does not move are dropped with the array: at the end of the owner's scope
for a place, and at the end of the statement for a temporary, as the parts of a temporary tuple are (DR5, DR7). Tests:
`array_patterns/array_drops.bd` (run, drop order printed), `array_temporary_drop.bd` (run).

**AP11 Runtime.** An array pattern runs no test: its length is known. A slice pattern tests the length first, and an
element read from the end of a slice is taken from the end of the view. Tests: `array_patterns/slice_match.bd` (run).

**AP12 Scope.** Slice and array patterns are the only patterns that destructure a sequence. Ranges, strings, and an
array scrutinee of a comptime value are covered by the existing rules: a comptime array can be matched, and a slice is
never a comptime value (`spec/comptime.md`). Tests: `array_patterns/array_comptime.bd` (run).

## Diagnostics

| Code | When |
| --- | --- |
| E0016 | A named rest `..name` in a pattern that is not an array or slice pattern (AP6) |

The other errors in this page are existing codes: E0003 for a pattern of the wrong type or length, E0006 for a refutable
`let` or `match` that is not exhaustive, E0010 and E0011 for rest elements (RP3, RP4), and E0101 for a move out of a slice.
