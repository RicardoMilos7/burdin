//! Code actions, code lenses, and the test commands (issues #366, #374, #662; `spec/lsp.md` LS18-LS19).

mod common;

use burdin_lsp::json::Json;
use common::*;

fn whole(text: &str) -> Json {
    burdin_lsp::text::range_json(0, 0, text.lines().count() as u32 + 1, 0)
}

fn actions(client: &mut Client, uri: &str, range: Json, only: Option<&str>) -> Vec<Json> {
    let mut context = Json::obj().with("diagnostics", Json::Arr(Vec::new()));
    if let Some(only) = only {
        context.set("only", Json::Arr(vec![Json::from(only)]));
    }
    let result = client.call("textDocument/codeAction", doc(uri).with("range", range).with("context", context));
    result.as_array().to_vec()
}

/// The text of the only file a workspace edit changes, after the edit.
fn after(client: &Client, text: &str, action: &Json) -> String {
    let changes = action.at(&["edit", "changes"]).as_object();
    assert_eq!(changes.len(), 1, "{}", action.to_text());
    apply(text, changes[0].1.as_array(), client.encoding)
}

#[test]
fn quick_fixes_for_the_errors_of_a_file() {
    let root = fixture("broken");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let fixes = actions(&mut client, &uri, whole(&text), Some("quickfix"));
    let titles: Vec<&str> = fixes.iter().map(|a| a.get("title").as_str().unwrap()).collect();
    assert_eq!(
        titles,
        [
            "add the missing method `id`",
            "make `count` mutable",
            "import `triple` from `helpers`",
            "add `mut` before this argument"
        ]
    );
    for fix in &fixes {
        assert_eq!(fix.get("kind").as_str(), Some("quickfix"));
        assert_eq!(fix.get("diagnostics").as_array().len(), 1, "each fix names the diagnostic it fixes");
    }
    // Applying every fix leaves only the type error, which has no fix.
    let all: Vec<Json> =
        fixes.iter().flat_map(|f| f.at(&["edit", "changes"]).as_object()[0].1.as_array().to_vec()).collect();
    let fixed = apply(&text, &all, client.encoding);
    assert!(fixed.contains("    fn id(self): i32 {\n        todo()\n    }\n}"));
    assert!(fixed.starts_with("use helpers.triple\n"));
    assert!(fixed.contains("let mut count = 1") && fixed.contains("bump(mut k)"));
    client.change(&uri, 2, &fixed);
    let left = client.diagnostics(&uri, |p| p.get("version").as_i64() == Some(2));
    let codes: Vec<&str> = left.iter().map(|d| d.get("code").as_str().unwrap()).collect();
    assert_eq!(codes, ["E0003"]);

    // A range picks the fixes of the diagnostics it touches.
    let line = text.lines().position(|l| l.contains("count = 2")).unwrap() as u32;
    let one = actions(&mut client, &uri, burdin_lsp::text::range_json(line, 4, line, 4), Some("quickfix"));
    assert!(one.is_empty(), "the text changed: version 2 has no `count` error");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn organize_imports_and_write_a_type() {
    let root = fixture("lints");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let organize = actions(&mut client, &uri, whole(&text), Some("source.organizeImports"));
    assert_eq!(organize.len(), 1);
    assert_eq!(organize[0].get("title").as_str(), Some("remove the unused imports"));
    assert!(after(&client, &text, &organize[0]).starts_with("use helpers.{double}\n"));

    let line = text.lines().position(|l| l.contains("let unused")).unwrap() as u32;
    let rewrite = actions(&mut client, &uri, burdin_lsp::text::range_json(line, 8, line, 8), Some("refactor.rewrite"));
    let titles: Vec<&str> = rewrite.iter().map(|a| a.get("title").as_str().unwrap()).collect();
    assert_eq!(titles, ["write the type of `unused`"]);
    assert!(after(&client, &text, &rewrite[0]).contains("let unused: i32 = 5"));
    // A `let` that already says its type, or a cursor elsewhere, offers nothing.
    let rewritten = after(&client, &text, &rewrite[0]);
    client.change(&uri, 2, &rewritten);
    client.diagnostics(&uri, |p| p.get("version").as_i64() == Some(2));
    assert!(
        actions(&mut client, &uri, burdin_lsp::text::range_json(line, 8, line, 8), Some("refactor.rewrite")).is_empty()
    );
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn lenses_run_and_debug_tests_and_main_with_exact_command_lines() {
    let root = fixture("geometry");
    let options = Json::obj().with("path", "/opt/burdin/bin/burdin");
    let mut client = Client::with(&[&root], default_caps(), options);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    let lenses = client.call("textDocument/codeLens", doc(&uri));
    golden("code-lenses", &normalize(&lenses, &root));
    let titles: Vec<&str> = lenses.as_array().iter().map(|l| l.at(&["command", "title"]).as_str().unwrap()).collect();
    assert_eq!(titles, ["▶ Run 2 tests in file", "▶ Run test", "Debug", "▶ Run test", "Debug", "▶ Run", "Debug"]);
    let run_test = &lenses.as_array()[1];
    let argv: Vec<&str> = run_test.at(&["command", "arguments"]).as_array()[0]
        .get("argv")
        .as_array()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(argv[0], "/opt/burdin/bin/burdin");
    assert_eq!(&argv[1..2], ["test"]);
    assert_eq!(&argv[3..], ["--exact", "total adds"]);

    // A module that is not the entry has no `main` lens; one without tests has none at all.
    let (math, _) = client.open(&root.join("src/math.bd"));
    assert!(client.call("textDocument/codeLens", doc(&math)).as_array().is_empty());
    // Lenses turned off.
    let settings = Json::obj().with("burdin", Json::obj().with("codeLens", Json::obj().with("enabled", false)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    assert!(client.call("textDocument/codeLens", doc(&uri)).as_array().is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn the_test_command_reports_a_missing_compiler_and_rejects_other_commands() {
    let root = fixture("geometry");
    let missing = root.join("no-such-burdin");
    let options = Json::obj().with("path", missing.to_string_lossy().as_ref());
    let mut client = Client::with(&[&root], default_caps(), options);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    let lenses = client.call("textDocument/codeLens", doc(&uri));
    let command = lenses.as_array()[0].get("command").clone();
    assert_eq!(command.get("command").as_str(), Some("burdin.runTests"));
    let params =
        Json::obj().with("command", command.get("command").clone()).with("arguments", command.get("arguments").clone());
    let response = client.request("workspace/executeCommand", params);
    assert_eq!(
        response.at(&["error", "code"]).as_i64(),
        Some(burdin_lsp::rpc::code::REQUEST_FAILED),
        "{}",
        response.to_text()
    );
    assert!(response.at(&["error", "message"]).as_str().unwrap().contains("cannot run"));
    // The run was reported as progress.
    let begin = client.wait_for("$/progress", |p| p.at(&["value", "kind"]).as_str() == Some("begin"));
    assert_eq!(begin.at(&["value", "title"]).as_str(), Some("burdin test"));
    client.wait_for("$/progress", |p| p.at(&["value", "kind"]).as_str() == Some("end"));

    let unknown = client.request("workspace/executeCommand", Json::obj().with("command", "burdin.format"));
    assert_eq!(unknown.at(&["error", "code"]).as_i64(), Some(burdin_lsp::rpc::code::INVALID_PARAMS));
    // Only `burdin test` runs: a lens argument cannot run anything else.
    let argument = Json::obj()
        .with("argv", Json::Arr(vec![Json::from("burdin"), Json::from("run")]))
        .with("root", burdin_query::package::source_path_text(&root));
    let other = client.request(
        "workspace/executeCommand",
        Json::obj().with("command", "burdin.runTest").with("arguments", Json::Arr(vec![argument])),
    );
    assert_eq!(other.at(&["error", "code"]).as_i64(), Some(burdin_lsp::rpc::code::INVALID_PARAMS));
    assert_eq!(client.shutdown(), 0);
}
