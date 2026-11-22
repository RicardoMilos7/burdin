---
title: Introduction
description: What Burdin is, what it is for, and what it is not.
---

Burdin is a statically typed, natively compiled language. Its goals are Rust-level memory safety, no garbage collector,
and value semantics: a value is copied or moved, never shared behind your back.

It is also a personal, AI-assisted experiment. It is **not for production**. The language changes whenever the design
needs it, and there are no compatibility promises yet. Read this site as a working description of what the compiler
accepts today, not as a stable standard.

## What it looks like

A Burdin program is a set of functions, types, and tests in `.bd` files. Here is a complete program:

```burdin
enum Shape {
    Circle(f64)
    Rect { w: f64, h: f64 }
}

fn area(s: Shape): f64 {
    match s {
        Shape.Circle(r) => 3.14 * r * r,
        Shape.Rect { w, h } => w * h,
    }
}

fn main() {
    let shapes = [Shape.Circle(1.5), Shape.Rect { w: 3.0, h: 4.0 }]
    for s in shapes {
        println("area: {area(s)}")
    }
}
```

A few things are already visible in that program:

- **No semicolons.** A statement ends where its expression ends, and a line break is enough.
- **Types after a colon.** Parameters and return types use `name: Type` and `): Type`.
- **Enums and `match`.** The compiler checks that every case is handled.
- **Interpolation.** `"area: {area(s)}"` puts the value of any expression into a string.
- **Types are written where they matter.** Inference fills in the rest, so `let shapes = ...` has no annotation.

## The ideas behind it

Burdin is organized around a small number of rules that apply everywhere:

- **Memory is checked at compile time.** Using a value after it moved, writing to a value that is borrowed, or
  returning a reference to a local is an error reported by `burdin check`. There is no run-time collector.
- **Parameters are read-only by default.** A function can look at a value without taking it. `mut` and `own` at the call
  site say when a value may change or is consumed. See [Ownership](/guide/ownership/).
- **Heap memory is visible.** Heap allocation is written `Heap<T>`. Standard collections such as `List<T>` and `Map<K, V>`
  own their memory and free it when they are dropped.
- **No nulls and no exceptions.** Missing values are `Option<T>`, and failures are `Result<T, E>`. The `?` operator passes
  a failure up to the caller.
- **Every error has a code.** Diagnostics name the code, the source location, and often a fix. `burdin explain E0100`
  prints the reference entry for one code.

## What is in the language today

The compiler implements a large core: functions, structs, enums, pattern matching, generics, interfaces, closures,
iterators, collections, string formatting, modules, a test runner, error types, tasks and channels, a C bridge for
calling C libraries, and a package manager with official packages for JSON, TOML, CSV, YAML, XML, HTTP, TLS, regex,
crypto, and math. See [Status and limits](/reference/status/) for what is finished and what is not.

## Where to go next

- [Install the compiler](/getting-started/installation/) on Windows, Linux, or macOS, or build it from source.
- Follow the [quickstart](/getting-started/quickstart/) to write and run your first program.
- Read the [language guide](/guide/basics/) from the first chapter.
- If you already know another language, start with [coming from another language](/coming-from/rust/).
- Browse the [examples](/examples/): complete programs you can run.
