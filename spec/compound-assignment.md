# Compound assignment

Issue #277. `place op= value` (`+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`) updates a place with a
binary operator. This page is the behavior the compiler implements and the tests that witness it. `tests/compound_assign/`
holds the conformance programs. The stage 0 and stage 1 pages listed compound assignment as "when wanted", and
`spec/operators.md` OP1 and OP13 said it was not part of stage 1; those rows now point here.

Every rule below is witnessed by the tests named in it. A rule without a witness is not a rule.

**Scope.** This page covers the built-in types: the integers, the floats, and `bool`. Overloading compound assignment
on a user type is an open decision (CA5). The issue asks for it, but `spec/operators.md` OP1 says compound assignment is
not overloadable, so the two disagree and the compiler does not choose one yet.

## CA1. Forms and placement

The ten operators are one token each: `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, and `>>=` (the lexer takes
the longest match, so `<<=` is not `<` `<=`). `place op= value` is an expression at the level of `=`
(`spec/grammar.md`, rule 6): right associative, statement-like, and its type is `()`. A line break before the operator
ends the statement, as it does before `=`. Inside generic arguments the token `>>=` splits like `>>`, so the first `>`
closes the arguments and `>=` is left for the rest.

Witnesses: `tests/compound_assign/all_operators.bd` (every operator on a `let`, a field, and an element),
`tests/compound_assign/newline_before_operator.bd` (E0001 at the operator on its own line).

## CA2. Meaning

`place op= value` reads the place, applies the binary operator `op` to the old value and `value`, and stores the result
in the place. The rules of the operator are the rules of the binary form (`spec/operators.md`, OP12): an integer
overflow panics in a debug build and wraps in a release build, a division or remainder by zero panics, and a shift by
the bit width or more panics in a debug build.

The order of evaluation matches `=`. The value is evaluated first, then the place, so `v[i] += f()` calls `f` before the
index is computed. The place's address is computed once, so `v[next()] += 1` calls `next` exactly once.

Witnesses: `tests/compound_assign/all_operators.bd` (results), `tests/compound_assign/evaluation_order.bd` (the order and
the single evaluation of the place), `tests/compound_assign/overflow_aborts.bd` and `divide_by_zero_aborts.bd` (the
panics), `tests/compound_assign/shift_aborts.bd` (the shift panic), `tests/compound_assign/shifts.bd` (shift amounts of
several integer types and an arithmetic `>>=` on a signed value).

## CA3. Operand types

The place and the value are checked as the binary operator checks its operands:

- `+=`, `-=`, `*=`, `/=`, `%=`: the place is numeric, and the value has the same type.
- `&=`, `|=`, `^=`: the place is an integer or `bool`, and the value has the same type.
- `<<=`, `>>=`: the place is an integer, and the value is an integer of any integer type (as `<<` and `>>` accept).

A mismatch is E0003 at the value, or at the place for a place of the wrong kind. A literal on the right takes the type of
the place (`x += 1` with `x: u8` is a `u8` addition). A raw C pointer is not numeric, so `p += n` is E0003: the binary
form `p = p + n` is the pointer offset, and this page does not extend the compound forms to it.

Witnesses: `tests/compound_assign/type_errors.bd` (E0003 for each kind of mismatch) and
`tests/compound_assign/raw_pointer_type.bd` (a raw C pointer, E0003).

## CA4. Places

The left side is a place, under the same rules as `=` (`spec/memory-rules.md`): a variable, or a field or element of
one, that is mutable. A place that is not mutable, or an expression that is not a place, is E0005 at the place. A
compound assignment writes its place, so it is subject to the same exclusivity rules as `=`: a live borrow of the place
conflicts with it (E0111).

Witnesses: `tests/compound_assign/immutable_place.bd` (E0005 for a `let` binding and a parameter that is not `mut`),
`tests/compound_assign/borrow_conflict.bd` (E0111).

## CA5. Types without built-in arithmetic

A place of a user type (a struct, an enum, or a type parameter) is E0001 for now. Whether `a += b` means `a = a + b` through
the `Add` interface, or a new `AddAssign`-style interface is needed, is the open decision in issue #277, and
`spec/operators.md` OP1 still says compound assignment is not overloadable. Until that decision is made, the compiler
reports the construct as not part of the language (E0001, as `spec/operators.md` OP13 recorded it before this change).

A place of a built-in type that has no such operator (`String`, `List`, a tuple, an array) is E0003, as `+` on those
types is.

Witnesses: `tests/m19_operators/compound_assignment.bd` (a struct, E0001), `tests/compound_assign/generic_place.bd`
(a type parameter, E0001), `tests/compound_assign/type_errors.bd` (`String` and a tuple, E0003).

## CA6. Comptime

A compound assignment inside `comptime` runs in the compile-time interpreter with the same rules as the run-time form:
the same operators, the same overflow and division panics, and the same order (`spec/comptime.md`, CT1-CT2).

Witness: `tests/compound_assign/comptime.bd`.

## Tests

| Rule | Tests (in `tests/compound_assign/`) |
| --- | --- |
| CA1 | `all_operators`, `newline_before_operator` |
| CA2 | `all_operators`, `evaluation_order`, `overflow_aborts`, `divide_by_zero_aborts`, `shift_aborts`, `shifts` |
| CA3 | `type_errors`, `raw_pointer_type` |
| CA4 | `immutable_place`, `borrow_conflict` |
| CA5 | `generic_place`, `../m19_operators/compound_assignment` |
| CA6 | `comptime` |
