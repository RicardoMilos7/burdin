//! Navigation (issues #311, #370, #470; `spec/lsp.md` LS8-LS10): definition, references, highlights, rename,
//! implementations, and the call and type hierarchies.

mod common;

use burdin_lsp::json::Json;
use common::*;
use std::path::{Path, PathBuf};

struct Geometry {
    root: PathBuf,
    client: Client,
    main: (String, String),
    shapes: (String, String),
}

fn geometry() -> Geometry {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let main = client.open(&root.join("src/main.bd"));
    let shapes = client.open(&root.join("src/shapes.bd"));
    Geometry { root, client, main, shapes }
}

/// `file:line:text` of a location, with the path relative to the fixture.
fn place(g: &Geometry, location: &Json) -> String {
    let uri = location.get("uri").as_str().or_else(|| location.get("targetUri").as_str()).unwrap();
    let range =
        if location.get("range").is_null() { location.get("targetSelectionRange") } else { location.get("range") };
    let path = burdin_lsp::text::uri_to_path(uri).unwrap();
    let text = read(&path);
    let file = relative(&g.root, &path);
    format!("{file}:{}:{}", line_char(range.get("start")).0 + 1, slice(&text, range, g.client.encoding))
}

fn relative(root: &Path, path: &Path) -> String {
    let root = burdin_query::package::source_path_text(root);
    let path = burdin_query::package::source_path_text(path);
    match path.strip_prefix(&root) {
        Some(rest) => rest.trim_start_matches(['/', '\\']).replace('\\', "/"),
        None => format!("std/{}", path.rsplit(['/', '\\']).next().unwrap()),
    }
}

fn definition(g: &mut Geometry, in_shapes: bool, needle: &str, nth: usize, delta: usize) -> String {
    let (uri, text) = if in_shapes { g.shapes.clone() } else { g.main.clone() };
    let result = g.client.call("textDocument/definition", g.client.at(&uri, &text, needle, nth, delta));
    if result.is_null() { "none".to_string() } else { place(g, &result) }
}

#[test]
fn definition_finds_functions_types_methods_fields_locals_and_modules() {
    let mut g = geometry();
    assert_eq!(definition(&mut g, false, "add(a, b)", 0, 0), "src/math.bd:3:add");
    assert_eq!(definition(&mut g, false, "use math.add", 0, 9), "src/math.bd:3:add");
    assert_eq!(definition(&mut g, false, "use math", 0, 4), "src/math.bd:1:");
    assert_eq!(definition(&mut g, false, "Square {", 0, 0), "src/shapes.bd:7:Square");
    assert_eq!(definition(&mut g, false, "sq.area", 0, 3), "src/shapes.bd:23:area");
    assert_eq!(definition(&mut g, false, "r.area", 0, 2), "src/shapes.bd:29:area");
    assert_eq!(definition(&mut g, false, "side: 4", 0, 0), "src/shapes.bd:8:side");
    assert_eq!(definition(&mut g, false, "Rect.new", 0, 5), "src/shapes.bd:17:new");
    assert_eq!(definition(&mut g, false, "{label}", 0, 1), "src/main.bd:10:label");
    assert_eq!(definition(&mut g, false, "add(a, b)", 0, 4), "src/main.bd:5:a");
    assert_eq!(definition(&mut g, true, "self.w", 0, 5), "src/shapes.bd:12:w");
    assert_eq!(definition(&mut g, true, "Color.Green", 0, 6), "src/shapes.bd:36:Green");
    assert_eq!(definition(&mut g, true, "Rect { w, h }", 0, 7), "src/shapes.bd:17:w", "shorthand names the parameter");
    // The standard library is opened from a materialized copy.
    assert_eq!(definition(&mut g, false, "assert_eq", 0, 0), "std/std.bd:257:assert_eq");
    // Nothing to find on a keyword, a literal, or a comment.
    assert_eq!(definition(&mut g, false, "fn main", 0, 0), "none");
    assert_eq!(definition(&mut g, false, "\"área\"", 0, 2), "none");
    assert_eq!(definition(&mut g, false, "Adds two", 0, 0), "none");
    assert_eq!(g.client.shutdown(), 0);
}

