# Lint reference

`burdin lint` checks a program for code that compiles but is probably not what you meant: names that break the
conventions, code that can never run, values that are cloned or copied for no reason. It reads the same program
`burdin build` compiles, so it only runs on a package that has no errors, and it never changes the program unless you
pass `--fix`.

Every lint has a name (`unused_variables`), a code (`L0007`), and a default level. This page has one section per lint:
what it means, how to fix it, and a checked example. `burdin lint --list` prints the table, and `burdin lint --explain
<lint>` prints the section of one lint by name or by code. The behavior is specified in [`spec/lint.md`](../spec/lint.md).

```
burdin lint <file.bd | package directory> [-A <lint>]... [-W <lint>]... [-D <lint>]... [--deny-warnings]
            [--fix [--dry-run]] [--comptime-steps <n>] [diagnostics]
burdin lint --list
burdin lint --explain <lint>
```

The exit status is 0 when no lint is reported as an error, 1 when one is (or when the package has compile errors), and 2
for a command line problem.

## Levels

Each lint is at one of three levels. `allow` does not run it, `warn` (the default of every lint except `missing_docs`) reports it as a warning,
and `deny` reports it as an error, which makes `burdin lint` exit with status 1. The levels come from three places,
and a later one wins over an earlier one:

1. the default, which is `warn` (`allow` for `missing_docs`);
2. the `[lint]` table of `burdin.toml`, one `name = "level"` line per lint;
3. the command line: `-A <lint>`, `-W <lint>`, `-D <lint>` (or `--allow`, `--warn`, `--deny`), applied left to right.

`all` names every lint, so `-A all -W unused_variables` runs only that one. `warnings` names the lints that are at `warn`
at that point, and `--deny-warnings` is the same as `-D warnings`: every lint that warns becomes an error, which is what a
CI job usually wants. Both the name and the code are accepted: `-D L0007` and `-D unused_variables` are the same. An
unknown name is a command line error.

```toml
[package]
name = "app"
version = "0.1.0"

[lint]
unused_imports = "deny"
dead_code = "allow"
```

A level in the manifest that does not exist, or a lint name that is misspelled, is an error that names the manifest
line.

## Suppressing one finding

A comment `// lint: allow(<lint>, <lint>...)` on its own line silences the named lints for the next item, statement,
or `match` arm, everything inside it included: above a function it covers the whole function, above a statement just that
statement. A comment at the end of a line covers that line. `// lint: allow-file(<lint>)` anywhere in a file covers the
whole file. The name `all` stands for every lint.

<!-- lint-example: none -->
```burdin
// lint: allow(non_snake_case)
fn computeTotal(x: i32): i32 {
    x + 1
}

fn main() {
    println("{}", computeTotal(1))
}
```
<!-- /lint-example -->

A name that is not a lint is reported as `L0000` so a typo cannot hide a warning.

## Fixing automatically

`burdin lint --fix` applies the fixes that cannot change what the program does: removing an unused import, prefixing an
unused variable with `_`, removing a `mut` that is never needed, replacing `x.clone()` by `x` when `x` is not used again.
A finding with a fix says so in its output. The command prints the change as a unified diff, checks that the changed
package still compiles, and only then writes the files, all of them or none. `--fix --dry-run` prints the diff and
writes nothing. Findings without a safe fix (renames, rewrites that change meaning) are reported and left alone.

## L0000: unknown_lint

A suppression comment names a lint that does not exist. It is always reported and cannot be turned off, because the
comment would otherwise silently do nothing.

**How to fix:** correct the name (`burdin lint --list` prints them all) or remove the comment.

<!-- lint-example: L0000 -->
```burdin
// lint: allow(no_such_lint) //~ L0000
fn main() {
    println("hello")
}
```
<!-- /lint-example -->

## L0001: non_snake_case

A function, variable, parameter, struct field, or module name is not `snake_case`.

**How to fix:** rename it to the name in the note, and every place that uses it. The lint has no automatic fix because a
rename touches every use. To keep a name on purpose (a function exported to C, say), write
`// lint: allow(non_snake_case)` above it.

<!-- lint-example: L0001 -->
```burdin
fn addOne(x: i32): i32 { //~ L0001
    x + 1
}

fn main() {
    println("{}", addOne(1))
}
```
<!-- /lint-example -->

## L0002: non_pascal_case

A struct, enum, enum variant, interface, type alias, derive, or type parameter name is not `PascalCase`.

**How to fix:** rename it to the name in the note and update its uses, or write `// lint: allow(non_pascal_case)` above
the declaration.

<!-- lint-example: L0002 -->
```burdin
struct my_point { //~ L0002
    x: i32
}

fn main() {
    let p = my_point { x: 1 }
    println("{}", p.x)
}
```
<!-- /lint-example -->

