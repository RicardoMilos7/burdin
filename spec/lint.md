# `burdin lint`

The built-in linter (stage 3; issues #230, #231, #232, #233, #464, #465, #466, and #661). It reports code that compiles
and is probably not what was meant, and it can fix some of it. This file is the exact behavior, with a rule ID for each
point and the tests that cover it. The user-facing reference, one entry per lint with a checked example, is
[`docs/lints.md`](../docs/lints.md).

The lints and the machinery around them live in the `burdin_lint` crate. It reads what the compiler already built: the
syntax tree for the lints about how code is written, the typed tree for the lints about what it means. It does not
change either, and it is not part of `burdin build` or `burdin test`: a build never prints a lint.

Every behavior below is covered by `tests/check_lint.py`, which runs the fixtures under `tests/lint/` and the examples
in `docs/lints.md`. A fixture is a `.bd` file (or a package directory) whose `//~ L0000` markers name the exact
diagnostics, by line, that the linter must report; a fixture with no markers proves that nothing is reported. The
`//% args:`, `//% exit:`, and `//% stderr:` header lines (or an `expect.txt` next to a package's `burdin.toml`) set the
command line, the exit status, and the text the output must contain. The Rust unit tests of the crate (`registry.rs`,
`suppress.rs`, `naming.rs`, `fix.rs`) cover the pieces that are easier to check in isolation.

## The command

```
burdin lint <file.bd | package directory> [-A <lint>]... [-W <lint>]... [-D <lint>]... [--deny-warnings]
            [--fix [--dry-run]] [--const-steps <n>] [diagnostics]
burdin lint --list
burdin lint --explain <lint>
```

**LN1. What is linted.** `burdin lint` loads the target like `burdin build` does, with the same front end: parsing,
name resolution, type checking, the memory checks, and compile-time evaluation. Only a program with no errors is linted;
when the front end reports an error, the linter prints it and exits with status 1 without running a lint. Only the
entry package is linted: its modules and its `test` blocks, not its dependencies and not the standard library.
Tests: `check_lint.py` (`compile errors stop the linter`), every fixture.

**LN2. Lints and levels.** A lint has a name (`unused_variables`) and a code (`L0007`), and one of three levels:
`allow` (it does not run), `warn` (a finding is a warning), and `deny` (a finding is an error). Every lint is at `warn` by
default except `missing_docs`, which is at `allow` (LN33). A command line option names a lint by name or by code, in any case. `all` names every lint; `warnings` names the
lints that are at `warn` when the option is read. An unknown name is a usage error (status 2).
Tests: `config/deny_by_name.bd`, `config/deny_by_code.bd`, `config/allow_all.bd`, `config/allow_and_warn.bd`, the
`registry.rs` unit tests.

**LN3. Where levels come from.** In increasing priority: the default; the `[lint]` table of the entry package's
`burdin.toml` (`name = "level"`, one lint per line, level one of `allow`, `warn`, `deny`); then `-A`/`--allow`,
`-W`/`--warn`, and `-D`/`--deny` in the order written, so the last option for a lint wins. A name that is not a lint, a
level that is not one of the three, or the group name `warnings` in the manifest is an error that names the manifest
line, and the exit status is 2.
Tests: `config/manifest_levels`, `config/manifest_cli_override`, `config/manifest_bad_name`, `config/manifest_bad_level`,
`config/manifest_warnings_group`, `config/allow_and_warn.bd`, and `check_lint.py` (`[lint]` cases).

**LN4. Deny.** A finding of a lint at `deny` is printed as an error, and the command exits with status 1 when at least
one is reported. `--deny-warnings` is `-D warnings`: every lint at `warn` at that point becomes `deny`, and a lint
allowed before it stays allowed. Warnings alone do not change the exit status.
Tests: `config/deny_by_name.bd`, `config/deny_warnings.bd`, `check_lint.py` (`--deny-warnings`).

**LN5. Output and status.** A diagnostic has the lint's code (`L0007`), a primary span, a message, and notes. The
diagnostic formats and the color and width options are the compiler's (`--diagnostic-format human|short|json|sarif`,
`spec/diagnostics.md`). After the diagnostics the command prints `lint: N warnings, M errors`, or `lint: no warnings in
<target>` when there is nothing to report; in `json` format the last line is
`{"type":"lint-summary","version":1,"warnings":N,"errors":M}`, and in `sarif` format the whole output is one SARIF log
(written even when nothing was found) with no line after it. The exit status
is 0 when no error was reported, 1 for a lint at `deny` or a compile error (or a source file that cannot be read), and 2
for a usage error, an invalid manifest `[lint]` table, or a failed write.
Tests: `check_lint.py` (summary line, json output, usage errors, `a missing file`).

**LN6. Suppression comments.** A comment `// lint: allow(<lint>, ...)` on its own line suppresses the named lints for
the next item, statement, or `match` arm: a finding anywhere inside that source range is not reported. A comment that
follows code on a line suppresses the findings that start on that line. A suppressed finding is not reported and is not
fixed (LN31). Suppression applies on top of the levels: a lint at `allow` does not run at all.
Tests: `config/suppress_forms.bd`, `naming/allow_comments.bd`, `else_enum/negatives.bd`, `eager_chain/short_and_lazy.bd`,
`unused/dead_code.bd`, the `suppress.rs` unit tests, the `docs/lints.md` suppression example.

**LN7. File-wide suppression.** `// lint: allow-file(<lint>, ...)` anywhere in a file suppresses the named lints for the
whole file. `all` stands for every lint in either form.
Tests: `config/suppress_file.bd`, `config/suppress_forms.bd` (`allow(all)`).

**LN8. Unknown names.** A suppression comment that names something that is not a lint is reported as `L0000` (a warning)
at the comment, so a typo cannot silently hide a finding. The comment suppresses nothing for that name; the real lints
named in it still apply. `L0000` has no level and cannot be allowed.
Tests: `config/unknown_lint_comment.bd`, the `L0000` example in `docs/lints.md`, the `suppress.rs` unit tests.

**LN9. `--list`.** Prints one line per lint: code, name, default level, and a one-line summary, then `L0000`. Exit 0.
Tests: `check_lint.py` (the table is what `docs/lints.md` is compared against).

**LN10. `--explain`.** `burdin lint --explain <lint>` takes a name or a code and prints the section of
`docs/lints.md` for that lint, which is embedded in the driver. An unknown lint exits with status 2. The page has exactly
one section for each lint in `--list`, each with a `**How to fix:**` paragraph and a checked example.
Tests: `check_lint.py` (documentation checks, `--explain`).

**LN11. Generated code.** Code the compiler or the parser makes up is never linted: derived impls, the impls of an `error
enum`, the type parameter of an `impl` argument, `main`, and the wrapper of a `test` body. A name
or an item whose source text is not what the compiler recorded for it is generated.
Tests: `generated/no_findings.bd`.

**LN12. Machine-applicable fixes.** A fix is a suggestion of the diagnostic with `machine_applicable` set; in `json`
output it carries `"applicability":"machine"`. A suggestion without it is advice that needs a human (a rename, a rewrite
that changes meaning). Only a machine-applicable suggestion is ever applied (LN28-LN32). The compiler's own E0114 and
E0115 suggestions are machine-applicable too.
Tests: `check_lint.py` (json output), every `fix/` fixture.

## The lints

Each lint's fixtures are the files named in its entry; the positive cases carry `//~` markers and the negative cases are
in the same files without one.

**LN13. Naming (`non_snake_case` L0001, `non_pascal_case` L0002, `non_screaming_case` L0003).** Functions, methods,
parameters, local bindings (`let`, patterns, `for`, closure parameters), struct and enum-variant fields, and modules are
`snake_case`; structs, enums, enum variants, interfaces, type aliases, derives, and type parameters are `PascalCase`;
constants and const generic parameters are `SCREAMING_SNAKE_CASE`. A name with a leading underscore is checked without
it, digits are allowed, and an uppercase name in a pattern is a variant or a constant, not a binding. The note names the
corrected spelling. There is no fix: a rename touches every use.
Tests: `naming/bad_names.bd`, `naming/good_names.bd`, `naming/allow_comments.bd`, the `naming.rs` unit tests.

**LN14. `unused_imports` (L0006).** A name brought in with `use` that the file never mentions is reported at the name
(or at the `use` for a module import or alias). A `pub use` is never reported. Fix: remove the name from the list, or the
whole `use` line when no name is left.
Tests: `unused/imports`, `fix_packages/imports`, the `L0006` example in `docs/lints.md`.

**LN15. `unused_variables` (L0007).** A local that is never read: a `let` binding, a pattern binding (`match`, `if let`,
`for`), or a closure parameter. Writing to a variable is not reading it; a name with a leading underscore is exempt. Names
are resolved lexically, so a shadowing binding is its own variable, and an interpolation (`"{name}"`) is a read. Fix:
prefix the name with `_`. A binding written as a struct-pattern shorthand has no fix, only a note, because a prefix would
rename the field.
Tests: `unused/variables.bd`, `fix/unused_variables.bd`.

**LN16. `unused_parameters` (L0008).** A parameter of a function or method with a body that is never read. Parameters of
interface method declarations and `extern` functions are exempt, and so are names with a leading underscore. No fix, since
a parameter is part of the signature; the note says how to rename it.
Tests: `unused/parameters.bd`.

**LN17. `else_on_local_enum` (L0004).** A `match` with an `else` arm whose other arms name variants of an enum declared in
the entry package is reported at the `else`, and the note lists the variants it catches. Enums from the standard library
or a dependency, matches on other types, matches that name every variant, and `if let ... else` are not reported.
Tests: `else_enum/hides_variants.bd`, `else_enum/negatives.bd`.

**LN18. `unreachable_code` (L0009).** The statement after one that never completes. A statement never completes when it
is `return`, `break`, or `continue`; a call to `panic`, `unreachable`, `todo`, `unimplemented`, `process.exit`, or a
function declared `: !`; a `loop` without a `break`; or a block, `if`/`else`, or `match` whose every path does so. Only the
first unreachable statement of a block is reported.
Tests: `unused/unreachable.bd`.

**LN19. `dead_code` (L0010).** A `priv` function, method, extension method, struct, enum, constant, type alias, or
interface that nothing outside itself mentions in the package. Items used only by themselves (recursion, a type's own
methods) are unused; `main`, names with a leading underscore, and public items are exempt. A mention is by name, so the
lint never reports an item that is used and may miss one that shares a name with something used.
Tests: `unused/dead_code.bd`.

**LN20. `eager_chain` (L0005).** Three or more of the eager `List` methods `map`, `filter`, `fold`, `any`, `all`, and
`find`, each applied to the result of the one before, are reported once, at the whole chain. The suggestion is to start
with `.iter()`. No fix: the closure parameters of the lazy forms are items, not views, so the closures may need to change.
Tests: `eager_chain/long_chains.bd`, `eager_chain/short_and_lazy.bd`.

**LN21. `redundant_clone` (L0011).** `x.clone()` where `x` is a local of a type that is `Clone` and not `Copy`, the clone is
the last use of `x`, no loop brings the clone around again, and the type has no user `Drop` (dropping the original would
be observable). A borrowed parameter is never reported (it cannot be moved out). Fix: replace the call with the receiver.
Tests: `ownership/redundant_clone.bd`, `fix/redundant_clone.bd`.

**LN22. `large_copy_in_loop` (L0012).** Inside a loop, an argument of a `Copy` type that is 64 bytes or larger passed to an
`own` parameter. No fix: the choices (a borrow, hoisting) change the signature or the structure.
Tests: `ownership/large_copy.bd`.

**LN23. `unused_mut` (L0013).** A `mut` local that is never assigned, never borrowed mutably, never passed to a `mut`
parameter or called with a `mut self` method, and not written by a closure or through a slice. Where the analysis cannot
see what a construct does (a slice, a closure that owns its capture), the binding counts as mutated, so the lint prefers
silence. Fix: remove `mut` and the space after it.
Tests: `ownership/unused_mut.bd`, `fix/unused_mut.bd`.

**LN24. `borrow_could_own` (L0014).** A `priv` function or method whose borrowed parameter is cloned inside the body while
every call in the package passes a temporary or the last use of a local. Reported at the parameter. No fix: it changes a
signature and its callers.
Tests: `ownership/borrow_could_own.bd`.

**LN25. `loop_invariant_alloc` (L0015).** An immutable local initialized inside a loop with an allocation (`String.from`,
`Heap.new`, `to_string`, ...) built only from literals and constants, so every iteration makes the same value. No fix.
Tests: `ownership/loop_alloc.bd`.

**LN26. `needless_to_list` (L0016).** `to_list()` whose result is iterated at once, by a `for` loop or by `.iter()` or
`.into_iter()`. No fix: the iterator type differs.
Tests: `ownership/needless_to_list.bd`.

**LN33. `missing_docs` (L0017).** A public item that has no `///` comment, at its name: a function, struct, enum, error
enum, interface (and each method of it), type alias, constant, derive, `extern` function, extension method, or method of an
inherent `impl` block. The list is `burdin_syntax::docs::public_items`, which `burdin doc --coverage` counts too
(`spec/doc.md`, DC19), so the lint and the report agree. A `priv` item, a member of a `priv` type, generated code, `main`,
a field, a variant, and a method that implements an interface are never reported. The default level is `allow`, the only
lint with one, so it runs when the manifest or the command line turns it on; `-D warnings` leaves it alone, and `-D all` or
`-W all` turns it on. No fix.
Tests: `docs/missing_docs.bd`, `docs/missing_docs_default_off.bd`, `docs/missing_docs_suppress.bd`,
`docs/missing_docs_file.bd`.

**LN27. Generic functions.** A generic function is checked once per instance, and the instances give the same finding
the same span. A finding is reported once, at its place in the source; a finding that depends on the type
(`redundant_clone`, `borrow_could_own`, `unused_mut`) is reported only when every instance agrees.
Tests: `generic/instances.bd`.

## `--fix`

**LN28. Fixes of `unused_imports` and `unused_variables`.** `--fix` removes an unused import (a name out of the list, or the
whole line) and prefixes an unused variable with `_`.
Tests: `fix/unused_variables.bd`, `fix_packages/imports`.

**LN29. Fix of `unused_mut`.** `--fix` removes `mut` from a binding that is never mutated. A binding that is both unused and
`mut` gets both edits (`let mut x` becomes `let _x`).
Tests: `fix/unused_mut.bd`, `fix/mixed.bd`.

**LN30. Fix of `redundant_clone`.** `--fix` replaces `x.clone()` by `x`.
Tests: `fix/redundant_clone.bd`, `fix/mixed.bd`.

**LN31. What is never fixed.** A finding that is at `allow`, one that a suppression comment covers, and one whose
suggestion is not machine-applicable are left as they are. The lints without a fix still report.
Tests: `fix/allowed_is_not_fixed.bd`, `fix/mixed.bd`.

**LN32. The procedure.** `--fix` lints the package, applies the machine-applicable fixes of every finding that is
reported, and lints again, until a round changes nothing (at most a fixed number of rounds), because one fix can expose
another. It then compiles the changed package in memory: if the fixed package would not compile, no file is changed, the
errors are printed, and the exit status is 1. The change is printed to stdout as a unified diff per file (`a/<path>`,
`b/<path>`, three lines of context), and the files are written all or none: each is written beside its target and renamed
into place, and if one write fails the files already replaced get their old text back, nothing is changed, and the exit
status is 2. `--dry-run` does everything but write, and prints `fix: would apply N edits to M files`; a real run prints
`fix: applied N edits to M files`; with nothing to fix it prints `fix: nothing to fix` and changes nothing. Edits that
overlap are not both applied: the later one is dropped and found again in the next round. The findings that remain after
the fixes are reported as usual.
Tests: every `fix/` and `fix_packages/` fixture (dry run leaves the file unchanged and prints the expected diff; `--fix`
writes the golden file; a second `--fix` finds nothing), the all-or-nothing check in `check_lint.py`, the `fix.rs` unit
tests.

## Choices that are not obvious

- **The linter does not run on a program with errors.** The checks it relies on (types, conventions, moves) are only
  trustworthy on a program that compiles, and a half-checked program would produce findings that are really errors.
- **Syntax lints and type lints are separate passes.** A name, an import, or an `else` is a question about what was
  written, so those lints read the syntax tree; a clone, a copy, or a `mut` is a question about types and conventions, so
  those lints read the typed tree. Neither pass depends on the other.
- **Silence is preferred to a wrong finding.** Wherever the analysis cannot tell (a name shared by two items, a slice, a
  capture, a mention it cannot place), it reports nothing. The false-positive rate was measured by running the linter over
  the conformance programs that compile (665 single-file programs); the findings were reviewed by lint, and each false
  positive that turned up (generated names, `test` bodies, writes inside closures, slices) was fixed
  and now has a fixture.
- **Lint codes are not error codes.** `L0000`-`L0017` are not in the `E` registry of `tests/README.md`, so
  `tests/check_tests.py` and `docs/error-codes.md` do not know them; `docs/lints.md` is their registry.
- **Not part of this issue.** Editor integration of the suggestions (the LSP) is stage 3 work of its own. `missing_docs`
  (L0017, LN33) came with `burdin doc` (issue #657); it shares the machinery here.
