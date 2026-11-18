//! The outline of files and the workspace (issues #367, #371; `spec/lsp.md` LS16-LS17): document and workspace
//! symbols, folding ranges, selection ranges, and document links.

mod common;

use burdin_lsp::json::Json;
use common::*;

#[test]
fn document_symbols_are_nested_or_flat_as_the_client_asks() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, _) = client.open(&root.join("src/shapes.bd"));
    let symbols = client.call("textDocument/documentSymbol", doc(&uri));
    golden("document-symbols", &normalize(&symbols, &root));
    assert_eq!(client.shutdown(), 0);

    // Without hierarchical support: a flat list with container names.
    let mut client = Client::with(&[&root], Json::obj(), Json::Null);
    let (uri, _) = client.open(&root.join("src/shapes.bd"));
    let flat = client.call("textDocument/documentSymbol", doc(&uri));
    let entries: Vec<String> = flat
        .as_array()
        .iter()
        .map(|s| format!("{}<{}", s.get("name").as_str().unwrap(), s.get("containerName").as_str().unwrap_or("")))
        .collect();
    assert!(entries.contains(&"Shape<".to_string()));
    assert!(entries.contains(&"area<Shape".to_string()));
    assert!(entries.contains(&"side<Square".to_string()));
    assert!(flat.as_array().iter().all(|s| !s.get("location").is_null()));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn workspace_symbols_match_fuzzily_across_files() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let found = |client: &mut Client, query: &str| -> Vec<String> {
        let result = client.call("workspace/symbol", Json::obj().with("query", query));
        result
            .as_array()
            .iter()
            .map(|s| format!("{}@{}", s.get("name").as_str().unwrap(), s.get("containerName").as_str().unwrap_or("")))
            .collect()
    };
    // No file is open: the folder's package is loaded for the query.
    let squares = found(&mut client, "squ");
    assert_eq!(squares[0], "Square@shapes", "an exact prefix ranks first: {squares:?}");
    assert!(squares.contains(&"square area@main".to_string()));
    // Letters in order, not necessarily together.
    assert!(found(&mut client, "fvrt").contains(&"favorite@shapes".to_string()));
    assert!(found(&mut client, "zzz").is_empty());
    // The standard library is left out.
    assert!(!found(&mut client, "println").iter().any(|s| s.starts_with("println@")));
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn folding_selection_and_links() {
    let root = fixture("geometry");
    let mut client = Client::new(&[&root]);
    let (uri, text) = client.open(&root.join("src/main.bd"));
    let folds = client.call("textDocument/foldingRange", doc(&uri));
    let spans: Vec<(i64, i64, String)> = folds
        .as_array()
        .iter()
        .map(|f| {
            (
                f.get("startLine").as_i64().unwrap() + 1,
                f.get("endLine").as_i64().unwrap() + 1,
                f.get("kind").as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert_eq!(
        spans,
        [
            (1, 2, "imports".to_string()),
            (5, 6, String::new()),
            (9, 15, String::new()),
            (18, 19, String::new()),
            (22, 24, String::new())
        ]
    );

    // Selection grows from the name to the expression, the statement, the block, the function, and the file.
    let positions = Json::Arr(vec![client.pos(&text, "sq.area", 0, 4)]);
    let result = client.call("textDocument/selectionRange", doc(&uri).with("positions", positions));
    let mut steps = Vec::new();
    let mut at = &result.as_array()[0];
    while !at.is_null() {
        steps.push(slice(&text, at.get("range"), client.encoding));
        at = at.get("parent");
    }
    assert_eq!(steps[0], "area");
    assert_eq!(steps[1], "sq.area");
    assert_eq!(steps[2], "sq.area()");
    assert!(steps.iter().any(|s| s == "let mut sum = total(sq.area(), r.area())"));
    assert!(steps.iter().any(|s| s.starts_with("fn main()")));
    assert_eq!(steps.last().unwrap(), &text);
    for pair in steps.windows(2) {
        assert!(pair[1].len() > pair[0].len() && pair[1].contains(pair[0].as_str()), "{pair:?}");
    }

    // Links: each module of a `use`, a URL in a comment, and a `[name]` in a doc comment.
    let edited = format!("/// See [total] and https://example.com/docs.\n{text}");
    client.change(&uri, 2, &edited);
    let links = client.call("textDocument/documentLink", doc(&uri));
    let found: Vec<(String, String)> = links
        .as_array()
        .iter()
        .map(|l| {
            let target = normalize(l.get("target"), &root).as_str().unwrap().to_string();
            (slice(&edited, l.get("range"), client.encoding), target)
        })
        .collect();
    assert_eq!(
        found,
        [
            ("total".to_string(), "ROOT/src/main.bd#L6".to_string()),
            ("https://example.com/docs".to_string(), "https://example.com/docs".to_string()),
            ("shapes".to_string(), "ROOT/src/shapes.bd".to_string()),
            ("math".to_string(), "ROOT/src/math.bd".to_string()),
        ]
    );
    assert_eq!(client.shutdown(), 0);
}
