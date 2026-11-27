# Constant evaluation

Milestone 20 added compile-time execution. The `comptime` language surface (expressions, blocks, parameters, `comptime
if`, `comptime for`, `comptime assert`, and `comptime generate`) was removed afterwards: a macro system is planned to
replace it, and none exists yet. What stays is the engine, the evaluation of `const` items, a compile-time condition on
a plain `if`, the unrolled reflection loop, and `embed`. This page is the exact behavior; the tests are
`tests/m20_const/`. Rule IDs are CE1-CE12.

## CE1. The engine: the typed tree, interpreted (decision, issue #158)

The design (`Burdin Language Design.md`, "Compiler architecture") says the compile-time interpreter and the backends
both read a MIR, so compile-time behavior matches compiled behavior. Stage 1 decides **not** to introduce a MIR. The
engine is an interpreter over the checked typed tree (`burdin_tir`), in its own crate, `burdin_interp`, which depends
only on `burdin_tir` and `burdin_diag`. It runs after the memory checks and before code generation:

```
burdin_syntax -> burdin_sema -> burdin_check -> burdin_interp -> burdin_codegen
```

The same interpreter evaluates `const` and `static` initializers (CE2), the compile-time conditions of CE4, `burdin
repl`, and `burdin test --interpret`. The reasons, recorded so the decision can be revisited:

- Code generation reads the typed tree directly and is the larger half of the compiler. A MIR would mean lowering
  every construct twice (tree to MIR, MIR to LLVM) and moving drop elaboration and the parameter checks, which are
  already correct over the tree. Issue #159 (introduce `burdin_mir`) is closed as not chosen; a MIR stays on the
  "when wanted" list.
- The interpreter shares the memory rules instead of re-deriving them: it only runs programs the memory checks
  accepted, it reads the tree's own ownership facts (parameter conventions, `own` parameters, owned and view
  pattern bindings, `Drop` implementations), and it moves values out of their places exactly where the tree moves
  them.
- Evaluation behaves like compiled code of the same profile: integer overflow panics in a debug build and wraps in a
  `--release` build (the arithmetic of `spec/stage0-subset.md`), division by zero and out-of-range shifts always
  panic, and float-to-integer casts saturate.

The interpreter evaluates every expression and statement form of the typed tree that a pure program can reach:
literals, operators and casts, locals, blocks and `let`/`let ... else`, assignment, `if`, `while`, `loop`, every `for`
form, `match` with guards, `break`/`continue`/`return`, calls of functions, methods, interface methods, closures,
and `any` vtables, structs, enums, tuples, arrays, slices, `Option`/`Result`, ranges, `String`, `List`, `Map`,
`Heap`, references, the iteration protocol, formatting, `clone`, `hash`, and the derived comparisons. What it does not
run is the impure surface of CE6.

## CE2. `const` values

A `const` initializer is any expression of a materializable type (CE3); it is evaluated by the engine at compile time,
with no locals of any function visible. It may call functions, declare locals, loop, and build values. Every constant
in the program is evaluated once, whether or not it is used, in declaration order, evaluating a constant it refers to
first; a constant that reaches itself through calls is E0009. The value is then materialized (CE3) at every use, so
`const` still costs nothing beyond the value itself. An array length and a const-generic argument are constants of this
kind (`spec/const-generics.md`). A `static` is the other global: its initializer is evaluated by these same rules, and
its value is stored once before `main` runs (`globals.md`, GL1-GL2).

Constants of a struct, enum, tuple, array, `Option`, or `Result` type, and constants that refer to other constants,
are values like any other. Default arguments (`spec/arguments.md`, AG2) follow these same rules: any pure
expression of a materializable type, evaluated once at compile time and materialized at every call that omits
the argument.

## CE3. Compile-time values

The value of a `const` is copied into the program. Its type must be one of the **materializable** types:

