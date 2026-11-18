//! What an edit makes the database redo (`spec/query.md`, QL5): counters for parsing and for checking bodies.

mod common;

use burdin_query::Db;
use common::*;

#[test]
fn the_first_analysis_checks_everything_and_reports_nothing() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert!(analysis.program.is_some());
    let user = checked(&db);
    for name in ["main.main", "math.add", "math.mul", "main.total_area", "main.scaled", "report.describe"] {
        assert!(user.iter().any(|n| n == name), "`{name}` was not checked: {user:?}");
    }
    // The standard library is checked too, once.
    assert!(db.stats().body_checks > 1000, "{}", db.stats().body_checks);
}

#[test]
fn an_analysis_with_no_edit_checks_and_parses_nothing() {
    let mut db = open();
    db.analyze().unwrap();
    db.analyze().unwrap();
    assert!(db.stats().bodies_checked.is_empty(), "{:?}", db.stats().bodies_checked);
    assert!(db.stats().files_parsed.is_empty());
    assert!(db.stats().bodies_reparsed.is_empty());
    assert!(db.stats().bodies_reused > 1000);
}

#[test]
fn editing_a_function_body_rechecks_only_that_function() {
    let mut db = open();
    db.analyze().unwrap();
    // `add` is called by `total_area` in another file; its signature is unchanged, so only its body is redone.
    db.set_file("math.bd", &replace(MATH, "a + b", "a + b + 0"));
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert_eq!(checked(&db), ["math.add"]);
    // The file was updated by reparsing the body, not parsed again.
    assert!(db.stats().files_parsed.is_empty(), "{:?}", db.stats().files_parsed);
    assert_eq!(db.stats().bodies_reparsed, ["math.bd"]);
}

#[test]
fn a_body_edit_moves_the_functions_after_it_without_checking_them() {
    let mut db = open();
    db.analyze().unwrap();
    // A longer body, with extra lines: `mul` comes after `add` in the file and its position changes.
    db.set_file("math.bd", &replace(MATH, "a + b", "let sum = a + b\n    let again = sum + 0\n    again"));
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert_eq!(checked(&db), ["math.add"]);
}

#[test]
fn a_new_error_in_a_body_is_reported_at_its_new_position_and_only_then() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("math.bd", &replace(MATH, "a * b", "\n\n    a * \"two\""));
    let analysis = db.analyze().unwrap();
    assert_eq!(checked(&db), ["math.mul"]);
    let text = render(&db, &analysis);
    assert!(text.contains("math.bd:8:9: error[E0003]"), "{text}");
    // Fixing it brings the diagnostics back to none, again checking only that body.
    db.set_file("math.bd", MATH);
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert_eq!(checked(&db), ["math.mul"]);
}

#[test]
fn a_diagnostic_in_an_untouched_function_follows_it_when_an_earlier_body_changes() {
    // `mul` has an error; editing `add` above it moves `mul` but does not change it, so its cached diagnostic
    // must move with it.
    let broken = replace(MATH, "a * b", "a * \"two\"");
    let mut db = Db::from_files(
        &[("main.bd", MAIN), ("math.bd", &broken), ("geometry/shapes.bd", SHAPES), ("report.bd", REPORT)],
        Default::default(),
    )
    .unwrap();
    let first = db.analyze().unwrap();
    assert!(render(&db, &first).contains("math.bd:6:9: error[E0003]"), "{}", render(&db, &first));
    db.set_file("math.bd", &replace(&broken, "a + b", "a + b\n"));
    let second = db.analyze().unwrap();
    assert_eq!(checked(&db), ["math.add"]);
    assert!(render(&db, &second).contains("math.bd:7:9: error[E0003]"), "{}", render(&db, &second));
}

#[test]
fn editing_a_signature_rechecks_its_callers() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("math.bd", &replace(MATH, "fn mul(a: i32, b: i32): i32 {", "fn mul(a: i32, b: i32, c: i32): i32 {"));
    let analysis = db.analyze().unwrap();
    // The caller now passes the wrong number of arguments.
    assert!(render(&db, &analysis).contains("main.bd:10:5: error[E0004]"), "{}", render(&db, &analysis));
    let user = checked(&db);
    for name in ["math.mul", "main.scaled"] {
        assert!(user.iter().any(|n| n == name), "`{name}` was not rechecked: {user:?}");
    }
}

#[test]
fn adding_an_item_rechecks_what_can_see_it() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("math.bd", &format!("{MATH}\nfn sub(a: i32, b: i32): i32 {{\n    a - b\n}}\n"));
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    let user = checked(&db);
    assert!(user.iter().any(|n| n == "math.sub"), "{user:?}");
    assert!(user.iter().any(|n| n == "main.main"), "{user:?}");
}

#[test]
fn a_syntax_error_reports_and_skips_checking() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("math.bd", &replace(MATH, "a + b", "a + "));
    let analysis = db.analyze().unwrap();
    assert!(analysis.program.is_none());
    assert!(render(&db, &analysis).contains("math.bd:"), "{}", render(&db, &analysis));
    assert!(db.stats().bodies_checked.is_empty());
    // Fixing it again costs one body.
    db.set_file("math.bd", MATH);
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert_eq!(checked(&db), ["math.add"]);
}
