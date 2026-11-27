---
title: Closures and iterators
description: Closures with annotated parameters, function types, and iterator chains with map, filter, and collect.
---

A closure is a function value written inline. The parameters are written between bars, and the body follows. Burdin
asks for the type of each parameter, so the compiler never has to guess what a closure accepts:

```burdin
fn apply(f: Fn(i32): i32, x: i32): i32 {
    f(x)
}

fn main() {
    let double = |x: i32| x * 2
    let describe = |x: i32|: i32 {
        let y = x + 1
        y * y
    }
    println("{apply(double, 21)} {apply(describe, 4)}")
}
```

A closure with one expression can be written on one line, as `double` is. A closure with a block body can state its return
type after the parameters, as `describe` does.

## Function types

`Fn(i32): i32` is the type of a function that takes an `i32` and returns an `i32`. A parameter of that type accepts any
closure or function with the same shape. The other two function types are `FnMut(...)`, for a closure that changes what
it captured, and `FnOnce(...)`, for a closure that can run only once because it consumes a value.

## Closures and the values they use

A closure can use local variables from the function around it. The compiler decides how it gets them from how the closure
is used:

- A closure passed to a function, or called right away, borrows what it uses. The variables are still owned by the
  function, and they can be used again when the call returns.
- A closure that is stored in a variable with a written function type, put in a struct, or returned takes what it uses by
  move, so it can outlive the function that made it.

```burdin
fn main() {
    let factor = 3
    let scale = |x: i32| x * factor
    println("{scale(5)} {factor}")
}
```

## Iterators

Most loops over a collection do not need a loop at all. An iterator is a value that yields one item at a time. Its
methods, called *adapters*, wrap it in another iterator, and nothing runs until a *consumer* asks for items:

```burdin
fn main() {
    let nums = [1, 2, 3, 4, 5, 6]
    let evens: List<i32> = nums.iter()
        .filter(|x: &i32|: bool { *x % 2 == 0 })
        .map(|x: i32|: i32 { x * 10 })
        .collect()
    println("{evens:?}")
}
```

`iter()` yields each element. For numbers, which are copied, the closure receives the value. `filter` looks at a
reference, so it takes `&i32` and dereferences it with `*`. Each adapter takes a closure, and `collect()` gathers the
results into a `List`.

The most common adapters and consumers:

| Method | Does |
| --- | --- |
| `map(f)` | Transforms each item |
| `filter(f)` | Keeps the items for which `f` is true |
| `take(n)` | Stops after `n` items |
| `enumerate()` | Pairs each item with its position: `(index, item)` |
| `rev()` | Walks the items backwards |
| `fold(start, f)` | Combines all items into one value, starting from `start` |
| `sum()` | Adds the items up |
| `any(f)`, `all(f)` | Whether some, or every, item satisfies `f` |
| `collect()` | Gathers the items into a collection, such as a `List`. The type comes from an annotation (`let xs: List<i32> = ...`), or from an explicit type argument: `collect<List<i32>>()` |

Adapters are lazy. `take(2)` pulls only two items from the chain before it stops, so a `map` after it runs only twice.

```burdin
fn main() {
    let total = [1, 2, 3].iter().fold(0, |acc: i32, x: i32|: i32 { acc + x })
    println("total {total}")

    let pairs = [10, 20]
    for (i, v) in pairs.iter().enumerate() {
        println("{i}: {v}")
    }
}
```

A chain can stand on a value that lives long enough. An array literal in the middle of a chain would be dropped before
the chain runs, so bind the array to a name first, as the `pairs` example does.

## Direct chaining on collections

Every adapter and consumer above works directly on any `Iterable` (`List`, slices, arrays, `Map`, ranges, and user
`Iterable` types), without writing `.iter()`. The call borrows the collection through `iter()` and stays lazy, so a
chain builds no intermediate lists. Call `to_list()` or `collect()` when you want an eager `List`:

```burdin
fn main() {
    let mut xs = List<i32>.new()
    xs.push(1)
    xs.push(2)
    xs.push(3)
    xs.push(4)
    let doubled = xs.map(|x: i32|: i32 { x * 2 }).to_list()
    let big = xs.filter(|x: &i32|: bool { *x > 2 }).to_list()
    let total = xs.fold(0, |acc: i32, x: i32|: i32 { acc + x })
    let back = (1..=4).rev().to_list()
    println("{doubled:?} {big:?} {total} {back:?}")
}
```

Use `.iter()` when you want the iterator cursor itself (to call `next()` by hand), and `.into_iter()` when you want to
consume the collection and move its items out.

`collect` builds the collection its target names, and a prelude constructor such as `Option.Some` can be passed as the function:

```burdin
fn main() {
    let squares = [1, 2, 3].iter().map(|x: i32|: i32 { x * x }).collect<List<i32>>()
    let wrapped: List<Option<i32>> = [1, 2].iter().map(Option.Some).collect()
    println("{squares:?} {wrapped:?}")
}
```

## for loops

A `for` loop uses the same protocol. Over a range, a list, a map, or an array, `for` walks the items directly, which is
the simplest form. Over a list of strings, each item is a reference, so the loop reads the strings without taking them:

```burdin
fn main() {
    let mut names = List<String>.new()
    names.push(String.from("ana"))
    names.push(String.from("ben"))
    for name in names {
        println("hello {name}")
    }
    println("{names.len()} names remain")
}
```

To take the items out and consume the collection, call `into_iter()`. The [collections](/guide/collections/) chapter
shows the rules for each collection.
