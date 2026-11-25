//! The LSP 3.17 features beyond the first server (issue #1045; `spec/lsp.md` LS22-LS24): go to type definition,
//! the `use` paths of a renamed file, lazily resolved completion documentation, and formatting as the user types.
//! The lazily resolved code actions are tested with the refactorings (`refactor_actions.rs`).

mod common;

use burdin_lsp::json::Json;
use common::*;

fn selection(location: &Json, text: &str, encoding: burdin_lsp::text::Encoding) -> String {
    slice(text, location.get("range"), encoding)
}

/// A local goes to the struct it holds, and so do a function that returns one and the type's own name; a primitive's
/// type is not in the package.
#[test]
fn type_definition_goes_to_the_declaration_of_the_type() {
    let root = fixture("completion");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let declaration = text.find("struct Point").unwrap() + "struct ".len();
    for (needle, delta) in [("p.shift", 0), ("origin()", 1), ("Point.origin", 2)] {
        let params = client.at(&uri, &text, needle, 0, delta);
        let result = client.call("textDocument/typeDefinition", params);
        assert_eq!(result.get("uri").as_str(), Some(uri.as_str()), "{needle}: {}", result.to_text());
        assert_eq!(selection(&result, &text, client.encoding), "Point", "{needle}");
        let (line, _) = line_char(result.at(&["range", "start"]));
        assert_eq!(line as usize, text[..declaration].matches('\n').count(), "{needle}");
    }
    let params = client.at(&uri, &text, "twice(n)", 0, 0);
    assert!(client.call("textDocument/typeDefinition", params).get("uri").as_str() != Some(uri.as_str()));
    assert_eq!(client.shutdown(), 0);
}

fn renamed(client: &Client, result: &Json, uri: &str, text: &str) -> String {
    let changes = result.get("changes").as_object();
    assert_eq!(changes.len(), 1, "{}", result.to_text());
    assert_eq!(changes[0].0, uri);
    apply(text, changes[0].1.as_array(), client.encoding)
}

fn rename_params(old: &std::path::Path, new: &std::path::Path) -> Json {
    let file = Json::obj().with("oldUri", uri(old)).with("newUri", burdin_lsp::text::path_to_uri(new));
    Json::obj().with("files", Json::Arr(vec![file]))
}

/// Renaming a module's file rewrites the `use` paths that name it; a `use` of the module itself keeps the name the
/// code writes. The server then follows the rename (`workspace/didRenameFiles`) and the package checks again.
#[test]
fn renaming_a_module_file_rewrites_the_use_paths() {
    let root = fixture("renames");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);

    // A file: `use shapes` keeps `shapes` as the name, and an item path takes the new module.
    let old = root.join("src/shapes.bd");
    let new = root.join("src/figures.bd");
    let result = client.call("workspace/willRenameFiles", rename_params(&old, &new));
    let edited = renamed(&client, &result, &uri, &text);
    assert!(edited.starts_with("use figures as shapes\nuse figures.Square\nuse geo.inner.depth\n"), "{edited}");

    // A folder renames every module below it, nested paths included.
    let result =
        client.call("workspace/willRenameFiles", rename_params(&root.join("src/geo"), &root.join("src/world")));
    let edited = renamed(&client, &result, &uri, &text);
    assert!(edited.starts_with("use shapes\nuse shapes.Square\nuse world.inner.depth\n"), "{edited}");

    // A file moved out of `src` is not a module any more: nothing to rewrite, and an unrelated file changes nothing.
    let result = client.call("workspace/willRenameFiles", rename_params(&old, &root.join("shapes.bd")));
    assert!(result.get("changes").as_object().is_empty(), "{}", result.to_text());
    let result =
        client.call("workspace/willRenameFiles", rename_params(&root.join("README.md"), &root.join("NOTES.md")));
    assert!(result.get("changes").as_object().is_empty(), "{}", result.to_text());

    // The editor does the rename; the server reloads, and the edited file checks with the new module.
    let result = client.call("workspace/willRenameFiles", rename_params(&old, &new));
    let edited = renamed(&client, &result, &uri, &text);
    std::fs::rename(&old, &new).unwrap();
    client.change(&uri, 2, &edited);
    client.notify("workspace/didRenameFiles", rename_params(&old, &new));
    let found = client.diagnostics(&uri, |p| p.get("version").as_i64() == Some(2));
    let errors: Vec<&str> = found
        .iter()
        .filter(|d| d.get("severity").as_i64() == Some(1))
        .map(|d| d.get("message").as_str().unwrap())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(client.shutdown(), 0);
}

