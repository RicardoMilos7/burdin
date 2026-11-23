# csv

CSV reading and writing on the serialization framework (issue #257; `spec/serialization.md`).

Version `0.1.0`. Use it with `use csv`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/csv/README.md`](../../packages/csv/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Writes a list of records as CSV, then reads them back.

```burdin
use csv

struct Person: Serialize, Deserialize {
    name: String
    age: u8
    email: Option<String>
}

fn main() {
    let mut people = List<Person>.new()
    people.push(Person { name: "Ada".to_string(), age: 36, email: None })
    people.push(Person { name: "Lin".to_string(), age: 29, email: Some("lin@example.test".to_string()) })
    let text = csv.to_string(people).unwrap()
    println(text)
    let back = csv.from_str<Person>(text.as_str()).unwrap()
    assert_eq(back.len(), 2)
    assert_eq(back[1].age, 29)
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use csv`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `csv.Writer` | `struct Writer` | The CSV writer. |
| `csv.Writer.new` | `fn new(): Writer` | A writer with commas, a header row, and `\n` line endings. |
| `csv.Writer.with_options` | `fn with_options(delimiter: char, has_headers: bool): Result<Writer, DeError>` | A writer with the given delimiter (an ASCII character other than a quote or a line break) and whether the first record of a struct or map writes a header row. |
| `csv.Writer.use_crlf` | `fn use_crlf(mut self)` | Ends each record with `\r\n` instead of `\n`. |
| `csv.Writer.finish` | `fn finish(own self): String` | The text written so far, ending the writer. |
| `csv.Writer.take` | `fn take(mut self): String` | The text written since the last call, leaving the writer empty. |
| `csv.Writer.write` | `fn write<T: Serialize>(mut self, record: T): Result<(), DeError>` | Writes one record. |
| `csv.Writer.write_row` | `fn write_row(mut self, fields: List<String>): Result<(), DeError>` | Writes one record of text fields, quoting what needs it. |
| `csv.Reader` | `struct Reader` | The CSV reader: a copy of the input, the offset of the next unread record, and the record being read. |
| `csv.Reader.new` | `fn new(text: str): Reader` | A reader over `text`, with commas and a header row. |
| `csv.Reader.with_options` | `fn with_options(text: str, delimiter: char, has_headers: bool): Result<Reader, DeError>` | A reader with the given delimiter (an ASCII character other than a quote or a line break) and whether the first record is a header row. |
| `csv.Reader.set_headers` | `fn set_headers(mut self, own names: List<String>)` | Names the columns of a text that has no header row, so a struct can be read from it. |
| `csv.Reader.headers` | `fn headers(mut self): Result<List<String>, DeError>` | The header row, reading it if nothing has been read yet. |
| `csv.Reader.read_row` | `fn read_row(mut self): Result<Option<List<String>>, DeError>` | The next record as its text fields, or `None` at the end. |
| `csv.Reader.read` | `fn read<T: Deserialize>(mut self): Result<Option<T>, DeError>` | The next record as a `T`, or `None` at the end. |
| `csv.to_string` | `fn to_string<T: Serialize>(records: List<T>): Result<String, DeError>` | Writes `records` as CSV: a header row of the first record's field names (for a struct or map), then one line for each record. |
| `csv.to_string_with` | `fn to_string_with<T: Serialize>(records: List<T>, delimiter: char, has_headers: bool): Result<String, DeError>` | Like `to_string`, with the delimiter and the header row chosen. |
| `csv.from_str` | `fn from_str<T: Deserialize>(text: str): Result<List<T>, DeError>` | Reads every record of `text` as a `T`. |
| `csv.from_str_with` | `fn from_str_with<T: Deserialize>(text: str, delimiter: char, has_headers: bool): Result<List<T>, DeError>` | Like `from_str`, with the delimiter and the header row chosen. |
