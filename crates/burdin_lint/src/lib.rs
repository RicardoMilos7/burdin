//! `burdin lint`: the built-in linter (`spec/lint.md`).
//!
//! The linter reads what the compiler already built. Lints about how code is written (names, imports, unused
//! variables, unreachable and dead code, `else` on an enum) walk the syntax tree of the package being linted. Lints
//! about what the code means (eager chains, redundant clones, never-mutated `mut`, allocations in loops) walk the
//! typed tree the checker produced, which has the types and the conventions the syntax tree lacks. Neither pass
//! changes the program: a lint finding is a [`Finding`], and [`run`] turns the findings into diagnostics after the
//! configuration (`Level`) and the suppression comments have had their say.
//!
//! The linter runs only on a program that compiles, so every name resolves and every type is known.

mod dead_code;
mod else_enum;
mod mentions;
mod missing_docs;
mod naming;
mod registry;
mod scopes;
mod suppress;
mod tir_lints;
mod unreachable;
mod walk;

pub use registry::{Config, Level, Lint};

use burdin_diag::{Diagnostic, Diagnostics, FileId, SourceMap, Span};
use burdin_syntax::ast;
use burdin_tir::Program;
use std::collections::HashMap;

/// One module of the package being linted.
pub struct Source<'a> {
    pub file: FileId,
    /// The module's path inside the package (`["geometry", "shapes"]`); empty for a one-file target.
    pub module: &'a [String],
    pub ast: &'a ast::File,
}

/// Everything a lint run reads.
pub struct Input<'a> {
    pub sources: &'a SourceMap,
    /// The modules of the package being linted (its `test` blocks included). Dependencies are not linted.
    pub files: Vec<Source<'a>>,
    /// The checked program. Its tree is the one `burdin build` would compile.
    pub program: &'a Program,
    pub config: &'a Config,
}

/// An edit that `burdin lint --fix` may apply on its own (`spec/lint.md`, LN12).
#[derive(Clone, Debug)]
pub struct Fix {
    pub span: Span,
    pub replacement: String,
    pub message: String,
}

/// What a lint found, before the configuration decides how loudly to say it.
#[derive(Clone, Debug)]
pub struct Finding {
    pub lint: Lint,
    pub span: Span,
    pub message: String,
    /// Free-standing notes that explain the finding or say how to fix it by hand.
    pub notes: Vec<String>,
    pub fixes: Vec<Fix>,
}

impl Finding {
    pub fn new(lint: Lint, span: Span, message: impl Into<String>) -> Finding {
        Finding { lint, span, message: message.into(), notes: Vec::new(), fixes: Vec::new() }
    }

    pub fn note(mut self, note: impl Into<String>) -> Finding {
        self.notes.push(note.into());
        self
    }

    pub fn fix(mut self, span: Span, message: impl Into<String>, replacement: impl Into<String>) -> Finding {
        self.fixes.push(Fix { span, replacement: replacement.into(), message: message.into() });
        self
    }
}

/// Shared state of one run.
pub(crate) struct Ctx<'a> {
    pub input: &'a Input<'a>,
}

impl Ctx<'_> {
    pub fn text(&self, file: FileId) -> &str {
        &self.input.sources.get(file).text
    }

    /// The source text of a span (empty when the span is out of range).
    pub fn slice(&self, span: Span) -> &str {
        self.text(span.file).get(span.lo as usize..span.hi as usize).unwrap_or("")
    }

    /// Whether an item's span is one the parser made up. Generated code (a derived `Debug`, the impls of an `error
    /// enum`, the wrapper around `main`) gives every token the span of one identifier, so the item's text has no
    /// whitespace, which no written item has.
    pub fn is_generated(&self, span: Span) -> bool {
        let text = self.slice(span);
        !text.is_empty() && !text.contains(char::is_whitespace) && !text.contains('{')
    }

    /// Whether the span belongs to a file of the package being linted.
    pub fn in_package(&self, span: Span) -> bool {
        self.input.files.iter().any(|f| f.file == span.file)
    }
}

