# Burdin formatting (stage 1, milestone 14)

How a Burdin program turns values into text: string interpolation, the `Display` and `Debug` interfaces, format
specifiers, `format`, and the output functions. The executable form of this page is `tests/m14_format/`; the rule
ids below are what the tests name.

The decision (issue #110) is stated first, because everything else follows from it: **format strings are a compile-
time construct.** A string literal's placeholders are checked against the call's arguments when the program is
compiled — the arguments that exist, the specifiers that apply, and the interfaces the values implement — and the
compiler lowers the whole string to ordinary `Formatter` calls. No format string is parsed at run time, and there
is no `{}`-style formatting that can fail while the program runs.

## FM1. Interpolated strings

A string literal may contain placeholders `{...}`:

```
"hello {name}"                 // an expression in scope; its value with `Display`
"sum: {a + b}"                 // any expression
"{} and {}"                    // positional arguments of a format call
"{0} {1} {0}"                  // positional arguments by index
"{value:>8.2}"                 // an expression with a specifier
"{{literal braces}}"           // `{{` and `}}` are the literal characters
```

- `{name}` and `{expr}` hold an arbitrary expression, evaluated where the string is written. Its scope is the
  surrounding function: a placeholder can call a function, index a collection, call a method, or hold a nested
  string, and a closure that captures a name used only in a placeholder captures it as usual.
- `{}` and `{n}` (digits only) name the arguments of the enclosing format call, numbered from zero; `{}` takes the
  next argument in order. A string literal that is not a format call's format string has no arguments, so `{}` in
  it is E0191.
- `{{` and `}}` are the literal characters `{` and `}`. A single unmatched `{` or `}` is E0001 (FM2).
- A placeholder stays on one line. Its expression may contain brackets, character literals, and string literals;
  the specifier starts at the first `:` outside any brackets.

The type of a string literal depends only on whether it has a placeholder (issue #923; decision 19 and decision 4 in
`tests/README.md`):

- A literal with at least one placeholder (`{...}`, `{}`, or `{n}`) is an interpolated string. Its value is a `String`,
  built where the literal is written, whatever its placeholders hold.
- A literal with no placeholder is a `str`, a static view as in stage 0. A literal whose only braces are the escapes `{{`
  and `}}` has no placeholder, so `"{{}}"` is a `str`.

So `"plain"` and `"a {n}"` differ in type, Copy-ness, storage, which methods they have, and whether they can be stored, and
the difference is decided by the placeholders alone. A format call's format string is not an expression of either
type; FM5 and FM6 check it. Tests: `tests/m14_format/literal_type.bd` (the run rule: a `let`, a return, a field, a
comparison, a format call, and a method call on each form) and `tests/m14_format/literal_type_mismatch.bd` (the same
positions with the other type, which are E0003).

## FM2. Escapes, raw strings, and multi-line strings

An ordinary string literal has the escapes `\n \r \t \\ \" \0` (FM3 is E0001 for any other escape). `{{` and `}}`
are escapes for the braces.

Raw strings have no escapes and no placeholders: `r"..."` is its bytes, and `r#"..."#` (any number of `#`) may
contain `"`. A raw string is a `str` like any other literal, and `{` in it is an ordinary character.

A string literal may span lines; the line break is part of the text, and a `\r\n` inside a literal is one `\n`.
An unterminated string is E0001 and stops the lexer at the end of the file.

## FM3. Format specifiers

The text after the `:` of a placeholder is

```
[[fill]align] [+|-] [#] [0] [width] [.precision] [type]
```

| Part | Meaning |
| --- | --- |
| `fill` | any single character: what the padding is made of (a space by default) |
| `align` | `<` left, `^` center, `>` right; without one, text is left-aligned and numbers right-aligned |
| `+` | show `+` on non-negative numbers |
| `#` | the radix prefix: `0x`, `0b`, `0o` |
| `0` | pad with zeros between the sign (and prefix) and the digits |
| `width` | the minimum number of characters |
| `.precision` | text: at most that many characters; floats: that many digits after the point |
| `type` | `?` for `Debug`; `x`, `X`, `b`, `o` for integers in hex, upper-hex, binary, octal |

A width or precision is at most 4096. Which specifier applies to which value is a compile-time check (FM5):

| Value | Width, fill, alignment | Precision | `+` | `0` | `#` | Radix type |
| --- | --- | --- | --- | --- | --- | --- |
| integers | yes | no (E0190) | yes | yes | with a radix | yes |
| floats | yes | yes | yes | yes | no | no (E0190) |
| `str`, `String`, `char`, `bool` | yes | yes | no (E0190) | no (E0190) | no (E0190) | no (E0190) |
| user `Display`/`Debug` | yes | yes | yes | yes | yes | no (E0190) |

## FM4. `Display` and `Debug`

`std` declares two interfaces:

```
interface Display {
    fn fmt(self, mut f: Formatter)
}

interface Debug {
    fn debug(self, mut f: Formatter)
}
```

`{}` uses `Display` and `{:?}` uses `Debug`. They are independent: a type may implement either, both, or neither
(E0193 when the needed one is missing).

- **Built in.** Integers, floats, `bool`, `char`, `str`, and `String` implement both. `()` and the compound types
  — tuples of 2, 3, or 4 elements, `Option<T>`, `Result<T, E>`, `List<T>`, `Heap<T>`, and `&T` — implement
  `Debug` when their parts do, and no `Display`. There are no impl blocks for the built-in types; the compiler
  formats them with the `priv` helpers in `std/src/fmt.bd`.
- **With a block.** `Type: Display { fn fmt(self, mut f: Formatter) { ... } }` and `Type: Debug { ... }` on any
  struct or enum, generic or not (`Wrapper<T>: Display where T: Display`).
- **Derived.** `struct T: Debug` writes the `Debug` impl: `Name { field: value, ... }` for a struct, `Variant` /
  `Variant(a, b)` / `Variant { field: value }` (the variant's name, not the type's) for an enum, and every field in
  declaration order. Each field's value is written with `Debug`, so every field must implement it (E0130 at the
  field). A generic type needs `Debug` on its parameters and gets it through `where`. `Display` is never derived.

