# `burdin_query`: the compiler as a library

The query-based compiler library of stage 3 (issue #241, sub-issues #645-#649). The front end answers questions
about a package — the syntax tree of a file, the diagnostics, the type of an expression — and remembers every answer,
so a second question about an edited program costs only what the edit can affect. The command line is one client
(the `burdin` binary runs its `build`, `run`, `test`, and `lint` front end through it), and a language server is the
other. This file is the exact behavior, with a rule ID for each point and the tests that cover it.

The crate is `crates/burdin_query`. It depends on `burdin_syntax`, `burdin_sema`, `burdin_check`, and
`burdin_comptime`, and never on `burdin_codegen`, so a tool that wants the checker does not link LLVM. The command
line keeps the code generation, the driver, and the package layout handling (`package.rs`, moved here from the driver
so the library can open a package too).

```
Db::open / Db::from_files / Db::from_workspace   the inputs: a package on disk, or files in memory
db.set_file(path, text)                           an edit
db.analyze()                                      parse -> declare -> check bodies -> memory checks -> comptime
analysis.diagnostics / analysis.program           the answer
db.stats() / db.counters()                        what the analysis did
```

## The engine

**QL1. An in-house memo, not salsa.** The memo is a table of results keyed by fingerprints, in two places:
`burdin_sema::CheckCache` for checked bodies (it shares the checker's private state), and `burdin_query::Db` for
parsing and for the pipeline around the checker. Salsa and its relatives want `Send + Sync` values in their storage;
`burdin_tir::Program` and the checker's registries are `Rc`/`RefCell` graphs, and the compiler holds no lock anywhere
today. Making them thread-safe would be a change to every stage for a cache that is used from one thread at a time,
and the repository takes no new external dependency without a decision. The memo is therefore a plain table with a
128-bit fingerprint per key, and it is enough: the questions are few, the keys are small, and the answers are what
the compiler already computed.

| Query | Key | Answer |
| --- | --- | --- |
| `parse(file)` | the file's text | the syntax tree and its syntax diagnostics |
| `type_check(item)` | the item's identity, the text of its declaration span, the declaration fingerprint, the pass chain | the typed function or constant, its diagnostics, its closures, its instantiation requests, and the registry changes it made |
| memory checks | none: the whole program | the return origins, moves, and borrow diagnostics |
| compile-time evaluation | none: the whole program | the evaluated program |
| `diagnostics()` | every file | every diagnostic, in the order the batch compiler reports them |

**QL2. Granularity.** A file's parse is keyed by its text. A function or constant body is keyed by the text of its
declaration span (so a body edit misses and a comment elsewhere hits), by the declaration fingerprint, and by the
pass chain (the registries and the generic instances the checker had when it checked the body). The declaration
fingerprint is the text of every file with the parser-recorded body spans blanked out: an edit inside a body does not
change it, and an edit to a signature, a type, an interface, an impl, an import, or a constant's initializer does.
A body that is replayed contributes its diagnostics, closures, instantiation requests, and registry changes to the
new run, with its spans moved to their new positions.

**QL3. What is not incremental.** Two whole-program passes run on every analysis: the memory checks
(`burdin_check`) and compile-time evaluation (`burdin_comptime`). The return-origin inference is a fixpoint over all
signatures, and the move and borrow checks read the whole program's signatures; compile-time evaluation rewrites the
program. Both are measured by the benchmark (`QL9`), and both are cheap next to a cold analysis on the packages the
tests use. The step that makes them per function is a later refinement, not a hidden behavior.

The other documented limit: a declaration edit rechecks every body, because type and function ids are baked into the
typed tree. A body edit rechecks one body.

## Parity

**QL4. One implementation.** `burdin build`, `burdin run`, `burdin test`, and `burdin lint` run their front end
through `Db::from_workspace` and `Db::analyze` (the driver's `front_end`), so a tool and the command line cannot
disagree about a program. The driver passes `Options::no_cache`, because it asks once and exits.

The answer of a database that has been edited all along is the answer of a fresh one:

- `crates/burdin_query/tests/equivalence.rs` generates edit sequences (deleted and duplicated lines, inserted
  comments, swapped operators and literals, added functions) from the conformance corpus and compares the rendered
  diagnostics and a digest of the typed program with a fresh database after every step, and again after restoring
  the original text. `BURDIN_EQUIV_FILES` and `BURDIN_EQUIV_STEPS` scale it; CI runs a larger sweep in release.
- `crates/burdin_driver/tests/parity.rs` runs every conformance test that must fail to compile (`//@ compile-fail`
  and `//@ test-compile-fail`) through the `burdin` binary and through `Db::open`, and requires the same set of
  `(file, line, code)` error diagnostics. Packages whose build script or grants fail are the driver's build phase
  and are covered by `tests/check_build_scripts.py`.

## What an edit makes the database redo

**QL5. Counters.** `Db::stats()` describes the last analysis: `files_parsed` (paths), `bodies_reparsed` (paths whose
syntax tree was updated by reparsing one body), `files_reused`, `bodies_checked` (names, each once),
`body_checks` (every check, including a generic body checked again in a later pass), `bodies_reused`, `passes`,
`memory_checked`, and `memory_reused`. `Db::counters()` sums every analysis so far.

`crates/burdin_query/tests/incremental.rs` pins the behavior: the first analysis checks everything; an analysis with
no edit parses and checks nothing; a body edit rechecks exactly that body and reparses the body, not the file; a body
that grows moves the functions after it without checking them; a new error is reported at its new position and
disappears when fixed; a cached diagnostic follows its function when an earlier body grows; a signature edit
rechecks the callers; adding an item rechecks what can see it; and a syntax error reports and skips checking, and
the fix costs one body.

## Reparsing one body

**QL6. The body-only reparse.** The parser records the span of every function and test body block in
`ast::File::bodies`. When an edit lies strictly inside one of those spans, the new text of that block is still
exactly one well-formed block, the old file had no syntax errors, and exactly one function owns the block, the file's
tree is the old tree with every span after the edit moved and that one body replaced. The moved spans come from
`burdin_syntax::spans::map_spans`, a walk that visits every span the parser produces (a unit test compares a shifted
tree with a parse of the shifted text over the whole conformance corpus). A comment in a body, a changed expression,
and a body with more lines than before all take this path.

Anything else is a full parse: an edit in a signature, between items, or that unbalances the braces; a file that had
syntax errors; the body of `main` (the parser rewrites it to return a `Result`); a body that mentions `select` (the
names `select` generates count the `select`s before it in the file); and a span with more or fewer than one owner.
`crates/burdin_query/tests/reparse.rs` compares the memoized tree with a full parse (the printed tree, every span
included) after each of these edits, and covers each fallback.

## The tool API

**QL7. Plain data, byte offsets.** The methods a tool calls return owned strings, byte offsets, and paths; a tool
depends on `burdin_query` and never on `burdin_tir` or the checker's internals. Positions are byte offsets into the
text `Db::file_text` returns; `Db::line_col` converts one to a 1-based line and column. Types are printed as the
source writes them, without the module path the checker uses to disambiguate.

- `Db::syntax_tree(path)` — a lossless `SyntaxTree`: the tokens, the trivia between them (white space, line
  comments, `///` documentation comments, and block comments), the tree the compiler checks, and the file's syntax
  diagnostics. Tokens and trivia tile the file, and `rebuild()` gives the text back.
- `Analysis::diagnostics_of(path)` — the diagnostics that point into one file.
- `Analysis::resolve_at(path, offset)` — the name at a position: a local, a parameter, a function, or a constant,
  with where it is declared and its type (or the function's signature).
- `Analysis::type_at(path, offset)` — the innermost expression that contains the position and its type.
- `Analysis::signature_of(path, offset)` — the function or method whose declaration contains the position, with its
  parameters (name, type, convention, default), its result, and a display form.
- `Analysis::ownership_of(path, offset)` — the innermost function or closure whose body contains the position: the
  places it moves out of (with the field path and whether the checker proved the local is dead there), the locals it
  owns, and the parameters a second-class result derives from.
- `Analysis::statements_of(path, function)` — a body's statements in order, the tail expression counted as one more:
  a `let` with the names it binds and their types, an expression statement with its type and whether that type is
  `()` or `!`. The REPL reads these to see what an input appended and whether to print it (`spec/repl.md`, RP5).
- `Analysis::expr_facts(path, lo, hi)` — the expression with exactly that span: its type, and every local it
  mentions in source order, merged per name, with whether the mention borrows or borrows `mut`. `:ownership` uses
  it (`spec/repl.md`, RP9).
- `Analysis::members_of_type(ty)` — the fields and variants of the struct or enum a printed type names, and the
  methods whose qualified name starts with it, for completion after `.` (`spec/repl.md`, RP13).

The navigation questions a language server asks of a whole program (`navigate.rs`; `spec/lsp.md` reads them):

- `Analysis::occurrences()` — every resolved name in every body (the standard library's too), once per span and
  name, with its kind and declaration. The span is the expression that resolves: the name for a path, the whole call
  for a call, and the literal for the calls an interpolated string makes.
- `Analysis::locals(path)` — the locals and parameters bound in a file, in source order, with their types and
  whether they are `mut`.
- `Analysis::calls()` — every call: the caller (a closure's calls belong to the function it is written in, and a test
  is shown by its name), the callee and its declaration, the call's span, and, for each argument written in the
  call, the parameter it binds.
- `Analysis::functions()` — every function and method once (a generic one by its declaration), with its signature.
- `Analysis::tests()` — the entry package's `test` items with their full path as `burdin test --list` prints it
  (`module.name`, or the name in the entry module), the name's span, and whether they are `[ignore]`d.
- `Db::modules()` and `Db::packages()` — every file's package and module path, and every package's name,
  directory, dependencies, root module, and whether it is the entry or the embedded standard library.

`crates/burdin_query/tests/api.rs` answers a fixture package probe by probe and compares the answers with
`tests/fixtures/api.golden`; `BURDIN_QUERY_UPDATE=1` rewrites the golden file after a deliberate change. The same
test checks that tokens and trivia tile the file, that a body edit updates the tree, that diagnostics are grouped by
file, and that a position outside a body has no type and no name. `tests/navigate.rs` pins the navigation questions on
the same fixture.

## Cancellation, budget, and memory

**QL8. Stopping, and staying small.** `Db::cancel_handle()` returns a `CancelHandle` (an `Arc<AtomicBool>`) that
another thread can set. The database polls it between files, between bodies, and between stages; the analysis then
returns `Err(Cancelled)` and clears the flag, so the next analysis is a fresh request and the database stays usable.

`Db::set_budget(Budget { bodies })` caps how many checked bodies the memo keeps (40,000 by default, enough for the
standard library and a large program). At the end of every analysis the least recently used bodies beyond the budget
are forgotten; a forgotten body is checked again when it is needed, never answered wrongly. `Db::forget()` drops the
whole memo. Nothing is interned in a global table: every string, tree, and typed body lives in the memo and is freed
with it, so a database that is dropped leaves nothing behind.

`crates/burdin_query/tests/stress.rs` covers cancelling before an analysis and from another thread, the budget's
eviction (with a smaller budget, the answer is still the fresh one), and two thousand keystroke edits that each
recheck one body and keep the memo inside the budget. `crates/burdin_query/tests/memory.rs` uses a counting global
allocator to show that two thousand edits with a budget a hundred entries above the first analysis stay within
8 MiB of the memory the first analysis used.

## Benchmarks

**QL9. The keystroke benchmark.** `cargo bench -p burdin_bench -- query` measures the path a language server takes on
a generated package (`generate_package`): `query/SIZE` is one keystroke-sized edit inside a function body followed by
a full analysis, and `query-cold/SIZE` is the first analysis of the same package. `small`, `medium`, and `large` use
one, two, and four modules. The run also prints how many bodies one keystroke rechecked. `--smoke` runs the same
path with generous time budgets and fails unless exactly one body is rechecked, and the cases are part of the
baseline comparison of `tests/bench_regression.py`.

## Tests and commands

```
cargo test -p burdin_query              the query tests (unit, incremental, reparse, equivalence, API, stress, memory)
cargo test -p burdin_driver --test parity   every failing conformance test, library against the command line
cargo bench -p burdin_bench -- query        the keystroke benchmark
cargo bench -p burdin_bench -- --smoke      the CI guard, including the incremental path
```