## L0003: non_screaming_case

A constant, or a const generic parameter, is not `SCREAMING_SNAKE_CASE`.

**How to fix:** rename it to the name in the note and update its uses, or write `// lint: allow(non_screaming_case)`
above it.

<!-- lint-example: L0003 -->
```burdin
const maxSize: i32 = 8 //~ L0003

fn main() {
    println("{}", maxSize)
}
```
<!-- /lint-example -->

## L0004: else_on_local_enum

A `match` on an enum declared in the same package ends in an `else` arm that stands in for variants nobody listed. When
the enum gains a variant later, that variant silently takes the `else` path and the compiler cannot tell you which
`match` expressions to revisit. Enums from the standard library and from dependencies are not reported: their authors
promise what they add.

**How to fix:** replace `else` with the variants it hides (the note lists them), so a new variant is a compile error
at every `match`. If the catch-all is the intended behavior, write `// lint: allow(else_on_local_enum)` above the arm.

<!-- lint-example: L0004 -->
```burdin
enum Shape {
    Circle(i32)
    Square(i32)
    Triangle
}

fn radius(s: Shape): i32 {
    match s {
        Shape.Circle(r) => r
        else => 0 //~ L0004
    }
}

fn main() {
    println("{}", radius(Shape.Square(1)))
}
```
<!-- /lint-example -->

## L0005: eager_chain

Three or more eager `List` methods (`map`, `filter`, `fold`, `any`, `all`, `find`) are chained, so every step builds a
new list that the next step reads once and throws away.

**How to fix:** start the chain with `.iter()` and finish it with `.to_list()` (or a `fold`, `count`, `sum`), so the
steps run lazily over the items without the intermediate lists. The closures of the lazy form take the items
themselves, where the eager methods pass views, so the closure bodies may need a small change. The lint has no
automatic fix for that reason.

<!-- lint-example: L0005 -->
```burdin
fn main() {
    let xs: List<i32> = [1, 2, 3].iter().to_list()
    let ys = xs.map(|x| *x + 1).filter(|x| *x > 1).map(|x| *x * 2) //~ L0005
    println("{}", ys.len())
}
```
<!-- /lint-example -->

## L0006: unused_imports

A name brought in with `use` is never mentioned in the file.

**How to fix:** remove the name from the `use`, or the whole `use` when it was the only one. `--fix` does this. A `pub use`
re-exports its names, so it is never reported.

<!-- lint-example: L0006 target=. -->
```burdin
// --- burdin.toml ---
[package]
name = "app"
version = "0.1.0"
// --- src/util.bd ---
fn add(a: i32, b: i32): i32 { a + b }
fn sub(a: i32, b: i32): i32 { a - b }
// --- src/main.bd ---
use util.{add, sub} //~ L0006

fn main() {
    println("{}", add(1, 2))
}
```
<!-- /lint-example -->

## L0007: unused_variables

A local variable (from `let`, a `for` loop, a `match` arm, or a closure parameter) is never read.

**How to fix:** delete it, or prefix its name with `_` to say it is unused on purpose; a name that starts with `_` is never
reported. `--fix` adds the underscore. Assigning to a variable does not count as reading it.

<!-- lint-example: L0007 -->
```burdin
fn main() {
    let unused = 5 //~ L0007
    println("hello")
}
```
<!-- /lint-example -->

## L0008: unused_parameters

A function parameter is never read in the body.

**How to fix:** remove the parameter and update the callers, or rename it with a leading `_` (`_unused_p`), which keeps the
signature the same. Callers that pass it by name must use the new name. The lint has no automatic fix because a
parameter is part of the function's interface. Parameters of interface methods and of `extern` functions are not
reported, because their signature is fixed elsewhere.

<!-- lint-example: L0008 -->
```burdin
fn area(w: i32, h: i32, unused_p: i32): i32 { //~ L0008
    w * h
}

fn main() {
    println("{}", area(2, 3, 0))
}
```
<!-- /lint-example -->

## L0009: unreachable_code

A statement follows one that never completes: a `return`, `break`, or `continue`, a call to `panic`, `unreachable`, `todo`,
`unimplemented`, `process.exit` or to a function declared `: !`, a `loop` without a `break`, or an `if` or `match` whose
every branch does one of those. The statement can never run.

**How to fix:** delete the unreachable statement, or the statement before it if that one was the mistake.

<!-- lint-example: L0009 -->
```burdin
fn main() {
    println("start")
    return
    println("end") //~ L0009
}
```
<!-- /lint-example -->

## L0010: dead_code

A `priv` function, method, struct, enum, constant, type alias, interface, or extension method is never used by anything
else in the package. Public items are never reported, since another package may use them. `main` and names that start
with `_` are exempt, and a function that only calls itself is still unused.

