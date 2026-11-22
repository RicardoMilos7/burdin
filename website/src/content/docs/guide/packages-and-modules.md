---
title: Packages and modules
description: How a package is laid out, how files become modules, use and re-exports, priv, and dependencies.
---

A *package* is a folder with a `burdin.toml` manifest. A package is what `burdin build`, `run`, and `test` operate on,
and what other packages depend on. A *module* is a file inside a package. This chapter covers how they fit together.

## Package layout

```text
greeter/
  burdin.toml        the manifest: name, version, dependencies
  src/
    main.bd          the program's entry point (a binary package)
    util.bd          a module, reached as `util`
    shapes/
      square.bd      a module, reached as `shapes.square`
```

A package with a `src/main.bd` builds a program. A library package has `src/lib.bd` instead, and no `main`. Create either
kind with `burdin new <name>` or `burdin new <name> --lib`.

Each `.bd` file is a module. Its path, from `src/`, is its name: `src/shapes/square.bd` is `shapes.square`.

## use

A `use` line brings names into scope. Its forms:

```burdin fragment
use util                          // the module; call its items as util.name(...)
use util.{helper, Config}         // specific items, used directly
use shapes.square as sq           // the module under another name
use shapes.square.{area}          // a function from a module below another
```

The file `main.bd` below uses a module in the same package, and a function from another module in it:

```burdin
// src/util.bd
fn helper(n: i32): i32 {
    n * 2
}
```

```burdin fragment
// src/main.bd
use util.{helper}

fn main() {
    println("{helper(21)}")
}
```

Items are public to the rest of the package by default. Every file in a package can see every other file's items, so a
package can be split across files freely.

## priv

`priv` marks an item as private to its package. Other packages cannot name it, and the package's own files still can:

```burdin
priv fn secret(): i32 {
    41
}
```

A `priv` item is also a way to keep an implementation detail out of a package's public surface. It does not guard
invariants between files of the same package; it only hides items from other packages.

## Dependencies

A package uses another by naming it in `[dependencies]`. A path dependency names a folder:

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
geo = { path = "../geo" }
```

Shorthand works too: `geo = "../geo"`. The dependency's root module is reached by its name, so the program writes
`use geo.{area}` or `geo.area(...)`.

A library reaches its items to the outside through its root module, with `pub use`. Its other modules are hidden unless
the root re-exports them:

```burdin fragment
// geo/src/lib.bd
pub use shapes.square.{Square, area}
```

```burdin fragment
// app/src/main.bd
use geo.{Square, area}

fn main() {
    let s = Square { side: 4 }
    println("area {area(s)}")
}
```

Without the `pub use`, `use geo.shapes.square` is an unknown module from the program's side. Re-exporting keeps the
library's internal layout free to change.

## Official packages

The compiler ships with several official packages: `json`, `toml`, `csv`, `yaml`, `xml`, `http`, `tls`, `regex`,
`crypto`, and `math`. Declare one with `name = true`:

```toml
[dependencies]
json = true
```

or add it from the command line, which writes the same line:

```console
$ burdin add json
Added dependency `json` (official)
```

Then `use json` brings the package in. The [packages reference](https://github.com/RicardoMilos7/burdin/blob/main/spec/packages.md)
describes how the compiler finds official packages and how `burdin.lock` pins them.

## Checking a package

`burdin check` type-checks a package without building it, and `burdin build` builds it. Commands accept a package
directory or a single `.bd` file. In a directory that holds several packages (a *workspace*), `-p <name>` selects one.
The [workspaces specification](https://github.com/RicardoMilos7/burdin/blob/main/spec/workspaces.md) covers that case.
