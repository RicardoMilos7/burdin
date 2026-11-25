---
title: Compile-time code
description: const values, if with a compile-time condition, reflection loops, and embed.
---

Burdin evaluates some code while it compiles. The initializer of a `const` runs in the compiler, and the value it
produces is placed in the program as a literal, so a table built this way costs exactly the table and nothing else. An
`if` whose condition is known while compiling keeps only the branch it takes, and `embed` reads a file into the program.

Compile-time code is pure: it cannot print, read input, touch the environment, look at the clock, or call C. That is
what makes the result depend only on the source, the target, and the profile.

:::note
Earlier versions had `comptime` expressions, parameters, `comptime if`, `comptime for`, `comptime assert`, and
`comptime generate`. They were removed, and `comptime` is an ordinary word again. A macro system is planned to cover what
they did; until then, a `const` and the forms below are the compile-time tools.
:::

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

## An `if` with a compile-time condition

An `if` whose condition is built only from literals, constants, operators, enum variants, and the functions of the
`target` module (and the `features` module of a package) keeps only the branch it takes in the generated program. The
other branch is still type-checked, but it is never emitted, so a platform decision costs nothing at run time:

```burdin
fn platform(): str {
    if target.os() == target.Os.Linux {
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

There is no special syntax: any other `if`, including one on a local or a call of your own function, is an ordinary
run-time `if`. An `if let` is never a compile-time condition. Because both branches are checked, the code for every
platform must compile on every platform.

## Looping over fields and variants

A `for` over `Type.fields` or `Type.variants` is unrolled while compiling: the body is repeated once per item, and
`item.name`, `item.type_name`, `item.index`, and `item.arity` are literals in each copy. That is how a derive visits every
field. The rule is syntactic: the iterable is written `Type.fields` or `Type.variants`; to loop over the array at run
time, bind it first (`let fields = Point.fields`). A `break` or `continue` that leaves an unrolled loop is `E0220`,
because the copies are not a loop. The [reflection rules](https://github.com/RicardoMilos7/burdin/blob/main/spec/reflection.md)
have the details.

## What can be a compile-time value

The value of a constant is copied into the program, so its type must be one that can be written there:

- `()`, `bool`, `char`, every integer and float type, `str`, and `String`;
- tuples and arrays of those types;
- structs and enums whose fields are all of those types;
- `Option<T>`, `Result<T, E>`, and `Range<T>` of those types.

`List`, `Map`, `Heap`, `any`, closures, references, slices, pointers, and iterators are not. A constant's initializer may
create and use them, but a value of one cannot be the constant itself (`E0221`).

A `str` value is copied into a static string literal, so it is static wherever it is used; a `String` value becomes a
fresh `String.from(literal)` each time the constant is used.

## Purity and limits

Compile-time code runs in the compiler, so it cannot do anything observable. Printing, reading input, files, the
environment, processes, the clock, the random seed, and C calls are all `E0224`. The check is dynamic: a function that
prints can be called at compile time as long as the path taken does not print.

Evaluation is also bounded. Each expression, statement, loop iteration, and call is one step, and every element a
`String`, `List`, or `Map` allocates is another. The budget is 10,000,000 steps per constant; `burdin build`, `run`, and
`test` take `--const-steps N` to change it. Going over is `E0225`, and so is a call depth beyond 512.

A panic during evaluation stops the build with `E0222` at the constant, showing the panic message and where it happened.
Integer overflow follows the profile: it panics in a debug build and wraps in a release build.

## embed

`embed("path")` is the contents of a file, read while compiling. The path is relative to the source file that writes the
call, and it may not be absolute or contain `..`. The result is a `str` for text files. With the expected type
`[u8; N]` it is the bytes of a file of exactly that length, and with the expected type `[u8]` it is a static byte view
of the file, so the length is never written (a constant may not have that type, so bind the view in a function or a `let`):

```burdin fragment
const LICENSE: str = embed("LICENSE.txt")
const HEADER: [u8; 4] = embed("magic.bin")
fn payload(): [u8] {
    embed("payload.bin")
}
```

A missing or unreadable file is `E0240`, and an invalid path or an array whose length does not match the file is
`E0241`. The compiler records every file an `embed` read, and `burdin build --emit-deps <file>` lists them, so a build
system can rebuild when one changes.

## Errors

| Code | Meaning |
| --- | --- |
| `E0220` | A `break` or `continue` leaving an unrolled `for` over `Type.fields` or `Type.variants`; a `static` named in a constant |
| `E0221` | A constant whose type cannot cross into the program |
| `E0222` | Compile-time evaluation panicked |
| `E0224` | An impure operation during compile-time evaluation |
| `E0225` | The step budget or the call depth |
| `E0240` | `embed` could not read its file, or the file is not valid UTF-8 |
| `E0241` | An invalid `embed` call: a path that is not a literal, is absolute, or has `..`; an array of the wrong length |
| `E0009` | A constant that depends on itself through calls |
