# regex

Regular expressions. A pattern is compiled once and then searches any number of texts, in time proportional to the pattern times the text (issue #264; `spec/regex.md`).

Version `0.1.0`. Use it with `use regex`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/regex/README.md`](../../packages/regex/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Finds the matches of a pattern, reads a capture group, and replaces every match.

```burdin
use regex.{Regex}

fn main() {
    let pattern = Regex.new("([a-z]+)@([a-z.]+)").unwrap()
    let text = "contact ada@example.test or lin@example.test"
    assert(pattern.is_match(text))
    let first = pattern.captures(text).unwrap()
    assert_eq(first.get(1).unwrap().as_str(), "ada")
    let hosts = pattern.find_all(text)
    assert_eq(hosts.len(), 2)
    let masked = pattern.replace_all(text, "<redacted>")
    println(masked)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use regex`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `errors.RegexError` | `error enum RegexError: Eq, Debug` | A pattern that is not valid. |
| `errors.RegexError.offset` | `fn offset(self): usize` | The byte offset in the pattern of the token the error is about. |
| `errors.RegexError.reason` | `fn reason(self): String` | What is wrong, without the position. |
| `regex.Regex` | `struct Regex` | A compiled regular expression (RX9). |
| `regex.Regex.new` | `fn new(pattern: str): Result<Regex, RegexError>` | Compiles `pattern` (RX2), or reports the first problem with its byte offset (RX3). |
| `regex.Regex.escape` | `fn escape(text: str): String` | `text` with each metacharacter escaped, so that `Regex.new` of the result matches `text` exactly (RX13). |
| `regex.Regex.as_str` | `fn as_str(self): str` | The pattern this regex was compiled from. |
| `regex.Regex.group_count` | `fn group_count(self): usize` | The number of capturing groups, not counting group 0. |
| `regex.Regex.group_index` | `fn group_index(self, name: str): Option<usize>` | The number of the group named `name`, if there is one. |
| `regex.Regex.is_match` | `fn is_match(self, text: str): bool` | Whether the pattern matches somewhere in `text`. |
| `regex.Regex.find` | `fn find(self, text: str): Option<Match>` | The leftmost match in `text`. |
| `regex.Regex.find_all` | `fn find_all(self, text: str): List<Match>` | Every match in `text`, from left to right without overlap (RX10). |
| `regex.Regex.captures` | `fn captures(self, text: str): Option<Captures>` | The leftmost match in `text` with its groups. |
| `regex.Regex.captures_all` | `fn captures_all(self, text: str): List<Captures>` | Every match in `text` with its groups (RX10). |
| `regex.Regex.replace` | `fn replace(self, text: str, replacement: str): String` | `text` with its first match replaced by `replacement`, which may use the references of RX11. |
| `regex.Regex.replacen` | `fn replacen(self, text: str, limit: usize, replacement: str): String` | `text` with its first `limit` matches replaced by `replacement` (RX11). |
| `regex.Regex.replace_all` | `fn replace_all(self, text: str, replacement: str): String` | `text` with every match replaced by `replacement` (RX11). |
| `regex.Regex.split` | `fn split(self, text: str): List<String>` | The pieces of `text` between the matches (RX12). |
| `regex.Regex.splitn` | `fn splitn(self, text: str, limit: usize): List<String>` | At most `limit` pieces of `text`: the last one is the rest of the text (RX12). |
| `regex.Match` | `struct Match: Clone, Eq, Debug` | One match: the byte offsets of its span in the searched text, and a copy of the matched text (RX14). |
| `regex.Match.start` | `fn start(self): usize` | The byte offset where the match starts. |
| `regex.Match.end` | `fn end(self): usize` | The byte offset just past the end of the match. |
| `regex.Match.as_str` | `fn as_str(self): str` | The matched text. |
| `regex.Match.len` | `fn len(self): usize` | The length of the match in bytes. |
| `regex.Match.is_empty` | `fn is_empty(self): bool` | Whether the match is empty. |
| `regex.Captures` | `struct Captures: Debug` | The groups of one match (RX8). |
| `regex.Captures.len` | `fn len(self): usize` | The number of groups, including group 0 (the whole match). |
| `regex.Captures.get` | `fn get(self, index: usize): Option<Match>` | Group `index`, or None when there is no such group or it did not take part in the match. |
| `regex.Captures.name` | `fn name(self, key: str): Option<Match>` | The group named `name`, or None when there is no such group or it did not take part. |
| `regex.Captures.expand` | `fn expand(self, template: str): String` | `template` with its references replaced by the groups of this match (RX11). |
