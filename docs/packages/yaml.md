# yaml

A safe subset of YAML 1.2 on the serialization framework (issue #258; `spec/serialization.md`). Tags other than `!!str`, merge keys, directives, and second documents are errors, never guesses.

Version `0.1.0`. Use it with `use yaml`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/yaml/README.md`](../../packages/yaml/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Writes a struct as YAML and reads it back.

```burdin
use yaml

struct Service: Serialize, Deserialize {
    name: String
    replicas: u32
    enabled: bool
}

fn main() {
    let service = Service { name: "api".to_string(), replicas: 3, enabled: true }
    let text = yaml.to_string(service).unwrap()
    println(text)
    let back = yaml.from_str<Service>(text.as_str()).unwrap()
    assert_eq(back.replicas, 3)
    assert(back.enabled)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use yaml`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `yaml.Loader` | `struct Loader` | The YAML parser: a normalized copy of the input, the byte offset of the next unread byte, the nodes built so far, and the anchors seen so far. |
| `yaml.Loader.new` | `fn new(text: str): Loader` | A loader over `text`. |
| `yaml.Loader.load` | `fn load(mut self): Result<usize, DeError>` | Loads the whole text as one document and returns the index of its root node. |
| `yaml.Loader.finish` | `fn finish(own self, root: usize): Reader` | Ends the loader, handing its tree and its text to a reader. |
| `yaml.Reader` | `struct Reader` | The YAML reader: a loaded tree, and the node the next value is read from. |
| `yaml.Writer` | `struct Writer` | The YAML writer: it builds a tree from the framework's calls, and `finish` prints it in block style. |
| `yaml.Writer.new` | `fn new(): Writer` | A writer with no value yet. |
| `yaml.Writer.finish` | `fn finish(own self): Result<String, DeError>` | The text of the value written, ending the writer. |
| `yaml.to_string` | `fn to_string<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as a YAML document in block style, ending in a line feed. |
| `yaml.from_str` | `fn from_str<T: Deserialize>(text: str): Result<T, DeError>` | Reads a value of type `T` from `text`, a single YAML document. |
