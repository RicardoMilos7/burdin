# Diagnostics for tools

`burdin build`, `run`, `test`, and `lint` print their diagnostics to stderr in the format `--diagnostic-format` selects.
This page is the reference for the two formats that tools read: `json`, for editors and scripts, and `sarif`, for CI and
code-scanning services. The behavior is specified in [`spec/diagnostics.md`](../spec/diagnostics.md) and checked by
`tests/check_diagnostics.py` and `tests/check_error_docs.py`.

| Format | For | Output |
| --- | --- | --- |
| `human` (default) | people | colored when asked, wrapped, with notes and help lines; the first line is always `path:LINE:COL: error[E0000]: message` |
| `short` | the test scripts | exactly one plain line per diagnostic; no notes, no link, no color |
| `json` | editors, scripts | one version 1 JSON object per line, then a summary object |
| `sarif` | CI, code scanning | one SARIF 2.1.0 log for the whole run |

The human format is the default and is unchanged by the other formats existing. Diagnostics are sorted by file and byte
offset before any format prints them, so the output is the same on every run and every machine, and `--error-limit`
keeps the first diagnostics of that order.

## The JSON format, version 1

`--diagnostic-format json` prints one JSON object per line. Each object has a `type`:

- `"diagnostic"`: one diagnostic.
- `"summary"`: the last line of a failed `build`, `run`, or `test`: `{"type":"summary","version":1,"errors":2,"shown":2,"limit":null}`.
  `errors` counts every diagnostic, `shown` the ones printed, and `limit` is the `--error-limit` or `null`.
- `"lint-summary"`: the last line of `burdin lint`: `{"type":"lint-summary","version":1,"warnings":1,"errors":0}`.

Output of the programs a build runs (a build script, for example) is not JSON; a reader ignores a line that does not
start with `{`.

The schema is [`diagnostic.schema.json`](diagnostic.schema.json) (JSON Schema 2020-12). It is closed, so any change to
the output changes the schema, and `tests/check_diagnostics.py` validates against it every line the driver prints for
every failing conformance test.

### A diagnostic

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"diagnostic"` | |
| `version` | `1` | the version of the format |
| `severity` | `"error"` or `"warning"` | a warning does not stop the build |
| `code` | string | `E0000` (the registry in `tests/README.md`), `L0000` (a lint), or `E9000` ("not implemented yet") |
| `message` | string | the primary message |
| `url` | string or `null` | the code's entry in the error reference (below); `null` for `E9000` |
| `span` | location | the primary span |
| `labels` | array | `{span, message, primary}`: text attached to a span; `primary` is true for the diagnostic's own span |
| `notes` | array | `{message, span}`: a note; `span` is `null` when it points nowhere |
| `help` | array of strings | advice that is not an edit |
| `related` | array | `{span, message}`: the notes that have a span, in the shape of the Language Server Protocol's `relatedInformation` |
| `children` | array | diagnostics that belong to this one: the same fields without `type`, `version`, and `rendered` |
| `suggestions` | array | `{message, applicability, edits}`: a fix; `applicability` is `"machine"` (safe to apply without review, what `burdin lint --fix` applies) or `"manual"`; `edits` is a list of `{span, replacement}` |
| `rendered` | string | the human text of this diagnostic: no color, no link, no wrapping |

A suggestion's edits apply together. An edit whose span is empty (`byte_start == byte_end`) inserts its replacement;
an empty `replacement` deletes the span. Today a suggestion has one edit; the field is a list so that a fix that
touches two places does not need a new format.

### A location

```json
{"file":"multi.bd","line":21,"column":18,"end_line":21,"end_column":23,"byte_start":262,"byte_end":271,
 "utf8_column":18,"utf16_column":18,"end_utf8_column":27,"end_utf16_column":24}
```

Lines and columns start at 1; byte offsets start at 0 and `byte_end` is exclusive. The same position is given in the
units each consumer indexes text by, so none has to read the file again:

| Fields | Unit of the column |
| --- | --- |
| `column`, `end_column` | Unicode scalar values (what the human format prints) |
| `utf8_column`, `end_utf8_column` | UTF-8 bytes |
| `utf16_column`, `end_utf16_column` | UTF-16 code units (the Language Server Protocol's default position encoding) |

In the example, the string `"añ𝄞"` is five characters, nine bytes, and six UTF-16 code units wide, and the three
`end` columns differ accordingly. `file` is the path as the diagnostic prints it.

### Versioning

`"version": 1` is in every object. Adding a field, or a value to a field with an open set of values, keeps the version.
Removing or renaming a field, or changing what a field means or the units it counts in, raises it, and the old version
is then documented here beside the new one. A reader should ignore a field it does not know.

### Reading it

```
$ burdin build bad.bd --diagnostic-format json 2>&1 | python3 -c "
import json, sys
for line in sys.stdin:
    if line.startswith('{'):
        d = json.loads(line)
        if d['type'] == 'diagnostic':
            print(d['span']['file'], d['span']['line'], d['code'], d['url'])
