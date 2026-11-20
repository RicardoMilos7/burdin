# Fixes, `burdin fix`, and editions

This is the behavior of the one edit model the compiler, the linter, the language server, and `burdin fix` share, and of
the command that applies it. The tests are `tests/run_tests.py` (the `suggest:` and `fixed-error:` directives),
`tests/check_fix.py`, `tests/check_lint.py`, and the language server's action tests.

## FX1 — A suggestion is a list of edits with an applicability

A diagnostic may carry suggestions. Each suggestion is a message, at least one edit, and an applicability:

| Applicability | Meaning | JSON name |
|---|---|---|
| `MachineApplicable` | The edit is the one thing that can be meant; a tool may apply it without asking. | `machine` |
| `MaybeIncorrect` | The edit is probably right but could change what the program does. Never applied automatically. | `maybe-incorrect` |
| `HasPlaceholders` | The edit holds text a person must fill in (`todo()`, a `...`). Never applied automatically. | `has-placeholders` |
| `Unspecified` | Advice only. | `manual` |

An edit is a span and the text that takes its place. A suggestion's edits never overlap, and a suggestion that spans two
files is not one fix. `Suggestion::is_machine_applicable` is the only question a tool asks.
(`crates/burdin_diag/src/lib.rs`, `crates/burdin_diag/src/edit.rs`.)

## FX2 — One applier, one diff

`burdin_diag::edit` holds the applier every tool uses: `apply_edits` for one suggestion, `apply_groups` for several
(all-or-none per group), `EditConflict` for two edits that overlap, `unified_diff` for what changed, and `inline_diff`
for the diff lines the human renderer shows under a suggestion.

## FX3 — `burdin fix <target>`

`burdin fix` fixes a package directory or a single file:

1. It runs the front end and the linter and collects every machine-applicable suggestion of the entry package. A
   suggestion whose edit is outside the entry package (the standard library, a dependency, a generated module) is not
   offered.
2. It applies the suggestions, one round after another, until a round changes nothing (FX4).
3. It formats every file it touched (FX8).
4. It prints `fixed <path>:<line>: [<code>] <message>` for each fix it applied, then a summary
   `fix: applied N fixes to M files`, and the diagnostics that remain. The exit status is 1 when an error remains and
   0 otherwise.

`burdin lint --fix` runs the same loop (FX3, FX4) with the linter's level flags.

## FX4 — Rounds

A fix can reveal another: a `mut` that makes a warning go away and shows a clone that is no longer needed. The loop runs
at most 8 rounds and stops when a round changes nothing.

## FX5 — Conflicting edits

Two fixes that touch the same text cannot both be applied. The first in source order is applied and the other is
skipped; after the last round, a skipped fix that its text still offers is reported as a warning naming the file and
line.

## FX6 — A round that makes things worse is undone

When a round's fixes would raise the number of errors, the round is undone, a warning says so, and the loop stops. The
result of `burdin fix` never has more errors than its input.

## FX7 — A dirty tree

`burdin fix` writes only when the version-control working tree under the target has no uncommitted change. Otherwise it
exits with status 2 and names the files; `--allow-dirty` writes anyway. Without `git`, or outside a repository, there is
nothing to protect a review with and the check passes.

## FX8 — The touched files are formatted

`burdin fix` formats every file it changed with `burdin fmt`. A file the formatter refuses keeps the fixed text.

## FX9 — `--dry-run`

With `--dry-run`, `burdin fix` prints the unified diff of every change, writes nothing, and exits 0 (or 1 when an error
remains).

## FX10 — `burdin fix --migrate`

`--migrate` needs a package directory, because an edition is recorded in its `burdin.toml`; a one-file target is refused
with status 2. The command applies the package's fixes (FX3), then:

1. If the package is already at the newest edition, it says so and changes nothing but what the fixes changed.
2. Otherwise it writes the manifest with the next edition (FX11) and checks the fixed package under that edition. On
   any error the migration is refused, the errors are printed, and no file is written: a migration is one reviewable
   change.

## FX11 — Editions

`[package] edition` in `burdin.toml` names the language edition. The compiler knows `2024` (the default, for a manifest
without the line and for a one-file target) and `2026` (the newest). An unknown edition is an error naming the known
ones.

An edition is how a language change reaches code one release later:

1. A form that edition *N+1* removes is deprecated in edition *N*: a warning (`E0301`) with a machine-applicable
   suggestion that writes the replacement.
2. In edition *N+1* the form is an error (`E0002`) with the same suggestion.
3. `burdin fix --migrate` applies the suggestions and moves the package to the edition that makes them errors.

The deprecation one edition earlier is required. The policy, and what the release notes say about it, is in
`docs/releasing.md`.

The first form on this path is `unimplemented`: deprecated from edition 2024 with `panic("not implemented")` as its
replacement, removed in edition 2026.

## FX12 — Conformance test directives

The test format (`tests/README.md`) has two directives for suggestions:

```text
//@ suggest: E0005 machine-applicable: make `n` mutable
//@ fixed-error: E0003
```

`//@ suggest:` names a code, an applicability, and the beginning of the suggestion's message that must be offered.
`//@ fixed-error:` names a code that must still be reported after every machine-applicable suggestion was applied. The
runner applies each suggestion to the case's file and checks that the result compiles, or that exactly the
`fixed-error` codes remain. `tests/check_tests.py` checks the directives, and the registry in `tests/README.md` lists
which codes offer suggestions.

## FX13 — What the compiler suggests

The checker offers a suggestion only when it is the one edit that can be meant:

| Code | Situation | Suggestion | Applicability |
|---|---|---|---|
| `E0002` | A name in another module of the package is spelled right; nothing else is close | `use <module>.<name>` after the last `use` | machine |
| `E0002` | A name is not in scope and exactly one name in scope is a near miss | the near name | machine |
| `E0002` | Several names are close | `help:` line naming them | — |
| `E0005` | A write to an immutable `let` or `for` binding | `mut ` before the binding's name | machine |
| `E0100` | A moved value is used again; the type is `Clone` and not `Copy` | `.clone()` at the move | maybe-incorrect |
| `E0113` | A `mut` parameter is passed without the marker | remove the marker, or take the binding `mut` | machine |
| `E0115` | A `mut` marker on an argument whose parameter takes a shared borrow | remove the marker | machine |
| `E0301` | A deprecated form (FX11) | the replacement | machine |

The linter's suggestions use the same model and the same table's mechanics (`spec/lint.md`).
