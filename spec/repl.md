# The REPL

`burdin repl` is the interactive read-eval-print loop (stage 3; issue #578 and its sub-issues #580, #582, #584). It
is built on the comptime interpreter (`spec/comptime.md`), which already evaluates the typed tree with a step
budget, and it checks every input through the query-based compiler library (`spec/query.md`), so every diagnostic
is the one `burdin build` reports. This file is the exact behavior, with a rule ID for each point and the tests
that cover it.

The REPL lives in `crates/burdin_repl`; the driver calls it for `burdin repl` and hands it the two operations that
need the LLVM back end, `:compile` and `:ir` (RP10), so the crate itself never links LLVM. The black-box tests are
`tests/check_repl.py` with the golden sessions under `tests/repl/` (`BURDIN_UPDATE_GOLDEN=1` rewrites them), run
by every CI job that has a built driver. The crate's unit tests cover the session text, the classifier, the
highlighter, the history, and the editor's helpers.

## The session model

**RP1. One growing program.** A session is a package held in memory with one module file, `main.bd`, plus the
embedded standard library. The file is regenerated from the accepted inputs after every input:

```burdin
fn main() {
<the statements of every accepted statement input, in order>
}

<the module content of every accepted item input, in order>
```

`main` comes first so that the session's own declarations precede any item's in the interpreter's closure and
vtable lists: appending an item then keeps every index a carried value holds stable (`:ir` prints what the back
end emits for exactly this program). The session reserves the name `main`; an input that declares `fn main` is a
duplicate-definition error like any other. The file is analyzed with the query library in test mode, so `test`
inputs are declared and checked and no `main` signature is required, and the memo makes every input cost what it
touches: the body of `main`, plus everything an item edit can affect.
Tests: `tests/repl/items.session`, `closure.session`, `multiline.session`; `text.rs` unit tests.

**RP2. Classification, decided by the parser.** An input is one unit, possibly several lines. It is
- **module content** when it parses as a file of items (`fn`, `struct`, `enum`, `use`, `test`, ...), which lands
  below `main`;
- **statements** when it parses as a block, which lands in `main`'s body;
- **incomplete** when every parse error is at the input's end (the parser decides, never a count of brackets) or
  the lexer stopped at an unterminated string or block comment (`spec/grammar.md`, FM2 permits both across lines):
  the REPL reads another line, with the continuation prompt, until the input is complete;
- **bad** otherwise: it is appended as statements so the front end reports it in the session's context, and then
  discarded.
White space and comments alone are accepted and append nothing.
Tests: `text.rs` unit tests; `multiline.session`; `empty` and per-line behavior in every session.

**RP3. Evaluation by the interpreter, printing is real.** An accepted statement input is evaluated by the comptime
interpreter, which sees the program the checks accepted — every relevant diagnostic is therefore the front end's.
Printing — `print`, `println`, `eprint`, `eprintln`, the standard library's writer builtins, and `is_terminal` —
writes the process's real streams in a session (compile-time evaluation still rejects it, CT9). Every other
compile-time restriction stands: files, the environment, processes, tasks, threads, and C calls are E0224 with a
note that `:compile` builds the session and runs it natively. A panic is E0222 and the budget E0225, both reported
at the fault's own span. In every fault case the input is discarded and the session is unchanged (RP7).
Tests: `impure.session`, `panic_rollback.session`.

**RP4. Bindings persist; moves persist.** The interpreter carries the values of the body's top-level `let`
bindings from one input to the next, keyed by their local ids, evaluating only the statements a new input
appended. So a value is alive across inputs exactly as it would be inside one function body, and since the whole
body is checked every time, **using a moved binding in a later line reports the same E-code as inside a function
body** (E0100), with the note pointing at the move.
Tests: `moved.session`, `lets.session`, `closure.session`.

**RP5. An expression's value prints with `Debug`.** A bare expression of a non-unit type is not a statement in
Burdin — the front end answers with exactly one type error at it (`expected (), found T`, E0003) — and that answer
is the REPL's signal to print. The input's trailing expression is rewritten as

```burdin
let __repl_v =
<the expression, verbatim, at its own columns>

println("{(__repl_v):?}")
```

so the value is printed by the standard library's `Debug` machinery as interpreted code, byte for byte as the
compiled program prints it. The expression's own lines keep their columns (the wrap adds only whole lines), and
the value moves into `__repl_v` exactly as evaluating it in a function body would move it. An expression whose
type does not implement `Debug` (E0193) is bound without the `println`, evaluated, and a note names the type;
nothing is printed for it. An expression of type `()` or `!` prints nothing; a `let` prints nothing.
Tests: `values.session`, `debug_note.session`, `items.session`, `type_meta.session`.

**RP6. Items.** An item input is checked against the session as it stands and appended when it passes; it
evaluates nothing (a constant's initializer is evaluated by the analysis, so a failing one is reported and
discarded with the item).
Tests: `items.session`, `drop_reset.session`.

**RP7. A failed input leaves the session unchanged.** An input the front end rejects is reported and discarded.
An input whose evaluation faults is reported and discarded too, and the carried bindings are restored to the
values they held before it, so the session is always one that compiled and ran.
Tests: `moved.session`, `type_error.session`, `panic_rollback.session`, `impure.session`.

**RP8. Diagnostics and the exit status.** Diagnostics render to standard error in the driver's formats and
coloring (`--diagnostic-format`, `--color`, `--width`, `--error-limit`), against the session file; a diagnostic's
line is its line in `main.bd` as RP1 lays it out. The process exits 0 when every input was accepted and 1 when any
input, meta-command, or evaluation failed. Standard output carries only what the session printed, meta-command
answers, and REPL notes.
Tests: every golden `.out`/`.err`/`.exit` triple.

## Meta-commands

**RP9. `:type` and `:ownership`.** Both analyze the session with the expression appended as one more statement and
then discard it: nothing is evaluated and nothing moves. `:type <expr>` prints the expression's type, from the
typed tree; a type error in the expression is reported as the input would report it. `:ownership <expr>` prints
four lines — the type, what the expression reads, the places it moves (as `local` or `local:field.index`, the diff
of the checker's move records with and without the probe), and what it borrows (`mut x` for an exclusive borrow).
Tests: `type_meta.session`, `ownership.session`.

**RP10. `:ir <fn>` and `:compile`.** These are the driver's: `:ir` builds the session with `--emit-ir` and prints
the `define` block whose symbol the name matches (`burdin.add` for `fn add`; the back end drops what nothing
reaches, so a function the session has not called prints as `unreachable`, like any dead code in a build).
`:compile` builds the session as an ordinary one-file program and runs it as a child process with the terminal's
streams, so its output appears in addition to the interpreted one; a compile failure is reported and marks the
session failed.
Tests: `tests/check_repl.py` (`:ir` and `:compile`).

**RP11. The other meta-commands.** `:help` prints the list; `:quit` leaves; `:reset` runs the drops of the live
bindings, last-declared first (`spec/memory-rules.md`, DR1), so a user `Drop` implementation's side effects appear,
and then starts the session over; `:explain <code>` prints the same reference text `burdin explain` prints (the
page `burdin_diag` embeds, checked by `tests/check_error_docs.py`), and the code list with no argument; `:load
<file>` parses a file as a module on its own — its errors report with its path and lines — and appends its content
as one atomic item input. An unknown `:word` is an error.
Tests: `drop_reset.session`, `explain.session`, `load.session`, `tests/check_repl.py`.

## Interactive mode

**RP12. Only on a terminal.** When standard input is a terminal, `burdin repl` prints a banner and a prompt,
edits lines with the keys below, keeps history, highlights, and completes. When it is not — a pipe, a file — all
of that is off and there are no prompts, so a scripted session is exactly reproducible: that is the mode
`tests/check_repl.py` pins.

**RP13. The editor.** One line at a time in raw mode (`crates/burdin_repl/src/terminal.rs`: `libc`'s termios on
Unix, the console API on Windows): printable characters insert at the cursor, Backspace and Delete erase, the
arrows and Home and End move, Up and Down walk the history (the draft in progress is kept), Tab completes, Enter
submits, Ctrl-C abandons what is being typed and the whole pending input, Ctrl-D on an empty line leaves (on a
line it deletes forward). The line is redrawn after every key, colored by the token classes of
`editors/vscode/syntaxes/burdin.tmLanguage.json` — keywords and modifiers, `constant.language`, `constant.numeric`,
strings and characters, comments, primitive and declared types, call names, and reserved words — so what is colored
is what an editor would color.
Completion asks the query library: names in scope (the session's bindings with their types, the session module's
items and `use` imports, the standard library's root, keywords, primitive types), and after a `.` the fields,
variants, and methods of the receiver's type when the receiver is a session binding.
History is one line per entry in `<burdin home>/repl_history`, the most recent 1000 kept, repeats and empty lines
dropped. The Burdin home is `BURDIN_HOME` when set, else the platform's data directory for the user (RP14).
Multi-line input is RP2's, unchanged: the continuation prompt is `......> `.

**RP14. Where state lives.** `BURDIN_HOME` when it is set; otherwise `$XDG_STATE_HOME/burdin` (or
`~/.local/state/burdin`) on Linux, `~/Library/Application Support/burdin` on macOS, and `%APPDATA%\burdin` on
Windows. The script cache (`spec/scripts.md`, SC6) lives under the same home.

**RP15. The manual checklist.** The interactive features cannot be driven by a redirected standard input, so they
are checked by hand. Recorded here, run on 2026-11-19 against the driver on Ubuntu 26.04 (5.15 WSL) in a pty:

```
python3 - <<'EOF'  # a pty driver sending scripted keys with delays
... pty.fork(); write b"1 + 2\r", b"let a = 41\r", b"\x1b[A\r", b"a + 1\r", b"dou\t(3)\r", b"fn f() {\r", b"    a\r", b"}\r", b":quit\r"
EOF
```

- `1 + 2` printed `3`; each keystroke redrew the line with the colors above (`1` and `2` yellow, `let` blue);
- `let a = 41` printed nothing; `a + 1` printed `42`;
- Up arrow redrew `let a = 41`, and Enter ran it again;
- after `fn double(n: i32): i32 { n * 2 }`, typing `dou` and Tab completed `double`, and `(3)` printed `6`;
- `fn f() {` moved to the `......> ` prompt, the body line and `}` completed the item;
- the history file held every entered line; `:quit` left with status 0 and the terminal restored.

Not yet checked on Windows and macOS consoles (the raw-mode code is `cfg(windows)` and untested here), and not
with a real human at a keyboard; do both before calling the editor finished.
