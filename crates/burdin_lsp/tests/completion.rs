//! Completion contexts, the built-ins, and the standard library (issues #819-#824; `spec/lsp.md` LS8, LS12, LS13):
//! members after a type name, a literal, and a primitive; the built-in functions and the prelude; `self` as a
//! method's first parameter; snippets; call parentheses; and the `: A, B` list after a type's name. It also covers
//! members of arrays, slices, ranges, and tuples (issue #1039) and completion beside errors (issue #1015, LS21).

mod common;

use burdin_lsp::json::Json;
use common::*;

/// The default capabilities with `snippetSupport` on; the additional-edit support they already declare is kept.
fn snippet_caps() -> Json {
    let mut caps = default_caps();
    let mut text_document = caps.get("textDocument").clone();
    let mut completion = text_document.get("completion").clone();
    let mut completion_item = completion.get("completionItem").clone();
    completion_item.set("snippetSupport", true);
    completion.set("completionItem", completion_item);
    text_document.set("completion", completion);
    caps.set("textDocument", text_document);
    caps
}

struct Session {
    client: Client,
    uri: String,
    text: String,
    version: i64,
}

/// The `completion` fixture's `main.bd`, open and analyzed without errors.
fn open(snippets: bool) -> Session {
    let root = fixture("completion");
    let caps = if snippets { snippet_caps() } else { default_caps() };
    let mut client = Client::with(&[&root], caps, Json::Null);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let diagnostics = client.diagnostics(&uri, |_| true);
    assert!(diagnostics.is_empty(), "{:?}", diagnostics.iter().map(Json::to_text).collect::<Vec<_>>());
    Session { client, uri, text, version: 1 }
}

impl Session {
    fn edit(&mut self, text: &str) {
        self.version += 1;
        self.client.change(&self.uri, self.version, text);
        self.text = text.to_string();
    }

    /// The completion items at the first `needle`, `delta` bytes in.
    fn complete(&mut self, needle: &str, delta: usize) -> Vec<Json> {
        let params = self.client.at(&self.uri, &self.text, needle, 0, delta);
        self.client.call("textDocument/completion", params).get("items").as_array().to_vec()
    }

    fn call(&mut self, method: &str, needle: &str, delta: usize) -> Json {
        let params = self.client.at(&self.uri, &self.text, needle, 0, delta);
        self.client.call(method, params)
    }
}

fn labels(items: &[Json]) -> Vec<String> {
    items.iter().map(|i| i.get("label").as_str().unwrap().to_string()).collect()
}

fn item<'a>(items: &'a [Json], label: &str) -> &'a Json {
    items
        .iter()
        .find(|i| i.get("label").as_str() == Some(label))
        .unwrap_or_else(|| panic!("`{label}` missing: {:?}", labels(items)))
}

fn has(items: &[Json], label: &str) -> bool {
    items.iter().any(|i| i.get("label").as_str() == Some(label))
}

#[test]
fn members_after_a_type_name_a_literal_and_a_primitive() {
    let mut s = open(false);
    let edited = s
        .text
        .replace("    println(", "    String.\n    \"\".\n    n.\n    5.\n    Point.\n    p.\n    mem.\n    println(");
    s.edit(&edited);
    // A type name: its associated functions, not its methods.
    let string = s.complete("String.\n", 7);
    for name in ["new", "from", "with_capacity"] {
        assert!(has(&string, name), "{name}: {:?}", labels(&string));
    }
    assert!(!has(&string, "push_str") && !has(&string, "len"), "{:?}", labels(&string));
    // A string literal is a `str`: the compiler's methods and the prelude's extensions.
    let literal = s.complete("\"\".\n", 3);
    for name in ["len", "to_string", "trim", "starts_with"] {
        assert!(has(&literal, name), "{name}: {:?}", labels(&literal));
    }
    // A primitive local, and an integer literal.
    let int = s.complete("    n.\n", 6);
    for name in ["to_string", "wrapping_add", "pow"] {
        assert!(has(&int, name), "{name}: {:?}", labels(&int));
    }
    assert!(!has(&int, "parse"), "an associated function is not a member of a value");
    assert!(has(&s.complete("    5.\n", 6), "to_string"));
    // A user type: associated functions; a value of it: fields first, then its methods and the derived ones.
    let ty = s.complete("Point.\n", 6);
    assert!(has(&ty, "origin") && !has(&ty, "sum"), "{:?}", labels(&ty));
    let value = s.complete("    p.\n", 6);
    assert_eq!(labels(&value)[..2], ["x", "y"]);
    for name in ["sum", "shift", "clone", "describe"] {
        assert!(has(&value, name), "{name}: {:?}", labels(&value));
    }
    assert!(!has(&value, "origin"));
    // A standard-library module the prelude binds.
    let mem = s.complete("    mem.\n", 8);
    assert!(has(&mem, "swap") && has(&mem, "replace"), "{:?}", labels(&mem));
    assert_eq!(s.client.shutdown(), 0);
}