The exact output of `Debug` for a value of each built-in type:

| Value | Output |
| --- | --- |
| `"a\"b"` (`str`, `String`) | `"a\"b"`: `"` around the text, with `\"`, `\\`, `\n`, `\r`, `\t`, and `\0` escaped |
| `'x'` (`char`) | `'x'`, with the same escapes and `\'` |
| `1`, `-2`, `2.0`, `2.5` (`f64`) | `1`, `-2`, `2.0`, `2.5`: a float always shows a point (`.0`) |
| `true`, `()` | `true`, `()` |
| `[1, 2]` | `[1, 2]` |
| `(1, "a")` | `(1, "a")` |
| `Some(1)`, `None` | `Some(1)`, `None` |
| `Ok(1)`, `Err(1)` | `Ok(1)`, `Err(1)` |
| `Heap.new(1)` | `1` (the pointee) |

`Display` output is the type's own: the digits of a number, the bytes of text, `true`/`false`, the character.

The compiler-provided implementations can be boxed as `any Display`/`any Debug` (`spec/existentials.md`, AN14); the boxed
value prints exactly as the value would.

`x.to_string()` formats a value with `Display`. It exists for the types that have a built-in `to_string` already
(integers, `bool`, `char`, `str`) and for floats, and for a struct or enum whose type implements `Display`; a type
with neither is E0002.

## FM5. Compile-time checking

The checker reports, at the format string's span, every error in it, not just the first:

| Problem | Code |
| --- | --- |
| a placeholder needs an argument the call does not have | E0191 |
| an argument of the call is not used by any placeholder | E0192 |
| the value's type does not implement the interface the placeholder needs | E0193 |
| a specifier does not apply to the value's type | E0190 |
| a call with arguments whose format string is not a literal | E0194 |
| an argument used by more than one placeholder that is neither a place nor `Copy` | E0195 |

An argument used by more than one placeholder is evaluated once: a place is borrowed, and a `Copy` value is
copied. A value that is neither (a call returning a `String`, say) is E0195 for the second use, whatever its type;
bind it first. `println("{0} {0}", String.from("x"))` is E0195, not E0191: the argument exists, it just cannot be
used twice.

