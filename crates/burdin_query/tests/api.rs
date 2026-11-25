//! The tool-facing API (`spec/query.md`, QL7): a lossless syntax tree, a name at a position, the type of an
//! expression, an item's signature, the diagnostics of one file, and the ownership facts of a function.
//!
//! The fixture package is answered probe by probe and compared with `tests/fixtures/api.golden`, so a change to any
//! answer shows up as a diff. Set `BURDIN_QUERY_UPDATE=1` to rewrite the golden file after a deliberate change.

mod common;

use burdin_query::{Analysis, Db, DefinitionKind, Location, Resolution};
use common::*;
use std::fmt::Write as _;

/// The byte offset of the first occurrence of `needle`, plus `skip` bytes.
fn at(text: &str, needle: &str, skip: usize) -> u32 {
    (text.find(needle).unwrap_or_else(|| panic!("`{needle}` is not in the text")) + skip) as u32
}

/// `path:line:col` of a location's start.
fn place(db: &Db, location: &Location) -> String {
    let (line, col) = db.line_col(&location.path, location.lo).expect("the location's file is open");
    format!("{}:{line}:{col}", location.path)
}

fn kind_name(kind: DefinitionKind) -> &'static str {
    match kind {
        DefinitionKind::Local => "local",
        DefinitionKind::Parameter => "parameter",
        DefinitionKind::Function => "function",
        DefinitionKind::Constant => "constant",
    }
}

fn show_resolution(db: &Db, resolution: &Resolution) -> String {
    let mut out = format!("  {} `{}` at {}", kind_name(resolution.kind), resolution.name, place(db, &resolution.at));
    match &resolution.definition {
        Some(definition) => writeln!(out, ", declared at {}", place(db, definition)).unwrap(),
        None => out.push_str(", no declaration\n"),
    }
    writeln!(out, "  type {}", resolution.ty).unwrap();
    out
}

/// One probe: a file, a byte offset into it, and what to ask there.
enum Probe {
    Resolve(&'static str, &'static str, usize),
    Type(&'static str, &'static str, usize),
    Signature(&'static str, &'static str, usize),
    Ownership(&'static str, &'static str, usize),
}

const PROBES: &[Probe] = &[
    // A call to a function in another file, and its arguments.
    Probe::Resolve("main.bd", "add(a, b)", 0),
    Probe::Resolve("main.bd", "add(a, b)", 4),
    Probe::Resolve("main.bd", "add(a, b)", 7),
    // A local, a field access through a method receiver, and an index.
    Probe::Resolve("main.bd", "s.area()", 0),
    Probe::Resolve("main.bd", "areas[1]", 0),
    Probe::Type("main.bd", "areas[0]", 0),
    Probe::Type("main.bd", "s.area()", 0),
    Probe::Type("main.bd", "Square { side: 4 }", 0),
    Probe::Type("main.bd", "describe(total_area", 0),
    // Declarations in other files.
    Probe::Signature("math.bd", "fn add", 0),
    Probe::Signature("math.bd", "fn mul", 3),
    Probe::Signature("geometry/shapes.bd", "fn area(self)", 0),
    Probe::Signature("geometry/shapes.bd", "fn new", 0),
    Probe::Signature("report.bd", "fn describe", 0),
    // A parameter and a local in a string interpolation.
    Probe::Resolve("report.bd", "{n}", 1),
    Probe::Resolve("report.bd", "{label}", 1),
    Probe::Type("report.bd", "\"area\"", 0),
    // Ownership facts.
    Probe::Ownership("main.bd", "fn main", 0),
    Probe::Ownership("main.bd", "fn consume", 0),
    Probe::Ownership("math.bd", "fn add", 0),
    // A position in a declaration, not in a body.
    Probe::Type("math.bd", "fn add", 0),
];

