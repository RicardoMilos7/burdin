//! Diagnostics (issues #310, #375; `spec/lsp.md` LS7): published after a debounce for the latest version, with
//! the compiler's codes and spans, cleared when fixed, lints by configuration, and the ranges of a line with
//! non-ASCII text.

mod common;

use burdin_lsp::json::Json;
use burdin_lsp::text::Encoding;
use common::*;

fn codes(diagnostics: &[Json]) -> Vec<String> {
    diagnostics.iter().map(|d| d.get("code").as_str().unwrap_or("").to_string()).collect()
}

#[test]
fn every_error_of_a_file_is_published_with_its_code_and_span() {
    let root = fixture("broken");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let diagnostics = client.diagnostics(&uri, |_| true);
    assert_eq!(codes(&diagnostics), ["E0140", "E0005", "E0003", "E0002", "E0114"]);
    let spans: Vec<String> = diagnostics.iter().map(|d| slice(&text, d.get("range"), client.encoding)).collect();
    assert_eq!(spans[1], "count");
    assert_eq!(spans[2], "\"text\"");
    assert_eq!(spans[3], "triple");
    assert_eq!(spans[4], "k");
    golden("broken-diagnostics", &normalize(&Json::Arr(diagnostics), &root));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn a_fix_clears_the_diagnostics_and_a_syntax_error_is_reported() {
    let root = fixture("broken");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    assert_eq!(client.diagnostics(&uri, |_| true).len(), 5);
    let fixed = text
        .replace("    fn name(self): String {\n        String.from(\"cat\")\n    }\n", "    fn name(self): String {\n        String.from(\"cat\")\n    }\n\n    fn id(self): i32 {\n        7\n    }\n")
        .replace("let count = 1", "let mut count = 1")
        .replace("let text: i32 = \"text\"", "let text: i32 = 3")
        .replace("let tripled = triple(count)", "let tripled = count * 3")
        .replace("bump(k)", "bump(mut k)");
    client.change(&uri, 2, &fixed);
    let published = client.wait_for("textDocument/publishDiagnostics", |p| {
        p.get("uri").as_str() == Some(uri.as_str()) && p.get("version").as_i64() == Some(2)
    });
    assert_eq!(published.get("diagnostics").as_array().len(), 0, "{}", published.to_text());

    let broken = fixed.replace("fn main() {", "fn main() {\n    let = 1");
    client.change(&uri, 3, &broken);
    let published = client.wait_for("textDocument/publishDiagnostics", |p| p.get("version").as_i64() == Some(3));
    let diagnostics = published.get("diagnostics").as_array();
    assert!(!diagnostics.is_empty());
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("code").as_str().unwrap().starts_with('E') && d.get("severity").as_i64() == Some(1))
    );
    assert_eq!(
        line_char(diagnostics[0].at(&["range", "start"])).0,
        broken.lines().position(|l| l == "    let = 1").unwrap() as u32
    );
    assert_eq!(client.shutdown(), 0);
}

