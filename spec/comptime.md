# Comptime

Milestone 20 adds compile-time execution: `comptime` expressions, blocks, and parameters, `comptime if` and
`comptime for`, `const` values computed by the same engine, `comptime assert`, and a step budget. This page is the
exact behavior; the tests are `tests/m20_comptime/`. Rule IDs are CT1-CT14.

## CT1. The engine: the typed tree, interpreted (decision, issue #158)

The design (`Burdin Language Design.md`, "Compiler architecture") says the comptime interpreter and the backends both
read a MIR, so comptime behavior matches compiled behavior. Stage 1 decides **not** to introduce a MIR for this
milestone. The comptime engine is an interpreter over the checked typed tree (`burdin_tir`), in its own crate,
`burdin_comptime`, which depends only on `burdin_tir` and `burdin_diag`. It runs after the memory checks and before
code generation:

```
burdin_syntax -> burdin_sema -> burdin_check -> burdin_comptime -> burdin_codegen
```

The reasons, recorded so the decision can be revisited:

- Code generation reads the typed tree directly and is the larger half of the compiler. A MIR would mean lowering
  every construct twice (tree to MIR, MIR to LLVM) and moving drop elaboration and the parameter checks, which are
  already correct over the tree. Issue #159 (introduce `burdin_mir`) is closed as not chosen; a MIR stays on the
  "when wanted" list.
- The interpreter shares the memory rules instead of re-deriving them: it only runs programs the memory checks
  accepted, it reads the tree's own ownership facts (parameter conventions, `own` parameters, owned and view
  pattern bindings, `Drop` implementations), and it moves values out of their places exactly where the tree moves
  them. A comptime value cannot be used after it was moved, because the checker already rejected such a program.
- Comptime behaves like compiled code of the same profile: integer overflow panics in a debug build and wraps in a
  `--release` build (the arithmetic of `spec/stage0-subset.md`), division by zero and out-of-range shifts always
  panic, and float-to-integer casts saturate.

The interpreter evaluates every expression and statement form of the typed tree that a pure program can reach:
literals, operators and casts, locals, blocks and `let`/`let ... else`, assignment, `if`, `while`, `loop`, every `for`
form, `match` with guards, `break`/`continue`/`return`, calls of functions, methods, interface methods, closures,
and `any` vtables, structs, enums, tuples, arrays, slices, `Option`/`Result`, ranges, `String`, `List`, `Map`,
`Heap`, references, the iteration protocol, formatting, `clone`, `hash`, and the derived comparisons. What it does not
run is the impure surface of CT9.

## CT2. Comptime expressions and blocks

`comptime E` evaluates the expression `E` while the program is compiled and puts the resulting value in its place.
`comptime { ... }` is the same rule applied to a block expression:

```
const TABLE: [u32; 4] = comptime { let mut t = [0; 4]; for i in 0..4 { t[i] = (i * i) as u32 }; t }
let x = comptime fib(20)
```

`E` is type-checked like any expression with the expected type of its position, and its memory rules are checked
like any other code (CT1). Inside `E`, a local of the enclosing function is visible only when it is compile-time
known: a `comptime` parameter (CT4) or a `comptime for` binding (CT6). Using any other local, `self`, or capturing
one in a closure is E0220. Locals declared inside `E` are ordinary locals.

A `comptime` expression is evaluated once per place it appears in the generated code (once per specialization of a
function with `comptime` parameters, CT4, and once per unrolled `comptime for` body, CT6), not each time the
surrounding code runs.

## CT3. Compile-time values

The value of a comptime expression, a `const`, a `comptime` parameter, and a `comptime for` item is copied into the
program. Its type must be one of the **materializable** types:

- `()`, `bool`, `char`, every integer and float type, `str`, and `String`;
- tuples and arrays of materializable types;
- structs and enums (including generic instances and `error enum`s) whose fields are all materializable;
- `Option<T>`, `Result<T, E>`, and `Range<T>`/`RangeInclusive<T>` of materializable types.

`List`, `Map`, `Heap`, `any`, closures, references, slices, `CPtr`, `CString`, and iterators are not materializable:
comptime code may create and use them, but they cannot be the value that crosses into the program (E0221, at the
comptime expression, the `const` type, or the parameter). A `str` value is copied into a static string literal, so a
comptime `str` is static wherever it is used (`spec/memory-rules.md` RT2); a `String` value becomes a fresh
`String.from(literal)` each time the materialized expression runs.

