# Compound assignment

Issue #277. `place op= value` (`+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, `>>=`) updates a place with a
binary operator. This page is the behavior the compiler implements and the tests that witness it. `tests/compound_assign/`
holds the conformance programs. The stage 0 and stage 1 pages listed compound assignment as "when wanted", and
`spec/operators.md` OP1 and OP13 said it was not part of stage 1; those rows now point here.

Every rule below is witnessed by the tests named in it. A rule without a witness is not a rule.

**Scope.** The built-in types (the integers, the floats, and `bool`) follow CA2 and CA3. A user type, and a type
parameter, follow CA5: their compound assignment is the binary operator's overloaded form, written out as `a = a op b`.

## CA1. Forms and placement

The ten operators are one token each: `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `|=`, `^=`, `<<=`, and `>>=` (the lexer takes
the longest match, so `<<=` is not `<` `<=`). `place op= value` is an expression at the level of `=`
(`spec/grammar.md`, rule 6): right associative, statement-like, and its type is `()`. A line break before the operator
ends the statement, as it does before `=`. Inside generic arguments the token `>>=` splits like `>>`, so the first `>`
closes the arguments and `>=` is left for the rest.

Witnesses: `tests/compound_assign/all_operators.bd` (every operator on a `let`, a field, and an element),
`tests/compound_assign/newline_before_operator.bd` (E0001 at the operator on its own line).

## CA2. Meaning on built-in types

`place op= value` on a built-in type reads the place, applies the binary operator `op` to the old value and `value`, and
stores the result in the place. The rules of the operator are the rules of the binary form (`spec/operators.md`, OP12):
an integer overflow panics in a debug build and wraps in a release build, a division or remainder by zero panics, and a
shift by the bit width or more panics in a debug build.

The order of evaluation matches `=`. The value is evaluated first, then the place, so `v[i] += f()` calls `f` before the
index is computed. The place's address is computed once, so `v[next()] += 1` calls `next` exactly once.

Witnesses: `tests/compound_assign/all_operators.bd` (results), `tests/compound_assign/evaluation_order.bd` (the order and
the single evaluation of the place), `tests/compound_assign/overflow_aborts.bd` and `divide_by_zero_aborts.bd` (the
panics), `tests/compound_assign/shift_aborts.bd` (the shift panic), `tests/compound_assign/shifts.bd` (shift amounts of
several integer types and an arithmetic `>>=` on a signed value), `tests/compound_assign/overflow_wraps_release.bd`
(the release wrap).

## CA3. Operand types on built-in types

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

## CA5. User types and type parameters: `a op= b` is `a = a op b`

A place of a user type (a struct or an enum) or of a type parameter takes the operator from its interface, exactly as
the binary operator does (`spec/operators.md`, OP1-OP3): `a op= b` means `a = a op b`, where `op` is the binary
operator with the interface it names (`Add` for `+=`, `Sub` for `-=`, `Mul` for `*=`, `Div` for `/=`, `Rem` for `%=`,
`BitAnd` for `&=`, `BitOr` for `|=`, `BitXor` for `^=`). No new interface is needed.

- The operator borrows both operands and returns a new value (OP1), so the value is not moved and stays usable. The
  result replaces the place, and the old value is dropped (`spec/memory-rules.md`, DR4).
- A place whose type has no such operator is E0141, as the binary operator reports it; the message names `+=`.
- `<<=` and `>>=` stay integer-only (OP1), so on a user type they are E0003.
- The desugaring is literal, so the place is evaluated twice: once to read it for the operator, and once to write the
  result. An index or a base with side effects runs them twice on a user type. This differs from CA2, where the place is
  evaluated once, and it is a known gap rather than a design choice; removing it needs the compiler to keep the place's
  address, which the desugaring does not do yet.
- A type parameter follows the same rule through its bounds: `T: Add` makes `x += y` valid in a generic body, and a
  type parameter without the bound is E0141.

Witnesses: `tests/compound_assign/overloaded.bd` (run: the operator, `a += a`, the value is not moved, and the result
replaces the place), `tests/compound_assign/overloaded_twice.bd` (run: the place's index runs twice, as above),
`tests/compound_assign/overloaded_errors.bd` (E0005 on an immutable place, E0003 for `<<=`, E0003 for a value of another
type), `tests/m19_operators/compound_assignment.bd` (a struct without `Add`, E0141),
`tests/compound_assign/generic_overloaded.bd` (run: `T: Add`), `tests/compound_assign/generic_no_bound.bd` (E0141).

A place of a built-in type that has no operator (`String`, `List`, a tuple, an array) is E0003, as `+` on those types is
(`tests/compound_assign/type_errors.bd`).

## CA6. Constant evaluation

A compound assignment inside a `const` initializer runs in the compile-time interpreter with the same rules as the run-time form:
the same operators, the same overflow and division panics, and the same order (`spec/const-eval.md`, CE1).

Witness: `tests/compound_assign/const.bd`.

## Tests

| Rule | Tests (in `tests/compound_assign/`) |
| --- | --- |
| CA1 | `all_operators`, `newline_before_operator` |
| CA2 | `all_operators`, `evaluation_order`, `overflow_aborts`, `overflow_wraps_release`, `divide_by_zero_aborts`, `shift_aborts`, `shifts` |
| CA3 | `type_errors`, `raw_pointer_type` |
| CA4 | `immutable_place`, `borrow_conflict` |
| CA5 | `overloaded`, `overloaded_twice`, `overloaded_errors`, `generic_overloaded`, `generic_no_bound`, `../m19_operators/compound_assignment` |
| CA6 | constant evaluation |
