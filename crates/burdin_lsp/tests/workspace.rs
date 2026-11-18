//! Workspaces (issues #372, #376; `spec/lsp.md` LS4-LS6): several roots and the package graph, folders added and
//! removed, files created on disk, configuration pulled from the client, and progress.

mod common;

use burdin_lsp::json::Json;
use common::*;

fn package<'a>(status: &'a Json, name: &str) -> &'a Json {
    status
        .get("packages")
        .as_array()
        .iter()
        .find(|p| p.get("name").as_str() == Some(name))
        .unwrap_or_else(|| panic!("no package `{name}` in {}", status.to_text()))
}

fn checked(status: &Json, name: &str) -> Vec<String> {
    package(status, name)
        .at(&["last", "bodiesChecked"])
        .as_array()
        .iter()
        .map(|b| b.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_dependency_in_another_root_is_navigable_and_not_checked_again() {
    let multi = fixture("multi");
    let (app, lib) = (multi.join("app"), multi.join("lib"));
    let mut client = Client::new(&[&app, &lib]);
    let (uri, text) = client.open(&app.join("src/main.bd"));
    assert!(client.diagnostics(&uri, |_| true).is_empty());

    // Go to the dependency's declaration, hover its docs.
    let definition = client.call("textDocument/definition", client.at(&uri, &text, "answer()", 0, 0));
    assert_eq!(normalize(definition.get("uri"), &multi).as_str(), Some("ROOT/lib/src/lib.bd"));
    let hover = client.call("textDocument/hover", client.at(&uri, &text, "answer()", 0, 0));
    assert!(hover.at(&["contents", "value"]).as_str().unwrap().contains("The answer."));
    // A private item of the dependency is not offered.
    let edited = text.replace("use lib.answer", "use lib.");
    client.change(&uri, 2, &edited);
    let result = client.call("textDocument/completion", client.at(&uri, &edited, "use lib.", 0, 8));
    let labels: Vec<&str> = result.get("items").as_array().iter().map(|i| i.get("label").as_str().unwrap()).collect();
    assert_eq!(labels, ["answer"]);

    // An edit of the app checks the app's bodies only: the dependency's are reused.
    let edited = text.replace("{answer()}", "{answer() + 1}");
    client.change(&uri, 3, &edited);
    client.diagnostics(&uri, |p| p.get("version").as_i64() == Some(3));
    let status = client.call("burdin/status", Json::Null);
    assert_eq!(checked(&status, "app"), ["main.main"]);
    assert!(package(&status, "app").at(&["last", "bodiesReused"]).as_i64().unwrap() > 0);

    // An edit of the dependency reaches the app: a broken signature is reported where it is used.
    let (lib_uri, lib_text) = client.open(&lib.join("src/lib.bd"));
    let changed = lib_text.replace("fn answer(): i32 {\n    42", "fn answer(): bool {\n    true");
    client.change(&lib_uri, 2, &changed);
    let in_app = client.diagnostics(&uri, |p| !p.get("diagnostics").as_array().is_empty());
    assert!(
        in_app.iter().any(|d| d.get("code").as_str() == Some("E0003") || d.get("code").as_str() == Some("E0013")),
        "{in_app:?}"
    );
    // References of the dependency's function include the app's use.
    let mut params = client.at(&lib_uri, &changed, "fn answer", 0, 3);
    params.set("context", Json::obj().with("includeDeclaration", false));
    let references = client.call("textDocument/references", params);
    let files: Vec<String> =
        references.as_array().iter().map(|l| normalize(l.get("uri"), &multi).as_str().unwrap().to_string()).collect();
    assert!(files.contains(&"ROOT/app/src/main.bd".to_string()), "{files:?}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn folders_come_and_go_and_new_files_are_picked_up() {
    let geometry = fixture("geometry");
    let lints = fixture("lints");
    let mut client = Client::new(&[&geometry]);
    let status = client.call("burdin/status", Json::Null);
    assert_eq!(status.get("folders").as_array().len(), 1);

    let added = Json::obj().with("uri", uri(&lints)).with("name", "lints");
    let event = Json::obj().with("added", Json::Arr(vec![added.clone()])).with("removed", Json::Arr(Vec::new()));
    client.notify("workspace/didChangeWorkspaceFolders", Json::obj().with("event", event));
    let found = client.call("workspace/symbol", Json::obj().with("query", "half"));
    assert_eq!(found.as_array().len(), 1, "the new folder's package is searched");
    let event = Json::obj().with("added", Json::Arr(Vec::new())).with("removed", Json::Arr(vec![added]));
    client.notify("workspace/didChangeWorkspaceFolders", Json::obj().with("event", event));
    let found = client.call("workspace/symbol", Json::obj().with("query", "half"));
    assert!(found.as_array().is_empty(), "a removed folder's package is dropped");

    // A module created on disk: the server learns of it from the watched-file notification.
    let (uri_main, text) = client.open(&geometry.join("src/main.bd"));
    client.diagnostics(&uri_main, |_| true);
    let edited = text.replace("use math.add", "use math.add\nuse extra.seven");
    client.change(&uri_main, 2, &edited);
    let missing = client.diagnostics(&uri_main, |p| p.get("version").as_i64() == Some(2));
    assert!(missing.iter().any(|d| d.get("code").as_str().is_some_and(|c| c.starts_with('E'))), "{missing:?}");
    let extra = geometry.join("src/extra.bd");
    std::fs::write(&extra, "fn seven(): i32 {\n    7\n}\n").unwrap();
    let change = Json::obj().with("uri", uri(&extra)).with("type", 1);
    client.notify("workspace/didChangeWatchedFiles", Json::obj().with("changes", Json::Arr(vec![change])));
    let fixed = client.diagnostics(&uri_main, |p| {
        !p.get("diagnostics").as_array().iter().any(|d| d.get("code").as_str().is_some_and(|c| c.starts_with('E')))
    });
    assert!(fixed.iter().all(|d| d.get("code").as_str().is_some_and(|c| c.starts_with('L'))), "{fixed:?}");
    // The server asked to watch Burdin files and manifests.
    let registration =
        client.server_requests.iter().find(|r| r.get("method").as_str() == Some("client/registerCapability"));
    let text = registration.expect("a watcher registration").to_text();
    assert!(text.contains("**/*.bd") && text.contains("**/burdin.toml"), "{text}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn configuration_is_pulled_when_the_client_only_says_it_changed() {
    let root = fixture("lints");
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    assert_eq!(client.diagnostics(&uri, |_| true).len(), 2);
    client.configuration = Json::obj().with("diagnostics", Json::obj().with("lints", false));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", Json::Null));
    let cleared = client.diagnostics(&uri, |p| p.get("diagnostics").as_array().is_empty());
    assert!(cleared.is_empty());
    let asked =
        client.server_requests.iter().filter(|r| r.get("method").as_str() == Some("workspace/configuration")).count();
    assert!(asked >= 1);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn loading_a_package_reports_progress() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let begin = client.wait_for("$/progress", |p| p.at(&["value", "kind"]).as_str() == Some("begin"));
    let token = begin.get("token").clone();
    assert!(begin.at(&["value", "title"]).as_str().unwrap().contains("burdin"));
    client.wait_for("$/progress", |p| *p.get("token") == token && p.at(&["value", "kind"]).as_str() == Some("end"));
    // The token was created first.
    assert!(
        client.server_requests.iter().any(|r| r.get("method").as_str() == Some("window/workDoneProgress/create")
            && *r.at(&["params", "token"]) == token)
    );
    assert_eq!(client.shutdown(), 0);

    // A client without progress support gets none.
    let mut client = Client::with(&[&root], Json::obj(), Json::Null);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    client.call("burdin/status", Json::Null);
    assert!(!client.inbox.iter().any(|m| m.get("method").as_str() == Some("$/progress")));
    assert!(client.server_requests.is_empty(), "{:?}", client.server_requests);
    assert_eq!(client.shutdown(), 0);
}
