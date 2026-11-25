# csv

CSV reading and writing on the serialization framework (issue #257; the framework is `spec/serialization.md`).
Use it with `use csv`; no `burdin.toml` entry is needed.

```burdin
use csv

struct Person: Serialize, Deserialize {
    name: String
    age: u8
    email: Option<String>
}

let text = csv.to_string(people)?                     // name,age,email + one line per record
let people: List<Person> = csv.from_str(text.as_str())?
```

## Surface

```
fn to_string<T: Serialize>(records: List<T>): Result<String, DeError>
fn to_string_with<T: Serialize>(records: List<T>, delimiter: char, has_headers: bool): Result<String, DeError>
fn from_str<T: Deserialize>(text: str): Result<List<T>, DeError>
fn from_str_with<T: Deserialize>(text: str, delimiter: char, has_headers: bool): Result<List<T>, DeError>
```

`from_str` reads every record; the first one is the header row. An empty list is empty text (there is no record to
take the field names from). `to_string_with` and `from_str_with` choose the delimiter (an ASCII character other
than a quote or a line break, which is an error) and whether there is a header row.

The streaming types are public, and they are what the functions above are built from:

```
Writer.new(): Writer                                                 // commas, a header row, "\n"
Writer.with_options(delimiter: char, has_headers: bool): Result<Writer, DeError>
writer.use_crlf()                                                    // end each record with "\r\n"
writer.write<T: Serialize>(record: T): Result<(), DeError>           // one record
writer.write_row(fields: List<String>): Result<(), DeError>          // one record of text, no header row
writer.take(): String                                                // what was written since the last call
writer.finish(): String                                              // everything not yet taken

Reader.new(text: str): Reader
Reader.with_options(text: str, delimiter: char, has_headers: bool): Result<Reader, DeError>
reader.set_headers(names: List<String>)                              // name the columns of a file with no header row
reader.headers(): Result<List<String>, DeError>
reader.read<T: Deserialize>(): Result<Option<T>, DeError>            // the next record; None at the end
reader.read_row(): Result<Option<List<String>>, DeError>             // the next record as text
```

"Streaming" means one record at a time: `read` parses one record and keeps no record after it, and `write` buffers one
record and appends it to the output, which `take` hands over after every record so a caller never holds the whole
table. The reader keeps a copy of the input text (the framework's v1 zero-copy story, SER11), so it is not an
incremental reader of a file.

## Records

A CSV record is flat: the framework value a record holds is a struct, a map with string keys, or a sequence of
scalars, and every field in it is one scalar.

| Framework shape | CSV |
| --- | --- |
| struct, map | a record; the field names (the map keys) are the header row |
| sequence (a tuple of scalars) | a record with no names, so no header row |
| unit | an empty field |
| `bool` | `true` / `false` |
| every integer width, `f32`, `f64` | the number's text |
| `char`, `str`, `String` | text |
| `Option.None` | an empty field |
| `Option.Some(value)` | the value itself |
| variant with no payload | its name |
| struct, map, sequence, or bytes inside a field; a variant with a payload | an error (`a CSV field cannot hold a struct, a map, or a sequence at address`) |

A struct is read by header name, in any column order: `role,name` and `name,role` both read a `{ name, role }`
struct, and the output is always in the struct's order. A header the struct has no field for is `unknown field`, a
field with no column is `missing field`, and a repeated header is `duplicate field`. Reading a struct from a file
with no header row needs `set_headers` first.

Writing: every record must have the same field names as the first (a different type is an error) and the same number
of fields. A failed `write` leaves the output as it was: nothing of the record is written.

## Quoting

A field is written in quotes when it holds the delimiter, a double quote, a `\n`, or a `\r`, and a quote inside is
doubled (`"Say ""hi"""`). A reader accepts quotes only around a whole field; a quote in the middle of an unquoted
field, or text after a closing quote, is a syntax error. Embedded line breaks are kept exactly as they were read,
`\r\n` included. Spaces are part of the field: nothing is trimmed.

An empty string is written as `""` so that it can be told from `Option.None`: an empty field with no quotes is `Option.None` (or
the unit value), and an empty quoted field is the empty string. In a record of one field an empty quoted field
reads as `Option.None` too, because that is how a lone `Option.None` is written (an empty line would be skipped as a blank line).

## Reading

- A record ends at `\n`, `\r\n`, or a lone `\r`; the last record may have no line ending.
- Blank lines between records are skipped. A UTF-8 byte order mark at the start of the text is skipped.
- Every record has as many fields as the header row (or the first record, without one): a record with another
  number is `expected 6 fields but found 4`.
- Numbers are read strictly: no spaces, and an integer has no `+` sign and no `-` for an unsigned type. An integer that does not
  fit its width is `a number that does not fit in u8`, and a float that overflows is an error, not an infinity
  (`inf` and `NaN` as text are read, and the writer writes them). Booleans are `true` or `false`.

## Errors

Every error is a `DeError` with a message. A syntax error and a value error have the line and the 1-based column,
counted in Unicode scalar values, of the field they are about (`line 3, column 5: expected a non-negative integer`),
and a line ends at `\n`, `\r\n`, or a lone `\r`. `read` and `from_str` add the index of the record (the header row is
not counted) and the field as the path: `line 3, column 5: expected a non-negative integer at [1].age`. The
framework's own `unknown field`, `missing field`, and `unknown variant` errors have no position of their own, so
the line and column are those of the start of the record. A writer error carries the field name as its path, and
`to_string` adds the record index (`at [0].address`).

Reading never panics on malformed text: an unterminated quote, a quote in the middle of a field, a ragged record,
and a value that does not parse are all reports, and after a syntax error the reader returns no more records.

## Tests

`tests/check_csv.py` drives this package through fixture files: `tests/csv/roundtrip` (read each record, write it
back as it is read, and compare with writing the whole list), `semicolon`, `noheader`, `crlf` (converted to
`\r\n` by the script), `raw` and `crlf_raw` and `cr_raw` (the text fields of every record), and `bad` and `bad_raw`
(malformed input, one document per file), each with an `.expected` golden. It also checks the implicit `use csv` load and
the official pin in `burdin.lock`. `tests/s3_serialize/csv` is the conformance case that runs the writer errors and
the hand-written sequence record inside the conformance suite.
