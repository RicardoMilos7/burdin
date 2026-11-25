---
title: Errors and absence
description: Option, Result, the ? operator, error enums with messages, main returning Result, and panics.
---

Burdin has no exceptions and no null. A value that might be missing is an `Option<T>`. An operation that might fail
returns a `Result<T, E>`. The compiler makes you handle both, and a small set of operators keeps that handling short.

## Option: a value that may be missing

`Option<T>` is either `Option.Some(value)` or `Option.None`:

```burdin
fn find_even(xs: [i32]): Option<i32> {
    for x in xs {
        if x % 2 == 0 {
            return Option.Some(x)
        }
    }
    Option.None
}

fn main() {
    let xs = [1, 3, 4]
    match find_even(xs[0..3]) {
        Option.Some(n) => println("found {n}"),
        Option.None => println("no even number"),
    }
}
```

When you want a default, write `orelse` after the expression. It gives the value inside `Option.Some`, or the value after
`orelse` for `Option.None`:

```burdin
fn find_even(xs: [i32]): Option<i32> {
    for x in xs {
        if x % 2 == 0 {
            return Option.Some(x)
        }
    }
    Option.None
}

fn main() {
    let xs = [1, 3, 5]
    let n = find_even(xs[0..3]) orelse -1
    println("{n}")
}
```

`unwrap()` takes the value out of `Option.Some` and panics on `Option.None`. `expect("message")` does the same and prints your message.
Use them when a missing value is a bug, not a normal case. `is_some()` and `is_none()` ask without taking the value.

`unwrap_or_default()` gives the value, or the type's default when there is none. `flatten()` turns an
`Option<Option<T>>` into an `Option<T>`.

## Result: an operation that may fail

`Result<T, E>` is `Result.Ok(value)` or `Result.Err(error)`. A function that can fail says so in its return type:

```burdin
enum MathError {
    DivideByZero
}

fn divide(a: i32, b: i32): Result<i32, MathError> {
    if b == 0 {
        Result.Err(MathError.DivideByZero)
    } else {
        Result.Ok(a / b)
    }
}

fn main() {
    match divide(10, 0) {
        Result.Ok(v) => println("{v}"),
        Result.Err(MathError.DivideByZero) => println("cannot divide by zero"),
    }
}
```

## The ? operator

Inside a function that returns `Result` or `Option`, a `?` after an expression means: if it is a failure, return that
failure from this function now; otherwise, use the value. This replaces a `match` that only passes errors along:

```burdin
fn parse_pair(a: str, b: str): Result<i32, ParseIntError> {
    let x = i32.parse(a)?
    let y = i32.parse(b)?
    Result.Ok(x + y)
}

fn main() {
    match parse_pair("4", "x") {
        Result.Ok(sum) => println("sum {sum}"),
        Result.Err(e) => println("failed: {e}"),
    }
}
```

`?` works only in a function whose return type can carry the failure. It also converts the error when the error types
differ, if the error type has a `From` conversion for the source error. Error enums are the usual way to provide one.

## Error enums

An `error enum` is an enum whose variants carry a message. The message is the text the variant prints, and `{0}` or a field
name puts a value into it:

```burdin
error enum ConfigError {
    Missing(String) "missing key {0}"
    Range { key: String, value: i32 } "{key} is out of range: {value}"
    Parse(ParseIntError) from "not a number: {0}"
}
```

The `from` keyword makes `?` convert a `ParseIntError` into a `ConfigError.Parse` automatically. Printing an error value
prints its message, so `println("{e}")` writes `missing key port`. The [bank example](/examples/bank-errors/) uses an error
enum with `?` from start to finish.

## Any error: AnyError

`Result<T>` with one type argument means `Result<T, AnyError>`: any error type can be returned through it. `AnyError`
keeps the original error, and `context` adds a message in front of it. Printing the error prints the messages from the
outside in, then the original error:

```burdin
error enum ConfigError {
    Missing(String) "missing key {0}"
}

fn read_key(own key: String): Result<String, ConfigError> {
    Result.Err(ConfigError.Missing(key))
}

fn load(): Result<String> {
    let value = read_key(String.from("port")).context("loading settings")?
    Result.Ok(value)
}

fn main() {
    match load() {
        Result.Ok(v) => println("value {v}"),
        Result.Err(e) => println("{e}"),
    }
}
```

The program prints `loading settings: missing key port`. The `Error` name is the interface that marks an error type. It
is not the dynamic value, which is `AnyError`.

## main that returns Result

`main` can return `Result<()>`. Then `?` works in `main`, and an error ends the program:

```burdin
error enum AppError {
    Bad(String) "bad input: {0}"
}

fn check(n: i32): Result<i32, AppError> {
    if n < 0 {
        Result.Err(AppError.Bad(String.from("negative")))
    } else {
        Result.Ok(n)
    }
}

fn main(): Result<()> {
    let n = check(5)?
    println("ok {n}")
    let m = check(-1)?
    println("not reached {m}")
    Result.Ok(())
}
```

The program prints `ok 5`, then prints the error message to standard error with a trace, and exits with a failure status.

The return type can also be a type alias of a `Result` declared in the same file:

```burdin
type Outcome = Result<()>

fn main(): Outcome {
    println("hello")
    Result.Ok(())
}
```

## Panics

Some failures are bugs, and Burdin treats them that way. A panic stops the program, prints a message and a backtrace, and
exits with a failure status. These are panics:

- indexing past the end of an array or list
- integer overflow in a debug build, and division by zero
- `unwrap()` on `Option.None` or on an `Result.Err`
- an explicit `panic("message")`

Use `Result` for failures you expect, such as bad input or a missing file. Use a panic when continuing would hide a bug.
