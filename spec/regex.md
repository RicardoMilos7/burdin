# Regex

The regular-expression package (stage 3, issue #264; `packages/regex`). It is an ordinary official package
(`spec/packages.md`, PKG15-PKG18) written in Burdin, so this page is the exact behavior of the patterns it accepts, the
matches it reports, and the functions a program calls. The rule IDs are RX1-RX14. `packages/regex/README.md` repeats the
decisions for users, and `tests/regex/` and `tests/check_regex.py` witness each rule.

Failures are values: a `RegexError` in a `Result` from `Regex.new`, never a panic and never a compiler diagnostic, so no
registry code is involved (`tests/README.md`). A pattern that the engine cannot run is rejected when it is compiled, and a
search never fails.

## RX1. A linear-time engine (decision)

The matcher is a Pike VM: a Thompson NFA simulated over the text with one thread per live instruction and the capture
slots copied with each thread. Its running time is proportional to the pattern size times the text length, whatever
the pattern. It never backtracks, so `(a+)+$` on a long run of `a` followed by `!` finishes in linear time. The price
is that the features that need backtracking are rejected rather than emulated: lookahead, lookbehind, backreferences,
atomic groups, and possessive quantifiers (RX3). The engine is written in Burdin and needs no C library, so it works
wherever the compiler does (the same reasoning as CR1 in `spec/crypto.md`). Witnessed by `tests/regex/cases.tsv`
(the `RX1 linear time` section), which the check runs under the time budget of the whole file, and by the rejected rows.

## RX2. Syntax

A pattern is a sequence of atoms, optionally repeated, separated by `|`. The accepted syntax is the common subset of
Perl, Python, and RE2:

- **Literals.** Every character except the metacharacters `\ . ^ $ | ( ) [ ] * + ? {`. A `}` alone is a literal.
- **Metacharacters.** `.` matches one character except a line feed (RX14); `^` and `$` are the text boundaries, or the
  line boundaries under `m` (RX6).
- **Escapes.** `\t`, `\n`, `\r`, `\f`, `\v`, `\a`; `\xHH` (two hex digits) and `\x{H...}` (up to U+10FFFF, not a surrogate);
  `\d \D \w \W \s \S` (RX5); `\p{Name}`, `\pN`, `\P{Name}`, `\PN` (RX5); `\A` (text start), `\z` (text end), `\b` and
  `\B` (word boundary and its negation). A backslash before any other punctuation or non-ASCII character is that
  character. An unknown ASCII letter after a backslash is an error.
- **Groups.** `(re)` is a capture group, numbered from 1 by the position of its `(`. `(?:re)` groups without capturing.
  `(?P<name>re)` and `(?<name>re)` capture under a name: a name is a run of ASCII letters, digits, and `_` that does not
  start with a digit, and no two groups share a name. `(?flags)` changes the flags until the end of the enclosing group, and
  `(?flags:re)` applies them to `re` only (RX6).
- **Alternation.** `a|b|` matches `a`, `b`, or the empty text; an empty branch is allowed.
- **Repetition.** `*`, `+`, `?`, `{n}`, `{n,}`, and `{n,m}`, each greedy, and each followed by `?` to be lazy (RX4). A
  repetition applies to the atom before it. A second repetition of the same atom is an error, and so is a repetition of
  an assertion or of an empty group (`nothing to repeat`). `{,m}` and a `{` that does not start a valid count are errors.
- **Verbose mode.** Under `x`, white space in the pattern outside a class is ignored and `#` starts a comment that runs
  to the end of the line (RX6).

Anything else is rejected with the error in RX3. In particular `\1` and `\k<name>` are backreferences, `(?=`, `(?!`,
`(?<=`, `(?<!`, `(?>` are lookaround and atomic groups, and `*+`, `++`, `?+` are possessive: all are errors that say so.
Witnessed by the `RX2 syntax` and `RX3 errors` sections of `tests/regex/cases.tsv`.

## RX3. Errors

`Regex.new` reports a bad pattern as `RegexError.Syntax { at, message }`. `at` is the byte offset in the pattern of the
token the error is about, and the display is `invalid regex at byte {at}: {message}`. The messages are:

| Message | When |
| --- | --- |
| `unclosed group` | a `(` with no matching `)` (reported at that `(`) |
| `unopened group` | a `)` with no `(` before it |
| `unclosed character class` | a `[` with no closing `]` (reported at that `[`) |
| `invalid class range` | a range whose end is before its start, or whose end is a class escape such as `\d` |
| `nothing to repeat` | a quantifier with nothing before it, or a second quantifier, or a repetition of an assertion |
| `invalid repetition count` | a `{...}` that is not `{n}`, `{n,}`, or `{n,m}`, or a maximum `m` below its minimum `n` |
| `repetition count above 1000` | a count past RX7's limit |
| `trailing backslash` | a pattern that ends in `\` |
| `unknown escape sequence` | an unknown ASCII letter after a backslash |
| `invalid hex escape` | `\x` without its digits, or a code point past U+10FFFF or a surrogate |
| `unknown Unicode property` | a `\p` name that RX5 does not list |
| `unknown POSIX class` | a `[:name:]` that is not one of RX5's |
| `unknown group flag` | a letter in `(?...)` that is not a flag of RX6, or a `-` after another `-` |
| `unknown group syntax` | `(?` followed by something that is not a letter, a group, or a flag |
| `invalid group name` | a name that is empty, starts with a digit, or has a character outside ASCII letters, digits, and `_` |
| `duplicate group name` | a name that an earlier group already has |
| `nesting is too deep` | more than RX7's depth of nested groups |
| `the compiled regex is too large` | more than RX7's instruction count |
| `backreferences are not supported` | `\1`-`\9`, `\0`, `\k` |
| `lookaround is not supported` | `(?=`, `(?!`, `(?<=`, `(?<!` |
| `atomic groups are not supported` | `(?>` |
| `possessive quantifiers are not supported` | a quantifier followed by `+` (the form `*+`) |

`RegexError` also has `offset()` (the `at` field) and `reason()` (the message without the position). The position is
the byte offset, so a caller can point at the bad character: a multibyte character's first byte is its offset.
Witnessed by the `RX3 errors` section of `tests/regex/cases.tsv`, one row per message.

## RX4. Matching semantics

Matching is leftmost-first, as in Perl, Python, and `regex` in Rust: the match that starts earliest wins, and among
the matches that start there, the one that the pattern prefers. The preference is the order of the alternatives,
greedy quantifiers prefer more, and lazy ones prefer fewer. Two engines that agree on a pattern therefore report the same
span and the same groups for it. The differential check in RX14's witness compares this engine with Python's `re` on
generated patterns, and the only disagreement it allows is the empty-iteration case of RX8.

## RX5. Character classes

- `\d` is the ASCII digits `0`-`9`. `\w` is a letter or digit (`char.is_alphanumeric`) or `_`. `\s` is a white-space
  character (`char.is_whitespace`). The negations `\D`, `\W`, `\S` are their complements. The Unicode-aware `\w` and `\s`
  use the predicates of `std`, so they follow `std`'s tables (`spec/stdlib.md`, "Characters").
- `\p{L}` (letters, `char.is_alphabetic`), `\p{Lu}` (upper case), `\p{Ll}` (lower case), `\p{N}` (numbers, `char.is_numeric`),
  and `\p{White_Space}` (the same as `\s`). These are approximations of the Unicode general categories built from `std`'s
  predicates; the package does not ship the full tables, and any other name is `unknown Unicode property`. `\P` negates.
- A bracket class is a list of characters, ranges `a-z`, and class escapes, with `^` for the complement. A `]` first in the
  class is a literal. A `-` that cannot form a range (after a class escape, before a `]`, or first) is a literal, so `[\d-z]`
  is the digits, `-`, and `z`; a range whose end is a class escape (`[a-\d]`) is an error. Inside a class, `\n`, `\t`, `\\`, `\]`, `\-`, and the other escapes of RX2 work, and `\b` is an
  error, not a backspace.
- The POSIX classes `[:alpha:]`, `[:digit:]`, `[:alnum:]`, `[:upper:]`, `[:lower:]`, `[:space:]`, `[:blank:]`, `[:punct:]`,
  `[:xdigit:]`, `[:word:]`, `[:cntrl:]`, `[:print:]`, `[:graph:]`, and `[:ascii:]` are ASCII sets, and `[:^name:]` is the
  complement of one. They are allowed inside a bracket class, as in `[[:alpha:]_]`.

Witnessed by the `RX5 classes` section of `tests/regex/cases.tsv` and by the conformance case.

## RX6. Flags

| Flag | Effect |
| --- | --- |
| `i` | case-insensitive: a literal or class matches its other case as well (the lower case of the character is compared, so `ß`-like letters with no case map are unchanged) |
| `m` | `^` and `$` also match at a line feed |
| `s` | `.` also matches a line feed |
| `x` | verbose: white space and `#` comments outside a class are ignored (RX2) |

A flag is set with `(?i)` (the rest of the enclosing group), scoped with `(?i:...)`, and cleared with a `-` (`(?-i)`,
`(?i-s:...)`). `Regex.new` takes no flags argument: the flags are part of the pattern.

## RX7. Limits

A repetition count is at most 1000. A pattern may nest groups 100 deep: the parser recurses once per level, and 100 levels fit the 1 MB main-thread
stack that Windows gives a program, with room to spare (a depth of 180 overflowed it). The compiled program may have at most 100000
instructions; a pattern that exceeds it is rejected, which bounds the memory of a search regardless of the text. These
are the limits behind `repetition count above 1000`, `nesting is too deep`, and `the compiled regex is too large`.

## RX8. Captures

Group 0 is the whole match. Group `i` is the `i`-th capturing group. A group that did not take part in the match is
`Option.None`. A group inside a repetition holds the text of its last iteration. Named groups are the same groups with a name
(`Captures.name`, `Regex.group_index`).

One difference from the other engines is allowed. An iteration of a repetition that matches the empty text does not
repeat, and a capture set inside such an iteration is kept or dropped by the thread's priority, so `(a*)*` on `b` can
report group 1 as `Option.None` where another engine reports `""`. A pattern that does not nest an empty iteration is not
affected. The differential check generates no such pattern.

## RX9. The API

```
Regex.new(pattern: str): Result<Regex, RegexError>
Regex.escape(text: str): String                   // escapes every metacharacter (RX13)
re.as_str(self): str                              // the pattern
re.group_count(self): usize                       // the capturing groups, not counting group 0
re.group_index(self, name: str): Option<usize>
re.is_match(self, text: str): bool
re.find(self, text: str): Option<Match>
re.find_all(self, text: str): List<Match>
re.captures(self, text: str): Option<Captures>
re.captures_all(self, text: str): List<Captures>
re.replace(self, text: str, replacement: str): String          // the first match
re.replacen(self, text: str, limit: usize, replacement: str): String
re.replace_all(self, text: str, replacement: str): String
re.split(self, text: str): List<String>
re.splitn(self, text: str, limit: usize): List<String>

Match: start(self), end(self): usize; as_str(self): str; len(self): usize; is_empty(self): bool
Captures: len(self): usize; get(self, index: usize): Option<Match>; name(self, name: str): Option<Match>;
          expand(self, template: str): String
```

A `Match` owns a copy of its text, so it outlives the searched string. Offsets are byte offsets into the searched string
(RX14). `replacen` with a limit of `0` returns the text unchanged, and `splitn` with a limit of `0` returns no pieces.
Witnessed by the rows of `tests/regex/cases.tsv` for each operation and by `tests/s3_regex/usage`.

## RX10. Iteration over matches

`find_all`, `captures_all`, `replace_all`, and `split` take the matches from left to right with the rule of Python's
`re.finditer` since Python 3.7. After a non-empty match that ends at `e`, the next search starts at `e`, and an empty match
at `e` is allowed. After an empty match at `e`, the next search starts at `e` but must not be an empty match there: a
non-empty match may begin at `e` (it is reported right after the empty one), and otherwise the search moves one character
on. `x*` on `"abxd"` therefore gives the empty matches at 0, 1, 3, and 4 and the match `x` at 2, and `b|` on `"ab"` gives
the empty match at 0, the match `b` at 1, and the empty match at 2. When the search starts past the end of the text, the
iteration stops. Witnessed by the `RX10 iteration` section, by the `RX12 splitting` section,
and by the differential check against Python 3.7 and later, which uses the same rule.

## RX11. Replacement templates

`expand(template)` and the `replace` family write the template with these references, and every other character as it is:

- `$0`, `$1`, ... `$N`: the group with that number; `$` followed by digits takes all of the digits.
- `$name`: the group with that name; the name is the longest run of letters, digits, and `_` after the `$`.
- `${name}` and `${N}`: the group with that name or number, for a name that is followed by other text.
- `$$`: a literal `$`.

A `$` that is not followed by any of these is a literal `$`. A reference to a group that does not exist, or that did not take
part in the match, is the empty text. Witnessed by the `RX11 replacement templates` section.

## RX12. Splitting

`split` returns the text between successive matches of `find_all` (RX10). The pieces before the first match and after the
last are kept, so `n` matches give `n + 1` pieces, and an empty match splits between its two characters. `splitn(limit)`
makes at most `limit` pieces: once `limit - 1` matches have split the text, the rest is the last piece. Witnessed by the
`RX12 splitting` section and the differential check.

## RX13. Escaping

`Regex.escape(text)` puts a backslash before each of the metacharacters `\ . + * ? ( ) | [ ] { } ^ $ # & - ~`, and no other
character, so `Regex.new(Regex.escape(text))` matches `text` exactly. Witnessed by the `RX13 escaping` section and by the
differential check, which round-trips generated texts.

## RX14. Text and characters

A search runs on a `str`, which is valid UTF-8, so every character is a whole code point and never splits a multibyte
sequence. `.` and a class match one character. The positions in a `Match`, a `Captures`, and an error are byte offsets. The
assertions `^`, `$`, `\b` look at the characters on either side of a position, and `\A` and `^` (without `m`) match only at
byte zero of the text, even when a search starts later. Witnessed by the `RX14 text and characters` section of `tests/regex/cases.tsv`.

## The fixtures and the check

- `tests/regex/cases.tsv` has one row per case: `op`, `pattern`, `text`, `arg`, and the `expected` output. Every field is
  percent-encoded (`%09` tab, `%0A` line feed, `%0D` carriage return, `%25` percent) so a field holds no tab; a line that
  begins with `#` is a comment. `tests/regex/README.md` lists the ops and the output format.
- `tests/regex/runner` is the program that runs the rows through the package and prints one line per row.
- `tests/check_regex.py` builds the runner against `packages/regex` through `BURDIN_PACKAGES`, compares every golden row,
  generates random patterns and texts of the subset in which Python's `re` and this package agree, and compares their
  output row by row with `re` as the oracle. It also checks the implicit load, the official pin, and `--locked`.
- `tests/s3_regex/usage` is the conformance case: the public surface with a fixed expected output.
