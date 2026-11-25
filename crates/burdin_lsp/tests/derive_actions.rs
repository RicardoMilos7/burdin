//! The quick fixes that add a derive: E0141 for an unmet bound and E0130 for a derive requirement a field does not
//! meet (issue #1043; `spec/lsp.md` LS18). Each applied edit is sent back through the checker, and the error is gone.

mod common;

use burdin_lsp::json::Json;
use common::*;

/// A client with `body` as the text of `main.bd`, after the server has checked it.
fn open(body: &str) -> (Client, String, String) {
    let root = fixture("arms");
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let text = format!("{body}\nfn main() {{}}\n");
    client.change(&uri, 2, &text);
    (client, uri, text)
}

fn codes(client: &mut Client, uri: &str, version: i64) -> Vec<String> {
    let found = client.diagnostics(uri, |p| p.get("version").as_i64() == Some(version));
    found.iter().map(|d| d.get("code").as_str().unwrap().to_string()).filter(|code| !code.starts_with('L')).collect()
}

/// The quick fixes of the whole text.
fn quick_fixes(client: &mut Client, uri: &str, text: &str) -> Vec<Json> {
    let range = burdin_lsp::text::range_json(0, 0, text.lines().count() as u32 + 1, 0);
    let context =
        Json::obj().with("diagnostics", Json::Arr(Vec::new())).with("only", Json::Arr(vec![Json::from("quickfix")]));
    client.call("textDocument/codeAction", doc(uri).with("range", range).with("context", context)).as_array().to_vec()
}

/// The program after the only quick fix, which must have `title` and name one diagnostic; the result must check.
fn fixed_by(body: &str, code: &str, title: &str) -> String {
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), [code]);
    let fixes = quick_fixes(&mut client, &uri, &text);
    let titles: Vec<&str> = fixes.iter().map(|a| a.get("title").as_str().unwrap()).collect();
    assert_eq!(titles, [title]);
    assert_eq!(fixes[0].get("kind").as_str(), Some("quickfix"));
    assert_eq!(fixes[0].get("diagnostics").as_array().len(), 1);
    let changes = fixes[0].at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1);
    let fixed = apply(&text, changes[0].1.as_array(), client.encoding);
    client.change(&uri, 3, &fixed);
    assert_eq!(codes(&mut client, &uri, 3), Vec::<String>::new(), "{fixed}");
    assert_eq!(client.shutdown(), 0);
    fixed
}

const DUP: &str = "fn dup<T: Clone>(v: T): T {\n    v.clone()\n}\n\n";

#[test]
fn a_derive_is_added_to_an_existing_list() {
    let body =
        format!("struct Point: Eq {{\n    x: i32\n}}\n\n{DUP}fn go() {{\n    let p = dup(Point {{ x: 1 }})\n}}\n");
    let fixed = fixed_by(&body, "E0141", "derive `Clone` for `Point`");
    assert!(fixed.starts_with("struct Point: Eq, Clone {\n"), "{fixed}");
}

#[test]
fn a_list_is_made_for_a_type_with_none() {
    let body = format!("struct Point {{\n    x: i32\n}}\n\n{DUP}fn go() {{\n    let p = dup(Point {{ x: 1 }})\n}}\n");
    let fixed = fixed_by(&body, "E0141", "derive `Clone` for `Point`");
    assert!(fixed.starts_with("struct Point: Clone {\n"), "{fixed}");
}

#[test]
fn an_enum_gets_the_derive_too() {
    let body = format!("enum Mode {{\n    On\n    Off\n}}\n\n{DUP}fn go() {{\n    let m = dup(Mode.On)\n}}\n");
    let fixed = fixed_by(&body, "E0141", "derive `Clone` for `Mode`");
    assert!(fixed.starts_with("enum Mode: Clone {\n"), "{fixed}");
}

#[test]
fn a_generic_struct_keeps_its_type_parameters_before_the_list() {
    let body = format!(
        "struct Wrapper<T> {{\n    value: T\n}}\n\n{DUP}fn go() {{\n    let w = dup(Wrapper {{ value: 1 }})\n}}\n"
    );
    let fixed = fixed_by(&body, "E0141", "derive `Clone` for `Wrapper`");
    assert!(fixed.starts_with("struct Wrapper<T>: Clone {\n"), "{fixed}");
}

