//! The code actions that write the arms of a `match`: the missing arms of an E0006, and a `_` arm expanded into the
//! cases it stands for (issues #1040 and #1041; `spec/lsp.md` LS18). Each applied edit is sent back through the
//! checker.

mod common;

use burdin_lsp::json::Json;
use common::*;

const COLOR: &str = "enum Color: Copy, Clone {
    Red
    Green
    Blue
}

";

/// A client with `body` as the text of `main.bd`, after the server has checked it. Returns the URI too.
fn open(body: &str) -> (Client, String, String) {
    let root = fixture("arms");
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let text = format!("{body}\nfn main() {{}}\n");
    client.change(&uri, 2, &text);
    (client, uri, text)
}

/// The codes of the errors (not the lints) of the text with the given version.
fn codes(client: &mut Client, uri: &str, version: i64) -> Vec<String> {
    let found = client.diagnostics(uri, |p| p.get("version").as_i64() == Some(version));
    found.iter().map(|d| d.get("code").as_str().unwrap().to_string()).filter(|code| !code.starts_with('L')).collect()
}

/// The code actions of `kind` where `needle` first appears in the text.
fn actions_at(client: &mut Client, uri: &str, text: &str, needle: &str, kind: &str) -> Vec<Json> {
    let at = client.pos(text, needle, 0, 0);
    let range = Json::obj().with("start", at.clone()).with("end", at);
    let context =
        Json::obj().with("diagnostics", Json::Arr(Vec::new())).with("only", Json::Arr(vec![Json::from(kind)]));
    client.call("textDocument/codeAction", doc(uri).with("range", range).with("context", context)).as_array().to_vec()
}

fn titles(actions: &[Json]) -> Vec<&str> {
    actions.iter().map(|a| a.get("title").as_str().unwrap()).collect()
}

/// The text after the only action there is, which must name `title`.
fn apply_only(client: &Client, text: &str, actions: &[Json], title: &str) -> String {
    assert_eq!(titles(actions), [title]);
    let changes = actions[0].at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1);
    apply(text, changes[0].1.as_array(), client.encoding)
}

/// Checks that `fixed` is a program with no error, and returns the client.
fn assert_checks(client: &mut Client, uri: &str, fixed: &str) {
    client.change(uri, 3, fixed);
    assert_eq!(codes(client, uri, 3), Vec::<String>::new(), "{fixed}");
}

/// A `match` with no `_` arm: E0006 at the `match`, one quick fix, and the result checks.
fn fill(body: &str) -> (String, String) {
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), ["E0006"]);
    let found = actions_at(&mut client, &uri, &text, "match", "quickfix");
    let fixed = apply_only(&client, &text, &found, "add the missing match arms");
    assert_eq!(found[0].get("kind").as_str(), Some("quickfix"));
    assert_eq!(found[0].get("diagnostics").as_array().len(), 1);
    assert_checks(&mut client, &uri, &fixed);
    assert_eq!(client.shutdown(), 0);
    (text, fixed)
}

#[test]
fn the_missing_arms_of_a_plain_enum_follow_the_declaration() {
    let body = format!("{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Red => 1,\n    }}\n}}\n");
    let (_, fixed) = fill(&body);
    assert!(
        fixed.contains("        Color.Red => 1,\n        Color.Green => todo(),\n        Color.Blue => todo(),\n    }"),
        "{fixed}"
    );
}

#[test]
fn a_last_arm_without_a_comma_gets_one() {
    let body = format!("{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Blue => 3\n    }}\n}}\n");
    let (_, fixed) = fill(&body);
    assert!(
        fixed.contains("        Color.Blue => 3,\n        Color.Red => todo(),\n        Color.Green => todo(),\n    }"),
        "{fixed}"
    );
}