#[test]
fn builtins_and_the_standard_library_answer_definition_hover_signatures_and_completion() {
    let mut s = open(false);
    // Definition: the built-ins' file, materialized beside the standard library's modules.
    let location = s.call("textDocument/definition", "println(", 2);
    let uri = location.get("uri").as_str().unwrap().to_string();
    assert!(uri.ends_with("/builtins.bd"), "{uri}");
    let path = burdin_lsp::text::uri_to_path(&uri).unwrap();
    assert!(path.to_string_lossy().contains("burdin-lsp-std-"), "{}", path.display());
    let builtins = std::fs::read_to_string(&path).unwrap();
    assert_eq!(slice(&builtins, location.get("range"), s.client.encoding), "println");
    // Hover shows the signature and the documentation.
    let hover = s.call("textDocument/hover", "println(", 2);
    let markdown = hover.at(&["contents", "value"]).as_str().unwrap().to_string();
    assert!(markdown.contains("fn println(fmt: str, ...)") && markdown.contains("standard output"), "{markdown}");
    // Signature help: `fmt: str`, then the variadic part for every later argument.
    let edited = s.text.replace("    println(\"{n} {p.sum()}\")", "    println(\"{} {}\", n, p.sum())");
    s.edit(&edited);
    let help = s.call("textDocument/signatureHelp", "println(\"", 9);
    let signature = &help.get("signatures").as_array()[0];
    let label = signature.get("label").as_str().unwrap().to_string();
    assert_eq!(label, "fn println(fmt: str, ...)");
    let parameters: Vec<String> = signature
        .get("parameters")
        .as_array()
        .iter()
        .map(|p| {
            let range = p.get("label").as_array();
            let (a, b) = (range[0].as_i64().unwrap() as usize, range[1].as_i64().unwrap() as usize);
            label[a..b].to_string()
        })
        .collect();
    assert_eq!(parameters, ["fmt: str", "..."]);
    assert_eq!(help.get("activeParameter").as_i64(), Some(0));
    let later = s.call("textDocument/signatureHelp", "p.sum())", 0);
    assert_eq!(later.get("activeParameter").as_i64(), Some(1));
    // A built-in method called on a value: `self` is the receiver, so the first argument is `other`.
    let edited = s.text.replace("    let n: i32 = 5\n", "    let n: i32 = 5\n    let k = n.wrapping_add(1)\n");
    s.edit(&edited);
    let method = s.call("textDocument/signatureHelp", "wrapping_add(1", 13);
    assert_eq!(
        method.get("signatures").as_array()[0].get("label").as_str(),
        Some("fn wrapping_add(self, other: i32): i32")
    );
    assert_eq!(method.get("activeParameter").as_i64(), Some(1));
    // An empty statement: the built-in functions, the prelude's functions, and the standard modules.
    let edited = s.text.replace("    println(", "    \n    println(");
    s.edit(&edited);
    let names = labels(&s.complete("    \n    println(", 4));
    for name in ["println", "format", "assert_eq", "hash_of", "swap", "env", "fs", "mem"] {
        assert!(names.contains(&name.to_string()), "{name}: {names:?}");
    }
    assert!(!names.iter().any(|n| n.starts_with("__")), "{names:?}");
    assert_eq!(s.client.shutdown(), 0);
}

#[test]
fn self_is_offered_only_as_the_first_parameter_of_a_method() {
    let mut s = open(false);
    let original = s.text.clone();
    let method = original.replace("    fn sum(self)", "    fn scale(\n\n    fn sum(self)");
    s.edit(&method);
    let first = s.complete("fn scale(", 9);
    for name in ["self", "mut self", "own self"] {
        assert!(has(&first, name), "{name}: {:?}", labels(&first));
    }
    // A later parameter of a method.
    let later = original.replace("    fn sum(self)", "    fn scale(a: i32, \n\n    fn sum(self)");
    s.edit(&later);
    let second = s.complete("fn scale(a: i32, ", 17);
    assert!(!has(&second, "self") && !has(&second, "mut self"), "{:?}", labels(&second));
    // The first parameter of a free function.
    let free = format!("{original}\nfn helper(\n");
    s.edit(&free);
    let items = s.complete("fn helper(", 10);
    assert!(!has(&items, "self") && !has(&items, "mut self"), "{:?}", labels(&items));
    assert_eq!(s.client.shutdown(), 0);
}

#[test]
fn snippets_follow_the_position_and_the_client() {
    let mut s = open(true);
    let original = s.text.clone();
    // At the top level: the item snippets.
    let top = format!("{original}\nma");
    s.edit(&top);
    let items = s.complete("\nma", 3);
    let main = item(&items, "main");
    assert_eq!(main.get("kind").as_i64(), Some(15));
    assert_eq!(main.get("insertTextFormat").as_i64(), Some(2));
    assert_eq!(main.get("insertText").as_str(), Some("fn main() {\n    $0\n}"));
    assert_eq!(item(&items, "fn").get("insertText").as_str(), Some("fn ${1:name}(${2}): ${3:Type} {\n    $0\n}"));
    assert_eq!(item(&items, "struct").get("insertText").as_str(), Some("struct ${1:Name} {\n    $0\n}"));
    assert_eq!(item(&items, "interface").get("insertText").as_str(), Some("interface ${1:Name} {\n    $0\n}"));
    for label in ["enum", "error enum", "impl", "impl interface", "test", "bench", "fuzz"] {
        assert_eq!(item(&items, label).get("kind").as_i64(), Some(15), "{label}");
    }
    assert_eq!(
        item(&items, "while").get("kind").as_i64(),
        Some(14),
        "a statement snippet is not offered at the top level"
    );
    // In a body: the statement snippets, and the item words only as keywords.
    let body = original.replace("    println(", "    wh\n    println(");
    s.edit(&body);
    let items = s.complete("    wh\n", 6);
    for label in ["if", "match", "for", "while"] {
        assert_eq!(item(&items, label).get("kind").as_i64(), Some(15), "{label}");
    }
    assert_eq!(item(&items, "for").get("insertText").as_str(), Some("for ${1:item} in ${2:items} {\n    $0\n}"));
    assert_eq!(item(&items, "fn").get("kind").as_i64(), Some(14));
    assert!(item(&items, "struct").get("insertText").is_null());
    assert_eq!(item(&items, "main").get("kind").as_i64(), Some(3), "the function, not the snippet");
    assert!(!has(&items, "impl interface"));
    // In an impl block: a method snippet.
    let block = original.replace("    fn sum(self)", "    f\n\n    fn sum(self)");
    s.edit(&block);
    let items = s.complete("    f\n", 5);
    assert_eq!(item(&items, "fn").get("insertText").as_str(), Some("fn ${1:name}(${2:self}): ${3:Type} {\n    $0\n}"));
    assert_eq!(s.client.shutdown(), 0);

    // Without `snippetSupport`: plain keywords, no snippet anywhere.
    let mut s = open(false);
    let top = format!("{}\nma", s.text);
    s.edit(&top);
    let items = s.complete("\nma", 3);
    assert!(items.iter().all(|i| i.get("kind").as_i64() != Some(15) && i.get("insertTextFormat").is_null()));
    assert_eq!(item(&items, "fn").get("kind").as_i64(), Some(14));
    assert_eq!(item(&items, "main").get("kind").as_i64(), Some(3), "the file's `main`, not the snippet");
    assert_eq!(s.client.shutdown(), 0);
}

