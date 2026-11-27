//! Source-level tests of generic type aliases, impl blocks for specific arguments, and `Drop` on generic types
//! (`spec/type-aliases.md`, `spec/impls.md`). They run the front end on a source text with no standard library, so
//! a program uses only the built-in types.

use crate::{Module, Package, check};
use burdin_diag::{Diagnostics, SourceMap};

/// The error codes the front end reports for a single-file program, in the order they are reported.
fn codes(source: &str) -> Vec<String> {
    let mut sources = SourceMap::default();
    let file = sources.add("test.bd".to_string(), source.to_string());
    let mut diags = Diagnostics::default();
    let ast = burdin_syntax::parse(file, source, &mut diags);
    let packages = [Package {
        name: "test".to_string(),
        is_root: true,
        edition: Default::default(),
        deps: Vec::new(),
        root_module: None,
        official: Vec::new(),
        modules: vec![Module { file, path: Vec::new(), ast: &ast }],
    }];
    check(&packages, 0, 0, crate::EntryMode::Build, false, &sources, &mut diags);
    diags.iter().filter(|d| d.severity == burdin_diag::Severity::Error).map(|d| d.code.0.to_string()).collect()
}

/// The errors of a one-file package whose manifest could declare the official packages `official`: each message and
/// its help lines.
fn errors_with_official(source: &str, official: &[&str]) -> Vec<(String, Vec<String>)> {
    let source = format!("{source}\nfn main() {{}}\n");
    let source = source.as_str();
    let mut sources = SourceMap::default();
    let file = sources.add("test.bd".to_string(), source.to_string());
    let mut diags = Diagnostics::default();
    let ast = burdin_syntax::parse(file, source, &mut diags);
    let packages = [Package {
        name: "test".to_string(),
        is_root: true,
        edition: Default::default(),
        deps: Vec::new(),
        root_module: None,
        official: official.iter().map(|name| name.to_string()).collect(),
        modules: vec![Module { file, path: Vec::new(), ast: &ast }],
    }];
    check(&packages, 0, 0, crate::EntryMode::Build, false, &sources, &mut diags);
    diags
        .iter()
        .filter(|d| d.severity == burdin_diag::Severity::Error)
        .map(|d| {
            assert_eq!(d.code.0, "E0002");
            (d.message.clone(), d.help.clone())
        })
        .collect()
}

// `spec/packages.md`, PKG18: an unresolved `use` of an official package the package does not load says how a
// hand-written file loads it, whichever way the `use` is written; any other unresolved `use` gets no help.
#[test]
fn unknown_module_names_an_official_package() {
    let help = "`json` is an official package: `use json` in a hand-written file of this package loads it for the \
                whole package (a generated module cannot load it on its own)";
    for source in ["use json", "use json.parse", "use json.{parse, Value}"] {
        let errors = errors_with_official(source, &["json", "toml"]);
        assert_eq!(errors.len(), 1, "{source}: {errors:?}");
        assert!(errors[0].0.starts_with("unknown module `json"), "{source}: {}", errors[0].0);
        assert_eq!(errors[0].1, [help], "{source}");
    }
    // Not official, or the package has no manifest to load it in: the error alone.
    assert_eq!(errors_with_official("use yaml", &["json"]), [("unknown module `yaml`".to_string(), vec![])]);
    assert_eq!(errors_with_official("use json", &[]), [("unknown module `json`".to_string(), vec![])]);
}

const PRELUDE: &str = "
interface Describe {
    fn describe(self): i32
}

struct Pair<A, B> {
    a: A
    b: B
}

struct Stack<T> {
    items: [T; 4]
}
";

fn with(body: &str) -> Vec<String> {
    codes(&format!("{PRELUDE}\n{body}\nfn main() {{}}\n"))
}

#[test]
fn a_generic_alias_is_its_definition_with_the_arguments() {
    assert!(with("type Twice<T> = (T, T)\nfn f(p: Twice<i32>): i32 { p.0 + p.1 }").is_empty());
    assert!(
        with("type Twice<T> = (T, T)\ntype Quad<T> = Twice<Twice<T>>\nfn f(p: Quad<i32>): i32 { p.0.0 }").is_empty()
    );
}

#[test]
fn a_generic_alias_checks_arity_and_cycles() {
    assert_eq!(with("type Twice<T> = (T, T)\nfn f(p: Twice<i32, i32>) {}"), ["E0003"]);
    assert_eq!(with("type Twice<T> = (T, T)\nfn f(p: Twice) {}"), ["E0003"]);
    assert_eq!(with("type Loop<T> = Loop<T>\nfn f(p: Loop<i32>) {}"), ["E0009"]);
}