"
```

## Links to the error reference

Every code has a page in the hosted error reference, at the root `https://ricardomilos7.github.io/burdin`, then
`/errors/` and the code: `https://ricardomilos7.github.io/burdin/errors/E0003`. A lint's page is under `/lints/`:
`https://ricardomilos7.github.io/burdin/lints/L0007`. The address ends in the code, and the scheme does not change
between versions. The pages are the entries of [`error-codes.md`](error-codes.md) and [`lints.md`](lints.md); `E9000`
has none.

The link is in every format that has room for one:

- `json`: the `url` of a diagnostic;
- `sarif`: the `helpUri` of the rule, and a `help` text that names the entry;
- `human`: when color is on and the terminal supports hyperlinks, the code in `error[E0003]` is an
  [OSC 8](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda) hyperlink to it. The visible text is the
  same with or without the link. `FORCE_HYPERLINK=1` turns it on and `FORCE_HYPERLINK=0` turns it off; otherwise the
  driver turns it on for the terminals known to support it (Windows Terminal, iTerm2, WezTerm, VS Code, Ghostty, kitty,
  Alacritty, foot, VTE-based terminals 0.50 and later, Konsole) and not inside `tmux` or `screen` or on a `dumb`
  terminal. Without color there is no hyperlink; `burdin explain <code>` is how a plain terminal reads the entry, and it
  prints the address too;
- `short`: none. It is exactly one line of the form the test scripts parse.

## SARIF

`--diagnostic-format sarif` prints one [SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html)
log on stderr. `burdin build` and `burdin lint` write the log even when they found nothing (an empty `results` and
`"executionSuccessful": true`), so a CI job always has a file to upload; `run` and `test` write one only when the
compilation reported something. Nothing else is written to the stream after the log: no summary line and no hint.
`burdin fmt` does not support the format, because it reports file by file.

```
burdin build . --diagnostic-format sarif 2> burdin.sarif
burdin lint . --diagnostic-format sarif 2> burdin-lint.sarif
```

The log has one run:

| SARIF | Content |
| --- | --- |
| `tool.driver` | `name` `burdin`, the compiler's `version`, `informationUri` |
| `tool.driver.rules` | every error code of the registry and every lint: `id`, `name` (the entry's title, or the lint's name), `shortDescription`, `helpUri` (the link above), and `help` naming the entry in `docs/error-codes.md` |
| `columnKind` | `unicodeCodePoints`: columns count the same characters as `column` in the JSON format |
| `results[]` | one result for each diagnostic, in the sorted order: `ruleId`, `ruleIndex`, `level` (`error` or `warning`), `message.text` |
| `locations` | the primary span as a `physicalLocation`: `artifactLocation.uri` and a `region` with `startLine`, `startColumn`, `endLine`, `endColumn`, `byteOffset`, and `byteLength` |
| `relatedLocations` | the labels, the notes that have a span, and the children, with the message of each |
| `fixes` | one fix for each suggestion: its description and a replacement of the file's region with the new text |
| `properties` | the notes without a span (`notes`), `help`, and `fixIsMachineApplicable` (one flag for each fix) |
| `invocations` | `executionSuccessful`: false when there is an error |
| run `properties.diagnosticsNotShown` | how many diagnostics `--error-limit` left out, when it left some out |

`artifactLocation.uri` is the path as the diagnostic prints it, with `/` as the separator and percent-encoding. A
relative path stays relative with `"uriBaseId": "%SRCROOT%"`, which is how code-scanning services resolve it against the
repository; an absolute path is a `file:` URI. Run the command from the repository root, with a relative target, for
alerts that land on the right file.

A code that is not in the registry (a lint, or `E9000`) still gets a rule, so every result has one.

[`tests/diagnostics/sarif-subset.schema.json`](../tests/diagnostics/sarif-subset.schema.json) describes exactly what the
compiler writes, as a restriction of the official schema, and `tests/check_diagnostics.py` validates every log against
it and keeps a golden copy of the results for a fixture with a moved value, a missing `mut`, and non-ASCII text.

## Checked by

- `tests/check_diagnostics.py`: the schema against every failing conformance test, the goldens
  (`tests/diagnostics/multi.golden.json`, `multi.golden.sarif`, `composite.golden.json`), the columns in all three
  units, SARIF for `build` and `lint`, and the hyperlinks (`BURDIN_UPDATE_GOLDEN=1` rewrites the goldens).
- `tests/check_error_docs.py`: the link scheme, the `url` of every diagnostic of every example, and `burdin explain`.
- The unit tests of `crates/burdin_diag` (`json.rs`, `sarif.rs`, and the rendering tests in `lib.rs`).
