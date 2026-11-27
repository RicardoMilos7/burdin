//! Diagnostics (`spec/lsp.md`, LS7): the compiler's, and the linter's on a program that compiles, converted to LSP.

use crate::json::Json;
use crate::project::{Loc, Package};
use crate::server::Server;
use crate::text::LineIndex;
use burdin_diag::{Diagnostic, Severity};
use std::collections::HashMap;
use std::rc::Rc;

/// The lint codes whose findings are code that can go (an editor fades it).
const UNNECESSARY: &[&str] = &["L0006", "L0007", "L0008", "L0009", "L0010"];

/// The diagnostics of the files a package owns, by path: the compiler's, then the linter's when `lints` is set and
/// the program has no errors.
pub fn collect(package: &mut Package, lints: bool) -> HashMap<String, Vec<Diagnostic>> {
    let mut out: HashMap<String, Vec<Diagnostic>> = HashMap::new();
    let files = package.own_files();
    let Some(analysis) = package.analysis.as_ref() else { return out };
    for path in &files {
        let diags: Vec<Diagnostic> = analysis.diagnostics_of(path).into_iter().cloned().collect();
        if !diags.is_empty() {
            out.insert(path.clone(), diags);
        }
    }
    if lints {
        let paths: Vec<String> = package.db.file_paths().iter().map(|p| p.to_string()).collect();
        for d in lint(package) {
            if let Some(path) = paths.get(d.span.file.0 as usize) {
                out.entry(path.clone()).or_default().push(d);
            }
        }
    }
    out
}

/// The linter's findings for the package, computed once per analysis.
pub fn lint(package: &mut Package) -> Vec<Diagnostic> {
    let Some(revision) = package.analysis.as_ref().map(|a| a.revision) else { return Vec::new() };
    if let Some((r, diags)) = &package.lint_cache
        && *r == revision
    {
        return diags.clone();
    }
    let files = package.own_files();
    let parsed: Vec<(String, Rc<burdin_query::Parsed>)> =
        files.iter().filter_map(|p| package.parsed(p).map(|t| (p.clone(), t))).collect();
    let mut config = burdin_lint::Config::default();
    for (name, level, _) in &package.lint {
        // A manifest error is the command line's to report; the editor uses the levels it can read.
        let _ = config.set_from_manifest(name, level);
    }
    let analysis = package.analysis.as_ref().expect("checked above");
    let diags = match analysis.program.as_ref().filter(|_| !analysis.diagnostics.has_errors()) {
        Some(program) => {
            let modules: Vec<Vec<String>> =
                parsed.iter().map(|(p, _)| package.module(p).map(|m| m.module.clone()).unwrap_or_default()).collect();
            let sources: Vec<burdin_lint::Source> = parsed
                .iter()
                .zip(&modules)
                .filter_map(|((p, tree), module)| {
                    Some(burdin_lint::Source { file: package.db.file_id(p)?, module, ast: &tree.ast })
                })
                .collect();
            let input = burdin_lint::Input { sources: package.db.sources(), files: sources, program, config: &config };
            burdin_lint::run(&input).into_vec()
        }
        None => Vec::new(),
    };
    package.lint_cache = Some((revision, diags.clone()));
    diags
}

/// An LSP `Diagnostic`.
/// `paths` are the file paths of the producing database, which the diagnostic's file ids index.
pub fn to_json(server: &Server, d: &Diagnostic, index: &LineIndex, paths: &[String]) -> Json {
    let mut message = d.message.clone();
    for help in &d.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    let mut out = Json::obj()
        .with("range", index.range(d.span.lo, d.span.hi))
        .with("severity", if d.severity == Severity::Error { 1 } else { 2 })
        .with("code", d.code.0)
        .with("source", if d.code.0.starts_with('L') { "burdin lint" } else { "burdin" })
        .with("message", message);
    if let Some(url) = burdin_diag::code_url(d.code.0) {
        out.set("codeDescription", Json::obj().with("href", url));
    }
    if UNNECESSARY.contains(&d.code.0) {
        out.set("tags", Json::Arr(vec![Json::from(1)]));
    }
    let mut related = Vec::new();
    for label in &d.labels {
        if let Some(p) = paths.get(label.span.file.0 as usize) {
            let loc = Loc::new(p, label.span.lo, label.span.hi);
            related.push(Json::obj().with("location", server.location(&loc)).with("message", label.message.as_str()));
        }
    }
    for note in &d.notes {
        match note.span.and_then(|s| paths.get(s.file.0 as usize).map(|p| Loc::new(p, s.lo, s.hi))) {
            Some(loc) => {
                related.push(Json::obj().with("location", server.location(&loc)).with("message", note.message.as_str()))
            }
            None => {
                let loc = Loc::new(paths.get(d.span.file.0 as usize).map_or("", |p| p.as_str()), d.span.lo, d.span.hi);
                related
                    .push(Json::obj().with("location", server.location(&loc)).with("message", note.message.as_str()));
            }
        }
    }
    for child in &d.children {
        if let Some(p) = paths.get(child.span.file.0 as usize) {
            let loc = Loc::new(p, child.span.lo, child.span.hi);
            related.push(
                Json::obj()
                    .with("location", server.location(&loc))
                    .with("message", format!("{}[{}]: {}", child.severity.as_str(), child.code.0, child.message)),
            );
        }
    }
    if !related.is_empty() {
        out.set("relatedInformation", Json::Arr(related));
    }
    out
}
