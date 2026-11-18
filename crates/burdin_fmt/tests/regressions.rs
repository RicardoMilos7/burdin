//! Inputs the fuzzer found (issue #462), kept as plain tests: each one used to produce an internal error or a layout
//! that changed when it was formatted again. Every input must format, and the result must be a fixed point.

use burdin_fmt::{Options, format_source};

fn format(src: &str) -> String {
    let options = Options::default();
    let once = format_source(src, &options).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let twice = format_source(&once, &options).expect("the output formats again");
    assert_eq!(once, twice, "not a fixed point");
    once
}

#[test]
fn a_parenthesized_type_never_gains_a_comma() {
    // `(T)` is `T`; `(T,)` would be a tuple.
    let out = format("fn main() {\n    let d: (// comment\n((((i32))))) = a\n}\n");
    assert!(!out.contains(",\n"), "{out}");
    assert!(out.contains("// comment"));
}

#[test]
fn a_tuple_field_of_an_integer_is_not_read_as_a_float() {
    let out = format("fn main() {\n    let a = 1.\n        2\n}\n");
    assert!(!out.contains("1.2"), "{out}");
}

#[test]
fn comments_after_a_priv_keyword_stay_separate_comments() {
    let out = format("priv// a\n// b\n// c\nfn f() {}\n");
    for c in ["// a", "// b", "// c"] {
        assert_eq!(out.matches(c).count(), 1, "{out}");
    }
    assert!(out.lines().all(|l| l.matches("//").count() <= 1), "two line comments share a line:\n{out}");
}

#[test]
fn two_trailing_line_comments_do_not_merge() {
    let out = format("fn main() {\n    it.next( // one\n) // two\n}\n");
    assert!(out.lines().all(|l| l.matches("//").count() <= 1), "{out}");
    assert!(out.contains("// one") && out.contains("// two"));
}

#[test]
fn an_own_line_comment_between_equals_and_the_value_settles() {
    let out = format("fn main() {\n    let total =\n        // note\n        b\n}\n");
    assert!(out.contains("// note"));
}

#[test]
fn a_comment_after_a_dot_settles() {
    let out = format("fn main() {\n    comment.\n\n/// Adds\n    total\n}\n");
    assert!(out.contains("/// Adds"));
}

#[test]
fn an_inline_block_comment_before_code_stays_inline() {
    let out = format("fn main() {\n    f(1,\n      2, /* b */ 3)\n}\n");
    assert!(out.contains("/* b */ 3"), "{out}");
}

#[test]
fn doc_blocks_in_unusual_runs_keep_their_text() {
    let out = format("/// ```\n/// not closed\nfn f() {}\n\n/// ```bur0din\n/// let   x\n/// ```\nfn g() {}\n");
    assert!(out.contains("/// not closed"));
    assert!(out.contains("/// let   x"), "{out}");
}