#[test]
fn references_and_highlights() {
    let mut g = geometry();
    let (uri, text) = g.main.clone();
    let mut params = g.client.at(&uri, &text, "fn total", 0, 3);
    params.set("context", Json::obj().with("includeDeclaration", true));
    let result = g.client.call("textDocument/references", params.clone());
    let places: Vec<String> = result.as_array().iter().map(|l| place(&g, l)).collect();
    assert_eq!(places, ["src/main.bd:5:total", "src/main.bd:13:total", "src/main.bd:19:total"]);
    params.set("context", Json::obj().with("includeDeclaration", false));
    assert_eq!(g.client.call("textDocument/references", params).as_array().len(), 2);

    // A method through its interface: every implementation's uses are separate symbols.
    let mut params = g.client.at(&uri, &text, "sq.area", 0, 3);
    params.set("context", Json::obj().with("includeDeclaration", true));
    let result = g.client.call("textDocument/references", params);
    let places: Vec<String> = result.as_array().iter().map(|l| place(&g, l)).collect();
    assert_eq!(places, ["src/main.bd:13:area", "src/main.bd:24:area", "src/shapes.bd:23:area"]);

    // Highlights: a write is kind 3, a read kind 2.
    let result = g.client.call("textDocument/documentHighlight", g.client.at(&uri, &text, "{sum}", 0, 1));
    let kinds: Vec<(u32, i64)> = result
        .as_array()
        .iter()
        .map(|h| (line_char(h.at(&["range", "start"])).0 + 1, h.get("kind").as_i64().unwrap()))
        .collect();
    assert_eq!(kinds, [(13, 3), (14, 3), (14, 2), (15, 2)]);
    assert_eq!(g.client.shutdown(), 0);
}

fn rename(g: &mut Geometry, in_shapes: bool, needle: &str, delta: usize, new_name: &str) -> Json {
    let (uri, text) = if in_shapes { g.shapes.clone() } else { g.main.clone() };
    let params = g.client.at(&uri, &text, needle, 0, delta).with("newName", new_name);
    g.client.request("textDocument/rename", params)
}

/// The texts after applying a workspace edit, by relative path.
fn applied(g: &Geometry, edit: &Json) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (uri, edits) in edit.get("changes").as_object() {
        let path = burdin_lsp::text::uri_to_path(uri).unwrap();
        out.push((relative(&g.root, &path), apply(&read(&path), edits.as_array(), g.client.encoding)));
    }
    out.sort();
    out
}

#[test]
fn rename_changes_every_use_across_files() {
    let mut g = geometry();
    let response = rename(&mut g, true, "struct Square", 7, "Box");
    let files = applied(&g, response.get("result"));
    assert_eq!(files.len(), 2);
    assert!(files[0].1.contains("use shapes.{Rect, Box}") && files[0].1.contains("let s = Box { side: 3 }"));
    assert!(files[1].1.contains("struct Box {") && files[1].1.contains("Box: Shape {"));
    assert!(!files.iter().any(|(_, t)| t.contains("Square")));

    // A field written in shorthand keeps the local's name.
    let response = rename(&mut g, true, "w: i32", 0, "width");
    let files = applied(&g, response.get("result"));
    assert!(files[0].1.contains("Rect { width: w, h }") && files[0].1.contains("self.width * self.h"));
    // A local written in shorthand keeps the field's name.
    let response = rename(&mut g, true, "fn new(w", 7, "wide");
    let files = applied(&g, response.get("result"));
    assert!(files[0].1.contains("fn new(wide: i32, h: i32)") && files[0].1.contains("Rect { w: wide, h }"));

    // prepareRename gives the name's range, and refuses what cannot be renamed.
    let (uri, text) = g.main.clone();
    let prepared = g.client.call("textDocument/prepareRename", g.client.at(&uri, &text, "total(sq", 0, 2));
    assert_eq!(slice(&text, prepared.get("range"), g.client.encoding), "total");
    assert_eq!(prepared.get("placeholder").as_str(), Some("total"));
    for (needle, delta) in [("println", 0), ("fn main", 0), ("test \"total", 0)] {
        let prepared = g.client.request("textDocument/prepareRename", g.client.at(&uri, &text, needle, 0, delta));
        assert!(
            prepared.get("result").is_null() || !prepared.get("error").is_null(),
            "{needle}: {}",
            prepared.to_text()
        );
    }
    // A name that is not an identifier, or that is a keyword, is an error.
    for bad in ["two words", "fn", "1x"] {
        let response = rename(&mut g, false, "fn total", 3, bad);
        assert_eq!(response.at(&["error", "code"]).as_i64(), Some(burdin_lsp::rpc::code::INVALID_PARAMS), "{bad}");
    }
    // The standard library is not renamed.
    let response = rename(&mut g, false, "assert_eq", 0, "check");
    assert!(!response.get("error").is_null(), "{}", response.to_text());
    assert_eq!(g.client.shutdown(), 0);
}