- `()`, `bool`, `char`, every integer and float type, `str`, and `String`;
- tuples and arrays of materializable types;
- structs and enums (including generic instances and `error enum`s) whose fields are all materializable;
- `Option<T>`, `Result<T, E>`, and `Range<T>`/`RangeInclusive<T>` of materializable types.

`List`, `Map`, `Heap`, `any`, closures, references, slices, `CPtr`, `CString`, and iterators are not materializable:
a `const` initializer may create and use them, but they cannot be the value that crosses into the program (E0221, at
the `const` type: "a `const` of type X cannot be evaluated at compile time ..."). A `str` value is copied into a
static string literal, so a constant `str` is static wherever it is used (`spec/memory-rules.md` RT2); a `String`
value becomes a fresh `String.from(literal)` each time the materialized expression runs.

## CE4. Compile-time conditions: conditional compilation with a plain `if`

An `if` (and each condition of an `else if` chain) whose condition is a **compile-time condition** keeps only the
taken branch in the generated code. There is no keyword: the compiler recognizes the condition by its form. A
compile-time condition is an expression built only from

- literals,
- `const` items,
- the unary operators `-` and `!`, the binary arithmetic, comparison, `&&`, and `||` operators, and parentheses,
- enum variant paths (`Os.Linux`), and
- calls of the functions and constants of the `target` module (`target.os()`, `target.arch()`, ...) and of the
  generated `features` module (`features.openssl`; `spec/target.md`, `spec/packages.md`).

```
if target.os() == Os.Windows {
    windows_sleep(ms)
} else if target.os() == Os.Linux {
    linux_sleep(ms)
} else {
    posix_sleep(ms)
}
```

Rules:

- Any other `if` is an ordinary run-time `if`. `if let` is never a compile-time condition. A condition that mentions
  a local, a parameter, `self`, or a call of any other function is not one, and is not an error: the `if` runs at run
  time.
- Both branches are type-checked and memory-checked exactly as for an ordinary `if`, so the branches must have
  compatible types and every platform's branch must compile on every platform. Only code generation drops the branch
  that is not taken: sema marks the condition, the interpreter evaluates it (CE1), and the untaken branch is removed
  before code generation. The branch that is not taken is never evaluated, so a panic in it has no effect.
- The kept branch has the scoping and drop rules of the branch it was: its block is still a block, and an `if`
  without `else` whose condition is false generates `()`.
- Evaluating the condition is pure (CE6) and bounded (CE8), as the evaluation of a `const`.

`target.os()` and `target.arch()` report the platform the program is built for: the host's, or the one `--target`
names (`spec/cross-compilation.md`, XC2). Compile-time evaluation itself still runs on the host.

## CE5. Reflection loops

A plain `for item in Type.fields { ... }` or `for item in Type.variants { ... }` is unrolled by sema while the body is
checked (`spec/reflection.md`, RFL4). It is the only `for` evaluated at compile time; any other `for` is the ordinary
run-time loop. `break` and `continue` cannot leave an unrolled loop (E0220).

## CE6. Purity

Compile-time evaluation is pure and deterministic. Evaluating any of these is E0224, reported at the `const` (or the
compile-time condition) with a note at the operation:

- output: `print`, `println`, `eprint`, `eprintln`, and the `Stdout`/`Stderr` handles' `write` (and so every
  `Write` default built on it) and `flush`;
- input: the `Stdin` handle's `read`, `read_until`, `read_line`, and `read_to_string` (and so every `Read` and
  `BufRead` default built on them), and `is_terminal` on any stream handle;
- files, the environment, and processes: `read_to_string`, `write_file`, `env.args`, `env.get`/`env.get_bytes`,
  `env.current_dir`/`env.current_dir_bytes`, `env.set`/`env.set_bytes`/`env.remove`, `env.set_current_dir` and its
  byte form, `process.exit`, `process.run`, and `Command.spawn`/`Command.status`/`Command.output`, a `Child`'s
  `stdin`/`stdout`/`stderr`, `Child.wait`, and `Child.wait_with_output` (`stdlib.md`, "Child processes");
