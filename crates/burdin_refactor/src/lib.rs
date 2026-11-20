//! The refactoring engine (`spec/refactor.md`): structural search and replace (RF1-RF6) and the refactorings
//! (RF7-RF14), over the query library. Every operation reads one analysis of the package, computes new file texts,
//! and checks them before it hands them back: the result must type-check with no error the package did not have,
//! and every local of every function must be moved exactly as often as before (RF2, RF8). When either check fails,
//! or the operation cannot be done without changing what the program means, it refuses with an explanation and
//! changes nothing.
//!
//! The driver's `burdin ssr` and `burdin refactor` and the language server's code actions call the same functions,
//! so a refactoring behaves the same from the command line and from an editor.

pub mod extract;
pub mod inline;
pub mod move_item;
pub mod signature;
pub mod ssr;
pub mod tree;

use burdin_diag::Severity;
use burdin_query::{Analysis, Db, OwnershipEventKind};

use burdin_syntax::ast;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// The new text of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// The path of the file, as [`Db::file_paths`] lists it.
    pub path: String,
    pub before: String,
    pub after: String,
}

/// Why an operation was not done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// What an operation returns: the changed files, in path order, or why it refused.
pub type Outcome = Result<Vec<FileChange>, Refusal>;

pub(crate) fn refuse<T>(message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(message.into()))
}

/// One package being refactored: the database and the analysis of its current texts.
pub struct Engine<'a> {
    pub db: &'a mut Db,
    pub analysis: Analysis,
}

impl<'a> Engine<'a> {
    /// Analyzes the package. A refactoring needs the types and the ownership facts of a package that checks, so a
    /// package with errors is refused (RF8).
    pub fn new(db: &'a mut Db) -> Result<Engine<'a>, Refusal> {
        let analysis = db.analyze().map_err(|_| Refusal("the analysis was cancelled".to_string()))?;
        if analysis.program.is_none() || analysis.diagnostics.has_errors() {
            let first = first_error(&analysis, db);
            return refuse(format!("the package has errors; a refactoring needs a package that checks{first}"));
        }
        Ok(Engine { db, analysis })
    }

    /// The current text of a file.
    pub fn text(&self, path: &str) -> String {
        self.db.file_text(path).unwrap_or("").to_string()
    }

    /// The syntax tree of a file.
    pub fn ast(&mut self, path: &str) -> Result<std::rc::Rc<burdin_query::Parsed>, Refusal> {
        self.db.syntax(path).ok_or_else(|| Refusal(format!("`{path}` is not a file of this package")))
    }

    /// The files of the entry package: the only ones an operation edits.
    pub fn entry_files(&self) -> Vec<String> {
        let packages = self.db.packages();
        self.db
            .modules()
            .into_iter()
            .filter(|m| packages.get(m.package).is_some_and(|p| p.is_entry && !p.embedded))
            .map(|m| m.path)
            .collect()
    }

    /// The module path of a file of the entry package (`geometry.shapes`), empty for the entry file.
    pub fn module_of(&self, path: &str) -> Option<Vec<String>> {
        self.db.modules().into_iter().find(|m| m.path == path).map(|m| m.module)
    }

    /// `path:line:col` of an offset, for messages.
    pub fn place(&self, path: &str, offset: u32) -> String {
        match self.db.line_col(path, offset) {
            Some((line, col)) => format!("{path}:{line}:{col}"),
            None => path.to_string(),
        }
    }

    /// A 1-based line and column (in characters) as a byte offset of a file.
    pub fn offset(&self, path: &str, line: u32, col: u32) -> Result<u32, Refusal> {
        let text = self.text(path);
        let mut at = 0usize;
        for (index, raw) in text.split_inclusive('\n').enumerate() {
            if index + 1 == line as usize {
                let mut chars = raw.char_indices();
                let mut column = 1u32;
                loop {
                    match chars.next() {
                        Some((i, _)) if column == col => return Ok((at + i) as u32),
                        Some(_) => column += 1,
                        None if column == col => return Ok((at + raw.len()) as u32),
                        None => break,
                    }
                }
                return refuse(format!("{path} has no column {col} on line {line}"));
            }
            at += raw.len();
        }
        if line as usize == text.split_inclusive('\n').count() + 1 && col == 1 {
            return Ok(text.len() as u32);
        }
        refuse(format!("{path} has no line {line}"))
    }

    /// Checks the new texts of some files (RF8) and returns them as changes. `renamed` maps the locals the operation
    /// renames, old name to new, so their moves are compared under the new name; locals the operation introduces
    /// or removes are not compared.
    pub fn verify(&mut self, edited: BTreeMap<String, String>, renamed: &HashMap<String, String>) -> Outcome {
        let before_moves = moves_by_function(&self.analysis, &self.entry_files());
        let before_errors = self.analysis.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
        let mut originals: Vec<(String, String)> = Vec::new();
        for (path, text) in &edited {
            originals.push((path.clone(), self.text(path)));
            self.db.set_file(path, text);
        }
        let after = self.db.analyze();
        let result = match &after {
            Err(_) => refuse("the analysis was cancelled"),
            Ok(after) => {
                let errors = after.diagnostics.iter().filter(|d| d.severity == Severity::Error).count();
                if after.program.is_none() || errors > before_errors {
                    let first = first_error(after, self.db);
                    refuse(format!("the result would not compile{first}"))
                } else {
                    let after_moves = moves_by_function(after, &self.entry_files());
                    match compare_moves(&before_moves, &after_moves, renamed) {
                        Some(message) => refuse(format!("the result would change ownership: {message}")),
                        None => Ok(()),
                    }
                }
            }
        };
        for (path, text) in &originals {
            self.db.set_file(path, text);
        }
        result?;
        Ok(originals
            .into_iter()
            .zip(edited)
            .filter(|((_, before), (_, after))| before != after)
            .map(|((path, before), (_, after))| FileChange { path, before, after })
            .collect())
    }

    /// The function declaration (free function, method, extension, or test body) of `path` whose body holds
    /// `lo..hi`, innermost first.
    pub fn function_at<'t>(&self, file: &'t ast::File, lo: u32, hi: u32) -> Option<&'t ast::FnDecl> {
        let mut best: Option<&ast::FnDecl> = None;
        burdin_syntax::walk::for_each_fn(file, &|_| false, &mut |_, decl| {
            let s = decl.body.span;
            if s.lo < s.hi
                && s.lo <= lo
                && hi <= s.hi
                && best.is_none_or(|b| s.hi - s.lo < b.body.span.hi - b.body.span.lo)
            {
                best = Some(decl);
            }
        });
        best
    }
}