#[test]
fn ord_is_written_with_the_eq_it_comes_with() {
    let body = "struct Version {\n    n: i32\n}\n\nfn first<T: Ord>(a: T, b: T): bool {\n    a < b\n}\n\nfn go() {\n    let v = first(Version { n: 1 }, Version { n: 2 })\n}\n";
    let fixed = fixed_by(body, "E0141", "derive `Eq, Ord` for `Version`");
    assert!(fixed.starts_with("struct Version: Eq, Ord {\n"), "{fixed}");
}

#[test]
fn eq_already_written_is_not_written_again() {
    let body = "struct Version: Eq {\n    n: i32\n}\n\nfn first<T: Ord>(a: T, b: T): bool {\n    a < b\n}\n\nfn go() {\n    let v = first(Version { n: 1 }, Version { n: 2 })\n}\n";
    let fixed = fixed_by(body, "E0141", "derive `Ord` for `Version`");
    assert!(fixed.starts_with("struct Version: Eq, Ord {\n"), "{fixed}");
}

#[test]
fn copy_is_written_with_clone() {
    let body = "struct Pair {\n    n: i32\n}\n\nfn dup<T: Copy>(v: T): T {\n    v\n}\n\nfn go() {\n    let p = dup(Pair { n: 1 })\n}\n";
    let fixed = fixed_by(body, "E0141", "derive `Clone, Copy` for `Pair`");
    assert!(fixed.starts_with("struct Pair: Clone, Copy {\n"), "{fixed}");
}

#[test]
fn a_field_that_lacks_a_derive_is_fixed_at_its_type() {
    let body = "struct Inner {\n    n: i32\n}\n\nstruct Outer: Clone {\n    inner: Inner\n}\n";
    let fixed = fixed_by(body, "E0130", "derive `Clone` for `Inner`");
    assert!(fixed.starts_with("struct Inner: Clone {\n"), "{fixed}");
    assert!(fixed.contains("struct Outer: Clone {\n"), "{fixed}");
}

#[test]
fn a_type_outside_the_package_gets_no_derive() {
    // `String` is the standard library's, so its declaration is not the workspace's to change.
    let body = "fn dup<T: Copy>(v: T): T {\n    v\n}\n\nfn go() {\n    let s = dup(String.from(\"a\"))\n}\n";
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), ["E0141"]);
    assert!(quick_fixes(&mut client, &uri, &text).is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn an_interface_that_cannot_be_derived_gets_no_derive() {
    let body = "interface Shape {\n    fn area(self): i32\n}\n\nstruct Plain {\n    n: i32\n}\n\nfn report<T: Shape>(s: T) {\n}\n\nfn go() {\n    report(Plain { n: 1 })\n}\n";
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), ["E0141"]);
    assert!(quick_fixes(&mut client, &uri, &text).is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn a_type_in_another_module_is_fixed_in_its_own_file() {
    let root = fixture("arms");
    std::fs::write(root.join("src/shapes.bd"), "struct Dot {\n    n: i32\n}\n").unwrap();
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let text = format!("use shapes.Dot\n\n{DUP}fn go() {{\n    let d = dup(Dot {{ n: 1 }})\n}}\n\nfn main() {{}}\n");
    client.change(&uri, 2, &text);
    assert_eq!(codes(&mut client, &uri, 2), ["E0141"]);
    let fixes = quick_fixes(&mut client, &uri, &text);
    assert_eq!(fixes.len(), 1);
    assert_eq!(fixes[0].get("title").as_str(), Some("derive `Clone` for `Dot`"));
    let changes = fixes[0].at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].0.ends_with("src/shapes.bd"), "{}", changes[0].0);
    let shapes = read(&root.join("src/shapes.bd"));
    let fixed = apply(&shapes, changes[0].1.as_array(), client.encoding);
    assert_eq!(fixed, "struct Dot: Clone {\n    n: i32\n}\n");
    assert_eq!(client.shutdown(), 0);
}
