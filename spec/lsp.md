# The language server

`burdin lsp` is the Burdin language server (stage 3; issue #242 and its sub-issues #309-#313, #365-#376, #470, and
#662; completion, snippets, and the built-ins in #819-#824). It speaks the Language Server Protocol 3.17 over standard input and output and answers every request from the
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
field in a struct literal the struct's field, and a shorthand field (`Rect { w, h }`) the local it reads. A name of the
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
  (`crypto.hash`). The package itself, a local module of that name, or a path or git dependency of it shadows the
  official package (`spec/packages.md`, PKG15).
- **In a `use` path**, the modules and the public items below the path written so far, from the package and its
  dependencies (a dependency's `priv` items are left out).
- **At a function declaration's parameter**: directly after `fn name(` inside an impl block, an interface, or a
  `derive`, or in an extension method (`fn Type.name(`), the receiver forms `self`, `mut self`, and `own self`; at any
  other parameter, and in a free function, only the conventions `mut`, `own`, and `comptime`. A `self` parameter is
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
  dependency offers nothing until the manifest declares it (`spec/packages.md`, PKG15).

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
(`fixtures/official-shadow`); the `builtins.rs` unit test and the `official.rs` unit test.

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
signatures and `todo()` bodies. Each quick fix names its diagnostic. `refactor.rewrite` writes the inferred type of a
`let` under the cursor; `source.organizeImports` applies the linter's removals of unused imports. Every edit is a
workspace edit (`changes` by URI), which the client previews.
Tests: `quick_fixes_for_the_errors_of_a_file`, `organize_imports_and_write_a_type`.

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
