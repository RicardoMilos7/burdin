//! A leading dot in the editor (`spec/lsp.md`, LS8 and LS12; `spec/target-typing.md`): `.Green`, `.new(1, 2)`, and
//! `.{ x: 3 }` answer definition, hover, references, and completion the way their explicit spellings do, and
//! completion after a bare dot offers the members of the type the position expects.

mod common;

use burdin_lsp::json::Json;
use common::*;

/// The `leading` fixture's `main.bd`, open and analyzed without errors.
fn open() -> (Client, String, String, std::path::PathBuf) {
    let root = fixture("leading");
    let mut client = Client::new(&[&root]);
    let path = root.join("src/main.bd");
    let (uri, text) = client.open(&path);
    let diagnostics = client.diagnostics(&uri, |_| true);
    assert!(diagnostics.is_empty(), "{:?}", diagnostics.iter().map(Json::to_text).collect::<Vec<_>>());
    (client, uri, text, root)
}

/// `main.bd:line:text` of a location, the text it covers.
fn place(client: &Client, location: &Json) -> String {
    let uri = location.get("uri").as_str().or_else(|| location.get("targetUri").as_str()).unwrap();
    let range =
        if location.get("range").is_null() { location.get("targetSelectionRange") } else { location.get("range") };
    let path = burdin_lsp::text::uri_to_path(uri).unwrap();
    let text = read(&path);
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    format!("{file}:{}:{}", line_char(range.get("start")).0 + 1, slice(&text, range, client.encoding))
}

fn definition(client: &mut Client, uri: &str, text: &str, needle: &str, nth: usize, delta: usize) -> String {
    let result = client.call("textDocument/definition", client.at(uri, text, needle, nth, delta));
    if result.is_null() { "none".to_string() } else { place(client, &result) }
}

fn hover(client: &mut Client, uri: &str, text: &str, needle: &str, delta: usize) -> String {
    let result = client.call("textDocument/hover", client.at(uri, text, needle, 0, delta));
    result.get("contents").get("value").as_str().unwrap_or("none").to_string()
}