#[test]
fn a_block_arm_needs_no_comma_before_the_new_arms() {
    let body = format!("{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Blue => {{ 3 }}\n    }}\n}}\n");
    let (_, fixed) = fill(&body);
    assert!(fixed.contains("        Color.Blue => { 3 }\n        Color.Red => todo(),\n"), "{fixed}");
}

#[test]
fn a_match_with_no_arms_gets_all_of_them() {
    let body = format!("{COLOR}fn code(c: Color): i32 {{\n    match c {{}}\n}}\n");
    let (_, fixed) = fill(&body);
    assert!(
        fixed.contains(
            "    match c {\n        Color.Red => todo(),\n        Color.Green => todo(),\n        Color.Blue => todo(),\n    }"
        ),
        "{fixed}"
    );
}

#[test]
fn the_missing_arms_of_a_payload_enum_bind_wildcards() {
    let body = "enum Shape {
    Dot
    Circle(i32)
    Rect { w: i32, h: i32 }
}

fn area(s: Shape): i32 {
    match s {
        Shape.Dot => 0,
    }
}
";
    let (_, fixed) = fill(body);
    assert!(
        fixed.contains(
            "        Shape.Dot => 0,\n        Shape.Circle(_) => todo(),\n        Shape.Rect { w: _, h: _ } => todo(),\n"
        ),
        "{fixed}"
    );
}

#[test]
fn the_missing_arm_of_an_option_is_none() {
    let body = "fn get(o: Option<i32>): i32 {\n    match o {\n        Some(v) => v,\n    }\n}\n";
    let (_, fixed) = fill(body);
    assert!(fixed.contains("        Some(v) => v,\n        None => todo(),\n    }"), "{fixed}");

    let body = "fn get(o: Option<i32>): i32 {\n    match o {\n        None => 0,\n    }\n}\n";
    let (_, fixed) = fill(body);
    assert!(fixed.contains("        None => 0,\n        Some(_) => todo(),\n    }"), "{fixed}");
}

#[test]
fn the_missing_arm_of_a_result_is_err() {
    let body = "fn get(r: Result<i32, i32>): i32 {\n    match r {\n        Ok(v) => v,\n    }\n}\n";
    let (_, fixed) = fill(body);
    assert!(fixed.contains("        Ok(v) => v,\n        Err(_) => todo(),\n    }"), "{fixed}");
}

#[test]
fn a_tuple_of_enums_lists_the_uncovered_pairs() {
    let body = format!(
        "{COLOR}fn code(a: Color, b: Color): i32 {{\n    match (a, b) {{\n        (Color.Red, Color.Red) => 1,\n    }}\n}}\n"
    );
    let (_, fixed) = fill(&body);
    assert!(
        fixed.contains(
            "        (Color.Red, Color.Red) => 1,\n        (Color.Red, Color.Green) => todo(),\n        (Color.Red, Color.Blue) => todo(),\n        (Color.Green, _) => todo(),\n        (Color.Blue, _) => todo(),\n    }"
        ),
        "{fixed}"
    );
}

#[test]
fn a_nested_payload_names_the_uncovered_inner_case() {
    let body = format!(
        "{COLOR}fn code(o: Option<Color>): i32 {{\n    match o {{\n        None => 0,\n        Some(Color.Red) => 1,\n    }}\n}}\n"
    );
    let (_, fixed) = fill(&body);
    assert!(
        fixed.contains("        Some(Color.Red) => 1,\n        Some(Color.Green) => todo(),\n        Some(Color.Blue) => todo(),\n    }"),
        "{fixed}"
    );
}

#[test]
fn an_arm_with_a_guard_does_not_cover_its_pattern() {
    let body = format!(
        "{COLOR}fn code(c: Color, big: bool): i32 {{\n    match c {{\n        Color.Red if big => 1,\n        Color.Green => 2,\n        Color.Blue => 3,\n    }}\n}}\n"
    );
    let (_, fixed) = fill(&body);
    assert!(fixed.contains("        Color.Blue => 3,\n        Color.Red => todo(),\n    }"), "{fixed}");
}

