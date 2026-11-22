---
title: Coming from Python
description: How Python's everyday code maps onto Burdin, with the types, the integers, and the errors made explicit.
---

Python programmers will find Burdin's reading experience familiar: interpolated strings, `for` loops over collections,
small functions, and little ceremony. What changes is that every value has a type the compiler knows before the program
runs, and that failures are values you handle, not exceptions that unwind the stack.

## The same program, both ways

Python:

```python
def count_evens(numbers):
    total = 0
    for n in numbers:
        if n % 2 == 0:
            total += 1
    return total

print(f"{count_evens([1, 2, 3, 4])} even numbers")
```

Burdin:

```burdin
fn count_evens(numbers: [i32]): i32 {
    let mut total = 0
    for n in numbers {
        if n % 2 == 0 {
            total = total + 1
        }
    }
    total
}

fn main() {
    let numbers = [1, 2, 3, 4]
    println("{count_evens(numbers[0..4])} even numbers")
}
```

The structure is the same. The differences: a function declares its parameter types and its return type, `+=` is written
out, the list is a fixed array that is viewed as a slice with `[0..4]`, and the f-string prefix is not needed because every
string literal with braces is interpolated.

## Vocabulary

| Python | Burdin | Notes |
| --- | --- | --- |
| `def f(x):` | `fn f(x: i32): i32 { ... }` | Types and the return type are declared. |
| `print(x)` | `println(x)` | |
| `f"{x}"` | `"{x}"` | Every string literal interpolates. |
| `x = 1` | `let mut x = 1` | Names are immutable unless declared `mut`. |
| `x += 1` | `x = x + 1` | No compound assignment. |
| `True`, `False` | `true`, `false` | |
| `None` | `None` (`Option<T>`) | Use `Option<T>` when a value may be missing. |
| `if`, `elif`, `else` | `if`, `else if`, `else` | |
| `and`, `or`, `not` | `&&`, `\|\|`, `!` | |
| `for i in range(n)` | `for i in 0..n` | |
| `for i, x in enumerate(xs)` | `for (i, x) in xs.iter().enumerate()` | |
| `while cond:` | `while cond { }` | |
| `list` | `List<T>` | Declared with `List<T>.new()`. |
| `dict` | `Map<K, V>` | Order is unspecified; sort the keys to print. |
| `set` | `Set<T>` | |
| `tuple` `(a, b)` | `(a, b)` | Same. Index with `t.0`, or destructure. |
| `len(xs)` | `xs.len()` | Returns a `usize`. |
| `xs.append(x)` | `xs.push(x)` | |
| `xs.sort()` | `xs.sort()` | Sorts in place. |
| `s.split(",")` | `s.split(",")` | Returns owned strings. |
| `"".join(xs)` | build a `String` with `push_str` | |
| `int("12")` | `i32.parse("12")` | Returns a `Result`. |
| `class C:` | `struct C { ... }` and `C { fn ... }` | No inheritance. |
| `self` | `self`, `mut self`, or `own self` | Read-only unless marked. |
| `try: ... except E:` | `match` on a `Result`, or `?` | |
| `raise E("msg")` | `Err(E.Variant)` | Errors are returned, not raised. |
| `assert x == y` | `assert_eq(x, y)?` in a `test` | |
| `with open(...) as f:` | a scope with a value that has `Drop` | Cleanup runs when the value goes out of scope. |
| `import json` | `use json` | With `json = true` in `burdin.toml`. |
| `pip install` | `[dependencies] name = true` | The official packages, or a path. |

## Integers and division

Python integers never overflow. Burdin integers are fixed size: `i32` by default, and `i64` or `i128` when a value is
bigger. An overflow stops a debug build with a panic, so the program does not quietly produce the wrong number.

Division and remainder truncate toward zero, which is the C rule, not Python's:

```burdin
fn main() {
    println("{-7 / 2} {-7 % 3}")
}
```

`-7 / 2` is `-3` in Burdin. In Python, `-7 // 2` is `-4`, because `//` rounds down. Burdin's `/` on integers always rounds
toward zero, and `%` takes the sign of the dividend, so `-7 % 3` is `-1`.

## Indexing

Indexes are `usize`, so there is no negative indexing. `xs[-1]` is a compile error. Write the last element as
`xs[xs.len() - 1]`. Indexing past the end is a panic at run time.

## Strings

A Python `str` is immutable. A Burdin `String` is owned and can change, with `push_str` and `push`, and a `str` is a
read-only view of text. A function that only reads text takes a `str`, and it accepts a `String` without a copy.

## Errors

Python raises exceptions and catches them far away. Burdin returns errors as values:

```burdin
fn parse_age(text: str): Result<i32, ParseIntError> {
    let age = i32.parse(text)?
    Ok(age)
}

fn main() {
    match parse_age("42") {
        Ok(age) => println("age {age}"),
        Err(e) => println("not an age: {e}"),
    }
}
```

The `?` after the call is the short form of "if this failed, return the failure". It is the same job a `try` block does in
Python, but it is visible at every call. The [errors chapter](/guide/errors/) covers error enums and `main` returning a
`Result`.

## Classes

A Python class with methods becomes a struct and a block of methods. There is no inheritance. Shared behavior goes in an
interface, and a type says it implements the interface:

```burdin
struct Account {
    owner: String
    balance: i32
}

Account {
    fn deposit(mut self, amount: i32) {
        self.balance = self.balance + amount
    }

    fn report(self): String {
        "{self.owner}: {self.balance}"
    }
}

fn main() {
    let mut a = Account { owner: String.from("ana"), balance: 0 }
    a.deposit(50)
    println(a.report())
}
```

Notice that `deposit` is marked `mut self`, and the call `a.deposit(50)` is on a `let mut` binding. Python lets you change
anything anywhere. Burdin asks you to say, in two places, where a value changes.

## Things to expect

- **A list is a fixed-type collection.** Every element of a `List<i32>` is an `i32`. Mixed types need `any` or an enum.
- **Types are inferred inside a function**, so `let x = 1` is an `i32` and you rarely write types inside a body. The
  signature is where types are written.
- **No REPL habits carry over directly.** Use `burdin repl` for quick experiments, or a test block.
