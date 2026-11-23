# regex

Regular expressions for Burdin (stage 3, issue #264; the exact behavior is `spec/regex.md`).
`use regex` loads it; no `burdin.toml` entry is needed.

```burdin
use regex.{Regex}

let date = Regex.new("(?P<year>\\d{4})-(?P<month>\\d{2})")?
if date.is_match("released 2024-05") {
    let caps = date.captures("released 2024-05").unwrap()
    println("{}", caps.name("month").unwrap().as_str())    // 05
}
let words = Regex.new("\\w+")?.find_all("two words")      // List<Match>
let clean = Regex.new("\\s+")?.replace_all("a   b", " ")  // "a b"
```

The engine is a Pike VM: a search takes time proportional to the pattern times the text, so no pattern makes it
backtrack exponentially (`(a+)+$` on a long run of `a` followed by `!` is fast). It is written in Burdin, so it needs no
C library. The price is that the features that need backtracking are errors, not emulations.

## The API

```
Regex.new(pattern: str): Result<Regex, RegexError>
Regex.escape(text: str): String                      // a pattern that matches `text` exactly

re.as_str(self): str                                 // the pattern
re.group_count(self): usize                          // the capturing groups, not counting the whole match
re.group_index(self, name: str): Option<usize>
re.is_match(self, text: str): bool
re.find(self, text: str): Option<Match>
re.find_all(self, text: str): List<Match>
re.captures(self, text: str): Option<Captures>
re.captures_all(self, text: str): List<Captures>
re.replace(self, text: str, replacement: str): String       // the first match
re.replacen(self, text: str, limit: usize, replacement: str): String
re.replace_all(self, text: str, replacement: str): String
re.split(self, text: str): List<String>
re.splitn(self, text: str, limit: usize): List<String>

Match: start(self), end(self): usize; as_str(self): str; len(self): usize; is_empty(self): bool
Captures: len(self): usize; get(self, index: usize): Option<Match>; name(self, name: str): Option<Match>;
          expand(self, template: str): String
RegexError: offset(self): usize; reason(self): str; to_string(self): String
```

A `Match` owns a copy of its text, so it can outlive the searched string. Positions are byte offsets, and a multibyte
character is one unit of matching.

## Patterns

- Literals, `.` (any character but a line feed), `^`, `$`, `\A`, `\z`, `\b`, `\B`.
- Classes: `[a-z]`, `[^...]`, `\d \D \w \W \s \S`, `\p{L} \p{Lu} \p{Ll} \p{N} \p{White_Space}` and their negations,
  and the POSIX classes `[[:alpha:]]` and the rest of their set.
- Groups: `(re)` captures, `(?:re)` does not, `(?P<name>re)` and `(?<name>re)` name a group (ASCII letters, digits, and
  `_`).
- Alternation `a|b`, and the repetitions `* + ? {n} {n,} {n,m}`, each lazy with a trailing `?`.
- Flags: `(?i)`, `(?m)`, `(?s)`, `(?x)`, scoped as `(?i:re)` and cleared with `-`.
- Escapes: `\t \n \r \f \v \a`, `\xHH`, `\x{H...}`.

Matching is leftmost-first, as in Python's `re` and Perl: the earliest match wins, and among those the pattern prefers.

## What is not supported

These are errors, reported by `Regex.new`, rather than silent differences:

- backreferences (`\1`, `\k<name>`), lookahead and lookbehind (`(?=`, `(?!`, `(?<=`, `(?<!`), atomic groups (`(?>`),
  and possessive quantifiers (`*+`).
- Unicode general categories other than those listed above, and the full Unicode tables.

## Replacement templates

`expand` and the `replace` family use `$0`, `$1`, ..., `$name`, `${name}`, `${1}`, and `$$` for a literal dollar sign.
A reference to a missing or unset group is empty, and a `$` that starts none of these is a literal `$`.

## Errors

`Regex.new` returns `RegexError.Syntax { at, message }` with the byte offset of the bad token, for example
`invalid regex at byte 0: unclosed group`. A search never fails.

## Limits

A repetition count is at most 1000, groups nest at most 100 deep, and a compiled pattern has at most 100000
instructions. These bound the memory of a search regardless of the text.