#[test]
fn an_open_domain_gets_one_wildcard_arm() {
    let body = "fn code(n: i32): i32 {\n    match n {\n        0 => 1,\n        1 => 2,\n    }\n}\n";
    let (_, fixed) = fill(body);
    assert!(fixed.contains("        1 => 2,\n        _ => todo(),\n    }"), "{fixed}");

    let body = "fn code(s: str): i32 {\n    match s {\n        \"a\" => 1,\n    }\n}\n";
    let (_, fixed) = fill(body);
    assert!(fixed.contains("        \"a\" => 1,\n        _ => todo(),\n    }"), "{fixed}");
}

// ---- expanding a `_` arm ----------------------------------------------------------------------------------------

/// The `refactor.rewrite` actions where `needle` first appears.
fn rewrites(client: &mut Client, uri: &str, text: &str, needle: &str) -> Vec<Json> {
    actions_at(client, uri, text, needle, "refactor.rewrite")
        .into_iter()
        .filter(|a| a.get("title").as_str().is_some_and(|t| t.starts_with("expand")))
        .collect()
}

/// A `match` with a `_` arm: the expansion of the arm under `_ =>`, which checks and has no wildcard left.
fn expand(body: &str) -> String {
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    let found = rewrites(&mut client, &uri, &text, "_ =>");
    assert_eq!(found[0].get("kind").as_str(), Some("refactor.rewrite"));
    let fixed = apply_only(&client, &text, &found, "expand `_` into the remaining cases");
    assert_checks(&mut client, &uri, &fixed);
    assert_eq!(client.shutdown(), 0);
    fixed
}

