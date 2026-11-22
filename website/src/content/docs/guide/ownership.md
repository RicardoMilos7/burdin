---
title: Ownership and references
description: Moves, copies, the read-only default, mut and own, references, exclusive borrows, views, and drops.
---

Burdin manages memory without a garbage collector, and it does so with rules that the compiler checks before the program
runs. This chapter is the most important one in the guide. Once the rules make sense, most compiler errors will read as
plain statements about the code.

There are three ideas:

1. **Every value has one owner.** When the owner goes out of scope, the value is dropped (its memory and its resources are
   released).
2. **Using a value moves it or copies it.** Non-`Copy` values move: after the move, the old name is no longer usable.
3. **Other code can look at a value without owning it.** Those are *borrows*, and while a borrow is in use the value cannot
   be changed out from under it.

## Moves

Assigning a `String` to another name moves it. The first name no longer refers to anything:

```burdin fragment
fn main() {
    let a = String.from("hello")
    let b = a
    println(b)
    println(a)
}
```

The compiler stops with this error:

```text
main.bd:5:13: error[E0100]: use of moved value `a`
  = note: main.bd:2:9: `a` is declared here
  = note: main.bd:3:13: value moved here
  = help: run `burdin check --explain` to trace the path to this error
  = help: clone `a` where it is moved, so this use has its own value
          - let b = a
          + let b = a.clone()
error: could not compile main.bd (1 error)
note: for an explanation of an error code, run `burdin explain <code>`
```

The fix depends on what you meant. If both names need the value, make a copy with `.clone()`: `let b = a.clone()`. If
`b` should be the only owner, use `b` and forget `a`.

## Copy types

Some types are copied instead of moved, so the old name keeps working. Integers, floats, `bool`, `char`, tuples and arrays
of copied types, and references are all `Copy`. A struct or enum becomes `Copy` when it derives it, and when all of its
fields are `Copy`:

```burdin
struct Point: Copy, Clone {
    x: i32
    y: i32
}

fn main() {
    let p = Point { x: 1, y: 2 }
    let q = p
    println("{p.x} {q.x}")
}
```

A type that owns heap memory, such as `String`, `List<T>`, or a struct with a `String` field, is never `Copy`.

## Parameters borrow by default

A parameter without a marker is a *read-only borrow* of the caller's value. The function can read it, and the caller can
use the value again afterwards:

```burdin
struct Counter {
    n: i32
}

fn peek(c: Counter): i32 {
    c.n
}

fn main() {
    let c = Counter { n: 1 }
    println("{peek(c)}")
    println("{peek(c)}")
}
```

Two markers change that, and both are visible at the call site:

| Parameter | Call | Meaning |
| --- | --- | --- |
| `mut c: Counter` | `add_to(mut c, 4)` | The function may change `c`. The caller passes it with `mut`, and the variable must be declared `let mut`. |
| `own c: Counter` | `consume(c)` | The function takes ownership. The caller's name is moved into the call. |

```burdin
struct Counter {
    n: i32
}

fn add_to(mut c: Counter, amount: i32) {
    c.n = c.n + amount
}

fn consume(own c: Counter): i32 {
    c.n * 10
}

fn main() {
    let mut c = Counter { n: 1 }
    add_to(mut c, 4)
    println("{c.n}")
    println("{consume(c)}")
}
```

The `mut` marker at the call site is not decoration. It tells a reader that this call may change `c`, so the code is easy
to audit. Forgetting it is an error with a suggestion.

If a function takes a value with `own` and you pass a value you still need, the compiler reports it as a move:

```text
main.bd:12:15: error[E0100]: use of moved value `c`
  = note: main.bd:10:9: `c` is declared here
  = note: main.bd:11:23: value moved here
  = help: run `burdin check --explain` to trace the path to this error
error: could not compile main.bd (1 error)
note: for an explanation of an error code, run `burdin explain <code>`
```

The same rules apply to methods. `self` is read-only, `mut self` can change the value, and `own self` consumes it. See
[structs and methods](/guide/structs-and-methods/).

## References

A reference is a name for a value that someone else owns. `&x` is a read-only reference, and `&mut x` is a mutable one.
Read through a reference with `*`, and write through a mutable one the same way:

```burdin
fn main() {
    let mut x = 10
    let r = &x
    println("{*r}")
    let m = &mut x
    *m = *m + 5
    println("{x}")
}
```

A reference lasts only as long as it is used. Once `r` has been read for the last time, `&mut x` may follow. That is
why the example above compiles.

