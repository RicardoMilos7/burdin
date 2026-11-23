# json

JSON reading and writing on the serialization framework (issue #255; `spec/serialization.md`).

Version `0.1.0`. Use it with `use json`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/json/README.md`](../../packages/json/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Writes a struct as pretty JSON and reads it back.

```burdin
use json

struct Server: Serialize, Deserialize {
    host: String
    port: u16
    tags: List<String>
}

fn main() {
    let mut tags = List<String>.new()
    tags.push("primary".to_string())
    let server = Server { host: "example.test".to_string(), port: 8080, tags: tags }
    let text = json.to_string_pretty(server).unwrap()
    println(text)
    let back = json.from_str<Server>(text.as_str()).unwrap()
    assert_eq(back.port, 8080)
    assert_eq(back.tags.len(), 1)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use json`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `json.Writer` | `struct Writer` | The JSON writer. |
| `json.Writer.new` | `fn new(): Writer` | A compact writer. |
| `json.Writer.new_pretty` | `fn new_pretty(): Writer` | A writer with two-space indentation. |
| `json.Writer.finish` | `fn finish(own self): String` | The text written so far, ending the writer. |
| `json.Parser` | `struct Parser` | The JSON reader: a copy of the input and the byte offset of the next unread byte. |
| `json.Parser.new` | `fn new(text: str): Parser` | A reader over `text`. |
| `json.Parser.end` | `fn end(mut self): Result<(), DeError>` | Checks that no non-whitespace text is left. |
| `json.to_string` | `fn to_string<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as compact JSON. |
| `json.to_string_pretty` | `fn to_string_pretty<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as JSON with two-space indentation and a final newline. |
| `json.from_str` | `fn from_str<T: Deserialize>(text: str): Result<T, DeError>` | Reads a value of type `T` from `text`. |
