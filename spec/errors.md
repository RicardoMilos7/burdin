# Errors (milestone 16)

This is the behavior of `error` declarations, the dynamic `AnyError`, the default `Result<T>`, `context`, error
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
package visibility of an ordinary enum (`priv`, a comma or a line break between variants, `: A, B` after the name).

Lowering is exactly:

- the enum itself, with the same variants (`IoError` above is `ConfigError.Io(IoError)`);
- `Name: Display` from the message templates (ERR2);
- `Name: Error` (ERR4), the marker that makes the type an error type;
- one `Name: From<Source>` block per `from` variant (ERR3).

The interfaces after the name are placed on the enum, so `error enum E: Debug` composes with the generated impls. Because
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
(`spec/stdlib.md`). Only the error side of a `?` has to fit the enclosing function: the operand's success type is
unwrapped whatever it is, so a `Result<usize, E>` is unwrapped inside a function that returns `Result<(), E>`, and an
`Option<i32>` inside one that returns `Option<String>` (`m16_errors/question_other_ok_type`).

## ERR4. The dynamic `AnyError`

`std` declares the interface and the type:

```
interface Error requires Display { }

struct AnyError {
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
`AnyError` is the dynamic error value, a struct of its own name, so the interface and the value never share one.
`any Error` is the boxed error, and an error type is boxed implicitly wherever the expected type is `AnyError`, like any other `any Interface` conversion
(`spec/existentials.md`, AN3): an annotation, an argument, a return, a field, an element, a `Some`/`Ok`/`Err`
payload, or a match arm.

The dynamic value supports the ordinary inspection surface:

- `e.is<T>()` is true when the box holds a `T`;
- `e.downcast<T>()` yields `Some(value)` when it holds a `T` and consumes the box;
- `{}` prints the wrapped error's `Display`;
- moving, dropping, and passing an `AnyError` are ordinary moves.

A `?`, an `Err(...)`, an annotation, or a `.context(...)` that converts an error type into `AnyError` boxes the
value and keeps it as `value`; the original error survives, so a downcast recovers it.

## ERR5. The default `Result<T>`

`Result<T>` is written with one argument and means `Result<T, AnyError>`; `Result<T, E>` keeps its two-argument
form. `Ok`, `Err`, the patterns, `is_ok`, `is_err`, `unwrap`, and `expect` work unchanged, and `?` in a function
returning `Result<U>` converts any error type into the dynamic error of ERR4.

## ERR6. `context`

`res.context(message)` is a compiler-known method on `Result<T, E>`. Its argument is a `str`; it returns
`Result<T, AnyError>`.

- `Ok(value)` passes through unchanged, and the message expression is evaluated only when there is an error.
- `Err(e)` becomes an `Err` whose dynamic `AnyError` keeps the original box in `value` and records `message` in
  front of any context messages it already had. Printing it prints the contexts outermost first, each followed by
  `: `, then the source error, so a chain prints in order:
  `loading settings: cannot read config: file not found`. Because the original error stays boxed, `downcast`
  still recovers it after `.context`.
- The error type `E` must be an error type (or `AnyError` itself); otherwise the call is E0212.
- The context wrapper keeps the dynamic error's trace frames.

`Option` has no `context` in stage 1.

## ERR7. Traces

A debug build (`burdin build`/`run`/`test` without `--release`) records a `TraceFrame` in the dynamic `AnyError`:

- when an error type is converted into `AnyError` (the creation site), and
- at every `?` that propagates a `Result<..., AnyError>` outward (the propagation site).

Frames are in order, creation first, and hold the function the error passed through, and the source file path,
line, and column of the expression that caused the conversion (`err?` reports the `?`). `.context` keeps the
frames it receives. A release build records none: the `frames` list stays empty and no frame is allocated. The
`AnyError` value has the same layout in both profiles; only the recording differs.

`error.frames` is a public field (a `List<TraceFrame>`), and `TraceFrame` implements `Display` as
`at {func} ({file}:{line}:{col})` (`spec/debug.md`, DBG3). `error.print_trace()` prints each frame on its own
line to standard error, and the generated `main` of `fn main(): Result<T>` (ERR8) and a failing test's wrapper
call it after printing the message.

## ERR8. `fn main(): Result<T>`

A `main` whose return type is `Result<...>` is accepted, written directly or through a type alias declared in the same
file that expands to one (`spec/type-aliases.md`, AL7):

- `Ok(_)` prints nothing and the process exits with status 0;
- `Err(e)` prints `e` with `Display` to standard error, then the recorded trace frames, one per line
  (`spec/debug.md`, DBG3), and exits with status 1. A release build recorded no frames, so its output is the
  message alone.

The printed error needs `Display` (E0193 at the `main` declaration otherwise). An alias of a `Result` that is
imported from another module is E0003 (main's signature rule): the wrapper is written when the file that declares
`main` is parsed, before imports resolve, so it cannot see that alias. `fn main()` returning unit keeps working. A
`main` with parameters or any other return type, an alias that does not expand to `Result`, is still E0003.

`?` inside `main(): Result<T>` converts error types into the default `AnyError` (ERR4/ERR5) exactly as in any other
function.

## ERR9. Typed test failures

`std` declares `AssertError` as an error enum with a payload:

```
error enum AssertError {
    Failed { condition: String, location: String }
        "assertion failed: {condition} at {location}"
    FailedMsg { condition: String, message: String, location: String }
        "assertion failed: {condition} at {location}: {message}"
    FailedEq { condition: String, left: String, right: String, location: String }
        "assertion failed: {condition} at {location}: {left} != {right}"
    FailedEqMsg { condition: String, left: String, right: String, message: String, location: String }
        "assertion failed: {condition} at {location}: {left} != {right}: {message}"
    FailedNe { condition: String, left: String, right: String, location: String }
        "assertion failed: {condition} at {location}: {left} == {right}"
    FailedNeMsg { condition: String, left: String, right: String, message: String, location: String }
        "assertion failed: {condition} at {location}: {left} == {right}: {message}"
}
```

In a `burdin test` build, `assert(cond)`, `assert_eq(a, b)`, and `assert_ne(a, b)` (with or without a message)
return `Result<(), AssertError>` and build one of these values at the call site: `condition` is the source text of
the whole call (`assert_eq(add(1, 1), 3)`), `location` is the call's `path:line:col`, `message` is the formatted
message of the message form (the variant without `Msg` is used when the call wrote no message), and `assert_eq`/
`assert_ne` store both values formatted with `Debug` (a type without `Debug` is E0141 at the call, because the
test-mode function's signature requires it). A `test` block is compiled as a function returning `Result<()>`, so
`assert(...)?` converts the `AssertError` into the dynamic error, and any other error type
(`read_to_string(...)?` and so on) converts there too. A test that returns `Err` prints the error's message to
standard error before it reports the failure, so `burdin test` shows it with the captured output. A normal build
keeps the panicking `assert`/`assert_eq`/`assert_ne` unchanged. The `debug_assert*` assertions keep their
debug-or-release behavior in a test build: they are not replaced, a release build drops them, and a debug-build
failure is a panic, which the runner reports as a failed test (`stdlib.md`, "Memory helpers and assertions").

## ERR10. The standard-library errors

```
error enum IoError {
    Read { path: String, kind: IoErrorKind, os_code: Option<i32> }      "cannot read {path}: {kind}"
    Write { path: String, kind: IoErrorKind, os_code: Option<i32> }     "cannot write {path}: {kind}"
    Run { command: String, kind: IoErrorKind, os_code: Option<i32> }    "cannot run {command}: {kind}"
    Net { op: String, addr: String, kind: IoErrorKind, os_code: Option<i32> } "cannot {op} {addr}: {kind}"
    UnexpectedEof             "unexpected end of input"
    WriteZero                 "the writer accepted no bytes"
    InvalidData               "the bytes are not valid text"
    Seek                      "seek to a position before the start"
    InvalidInput              "invalid open options"
}

