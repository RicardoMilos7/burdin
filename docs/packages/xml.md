# xml

An XML document model, a reader, and serialization on the framework (issue #259; `spec/serialization.md`). A DOCTYPE is rejected, so there are no external or user-defined entities.

Version `0.1.0`. Use it with `use xml`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/xml/README.md`](../../packages/xml/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Writes a struct as XML and reads it back.

```burdin
use xml

struct Book: Serialize, Deserialize {
    title: String
    year: u16
}

fn main() {
    let book = Book { title: "Notes".to_string(), year: 1843 }
    let text = xml.to_string_pretty(book).unwrap()
    println(text)
    let back = xml.from_str<Book>(text.as_str()).unwrap()
    assert_eq(back.year, 1843)
    assert_eq(back.title.as_str(), "Notes")
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use xml`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `xml.XML_NAMESPACE` | `const XML_NAMESPACE: str = "http://www.w3.org/XML/1998/namespace"` | The namespace URI the `xml` prefix is always bound to (`xml:lang` and the other reserved names belong to it). |
| `xml.MAX_DEPTH` | `const MAX_DEPTH: usize = 256` | The deepest element nesting the reader accepts and the serializer writes: 256 levels. |
| `xml.MAX_ATTRIBUTES` | `const MAX_ATTRIBUTES: usize = 1024` | The most attributes one start tag may have: 1024. |
| `xml.Attribute` | `struct Attribute: Clone` | An attribute. |
| `xml.Attribute.new` | `fn new(name: str, value: str): Attribute` | An attribute with no namespace. |
| `xml.Attribute.is_declaration` | `fn is_declaration(self): bool` | Whether this attribute declares a namespace (`xmlns` or `xmlns:prefix`). |
| `xml.Attribute.prefix` | `fn prefix(self): String` | The part of the name before the colon, or an empty string. |
| `xml.Attribute.local_name` | `fn local_name(self): String` | The part of the name after the colon, or the whole name. |
| `xml.Instruction` | `struct Instruction: Clone` | A processing instruction: `<?target data?>`. |
| `xml.Declaration` | `struct Declaration: Clone` | The XML declaration: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>`. |
| `xml.Declaration.new` | `fn new(): Declaration` | Version 1.0 and UTF-8, the declaration a new document gets from `with_declaration`. |
| `xml.Element` | `struct Element` | An element. |
| `xml.Element.new` | `fn new(name: str): Element` | An element with the qualified name `name`, no namespace, no attributes, and no children. |
| `xml.Element.prefix` | `fn prefix(self): String` | The part of the name before the colon, or an empty string. |
| `xml.Element.local_name` | `fn local_name(self): String` | The part of the name after the colon, or the whole name. |
| `xml.Element.set_attr` | `fn set_attr(mut self, name: str, value: str)` | Adds or replaces the attribute `name`. |
| `xml.Element.set_attr_ns` | `fn set_attr_ns(mut self, name: str, namespace: str, value: str)` | Adds or replaces a prefixed attribute and records its namespace; the declaration `xmlns:prefix` is added when this element does not declare the prefix yet. |
| `xml.Element.remove_attr` | `fn remove_attr(mut self, name: str): bool` | Removes the attribute `name`; whether there was one. |
| `xml.Element.declare_namespace` | `fn declare_namespace(mut self, prefix: str, uri: str)` | Declares a namespace on this element: `xmlns="uri"` for an empty prefix, `xmlns:prefix="uri"` otherwise. |
| `xml.Element.set_namespace` | `fn set_namespace(mut self, uri: str)` | Puts this element in the namespace `uri` and declares it for the element's own prefix. |
| `xml.Element.push_child` | `fn push_child(mut self, own child: Element)` | Appends a child element. |
| `xml.Element.push_text` | `fn push_text(mut self, text: str)` | Appends a text node. |
| `xml.Element.push_cdata` | `fn push_cdata(mut self, text: str)` | Appends a CDATA section. |
| `xml.Element.push_comment` | `fn push_comment(mut self, text: str)` | Appends a comment. |
| `xml.Element.push_instruction` | `fn push_instruction(mut self, target: str, data: str)` | Appends a processing instruction. |
| `xml.Element.with_attr` | `fn with_attr(own self, name: str, value: str): Element` | The builder form of `set_attr`. |
| `xml.Element.with_namespace` | `fn with_namespace(own self, uri: str): Element` | The builder form of `set_namespace`. |
| `xml.Element.with_child` | `fn with_child(own self, own child: Element): Element` | The builder form of `push_child`. |
| `xml.Element.with_text` | `fn with_text(own self, text: str): Element` | The builder form of `push_text`. |
| `xml.Element.attr` | `fn attr(self, name: str): Option<String>` | The value of the attribute `name` (the qualified name as written), if there is one. |
| `xml.Element.has_attr` | `fn has_attr(self, name: str): bool` | Whether there is an attribute `name`. |
| `xml.Element.attr_ns` | `fn attr_ns(self, namespace: str, local: str): Option<String>` | The value of the attribute whose namespace is `namespace` and whose local name is `local`. |
| `xml.Element.lookup_namespace` | `fn lookup_namespace(self, prefix: str): Option<String>` | The URI this element's own declarations bind `prefix` to (an empty prefix is the default namespace); `xml` is always bound. |
| `xml.Element.text` | `fn text(self): String` | The text of the text and CDATA children, joined. |
| `xml.Element.text_content` | `fn text_content(self): String` | The text of this element and all its descendants, in document order. |
| `xml.Element.child` | `fn child(self, name: str): Option<Element>` | The first child element whose qualified name is `name`: a copy. |
| `xml.Element.children_named` | `fn children_named(self, name: str): List<Element>` | The child elements whose qualified name is `name`: copies, in order. |
| `xml.Element.elements` | `fn elements(self): List<Element>` | Every child element: copies, in order. |
| `xml.Element.child_ns` | `fn child_ns(self, namespace: str, local: str): Option<Element>` | The first child element in the namespace `namespace` with the local name `local`: a copy. |
| `xml.Element.children_ns` | `fn children_ns(self, namespace: str, local: str): List<Element>` | The child elements in the namespace `namespace` with the local name `local`: copies, in order. |
| `xml.Element.find_ns` | `fn find_ns(self, namespace: str, local: str): Option<Element>` | The first element, this one or a descendant in document order, in the namespace `namespace` with the local name `local`: a copy. |
| `xml.Element.is_named` | `fn is_named(self, namespace: str, local: str): bool` | Whether this element is in the namespace `namespace` and has the local name `local`. |
| `xml.Element.copy` | `fn copy(self): Element` | A deep copy. |
| `xml.Element.to_xml` | `fn to_xml(self): Result<String, DeError>` | This element as compact XML (no declaration, no added white space). |
| `xml.Element.to_xml_pretty` | `fn to_xml_pretty(self): Result<String, DeError>` | This element as XML with two-space indentation: an element that has only element, comment, and instruction children puts each on its own line, and an element with text keeps its content as written. |
| `xml.Node` | `enum Node` | A child of an element. |
| `xml.Node.copy` | `fn copy(self): Node` | A deep copy. |
| `xml.Node.is_blank_text` | `fn is_blank_text(self): bool` | Whether this is a text node of white space only. |
| `xml.Document` | `struct Document` | A document: the optional declaration, the comments and instructions before the root, the root element, and the comments and instructions after it. |
| `xml.Document.new` | `fn new(own root: Element): Document` | A document with this root element, no declaration, and nothing before or after the root. |
| `xml.Document.with_declaration` | `fn with_declaration(own self): Document` | The same document with the declaration `<?xml version="1.0" encoding="UTF-8"?>`. |
| `xml.Document.to_xml` | `fn to_xml(self): Result<String, DeError>` | The document as compact XML: the declaration (if any) and the nodes directly one after the other. |
| `xml.Document.to_xml_pretty` | `fn to_xml_pretty(self): Result<String, DeError>` | The document as XML with two-space indentation (see `Element.to_xml_pretty`), one line for the declaration and each node outside the root, and a final newline. |
| `xml.Parser` | `struct Parser` | The XML reader: a copy of the input and the byte offset of the next unread byte. |
| `xml.Parser.new` | `fn new(text: str): Parser` | A reader over `text`. |
| `xml.Parser.parse_document` | `fn parse_document(mut self): Result<Document, DeError>` | Reads the whole text as one document. |
| `xml.Writer` | `struct Writer` | The format side of serialization (`Serializer`): it builds the DOM that the README's mapping describes. |
| `xml.Writer.new` | `fn new(): Writer` | A writer that names the root element after the value. |
| `xml.Writer.with_root` | `fn with_root(name: str): Writer` | A writer whose root element is called `name`. |
| `xml.Writer.finish` | `fn finish(own self): Result<Element, DeError>` | The root element that was written, ending the writer. |
| `xml.Reader` | `struct Reader` | The format side of deserialization (`Deserializer`) over a parsed document. |
| `xml.Reader.new` | `fn new(text: str): Result<Reader, DeError>` | A reader over the document in `text`: the text is parsed first, so a syntax error is reported here. |
| `xml.Reader.from_document` | `fn from_document(document: Document): Result<Reader, DeError>` | A reader over the root element of `document`. |
| `xml.parse` | `fn parse(text: str): Result<Document, DeError>` | Reads `text` as one XML document. |
| `xml.to_document` | `fn to_document<T: Serialize>(value: T): Result<Document, DeError>` | Writes `value` as a DOM: the tree the serialization mapping builds. |
| `xml.to_document_root` | `fn to_document_root<T: Serialize>(value: T, root: str): Result<Document, DeError>` | Like `to_document`, with the root element called `root`. |
| `xml.to_string` | `fn to_string<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as compact XML, with no declaration. |
| `xml.to_string_pretty` | `fn to_string_pretty<T: Serialize>(value: T): Result<String, DeError>` | Writes `value` as XML with two-space indentation and a final newline. |
| `xml.from_str` | `fn from_str<T: Deserialize>(text: str): Result<T, DeError>` | Reads a value of type `T` from the document in `text`. |
| `xml.from_document` | `fn from_document<T: Deserialize>(document: Document): Result<T, DeError>` | Reads a value of type `T` from the root element of a document. |
