---
title: Compile-time code
description: const values, comptime expressions and blocks, comptime parameters, comptime if and for, comptime assert, and embed.
---

Burdin can run code while it compiles. The compiler evaluates a `comptime` expression, and the value it produces is placed
in the program as a literal. None of the evaluation reaches the generated code, so a table built this way costs exactly
the table and nothing else.

Compile-time code is pure: it cannot print, read input, touch the environment, look at the clock, or call C. That is
what makes the result depend only on the source, the target, and the profile.

## Constants

The initializer of a `const` runs at compile time. It may call functions, declare locals, and loop, so a small program
can build a value that the shipped binary only reads:

```burdin
fn squares(): [u32; 8] {
    let mut table = [0 as u32; 8]
    let mut i: usize = 0
    while i < 8 {
        table[i] = (i * i) as u32
        i = i + 1
    }
    table
}

const SQUARES: [u32; 8] = squares()

fn main() {
    println("the last square is {SQUARES[7]}")
}
```

Every constant is evaluated once, whether or not it is used, and a constant that refers to another is evaluated after it.
A constant that reaches itself through a call is `E0009`.

## comptime expressions

`comptime E` evaluates `E` during compilation. `comptime { ... }` does the same for a block, which is how a loop or a
few locals fit in one place:

```burdin
fn factorial(n: u64): u64 {
    if n == 0 { 1 } else { n * factorial(n - 1) }
}

fn main() {
    let table = comptime {
        let mut t = [0 as u64; 5]
        let mut i: usize = 0
        while i < 5 {
            t[i] = factorial((i + 1) as u64)
            i = i + 1
        }
        t
    }
    println("five factorials: {table[4]}")
    let big = comptime factorial(10)
    println("10! is {big}")
}
```

A compile-time expression sees the enclosing function only through its `comptime` parameters and `comptime for` bindings.
Any other local is a runtime value, and using one is `E0220`.

## comptime parameters

A parameter written `comptime name: Type` takes a value known while compiling. Each distinct value produces its own copy
of the function, so a condition on the parameter can decide what the copy contains:

```burdin
fn power(x: i64, comptime n: u32): i64 {
    comptime if n == 0 { 1 } else { x * power(x, n - 1) }
}

fn main() {
    println("2^10 = {power(2, 10)}")
    println("3^5 = {power(3, 5)}")
}
```

The copy for `n = 0` returns `1`; the copy for `n = 2` returns `x * power(x, 1)`; and so on, so the recursion ends while
the program is compiled instead of every time it runs. Inside the body the parameter is also an ordinary read-only value.

