# Debugging (`spec/debug.md`)

How a Burdin program is debugged: the panic hook, symbolized panic frames, debugger variables that tell the
truth about moves, the pretty-printers, `burdin debug`, and the Debug Adapter Protocol server. The issues are
#565 and its sub-issues #567 (gdb/lldb printers), #569 (Natvis), #571 (variable lifetimes), #572 (break on panic
and symbolized frames), #574 (`burdin debug`), and #576 (`burdin dap`).

`-g` itself — DWARF and the CodeView metadata the COFF backend carries, subprograms, parameters, locals, the line
table — is issue #422 and stays as it is. This document is everything above it. The checks are
`tests/check_debug_info.py` (issue #422 plus the lifetimes of #571) and `tests/check_debug_printers.py`
(the printers, the panic hook, the symbolized frames, and `burdin debug`), both run by every CI job, and
`tests/check_dap.py` plus `crates/burdin_dap/tests/protocol.rs` for the DAP server.

## The panic hook (issue #572)

**DBG1 — `burdin_panic_begin`.** The runtime defines an empty, externally visible function
`burdin_panic_begin` and every panic path calls it first, before it prints anything or aborts: the message
panic, the bounds panic, and the stack-overflow guard. It is a breakpoint target, not code: it exists so
`break burdin_panic_begin`, `burdin debug --break-on-panic`, and the DAP "Burdin panic" exception filter stop
with the whole stack intact — before the panic printing runs, the stack is unwound by nothing, and every frame
above the panicking function can still be read. The symbol is external so the linker and `globaldce` keep it in
every program, including ones that cannot panic; it is documented in the driver contract as the stable name.
`tests/check_debug_printers.py` breaks on it under gdb and asserts the stack contains the Burdin frames.

**DBG2 — symbolized panic frames.** With `-g`, code generation emits one private constant per emitted body
function — a frame entry `{ i64 addr, ptr name, usize name_len, ptr file, usize file_len, u32 line }` — plus the
array of them and the lookup `burdin_rt_frame_info(pc): ptr`, which answers with the entry whose address is the
greatest one not after `pc`, or `null` when the table has no such entry. Every defined function of the module is
in the table (program functions and closures with their Burdin names and source lines; generated glue, the
runtime, and the task runtime with their symbol names), so a frame inside generated code is attributed to that
generated function and never to the Burdin function that happens to precede it. The panic backtrace then prints
one line per frame:

```
stack backtrace:
  #0  at src/app.bd:12:3 in app.main
  #1  0x7f0a41c2b8c2
```

The Burdin entries print `at <file>:<line> in <Burdin path>` — the function's own declaration line, which is
exact and known at compile time. The line a frame *stopped at* is the debugger's job (`burdin debug`, `burdin
dap`); printing it at run time would need a link-time DWARF line-table reader the runtime does not have, and the
table's function granularity is what the spec promises. Frames outside the table (libc, the operating system)
prints as raw addresses with a `note: rebuild with -g for a symbolized backtrace` hint. Without `-g` the lookup is a stub that answers `null` and the backtrace prints exactly
what it printed before (issue #423): `backtrace_symbols_fd` on Unix and addresses on Windows — the conformance
tests `m02_basics/panic_backtrace.bd` and `panic_backtrace_names.bd` pin that path. The panic site's own
`path:LINE:COL:` prefix is unchanged.

**DBG3 — error traces carry the function.** A debug-build error trace frame names the Burdin function the
error passed through, not only the site: `TraceFrame` gains `func: String`, the `error_at` and
`error_frame` helpers take the function first, and sema passes the enclosing function's qualified name.
`TraceFrame`'s `Display` is `at {func} ({file}:{line}:{col})`. The wrappers that print an error — the generated
`main` of `fn main(): Result<T>` and the test wrapper — print the frames after the message, one per line; a
release build records no frames, so it prints none and its output is unchanged (`spec/errors.md`, ERR7-ERR8
gain the field; the tests are `m16_errors/traces_debug.bd`, which also prints `f.func`, and the new
`m16_errors/main_result_trace.bd`).

## Variable lifetimes (issue #571)

**DBG4 — a local's location follows its value.** With `-g`, a local's DWARF location starts at its first
initialization, ends where the move checker proved it was moved out or dropped, and a debugger shows
`<optimized out>` / `<not available>` instead of stale bytes in between. The mechanism is LLVM's own:
`dbg.declare` still describes the stack slot, a `dbg.value` with an undefined value *kills* the location from
that point in the instruction stream, and a `dbg.value` naming the slot with `DW_OP_deref` restores it. Kills are
placed at four kinds of point, each sound under LLVM's linear location ranges:

- *entry* — every local that is not a parameter is killed right after its declare, so it stops showing garbage
  before its `let` runs;
- *initialization* — each binding form (`let`, a pattern, an assignment, a loop binding) restores the location
  at the `set_initialized` the code generator already emits;
- *a whole-local move* — the move checker (`burdin_check::moves`) records `(local, span)` kill sites for
  whole-local moves it can prove unconditional: the move is at conditional depth zero (not inside an `if` or
  `match` arm, whose other path would be hidden by the linear range) and not inside a loop over a local that was
  already in scope there (a valid program moves such a local only once, but the zero-iteration path must keep
  it). The code generator emits the kill after the statement containing the span. A conditional move keeps its
  location and shows its (stale) bytes: the checker proved only that it *may* have moved, and the drop flag is
  the honest answer;
- *scope end* — the fallthrough of a block kills every local of that block, whether it was moved, dropped, or
  plain out of scope, which is also what makes an inner block's locals stop appearing after the block ends.

Early exits (`return`, `break`, `continue`) drop the scopes but do not kill: the kill would sit on one path's
layout and hide the local on another's. A partial move (a field moved out of a struct) does not kill the local's
location either; the debugger keeps showing the aggregate. Both are documented gaps, chosen because a linear
location range cannot express them. The checker's kill list travels in `tir::Function::debug_kills`, so code
generation learns it from the typed tree and depends on nothing else. The fixtures are in
`tests/check_debug_info.py`: stop after a move, after a drop, and inside a branch with a conditional move, and
assert what gdb (and lldb, where installed) print for the local.

## Pretty-printers (issues #567 and #569)

**DBG5 — the DWARF is the layout contract.** The debug types of `emit/debug.rs` are the one description of the
built-in layouts that the printers read: `String` is `{ ptr, len, cap }`, `str` and slices are `{ ptr, len }`,
`List` and `Map` are `{ ptr, len, cap }`, `Option` is `{ is_some, value }`, `Result` is `{ is_ok, ok, err }`, a
range is `{ lo, hi }` (plus `done` for the iterator), `Heap` is a pointer, `Shared` is the pointer to its cell,
`any I` is `{ data, vtable }`, a closure is `{ code, env, drop }`, and an enum is `{ tag: i32, payload }` where
the payload is a union with one member per variant, so the printers can name the active variant from the tag.
The printers and the Natvis file read members *by name* through the debugger's own type information, never by
hard-coded offsets, so a layout change moves them only if the names move. Facts the DWARF cannot carry are
written once in this document and mirrored in the three printer files, and the golden fixtures fail when either
side drifts: a `List`'s `ptr` addresses `cap` contiguous elements; a `Map`'s buffer holds `cap` (a power of
two) slots of `(key, value, u64 hash)` where a zero hash marks an empty slot and there are no tombstones; a
`Shared` cell is `{ count: usize, value }` — the strong count first — and the `Shared<T>` value points at it;
the `any` vtable is `{ i64 type_id, ptr drop, ptr name, usize name_len, ptr method_0, ... }`, where `name` is
the concrete type's Burdin name at index 2, after `type_id` and `drop`. The DWARF types give the containers a
typed element pointer (`List<T>`'s `ptr` is `T*`, a `Map`'s points at its `(key, value, hash)` slot struct,
`Heap<T>` has a `value: T*` member and `Shared<T>` a `cell` member), so the printers never guess a type.

**DBG6 — gdb printers are embedded in the binary.** `burdin build -g` writes a `.debug_gdb_scripts` section
into the object, holding one embedded script entry: the type byte `4` (inline Python text), the script's name
`burdin_printers.py` and a newline (the name is what gdb uses to run a script once), the script's text, and a
terminating NUL. There is no version byte. gdb auto-loads the section when it opens the program, so no
installation or configuration is needed; a site policy that restricts auto-loading is lifted with
`set auto-load safe-path /`. The script is the repository file `debuggers/gdb/burdin_printers.py`, embedded by
`burdin_codegen` — the compiler and the printers ship together. Without `-g` there is no section, and a macOS build never has one: Mach-O has no section by that name and
macOS debugs with lldb (DBG7).

**DBG7 — lldb formatters are written beside the session.** lldb has no equivalent of the section, so the
driver writes `debuggers/lldb/burdin_formatters.py` (embedded in the binary like `std/`) to the temporary
directory of the session and `burdin debug` and `burdin dap` import it: `command script import <path>` as an
`initCommands` entry for lldb and a `preRunCommands` entry for lldb-dap. A plain `lldb` session can load the
same file by hand; `burdin debug` is the supported way.

**DBG8 — the Windows PDB and Natvis.** With `-g` on MSVC the driver passes `/DEBUG`, so the linker writes a
PDB the debuggers can read (until now the COFF object carried the CodeView metadata but no PDB was produced),
and `/NATVIS:<file>` with the embedded `debuggers/windows/burdin.natvis`, so the visualizers travel inside the
PDB and no file has to be installed. The natvis rules are the DWARF rules of DBG5 spelled in the debugger's own
syntax, including `CustomListItems` for `List` and `Map`. The driver also passes `/INCREMENTAL:NO`: an
incremental link (the default with `/DEBUG`) makes a function's address a jump thunk, and the frame table of DBG2
matches return addresses against real function addresses.

## `burdin debug` (issue #574)

**DBG9 — the command.** `burdin debug <target> [-- args]` compiles the target in the selected profile (`dev` by
default; `--release` is `--profile release`, `spec/profiles.md`) with `-g`
into a temporary directory and starts the platform debugger on it: lldb on macOS, gdb on Linux when it is
installed and lldb otherwise, and cdb (the VS/WinDbg console debugger) on Windows; `--debugger <name or path>`
overrides (`gdb`, `lldb`, `cdb`, or an arbitrary program, which is how the black-box tests drive it with a
stub). The debugger is interactive and the command passes its exit status through. Pretty-printers load
automatically (DBG6, DBG7). `--break-on-panic` starts with the panic breakpoint set (`break burdin_panic_begin`
under gdb, `breakpoint set --name burdin_panic_begin` under lldb, `bp burdin_panic_begin` under cdb). Nothing
about `burdin build` changes: the command only picks the profile, the `-g` flag, and where the executable goes.

**DBG10 — `--test`.** `burdin debug --test <name> <target>` compiles the target the way `burdin test` does —
test blocks declared, the test dispatcher emitted — resolves the name against the compiled test list exactly
like the runner's filters (the first full path that contains `<name>`, or `--test <index>` for an explicit
index), and starts the debugger on the dispatcher with the test index as the program argument, so the debugger
stops inside the test's own code. Output capture and isolation are off: the test writes to the terminal the
debugger owns.

## The Debug Adapter Protocol server (issue #576)

**DBG11 — wrap lldb-dap.** `burdin dap` speaks DAP 1.x on standard input and output and forwards to lldb-dap
(the `lldb-dap` binary of the LLVM installation; `BURDIN_DAP_BACKEND` names another program, which is how the
tests drive it with a stub). Driving gdb or lldb directly was rejected: every DAP feature would be
re-implemented per debugger, and the adapter's value is the Burdin-specific behavior on top, not a second
debugger engine. The adapter adds Burdin behavior at four points and forwards everything else byte for byte:

- *launch and attach* — a request may name `target` (a `.bd` file or package directory) instead of a built
  `program`, and the adapter compiles it with `-g` in the debug profile (in test mode when the request also
  carries `test`: the name is resolved like `--test` and the index becomes the program's first argument). The
  `preRunCommands` of the forwarded request import the lldb formatters (DBG7), so scopes, variables, and
  `evaluate` print through them.
- *the panic filter* — the `initialize` response advertises the exception filter `panic` ("Burdin panic").
  `setExceptionBreakpoints` with that filter is translated into a function breakpoint on `burdin_panic_begin`
  (DBG1), merged with the client's own `setFunctionBreakpoints`; requests that no longer carry it stop setting
  the breakpoint. `--break-on-panic` is the CLI form.
- *stepping skips generated glue* — when a step stops in code with no Burdin source (drop and clone glue,
  closure records, the runtime, vtable constants, the codegen-unit copies), the adapter asks for the top frame,
  steps out, and does not report the stop; the client sees the next stop that is Burdin code. `continue` is not
  rewritten: a breakpoint the user set is the user's business.
- *backend transparency* — every other request, response, and event is forwarded unchanged, including sequence
  numbers; the adapter's own requests to the backend use ids above a million and their responses are dropped.

**DBG12 — the editor glue.** The VS Code extension registers the `burdin` debug type against `burdin dap` and
keeps its CodeLLDB fallback for the program case; the Debug lens's launch configuration is the one the adapter
accepts. `editors/README.md` documents the Neovim (`nvim-dap`) and Helix (`debug-adapter` binary) setups with
the same launch fields. `tests/check_dap.py` drives `burdin dap` with a stub backend over the real protocol
(initialize, launch, breakpoints, the panic filter, stepping, variables, disconnect) and checks the editor
files; `crates/burdin_dap/tests/protocol.rs` runs the same sessions in-process; where a real `lldb-dap` is
installed the script also runs one fixture end to end.

## What is deliberately not here

- Call-site line numbers in run-time backtraces (DBG2 explains why; the debugger prints them).
- Location kills on early exits and after partial moves (DBG4).
- A Natvis check on a machine without cdb: the check records a manual checklist instead (issue #569).
- Debugging inside tasks and threads beyond what the debugger itself shows.
