# Error code reference

Every Burdin diagnostic carries a code from the registry in [`tests/README.md`](../tests/README.md#error-codes).
This page explains each code, shows a program that triggers it, and says how to fix it.

The page and its examples are checked by `tests/check_error_docs.py` in CI: the codes must match the registry
exactly, every `burdin` snippet below is a complete program that the check compiles and expects to fail with
exactly the marked diagnostics, and `burdin explain <code>` (which reads this page) must print every entry. The
`//~ E0000` comment at the end of a line marks the line the diagnostic is reported on. An example marked
`mode=test` is compiled with `burdin test`, because a test attribute is checked only in test mode.

Run `burdin explain E0003` to print one entry in the terminal, or `burdin explain` to list the codes.

## E0001: Syntax error

The lexer or the parser could not build a syntax tree from the source: a token, statement, expression, or item
is incomplete or out of place. The message names what was expected or what was not allowed.

<!-- error-example: E0001 -->
```burdin
fn main() {
    let = 5 //~ E0001
}
```
<!-- /error-example -->

**How to fix:** correct the shape of the construct the message names. For a missing name, write it before the `=`;
for an unexpected token, remove it or finish the construct it belongs to.

## E0002: Unknown name

A name does not resolve: no local, parameter, constant, function, type, module, or imported item has that
spelling in the current module and package. The message says which namespace was searched.

<!-- error-example: E0002 -->
```burdin
fn main() {
    println(missing) //~ E0002
}
```
<!-- /error-example -->

**How to fix:** declare the name, import it with a `use` item, or fix the spelling. A `priv` item in another
package is E0150 instead, because the name exists but is not visible; an item that only exists in a later stage is
E9000.

## E0003: Type mismatch

The type of a value is not the type its context requires: a `let` annotation, a parameter, a return type, an
operand, an array element, or a `match` arm. Integer and float literals take their type from context, so a literal
is only mismatched when no annotation can satisfy it.

<!-- error-example: E0003 -->
```burdin
fn main() {
    let n: i32 = "text" //~ E0003
}
```
<!-- /error-example -->

**How to fix:** make the two types agree. Convert explicitly, for example with `to_string`, `parse`, or `as` for a
numeric conversion. The one implicit conversion in the language is `String` to `str` in an argument-like position.

## E0004: Wrong number of arguments

A call passes a different number of arguments than the function, method, or built-in declares. The message gives
both counts.

<!-- error-example: E0004 -->
```burdin
fn add(a: i32, b: i32): i32 {
    a + b
}

fn main() {
    let n = add(1) //~ E0004
}
```
<!-- /error-example -->

**How to fix:** pass exactly one argument for each parameter, or change the function's signature to match the call.

## E0005: Assignment to an immutable binding

The left side of an assignment is not mutable: the binding was declared with plain `let`, or it is a read-only
parameter, or it is a binding made by a pattern from a borrowed value. The same rule covers writing through a
read-only parameter.

<!-- error-example: E0005 -->
```burdin
fn main() {
    let x = 1
    x = 2 //~ E0005
}
```
<!-- /error-example -->

**How to fix:** declare the binding with `let mut`, and take the value as a `mut` parameter when a function must
change its caller's place. Read-only parameters are immutable on purpose.

## E0006: Non-exhaustive `match`

The patterns of a `match` do not cover every value the scrutinee can have. The diagnostic is reported at the
`match` line and names the first missing case.

<!-- error-example: E0006 -->
```burdin
enum Color {
    Red
    Green
    Blue
}

fn name(c: Color): str {
    match c { //~ E0006
        Color.Red => "red",
        Color.Green => "green",
    }
}

fn main() {
    println(name(Color.Red))
}
```
<!-- /error-example -->

**How to fix:** add the missing variant, literal, or range patterns, or add an `else` arm to cover everything
that is left.

## E0008: Literal out of range for its type

The value of an integer or float literal does not fit the type the context gives it.

<!-- error-example: E0008 -->
```burdin
fn main() {
    let x: u8 = 300 //~ E0008
}
```
<!-- /error-example -->

**How to fix:** use a wider type for the binding or the expression, or write a value the type can represent.

## E0100: Use of a moved value

The value was moved earlier, so this use would read storage that no longer owns anything. The diagnostic carries
a note pointing at the move, and the same code covers a value that is only possibly moved after a branch and one
that is partially moved.

<!-- error-example: E0100 -->
```burdin
struct Thing {
    id: i32
}

fn take(own t: Thing) {}

fn main() {
    let t = Thing { id: 1 }
    take(t)
    let u = t //~ E0100
}
```
<!-- /error-example -->

**How to fix:** do not use the value after moving it. Pass it by reference (a read-only or `mut` parameter) if the
callee only needs to look at it, clone it first, or reorder the statements so the move is the last use.

## E0101: Move out of borrowed content

The value is only borrowed here: a read-only parameter, a `mut` parameter, a `self` receiver, a binding from a
borrowed `match` scrutinee, or a view. The callee does not own it, so it cannot give it away.

<!-- error-example: E0101 -->
```burdin
fn eat(own s: String) {}

fn touch(s: String) {
    eat(s) //~ E0101
}

fn main() {
    touch(String.from("x"))
}
```
<!-- /error-example -->

**How to fix:** take the parameter as `own` if the function is supposed to consume it, or clone the value before
moving the clone.

## E0103: Partial move out of a type that implements `Drop`

A field may not be moved out of a value whose type has a user `Drop` implementation: the destructor must always
see the whole value. The type may be moved as a whole.

<!-- error-example: E0103 -->
```burdin
struct Inner {
    n: i32
}

struct Outer {
    inner: Inner
}

Outer: Drop {
    fn drop(mut self) {}
}

fn main() {
    let o = Outer { inner: Inner { n: 1 } }
    let i = o.inner //~ E0103
}
```
<!-- /error-example -->

**How to fix:** move the whole value, or restructure so the field that must move out is not stored in a type with
a destructor.

## E0104: Value moved in a previous loop iteration

A value declared outside a loop is moved inside its body. The first iteration succeeds; the next one would use the
same, already moved value.

<!-- error-example: E0104 -->
```burdin
struct Thing {
    id: i32
}

fn take(own t: Thing) {}

fn main() {
    let t = Thing { id: 1 }
    for i in 0..3 {
        take(t) //~ E0104
    }
}
```
<!-- /error-example -->

**How to fix:** create the value inside the loop, clone it for each iteration, or move it before or after the loop
instead of inside it.

## E0110: Exclusivity violation in a call

Two arguments of the same call mention overlapping places, and one of them is borrowed exclusively (`mut`, `own`,
or a `mut self` receiver). The other argument's span is attached as a note.

<!-- error-example: E0110 -->
```burdin
struct Counter {
    n: i32
}

fn add_to(mut c: Counter, amount: i32) {
    c.n = c.n + amount
}

fn main() {
    let mut c = Counter { n: 1 }
    add_to(mut c, c.n) //~ E0110
}
```
<!-- /error-example -->

**How to fix:** pass distinct places. Compute the overlapping value into a local before the call, or split the
receiver so one argument uses a field the other does not mention.

## E0111: Conflicting borrow

A write, a `&mut`, or a move happens while a borrow of the same place is still live (its last use is later).

<!-- error-example: E0111 -->
```burdin
struct Pair {
    a: i32
    b: i32
}

fn first_of(p: Pair): &i32 {
    &p.a
}

fn main() {
    let mut p = Pair { a: 1, b: 2 }
    let r = first_of(p)
    p.a = 5 //~ E0111
    println((*r).to_string())
}
```
<!-- /error-example -->

**How to fix:** finish using the reference before changing the place, for example by moving the last read of `r`
above the assignment, or by copying the value out instead of holding a view.

## E0113: Mutable use of an immutable place

An operation needs to mutate a place, but the place cannot be mutated: the binding is not `mut`, the place is a
read-only parameter or pattern binding, or a `&mut` is taken from an immutable place.

<!-- error-example: E0113 -->
```burdin
struct Counter {
    n: i32
}

fn bump(mut c: Counter) {
    c.n = c.n + 1
}

fn main() {
    let c = Counter { n: 0 }
    bump(mut c) //~ E0113
}
```
<!-- /error-example -->

**How to fix:** declare the binding with `let mut`, or make the root of the place mutable. A `mut` argument at the
call site only says the callee may mutate; the binding itself must still be mutable.

## E0114: Missing `mut` marker

The parameter is declared `mut`, so every call must mark the argument with `mut` as well (CV2). The call is passed
plainly here.

<!-- error-example: E0114 -->
```burdin
struct Counter {
    n: i32
}

fn bump(mut c: Counter) {
    c.n = c.n + 1
}

fn main() {
    let mut c = Counter { n: 0 }
    bump(c) //~ E0114
}
```
<!-- /error-example -->

**How to fix:** write `mut` before the argument: `bump(mut c)`. The diagnostic's JSON suggestion is the insertion
of `mut ` at the argument's position.

## E0115: `mut` marker on a non-`mut` parameter

The argument is marked `mut`, but the parameter is read-only or `own`, and only a named `mut` parameter takes the
marker at the call site (CV3).

<!-- error-example: E0115 -->
```burdin
struct P {
    id: i32
}

fn eat(own p: P): i32 {
    p.id
}

fn main() {
    let p = P { id: 1 }
    println(eat(mut p).to_string()) //~ E0115
}
```
<!-- /error-example -->

**How to fix:** remove the `mut` marker. The diagnostic's JSON suggestion is the deletion of the marker and the
whitespace after it.

## E0120: Reference or view in a struct field, enum variant, or collection

References and views are second-class: they may live in a local, a parameter, a return value, and a pattern
binding, but not inside a struct field, an enum variant, a collection element, or a type stored in one.

<!-- error-example: E0120 -->
```burdin
struct Holder {
    o: Option<&i32> //~ E0120
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** store the value itself and own it, or pass the reference as a function parameter instead of
packing it into a data type.

## E0121: Returned reference derives from more than one parameter

A second-class result must derive from exactly one parameter (RT1). This function returns a reference that can
come from either of two parameters, so the caller cannot tell which one stays borrowed.

<!-- error-example: E0121 -->
```burdin
struct Counter {
    n: i32
}

fn pick(a: Counter, b: Counter, first: bool): &i32 { //~ E0121
    if first { &a.n } else { &b.n }
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** return a reference derived from one parameter only, or return an owned value instead. A static
string literal is the one result that needs no parameter.

## E0122: Returned reference points to a local

A returned reference or view points at storage that dies when the function returns: a local, an `own` parameter
(which is dropped), or a temporary.

<!-- error-example: E0122 -->
```burdin
struct Counter {
    n: i32
}

fn make(own c: Counter): &i32 { &c.n } //~ E0122

fn main() {}
```
<!-- /error-example -->

**How to fix:** return an owned value, or return a view derived from a read-only or `mut` parameter, whose storage
belongs to the caller.

## E0130: Derive requirement not met

A field's type does not satisfy the trait being derived, or the type combines derives or interfaces that cannot
coexist (`Drop` with `Copy`). The diagnostic points at the offending field or derive clause.

<!-- error-example: E0130 -->
```burdin
struct Bad derives (Copy, Clone) { //~ E0130
    n: i32
}

Bad: Drop {
    fn drop(mut self) {}
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** give the field type the derive it needs, stop deriving the member, or remove one of the conflicting
declarations. A type with a destructor is never `Copy`.

## E0140: Interface implementation is missing a method

The `Type: Interface` block does not implement every method that has no default. The diagnostic is reported at the
implementation line.

<!-- error-example: E0140 -->
```burdin
interface Shape {
    fn area(self): i32
    fn name(self): str
}

struct Sq {
    n: i32
}

Sq: Shape { //~ E0140
    fn area(self): i32 { self.n }
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** implement the missing method in the block, or give the interface method a default body that the
implementation can inherit.

## E0141: Interface bound or `requires` not satisfied

A generic call passes a type argument that does not meet the bound, an `impl` does not satisfy the interface's
`requires`, or a generic body uses an operation its bounds do not provide. The message names the missing bound.

<!-- error-example: E0141 -->
```burdin
interface Shape {
    fn area(self): i32
}

struct Plain {
    n: i32
}

fn report<T: Shape>(s: T) {
    println(s.area().to_string())
}

fn main() {
    report(Plain { n: 1 }) //~ E0141
}
```
<!-- /error-example -->

**How to fix:** implement the interface (or add the derive) for the type argument, or pass a type that already has
it. Inside a generic body, only the operations the bounds promise are available.

## E0142: Orphan rule violation

An `impl` block adds inherent methods or a `Drop` implementation to a type the current package does not declare,
or implements a foreign interface for a foreign type. An implementation needs a local type or a local interface
(EX2/coherence).

<!-- error-example: E0142 -->
```burdin
String: From<i32> { //~ E0142
    fn from(n: i32): String { n.to_string() }
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** move the implementation to the package that declares the type, or implement a local interface for
the foreign type instead.

## E0150: Use of a `priv` item from another package

The item exists but is package-private: another package may not import it, read or write a private field,
construct a struct with a private field, or call a private method. The bundle below is two packages; the app
imports a private function from the dependency.

<!-- error-example: E0150 target=app -->
```burdin
// --- app/burdin.toml ---
[package]
name = "app"

[dependencies]
lib = { path = "../lib" }

// --- app/src/main.bd ---
use lib.secret //~ E0150

fn main() {
    secret()
}

// --- lib/burdin.toml ---
[package]
name = "lib"

// --- lib/src/lib.bd ---
priv fn secret() {}
```
<!-- /error-example -->

**How to fix:** make the item visible in the package that declares it (remove `priv`), or stop using it from the
other package and rely on the public surface instead.

## E0160: `?` outside a function returning `Option` or `Result`

The `?` operator can only return early from a function (or test block) that returns `Option` or `Result`, with an
error type the value can convert into.

<!-- error-example: E0160 -->
```burdin
fn get(): Result<i32, i32> {
    Ok(1)
}

fn f(): i32 {
    let x = get()? //~ E0160
    x
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** change the function's return type to a matching `Result`/`Option`, or handle the value with
`match`, `orelse`, or `unwrap` instead of `?`.

## E0161: Operation that requires `unsafe` outside `unsafe`

Calling an `extern "C"` function, an `unsafe fn`, or an `unsafe` interface method needs an `unsafe` block (or an
`unsafe fn` body).

<!-- error-example: E0161 -->
```burdin
extern "C" {
    fn abs(x: i32): i32
}

fn main() {
    let n = abs(-5) //~ E0161
}
```
<!-- /error-example -->

**How to fix:** wrap the call in `unsafe { ... }`, or mark the enclosing function `unsafe` if the whole body
should carry the requirement.

## E0162: Type or convention not allowed in an `extern "C"` declaration

Parameters and results of a C declaration are limited to integers, floats, and `CPtr<T>`; declarations take no
`mut`, `own`, or `self`, no generics, and only the `C` convention.

<!-- error-example: E0162 -->
```burdin
extern "C" {
    fn takes_string(s: String): usize //~ E0162
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** use an integer, float, or `CPtr<T>` in the declaration; pass strings as a `CString`/`CStr` pointer,
and declare a plain C signature without conventions or receivers.

## E0170: Invalid test attribute

A `test` item's attribute list is checked when the test is compiled. The list may name `ignore`, `timeout`,
`should_panic`, or `should_fail`, each once. The message names the unknown or repeated attribute, the value the
attribute does not take, or the `should_panic`/`should_fail` combination.

<!-- error-example: E0170 mode=test -->
```burdin
test "repeated" [ignore, ignore] { //~ E0170
    assert(true)?
}
```
<!-- /error-example -->

**How to fix:** write each attribute once, with the value it takes: `ignore`, `timeout(500)`, `should_panic`, or
`should_panic("message")`. Use `should_fail` on its own rather than together with `should_panic`; the first expects
a returned error and the second a panic abort.
