//! Editing help (issues #312, #313, #368, #369; `spec/lsp.md` LS11-LS15): hover, completion, signature help, inlay
//! hints, semantic tokens, and formatting through `burdin fmt`.

mod common;

use burdin_lsp::json::Json;
use common::*;

fn open_geometry() -> (std::path::PathBuf, Client, String, String) {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    (root, client, uri, text)
}

#[test]
fn hover_shows_signatures_types_and_docs() {
    let (_root, mut client, uri, text) = open_geometry();
    let mut hover = |needle: &str, delta: usize| -> String {
        let result = client.call("textDocument/hover", client.at(&uri, &text, needle, 0, delta));
        if result.is_null() {
            "none".to_string()
        } else {
            result.at(&["contents", "value"]).as_str().unwrap().to_string()
        }
    };
    let total = hover("total(sq", 0);
    assert!(total.contains("fn total(a: i32, b: i32): i32"), "{total}");
    assert!(total.contains("Adds two areas."), "{total}");
    let sq = hover("sq.area", 0);
    assert!(sq.contains("let sq: Square"), "{sq}");
    let area = hover("sq.area", 3);
    assert!(area.contains("fn area(self): i32"), "{area}");
    let square = hover("Square {", 0);
    assert!(square.contains("struct Square") && square.contains("A square."), "{square}");
    let side = hover("side: 4", 0);
    assert!(side.contains("side: i32"), "{side}");
    assert_eq!(hover("fn main", 0), "none");
    // A literal shows its type.
    assert!(hover("\"área\"", 1).contains("str"));
    assert_eq!(client.shutdown(), 0);
}

/// Hover on the callee of a call that is still being written (issue #848): a method named without its call, a call
/// whose arguments are not closed yet, a module's function named without arguments. The signature shows, never
/// `{error}`.
#[test]
fn hover_shows_the_callee_of_an_incomplete_call() {
    let root = fixture("incomplete");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    // Each case writes `let q = <expr>` before the `println` of `main`, then hovers the callee's name in `<expr>`.
    let cases: &[(&str, &str, &str)] = &[
        ("sq.area", "area", "fn area(self): i32"),
        ("sq.area(", "area", "fn area(self): i32"),
        ("sq.area()", "area", "fn area(self): i32"),
        ("sq.grow", "grow", "fn grow(mut self, by: i32)"),
        ("sq.grow(", "grow", "fn grow(mut self, by: i32)"),
        ("Square.area", "area", "fn area(self): i32"),
        ("Square.area(", "area", "fn area(self): i32"),
        ("json.from_str", "from_str", "fn from_str<T: Deserialize>(text: str): Result<T, DeError>"),
        ("json.from_str(", "from_str", "fn from_str<T: Deserialize>(text: str): Result<T, DeError>"),
        ("add", "add", "fn add(a: i32, b: i32): i32"),
        ("add(1, ", "add", "fn add(a: i32, b: i32): i32"),
    ];
    for (version, (expr, callee, expected)) in (2..).zip(cases) {
        let edited = text.replace("    println", &format!("    let q = {expr}\n    println"));
        client.change(&uri, version, &edited);
        let needle = format!("let q = {expr}");
        let delta = "let q = ".len() + expr.find(callee).unwrap();
        let result = client.call("textDocument/hover", client.at(&uri, &edited, &needle, 0, delta));
        assert!(!result.is_null(), "no hover on `{callee}` in `{expr}`");
        let markdown = result.at(&["contents", "value"]).as_str().unwrap().to_string();
        assert!(markdown.contains(expected), "`{expr}`: {markdown}");
        assert!(!markdown.contains("{error}"), "`{expr}`: {markdown}");
    }
    assert_eq!(client.shutdown(), 0);
}