/// Every answer of the fixture, in a fixed order.
fn report(db: &mut Db, analysis: &Analysis) -> String {
    let mut out = String::new();
    let mut texts = Vec::new();
    for path in db.file_paths() {
        texts.push((path.to_string(), db.file_text(path).unwrap().to_string()));
    }
    for probe in PROBES {
        let (what, path, needle, skip) = match *probe {
            Probe::Resolve(path, needle, skip) => ("resolve", path, needle, skip),
            Probe::Type(path, needle, skip) => ("type", path, needle, skip),
            Probe::Signature(path, needle, skip) => ("signature", path, needle, skip),
            Probe::Ownership(path, needle, skip) => ("ownership", path, needle, skip),
        };
        let text = &texts.iter().find(|(p, _)| p == path).expect("a fixture file").1;
        let offset = at(text, needle, skip);
        let (line, col) = db.line_col(path, offset).expect("the file is open");
        writeln!(out, "{what} {path}:{line}:{col}").unwrap();
        match *probe {
            Probe::Resolve(..) => match analysis.resolve_at(path, offset) {
                Some(resolution) => out.push_str(&show_resolution(db, &resolution)),
                None => out.push_str("  nothing\n"),
            },
            Probe::Type(..) => match analysis.type_at(path, offset) {
                Some(typed) => writeln!(out, "  {} at {}", typed.ty, place(db, &typed.at)).unwrap(),
                None => out.push_str("  nothing\n"),
            },
            Probe::Signature(..) => match analysis.signature_of(path, offset) {
                Some(signature) => {
                    writeln!(out, "  {}", signature.display).unwrap();
                    writeln!(out, "  name {}", signature.name).unwrap();
                    for param in &signature.params {
                        writeln!(
                            out,
                            "  param {} {} {:?} default={}",
                            param.name, param.ty, param.convention, param.has_default
                        )
                        .unwrap();
                    }
                    writeln!(out, "  ret {}", signature.ret).unwrap();
                }
                None => out.push_str("  nothing\n"),
            },
            Probe::Ownership(..) => match analysis.ownership_of(path, offset) {
                Some(ownership) => {
                    writeln!(out, "  function {}", ownership.function).unwrap();
                    for owned in &ownership.owned {
                        writeln!(out, "  owned {} {} parameter={}", owned.name, owned.ty, owned.is_parameter).unwrap();
                    }
                    for moved in &ownership.moves {
                        writeln!(out, "  move {} {:?} always_moved={}", moved.local, moved.path, moved.always_moved)
                            .unwrap();
                    }
                    writeln!(out, "  origins {:?}", ownership.return_origins).unwrap();
                }
                None => out.push_str("  nothing\n"),
            },
        }
    }
    out
}

#[test]
fn the_public_api_answers_the_fixture_as_the_golden_file_says() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let got = report(&mut db, &analysis);
    let golden = include_str!("fixtures/api.golden");
    if std::env::var("BURDIN_QUERY_UPDATE").is_ok() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api.golden");
        std::fs::write(path, &got).unwrap();
        return;
    }
    assert_eq!(got, golden, "\n--- got ---\n{got}");
}

#[test]
fn the_syntax_tree_keeps_every_byte_and_labels_the_comments() {
    let text = "/// A documented function.\nfn main() {\n    // a note\n    let x = 1 /* inline */ + 2\n}\n";
    let mut db = Db::from_files(&[("main.bd", text)], Default::default()).unwrap();
    let tree = db.syntax_tree("main.bd").unwrap();
    assert_eq!(tree.text(), text);
    assert_eq!(tree.rebuild(), text, "tokens and trivia rebuild the file");
    assert!(tree.diagnostics().is_empty());
    // Tokens and trivia tile the file: no byte is counted twice and none is lost.
    let mut spans: Vec<(u32, u32)> = tree.tokens.iter().map(|t| (t.span.lo, t.span.hi)).collect();
    spans.extend(tree.trivia.iter().map(|t| (t.lo, t.hi)));
    spans.sort_unstable();
    let mut end = 0;
    for (lo, hi) in &spans {
        assert_eq!(*lo, end, "a gap or an overlap at {lo}..{hi}");
        end = *hi;
    }
    assert_eq!(end as usize, text.len());
    let comments: Vec<&str> = tree
        .trivia
        .iter()
        .filter(|t| t.kind == burdin_query::TriviaKind::DocComment)
        .map(|t| tree.slice(t.lo, t.hi))
        .collect();
    assert_eq!(comments, ["/// A documented function."]);
    let line: Vec<&str> = tree
        .trivia
        .iter()
        .filter(|t| t.kind == burdin_query::TriviaKind::LineComment)
        .map(|t| tree.slice(t.lo, t.hi))
        .collect();
    assert_eq!(line, ["// a note"]);
    let block: Vec<&str> = tree
        .trivia
        .iter()
        .filter(|t| t.kind == burdin_query::TriviaKind::BlockComment)
        .map(|t| tree.slice(t.lo, t.hi))
        .collect();
    assert_eq!(block, ["/* inline */"]);
    // The tokens are the syntax tree's leaves: the file opens with the `fn` keyword.
    assert!(tree.tokens.len() > 5);
    assert_eq!(tree.slice(tree.tokens[0].span.lo, tree.tokens[0].span.hi), "fn");
}