## One writer or many readers

While a borrow is live, the value it refers to cannot be changed in a way that would invalidate the borrow. A read-only
borrow allows other reads. A mutable borrow allows no other access until it ends:

```burdin fragment
fn main() {
    let mut v = [1, 2, 3]
    let first = &v[0]
    v[1] = 9
    println("{*first}")
}
```

```text
main.bd:4:5: error[E0111]: this access conflicts with a borrow that is still
    live
  = note: main.bd:3:18: the borrow is created here
  = help: run `burdin check --explain` to trace the path to this error
error: could not compile main.bd (1 error)
note: for an explanation of an error code, run `burdin explain <code>`
```

The borrow `first` is used after the write, so it is still live at the write. Move the read above the write, or read the
value into a plain `let` and use that.

## Views: str and slices

`str` and `[T]` are views. They look at text or elements owned somewhere else, and they cannot be stored in a struct or
returned from a function that has no owner for them. A function may return a view derived from one of its parameters:

```burdin
fn first(xs: [i32]): &i32 {
    &xs[0]
}

fn main() {
    let v = [4, 5]
    println("{*first(v[0..2])}")
}
```

A function that returns a view must take it from exactly one parameter. Burdin has no lifetime annotations, so the
checker reads one function at a time, from its signature alone. If the result could come from two parameters, the
compiler does not know which one, and it refuses:

```burdin fragment
fn pick(a: str, b: str): str {
    if a.len() > b.len() {
        a
    } else {
        b
    }
}
```

```text
main.bd:1:1: error[E0121]: this returns views of more than one parameter (`a`,
    `b`); a second-class result must derive from exactly one parameter
  = note: main.bd:1:9: `a` is one of the sources of this result
  = note: main.bd:1:17: `b` is one of the sources of this result
  = help: run `burdin check --explain` to trace the path to this error
error: could not compile main.bd (1 error)
note: for an explanation of an error code, run `burdin explain <code>`
```

The fix is to return an owned `String`, which you build from the two inputs, or to take the inputs one at a time. A
reference to a local variable cannot be returned at all, because the local is dropped when the function returns. The
compiler reports that as E0122.

## Drops

When a value goes out of scope, its drop runs. A type that owns memory or a resource gets that cleanup automatically. A
type can also define its own `Drop` implementation, which runs when a value of that type is dropped:

```burdin
struct Noisy {
    name: String
}

Noisy: Drop {
    fn drop(mut self) {
        println("drop {self.name}")
    }
}

fn consume(own n: Noisy) {
    println("consuming {n.name}")
}

fn main() {
    let a = Noisy { name: String.from("a") }
    let b = Noisy { name: String.from("b") }
    {
        let c = Noisy { name: String.from("c") }
        println("inner {c.name}")
    }
    consume(b)
    println("end of main")
}
```

The program prints each drop as it happens: `c` at the end of its block, `b` when `consume` finishes, and `a` last, when
`main` returns. Values are dropped in the reverse of the order they were created. A moved value is not dropped by the
name it was moved from; it is dropped by its new owner.

## Heap memory

Heap memory is spelled `Heap<T>`. `Heap.new(value)` moves the value onto the heap and returns a box, and `*box` reads the
value out. The box is dropped with its owner, so memory is never freed twice and never leaked by accident:

```burdin
fn main() {
    let b = Heap.new(41)
    println("{*b + 1}")
}
```

Recursive types such as trees need a `Heap` for the recursive part. See [enums and match](/guide/enums-and-match/).

## Shared ownership

Sometimes several owners need one value. `Shared<T>` is a reference-counted handle. Each `clone()` makes another handle to
the same value, and the value is dropped when the last handle goes:

```burdin
fn main() {
    let a = Shared.new(String.from("shared"))
    let b = a.clone()
    println("count {a.strong_count()} value {b.get()}")
}
```

Use `get()` to borrow the value. A shared value cannot be changed through `get()`. To change a value that several tasks
share, put a `Mutex<T>` inside the `Shared`, as the [concurrency](/guide/concurrency/) chapter shows.

## The short version

- Assigning or passing a non-`Copy` value moves it. Use `.clone()` to keep both.
- A parameter is read-only. `mut` lets a function change it, and the call says `mut`. `own` takes it.
- `&` and `&mut` borrow. A borrow is live until its last use, and a live borrow blocks conflicting access.
- A function can return a view only of one parameter, and never of a local.
