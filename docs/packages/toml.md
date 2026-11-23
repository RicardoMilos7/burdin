# toml

TOML 1.0.0 reading and writing on the serialization framework (issue #256; `spec/toml.md`).

Version `0.1.0`. Use it with `use toml`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/toml/README.md`](../../packages/toml/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Reads a TOML document into a struct and writes it back.

```burdin
use toml

struct Package: Serialize, Deserialize {
    name: String
    version: String
    features: List<String>
}

fn main() {
    let text = "name = \"demo\"\nversion = \"1.2.3\"\nfeatures = [\"fast\"]\n"
    let package = toml.from_str<Package>(text).unwrap()
    assert_eq(package.version.as_str(), "1.2.3")
    assert_eq(package.features.len(), 1)
    let out = toml.to_string(package).unwrap()
    println(out)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use toml`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `toml.Span` | `struct Span: Copy` | A position in the input: `line` and `column` are 1-based (a column counts Unicode scalar values), and `start` and `end` are byte offsets. |
| `toml.Date` | `struct Date: Copy` | A calendar date: a four-digit year, a month from 1 to 12, and a day that exists in its month. |
| `toml.Time` | `struct Time: Copy` | A time of day. |
| `toml.Datetime` | `enum Datetime: Copy` | One of the four TOML datetime forms (TOML9). |
| `toml.Value` | `enum Value` | A TOML value. |
| `toml.Item` | `struct Item` | A value and the place it starts. |
| `toml.Entry` | `struct Entry` | A key of a table, the place the key starts, and the item it names. |
| `toml.parse` | `fn parse(text: str): Result<Value, DeError>` | Reads a TOML document into a `Value` tree (TOML2-TOML11). |
| `toml.render` | `fn render(value: Value): Result<String, DeError>` | Writes `value` in the canonical form (TOML13): the scalars of each table first, then its tables as sections. |
| `toml.Writer` | `struct Writer` | The TOML writer. |
| `toml.Writer.new` | `fn new(): Writer` | A writer with an empty root table. |
| `toml.Writer.document` | `fn document(self): Value` | The value written, as a `Value` tree. |
| `toml.to_value` | `fn to_value<T: Serialize>(value: T): Result<Value, DeError>` | Writes `value` as a `Value` tree: the framework's writes, arranged as TOML (TOML14). |
| `toml.to_string` | `fn to_string<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as canonical TOML text (TOML13). |
| `toml.Reader` | `struct Reader` | The TOML reader. |
| `toml.from_value` | `fn from_value<T: Deserialize>(value: Value): Result<T, DeError>` | Reads a `Value` tree as a `T` (the framework's reads, arranged as TOML; TOML15). |
| `toml.from_str` | `fn from_str<T: Deserialize>(text: str): Result<T, DeError>` | Reads a `T` from TOML text (TOML2-TOML15). |
