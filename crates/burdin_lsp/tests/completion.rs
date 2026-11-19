//! Completion contexts, the built-ins, and the standard library (issues #819-#824; `spec/lsp.md` LS8, LS12, LS13):
//! members after a type name, a literal, and a primitive; the built-in functions and the prelude; `self` as a
//! method's first parameter; snippets; call parentheses; and `derives`.

mod common;

use burdin_lsp::json::Json;
use common::*;

/// The default capabilities with `snippetSupport` on.
fn snippet_caps() -> Json {
    let mut caps = default_caps();
    let mut text_document = caps.get("textDocument").clone();
    text_document.set("completion", Json::obj().with("completionItem", Json::obj().with("snippetSupport", true)));
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
    for label in ["enum", "error enum", "impl", "impl interface", "test", "bench"] {
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
    let after_name = format!("{original}\nstruct Pair \n");
    s.edit(&after_name);
    assert_eq!(labels(&s.complete("struct Pair ", 12)), ["derives"]);
    let generic = format!("{original}\nenum Either<A, B> \n");
    s.edit(&generic);
    assert_eq!(labels(&s.complete("enum Either<A, B> ", 18)), ["derives"]);
    let inside = format!("{original}\nstruct Pair derives (\n");
    s.edit(&inside);
    let items = s.complete("struct Pair derives (", 21);
    for name in ["Copy", "Clone", "Eq", "Ord", "Hash", "Default", "Debug", "Describe"] {
        assert!(has(&items, name), "{name}: {:?}", labels(&items));
    }
    for name in ["Point", "String", "i32", "main"] {
        assert!(!has(&items, name), "{name} is not derivable: {:?}", labels(&items));
    }
    // After a comma in the list, too.
    let second = format!("{original}\nstruct Pair derives (Clone, \n");
    s.edit(&second);
    assert!(has(&s.complete("derives (Clone, ", 16), "Eq"));
    assert_eq!(s.client.shutdown(), 0);
}
