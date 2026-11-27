//! Parity (`spec/query.md`, QL4): for every conformance test that must fail to compile, the error diagnostics
//! the query library reports are the ones the command line reports — the same file, line, and code, in the same
//! number.
//!
//! The command line is a separate process (`burdin build` or `burdin test` on the test's target), and the library
//! is `burdin_query::Db::open` on the same target, so this checks the whole path a tool takes, not a shortcut.
//!
//! Skipped: packages whose build script fails, because the build phase is the driver's and runs before the
//! library sees any source (`tests/check_build_scripts.py` covers those), packages that bind Rust crates (the same
//! phase; `tests/check_rust_interop.py` and the conformance runner cover those), and tests marked `release-only`, which
//! `cargo test` runs in debug.

use burdin_query::{Db, Options};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One conformance test that must fail to compile.
struct Case {
    /// The package directory or the single file, as the command line is given it.
    target: PathBuf,
    /// `burdin test` (the `test` items are checked) rather than `burdin build`.
    /// `test-compile-fail` checks the tests, `bench-compile-fail` the benches; a plain `compile-fail` builds.
    mode: burdin_query::EntryMode,
}

/// An error diagnostic, as both sides report it: the file, the 1-based line, and the code.
type DiagnosticKey = (String, u32, String);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root exists")
}

/// Every `.bd` file under `tests/`, at any depth.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "bd") {
            out.push(path);
        }
    }
}

/// The package directory of a test (the nearest ancestor with a `burdin.toml` inside `tests/`), or the file.
fn target_of(root: &Path, tests: &Path) -> PathBuf {
    let mut directory = root.parent();
    while let Some(current) = directory {
        if current == tests {
            break;
        }
        if current.join("burdin.toml").is_file() {
            return current.to_path_buf();
        }
        directory = current.parent();
    }
    root.to_path_buf()
}

fn cases(repo: &Path) -> Vec<Case> {
    let tests = repo.join("tests");
    let mut files = Vec::new();
    walk(&tests, &mut files);
    files.sort();
    let mut cases = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let directives: Vec<&str> = text.lines().map(str::trim_end).filter(|line| line.starts_with("//@ ")).collect();
        let mode = if directives.contains(&"//@ test-compile-fail") {
            burdin_query::EntryMode::Test
        } else if directives.contains(&"//@ bench-compile-fail") {
            burdin_query::EntryMode::Bench
        } else if directives.contains(&"//@ compile-fail") {
            burdin_query::EntryMode::Build
        } else {
            continue;
        };
        if directives.contains(&"//@ release-only") {
            continue;
        }
        let target = target_of(&file, &tests);
        // The build phase (build scripts and the grants that allow them) is the driver's, and it runs before the
        // library reads any source; `tests/check_build_scripts.py` covers it.
        if has_build_phase(&target, &mut std::collections::HashSet::new()) {
            continue;
        }
        let target = target.canonicalize().unwrap_or(target);
        cases.push(Case { target, mode });
    }
    cases
}

/// Whether a package, or any package it depends on by path, runs a build script or declares build grants: those
/// diagnostics come from the driver's build phase, which the library does not run.
fn has_build_phase(directory: &Path, seen: &mut std::collections::HashSet<PathBuf>) -> bool {
    if directory.is_file() || !seen.insert(directory.to_path_buf()) {
        return false;
    }
    if directory.join("build.bd").is_file() {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(directory.join("burdin.toml")) else { return false };
    // `[rust.crates]` binds Rust crates in the same phase (`spec/rust-interop.md`, RI7): its errors are E0264 and E0260.
    if text.lines().any(|line| matches!(line.trim(), "[build]" | "[rust]" | "[rust.crates]")) {
        return true;
    }
    // A dependency line with a path, as the manifests in `tests/` write it: `lib = { path = "../lib" }`.
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("path = \"") else { continue };
        let Some((value, _)) = rest.split_once('"') else { continue };
        let dependency = directory.join(value);
        if has_build_phase(&dependency, seen) {
            return true;
        }
    }
    false
}