/// `: <message> (at <path:line:col>)` for the first error of an analysis, or nothing.
fn first_error(analysis: &Analysis, db: &Db) -> String {
    let Some(d) = analysis.diagnostics.iter().find(|d| d.severity == Severity::Error) else { return String::new() };
    let path = analysis.file_paths().get(d.span.file.0 as usize).cloned().unwrap_or_default();
    let place = match db.line_col(&path, d.span.lo) {
        Some((line, col)) => format!("{path}:{line}:{col}"),
        None => path,
    };
    format!(": error[{}] {} (at {place})", d.code.0, d.message)
}

/// How often each function of the given files moves each of its locals: function name, then local and field path.
type Moves = BTreeMap<String, BTreeMap<String, usize>>;

fn moves_by_function(analysis: &Analysis, files: &[String]) -> Moves {
    let mut out = Moves::new();
    for path in files {
        for function in analysis.ownership_events(path) {
            let entry = out.entry(function.function.clone()).or_default();
            for event in function.events {
                if let (OwnershipEventKind::Move { .. }, Some(local)) = (&event.kind, &event.local) {
                    let key = if event.path.is_empty() { local.clone() } else { format!("{local}.{:?}", event.path) };
                    *entry.entry(key).or_default() += 1;
                }
            }
        }
    }
    out
}

/// The first difference between the moves of the functions both versions have, for the locals both have.
fn compare_moves(before: &Moves, after: &Moves, renamed: &HashMap<String, String>) -> Option<String> {
    for (function, old) in before {
        let Some(new) = after.get(function) else { continue };
        let rename = |key: &str| -> String {
            let (local, rest) = key.split_once('.').map_or((key, ""), |(l, r)| (l, r));
            let local = renamed.get(local).map_or(local, String::as_str);
            if rest.is_empty() { local.to_string() } else { format!("{local}.{rest}") }
        };
        let old: BTreeMap<String, usize> = old.iter().map(|(k, v)| (rename(k), *v)).collect();
        for (key, count) in &old {
            if let Some(now) = new.get(key)
                && now != count
            {
                let local = key.split('.').next().unwrap_or(key);
                return Some(format!("`{function}` would move `{local}` {} instead of {}", times(*now), times(*count)));
            }
        }
        for (key, now) in new {
            let local = key.split('.').next().unwrap_or(key);
            let existed = old.keys().any(|k| k.split('.').next() == Some(local));
            if !old.contains_key(key) && existed {
                return Some(format!("`{function}` would move `{local}` {} instead of never", times(*now)));
            }
        }
    }
    None
}

fn times(n: usize) -> String {
    match n {
        1 => "once".to_string(),
        2 => "twice".to_string(),
        n => format!("{n} times"),
    }
}

/// Applies edits `(lo, hi, replacement)` to one text. The edits must not overlap.
pub(crate) fn apply(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse((e.0, e.1)));
    let mut out = text.to_string();
    for (lo, hi, replacement) in edits {
        out.replace_range(lo..hi, &replacement);
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A database over one-package files in memory, `main.bd` the entry file.
    pub fn db(files: &[(&str, &str)]) -> Db {
        Db::from_files(files, burdin_query::Options::default()).expect("files")
    }

    /// The text `path` has after an outcome, or the refusal's message.
    pub fn after(outcome: Outcome, path: &str, original: &str) -> Result<String, String> {
        match outcome {
            Ok(changes) => {
                Ok(changes.into_iter().find(|c| c.path == path).map(|c| c.after).unwrap_or(original.to_string()))
            }
            Err(r) => Err(r.0),
        }
    }

    /// The byte span of the `n`th (0-based) occurrence of `needle` in `text`.
    pub fn span_of(text: &str, needle: &str, n: usize) -> (u32, u32) {
        let lo = text.match_indices(needle).nth(n).expect("needle").0;
        (lo as u32, (lo + needle.len()) as u32)
    }
}
