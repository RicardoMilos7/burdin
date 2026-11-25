---
title: Collections
description: List, Map, Set, Deque, arrays, and slices, with the rules for reading, changing, and consuming them.
---

Burdin's standard collections own their elements. A `List` owns its items, a `Map` owns its keys and values, and when the
collection is dropped, so are they. This chapter shows the common operations.

## Arrays and slices

An array has a fixed length. Its type includes the length: `[i32; 3]`. A slice is a view of part of an array or a list,
written `[i32]`, and a range makes one: `xs[1..3]` is the elements at positions 1 and 2.

```burdin
fn total(xs: [i32]): i32 {
    let mut sum = 0
    for x in xs {
        sum = sum + x
    }
    sum
}

fn main() {
    let all = [4, 8, 15, 16, 23, 42]
    println("first three: {total(all[0..3])}")
    println("all: {total(all[0..all.len()])}")
}
```

A function that accepts `[i32]` works with any part of any array. A range that goes past the end is a panic.

## List

`List<T>` is a growable array. Create one with `List<T>.new()`, then add to it:

```burdin
fn main() {
    let mut xs = List<i32>.new()
    xs.push(10)
    xs.push(20)
    xs.push(30)
    xs.insert(1, 15)              // [10, 15, 20, 30]
    let removed = xs.remove(0)    // [15, 20, 30]
    println("removed {removed}, now {xs.len()} left")
    xs[1] = 25                    // [15, 25, 30]
    println("second is {xs[1]}")
    match xs.pop() {              // [15, 25]
        Option.Some(v) => println("popped {v}"),
        Option.None => println("empty"),
    }
    println("contains 25: {xs.contains(25)}")
    xs.sort()
    xs.clear()
    println("empty now: {xs.len() == 0}")
}
```

Changing an element with `xs[i] = value` needs the list to be `let mut`. Reading `xs[i]` copies a `Copy` element. For a
non-`Copy` element, read it through a reference: `&xs[i]`, or with `match xs.last() { Option.Some(v) => ..., Option.None => ... }`,
which gives a reference.

A list is equal to another list when both have the same elements in the same order, and the elements can be compared with
`==`. `[1, 2] == [2, 1]` is false. `dedup_by_key(key)` removes each element whose key is equal to the key of the element
before it, so a run of equal keys is reduced to its first element.

## Map

`Map<K, V>` pairs keys with values. A key type must be comparable and hashable: `String`, the integers, and any struct
that derives `Eq` and `Hash` qualify. `get` returns an `Option` of a reference to the value:

```burdin
fn main() {
    let mut ages = Map<String, i32>.new()
    ages.insert(String.from("ana"), 31)
    ages.insert(String.from("ben"), 27)
    match ages.get("ana") {
        Option.Some(age) => println("ana is {*age}"),
        Option.None => println("no ana"),
    }
    println("has ben: {ages.contains_key("ben")}")
    ages.remove("ben")
    println("{ages.len()} entry left")
}
```

A map has no order. To print its entries in a stable order, collect the keys into a `List`, sort the list, and look each key
up, as the [word count example](/examples/word-count/) does.

A `for` loop over a map reads it: each key and value arrives as a reference, and the map is still usable after the loop.
To take the entries out instead, call `into_iter()`, which moves each key and value to you and consumes the map.

## Set

`Set<T>` holds each value once. It uses the same key rules as a map:

```burdin
fn main() {
    let mut seen = Set<i32>.new()
    seen.insert(3)
    seen.insert(3)
    println("{seen.len()} distinct, has 3: {seen.contains(3)}")
    seen.remove(3)
    println("empty: {seen.is_empty()}")
}
```

## Deque

`Deque<T>` is a double-ended queue. It is fast to add or remove at either end:

```burdin
fn main() {
    let mut q = Deque<i32>.new()
    q.push_back(1)
    q.push_back(2)
    q.push_front(0)
    println("front is {q.pop_front().unwrap()}")
    println("{q.len()} left")
}
```

## Ownership and iteration

The rules for taking items out of a collection follow the ownership rules:

| You write | You get | The collection |
| --- | --- | --- |
| `for x in list` | each item by reference, unless the item is `Copy` | still usable afterwards |
| `for x in list.iter()` | the same, and works for any collection with `iter()` | still usable afterwards |
| `for x in list.into_iter()` | each item by value | consumed |
| `for (k, v) in map` | each key and value by reference | still usable afterwards |
| `for (k, v) in map.into_iter()` | each key and value by value | consumed |

Reading a collection while you loop over it is fine. Changing it inside the loop is not, because the loop borrows it. A
loop that needs to add items should collect them into a new list, then add them afterwards.

For transforming a sequence, see [closures and iterators](/guide/closures-and-iterators/).