A comptime argument may be a literal, a constant, or a `comptime` value from the caller, and the parameter's type must be
one that can cross into the program (see [What can be a compile-time value](#what-can-be-a-compile-time-value)). Strings
and structs work too:

```burdin
struct Point: Copy, Clone, Eq {
    x: i32
    y: i32
}

fn tagged(comptime tag: str, message: str): String {
    let mut out = String.from("[")
    out.push_str(tag)
    out.push_str("] ")
    out.push_str(message)
    out
}

fn manhattan(comptime p: Point, q: Point): i32 {
    let dx = if q.x > p.x { q.x - p.x } else { p.x - q.x }
    let dy = if q.y > p.y { q.y - p.y } else { p.y - q.y }
    dx + dy
}

fn main() {
    println(tagged("warn", "low disk"))
    println("{manhattan(Point { x: 0, y: 0 }, Point { x: 3, y: 4 })}")
}
```

Only free functions, inherent methods, and extension methods may take `comptime` parameters; interface methods,
functions with a `comptime` parameter used as a value, and closures cannot (`E0220`). One function may have at most 1024
specializations, and more is `E0225`.

## comptime if

`comptime if` evaluates its condition during compilation and keeps only the branch it selects. The other branch is still
type-checked, but it is never evaluated and never emitted, so a platform decision costs nothing at run time:

```burdin
fn platform(): str {
    comptime if target.os() == target.Os.Linux {
        "Linux"
    } else if target.os() == target.Os.MacOs {
        "macOS"
    } else if target.os() == target.Os.Windows {
        "Windows"
    } else {
        "another system"
    }
}

fn main() {
    println("this program was built for {platform()}")
}
```

In an `else if` chain every condition is a compile-time condition. Because only one branch survives, a `comptime if`
around a call is also how a recursion through a `comptime` parameter stops.

## comptime for

`comptime for` runs its body once per item during compilation, and each copy becomes ordinary code. The binding is known
while compiling inside its copy, and at run time it is an immutable local:

```burdin
const WEIGHTS: [i32; 3] = [10, 20, 30]

fn main() {
    comptime for i in 0..3 as usize {
        println("field {i}: {comptime WEIGHTS[i]}")
    }
    let mut total = 0
    comptime for w in WEIGHTS {
        comptime assert(w % 10 == 0)
        total = total + w
    }
    println("total {total}")
}
```

The iterable is a range of integers, an array, or a `List` of values that can cross into the program. A `mut` binding is
`E0220`, and so is a `break` or `continue` that leaves the unrolled body: the copies are not a loop.

## comptime assert

`comptime assert(condition)` fails the build when the condition is false, which is how an assumption about constants is
checked once instead of at every run. A message may follow:

```burdin
const BUFFER_SIZE: usize = 256

comptime assert(BUFFER_SIZE % 64 == 0, "the buffer must be a whole number of cache lines")

fn main() {
    comptime assert(BUFFER_SIZE / 64 == 4)
    println("a buffer of {BUFFER_SIZE} bytes")
}
```

With a false condition the compiler reports `E0223` at the assertion and shows the message when there is one. The form
works in a body and as a module item.

## What can be a compile-time value

The result of a compile-time expression, a constant, and a `comptime` parameter is copied into the program, so its type
must be one that can be written there:

- `()`, `bool`, `char`, every integer and float type, `str`, and `String`;
- tuples and arrays of those types;
- structs and enums whose fields are all of those types;
- `Option<T>`, `Result<T, E>`, and `Range<T>` of those types.

`List`, `Map`, `Heap`, `any`, closures, references, slices, pointers, and iterators are not. Compile-time code may
create and use them, but a value of one cannot be the result that crosses into the program (`E0221`).

A `str` value is copied into a static string literal, so it is static wherever it is used; a `String` value becomes a
fresh `String.from(literal)` each time the materialized expression runs.

## Purity and limits

Compile-time code runs in the compiler, so it cannot do anything observable. Printing, reading input, files, the
environment, processes, the clock, the random seed, and C calls are all `E0224`. The check is dynamic: a function that
prints can be called at compile time as long as the path taken does not print.

Evaluation is also bounded. Each expression, statement, loop iteration, and call is one step, and every element a
`String`, `List`, or `Map` allocates is another. The budget is 10,000,000 steps per expression, per constant, and per
specialization; `burdin build`, `run`, and `test` take `--comptime-steps N` to change it. Going over is `E0225`, and so
is a call depth beyond 512 or more than 1024 specializations of one function.

A panic during evaluation stops the build with `E0222` at the compile-time expression, showing the panic message and
where it happened. Integer overflow follows the profile: it panics in a debug build and wraps in a release build.

## embed

`embed("path")` is the contents of a file, read while compiling. The path is relative to the source file that writes the
call, and it may not be absolute or contain `..`. The result is a `str` for text files, or the bytes as `[u8; N]` when
the expected type is an array of bytes:

```burdin fragment
const LICENSE: str = embed("LICENSE.txt")
const HEADER: [u8; 4] = embed("magic.bin")
```

A missing or unreadable file is `E0240`, and an invalid path or an array whose length does not match the file is
`E0241`. The compiler records every file an `embed` read, and `burdin build --emit-deps <file>` lists them, so a build
system can rebuild when one changes.

## Errors

| Code | Meaning |
| --- | --- |
| `E0220` | A runtime local, `self`, or a capture in a compile-time context; `comptime` where it cannot go; a `break` or `continue` leaving a `comptime for` |
| `E0221` | A value whose type cannot cross into the program |
| `E0222` | Compile-time evaluation panicked |
| `E0223` | A `comptime assert` failed |
| `E0224` | An impure operation during compile-time evaluation |
| `E0225` | The step budget, the call depth, or the specialization limit |
| `E0240` | `embed` could not read its file, or the file is not valid UTF-8 |
| `E0241` | An invalid `embed` call: a path that is not a literal, is absolute, or has `..`; an array of the wrong length |
| `E0009` | A constant that depends on itself through calls |

## Generating declarations

The same engine can also add declarations to a module: a module-scope `comptime generate { ... }` block returns source
text, and the compiler adds the structs, enums, and functions that text declares. The exact behavior is
[`spec/generate.md`](https://github.com/RicardoMilos7/burdin/blob/main/spec/generate.md).
