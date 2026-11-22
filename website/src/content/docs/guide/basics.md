---
title: Basics
description: Bindings, types, numbers, tuples, arrays, and the operators you will use first.
---

This chapter covers the values a program works with: bindings, the built-in types, numbers, tuples, and arrays.

## Bindings

A `let` introduces a name. Bindings are immutable unless you write `mut`:

```burdin
fn main() {
    let name = "Burdin"
    let mut count = 0
    count = count + 1
    println("{name} has been run {count} time")
}
```

Assigning to a name that was not declared `mut` is an error, so the compiler tells you which values can change. A
name can be declared again with `let` (shadowing). The new name replaces the old one from that point on:

```burdin
fn main() {
    let input = "42"
    let input = i32.parse(input).unwrap()
    println("{input + 1}")
}
```

A type can be written after the name. It is needed when the compiler cannot infer it, or when you want a specific size:

```burdin
fn main() {
    let big: i64 = 5_000_000_000
    let flag: bool = true
    println("{big} {flag}")
}
```

`const` declares a value known at compile time, with a required type. Constants are written in `SCREAMING_SNAKE_CASE`:

```burdin
const MAX_PLAYERS: i32 = 4

fn main() {
    println("up to {MAX_PLAYERS} players")
}
```

## Built-in types

| Kind | Types |
| --- | --- |
| Signed integers | `i8`, `i16`, `i32`, `i64`, `i128`, `isize` |
| Unsigned integers | `u8`, `u16`, `u32`, `u64`, `u128`, `usize` |
| Floating point | `f32`, `f64` |
| Other scalars | `bool`, `char` (one Unicode scalar value) |
| Text | `str` (a view of text owned elsewhere), `String` (owned, growable) |
| Compound | tuples `(i32, str)`, arrays `[i32; 3]`, slices `[i32]`, `List<T>`, `Map<K, V>`, `Set<T>`, `Deque<T>` |

Integer literals have no suffix. Their type comes from the context: `let big: i64 = 5_000_000_000` makes the literal an `i64`.
If nothing else decides, an integer literal is `i32` and a decimal literal is `f64`. Underscores may separate the digits of
a literal, and `0x`, `0o`, and `0b` prefixes give hexadecimal, octal, and binary numbers.

Indexes and lengths are `usize`. `list.len()` returns a `usize`, so comparing it with an `i32` needs a cast:
`xs.len() as i32`.

## Numbers and arithmetic

The operators are the ones you expect: `+ - * / %`, comparisons, `&& || !`, and bit operations. Two rules are worth
knowing:

- **Overflow is an error.** In a debug build, an integer that goes past its range stops the program with a panic. A
  release build wraps around instead. When wrapping is what you want, ask for it: `wrapping_add`, `checked_add`
  (which returns an `Option`), and `saturating_add` are methods on each integer type.
- **Division truncates toward zero.** `7 / 2` is `3`, and dividing by zero panics.

```burdin
fn main() {
    println("{7 / 2} {-7 / 2} {-7 % 3}")
    println("{(250 as u8).wrapping_add(10)}")
    println("{(5 as i32).checked_div(0).is_none()}")
}
```

Use `as` to convert between numeric types. It truncates or extends the value, and it never fails:

```burdin
fn main() {
    println("{1000 as u8} {-1 as u32} {3 as f64 / 2.0}")
}
```

## Booleans and characters

Conditions are `bool`. There is no implicit conversion from a number to a `bool`, so `if n` is an error; write `if n != 0`.
A `char` is written in single quotes, and `as` converts it to and from its code point:

```burdin
fn main() {
    let letter = 'a'
    println("{letter} is {letter as u32}")
    println("{(66 as u8) as char}")
}
```

## Tuples

A tuple groups values of different types. Its fields are numbered from zero, and a `let` can take it apart:

```burdin
fn min_max(a: i32, b: i32): (i32, i32) {
    if a < b { (a, b) } else { (b, a) }
}

fn main() {
    let pair = min_max(9, 2)
    println("first {pair.0}, second {pair.1}")
    let (low, high) = min_max(7, 3)
    println("{low} to {high}")
}
```

## Arrays

An array has a fixed length that is part of its type, `[i32; 3]`. A `let mut` array can be changed element by element.
`[0; 8]` makes eight zeros:

```burdin
fn main() {
    let mut scores = [0; 3]
    scores[0] = 90
    scores[2] = 75
    println("{scores[0] + scores[1] + scores[2]} in {scores.len()} scores")
}
```

An index outside the array stops the program with a panic. Every index is bounds-checked when the program runs, so an
out-of-range index is never silent.

## Ranges

`a..b` is the numbers from `a` up to, but not including, `b`. `a..=b` includes `b`. Ranges are most often used in `for`
loops, which the next chapter covers, and in slices, which the [collections](/guide/collections/) chapter covers.

## Blocks are expressions

A block in braces has a value: its last expression, if the line has no `let`. `if` and `match` are blocks too, so they can
be the value of a `let`:

```burdin
fn main() {
    let grade = 83
    let letter = if grade >= 90 { 'A' } else if grade >= 80 { 'B' } else { 'C' }
    println("grade {grade} is {letter}")
}
```

A function's body is a block, so the last expression of a function is its return value. `return` is needed only for an
early exit.