A specifier that cannot be parsed at all (an unknown type letter, a `.` with no digits, a width over 4096) is
E0190 from the parser; if any such error is present, the rest of the file is not checked, as with any syntax
error. A specifier that parses but does not apply is E0190 from the checker, alongside the other errors in the
file.

## FM6. `format` and the output functions

```
fn format(fmt: str, ...): String
fn print(fmt: str, ...)
fn println(fmt: str, ...)
fn eprint(fmt: str, ...)
fn eprintln(fmt: str, ...)
```

Each takes a string literal followed by its arguments (FM1), builds the text, and — for the output functions —
writes it. `println` and `eprintln` add one `\n`. `format` returns the `String`. A call with a single argument
takes no format string at all: `println(text)` writes any `str` value, exactly as before, so existing programs and
the `String`-to-`str` conversion are unchanged. A call with more than one argument requires its first argument to
be a string literal (E0194).

## FM7. `Formatter`

`Formatter` is the type the compiler builds the text in (`std/src/fmt.bd`). A `fmt` or `debug` body writes to it:

| Member | Meaning |
| --- | --- |
| `f.write_str(s: str)` | append text as it is, ignoring the specifier |
| `f.write_char(c: char)` | append one character as it is |
| `f.pad(s: str)` | append text with the placeholder's precision, width, fill, and alignment (text is left-aligned) |
| `f.pad_plain(s: str)` | append text with the width, fill, and alignment, but not the precision |
| `f.pad_number(sign, prefix, digits)` | append a number: right-aligned by default, and the `0` flag puts zeros after the sign and prefix |
| `f.width(): Option<usize>` | the placeholder's width |
| `f.precision(): Option<usize>` | the placeholder's precision |
| `f.fill(): char` | the placeholder's fill character |
| `f.alternate(): bool` | the `#` flag |
| `f.sign_plus(): bool` | the `+` flag |

A user `fmt` chooses what to honor (the `+`, `#`, and `0` flags are passed through for it to consult with
`f.sign_plus()`, `f.alternate()`, and the width): `f.write_str` ignores the specifier, `f.pad` applies it, and a type may read
`f.width()` and so on to do something else. When the program calls `x.fmt(mut f)` on a built-in type, the standard
library's helper formats it.

The compiler lowers `"a {x} b"` to a block equivalent to

```burdin
{
    let mut f = Formatter.new()
    f.write_str("a ")
    x.fmt(mut f)
    f.write_str(" b")
    f.finish()
}
```

with `f.set_spec(...)` before and `f.reset_spec()` after a value whose placeholder has a specifier, and `debug`
instead of `fmt` for `{:?}`. The block's value is a `String`.

## FM8. Floats

A float without a precision prints the shortest text that reads back as the same number: `0.1` stays `0.1`,
`1.0 / 3.0` prints `0.3333333333333333`, and `1.0e21` prints `1000000000000000000000` (no exponent notation). An
`f32` prints the shortest text that reads back as the same `f32`. With a precision, the digits are rounded to
nearest; `2.0` with `{:.1}` is `2.0`. `NaN`, `inf`, and `-inf` print as `NaN`, `inf`, and `-inf`, with the sign
and the width honored.

The shortest text is found at run time by formatting with `snprintf("%.*e")` and reading the text back with
`strtod` until it matches, which is the C library's own round-trip rule; the fixed-precision path is one
`snprintf("%.*f")`.

## FM9. Integers in other radixes

`{n:x}`, `{n:X}`, `{n:b}`, and `{n:o}` format the bits of the value. An unsigned value shows its value; a signed
value shows its two's complement in its own width, so `-1` as `i8` is `ff` and as `i64` is
`ffffffffffffffff`. The `#` prefix is `0x`, `0b`, or `0o`. There is no precision, and the `+` and `0` flags apply
to the sign and the padding as in decimal.

## FM10. Where the text goes

`print` and `eprint` write through the same buffered stdout and unbuffered stderr as before
(`spec/stdlib.md`, Output): the text is one write, and the flush points are unchanged. The `Stdout` and `Stderr`
handles write into the same buffer and are subject to the same rules (`spec/stdlib.md`, Streams).