- the system's clocks, entropy, and sleeping: `Instant.now`, `SystemTime.now` (so `SystemClock`'s methods), `sleep`,
  and `random_seed` (so `Random.from_entropy`); a `Duration`, a `DateTime`, a `ManualClock`, and a seeded `Random`
  are pure and work at compile time;
- C: calling an `extern` function, `CPtr` values and dereferences, `CString`, and `CStr.to_string`;
- parallel iteration: running a `std.parallel` pipeline (`spec/parallel.md`, PR10), whatever its size.

The check is dynamic: a function that prints can be called at compile time as long as the path taken does not print.
Nothing else observable depends on the machine, the time, or the order of compilation, so a result depends only on
the source, the target, and the profile (`--release` changes overflow behavior, CE1). `Map` iteration order at compile
time is insertion order, which is one of the orders `spec/stdlib.md` allows. Reading a file at compile time is `embed`
alone (CE12); CE6 rejects every other file operation.

## CE7. Panics

A panic during compile-time evaluation (an explicit `panic`, `unreachable`, a failed `assert`/`assert_eq`, `unwrap`
or `expect` on the empty case, an index out of bounds, an overflow in a debug build, a division by zero, a bad slice
range, or a failed `List.remove`) stops the evaluation and is E0222 at the `const` (or the compile-time condition),
with the panic message and a note at the operation that panicked. Nothing of the failed evaluation reaches the
program.

## CE8. The step budget

Every evaluation has a budget of steps: each evaluated expression, statement, loop iteration, and call is one step,
and every element a `String`, `List`, or `Map` allocates is one more, so the budget bounds memory as well as time.
The default is 10,000,000 steps per `const` and per compile-time condition; `burdin build`, `run`, and `test` accept
`--const-steps N` to change it. Exceeding the budget is E0225 at the `const`. The same code covers a call depth beyond
512 nested compile-time calls.

## CE9. Drops at compile time

The interpreter drops values where compiled code does (`spec/memory-rules.md` DR1-DR10): owned locals at the end of
their scope in reverse declaration order, the old value of an assigned place, `own` parameters at the end of the
callee, the temporaries of a statement at its end, and the elements removed by `clear`. A user `Drop`
implementation runs at those points. A moved value is not dropped. Because evaluation has no output and a value with a
`Drop` implementation cannot hold a reference (E0120), a drop is observable at compile time only through a panic or
the steps it consumes.

## CE10. What is generated

After evaluation the program contains no compile-time construct: a `const` use is replaced by its materialized value
and a compile-time `if` by its taken branch. A function, method, or `const` initializer that only compile-time
evaluation calls is not emitted. Code generation never sees a compile-time construct.

## CE11. Caching

Because evaluation is pure (CE6) and bounded (CE8), its results are a function of the source files, the target, and
the profile, so a build cache may key them on those inputs. The compiler does not cache evaluation results across
builds; the object cache (`spec/target.md`, TG16) keys on the generated code, which already holds them.

## CE12. `embed`

`embed("path")` is the contents of a file, read by the compiler while it checks the call. It is the one place a
compile-time construct reads a file, and it does so outside the interpreter: the result is a literal in the program,
so the purity rule (CE6) is untouched.

- The path is a string literal, relative to the directory of the source file that writes the call. An absolute path,
  or one with a `..` component, is E0241, and so is a path that is not a literal.
