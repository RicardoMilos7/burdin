//! The suggestions the checker attaches to its diagnostics (`spec/fix.md`, FX1, FX13): `mut` on the binding a write
//! needs, the one name in scope a misspelled name is close to, and the `use` that imports a name declared in another
//! module of the package. Each is machine-applicable only when it is the one fix that can be meant.

use super::FnCx;
use crate::globals::Global;
use burdin_diag::{Applicability, Diagnostic, Edit, Span};
use burdin_tir::place::place_root;
use burdin_tir::{self as tir, LocalKind, Ty};

/// The value names the compiler provides that are not declared anywhere: what a misspelled call may have meant.
const PROVIDED: [&str; 14] = [
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "Some",
    "None",
    "Ok",
    "Err",
    "swap",
    "replace",
    "format",
    "embed",
    "black_box",
];

/// A fix for a name that is not in scope.
pub(super) enum NameFix {
    /// Exactly one name in scope is spelled almost like it.
    Rename(String),
    /// Several are; a help line lists them.
    Candidates(Vec<String>),
    /// No name in scope is close, but exactly one other module of the package declares it.
    Import {
        path: String,
        edit: Edit,
    },
    None,
}

/// The edit distance between two names, counted in characters (insert, delete, substitute, and swap two neighbors).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}

