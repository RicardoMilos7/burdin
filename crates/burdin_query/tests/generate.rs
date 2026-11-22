//! `comptime generate` through the database (`spec/generate.md`, GN3, GN7, GN8).

use burdin_query::{Db, Options};

const MAIN: &str = "\
fn text(): String {
    String.from(\"fn made(): i32 {{ 1 }}\\n\")
}

comptime generate {
    text()
}

fn main() {
    let x: i32 = made()
    println(format(\"{}\", x))
}
";

fn open(main: &str) -> Db {
    Db::from_files(&[("main.bd", main)], Options::default()).expect("the package opens")
}

#[test]
fn a_generated_declaration_is_part_of_the_program() {
    let mut db = open(MAIN);
    let analysis = db.analyze().unwrap();
    assert_eq!(analysis.diagnostics.render(db.sources()), "");
    let program = analysis.program.expect("the program is checked");
    assert!(program.functions.iter().any(|f| f.name == "main.made" || f.name == "made"));
    // The generator's function is in the program but has no code.
    let generator = program.functions.iter().find(|f| f.name.ends_with("comptime generate 1")).expect("the generator");
    assert!(generator.symbolic);
}

#[test]
fn a_body_edit_that_changes_the_generated_text_rechecks_the_bodies_that_use_it() {
    // `text` is a hand-written body, so editing it is a body edit: the declarations as written are unchanged. What
    // `made` is changes anyway, and `main` must be checked against the new `made`, not reused from the run in which
    // `made` returned an `i32` (GN8).
    let mut db = open(MAIN);
    let analysis = db.analyze().unwrap();
    assert_eq!(analysis.diagnostics.render(db.sources()), "");
    let edited = MAIN.replace("fn made(): i32 {{ 1 }}", "fn made(): String {{ String.from(\\\"s\\\") }}");
    assert_ne!(edited, MAIN);
    db.set_file("main.bd", &edited);
    let analysis = db.analyze().unwrap();
    let text = analysis.diagnostics.render(db.sources());
    assert!(text.contains("error[E0003]"), "{text}");
    assert!(text.contains("main.bd:10:"), "the error is in `main`: {text}");

    // Back to the first text: no error, `main` is checked again.
    db.set_file("main.bd", MAIN);
    let analysis = db.analyze().unwrap();
    assert_eq!(analysis.diagnostics.render(db.sources()), "");
}

#[test]
fn a_generator_that_refers_to_a_generated_name_stops_generation() {
    let src = "\
comptime generate {
    String.from(\"fn made(): i32 {{ 1 }}\\n\")
}

comptime generate {
    let x = made()
    String.new()
}

fn main() {
    let unrelated: i32 = \"not checked\"
}
";
    let mut db = open(src);
    let analysis = db.analyze().unwrap();
    let text = analysis.diagnostics.render(db.sources());
    assert!(text.contains("main.bd:6:13: error[E0002]"), "{text}");
    // Generation stopped: the rest of the program was not checked (GN7).
    assert!(!text.contains("E0003"), "{text}");
    assert!(analysis.program.is_none());
}

#[test]
fn a_generator_failing_beside_a_syntax_error_adds_no_follow_on_error() {
    // `broken` is not declared because it did not parse, so the generator's call is an error that only follows from
    // the syntax error: the report holds the syntax error and nothing else (`spec/check.md`, CK4).
    let src = "\
fn broken( {
}

comptime generate {
    let x = broken()
    String.new()
}

fn main() {
}
";
    let mut db = open(src);
    let analysis = db.analyze().unwrap();
    let text = analysis.diagnostics.render(db.sources());
    assert_eq!(text.matches("error[").count(), 1, "{text}");
    assert!(text.contains("main.bd:1:12: error[E0001]"), "{text}");
}