/// No expansion is offered under the `_ =>` of `body`, which checks.
fn no_expansion(body: &str, needle: &str) {
    let (mut client, uri, text) = open(body);
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    assert!(rewrites(&mut client, &uri, &text, needle).is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn a_wildcard_arm_of_a_plain_enum_becomes_the_remaining_variants() {
    let body = format!(
        "{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Red => 1,\n        _ => 0,\n    }}\n}}\n"
    );
    let fixed = expand(&body);
    assert!(
        fixed.contains("        Color.Red => 1,\n        Color.Green => 0,\n        Color.Blue => 0,\n    }"),
        "{fixed}"
    );
}

#[test]
fn the_body_is_copied_whole_into_each_arm() {
    let body = format!(
        "{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Green => 1,\n        _ => {{\n            let n = 4\n            n * 2\n        }}\n    }}\n}}\n"
    );
    let fixed = expand(&body);
    let block = "{\n            let n = 4\n            n * 2\n        }";
    assert!(fixed.contains(&format!("        Color.Red => {block}\n        Color.Blue => {block}\n    }}")), "{fixed}");
}

#[test]
fn a_wildcard_arm_of_a_payload_enum_uses_wildcard_payloads() {
    let body = "enum Shape {
    Dot
    Circle(i32)
    Rect { w: i32, h: i32 }
}

fn area(s: Shape): i32 {
    match s {
        Shape.Dot => 0,
        _ => 1,
    }
}
";
    let fixed = expand(body);
    assert!(
        fixed.contains(
            "        Shape.Dot => 0,\n        Shape.Circle(_) => 1,\n        Shape.Rect { w: _, h: _ } => 1,\n    }"
        ),
        "{fixed}"
    );
}

#[test]
fn a_wildcard_arm_of_an_option_or_a_bool_is_the_other_case() {
    let fixed =
        expand("fn get(o: Option<i32>): i32 {\n    match o {\n        Some(v) => v,\n        _ => 0,\n    }\n}\n");
    assert!(fixed.contains("        Some(v) => v,\n        None => 0,\n    }"), "{fixed}");

    let fixed = expand("fn get(b: bool): i32 {\n    match b {\n        true => 1,\n        _ => 0,\n    }\n}\n");
    assert!(fixed.contains("        true => 1,\n        false => 0,\n    }"), "{fixed}");
}

#[test]
fn a_wildcard_in_a_tuple_expands_to_the_pairs_it_covers() {
    let body = format!(
        "{COLOR}fn code(a: Color, b: bool): i32 {{\n    match (a, b) {{\n        (Color.Red, _) => 1,\n        _ => 0,\n    }}\n}}\n"
    );
    let fixed = expand(&body);
    assert!(
        fixed.contains(
            "        (Color.Red, _) => 1,\n        (Color.Green, _) => 0,\n        (Color.Blue, _) => 0,\n    }"
        ),
        "{fixed}"
    );
}

#[test]
fn an_else_arm_expands_like_a_wildcard() {
    let body = format!(
        "{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        Color.Red => 1,\n        else => 0,\n    }}\n}}\n"
    );
    let (mut client, uri, text) = open(&body);
    let found = rewrites(&mut client, &uri, &text, "else =>");
    let fixed = apply_only(&client, &text, &found, "expand `else` into the remaining cases");
    assert!(
        fixed.contains("        Color.Red => 1,\n        Color.Green => 0,\n        Color.Blue => 0,\n    }"),
        "{fixed}"
    );
    assert_checks(&mut client, &uri, &fixed);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn an_arm_with_a_guard_is_not_expanded() {
    let body = format!(
        "{COLOR}fn code(c: Color, big: bool): i32 {{\n    match c {{\n        _ if big => 2,\n        Color.Red => 1,\n        Color.Green => 3,\n        Color.Blue => 4,\n    }}\n}}\n"
    );
    no_expansion(&body, "_ if");
}

#[test]
fn an_integer_or_string_wildcard_is_not_expanded() {
    no_expansion("fn code(n: i32): i32 {\n    match n {\n        0 => 1,\n        _ => 0,\n    }\n}\n", "_ =>");
    no_expansion("fn code(s: str): i32 {\n    match s {\n        \"a\" => 1,\n        _ => 0,\n    }\n}\n", "_ =>");
    // A match on a tuple of integers has no list of cases either.
    no_expansion(
        "fn code(a: i32, b: i32): i32 {\n    match (a, b) {\n        (0, 0) => 1,\n        _ => 0,\n    }\n}\n",
        "_ =>",
    );
}

#[test]
fn a_wildcard_with_nothing_before_it_is_not_expanded_beyond_the_variants() {
    // The first arm is the wildcard: it stands for every variant.
    let body = format!("{COLOR}fn code(c: Color): i32 {{\n    match c {{\n        _ => 0,\n    }}\n}}\n");
    let fixed = expand(&body);
    assert!(
        fixed.contains(
            "    match c {\n        Color.Red => 0,\n        Color.Green => 0,\n        Color.Blue => 0,\n    }"
        ),
        "{fixed}"
    );
}

#[test]
fn the_cursor_inside_the_body_offers_the_arm_it_is_in_not_the_wildcard_around_it() {
    let body = format!(
        "{COLOR}fn code(c: Color, d: Color): i32 {{\n    match c {{\n        Color.Red => 1,\n        _ => match d {{\n            Color.Red => 2,\n            _ => 3,\n        }}\n    }}\n}}\n"
    );
    let (mut client, uri, text) = open(&body);
    assert_eq!(codes(&mut client, &uri, 2), Vec::<String>::new());
    // On the inner `_ =>` only the inner match is expanded.
    let found = rewrites(&mut client, &uri, &text, "_ => 3");
    let fixed = apply_only(&client, &text, &found, "expand `_` into the remaining cases");
    assert!(fixed.contains("            Color.Green => 3,\n            Color.Blue => 3,\n"), "{fixed}");
    assert!(fixed.contains("        _ => match d {"), "{fixed}");
    assert_checks(&mut client, &uri, &fixed);
    assert_eq!(client.shutdown(), 0);
}