#[test]
fn completing_a_function_inserts_the_call() {
    let mut s = open(true);
    let original = s.text.clone();
    let edited = original.replace("    println(", "    p.su\n    p.sh\n    Point.or\n    tw\n    println(");
    s.edit(&edited);
    let sum = s.complete("p.su\n", 4);
    let sum = item(&sum, "sum");
    assert_eq!(sum.get("insertText").as_str(), Some("sum()$0"));
    assert_eq!(sum.get("insertTextFormat").as_i64(), Some(2));
    assert!(sum.get("command").is_null(), "nothing to help with");
    let shift = s.complete("p.sh\n", 4);
    let shift = item(&shift, "shift");
    assert_eq!(shift.get("insertText").as_str(), Some("shift($0)"));
    assert_eq!(shift.at(&["command", "command"]).as_str(), Some("editor.action.triggerParameterHints"));
    assert_eq!(item(&s.complete("Point.or\n", 8), "origin").get("insertText").as_str(), Some("origin()$0"));
    assert_eq!(item(&s.complete("    tw\n", 6), "twice").get("insertText").as_str(), Some("twice($0)"));
    // A `(` already follows: the bare name.
    s.edit(&original);
    let before_paren = s.complete("p.shift(", 4);
    assert!(item(&before_paren, "shift").get("insertText").is_null());
    // A `use` path names the function without calling it.
    let in_use = s.complete("use util.twice", 9);
    assert!(item(&in_use, "twice").get("insertText").is_null());
    assert_eq!(s.client.shutdown(), 0);

    // Without `snippetSupport`: the bare name.
    let mut s = open(false);
    let edited = s.text.replace("    println(", "    p.su\n    println(");
    s.edit(&edited);
    assert!(item(&s.complete("p.su\n", 4), "sum").get("insertText").is_null());
    assert_eq!(s.client.shutdown(), 0);
}

#[test]
fn derives_and_the_derivable_interfaces() {
    let mut s = open(false);
    let original = s.text.clone();
    // After the name, before the list, nothing is offered: `:`, `repr(C)`, or the body comes next.
    let after_name = format!("{original}\nstruct Pair \n");
    s.edit(&after_name);
    assert!(s.complete("struct Pair ", 12).is_empty());
    // A bound in the generic parameters is not the list.
    let bound = format!("{original}\nenum Either<A: \n");
    s.edit(&bound);
    assert!(!has(&s.complete("enum Either<A: ", 15), "Default"));
    let generic = format!("{original}\nenum Either<A, B>: \n");
    s.edit(&generic);
    assert!(has(&s.complete("enum Either<A, B>: ", 19), "Copy"));
    let inside = format!("{original}\nstruct Pair: \n");
    s.edit(&inside);
    let items = s.complete("struct Pair: ", 13);
    for name in ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug", "Describe"] {
        assert!(has(&items, name), "{name}: {:?}", labels(&items));
    }
    for name in ["Point", "String", "i32", "main"] {
        assert!(!has(&items, name), "{name} is not derivable: {:?}", labels(&items));
    }
    // After a comma in the list, too.
    let second = format!("{original}\nstruct Pair: Clone, \n");
    s.edit(&second);
    assert!(has(&s.complete("struct Pair: Clone, ", 20), "Eq"));
    assert_eq!(s.client.shutdown(), 0);
}

/// The labels completion offers at `needle`, `delta` bytes in, once `edit` has changed the `completion` fixture's
/// `main.bd`. Each call opens its own session.
fn offered_after(edit: impl Fn(&str) -> String, needle: &str, delta: usize) -> Vec<String> {
    let mut s = open(false);
    let edited = edit(&s.text);
    s.edit(&edited);
    let items = s.complete(needle, delta);
    assert_eq!(s.client.shutdown(), 0);
    labels(&items)
}

fn contains(labels: &[String], names: &[&str]) -> bool {
    names.iter().all(|name| labels.iter().any(|l| l == name))
}

fn excludes(labels: &[String], names: &[&str]) -> bool {
    names.iter().all(|name| !labels.iter().any(|l| l == name))
}