/// How far a candidate may be from a name of `len` characters: nothing for one or two characters (any short name
/// is close to many others), one edit up to five, two from six.
fn limit(len: usize) -> usize {
    match len {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

/// Whether the text before a binding is `let` or `for`, so `mut ` can be written in front of the name.
fn simple_binding(text: &str, lo: usize) -> bool {
    let before = text.get(..lo).unwrap_or("").trim_end();
    let word = before.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("");
    (word == "let" || word == "for") && before.ends_with(word)
}

impl FnCx<'_> {
    /// Whether `e` would be a mutable place if its root binding were declared `mut`.
    fn mutable_with_root(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            tir::ExprKind::Local(_) => true,
            tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => self.mutable_with_root(base),
            tir::ExprKind::DerefRef(base) => matches!(base.ty, Ty::Ref { mutable: true, .. }),
            tir::ExprKind::PtrDeref(_) => true,
            tir::ExprKind::Index(base, _) => {
                matches!(base.ty, Ty::Array(..) | Ty::List(_, _)) && self.mutable_with_root(base)
            }
            _ => false,
        }
    }

    /// The `mut ` a `let` or `for` binding needs for the place `e` to be written: `(insertion point, name)`. `None`
    /// when the root is a parameter, a pattern inside another pattern, or a binding `mut` would not help.
    pub(super) fn mut_binding_fix(&self, e: &tir::Expr) -> Option<(Span, String)> {
        let root = place_root(e)?;
        let local = self.locals.get(root.0)?;
        if local.mutable || local.kind != LocalKind::Owned || local.span.lo >= local.span.hi {
            return None;
        }
        if !self.mutable_with_root(e) {
            return None;
        }
        let text = &self.sources.get(local.span.file).text;
        if !simple_binding(text, local.span.lo as usize) {
            return None;
        }
        Some((Span::new(local.span.file, local.span.lo, local.span.lo), local.name.clone()))
    }

    /// Attaches the `mut` fix for the place `e` to `d`, when there is one.
    pub(super) fn attach_mut_fix(fix: Option<(Span, String)>, d: &mut Diagnostic) {
        if let Some((at, name)) = fix {
            d.suggest_safe(at, format!("make `{name}` mutable"), "mut ");
        }
    }

    /// The names in scope a misspelled `name` may have meant. `functions` keeps only what can be called.
    fn similar_names(&self, name: &str, functions: bool) -> Vec<String> {
        let max = limit(name.chars().count());
        if max == 0 {
            return Vec::new();
        }
        let mut names: Vec<String> = Vec::new();
        if !functions {
            for scope in &self.scopes {
                names.extend(scope.keys().cloned());
            }
        }
        for (candidate, global) in &self.globals.modules[self.module].values {
            if !functions || matches!(global, Global::Func(_) | Global::Generic(_) | Global::Extern(_)) {
                names.push(candidate.clone());
            }
        }
        names.extend(PROVIDED.iter().filter(|n| !functions || **n != "None").map(|n| n.to_string()));
        names.retain(|c| c != name && !c.starts_with("__") && distance(c, name) <= max);
        names.sort();
        names.dedup();
        names
    }

    /// The other modules of this package that declare `name` (as a function, when `functions`), by module path.
    fn declaring_modules(&self, name: &str, functions: bool) -> Vec<String> {
        let package = self.globals.package_of(self.module);
        let mut out: Vec<String> = Vec::new();
        for (index, module) in self.globals.modules.iter().enumerate() {
            if index == self.module || module.package != package || module.path.is_empty() {
                continue;
            }
            if !module.value_priv.contains_key(name) {
                continue;
            }
            let fits = match module.values.get(name) {
                Some(Global::Func(_) | Global::Generic(_) | Global::Extern(_)) => true,
                Some(Global::Const(_)) => !functions,
                None => false,
            };
            if fits {
                out.push(module.path.join("."));
            }
        }
        out.sort();
        out
    }

    /// Where a new `use` goes in this module's file, and the text to insert: after the last top-level `use` line,
    /// or after the `//!` comments at the top (with a blank line after the new `use`).
    fn import_edit(&self, line: &str) -> (u32, String) {
        let file = self.globals.modules[self.module].file;
        let text = &self.sources.get(file).text;
        let mut at = 0usize;
        let mut last_use: Option<usize> = None;
        let mut header = 0usize;
        let mut in_header = true;
        for raw in text.split_inclusive('\n') {
            let trimmed = raw.trim_end();
            if in_header && (trimmed.starts_with("//!") || trimmed.starts_with("#!")) {
                header = at + raw.len();
            } else {
                in_header = false;
            }
            if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
                last_use = Some(at + raw.len());
            }
            at += raw.len();
        }
        match last_use {
            Some(end) if text[..end].ends_with('\n') => (end as u32, format!("{line}\n")),
            Some(end) => (end as u32, format!("\n{line}")),
            None => (header as u32, format!("{line}\n\n")),
        }
    }

    /// The fix for an unknown `name`: a near spelling in scope, or an import from another module of the package.
    pub(super) fn unknown_name_fix(&self, name: &str, functions: bool) -> NameFix {
        let similar = self.similar_names(name, functions);
        match similar.len() {
            1 => return NameFix::Rename(similar.into_iter().next().expect("one name")),
            0 => {}
            _ => return NameFix::Candidates(similar),
        }
        let modules = self.declaring_modules(name, functions);
        if let [module] = modules.as_slice() {
            let line = format!("use {module}.{name}");
            let (at, text) = self.import_edit(&line);
            let file = self.globals.modules[self.module].file;
            let edit = Edit { span: Span::new(file, at, at), replacement: text };
            return NameFix::Import { path: format!("{module}.{name}"), edit };
        }
        NameFix::None
    }

    /// Attaches `fix` to the diagnostic for an unknown name written at `span`.
    pub(super) fn attach_name_fix(fix: NameFix, span: Span, d: &mut Diagnostic) {
        match fix {
            NameFix::Rename(name) => {
                d.suggest_safe(span, format!("use `{name}`, the name in scope with a similar spelling"), name);
            }
            NameFix::Candidates(names) => {
                let list: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                d.help(format!("names in scope with a similar spelling: {}", list.join(", ")));
            }
            NameFix::Import { path, edit } => {
                d.suggest_edits(format!("import `{path}`"), vec![edit], Applicability::MachineApplicable);
            }
            NameFix::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_edits_and_swaps() {
        assert_eq!(distance("count", "cuont"), 1);
        assert_eq!(distance("count", "coun"), 1);
        assert_eq!(distance("count", "mount"), 1);
        assert_eq!(distance("count", "total"), 4);
        assert_eq!(limit(2), 0);
        assert_eq!(limit(5), 1);
        assert_eq!(limit(7), 2);
    }

    #[test]
    fn mut_goes_after_let_and_for_only() {
        assert!(simple_binding("let x = 1", 4));
        assert!(simple_binding("for x in xs", 4));
        assert!(!simple_binding("let (a, x) = t", 8));
        assert!(!simple_binding("fn f(x: i32)", 5));
    }
}
