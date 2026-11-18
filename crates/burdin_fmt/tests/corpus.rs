//! The formatter against every Burdin source in the repository: the standard library, the conformance tests, and the
//! formatter's own fixtures.
//! Each file that parses must format without an internal error (which includes the check that the output is the same
//! program with the same comments) and must format to itself the second time (issue #462).

use burdin_fmt::{FormatError, Options, format_source};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("readable directory").map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "bd" || e == "input" || e == "golden") {
            out.push(path);
        }
    }
}

fn first_difference(a: &str, b: &str) -> String {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return format!("line {}:\n    first : {x}\n    second: {y}", i + 1);
        }
    }
    format!("the outputs have {} and {} lines", a.lines().count(), b.lines().count())
}

#[test]
fn every_source_in_the_repository_formats_idempotently() {
    let mut files = Vec::new();
    sources(&root().join("std"), &mut files);
    sources(&root().join("tests"), &mut files);
    let options = Options::default();
    let (mut formatted, mut skipped) = (0, 0);
    let mut problems: Vec<String> = Vec::new();
    for path in &files {
        let name = path.strip_prefix(root()).unwrap().display().to_string();
        let src = std::fs::read_to_string(path).expect("readable source");
        match format_source(&src, &options) {
            Err(FormatError::Syntax(_)) => skipped += 1,
            Err(FormatError::Internal(message)) => problems.push(format!("{name}: {message}")),
            Ok(once) => {
                formatted += 1;
                match format_source(&once, &options) {
                    Ok(twice) if twice == once => {}
                    Ok(twice) => problems.push(format!("{name}: not idempotent, {}", first_difference(&once, &twice))),
                    Err(e) => problems.push(format!("{name}: the output does not format again: {e}")),
                }
            }
        }
    }
    println!("{formatted} files formatted, {skipped} skipped for syntax errors, {} problems", problems.len());
    for p in problems.iter().take(40) {
        println!("{p}");
    }
    assert!(problems.is_empty(), "{} files failed", problems.len());
}