/// `self` and a bounded generic parameter complete inside an interface impl's method (issue #852; LS12): `self.` offers
/// the impl's type's fields and methods while the body does not parse, and `s.` with `s: S` and `S: Serializer` offers
/// the methods of the bound, from the declared parameter or from the last analysis.
#[test]
fn self_and_bounded_parameters_complete_in_an_interface_impl() {
    let root = fixture("impls");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let offered = |client: &mut Client, text: &str, needle: &str, delta: usize| -> Vec<String> {
        let params = client.at(&uri, text, needle, 0, delta);
        labels(client.call("textDocument/completion", params).get("items").as_array())
    };
    // As the issue types it: `self.` alone on its line, the body unfinished, and no analysis has ever run on it.
    let own = offered(&mut client, &text, "self.\n", 5);
    assert!(contains(&own, &["name", "serialize"]), "{own:?}");
    assert!(excludes(&own, &["main", "println", "Data"]), "{own:?}");
    // A body that parses gives the same answer for `self`, and `s` is a method of its bound.
    let valid =
        text.replace("        self.\n        // s.\n", "        let n = self.name\n        s.serialize_unit()\n");
    client.change(&uri, 2, &valid);
    client.diagnostics(&uri, |_| true);
    let bound = offered(&mut client, &valid, "s.serialize_unit", 2);
    assert!(contains(&bound, &["serialize_bool", "serialize_str", "serialize_unit"]), "{bound:?}");
    assert!(excludes(&bound, &["name", "serialize"]), "{bound:?}");
    let own = offered(&mut client, &valid, "self.name", 5);
    assert!(contains(&own, &["name", "serialize"]), "{own:?}");
    // Hover names the bound's method, not `{error}`.
    let hover = client.call("textDocument/hover", client.at(&uri, &valid, "serialize_unit", 0, 3));
    let markdown = hover.at(&["contents", "value"]).as_str().unwrap().to_string();
    assert!(markdown.contains("method of `Serializer`"), "{markdown}");
    // Back to `s.` alone on its line, with the last good analysis still holding `s`.
    let broken = valid.replace("s.serialize_unit()\n", "s.\n");
    client.change(&uri, 3, &broken);
    std::thread::sleep(std::time::Duration::from_millis(200));
    let after = offered(&mut client, &broken, "s.\n", 2);
    assert!(contains(&after, &["serialize_bool", "serialize_unit"]), "{after:?}");
    assert_eq!(client.shutdown(), 0);
}

/// Completion narrows to what the position takes (issue #849; LS12): a type where a type is written, an interface
/// where a bound or an impl's interface list is written, and every name where a value is. The derive list after a
/// struct's name offers only derives.
/// A dependency completes after `.` even in a file with no `use` for it: the package's graph already has it
/// (`spec/lsp.md`, LS12).
#[test]
fn a_dependency_completes_without_a_use_in_the_file() {
    let root = fixture("incomplete");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let _ = client.diagnostics(&uri, |_| true);
    // Dropping the `use json` keeps json in the package's graph: the database loaded it when the file was opened.
    let edited = format!("{}\nfn __probe() {{\n    let _ = json.;\n}}\n", text.replacen("use json", "", 1));
    client.change(&uri, 2, &edited);
    let params = client.at(&uri, &edited, "json.;", 0, "json.".len());
    let items = client.call("textDocument/completion", params).get("items").as_array().to_vec();
    let labels: Vec<String> = items.iter().map(|i| i.get("label").as_str().unwrap().to_string()).collect();
    assert!(labels.iter().any(|label| label == "from_str"), "{labels:?}");
    assert_eq!(client.shutdown(), 0);
}

/// `use` offers the official packages, which a program reaches with no manifest entry (`spec/packages.md`, PKG15).
#[test]
fn a_use_offers_every_official_package() {
    let mut s = open(false);
    let edited = format!("use \n{}", s.text);
    s.edit(&edited);
    let labels = labels(&s.complete("use \n", "use ".len()));
    for name in ["json", "toml", "csv", "yaml", "xml", "http", "regex", "crypto", "math", "tls"] {
        assert!(labels.iter().any(|label| label == name), "`use` does not offer `{name}`: {labels:?}");
    }
}

/// The official packages are indexed (`spec/lsp.md`, LS12; `spec/packages.md`, PKG15): `json.` lists its API even
/// when nothing in the package imports json, and a bare `Client` offers `http.Client` with the `use http` that makes
/// it compile.
#[test]
fn official_packages_complete_before_any_file_imports_them() {
    let mut s = open(false);
    let edited = s.text.replace("    println(", "    let _ = json.;\n    println(");
    s.edit(&edited);
    let members = s.complete("json.;", "json.".len());
    for name in ["from_str", "to_string", "to_string_pretty"] {
        assert!(has(&members, name), "{name}: {:?}", labels(&members));
    }
    assert!(!has(&members, "Client"), "another package's item: {:?}", labels(&members));

    let edited = s.text.replace("    println(", "    Cl\n    println(");
    s.edit(&edited);
    let items = s.complete("    Cl\n", 6);
    let client = item(&items, "http.Client");
    assert_eq!(client.get("filterText").as_str(), Some("Client"));
    assert_eq!(client.get("kind").as_i64(), Some(22));
    assert!(client.get("insertText").is_null(), "a type is not called");
    let edits = client.get("additionalTextEdits").as_array();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].get("newText").as_str(), Some("use http\n"));
    let start = edits[0].at(&["range", "start"]);
    assert_eq!((start.get("line").as_i64(), start.get("character").as_i64()), (Some(1), Some(0)));
    assert_eq!(s.client.shutdown(), 0);
}

/// An official package's name completes like a module (`spec/lsp.md`, LS12): `htt` offers `http`, and accepting it adds
/// `use http` after the file's last `use` (the edit is absent once the file or the package's graph already loads it).
#[test]
fn an_official_package_name_completes_and_loads_itself() {
    let mut s = open(false);
    let edited = s.text.replace("    println(", "    htt\n    jso\n    println(");
    s.edit(&edited);
    let items = s.complete("    htt\n", 7);
    let package = item(&items, "http");
    assert_eq!(package.get("kind").as_i64(), Some(9), "a module");
    let edits = package.get("additionalTextEdits").as_array();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].get("newText").as_str(), Some("use http\n"));
    let start = edits[0].at(&["range", "start"]);
    assert_eq!((start.get("line").as_i64(), start.get("character").as_i64()), (Some(1), Some(0)));
    assert!(!has(&items, "json"), "`htt` offers only what starts with it: {:?}", labels(&items));
    let json_items = s.complete("    jso\n", 7);
    let json_edits = item(&json_items, "json").get("additionalTextEdits").as_array().to_vec();
    assert_eq!(json_edits[0].get("newText").as_str(), Some("use json\n"));
    // A file that loads the package already: the name is offered with no edit.
    let loaded = format!("use http\n{}", s.text);
    s.edit(&loaded);
    let items = s.complete("    htt\n", 7);
    assert!(item(&items, "http").get("additionalTextEdits").is_null(), "already loaded");
    assert_eq!(s.client.shutdown(), 0);
}