#[test]
fn leading_names_define_like_their_explicit_spelling() {
    // LS8: `.Green` is `Color.Green`, `.new(1, 2)` is a method of `Point`, and `.{ x: 3 }`'s `x` is the field of `Point`.
    let (mut client, uri, text, _root) = open();
    let explicit = definition(&mut client, &uri, &text, "= Color.Green", 0, 8);
    assert_eq!(explicit, "main.bd:3:Green");
    assert_eq!(definition(&mut client, &uri, &text, "= .Green", 0, 3), explicit);
    assert_eq!(definition(&mut client, &uri, &text, "= .new(1, 2)", 0, 3), "main.bd:12:new");
    let field = definition(&mut client, &uri, &text, "Point { x: 5", 0, 8);
    assert_eq!(field, "main.bd:7:x");
    assert_eq!(definition(&mut client, &uri, &text, "= .{ x: 3", 0, 5), field);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn leading_names_hover_like_their_explicit_spelling() {
    // LS11: the hover of `.Green` shows the variant as `Color.Green` does, with the range of the name.
    let (mut client, uri, text, _root) = open();
    let explicit = hover(&mut client, &uri, &text, "= Color.Green", 8);
    assert!(explicit.contains("variant of `Color`"), "{explicit}");
    assert_eq!(hover(&mut client, &uri, &text, "= .Green", 3), explicit);
    assert!(hover(&mut client, &uri, &text, "= .new(1, 2)", 3).contains("method of `Point`"));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn references_and_rename_include_a_leading_name() {
    // LS9: the variant's references include `.Green`, so a rename changes it too.
    let (mut client, uri, text, _root) = open();
    let mut params = client.at(&uri, &text, "    Green\n}", 0, 4);
    params.set("context", Json::obj().with("includeDeclaration", true));
    let refs = client.call("textDocument/references", params);
    let starts: Vec<(u32, u32)> = refs.as_array().iter().map(|l| line_char(l.get("range").get("start"))).collect();
    let leading = client.pos(&text, "= .Green", 0, 3);
    assert!(starts.contains(&line_char(&leading)), "{starts:?} lacks the leading name at {leading:?}");
    let explicit = client.pos(&text, "= Color.Green", 0, 8);
    assert!(starts.contains(&line_char(&explicit)), "{starts:?} lacks the explicit name");
    assert_eq!(client.shutdown(), 0);
}

/// Opens the fixture, replaces `from` with `to`, and returns the completion labels at `needle` (`delta` bytes in).
fn labels_after(from: &str, to: &str, needle: &str, delta: usize) -> Vec<String> {
    let (mut client, uri, text, _root) = open();
    let edited = text.replacen(from, to, 1);
    assert_ne!(edited, text, "the edit did not apply: {from:?}");
    client.change(&uri, 2, &edited);
    let params = client.at(&uri, &edited, needle, 0, delta);
    let items = client.call("textDocument/completion", params).get("items").as_array().to_vec();
    // The document is what the client sent again: a name outside the edited line still answers (`Point` in
    // `Point { x: 5 }` is the struct's declaration).
    let check = definition(&mut client, &uri, &edited, "Point { x: 5", 0, 0);
    assert_eq!(check, "main.bd:6:Point", "the document is back to its text after completion");
    assert_eq!(client.shutdown(), 0);
    items.iter().map(|i| i.get("label").as_str().unwrap().to_string()).collect()
}

#[test]
fn a_bare_dot_completes_the_variants_of_its_let_annotation() {
    // LS12: `let leading: Color = .` offers the variants of `Color`, and no other name.
    let labels = labels_after("    let leading: Color = .Green\n", "    let leading: Color = .\n", "= .\n", 3);
    assert!(labels.contains(&"Red".to_string()) && labels.contains(&"Green".to_string()), "{labels:?}");
    assert!(!labels.contains(&"name".to_string()) && !labels.contains(&"new".to_string()), "{labels:?}");
}

#[test]
fn a_bare_dot_completes_the_argument_type_of_its_call() {
    // LS12: `name(.` takes the parameter's type, `Color`, through the call's parentheses the editor closed.
    let labels = labels_after("println(name(.Red))", "println(name(.))", "name(.)", 6);
    assert!(labels.contains(&"Red".to_string()) && labels.contains(&"Green".to_string()), "{labels:?}");
}

#[test]
fn a_bare_dot_completes_the_associated_functions_of_a_struct() {
    // LS12: `let p: Point = .` offers `new` (an associated function), not the fields.
    let labels = labels_after("    let p: Point = .new(1, 2)\n", "    let p: Point = .\n", "= .\n", 3);
    assert!(labels.contains(&"new".to_string()), "{labels:?}");
    assert!(!labels.contains(&"x".to_string()) && !labels.contains(&"Red".to_string()), "{labels:?}");
}

#[test]
fn a_bare_dot_completes_the_prelude_constructors_of_an_option() {
    // LS12: `let o: Option<i32> = .` offers `Some` and `None`.
    let labels = labels_after(
        "    let explicit = Color.Green\n",
        "    let explicit = Color.Green\n    let o: Option<i32> = .\n",
        "Option<i32> = .\n",
        "Option<i32> = .".len(),
    );
    assert!(labels.contains(&"Some".to_string()) && labels.contains(&"None".to_string()), "{labels:?}");
}

#[test]
fn a_typed_prefix_after_a_leading_dot_filters_the_same_members() {
    // LS12: `.Gr` is completed like `.`: the prefix is the name typed, and the expected type is still `Color`.
    let labels = labels_after("    let leading: Color = .Green\n", "    let leading: Color = .Gr\n", "= .Gr\n", 5);
    assert!(labels.contains(&"Green".to_string()), "{labels:?}");
}

#[test]
fn a_bare_dot_with_no_expected_type_offers_nothing() {
    // TT3: `let untyped = .` has no type to give its name, so there is nothing to offer.
    let labels = labels_after(
        "    let explicit = Color.Green\n",
        "    let explicit = Color.Green\n    let untyped = .\n",
        "untyped = .\n",
        "untyped = .".len(),
    );
    assert!(labels.is_empty(), "{labels:?}");
}

#[test]
fn a_field_name_in_a_struct_literal_completes_the_fields_it_has_not_written() {
    // LS12: `Point {` offers the fields of `Point` the literal has not written (`y`, not `x`), then the names in scope
    // for shorthand (`explicit`). The same holds for the explicit spelling as for `.{` below.
    let labels = labels_after(
        "    let explicit = Color.Green\n",
        "    let explicit = Color.Green\n    let s = Point {\n        x: 1,\n    }\n",
        "x: 1,\n",
        6,
    );
    assert!(labels.contains(&"y".to_string()) && labels.contains(&"explicit".to_string()), "{labels:?}");
    assert!(!labels.contains(&"x".to_string()), "{labels:?}");
}

#[test]
fn a_field_name_in_a_leading_struct_literal_completes_the_fields_of_its_expected_type() {
    // LS12: `.{` takes the struct its position expects, `Point`, and offers its fields the literal has not written.
    let labels = labels_after(
        "    let q: Point = .{ x: 3, y: 4 }\n",
        "    let q: Point = .{\n        x: 3,\n    }\n",
        "x: 3,\n",
        6,
    );
    assert!(labels.contains(&"y".to_string()), "{labels:?}");
    assert!(!labels.contains(&"x".to_string()), "{labels:?}");
}
