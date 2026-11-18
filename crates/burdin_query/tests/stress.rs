//! Cancellation and the memory budget (`spec/query.md`, QL8): an analysis can be stopped from another thread and
//! leaves the database usable, and the memo stays inside the budget however many edits arrive.

mod common;

use burdin_query::{Budget, Cancelled};
use common::*;
use std::time::Duration;

#[test]
fn cancelling_before_an_analysis_returns_cancelled_and_the_next_one_succeeds() {
    let mut db = open();
    let handle = db.cancel_handle();
    handle.cancel();
    assert!(matches!(db.analyze(), Err(Cancelled)));
    // The flag was cleared, so the next analysis is a fresh request.
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert!(analysis.program.is_some());
}

#[test]
fn a_cancel_from_another_thread_leaves_the_database_usable() {
    let mut db = open();
    let handle = db.cancel_handle();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(2));
        handle.cancel();
    });
    // Whether the cancel arrived before the analysis finished or after it is a race the test does not fix: both
    // outcomes are correct, and both must leave the database ready for the next analysis.
    if let Ok(analysis) = db.analyze() {
        assert!(analysis.program.is_some());
    }
    canceller.join().expect("the cancelling thread finishes");
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert!(analysis.program.is_some());
}

#[test]
fn the_budget_evicts_bodies_and_the_next_analysis_is_still_right() {
    let mut db = open();
    db.analyze().unwrap();
    let full = db.remembered_bodies();
    assert!(full > 1000, "the standard library alone is over a thousand bodies: {full}");
    db.set_budget(Budget { bodies: 50 });
    // A body edit runs an analysis, which evicts down to the budget at its end.
    db.set_file("math.bd", &replace(MATH, "a + b", "a + b + 0"));
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    assert!(db.remembered_bodies() <= 50, "{} bodies after eviction", db.remembered_bodies());
    // Everything evicted is checked again rather than answered wrongly.
    db.set_file("math.bd", &replace(MATH, "a * b", "a * \"two\""));
    let analysis = db.analyze().unwrap();
    assert!(render(&db, &analysis).contains("math.bd:6:9: error[E0003]"), "{}", render(&db, &analysis));
}

/// A package of a few plain functions, with no standard library: the memo's behavior is the same, and two
/// thousand analyses fit in a test.
const SMALL: &str = "fn add(a: i32, b: i32): i32 {
    a + b
}

fn mul(a: i32, b: i32): i32 {
    a * b
}

fn main() {
    let x = add(1, 2)
    let y = mul(x, 3)
    if y > 0 {
        let z = add(y, x)
    }
}
";

#[test]
fn two_thousand_edits_keep_the_answer_and_the_memo_bounded() {
    let mut db = standalone(&[("main.bd", SMALL)]);
    db.analyze().unwrap();
    // A budget just above what the first analysis filled the memo with: the working set stays, and the entries the
    // edits add are what the budget has to keep in check.
    let full = db.remembered_bodies();
    db.set_budget(Budget { bodies: full + 100 });
    let mut text = SMALL.to_string();
    for edit in 0..2000 {
        // One keystroke: a space goes in or comes out of an expression inside `add`.
        text = if edit % 2 == 0 { replace(&text, "a + b", "a +  b") } else { replace(&text, "a +  b", "a + b") };
        db.set_file("main.bd", &text);
        let analysis = db.analyze().unwrap();
        assert_eq!(render(&db, &analysis), "", "edit {edit}");
        assert!(db.remembered_bodies() <= full + 100, "edit {edit}: {} bodies", db.remembered_bodies());
        assert_eq!(checked(&db), ["main.add"], "edit {edit}");
    }
}
