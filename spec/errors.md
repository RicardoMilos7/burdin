# Errors (milestone 16)

This is the behavior of `error` declarations, the dynamic `Error`, the default `Result<T>`, `context`, error
traces, `fn main(): Result<T>`, and typed test failures. The tests in `tests/m16_errors/` are the executable form.
The design document's "Error handling" section is the source; this page settles how it lowers.

An `error` declaration is an ordinary enum plus generated impls. The checker, the memory checks, and code
generation see only the enum and the impls, so nothing about an error value is special at run time.

## ERR1. The declaration

```
error enum ConfigError {
    Io(IoError)          from "cannot read config: {0}"
    Syntax { line: i32 }      "syntax error on line {line}"
    Missing(String)           "missing key {0}"
    Unknown                   "unknown configuration"
}
```

`error` is a keyword; `enum` follows it. The declaration takes the permissive layout, the derives, and the
package visibility of an ordinary enum (`priv`, a comma or a line break between variants, `derives (...)`).

Lowering is exactly:

- the enum itself, with the same variants (`IoError` above is `ConfigError.Io(IoError)`);
- `Name: Display` from the message templates (ERR2);
- `Name: Error` (ERR4), the marker that makes the type an error type;
- one `Name: From<Source>` block per `from` variant (ERR3).

A `derives (...)` clause is placed on the enum, so `derives (Debug)` composes with the generated impls. Because
the lowering produces ordinary items, an error type is an ordinary value: it is moved, dropped, matched, stored,
and compared like any enum, and it may be put in a `Result`, an `Option`, a `List`, or a box.

An `error` declaration may be generic, and its generated `Display` and `Error` impls carry a `where` clause
requiring `Display` of every type parameter. A generic error enum cannot declare `from` (E0210): the conversion
registry takes only concrete impls.

## ERR2. Message templates

The string literal after a variant is its message, and it is required. Its `{...}` placeholders name the payload
fields of that variant; the type checker checks the template exactly like an interpolated string literal
(`spec/format.md`), so `{0:?}`, `{0:>8}`, and `{}` work with the usual rules (E0190, E0193).

- A tuple variant names its fields by position: `{0}`, `{1}`, ..., and `{}` is the next position.
- A struct variant also names fields by name: `{line}` is the field `line`. A name that is not a field is E0210.
- A unit variant has no fields, so a placeholder on it is E0210.
- An index that is not a payload field is E0210, as is a placeholder expression that is not a single field name
  (`{a.b}`, `{a + 1}`).
- A fill character of `{`, `}`, `"`, or `\` cannot be written back as source and is E0210.

The generated `Display` matches the variant and writes the message with the payload fields bound, so printing an
error value prints its message and nothing else.

## ERR3. `from` conversions

`from` is written between the variant's payload and its message. The variant must be a tuple variant with
exactly one field; that field's type is the conversion's source type (E0210 otherwise). Two variants of one
error enum converting from the same source type are E0211.

The generated block is the ordinary `Name: From<Source> { fn from(value: Source): Name { Name.V(value) } }`, so
`?` converts a `Result<T, Source>` into a `Result<T, Name>` exactly as it converts through a hand-written block
(`spec/stdlib.md`).

## ERR4. The dynamic `Error`

`std` declares the interface and the type:

```
interface Error requires Display { }

struct Error {
    value: any Error
    frames: List<TraceFrame>
    context: String
}