/// A member of an official package the file does not load adds its `use` when accepted (`spec/lsp.md`, LS12): `http.Cli`
/// offers `Client` with `use http` after the file's last `use`. Applying that edit makes the file check, and a file that
/// already loads the package gets no edit.
#[test]
fn a_member_of_an_unloaded_official_package_adds_its_use() {
    let mut s = open(false);
    let edited = s.text.replace("    println(", "    let c = http.Cli.new()\n    println(");
    s.edit(&edited);
    let items = s.complete("http.Cli.new()", "http.Cli".len());
    let client = item(&items, "Client");
    let edits = client.get("additionalTextEdits").as_array().to_vec();
    assert_eq!(edits.len(), 1, "{}", client.to_text());
    assert_eq!(edits[0].get("newText").as_str(), Some("use http\n"));
    let start = edits[0].at(&["range", "start"]);
    assert_eq!((start.get("line").as_i64(), start.get("character").as_i64()), (Some(1), Some(0)));

    // Accepting `Client` replaces `Cli` and applies the edit: the file then checks.
    let at = edited.find('\n').unwrap() + 1;
    let accepted =
        format!("{}use http\n{}", &edited[..at], &edited[at..]).replace("http.Cli.new()", "http.Client.new()");
    s.edit(&accepted);
    let left = s.client.diagnostics(&s.uri, |p| p.get("version").as_i64() == Some(s.version));
    // Only the lint about the unused `c` is left: no error, so the file checks.
    assert!(
        left.iter().all(|d| d.get("source").as_str() == Some("burdin lint")),
        "{:?}",
        left.iter().map(Json::to_text).collect::<Vec<_>>()
    );

    // Once the file loads the package, the name is offered with no edit.
    s.edit(&format!("use http\n{}", s.text));
    let items = s.complete("http.Client.new()", "http.Cli".len());
    assert!(item(&items, "Client").get("additionalTextEdits").is_null(), "already loaded");
    assert_eq!(s.client.shutdown(), 0);
}

