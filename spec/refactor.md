# The refactoring engine: `burdin ssr` and `burdin refactor`

One engine serves the command line and the language server: it reads one analysis of a package, computes the new texts,
and checks them before it hands them back. A refactoring that would not compile, or would change what the program does
to a local variable, is refused with the reason and changes nothing. The crate is `crates/burdin_refactor`; the tests are
`tests/check_ssr.py`, `tests/check_refactor.py`, `crates/burdin_refactor`'s unit tests, and the language server's action
tests.

## RF1 — A pattern is Burdin code with placeholders

`burdin ssr '<pattern> [==>> <replacement>]' <target>` takes a pattern written as one Burdin expression. `$name` stands
for any expression; the same placeholder twice binds the same text twice (`$a - $a` matches `x - x` but not `x - y`).

After the pattern, an optional `where` clause constrains the placeholders' types:

```text
$a.unwrap_or($b) where $a: Option<_>
```

`_` covers any one type argument, or the rest of the type when it is last (`Option<_>`, `Result<_, Result.Err>`). The type is
what the checker typed the matched expression with; a placeholder whose type the checker does not have never matches a
constraint (`crates/burdin_refactor/src/ssr.rs`).

## RF2 — The result is checked

Every operation checks the new texts before returning them: the package must compile with no error it did not have,
and the moves of every local must be the same (RF8). A replacement that does not check is not written. When several
matches are replaced and the result as a whole does not check, each match is tried alone and the ones that keep the
package checking are applied; the others are reported (`burdin ssr` prints `warning: skipped the match at ...`).

## RF3 — Names match by what they resolve to

A name in a pattern matches a name in the code when they denote the same declaration, not when they are spelled the
same: `geometry.rectangle_area` matches `rectangle_area` in a file that imported it, and a name imported under an alias
matches its declaration. Two names that do not resolve, or that resolve to different declarations, match only when
their spelling is equal.

## RF4 — A match is a whole expression, and never inside a string

Only whole expression nodes are matched and replaced. Text inside a string literal, an interpolated string's `{...}`
holes, a comment, or a token is never touched. A match inside another match is not listed separately: replacing the
outer one covers it. Code the compiler generated (a derive's impl, a lowered `error enum`) is not searched.

## RF5 — The replacement carries the matched text

Each `$name` in the replacement becomes the text the pattern bound. When the text is not atomic (a name, a literal, a
call, an indexing, a field access) and the placeholder stands in an operand position — beside an operator, after `.` —
it is parenthesized: `$a * 2` with `$a = x + 1` becomes `(x + 1) * 2`.

## RF6 — Search and replace on the command line and in the editor

- Without `==>>`, `burdin ssr` lists every match as `<path>:<line>:<col>: <text>` and exits 1 when there is none.
- With it, the matches are replaced and every touched file is formatted. `--dry-run` prints the unified diff and writes
  nothing; the version-control check of `spec/fix.md` FX7 applies (`--allow-dirty`).
- In the language server, the command `burdin.structuralSearchReplace` takes
  `{ "pattern": "...", "replacement": "...", "root": "<package directory>" }` and returns a workspace edit.

## RF7 — `burdin refactor`

`burdin refactor <refactoring> <file>:<line>:<col>[-<line>:<col>] [OPTIONS]`. A position is a 1-based line and column;
a range's end is the position one past its last character. Each refactoring reads the package the file belongs to (the
nearest directory above it with a `burdin.toml`), and refuses a package that has errors. `--dry-run`, `--allow-dirty`,
and the formatting of touched files are those of `burdin fix` (FX7-FX9). The refactorings are also code actions in the
language server: `refactor.extract` for a selection, `refactor.inline` at a position.

## RF8 — What "the same meaning" means

Each refactoring computes its new texts and hands them to the engine, which:

1. analyzes the new texts and refuses when the package has a new error; and
2. compares, for every function both versions have, how often each local is moved. A local the operation renames is
   compared under its new name; a local the operation introduces or removes is not compared.

A refusal prints the first difference: `` `main` would move `s` twice instead of once ``. The old texts are restored
before returning, so a refused operation leaves the database as it was.

## RF9 — Extract variable

`burdin refactor extract-variable <range> --name <name>` replaces the innermost expression that covers the selection
with `name`, and inserts `let name = <expression>` on its own line before the statement holding it.

It refuses when:

- the expression would run a different number of times where the local is declared — it is inside a loop body, a
  `while` condition, the right of `&&`/`||`/`orelse`, a branch, a closure, or a string's hole;
- the expression is assigned to or taken with `&mut` (its value is not what is meant);
- the statement does not start its line;
- the name is not free in the function.

## RF10 — Extract function

`burdin refactor extract-function <range> --name <name>` moves the statements the selection covers into a new function
declared after the one they came from, and replaces them with a call.

- **Parameters.** Every local the selection mentions and does not declare, in the order it first mentions it:
  - a local it moves becomes `own`;
  - a local it writes, or borrows exclusively, becomes `mut`;
  - every other one is read, the default convention.
  The call passes `mut x` for a `mut` parameter and `x` for the rest.
- **The value.** When the last selected statement is an expression whose type is not `()` or `!`, the function returns
  it and the call stands in its place.
- **The body** is re-indented by one level.

It refuses when the selection is not whole statements of one block, holds `return`, `break`, `continue`, or `self`, binds a local the code after the selection uses, or uses a local whose type is not one a
parameter can take.

## RF11 — Inline variable

`burdin refactor inline-variable <position>` replaces the one use of a local with the value it was given and removes its
`let`.

It refuses when the local is a parameter or `mut`, has no use or more than one, its `let` is not alone on its lines, the
value is a computation rather than a name or a literal, the value reads a mutable local, or the use runs a different
number of times than the declaration (RF9's list).

## RF12 — Inline function

`burdin refactor inline-function <position>` replaces the one call of a free function with the function's body as a
block expression, and removes the declaration with its doc comment.

It refuses when the function is generic, a method, `unsafe`, `blocking`, called zero or more than one time, calls
itself, holds `return` or `self`, or takes an argument that is not a name or a literal (moving it could
reorder effects) or whose text names a local the body declares (the substitution would capture it).

## RF13 — Change signature

`burdin refactor change-signature <position> [--reorder a,b] [--add 'name: type = default'] [--remove name]
[--rename old=new]` rewrites a free function's parameter list and every call of it. The parts apply in this order:
remove, reorder, add, rename.

- `--reorder` names every parameter that stays, once, in its new order.
- `--add` needs a default: a call written before the change does not pass it.
- A call keeps its argument for each parameter it wrote, in the new order, and keeps a named argument named (under the
  new name after a rename); a parameter it left out stays left out.
- A rename rewrites the parameter's uses in the body.

It refuses a name that is not a parameter, a duplicate name, a reorder that is not a permutation, an addition without a
default, and the removal or rename-to of a parameter the body uses.

## RF14 — Move item

`burdin refactor move-item <position> --to <module>` moves a top-level function, constant, struct, enum, type alias, or
interface into another module of the same package.

- The item keeps its name, so the module it left re-exports it: `pub use <new module>.<name>` goes after its last `use`.
  Every file that used the item keeps working.
- The names the item used from its old module are imported into the new one (`use <old module>.<name>`), unless the new
  module already has them. An item that uses names of the package root is refused when it moves out of the root, since
  no other module can import them.
- The doc comment above the item moves with it.

It refuses a selection that is not a movable item, a module the package does not have, and a move into the item's own
module.
