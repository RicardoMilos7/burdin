# Struct update syntax

Issue #994. `Pair { left: 1, ..other }` builds a struct from the fields written in the literal and takes every other field
from a base value after `..`, as Rust's struct update syntax does. This page is the behavior the compiler implements and
the tests that witness it. `tests/struct_update/` holds the conformance programs. Decision 37 in `tests/README.md` records
the choices; the grammar is `spec/grammar.md`, rules 6 and 8.12.

Every rule below is witnessed by the tests named in it. A rule without a witness is not a rule.

**SU1 Syntax.** A struct literal may end with `..` followed by an expression, the base: `Config { port: 9090, ..p }`.
The base is the last item. It takes no separator after it, so `Config { ..p, }` is an error, and no field may follow it
(E0014). A second base is E0013. A literal may be only a base, `Config { ..p }`, which copies every field. The base is
an ordinary expression, so `..Config.default()` and `..(make())` work. Patterns have no base form; `..` in a pattern is
`spec/rest-patterns.md`. Tests: `struct_update/update_basic.bd` (run), `struct_update/update_syntax_errors.bd`
(E0013, E0014).

**SU2 Meaning.** The literal's fields are the ones it names, and every other field is read from the base. The base must
have the literal's type exactly: the same struct, with the same type arguments (E0003). For a generic struct the type
arguments come from the named fields and the expected type, as they do without a base, and the base is checked against
that type afterwards, so the base does not decide a type argument on its own. `Self { x: 1, ..Self.default() }` is the
common case. Tests: `struct_update/update_basic.bd`, `struct_update/update_generic.bd` (run),
`struct_update/update_type_errors.bd` (E0003).

**SU3 Evaluation order.** The named field values are evaluated in source order, then the base, then the fields taken
from it. A named field's value is therefore evaluated before anything the base expression does. Test:
`struct_update/update_order.bd` (run, output printed by the operands).

**SU4 Moves from a place base.** When the base is a place (an owned local, a field of one, or a field of an owned
`self` or `own` parameter), each field taken from it is moved out if it is not `Copy` (a partial move, MV5, and a later
use of that field or of the whole base is E0100), and copied if it is `Copy`. A field the literal names is not read
from the base, so it is not moved and stays usable there. Tests: `struct_update/update_partial_move.bd` (run),
`struct_update/update_moved_field.bd` (E0100).

**SU5 Moves from a temporary base.** When the base is not a place (a call, a literal, a block), it is evaluated into a
compiler-made local of the literal. The fields taken from it are moved out as in SU4. The fields the literal names
are dropped when the literal has been evaluated, because the local goes out of scope then (DR7). The timing is that of a
block's locals, not of the enclosing statement's temporaries. Test: `struct_update/update_temporary.bd` (run, with a
`Drop` type that prints).

**SU6 Drop types.** A struct that implements `Drop` may be a base. A field taken from it must be `Copy`; a non-`Copy`
field taken from it is E0103, as a move out of a `Drop` type always is (MV5, DR8). A base of such a type that takes only
`Copy` fields is read, and its `Drop` runs when the base's owner goes out of scope (a place) or when the literal has
been evaluated (a temporary, SU5). Tests: `struct_update/update_drop.bd` (run), `struct_update/update_drop_move.bd` (E0103).

**SU7 Struct-like enum variants.** `..base` is not allowed on an enum variant with named fields (`Shape.Rect { w: 2, ..r }`):
the fields a variant has depend on which variant the base holds, so a base cannot supply them. That is E0015. Struct
update is for structs only.

**SU8 Privacy.** Each field taken from a base is a field of the struct, so a `priv` field of a struct in another package
is E0150 as it is for a named field. A `pub` struct whose fields are all readable can be updated from any package.
Test: `struct_update/private_package/app/src/main.bd` (compile-fail, with the `lib` package it depends on).

**SU9 Missing fields.** A literal with a base needs no missing-field check: every field is either named or taken from
the base. A struct without a base keeps the E0003 missing-field error.

**SU10 Scope.** Struct update applies to structs. Enum variants are E0015 (SU7). Patterns do not take a base. A struct
literal with a base in a `if`, `while`, `match`, or `for` head follows the struct-literal rule of `spec/grammar.md`, rule
8.1: it must be parenthesized there.

## Diagnostics

| Code | When |
| --- | --- |
| E0013 | A second `..base` in one struct literal (SU1) |
| E0014 | `..base` followed by a field, or by a separator (SU1) |
| E0015 | `..base` on a struct-like enum variant (SU7) |

A base of the wrong type or of the wrong generic instance is E0003. A moved field is E0100, a `Drop` move is E0103, and
a private field is E0150; those are the existing codes for the same events.
