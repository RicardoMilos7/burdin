# The regex fixtures

`cases.tsv` holds the golden rows of the `regex` package (spec/regex.md). `runner/` is the program that runs them. The
check that builds the runner against `packages/regex`, compares the goldens, and runs the Python `re` differential is
`tests/check_regex.py`.

## The rows

One row per case, five tab-separated fields:

```
op<TAB>pattern<TAB>text<TAB>arg<TAB>expected
```

Every field is percent-encoded: `%25` is `%`, `%09` a tab, `%0A` a line feed, and `%0D` a carriage return. Nothing else
is encoded, so a space is a space in a field. A line that starts with `#` is a comment. `expected` is the line the runner
prints for the row, encoded the same way, so the checker decodes it before the comparison.

## The ops

| Op | Pattern | Text | Arg | Output |
| --- | --- | --- | --- | --- |
| `as_str` | the pattern | | | the pattern |
| `groups` | the pattern | | | the number of capturing groups |
| `index` | the pattern | | a group name | its number, or `none` |
| `match` | the pattern | the text | | `true` or `false` |
| `find` | the pattern | the text | | the first match, or `none` |
| `find_all` | the pattern | the text | | `[` each match `]` |
| `captures` | the pattern | the text | | `[` each group of the first match `]`, or `none` |
| `captures_all` | the pattern | the text | | `[` the groups of each match `]` |
| `name` | the pattern | the text | a group name | the named group of the first match, or `none` |
| `expand` | the pattern | the text | a template | the template expanded from the first match, or `none` |
| `replace` | the pattern | the text | a template | the text with its first match replaced |
| `replace_all` | the pattern | the text | a template | the text with every match replaced |
| `replacen` | the pattern | the text | `limit;template` | the text with its first `limit` matches replaced |
| `split` | the pattern | the text | | `[` the pieces `]` |
| `splitn` | the pattern | the text | a limit | `[` at most that many pieces `]` |
| `escape` | | the text | | the escaped text (a pattern that matches it exactly) |

A failed `Regex.new` prints `error: ` and the `RegexError` message, as the golden rows of the error messages show. An
unknown op prints `unknown op` and its name.

## The output format

- A span is `start:end:text`, with byte offsets. A group that did not take part is `-` in a group list.
- A list is `[` the items separated by one space `]`, and `[]` is the empty list.
- A text is printed as written, except that `%` `<` space, tab, line feed, and carriage return are percent-encoded
  (`%25 %3C %20 %09 %0A %0D`), and the empty text is `<>`.

The differential check in `tests/check_regex.py` generates rows over the same ops and compares them with Python's `re`,
so the goldens and the differential agree on everything both engines define the same way.
