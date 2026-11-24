---
title: Global variables
description: static and static mut, the values that live for the whole program, the rules for reading and writing them, and why a mutable one needs unsafe.
---

A `static` is a variable that belongs to a module rather than to a function. There is one copy for the whole program, it
holds its value before `main` runs, and every function in the module can name it. Write it like a `const`, with `static`
in place of `const`:

```burdin
static GREETING: str = "hi"
static PRIMES: [i32; 4] = [2, 3, 5, 7]

fn main() {
    println("{GREETING} {PRIMES[2]}")
}
```

A `static` is read-only unless it is written `static mut`, which is covered below.

## The initial value

The initial value is computed at compile time, by the same rules as a `const`: it may call functions and use constants,
but it may not name another static. A function that builds a table can fill a static:

```burdin
fn squares(): [u32; 4] {
    let mut table = [0 as u32; 4]
    let mut i: usize = 0
    while i < 4 {
        table[i] = (i * i) as u32
        i = i + 1
    }
    table
}

static SQUARES: [u32; 4] = squares()

fn main() {
    println("the last square is {SQUARES[3]}")
}
```

## Immutable statics

An immutable `static` can be read but not assigned. Reading a `Copy` value copies it. Borrowing one with `&` gives a
view that lives for the whole program, so a function may return it:

```burdin
static TABLE: [i32; 3] = [5, 6, 7]

fn second(): &i32 {
    &TABLE[1]
}

fn main() {
    let value = *second()
    println("{value}")
}
```

Assigning to an immutable static is `E0005`, as assigning to an immutable variable is. Moving a value that is not `Copy`
out of a static is `E0101`: a static is never emptied, so its value must be copied or borrowed.

## Mutable statics

A `static mut` can be read and written, but only inside an `unsafe` block or an `unsafe` function. Outside one, the
access is `E0161`, the same as calling an unsafe function:

```burdin
static mut COUNT: i32 = 0

fn bump(by: i32) {
    unsafe {
        COUNT = COUNT + by
    }
}

fn main() {
    bump(1)
    bump(2)
    let now = unsafe { COUNT }
    println("{now}")
}
```

The `unsafe` keyword marks each function that touches a mutable static. The checker looks at one function at a time, so
it relies on that marking rather than on seeing every other function. A mutable static cannot be borrowed: `&COUNT`
and `&mut COUNT` are `E0304`, so no reference can point at it and change under you. Copy a value out with
`unsafe { COUNT }`, or write into it.

## Tasks

Several tasks may read an immutable static at the same time. A `static mut` is shared the way raw memory is: two tasks
that write it at once race, and keeping them from doing so is your job inside the `unsafe` blocks.

## Limits

- **The type must be plain data.** A static is never dropped, so its type may own no memory and have no destructor.
  `str`, scalars, tuples, arrays, and structs, enums, `Option`, and `Result` of those are allowed. A `String`, `List`,
  `Map`, `Shared`, a closure, an `any`, or a type with a `Drop` implementation is `E0303`.
- **Statics are module items.** A `static` declared inside a function body is `E0302`.
- **Compile-time code cannot read a static.** A `const` initializer or a `comptime` block that names one is `E0220` and
  `E0224`, and a static cannot give an array length, which is `E0251`.
- **Atomics and `Mutex` cannot be statics yet.** They cannot be built at compile time, so
  `static COUNT: AtomicI64 = ...` is `E0221`. Pass a counter to the tasks that share it instead, as a `Shared<T>` or an
  atomic value they receive.
- **`burdin test --interpret` skips tests that use a static.** The interpreter does not model them.

| Code | Meaning |
| --- | --- |
| `E0005` | An assignment to an immutable static |
| `E0101` | A value that is not `Copy` moved out of a static |
| `E0161` | A `static mut` read or written outside `unsafe` |
| `E0302` | A `static` declared in a function body |
| `E0303` | A static whose type owns memory or has a destructor |
| `E0304` | A borrow of a `static mut` |
| `E0220` | A static named in a constant's initializer |
| `E0221` | A static whose type cannot be built at compile time |
| `E0224` | A static read during compile-time evaluation |
| `E0251` | A static used where a constant is needed, such as an array length |

The full rules, with their rule numbers and the tests that check them, are in
[`spec/globals.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/globals.md).
