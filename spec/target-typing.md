# Target-typed initializers

Status: implemented (issue #998, 2026-11-24). The forms are in `grammar.md` (`TargetExpr`), and their tests are in
`tests/target_typing/`. Before this, a leading `.` was a syntax error (E0001). The stage placement is in
`stage1-subset.md`, under "When wanted".

A value's type can come from the place it is used, so a constructor or a struct literal does not repeat a type the
position already says:

```
let list: List<String> = .new()
let names: Map<String, i32> = .with_capacity(16)

fn a(p: Point) { ... }
a(.{ x: 10, y: 20 })          // Point { x: 10, y: 20 }

fn pick(c: Color) { ... }
pick(.Red)                    // Color.Red
```

The spelling `T.name(args)`, `T.Variant`, and `T { ... }` stays valid and means the same thing. The forms are an
addition.

## Rules

**TT1 (the forms).** Three forms start an expression: `.name`, `.name(args)`, and `.{ fields }`. Each takes the
expected type `T` of its position, and means `T.name`, `T.name(args)`, and `T { fields }` respectively. `.name` is
whatever `T.name` is: a function value, a variant, or a call's callee.

**TT2 (where the expected type comes from).** The expected type is the one the checker already passes down to the
expression, and the forms add no new propagation. It comes from:

- the type of a `let` with an annotation;
- a parameter: an argument's expected type is its parameter's type;
- the declared return type of a function, for a `return` value and for the tail value of its body;
- a field of a struct literal;
- the payload of `Some`, `Ok`, or `Err` when the expected type gives it;
- the target of an assignment;
- the arms of an `if` or `match`, the value of a block, and a `break` value, each when that construct has an expected
  type;
- an element of an array whose expected type is known.

**TT3 (no later use, no receiver, no operand).** A form with no expected type is E0003. A later use never gives it a
type: `let x = .new()` is E0003 even when `x` is passed to a `List<String>` parameter afterwards. A form is never a
method receiver, so `.new().push(1)` is E0003. The other operand of a binary operator is not an expected type, so
`c == .Red` is E0003; write `c == Color.Red`. (Letting the left operand give the right one a type would be a new rule;
this file does not add it.)

**TT4 (generic types and arguments).** The expected type is taken after alias resolution (`type-aliases.md`), so an
alias names its target. If the expected type is a type parameter that the position does not know, the form is E0003.
That covers a type parameter of the enclosing item (`fn f<T>(): T { .new() }`) and a type parameter of a generic callee
that the call has not fixed. A type that names a parameter is known: `fn f<T>(): List<T> { .new() }` is `List<T>.new()`.
For a generic callee, the expected type of a target-typed argument is its parameter type with the call's explicit type
arguments substituted; the call's other arguments do not count. So `keep<i32, List<String>>(1, .new())` is valid, and
`keep(1, .new())` is E0003 when the second parameter is a type parameter. A generic type's arguments come from the
expected type: `.new()` against `List<String>` is `List<String>.new()`.

**TT5 (what the expected type must be).** After TT4 the expected type must be a named type: a struct, an enum, or a
built-in named type such as `List<T>`, `Map<K, V>`, `String`, `Option<T>`, or `i32`. These are E0003: a reference, a
view (`str`, `[T]`), an array, a tuple, an interface type (`any I`, `impl I`), a closure type, and a type parameter. The
form does not look through a reference: a `&Point` parameter does not take `.{ x: 1, y: 2 }`.

**TT6 (what a name means).** `.name` resolves as `T.name` resolves in the same position: an associated function, a
method value (`memory-rules.md`, CL11), a variant, including an associated extension function (`extensions.md`, EX).
`.Some(x)`, `.None`, `.Ok(v)`, and `.Err(e)` work when the expected type is `Option<T>` or `Result<T, E>`, because
`Option.Some(x)`, `Option.None`, `Result.Ok(v)`, and `Result.Err(e)` already are their spellings. The bare prelude
names `Some`, `Ok`, and `Err` do not change. A name that `T` does not have is E0002, as `T.name` reports it.

**TT7 (the struct form).** `.{ fields }` is `T { fields }`: the same field list, the same shorthand, and the same checks,
reporting the same codes. `T` must be a struct that the program declares. For any other expected type (an enum, a
built-in type, a primitive) the form is E0003. Tuples and arrays have no form here: `(a, b)` and `[a, b]` keep their
spelling.

**TT8 (diagnostics).** The forms add no code. The codes are the registered ones:

| Situation | Code |
| --- | --- |
| No expected type (TT3), or an expected type excluded by TT4 or TT5 | E0003 |
| `.{ ... }` where the expected type is not a struct (TT7) | E0003 |
| A name the expected type does not have (TT6) | E0002, as `T.name` reports it |
| A field error inside `.{ ... }` (TT7) | the code `T { ... }` reports for it |
| A `.` at the start of an expression that is not followed by an identifier or `{` | E0001 |

Each message names the form and says what to write, for example: "`.new` needs an expected type; write the type, as in
`List<String>.new()`". Every error in a file is reported (`tests/README.md`, the working rules), not only the first.

**TT9 (lowering and the checks).** Sema lowers a form to the typed-tree node that `T.name(args)` or `T { ... }` lowers
to. The memory checks (`burdin_check`), the comptime engine (`burdin_comptime`), and code generation see no new node and
no new rule. The expected type is local to one function body: it comes from annotations and signatures, never from a
later use (TT3), so the checker stays local.

**TT10 (syntax).** A form is recognized only where an expression begins. So `x.{name}` (`reflection.md`, RFL5) is still
the computed field access after an expression, `a.b` and `t.0` are unchanged, `..` is still the range operator, and a
line that starts with `.` continues the expression before it wherever the grammar already continues an expression onto
the next line. The syntax is `TargetExpr` in `grammar.md`.

**TT11 (stage placement).** The forms are not a stage 1 milestone. The inclusion rule in `stage1-subset.md` admits a
feature that the core language as designed needs, that the standard library or the author's projects need to be usable,
or that closes a stage 0 gap. None of the three applies: every form has a spelling that already works (`List<String>.new()`,
`Point { ... }`, `Color.Red`), and the design document does not include the forms. They are listed under "When wanted" in
`stage1-subset.md`, and the folder `tests/target_typing/` is outside the milestone list, so it is not one of the
milestone folders CI checks for completeness. CI runs it with the other non-milestone folders.

**TT12 (registry).** The forms add no registry code, so `tests/README.md` gets a note pointing here and no new row.
A new code would need a compile-fail test in the same change (`tests/check_tests.py`) and a checked example in
`docs/error-codes.md` (`tests/check_error_docs.py`), and neither can exist before the compiler reports it.

## Answers to the open questions in issue #998

1. **Is `.{ ... }` needed, or is `.name` enough?** It is needed. It is the only form that drops a struct's name in an
   argument, `a(.{ x: 10, y: 20 })`, which is the motivating case. It is a second spelling of a struct literal, and TT7
   gives it the same checks.
2. **Does `.{ ... }` cover tuples and arrays?** No (TT7). Neither has a name to drop, and `[a, b]` already takes its
   element type from its expected type.
3. **Does the expected type flow through `if`, `match`, and generic calls?** Through `if`, `match`, blocks, and `break`
   values, where the checker already passes an expected type (TT2). Through a generic call it is the parameter type
   with the explicit type arguments substituted; the other arguments do not count (TT4).
4. **Is a generic parameter as the expected type an error?** Yes, E0003 (TT4, TT5).
5. **Stage placement.** Not stage 1, and not built early. It is a decided item under "When wanted" (TT11).

`.Some(x)` and `.Ok(x)` are part of this rule, through TT6, because `Option.Some` and `Result.Ok` already are valid
spellings.

## Tests

The conformance folder is `tests/target_typing/`:

- `forms.bd` (run): each expected-type source at run time: a `let` annotation with `.new()`, `.with_capacity(16)`, and a
  type alias; an argument (`.{ ... }` and `.Green`); a return value and a body's tail; a struct field; `.Some`, `.None`,
  and `.Err` against `Option` and `Result`; the arms of an `if`; an assignment; a shorthand field; a generic argument
  with explicit type arguments (`keep<i32, List<String>>(1, .new())`); and a nested `.{ ... }` field.
- `no_expected_type.bd` (E0003): `let a = .new()`, a receiver (`.new().len()`), the operand of `==`, `.{ ... }` and `.Red`
  with no expected type; `takes_list(a)` after the first does not type it.
- `expected_type_kinds.bd` (E0003): a type parameter that is not known (`.new()` in a generic body that is checked
  generically), `.{ ... }` on an `i32` and on an enum, a reference parameter, and a `str` expected type.
- `names_and_fields.bd` (E0002 and E0003): a variant the type does not have, a function the type does not have, a
  `.{ ... }` with an unknown field (E0002) and a missing one (E0003), as `Point { ... }` reports them.
- `bare_dot.bd` (E0001): a `.` not followed by a name or `{`.