/// The error diagnostics in the command line's output, from `error[CODE]: ...` + `--> path:line:col` blocks
/// (or legacy `path:line:col: error[CODE]` lines).
fn cli_errors(output: &str) -> BTreeSet<DiagnosticKey> {
    let mut errors = BTreeSet::new();
    let mut pending_code: Option<String> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("error[")
            && let Some((code, _)) = rest.split_once("]:")
            && code.len() == 5
            && code.starts_with('E')
        {
            pending_code = Some(code.to_string());
            continue;
        }
        if let Some(code) = pending_code.as_ref()
            && let Some(place) = line.trim_start().strip_prefix("--> ")
        {
            let mut parts = place.rsplitn(3, ':');
            if let (Some(_col), Some(line_no), Some(path)) = (parts.next(), parts.next(), parts.next())
                && let Ok(line_no) = line_no.parse::<u32>()
            {
                errors.insert((path.to_string(), line_no, code.clone()));
            }
            pending_code = None;
            continue;
        }
        if line.starts_with("error:") || line.starts_with("warning") {
            pending_code = None;
        }
        let Some(at) = line.find(": error[") else { continue };
        let (place, code) = line.split_at(at);
        let code = code.trim_start_matches(": error[");
        let Some(code) = code.split(']').next() else { continue };
        if code.len() != 5 || !code.starts_with('E') {
            continue;
        }
        // The place is `path:line:col`; a path may itself contain colons (a Windows drive).
        let mut parts = place.rsplitn(3, ':');
        let (Some(_col), Some(line_no), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Ok(line_no) = line_no.parse::<u32>() else { continue };
        errors.insert((path.to_string(), line_no, code.to_string()));
    }
    errors
}

/// The error diagnostics the library reports.
fn library_errors(db: &Db, analysis: &burdin_query::Analysis) -> BTreeSet<DiagnosticKey> {
    let mut errors = BTreeSet::new();
    for diagnostic in analysis.diagnostics.iter().filter(|d| d.code.0.starts_with('E')) {
        let Some(path) = analysis.file_paths().get(diagnostic.span.file.0 as usize) else { continue };
        let Some((line, _)) = db.line_col(path, diagnostic.span.lo) else { continue };
        errors.insert((path.to_string(), line, diagnostic.code.0.to_string()));
    }
    errors
}

#[test]
fn the_library_reports_the_command_lines_errors() {
    let repo = repo_root();
    let binary = env!("CARGO_BIN_EXE_burdin");
    let cases = cases(&repo);
    assert!(cases.len() > 100, "the parity test found only {} failing tests", cases.len());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8).min(cases.len());
    let chunk = cases.len().div_ceil(workers);
    let mut failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = cases
            .chunks(chunk)
            .map(|part| {
                // The parser and the checker recurse over nested expressions; the main thread's stack is the size
                // the compiler is built for, so the workers get the same.
                std::thread::Builder::new()
                    .stack_size(64 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        let mut failures = Vec::new();
                        for case in part {
                            // The command the driver would run; a bench or test build that fails to compile runs
                            // nothing, so the binary side is only compared on its diagnostics.
                            let command = match case.mode {
                                burdin_query::EntryMode::Build => "build",
                                burdin_query::EntryMode::Test => "test",
                                burdin_query::EntryMode::Bench => "bench",
                                // No conformance test is a fuzz build; the arm is for completeness.
                                burdin_query::EntryMode::Fuzz => "fuzz",
                            };
                            let output = Command::new(binary)
                                .arg(command)
                                .arg(&case.target)
                                .current_dir(std::env::temp_dir())
                                .output()
                                .expect("the compiler runs");
                            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                            text.push_str(&String::from_utf8_lossy(&output.stderr));
                            let cli = cli_errors(&text);
                            let mut db = Db::open(&case.target, Options { mode: case.mode, ..Options::default() })
                                .unwrap_or_else(|error| {
                                    panic!("the library cannot open {}: {error}", case.target.display())
                                });
                            let analysis = db.analyze().expect("a batch analysis is not cancelled");
                            let library = library_errors(&db, &analysis);
                            if cli != library {
                                failures.push(format!(
                                    "{} ({}):
  command line: {cli:?}
  library:      {library:?}",
                                    case.target.display(),
                                    command
                                ));
                            }
                        }
                        failures
                    })
                    .expect("a parity worker starts")
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().expect("a parity worker panicked")).collect()
    });
    failures.sort();
    assert!(
        failures.is_empty(),
        "{} of {} failing tests disagree between the library and the command line:\n{}",
        failures.len(),
        cases.len(),
        failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
    );
}