## CT4. `comptime` parameters

A named parameter written `comptime name: Type` takes a compile-time value:

```
fn power(x: i64, comptime n: u32): i64 {
    comptime if n == 0 { 1 } else { x * power(x, n - 1) }
}
```

- The parameter's type must be materializable (E0221); it is passed like a read-only parameter, so it cannot be
  written `mut`, `own`, or have a `self` form (E0220).
- Each argument for a `comptime` parameter is checked as `comptime argument` (CT2): it may use constants, literals,
  and the caller's own compile-time-known locals, but not its runtime locals (E0220). A default value is allowed and
  follows `spec/arguments.md`.
- Inside the body, the parameter is compile-time known: it may be used in comptime expressions, `comptime if`
  conditions, `comptime for` iterables, and `comptime assert`. It may also be read at run time like an ordinary
  read-only parameter.
- A call is **specialized**: the compiler makes one copy of the function for each distinct list of comptime argument
  values, binds each comptime parameter to its value at the start of that copy, removes it from the copy's
  signature, and evaluates the copy's comptime expressions with those values. A copy is generated only for values a
  call actually passes. More than 1024 specializations of one function is the budget error E0225.
- Only free functions, inherent methods, and extension methods may have comptime parameters. An interface method,
  an implementation of an interface method, an `extern` declaration, a closure, and a test cannot (E0220), and a
  function with comptime parameters cannot be used as a function value (E0220).

## CT5. `comptime if`

