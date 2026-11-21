# Machine-readable diagnostics

The JSON and SARIF diagnostic formats and the links to the error reference (stage 3; issues #236, #642, #643, and #644).
This file is the exact behavior, with a rule ID for each point and the tests that cover it. The user-facing description,
with the field tables, is [`docs/diagnostics.md`](../docs/diagnostics.md); the human format and the options that shape it
(`--color`, `--width`, `--error-limit`, ordering) are specified by issues #433-#437 and described in the driver contract
of `README.md`.

The data is the compiler's `Diagnostic` (`crates/burdin_diag`): a code, a severity, a primary span, a message, labels,
notes (with an optional span), help lines, children, and suggestions (a replacement of one span). Every format is
rendered from that one value, so none can say something another cannot. `json.rs` renders the JSON objects, `sarif.rs`
the SARIF log, and the driver (`crates/burdin_driver/src/main.rs`) chooses the format and prints what is not a
diagnostic (the summary objects).

Every behavior below is covered by `tests/check_diagnostics.py` (black-box, with the fixtures and goldens in
`tests/diagnostics/`), `tests/check_error_docs.py`, or the unit tests of `burdin_diag` and the driver.

## Formats

**DG1. Four formats, one order.** `--diagnostic-format` is `human` (the default), `short`, `json`, or `sarif`; any other
value is a usage error (status 2). The human and short formats are exactly what they were before the other two existed.
Whatever the format, the diagnostics are sorted by file, byte offset, code, message, and insertion order, and
`--error-limit` keeps the first ones of that order, so the output is the same on every run.
Tests: `check_diagnostics.py` (`unknown diagnostic format`, `determinism`, `ordering`), `burdin_diag` unit tests
`equal_spans_are_ordered_by_code_message_and_insertion` and `error_limit_applies_after_sorting_and_reports_the_total`.

## The JSON format

**DG2. Locations.** A location has `file`, the 1-based `line` and `column` and `end_line` and `end_column`, the 0-based
`byte_start` and exclusive `byte_end`, and the column of both ends in UTF-8 bytes (`utf8_column`, `end_utf8_column`) and in
UTF-16 code units (`utf16_column`, `end_utf16_column`). `column` and `end_column` count Unicode scalar values, which is
what the human format prints. All five columns are measured from the start of the line the position is on.
Tests: `burdin_diag` `json_columns_count_characters_bytes_and_utf16_units`, `check_diagnostics.py` (`multi.bd json`: the
string `"añ𝄞"`; every location of every conformance diagnostic satisfies `utf8_column >= column` and
`utf16_column >= column`).

**DG3. The diagnostic object.** `--diagnostic-format json` prints one object per diagnostic, on one line, with these
fields in this order: `type` (`"diagnostic"`), `version`, `severity` (`error` or `warning`), `code`, `message`, `url`,
`span`, `labels`, `notes`, `help`, `related`, `children`, `suggestions`, and `rendered`. Every field is always present;
an empty list is `[]`. `labels` are `{span, message, primary}` (`primary` when the span is the diagnostic's own), `notes`
are `{message, span}` with a `null` span when the note points nowhere, `help` is a list of strings, and `children` are
diagnostics with the same fields except `type`, `version`, and `rendered`. `rendered` is the human text of the
diagnostic with no color, no link, and no wrapping, so it does not depend on the terminal.
Tests: `burdin_diag` `json_is_version_1_with_every_documented_field_in_order`,
`json_carries_labels_help_children_and_related_information`, `json_rendered_is_the_plain_unwrapped_human_text`,
`json_golden_for_a_diagnostic_with_every_field` (`tests/diagnostics/composite.golden.json`), `check_diagnostics.py`
(`rendered` starts with the human first line for every conformance diagnostic).

**DG4. Related information.** `related` lists the notes that have a span, as `{span, message}`: the shape of the Language
Server Protocol's `relatedInformation`. A note without a span is only in `notes`.
Tests: `json_represents_spanless_notes_deletions_and_machine_fixes`, `check_diagnostics.py` (`multi.bd json`: the move
error has one note and one related entry).

**DG5. Suggestions.** A suggestion is `{message, applicability, edits}`. `applicability` is `machine` when the edit is one
`burdin lint --fix` may apply on its own, and `manual` otherwise. `edits` is a non-empty list of `{span, replacement}`;
the edits of one suggestion apply together, an empty span inserts, and an empty replacement deletes. A suggestion has one
edit today.
Tests: `check_diagnostics.py` (`multi.bd json`: the `mut ` insertion; the schema requires one edit or more),
`json_represents_spanless_notes_deletions_and_machine_fixes`.

**DG6. Versions and summaries.** Every object has `"version": 1`, the diagnostics and the summary objects. The summary
of a failed `build`, `run`, or `test` is `{"type":"summary","version":1,"errors":N,"shown":M,"limit":L|null}`, and
`burdin lint` ends with `{"type":"lint-summary","version":1,"warnings":W,"errors":E}`. `burdin check` ends with
`{"type":"check-summary","version":1,"errors":E,"warnings":W,"shown":M,"limit":L|null,"files":F,"elapsed_ms":T}` on every
run that reaches the checks, clean or not (`spec/check.md`, CK12). Adding a field keeps the version;
removing or renaming one, or changing what it means or the unit it counts in, raises it, and the page documents both
versions. A reader ignores fields it does not know.
Tests: `check_diagnostics.py` (`json format`, `lint json`), the schema, and `tests/check_check.py` (the check summary,
CK12: the clean, warning-only, failing, package, and `--error-limit` runs).

**DG7. The schema is the contract.** [`docs/diagnostic.schema.json`](../docs/diagnostic.schema.json) describes every object
the format prints and rejects an object with a field it does not list, so a change to the output cannot pass unnoticed:
it needs a schema change, a golden change, and a page change. `check_diagnostics.py` validates against it every line the
driver prints, as JSON, for every `compile-fail` and `test-compile-fail` conformance test (and requires at least one
diagnostic from each), the golden files, and the lint output; and it checks that ten mutations of a valid object are
rejected, so the schema cannot be satisfied by accident.
Tests: `check_diagnostics.py` (`Validated N JSON diagnostics from M compile-fail tests`, `schema:` mutations), and the
goldens `tests/diagnostics/multi.golden.json` (a moved value with a note that has a span, a missing `mut` with a machine
fix, and a non-ASCII string) and `composite.golden.json`.

## The SARIF format

**DG8. One log.** `--diagnostic-format sarif` prints one SARIF 2.1.0 log (`$schema`, `version`, and one run) on stderr, on
one line. The run has the tool (`name` `burdin`, `version`, `informationUri`), `columnKind` `unicodeCodePoints`, one
invocation with `executionSuccessful` (false when a diagnostic is an error), and the results. Nothing else is written
to the stream: no summary line, no hint, no color. A non-diagnostic error (a usage error, an unreadable file) is still a
plain `error:` line.
Tests: `check_diagnostics.py` (`sarif multi.bd`, `sarif two_errors.bd`), `burdin_diag`
`sarif_log_has_one_run_with_rules_results_related_locations_and_fixes`,
`tests/diagnostics/sarif-subset.schema.json`.

**DG9. Rules.** The run's `rules` are every error code of the registry, in the order of the error reference, and every
lint, each with `id`, `name` (the title of the entry, or the lint's name), `shortDescription`, `helpUri` (DG12), and a
`help` that names the entry in `docs/error-codes.md`. A diagnostic whose code is not listed (`E9000`) gets a rule too, so
every result has a `ruleId` and a `ruleIndex` that point at one. The driver builds the list from the embedded error
reference and the linter's own table, so it cannot disagree with them.
Tests: `check_diagnostics.py` (`sarif rules`: every registry code and lint is a rule, no rule repeats, `helpUri` for each,
`ruleIndex` points at the rule with the result's id), driver unit test `the_sarif_tool_lists_every_error_code_and_lint_with_its_link`.

**DG10. Results.** A result has the `ruleId`, `ruleIndex`, `level` (`error` or `warning`), `message.text`, and the
primary span as `locations[0]`, a `physicalLocation` with the `artifactLocation` and a `region` (`startLine`,
`startColumn`, `endLine`, `endColumn`, `byteOffset`, `byteLength`). The labels, the notes with a span, and the children
are `relatedLocations`, numbered, each with its message. Each suggestion is a `fix`: its description and a replacement of
the region by the new text (an insertion has `byteLength` 0). `properties` carries what has no place in the result: the
notes without a span, the help lines, and `fixIsMachineApplicable`. A path is a URI: a relative path stays relative with
`uriBaseId` `%SRCROOT%`, an absolute one becomes a `file:` URI, and `\` becomes `/` and unsafe characters are
percent-encoded.
Tests: `check_diagnostics.py` (`sarif multi.bd`: the related location, the `mut ` fix, the region of the non-ASCII
string; the golden `tests/diagnostics/multi.golden.sarif`), `burdin_diag` `relative_paths_stay_relative_and_absolute_paths_become_file_uris`.

**DG11. When a log is written.** `burdin build`, `burdin check`, and `burdin lint` always write the log, with an empty
`results` when there is nothing to report. `burdin run` and `burdin test` write it only when the compilation reported something (a warning or an
error), so a program's own stderr is not preceded by an empty log. With `--error-limit`, the log has the first diagnostics
and `properties.diagnosticsNotShown` on the run says how many were left out. `burdin lint` writes the log for a lint
at any level, and its `fixes` are the machine-applicable ones. `burdin fmt` does not support the format (status 2),
because it reports file by file and a SARIF log is one document.
Tests: `check_diagnostics.py` (`sarif clean build`, `sarif warning-only build`, `sarif clean run`, `sarif lint, clean`,
`sarif lint, a warning`, `sarif lint, a denied lint`, `sarif --error-limit`, `fmt refuses sarif`), `check_diagnostics.py`
and `check_check.py` (`a clean check writes an empty SARIF log`, `a failed check writes one SARIF log`), `burdin_diag`
`sarif_with_no_diagnostics_is_a_valid_empty_log_and_records_a_limit`.

## Links to the error reference

**DG12. The URL scheme.** A code's page in the hosted reference is `<root>/errors/<code>` for an `E` code and
`<root>/lints/<code>` for a lint, where the root is `https://ricardomilos7.github.io/burdin`. The address ends in the
code and the scheme does not change between versions; changing the root or the scheme is a deliberate edit of
`URL_BASE` in `burdin_diag`, `tests/check_error_docs.py`, and `tests/check_diagnostics.py` together. `E9000` has no page,
and its `url` is `null`. The page serves the entry of `docs/error-codes.md` (or `docs/lints.md`) for that code; publishing
it is a deployment concern, not part of the compiler. `burdin explain <code>` prints `Online: <url>` under the entry.
Tests: `check_error_docs.py` (the scheme is documented, `burdin explain` prints the link for every code, every JSON
diagnostic of every example has `url` equal to `<root>/errors/<code>`), `check_diagnostics.py` (`url` of every
conformance diagnostic, `helpUri` of every rule), `burdin_diag` `codes_link_to_the_reference_except_those_it_does_not_explain`.

**DG13. Hyperlinks in human output.** In the human format, when color is on and the terminal supports hyperlinks, the
code of a diagnostic is an OSC 8 hyperlink to its page: `ESC ] 8 ; ; URL ESC \ CODE ESC ] 8 ; ; ESC \`, around the code
and nothing else, so the visible text does not change. The driver turns the link on when `FORCE_HYPERLINK` is set and
not `0`; off when it is `0`; otherwise when the environment names a terminal known to support it (Windows Terminal,
kitty, Konsole, VTE 0.50 or later, iTerm2, WezTerm, VS Code, Ghostty, Hyper, Tabby, rio, Alacritty, foot) and it is not
`dumb`, `tmux`, or `screen`. Without color (or without hyperlinks) there is no escape sequence at all.
Tests: `check_diagnostics.py` (`hyperlinks`: two links to E0003 with `FORCE_HYPERLINK=1`, each closed, removing the
sequences gives the `--no-color` text, none with `FORCE_HYPERLINK=0`, none without color, none in json or short),
`burdin_diag` `human_links_the_code_only_with_color_and_hyperlinks_and_keeps_the_visible_text`, driver unit test
`hyperlinks_follow_force_hyperlink_then_the_terminal_that_is_running`.

**DG14. The short format has no link.** `short` is exactly one line of the form the test scripts parse, so it carries no
URL. A reader that wants the link takes the code from the line and builds the address (DG12), or uses `json`.
Tests: `check_diagnostics.py` (`short format: contains a URL`).
