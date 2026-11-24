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
`crypto`, and `math`. You do not declare them in `burdin.toml`. A `use` line loads one:

```burdin
use json

struct Server: Serialize, Deserialize {
    host: String
    port: u16
}

fn main() {
    let server = Server { host: "example.test".to_string(), port: 8080 }
    let text = json.to_string_pretty(server).unwrap()
    println(text)
}
```

The compiler reads each `use` in your files and loads only the official packages they name. A package you never import
is not built, checked, or pinned in `burdin.lock`.

The editor helps with them. Typing the start of a package's name offers it (`htt` offers `http`), and accepting a
completion from a package that the file does not load, such as `http.Client`, adds its `use` line for you. Writing
`http.Client` without that `use` gets a quick fix titled "import http" that adds it, and a bare `Client` gets "import
http.Client", which adds `use http.Client`. Writing `json = true` in `burdin.toml`, or running `burdin add json`,
is an error that points you to `use json`. The [packages reference](https://github.com/RicardoMilos7/burdin/blob/main/spec/packages.md)
describes how the compiler finds official packages and how `burdin.lock` pins them.

## Libraries

A library package has `src/lib.bd` and no `src/main.bd`. By default it has no build output: other Burdin packages
compile its source into their own programs. To build an artifact that programs in other languages can link, set the
`[lib]` table in `burdin.toml`:

```toml
[lib]
kind = "static"             # "source" (the default), "static", or "shared"
c-header = "include/greet.h" # optional: a C header of the exported functions
```

- `static` writes an archive: `libNAME.a` on Linux and macOS, `NAME.lib` on Windows.
- `shared` writes a shared library: `libNAME.so` on Linux, `libNAME.dylib` on macOS, and `NAME.dll` with `NAME.lib` on
  Windows.
- The exports are the `extern "C" fn` items that are not `priv`. A `priv` function has no C symbol.
- `burdin build` writes the artifact and, with `c-header`, the header. `burdin run` refuses a library, since it has
  nothing to run.
- A static or shared library must not have a `src/main.bd`. An invalid `[lib]` table is error E0265.

A C program links the artifact like any other library. For a static library on Linux, add `-lm`. The
[library specification](https://github.com/RicardoMilos7/burdin/blob/main/spec/library.md) lists every rule.

## Checking a package

`burdin check` type-checks a package without building it, and `burdin build` builds it. Commands accept a package
directory or a single `.bd` file. In a directory that holds several packages (a *workspace*), `-p <name>` selects one.
The [workspaces specification](https://github.com/RicardoMilos7/burdin/blob/main/spec/workspaces.md) covers that case.
