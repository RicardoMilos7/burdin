//! `else_on_local_enum` (`spec/lint.md`, LN17): a `match` whose `else` arm catches variants of an enum that the same
//! package declares. When the enum later gains a variant, the compiler's exhaustiveness check stays silent because
//! `else` accepts it, so the new case is handled by code written before it existed.
//!
//! The enum is found from the other arms: a pattern `Shape.Circle(r)` names the enum `Shape`, which must be declared in
//! the package being linted. A `match` on an enum of another package is not reported: its author, not this package,
//! decides when it grows. An intentional catch-all carries `// lint: allow(else_on_local_enum)` above the arm.

use crate::walk::{Visit, item_span, walk_expr};
use crate::{Ctx, Finding, Lint};
use burdin_diag::{FileId, Span};
use burdin_syntax::ast::*;
use std::collections::HashMap;

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    let mut enums: HashMap<&str, Vec<(FileId, &EnumDecl)>> = HashMap::new();
    for source in &cx.input.files {
        for item in &source.ast.items {
            if let Item::Enum(e) = item
                && !cx.is_generated(e.span)
            {
                enums.entry(e.name.name.as_str()).or_default().push((source.file, e));
            }
        }
    }
    if enums.is_empty() {
        return;
    }
    for source in &cx.input.files {
        let mut finder = Finder { cx, enums: &enums, file: source.file, self_enum: None, out: &mut *out };
        for item in &source.ast.items {
            if cx.is_generated(item_span(item)) {
                continue;
            }
            match item {
                Item::Fn(f) => finder.function(f),
                Item::Extension(x) => finder.function(&x.func),
                Item::ExternFn(x) => finder.function(&x.decl),
                Item::Impl(b) => {
                    finder.self_enum = type_name(&b.ty);
                    b.methods.iter().for_each(|m| finder.function(m));
                    finder.self_enum = None;
                }
                Item::Interface(i) => i.methods.iter().for_each(|m| finder.function(&m.decl)),
                Item::Derive(d) => d.methods.iter().for_each(|m| finder.function(m)),
                Item::Const(c) => finder.expr(&c.value),
                _ => {}
            }
        }
        for t in &source.ast.tests {
            finder.function(&t.body_func);
        }
    }
}

fn type_name(ty: &Type) -> Option<String> {
    match &ty.kind {
        TypeKind::Path(segments) => segments.last().map(|s| s.name.name.clone()),
        _ => None,
    }
}

struct Finder<'a, 'c> {
    cx: &'a Ctx<'c>,
    enums: &'a HashMap<&'a str, Vec<(FileId, &'a EnumDecl)>>,
    file: FileId,
    self_enum: Option<String>,
    out: &'a mut Vec<Finding>,
}

impl Finder<'_, '_> {
    fn function(&mut self, f: &FnDecl) {
        for p in &f.params {
            if let Some(d) = &p.default {
                self.expr(d);
            }
        }
        self.block(&f.body);
    }

    fn check_match(&mut self, arms: &[MatchArm]) {
        let Some(else_arm) = arms.iter().find(|a| a.pattern.is_none()) else { return };
        let mut named: Vec<(String, String)> = Vec::new();
        for arm in arms {
            if let Some(p) = &arm.pattern {
                self.variants_of(p, &mut named);
            }
        }
        let Some((enum_name, _)) = named.first().cloned() else { return };
        if named.iter().any(|(e, _)| *e != enum_name) {
            return;
        }
        let Some(candidates) = self.enums.get(enum_name.as_str()) else { return };
        let decl = match candidates.as_slice() {
            [(_, only)] => *only,
            many => match many.iter().find(|(file, _)| *file == self.file) {
                Some((_, e)) => *e,
                None => return,
            },
        };
        let hidden: Vec<&str> =
            decl.variants.iter().map(|v| v.name.name.as_str()).filter(|v| !named.iter().any(|(_, n)| n == v)).collect();
        if hidden.is_empty() {
            return;
        }
        let keyword = Span::new(else_arm.span.file, else_arm.span.lo, else_arm.span.lo + 4);
        let span = if self.cx.slice(keyword) == "else" { keyword } else { else_arm.span };
        let list: Vec<String> = hidden.iter().map(|v| format!("`{v}`")).collect();
        self.out.push(
            Finding::new(
                Lint::ElseOnLocalEnum,
                span,
                format!("this `else` arm hides new variants of the local enum `{enum_name}`"),
            )
            .note(format!("it currently catches {}", list.join(", ")))
            .note("list the variants so a new one is a compile error, or write `// lint: allow(else_on_local_enum)` above the arm if the catch-all is intended"),
        );
    }

    /// The `(enum, variant)` pairs a pattern names, through `|` and `name @ ...`.
    fn variants_of(&self, p: &Pattern, out: &mut Vec<(String, String)>) {
        match &p.kind {
            PatternKind::Or(ps) => ps.iter().for_each(|p| self.variants_of(p, out)),
            PatternKind::At(_, inner) => self.variants_of(inner, out),
            PatternKind::Path(path) | PatternKind::TupleStruct(path, _) | PatternKind::Struct(path, _)
                if path.len() >= 2 =>
            {
                let mut enum_name = path[path.len() - 2].name.clone();
                if enum_name == "Self" {
                    match &self.self_enum {
                        Some(name) => enum_name = name.clone(),
                        None => return,
                    }
                }
                out.push((enum_name, path[path.len() - 1].name.clone()));
            }
            _ => {}
        }
    }
}

impl Visit for Finder<'_, '_> {
    fn expr(&mut self, e: &Expr) {
        if let ExprKind::Match(_, arms) = &e.kind {
            self.check_match(arms);
        }
        walk_expr(self, e);
    }
}