/// Runs every lint that is not allowed and returns the diagnostics: a warning for a lint at `warn`, an error for one
/// at `deny`. Suppression comments are applied here, so a suppressed finding is never reported.
pub fn run(input: &Input) -> Diagnostics {
    let cx = Ctx { input };
    let config = input.config;
    let on = |lints: &[Lint]| lints.iter().any(|l| config.level(*l) != Level::Allow);
    let mut findings: Vec<Finding> = Vec::new();
    if on(&[Lint::NonSnakeCase, Lint::NonPascalCase, Lint::NonScreamingCase]) {
        naming::check(&cx, &mut findings);
    }
    if on(&[Lint::ElseOnLocalEnum]) {
        else_enum::check(&cx, &mut findings);
    }
    if on(&[Lint::UnusedImports, Lint::UnusedVariables, Lint::UnusedParameters]) {
        scopes::check(&cx, &mut findings);
    }
    if on(&[Lint::MissingDocs]) {
        missing_docs::check(&cx, &mut findings);
    }
    if on(&[Lint::UnreachableCode]) {
        unreachable::check(&cx, &mut findings);
    }
    if on(&[Lint::DeadCode]) {
        dead_code::check(&cx, &mut findings);
    }
    if on(&[
        Lint::EagerChain,
        Lint::RedundantClone,
        Lint::LargeCopyInLoop,
        Lint::UnusedMut,
        Lint::LoopInvariantAlloc,
        Lint::NeedlessToList,
        Lint::BorrowCouldOwn,
    ]) {
        tir_lints::check(&cx, &mut findings);
    }
    finish(&cx, findings)
}

/// Applies the configuration and the suppression comments and builds the diagnostics.
fn finish(cx: &Ctx, mut findings: Vec<Finding>) -> Diagnostics {
    let mut diags = Diagnostics::default();
    let mut suppressions: HashMap<FileId, suppress::Suppressions> = HashMap::new();
    let mut problems = Diagnostics::default();
    for source in &cx.input.files {
        let text = cx.text(source.file);
        suppressions.insert(source.file, suppress::read(source.file, text, source.ast, &mut problems));
    }
    // Findings come in a stable order, and a span reports each lint once (a generic body is checked once per
    // instance, so the typed lints can find the same span twice).
    findings.sort_by_key(|f| (f.span.file.0, f.span.lo, f.span.hi, f.lint as usize));
    findings.dedup_by(|a, b| a.lint == b.lint && a.span == b.span);
    for finding in findings {
        let level = cx.input.config.level(finding.lint);
        if level == Level::Allow {
            continue;
        }
        if suppressions.get(&finding.span.file).is_some_and(|s| s.covers(finding.lint, finding.span)) {
            continue;
        }
        let code = burdin_diag::Code(finding.lint.code());
        let d = match level {
            Level::Deny => diags.error(code, finding.span, finding.message),
            _ => diags.warning(code, finding.span, finding.message),
        };
        for note in finding.notes {
            d.note_without_span(note);
        }
        for fix in finding.fixes {
            d.suggest_safe(fix.span, fix.message, fix.replacement);
        }
    }
    for d in problems.into_vec() {
        diags.push(d);
    }
    diags
}

/// The fixes a set of diagnostics offers: the machine-applicable suggestions, each a group of edits that apply
/// together (`spec/fix.md`, FX2). Children's suggestions count too.
pub fn applicable_suggestions(diags: &Diagnostics) -> Vec<burdin_diag::Suggestion> {
    let mut out = Vec::new();
    for d in diags.iter() {
        collect(d, &mut out);
    }
    out
}

fn collect(d: &Diagnostic, out: &mut Vec<burdin_diag::Suggestion>) {
    out.extend(d.suggestions.iter().filter(|s| s.is_machine_applicable()).cloned());
    for child in &d.children {
        collect(child, out);
    }
}

pub use burdin_diag::edit::{EditConflict, apply_edits, unified_diff};
