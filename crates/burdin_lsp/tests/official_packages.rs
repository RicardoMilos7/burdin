//! The official packages open cleanly in the language server (issue #1087; `spec/lsp.md`, LS4 and LS7). Each one is
//! opened where it lives, in `packages/`, as an editor opens it: every module of the package is a document, and the
//! server must publish them without a compiler error and without hanging. `csv` once reported a false E0003 on every
//! `Result.Ok(...)`/`Result.Err(...)` (the installed server predated qualified variants, issue #1069), and `http`,
//! which loads `tls` as an implicit official dependency, overflowed the server's stack.

mod common;

use burdin_lsp::json::Json;
use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long one package may take to load, analyze, and publish. The client gives up waiting for a publication after a
/// minute, so a server that hangs fails there; this bound is the same, stated for the test.
const BOUND: Duration = Duration::from_secs(60);

fn packages_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages").canonicalize().expect("the official packages")
}

/// The `.bd` files under a directory, sorted.
fn modules(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(modules(&path));
        } else if path.extension().is_some_and(|e| e == "bd") {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// The diagnostics the server publishes for each module of official package `name`, by file name, and how long it took.
fn open_package(name: &str) -> (Vec<(String, Vec<Json>)>, Duration) {
    let root = packages_dir().join(name);
    let started = Instant::now();
    let mut client = Client::new(&[&root]);
    let files = modules(&root.join("src"));
    assert!(!files.is_empty(), "{name} has no modules");
    let uris: Vec<(String, String)> = files
        .iter()
        .map(|file| {
            let (uri, _) = client.open(file);
            (file.file_name().unwrap().to_string_lossy().into_owned(), uri)
        })
        .collect();
    let published = uris.into_iter().map(|(file, uri)| (file, client.diagnostics(&uri, |_| true))).collect();
    let elapsed = started.elapsed();
    assert_eq!(client.shutdown(), 0);
    (published, elapsed)
}

/// The compiler's diagnostics (not the linter's) among a publication, as `code line:col message`.
fn errors(diagnostics: &[Json]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.get("code").as_str().is_some_and(|c| c.starts_with('E')))
        .map(|d| {
            let (line, col) = line_char(d.at(&["range", "start"]));
            format!(
                "{} {}:{} {}",
                d.get("code").as_str().unwrap(),
                line + 1,
                col + 1,
                d.get("message").as_str().unwrap_or("")
            )
        })
        .collect()
}

#[test]
fn csv_has_no_false_e0003_on_result_variants() {
    let (published, elapsed) = open_package("csv");
    assert!(elapsed < BOUND, "csv took {elapsed:?}");
    let (_, diagnostics) = published.iter().find(|(file, _)| file == "csv.bd").expect("csv.bd is published");
    let found = errors(diagnostics);
    assert!(found.is_empty(), "csv.bd: {found:#?}");
    // `Writer.with_options` returns `Result<Writer, DeError>` through `Result.Err(...)` and `Result.Ok(writer)`.
    let text = read(&packages_dir().join("csv/src/csv.bd"));
    let start = text.find("fn with_options(").expect("with_options");
    let end = start + text[start..].find("\n    }\n").expect("the end of with_options");
    // Zero-based lines, as the protocol counts them: the signature through the closing brace.
    let first = text[..start].matches('\n').count() as u32;
    let last = text[..end].matches('\n').count() as u32 + 1;
    assert!(
        !diagnostics.iter().any(|d| {
            let line = line_char(d.at(&["range", "start"])).0;
            d.get("code").as_str() == Some("E0003") && (first..=last).contains(&line)
        }),
        "E0003 in with_options"
    );
}

#[test]
fn http_and_its_official_tls_dependency_are_analyzed_promptly() {
    let (published, elapsed) = open_package("http");
    assert!(elapsed < BOUND, "http took {elapsed:?}");
    for (file, diagnostics) in &published {
        let found = errors(diagnostics);
        assert!(found.is_empty(), "{file}: {found:#?}");
    }
}

#[test]
fn every_official_package_opens_without_a_compiler_error() {
    let mut names: Vec<String> = std::fs::read_dir(packages_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("burdin.toml").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(names.iter().any(|n| n == "csv") && names.iter().any(|n| n == "http"), "{names:?}");
    let mut failures = Vec::new();
    for name in names.iter().filter(|n| *n != "csv" && *n != "http") {
        let (published, elapsed) = open_package(name);
        if elapsed >= BOUND {
            failures.push(format!("{name} took {elapsed:?}"));
        }
        for (file, diagnostics) in published {
            for error in errors(&diagnostics) {
                failures.push(format!("{name}/{file}: {error}"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