// `spec/type-aliases.md`, AL5: an alias is the type it names, so an impl block written through one is an impl block
// of the aliased type (issue #921).
#[test]
fn an_impl_through_an_alias_is_an_impl_of_the_aliased_type() {
    let cases = [
        // A generic alias with a concrete argument: only that instance has the method.
        "type Stacked<T> = Stack<T>\nStacked<i32> { fn sum(self): i32 { 0 } }\nfn f(s: Stack<i32>): i32 { s.sum() }",
        // A generic alias with a name: every instance has it, whatever the alias calls its parameter.
        "type Stacked<U> = Stack<U>\nStacked<T> { fn size(self): i32 { 1 } }\nfn f(s: Stack<bool>): i32 { s.size() }",
        // An interface implementation.
        "type Stacked<T> = Stack<T>\nStacked<T>: Describe { fn describe(self): i32 { 1 } }\n\
         fn f(s: Stack<i32>): i32 { s.describe() }",
        // An alias with fewer parameters than the type, and one that reorders them.
        "type Keyed<B> = Pair<i32, B>\nKeyed<B> { fn first(self): i32 { self.a } }\n\
         fn f(p: Pair<i32, bool>): i32 { p.first() }",
        "type Flip<A, B> = Pair<B, A>\nFlip<bool, i32> { fn first(self): i32 { self.a } }\n\
         fn f(p: Pair<i32, bool>): i32 { p.first() }",
        // Aliases without parameters: of an instance, of an alias, and of a plain struct.
        "type Ints = Stack<i32>\nInts { fn sum(self): i32 { 0 } }\nfn f(s: Stack<i32>): i32 { s.sum() }",
        "type Ints = Stack<i32>\ntype More = Ints\nMore { fn sum(self): i32 { 0 } }\nfn f(s: Stack<i32>): i32 { s.sum() }",
        "struct Point { x: i32 }\ntype Spot = Point\nSpot { fn get(self): i32 { self.x } }\nfn f(p: Point): i32 { p.get() }",
        // A generic alias of a generic alias.
        "type A<T> = Stack<T>\ntype B<T> = A<T>\nB<i32> { fn sum(self): i32 { 0 } }\nfn f(s: Stack<i32>): i32 { s.sum() }",
    ];
    for source in cases {
        let found = with(source);
        assert!(found.is_empty(), "{source}: {found:?}");
    }
    // The method is the instance's alone, as when the type is written directly.
    assert_eq!(
        with("type Ints = Stack<i32>\nInts { fn sum(self): i32 { 0 } }\nfn f(s: Stack<bool>): i32 { s.sum() }"),
        ["E0002"]
    );
    // A method added through an alias and again directly is a second method of one name.
    assert_eq!(
        with(
            "type Stacked<T> = Stack<T>\nStacked<T> { fn size(self): i32 { 1 } }\nStack<T> { fn size(self): i32 { 2 } }"
        ),
        ["E0007"]
    );
}

#[test]
fn an_impl_through_an_alias_checks_the_alias() {
    // The alias's arity.
    assert_eq!(with("type Stacked<T> = Stack<T>\nStacked<i32, i32> {}"), ["E0003"]);
    assert_eq!(with("type Stacked<T> = Stack<T>\nStacked {}"), ["E0003"]);
    // Its bounds, on a concrete argument.
    assert_eq!(with("struct Plain {}\ntype Sorted<T: Ord> = Stack<T>\nSorted<Plain> {}"), ["E0141"]);
    // A cycle, with parameters or without.
    assert_eq!(with("type Loop<T> = Loop<T>\nLoop<i32> {}"), ["E0009"]);
    assert_eq!(with("type A = B\ntype B = A\nA {}"), ["E0009"]);
    // What it names: a tuple has no methods of its own.
    assert_eq!(with("type Two<T> = (T, T)\nTwo<i32> {}"), ["E0003"]);
}

#[test]
fn an_impl_for_specific_arguments_applies_to_those_instances() {
    let found = with(
        "Stack<i32> { fn sum(self): i32 { 0 } }\n\
         fn f(s: Stack<i32>): i32 { s.sum() }",
    );
    assert!(found.is_empty(), "{found:?}");
    // Another instance does not have the method.
    assert_eq!(with("Stack<i32> { fn sum(self): i32 { 0 } }\nfn f(s: Stack<bool>): i32 { s.sum() }"), ["E0002"]);
}

#[test]
fn an_impl_for_specific_arguments_is_checked_even_when_unused() {
    assert_eq!(with("Stack<i32> { fn wrong(self): bool { 1 } }"), ["E0003"]);
    assert_eq!(with("Pair<i32, B> { fn wrong(self): bool { self.b } }"), ["E0003"]);
}

#[test]
fn the_arguments_of_an_impl_meet_the_bounds_and_the_arity() {
    assert_eq!(with("struct Sorted<T: Ord> { x: T }\nstruct Plain {}\nSorted<Plain> {}"), ["E0141"]);
    assert_eq!(with("Stack<i32, i32> {}"), ["E0003"]);
    assert_eq!(with("Pair<T, T> {}"), ["E0007"]);
}

#[test]
fn two_impls_of_one_interface_cannot_share_an_instance() {
    let overlap = "Pair<i32, B>: Describe { fn describe(self): i32 { 1 } }\n\
                   Pair<A, i32>: Describe { fn describe(self): i32 { 2 } }";
    assert_eq!(with(overlap), ["E0143"]);
    let apart = "Pair<i32, B>: Describe { fn describe(self): i32 { 1 } }\n\
                 Pair<bool, B>: Describe { fn describe(self): i32 { 2 } }";
    assert!(with(apart).is_empty());
    let general = "Stack<T>: Describe { fn describe(self): i32 { 1 } }\n\
                   Stack<i32>: Describe { fn describe(self): i32 { 2 } }";
    assert_eq!(with(general), ["E0143"]);
}

#[test]
fn two_methods_of_one_name_cannot_share_an_instance() {
    assert_eq!(with("Stack<T> { fn size(self): i32 { 1 } }\nStack<i32> { fn size(self): i32 { 2 } }"), ["E0007"]);
}

#[test]
fn drop_covers_every_instance_of_a_generic_type() {
    assert!(with("Stack<T>: Drop { fn drop(mut self) {} }").is_empty());
    assert_eq!(with("Stack<i32>: Drop { fn drop(mut self) {} }"), ["E0131"]);
    assert_eq!(with("Stack<T>: Drop where T: Copy { fn drop(mut self) {} }"), ["E0131"]);
}
