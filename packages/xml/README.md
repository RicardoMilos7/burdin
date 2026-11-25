# xml

XML reading and writing: a small DOM, and the serialization framework's format side (issue #259; the framework is
`spec/serialization.md`). Use it with `use xml`; no `burdin.toml` entry is needed.

```burdin
use xml

let text = xml.to_string(config)?
let config = xml.from_str(text.as_str())?
```

The package has three layers, each usable by itself: the DOM (`Document`, `Element`, `Node`, `Attribute`) with a
builder and a serializer; the reader that turns text into a DOM (`xml.parse`); and `Writer` and `Reader`, which
implement `Serializer` and `Deserializer` over the DOM so that derived types round trip.

## Surface

```
fn parse(text: str): Result<Document, DeError>
fn to_string<T: Serialize>(value: T): Result<String, DeError>        // compact, no declaration
fn to_string_pretty<T: Serialize>(value: T): Result<String, DeError> // two-space indentation, final newline
fn from_str<T: Deserialize>(text: str): Result<T, DeError>
fn to_document<T: Serialize>(value: T): Result<Document, DeError>
fn to_document_root<T: Serialize>(value: T, root: str): Result<Document, DeError>
fn from_document<T: Deserialize>(document: Document): Result<T, DeError>
```

The DOM, in short:

- `Document { declaration, prolog, root, epilog }`: `Document.new(root)`, `with_declaration()`, `to_xml()`,
  `to_xml_pretty()`.
- `Element { name, namespace, attributes, children, line, column }`, where `name` is the qualified name as written
  (`p:item`). Builder: `Element.new(name)`, `with_attr`, `with_namespace`, `with_child`, `with_text`, `set_attr`,
  `set_attr_ns`, `declare_namespace`, `push_child`, `push_text`, `push_cdata`, `push_comment`, `push_instruction`.
  Queries: `attr`, `attr_ns(namespace, local)`, `lookup_namespace(prefix)`, `text`, `text_content`, `child(name)`,
  `children_named(name)`, `elements()`, `child_ns`, `children_ns`, `find_ns` (a depth-first search),
  `is_named(namespace, local)`, `prefix()`, `local_name()`.
- `Node` is `Text`, `CData`, `Comment`, `Instruction`, or `Element(Heap<Element>)`. The element is boxed because a
  tree is recursive.

The DOM types have a `copy()` method and no derives (`Clone`, `Debug`, `Eq`): the compiler cannot yet derive them for
a recursive type.

`to_xml` and `to_xml_pretty` check what they write: a name that is not a valid XML name, a character XML cannot hold,
`--` in a comment, `]]>` in a CDATA section, an instruction named `xml`, and a nesting deeper than 256 are each an
error, never invalid text. Text is escaped (`&amp;`, `&lt;`, `&gt;`, and a carriage return as `&#13;`), and an
attribute value also escapes `&quot;`, tab, and line breaks, so they survive reading. Pretty output puts each child of
an element that has no text on its own indented line; an element that holds text, CDATA, or mixed content is written
as it is, so pretty output never adds whitespace to a value.

## Mapping

Only elements carry data. Attributes are read and written through the DOM, never by `to_string` and `from_str`.

| Framework shape | XML |
| --- | --- |
| unit | an empty element |
| `bool` | `true` / `false` |
| every integer width | the digits |
| `f32`, `f64` | the shortest text that reads back (`0.25`, `1`); `NaN`, `INF`, `-INF` |
| `char`, `str`, `String` | the text, escaped; an empty string is an empty element |
| bytes (`[u8]`) | hexadecimal digits, two per byte |
| sequence | the elements, each an `<item>` child |
| map with `String` keys | `<entry><key>k</key><value>v</value></entry>` children, in order |
| struct | one child element per field, named after the field; the root is named after the struct |
| variant with no payload | `<Variant/>` inside the value's element |
| variant with one value | `<Variant>value</Variant>`, the value written as that element's content |
| variant with several values | `<Variant><item>1</item><item>2</item></Variant>` |
| `Option` | an ordinary enum: `<Option.None/>` and `<Option.Some>value</Option.Some>` |