`comptime if cond { A } else { B }` evaluates `cond` at compile time (CT2's visibility rule applies to it) and keeps
only the selected branch in the generated code. In an `else if` chain every condition is a compile-time condition.
Both branches are type-checked and memory-checked like an ordinary `if`, so the two branches must have compatible
types; the branch that is not selected is not evaluated, so a comptime expression, `comptime assert`, or panic in it
has no effect. The kept branch has the scoping and drop rules of the branch it was: its block is still a block.

## CT6. `comptime for`

`comptime for pattern in iterable { body }` evaluates `iterable` at compile time and repeats the body once per
item, in order, in the generated code:

- The iterable is a range of integers, an array, or a `List` of materializable items (E0003 for any other type,
  E0221 for an item that is not materializable).
- Each copy of the body is a block that starts with `let pattern = item`: the item is an owned value, the pattern
  must be irrefutable, and its bindings are compile-time known inside that copy (so `comptime if i == 0` and
  `comptime assert(i < 4)` work), and are ordinary immutable locals at run time. A `mut` binding is E0220.
- `break` and `continue` cannot leave a `comptime for` body, because the unrolled body is no loop (E0220); a loop
  written inside the body may use them for itself.
- The body is checked like a loop body, so moving an outer value in it is E0104, as in any loop.

## CT7. `const` values

A `const` initializer is any expression of a materializable type (CT3); it is evaluated by the comptime engine,
exactly as if written `comptime { initializer }`, with no locals of any function visible. It may call functions,
declare locals, loop, and build values. Every constant in the program is evaluated once, whether or not it is used, in
declaration order, evaluating a constant it refers to first; a constant that reaches itself through calls is E0009.
The value is then materialized (CT3) at every use, so `const` still costs nothing beyond the value itself.

Constants of a struct, enum, tuple, array, `Option`, or `Result` type, and constants that refer to other constants,
are values like any other. Default arguments (`spec/arguments.md`, AG2) keep their narrower rule: literals, other
constants, operators, and casts.

## CT8. `comptime assert`

`comptime assert(condition)` and `comptime assert(condition, "message")` evaluate `condition` (a `bool`, under CT2's
visibility rule) at compile time. When it is false, compilation fails with E0223 at the assertion, showing the
message when one is given. The message must be a string literal (E0220). The form is an expression of type `()` in a
body, and also an item at module scope, where it is checked once when the program is compiled:

```
comptime assert(BUFFER_SIZE % 64 == 0, "the buffer must be a whole number of cache lines")
```

## CT9. Purity

Comptime is pure and deterministic. Evaluating any of these is E0224, reported at the comptime expression with a
note at the operation:

- output: `print`, `println`, `eprint`, `eprintln`;
- files, the environment, and processes: `read_to_string`, `write_file`, `env.args`, `process.exit`, `process.run`;
- C: calling an `extern` function, `CPtr` values and dereferences, `CString`, and `CStr.to_string`.

The check is dynamic: a function that prints can be called at compile time as long as the path taken does not print.
Nothing else observable depends on the machine, the time, or the order of compilation, so a comptime result depends
only on the source, the target, and the profile (`--release` changes overflow behavior, CT1). `Map` iteration order
at compile time is insertion order, which is one of the orders `spec/stdlib.md` allows. There is no comptime file
access yet; `embed` belongs to milestone 21.

## CT10. Panics

A panic during compile-time evaluation (an explicit `panic`, `unreachable`, a failed `assert`/`assert_eq`, `unwrap`
or `expect` on the empty case, an index out of bounds, an overflow in a debug build, a division by zero, a bad slice
range, or a failed `List.remove`) stops the evaluation and is E0222 at the comptime expression (or the `const`, or
the call that specialized the function), with the panic message and a note at the operation that panicked. Nothing
of the failed evaluation reaches the program.

## CT11. The step budget

Every evaluation has a budget of steps: each evaluated expression, statement, loop iteration, and call is one step,
and every element a `String`, `List`, or `Map` allocates is one more, so the budget bounds memory as well as time.
The default is 10,000,000 steps per comptime expression, per `const`, and per specialization's comptime arguments;
`burdin build`, `run`, and `test` accept `--comptime-steps N` to change it. Exceeding the budget is E0225 at the
comptime expression. The same code covers a call depth beyond 512 nested comptime calls and more than 1024
specializations of one function (CT4).

## CT12. Drops at compile time

The interpreter drops values where compiled code does (`spec/memory-rules.md` DR1-DR10): owned locals at the end of
their scope in reverse declaration order, the old value of an assigned place, `own` parameters at the end of the
callee, the temporaries of a statement at its end, and the elements removed by `clear`. A user `Drop`
implementation runs at those points. A moved value is not dropped. Because comptime has no output and a value with a
`Drop` implementation cannot hold a reference (E0120), a drop is observable at compile time only through a panic or
the steps it consumes.

## CT13. What is generated

After evaluation the program contains no comptime construct: a comptime expression is replaced by its materialized
value, a `comptime if` by its selected branch, a `comptime for` by its unrolled copies, a `comptime assert` by `()`,
and each call to a function with comptime parameters by a call to its specialization. A function, method, or `const`
initializer that only comptime code calls is not emitted. Code generation never sees a comptime construct.

## CT14. Caching

Because comptime is pure (CT9) and bounded (CT11), its results are a function of the source files, the target, and
the profile, so a build cache may key them on those inputs. The compiler does not cache comptime results across
builds yet; a later stage that adds cached object files can rely on this rule.

## Diagnostics

| Code | Meaning |
| --- | --- |
| E0220 | Invalid comptime construct: a runtime local, `self`, or capture used in a compile-time context; `comptime` on a parameter that cannot take it; a comptime-parameter function used as a value; `break`/`continue` leaving a `comptime for`; a `mut` `comptime for` binding; a `comptime assert` message that is not a string literal |
| E0221 | A compile-time value whose type cannot be materialized in the program |
| E0222 | Compile-time evaluation panicked |
| E0223 | A `comptime assert` failed |
| E0224 | An impure operation (I/O, environment, process, C) during compile-time evaluation |
| E0225 | Compile-time evaluation exceeded its step budget, its call depth, or the specialization limit |
| E0009 | A constant that depends on itself through calls (existing code) |

## Tests

`tests/m20_comptime/` covers CT2-CT13: a compile-time lookup table (`const_table`), a CRC table (`crc_table`),
`comptime` blocks and expressions (`comptime_block`, `comptime_expr`), parameters and specialization
(`comptime_param`, `comptime_param_str`, `comptime_param_errors`), `comptime if` platform and recursion branches
(`comptime_if_platform`, `comptime_if_recursion`, `comptime_if_dead_branch`), `comptime for` unrolling
(`comptime_for_unroll`, `comptime_for_errors`), constants of every materializable kind (`const_values`,
`const_cycle_through_call`), `comptime assert` (`assert_passes`, `assert_fails`, `assert_message_literal`), the
budget (`budget_infinite_loop`, `budget_recursion`), a comptime panic (`comptime_panic`, `comptime_overflow`),
purity (`purity_print`, `purity_file`), materialization (`not_materializable`), runtime locals (`runtime_local`),
memory rules at compile time (`comptime_moves`, `comptime_drop`), and the interpreter's coverage of the language
(`interp_*`).