fn resolving_caps() -> Json {
    let mut caps = default_caps();
    caps.set_at(
        &["textDocument", "completion", "completionItem", "resolveSupport", "properties"],
        Json::Arr(vec![Json::from("documentation")]),
    );
    caps
}

/// A client that resolves documentation gets items without it, and `completionItem/resolve` adds it; one that does
/// not gets it with the item.
#[test]
fn completion_documentation_is_resolved_when_an_item_is_chosen() {
    let root = fixture("completion");
    let at_statement = |client: &mut Client, uri: &str, text: &str| {
        let params = client.at(uri, text, "println(", 0, 0);
        client.call("textDocument/completion", params).get("items").as_array().to_vec()
    };
    let mut plain = Client::new(&[&root]);
    let (uri, text) = plain.open(&root.join("src/main.bd"));
    plain.diagnostics(&uri, |_| true);
    let items = at_statement(&mut plain, &uri, &text);
    let eager = items.iter().find(|i| i.get("label").as_str() == Some("Point")).unwrap().clone();
    assert_eq!(eager.at(&["documentation", "value"]).as_str(), Some("A point."));
    assert_eq!(plain.shutdown(), 0);

    let mut client = Client::with(&[&root], resolving_caps(), Json::Null);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let items = at_statement(&mut client, &uri, &text);
    let lazy = items.iter().find(|i| i.get("label").as_str() == Some("Point")).unwrap().clone();
    assert!(lazy.get("documentation").is_null(), "{}", lazy.to_text());
    let resolved = client.call("completionItem/resolve", lazy.clone());
    assert_eq!(resolved.at(&["documentation", "value"]).as_str(), Some("A point."));
    assert_eq!(resolved.get("label"), lazy.get("label"));
    assert_eq!(resolved.get("kind"), lazy.get("kind"));
    // An item with nothing to add, or one the server does not know, comes back as it is.
    let unknown = Json::obj().with("label", "nothing_like_this");
    assert_eq!(client.call("completionItem/resolve", unknown.clone()), unknown);
    assert_eq!(client.call("completionItem/resolve", resolved.clone()), resolved);
    assert_eq!(client.shutdown(), 0);
}

fn on_type(client: &mut Client, uri: &str, text: &str, needle: &str, delta: usize, ch: &str) -> Json {
    let params = client
        .at(uri, text, needle, 0, delta)
        .with("ch", ch)
        .with("options", Json::obj().with("tabSize", 4).with("insertSpaces", true));
    client.call("textDocument/onTypeFormatting", params)
}

/// Typing `}` re-indents the line it closes, and a newline the line it ends, with the formatter's rules; nothing
/// outside that line changes, and a file that does not parse is left alone.
#[test]
fn typing_a_brace_or_a_newline_formats_the_line() {
    let root = fixture("completion");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);

    let crooked = text.replace("    println(\"{n} {p.sum()}\")\n}", "    println(\"{n} {p.sum()}\")\n        }");
    assert_ne!(crooked, text);
    client.change(&uri, 2, &crooked);
    let edits = on_type(&mut client, &uri, &crooked, "        }", 9, "}");
    assert_eq!(apply(&crooked, edits.as_array(), client.encoding), text, "{}", edits.to_text());

    // A newline formats the line before the cursor: the `let` below is indented too far.
    let crooked = text.replace("    let n: i32 = 5\n", "          let n: i32 = 5\n");
    assert_ne!(crooked, text);
    client.change(&uri, 3, &crooked);
    let edits = on_type(&mut client, &uri, &crooked, "    let mut p", 0, "\n");
    assert_eq!(apply(&crooked, edits.as_array(), client.encoding), text, "{}", edits.to_text());

    // A file with a syntax error is not formatted while the user types.
    let broken = text.replace("let n: i32 = 5", "let n: i32 = (");
    client.change(&uri, 4, &broken);
    let edits = on_type(&mut client, &uri, &broken, "    let mut p", 0, "\n");
    assert!(edits.is_null() || edits.as_array().is_empty(), "{}", edits.to_text());
    assert_eq!(client.shutdown(), 0);
}