The root element is named after a struct or an enum (`<Config>`), and `root` for anything else; `to_document_root`
chooses the name. The element a struct field or a sequence item becomes keeps its field name or `item`, so a struct in
a sequence is `<item><host>..</host></item>`.

```xml
<Config>
  <name>edge</name>
  <servers>
    <item><host>a.example</host><port>80</port></item>
  </servers>
  <protocol><Both><item>80</item><item>443</item></Both></protocol>
  <backup><None/></backup>
  <weights><entry><key>a</key><value>1</value></entry></weights>
</Config>
```

Reading follows the same shapes and is deliberately forgiving about everything that is not data: it matches elements
by local name, so a namespace prefix does not matter, ignores attributes, comments, and processing instructions, and
accepts CDATA anywhere text is. The root element's name is not checked. Fields may come in any order, but each may
appear once: a duplicate, a missing field, and an unknown field are errors. Whitespace between elements is ignored;
inside an element that holds a value, numbers, booleans, and bytes are trimmed and a string is read as written. A
value that has both text and child elements is an error (`unexpected text in <t>`).

## Namespaces and entities

`parse` resolves prefixes: `Element.namespace` and `Attribute.namespace` hold the URI (empty for none), an unprefixed
attribute never takes the default namespace, `xmlns="..."` and `xmlns=""` change and clear the default, and `xml` is
always bound to `http://www.w3.org/XML/1998/namespace`. Binding `xml` or `xmlns` to anything else, an empty URI for a
prefix, an undeclared prefix, and two attributes with the same expanded name are errors. Declarations stay in
`attributes`, with an empty namespace, so a document writes back as it was read.

The five predefined entities (`&lt; &gt; &amp; &quot; &apos;`) and numeric character references (`&#65;`, `&#x41;`) are
decoded in text and attribute values. A reference to a character XML does not allow, such as `&#0;` or a surrogate,
is an error. Line breaks are normalized (`CRLF` and `CR` become `LF`), a leading byte order mark is skipped, and
attribute values are normalized as the XML specification says.

## DOCTYPE and external entities

A `<!DOCTYPE` is an error. There is no DTD, so there are no user-defined entities, no external entities, no parameter
entities, and no entity expansion: the XXE attacks and the "billion laughs" bomb have nothing to work with. An
entity the five do not name (`&nbsp;`, `&xxe;`) is an `undefined entity`. Other limits keep hostile input cheap: a
nesting deeper than 256 elements and a start tag with more than 1024 attributes are errors, and the parser keeps its
own stack, so a deeply nested file cannot overflow the real one.

## Errors

Every failure is a `DeError`. A syntax error carries a line and a 1-based column counted in Unicode scalar values
(`line 1, column 16: the closing tag </nam> does not match <name>`); a value error is positioned at the element and
names the path inside the data (`line 4, column 25: a number that does not fit in u16 at servers[0].port`, `line 4,
column 39: expected 'true' or 'false' at servers[0].tls`). Parsed elements remember their `line` and `column`, and the
reader reports them. A field that is missing, unknown, or duplicated is reported by the framework's derive (`missing
field 'note'`, `unknown field 'colour' at colour`).

Reading and parsing never panic on malformed text: truncated input, bad names, bad references, and every other
malformation is a report. Writing reports a value or a tree XML has no form for.

## Tests

`tests/check_xml.py` drives this package through fixture files: `tests/xml/roundtrip` (parse into a derived type and
write it back), `tests/xml/pretty`, `tests/xml/bad` (one bad document per file, with its exact message in an
`.expected` file), `tests/xml/dom` (the DOM: namespaces, entities, positions), `tests/xml/dombad` (parse errors,
including DOCTYPE and entity attacks), and `tests/xml/truncate` (every prefix of a document, and every single-character
deletion, must be reported and never panic). `tests/s3_serialize/xml` is the conformance case; it also holds what a
fixture file cannot carry (carriage returns, a byte order mark, the builder, writer errors, and a 100000-deep input).
