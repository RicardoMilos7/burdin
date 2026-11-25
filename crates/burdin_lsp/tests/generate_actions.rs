//! The actions that write code the compiler does not suggest: the missing fields of a struct literal, a function or a
//! method that a call needs, and an interface implementation (issue #1044; `spec/lsp.md` LS18). Each applied edit is
//! sent back through the checker and the error is gone, and the result is what `burdin fmt` would write.

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

fn actions(client: &mut Client, uri: &str, text: &str, kind: &str, range: Json) -> Vec<Json> {
    let _ = text;
    let context =
        Json::obj().with("diagnostics", Json::Arr(Vec::new())).with("only", Json::Arr(vec![Json::from(kind)]));
    client.call("textDocument/codeAction", doc(uri).with("range", range).with("context", context)).as_array().to_vec()
}

fn whole(text: &str) -> Json {
    burdin_lsp::text::range_json(0, 0, text.lines().count() as u32 + 1, 0)
}

fn burdin_fmt_text(text: &str) -> String {
    let groups =
        burdin_fmt::UseGroups { std_modules: burdin_query::package::std_module_names(), dependencies: Vec::new() };
    burdin_fmt::format_source(text, &burdin_fmt::Options { groups }).expect("the program parses")
}

/// Applies the action `title` of kind `kind`, checks the program, and checks that the formatter leaves it alone.
fn apply_action(mut client: Client, uri: &str, text: &str, kind: &str, title: &str, range: Json) -> String {
    let found = actions(&mut client, uri, text, kind, range);
    let titles: Vec<&str> = found.iter().map(|a| a.get("title").as_str().unwrap()).collect();
    let action = found.iter().find(|a| a.get("title").as_str() == Some(title)).unwrap_or_else(|| {
        panic!("no action `{title}` among {titles:?}");
    });
    assert_eq!(action.get("kind").as_str(), Some(kind));
    let changes = action.at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1);
    let fixed = apply(text, changes[0].1.as_array(), client.encoding);
    client.change(uri, 3, &fixed);
    assert_eq!(codes(&mut client, uri, 3), Vec::<String>::new(), "{fixed}");
    assert_eq!(burdin_fmt_text(&fixed), fixed, "the result is not formatted");
    assert_eq!(client.shutdown(), 0);
    fixed
}

/// The program after the quick fix `title` for the one error `code` in `body`.
fn fixed_by(body: &str, code: &str, title: &str) -> String {
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), [code]);
    apply_action(client, &uri, &text, "quickfix", title, whole(&text))
}

const POINT: &str = "struct Point {
    x: i32,
    y: i32,
    z: i32,
}

";

// ---- missing fields -----------------------------------------------------------------------------------------------

#[test]
fn a_literal_with_missing_fields_gets_them_in_declaration_order() {
    let body = format!("{POINT}fn make(): Point {{\n    Point {{ y: 2 }}\n}}\n");
    let fixed = fixed_by(&body, "E0003", "add the missing fields");
    assert!(fixed.contains("    Point { y: 2, x: todo(), z: todo() }\n"), "{fixed}");
}

#[test]
fn a_literal_with_no_fields_gets_all_of_them() {
    let body = format!("{POINT}fn make(): Point {{\n    Point {{}}\n}}\n");
    let fixed = fixed_by(&body, "E0003", "add the missing fields");
    assert!(fixed.contains("    Point { x: todo(), y: todo(), z: todo() }\n"), "{fixed}");
}

#[test]
fn a_multi_line_literal_gets_one_field_per_line_with_the_trailing_comma() {
    let body = format!("{POINT}fn make(): Point {{\n    Point {{\n        x: 1,\n        y: 2,\n    }}\n}}\n");
    let fixed = fixed_by(&body, "E0003", "add the missing fields");
    assert!(fixed.contains("        x: 1,\n        y: 2,\n        z: todo(),\n    }\n"), "{fixed}");
}

#[test]
fn a_target_literal_gets_the_fields_too() {
    let body = format!("{POINT}fn make(): Point {{\n    let p: Point = .{{ x: 1, y: 2 }}\n    p\n}}\n");
    let fixed = fixed_by(&body, "E0003", "add the missing fields");
    assert!(fixed.contains("let p: Point = .{ x: 1, y: 2, z: todo() }\n"), "{fixed}");
}

#[test]
fn a_struct_variant_gets_its_missing_fields() {
    let body =
        "enum Shape {\n    Rect { w: i32, h: i32 },\n    Dot,\n}\n\nfn make(): Shape {\n    Shape.Rect { w: 1 }\n}\n";
    let fixed = fixed_by(body, "E0003", "add the missing fields");
    assert!(fixed.contains("    Shape.Rect { w: 1, h: todo() }\n"), "{fixed}");
}

