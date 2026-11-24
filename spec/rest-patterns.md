# Rest patterns

Issue #993. A `..` in a pattern stands for the elements or fields the pattern does not name, as Rust's `..` does. This
page is the behavior the compiler implements and the tests that witness it. `tests/rest_patterns/` holds the conformance
programs. Decision 36 in `tests/README.md` records the choices; the grammar is `spec/grammar.md`, rule 7.1.

Every rule below is witnessed by the tests named in it. A rule without a witness is not a rule.

**Where `..` goes.** In a tuple pattern `(...)` and in a tuple-variant pattern `Path(...)`, `..` is one element of the
list and may stand anywhere in it. In a struct pattern `Path { ... }` (a struct, or a struct-like enum variant), `..` is
the last item, after every named field (RP3). `..` is not an expression and is not a pattern on its own: `let .. = v`
is E0001.

**RP1 Meaning.** `..` stands for the elements (or fields) of the matched type that no other part of the pattern names,
in declaration order: `(first, .., last)` on a four-element tuple is `(first, _, _, last)`. The part it stands for is
a wildcard: it binds nothing, and it neither moves nor copies what it ignores. The rest of the pattern follows the
same rules as the same pattern with wildcards written out (`spec/memory-rules.md`, MV8).
Tests: `rest_patterns/tuple_rest.bd`, `rest_patterns/struct_rest.bd` (run).

**RP2 Moves and drops.** A part a `..` stands for stays where it was. Matching an owned place moves only the parts the
pattern binds, so the ignored parts remain in the place and are dropped with it when it goes out of scope (DR7). A
temporary scrutinee is moved into a compiler-made local of the statement, so its ignored parts are dropped at the end of
that statement (DR5), the same as `_`. Tests: `rest_patterns/rest_drops.bd` (run, drop order printed).

**RP3 Position.** A `..` appears at most once in a pattern (E0010). In a struct pattern it is last: a field after it,
or a `..` before a field, is E0012. A trailing separator after the `..` is allowed, as after any field.

**RP4 Ignoring nothing.** A `..` that stands for no element or field is E0011: `(a, b, ..)` on a pair, `(..)` on the
unit type `()`, and `Pair { left, right, .. }` on a struct with exactly those two fields. The check uses the
type of the scrutinee, so it is made where the type is known. A `..` is never E0011 when it stands for one or more
parts, even if the pattern could be written without it.

**RP5 Exhaustiveness.** A `..` does not change what a `match` covers: the pattern is checked as if its wildcards were
written, so `Shape.Rect { w, .. }` covers the `Rect` variant and a `match` that omits `Circle` is still E0006.
Tests: `rest_patterns/rest_exhaustive.bd` (run) and `rest_patterns/rest_not_exhaustive.bd` (E0006).

**RP6 Patterns that bind nothing.** `Pair { .. }`, `Token.Pair(..)`, and `(..)` are accepted when they stand for at
least one part (RP4). They bind nothing, and the linter does not report them (`spec/lint.md`).

**RP7 Scope.** The `..` of a slice or array pattern is not in this page: it is decided in `spec/array-patterns.md` (AP1-AP12,
issue #486), which also gives the named rest `..name` of a slice or array. A struct literal's `..base`
(`spec/struct-update.md`) is a separate feature that shares the token; the parser tells them apart by where they occur.

**Omitted fields.** A struct pattern without a `..` may leave fields out: the omitted fields are not bound and stay where
they are, as they do in an owned scrutinee (RP2). This is the existing behavior, and the `match` tests rely on it:
`m05_ownership/match_guard_temporary_fields.bd` and `match_temporary_fields_drop.bd` match `Pair { a }` on a two-field
struct. The author decided that requiring a `..` there is not wanted: omitting fields stays legal, and no diagnostic
is added for it.

## Diagnostics

| Code | When |
| --- | --- |
| E0010 | A second `..` in one pattern (RP3) |
| E0011 | A `..` that stands for no element or field (RP4) |
| E0012 | In a struct pattern, a `..` before a field or a field after a `..` (RP3) |

A tuple pattern of the wrong length, with or without `..`, is still E0003 (type mismatch): the `..` count does not
change which type the pattern has.
