# Global variables: `static` and `static mut`

Burdin has global variables, declared with `static` (issue #485, decision 36 in `tests/README.md`). A `static` is one
value that lives for the whole program, in one place, and every function names it. `static mut` is the same, and may
also be written. This page is the rule set; the tests named at the end witness each rule.

The design keeps the checker local (`CLAUDE.md`), so a global's accesses are checked where they happen, without
whole-program analysis. A read or write of a `static mut` is therefore an `unsafe` operation, as a raw pointer is:
the function that touches it says so, and the checker does not have to see every other function to decide it is safe.

## GL1. Declaration and initial value

A static is an item at module scope, optionally `priv`:

```
static NAME: Type = expr
static mut NAME: Type = expr
```

The initializer is evaluated at compile time by the rules of a `const` (`const-eval.md`, CE2): it may call pure
functions and use constants, but not other statics or run-time locals. The program stores its value into the static
before `main` runs (and before a `burdin test` body runs), in declaration order. Storing a value runs no user code and
cannot fail, and no initializer can name another static, so the order is not observable. An initializer that names a
static is E0220. A `static` in a function body is E0302: statics are module items.

## GL2. The type must be plain data

A static's type must be materializable (`const-eval.md`, CE3; else E0221) and **plain**: it owns no memory and has no
destructor. Plain types are the scalars, `bool`, `char`, `str` (a static view), tuples, arrays, and structs and enums
whose fields are plain, `Option`, `Result`, and ranges of plain types. A `String`, a `List`, `Map`, `Heap`, `Shared`,
an `Arena`, a closure, an `any`, or any type with a `Drop` implementation is E0303, because the program would have
to drop it at exit, and GL5 says statics are never dropped.

## GL3. An immutable `static`

`static NAME` is read-only.

* A read of a `Copy` type copies the value. A read of any other plain type moves out of the static, which is E0101.
* A borrow of it, `&NAME` or a method that takes `self` by reference, is a **static view**: it is valid for the whole
  program, so it may be returned from any function, as a string literal is (RT4).
* An assignment to it is E0005, as an assignment to an immutable binding.
* Any task may read it at the same time as another, so its type must be `Sync` (SH4). Every type GL2 admits is `Sync`
  already (the scalars, `str`, and the plain aggregates of them), so the declaration needs no check of its own. A raw
  pointer, a reference, a closure, or a handle to shared state is not plain or not materializable, so GL2 or CE3
  rejects it first.

A static cannot yet hold interior mutability. The atomics and `Mutex<T>` are not materializable (CE3), so
`static COUNT: AtomicI64 = AtomicI64.new(0)` is E0221. Sharing a counter between tasks through a static waits for a
design that lets those types be initialized at compile time; until then a counter is passed to the tasks as a
`Shared<T>` or an atomic value they receive.

## GL4. A mutable `static mut`

`static mut NAME` may be read and written, but only in an `unsafe` context: an `unsafe` block or an `unsafe fn`. A
read or write outside one is E0161, as calling an unsafe function is.

* A read copies a `Copy` type, and a write stores into the whole static or a field or element of it. Reading a
  non-`Copy` value out of it is E0101.
* Its value may not be borrowed: `&NAME`, `&mut NAME`, and a method call that takes `self` by reference are E0304.
  No reference can then alias it, so the expression that reads or writes it never conflicts with another.
* It has no `Sync` requirement. Two tasks that write it at once race, and that race is the program's to avoid inside
  its `unsafe` blocks, as with any shared raw memory.

## GL5. Nothing is dropped

A static is never dropped. The program exits without running destructors for statics, and GL2 guarantees that there
is none to run.

## GL6. Tasks and threads

An immutable static is read by any task (GL3). A `static mut` is read or written by a task inside an `unsafe` block
(GL4). A static is not a capture: a closure that names one captures nothing for it, so it needs no `Send` bound.

## GL7. Compile-time evaluation and the interpreter

Reading or writing a static during compile-time evaluation, from a `const` initializer, a compile-time condition, or a function
they call, is E0224: global state is not pure. Naming a static where a constant is needed (an array length, a const
argument) is E0251, since it is not a constant.

`burdin test --interpret` does not model statics: a test that reads or writes one is skipped with the reason
(`spec/test-runner.md`, IN1), as it is for a C call.

## GL8. Diagnostics

| Code | Meaning |
| --- | --- |
| E0302 | A `static` declaration in a function body (GL1) |
| E0303 | A static whose type is not plain data: it owns memory or has a destructor (GL2) |
| E0304 | A borrow of a `static mut`, whether shared, mutable, or a method's `self` (GL4) |

The other rules reuse the codes their binding and interface counterparts already have: E0005 (GL3), E0101 (GL3, GL4),
E0161 (GL4), E0220 and E0224 (GL1, GL7), E0221 (GL2), and E0251 (GL7).

## Tests

- `m20_const/static_values`: immutable statics of scalars, `str`, tuples, arrays, structs, and enums, read from
  functions and initialized from constants and a block initializer (GL1, GL3).
- `m20_const/static_mut`: a `static mut` counter written in `unsafe` blocks and read back (GL4).
- `m20_const/static_view`: `&NAME` of an immutable static returned from a function (GL3).
- `s2_tasks/static_in_task`: two tasks read a static (GL3, GL6).
- `m20_const/static_in_block`: E0302 (GL1).
- `m20_const/static_string`: E0303 for a `String` (GL2).
- `m20_const/static_drop`: E0303 for a struct with a `Drop` implementation (GL2).
- `m20_const/static_assign`: E0005 for an assignment to an immutable static (GL3).
- `m20_const/static_move`: E0101 for moving a non-`Copy` immutable static out (GL3).
- `m20_const/static_mut_safe`: E0161 for a read and a write outside `unsafe` (GL4).
- `m20_const/static_mut_borrow`: E0304 for `&` and `&mut` of a `static mut` (GL4).
- `m20_const/static_in_const`: E0224 for a static read in a `const` initializer (GL7).
- `m20_const/static_const_init`: E0220 for a static in a constant's initializer (GL1).
- `m20_const/static_array_len`: E0251 for a static used as an array length (GL7).