// ---- a missing function -------------------------------------------------------------------------------------------

const RECT: &str = "struct Rect {
    w: i32,
    h: i32,
}

";

#[test]
fn a_called_function_is_created_after_its_caller_with_the_argument_types() {
    let body = format!("{RECT}fn go(r: Rect) {{\n    area(r)\n}}\n");
    let fixed = fixed_by(&body, "E0002", "create function `area`");
    assert!(fixed.contains("fn go(r: Rect) {\n    area(r)\n}\n\nfn area(r: Rect) {\n    todo()\n}\n"), "{fixed}");
}

#[test]
fn an_argument_that_is_not_a_name_is_called_argn_and_the_let_type_is_the_return_type() {
    let body = format!("{RECT}fn go(r: Rect) {{\n    let a: i32 = area(r, 2, \"x\")\n}}\n");
    let fixed = fixed_by(&body, "E0002", "create function `area`");
    assert!(fixed.contains("fn area(r: Rect, arg2: i32, arg3: str): i32 {\n    todo()\n}\n"), "{fixed}");
}

#[test]
fn a_call_without_arguments_makes_a_function_without_parameters() {
    let body = "fn go() {\n    setup()\n}\n";
    let fixed = fixed_by(body, "E0002", "create function `setup`");
    assert!(fixed.contains("fn go() {\n    setup()\n}\n\nfn setup() {\n    todo()\n}\n"), "{fixed}");
}

#[test]
fn a_function_called_from_a_method_goes_after_the_whole_impl_block() {
    let body = format!(
        "{RECT}Rect {{\n    fn size(self): i32 {{\n        let n: i32 = measure(self.w)\n        n\n    }}\n}}\n"
    );
    let fixed = fixed_by(&body, "E0002", "create function `measure`");
    assert!(fixed.contains("    }\n}\n\nfn measure(arg1: i32): i32 {\n    todo()\n}\n"), "{fixed}");
}

// ---- a missing method ---------------------------------------------------------------------------------------------

#[test]
fn a_method_is_added_in_a_new_impl_block_after_the_type() {
    let body = format!("{RECT}fn go(r: Rect) {{\n    r.grow(2)\n}}\n");
    let fixed = fixed_by(&body, "E0002", "create method `grow` on `Rect`");
    assert!(
        fixed.contains("}\n\nRect {\n    fn grow(self, arg1: i32) {\n        todo()\n    }\n}\n\nfn go"),
        "{fixed}"
    );
}

#[test]
fn a_method_is_added_to_the_impl_block_the_type_has() {
    let body = format!(
        "{RECT}Rect {{\n    fn area(self): i32 {{\n        self.w * self.h\n    }}\n}}\n\nfn go(r: Rect) {{\n    let n: i32 = r.perimeter()\n}}\n"
    );
    let fixed = fixed_by(&body, "E0002", "create method `perimeter` on `Rect`");
    assert!(
        fixed.contains("        self.w * self.h\n    }\n\n    fn perimeter(self): i32 {\n        todo()\n    }\n}\n"),
        "{fixed}"
    );
}

#[test]
fn a_method_of_a_generic_type_keeps_the_type_parameters() {
    let body = "struct Wrapper<T> {\n    value: T,\n}\n\nfn go(w: Wrapper<i32>) {\n    w.reset()\n}\n";
    let fixed = fixed_by(body, "E0002", "create method `reset` on `Wrapper`");
    assert!(fixed.contains("}\n\nWrapper<T> {\n    fn reset(self) {\n        todo()\n    }\n}\n"), "{fixed}");
}

#[test]
fn a_method_on_a_receiver_that_is_a_chain_is_typed_from_its_arguments() {
    let body = format!("{RECT}struct Holder {{\n    r: Rect,\n}}\n\nfn go(h: Holder) {{\n    h.r.scale(h.r.w)\n}}\n");
    let fixed = fixed_by(&body, "E0002", "create method `scale` on `Rect`");
    assert!(fixed.contains("Rect {\n    fn scale(self, arg1: i32) {\n        todo()\n    }\n}\n"), "{fixed}");
}

