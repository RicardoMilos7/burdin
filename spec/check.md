# `burdin check`

The type-check-only command (stage 3; issue #237). It runs the compiler's front end and every memory check and stops
before code generation, so it is fast enough for CI gates and editor feedback and needs neither LLVM work nor a linker.
This file is the exact behavior, with a rule ID for each point and the tests that cover it.

Every behavior below is covered by `tests/check_check.py`, which runs the fixtures under `tests/check/` and checks the
command against the whole conformance suite. A fixture is a `.bd` file (or a package under `tests/check/packages/`)
whose `//~ E0000` markers name the exact diagnostics, by file and line, that the command must report; a fixture with no
markers proves that nothing is reported. The `//% args:`, `//% exit:`, and `//% stderr:` header lines set the command
line, the exit status, and the text the output must contain. The golden files in `tests/check/golden/` hold the whole
output of every failing fixture. The Rust unit tests in `crates/burdin_driver/src/check.rs` cover the command line.

## The command

```
burdin check <file.bd | package directory> [--release | --profile <name>] [--tests] [--comptime-steps <n>] [diagnostics]
      [--explain-ownership [--fn <name>] [--format <text|json>]] [--explain]
```

`diagnostics` is the group of options every command shares (`--diagnostic-format`, `--color`, `--no-color`, `--width`,
`--error-limit`; the driver contract in `README.md`).

**CK1. The same front end as a build.** `burdin check` loads the target like `burdin build` does and runs the same
stages in the same order: build scripts, parsing, name resolution and type checking (including monomorphization), the
return-origin, move, and borrow checks, and compile-time evaluation (`comptime`, constants, `embed`). It is one function,
`front_end` in `crates/burdin_driver/src/main.rs`, which `build`, `run`, `test`, and `lint` call too, so a diagnostic
cannot be reported by one command and missed by another. The command exits 0 exactly when `burdin build` would reach code
generation, and 1 when the front end reports an error. A `main` is required (E0002, "the program has no `main`
function"), as it is for a build.
Tests: every fixture; every conformance test (`compile-fail` programs report exactly their markers, every other program
is accepted); `err_*` fixtures against `burdin build` line for line.

**CK2. Nothing is generated.** The command runs no code generation and no linker: it succeeds without `cc`, writes no
file (no executable, object, or IR), and has no option for one. The options of a build that only matter for its output
(`-o`, `--emit-ir`, `-g`, `--strip`, `--lto`, the PGO options, `--link-lib`, `--emit-deps`) are usage errors. A package
with a `build.bd` still runs its script, because a generated module has to exist before the modules are read (see
`spec/build-scripts.md`, BS3-BS4), and a script is built and run, so that one needs the linker.
Tests: `a clean check with no linker`, `checking a package wrote files or needed a linker`, the rejected options.

**CK3. Output.** The diagnostics are printed to standard error in the chosen format, exactly as `burdin build` prints
them (`--diagnostic-format`, `--color`, `--width`, `--error-limit`), and a failed check ends with `error: could not check
<target> (N errors)` (and, in the human format, the note about `burdin explain`); the JSON format ends with the same
`summary` object. Warnings are printed too and do not fail the check. A check with no errors and no warnings prints
nothing at all, on either stream. The SARIF format follows `spec/diagnostics.md` (DG11): a check is a CI gate like a
build, so it writes one log even when the package is clean, and the log is the only thing on standard error. Standard
output is never used.
Tests: the goldens, `ok_program.bd` (silent), `ok_warning.bd`, the JSON, `--error-limit`, the human-format and SARIF
checks.

**CK4. Every error is reported.** Like a build, a check reports every error of the front end, not the first. A file
with a syntax error is not type checked: its own checks are not reported, but every other module is, and the other
modules are checked against the declarations the broken file did read (its `ast::File` keeps every item it parsed). A
declaration the parser could not read (`ast::File::unparsed`, the source of each item the parser recovered from) is
missing from that tree, so a diagnostic in another module is a follow-on of it, and is not reported, when its span or
a backticked name in its message mentions a name the unreadable declaration declares: an item's name, the method
names of a `fn`, the interface and type of an `impl` header, the variants of an `enum`, and the leaf names of a
`use`. Parameter names, fields, payload types, and module paths are not declared names. A name that only parsed
declarations declare is never a follow-on, so an error that names it is reported. When in doubt the follow-on is dropped: a diagnostic in a
file with a syntax error never survives, and a missing `main` is not reported when the unreadable declarations declare
`main`. The syntax errors themselves are always reported. With syntax errors the program is not returned (the
language server keeps its last good one), and compile-time evaluation never runs (`spec/comptime.md`, CT1).
The rule lives in `crates/burdin_query/src/follow_on.rs`, so a build, a check, and a language server report the same.
Tests: `err_syntax_declared.bd` (a broken function is not reported, and neither is a mismatch in the broken file),
`packages/syntax_neighbors` (a broken module's neighbors are checked against what it parsed), `packages/syntax_no_main`
(a `main` that did not parse), `err_types.bd` and `err_syntax.bd` (one file, unchanged), and the unit tests of
`follow_on.rs`.

**CK5. `test` blocks: `--tests`.** A plain check does not declare or check `test` blocks, and neither does a build. With
`--tests` the package is loaded the way `burdin test` loads it: the `test` blocks and their attributes are checked, and a
package without `src/main.bd` is accepted (a library package; its entry module is the one other packages import, or its
first module), so `--tests` is also how a library is checked. In this mode `main` is not looked up, so a wrong
signature of `main` is not reported; check without `--tests` for that.
Tests: `lib_tests` and `tests_error` packages, the `test-compile-fail` and `burdin-test` conformance tests.

**CK6. The profile.** The profile changes what the front end does in two places: error trace frames are recorded only
when the profile's `debug-assertions` is on (`spec/errors.md`, ERR7), and compile-time evaluation follows its
`overflow-checks` and `debug-assertions` (`spec/comptime.md`). `--profile <name>` selects one
(`spec/profiles.md`); `--release` is `--profile release`, and the default is `dev`.
Tests: `--release on a clean program`, the `release-only` conformance tests.

**CK7. `--comptime-steps <n>`.** Sets the step budget of each compile-time evaluation, as for a build
(`spec/comptime.md`, CT11). `0` and non-numbers are usage errors.
Tests: `budget.bd` (fine with the default budget, E0225 with 1000 steps), the usage-error checks.

**CK8. Exit statuses.** 0: no errors (warnings allowed). 1: the front end reported errors, or the package could not be
loaded or read (a missing target, an unreadable file, a missing `main.bd`, a failing build script). 2: a usage error (a
missing target, an unknown option, a bad value); the usage is printed after the message. `--help` prints the command's
help (usage, options, and notes) to standard output and exits 0.
Tests: the exit status of every fixture, the missing target, the usage-error checks, `--help`.

**CK9. `--print-instances`.** After a clean check the command prints the monomorphization report (`spec/inspect.md`,
INS12-INS15) to standard output; `--format json` makes it one JSON object. A check with errors prints the diagnostics and
no report, and `--format` without `--print-instances` is a usage error.
Tests: `tests/check_inspect.py` (`check_print_instances`).

## Ownership events and annotations

**CK10. `--explain-ownership`: the ownership lens on the command line.** The memory checks already record every move,
copy, borrow, drop, drop flag, argument convention, and derived clone of a body. `burdin check --explain-ownership`
adds a stable event list to that output (no new analysis) and prints each source function of the entry package with
margin annotations, in source order:

```
fn main (ownership/moves.bd:12):
  12 | fn main() {
  13 |     let a = Point { x: 1, y: 2 }
  14 |     let b = a
     | move a -> b
  15 |     take(b)
     | call arg `p`: own
     | move b
  19 |     let q = p.clone()
     | call arg `self`: read
     | clone p (derived)
     | drop q
  21 | }
```

The event kinds (`burdin_tir::OwnershipEvent`, exposed as `burdin_query::OwnershipEventInfo`) are `declare`, `move`
(with the destination when there is one), `copy`, `borrow` (shared or mutable), `drop` (with whether a runtime flag
guards it), `drop_flag`, `call_arg` (the parameter's name and `read`/`mut`/`own` convention), and `clone` (with
whether `Clone` is derived). A drop is anchored at the closing brace of the scope that drops it; a move or borrow, at
the expression that performs it. A move whose value is left behind as a statement temporary is a `drop temporary`.
The compiler's own generated code — an interpolated string's hidden formatter, a desugared loop counter — is left out
of the list, because it is not source the user wrote.

`--fn <name>` prints only the function with that name, its qualified name (`geometry.Rect.area`), or its last
component (`area`). `--format json` prints the same functions and events as one JSON array, one function per line,
each with the event's `kind`, its span in the JSON location shape of `spec/diagnostics.md` (DG2), and the kind's
fields (`local`, `path`, `target`, `mutable`, `conditional`, `parameter`, `convention`, `derived`). The generic
standard library, generic instantiations, and compiler-generated items are not printed. The command prints the
annotations on standard output; diagnostics still go to standard error, and the exit status is the usual one. The
`--fn` and `--format` options need `--explain-ownership`; anything else is a usage error (exit 2).
Tests: `tests/check/ownership/` (one fixture per rule family: moves and copies, borrows, drops and reassignment,
partial moves and `Heap`, temporaries), `ownership_*` goldens, and the `--fn`/`--format json` checks in
`tests/check_check.py`.

**CK11. `--explain`: the path to a memory error.** For each diagnostic of a memory code (E0100, E0101, E0103, E0104,
E0110, E0111, E0121, E0122, E0123), the command prints a trace after the diagnostics, on standard error:

```
trace for explain/E0111.bd:13:5: error[E0111]: this access conflicts with a borrow that is still live
  rule: spec/memory-rules.md RF3
  note at explain/E0111.bd:12:22: the borrow is created here
  fails here: explain/E0111.bd:13:5
  fix: finish using the reference before changing the place, ...
```

The steps are the events for the value the diagnostic names (from its backticked name, or from the event at the
failing span), in source order: `created`, `moved`, `copied`, `borrowed`, or `cloned`, each with its location. The
rule ID is the one from `spec/memory-rules.md`. The suggested fix is the reference's `**How to fix:**` paragraph
(`burdin_diag::explain_fix`). Every memory diagnostic also carries one `help` line pointing at the mode, and its
`notes` (which the JSON format publishes as `related` information) point at the declaration and each move or borrow
on the way. The plain diagnostic line — and therefore every conformance test's `//~` markers and the short format —
does not change.
Tests: `tests/check/explain/` (one fixture per code) and its goldens, the JSON/SARIF goldens in
`tests/diagnostics/`, and `tests/check_check.py`.

## Choices that are not obvious

- **There is no separate "check" implementation.** The issue asks to reuse the query-based library. That library is a
  later stage 3 item and does not exist yet; `front_end` is the one shared entry point today, and the day the
  query-based library lands it replaces `front_end` for every command at once, `check` included. Nothing in
  `check.rs` would change but the call.
- **`--tests` is explicit.** Checking `test` blocks needs the test-mode front end, which changes how the entry package
  is loaded and drops the `main` lookup. Doing it silently would hide errors of `main`; doing it only for a package
  without `main.bd` would make the same command check different things in a library and a program. One flag says what
  is checked.
- **Silence on success.** A build prints nothing when it succeeds, and a check is the part of a build that needs no
  output at all. A summary line would only be noise in a CI log and in an editor's problem list.
- **The conformance suite is the equivalence test.** CK1 is a claim about every program, so the check script runs the
  command on every conformance test and compares it with that test's markers, instead of on a few hand-picked programs.