#[test]
fn the_syntax_tree_follows_an_edit() {
    let mut db = open();
    let before = db.syntax_tree("math.bd").unwrap();
    let old = before.text().to_string();
    db.set_file("math.bd", &replace(&old, "a + b", "a + b + 0"));
    let after = db.syntax_tree("math.bd").unwrap();
    assert_ne!(after.text(), before.text());
    assert_eq!(after.rebuild(), after.text());
    assert!(after.text().contains("a + b + 0"));
    // The edit stayed inside one body, so asking for the tree reparsed that body, not the file.
    assert_eq!(db.counters().parses, 1);
    assert_eq!(db.counters().body_reparses, 1);
}

#[test]
fn diagnostics_are_grouped_by_file() {
    let mut db = open();
    db.analyze().unwrap();
    db.set_file("report.bd", &replace(REPORT, "\"{label} {n}\"", "42"));
    let analysis = db.analyze().unwrap();
    let report = analysis.diagnostics_of("report.bd");
    assert_eq!(report.len(), 1, "{:?}", analysis.diagnostics_of("report.bd"));
    assert_eq!(report[0].code.0, "E0003", "{}", report[0].message);
    assert!(analysis.diagnostics_of("math.bd").is_empty());
    assert!(analysis.diagnostics_of("not_a_file.bd").is_empty());
}

#[test]
fn a_constant_resolves_to_its_declaration() {
    let text = "const LIMIT: i32 = 10\n\nfn main() {\n    let copy = LIMIT\n    println(\"ok\")\n}\n";
    let mut db = Db::from_files(&[("main.bd", text)], Default::default()).unwrap();
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    let use_at = at(text, "= LIMIT", 2);
    let resolution = analysis.resolve_at("main.bd", use_at).expect("the use resolves");
    assert_eq!(resolution.kind, DefinitionKind::Constant);
    assert_eq!(resolution.name, "LIMIT");
    assert_eq!(resolution.ty, "i32");
    let definition = resolution.definition.expect("a constant has a declaration");
    assert_eq!(db.line_col(&definition.path, definition.lo), Some((1, 7)));
    // The declaration is the name: `LIMIT`, not the whole `const` item.
    assert_eq!(db.file_text("main.bd").unwrap().get(definition.lo as usize..definition.hi as usize), Some("LIMIT"));
}

#[test]
fn a_position_outside_a_body_has_no_type_and_no_name() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let keyword = at(MATH, "fn add", 0);
    assert_eq!(analysis.type_at("math.bd", keyword), None);
    assert_eq!(analysis.resolve_at("math.bd", keyword), None);
    // Past the end of a file, nothing contains the position.
    let end = db.file_text("math.bd").unwrap().len() as u32 + 100;
    assert_eq!(analysis.type_at("math.bd", end), None);
    // A file that is not part of the program has no answer either.
    assert_eq!(analysis.type_at("nope.bd", 0), None);
}