#[test]
fn a_method_of_a_type_in_another_module_is_written_in_its_own_file() {
    let root = fixture("arms");
    std::fs::write(root.join("src/shapes.bd"), "struct Dot {\n    n: i32,\n}\n").unwrap();
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let text = "use shapes.Dot\n\nfn go(d: Dot) {\n    d.bump()\n}\n\nfn main() {}\n";
    client.change(&uri, 2, text);
    assert_eq!(codes(&mut client, &uri, 2), ["E0002"]);
    let found = actions(&mut client, &uri, text, "quickfix", whole(text));
    let action = found.iter().find(|a| a.get("title").as_str() == Some("create method `bump` on `Dot`")).unwrap();
    let changes = action.at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].0.ends_with("src/shapes.bd"), "{}", changes[0].0);
    let fixed = apply(&read(&root.join("src/shapes.bd")), changes[0].1.as_array(), client.encoding);
    assert_eq!(fixed, "struct Dot {\n    n: i32,\n}\n\nDot {\n    fn bump(self) {\n        todo()\n    }\n}\n");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn a_method_of_a_standard_type_gets_no_action() {
    let (mut client, uri, text) = open("fn go(s: String) {\n    s.shout()\n}\n");
    assert_eq!(codes(&mut client, &uri, 2), ["E0002"]);
    let found = actions(&mut client, &uri, &text, "quickfix", whole(&text));
    assert!(found.iter().all(|a| !a.get("title").as_str().unwrap().starts_with("create method")));
    assert_eq!(client.shutdown(), 0);
}

// ---- implementing an interface ------------------------------------------------------------------------------------

const SHAPE: &str = "interface Shape {\n    fn area(self): i32\n    fn ready(self): bool\n    fn double(self): i32 {\n        self.area() * 2\n    }\n}\n\n";

/// The range of the first occurrence of `word` in `text`.
fn range_of(text: &str, word: &str) -> Json {
    let at = text.find(word).unwrap();
    let line = text[..at].matches('\n').count() as u32;
    let column = (at - text[..at].rfind('\n').map_or(0, |n| n + 1)) as u32;
    burdin_lsp::text::range_json(line, column, line, column + 1)
}

#[test]
fn an_interface_is_implemented_with_the_methods_that_have_no_default() {
    let (mut client, uri, text) = open(&format!("{SHAPE}struct Square {{\n    side: i32,\n}}\n"));
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    let range = range_of(&text, "Square");
    let fixed = apply_action(client, &uri, &text, "refactor.rewrite", "implement `Shape` for `Square`", range);
    assert!(
        fixed.contains(
            "struct Square {\n    side: i32,\n}\n\nSquare: Shape {\n    fn area(self): i32 {\n        todo()\n    }\n\n    fn ready(self): bool {\n        todo()\n    }\n}\n"
        ),
        "{fixed}"
    );
}

#[test]
fn a_generic_type_is_implemented_too() {
    let (mut client, uri, text) = open(&format!("{SHAPE}struct Boxed<T> {{\n    inner: T,\n}}\n"));
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    let range = range_of(&text, "Boxed");
    let fixed = apply_action(client, &uri, &text, "refactor.rewrite", "implement `Shape` for `Boxed`", range);
    assert!(fixed.contains("\n\nBoxed<T>: Shape {\n    fn area(self): i32 {"), "{fixed}");
}

#[test]
fn an_interface_the_type_already_implements_is_not_offered() {
    let body = format!(
        "{SHAPE}struct Square {{\n    side: i32,\n}}\n\nSquare: Shape {{\n    fn area(self): i32 {{\n        1\n    }}\n\n    fn ready(self): bool {{\n        true\n    }}\n}}\n"
    );
    let (mut client, uri, text) = open(&body);
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    let found = actions(&mut client, &uri, &text, "refactor.rewrite", range_of(&text, "Square"));
    assert!(found.iter().all(|a| !a.get("title").as_str().unwrap().starts_with("implement")));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn an_interface_with_only_default_methods_is_not_offered() {
    let body =
        "interface Named {\n    fn double(self): i32 {\n        2\n    }\n}\n\nstruct Square {\n    side: i32,\n}\n";
    let (mut client, uri, text) = open(body);
    let found = actions(&mut client, &uri, &text, "refactor.rewrite", range_of(&text, "Square"));
    assert!(found.iter().all(|a| !a.get("title").as_str().unwrap().starts_with("implement")));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn the_cursor_away_from_the_type_name_offers_nothing() {
    let (mut client, uri, text) = open(&format!("{SHAPE}struct Square {{\n    side: i32,\n}}\n"));
    let found = actions(&mut client, &uri, &text, "refactor.rewrite", range_of(&text, "side"));
    assert!(found.iter().all(|a| !a.get("title").as_str().unwrap().starts_with("implement")));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn an_interface_of_another_module_brings_its_use() {
    let root = fixture("arms");
    std::fs::write(root.join("src/shapes.bd"), SHAPE).unwrap();
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let text = "struct Square {\n    side: i32,\n}\n\nfn main() {}\n";
    client.change(&uri, 2, text);
    let fixed = apply_action(
        client,
        &uri,
        text,
        "refactor.rewrite",
        "implement `Shape` for `Square`",
        range_of(text, "Square"),
    );
    assert!(fixed.starts_with("use shapes.Shape\n\nstruct Square {"), "{fixed}");
}
