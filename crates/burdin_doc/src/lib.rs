//! `burdin doc`: documentation generated from `///` comments (`spec/doc.md`).
//!
//! The generator reads the syntax trees and the source text of the packages the compiler loaded and builds an [`Api`]:
//! the modules, the items, their doc comments and declarations, the members that Burdin spreads over impl blocks,
//! interfaces, operators, extension methods, and derives, the `pub use` re-exports, and the intra-doc links. Three
//! renderers read it: [`html`] (a static site with search), [`json`] (a versioned schema for other tools), and
//! [`coverage`] (how much of the public interface is documented).
//!
//! It depends only on `burdin_diag` and `burdin_syntax`; the driver supplies the loaded packages and prints the
//! diagnostics.

pub mod html;
pub mod json;
pub mod markdown;
pub mod model;

mod extract;
mod resolve;
mod signature;

pub use model::*;

use burdin_diag::{Code, Diagnostics, FileId, SourceMap};
use burdin_syntax::ast;

/// A link in a doc comment names nothing.
pub const BROKEN_LINK: Code = Code("D0001");
/// A link in a doc comment names a `priv` item, which the site does not document.
pub const HIDDEN_LINK: Code = Code("D0002");

/// One module of a package.
pub struct ModuleInput<'a> {
    pub file: FileId,
    /// The path inside the package; empty for a one-file target.
    pub path: &'a [String],
    pub ast: &'a ast::File,
    /// The file's path relative to the package, with `/` separators.
    pub file_path: String,
    /// Whether other packages reach this module as the package itself.
    pub is_root: bool,
}

/// One package the compiler loaded.
pub struct PackageInput<'a> {
    pub name: &'a str,
    /// The dependencies by the name `use` writes, and the package index of each.
    pub deps: &'a [(String, usize)],
    pub modules: Vec<ModuleInput<'a>>,
    /// The embedded standard library: its names resolve, and nothing in it is documented.
    pub is_std: bool,
}

pub struct Input<'a> {
    pub sources: &'a SourceMap,
    pub packages: Vec<PackageInput<'a>>,
    /// The package to document.
    pub entry: usize,
    /// Whether `priv` items and members are documented too.
    pub private: bool,
}

/// Builds the documented API of the entry package. The diagnostics are the warnings about its doc comments (broken
/// links); the API is complete whether or not there are any.
pub fn build(input: &Input) -> (Api, Diagnostics) {
    extract::build(input)
}

/// One row of the coverage report: a module's public interface and how much of it is documented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageRow {
    pub module: String,
    pub items: usize,
    pub documented: usize,
    /// Items whose documentation has at least one Burdin code block.
    pub with_examples: usize,
}

/// A public item that has no documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub file_path: String,
    pub line: u32,
    pub kind: &'static str,
    pub name: String,
}

/// Counts, per module, the public items and how many are documented (`spec/doc.md`, DC20). The items are the ones the
/// `missing_docs` lint reports, so the two always agree.
pub fn coverage(input: &Input) -> (Vec<CoverageRow>, Vec<Missing>) {
    let entry = &input.packages[input.entry];
    let mut modules: Vec<&ModuleInput> = entry.modules.iter().collect();
    modules.sort_by(|a, b| a.path.cmp(b.path));
    let mut rows = Vec::new();
    let mut missing = Vec::new();
    for m in modules {
        let file = input.sources.get(m.file);
        let items = burdin_syntax::docs::public_items(m.ast, m.file, &file.text);
        let mut row = CoverageRow {
            module: if m.path.is_empty() || m.is_root { entry.name.to_string() } else { m.path.join(".") },
            items: items.len(),
            documented: 0,
            with_examples: 0,
        };
        for item in items {
            match &item.docs {
                Some(doc) => {
                    row.documented += 1;
                    if markdown::example_count(&markdown::parse(&doc.text)) > 0 {
                        row.with_examples += 1;
                    }
                }
                None => missing.push(Missing {
                    file_path: m.file_path.clone(),
                    line: file.line_col(item.span.lo).0,
                    kind: item.kind,
                    name: item.name,
                }),
            }
        }
        rows.push(row);
    }
    (rows, missing)
}

/// `3/4 (75%)`, or `0/0` when there is nothing to count.
fn cell(part: usize, whole: usize) -> String {
    match (part * 100).checked_div(whole) {
        Some(percent) => format!("{part}/{whole} ({percent}%)"),
        None => "0/0".to_string(),
    }
}

/// The coverage report as text.
pub fn render_coverage(package: &str, rows: &[CoverageRow], missing: &[Missing]) -> String {
    use std::fmt::Write;
    let mut table: Vec<[String; 3]> =
        rows.iter().map(|r| [r.module.clone(), cell(r.documented, r.items), cell(r.with_examples, r.items)]).collect();
    let items: usize = rows.iter().map(|r| r.items).sum();
    let documented: usize = rows.iter().map(|r| r.documented).sum();
    let examples: usize = rows.iter().map(|r| r.with_examples).sum();
    table.push(["total".to_string(), cell(documented, items), cell(examples, items)]);
    let header = ["module".to_string(), "documented".to_string(), "with examples".to_string()];
    let widths: Vec<usize> =
        (0..3).map(|c| table.iter().chain([&header]).map(|r| r[c].len()).max().unwrap_or(0)).collect();
    let mut out = String::new();
    let _ = writeln!(out, "documentation coverage of {package}");
    for row in std::iter::once(&header).chain(&table) {
        let _ = writeln!(out, "{:<w0$}  {:<w1$}  {}", row[0], row[1], row[2], w0 = widths[0], w1 = widths[1]);
    }
    if !missing.is_empty() {
        out.push_str(
            "
undocumented public items:
",
        );
        for m in missing {
            let _ = writeln!(out, "  {}:{}  {} {}", m.file_path, m.line, m.kind, m.name);
        }
    }
    out
}

#[cfg(test)]
mod tests;
