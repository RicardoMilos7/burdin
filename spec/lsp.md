# The language server

`burdin lsp` is the Burdin language server (stage 3; issue #242 and its sub-issues #309-#313, #365-#376, #470, and
#662; completion, snippets, and the built-ins in #819-#824; arrays and slices in #1039; completion beside errors in #1015). It speaks the Language Server Protocol 3.17 over standard input and output and answers every request from the
query-based compiler library (`spec/query.md`), so an editor sees exactly the diagnostics, types, and names the command
line does. This file is the exact behavior, with a rule ID for each point and the tests that cover it.

The server lives in `crates/burdin_lsp`. `burdin lsp` runs it from the driver; the crate's own `burdin-lsp` binary is the
same server without the LLVM back end, which the integration tests start. The tests are black-box: each one starts the
binary as a child process and talks JSON-RPC to it through the client in `crates/burdin_lsp/tests/common/mod.rs`, with
the fixtures under `crates/burdin_lsp/tests/fixtures/` copied to a fresh temporary directory, and compares structured
answers with golden files in `crates/burdin_lsp/tests/golden/` (`BURDIN_LSP_UPDATE=1` rewrites them; paths are
written `ROOT/...` and the standard library `STD/...`, so the goldens hold on every platform). The unit tests inside the
crate cover the JSON value, the framing, position conversion, the file index, and the text mapping of stale analyses.
`tests/check_lsp.py` drives the real `burdin lsp` binary, including a test run through a lens. Every test file starts
with the rule IDs it covers.

## The decision (issue #365)

- **Stack.** The server is Rust, in this workspace, with no new external dependency: JSON and the `Content-Length`
  framing are small enough to write (`json.rs`, `rpc.rs`), and a protocol crate would bring a serialization stack the
  rest of the compiler does not use. It links the query library, the formatter, and the linter, and never LLVM.
- **Transport.** Standard input and output only (`--stdio` is accepted and changes nothing). No socket or pipe
  transport: every editor the project targets starts a server as a child process.
- **Command.** Editors run `burdin lsp`. The `burdin.path` setting (LS5) names another `burdin` for the test lenses.
- **Threads.** The query database is not thread-safe (`Rc` and `RefCell`), so one thread owns every database and
  answers requests in order. A reader thread takes messages off the input, records cancellations, and stops a running
  analysis; test runs happen on worker threads (LS19).
- **Editor targets.** VS Code, through the extension in `editors/vscode`; Zed, through the extension in `editors/zed`;
  Neovim (0.10 and later) and Helix through their built-in clients, configured as `editors/README.md` shows. Anything
  else that speaks LSP over stdio works the same way. LS20 lists what is checked automatically and what is a manual
  checklist.
- **Test path.** `cargo test -p burdin_lsp` (the integration tests above, in CI's Rust job on every platform) and
  `python3 tests/check_lsp.py` (the driver's `burdin lsp`, in CI's LLVM jobs).

## Transport and lifecycle

**LS1. Framing.** Each message is a `Content-Length` header (other headers are ignored), an empty line, and a UTF-8
JSON body of exactly that length. A body that is not JSON is answered with a `-32700` parse error (id `null`); a body
that is JSON but not an object with `-32600`; the server then goes on reading. A malformed header (a line without
`:`, or a length that is not a number) means the stream is not LSP at all and ends the server as if the input closed. Responses and notifications are written whole, one at a
time.
Tests: `malformed_messages_get_a_parse_error_and_the_server_goes_on`; `rpc.rs` and `json.rs` unit tests.

**LS2. Lifecycle and documents.** Before `initialize`, a request gets `-32002` and a notification is dropped (except
`exit`). `initialize` answers the capabilities in `tests/golden/capabilities.json` and `serverInfo`; a second
`initialize` is `-32600`. After `shutdown`, requests get `-32600`. `exit` ends the process with status 0 after
`shutdown` and 1 without it; the input closing ends it the same way. An unknown request is `-32601`; an unknown
notification is ignored. Documents sync incrementally (`textDocumentSync.change = 2`): `didOpen` takes the editor's
text over the file on disk, `didChange` applies each change in order (a change without a range replaces the text),
`didClose` returns the file to its text on disk, and `didSave` changes nothing (the editor's text is already current).
Tests: `initialize_announces_the_capabilities`, `exit_after_shutdown_is_0_and_without_it_is_1`,
`requests_out_of_order_and_unknown_methods_are_errors`.

**LS3. Positions and URIs.** The client may offer `general.positionEncodings`; the server picks `utf-8` when offered
and `utf-16` otherwise (the protocol's default), and announces the choice as `positionEncoding`. Every position the
server reads or writes counts in that encoding: UTF-16 code units (an emoji is two) or bytes. A character past the end
of its line means the end of the line, and a line past the end means the end of the text. Only `file:` URIs are served;
paths are percent-decoded, drive letters are normalized, and Windows' `\\?\` prefix never reaches a URI.
Tests: `positions_count_utf16_units_or_utf8_bytes_as_negotiated`,
`diagnostic_ranges_after_non_ascii_text_follow_the_encoding`; `text.rs` unit tests.

## Packages and workspaces

**LS4. One database per package.** A file belongs to the package of the nearest directory above it with a
`burdin.toml`, or, with none, is a package of one file. Each package root has one query database (`spec/query.md`),
loaded the way `burdin test` loads a package, so `test` blocks are checked. The workspace folders' packages are found
when the client is initialized (a folder's own `burdin.toml`, and those in its subdirectories up to four levels down,
skipping hidden directories, `target`, and `node_modules`); others load when one of their files opens. A package's path dependencies are part of its database, and
an edit in one is applied to every database that contains the file, so the packages that depend on it see the change.
The memo of the query library means an edit re-checks only the bodies it can affect: the bodies of a dependency are
reused, not checked again, after an edit of the package that uses it (`burdin/status` reports the bodies the last
analysis checked). Workspace folders can be added and removed (`workspace/didChangeWorkspaceFolders`); a removed
folder's packages are dropped unless a file of theirs is open. The server registers a watcher for `**/*.bd` and
`**/burdin.toml` when the client allows dynamic registration: a module created or deleted on disk reloads its package,
a changed manifest reloads the packages that use it, and a changed file that is not open replaces its text.

An official package needs no manifest entry (`spec/packages.md`, PKG15), so the server loads it the way the compiler
does: it scans the package's sources for `use <name>`, and the text of an open document replaces the file on disk, so
an unsaved `use json` counts. A database's package set is fixed when it opens, so when an edit adds an official import
the server reopens that package; a `use` completion offers every official package the compiler has, not only the ones
already loaded. Tests: `a_dependency_in_another_root_is_navigable_and_not_checked_again`,
`folders_come_and_go_and_new_files_are_picked_up`, `workspace_symbols_match_fuzzily_across_files`,
`a_new_official_use_reloads_the_package`, `a_use_offers_every_official_package`.

**LS5. Configuration.** The settings live under `burdin` and arrive as `initializationOptions`, in
`workspace/didChangeConfiguration`, or, when that notification carries no `burdin` settings and the client supports
`workspace/configuration`, from a request the server sends. A key that is absent keeps its value.

| Setting | Default | Meaning |
| --- | --- | --- |
| `inlayHints.types` | `true` | the inferred type after a `let` name without one |
| `inlayHints.parameterNames` | `true` | the parameter's name before a positional argument |
| `inlayHints.ownership` | `false` | the ownership lens: moves and implicit drops (`spec/check.md`, CK10) |
| `diagnostics.lints` | `true` | publish `burdin lint` diagnostics too (LS7) |
| `diagnostics.debounceMs` | `200` | the quiet time after an edit before the analysis (at most 10000) |
| `formatting.enabled` | `true` | answer formatting requests (LS15) |
| `codeLens.enabled` | `true` | show the lenses (LS19) |
| `path` | `""` | the `burdin` a lens runs; empty means this executable when it is `burdin`, else `burdin` on `PATH` |

A change republishes the diagnostics and asks the client to refresh its inlay hints and lenses when it supports that.
Tests: `lints_are_published_unless_turned_off`, `configuration_is_pulled_when_the_client_only_says_it_changed`,
`inlay_hints_for_types_and_parameter_names_can_be_turned_off`, `the_ownership_lens_shows_moves_and_drops_and_a_hover_lists_them`,
`lenses_run_and_debug_tests_and_main_with_exact_command_lines`,
`formatting_matches_burdin_fmt_and_refuses_a_file_that_does_not_parse`.

**LS6. Cancellation and progress.** `$/cancelRequest` for a request that has not started makes it answer `-32800`
(`RequestCancelled`) without doing the work; for one that is running, it stops the analysis it waits on (through the
query library's cancellation, `spec/query.md`) and the request answers `-32800`. Cancelling a request that has been
answered does nothing. An edit stops the background analysis of the package it changes; the analysis starts again
after the debounce. When the client supports `window.workDoneProgress`, loading and analyzing a package and running
tests report `$/progress` (`begin`, then `end`) on a token the server creates first; a client without that support
gets no progress.
Tests: `a_cancelled_request_answers_request_cancelled_and_the_next_one_succeeds`, `loading_a_package_reports_progress`,
`the_test_command_reports_a_missing_compiler_and_rejects_other_commands`.

## Diagnostics

**LS7. Published diagnostics.** After an edit and the debounce, the server analyzes the package and publishes the
diagnostics of every file of the entry package that has some, of every open file (an empty list clears it), and of
every file whose diagnostics went away. A publication carries the document's version, and an analysis overtaken by a
newer edit is cancelled, never published: rapid edits publish once, for the last version. Each diagnostic has the
compiler's code (`E....`, a link to the error reference as `codeDescription`), severity, message, the span the compiler
reports (in the negotiated encoding), and the related locations as `relatedInformation`. Every error of a file is
published, not the first (CLAUDE.md, "Report every error in a file"). On a program that compiles, `burdin lint` runs
with the levels of the package manifest's `[lint]` table and its diagnostics are published with source
`burdin lint`; unused imports, variables, and code carry the `Unnecessary` tag. A file outside every package and
every folder is analyzed alone; closing it clears its diagnostics. A package whose manifest does not load reports
`cannot load the package: ...` at the start of each of its open files, and is loaded again when the manifest changes.
Tests: `every_error_of_a_file_is_published_with_its_code_and_span` (`broken-diagnostics.json`),
`a_fix_clears_the_diagnostics_and_a_syntax_error_is_reported`, `rapid_edits_publish_once_for_the_latest_version`,
`lints_are_published_unless_turned_off`, `closing_a_file_outside_any_package_clears_its_diagnostics`,
`a_manifest_that_does_not_load_is_reported_until_it_is_fixed`.

## Navigation

**LS8. Definition.** `textDocument/definition` (and `declaration`, the same) resolves the name under the cursor: a
declaration names itself; a local or parameter its binding; a `use` path its module or item; a call, type, constant,
or variant its declaration; a field or method after `.` the member of the receiver's type (through the analysis); a
field in a struct literal the struct's field, and a shorthand field (`Rect { w, h }`) the local it reads. A name after a
leading dot resolves as its explicit spelling does (`target-typing.md`): `.Green` is the variant `Color.Green`, `.new(1,
2)` the method `Point.new`, and the `x` of `.{ x: 3 }` the field of the struct the position expects. A name of the
standard library opens a copy of its source materialized under the temporary directory. The standard library is part of
every package's database (embedded, read-only, and memoized like a dependency: an edit never checks it again). The
compiler's built-ins have no Burdin source (`println`, `format`, `panic`, the built-in types, and their compiler-provided
methods such as `String.new`, `str.len`, `i32.to_string`, `List.push`); the server writes their declarations, with a
doc comment each, into one generated file, `builtins.bd`, beside the materialized standard library (`builtins.rs`),
and a definition lands there. The same declarations answer hover, signature help, and completion; the file is never
compiled, and its names cannot be renamed. A keyword, literal, or comment
has no definition (`null`). When the latest analysis has no program (the file does not parse), the last analysis that
had one answers, with positions mapped through the edit.
`self` and `Self` name the type of the method (or impl block) around them, so `self.name` and `self.area(` resolve to
that type's member whether or not the body parses. A value of a generic parameter's type (`s: S` with `S: Serializer`)
resolves its methods through the bounds (the interface's declaration), and a parameter's declared type answers when no
analysis does. The callee of a call still being written resolves like a complete one: a method named with missing
arguments, with an unclosed list, or without its call (`a.append()`, `a.append(`, `sq.grow`), and a function named the
same way (issue #851).
Tests: `definition_finds_functions_types_methods_fields_locals_and_modules`,
`builtins_and_the_standard_library_answer_definition_hover_signatures_and_completion`,
`definition_resolves_the_callee_of_an_incomplete_call`.

**LS9. References, highlights, rename.** `textDocument/references` lists every identifier, in every file of every
package that contains the declaration, that resolves to the same declaration; `includeDeclaration` adds the
declaration. A method of one implementation is a different symbol from the same method of another.
`textDocument/documentHighlight` is the references in the file, with the declaration and assignments as writes (kind 3)
and the rest as reads (kind 2). `textDocument/rename` edits every reference; a new name that is not an identifier or is
a keyword is `-32602`, and the standard library, a module, a test's name, and a keyword cannot be renamed
(`prepareRename` answers `null` or an error, rename an error). Renaming a field written in shorthand keeps the local
(`Rect { width: w }`), and renaming a local written in shorthand keeps the field (`Rect { w: wide }`).
Tests: `references_and_highlights`, `rename_changes_every_use_across_files`.

**LS10. Implementations and hierarchies.** `textDocument/implementation` goes from an interface to the types'
implementations of it, from an interface's method to each implementation's method, from an implementation's method to
the interface's declaration, and from a type to its impl blocks. The call hierarchy (`prepareCallHierarchy`,
`incomingCalls`, `outgoingCalls`) is built from the typed program: a call is listed only when the source writes the
callee's name (not the formatting calls of an interpolated string, nor an overloaded operator), a call inside a closure
belongs to the function the closure is written in, a `test` block is a caller, and a built-in function without a
declaration is not a callee. The type hierarchy goes from an interface to the types that implement it and the
interfaces that require it (subtypes), and back (supertypes).
Tests: `implementations_of_interfaces_their_methods_and_types`, `call_hierarchy_in_both_directions`,
`type_hierarchy_between_interfaces_and_types` (`type-hierarchy.json`).

## Editing

**LS11. Hover.** A declaration, call, or member shows its signature as the command line prints it (`fn total(a: i32,
b: i32): i32`), followed by its doc comment; a local or parameter shows `let name: Type`; any other expression shows its
type. A local or parameter also lists the ownership events the memory checks recorded for it (`spec/check.md`, CK10),
one line each: `moved to \`t\``, `copied`, `borrowed (read)`, `borrowed (mut)`, `dropped`, `cloned`, with the
`line:column` of the event. The events come from `Analysis::ownership_events`; the server computes nothing. The
answer's range is the name. A callee still being written shows its signature too: a method named without its call
(`sq.area`), a call whose arguments are not closed (`sq.area(`), and a module's function named alone. A member of a
value resolves through the local declared last before it when the typed tree no longer holds the value (the method
named without a call drops its receiver from the tree, and unclosed text has no tree); a hover never shows `{error}`
for those. A member named after a dot is never resolved through a local or parameter that shares its name when it is
itself a member: in `resp.body.read_to_string()`, `body` is the field, and a `let body` on the same line is not its
declaration. The method's signature shows in a nested chain as it does on a one-level call.
Tests: `hover_shows_signatures_types_and_docs`, `hover_shows_the_callee_of_an_incomplete_call` (`fixtures/incomplete`),
`hover_on_a_method_of_a_nested_receiver_shows_its_signature` (`fixtures/nested`),
`the_ownership_lens_shows_moves_and_drops_and_a_hover_lists_them`
(`ownership-hover.json`), `positions_count_utf16_units_or_utf8_bytes_as_negotiated`.

**LS12. Completion.** What is offered depends on where the cursor is (issues #819-#824):

- **After `x.`**, the members of `x`. A module (an alias, an imported module, a dependency, or a standard-library
  module the prelude binds, such as `mem` or `fs`): its items. A type written by name (`String.`, `i32.`, `Rect.`): its
  variants and its associated functions (no `self`): the built-ins' (`String.new`, `String.with_capacity`), the
  standard library's hidden `__type_name` ones by their name (`f64.parse`, `i32.from_le_bytes`), and the user's. A
  value: its fields, then its methods, by the receiver's type: the type's own methods and the extension methods on it
  (the prelude's included, such as `str.trim` and `i32.pow`), the built-in methods of a built-in type
  (`i32.to_string`, `String.push_str`), the methods of every interface it implements or derives, and the ones an
  implementation or derive provides (`to_string` through `Display`, `clone` through `Clone`, `cmp` through `Ord`). A
  literal is a value of its type: `"..."` a `str` (an interpolated one a `String`), an integer literal an `i32`, a float
  literal an `f64`, a character a `char`, and `true`/`false` a `bool`. On a line that does not parse yet, the type
  comes from the last analysis with a program and the receiver's declaration (or its spelling, for a literal).
  An array or a slice (`[T; N]`, `[T]`, `&[T]`; issue #1039) has the methods of `[T]`: the ones the compiler
  implements (`len`, `iter`, `get`, `last`, `to_list`, `swap`, `fill`, `copy_from_slice`, `clone_from_slice`, `chunks`,
  `chunks_exact`, `windows`, and, for an array only, `iter_mut`; `spec/stdlib.md`, "Slices"), declared in `builtins.bd`
  under the owners `[T]` and `[T; N]`, and the extension methods of `std/src/slice.bd`, whose owner is the receiver as
  written (`[T]`, `[[T]]`, `[u8]`, `[String]`, `[str]`) and which fit when the element does (a one-capital-letter
  name is a type parameter and fits any element, so `[u8].is_ascii` is offered on `[u8]` only and `[T].first` on every
  slice). An array also has `clone` and `cmp`; a slice is an `Iterator` (`spec/iterators.md`, IT5), so it has the
  adapters (`map`, `filter`, `fold`, ...) and `next_back` too, which an array does not (`xs.map(f)` is E0002;
  `xs.iter().map(f)` is the spelling). A `List` is not viewed as a slice implicitly: it offers its own methods (its
  `first`, `contains`, and `sort` are built in) and never an extension of `[T]` that needs `as_slice()`
  (`list.binary_search(1)` is E0002, so it is not offered). A range offers `clone`, `cmp`, `iter`, and `into_iter`, a
  tuple `clone` and `cmp`, and the compiler's own iterators (`Chars`, `RangeIter`, `MapIter`, `IterMut`,
  `ListIntoIter`, `ArrayIntoIter`, `MapIntoIter`, `Chunks`, `ChunksExact`, `Windows`) the methods of `Iterator`. Hover,
  definition, and signature help answer the same methods: `xs.len` lands in `builtins.bd`, `xs.first` in `slice.bd`,
  and `xs.swap(` shows `fn swap(mut self, a: usize, b: usize)`. The element of an array literal receiver is read
  from its first element when the typed tree does not hold it (`[1, 2].` is `[i32; _]`).
  `self.` in a method offers the members of the type the method belongs to (its impl block's type, or an extension
  method's receiver), whether or not the body parses. A value whose type is a generic parameter (`s: S`) offers the
  methods of the parameter's bounds (`S: Serializer` gives `serialize_bool`, `serialize_unit`, ...), read from the
  function's `<...>` list or `where` clause; with no analysis to answer, the parameter's declared type says what it is
  (issue #852). A member chain whose last member is still missing (`resp.body.`, `client.send(req).`) is typed from the
  declarations when the typed tree does not hold its receiver (the analysis of the text with the dangling dot types it
  `{error}`): the root's type (a local, a parameter, or `self`), then each field's declared type and each method's
  return type, so the list appears on the keystroke that types the dot (issue #879). A name that is an official
  package (`json.`, `crypto.`) lists its public API even when no file of the package imports it: the items its root
  declares and the ones its `pub use` declarations re-export, transitively, and the modules it re-exports by name
  (`crypto.hash`). Accepting one of those names when the file does not load the package adds its `use` as an
  `additionalTextEdits` entry, as a bare name does (`http.Cli` accepts `Client` and inserts `use http`), when the client
  advertises `additionalTextEditsSupport`. The package itself, a local module of that name, or a path or git dependency
  of it shadows the official package (`spec/packages.md`, PKG15).
- **After a leading dot** (`.Red`, `.new(3)`, `.{ x: 1 }`, `.Gr`), the members of the type the position expects, as
  `T.` offers them: its variants and its associated functions (no `self`), and `Some` and `None` for `Option`, `Ok` and
  `Err` for `Result`. The expected type is the one the compiler gives the form (`target-typing.md`, TT2). A dot with
  nothing after it is completed through a copy of the text that names a placeholder after the dot and closes the
  brackets its line still needs; the document's own text is put back before the answer. A dot whose position gives no
  type offers nothing (TT3). Tests: `leading_dot.rs`.
- **At a field's name inside a struct literal's braces** (`Point { x: 1, `, `.{ x: 1, `), the fields of the struct the
  literal builds that it has not written yet, then the names in scope for shorthand (`Point { x, y }`). A literal is
  `Point {` (an uppercase name before the brace, which names a struct) or a leading `.{`, whose struct is the type the
  dot's position expects, read from the document as it stands. Only a brace in a code body counts, so a struct's own
  declaration offers nothing here. Tests: `leading_dot.rs`.
- **In a `use` path**, the modules and the public items below the path written so far, from the package and its
  dependencies (a dependency's `priv` items are left out).
- **At a function declaration's parameter**: directly after `fn name(` inside an impl block, an interface, or a
  `derive`, or in an extension method (`fn Type.name(`), the receiver forms `self`, `mut self`, and `own self`; at any
  other parameter, and in a free function, only the conventions `mut` and `own`. A `self` parameter is
  valid only first and only in a method, so it is never offered elsewhere.
- **In the `: A, B` list after `struct Name` or `enum Name`** (and their generic parameters; issue #825), after the
  `:` or a `,`, the nine derives the compiler knows (`Copy`, `Clone`, `Eq`, `Ord`, `Hash`, `Default`, `Debug`,
  `Serialize`, `Deserialize`, `spec/derives.md`) and
  the `derive` declarations the file can name (its own, the imported ones, and the standard library's public ones),
  and nothing else. Directly after the type's name nothing is offered (the list's `:`, `repr(C)`, or the body comes
  next), and a `:` inside the generic parameters starts a bound, not the list. The name a `struct`, `enum`,
  `interface`, or `fn` is introducing gets no completion.
- **Where a type is written** (issue #849): after the `:` of a `let` or `const` annotation, of a function parameter
  (`fn f(a: `), of a return type (`fn f(): `), and of a struct's or enum's field, the types the name can stand for: the
  file's structs, enums, and type aliases, the imported and the prelude's, the built-in types, and the generic
  parameters of the function around the cursor (in its body; a signature still being written has none yet). Values,
  functions, and interfaces are not offered.
- **Where an interface is written** (issue #849): after `any` or `impl` (`x: any `), in a generic bound (`fn f<T: `),
  in a `where` clause (`where T: `), after a `+` that continues a bound list, in an interface's `requires` list, and
  after an impl block's `Type: ` written at the start of a top-level line (`Point: `, which names the interface the
  impl implements). The interfaces the name can stand for are offered (the file's, imported, and the prelude's), and
  nothing else. A module stays offered in both, as a path prefix.
- **Anywhere else**, the locals and parameters declared before the cursor (nearest first), the file's items, the
  imported names, the built-in functions, the prelude (every public item of every standard-library module, and the
  one-segment standard modules by name, such as `env`, `fs`, and `mem`), the built-in types, the keywords, and, while
  a name is being typed, the items of every official package the package does not shadow, by their qualified name
  (`http.Client`). Such an item filters by its own name (`filterText`), and carries an `additionalTextEdits` entry
  inserting `use <package>` — after the file's last `use`, or at the top with a blank line after when it has none —
  when the package is not already in the graph and the client advertises
  `textDocument.completion.completionItem.additionalTextEditsSupport`; once the package is in the graph, the
  qualified name compiles as written and no edit is sent. Only official packages are indexed this way: a path or git
  dependency offers nothing until the manifest declares it (`spec/packages.md`, PKG15). The package's own name is
  offered too, as a module (`htt` offers `http`), with the same `additionalTextEdits` entry: accepting it loads the
  package the way accepting one of its items does.

**LS21. Completion does not depend on the file being error-free** (issue #1015). An error elsewhere in the file or the
package (an unknown name, a type mismatch, an unfinished statement, a dangling dot) never takes away what the position's
own code resolves: members after a dot, the names in scope, struct-literal fields, and hover, definition, and signature
help beside it. The compiler hands over a typed program only when no file has a syntax error, and the line being typed
usually is one (`p.`), so the server types the receiver of a member dot in this order, cheapest first: the last
analysis that had a program, trusted only for an expression that ends where the receiver does (text typed over a line
maps onto unrelated code in an older tree); the receiver's declaration read from the text, for a file that never
parsed since it opened (`let p: Point`, `Point { ... }`, `Point.origin()`, `make()`, an array literal, a literal alone on
its line, and a parameter's annotation); and an analysis of a copy of the text without the dot and the member name
being typed (`p.` becomes `p`), which types the receiver whenever the rest of the file parses. The last costs a
check without the memo, so it runs only for a receiver that a `let` or a parameter in sight declares, or that ends in
`)` or `]`. A local the older analysis saw still says that its name is a value, so `list.` is not the `list` module
(`is_value`). The document's own text is put back before the answer.
Tests: `completion_does_not_wait_for_the_rest_of_the_file_to_be_error_free` (an unknown name, a mismatch, an unfinished
statement, and an unfinished member on another line, in the same function and in another, and an error after the
cursor's line), `completion_works_in_a_file_that_never_parsed` (`fixtures/partial-syntax`),
`hover_and_definition_work_beside_an_unrelated_error` (`fixtures/partial`).

**Snippets.** When the client advertises `textDocument.completion.completionItem.snippetSupport`, the server adds
snippet items (kind `Snippet`, `insertTextFormat` 2, tab stops `${n:...}` and the final `$0`). A snippet replaces the
plain keyword of the same label. At the start of a line at the top level (item position):

| Label | Inserts |
| --- | --- |
| `main` | `fn main() {` / `$0` / `}` |
| `fn` | `fn ${1:name}(${2}): ${3:Type} {` / `$0` / `}` |
| `struct`, `enum`, `interface` | `struct ${1:Name} {` / `$0` / `}` (and the same for the others) |
| `error enum` | `error enum ${1:Name} {` / `${2:Variant} "${3:message}"` / `}` |
| `impl` | `${1:Type} {` / `$0` / `}` |
| `impl interface` | `${1:Type}: ${2:Interface} {` / `$0` / `}` |
| `test`, `bench` | `test "${1:name}" {` / `$0` / `}` (and the same for `bench`) |
| `fuzz` | `fuzz ${1:name}(${2:data}: [u8]) {` / `$0` / `}` |
| `fuzz` | `fuzz ${1:name}(${2:data}: [u8]) {` / `$0` / `}` |

At the start of a line inside an impl block, an interface, or a `derive`, `fn` is a method:
`fn ${1:name}(${2:self}): ${3:Type} {` / `$0` / `}`. At the start of a line inside a body (statement position): `if`
(`if ${1:condition} {` / `$0` / `}`), `match` (`match ${1:value} {` / `${2:pattern} => ${3:result}` / `}`), `for`
(`for ${1:item} in ${2:items} {` / `$0` / `}`), and `while` (`while ${1:condition} {` / `$0` / `}`). Item snippets are
offered only at item position and statement snippets only inside a body. A client without `snippetSupport` gets no
snippet item, only the plain keywords.

**Call parentheses.** With `snippetSupport`, accepting a function or method inserts the call too: `name($0)` when it
takes parameters, with the `editor.action.triggerParameterHints` command so signature help opens, and `name()$0` when
it takes none (a method's `self` does not count). The bare name is inserted when the next character is already `(`, in
a `use` path, and when the client has no `snippetSupport`. `.` triggers completion.
Tests: `completion_offers_members_names_in_scope_and_imports`, `a_dependency_in_another_root_is_navigable_and_not_checked_again`,
`members_after_a_type_name_a_literal_and_a_primitive`, `member_chains_offer_their_members_on_the_dot_that_starts_them`
(`fixtures/nested`), `builtins_and_the_standard_library_answer_definition_hover_signatures_and_completion`,
`self_is_offered_only_as_the_first_parameter_of_a_method`, `snippets_follow_the_position_and_the_client`,
`completing_a_function_inserts_the_call`, `derives_and_the_derivable_interfaces`,
`self_and_bounded_parameters_complete_in_an_interface_impl`, `completion_offers_only_what_the_position_takes`,
`a_dependency_completes_without_a_use_in_the_file`, `a_use_offers_every_official_package`,
`official_packages_complete_before_any_file_imports_them`, `a_local_module_shadows_an_official_package`
(`fixtures/official-shadow`), `an_official_package_name_completes_and_loads_itself`,
`a_member_of_an_unloaded_official_package_adds_its_use`,
`arrays_and_slices_offer_the_slice_methods_and_a_list_does_not` (`fixtures/arrays`: an array local, an array literal,
a `[i32]` and a `[u8]` parameter, and a `List`), `ranges_tuples_and_iterators_have_members` (`fixtures/receivers`),
`array_and_slice_methods_answer_hover_definition_and_signature_help`; the `builtins.rs`, `nav.rs`, and
`official.rs` unit tests.

**LS13. Signature help and inlay hints.** Inside a call's argument list, signature help shows the callee's signature
with each parameter's label range and the active parameter (counting commas at the call's depth; a method's `self` is
not a parameter); outside a call it is `null`. A built-in without source is answered from its declaration in
`builtins.bd` (LS8): `println(` shows `fn println(fmt: str, ...)` with the parameters `fmt: str` and `...`, and the
variadic `...` stays the active parameter for every argument after `fmt`; a built-in method on a value
(`n.wrapping_add(`) skips its `self` like any other method. Inlay hints show `: Type` after a `let` name without a written type
(not when the type is unknown or an error), and `name:` before a positional argument written in an argument list,
unless the argument already says the name (`a` for `a`, `x.a` for `a`), the argument is named, or the parameter is
`self`; a `mut` marker comes after the hint. LS5's two settings turn each kind off.

The ownership lens is the third kind, off by default (`inlayHints.ownership`): a `move <local>` (or
`move <local> -> <target>`) at the expression that moves it, a `drop <local>` at the closing brace that drops it
(`drop <local> (conditional)` when a runtime flag guards it, and `drop flag <local>` at a conditional move), all from
the same event list as the command line's `--explain-ownership`. The server renders the events of
`Analysis::ownership_events` and stops there; it never re-derives a move, a drop, or a flag.
Tests: `signature_help_marks_the_active_parameter`,
`builtins_and_the_standard_library_answer_definition_hover_signatures_and_completion`,
`inlay_hints_for_types_and_parameter_names_can_be_turned_off`
(`inlay-hints.json`), `the_ownership_lens_shows_moves_and_drops_and_a_hover_lists_them` (`ownership-hints.json`).

**LS14. Semantic tokens.** `semanticTokens/full` and `/range` classify every identifier by what it resolves to, with
the legend in the capabilities: `namespace` (modules), `type`, `struct`, `enum`, `interface`, `enumMember`, `function`,
`method`, `parameter`, `variable`, and `property` (fields), with the modifiers `declaration`, `readonly` (a
constant), `mutable` (a `mut` binding), and `defaultLibrary` (the standard library and the built-ins). A range request answers the
tokens that start inside the range. Keywords, strings, numbers, and comments are left to the editor's grammar.
Tests: `semantic_tokens_cover_the_file_and_a_range` (`semantic-tokens.json`).

**LS15. Formatting.** `textDocument/formatting` and `rangeFormatting` format with the library `burdin fmt` uses
(`spec/fmt.md`), with the package's dependencies grouped as `burdin fmt` groups them, and answer one minimal edit (the
changed middle of the text) or none when the text is already formatted. A file with syntax errors is not formatted
(`null`, with a log message), because the diagnostics already say why. The client's tab size is ignored: Burdin has
one style.
Tests: `formatting_matches_burdin_fmt_and_refuses_a_file_that_does_not_parse`.

## Outline

**LS16. Symbols.** `textDocument/documentSymbol` answers the items of a file nested (types with their fields, methods,
and variants; impl blocks with their methods) when the client supports hierarchical symbols, and a flat list with
container names otherwise. `workspace/symbol` searches every loaded package's own items (not the standard library's)
by a fuzzy match: the query's letters in order, case-insensitive, exact matches first, then prefixes, then the rest.
Tests: `document_symbols_are_nested_or_flat_as_the_client_asks` (`document-symbols.json`),
`workspace_symbols_match_fuzzily_across_files`.

**LS17. Folding, selection, links.** Folding ranges cover blocks of items and statements, runs of `use`
declarations (kind `imports`), and runs of comments (kind `comment`). A selection range grows from the token to each
syntax node around it, to the file. Document links: each module of a `use` path links to its file, a URL in a comment
to itself, and a `[name]` in a doc comment to the declaration it names (`#L<line>`).
Tests: `folding_selection_and_links`.

## Actions and lenses

**LS18. Code actions.** Quick fixes come from the compiler's suggestions and from the server: E0002 for an unknown
name that another module declares imports it (`use helpers.triple`); E0005 and E0113 make the binding `mut`; E0114 adds
the `mut` marker before the argument; E0140 adds the missing methods of an implementation, with the interface's
signatures and `todo()` bodies. An E0002 on the name of an official package that the file does not load, where the name
is followed by `.` (`http.Client` with no `use http`), is fixed by the action titled "import http": the server inserts the `use`
that loads the package, where a completion's edit would put it (LS12). An E0002 on a bare name that an official package
exports, not written after a dot (`Client.new()` with no `use`), is fixed by the action titled "import http.Client", one
per package that exports the name, which inserts `use http.Client`. Each quick fix names its diagnostic. `refactor.rewrite` writes the inferred type of a
`let` under the cursor; `source.organizeImports` applies the linter's removals of unused imports. Every edit is a
workspace edit (`changes` by URI), which the client previews.
Tests: `quick_fixes_for_the_errors_of_a_file`, `organize_imports_and_write_a_type`,
`an_official_package_used_without_a_use_is_imported_by_a_quick_fix`,
`a_bare_official_item_is_imported_by_a_quick_fix`.

*Missing match arms (issue #1040).* An E0006 gets the quick fix "add the missing match arms": one `case => todo(),`
arm per uncovered case, after the last arm, at that arm's indentation. The cases are the witnesses the exhaustiveness
checker computes (`burdin_sema::missing_patterns`, `crates/burdin_sema/src/exhaust.rs`), never a guess: the variants of
an enum in declaration order (`Color.Green`), a payload as wildcards (`Shape.Circle(_)`, `Shape.Rect { w: _, h: _ }`,
`Some(_)`, `Err(_)`), and for a tuple or a nested pattern the uncovered combinations (`(Color.Red, Color.Green)`,
`(Color.Green, _)`, `Some(Color.Blue)`). An open domain (an integer, a string, a slice) has no finite list, so the
action writes one `_ => todo(),` arm. An arm with a guard does not cover its pattern, so the case it names is still
missing. The last arm gets a comma when it is an expression without one, and none is added after a block body; a
`match` with no arms gets them all inside its braces. The type is named as the file would write it (`Color`, not its
module path), so a type that is not in scope in the file can need an import afterwards.
Tests: `the_missing_arms_of_a_plain_enum_follow_the_declaration`, `a_last_arm_without_a_comma_gets_one`,
`a_block_arm_needs_no_comma_before_the_new_arms`, `a_match_with_no_arms_gets_all_of_them`,
`the_missing_arms_of_a_payload_enum_bind_wildcards`, `the_missing_arm_of_an_option_is_none`,
`the_missing_arm_of_a_result_is_err`, `a_tuple_of_enums_lists_the_uncovered_pairs`,
`a_nested_payload_names_the_uncovered_inner_case`, `an_arm_with_a_guard_does_not_cover_its_pattern`,
`an_open_domain_gets_one_wildcard_arm` (`crates/burdin_lsp/tests/match_actions.rs`; each applied edit is sent back
through the checker and must leave no error).

*Expanding a wildcard arm (issue #1041).* A `refactor.rewrite` action, "expand `_` into the remaining cases" (or
"expand `else` ..."), is offered with the cursor on the pattern or the `=>` of a `_` or `else` arm of a `match`. It
replaces the arm with one arm per case the arm is the first to see (the missing cases of the unguarded arms before it,
the same witnesses as above), each carrying a copy of the arm's body text: it only duplicates text, so a body is never
evaluated more than once per run, and the new patterns bind nothing (payloads are `_`), so no name in the body is
shadowed. It is not offered when the cases are not a finite list (an integer, a string, or a tuple of them: a case that
is only wildcards), when the arm has a guard, when no arm before it leaves anything for it, or when the cursor is inside
the arm's body (an inner `match` is expanded on its own arm). The arms after it are kept.
Tests: `a_wildcard_arm_of_a_plain_enum_becomes_the_remaining_variants`, `the_body_is_copied_whole_into_each_arm`,
`a_wildcard_arm_of_a_payload_enum_uses_wildcard_payloads`, `a_wildcard_arm_of_an_option_or_a_bool_is_the_other_case`,
`a_wildcard_in_a_tuple_expands_to_the_pairs_it_covers`, `an_else_arm_expands_like_a_wildcard`,
`an_arm_with_a_guard_is_not_expanded`, `an_integer_or_string_wildcard_is_not_expanded`,
`a_wildcard_with_nothing_before_it_is_not_expanded_beyond_the_variants`,
`the_cursor_inside_the_body_offers_the_arm_it_is_in_not_the_wildcard_around_it` (same file).

*Missing derives (issue #1043).* An E0141 whose message says `` `Point` does not implement `Clone` `` and an E0130
whose message names the field's type (`` `Clone` requires every field to be `Clone`, but this one is `Inner` ``) get
the quick fix "derive `Clone` for `Point`" when the interface is derivable (`Copy`, `Clone`, `Eq`, `Ord`, `Hash`,
`Default`, `Debug`, `Serialize`, `Deserialize`; `spec/derives.md`) and the type is the one struct or enum of that name
that the entry package declares. The edit is in the type's own file (which can be another file than the diagnostic's):
`, Clone` after the last name of the type's `: A, B` list, or `: Clone` after the type's name and type parameters when
it has no list. `Ord` is written with `Eq` and `Copy` with `Clone` in one edit ("derive `Eq, Ord` for `Version`"),
leaving out what the list already has; this is the convention, not a requirement (`Ord` implies `Eq`, and `Copy` does
not need `Clone`; `spec/derives.md`). Nothing is offered for a type of the standard library or a dependency, for a type
that is not a plain struct or enum (a `List<T>`, a tuple, a reference), or for an interface that is not derivable (a
user interface: the fix there is an `impl`, E0140's stub action).
Tests: `a_derive_is_added_to_an_existing_list`, `a_list_is_made_for_a_type_with_none`, `an_enum_gets_the_derive_too`,
`a_generic_struct_keeps_its_type_parameters_before_the_list`, `ord_is_written_with_the_eq_it_comes_with`,
`eq_already_written_is_not_written_again`, `copy_is_written_with_clone`,
`a_field_that_lacks_a_derive_is_fixed_at_its_type`, `a_type_outside_the_package_gets_no_derive`,
`an_interface_that_cannot_be_derived_gets_no_derive`, `a_type_in_another_module_is_fixed_in_its_own_file`
(`crates/burdin_lsp/tests/derive_actions.rs`; each applied edit is sent back through the checker and the error is gone).

*Generating code (issue #1044).* Four more actions write code that the compiler's suggestions do not. Each result
checks (a `todo()` body has the type of anything) and is what `burdin fmt` writes.

- **Missing struct fields.** An E0003 whose message says `` `Point` is missing the field(s) `x`, `z` `` gets the quick fix
  "add the missing fields": each missing field is written `name: todo()` after the last one the literal has, in
  declaration order, on the same line (`Point { y: 2, x: todo(), z: todo() }`) or, in a literal that spans lines, one per
  line at the last field's indentation, after its trailing comma when it has one. A literal with no fields gets them
  all inside its braces. It covers `Path { }`, a variant's `Enum.Variant { }`, and `.{ }`; a literal with a base
  (`..base`) misses nothing. The diagnostic is the existing E0003; no code is added.
- **A missing function.** An E0002 `` unknown function `area` `` on a call gets "create function `area`": a function
  after the item the call is in (after a whole `impl` block when the call is in a method), with one parameter for each
  argument and `todo()` as the body. An argument's type is the checker's: the call is analyzed as the tuple of its
  arguments, so the type of `p.x` or `a + 1` is known even though the callee is not. A plain name is the parameter's
  name (`area(r)` gives `r: Rect`; a named argument keeps its label; a `mut` argument gives a `mut` parameter) and any
  other argument is `arg<n>`. In `let x: T = area(r)` the function returns `T`; elsewhere it returns nothing, so a call
  whose value is used in another way needs its type written by hand.
- **A missing method.** An E0002 `` no method `grow` on type `Rect` `` where the entry package declares `Rect` gets
  "create method `grow` on `Rect`": `fn grow(self, ...)` with the parameters of the arguments (as above) and `todo()`,
  in the type's own `Rect { }` block (before its closing brace, after a blank line), or in a new block after the type
  (`Wrapper<T> { }` for a generic one). The edit is in the file that declares the type. A method on a standard-library
  or dependency type gets no action.
- **Implementing an interface.** A `refactor.rewrite` action "implement `Shape` for `Rect`" is offered with the range on
  the name of a struct or enum of the entry package, for each interface of the entry package that the type does not
  implement (an `impl` block or a `: Shape` derive list) and that has at least one method without a default body. It
  writes `Rect: Shape { }` after the type with those methods (the interface's signatures, `todo()` bodies), and adds
  `use module.Shape` when the interface is in another file the type's file does not import. An interface with type
  parameters, associated types, or requirements is not offered: its implementation needs more than the signatures.

*No standard-library import action.* The prelude makes every public name of every standard-library module visible in
every module (`spec/stdlib.md`), so an E0002 never stands for a standard-library name that only needs a `use`; there is
nothing to import (checked against every public top-level name of `std`), and the official-package imports above are the
only import actions.
Tests: `a_literal_with_missing_fields_gets_them_in_declaration_order`, `a_literal_with_no_fields_gets_all_of_them`,
`a_multi_line_literal_gets_one_field_per_line_with_the_trailing_comma`, `a_target_literal_gets_the_fields_too`,
`a_struct_variant_gets_its_missing_fields`, `a_called_function_is_created_after_its_caller_with_the_argument_types`,
`an_argument_that_is_not_a_name_is_called_argn_and_the_let_type_is_the_return_type`,
`a_call_without_arguments_makes_a_function_without_parameters`,
`a_function_called_from_a_method_goes_after_the_whole_impl_block`,
`a_method_is_added_in_a_new_impl_block_after_the_type`, `a_method_is_added_to_the_impl_block_the_type_has`,
`a_method_of_a_generic_type_keeps_the_type_parameters`, `a_method_on_a_receiver_that_is_a_chain_is_typed_from_its_arguments`,
`a_method_of_a_type_in_another_module_is_written_in_its_own_file`, `a_method_of_a_standard_type_gets_no_action`,
`an_interface_is_implemented_with_the_methods_that_have_no_default`, `a_generic_type_is_implemented_too`,
`an_interface_the_type_already_implements_is_not_offered`, `an_interface_with_only_default_methods_is_not_offered`,
`the_cursor_away_from_the_type_name_offers_nothing`, `an_interface_of_another_module_brings_its_use`
(`crates/burdin_lsp/tests/generate_actions.rs`; each applied edit is sent back through the checker and through
`burdin_fmt`, which must leave it unchanged).

*Refactorings (issue #1042).* Every refactoring of `spec/refactor.md` is in the editor: extract variable and function
(`refactor.extract`, RF9-RF10), inline variable and function (`refactor.inline`, RF11-RF12), change signature
(`refactor.rewrite`, RF13), move item (`refactor.move`, RF14), and structural search and replace (the command below).
The actions come from the same functions as `burdin refactor` (`burdin_refactor`), run on the texts the editor has, so an
action's edit is the one the command line writes for the same position. Each is a workspace edit with one edit per
changed file, from the first difference to the last. The menu is decided from the syntax tree, so it is cheap:

- *Change signature*, on a free function's name: "add a parameter to `f`" (a parameter `extra: i32 = 0`, which the user
  renames and retypes; a call written before the change does not pass it). On a parameter: "remove the parameter `p`",
  "move the parameter `p` before `q`", and "move the parameter `p` after `r`" (its neighbors). A method is not offered
  (RF13).
- *Move item*, on the name of a top-level function, constant, struct, enum, alias, or interface: "move `f` to the module
  `m`", one action per other module of the package.

An action the engine would refuse (a parameter the body uses, a move that would not compile, a package with errors) is
not shown: the refusal's text goes to the log (`window/logMessage`) with the action's title, and the command line
prints the same text (RF7). When the client resolves actions lazily (LS23) the menu is the syntactic one, the refusal
answers `codeAction/resolve` with the text as the error's message, and it is logged too.

Some changes need input a code action cannot ask for, so they are server commands (`workspace/executeCommand`) that
answer a workspace edit, as `burdin.searchReplace` does: `burdin.changeSignature` takes `{ "uri", "position",
"remove": [name], "reorder": [name], "add": [{ "name", "type", "default" }], "rename": [{ "from", "to" }] }` (the
function is the one at the position) and `burdin.moveItem` takes `{ "uri", "position", "to": "<module>" }`.
`burdin.searchReplace` (spelled `burdin.structuralSearchReplace` before; both are accepted) takes `{ "pattern",
"replacement", "root" }` (RF6). A refusal is the command's error, with its text, and is logged too.
Tests (`crates/burdin_lsp/tests/refactor_actions.rs`):
`change_signature_actions_edit_what_the_engine_edits`, `move_item_actions_list_the_modules_and_edit_what_the_engine_edits`,
`commands_take_what_a_code_action_cannot_ask_for`, `search_and_replace_is_a_command_and_a_refusal_is_logged` (each edit
equals what the engine computes on the same package; each refused case has no action and a log line), and the extract
and inline actions in `structural_search_replace_and_refactoring_actions_use_the_refactoring_engine` (`actions.rs`).

**LS19. Lenses and test commands.** In a file with tests, a lens at the top runs them all (`▶ Run N tests in file`), and
above each `test` block a lens runs it and another debugs it; above the entry module's `main`, a lens runs the program
and another debugs it. Each lens's argument carries the exact command line (`burdin test <root> --exact <path>`, where
the path is the test's full path as `burdin test --list` prints it), the root, and the working directory, so an editor
and a terminal run the same thing. `burdin.runTest` and `burdin.runTests` are server commands
(`workspace/executeCommand`): the server runs that `burdin test` invocation with `--format json` on a worker thread,
reports progress, and answers with each test's path, status, duration, and failure reason; the next lenses show the
result (`✓ passed (N ms)` or `✗ failed: reason`). An argument that is not a `burdin test` command line is `-32602`, and a
`burdin` that cannot run is `-32803` with a message to the user. `burdin.run` and `burdin.debug` are client commands:
the editor runs the program in a terminal, and starts a debug session with the lens's DAP-style `launch`
configuration — `type: "burdin"`, the package root as `target`, and `test` for a test — which `burdin dap` answers
(`spec/debug.md`, DBG11-DBG12). The editor registers the `burdin` debug type against `burdin dap`, so no adapter has to
be installed or configured.
Tests: `lenses_run_and_debug_tests_and_main_with_exact_command_lines` (`code-lenses.json`),
`the_test_command_reports_a_missing_compiler_and_rejects_other_commands`; `tests/check_lsp.py` runs a test through a
lens with the real driver and checks the result lens.

## Protocol coverage

**LS22. Type definition and renamed files** (issue #1045). `textDocument/typeDefinition` goes to the declaration of the
type of what is under the cursor: a type's own name to itself, a local, parameter, or field to the type it holds, a
function to its return type, and any other expression to the type the analysis gives it. An array or slice goes to its
element type, and a generic type to the generic (`List<Point>` to `List`); a type the package does not declare (a
primitive) answers `null`.
`workspace/willRenameFiles` (and `workspace/didRenameFiles`, which reloads the packages that held or now hold the
files) answers the edits that keep the `use` paths of the package pointing at a module whose file the editor renames or
moves under `src`, or whose folder it renames: every `use` that begins with the old module path begins with the new one.
A `use shapes` that renamed `shapes.bd` to `figures.bd` becomes `use figures as shapes`, so the code that writes
`shapes.Square` still compiles; an item path (`use shapes.Square`) and a brace list take the new module path.
A file moved out of `src`, or outside every package, changes nothing. (Move item, RF14, needs no such edit: the module
re-exports the item.)
Tests (`crates/burdin_lsp/tests/protocol_features.rs`): `type_definition_goes_to_the_declaration_of_the_type`,
`renaming_a_module_file_rewrites_the_use_paths` (a file, a folder, a file moved out of `src`, and the package checking
after `didRenameFiles`).

**LS23. Resolving lazily** (issue #1045). Completion and code actions advertise `resolveProvider`. A client that
lists `documentation` in `completionItem.resolveSupport.properties` gets completion items without their documentation
(the doc comments of the package, the standard library, and the official packages), and `completionItem/resolve` of an
item of the latest completion adds it; an item the server does not know, or that has it, comes back unchanged. A client
that lists `edit` in `codeAction.resolveSupport.properties` gets the refactoring actions of LS18 without their edit but
with `data`, and `codeAction/resolve` computes the edit (the engine runs when the user chooses, not on every cursor
move). The quick fixes of diagnostics keep their edit: they cost nothing to compute. A client with no resolve support
gets everything up front, as before.
Tests: `completion_documentation_is_resolved_when_an_item_is_chosen` (`protocol_features.rs`),
`refactorings_are_resolved_when_chosen` (`refactor_actions.rs`).

**LS24. Formatting as the user types** (issue #1045). Decision: wanted, because `burdin fmt` already formats a range.
`textDocument/onTypeFormatting` triggers on `}` and on a newline: typing `}` formats the line it is on, and a newline
the line before the cursor, through the range formatting of LS15 (`burdin_fmt::format_range`). A file that does not parse
yet is left alone, as in LS15, so typing a half-written statement never moves text. `formatting.enabled` (LS5) turns it
off.
Test: `typing_a_brace_or_a_newline_formats_the_line` (`protocol_features.rs`).

**LS25. The rest of LSP 3.17** (issue #1045). The server was compared with the whole 3.17 method list. Not implemented,
on purpose:

| Feature | Decision |
| --- | --- |
| `textDocument/diagnostic`, `workspace/diagnostic` (pull diagnostics) | The server publishes (LS7), with versions and a debounce, and a client that also pulls would show each error twice; the analysis is per package, so a pull would cost the same. Revisit when a supported editor needs it. |
| `textDocument/linkedEditingRange` | Burdin has no paired tags or delimiters that must be edited together; renaming is `rename` (LS9). |
| `textDocument/documentColor`, `colorPresentation`, `moniker`, `inlineValue`, `inlineCompletion` | Nothing in the language corresponds; inline values are `burdin dap`'s (`spec/debug.md`). |
| `notebookDocument/*`, `workspace/textDocumentContent` | There are no notebooks. |
| `workspace/willCreateFiles`, `willDeleteFiles` and their `did` notifications | A new or deleted file needs no edit to another; a deleted module's `use` is an E0002 the quick fixes of LS18 already explain. The watched-files notification below reloads the package. |
| `codeLens/resolve`, `documentLink/resolve`, `inlayHint/resolve`, `workspaceSymbol/resolve` | Their answers are complete and cheap; the providers say `resolveProvider: false`. |
| `window/showDocument`, `$/setTrace`, `$/logTrace` | Not needed; messages go through `window/logMessage` and `window/showMessage`. |

Already implemented and recorded here: `workspace/didChangeWatchedFiles` (the server registers `**/*.bd` and
`**/burdin.toml` watchers and reloads what changed, LS4), `workspace/didChangeWorkspaceFolders`, `$/cancelRequest`,
`$/progress`, call and type hierarchies (LS10), selection ranges, folding ranges, semantic tokens (LS14), and the inlay
hint refresh. `tests/check_lsp.py` checks that the driver's server announces the capabilities above.

## Editors

**LS20. Editor packaging.** `editors/vscode` is a VS Code extension: it starts `burdin lsp` (the `burdin.path` setting,
else `burdin` on `PATH`), passes the `burdin` settings, registers the `burdin` language for `.bd` files with a TextMate
grammar and a language configuration (comments, brackets, indentation), implements the client commands of LS19, and
registers the `burdin` debug type against `burdin dap` (`spec/debug.md`, DBG12).
`editors/zed` is a Zed extension: it registers the `Burdin` language for `.bd` files, highlighted by the tree-sitter
grammar in `editors/tree-sitter-burdin`, and starts `burdin lsp` (`lsp.burdin.binary.path`, else `burdin` on `PATH`)
with the `lsp.burdin.settings` object as the `burdin` settings. `editors/README.md` gives the Neovim and Helix
configuration. `tests/check_lsp.py` checks what can be checked without an editor: the extension's manifest names the
server command, the settings it declares are the ones LS5 lists, the grammar's keywords are exactly the lexer's, the Zed
extension starts `burdin lsp` for a `.bd` language and pins its API crate, and the files parse. Installing and using the
extension in a real editor is the manual checklist below, recorded in the closing comment of each editor change.

Manual checklist (VS Code; repeat the relevant items in Zed, Neovim, and Helix):

1. Open a package; the status shows the server running, and a file with an error shows it underlined.
2. Fix the error; the underline goes away within the debounce.
3. Go to definition, find references, and rename a function used in two files.
4. Hover a call and a local; complete after `x.` and in a `use` path.
5. Format a file with a formatting mistake (and on save, when the editor formats on save).
6. Click `▶ Run test` above a test; the result lens appears.
7. Turn off `burdin.inlayHints.types`; the type hints go away without a restart.

## Tests and commands

```
cargo test -p burdin_lsp          the unit tests and the JSON-RPC integration tests (no LLVM)
python3 tests/check_lsp.py        `burdin lsp` from the driver, a test run through a lens, and the editor files
```
