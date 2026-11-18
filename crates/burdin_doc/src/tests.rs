//! Unit tests of the documentation generator over small in-memory packages.

use super::*;
use burdin_diag::SourceMap;
use burdin_syntax::ast;

struct Fixture {
    sources: SourceMap,
    asts: Vec<(FileId, ast::File)>,
    paths: Vec<Vec<String>>,
    names: Vec<String>,
}

impl Fixture {
    /// `modules` is `(module path joined by '.', source)`; the empty string is a one-file target.
    fn new(modules: &[(&str, &str)]) -> Fixture {
        let mut sources = SourceMap::default();
        let mut asts = Vec::new();
        let mut paths = Vec::new();
        let mut names = Vec::new();
        for (path, text) in modules {
            let name = if path.is_empty() { "main".to_string() } else { format!("src/{}.bd", path.replace('.', "/")) };
            let id = sources.add(name.clone(), text.to_string());
            let mut diags = Diagnostics::default();
            let file = burdin_syntax::parse(id, text, &mut diags);
            assert!(!diags.has_errors(), "{}", diags.render(&sources));
            asts.push((id, file));
            paths.push(if path.is_empty() { Vec::new() } else { path.split('.').map(str::to_string).collect() });
            names.push(name);
        }
        Fixture { sources, asts, paths, names }
    }

    fn input(&self, private: bool) -> Input<'_> {
        let modules = self
            .asts
            .iter()
            .enumerate()
            .map(|(i, (file, ast))| ModuleInput {
                file: *file,
                path: &self.paths[i],
                ast,
                file_path: self.names[i].clone(),
                is_root: i == 0,
            })
            .collect();
        Input {
            sources: &self.sources,
            packages: vec![PackageInput { name: "pkg", deps: &[], modules, is_std: false }],
            entry: 0,
            private,
        }
    }

    fn api(&self, private: bool) -> (Api, String) {
        let (api, diags) = build(&self.input(private));
        (api, diags.render(&self.sources))
    }
}

const SHAPES: &str = "\
//! Shapes.

/// A point in the plane.
struct Point derives (Copy, Eq) {
    /// The x coordinate.
    x: i32
    priv secret: i32
}

