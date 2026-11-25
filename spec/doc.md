# `burdin doc`

Documentation generated from doc comments (stage 3; issues #234, #467, #468, #469, #656, #657, #658, and #910). `burdin doc`
builds the documented API of a package from its source and writes it as a static HTML site, as JSON, or as a coverage
report. This file is the exact behavior, with a rule ID for each point and the tests that cover it.

The generator is the `burdin_doc` crate. It reads the syntax trees and source text of the packages the compiler loaded
and depends only on `burdin_diag` and `burdin_syntax`; the driver (`crates/burdin_driver/src/doc.rs`) loads the
package, prints the diagnostics, and writes the files. It does not run the type checker over the API it documents, and
it is not part of `burdin build` or `burdin test`.

Every behavior below is covered by `tests/check_doc.py`, which runs the fixtures under `tests/doc/` and compares the
output with the golden files under `tests/doc/expect/`. A fixture is a `.bd` file or a package directory; its `//~`
markers name the exact diagnostics, by line, that must be reported, and its `//% args:` line sets the command line.
`python3 tests/check_doc.py --update` rewrites the golden files. The unit tests in `crates/burdin_syntax/src/docs.rs`
(which comment belongs to which declaration) and `crates/burdin_doc/src/tests.rs` (Markdown, links, signatures) cover
the pieces that are easier to check in isolation.

Doc tests (` ``` ` blocks run as tests, issues #250 and #650-#652) exist now: `burdin test` extracts the Burdin code
blocks of the doc comments and runs them ([`test-runner.md`](test-runner.md), "Doc tests"). They are not part of
`burdin doc`'s behavior: this command shows the code blocks and never runs them.

## Doc comments

**DC1. Where a doc comment is.** A run of `///` lines directly above a declaration, with no blank line and no ordinary
comment between them, is that declaration's documentation. Each line is the text after `///` and one optional space, and
the lines are joined with newlines. `////` and longer runs are ordinary comments, so a separator line is never
documentation. `// lint: allow(...)` lines may sit between the comment and its item and do not detach it. CRLF line
endings are read the same way.
Tests: `crates/burdin_syntax/src/docs.rs` (`a_run_of_lines_above_the_item`, `a_blank_line_or_plain_comment_detaches`,
`a_suppression_comment_may_sit_between`, `crlf_lines`), `tests/doc/geometry`.

**DC2. Which declaration it belongs to.** Only a declaration that is the first thing on its line takes the comment
above it (modifiers such as `priv` may come before it), so in `x: i32, y: i32` only `x` has documentation. A comment
above an impl block, an interface method, a field, a variant, or an extension method documents that member.
Tests: `docs.rs` (`only_the_first_declaration_of_a_line_has_docs`), `tests/doc/geometry`.

**DC3. Module documentation.** The `//!` lines at the top of a file, after any blank lines, document the module the
file declares. The first line that is not `//!` ends the run. A one-file target's module is the package, and its
documentation is the package's overview page.
Tests: `docs.rs` (`module_docs_are_the_leading_bang_lines`), `tests/doc/geometry` (the root module and `index.html`).

**DC4. `///` and `//!` are the only sources.** A comment that the lexer drops for code is read back from the source
text by `burdin_syntax::docs`, which `burdin doc` and the `missing_docs` lint share (DC19), so the two always agree
about which item a comment documents.
Tests: `tests/doc/geometry` (the coverage report and `burdin lint -W missing_docs` name the same items),
`tests/lint/docs/missing_docs.bd`.

## What is documented

**DC5. Markdown.** The text is a subset of Markdown: paragraphs, `#` headings (rendered as `h3` and below, because the
page has its own `h1` and `h2`), fenced code blocks with an optional language, block quotes, nested `-`/`*`/`+` and
numbered lists, tables, and horizontal rules. Inline: `` `code` ``, `*emph*` and `_emph_`, `**strong**`, links, `<url>`
autolinks, and `\` escapes. Text is always escaped: an HTML tag, a `<script>` element, or an `&` in a doc comment is
text, never markup of its own. A link whose address uses a scheme other than `http`, `https`, and `mailto` (for
example `javascript:`) is shown as its text and gets no address.
Tests: `tests/doc/safety/escape.bd`, `tests/doc/links/broken.bd` (`fn.not_links`), the `tests.rs` unit tests for
Markdown.

**DC6. Code blocks.** An untagged fenced block is Burdin and is marked `language-burdin`; so is one tagged `burdin`
or `bd` (the first word of the info string — the words after it are the doc-test attributes, `test-runner.md`
"Doc-test attributes"). The block's text is shown verbatim and escaped, except for its *hidden lines*: a line that
begins with `# ` (a hash and one space), or a line that is only `#`, is compiled by a doc test and not shown; the
indentation before the marker is kept in the compiled line, and in a block tagged with another language the `#` is
literal text. A block whose every line is hidden shows nothing at all — no empty code block is written. The
coverage report counts an item as documented "with examples" when its documentation has at least one code block,
hidden or not. The JSON keeps the doc text exactly as written, hidden lines included (DC19).
Tests: `crates/burdin_doc/src/markdown.rs` (`hidden_lines_are_marked_and_stripped_in_place`,
`rendered_burdin_blocks_hide_setup_lines`), `tests/doc/geometry` (`lib/fn.distance_squared.html`, whose example
hides its `use` line), `tests.rs`.

**DC7. What a package documents.** The entry package and only its public interface: the non-`priv` functions, structs,
enums, `error enums`, interfaces and their methods, type aliases, constants, `derive` declarations, and `extern`
functions; the non-`priv` methods of an inherent `impl` block; the non-`priv` extension methods; and the fields and
variants of the types that are documented. Items the parser generated (the impls of an `error enum`, a `: Debug`
in a declaration, the `main` wrapper) and the compiler's internal names (`main`, `__x`) are never documented. The members of a
`priv` type are left out even with `--private`: nothing outside the package can name them.
Tests: `docs.rs` (`public_items_skip_private_and_generated`, `error_enums_are_recognized`), `tests/doc/geometry`.

**DC8. Name resolution for links.** A name in a doc comment is looked up the way a path in code is, from the module the
comment is in (the module itself for module documentation): the module's declarations, then its `use` names, then a
module of the package, then a name from outside the package (a dependency, the standard library, a primitive type).
Only the documented package is followed in detail; anything that leaves it is `external`. A path of one segment names
an item; two segments name `module.Item` or `Item.member`; `Item::member` is the same as `Item.member`.
Tests: `tests/doc/links/broken.bd` (`fn.good`), `tests.rs`.

**DC9. Signatures.** Every item and member has the signature as it is written in the source, from the declaration's
head: `fn name(params): Ret`, `struct Name`, `struct Name: Copy`, `interface Name`, `type Name = ...`,
`const NAME: T = ...`, `error enum` variant texts with their message, and so on. Where the head spans several lines,
the whitespace is collapsed. In the HTML, names of types and interfaces of the package that appear in a signature are
links to their pages; parameters, fields, and methods stay plain.
Tests: `tests/doc/geometry` (`shapes/struct.Point.html`, the JSON golden), `tests.rs`.

## Links in doc comments

**DC10. Forms.** A link is `[Target]`, `` [`Target`] `` (shown in code), an explicit `[text]` with its target in
parentheses, or `[text][Target]`. A
target has one or two segments, `.` or `::` separated: `Circle`, `shapes.Shape`, `Circle.area`, `Circle::area`,
`GeometryError.NegativeRadius`. A bracket that names nothing valid (`[ ]`, `[1]`, `[a b]`) stays text; `\[` shows a
literal bracket. An address with a scheme (`https://…`) or a relative file address is an ordinary link and is not
resolved against the API.
Tests: `tests/doc/links/broken.bd`, `tests.rs`.

**DC11. Resolution and diagnostics.** A link resolves to a module, an item, a member, or an external name, and the HTML
and JSON record which. A link that resolves to nothing is `D0001` (a warning) at the line of the comment that holds it.
A link to a `priv` item or member while `--private` is not given is `D0002` (a warning). Every doc comment is resolved,
including module docs, fields, variants, and members of impl blocks; an extension method's comment is reported once,
though the method is shown on both its module's page and the type's page. `--deny-warnings` turns the warnings into a
failure (status 1) but the output is still written.
Tests: `tests/doc/links/broken.bd` (the `//~ D0001` markers), `tests/doc/links/private.bd` (the `//~ D0002` markers),
`check_doc.py` (`--deny-warnings`).

**DC12. Rendered URLs.** A resolved link becomes an `<a href>` to the target's page, with the anchor of the member. In
the site, links are written from the site root and prefixed with the number of `../` the page needs, so a page under
`shapes/` and a page at the root both work from disk. `TARGET` resolves to the item's page; `Item.member` to
`<page>#<kind>.<member>`.
Tests: `check_doc.py` (site link integrity: every `href` and `src` leads to a file and an anchor that exists),
`tests/doc/expect/geometry`.

## The site

**DC13. What a page shows.** The overview page has the package name, the root module's documentation (DC3), and a table
of modules. A module page has its documentation, then one table per kind of item, then its extension methods and its
`pub use` re-exports. An item page has its signature, its documentation, then its members: fields, variants, methods
(required and provided for an interface), operators, extension methods, and the interfaces it implements. Every page has
a sidebar with the package, every module, and the current module's items, and a search box.
Tests: `tests/doc/expect/geometry`, `check_doc.py` (the type page checks).

**DC14. The type page.** For a type, an implemented interface is listed with the methods it gives the type; an
interface with the same name in the package is linked to its page. A derived interface is marked `derived`. An operator
is shown by how it is written, not by its method name: `Add.add` is `a + b`, `Neg.neg` is `-a`. An extension method is
listed on its receiver's page with the module that declares it and the `use` a caller needs (`use shapes.doubled`). A
method from an impl block for a specific instance (for example `Stack<i32>`) is marked with that instance, and only
those methods are marked.
Tests: `tests/doc/geometry` (`shapes/struct.Point.html`, `shapes/struct.Circle.html`,
`collections/struct.Stack.html`), `check_doc.py` (the type page checks).

**DC15. Private items.** By default the site documents only the public interface. `--private` also documents `priv`
functions, types, fields, methods, and extension methods, each marked `private`, and resolves links to them. A `priv`
item keeps its place in the same tables and pages; it never appears by default.
Tests: `check_doc.py` (the private site), `tests/doc/links/private.bd`.

**DC16. Re-exports.** A `pub use` is listed on the module page with its path and what it stands for, linked when the
target is in the package. An item says which modules re-export it, where they are (`Re-exported by <module>`), and the
JSON records the same (`reexported_by`). The root module's documentation is the package overview, so its re-exports
appear there too.
Tests: `tests/doc/geometry` (`lib/index.html`, `shapes/struct.Circle.html`, the JSON golden).

**DC17. Self-contained.** The stylesheet and the search script are compiled into the binary and copied to
`static/doc.css` and `static/search.js`; a page loads nothing from another server. The search index is
`search-index.js`, a script that sets one global (`window.BURDIN_DOC_INDEX`), so the site works when opened from disk,
where a `fetch` would be blocked. `static/doc.css` follows the system's light and dark setting.
Tests: `check_doc.py` (no page loads an address on another server; the assets are the embedded ones; site integrity),
`tests/doc/expect/geometry`.

**DC18. Search.** Each entry is an object with a short name (`n`), the path a reader knows it by (`p`), a kind (`k`),
the URL inside the site (`u`), and the summary text (`d`). The index lists every module, item, field, variant, method,
operator, extension method, and the interface methods an impl block provides. The browser ranks an exact name, then a
prefix, then a name that contains the query, then a path that contains it, and shows at most 50.
Tests: `check_doc.py` (the index parses, names the items, their members, and the modules, and every entry has the same
fields), `tests/doc/expect/geometry/search-index.js`.

## JSON

**DC19. The schema.** `--output-format json` writes one JSON document:

```
{ "format_version": 1, "package": "geometry", "private_items": false,
  "modules": [ { "name": "shapes", "segments": ["shapes"], "file": "src/shapes.bd", "root": false,
                 "docs": "…" | null, "links": [ … ],
                 "items": [ { "name": "Point", "path": "shapes.Point", "kind": "struct", "private": false,
                              "signature": "struct Point: Copy, Eq, Debug",
                              "docs": "…" | null, "links": [ … ],
                              "span": { "file": "src/shapes.bd", "line": 6, "column": 8,
                                        "end_line": 6, "end_column": 13 },
                              "fields": [ … ], "variants": [ … ], "methods": [ … ],
                              "implementations": [ … ], "operators": [ … ], "extensions": [ … ],
                              "reexported_by": ["geometry"] } ],
                 "extensions": [ … ], "reexports": [ … ] } ] }
```

A member has `name`, `display` (the operator as written), `kind`, `signature`, `private`, `docs`, `links`, `span`,
`instance` (the specific instance of an impl block, or `null`), and `origin` (`{"module", "import"}` for an extension
method, else `null`). An implementation has `interface`, `derived`, `instance`, `where`, `target` (the resolution of an
interface of the package, else `null`), and `members`. A link has `text`, `target`, and `status` (`module`, `item`,
`external`, `hidden`, `broken`) with `path` when it resolved. A re-export has `name`, `path`, `status`, `target`, and
`span`; a `target` is the path when the status is `item`. `format_version` changes only when a field is removed or
changes meaning.
Tests: `tests/doc/expect/geometry.json`, `check_doc.py` (the JSON golden, the header, the item, member, and
implementation fields, determinism).

## Coverage and the lint

**DC20. `--coverage`.** Prints, per module and then for the package, how many public items are documented and how many
have examples, as `3/4 (75%)`, and then the undocumented items with their file, line, kind, and name. `main` is not a
public item and is not counted. The items are the ones `missing_docs` reports, so `burdin doc --coverage` and
`burdin lint -A all -W missing_docs` name the same items.
Tests: `tests/doc/expect/coverage.txt`, `check_doc.py` (the report and the lint name the same items),
`tests/lint/docs/missing_docs*.bd`.

**DC21. `missing_docs`.** The lint `L0017` (`missing_docs`) is off by default and reports a public item with no doc
comment, using the same reading of the source as DC4. Its message names the kind and the name. See
[`spec/lint.md`](lint.md) (LN33) and [`docs/lints.md`](../docs/lints.md).
Tests: `tests/lint/docs/`, `tests/check_lint.py`.

## The command

```
burdin doc <file.bd | package directory> [-o <dir | file>] [--output-format html|json] [--private]
           [--coverage] [--deny-warnings] [--const-steps <n>] [diagnostics]
burdin doc <workspace> -p <member>... | --workspace [--exclude <member>]... [the same options]
burdin doc --std [the same options]
```

**DC21b. Members.** In a manifest-level workspace (`spec/workspaces.md`, WS12), `-p`, `--workspace`, and `--exclude`
select the members to document: one site per member, under that member's `target/doc` when `-o` is absent, with the
usual message for each, and `--coverage` prints one report per member. `-o` names one place, so it needs exactly one
selected member.

**DC22. Loading and status.** `burdin doc` loads the target the way `burdin build` does, including build scripts and
compile-time evaluation, with `test` blocks treated as tests (so a library package needs no `main`). A package that
does not compile is reported with its errors and documented not at all: the exit status is 1 and no file is written.
A usage error (no target, an unknown option, a second target, an unknown `--output-format`, `-o` with `--coverage`, a
missing option value) prints the usage text and exits with status 2. Warnings alone leave the status at 0 unless
`--deny-warnings` is given; a failed write is a status of 2. In `json` diagnostic format the last line is
`{"type":"doc-summary","warnings":N}`.
Tests: `check_doc.py` (the command line, `tests/doc/errors/type_error.bd`, `--deny-warnings`).

**DC23. Where the output goes.** For HTML, `-o` names a directory; the default is `<package>/target/doc`, or
`target/doc` beside a single file. The directory must be empty or one an earlier `burdin doc` wrote: a directory with
other files in it is refused with status 2, so `-o .` cannot overwrite a project. An earlier run leaves a marker file
(`.burdin-doc`) listing what it wrote, and a later run removes the pages of items that are gone. For JSON, the document
goes to standard output, or to the file `-o` names; `-o` is a file here, not a directory. `--coverage` writes nothing
but the report and does not take `-o`.
Tests: `check_doc.py` (the output directory section: refusal, an empty directory, stale removal, the default location,
a single file, JSON to a file).

**DC24. The standard library.** `burdin doc --std` documents the standard library this compiler carries, in place of
a target: the package `std`, its modules, and their public interface, by the same rules as any package (DC5-DC21). The
standard library is built into the compiler, so `burdin doc std/` cannot do this: it would load the folder as a second
package beside the built-in one. The other options are unchanged. A module's file is named as in the repository
(`src/iter.bd`). With no `-o` the site goes to `target/doc/std` in the working directory. `--std` with a target, or
with `-p`, `--workspace`, or `--exclude` (DC21b), is a usage error (status 2). The standard library's own doc comments
have no broken links, so `--deny-warnings` passes.
Tests: `check_doc.py` (the standard library section: the overview and its modules, the `Deque` and `Iterator` pages,
`peekable`, the search index, site integrity, `--deny-warnings`, the JSON, `--coverage`, the default location; the
command line section: `--std` with a target, with `-p`, with `--workspace`, and with `-o` and `--coverage`).

**DC25. The standard library's documentation.** Every public item of the standard library has a doc comment and at
least one example, and every example runs. `tests/check_std_docs.py` reads `burdin doc --std --output-format json` and
checks three things: each entry has a doc comment (always required); each entry has a Burdin example, and an entry that
lacks one is listed in `tests/std_docs/no_example.txt`, a list that may only shrink and is written with
`--write-baseline`; and each example is built and run with the doc-test attributes (spec/test-runner.md, DC6). The
site is not committed: the website builds it from `burdin doc --std`. Compiler-internal names are never documented
(DC7): the `__` helpers that the `select` desugaring calls stay public but are left out of the site (spec/channels.md).
Tests: `check_std_docs.py` (run by `check_ci.py`; `--no-run` skips the examples).
