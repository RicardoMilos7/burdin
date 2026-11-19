# Error code reference

Every Burdin diagnostic carries a code from the registry in [`tests/README.md`](../tests/README.md#error-codes).
This page explains each code, shows a program that triggers it, and says how to fix it. Errors stop compilation;
E0215 is a warning and does not.

The page and its examples are checked by `tests/check_error_docs.py` in CI: the codes must match the registry
exactly, every `burdin` snippet below is a complete program that the check compiles and expects to fail with
exactly the marked diagnostics, and `burdin explain <code>` (which reads this page) must print every entry. The
`//~ E0000` comment at the end of a line marks the line the diagnostic is reported on. An example marked
`mode=test` is compiled with `burdin test`, because a test attribute is checked only in test mode; `mode=export`
runs `burdin bindgen --export` on the package, because that check runs after compilation and only for that command. An example in a
`warning-example` block must compile and emit exactly the marked warnings.

Run `burdin explain E0003` to print one entry in the terminal, or `burdin explain` to list the codes.

Every entry also has a stable address on the hosted copy of this page: `https://ricardomilos7.github.io/burdin/errors/E0003`
for E0003, and in general the root, `/errors/`, and the code. The address ends in the code, and the scheme does not
change between versions, so a link in a log, a bug report, or a code-scanning alert keeps working. Every diagnostic
carries its link: the `url` field of the JSON format, the `helpUri` of a SARIF rule, and a terminal hyperlink on the
code in colored output when the terminal supports it (see [diagnostics.md](diagnostics.md)). `burdin explain` prints it
under the entry.

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

## E0007: A name is declared more than once

A type, interface, function, constant, test, type parameter, field, enum variant, method, or import repeats a name
that is already declared in the same namespace. The diagnostic points at the second declaration.

<!-- error-example: E0007 -->
```burdin
fn value(): i32 {
    1
}

fn value(): i32 { //~ E0007
    2
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** rename one of the two declarations, or remove it. Types and values have separate namespaces, so a
type and a function may share a spelling; two items in the same namespace may not.

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

## E0009: A declaration depends on itself

A constant's initializer depends on the constant itself (directly, through other constants, or through the
functions it calls while it is evaluated at compile time), a type alias refers to itself (directly or through other
aliases), or a struct or enum contains itself by value, which would need infinite size.

<!-- error-example: E0009 -->
```burdin
const LIMIT: i32 = LIMIT //~ E0009

fn main() {}
```
<!-- /error-example -->

**How to fix:** break the cycle: give the constant a value that does not mention it, point the alias at a concrete
type, or put the recursive value behind a `Heap<T>` (or a `List<T>`, which allocates).

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
borrowed `match` scrutinee, a view, or an element read through a user type's `Index` implementation (which returns
a borrowed reference). The callee does not own it, so it cannot give it away.

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

The same code reports the conflicts between the tasks of a `task.scope`: what a task started with `Scope.spawn`
borrows stays borrowed until the scope body ends, so the body (or another task) cannot write a place a task borrows,
two tasks cannot both write one place, and a task started in a loop cannot borrow a place mutably, because every
iteration would start another task that writes it (`spec/tasks.md`, TK7).

**How to fix:** finish using the reference before changing the place, for example by moving the last read of `r`
above the assignment, or by copying the value out instead of holding a view.

## E0113: Mutable use of an immutable place

An operation needs to mutate a place, but the place cannot be mutated: the binding is not `mut`, the place is a
read-only parameter or pattern binding, a `&mut` is taken from an immutable place, or an index assignment needs the
user type's `IndexMut` implementation and the receiver is not mutable.

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

## E0123: View outlives the storage it borrows from

A view stored in a local borrows from storage that dies before the binding: a temporary, which is dropped at the
end of its statement, or a local of an inner block, which is dropped before the binding escapes it.

<!-- error-example: E0123 -->
```burdin
struct Pair {
    a: i32
    b: i32
}

fn first_of(p: Pair): &i32 {
    &p.a
}

fn main() {
    let r = first_of(Pair { a: 1, b: 2 }) //~ E0123
}
```
<!-- /error-example -->

**How to fix:** keep the source in a local that outlives the view (`let pair = Pair { a: 1, b: 2 }` and then
`first_of(pair)`), or store an owned value instead of a view.

## E0130: Derive requirement not met

A field's type does not satisfy the trait being derived, or the type combines derives or interfaces that cannot
coexist (`Drop` with `Copy`). The diagnostic points at the offending field or derive clause.

<!-- error-example: E0130 -->
```burdin
struct Bad: Copy, Clone { //~ E0130
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

## E0131: Invalid `Drop` implementation

A `Drop` block is exactly one method named `drop`. The diagnostic is reported at the block when it has more than
one method, or at the method name when the one method is named something else.

<!-- error-example: E0131 -->
```burdin
struct Widget {
    id: i32
}

Widget: Drop { //~ E0131
    fn drop(mut self) {}
    fn extra(self) {}
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** write `Type: Drop { fn drop(mut self) { ... } }` with no other methods. Put any other cleanup in an
ordinary method or in the `drop` body.

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
`requires`, a generic body uses an operation its bounds do not provide, or a user type has no implementation of an
operator interface (`Add`, `Neg`, `PartialEq`, `PartialOrd`, `Index`, `IndexMut`, ...) that the expression needs. The
message names the missing interface or bound.

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

## E0143: Overlapping implementations

Two impl blocks of one interface for one generic type apply to a common instance: `Stack<T>: Describe` covers every
instance, so `Stack<i32>: Describe` covers an instance the first one already has. The compiler reports the later block
and leaves it out. `where` clauses do not tell two impls apart, because the compiler does not decide whether two
bounds can hold together.

<!-- error-example: E0143 -->
```burdin
interface Describe {
    fn describe(self): String
}

struct Stack<T> {
    items: List<T>
}

Stack<T>: Describe {
    fn describe(self): String { String.from("any") }
}

Stack<i32>: Describe { //~ E0143
    fn describe(self): String { String.from("ints") }
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** keep one impl that covers the instances you mean, or give the two impls patterns that do not
overlap (`Stack<i32>` and `Stack<bool>`). An inherent impl on `Stack<i32>` cannot repeat a method the general impl
already gives that instance (E0007): give it another name.

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
`unsafe fn` body). So does any raw-pointer operation: creating, holding, passing, moving, dereferencing, testing
(`.is_null()`), comparing, offsetting, or casting a `CPtr` value.

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
`mut`, `own`, or `self`, no generics, and only the `C` convention. The same rules apply to an exported
`extern "C" fn`, which also rejects `...` and must declare every parameter `own`. A `repr(C)` struct crosses the C
ABI through `CPtr<T>` only, because the platform by-value coercions are not implemented, and a Burdin `bool` is a
one-bit value while C `_Bool` is a one-byte integer, so pass it as `u8` (0 or 1).

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

## E0163: Invalid `extern` declaration

An `extern` declaration names a symbol the runtime reserves (`main`, `burdin_rt_*`, `burdin.*`), declares a generic
function, or gives a second declaration of one symbol a different signature than the first. The diagnostic points
at the offending name.

<!-- error-example: E0163 -->
```burdin
extern "C" {
    fn burdin_rt_panic(): i32 //~ E0163
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** choose a symbol the runtime does not own, remove the type parameters from the declaration, or make
every declaration of one symbol use the same signature. The first declaration defines the ABI.

## E0164: Unsupported `repr`

Only `repr(C)` exists: a struct may ask for the C layout or for the default Burdin layout, and no other
representation is accepted.

<!-- error-example: E0164 -->
```burdin
struct Packed repr(packed) { //~ E0164
    x: i32
    y: i32
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** write `repr(C)`, or remove the marker to keep the default layout.

## E0170: Invalid test attribute

A `test` item's attribute list is checked when the test is compiled. The list may name `ignore`, `timeout`,
`should_panic`, `should_fail`, or `differential`, each once. The message names the unknown or repeated attribute,
the value the attribute does not take, or an incompatible combination: `should_panic` with `should_fail`, and
`differential` with either of them (a differential test takes its expectation from the compile-time run).

<!-- error-example: E0170 mode=test -->
```burdin
test "repeated" [ignore, ignore] { //~ E0170
    assert(true)?
}
```
<!-- /error-example -->

**How to fix:** write each attribute once, with the value it takes: `ignore`, `timeout(500)`, `should_panic`,
`should_panic("message")`, `should_fail`, or `differential`. Use `should_fail` on its own rather than together with
`should_panic`; the first expects a returned error and the second a panic abort. Use `differential` on its own
rather than with either; its expectation is the compile-time outcome.

## E0171: A second-class closure escapes

A closure that borrows its captures is a second-class value: it may be called in place, kept in a local, or passed
to a generic parameter, but it cannot be returned or stored where an owned closure type (`Fn(...)`, `FnMut(...)`,
`FnOnce(...)`) is expected, because the borrow would outlive the storage it points at. A closure that moves a view
(`str`, `&T`, `[T]`, or an aggregate containing one) into its record is second-class for the same reason: the record
holds a pointer to storage it does not own. The same code reports a closure whose declared return type is a view
(`str`, `&T`, `[T]`): the result would point into the closure's capture record, which is gone once the call returns.

<!-- error-example: E0171 -->
```burdin
fn main() {
    let n = 1
    let f = || n
    let g: Fn(): i32 = f //~ E0171
}
```
<!-- /error-example -->

Tasks of a scope have two more cases (`spec/tasks.md`, TK7). A closure given to `Scope.spawn` that borrows must be
written at the call; passing a closure variable that borrows hides the borrow from the checker. And a task may borrow
only places that outlive the scope body, so borrowing a local of the body is rejected: the local is gone before the
scope waits for the task. Declare the variable before the `scope` call, give the task a copy of the value it needs, or return the
result through a task handle (`task.spawn`).

**How to fix:** make the closure own its captures by writing it where an owned closure type is expected (the
compiler then captures by move), or move the captured value into a local first and capture that. A captured view
counts as borrowed even when its bits are copied into the record, so convert it to an owned value (for example,
`String.from(s)`) before capturing it. A closure passed through a generic `F` with an `Fn` bound keeps its borrow
and needs no change.

## E0180: Type arguments on a non-generic type

A type path supplies type arguments to a type that declares no type parameters, such as a plain struct or a
built-in like `i32`.

<!-- error-example: E0180 -->
```burdin
struct Pair {
    a: i32
}

fn main() {
    let p: Pair<i32> = Pair { a: 1 } //~ E0180
}
```
<!-- /error-example -->

**How to fix:** remove the type arguments, or declare the type with the parameters the program needs
(`struct Pair<T> { ... }`).

## E0190: Format specifier that is malformed or does not apply

The text after the `:` of a placeholder (`{value:spec}`) follows the grammar
`[[fill]align][+][#][0][width][.precision][type]`. The diagnostic is reported for a character that does not fit
that grammar (an unknown type letter, a `.` with no digits, a width over 4096), and for a specifier the value
cannot take: `x`, `X`, `b`, and `o` format integers only, a precision formats floats, text, and user types, and
`+`, `#`, and `0` format numbers. `spec/format.md` has the table of what each value type accepts.

<!-- error-example: E0190 -->
```burdin
fn main() {
    let n = 5
    let s = "text"
    println("{s:x}") //~ E0190
    println("{n:.2}") //~ E0190
}
```
<!-- /error-example -->

**How to fix:** use a specifier the value's type accepts. Integers take a width, fill and alignment, `+`, `0`, and
a radix; floats and text take a width, fill, alignment, and a precision; `{:?}` takes a width, fill, and
alignment. A user type chooses what to honor in its `fmt` body (through `f.pad`).

## E0191: Placeholder naming an argument the call does not have

A `{}` or `{n}` placeholder uses an argument of the enclosing `println`, `print`, `format`, or `eprint` call. The
diagnostic is reported when there are fewer arguments than the placeholder needs (in a bare string literal outside
a format call, every `{}` and `{n}` is such a placeholder), and when a needed argument appears in more than one
placeholder but is not a place and is not `Copy`, so it cannot be evaluated once and reused.

<!-- error-example: E0191 -->
```burdin
fn made(): String {
    String.from("made")
}

fn main() {
    let name = "ana"
    println("{} {}", 1) //~ E0191
    println("{1}") //~ E0191
    println("{name} {name}")
    println("{0} {0}", made()) //~ E0191
}
```
<!-- /error-example -->

**How to fix:** add the argument the placeholder needs, or use the placeholder name syntax for a value in scope
(`{name}`). An argument used by two placeholders is evaluated once when it is a local or another place, or a
`Copy` value; bind anything else to a `let` first.

## E0192: Argument of a format call that the format string does not use

Every argument after the format string must be used by a placeholder. The diagnostic is reported at the unused
argument, so an argument left over from an edit is easy to spot.

<!-- error-example: E0192 -->
```burdin
fn main() {
    println("hello {}", "ana", 7) //~ E0192
    println("{} {}", 1, "x", 2) //~ E0192
}
```
<!-- /error-example -->

**How to fix:** use the argument with `{}` or `{n}`, or remove it from the call.

## E0193: Value that does not implement `Display` or `Debug`

`{}` needs the value's type to implement `Display`, and `{:?}` needs `Debug`. Integers, floats, `bool`, `char`,
`str`, and `String` implement both; `()` and the compound types (tuples of 2 to 4, `Option`, `Result`, `List`,
`Heap`) implement `Debug` when their parts do; a struct or enum implements them with an impl block or, for `Debug`
only, with `struct T: Debug`. A type parameter needs the interface as a bound (`fn show<T: Display>(value: T)`).

<!-- error-example: E0193 -->
```burdin
struct Plain {
    id: i32
}

struct Shown {
    id: i32
}

Shown: Display {
    fn fmt(self, mut f: Formatter) {
        self.id.fmt(mut f)
    }
}

fn main() {
    let plain = Plain { id: 1 }
    let shown = Shown { id: 2 }
    println("{plain}") //~ E0193
    println("{plain:?}") //~ E0193
    println("{shown:?}") //~ E0193
    println("{shown}")
}
```
<!-- /error-example -->

**How to fix:** write `Type: Display { fn fmt(self, mut f: Formatter) { ... } }`, or add `Debug` to the
interfaces after the type's name, or use `{:?}` when the type only implements `Debug`. A generic function adds the bound
(`<T: Display>`). When the message says the type implements `Debug`, `{:?}` formats it.

## E0194: Format string that is not a string literal

A call with more than one argument builds its text from the first argument, which must be a string literal (with
or without placeholders), because its placeholders are checked against the rest of the call at compile time. A
plain call with one argument is unchanged: `println(text)` writes any `str` value, literal or not.

<!-- error-example: E0194 -->
```burdin
fn main() {
    let template = "n is"
    let n = 3
    println(template, n) //~ E0194
}
```
<!-- /error-example -->

**How to fix:** write the format string out at the call (`println("n = {n}")`), or format into a `String` first
(`let text = format("n = {n}")`) and print the value.

## E0200: `any` or `impl` applied to something that is not an interface

`any` and `impl` take an interface name (with optional type arguments), because the value is reached through a
vtable and implemented by types that satisfy that interface. A type (`any P`), a type parameter, or a built-in bound
(`any Copy`) cannot be used this way.

<!-- error-example: E0200 -->
```burdin
struct P {
    x: i32
}

fn main() {
    let p: any P = P { x: 1 } //~ E0200
}
```
<!-- /error-example -->

**How to fix:** name an interface after `any` or `impl`, or use the type directly if no dynamic dispatch is wanted.
A stored boxed value is `any Interface`; a parameter that accepts any implementation statically is `impl Interface`.

## E0201: `impl Interface` outside a parameter list

`impl Interface` is sugar for one hidden generic parameter with that interface as its bound, so it needs a call site
to infer the concrete type from. A return type, a local annotation, a field, a container element, a bound, or an
`extern` signature has no such parameter.

<!-- error-example: E0201 -->
```burdin
interface Shape {
    fn area(self): f64
}

struct C {
    r: f64
}

fn main() {
    let s: impl Shape = C { r: 1.0 } //~ E0201
}
```
<!-- /error-example -->

**How to fix:** write `impl Interface` only in a parameter list (`fn draw(shape: impl Shape)`). For a value that is
stored, returned, or put in a field and still needs dynamic dispatch, use `any Interface`; otherwise name the
concrete type.

## E0202: An interface that cannot be used as `any`

An interface used as `any Interface` must be object-safe, because its methods are reached through a fixed vtable:
no method may have its own type parameters, every method needs a `self` receiver, `own self` is not allowed (the
boxed value is reached through a pointer), no method may mention `Self` outside its receiver, no method may return
a view, and every interface it `requires` must itself be object-safe.

<!-- error-example: E0202 -->
```burdin
interface Bad {
    fn map<U>(self, value: U)
}

fn take(x: any Bad) {} //~ E0202

fn main() {}
```
<!-- /error-example -->

**How to fix:** make the interface object-safe: remove the type parameters from the method or move the operation to
a `where`-bounded generic function, give every method a `self` or `mut self` receiver, return an owned value instead
of a view, and make the `requires` interfaces object-safe too. When a vtable is not needed, `impl Interface` or a
generic bound dispatches statically and does not require object safety.

## E0204: Boxing a type whose implementation is compiler-provided

The compiler implements `Display` and `Debug` for the primitive types and the built-in containers directly, without
a method, so there is no function for an `any Display`/`any Debug` vtable to dispatch to. A user type with a
`Type: Display` block or `struct T: Debug` can be boxed, because it has callable methods.

<!-- error-example: E0204 -->
```burdin
fn main() {
    let x: any Display = 3 //~ E0204
}
```
<!-- /error-example -->

**How to fix:** format the value directly (`println("{}", 3)`), or wrap it in a user type that implements the
interface with a method and box that. `any Interface` needs a callable implementation for every vtable slot.

## E0210: Invalid error declaration

An `error enum` variant's message template names its payload fields, and a `from` variant must be a tuple variant
with exactly one field (that field's type is the conversion's source). A placeholder that names no payload field
(an index out of range, a name that is not a field, a placeholder on a unit variant, or an expression), a `from` on
a variant with zero or several fields, or a `from` on a generic error enum is this error. The message's fill
character may not be `{`, `}`, `"`, or `\`, because the message is written back as a string literal.

<!-- error-example: E0210 -->
```burdin
error enum Bad {
    Pair(i32, i32) from "pair {0}" //~ E0210
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** give a `from` variant exactly one tuple field, use only payload field names or positions in the
message, and write no `from` on a generic error enum. `spec/errors.md` ERR1-ERR3.

## E0211: Duplicate `from` conversion

One error enum may convert from a given source type only once: the generated `Type: From<Source>` blocks would
collide, and `?` would not know which variant to build.

<!-- error-example: E0211 -->
```burdin
error enum Twice {
    First(IoError) from "first: {0}"
    Second(IoError) from "second: {0}" //~ E0211
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** keep one `from` variant per source type; convert the other source types with a hand-written
`Type: From<Source>` block if they must map to a different variant.

## E0212: Missing error conversion

`?` converts the error of a `Result` into the enclosing function's error type through a `Type: From<Source>` block,
or into the dynamic `Error` when the function returns `Result<...>` and the value's type is an error type (a type
implementing `Error`, which requires `Display`). An integer, a `String`, or any other non-error type cannot be
converted this way, and `.context(...)` needs an error type too.

<!-- error-example: E0212 -->
```burdin
fn load(): Result<i32, i32> {
    Ok(1)
}

fn run(): Result<i32, Error> {
    let n = load()? //~ E0212
    Ok(n)
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** give the inner function an error type (use an `error enum` or `Error`), or add a
`Type: From<Source> { fn from(value: Source): Type { ... } }` block that converts it.

## E0213: Invalid default argument

A parameter written `name: Type = expr` declares a value the call may omit. The default is checked once at the
declaration against the parameter's type, so it must be a constant expression (a literal, another constant, an
operator on constants, or a cast), and the function's parameters and `self` are not in scope in it. A default may
not depend on a type parameter, may not be declared on a `mut` parameter (which is the caller's place, not a
value), may not appear in an interface method signature (shared by every implementation), and may not appear in
an `extern "C"` declaration. The parameters with defaults must be the trailing ones: a default before a required
parameter would make a positional call ambiguous. `spec/arguments.md` AG1-AG3 and AG5.

<!-- error-example: E0213 -->
```burdin
fn update(mut total: i32 = 0) { //~ E0213
    total = total + 1
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** write the value at the call site, declare the parameter without `mut` if it does not need to
write back, move the parameters with defaults to the end of the list, and use only constants from the enclosing
module in the default.

## E0214: Invalid named argument

A call may write `name: value` to fill a parameter by name. The name must be one of the callee's parameters, may
be given only once, and once one argument is named every later argument must be named too. A callee without
parameter names takes no named arguments: a built-in function or method, `Some`/`Ok`/`Err`, a variant
constructor, a closure value, and an `extern` declaration. `spec/arguments.md` AG4.

<!-- error-example: E0214 -->
```burdin
fn move_to(x: i32, y: i32) {}

fn main() {
    move_to(y: 1, z: 2) //~ E0214
}
```
<!-- /error-example -->

**How to fix:** use the parameter's spelling from the declaration, remove the duplicate argument, move every
positional argument before the named ones, or pass the arguments positionally to a callee that has no parameter
names.

## E0215: Shadowed extension method

An extension method `fn Type.name(self, ...)` is visible in its module and in modules that import it by name.
When the receiver type already has a method of that name — declared by an impl block, provided by an interface
implementation, or provided by the compiler for a built-in type — the method is called at every use and the
extension is unreachable through dot syntax. The compiler warns at the extension's declaration so the dead
extension is not a surprise. The program still compiles and runs. `spec/extensions.md` EX3, EX4.

<!-- warning-example: E0215 -->
```burdin
struct Gadget { n: i32 }

Gadget {
    fn label(self): String {
        String.from("method")
    }
}

fn Gadget.label(self): String { //~ E0215
    String.from("extension")
}

fn main() {
    let g = Gadget { n: 1 }
    println(g.label())
}
```
<!-- /warning-example -->

**How to fix:** rename the extension, or delete it if the type's method now does the work. If the extension is
meant to replace the method, remove the method or call a differently named extension.

## E0216: Invalid extension method declaration

An extension method is `fn Type.name(self, ...)`: the receiver is a type path in scope and `self` is its first
parameter. This error reports a declaration without `self` first, type arguments on a segment of the receiver path
other than the type itself, a `where` predicate naming something that is not one of the extension's type
parameters, and importing an extension under another name (its calls always use the declared name).
`spec/extensions.md` EX1, EX2, EX5.

<!-- error-example: E0216 -->
```burdin
fn String.bad(): String { //~ E0216
    String.from("x")
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** write `self` as the first parameter (`fn String.bad(self)`), put the type arguments on the type name
at the end of the receiver (`fn List<T>.bad(self)`, not `fn List<i32>.Inner.bad(self)`), move the `where` predicate
to an extension type parameter, or import the extension by its declared method name.

## E0220: Invalid comptime construct

A compile-time context (a `comptime` expression or block, a `comptime if` condition, a `comptime for` iterable,
a `comptime assert`, or the argument of a `comptime` parameter) can only use what is known while compiling:
constants, `comptime` parameters, `comptime for` bindings, and its own locals. Using a run-time local or `self`
there is this error, and so are the forms that cannot be compile-time: a `comptime` parameter written `mut` or
`own`, on `self`, an interface method, its implementation, or an `extern` declaration; a function with comptime
parameters used as a value; `break` or `continue` leaving a `comptime for`; a `mut` `comptime for` binding; and a
`comptime assert` message that is not a string literal. `spec/comptime.md` CT2, CT4, CT6, CT8.

<!-- error-example: E0220 -->
```burdin
fn double(v: i32): i32 {
    v * 2
}

fn main() {
    let x = 4
    let y = comptime double(x) //~ E0220
}
```
<!-- /error-example -->

**How to fix:** compute the value from constants or literals (`comptime double(4)`), make the input a `comptime`
parameter of the enclosing function, or drop `comptime` and compute the value at run time.

## E0221: Value that cannot be materialized

A compile-time value is copied into the program, so its type must be one the compiler can write down as a literal:
a scalar, `str`, `String`, or a tuple, array, struct, enum, `Option`, `Result`, or range of those. A `List`, `Map`,
`Heap`, `any` value, closure, reference, slice, C type, or iterator can be created and used inside comptime code,
but not be the value of a `const`, a comptime expression, a `comptime` parameter, or a `comptime for` item.
`spec/comptime.md` CT3.

<!-- error-example: E0221 -->
```burdin
const EMPTY: List<i32> = List<i32>.new() //~ E0221

fn main() {}
```
<!-- /error-example -->

**How to fix:** materialize a fixed-size form instead (an array, or a `String` built from the values), or build the
collection at run time.

## E0222: Compile-time evaluation panicked

Evaluating a constant, a comptime expression, or a specialization's comptime arguments panicked: an explicit
`panic`, `unreachable`, a failed `assert`, `unwrap` on an empty value, an index out of bounds, a division by zero,
or an integer overflow in a debug build. The message is the panic message, with a note at the operation that
panicked. `spec/comptime.md` CT10.

<!-- error-example: E0222 -->
```burdin
fn pick(i: usize): i32 {
    let xs = [1, 2, 3]
    xs[i]
}

const BAD: i32 = pick(5) //~ E0222

fn main() {}
```
<!-- /error-example -->

**How to fix:** correct the computation, or check the condition the panic reports before the operation runs.

## E0223: Comptime assertion failed

The condition of a `comptime assert` was false when the program was compiled. The message of the assertion, when it
has one, is part of the diagnostic. `spec/comptime.md` CT8.

<!-- error-example: E0223 -->
```burdin
const BUFFER_SIZE: usize = 100

comptime assert(BUFFER_SIZE % 64 == 0, "the buffer must be a whole number of cache lines") //~ E0223

fn main() {}
```
<!-- /error-example -->

**How to fix:** change the values the assertion checks so the condition holds, or change the assertion if the
requirement it states is no longer true.

## E0224: Impure operation at compile time

Compile-time evaluation is pure: it cannot print, read or write files, read the process arguments, end the process,
run a command, call a C function, or use C pointers and C strings. The check is dynamic, so a function that could do
one of these may still run at compile time when the path it takes does not. The same code reports an internal error
of the comptime interpreter. `spec/comptime.md` CT9.

<!-- error-example: E0224 -->
```burdin
fn noisy(): i32 {
    println("hello")
    7
}

const N: i32 = noisy() //~ E0224

fn main() {}
```
<!-- /error-example -->

**How to fix:** keep the input and output at run time and pass the computed values in, or split the pure part of
the computation into a function the constant can call.

## E0225: Compile-time budget exceeded

A compile-time evaluation ran for more steps than its budget (10,000,000 by default), nested more than 512 calls,
or a function with comptime parameters needed more than 1024 specializations. The usual cause is a loop or a
recursion that does not end; a recursion through a comptime parameter needs a `comptime if` that stops it.
`spec/comptime.md` CT4, CT11.

<!-- error-example: E0225 -->
```burdin
fn spin(): i32 {
    let mut i = 0
    loop {
        i = (i + 1) % 10
    }
}

const STUCK: i32 = spin() //~ E0225

fn main() {}
```
<!-- /error-example -->

**How to fix:** make the computation end. If it is meant to be long, raise the budget with
`burdin build --comptime-steps <n>`.

## E0230: Invalid reflection

`Type.fields` lists the fields of a struct and `Type.variants` the variants of an enum, and any other type (a
number, `bool`, `str`, a tuple, an array) has neither. A struct has no variants and an enum no fields, so a derive
written for both kinds can ask for either one and get an empty array. `spec/reflection.md` RFL2.

<!-- error-example: E0230 -->
```burdin
fn main() {
    let fields = i32.fields //~ E0230
}
```
<!-- /error-example -->

**How to fix:** ask for the members of a struct or an enum, or do not reflect on a type that has none.

## E0231: Name reserved for reflection

Every type has the compile-time members `name`, `size`, `align`, `fields`, and `variants`. A function with no
`self` called one of those names, or an enum variant called one of them, would be ambiguous with the member, so it
is an error. A method with `self` is called on a value and is not affected. `spec/reflection.md` RFL3.

<!-- error-example: E0231 -->
```burdin
enum Mode {
    name //~ E0231
    Fast
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** give the variant or the function another name.

## E0232: Invalid computed field access

`x.{name}` reads the field whose name is the compile-time string in the braces: a string literal, or the `name` of
the loop variable of a `comptime for` over `Type.fields`. A name that is only known while the program runs cannot
choose a field, and the type must have a field of that name. `spec/reflection.md` RFL5.

<!-- error-example: E0232 -->
```burdin
struct Point {
    x: i32
}

fn main() {
    let p = Point { x: 1 }
    let name = "x"
    println("{}", p.{name}) //~ E0232
}
```
<!-- /error-example -->

**How to fix:** write the name as a string literal, or loop over `Type.fields` in a `comptime for` and use
`field.name`.

## E0233: Invalid derive

A `derive Name { ... }` declaration cannot take the name of a built-in derive (`Copy`, `Clone`, `Eq`, `Ord`,
`Hash`, `Default`, `Debug`), and `struct Type: Name` with a user derive needs the interface `Name` in scope, because
the derive generates `Type: Name`. `spec/reflection.md` RFL8, RFL9.

<!-- error-example: E0233 -->
```burdin
derive Clone { //~ E0233
    fn clone(self): Self { self }
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** pick a name no built-in derive uses, and import the interface together with the derive.

## E0234: Interface cannot be derived

The interfaces after a struct's or enum's name (`struct Point: Copy, Eq { ... }`) are implemented by the declaration
itself: a built-in derive, a `derive` declaration, or an interface whose methods all have default bodies. An interface
with a method that has no default, such as `Drop`, `From`, or `Display`, needs that method written, so the
declaration cannot implement it. `spec/derives.md`.

<!-- error-example: E0234 -->
```burdin
struct Handle: Drop { //~ E0234
    id: i32
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** remove the interface from the list and write an impl block with the method,
`Handle: Drop { fn drop(mut self) { ... } }`.

## E0240: `embed` could not read its file

`embed("path")` reads a file while compiling. The file is missing, cannot be read, or, for `str`, is not valid
UTF-8 text. The path is relative to the source file that writes the call. `spec/comptime.md` CT15.

<!-- error-example: E0240 -->
```burdin
fn main() {
    let text = embed("no_such_file.txt") //~ E0240
    println(text)
}
```
<!-- /error-example -->

**How to fix:** correct the path, or embed a binary file as bytes with an expected type of `[u8; N]`.

## E0241: Invalid `embed`

The path of `embed` must be a string literal, so that the file is known while compiling, and it must be relative and
stay inside the directory of the source file (no `..`). When the expected type is `[u8; N]`, `N` must be the length
of the file. `spec/comptime.md` CT15.

<!-- error-example: E0241 -->
```burdin
fn main() {
    let name = "data.txt"
    let text = embed(name) //~ E0241
    println(text)
}
```
<!-- /error-example -->

**How to fix:** write the path as a string literal inside the source directory, and give `[u8; N]` the file's
length.

## E0250: Invalid const generic parameter or argument

A const parameter (`const N: usize`) takes a value of its own type, which is an integer type, `bool`, or `char`; a
type parameter takes a type. Giving a type where a value is expected, a value where a type is expected, a value that
does not fit the parameter's type, or declaring a const parameter of another type are all errors.
`spec/const-generics.md` CG2, CG3.

<!-- error-example: E0250 -->
```burdin
struct Grid<const W: usize> {
    cells: [i32; W]
}

fn main() {
    let g: Grid<i32> = Grid { cells: [0; 4] } //~ E0250
}
```
<!-- /error-example -->

**How to fix:** write a number (or a constant, or `{ expression }`) for a const parameter, and a type for a type
parameter.

## E0251: Const expression that cannot be evaluated

An array length or a const argument is computed while compiling. It can use numbers, const parameters, constants,
and `+ - * / % << >> & | ^`; a name that is none of those, an overflow, a division by zero, or an array length
outside 0 to 4294967295 cannot be computed. `spec/const-generics.md` CG4.

<!-- error-example: E0251 -->
```burdin
fn takes(values: [i32; 1 / 0]) {} //~ E0251

fn main() {}
```
<!-- /error-example -->

**How to fix:** use only numbers and constants, and keep the value in range.

## E0252: Const argument that cannot be inferred

A call to a generic function infers its const arguments from the argument types (`[T; N]` gives `N`) or from the
expected type. When nothing determines one, the call cannot be compiled. `spec/const-generics.md` CG6.

<!-- error-example: E0252 -->
```burdin
fn make<const N: usize>(): usize { N }

fn main() {
    let n = make() //~ E0252
    println("{}", n)
}
```
<!-- /error-example -->

**How to fix:** pass an argument or write a type annotation that fixes the value, for example an array of the
wanted length.

## E0260: Invalid build configuration

A package with a `build.bd` is a dependency of the entry package, but the entry package's manifest does not grant
it. Running a dependency's script is arbitrary code execution, so the entry package must name every dependency
script it trusts in `[build] allow`; an entry that names no dependency in the build is rejected too.
`spec/build-scripts.md` BS2.

<!-- error-example: E0260 target=app -->
```burdin
// --- app/burdin.toml ---
[package]
name = "app"

[dependencies]
lib = { path = "../lib" }

// --- app/src/main.bd ---
use lib.VALUE //~ E0260

fn main() {
    println("{}", VALUE)
}

// --- lib/burdin.toml ---
[package]
name = "lib"

// --- lib/build.bd ---
fn main() {
    let args = env.args()
    let out = args[2]
    match write_file("{out}/src/generated.bd", "const VALUE: i32 = 1\n") {
        Ok(_) => {}
        Err(_) => panic("cannot write the generated module"),
    }
}

// --- lib/src/lib.bd ---
pub use generated.VALUE
```
<!-- /error-example -->

**How to fix:** add the dependency's package name to the entry package's manifest, under `[build]`:
`allow = ["lib"]`. Grant only the scripts you are willing to run; the line is the whole permission.

## E0261: Build script failed

`build.bd` did not compile, could not be started, ended with a non-zero status, or declared an input that does not
exist. The script's working directory is its package, so a relative path in the script resolves there. The script's
own diagnostics come first, and E0261 names the package at the entry source. `spec/build-scripts.md` BS3 and BS5.

<!-- error-example: E0261 target=. -->
```burdin
// --- burdin.toml ---
[package]
name = "app"

// --- build.bd ---
fn main() {
    process.exit(3)
}

// --- src/main.bd ---
fn main() {} //~ E0261
```
<!-- /error-example -->

**How to fix:** make the script compile and finish successfully. A script that discovers an unusable environment
should print why and exit non-zero, which reports this error with its output above it.

## E0262: Invalid generated modules

A build script's generated files are modules of the package under `<out>/src/`. A generated path that is not a
Burdin identifier, or a module whose path repeats one of the package's source modules, cannot be loaded. Replacing a
source module with generated code is not allowed; a script only adds module names. `spec/build-scripts.md` BS4.

<!-- error-example: E0262 target=. -->
```burdin
// --- burdin.toml ---
[package]
name = "app"

// --- build.bd ---
fn main() {
    let args = env.args()
    let out = args[2]
    match write_file("{out}/src/main.bd", "fn main() {{}}\n") {
        Ok(_) => {}
        Err(_) => panic("cannot write the generated module"),
    }
}

// --- src/main.bd ---
fn main() {} //~ E0262
```
<!-- /error-example -->

**How to fix:** give the generated module a name the package does not already use, for example
`<out>/src/generated.bd`, and import it with that name.

## E0263: A declaration that cannot be exported to a C header

`burdin bindgen --export` writes a C header for the `extern "C" fn`s of a package, the `repr(C)` structs they reach,
and the constants those modules declare. Some declarations have no C form: a name C reserves (`int`, `double`) or
one that is not a C identifier, two declarations that would share a C name, a generic struct, a struct held by value
that is not `repr(C)`, or a field whose name C cannot declare. Every offending declaration is reported, and no header
is written. `spec/bindgen.md` BG10 and BG11.

<!-- error-example: E0263 target=. mode=export -->
```burdin
// --- burdin.toml ---
[package]
name = "app"

// --- src/main.bd ---
struct Loose {
    a: i32
}

struct Holder repr(C) {
    loose: Loose
}

extern "C" fn holder_value(own h: CPtr<Holder>): i32 { //~ E0263
    0
}

extern "C" fn int(own x: i32): i32 { //~ E0263
    x
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** rename the declaration to a C identifier C does not reserve (and keep C names unique across the
package), use a concrete struct instead of a generic one, mark a struct that C holds by value `repr(C)`, and rename
any field that is not a C identifier.

## E0264: Rust interop failed

A package binds a Rust crate through `[rust.crates]`: the driver reads the crate's API from rustdoc JSON, generates a
C-ABI shim and its Burdin module, builds the shim with the Rust toolchain the manifest pins, and links it. The step
stops with this error when the `[rust]` table is invalid (no `toolchain`, or one that is not an exact version), a
crate path holds no `Cargo.toml`, the installed toolchain is not the pinned one, `cargo` or rustdoc fails on the
crate, or the shim does not build. A shim also fails to build when the size, alignment, or field offset of an
exposed `repr(C)` type differs from the one the Burdin module declares, so a layout mismatch is an error and never
undefined behavior. The message names the crate and carries the tool's output. `spec/rust-interop.md` RI5-RI9.

<!-- error-example: E0264 target=. -->
```burdin
// --- burdin.toml ---
[package]
name = "app"

[rust]
toolchain = "1.100.0"

[rust.crates]
nowhere = { path = "no_such_crate" }

// --- src/main.bd ---
fn main() {} //~ E0264
```
<!-- /error-example -->

**How to fix:** point the crate's `path` at a directory with a `Cargo.toml`, pin an exact toolchain such as
`toolchain = "1.100.0"` and install it with `rustup toolchain install`, and fix the crate so `cargo doc` and
`cargo build` succeed. A layout failure means the crate changed a `repr(C)` type: rebuild so the module is generated
again.

## E0270: Not an allocator

The last type argument of `List<T, A>`, `Map<K, V, A>`, and `Heap<T, A>` is the allocator the container
allocates through, and the value passed to `new_in` is stored as that allocator. Both must implement the
`Allocator` interface from `std.alloc`; a type that does not, such as `i32`, cannot supply or store memory.
`spec/allocators.md` AL2-AL4.

<!-- error-example: E0270 -->
```burdin
fn takes(xs: List<i32, i32>) {} //~ E0270

fn main() {}
```
<!-- /error-example -->

**How to fix:** use `Global` (the default, written by leaving the argument out) or `Arena`, or implement
`Allocator` for your type with an `impl` block that provides `allocate`, `reallocate`, and `deallocate`.

## E0271: Invalid default type parameter

A struct's or enum's type parameter may end with a default (`struct Buffer<A: Allocator = Global>`), which a use
may omit. The defaulted parameters must be the trailing ones, because a use fills the trailing arguments; a
const parameter takes a value rather than a type; and a function's, an interface's, and a type alias's type
parameters take no default at all. Each of those is this error. `spec/allocators.md` AL5.

<!-- error-example: E0271 -->
```burdin
struct Bad<A = i32, B> { //~ E0271
    a: A
    b: B
}

fn main() {}
```
<!-- /error-example -->

**How to fix:** move every parameter with a default to the end of the list (`struct Pair<A, B = i32>`), and
declare defaults only on a struct or enum. A function's type arguments are inferred at each call, an impl writes
an interface's arguments, and an alias is expanded where it is used.

## E0272: Missing allocator

A container stores its allocator value when its allocator type is not `Global` (`List<i32, Arena>`,
`Heap<i32, Arena>`). The `new` constructor has no allocator value to store, so it exists only for the default
allocator; calling it for such a container is this error, and so is `from_iter` or `from`, which build a `List` or a
`Map` with the default allocator (`List<i32, Arena>.from_iter(it)`). `spec/allocators.md` AL4,
`spec/iterators.md` IT22.

<!-- error-example: E0272 -->
```burdin
fn main() {
    let xs: List<i32, Arena> = List<i32, Arena>.new() //~ E0272
}
```
<!-- /error-example -->

**How to fix:** construct the container with the `new_in` form and pass the allocator:
`List.new_in(arena)`, `Map.new_in(arena)`, or `Heap.new_in(arena, value)`. `new()` remains the constructor for
`List<T>`, `Map<K, V>`, and `Heap.new(value)`, which use `Global`. To fill such a container from an iterator, make it with
`new_in` and call `extend_iter`.

## E0280: Not `Send` or not `Sync`

A value crosses to another task or thread only when it is `Send`, and several tasks use one value at the same time
only when it is `Sync`. The compiler works both out from a type's parts: a record is `Send` when each field is, a raw
`CPtr<T>` is never either, and a `Shared<T>` is `Send` when `T` is both. A task or thread that moves a value which is
not `Send`, a scoped task that borrows one which is not `Sync`, a `Channel<T>` of a type that is not `Send`, and a call
whose type argument misses a `Send` or `Sync` bound are each this error. The message follows the chain of types down to
the field that decides it. `spec/shared.md` SH4-SH6.

<!-- error-example: E0280 -->
```burdin
struct Cursor {
    ptr: CPtr<u8>
}

fn main() {
    let c = Cursor { ptr: unsafe { CPtr.null() } }
    let t = task.spawn(|| unsafe { c.ptr.is_null() }) //~ E0280
}
```
<!-- /error-example -->

**How to fix:** keep the value on one task, or send plain data instead of the pointer. When the type really is safe
to move or share (it only reads what the pointer holds, say), say so with an `unsafe` impl block: `unsafe Cursor: Send { }`.
A generic function that passes its parameter to a task asks for it with a bound: `fn run<T: Send>(own value: T)`.

## E0281: Invalid `Send` or `Sync` implementation

`Send` and `Sync` are promises the compiler cannot check, so an implementation of either is written `unsafe` and
has an empty body. An implementation that is not marked `unsafe`, an `unsafe` implementation of any other interface,
and one with methods in its body are this error. `spec/shared.md` SH5.

<!-- error-example: E0281 -->
```burdin
struct Handle {
    cell: CPtr<i32>
}

Handle: Send { } //~ E0281

fn main() {}
```
<!-- /error-example -->

**How to fix:** write `unsafe Handle: Send { }` when the promise holds, or remove the implementation and let the
compiler derive the interface from the fields.

## E0290: Invalid `blocking` marker

`blocking` marks an `extern "C"` function declaration as one that may block its thread, so a call to it runs on the
helper pool and parks the calling task (`spec/io.md`, IO4). The marker is allowed only there, only on a declaration
whose parameters are fixed, and every declaration of one C symbol must agree about it.

<!-- error-example: E0290 -->
```burdin
blocking fn slow(): i32 { 0 } //~ E0290

fn main() {}
```
<!-- /error-example -->

**How to fix:** write a helper with an ordinary body when the call does not block, or declare the C function inside
an `extern "C" { ... }` block and mark the declaration there. A variadic declaration cannot be `blocking`, because
the thunk marshals a fixed argument list, and every declaration of one C symbol must agree about the marker.

## E0300: Misplaced shebang line

A `#!` line makes a `.bd` file a script: the first line `#!/usr/bin/env burdin run` tells the operating system which
program runs the file, and the compiler skips it (`spec/scripts.md`, SC2). The line is recognized only as the
file's very first line, byte zero, so a file that starts with anything else and later holds `#!` is E0300.

<!-- error-example: E0300 -->
```burdin
//! A helper with a stray shebang.
#!/usr/bin/env burdin run //~ E0300
fn main() {}
```
<!-- /error-example -->

**How to fix:** move the `#!` line to the very top of the file, or delete it. Burdin has no other use for `#`, so
the line is never a comment: a shebang below the first line is almost always a paste that landed one line too low.