/// Edits in quick succession are checked once, after the debounce, and the publication carries the last version.
#[test]
fn rapid_edits_publish_once_for_the_latest_version() {
    let root = fixture("broken");
    let options = Json::obj().with("diagnostics", Json::obj().with("debounceMs", 300));
    let mut client = Client::with(&[&root], default_caps(), options);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    client.diagnostics(&uri, |_| true);
    let mut messages = Vec::new();
    for version in 2..=6 {
        let edited = text.replace("let text: i32 = \"text\"", &format!("let text: i32 = {version}"));
        messages.push(burdin_lsp::rpc::notification(
            "textDocument/didChange",
            Json::obj()
                .with("textDocument", Json::obj().with("uri", uri.as_str()).with("version", version))
                .with("contentChanges", Json::Arr(vec![Json::obj().with("text", edited)])),
        ));
    }
    client.send_all(&messages);
    let published = client.wait_for("textDocument/publishDiagnostics", |p| p.get("uri").as_str() == Some(uri.as_str()));
    assert_eq!(published.get("version").as_i64(), Some(6), "only the last version is published");
    assert_eq!(codes(published.get("diagnostics").as_array()), ["E0140", "E0005", "E0002", "E0114"]);
    // Requests are answered while the analysis waits: a status needs no analysis.
    client.call("burdin/status", Json::Null);
    assert!(
        !client.inbox.iter().any(|m| m.get("method").as_str() == Some("textDocument/publishDiagnostics")
            && m.at(&["params", "version"]).as_i64().is_some_and(|v| v < 6)),
        "an older version was published"
    );
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn lints_are_published_unless_turned_off() {
    let root = fixture("lints");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let diagnostics = client.diagnostics(&uri, |_| true);
    let mut lint_codes = codes(&diagnostics);
    lint_codes.sort();
    assert_eq!(lint_codes, ["L0006", "L0007"]);
    for d in &diagnostics {
        assert_eq!(d.get("source").as_str(), Some("burdin lint"));
        assert_eq!(
            d.get("tags").as_array().first().and_then(Json::as_i64),
            Some(1),
            "unused code is marked unnecessary"
        );
    }
    let unused = diagnostics.iter().find(|d| d.get("code").as_str() == Some("L0006")).unwrap();
    assert_eq!(slice(&text, unused.get("range"), client.encoding), "half");

    let settings = Json::obj().with("burdin", Json::obj().with("diagnostics", Json::obj().with("lints", false)));
    client.notify("workspace/didChangeConfiguration", Json::obj().with("settings", settings));
    let published = client.diagnostics(&uri, |p| p.get("diagnostics").as_array().is_empty());
    assert!(published.is_empty());
    assert_eq!(client.shutdown(), 0);
}

/// An error after `é` and `😀` on its line: its range counts in the negotiated encoding.
#[test]
fn diagnostic_ranges_after_non_ascii_text_follow_the_encoding() {
    for offer in ["utf-16", "utf-8"] {
        let root = fixture("geometry");
        let mut caps = default_caps();
        caps.set("general", Json::obj().with("positionEncodings", Json::Arr(vec![Json::from(offer)])));
        let mut client = Client::with(&[&root], caps, Json::Null);
        let (uri, text) = client.open(&root.join("src/main.bd"));
        let edited = text.replace("println(\"{label} 😀 {sum}\")", "println(\"é {label} 😀 {total(sum, true)}\")");
        client.change(&uri, 2, &edited);
        let diagnostics = client
            .diagnostics(&uri, |p| p.get("version").as_i64() == Some(2) && !p.get("diagnostics").as_array().is_empty());
        let e = diagnostics.iter().find(|d| d.get("code").as_str() == Some("E0003")).expect("a type error");
        let start = line_char(e.at(&["range", "start"]));
        let line = edited.lines().nth(start.0 as usize).unwrap();
        let byte = line.find("true").unwrap();
        let expected = if client.encoding == Encoding::Utf8 { byte } else { line[..byte].encode_utf16().count() };
        assert_eq!(start.1 as usize, expected, "{offer}");
        assert_eq!(slice(&edited, e.get("range"), client.encoding), "true");
        assert_eq!(client.shutdown(), 0);
    }
}

#[test]
fn closing_a_file_outside_any_package_clears_its_diagnostics() {
    // A directory with no `burdin.toml` (and none above it in the temporary directory).
    let dir = fresh_dir("lone");
    let lone = dir.join("lone.bd");
    std::fs::write(&lone, "fn main() {\n    let x: i32 = true\n}\n").unwrap();
    let mut client = Client::new(&[]);
    let (uri, _) = client.open(&lone);
    let diagnostics = client.diagnostics(&uri, |p| !p.get("diagnostics").as_array().is_empty());
    assert!(codes(&diagnostics).contains(&"E0003".to_string()), "{diagnostics:?}");
    client.notify("textDocument/didClose", doc(&uri));
    let cleared = client.diagnostics(&uri, |_| true);
    assert!(cleared.is_empty());
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn a_manifest_that_does_not_load_is_reported_until_it_is_fixed() {
    let root = fixture("lints");
    let manifest = root.join("burdin.toml");
    let good = read(&manifest);
    std::fs::write(&manifest, "[package\nname = \"lints\"\n").unwrap();
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/main.bd"));
    let reported = client.diagnostics(&uri, |p| !p.get("diagnostics").as_array().is_empty());
    assert_eq!(reported.len(), 1);
    assert!(
        reported[0].get("message").as_str().unwrap().starts_with("cannot load the package: "),
        "{}",
        reported[0].to_text()
    );
    assert_eq!(line_char(reported[0].at(&["range", "start"])), (0, 0));

    std::fs::write(&manifest, good).unwrap();
    let change = Json::obj().with("uri", common::uri(&manifest)).with("type", 2);
    client.notify("workspace/didChangeWatchedFiles", Json::obj().with("changes", Json::Arr(vec![change])));
    let fixed = client.diagnostics(&uri, |p| {
        p.get("diagnostics").as_array().iter().all(|d| d.get("code").as_str().is_some_and(|c| c.starts_with('L')))
    });
    assert_eq!(codes(&fixed).len(), 2, "the lints of the loaded package: {fixed:?}");
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn labels_and_spanned_notes_map_to_lsp_related_information() {
    let dir = fresh_dir("related");
    let src = dir.join("related.bd");
    std::fs::write(
        &src,
        "struct Item { id: i32 }\nfn consume(own x: Item) {}\nfn main() {\n    let a = Item { id: 1 }\n    consume(a)\n    let b = a\n}\n",
    )
    .unwrap();
    let mut client = Client::new(&[]);
    let (uri, text) = client.open(&src);
    let diagnostics = client.diagnostics(&uri, |p| !p.get("diagnostics").as_array().is_empty());
    let moved = diagnostics.iter().find(|d| d.get("code").as_str() == Some("E0100")).expect("an E0100 error");
    let related = moved.get("relatedInformation").as_array();
    assert!(!related.is_empty(), "expected relatedInformation on E0100: {}", moved.to_text());
    let first = &related[0];
    assert_eq!(first.at(&["location", "uri"]).as_str(), Some(uri.as_str()));
    assert!(!first.get("message").as_str().unwrap_or("").is_empty());
    let _ = slice(&text, first.at(&["location", "range"]), client.encoding);
    // Direct unit check on `to_json` for explicit `Diagnostic::label` (#1046).
    let mut manual = burdin_diag::Diagnostic::new(
        burdin_diag::Code::TYPE_MISMATCH,
        burdin_diag::Severity::Error,
        burdin_diag::Span::new(burdin_diag::FileId(0), 10, 11),
        "mismatch",
    );
    manual.label(burdin_diag::Span::new(burdin_diag::FileId(0), 0, 6), "expected from here");
    assert_eq!(manual.labels.len(), 1);
    assert_eq!(client.shutdown(), 0);
}