struct TraceFrame {
    file: String
    line: i32
    col: i32
}
```

`Error` (the interface) is the marker: a type is an *error type* when it implements it, which requires `Display`.
The same name in the type namespace is the dynamic error value. `any Error` is the boxed error, and an error type
is boxed implicitly wherever the expected type is `Error`, like any other `any Interface` conversion
(`spec/existentials.md`, AN3): an annotation, an argument, a return, a field, an element, a `Some`/`Ok`/`Err`
payload, or a match arm.

The dynamic value supports the ordinary inspection surface:

- `e.is<T>()` is true when the box holds a `T`;
- `e.downcast<T>()` yields `Some(value)` when it holds a `T` and consumes the box;
- `{}` prints the wrapped error's `Display`;
- moving, dropping, and passing an `Error` are ordinary moves.

A `?`, an `Err(...)`, an annotation, or a `.context(...)` that converts an error type into `Error` boxes the
value and keeps it as `value`; the original error survives, so a downcast recovers it.

## ERR5. The default `Result<T>`

`Result<T>` is written with one argument and means `Result<T, Error>`; `Result<T, E>` keeps its two-argument
form. `Ok`, `Err`, the patterns, `is_ok`, `is_err`, `unwrap`, and `expect` work unchanged, and `?` in a function
returning `Result<U>` converts any error type into the dynamic error of ERR4.

## ERR6. `context`

`res.context(message)` is a compiler-known method on `Result<T, E>`. Its argument is a `str`; it returns
`Result<T, Error>`.

- `Ok(value)` passes through unchanged, and the message expression is evaluated only when there is an error.
- `Err(e)` becomes an `Err` whose dynamic `Error` keeps the original box in `value` and records `message` in
  front of any context messages it already had. Printing it prints the contexts outermost first, each followed by
  `: `, then the source error, so a chain prints in order:
  `loading settings: cannot read config: file not found`. Because the original error stays boxed, `downcast`
  still recovers it after `.context`.
- The error type `E` must be an error type (or `Error` itself); otherwise the call is E0212.
- The context wrapper keeps the dynamic error's trace frames.

`Option` has no `context` in stage 1.

## ERR7. Traces

A debug build (`burdin build`/`run`/`test` without `--release`) records a `TraceFrame` in the dynamic `Error`:

- when an error type is converted into `Error` (the creation site), and
- at every `?` that propagates a `Result<..., Error>` outward (the propagation site).

Frames are in order, creation first, and hold the source file path, line, and column of the expression that
caused the conversion (`err?` reports the `?`). `.context` keeps the frames it receives. A release build records
none: the `frames` list stays empty and no frame is allocated. The `Error` value has the same layout in both
profiles; only the recording differs.

`error.frames` is a public field (a `List<TraceFrame>`), and `TraceFrame` implements `Display` as
`file:line:col`.

## ERR8. `fn main(): Result<T>`

A `main` written directly as returning `Result<...>` is accepted:

- `Ok(_)` prints nothing and the process exits with status 0;
- `Err(e)` prints `e` with `Display` to standard error, then exits with status 1.

The printed error needs `Display` (E0193 at the `main` declaration otherwise). The source must write the
`Result` type directly; an alias that resolves to a `Result` is E0003 (main's signature rule). `fn main()`
returning unit keeps working. A `main` with parameters or any other return type is still E0003.

`?` inside `main(): Result<T>` converts error types into the default `Error` (ERR4/ERR5) exactly as in any other
function.

## ERR9. Typed test failures

`std` declares `AssertError` as an error enum with a payload:

```
error enum AssertError {
    Failed { condition: String, location: String }
        "assertion failed: {condition} at {location}"
    FailedEq { condition: String, left: String, right: String, location: String }
        "assertion failed: {condition} at {location}: {left} != {right}"
}
```

In a `burdin test` build, `assert(cond)` and `assert_eq(a, b)` return `Result<(), AssertError>` and build one of
these values at the call site: `condition` is the source text of the whole call (`assert_eq(add(1, 1), 3)`),
`location` is the call's `path:line:col`, and `assert_eq` stores both values formatted with `Debug` (a type
without `Debug` is E0141 at the call, because the test-mode function's signature requires it). A `test` block is
compiled as a function returning `Result<()>`, so `assert(...)?` converts the `AssertError` into the dynamic error,
and any other error type (`read_to_string(...)?` and so on) converts there too. A test that returns `Err` prints
the error's message to standard error before
it reports the failure, so `burdin test` shows it with the captured output. A normal build keeps the panicking
`assert`/`assert_eq` unchanged.

## ERR10. The standard-library errors

```
error enum IoError {
    Read { path: String }     "cannot read {path}"
    Write { path: String }    "cannot write {path}"
    Run { command: String }   "cannot run {command}"
}

error enum ParseIntError {
    Invalid  "invalid integer"
}
```

`read_to_string` returns `Result<String, IoError>` with `IoError.Read`; `write_file` returns
`Result<(), IoError>` with `IoError.Write`; `process.run` returns `Result<i32, IoError>` with `IoError.Run`.
The `Stdin` handle's `read_line` and `read_to_string` return `IoError.Read` with the path `<stdin>` (`stdlib.md`,
Streams). `i32.parse` (and the other integer parsers) returns `Result<T, ParseIntError>` with `Invalid`.

The compiler primitives behind them (`__read_to_string`, `__write_file`, `__run`, and the integer parsers) are
typed with the same error types but produce a placeholder error value; the `std` wrappers replace it with the
real variant. A program calls the `std` functions, not the primitives.

## Diagnostics

| Code | Meaning |
| --- | --- |
| E0210 | Invalid `error` declaration: a `from` variant that is not a tuple variant with exactly one field, `from` on a generic error enum, or a message placeholder that does not name a payload field (or a fill character that cannot be written as source) |
| E0211 | Two variants of one error enum convert from the same source type |
| E0212 | An error conversion is missing: `?` between two error types with no `From` block, or an implicit conversion into `Error` from a type that does not implement `Error` |

`?` that is not in a function returning `Option`/`Result` is still E0160, an unknown interface or type is E0002,
and a value that does not implement `Display` where a message needs it is E0193.