#[test]
fn implementations_of_interfaces_their_methods_and_types() {
    let mut g = geometry();
    let (uri, text) = g.shapes.clone();
    let places = |g: &mut Geometry, needle: &str, delta: usize| -> Vec<String> {
        let result = g.client.call("textDocument/implementation", g.client.at(&uri, &text, needle, 0, delta));
        result.as_array().iter().map(|l| place(g, l)).collect()
    };
    assert_eq!(places(&mut g, "fn area(self): i32\n", 3), ["src/shapes.bd:23:area", "src/shapes.bd:29:area"]);
    assert_eq!(places(&mut g, "interface Shape", 10), ["src/shapes.bd:22:Square", "src/shapes.bd:28:Rect"]);
    assert_eq!(places(&mut g, "struct Rect", 7), ["src/shapes.bd:16:Rect", "src/shapes.bd:28:Rect"]);
    // From an implementation back to the interface's declaration.
    assert_eq!(places(&mut g, "fn area(self): i32 {", 3), ["src/shapes.bd:3:area"]);
    assert_eq!(g.client.shutdown(), 0);
}

#[test]
fn call_hierarchy_in_both_directions() {
    let mut g = geometry();
    let math = g.client.open(&g.root.join("src/math.bd"));
    let items = g.client.call("textDocument/prepareCallHierarchy", g.client.at(&math.0, &math.1, "fn add", 0, 3));
    let add = items.as_array()[0].clone();
    assert_eq!(add.get("detail").as_str(), Some("fn add(a: i32, b: i32): i32"));
    let incoming = g.client.call("callHierarchy/incomingCalls", Json::obj().with("item", add));
    let callers: Vec<(String, usize)> = incoming
        .as_array()
        .iter()
        .map(|c| (c.at(&["from", "name"]).as_str().unwrap().to_string(), c.get("fromRanges").as_array().len()))
        .collect();
    assert_eq!(callers, [("total".to_string(), 1), ("main".to_string(), 1)]);

    let (uri, text) = g.main.clone();
    let items = g.client.call("textDocument/prepareCallHierarchy", g.client.at(&uri, &text, "fn main", 0, 3));
    let outgoing = g.client.call("callHierarchy/outgoingCalls", Json::obj().with("item", items.as_array()[0].clone()));
    let mut callees: Vec<String> =
        outgoing.as_array().iter().map(|c| c.at(&["to", "name"]).as_str().unwrap().to_string()).collect();
    callees.sort();
    // Only calls the source writes, to functions with a declaration: not the formatting calls of the interpolated
    // string, and not the built-in `println`.
    assert_eq!(callees, ["add", "area", "area", "new", "total"]);
    for call in outgoing.as_array() {
        for range in call.get("fromRanges").as_array() {
            assert_eq!(slice(&text, range, g.client.encoding), call.at(&["to", "name"]).as_str().unwrap());
        }
    }
    // A test is a caller too.
    let items = g.client.call("textDocument/prepareCallHierarchy", g.client.at(&uri, &text, "fn total", 0, 3));
    let incoming = g.client.call("callHierarchy/incomingCalls", Json::obj().with("item", items.as_array()[0].clone()));
    let mut callers: Vec<String> =
        incoming.as_array().iter().map(|c| c.at(&["from", "name"]).as_str().unwrap().to_string()).collect();
    callers.sort();
    assert_eq!(callers, ["main", "total adds"]);
    assert_eq!(g.client.shutdown(), 0);
}

