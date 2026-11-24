//! Shared by the query tests: the `shapes` fixture package, and helpers that read what an analysis did.
#![allow(dead_code)]

use burdin_query::package::{ModuleFile, PackageFiles, Workspace};
use burdin_query::{Analysis, Db, Options};
use std::collections::HashMap;
use std::path::PathBuf;

pub const MAIN: &str = include_str!("../fixtures/shapes/main.bd");
pub const MATH: &str = include_str!("../fixtures/shapes/math.bd");
pub const SHAPES: &str = include_str!("../fixtures/shapes/geometry/shapes.bd");
pub const REPORT: &str = include_str!("../fixtures/shapes/report.bd");

pub fn files() -> Vec<(&'static str, &'static str)> {
    vec![("main.bd", MAIN), ("math.bd", MATH), ("geometry/shapes.bd", SHAPES), ("report.bd", REPORT)]
}

pub fn open() -> Db {
    Db::from_files(&files(), Options::default()).expect("the fixture package opens")
}

/// A package with no standard library, from `(path, text)` pairs whose entry point is `main.bd`. The embedded
/// standard library is a dependency package of every `Db`, and its memory checks cost more than the memo behavior
/// under test; a package of a few plain functions runs a thousand analyses in the time a hundred cost with it.
pub fn standalone(files: &[(&str, &str)]) -> Db {
    let modules = files
        .iter()
        .map(|(path, _)| ModuleFile {
            path: path.trim_end_matches(".bd").split('/').map(str::to_string).collect(),
            file: PathBuf::from(path),
            text: None,
        })
        .collect();
    let entry = files.iter().position(|(path, _)| *path == "main.bd").unwrap_or(0);
    let workspace = Workspace {
        packages: vec![PackageFiles {
            name: "app".to_string(),
            dir: None,
            modules,
            deps: Vec::new(),
            build_allow: Vec::new(),
            link_libs: Vec::new(),
            lib: Vec::new(),
            root_module: Some(entry),
            lint: Vec::new(),
            rust: Default::default(),
            edition: Default::default(),
        }],
        entry_package: 0,
        entry_module: entry,
        stem: "app".to_string(),
    };
    let texts: HashMap<PathBuf, String> =
        files.iter().map(|(path, text)| (PathBuf::from(path), (*text).to_string())).collect();
    Db::from_workspace(&workspace, &texts, Options::default()).expect("the package opens")
}

/// The diagnostics of an analysis, rendered as the command line prints them.
pub fn render(db: &Db, analysis: &Analysis) -> String {
    analysis.diagnostics.render(db.sources())
}

/// The bodies the last analysis checked from scratch that the fixture itself wrote (the standard library's are
/// filtered out), sorted.
pub fn checked(db: &Db) -> Vec<String> {
    let mut names: Vec<String> = db.stats().bodies_checked.iter().filter(|n| !n.starts_with("std.")).cloned().collect();
    names.sort();
    names
}

/// `text` with the first occurrence of `from` replaced by `to`. Panics when `from` is not there, so a test cannot
/// pass because its edit did nothing.
pub fn replace(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "`{from}` is not in the text");
    text.replacen(from, to, 1)
}
