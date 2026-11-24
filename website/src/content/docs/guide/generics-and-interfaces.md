---
title: Generics and interfaces
description: Generic functions and types, interfaces, default methods, where clauses, and existential values.
---

Generics let one function or type work for many types. Interfaces say what a type can do. Together they cover the
ground that traits and type classes cover in other languages.

## Generic functions

A type parameter goes in angle brackets after the name. A bound after a colon says which interfaces it must implement:

```burdin
interface Shape {
    fn area(self): f64
    fn name(self): String
}

struct Square {
    side: f64
}

Square: Shape {
    fn area(self): f64 { self.side * self.side }
    fn name(self): String { String.from("square") }
}

fn describe<T: Shape>(shape: T) {
    println("{shape.name()} with area {shape.area()}")
}

fn main() {
    describe(Square { side: 2.0 })
}
```

The compiler creates a separate version of `describe` for each type it is called with, so a call does not go through a
table of methods at run time. A call with a type that does not implement `Shape` is a compile-time error.

When the compiler can work out the type from the arguments, you do not write it. `describe(Square { ... })` is enough.
To name it, write the type in front of the call's name: `describe<Square>(...)`.

## Interfaces

An interface lists the methods a type must provide. A type states that it implements an interface with a block after its
name, and that block contains the methods:

```burdin
interface Greeter {
    fn name(self): String
}

struct Robot {
    id: i32
}

Robot: Greeter {
    fn name(self): String {
        "robot {self.id}"
    }
}

fn main() {
    let r = Robot { id: 7 }
    println("hello, {r.name()}")
}
```

The implementation is explicit. A `Robot` has `name` as a method only because the block says it implements `Greeter`.
That keeps the set of methods on a type easy to find.

## Default methods

An interface method can have a body. A type that implements the interface gets that method for free, and it can override
it:

```burdin
interface Named {
    fn name(self): String
    fn greet(self): String {
        "hi {self.name()}"
    }
}

struct Dog {
    label: String
}

Dog: Named {
    fn name(self): String { self.label.clone() }
}

fn main() {
    let d = Dog { label: String.from("rex") }
    println(d.greet())
}
```

## Generic types

Structs and enums take type parameters the same way. Methods of a generic type are written in a block with the same
parameters:

```burdin
struct Pair<A, B> {
    first: A
    second: B
}

Pair<A, B> {
    fn swap(own self): Pair<B, A> {
        Pair { first: self.second, second: self.first }
    }
}

fn main() {
    let p = Pair { first: 1, second: "one" }
    let q = p.swap()
    println("{q.first} {q.second}")
}
```

The [linked stack](/examples/linked-stack/) example is a generic type that stores itself through `Heap<T>`.

## where clauses

When a bound is long, or it applies to a type that is not a plain parameter, put it in a `where` clause after the
signature:

```burdin
interface Named {
    fn greet(self): String
}

struct Dog {
    name: String
}

Dog: Named {
    fn greet(self): String {
        "woof, I am {self.name}"
    }
}

fn describe_all<T>(items: [T]) where T: Named {
    for item in items {
        println(item.greet())
    }
}

fn main() {
    let dogs = [Dog { name: String.from("rex") }, Dog { name: String.from("ana") }]
    describe_all(dogs[0..2])
}
```

## Returning an interface: impl

A return type can be `impl I`. The function returns one concrete type, which its body decides, and the caller sees only
the interface. The caller can call the interface's methods and pass the value where `I` is expected. Every `return` and
the final expression must have the same concrete type, and that type must implement `I`:

```burdin
interface Shape {
    fn area(self): i32
}

struct Square {
    side: i32
}

Square: Shape {
    fn area(self): i32 {
        self.side * self.side
    }
}

fn make_square(side: i32): impl Shape {
    Square { side: side }
}

fn main() {
    let s = make_square(3)
    println("area {s.area()}")
}
```

When different branches must return different types, use `any Shape` instead, as the next section describes.

## Existential values: any

A generic function needs one concrete type at each call. A `List` of different shapes needs a different tool. The type
`any Shape` holds a value of some type that implements `Shape`, and the list may mix types:

```burdin
interface Shape {
    fn area(self): f64
}

struct Circle {
    radius: f64
}

struct Rect {
    w: f64
    h: f64
}

Circle: Shape {
    fn area(self): f64 { 3.0 * self.radius * self.radius }
}

Rect: Shape {
    fn area(self): f64 { self.w * self.h }
}

fn main() {
    let mut shapes = List<any Shape>.new()
    shapes.push(Circle { radius: 1.0 })
    shapes.push(Rect { w: 2.0, h: 5.0 })
    let mut total = 0.0
    for s in shapes {
        total = total + s.area()
    }
    println("total {total}")
}
```

A value behind `any` is called through the interface, so only the interface's methods are visible. The value is stored
in a heap box, and the call goes through the interface at run time. Use a generic parameter when every value has one known
type; use `any` when a collection holds mixed types. The [shapes example](/examples/shapes/) puts both side by side.

## Built-in interfaces

Several interfaces are built into the language and the standard library, and many types implement them:

| Interface | What it gives you |
| --- | --- |
| `Display` | Printing with `{}` |
| `Debug` | Printing with `{:?}` |
| `Clone` | `.clone()` |
| `Copy` | Copy on assignment, with no `.clone()` |
| `Eq`, `Ord`, `Hash` | `==`, ordering, and use as a map key |
| `Default` | A default value, `T.default()` |
| `Drop` | Code that runs when a value is dropped |
| `Iterator<T>` | Use with adapters and `for`. See [closures and iterators](/guide/closures-and-iterators/). |

Most of them can be derived. See [structs and methods](/guide/structs-and-methods/) for the derive list.