#[test]
fn type_hierarchy_between_interfaces_and_types() {
    let mut g = geometry();
    let (uri, text) = g.shapes.clone();
    let items = g.client.call("textDocument/prepareTypeHierarchy", g.client.at(&uri, &text, "interface Shape", 0, 10));
    let shape = items.as_array()[0].clone();
    let subtypes = g.client.call("typeHierarchy/subtypes", Json::obj().with("item", shape.clone()));
    let names: Vec<&str> = subtypes.as_array().iter().map(|t| t.get("name").as_str().unwrap()).collect();
    assert_eq!(names, ["Square", "Rect"]);
    let supertypes =
        g.client.call("typeHierarchy/supertypes", Json::obj().with("item", subtypes.as_array()[0].clone()));
    let names: Vec<&str> = supertypes.as_array().iter().map(|t| t.get("name").as_str().unwrap()).collect();
    assert_eq!(names, ["Shape"]);
    // Not a type: no hierarchy.
    let none = g.client.call("textDocument/prepareTypeHierarchy", g.client.at(&uri, &text, "fn favorite", 0, 3));
    assert!(none.is_null());
    golden("type-hierarchy", &normalize(&Json::Arr(vec![shape, subtypes]), &g.root));
    assert_eq!(g.client.shutdown(), 0);
}

/// Definition on the callee of a call that is still being written resolves to its declaration (issue #851; LS9): a
/// method named with missing arguments, with an unclosed list, or without a call, a method called on `self`, and the
/// same from inside a method. Each case writes its call into the file, so the call's text is the file's last.
#[test]
fn definition_resolves_the_callee_of_an_incomplete_call() {
    let root = fixture("incomplete");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let mut g = Geometry { root, client, main: (uri.clone(), text.clone()), shapes: (uri.clone(), text.clone()) };
    // The receiver is declared first and analyzed, so the calls written after it are the only text that is new.
    let declared = text.replacen("    println", "    let mut a = List<i32>.new()\n    println", 1);
    let mut version = 2;
    g.client.change(&uri, version, &declared);
    g.client.diagnostics(&uri, |_| true);
    g.main = (uri.clone(), declared.clone());
    let mut ask = |g: &mut Geometry, base: &str, anchor: &str, replacement: &str, callee: &str| -> String {
        version += 1;
        let edited = base.replacen(anchor, replacement, 1);
        let at = edited.find(replacement.trim_start()).unwrap() + replacement.trim_start().find(callee).unwrap() + 1;
        g.client.change(&uri, version, &edited);
        let before = &edited[..at];
        let line = before.matches('\n').count() as i64;
        let character = (before.len() - before.rfind('\n').map_or(0, |n| n + 1)) as i64;
        let params = Json::obj()
            .with("textDocument", Json::obj().with("uri", uri.as_str()))
            .with("position", Json::obj().with("line", line).with("character", character));
        let result = g.client.call("textDocument/definition", params);
        g.main = (uri.clone(), edited);
        if result.is_null() { "none".to_string() } else { place(g, &result) }
    };
    // `append` is a method of the standard library's `List`: the built-ins' materialized copy.
    let list = "    println";
    for call in ["    a.append()\n    println", "    a.append(\n    println", "    a.append(List<i32>.\n    println"] {
        let got = ask(&mut g, &declared, list, call, "append");
        assert!(got.starts_with("std/builtins.bd:") && got.ends_with(":append"), "`{call}`: {got}");
    }
    // A function of the file, named with an unclosed list: `sq.grow(` in `main`.
    let grow_line = declared.lines().position(|l| l.contains("fn grow")).unwrap() + 1;
    for call in ["    sq.grow(\n    println", "    sq.grow\n    println"] {
        let got = ask(&mut g, &declared, list, call, "grow");
        assert_eq!(got, format!("src/main.bd:{grow_line}:grow"), "`{call}`");
    }
    // A method called on `self` from inside another method, with the call unfinished, unclosed, or without its call.
    let area_line = declared.lines().position(|l| l.contains("fn area")).unwrap() + 1;
    let inside = "        self.side = self.side + by\n";
    for (call, callee) in [
        ("        self.area()\n", "area"),
        ("        self.area(\n", "area"),
        ("        let z = self.area\n", "area"),
        ("        self.grow(\n", "grow"),
    ] {
        let replacement = format!("{inside}{call}");
        let got = ask(&mut g, &declared, inside, &replacement, callee);
        let line = if callee == "area" { area_line } else { grow_line };
        assert_eq!(got, format!("src/main.bd:{line}:{callee}"), "`{}`", call.trim());
    }
    assert_eq!(g.client.shutdown(), 0);
}
