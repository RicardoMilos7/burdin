//! The memo's memory (`spec/query.md`, QL8): two thousand edits do not grow the database without bound, because the
//! budget drops the least recently used bodies and nothing else keeps a per-edit copy.
//!
//! This file holds one test, so the counting allocator measures this test alone: the integration-test binary is its
//! own process, but the other tests of a binary would run beside it.

mod common;

use burdin_query::Budget;
use common::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts the bytes currently allocated through the global allocator.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            let live = LIVE.fetch_add(new_size, Ordering::Relaxed) + new_size;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// A package of a few plain functions, with no standard library, so the two thousand analyses of this test run in
/// seconds and the baseline is the package itself.
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
fn two_thousand_edits_do_not_grow_the_database_without_bound() {
    let mut db = standalone(&[("main.bd", SMALL)]);
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    // The first analysis is the baseline. The budget leaves a hundred entries of room, so the edits have to be
    // evicted rather than pile up.
    let baseline = live();
    let full = db.remembered_bodies();
    db.set_budget(Budget { bodies: full + 100 });
    let mut text = SMALL.to_string();
    for edit in 0..2000 {
        text = if edit % 2 == 0 { replace(&text, "a + b", "a +  b") } else { replace(&text, "a +  b", "a + b") };
        db.set_file("main.bd", &text);
        let analysis = db.analyze().unwrap();
        assert_eq!(render(&db, &analysis), "", "edit {edit}");
        assert!(db.remembered_bodies() <= full + 100, "edit {edit}: {} bodies", db.remembered_bodies());
    }
    // The memo holds at most the budget's bodies and the database holds one text per file, so what the two thousand
    // edits added is a hundred entries, not two thousand copies of the program.
    let after = live();
    assert!(
        after < baseline + 8 * 1024 * 1024,
        "live bytes went from {baseline} to {after} over two thousand edits (peak {})",
        PEAK.load(Ordering::Relaxed)
    );
    // And the answer is still the first one.
    db.set_file("main.bd", SMALL);
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
}
