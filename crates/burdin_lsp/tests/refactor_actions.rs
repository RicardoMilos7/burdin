//! Change signature, move item, and structural search and replace in the editor (issue #1042; `spec/lsp.md` LS18,
//! `spec/refactor.md` RF7): each edit equals what `burdin refactor` and `burdin ssr` compute for the same input (the
//! engine is the same library, called here directly), and a refusal shows no action and goes to the log.

mod common;

use burdin_lsp::json::Json;
use burdin_query::{Db, EntryMode, Options};
use burdin_refactor::signature::Change;
use burdin_refactor::{Engine, FileChange, Refusal};
use common::*;
use std::path::Path;

/// What the engine computes for the package at `root`, or why it refuses.
fn engine(
    root: &Path,
    run: impl FnOnce(&mut Engine<'_>) -> Result<Vec<FileChange>, Refusal>,
) -> Result<Vec<FileChange>, String> {
    let mut db = Db::open(root, Options { mode: EntryMode::Test, ..Options::default() }).unwrap();
    let mut engine = Engine::new(&mut db).map_err(|r| r.0)?;
    run(&mut engine).map_err(|r| r.0)
}

fn path_key(root: &Path, file: &str) -> String {
    burdin_query::package::source_path_text(&root.join(file))
}

/// The byte span of `needle` (the `nth` occurrence) in `text`, `delta` bytes in.
fn at(text: &str, needle: &str, nth: usize, delta: usize) -> u32 {
    (text.match_indices(needle).nth(nth).unwrap().0 + delta) as u32
}

/// The actions of one kind at a cursor `delta` bytes into `needle`'s first occurrence.
fn actions_at(client: &mut Client, uri: &str, text: &str, needle: &str, delta: usize, only: &str) -> Vec<Json> {
    let p = client.pos(text, needle, 0, delta);
    actions(client, uri, Json::obj().with("start", p.clone()).with("end", p), only)
}

fn actions(client: &mut Client, uri: &str, range: Json, only: &str) -> Vec<Json> {
    let context =
        Json::obj().with("diagnostics", Json::Arr(Vec::new())).with("only", Json::Arr(vec![Json::from(only)]));
    client.call("textDocument/codeAction", doc(uri).with("range", range).with("context", context)).as_array().to_vec()
}

fn titles(actions: &[Json]) -> Vec<String> {
    actions.iter().map(|a| a.get("title").as_str().unwrap().to_string()).collect()
}

fn action<'a>(actions: &'a [Json], title: &str) -> &'a Json {
    actions.iter().find(|a| a.get("title").as_str() == Some(title)).unwrap_or_else(|| panic!("no action `{title}`"))
}

/// Applies a workspace edit to the files the engine changed and compares every one: the edit changes the same files
/// and gives the same texts.
fn assert_same(client: &Client, edit: &Json, expected: &[FileChange]) {
    let changes = edit.get("changes").as_object();
    assert_eq!(changes.len(), expected.len(), "{}", edit.to_text());
    for change in expected {
        let wanted = uri(Path::new(&change.path));
        let entry = changes.iter().find(|(u, _)| *u == wanted).unwrap_or_else(|| panic!("no edit for {wanted}"));
        assert_eq!(apply(&change.before, entry.1.as_array(), client.encoding), change.after, "{}", change.path);
    }
}

/// Waits for a log message that holds `needle`.
fn logged(client: &mut Client, needle: &str) -> String {
    let params =
        client.wait_for("window/logMessage", |p| p.get("message").as_str().is_some_and(|m| m.contains(needle)));
    params.get("message").as_str().unwrap().to_string()
}

fn open_ready(client: &mut Client, root: &Path, file: &str) -> (String, String) {
    let (uri, text) = client.open(&root.join(file));
    client.diagnostics(&uri, |_| true);
    (uri, text)
}