- With the expected type `[u8; N]` (a `let` annotation, a parameter, a constant's type), the result is the file's bytes
  and `N` must be the file's length (E0241).
- With the expected type `[u8]` (a `let` annotation, a parameter, a return type), the result is a static byte view of
  the file: its length is not written, so it needs no length check. It is static data, like a string literal
  (`memory-rules.md`, RT4), so a function may return it and a local may hold it, whatever its parameters. A file that is
  missing or cannot be read is still E0240, and an absolute or `..` path is still E0241. A constant may not have type
  `[u8]` (CE3, E0221): a constant's value is copied into each use, so a view is bound in a function or a `let` instead.
- Anywhere else it is a `str`, and the file must be valid UTF-8 (E0240). A binary file therefore needs `[u8]` or
  `[u8; N]`.
- A file that is missing, a directory, or unreadable is E0240 at the path, naming the file; the build stops without
  a crash.
- The compiler records every file `embed` read as an input of the build: `burdin build --emit-deps <file>` writes them,
  one path per line, in the order they were first read, so a build system can rebuild when one changes.

```
const LICENSE: str = embed("LICENSE.txt")
const HEADER: [u8; 4] = embed("magic.bin")
fn payload(): [u8] {
    embed("payload.bin")
}
```

## Diagnostics

| Code | Meaning |
| --- | --- |
| E0220 | `break` or `continue` leaving an unrolled reflection loop (CE5, RFL4), or a `static` named in a `const` initializer (`globals.md`, GL1) |
| E0221 | A `const` whose type cannot be materialized in the program (CE3) |
| E0222 | Compile-time evaluation panicked (CE7) |
| E0224 | An impure operation (I/O, environment, process, C) during compile-time evaluation (CE6) |
| E0225 | Compile-time evaluation exceeded its step budget or its call depth (CE8) |
| E0240 | `embed` could not read its file, or a `str` file is not valid UTF-8 text (`[u8]` and `[u8; N]` take any bytes) |
| E0241 | An invalid `embed`: a path that is not a literal, is absolute, or has `..`; a `[u8; N]` with the wrong length |
| E0009 | A constant that depends on itself through calls (existing code) |

E0223 (a failed `comptime assert`), E0242, and E0243 (`comptime generate`) are retired with the features they
reported; their numbers are never reused.

## Rule mapping from the removed pages

| Old rule | New rule |
| --- | --- |
| CT1 the engine | CE1 |
| CT2 `comptime` expressions and blocks | removed |
| CT3 compile-time values | CE3 (for `const` only) |
| CT4 `comptime` parameters and specialization | removed |
| CT5 `comptime if` | CE4 (a plain `if` with a compile-time condition) |
| CT6 `comptime for` | CE5 (only the reflection loop, RFL4); other uses removed |
| CT7 `const` values | CE2 |
| CT8 `comptime assert` | removed |
| CT9 purity | CE6 |
| CT10 panics | CE7 |
| CT11 the step budget | CE8 (`--comptime-steps` became `--const-steps`) |
| CT12 drops | CE9 |
| CT13 what is generated | CE10 |
| CT14 caching | CE11 |
| CT15 `embed` | CE12 |
| GN1-GN10 declaration generation | removed |

## Tests

`tests/m20_const/` covers CE1-CE10: a lookup table (`const_table`), a CRC table (`crc_table`), constants of every
materializable kind (`const_values`, `const_cycle_through_call`), compile-time conditions (`const_if_platform`,
`const_if_recursion`, `const_if_dead_branch`), the budget (`budget_infinite_loop`, `budget_recursion`), a panic during
evaluation (`const_panic`, `const_overflow`), purity (`purity_print`, `purity_file`, `purity_env`), materialization
(`not_materializable`), and the interpreter's coverage of the language (`interp_*`). The exact case list is the folder.

`tests/m21_reflect/` covers CE5 and CE12: the unrolled reflection loop (`reflect_loop`), and `embed_basic` (a text
file as `str` and a binary file as `[u8; N]`), `embed_slice` (a `[u8]` view as a local, a return value, and a
parameter, with no length written), `embed_missing` (E0240), `embed_invalid` (E0241), and `embed_slice_invalid`
(E0240, E0241, and E0221 for a constant of type `[u8]`).
