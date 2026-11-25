---
title: Enums and match
description: Enums with data, exhaustive match, patterns, guards, and when to use each form.
---

An enum is a type with a fixed set of variants. A variant can carry no data, a tuple of values, or named fields:

```burdin
enum Shape {
    Circle(f64)
    Rect { w: f64, h: f64 }
    Dot
}
```

A `match` on the enum handles each variant. It must cover every variant, so adding a variant to an enum makes every
`match` that misses it a compile-time error:

```burdin
enum Shape {
    Circle(f64)
    Rect { w: f64, h: f64 }
    Dot
}

fn area(s: Shape): f64 {
    match s {
        Shape.Circle(r) => 3.14 * r * r,
        Shape.Rect { w, h } => w * h,
        Shape.Dot => 0.0,
    }
}

fn main() {
    println("{area(Shape.Circle(1.0))}")
    println("{area(Shape.Rect { w: 2.0, h: 5.0 })}")
    println("{area(Shape.Dot)}")
}
```

Variants are named through the enum: `Shape.Circle(1.0)`, not `Circle(1.0)`. Inside a `match`, the patterns use the same
names.

## Patterns

A pattern can be a literal, a name that binds a value, `_` for anything, a variant with sub-patterns, or a tuple of patterns:

```burdin
fn describe(pair: (i32, bool)): str {
    match pair {
        (0, _) => "zero",
        (n, true) if n > 10 => "big and flagged",
        (_, true) => "flagged",
        (_, false) => "plain",
    }
}

fn main() {
    println(describe((0, true)))
    println(describe((12, true)))
    println(describe((3, true)))
    println(describe((3, false)))
}
```

A guard, written `if` after a pattern, adds a condition. Arms are tried in order, so the first one that fits wins. Put
the more specific arms first.

Matching a number or a `str` needs an `_` arm, because there are too many possible values to list. The compiler reports
the missing case when you forget it.

## Options and Results are enums

`Option<T>` is `Option.Some(value)` or `Option.None`. `Result<T, E>` is `Result.Ok(value)` or `Result.Err(error)`. Both are ordinary enums, so they
are matched the same way:

```burdin
fn parse_or_zero(text: str): i32 {
    match i32.parse(text) {
        Result.Ok(n) => n,
        Result.Err(_) => 0,
    }
}

fn main() {
    println("{parse_or_zero("41")}")
    println("{parse_or_zero("nope")}")
}
```

Where the place expects one of these two types, the variant can start with a dot: `let found: Option<i32> = .Some(3)` and
`let missing: Option<i32> = .None`.

The [errors](/guide/errors/) chapter covers the shorter forms for these two types.

## Enums with generic data

An enum can take type parameters, just as a struct can. A recursive enum must put the recursive part behind `Heap<T>`,
because a value cannot contain itself directly:

```burdin
enum Tree {
    Leaf(i32)
    Node(Heap<Tree>, Heap<Tree>)
}

fn sum(t: Tree): i32 {
    match t {
        Tree.Leaf(n) => n,
        Tree.Node(left, right) => sum(*left) + sum(*right),
    }
}

fn main() {
    let t = Tree.Node(Heap.new(Tree.Leaf(3)), Heap.new(Tree.Node(Heap.new(Tree.Leaf(4)), Heap.new(Tree.Leaf(5)))))
    println("sum {sum(t)}")
}
```

`*left` reads the value out of the box. The [ownership](/guide/ownership/) chapter explains why the match above moves the
boxes out of `t`.

## Variants where the type is known

A variant can start with a dot when the place expects its enum. This follows the rule for struct values, described in
[structs and methods](/guide/structs-and-methods/#leaving-out-the-type-name):

```burdin
enum Color {
    Red
    Green
}

fn name(c: Color): str {
    match c {
        Color.Red => "red",
        Color.Green => "green",
    }
}

fn main() {
    let favorite: Color = .Green
    println(name(favorite))
    println(name(.Red))
}
```

The program prints `green` and then `red`. The leading dot needs an expected type, and a comparison does not give one, so
`favorite == .Green` is an error. Write `favorite == Color.Green` instead.

### Struct-like variants

A variant with named fields can be built with a literal, and a generic enum's variant works the same way. The type
arguments can be written on the enum, or left out and taken from the field values and the expected type, as a tuple
variant takes them:

```burdin
enum Shape<T> {
    Dot
    Circle(T)
    Rect { w: T, h: T }
}

fn area(s: Shape<i32>): i32 {
    match s {
        Shape.Dot => 0,
        Shape.Circle(r) => r * r,
        Shape.Rect { w, h } => w * h,
    }
}

fn main() {
    let a = Shape<i32>.Rect { w: 2, h: 3 }
    let b: Shape<i32> = Shape.Rect { w: 4, h: 5 }
    println("{area(a)} {area(b)}")
}
```

The literal is checked like any other struct literal: every field is given once, and each value must have the field's
type. A module path works too, as in `shapes.Shape<i32>.Rect { w: 2, h: 3 }`. The arguments belong to the type, so a
variant with its own arguments, `Shape.Rect<i32> { ... }`, is an error. Inside the impl block of a generic enum,
`Self.Rect { ... }` builds the variant of that same instance.

## Enums in other forms

- An enum that derives `Copy` is copied on assignment. Its variants must all hold `Copy` data.
- `error enum` declares an error type whose variants have messages. It is covered in [errors](/guide/errors/).
- `if let` checks one variant without a full `match`. See [control flow](/guide/control-flow/).