#[test]
fn change_signature_actions_edit_what_the_engine_edits() {
    let root = fixture("refactor");
    let mut client = Client::new(&[&root]);
    let (uri, text) = open_ready(&mut client, &root, "src/main.bd");
    let main = path_key(&root, "src/main.bd");
    let function = at(&text, "fn scale", 0, 3);

    // On the function's name: add a parameter, and nothing about one parameter.
    let found = actions_at(&mut client, &uri, &text, "scale(value", 0, "refactor.rewrite");
    assert_eq!(titles(&found), ["add a parameter to `scale`"]);
    let add = Change { add: vec![("extra".into(), "i32".into(), "0".into())], ..Change::default() };
    let expected =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &add)).unwrap();
    assert_same(&client, action(&found, "add a parameter to `scale`").get("edit"), &expected);

    // On a parameter: remove it, or move it past its neighbor. `note` is unused, so it can go.
    let found = actions_at(&mut client, &uri, &text, "note: i32", 1, "refactor.rewrite");
    assert_eq!(titles(&found), ["remove the parameter `note`", "move the parameter `note` before `factor`"]);
    let remove = Change { remove: vec!["note".into()], ..Change::default() };
    let expected =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &remove)).unwrap();
    assert_same(&client, action(&found, "remove the parameter `note`").get("edit"), &expected);
    let reorder = Change { reorder: Some(vec!["value".into(), "note".into(), "factor".into()]), ..Change::default() };
    let expected =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &reorder)).unwrap();
    assert_same(&client, action(&found, "move the parameter `note` before `factor`").get("edit"), &expected);
    assert!(expected[0].after.contains("fn scale(value: i32, note: i32, factor: i32)"), "{}", expected[0].after);
    assert!(expected[0].after.contains("scale(2, 0, 3)"), "the calls follow the new order: {}", expected[0].after);

    // The body uses `value`: the engine refuses to remove it, so there is no such action, and the reason is logged.
    let found = actions_at(&mut client, &uri, &text, "value: i32", 1, "refactor.rewrite");
    assert_eq!(titles(&found), ["move the parameter `value` after `factor`"]);
    let remove = Change { remove: vec!["value".into()], ..Change::default() };
    let refusal =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &remove)).unwrap_err();
    let message = logged(&mut client, "remove the parameter `value`");
    assert!(message.contains(&refusal), "{message} should hold {refusal}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn move_item_actions_list_the_modules_and_edit_what_the_engine_edits() {
    let root = fixture("refactor");
    let mut client = Client::new(&[&root]);
    let (uri, text) = open_ready(&mut client, &root, "src/util.bd");
    let util = path_key(&root, "src/util.bd");
    let triple = at(&text, "triple", 0, 0);
    let found = actions_at(&mut client, &uri, &text, "triple", 1, "refactor.move");
    let wanted = "move `triple` to the module `extra`";
    assert!(titles(&found).contains(&wanted.to_string()), "{:?}", titles(&found));
    assert!(!titles(&found).iter().any(|t| t.contains("`util`")), "the item's own module is not offered");
    let expected = engine(&root, |e| burdin_refactor::move_item::item(e, &util, triple, triple + 1, "extra")).unwrap();
    assert_eq!(expected.len(), 2, "the old module and the new one change");
    assert_same(&client, action(&found, wanted).get("edit"), &expected);

    // `main` cannot leave the entry file (the program would have none): no action, and a log line.
    let (main_uri, main_text) = open_ready(&mut client, &root, "src/main.bd");
    let main = path_key(&root, "src/main.bd");
    let entry = at(&main_text, "fn main", 0, 3);
    let found = actions_at(&mut client, &main_uri, &main_text, "main()", 0, "refactor.move");
    assert!(found.is_empty(), "{:?}", titles(&found));
    let refusal = engine(&root, |e| burdin_refactor::move_item::item(e, &main, entry, entry + 1, "extra")).unwrap_err();
    let message = logged(&mut client, "move `main`");
    assert!(message.contains(&refusal), "{message}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn commands_take_what_a_code_action_cannot_ask_for() {
    let root = fixture("refactor");
    let mut client = Client::new(&[&root]);
    let (uri, text) = open_ready(&mut client, &root, "src/main.bd");
    let main = path_key(&root, "src/main.bd");
    let function = at(&text, "fn scale", 0, 3);
    let position = client.pos(&text, "fn scale", 0, 3);

    // Rename a parameter and add one with a type and default of the caller's choice.
    let argument = Json::obj()
        .with("uri", uri.as_str())
        .with("position", position.clone())
        .with("rename", Json::Arr(vec![Json::obj().with("from", "note").with("to", "memo")]))
        .with("add", Json::Arr(vec![Json::obj().with("name", "limit").with("type", "u8").with("default", "9")]));
    let result = client.call(
        "workspace/executeCommand",
        Json::obj().with("command", "burdin.changeSignature").with("arguments", Json::Arr(vec![argument])),
    );
    let change = Change {
        rename: vec![("note".into(), "memo".into())],
        add: vec![("limit".into(), "u8".into(), "9".into())],
        ..Change::default()
    };
    let expected =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &change)).unwrap();
    assert_same(&client, &result, &expected);
    assert!(expected[0].after.contains("fn scale(value: i32, factor: i32, memo: i32, limit: u8 = 9)"));

    // A refusal is an error response, and the reason is logged.
    let argument = Json::obj()
        .with("uri", uri.as_str())
        .with("position", position)
        .with("remove", Json::Arr(vec![Json::from("value")]));
    let response = client.request(
        "workspace/executeCommand",
        Json::obj().with("command", "burdin.changeSignature").with("arguments", Json::Arr(vec![argument])),
    );
    let message = response.at(&["error", "message"]).as_str().unwrap().to_string();
    assert!(message.contains("uses `value`"), "{message}");
    logged(&mut client, "burdin.changeSignature");

    // Move an item to a module the caller names.
    let (util_uri, util_text) = open_ready(&mut client, &root, "src/util.bd");
    let util = path_key(&root, "src/util.bd");
    let triple = at(&util_text, "triple", 0, 0);
    let argument = Json::obj()
        .with("uri", util_uri.as_str())
        .with("position", client.pos(&util_text, "triple", 0, 1))
        .with("to", "extra");
    let result = client.call(
        "workspace/executeCommand",
        Json::obj().with("command", "burdin.moveItem").with("arguments", Json::Arr(vec![argument])),
    );
    let expected = engine(&root, |e| burdin_refactor::move_item::item(e, &util, triple, triple + 1, "extra")).unwrap();
    assert_same(&client, &result, &expected);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn search_and_replace_is_a_command_and_a_refusal_is_logged() {
    let root = fixture("refactor");
    let mut client = Client::new(&[&root]);
    open_ready(&mut client, &root, "src/main.bd");
    let pattern = burdin_refactor::ssr::parse("scale($a, $b, $c)").unwrap();
    let expected = engine(&root, |e| Ok(burdin_refactor::ssr::replace(e, &pattern, "$a * $b")?.changes)).unwrap();
    for command in ["burdin.searchReplace", "burdin.structuralSearchReplace"] {
        let argument = Json::obj()
            .with("pattern", "scale($a, $b, $c)")
            .with("replacement", "$a * $b")
            .with("root", burdin_query::package::source_path_text(&root));
        let result = client.call(
            "workspace/executeCommand",
            Json::obj().with("command", command).with("arguments", Json::Arr(vec![argument])),
        );
        assert_same(&client, &result, &expected);
    }

    // A pattern that does not parse is refused, and the command's answer says why.
    let argument = Json::obj()
        .with("pattern", "scale($a,")
        .with("replacement", "$a")
        .with("root", burdin_query::package::source_path_text(&root));
    let response = client.request(
        "workspace/executeCommand",
        Json::obj().with("command", "burdin.searchReplace").with("arguments", Json::Arr(vec![argument])),
    );
    assert_eq!(response.at(&["error", "code"]).as_i64(), Some(burdin_lsp::rpc::code::INVALID_PARAMS));
    assert_eq!(client.shutdown(), 0);
}

