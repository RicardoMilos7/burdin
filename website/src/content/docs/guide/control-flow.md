---
title: Control flow
description: if, while, for, loop with a value, labeled loops, match, if let, and let else.
---

Burdin has the usual control flow. Conditions and loop headers take no parentheses, and the bodies are always braces.
Since blocks are expressions, most of these forms can produce a value.

## if and else

```burdin
fn sign(n: i32): str {
    if n < 0 {
        "negative"
    } else if n == 0 {
        "zero"
    } else {
        "positive"
    }
}

fn main() {
    println(sign(-4))
    println(sign(0))
}
```

When `if` produces a value, every branch must produce the same type, and there must be an `else`.

## while

```burdin
fn main() {
    let mut n = 3
    while n > 0 {
        println("{n}...")
        n = n - 1
    }
    println("liftoff")
}
```

## for

`for` walks a range, an array, a list, a map, or any value that implements the iteration protocol.

```burdin
fn main() {
    for i in 1..4 {
        println("step {i}")
    }

    let colors = ["red", "green", "blue"]
    for color in colors {
        println(color)
    }
}
```

`1..4` stops before `4`, and `1..=4` includes it. See [closures and iterators](/guide/closures-and-iterators/) for the
adapters that transform a sequence before a loop walks it.

## loop and break with a value

`loop` repeats until a `break`. A `break` can carry a value, and then the `loop` is an expression that produces it:

```burdin
fn main() {
    let mut attempts = 0
    let found = loop {
        attempts = attempts + 1
        if attempts * attempts > 50 {
            break attempts
        }
    }
    println("first square over 50 comes from {found}")
}
```

## Labels

A loop can have a label, written `'name:`. `break` and `continue` then name the loop they act on, which is how you leave
an inner loop and an outer loop at once:

```burdin
fn main() {
    'rows: for r in 0..3 {
        for c in 0..3 {
            if c > r {
                continue 'rows
            }
            print("{r}{c} ")
        }
    }
    println("")
}
```

## match

`match` compares a value against patterns, in order, and runs the first arm that fits. The arms must cover every possible
value. The compiler reports a missing case, so a new variant of an enum makes every `match` on it fail until it is
handled:

```burdin
fn main() {
    let n = 7
    let kind = match n {
        0 => "zero",
        k if k % 2 == 0 => "even",
        _ => "odd",
    }
    println("{n} is {kind}")
}
```

An arm can have a guard after `if`, as `k if k % 2 == 0` does above. `_` matches anything and binds nothing.
[Enums and match](/guide/enums-and-match/) goes further with patterns on data.

## if let and while let

When you care about one case only, `if let` and `while let` take a pattern and run the block when it matches:

```burdin
fn main() {
    let maybe: Option<i32> = Option.Some(5)
    if let Option.Some(x) = maybe {
        println("got {x}")
    }

    let mut stack = List<i32>.new()
    stack.push(1)
    stack.push(2)
    while let Option.Some(top) = stack.pop() {
        println("popped {top}")
    }
}
```

## let else

`let ... else` binds a pattern, and if it does not match, runs the `else` block, which must leave the function or the loop
(with `return`, `break`, or `continue`):

```burdin
fn double_if_present(value: Option<i32>): i32 {
    let Option.Some(n) = value else { return 0 }
    n * 2
}

fn main() {
    println("{double_if_present(Option.Some(21))}")
    println("{double_if_present(Option.None)}")
}
```

Use it to keep the happy path unindented. The [errors](/guide/errors/) chapter uses it with `Option` and `Result`.
