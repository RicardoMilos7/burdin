//! Reparsing one function body (`spec/query.md`, QL6): after an edit inside a body, the memoized syntax tree is
//! the tree a full parse of the new text gives — every span included — and an edit the body-only path cannot
//! handle is reparsed in full.

mod common;

use burdin_query::Db;
use common::*;

/// The syntax tree a full parse of the file's current text gives, printed.
fn full_parse(db: &Db, path: &str) -> String {
    let id = db.file_id(path).expect("the file is open");
    let text = db.file_text(path).expect("the file is open");
    let mut diags = burdin_diag::Diagnostics::default();
    let ast = burdin_syntax::parse(id, text, &mut diags);
    assert!(!diags.has_errors(), "the edited text should parse:\n{text}");
    format!("{ast:?}")
}

/// The memoized tree of the file, printed the same way.
fn memo_parse(db: &mut Db, path: &str) -> String {
    let parsed = db.syntax(path).expect("the file is open");
    assert!(parsed.diags.is_empty(), "the memoized tree has syntax errors");
    format!("{:?}", parsed.ast)
}

#[test]
fn a_body_edit_reparses_the_body_to_the_same_tree() {
    let mut db = open();
    db.analyze().unwrap();
    // The counters accumulate until the next analysis, so the test watches how much they grow.
    let parsed_before = db.stats().files_parsed.len();
    let mut reparsed_before = db.stats().bodies_reparsed.len();
    let edits: &[(&str, &str, &str)] = &[
        // A changed expression in a function of the entry file (not `main`, whose body the parser rewrites).
        ("main.bd", "add(a, b)", "add(a, b) + 0"),
        ("main.bd", "mul(n, 2)", "mul(n, 2) + 0"),
        // A comment inside a body.
        ("report.bd", "let label = \"area\"", "let label = \"area\"\n    // note"),
        // More lines than before, which moves everything after the body.
        ("report.bd", "let label = \"area\"", "let label = \"area\"\n    let more = label\n    let more = more"),
        // A body in another file.
        ("math.bd", "a + b", "a + b\n    // sum"),
        ("math.bd", "a * b", "let product = a * b\n    product"),
        // A method body.
        ("geometry/shapes.bd", "self.w * self.h", "self.w * self.h + 0"),
    ];
    for &(path, from, to) in edits {
        let before = db.file_text(path).expect("the file is open").to_string();
        db.set_file(path, &replace(&before, from, to));
        let memo = memo_parse(&mut db, path);
        let full = full_parse(&db, path);
        assert_eq!(memo, full, "after replacing `{from}` with `{to}` in {path}");
        assert_eq!(db.stats().files_parsed.len(), parsed_before, "{} was parsed in full: {:?}", path, db.stats());
        assert!(
            db.stats().bodies_reparsed.len() > reparsed_before,
            "{} was not reparsed by body: {:?}",
            path,
            db.stats()
        );
        reparsed_before = db.stats().bodies_reparsed.len();
    }
}

#[test]
fn an_edit_the_body_path_cannot_handle_is_parsed_in_full() {
    let mut db = open();
    db.analyze().unwrap();
    let mut parsed_before = db.stats().files_parsed.len();
    // A signature change is not inside a body.
    db.set_file("math.bd", &replace(MATH, "fn mul(a: i32, b: i32)", "fn mul(a: i64, b: i32)"));
    let memo = memo_parse(&mut db, "math.bd");
    assert_eq!(memo, full_parse(&db, "math.bd"));
    assert!(db.stats().files_parsed[parsed_before..].contains(&"math.bd".to_string()), "{:?}", db.stats());
    assert!(db.stats().bodies_reparsed.is_empty());

    // `main` returns its `Result`, so the parser rewrites its body: an edit inside it is a full parse.
    parsed_before = db.stats().files_parsed.len();
    let main = db.file_text("main.bd").expect("the fixture opens").to_string();
    db.set_file("main.bd", &replace(&main, "let s = Square { side: 4 }", "let s = Square { side: 5 }"));
    let memo = memo_parse(&mut db, "main.bd");
    assert_eq!(memo, full_parse(&db, "main.bd"));
    assert!(db.stats().files_parsed[parsed_before..].contains(&"main.bd".to_string()), "{:?}", db.stats());
    assert!(db.stats().bodies_reparsed.is_empty());
}

#[test]
fn a_file_that_had_syntax_errors_is_parsed_in_full_again() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("math.bd", &replace(MATH, "a + b", "a +"));
    let analysis = db.analyze().unwrap();
    assert!(analysis.program.is_none());
    assert!(!analysis.diagnostics.is_empty());
    // The tree is kept with its errors, and the fix is a full parse because the old tree was broken.
    db.set_file("math.bd", &replace(MATH, "a + b", "a + b + 0"));
    let analysis = db.analyze().unwrap();
    assert!(analysis.diagnostics.is_empty(), "{}", render(&db, &analysis));
    assert_eq!(db.stats().files_parsed, ["math.bd"]);
    assert!(db.stats().bodies_reparsed.is_empty());
    assert_eq!(memo_parse(&mut db, "math.bd"), full_parse(&db, "math.bd"));
}