/// Hover on a method called through a nested receiver (issue #880; `spec/lsp.md` LS11): `resp.body.read_to_string()`
/// shows the method's signature, as a one-level call does, while the receivers keep their types.
#[test]
fn hover_on_a_method_of_a_nested_receiver_shows_its_signature() {
    let root = fixture("nested");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let hover = |client: &mut Client, needle: &str, delta: usize| -> String {
        let result = client.call("textDocument/hover", client.at(&uri, &text, needle, 0, delta));
        assert!(!result.is_null(), "no hover at `{needle}` + {delta}");
        result.at(&["contents", "value"]).as_str().unwrap().to_string()
    };
    let nested = "let body = resp.body.read_to_string()";
    let method = hover(&mut client, nested, "let body = resp.body.".len());
    assert!(method.contains("fn read_to_string(self): Option<String>"), "{method}");
    assert!(method.contains("Reads the whole body."), "{method}");
    let receiver = hover(&mut client, nested, "let body = ".len());
    assert!(receiver.contains("Response"), "{receiver}");
    let field = hover(&mut client, nested, "let body = resp.".len());
    assert!(field.contains("body: Body"), "{field}");
    let one = hover(&mut client, "let one = resp.read_to_string()", "let one = resp.".len());
    assert!(one.contains("fn read_to_string(self): Option<String>"), "{one}");
    assert_eq!(client.shutdown(), 0);
}

fn labels(result: &Json) -> Vec<String> {
    let items = if result.get("items").is_null() { result.as_array() } else { result.get("items").as_array() };
    items.iter().map(|i| i.get("label").as_str().unwrap().to_string()).collect()
}