error enum ParseIntError {
    Invalid  "invalid integer"
}

error enum EnvError {
    Name { name: String }        "invalid environment variable name {name}"
    Value { name: String }       "invalid value for environment variable {name}"
    Access { name: String }      "cannot access environment variable {name}"
    WorkingDirectory             "cannot read the working directory"
    SetWorkingDirectory          "cannot set the working directory"
}
```

`read_to_string` returns `Result<String, IoError>` with `IoError.Read`; `write_file` returns
`Result<(), IoError>` with `IoError.Write`; `process.run` returns `Result<i32, IoError>` with `IoError.Run`; a socket
operation returns `IoError.Net` with the operation's verb and the address it used (`io.md`, IO5).
The `Stdin` handle's `read_line` and `read_to_string` return `IoError.Read` with the path `<stdin>` (`stdlib.md`,
Streams). The reader and writer interfaces return `UnexpectedEof`, `WriteZero`, `InvalidData`, and `Seek` (`stdlib.md`,
Readers and writers). `i32.parse` (and the other integer parsers) returns `Result<T, ParseIntError>` with `Invalid`. The
directory operations (`read_dir`, `metadata`, `canonicalize`, `walk`, and `walk_glob`) use `IoError.Read`, and the
ones that change the file system (`create_dir`, `create_dir_all`, `remove_dir`, `remove_dir_all`, `remove_file`, and
`rename`) use `IoError.Write` (`stdlib.md`, "Directories, metadata, and walking"). A file handle reports its
operations' failures as `IoError.Read` or `IoError.Write` with the path it was opened with, a seek before the start
as `IoError.Seek`, and an option set `OpenOptions` rejects as `IoError.InvalidInput` (`stdlib.md`, "Files and file
handles"). The environment functions of
`std.env` return `EnvError`: `Name` for an invalid name, `Value` for an invalid value, `Access` when the system
refuses a change, `WorkingDirectory` when the directory cannot be read as text, and `SetWorkingDirectory` when it
cannot be changed (`stdlib.md`, "Environment variables and the working directory"). A child's `spawn` failure
(including a missing program and an argument or environment entry the system cannot take) is `IoError.Run` with the
program name; reading a piped child stream reports `<stdout>`, `<stderr>`, or `<child>`, and writing the child's
standard input reports `<stdin>` (`stdlib.md`, "Child processes").

The message ends with the kind's text, so a missing file prints `cannot read missing.txt: not found`. `kind()` is
the portable classification of ERR11, and `os_code()` is the operating system's own code when a system call reported
the failure. The unit variants (`UnexpectedEof`, `WriteZero`, `InvalidData`, `Seek`, and `InvalidInput`) are failures
`std` observed itself: they carry no path and no code, so `os_code()` is `None` and `kind()` maps them onto the
portable kinds (ERR11). The other failures `std` detects have `os_code() == None` too; the ones that exist today are
`remove_dir_all` on a path that is not a directory (`IoErrorKind.NotADirectory`), a symbolic link `remove_dir_all`
cannot remove (`IoErrorKind.Other`), and a process or pipe failure whose runtime path has no OS code to report.

The compiler primitives behind the I/O functions (`__read_to_string`, `__write_file`, `__file_open` and the other
file-handle primitives, `__run`, `__read_dir`, `__create_dir`, `__remove_dir`, `__remove_file`, `__rename`,
`__metadata`, `__canonicalize`, `__stdin_read_line`, `__stdin_read_to_string`, `__stdin_avail`, `__process_spawn`,
`__pipe_read_to_string`, `__pipe_read_line`, and `__pipe_read_both`) are typed `Result<..., i32>`: the error is the
operating system's code, which the `std` wrapper turns into the `IoError` with `__io_kind(code)` (ERR11).
`__stdin_avail` and `__file_*` report the code separately through `__stdin_error` and `__file_error`, because their
results are the byte counts or a descriptor. The process and pipe primitives report 0 when their runtime path has no
code, and `std` maps 0 to `IoErrorKind.Other` with `os_code() == None`. The integer parsers stay typed with
`ParseIntError` and produce a placeholder error value, which the `std` wrapper replaces with `Invalid`.

## ERR11. I/O error kinds and OS codes

The kind is portable; the code is not. `std` declares the kinds once and `IoError` carries one:

```
enum IoErrorKind: Copy, Clone, Eq, Debug {
    NotFound  PermissionDenied  AlreadyExists  IsADirectory  NotADirectory  DirectoryNotEmpty
    InvalidInput  InvalidData  UnexpectedEof  Interrupted  BrokenPipe  WouldBlock  TimedOut
    ConnectionRefused  ConnectionReset  ConnectionAborted  NotConnected
    AddrInUse  AddrNotAvailable  NetworkUnreachable  HostUnreachable  Other
}
```

`IoErrorKind` implements `Display` with lowercase text, and that text is what an `IoError` message ends with:
`not found`, `permission denied`, `already exists`, `is a directory`, `not a directory`, `directory not empty`,
`invalid input`, `invalid data`, `unexpected end of file`, `interrupted`, `broken pipe`, `would block`, `timed out`,
`connection refused`, `connection reset`, `connection aborted`, `not connected`, `address already in use`,
`address not available`, `network unreachable`, `host unreachable`, and `other`. The message is stable, so a test
can compare it.

`IoError.kind()` returns the kind and `IoError.os_code()` returns `Some(code)` when a system call reported the
failure. The compiler maps a code with the target's table (`burdin_tir::ioerror`): `errno` on Linux and macOS, and
the Win32 error code (`GetLastError`, which the CRT also leaves in `_doserrno` for its file functions) on Windows.
The codes that matter are:

| Kind | Linux / macOS | Windows |
| --- | --- | --- |
| `NotFound` | `ENOENT` | `ERROR_FILE_NOT_FOUND`, `ERROR_PATH_NOT_FOUND`, `ERROR_INVALID_DRIVE` |
| `PermissionDenied` | `EACCES`, `EPERM`, `EROFS` | `ERROR_ACCESS_DENIED`, `ERROR_WRITE_PROTECT`, `ERROR_SHARING_VIOLATION` |
| `AlreadyExists` | `EEXIST` | `ERROR_ALREADY_EXISTS`, `ERROR_FILE_EXISTS` |
| `IsADirectory` | `EISDIR` | none: removing a directory is `ERROR_ACCESS_DENIED`, so `PermissionDenied` |
| `NotADirectory` | `ENOTDIR` | `ERROR_DIRECTORY` |
| `DirectoryNotEmpty` | `ENOTEMPTY` (39 on Linux, 66 on macOS) | `ERROR_DIR_NOT_EMPTY` |
| `InvalidInput` | `EINVAL`, `ENAMETOOLONG`, `ELOOP` | `ERROR_INVALID_PARAMETER`, `ERROR_INVALID_NAME`, `ERROR_FILENAME_EXCED_RANGE` |
| `InvalidData` | `EILSEQ`, `ENOEXEC` | `ERROR_INVALID_DATA`, `ERROR_CRC` |
| `UnexpectedEof` | none | `ERROR_HANDLE_EOF` |
| `Interrupted` | `EINTR` | `ERROR_OPERATION_ABORTED` |
| `BrokenPipe` | `EPIPE` | `ERROR_BROKEN_PIPE`, `ERROR_NO_DATA` |
| `WouldBlock` | `EAGAIN` (11 on Linux, 35 on macOS) | `ERROR_IO_PENDING` |
| `TimedOut` | `ETIMEDOUT` (110 on Linux, 60 on macOS) | `ERROR_SEM_TIMEOUT`, `ERROR_TIMEOUT`, `WSAETIMEDOUT` |
| `ConnectionRefused` | `ECONNREFUSED` (111 on Linux, 61 on macOS) | `WSAECONNREFUSED` |
| `ConnectionReset` | `ECONNRESET` (104 on Linux, 54 on macOS) | `WSAECONNRESET` |
| `ConnectionAborted` | `ECONNABORTED` (103 on Linux, 53 on macOS) | `WSAECONNABORTED` |
| `NotConnected` | `ENOTCONN` (107 on Linux, 57 on macOS) | `WSAENOTCONN` |
| `AddrInUse` | `EADDRINUSE` (98 on Linux, 48 on macOS) | `WSAEADDRINUSE` |
| `AddrNotAvailable` | `EADDRNOTAVAIL` (99 on Linux, 49 on macOS) | `WSAEADDRNOTAVAIL` |
| `NetworkUnreachable` | `ENETUNREACH` (101 on Linux, 51 on macOS) | `WSAENETUNREACH` |
| `HostUnreachable` | `EHOSTUNREACH` (113 on Linux, 65 on macOS) | `WSAEHOSTUNREACH` |
| `Other` | every other code | every other code |

The socket primitives also report the resolver's own codes (`EAI_NONAME`, `WSAHOST_NOT_FOUND`) and the Win32 codes
that a socket call can leave; a code no table lists is `Other`. A network failure that `std` detects itself (an
address that does not parse, a timeout that passed) has `os_code() == None`, so a program that wants to tell the two
apart looks at the code instead of guessing from the kind.

The numeric values are the platform's; a program that compares `os_code()` with a number is platform-specific and
should ask `target.os()` first (`comptime.md`, CT5), while `kind()` is the same on every platform.

The reader and writer failures of #409 carry no path and no OS code, so they stay unit variants and `os_code()` is
`None` for them. `kind()` still classifies every one: `UnexpectedEof` and `InvalidData` are their own kinds,
`Seek` (a position before the start) is `InvalidInput`, and `WriteZero` (a sink that takes no bytes) is `Other`,
because neither is an operating-system condition with a portable kind of its own.

Two decisions are recorded here (issue #701):

- **The operation and the path stay variants.** `Read`, `Write`, and `Run` already say which operation failed, and
  each carries its own `path` or `command`. A single struct with an `operation` field would make every match write
  the operation again, let a `Run` carry a path, and buy nothing: the three payloads differ only in that name.
  `kind` and `os_code` are fields of all three, so `kind()` and `os_code()` are the same method on every variant.
  The reader and writer failures that have neither a path nor a code stay unit variants for the same reason.
- **`UnexpectedEof` and `InvalidData` are I/O kinds, and #409's reader variants map to them.** The reader/writer
  layer returns `IoError` too, so a short read or malformed bytes need no second error type and every caller keeps
  one `Result<..., IoError>`. An OS code that means malformed data maps to `InvalidData`, and Windows' EOF code maps
  to `UnexpectedEof`; the reader layer produces either without an OS code, which `os_code()` reports as `None` and
  `kind()` still classifies.

## Diagnostics

| Code | Meaning |
| --- | --- |
| E0210 | Invalid `error` declaration: a `from` variant that is not a tuple variant with exactly one field, `from` on a generic error enum, or a message placeholder that does not name a payload field (or a fill character that cannot be written as source) |
| E0211 | Two variants of one error enum convert from the same source type |
| E0212 | An error conversion is missing: `?` between two error types with no `From` block, or an implicit conversion into `AnyError` from a type that does not implement `Error` |

`?` that is not in a function returning `Option`/`Result` is still E0160, an unknown interface or type is E0002,
and a value that does not implement `Display` where a message needs it is E0193.