**How to fix:** delete the item, or write `// lint: allow(dead_code)` above it if it is kept on purpose.

<!-- lint-example: L0010 -->
```burdin
priv fn never_called(x: i32): i32 { //~ L0010
    x
}

fn main() {
    println("hello")
}
```
<!-- /lint-example -->

## L0011: redundant_clone

`x.clone()` where `x` is not used again, so the clone only pays for a copy that moving would have avoided. Values of
types with a user `Drop` are not reported, because dropping the original is observable.

**How to fix:** pass or return `x` itself. `--fix` replaces `x.clone()` by `x`.

<!-- lint-example: L0011 -->
```burdin
fn consume(own xs: List<i32>): usize {
    xs.len()
}

fn main() {
    let xs: List<i32> = [1, 2, 3].iter().to_list()
    println("{}", consume(xs.clone())) //~ L0011
}
```
<!-- /lint-example -->

## L0012: large_copy_in_loop

A `Copy` value of 64 bytes or more is passed by value to a function inside a loop, so every iteration copies it.

**How to fix:** change the parameter to a borrow (drop `own`), or call the function once outside the loop.

<!-- lint-example: L0012 -->
```burdin
struct Big derives (Copy, Clone) {
    a: i64
    b: i64
    c: i64
    d: i64
    e: i64
    f: i64
    g: i64
    h: i64
}

fn first(own b: Big): i64 {
    b.a
}

fn main() {
    let big = Big { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8 }
    let mut total: i64 = 0
    for _k in 0..3 {
        total = total + first(big) //~ L0012
    }
    println("{}", total)
}
```
<!-- /lint-example -->

## L0013: unused_mut

A variable is declared `mut` but is never assigned, borrowed mutably, or passed where a `mut` argument is expected.

**How to fix:** remove `mut`. `--fix` does this. A variable that a closure writes, or that a mutable slice is taken from,
counts as mutated.

<!-- lint-example: L0013 -->
```burdin
fn main() {
    let mut counter = 5 //~ L0013
    println("{}", counter)
}
```
<!-- /lint-example -->

## L0014: borrow_could_own

A `priv` function takes a parameter by borrow and then clones it, while every caller in the package passes a temporary or a
local it never uses again. If the parameter were `own`, the callers would hand the value over and the function would use
it without the copy.

**How to fix:** declare the parameter `own` and use it directly instead of cloning. The lint only looks at `priv` functions,
since a public function's callers are not all known.

<!-- lint-example: L0014 -->
```burdin
priv fn total(items: List<i32>): usize { //~ L0014
    let copy = items.clone()
    copy.len()
}

fn main() {
    println("{}", total([1, 2].iter().to_list()))
}
```
<!-- /lint-example -->

## L0015: loop_invariant_alloc

A value is allocated inside a loop from literals and constants only, so every iteration builds the same string, list, or
box again.

**How to fix:** build it once before the loop.

<!-- lint-example: L0015 -->
```burdin
fn main() {
    let mut n: usize = 0
    for _k in 0..3 {
        let fixed = String.from("abc") //~ L0015
        n = n + fixed.len()
    }
    println("{}", n)
}
```
<!-- /lint-example -->

## L0016: needless_to_list

`to_list()` builds a list that is iterated right away by a `for` loop or by `.iter()`. The list is never needed, only its
items.

**How to fix:** iterate the source directly and drop the `to_list()`.

<!-- lint-example: L0016 -->
```burdin
fn main() {
    let xs: List<i32> = [1, 2, 3].iter().to_list()
    for v in xs.iter().to_list() { //~ L0016
        println("{}", v)
    }
}
```
<!-- /lint-example -->

## L0017: missing_docs

A public item has no `///` comment. The items are the ones a reader of the documentation looks for: the public functions,
structs, enums, interfaces, type aliases, constants, derives, `extern` functions, and extension methods of a module, the
methods of an inherent `impl` block, and the methods of a public interface. `priv` items, the members of a `priv` type,
generated code, and `main` are never reported, and neither are fields, variants, and the methods that implement an
interface (the interface's own comment describes them). A blank line or an ordinary `//` comment between the comment
and the item detaches it. This lint is `allow` by default, so it runs only when it is turned on with `-W missing_docs`,
`-D missing_docs`, or the `[lint]` table. `burdin doc --coverage` counts the same items.

**How to fix:** write a `///` comment on the lines directly above the item, or make the item `priv` if it is not part of the
package's interface.

<!-- lint-example: L0017 -->
```burdin
fn area(width: i32, height: i32): i32 { //~ L0017
    width * height
}

/// Prints the greeting.
fn main() {
    println("hello")
}
```
<!-- /lint-example -->