#[test]
fn completion_offers_members_names_in_scope_and_imports() {
    let (root, mut client, uri, text) = open_geometry();
    // Members after a dot, even while the line does not parse yet: from the last analysis that had a program.
    client.diagnostics(&uri, |_| true);
    let edited = text.replace("    sum = add(sum, 1)\n", "    sum = add(sum, 1)\n    sq.\n");
    client.change(&uri, 2, &edited);
    let result = client.call("textDocument/completion", client.at(&uri, &edited, "    sq.\n", 0, 7));
    assert_eq!(labels(&result), ["side", "area"]);
    let r = client.call("textDocument/completion", client.at(&uri, &edited, "Rect.new", 0, 5));
    assert!(labels(&r).contains(&"new".to_string()));
    // Names in scope, locals first.
    let result = client.call("textDocument/completion", client.at(&uri, &edited, "    sum = add", 0, 4));
    let names = labels(&result);
    let position = |n: &str| names.iter().position(|x| x == n).unwrap_or_else(|| panic!("{n} missing: {names:?}"));
    assert!(position("sq") < position("total") && position("total") < position("println"));
    assert!(names.contains(&"Square".to_string()) && names.contains(&"let".to_string()));
    assert!(!names.contains(&"mul".to_string()), "a function not imported is not in scope");
    // A module's items in a `use`.
    let with_use = format!("use math.\n{edited}");
    client.change(&uri, 3, &with_use);
    let result = client.call("textDocument/completion", client.at(&uri, &with_use, "use math.\n", 0, 9));
    assert_eq!(labels(&result), ["add", "mul"]);
    let _ = root;
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn signature_help_marks_the_active_parameter() {
    let (_root, mut client, uri, text) = open_geometry();
    let result = client.call("textDocument/signatureHelp", client.at(&uri, &text, "add(sum, 1)", 0, 9));
    let signature = &result.get("signatures").as_array()[0];
    assert_eq!(signature.get("label").as_str(), Some("fn add(a: i32, b: i32): i32"));
    assert_eq!(result.get("activeParameter").as_i64(), Some(1));
    let first = client.call("textDocument/signatureHelp", client.at(&uri, &text, "add(sum, 1)", 0, 4));
    assert_eq!(first.get("activeParameter").as_i64(), Some(0));
    // A method call does not count `self`.
    let method = client.call("textDocument/signatureHelp", client.at(&uri, &text, "Rect.new(2, 3)", 0, 12));
    assert_eq!(method.get("signatures").as_array()[0].get("label").as_str(), Some("fn new(w: i32, h: i32): Rect"));
    assert_eq!(method.get("activeParameter").as_i64(), Some(1));
    // Outside a call: nothing.
    let none = client.call("textDocument/signatureHelp", client.at(&uri, &text, "fn main", 0, 0));
    assert!(none.is_null());
    assert_eq!(client.shutdown(), 0);
}

fn whole(text: &str) -> Json {
    let lines = text.lines().count() as u32;
    burdin_lsp::text::range_json(0, 0, lines + 1, 0)
}

#[test]
fn inlay_hints_for_types_and_parameter_names_can_be_turned_off() {
    let (root, mut client, uri, text) = open_geometry();
    let params = doc(&uri).with("range", whole(&text));
    let hints = client.call("textDocument/inlayHint", params.clone());
    golden("inlay-hints", &normalize(&hints, &root));
    let shown: Vec<String> = hints.as_array().iter().map(|h| h.get("label").as_str().unwrap().to_string()).collect();
    assert!(shown.contains(&": Square".to_string()) && shown.contains(&"a:".to_string()));

    let settings = Json::obj().with("burdin", Json::obj().with("inlayHints", Json::obj().with("types", false)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    let hints = client.call("textDocument/inlayHint", params.clone());
    assert!(hints.as_array().iter().all(|h| h.get("kind").as_i64() == Some(2)), "only parameter names are left");
    assert!(client.server_requests.iter().any(|r| r.get("method").as_str() == Some("workspace/inlayHint/refresh")));
    let settings =
        Json::obj().with("burdin", Json::obj().with("inlayHints", Json::obj().with("parameterNames", false)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    assert!(client.call("textDocument/inlayHint", params).as_array().is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn the_ownership_lens_shows_moves_and_drops_and_a_hover_lists_them() {
    let root = fixture("ownership");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let params = doc(&uri).with("range", whole(&text));
    // The lens is off by default (`spec/lsp.md`, LS5/LS13).
    let off = client.call("textDocument/inlayHint", params.clone());
    assert!(
        off.as_array().iter().all(|hint| {
            let label = hint.get("label").as_str().unwrap_or("");
            !label.starts_with("move ") && !label.starts_with("drop ")
        }),
        "the ownership lens is off by default: {}",
        off.to_text()
    );

    let settings = Json::obj().with("burdin", Json::obj().with("inlayHints", Json::obj().with("ownership", true)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    let hints = client.call("textDocument/inlayHint", params);
    golden("ownership-hints", &normalize(&hints, &root));
    let labels: Vec<String> =
        hints.as_array().iter().map(|hint| hint.get("label").as_str().unwrap_or("").to_string()).collect();
    assert!(labels.contains(&"move s -> t".to_string()), "{labels:?}");
    assert!(labels.iter().any(|label| label == "drop u"), "{labels:?}");

    // A hover on the local lists the events the checker recorded for it.
    let hover = client.call("textDocument/hover", client.at(&uri, &text, "println(u)", 0, 8));
    let value = hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string();
    assert!(value.contains("**Ownership**"), "{value}");
    assert!(value.contains("borrowed (read)"), "{value}");
    assert!(value.contains("dropped"), "{value}");
    golden("ownership-hover", &normalize(&hover, &root));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn semantic_tokens_cover_the_file_and_a_range() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/shapes.bd"));
    let full = client.call("textDocument/semanticTokens/full", doc(&uri));
    let data: Vec<i64> = full.get("data").as_array().iter().map(|n| n.as_i64().unwrap()).collect();
    assert_eq!(data.len() % 5, 0);
    // Decode into `line:text:type` lines, which read better in a golden file than the numbers.
    let types = burdin_lsp::TOKEN_TYPES;
    let lines: Vec<&str> = text.lines().collect();
    let (mut line, mut start) = (0usize, 0usize);
    let mut decoded = Vec::new();
    for chunk in data.chunks(5) {
        if chunk[0] > 0 {
            line += chunk[0] as usize;
            start = chunk[1] as usize;
        } else {
            start += chunk[1] as usize;
        }
        let utf16: Vec<u16> = lines[line].encode_utf16().collect();
        let word = String::from_utf16(&utf16[start..start + chunk[2] as usize]).unwrap();
        decoded.push(Json::from(format!("{}:{}:{}:{}", line + 1, word, types[chunk[3] as usize], chunk[4])));
    }
    golden("semantic-tokens", &Json::Arr(decoded));

    let range = burdin_lsp::text::range_json(21, 0, 26, 0);
    let part = client.call("textDocument/semanticTokens/range", doc(&uri).with("range", range));
    let part: Vec<i64> = part.get("data").as_array().iter().map(|n| n.as_i64().unwrap()).collect();
    assert!(!part.is_empty() && part.len() < data.len());
    assert_eq!(part[0], 21, "the first token of the range is on line 22 (`Square: Shape`)");
    assert_eq!(client.shutdown(), 0);
}

fn options() -> Json {
    Json::obj().with("tabSize", 4).with("insertSpaces", true)
}

#[test]
fn formatting_matches_burdin_fmt_and_refuses_a_file_that_does_not_parse() {
    let (_root, mut client, uri, text) = open_geometry();
    let edits = client.call("textDocument/formatting", doc(&uri).with("options", options()));
    let formatted = apply(&text, edits.as_array(), client.encoding);
    assert_eq!(formatted, burdin_fmt_text(&text));
    assert!(formatted.starts_with("use math.add\nuse shapes.{Rect, Square}\n"), "imports are sorted");
    assert_eq!(edits.as_array().len(), 1, "one minimal edit");
    // An already formatted file: no edits.
    client.change(&uri, 2, &formatted);
    let none = client.call("textDocument/formatting", doc(&uri).with("options", options()));
    assert!(none.as_array().is_empty());

    // Range formatting changes only the lines asked for.
    let messy = formatted
        .replace("    let sq = Square { side: 4 }", "    let sq   =  Square{side:4}")
        .replace("    a + b", "    a+b");
    let messy = messy.replace("    add(a, b)", "    add( a,b )");
    client.change(&uri, 3, &messy);
    let line = messy.lines().position(|l| l.contains("Square{side:4}")).unwrap() as u32;
    let range = burdin_lsp::text::range_json(line, 0, line, 10);
    let edits = client.call("textDocument/rangeFormatting", doc(&uri).with("range", range).with("options", options()));
    let result = apply(&messy, edits.as_array(), client.encoding);
    assert!(result.contains("    let sq = Square { side: 4 }"));
    assert!(result.contains("    add( a,b )"), "a line outside the range is left alone");

    // A syntax error: no edits rather than a mangled file.
    let broken = formatted.replace("fn main() {", "fn main( {");
    client.change(&uri, 4, &broken);
    let response = client.request("textDocument/formatting", doc(&uri).with("options", options()));
    assert!(response.get("error").is_null() && response.get("result").is_null(), "{}", response.to_text());

    // Formatting turned off: no edits.
    client.change(&uri, 5, &messy);
    let settings = Json::obj().with("burdin", Json::obj().with("formatting", Json::obj().with("enabled", false)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    let off = client.call("textDocument/formatting", doc(&uri).with("options", options()));
    assert!(off.is_null() || off.as_array().is_empty());
    assert_eq!(client.shutdown(), 0);
}

/// What `burdin fmt` makes of a text: the same library and options the command uses.
fn burdin_fmt_text(text: &str) -> String {
    let groups =
        burdin_fmt::UseGroups { std_modules: burdin_query::package::std_module_names(), dependencies: Vec::new() };
    burdin_fmt::format_source(text, &burdin_fmt::Options { groups }).expect("the fixture parses")
}
