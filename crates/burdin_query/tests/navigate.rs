//! The navigation questions a language server asks (`spec/query.md`, QL7): every resolved name, the locals of a file,
//! the call sites with the argument written for each parameter, the functions, the tests, and the layout of modules
//! and packages. `crates/burdin_lsp/tests/` exercises them through the server; these tests pin the library's answers.

mod common;

use burdin_query::{Db, DefinitionKind, Options};
use common::*;

fn at(text: &str, needle: &str, skip: usize) -> u32 {
    (text.find(needle).unwrap_or_else(|| panic!("`{needle}` is not in the text")) + skip) as u32
}

fn slice<'a>(db: &'a Db, path: &str, lo: u32, hi: u32) -> &'a str {
    &db.file_text(path).unwrap()[lo as usize..hi as usize]
}

#[test]
fn occurrences_name_every_use_with_its_declaration() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let occurrences = analysis.occurrences();
    let call = at(MAIN, "add(a, b)", 0);
    let add = occurrences.iter().find(|o| o.at.path == "main.bd" && o.at.lo == call).expect("the call of `add`");
    assert_eq!(add.name, "add");
    assert_eq!(add.kind, DefinitionKind::Function);
    // The declaration is the whole item, as `resolve_at` reports it.
    let declared = add.definition.as_ref().expect("a declaration");
    assert_eq!(declared.path, "math.bd");
    assert!(slice(&db, &declared.path, declared.lo, declared.hi).starts_with("fn add(a: i32, b: i32): i32 {"));
    // An argument is a parameter of the function it is written in.
    let a = occurrences.iter().find(|o| o.at.path == "main.bd" && o.at.lo == call + 4).expect("the argument `a`");
    assert_eq!(a.kind, DefinitionKind::Parameter);
    // A method call's span is the whole call.
    let method = at(MAIN, "s.area()", 0);
    let area = occurrences.iter().find(|o| o.at.lo == method && o.name == "area").expect("the method call");
    assert_eq!(slice(&db, "main.bd", area.at.lo, area.at.hi), "s.area()");
    // Once per span and name, sorted.
    let keys: Vec<(&str, u32, u32, &str)> =
        occurrences.iter().map(|o| (o.at.path.as_str(), o.at.lo, o.at.hi, o.name.as_str())).collect();
    let mut unique = keys.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(keys, unique);
    // The standard library's bodies are answered too, under `<std>/`.
    assert!(occurrences.iter().any(|o| o.at.path.starts_with("<std>/")));
}

#[test]
fn locals_have_their_types_and_mutability() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let locals = analysis.locals("main.bd");
    let find = |name: &str| locals.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no local `{name}`"));
    assert_eq!(find("s").ty, "Square");
    assert!(!find("s").mutable && !find("s").is_parameter);
    assert_eq!(find("r").ty, "Rect");
    assert!(find("areas").mutable);
    assert_eq!(find("areas").ty, "List<i32>");
    assert!(find("n").is_parameter);
    assert_eq!(slice(&db, "main.bd", find("side").at.lo, find("side").at.hi), "side");
    // In source order, once each.
    let starts: Vec<u32> = locals.iter().map(|l| l.at.lo).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(starts, sorted);
}

#[test]
fn calls_list_the_caller_the_callee_and_each_written_argument() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let calls = analysis.calls();
    let add = calls.iter().find(|c| c.callee == "add" && c.at.path == "main.bd").expect("the call of `add`");
    assert_eq!(add.caller, "total_area");
    assert_eq!(add.callee_location.as_ref().map(|l| l.path.as_str()), Some("math.bd"));
    let written: Vec<(&str, &str)> =
        add.args.iter().map(|(p, at)| (p.name.as_str(), slice(&db, &at.path, at.lo, at.hi))).collect();
    assert_eq!(written, [("a", "a"), ("b", "b")]);
    // A method call: `self` is the receiver.
    let new = calls.iter().find(|c| c.callee == "new" && c.at.path == "main.bd").expect("`Rect.new`");
    assert_eq!(new.caller, "main");
    assert_eq!(new.args.iter().map(|(p, _)| p.name.as_str()).collect::<Vec<_>>(), ["w", "h"]);
    // Sorted by position.
    let order: Vec<(&str, u32)> = calls.iter().map(|c| (c.at.path.as_str(), c.at.lo)).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
}

#[test]
fn functions_are_listed_once_with_their_signatures() {
    let mut db = open();
    let analysis = db.analyze().unwrap();
    let functions = analysis.functions();
    // The standard library's functions are listed too; this is the package's own.
    let add = functions.iter().find(|f| f.name == "add" && f.location.path == "math.bd").expect("`add`");
    assert_eq!(add.display, "fn add(a: i32, b: i32): i32");
    assert_eq!(add.ret, "i32");
    assert_eq!(add.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(add.location.path, "math.bd");
    let areas: Vec<_> =
        functions.iter().filter(|f| f.name == "area" && f.location.path == "geometry/shapes.bd").collect();
    assert_eq!(areas.len(), 2, "one per implementation");
    let mut places: Vec<(&str, u32)> = functions.iter().map(|f| (f.location.path.as_str(), f.location.lo)).collect();
    let all = places.len();
    places.sort();
    places.dedup();
    assert_eq!(places.len(), all);
}

#[test]
fn tests_and_the_layout_of_modules_and_packages() {
    let text = "fn double(n: i32): i32 {\n    n * 2\n}\n\ntest \"doubles\" {\n    assert_eq(double(2), 4)?\n}\n\ntest \"later\" [ignore] {\n}\n";
    let options = Options { mode: burdin_query::EntryMode::Test, ..Options::default() };
    let mut db = Db::from_files(&[("main.bd", text)], options).unwrap();
    let analysis = db.analyze().unwrap();
    let tests = analysis.tests();
    let names: Vec<(&str, &str, bool)> = tests.iter().map(|t| (t.name.as_str(), t.path.as_str(), t.ignored)).collect();
    assert_eq!(names, [("doubles", "doubles", false), ("later", "later", true)]);
    // A test's location is its name, the string.
    assert_eq!(slice(&db, "main.bd", tests[0].location.lo, tests[0].location.hi), "\"doubles\"");

    let db = open();
    let modules = db.modules();
    let shapes = modules.iter().find(|m| m.path == "geometry/shapes.bd").expect("the module");
    assert_eq!(shapes.module, ["geometry", "shapes"]);
    let packages = db.packages();
    let entry = &packages[shapes.package];
    assert!(entry.is_entry && !entry.embedded);
    assert!(packages.iter().any(|p| p.embedded && p.name == "std"), "the standard library is a package");
    assert!(modules.iter().filter(|m| packages[m.package].embedded).all(|m| m.path.starts_with("<std>/")));
}
