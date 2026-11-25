---
title: Strings and formatting
description: String and str, building text, interpolation, format specifiers, and printing.
---

Burdin has two text types. A `String` owns its characters and can grow. A `str` is a read-only view of text that lives
somewhere else, such as a string literal or a `String` owned by someone else.

```burdin
fn shout(text: str): String {
    let mut out = String.from(text)
    out.push_str("!")
    out
}

fn main() {
    let owned = String.from("hi")
    println(shout(owned))
    println(shout("lit"))
}
```

A function that only reads text takes a `str`. A `String` is passed to it as is, because a `String` can be viewed as a
`str` without copying. A literal such as `"lit"` is a `str`.

## Building text

| To | Use |
| --- | --- |
| Make a `String` from a `str` | `String.from(text)` |
| Make an empty `String` | `String.new()` |
| Add text to the end | `s.push_str(text)`, or `s.push(c)` for one `char` |
| Get a view for reading | `s.as_str()` |
| Measure | `s.len()` (in bytes, for UTF-8 text) |
| Trim, change case | `s.trim()`, `s.to_lowercase()`, `s.to_uppercase()` |
| Split | `s.split(",")`, `s.split_whitespace()`, `s.lines()` |
| Look at characters | `s.chars()` |
| Test | `s.contains("x")`, `s.starts_with("x")` |

```burdin
fn main() {
    let text = "  alpha, beta, gamma  "
    for part in text.trim().split(", ") {
        println("[{part}]")
    }
    let mut line = String.new()
    line.push_str("count: ")
    line.push_str(3.to_string())
    println(line)
}
```

A `String` and a `str` compare directly, in either order, with `==`, `!=`, `<`, `<=`, `>`, and `>=`. The comparison is by
the bytes of the text:

```burdin
fn main() {
    let name = String.from("ana")
    if name == "ana" {
        println("found ana")
    }
    if name < "bob" {
        println("ana sorts before bob")
    }
}
```

Two `String` values, or two `str` values, compare the same way. Other pairs of different types do not compare.

To join text, `String + str` returns a new `String` that holds both. Both operands stay usable:

```burdin
fn main() {
    let first = String.from("Burdin")
    let greeting = first + " language"
    println("{greeting} and {first}")
}
```

`str + str` and `String + String` are errors. For two `String` values, write `s + t.as_str()`. `+=` is not available for
text; use `push_str`.

Slicing a string with `s[a..b]` panics when an end falls inside a multi-byte character. `s.get(a, b)` returns `Option.None` in
that case instead.

## Interpolation

A string literal can contain `{...}`. Each hole is an expression, and its value is written into the string. A literal with at
least one hole is a `String`; a literal with no hole is a `str`:

```burdin
fn main() {
    let name = "Burdin"
    let version = 2
    println("Hello, {name} version {version + 1}!")
    println("{name.len()} letters")
}
```

Write `{{` and `}}` for literal braces. A hole can call functions and methods. The compiler checks every hole, so a name that
does not exist is an error, not an empty string at run time.

## Format specifiers

After a colon inside a hole, a specifier controls how the value is written:

```burdin
fn main() {
    let price = 3.14159
    let n = 42
    let code = 255
    let bits = 5
    let sign = -3
    println("[{price:.2}]")
    println("[{n:>6}] [{n:<6}] [{n:^6}]")
    println("[{n:06}] [{code:x}] [{bits:b}]")
    println("[{sign:+}]")
}
```

| Specifier | Effect |
| --- | --- |
| `:.2` | Two digits after the decimal point (floats), or at most two characters (text) |
| `:>6`, `:<6`, `:^6` | Right, left, or center alignment in a field six characters wide |
| `:06` | Pad a number with zeros to six characters |
| `:+` | Show the sign of a number, even when it is positive |
| `:x`, `:X`, `:o`, `:b` | Hexadecimal, upper-case hexadecimal, octal, and binary for integers |
| `:#x`, `:#b` | The same, with the `0x` or `0b` prefix |
| `:?` | The debug form, which quotes strings and shows structure |

The compiler rejects a specifier that does not make sense for the value, such as hexadecimal for a `str`.

## Printing

| Function | Writes to | Newline |
| --- | --- | --- |
| `print(...)` | standard output | no |
| `println(...)` | standard output | yes |
| `eprint(...)`, `eprintln(...)` | standard error | no, yes |
| `format(...)` | a new `String`, and prints nothing | no |

```burdin
fn main() {
    let label = format("{:>4}|{:<4}|", 1, 2)
    println(label)
    println("{:?} is a debug string", "quoted")
}
```

Use `{:?}` to see a value the way the compiler sees it. Many standard types, and any type that derives `Debug`, have a debug
form:

```burdin
struct Point: Debug {
    x: i32
    y: i32
}

fn main() {
    let p = Point { x: 1, y: -2 }
    println("{p:?}")
}
```

To control how your own type prints with `{}`, implement the `Display` interface. Its `fmt` method writes into a
`Formatter` with `write_str`:

```burdin
struct Point {
    x: i32
    y: i32
}

Point: Display {
    fn fmt(self, mut f: Formatter) {
        f.write_str("<{self.x}, {self.y}>")
    }
}

fn main() {
    println("point {Point { x: 3, y: 4 }}")
}
```