/// A shape with an area.
interface Shape {
    /// The area.
    fn area(self): f64
    fn name(self): String { String.from(\"shape\") }
}

Point {
    /// Makes a point.
    fn new(x: i32): Point { Point { x, secret: 0 } }
    priv fn hidden(self) {}
}

Point: Add {
    /// Adds the coordinates.
    fn add(self, other: Point): Point { Point { x: self.x + other.x, secret: 0 } }
}

Point: Shape {
    fn area(self): f64 { 0.0 }
}

fn Point.shifted(self, by: i32): Point { Point { x: self.x + by, secret: 0 } }

/// Doubles a number.
fn double(n: i32): i32 { n * 2 }
priv fn helper() {}
";

#[test]
fn items_members_and_signatures() {
    let f = Fixture::new(&[("", SHAPES)]);
    let (api, diags) = f.api(false);
    assert_eq!(diags, "");
    assert_eq!(api.modules.len(), 1);
    let m = &api.modules[0];
    assert_eq!(m.docs.as_ref().map(|d| d.text.as_str()), Some("Shapes."));
    let names: Vec<_> = m.items.iter().map(|i| (i.kind, i.name.as_str())).collect();
    assert_eq!(names, vec![(Kind::Struct, "Point"), (Kind::Interface, "Shape"), (Kind::Function, "double")]);
    let point = api.item(&[], "Point").unwrap();
    assert_eq!(point.signature, "struct Point derives (Copy, Eq)");
    assert_eq!(point.docs.as_ref().unwrap().text, "A point in the plane.");
    // The private field and the private method are left out by default.
    assert_eq!(point.members.iter().map(|x| x.signature.as_str()).collect::<Vec<_>>(), vec!["x: i32"]);
    assert_eq!(point.methods.iter().map(|x| x.signature.as_str()).collect::<Vec<_>>(), vec!["fn new(x: i32): Point"]);
    let shape = api.item(&[], "Shape").unwrap();
    assert_eq!(
        shape.methods.iter().map(|x| x.kind).collect::<Vec<_>>(),
        vec![MemberKind::Required, MemberKind::Provided]
    );
}

#[test]
fn type_pages_gather_operators_interfaces_derives_and_extensions() {
    let f = Fixture::new(&[("", SHAPES)]);
    let (api, _) = f.api(false);
    let point = api.item(&[], "Point").unwrap();
    assert_eq!(point.operators.len(), 1);
    assert_eq!(point.operators[0].display, "a + b");
    assert_eq!(point.operators[0].name, "add");
    let interfaces: Vec<_> = point.implementations.iter().map(|i| (i.interface.as_str(), i.derived)).collect();
    assert_eq!(interfaces, vec![("Copy", true), ("Eq", true), ("Add", false), ("Shape", false)]);
    let shape_impl = point.implementations.iter().find(|i| i.interface == "Shape").unwrap();
    assert_eq!(shape_impl.members.len(), 1);
    assert!(matches!(&shape_impl.target, Some(Resolution::Item { name, .. }) if name == "Shape"));
    assert_eq!(point.extensions.len(), 1);
    assert_eq!(point.extensions[0].signature, "fn Point.shifted(self, by: i32): Point");
    assert_eq!(api.modules[0].extensions.len(), 1);
}

#[test]
fn private_items_need_the_flag() {
    let f = Fixture::new(&[("", SHAPES)]);
    let (api, _) = f.api(true);
    assert!(api.item(&[], "helper").is_some_and(|i| i.private));
    let point = api.item(&[], "Point").unwrap();
    assert_eq!(point.members.len(), 2);
    assert_eq!(point.methods.len(), 2);
    let (api, _) = f.api(false);
    assert!(api.item(&[], "helper").is_none());
}

#[test]
fn generic_instance_blocks_are_marked() {
    let src = "struct Stack<T> {\n    items: List<T>\n}\nStack<T> {\n    fn len(self): usize { 0 }\n}\nStack<i32> {\n    fn sum(self): i32 { 0 }\n}\n";
    let f = Fixture::new(&[("", src)]);
    let (api, _) = f.api(false);
    let stack = api.item(&[], "Stack").unwrap();
    let by_name: Vec<_> = stack.methods.iter().map(|m| (m.name.as_str(), m.instance.as_deref())).collect();
    assert_eq!(by_name, vec![("len", None), ("sum", Some("Stack<i32>"))]);
}

#[test]
fn error_enums_show_their_messages() {
    let src =
        "/// Failures.\nerror enum Failure {\n    /// Bad.\n    Bad(i32) from \"bad {0}\"\n    Worse \"worse\"\n}\n";
    let f = Fixture::new(&[("", src)]);
    let (api, _) = f.api(false);
    let e = api.item(&[], "Failure").unwrap();
    assert_eq!(e.kind, Kind::ErrorEnum);
    assert_eq!(
        e.members.iter().map(|m| m.signature.as_str()).collect::<Vec<_>>(),
        vec!["Bad(i32) from \"bad {0}\"", "Worse \"worse\""]
    );
    assert_eq!(e.members[0].docs.as_ref().unwrap().text, "Bad.");
    // The generated `Display`, `Error`, and `From` impls are not listed.
    assert!(e.implementations.is_empty());
}

#[test]
fn links_resolve_in_the_documented_package() {
    let shapes = "/// A circle.\nstruct Circle {\n    r: f64\n}\nCircle {\n    fn area(self): f64 { 0.0 }\n}\n";
    let main = "use shapes.Circle\n\n/// Uses [Circle], [Circle.area], [`shapes.Circle`], [List], [i32], [Circle.nope], and [Nope].\nfn f() {}\n";
    let f = Fixture::new(&[("main", main), ("shapes", shapes)]);
    let (api, diags) = f.api(false);
    let doc = api.item(&["main".to_string()], "f").unwrap().docs.as_ref().unwrap();
    let kinds: Vec<_> = doc
        .links
        .iter()
        .map(|l| {
            (l.target.as_str(), format!("{:?}", l.resolution).split(['{', '(']).next().unwrap().trim().to_string())
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("Circle", "Item".to_string()),
            ("Circle.area", "Item".to_string()),
            ("shapes.Circle", "Item".to_string()),
            ("List", "External".to_string()),
            ("i32", "External".to_string()),
            ("Circle.nope", "Broken".to_string()),
            ("Nope", "Broken".to_string()),
        ]
    );
    assert_eq!(diags.matches("warning[D0001]").count(), 2, "{diags}");
    assert!(diags.contains("main.bd:3:"), "{diags}");
}

#[test]
fn links_to_private_items_are_reported_once() {
    let src = "priv struct Secret {}\n/// See [Secret].\nfn f() {}\n";
    let f = Fixture::new(&[("", src)]);
    let (_, diags) = f.api(false);
    assert_eq!(diags.matches("warning[D0002]").count(), 1, "{diags}");
    let (api, diags) = f.api(true);
    assert_eq!(diags, "");
    assert!(matches!(api.item(&[], "f").unwrap().docs.as_ref().unwrap().links[0].resolution, Resolution::Item { .. }));
}

#[test]
fn pub_use_is_a_reexport() {
    let lib = "//! The library.\npub use inner.{answer}\npub use inner\n";
    let inner = "/// The answer.\nfn answer(): i32 { 42 }\n";
    let f = Fixture::new(&[("lib", lib), ("inner", inner)]);
    let (api, _) = f.api(false);
    let lib = api.module(&["lib".to_string()]).unwrap();
    let shown: Vec<_> = lib.reexports.iter().map(|r| (r.name.as_str(), r.path.as_str())).collect();
    assert_eq!(shown, vec![("answer", "inner.answer"), ("inner", "inner")]);
    assert!(matches!(&lib.reexports[1].target, Resolution::Module(p) if p == &["inner".to_string()]));
    let answer = api.item(&["inner".to_string()], "answer").unwrap();
    assert_eq!(answer.reexported_by, vec!["pkg".to_string()]);
}

#[test]
fn extension_methods_say_where_they_come_from() {
    let text = "fn String.shout(self): String { self }\n";
    let main = "struct Name {}\nfn Name.twice(self): Name { self }\n";
    let f = Fixture::new(&[("main", main), ("text", text)]);
    let (api, _) = f.api(false);
    let ext = &api.item(&["main".to_string()], "Name").unwrap().extensions[0];
    let origin = ext.origin.as_ref().unwrap();
    assert_eq!(origin.module, "pkg");
    assert_eq!(origin.import.as_deref(), Some("use pkg.twice"));
    let shout = &api.module(&["text".to_string()]).unwrap().extensions[0];
    assert_eq!(shout.origin.as_ref().unwrap().import.as_deref(), Some("use text.shout"));
}

#[test]
fn html_pages_link_and_escape() {
    let src = "/// A <b>box</b> with `code` and [Other].\nstruct Box {\n    /// The size.\n    size: i32\n}\nstruct Other {}\n";
    let f = Fixture::new(&[("", src)]);
    let (api, _) = f.api(false);
    let pages = html::render(&api);
    let paths: Vec<_> = pages.iter().map(|p| p.path.as_str()).collect();
    assert!(paths.contains(&"index.html") && paths.contains(&"struct.Box.html") && paths.contains(&"static/doc.css"));
    assert!(paths.contains(&"search-index.js"));
    let page = &pages.iter().find(|p| p.path == "struct.Box.html").unwrap().text;
    assert!(page.contains("A &lt;b&gt;box&lt;/b&gt; with <code>code</code>"), "{page}");
    assert!(page.contains("<a href=\"struct.Other.html\">Other</a>"), "{page}");
    assert!(page.contains("id=\"field.size\""), "{page}");
    assert!(!page.contains("<script>alert"), "{page}");
}

#[test]
fn html_pages_below_the_root_reach_the_assets() {
    let f = Fixture::new(&[("geometry.shapes", "struct Circle {}\n"), ("main", "fn main() {}\n")]);
    let (api, _) = f.api(false);
    let pages = html::render(&api);
    let page = &pages.iter().find(|p| p.path == "geometry/shapes/struct.Circle.html").unwrap().text;
    assert!(page.contains("href=\"../../static/doc.css\""), "{page}");
    assert!(page.contains("href=\"../../geometry/shapes/index.html\""), "{page}");
}

#[test]
fn json_has_a_version_and_the_documented_api() {
    let f = Fixture::new(&[("", SHAPES)]);
    let (api, _) = f.api(false);
    let text = json::render(&api, &f.sources);
    assert!(text.starts_with("{\n  \"format_version\": 1,\n  \"package\": \"pkg\","), "{text}");
    assert!(text.contains("\"signature\": \"struct Point derives (Copy, Eq)\""), "{text}");
    assert!(text.contains("\"display\": \"a + b\""), "{text}");
    assert!(text.contains("\"line\": 4"), "{text}");
}

#[test]
fn coverage_counts_public_items() {
    let src = "/// Documented.\n///\n/// ```\n/// f()\n/// ```\nfn a() {}\nfn b() {}\npriv fn c() {}\n/// Doc.\nstruct S {}\nS {\n    fn m(self) {}\n}\n";
    let f = Fixture::new(&[("", src)]);
    let (rows, missing) = coverage(&f.input(false));
    assert_eq!(rows, vec![CoverageRow { module: "pkg".to_string(), items: 4, documented: 2, with_examples: 1 }]);
    assert_eq!(missing.iter().map(|m| (m.name.as_str(), m.line)).collect::<Vec<_>>(), vec![("b", 7), ("S.m", 12)]);
    let text = render_coverage("pkg", &rows, &missing);
    assert!(text.contains("total"), "{text}");
    assert!(text.contains("undocumented public items:"), "{text}");
}
