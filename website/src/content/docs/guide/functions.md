---
title: Functions
description: Declaring functions, parameters, return values, default and named arguments, and main.
---

A function is declared with `fn`, a name, parameters with types, and an optional return type after a colon:

```burdin
fn add(a: i32, b: i32): i32 {
    a + b
}

fn main() {
    println("{add(2, 3)}")
}
```

The last expression of the body is the return value, so there is no `return` and no semicolon. Use `return` to leave early:

```burdin
fn first_negative(xs: [i32]): Option<i32> {
    for x in xs {
        if x < 0 {
            return Option.Some(x)
        }
    }
    Option.None
}
```

A function without a return type returns nothing, and calling it gives `()`, the empty value.

## Parameters are read-only by default

A parameter is a read-only view of the caller's value. The function can read it, and the caller keeps it afterwards:

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

To let a function change a parameter, mark the parameter `mut` and mark the argument `mut` at the call site. To take
ownership, mark the parameter `own`. Those rules are the subject of the [ownership](/guide/ownership/) chapter.

```burdin
fn bump(mut n: i32) {
    n = n + 1
}

fn main() {
    let mut count = 10
    bump(mut count)
    println("{count}")
}
```

## Default and named arguments

A parameter can have a default value after `=`. The caller can leave it out, or name it:

```burdin
fn connect(host: str, port: i32 = 8080): String {
    "{host}:{port}"
}

fn main() {
    println(connect("localhost"))
    println(connect("localhost", port: 9000))
}
```

A named argument uses the parameter's name and a colon. Positional arguments come first, and once one argument is named,
every later argument must be named too. Named arguments may be written in any order and are evaluated left to right in
the order they are written. Default values are evaluated once at compile time, like `const` items, so they can call
pure functions or build structs and enums.

## Arguments take the parameter's type

An argument can start with a dot. It then takes the type of its parameter, so a struct can be passed without naming it:
if `distance` takes two `Point` values, `distance(.{ x: 1, y: 1 }, .{ x: 4, y: 3 })` builds both. The
[structs and methods](/guide/structs-and-methods/#leaving-out-the-type-name) chapter lists the places where the type comes
from.

## Recursion

A function can call itself. Each call has its own parameters and its own locals:

```burdin
fn factorial(n: u64): u64 {
    if n <= 1 {
        1
    } else {
        n * factorial(n - 1)
    }
}

fn main() {
    println("{factorial(10)}")
}
```

## main

Every program starts at `fn main()`. It can take no arguments, or it can return a `Result`, so that an error ends the
program with a message:

```burdin
fn main(): Result<()> {
    let n = i32.parse("12")?
    println("parsed {n}")
    Result.Ok(())
}
```

If `main` returns `Result.Err`, the program prints the error and exits with a failure status. The [errors](/guide/errors/)
chapter explains `Result` and `?`.

## Methods

A function declared inside a type's block is a method, and it is called with a dot. The first parameter is `self`. The
[structs and methods](/guide/structs-and-methods/) chapter covers them.