/// A local module of an official package's name shadows it (`spec/packages.md`, PKG15): `json.` is the package's own
/// module, not the official one.
#[test]
fn a_local_module_shadows_an_official_package() {
    let root = fixture("official-shadow");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let edited = text.replace("json.local_only()", "json.");
    client.change(&uri, 2, &edited);
    let params = client.at(&uri, &edited, "json.", 0, "json.".len());
    let items = client.call("textDocument/completion", params).get("items").as_array().to_vec();
    let labels = labels(&items);
    assert!(labels.iter().any(|label| label == "local_only"), "{labels:?}");
    assert!(!labels.iter().any(|label| label == "from_str"), "the official json leaked in: {labels:?}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn completion_offers_only_what_the_position_takes() {
    let in_main = |line: &'static str| move |t: &str| t.replace("    let mut p", &format!("{line}\n    let mut p"));
    let appended = |text: &'static str| move |t: &str| format!("{t}{text}");
    let types = |edit: &dyn Fn(&str) -> String, needle: &str| -> Vec<String> {
        let labels = offered_after(edit, needle, needle.len());
        assert!(contains(&labels, &["Point", "String", "i32"]), "{needle}: {labels:?}");
        assert!(excludes(&labels, &["main", "println", "n", "twice", "Describe"]), "{needle}: {labels:?}");
        labels
    };
    // A type: a `let` annotation, a parameter, a return, a field, and a generic parameter in a body.
    types(&in_main("    let t: "), "let t: ");
    types(&appended("\nfn take(a: \n"), "take(a: ");
    types(&appended("\nfn make(): \n"), "make(): ");
    types(&appended("\nstruct Pair {\n    first: \n}\n"), "first: ");
    let generic = types(&appended("\nfn gen<Item>(x: i32) {\n    let t: \n}\n"), "let t: ");
    assert!(contains(&generic, &["Item"]), "{generic:?}");
    // An interface: an impl block's interface, a bound, a `where` clause, a `+`, `any`, and a `requires` list.
    let bounds: [(&str, &str); 6] = [
        ("\nPair: \n", "Pair: "),
        ("\nfn bounded<T: \n", "bounded<T: "),
        ("\nfn bounded2<T>(x: T): T where T: \n", "where T: "),
        ("\nfn bounded3<T: Describe + \n", "Describe + "),
        ("\nfn any_of(x: any \n", "any "),
        ("\ninterface Shape requires \n", "requires "),
    ];
    for (insert, needle) in bounds {
        let labels = offered_after(|t| format!("{t}{insert}"), needle, needle.len());
        assert!(contains(&labels, &["Describe"]), "{needle}: {labels:?}");
        assert!(
            excludes(&labels, &["Point", "String", "i32", "main", "println", "n", "twice"]),
            "{needle}: {labels:?}"
        );
    }
    // The derive list after `struct Data:` (the issue's example): derives, and nothing else.
    let derives = offered_after(appended("\nstruct Data:  {\n}\n"), "struct Data: ", 13);
    assert!(contains(&derives, &["Clone", "Ord", "Describe"]), "{derives:?}");
    assert!(excludes(&derives, &["Point", "String", "main", "println", "n"]), "{derives:?}");
    // A value still offers everything in scope.
    let value = offered_after(in_main("    let t = "), "let t = ", 8);
    assert!(contains(&value, &["n", "println", "Point", "twice"]), "{value:?}");
}

/// The `nested` fixture: a response read through member chains (issue #879).
fn open_nested() -> Session {
    let root = fixture("nested");
    let mut client = Client::with(&[&root], default_caps(), Json::Null);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let diagnostics = client.diagnostics(&uri, |_| true);
    assert!(diagnostics.is_empty(), "{:?}", diagnostics.iter().map(Json::to_text).collect::<Vec<_>>());
    Session { client, uri, text, version: 1 }
}

#[test]
fn member_chains_offer_their_members_on_the_dot_that_starts_them() {
    // Each line is typed after `let body = resp.body`, and the dot that ends it must offer these members.
    let cases: [(&str, &[&str]); 6] = [
        // One level: a local's fields and methods.
        ("    body.", &["text", "read_to_string", "size"]),
        ("    resp.", &["status", "body"]),
        // Two levels: a field of a field.
        ("    let text = resp.body.", &["text", "read_to_string", "size"]),
        ("    let text = resp.body.rea", &["read_to_string"]),
        // A chain after a method call.
        ("    client.send(req).", &["status", "body"]),
        ("    let text = client.send(req).body.", &["text", "read_to_string", "size"]),
    ];
    for (line, expected) in cases {
        let mut s = open_nested();
        let edited = s.text.replace("    let body = resp.body\n", &format!("    let body = resp.body\n{line}\n"));
        s.edit(&edited);
        let items = s.complete(&format!("{line}\n"), line.len());
        for name in expected {
            assert!(has(&items, name), "`{line}`: {name} missing: {:?}", labels(&items));
        }
        assert_eq!(s.client.shutdown(), 0);
    }
}

/// A fixture's `main.bd`, open; its diagnostics are not checked (a fixture may hold errors on purpose).
fn open_fixture(name: &str) -> Session {
    let root = fixture(name);
    let mut client = Client::with(&[&root], default_caps(), Json::Null);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    Session { client, uri, text, version: 1 }
}

/// The slice methods an array or a slice offers (`spec/lsp.md`, LS12): the extensions of `std/src/slice.bd`.
const SLICE_METHODS: &[&str] = &["first", "contains", "sort", "reverse", "binary_search", "split_at", "is_empty"];
/// The methods the compiler itself provides on an array or a slice, and the iterator adapters.
const SLICE_BUILTINS: &[&str] = &["len", "iter", "get", "last", "swap", "fill", "chunks", "windows", "to_list"];

/// The labels completion offers after typing `line` in place of `anchor` in the `arrays` fixture's `main.bd`.
fn typed_in_arrays(anchor: &str, line: &str) -> Vec<String> {
    let mut s = open_fixture("arrays");
    let edited = s.text.replacen(
        anchor,
        &format!(
            "{line}
{anchor}"
        ),
        1,
    );
    s.edit(&edited);
    let items = s.complete(
        &format!(
            "{line}
"
        ),
        line.len(),
    );
    assert_eq!(s.client.shutdown(), 0);
    labels(&items)
}

/// An array local, an array literal, a slice parameter, and a `[u8]` complete with the slice methods; a `List` does not
/// offer the ones that need `as_slice()` (issue #1039; `spec/lsp.md`, LS12).
#[test]
fn arrays_and_slices_offer_the_slice_methods_and_a_list_does_not() {
    let array_cases = [
        (typed_in_arrays("    let text", "    xs."), "an array local"),
        (typed_in_arrays("    let text", "    [1, 2]."), "an array literal"),
        (typed_in_arrays("    let text", "    let t = xs."), "an array local in a `let`"),
    ];
    for (items, what) in &array_cases {
        for name in SLICE_METHODS.iter().chain(SLICE_BUILTINS).chain(&["iter_mut", "clone"]) {
            assert!(items.iter().any(|l| l == name), "{what}: {name} missing: {items:?}");
        }
        // An array is not itself an iterator: `xs.map(...)` is E0002, and `xs.iter().map(...)` is the spelling.
        assert!(excludes(items, &["map", "filter", "is_ascii", "join", "concat"]), "{what}: {items:?}");
    }
    // A slice parameter is an `Iterator` (`spec/iterators.md`, IT5), so the adapters apply to it directly; it has no
    // `iter_mut`, and its element type picks the specialised extensions.
    let slice = labels_in_function(
        "fn sum_of(xs: [i32]): i32 {
",
        "    xs.",
    );
    for name in SLICE_METHODS.iter().chain(&["map", "filter", "fold", "len"]) {
        assert!(slice.iter().any(|l| l == name), "{name}: {slice:?}");
    }
    assert!(excludes(&slice, &["iter_mut", "is_ascii", "join", "concat", "to_ascii_lowercase"]), "{slice:?}");
    let bytes = labels_in_function(
        "fn size_of(data: [u8]): usize {
",
        "    data.",
    );
    for name in SLICE_METHODS.iter().chain(&["is_ascii", "to_ascii_lowercase", "eq_ignore_ascii_case", "len"]) {
        assert!(bytes.iter().any(|l| l == name), "{name}: {bytes:?}");
    }
    // A `List` has its own methods (its `first`, `contains`, and `sort` are built in) but not the slice extensions: a
    // list is viewed with `as_slice()`, so `list.binary_search(1)` is E0002 and is not offered.
    let list = typed_in_arrays("    let text", "    list.");
    for name in ["push", "pop", "len", "as_slice", "iter", "first", "contains", "sort"] {
        assert!(list.iter().any(|l| l == name), "{name}: {list:?}");
    }
    assert!(excludes(&list, &["binary_search", "split_at", "ends_with", "rotate_left"]), "{list:?}");
}

/// The labels completion offers after `line`, typed as the first statement of the function that starts with `header`.
fn labels_in_function(header: &str, line: &str) -> Vec<String> {
    let mut s = open_fixture("arrays");
    let edited = s.text.replacen(
        header,
        &format!(
            "{header}{line}
"
        ),
        1,
    );
    s.edit(&edited);
    let items = s.complete(
        &format!(
            "{line}
"
        ),
        line.len(),
    );
    assert_eq!(s.client.shutdown(), 0);
    labels(&items)
}

/// The `partial` fixture opened as it is, with an unknown function and a type mismatch in other functions, so no
/// analysis of it ever checked without errors.
fn open_partial() -> Session {
    open_fixture("partial")
}

/// What completion must offer after `p.`, and in an expression, however the rest of the file is broken.
fn expect_members_and_names(s: &mut Session, what: &str) {
    let members = s.complete("    p.\n", 6);
    for name in ["x", "y", "sum", "clone"] {
        assert!(has(&members, name), "{what}: `p.` lacks {name}: {:?}", labels(&members));
    }
    let names = s.complete("    let q = \n", 12);
    for name in ["p", "total", "Point", "main", "println"] {
        assert!(has(&names, name), "{what}: an expression lacks {name}: {:?}", labels(&names));
    }
}

/// Completion does not depend on the file being error-free (issue #1015; `spec/lsp.md`, LS12): an unknown name, a type
/// mismatch, or an unfinished statement elsewhere leaves the position's own answers alone, in the same function and in
/// another, even when the file never checked without errors.
#[test]
fn completion_does_not_wait_for_the_rest_of_the_file_to_be_error_free() {
    // Other functions already hold an unknown name and a mismatch; the cursor's line is the only one being typed.
    let mut s = open_partial();
    let edited = s.text.replace("    println(\"{total}\")", "    p.\n    let q = \n    println(\"{total}\")");
    s.edit(&edited);
    expect_members_and_names(&mut s, "errors in other functions");
    // The same function holds each kind of error on another line: an unknown name, a mismatch, an unfinished statement.
    for (broken, what) in [
        ("    let bad = nothing_here + 1\n", "an unknown name on another line"),
        ("    let wrong: i32 = \"text\"\n", "a type mismatch on another line"),
        ("    let half = (1 +\n", "an unfinished statement on another line"),
        ("    let half = Point.\n", "an unfinished member on another line"),
    ] {
        let mut s = open_partial();
        let edited = s
            .text
            .replace("    println(\"{total}\")", &format!("{broken}    p.\n    let q = \n    println(\"{{total}}\")"));
        s.edit(&edited);
        expect_members_and_names(&mut s, what);
        assert_eq!(s.client.shutdown(), 0);
    }
    // Errors after the cursor's line, in the same function.
    let mut s = open_partial();
    let edited = s.text.replace(
        "    println(\"{total}\")",
        "    p.\n    let q = \n    let bad = nothing_here\n    println(\"{total}\")",
    );
    s.edit(&edited);
    expect_members_and_names(&mut s, "an error on a later line");
    assert_eq!(s.client.shutdown(), 0);
}

/// Hover and definition answer at a name whose code resolves, whatever else is wrong in the file.
#[test]
fn hover_and_definition_work_beside_an_unrelated_error() {
    let mut s = open_partial();
    let hover = s.call("textDocument/hover", "p.sum()", 0);
    let text = hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string();
    assert!(text.contains("let p: Point"), "{text}");
    let hover = s.call("textDocument/hover", "p.sum()", 2);
    let text = hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string();
    assert!(text.contains("fn sum(self): i32"), "{text}");
    let definition = s.call("textDocument/definition", "p.sum()", 2);
    assert_eq!(definition.at(&["range", "start", "line"]).as_i64(), Some(6), "{}", definition.to_text());
    assert_eq!(s.client.shutdown(), 0);
}

/// A syntax error elsewhere does not take the typed tree away from the code the parser kept: the checker's program of
/// the partial tree answers types, and a dangling dot keeps its receiver (issue #1015; `spec/lsp.md`, LS21;
/// `spec/query.md`, QL10). The names typed here are new, so neither the last good analysis nor a declaration read from
/// the text knows their types.
#[test]
fn types_work_beside_a_syntax_error_elsewhere() {
    let mut s = open_partial();
    let edited = s
        .text
        .replace(
            "fn main() {",
            "fn broken() {
    let half = (1 +
}

fn main() {",
        )
        .replace(
            "    println(\"{total}\")",
            "    let n = total + 1
    n.
    println(\"{total}\")",
        );
    s.edit(&edited);
    let members = s.complete(
        "    n.
", 6,
    );
    for name in ["to_string", "wrapping_add", "pow"] {
        assert!(has(&members, name), "`n.` lacks {name}: {:?}", labels(&members));
    }
    // The type of an expression with no name to look up is the partial program's answer.
    let hover = s.call("textDocument/hover", "total + 1", 8);
    let text = hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string();
    assert!(text.contains("i32"), "{text}");
    // Beside the unfinished call and the dangling dot, the receiver's methods still complete.
    let edited = edited.replace(
        "    n.
",
        "    n.
    let m = p.sum(
    let k = 
",
    );
    s.edit(&edited);
    let members = s.complete(
        "    n.
", 6,
    );
    assert!(has(&members, "to_string"), "{:?}", labels(&members));
    let hover = s.call(
        "textDocument/hover",
        "p.sum(
",
        2,
    );
    let text = hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string();
    assert!(text.contains("fn sum(self): i32"), "{text}");
    assert_eq!(s.client.shutdown(), 0);
}

/// A file that has had a syntax error since it was opened has no typed tree at all, so the receiver of a dot is read
/// from its declaration: a struct literal, an associated function's return type, an array literal, an annotation, a
/// parameter's type, and a function's return type (issue #1015; `spec/lsp.md`, LS12).
#[test]
fn completion_works_in_a_file_that_never_parsed() {
    let mut s = open_fixture("partial-syntax");
    let cases: [(&str, usize, &[&str]); 6] = [
        ("    p.\n", 6, &["x", "y", "sum", "clone"]),
        ("    q.\n", 6, &["x", "y", "sum", "clone"]),
        ("    xs.\n", 7, &["len", "first", "sort", "iter"]),
        ("    n.\n", 6, &["to_string", "wrapping_add"]),
        ("    make().\n", 11, &["x", "sum"]),
        ("    data.\n", 9, &["len", "is_ascii", "first", "map"]),
    ];
    for (needle, delta, expected) in cases {
        let items = s.complete(needle, delta);
        for name in expected {
            assert!(has(&items, name), "`{}`: {name} missing: {:?}", needle.trim(), labels(&items));
        }
    }
    assert_eq!(s.client.shutdown(), 0);
}

/// Hover, definition, and signature help know the methods of arrays and slices: the compiler's own (`len`, `swap`,
/// `iter_mut`, which have no Burdin source and land in the built-ins' file) and the extensions of `std/src/slice.bd`
/// (`first`, `binary_search`), through an array's implicit view (issue #1039; `spec/lsp.md`, LS8, LS11, LS13).
#[test]
fn array_and_slice_methods_answer_hover_definition_and_signature_help() {
    let mut s = open_fixture("arrays");
    let edited = s.text.replace(
        "    println(",
        "    let mut ys = [1, 2, 3]\n    ys.swap(0, 1)\n    let a = xs.len()\n    let b = xs.first()\n    let c = xs.binary_search(1)\n    let d = ys.iter_mut()\n    println(",
    );
    s.edit(&edited);
    let hover_text = |s: &mut Session, needle: &str, delta: usize| {
        let hover = s.call("textDocument/hover", needle, delta);
        hover.at(&["contents", "value"]).as_str().unwrap_or("").to_string()
    };
    let len = hover_text(&mut s, "xs.len()", 3);
    assert!(len.contains("fn len(self): usize") && len.contains("method of `[T]`"), "{len}");
    let first = hover_text(&mut s, "xs.first()", 3);
    assert!(first.contains("fn [T].first(self): Option<&T>"), "{first}");
    let search = hover_text(&mut s, "xs.binary_search(1)", 3);
    assert!(search.contains("binary_search"), "{search}");
    let iter_mut = hover_text(&mut s, "ys.iter_mut()", 3);
    assert!(
        iter_mut.contains("fn iter_mut(mut self): IterMut<T>") && iter_mut.contains("method of `[T; N]`"),
        "{iter_mut}"
    );
    // Definition: a built-in lands in `builtins.bd`, an extension in `slice.bd`.
    let location = s.call("textDocument/definition", "xs.len()", 3);
    assert!(location.get("uri").as_str().unwrap().ends_with("/builtins.bd"), "{}", location.to_text());
    let location = s.call("textDocument/definition", "xs.first()", 3);
    assert!(location.get("uri").as_str().unwrap().ends_with("/slice.bd"), "{}", location.to_text());
    // Signature help marks the active parameter; the label lists `self`, which the call does not write.
    for (needle, delta, label, active) in [
        ("ys.swap(0, 1)", 11, "fn swap(mut self, a: usize, b: usize)", 2),
        ("xs.binary_search(1)", 17, "binary_search", 1),
    ] {
        let help = s.call("textDocument/signatureHelp", needle, delta);
        let shown = help.get("signatures").as_array()[0].get("label").as_str().unwrap().to_string();
        assert!(shown.contains(label), "{shown}");
        assert_eq!(help.get("activeParameter").as_i64(), Some(active), "{}", help.to_text());
    }
    assert_eq!(s.client.shutdown(), 0);
}

/// Ranges, tuples, `Chars`, and the result of `iter()` complete too (issue #1039; `spec/lsp.md`, LS12): the members the
/// compiler gives them without a declaration, and the `Iterator` adapters on its own iterators.
#[test]
fn ranges_tuples_and_iterators_have_members() {
    let cases: [(&str, &[&str], &[&str]); 5] = [
        // A range clones, compares, and gives its iterator; it is not an iterator (`r.map(...)` is E0002).
        ("r", &["clone", "cmp", "iter", "into_iter"], &["map", "filter", "contains"]),
        ("t", &["clone", "cmp"], &["iter", "map"]),
        // `Chars` is an iterator: the adapters, and `next_back`.
        ("ch", &["map", "count", "filter", "next_back", "enumerate"], &["len", "iter"]),
        // `[1, 2].iter()` is a slice, an iterator too.
        ("it", &["map", "fold", "first", "len", "contains"], &["iter_mut", "clone"]),
        ("unit", &[], &["clone", "map"]),
    ];
    for (name, expected, absent) in cases {
        let mut s = open_fixture("receivers");
        let edited = s.text.replace(
            "    println(",
            &format!(
                "    {name}.
    println("
            ),
        );
        s.edit(&edited);
        let items = s.complete(
            &format!(
                "    {name}.
"
            ),
            name.len() + 5,
        );
        for label in expected {
            assert!(has(&items, label), "`{name}.`: {label} missing: {:?}", labels(&items));
        }
        for label in absent {
            assert!(!has(&items, label), "`{name}.`: {label} is not a member: {:?}", labels(&items));
        }
        assert_eq!(s.client.shutdown(), 0);
    }
}
