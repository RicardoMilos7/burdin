---
title: Structs and methods
description: Defining structs, building values, adding methods, and the self conventions.
---

A struct groups named fields. It is declared with `struct`, and each field has a name and a type:

```burdin
struct Point {
    x: i32
    y: i32
}

fn main() {
    let p = Point { x: 3, y: -4 }
    println("({p.x}, {p.y})")
}
```

Fields can be on one line, separated by commas, or one per line, as above. A field name that matches a variable can be
written alone: `Point { x, y }` is the same as `Point { x: x, y: y }`.

## Methods

Methods are written in a block that names the type. The block is not a separate declaration: `Point { ... }` says
"these functions belong to `Point`":

```burdin
struct Point {
    x: i32
    y: i32
}

Point {
    fn new(x: i32, y: i32): Point {
        Point { x, y }
    }

    fn origin(): Point {
        Point { x: 0, y: 0 }
    }

    fn manhattan(self): i32 {
        abs(self.x) + abs(self.y)
    }

    fn translate(mut self, dx: i32, dy: i32) {
        self.x = self.x + dx
        self.y = self.y + dy
    }
}

fn abs(n: i32): i32 {
    if n < 0 { -n } else { n }
}

fn main() {
    let mut p = Point.new(3, -4)
    println("distance {p.manhattan()}")
    p.translate(1, 1)
    println("moved to ({p.x}, {p.y})")
    println("origin is {Point.origin().x}")
}
```

Two kinds of function live in that block:

- **Associated functions** have no `self`. They are called on the type: `Point.new(3, -4)`.
- **Methods** take `self` as their first parameter, and they are called on a value: `p.manhattan()`.

The first parameter `self` is read-only, like any parameter. The method can read the fields and cannot change them. Two
other forms are available:

| Receiver | Meaning |
| --- | --- |
| `self` | Read the value. The caller keeps it. |
| `mut self` | Change the value in place. The caller writes `p.translate(...)`, and `p` must be a `mut` binding. |
| `own self` | Consume the value. The caller cannot use it afterwards. |

`own self` is useful for a method that builds a new value out of the old one:

```burdin
struct Counter {
    n: i32
}

Counter {
    fn finish(own self): i32 {
        self.n * 10
    }
}

fn main() {
    let c = Counter { n: 4 }
    println("{c.finish()}")
    // println("{c.finish()}") would not compile: c was consumed by the first call.
}
```

The [ownership](/guide/ownership/) chapter explains these conventions in full.

## Deriving common behavior

A struct can ask the compiler to write standard behavior for it. The names go after the struct's name, after a colon:

```burdin
struct Point: Debug, Copy, Clone, Eq, Hash, Default, Ord {
    x: i32
    y: i32
}

fn main() {
    let a = Point { x: 1, y: 2 }
    let b = a                     // Copy: a is still usable after this
    println("{a:?} and {b:?} equal: {a == b}")
}
```

| Derive | Gives the type |
| --- | --- |
| `Debug` | `{:?}` printing, showing the fields |
| `Copy` | Copies on assignment, instead of moves. Every field must be `Copy`, and the type must not have a `Drop` impl. |
| `Clone` | A `.clone()` method that makes a full copy |
| `Eq` | `==` and `!=`, comparing every field |
| `Ord` | `<`, `<=`, `>`, `>=`, comparing fields in order |
| `Hash` | Use as a key in a `Map` or `Set` |
| `Default` | `Point.default()`, with every field at its default value |

Each derive checks its own requirements: `Copy` needs every field to be `Copy`, and `Ord` needs every field to be ordered. Deriving
`Ord` does not give you `Eq`, so a type that needs both lists both. A float field has no derived ordering, so a struct with an
`f64` field cannot derive `Ord`. The compiler names the field when a requirement fails.

## Structs with heap data

A field can hold a `String` or a `List`. The struct then owns those values, and drops them when it is dropped:

```burdin
struct Person {
    name: String
    tags: List<String>
}

fn main() {
    let mut p = Person { name: String.from("Ana"), tags: List<String>.new() }
    p.tags.push(String.from("admin"))
    println("{p.name} has {p.tags.len()} tag")
}
```

A field of a view type, such as `str`, is not allowed. A struct stores values it owns, so text in a struct is a `String`.

## Leaving out the type name

When the place a value is written in already says its type, the type name can be left out. A leading dot means "the type
this place expects":

```burdin
struct Point {
    x: i32
    y: i32
}

fn distance(a: Point, b: Point): i32 {
    let dx = a.x - b.x
    let dy = a.y - b.y
    dx * dx + dy * dy
}

fn origin(): Point {
    .{ x: 0, y: 0 }
}

fn main() {
    let p: Point = .{ x: 3, y: 4 }
    println("{distance(p, origin())}")
    println("{distance(.{ x: 1, y: 1 }, .{ x: 4, y: 3 })}")
}
```

The program prints `25` and then `13`. The three forms are:

| Written | Means | Example |
| --- | --- | --- |
| `.{ x: 3, y: 4 }` | `Point { x: 3, y: 4 }` | `let p: Point = .{ x: 3, y: 4 }` |
| `.new(3, 4)` | `Point.new(3, 4)`, when `Point` has a `new` function | `let p: Point = .new(3, 4)` |
| `.Red` | `Color.Red`, a variant of an enum | `let c: Color = .Red` |

These places give the type:

- a `let` with an annotation: `let names: List<String> = .new()`;
- a parameter, for an argument: `distance(.{ x: 1, y: 1 }, ...)` in the program above;
- a function's return type, for a `return` value and for the last value of the body, as `origin` does above;
- a struct field: `Stack { items: .new() }`;
- the place an assignment writes to: `cur = .{ x: 2, y: 3 }`.

The type comes only from the place, never from a later use. A value that has no type of its own is an error, and the
compiler says so:

```text
main.bd:7:13: error[E0003]: `.{ ... }` needs an expected type: write the
    type, as in `List<String>.new()`, or give the place a type
```

A leading dot is not always allowed:

- It cannot start a method call on its own. `.new().len()` is an error; write `List<String>.new().len()`.
- The other side of `==` does not give a type. `c == .Red` is an error; write `c == Color.Red`.
- The type must be a named type. A reference such as `&Point`, a `str`, and a type parameter that the place does not
  know are errors.
- `.{ ... }` builds a struct only. For an enum, write the variant, as in `.Red`. Tuples and arrays keep their own
  spelling, `(1, 2)` and `[1, 2]`.

The rules are the same as for `Point { ... }`, so every check on a struct literal applies here too. The
[target-typed initializers](https://github.com/RicardoMilos7/burdin/blob/main/spec/target-typing.md) specification lists
each rule.