#[test]
fn a_signature_reports_the_parameters_a_call_site_writes() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let signature = analysis.signature_of("geometry/shapes.bd", at(SHAPES, "fn new", 0)).expect("a method");
    assert_eq!(signature.params.len(), 2);
    assert_eq!(signature.params[0].name, "w");
    assert_eq!(signature.params[0].convention, burdin_query::Convention::Read);
    assert!(!signature.params[0].has_default);
    assert_eq!(signature.ret, "Rect");
    assert_eq!(signature.display, "fn new(w: i32, h: i32): Rect");
    // A receiver is written `self` and borrows.
    let method = analysis.signature_of("geometry/shapes.bd", at(SHAPES, "fn area(self)", 0)).expect("a method");
    assert_eq!(method.params[0].name, "self");
    assert_eq!(method.display, "fn area(self): i32");
}

#[test]
fn ownership_reports_a_move_out_of_a_local() {
    let text = "struct Pair {\n    value: i32\n}\n\nfn consume(own p: Pair) {\n    let v = p.value\n    println(\"ok\")\n}\n\nfn main() {\n    let p = Pair { value: 1 }\n    consume(p)\n}\n";
    let mut db = Db::from_files(&[("main.bd", text)], Default::default()).unwrap();
    let analysis = db.analyze().unwrap();
    assert_eq!(render(&db, &analysis), "");
    let ownership = analysis.ownership_of("main.bd", at(text, "fn main", 0)).expect("main");
    assert_eq!(ownership.function, "main.main");
    assert!(ownership.moves.iter().any(|m| m.local == "p" && m.path.is_empty()), "{:?}", ownership.moves);
    assert!(ownership.owned.iter().any(|l| l.name == "p" && l.ty == "Pair"), "{:?}", ownership.owned);
    let consume = analysis.ownership_of("main.bd", at(text, "fn consume", 0)).expect("consume");
    assert!(consume.owned.iter().any(|l| l.name == "p" && l.is_parameter), "{:?}", consume.owned);
}

/// A syntax error in one function leaves the typed answers of the code the parser kept (`spec/query.md`, QL10): the
/// program is not handed out, but the position queries read the one checked against the partial tree, and a dangling
/// dot keeps its receiver.
#[test]
fn position_queries_answer_beside_a_syntax_error() {
    let text = "fn broken() {\n    let half = (1 +\n}\n\nfn main() {\n    let total = 40\n    let n = total + 2\n    n.\n    let m = n\n}\n";
    let mut db = Db::from_files(&[("main.bd", text)], Default::default()).unwrap();
    let analysis = db.analyze().unwrap();
    assert!(analysis.program.is_none(), "a program with a syntax error is not handed out");
    assert!(analysis.has_types() && analysis.is_partial());
    // The code the parser kept is typed: a local, an expression, and the receiver of the dangling dot.
    let total = analysis.resolve_at("main.bd", at(text, "total + 2", 0)).expect("`total` resolves");
    assert_eq!((total.kind, total.ty.as_str()), (DefinitionKind::Local, "i32"));
    assert_eq!(analysis.type_at("main.bd", at(text, "total + 2", 6)).map(|t| t.ty), Some("i32".to_string()));
    assert_eq!(analysis.type_at("main.bd", at(text, "n.\n", 0)).map(|t| t.ty), Some("i32".to_string()));
    assert_eq!(analysis.locals("main.bd").iter().filter(|l| l.name == "n").count(), 1);
    // The statement the parser could not read is not there.
    assert_eq!(analysis.type_at("main.bd", at(text, "(1 +", 1)), None);
    // Only the position queries use the partial program: ownership and references need a complete one.
    assert!(analysis.ownership_events("main.bd").is_empty());
    // Fixing the error gives a real program again.
    let fixed = text.replace("(1 +", "(1 + 2)").replace("    n.\n", "");
    db.set_file("main.bd", &fixed);
    let analysis = db.analyze().unwrap();
    assert!(analysis.program.is_some() && !analysis.is_partial(), "{}", render(&db, &analysis));
}
