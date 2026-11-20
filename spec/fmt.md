# The Burdin formatter

This file is the behavior of `burdin fmt` (stage 3, issues #229, #460, #461, #462, #463, #659, #660). The formatter is
opinionated and has **no options**: one style, so every Burdin program looks the same. The syntax it reads is in
[`grammar.md`](grammar.md); the driver contract is summarized in [`README.md`](../README.md#burdin-fmt); the black-box
checks are in [`tests/check_fmt.py`](../tests/check_fmt.py) with the fixtures in [`tests/fmt/`](../tests/fmt), and the
unit and corpus tests are in [`crates/burdin_fmt`](../crates/burdin_fmt).

The formatter lives in its own crate, `burdin_fmt`, which depends only on `burdin_diag` and `burdin_syntax`. It reads
tokens with the compiler's lexer and checks its input and its output with the compiler's parser, but it lays the
program out with its own parser, because the compiler's syntax tree throws away what a formatter must keep: comments,
parentheses, the order of `use` declarations, and how a `select` was written.

## FM1: what is formatted, and what is guaranteed

`format_source` takes the text of one file. The result:

- **keeps the program.** The output parses to the same syntax tree as the input, ignoring source positions. `use`
  declarations are compared by what they import, because FM9 merges and sorts them. A formatter that cannot show this
  returns an internal error and the file is left as it is; the output is never written.
- **keeps every comment.** Every comment of the input is in the output, once, with its text unchanged (FM10 is the
  one exception: the code in a doc comment's fenced block is reformatted).
- **is idempotent.** Formatting the output gives the output back, byte for byte. A comment in an unusual place can move
  the first time (it was between two tokens and ends up at the end of a line), so the layout is repeated until it stops
  changing, at most four times, and every round is checked against the source; a layout that does not settle is an
  internal error.
- **is deterministic.** The same input gives the same output on every platform.
- **ends with exactly one line break**; an empty file stays empty, and a file of only comments keeps them. Trailing
  white space is removed from every line. Line endings are kept: a file that uses `\r\n` throughout still does after
  formatting, and any other file uses `\n`.

A file with a syntax error is not formatted: the error is reported with the compiler's own diagnostics (the same codes
and spans as `burdin build`) and nothing is written.

## FM2: lines and indentation

Indentation is four spaces per level and never a tab. Lines are at most **120 columns** wide. A construct that fits on
the line it started on stays on it; one that does not breaks at its outermost bracket first, and breaks the next level
inside only when that is still too long. A single token that is wider than the line (a long string or a long name) is
never split.

When a bracketed list breaks, it goes one element per line, indented one level, with the closing bracket on its own
line and a trailing comma after the last element. A list that fits on one line has no trailing comma, except a
one-element tuple, which keeps its comma.

```
let point = Point { x: 1, y: 2 }
let origin = Point {
    x: 0,
    y: 0,
}
```

## FM3: comments

A comment is attached to a token: a comment on the same line as the token before it, with a line break after it,
**trails** that token; every other comment **leads** the token after it. The rules that follow from this:

- A comment on its own line stays on its own line at the same nesting level as the code it precedes, in front of an
  item, a statement, a member, a variant, a field, a match arm, or a list element that starts a line.
- A trailing comment stays at the end of its line, one space after the code. A trailing comment never has code after it
  on its line, so the construct it ends is broken across lines if it must be.
- A comment before a closing bracket (`}`, `)`, `]`) stays inside the brackets, on its own line, after the last element.
- A block comment (`/* */`) written inline keeps the spacing it had on each side. A block comment that spans lines is
  copied as written.
- Where the grammar does not allow a line break (inside a statement that continues no further), an own-line comment is
  moved to the end of that line. The comment is kept and its text is not changed.
- A line comment keeps its text and loses its trailing white space.

## FM4: blank lines

The blank lines of the source are kept, at most one in a row: between items, between statements, between members of a
type, between the comments and the code. Blank lines at the start of a block or the end of a block are removed. A blank
line is never added between two statements, except FM9 puts one between the groups of `use` declarations.

## FM5: items

- `fn`: `fn name<T>(params): Type where bounds {`. The parameters break one per line when the signature is too long;
  the return type stays on the line of the closing parenthesis. A `where` clause stays on the signature's line when it
  fits; otherwise `where` starts a new line indented one level and the bounds go one per line.
- `struct`, `enum`, `error enum`, `interface` and an `impl` block put every field, variant, or member on its own
  line, even when the declaration would fit on one line, except an empty body, which is `{}`. A `struct` or `enum`
  member always ends with a comma, and a blank line between two members is kept.
- An `impl` block `Type: Interface where bounds { ... }` follows the same `where` rule as a function.
- The interfaces after a struct's or enum's name are written `Name<T>: A, B`, with no space before `:` and one
  after it and after each comma, all on the declaration's line. `repr(...)`, `const`, `type`, `extern`, `comptime`, `test`, `derive` and attribute lists keep their
  tokens and take one space between them.

## FM6: statements and blocks

- One statement per line. A `;` that ends a statement is removed, except where the next statement starts with `||`,
  which would otherwise continue the expression.
- A block holds its statements one per line. A **short block** is a block of one expression statement with no comment
  in it; where a block appears inside an expression (a match arm, a closure body, an `if` that is an operand) a short
  block stays on one line with its braces when it fits: `Some(x) => { f(x) }`. The block of a statement-level `if`,
  `while`, `for`, `loop`, `match` or function is never inline.
- `if`/`else if`/`else` chains put `else` on the line of the closing brace, and `match` puts the arms one per line.
- `let pattern: Type = value` keeps the type annotation when written, and breaks after `=` only when the value is a
  chain of binary operators that does not fit.

## FM7: expressions

- A call or an index breaks its arguments one per line when they do not fit. When only the last argument is a closure
  or a struct or array literal, the call **hugs** it: `run(a, b, |x| {` followed by the body and `})`.
- A chain of binary operators breaks before `&&`, `||`, and `orelse`, and after every other operator, with the
  continuation line indented one level.
- A method chain with two or more calls breaks before each `.` when it does not fit on one line; a chain with a
  single call never does.
- A struct literal, an array literal, a tuple, a closure's parameter list, and the generic arguments of a type break
  like any bracketed list (FM2).
- A string literal and an interpolation's text are never changed.
- Parentheses written by the author are kept; the formatter does not add or remove them.

## FM8: the trailing comma

A list that already has a trailing comma **and** whose closing bracket is on a later line than its opening bracket
stays broken, one element per line, even if it would fit on one line. This is the escape hatch for a list the author
wants to read vertically; removing the trailing comma (or putting the closing bracket on the opening line) lets the
formatter join it again. A list that breaks because it is too long gets the trailing comma (FM2).

## FM9: `use` declarations

A run of `use` declarations (adjacent, with no other item between them, blank lines allowed) of the same visibility
(`use`, or `pub use`) is rewritten as follows:

1. **Duplicates are removed**, and two brace lists with the same path are merged: `use a.{b}` and `use a.{c}` become
   `use a.{b, c}`. A plain `use a.b` is never merged into a brace list: it may name a module, which `use a.{b}` does
   not import, so merging could change the meaning. An alias (`use a.b as c`) is never merged with anything but an
   identical alias.
2. The names inside a brace list are sorted, and the list breaks like any other list when it is too long (FM2).
3. **Groups.** The declarations are placed in three groups separated by one blank line: first the standard library
   (a path whose first name is `std` or one of the standard modules), then the dependencies named in the package's
   `burdin.toml`, then the package itself. A group with no declaration has no blank line. Inside a group the
   declarations are sorted by path.
4. **Comments travel with their declaration.** A comment that leads a `use` stays in front of it, and a comment that
   trails it stays after it. A declaration with a comment is never merged with another, so no comment is dropped or
   moved to another declaration.
5. A `pub use` run is sorted and grouped on its own, and never mixed with plain `use` declarations: visibility is part
   of what a declaration means.

Formatting never changes what a name resolves to. A run of `use` declarations that is not at the top of the file is
sorted where it stands.

## FM10: code blocks in doc comments

A fenced code block in a run of `///` lines, whose fence is empty, `burdin`, or `bd` (attributes may follow the
word, as in `burdin,no_run`; the word is what the doc tests read, DC6), holds Burdin code and is formatted:

```
/// ```
/// let x=add(1,2)
/// ```
```

becomes

```
/// ```
/// let x = add(1, 2)
/// ```
```

- The code is formatted first as a list of items (a file); when that does not parse it is formatted as the statements
  of a function body.
- The code gets the width left after the indentation and the `/// ` prefix, so a formatted line never passes column 120
  (a line the formatter cannot split may).
- A block whose fence names another language or word (`text`, `console`, `rust`, ...), a block that does not parse, a
  block whose formatted form is not the same program, a block not closed by a fence, an empty block, and a block with a
  hidden `# ` setup line (DC6: the marker is not Burdin text, so the block cannot be checked against the formatted
  program) are left as written. A block that is not formatted is never reported as an error.
- Only `///` comments are searched; a fence in a `//` or `/* */` comment is an ordinary comment.

## FM11: formatting a range

`burdin fmt --stdin --range lo:hi` formats only the top-level items that overlap the byte range `lo..hi` (a range
inside a method formats the whole `impl` block it is in) and copies the rest of the file exactly. It is meant for an
editor's "format selection". `use` declarations are not sorted or merged in a range, because that would move text
outside the range. Offsets are bytes of the original text. An item that is not touched by the range, and the white
space between items, are unchanged, so formatting a range inside an already formatted file changes nothing.

## FM12: the command

```
burdin fmt [<file.bd | directory>]... [--check] [-j <n>] [diagnostics]
burdin fmt -p <member>... | --workspace [--exclude <member>]... [--check] [-j <n>] [diagnostics]
burdin fmt --stdin [--stdin-filepath <path>] [--range <lo>:<hi>] [--check] [diagnostics]
```

- In a manifest-level workspace (`spec/workspaces.md`, WS13), `-p`, `--workspace`, and `--exclude` select the
  members whose directories are formatted, found from the working directory; they cannot be combined with paths or
  `--stdin`. Two members and the directories between them may hold the same file, and a file is formatted once.
- With paths, `burdin fmt` formats each `.bd` file in place. A directory means every `.bd` file under it, in sorted
  order, skipping directories that start with `.` and `target`; with no path it means the current directory. A file
  that is already formatted is not written (its modification time is unchanged). The path of every file the command
  changed is printed on standard output, one per line.
- Files are read and formatted on up to `-j` threads (default: `BURDIN_JOBS`, then the number of CPUs). Reports,
  writes, and the exit status follow the sorted file order whatever the thread count, so `-j 1` and `-j 8` print
  the same bytes and leave the same files.
- `--check` writes nothing. It prints the path of every file that is not formatted, one per line on standard output,
  prints a one-line summary on standard error, and exits with status 1 when there is at least one. It is meant for CI.
- `--stdin` reads one source from standard input and writes the formatted source to standard output (nothing else is
  written there). `--stdin-filepath` says which file the source stands for: its name appears in diagnostics and the
  `burdin.toml` above it tells FM9 which names are dependencies. With `--check`, nothing is written to standard
  output except the name of the file when it would change.
- `--range` selects FM11.
- The diagnostic options (`--diagnostic-format`, `--color`, `--no-color`, `--width`, `--error-limit`) apply to syntax
  errors, which are reported exactly as `burdin build` reports them. `--diagnostic-format sarif` is the exception: `fmt`
  reports file by file and a SARIF log is one document, so it is a usage error (`spec/diagnostics.md`, DG11).

**Exit status.** 0: everything is formatted (or was formatted). 1: `--check` found files that are not formatted.
2: a usage error, a file or standard input that cannot be read or written, a syntax error in a file, or an internal
formatter error. When some files fail, the others are still formatted. A file that fails is never modified.

## FM13: how the formatter is tested

- `crates/burdin_fmt/tests/corpus.rs` formats every `.bd` file of `std/` and `tests/` that parses and requires FM1:
  no internal error, and the output is a fixed point.
- `crates/burdin_fmt/tests/fuzz.rs` formats the lexer/parser fuzz corpus, the formatter's fixtures, and 20,000
  deterministic mutations of them (`BURDIN_FMT_FUZZ_SEED` and `BURDIN_FMT_FUZZ_MUTATIONS` choose a longer session, which
  the scheduled `fuzz` CI job runs); every input that parses must format to a fixed point without an internal error.
  `crates/burdin_fmt/tests/regressions.rs` keeps the inputs the fuzzer found.
- `tests/check_fmt.py` runs `burdin fmt` as a black box over the fixtures in `tests/fmt/`: each `name.input` has a
  `name.golden` text it must turn into. (The fixtures do not end in `.bd`, because the conformance runner treats every
  `.bd` file under `tests/` as a test.) The script checks the golden output, its idempotence, `--check`, `--stdin`,
  `--stdin-filepath`, `--range`, directories and packages, exit statuses, Windows line endings, syntax errors, and a
  set of tricky sources (comments in awkward places and other inputs formatters get wrong).
- A change to the style regenerates the goldens (`burdin fmt --stdin < tests/fmt/name.input > tests/fmt/name.golden`)
  and the diff is reviewed in the same change as this file.
