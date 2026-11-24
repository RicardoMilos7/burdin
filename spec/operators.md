# Operator overloading (milestone 19)

A user type takes part in the operators by implementing ordinary interfaces declared in the standard library. The
built-in operators never change: primitive numbers, `bool`, `char`, `str`, `String`, tuples, arrays, and a type that
derives `Eq` or `Ord` behave exactly as they did in stage 0 (issue #155). The rules below are normative;
`tests/m19_operators/` is their executable form.

## OP1: What can be overloaded

These operators resolve to an interface method for a user type:

| Operator | Interface | Method |
| --- | --- | --- |
| `a + b` | `Add` | `add` |
| `a - b` | `Sub` | `sub` |
| `a * b` | `Mul` | `mul` |
| `a / b` | `Div` | `div` |
| `a % b` | `Rem` | `rem` |
| `a & b` | `BitAnd` | `bitand` |
| `a \| b` | `BitOr` | `bitor` |
| `a ^ b` | `BitXor` | `bitxor` |
| `-a` | `Neg` | `neg` |
| `!a` | `Not` | `not` |
| `a == b` | `PartialEq` | `eq` |
| `a != b` | `PartialEq` | `ne` |
| `a < b` | `PartialOrd` | `lt` |
| `a <= b` | `PartialOrd` | `le` |
| `a > b` | `PartialOrd` | `gt` |
| `a >= b` | `PartialOrd` | `ge` |
| `a[i]` | `Index<T>` | `index` |
| `a[i] = v` | `IndexMut<T>` | `index_mut` |

These are not overloadable and keep their stage 0 meaning: `&&`, `||`, `=` (the design document says so), `as`, `?`,
unary `&`/`&mut`, and `<<`/`>>` (shifts remain integer-only). Compound assignment (`+=`, `-=`, `*=`, `/=`, `%=`,
`&=`, `|=`, `^=`, `<<=`, `>>=`) is part of the language (issue #277, `spec/compound-assignment.md`). On a built-in type it
is the built-in operator. On a user type, `a op= b` means `a = a op b` through the operator's interface, so it needs no
interface of its own (CA5); the shifts are not overloaded and stay integer-only.

## OP2: The interface set

`std` declares the interfaces in `std/src/ops.bd`; the prelude makes their names visible in every module. They are
ordinary interfaces, not built-in bounds: a program implements them with an ordinary impl block, and they can be used
as bounds (`T: Add`) and named in `where` clauses like any other interface. Every operand interface is same-type: both
operands have the receiver's type, so there are no mixed-type operators for a user type (`V + i32` is E0003; implement
`Add` and convert first). The one built-in mixed pair is a `String` with a `str` (OP14).

```burdin
interface Add { fn add(self, other: Self): Self }
interface Sub { fn sub(self, other: Self): Self }
interface Mul { fn mul(self, other: Self): Self }
interface Div { fn div(self, other: Self): Self }
interface Rem { fn rem(self, other: Self): Self }
interface BitAnd { fn bitand(self, other: Self): Self }
interface BitOr { fn bitor(self, other: Self): Self }
interface BitXor { fn bitxor(self, other: Self): Self }
interface Neg { fn neg(self): Self }
interface Not { fn not(self): Self }

interface PartialEq {
    fn eq(self, other: Self): bool
    fn ne(self, other: Self): bool { !self.eq(other) }
}

interface PartialOrd {
    fn lt(self, other: Self): bool
    fn le(self, other: Self): bool { !self.gt(other) }
    fn gt(self, other: Self): bool { other.lt(self) }
    fn ge(self, other: Self): bool { !self.lt(other) }
}

interface Index<T> { fn index(self, i: usize): &T }
interface IndexMut<T> { fn index_mut(mut self, i: usize): &mut T }
```

- The arithmetic, bitwise, unary, and comparison methods take a **read-only** receiver and a read-only second
  operand. The operator borrows both operands and returns a new owned value; neither operand is moved, and both stay
  usable after the expression. `Neg`/`Not` borrow their operand the same way.
- `PartialEq` and `PartialOrd` are independent: a type may implement either alone. `lt` must be a strict order, and
  the defaults define `le` as `!self.gt(other)`, `gt` as `other.lt(self)`, and `ge` as `!self.lt(other)`. An
  implementation may override any default. `ne` defaults to `!self.eq(other)`.
- `Index<T>` and `IndexMut<T>` take the **element** type as their parameter; the index type is always `usize`. The
  methods return references (`&T` and `&mut T`), which is what makes `x[i]` a place. `IndexMut` does not require
  `Index`.
- A type implements each operator interface at most once. The method table is keyed by the type and method name, so a
  second impl of the same interface for the same type (or an inherent method with the same name) is E0007.
- The operator interfaces are the standard library's: a module that declares its own interface named `Add` shadows
  the prelude name for ordinary code, but `+` still resolves against `std`'s interface. Declaring a type with an
  operator interface and calling the method directly (`a.add(b)`) is allowed; the operator is sugar for that call.

## OP3: Built-in first

For `a op b` the checker first applies the stage 0 rule to the common operand type `T`:

1. arithmetic (`+ - * / %`) is built in for `Int` and `Float`;
2. bitwise (`& | ^`) is built in for `Int` and `Bool`;
3. `==`/`!=` is built in when `T` has stage 0 equality (`has_eq`: primitives, `str`/`String`, `Option`/`Result`/
   `Range`/tuples of such types, and a struct or enum that derives `Eq` and whose fields do);
4. the ordering operators are built in for `Int`, `Float`, `Bool`, `Char`, and when `T` has stage 0 ordering
   (`has_ord`: primitives, `str`/`String`, tuples/`Range`, and a struct or enum that derives `Ord` and whose fields do).

A type parameter obeys its bounds: `T: Eq` uses the built-in structural comparison, and `T: PartialEq` uses the
interface; when both are present the built-in wins. A derive installs the interface (`spec/derives.md`), so a
concrete type never has both: deriving `Eq` or `Ord` and writing `PartialEq`/`PartialOrd` is E0007.

Only when the built-in rule does not apply does the checker try the interface, in this order:

- a struct or enum that implements the interface: the impl's method is called statically;
- a type parameter whose bounds provide the interface: a symbolic call is emitted and monomorphization resolves it;
- otherwise the operator is rejected. A struct, enum, or type parameter gets **E0141** naming the missing interface
  (`it does not implement Add`, or `its bounds do not provide Add`); any other type keeps the stage 0 **E0003**
  (`+ cannot be applied to List<i32>`). A struct or enum that derives `Eq`/`Ord` but fails the derive requirement
  keeps the stage 0 **E0003** when it has no interface implementation, because the failure names a field, not a
  missing interface (E0130 already reports the derive itself). If such a type also implements `PartialEq` or
  `PartialOrd`, that implementation is used and the derive still fails the build with E0130.

The built-in rule is chosen from the common operand type after the ordinary literal inference, and the two operands
must have the same type before an interface is tried (`compatible`), so `V + i32` is E0003. `Result` of the operator
is the interface method's return type; every operand interface returns `Self`, and the comparison interfaces return
`bool`.

## OP4: Unary operators

`-a` first folds a literal (`-128` is one literal) and is built in for signed integers and floats. Otherwise a user
type may implement `Neg`; `-a` calls `a.neg()`. An unsigned integer still rejects `-` (E0003), and a missing `Neg`
is E0141 for a struct/enum/type parameter and E0003 for any other type.

`!a` is built in for `Bool` and `Int`. Otherwise a user type may implement `Not`; `!a` calls `a.not()`. A missing
`Not` is reported like a missing `Neg`.

## OP5: Comparison operators

`==` lowers to `eq`, `!=` to `ne`, `<` to `lt`, `<=` to `le`, `>` to `gt`, and `>=` to `ge`. The comparison
operators are always same-type and always return `bool`. A derived comparison is the type's own implementation of the
interface (`spec/derives.md`, issue #918), so a type that derives `Eq` or `Ord` cannot also implement `PartialEq`
or `PartialOrd` (E0007); remove the derive to write the comparison by hand. A type that derives `Ord` also gets
`==`/`!=` (`Ord` implies `Eq`, as the bound does); a type that derives only `Eq` gets `==`/`!=` but no ordering.
Each operator is decided on its own.

`PartialOrd` and `PartialEq` need not agree; the compiler does not check that `lt` and `eq` are consistent. That is
the implementation's contract.

## OP6: Operators in generic code

A generic function may use an operator through a bound:

```burdin
fn combine<T: Add>(a: T, b: T): T { a + b }
fn first<T: Index<i32>>(v: T): i32 { v[0] }
```

The body is checked against the bounds, emitting a symbolic call; each instantiation re-checks the body with the
concrete type and emits a direct call to that type's implementation. A bound the body needs and does not have is
E0141 at the operator, exactly as for a method call. An interface default body sees `Self: Interface`; it may use an
operator only when the interface's `requires` clauses provide the needed bound.

## OP7: Literals

Literal typing is unchanged (tests/README.md decision 12): an integer literal takes its type from context, else from
the other operand of a binary operator when that operand is a numeric built-in type, else `i32` (`f64` for a float
literal). A literal never adopts a user type, so `v + 1` with `v: V` does not call `V.add`; the literal is `i32` and
the operands do not have the same type, which is E0003. To write a value of `V`, construct one (`v + V.one()`).

## OP8: Evaluation and ownership

`a op b` evaluates `a` and then `b`, once each, exactly like the call `a.method(b)`. The interface methods borrow
their operands, so an operator expression does not move either one; a temporary operand drops at the end of the
statement like any other temporary. The result is an ordinary owned value: it drops at the end of its scope or
statement, and it can itself be the operand of another operator.

## OP9: Reading `x[i]`

For an array, a slice, or a `List<T>`, `x[i]` keeps the stage 0 behavior: a bounds-checked place of type `T`. For any
other type:

- `x` must be a struct or enum implementing `Index<T>` (or a type parameter with that bound). `i` is checked as
  `usize`, and the expression lowers to `*x.index(i)`, with type `T`.
- The result is a place, so it can be read, matched, borrowed, or used as a field base. Reading it copies a `Copy`
  element. Moving a non-`Copy` element out (`let s = x[i]`) is **E0101** (`cannot move out of borrowed content`),
  because the reference is borrowed: use `.clone()` or a method that returns an owned value.
- The returned reference follows the ordinary return rules (RT1-RT5): its origin is `x` (exactly one source), and a
  view stored in a local must not outlive `x`.
- A type with no `Index` implementation is **E0141** for a struct/enum/type parameter and **E0003** for any other
  type, with the stage 0 message.
- A range index (`x[a..b]`) is not part of `Index`: the slice rules stay built-in, and a user type with a range index
  is rejected as before.

The borrow checker tracks the reference: `let p = &x[i]; x[j] = v` conflicts while `p` is used (E0111), and the
reference counts as a loan on `x` for the exclusivity rule (E0110).

## OP10: Writing `x[i] = v`

`x[i] = v` on a user type requires `IndexMut<T>` and lowers to `*x.index_mut(i) = v`:

- the receiver must be a mutable place (MU3): `let x` without `mut`, or a read-only parameter, is **E0113**
  (`index_mut changes its receiver, but x is not mutable`);
- `i` is `usize`; the old element is dropped before the new value is stored, exactly as for a built-in index (DR4);
- a missing `IndexMut` is **E0141** (`cannot assign through an index of X: it does not implement IndexMut`);
- the write lowers through `index_mut` wherever the place is written: the left side of `=`, `&mut x[i]`, a `mut x[i]`
  argument, and an index inside a written place (`x[i].field = v`, `swap(mut x[i], mut x[j])`). A plain read, a shared
  borrow, and the index operand itself use `index`. This is how `index_mut` differs from an ordinary `mut self`
  method: the compiler selects it from the place's write position.
- `x[i]` itself is the read-only view of `index`, so calling a `mut self` method directly on it is E0113; take
  `&mut x[i]` first. `&mut x[i]` gives the writable view that `index_mut` returns.

Indices of a user container are one place for exclusivity (tests/README.md decision 16): `swap(mut x[i], mut x[j])`
is E0110, even when `i` and `j` are different constants, because the checker cannot prove them distinct. Two
different containers (`swap(mut x[i], mut y[j])`) do not overlap.

## OP11: The standard library surface

`spec/stdlib.md` lists the interfaces with their signatures. The interfaces live in `std.ops`; the prelude makes the
names visible in every module, so a user program needs no `use`. A package that replaces the embedded `std` without
these declarations keeps only the built-in operators.

## OP12: Built-in behavior unchanged

Everything stage 0 did with an operator is unchanged: integer and float arithmetic, division and remainder
truncation, overflow panics in debug builds and wrapping in release builds, division by zero, shifts, `bool` logic,
`str`/`String` comparison, tuple and derived-struct equality and ordering, array/`List` indexing and slicing, and the
`assert_eq` builtin. `tests/m19_operators/builtin_*.bd` are the regression tests for these rules. `&&` and `||`
still short-circuit and accept only `bool`.

## OP13: Diagnostics

| Code | Cause |
| --- | --- |
| E0141 | A struct, enum, or type parameter does not implement the operator interface (`Add`, `Neg`, `PartialEq`, `PartialOrd`, `Index`, `IndexMut`, ...) the operator needs |
| E0113 | `IndexMut` is needed through an immutable place, or an index expression is the receiver of a `mut self` method through the read-only `index` view |
| E0101 | A non-`Copy` element is moved out of `x[i]` |
| E0003 | Operands or the index have the wrong type; the type has no built-in operator and no overload; a failed `Eq`/`Ord` derive keeps the stage 0 code |

Missing interface methods and wrong impl signatures keep E0140 and E0003, like every other interface.

## OP14: `String` and `str`

A `String` and a `str` may meet in two places. This is the only mixed-type pair the built-in rules accept; every other
pair of different types keeps the same-type rule (OP2, OP7).

- `String + str` is built in and has type `String`. The result is a new string holding the text of the left operand
  followed by the text of the right one. Both operands are borrowed (OP8), so neither is moved and both stay usable.
  It is the standard library's `__concat` (`std/src/text.bd`), so it is an ordinary call for the checks and the code
  generator.
- The comparisons `==`, `!=`, `<`, `<=`, `>`, `>=` accept a `String` on either side of a `str` (`s == "x"`,
  `"x" == s`, `s < t.as_str()`). The `String` is viewed as a `str` (`as_str`), and the comparison is the `str`
  comparison: byte order for the ordering operators, byte equality for `==`.

Not accepted, and still E0003: `str + String` and `str + str` (`"a" + "b"`, `string_plus_unsupported`), `String + String`
(write `s + t.as_str()`), and `String + i32` or a comparison with a number. The result of `String + str` is an owned
`String`; it does not change the left operand. Compound assignment has no mixed form: `s += "x"` is not part of this
rule and stays E0003 (the same-type rule).

Known gap (RF3): a borrow made in a statement stays live until that statement ends, so `s = s + "x"` is E0111, the
same as `s = join(s.as_str(), "x")` for an ordinary function. Write `let t = s + "x"` and then `s = t`. Fixing this is a
change to the borrow checker's liveness, not to this rule.

The rule is decided here rather than by a general `Add<Rhs>` interface (issue #936 offered both). A general right-hand
type would let one type implement `Add` for several operands and would change the once-per-type rule of OP2 and the
literal rule of OP7, so it is not part of this rule and would need its own decision.

## Tests

| Rule | Tests (in `tests/m19_operators/`) |
| --- | --- |
| OP1-OP2, OP8 | `vector_add_index`, `arith_ops`, `bitwise_ops`, `unary_ops`, `borrows_operands` |
| OP3, OP6, OP7 | `generic_operator_body`, `missing_add`, `missing_neg`, `missing_eq`, `missing_ord`, `literal_against_user_type`, `mixed_operands` |
| OP4 | `unary_ops`, `missing_neg`, `missing_not` |
| OP5 | `comparison_ops`, `derived_wins`, `missing_eq`, `missing_ord` |
| OP9-OP10 | `vector_add_index`, `generic_container`, `nested_index`, `index_view`, `index_closure`, `index_assignment_drops_old`, `index_mut_immutable`, `index_mut_readonly_param`, `index_mut_receiver`, `missing_index`, `missing_index_mut`, `move_out_of_index`, `exclusivity_index`, `mut_index_argument` |
| OP10 (write mode) | `index_write_place`, `nested_index` |
| OP11 | `vector_add_index` (the std interfaces are visible without `use`) |
| OP12 | `builtin_operators`, `builtin_overflow_aborts`, `builtin_division_by_zero_aborts`, `builtin_shift_overflow_aborts`, `builtin_index_errors`, `and_or_not_overloadable` |
| OP14 | `string_str_operators` (run), `string_str_mixed_compile_fail` (E0003 for the pairs still rejected) |
| OP1 (compound assignment) | `compound_assignment` (a user type without `Add`, E0141); the rest is in `tests/compound_assign/` (`spec/compound-assignment.md`, CA5) |