/// A client that asks for the edit of a refactoring when it is chosen (`codeAction/resolve`, LS23).
fn lazy_client(root: &Path) -> Client {
    let mut caps = default_caps();
    caps.set_at(&["textDocument", "codeAction", "resolveSupport", "properties"], Json::Arr(vec![Json::from("edit")]));
    Client::with(&[root], caps, Json::Null)
}

#[test]
fn refactorings_are_resolved_when_chosen() {
    let root = fixture("refactor");
    let mut client = lazy_client(&root);
    let (uri, text) = open_ready(&mut client, &root, "src/main.bd");
    let main = path_key(&root, "src/main.bd");
    let function = at(&text, "fn scale", 0, 3);

    // The menu holds the actions that fit the cursor, with no edit yet.
    let found = actions_at(&mut client, &uri, &text, "value: i32", 1, "refactor.rewrite");
    assert_eq!(
        titles(&found),
        ["remove the parameter `value`", "move the parameter `value` after `factor`"],
        "the syntax decides the menu; the engine has not run"
    );
    assert!(found.iter().all(|a| a.get("edit").is_null() && !a.get("data").is_null()));

    // Resolving fills the edit, which is the engine's.
    let chosen = action(&found, "move the parameter `value` after `factor`").clone();
    let resolved = client.call("codeAction/resolve", chosen);
    let reorder = Change { reorder: Some(vec!["factor".into(), "value".into(), "note".into()]), ..Change::default() };
    let expected =
        engine(&root, |e| burdin_refactor::signature::change(e, &main, function, function + 1, &reorder)).unwrap();
    assert_same(&client, resolved.get("edit"), &expected);
    assert_eq!(resolved.get("title").as_str(), Some("move the parameter `value` after `factor`"));
    // An action that has its edit comes back as it is.
    assert_eq!(client.call("codeAction/resolve", resolved.clone()), resolved);

    // A refusal answers the resolve with the reason, and the log has it.
    let refused = action(&found, "remove the parameter `value`").clone();
    let response = client.request("codeAction/resolve", refused);
    assert!(response.at(&["error", "message"]).as_str().unwrap().contains("uses `value`"), "{}", response.to_text());
    logged(&mut client, "remove the parameter `value`");
    assert_eq!(client.shutdown(), 0);
}
