//! `unused_imports`, `unused_variables`, and `unused_parameters` (`spec/lint.md`, LN14-LN16).
//!
//! Variables are resolved lexically, which is exact: a name is a local when a `let`, a pattern, a parameter, or a
//! closure parameter that is in scope binds it. A read is any use except being the whole left side of `=`. Imports are
//! checked by name against everything the file mentions (`mentions.rs`), so an import is reported only when its name
//! appears nowhere in the file.

use crate::mentions::Mentions;
use crate::walk::{FnOrigin, Visit, for_each_fn, walk_block, walk_expr};
use crate::{Ctx, Finding, Lint, Source};
use burdin_diag::Span;
use burdin_syntax::ast::*;
use std::collections::HashMap;

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    for source in &cx.input.files {
        imports(cx, source, out);
        for_each_fn(source.ast, &|span| cx.is_generated(span), &mut |origin, f| {
            let mut r = Resolver::new(origin);
            r.function(f);
            r.report(out);
        });
    }
}

// ----- imports ----------------------------------------------------------------------------------------------------

fn imports(cx: &Ctx, source: &Source, out: &mut Vec<Finding>) {
    let file = source.ast;
    if file.uses.is_empty() {
        return;
    }
    let mut mentions = Mentions::default();
    for item in &file.items {
        mentions.item(item);
    }
    for t in &file.tests {
        mentions.function(&t.body_func);
    }
    let text = cx.text(source.file);
    for decl in &file.uses {
        // A `pub use` re-exports the name: it is part of the module's interface even when nothing here uses it.
        if decl.is_pub {
            continue;
        }
        if decl.items.is_empty() {
            let bound = decl.alias.as_ref().or(decl.path.last());
            let Some(bound) = bound else { continue };
            if !is_unused(&bound.name, &mentions) {
                continue;
            }
            let fix_span = whole_lines(text, decl.span);
            out.push(Finding::new(Lint::UnusedImports, decl.span, format!("unused import `{}`", bound.name)).fix(
                fix_span,
                "remove the unused import",
                "",
            ));
            continue;
        }
        let unused: Vec<&Ident> = decl.items.iter().filter(|i| is_unused(&i.name, &mentions)).collect();
        if unused.is_empty() {
            continue;
        }
        let kept: Vec<&Ident> = decl.items.iter().filter(|i| !is_unused(&i.name, &mentions)).collect();
        let fix = if kept.is_empty() {
            Some((whole_lines(text, decl.span), String::new(), "remove the unused import"))
        } else {
            let first = decl.items.first().expect("a brace list has items");
            let last = decl.items.last().expect("a brace list has items");
            let range = Span::new(first.span.file, first.span.lo, last.span.hi);
            // A comment between the names would be lost by rewriting the list, so leave that one to the user.
            let between = text.get(range.lo as usize..range.hi as usize).unwrap_or("");
            (!between.contains("//") && !between.contains("/*")).then(|| {
                let names: Vec<&str> = kept.iter().map(|i| i.name.as_str()).collect();
                (range, names.join(", "), "remove the unused names from the list")
            })
        };
        for item in unused {
            let mut finding = Finding::new(Lint::UnusedImports, item.span, format!("unused import `{}`", item.name));
            if let Some((span, replacement, message)) = &fix {
                finding = finding.fix(*span, *message, replacement.clone());
            }
            out.push(finding);
        }
    }
}

fn is_unused(name: &str, mentions: &Mentions) -> bool {
    !name.starts_with('_') && !mentions.names.contains(name)
}

/// The span to delete so a declaration that stands alone on its lines leaves no blank line behind.
fn whole_lines(text: &str, span: Span) -> Span {
    let (lo, hi) = (span.lo as usize, span.hi as usize);
    let line_start = text[..lo].rfind('\n').map_or(0, |n| n + 1);
    let line_end = text[hi..].find('\n').map_or(text.len(), |n| hi + n + 1);
    let alone_before = text[line_start..lo].trim().is_empty();
    let alone_after = text[hi..line_end].trim().is_empty();
    if alone_before && alone_after { Span::new(span.file, line_start as u32, line_end as u32) } else { span }
}

// ----- variables and parameters -------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Param,
    Variable,
}

struct Binding {
    name: String,
    span: Span,
    kind: Kind,
    used: bool,
    /// `Point { x }`: the name is also the field's, so it cannot be renamed.
    shorthand: bool,
    /// A `mut` parameter: assigning to it writes into the caller's variable, which counts as a use.
    is_mut_param: bool,
}

struct Resolver {
    origin: FnOrigin,
    scopes: Vec<HashMap<String, usize>>,
    bindings: Vec<Binding>,
    /// Whether unused parameters are reported at all for this function.
    check_params: bool,
}

impl Resolver {
    fn new(origin: FnOrigin) -> Resolver {
        Resolver { origin, scopes: Vec::new(), bindings: Vec::new(), check_params: false }
    }

    fn function(&mut self, f: &FnDecl) {
        // The interface dictates the parameters of an implementation, a derive, and an interface method; an
        // exported function's signature is a C contract; an empty body is a placeholder.
        self.check_params = matches!(self.origin, FnOrigin::Free | FnOrigin::Method) && !f.body.stmts.is_empty();
        self.scopes.push(HashMap::new());
        for p in &f.params {
            if let ParamKind::Named { name, ty } = &p.kind {
                self.ty(ty);
                if let Some(d) = &p.default {
                    self.expr(d);
                }
                self.declare(name, Kind::Param, false, false, p.convention == Convention::Mut);
            }
        }
        self.block(&f.body);
        self.scopes.pop();
    }

    fn declare(&mut self, name: &Ident, kind: Kind, used: bool, shorthand: bool, is_mut_param: bool) -> usize {
        let id = self.bindings.len();
        self.bindings.push(Binding { name: name.name.clone(), span: name.span, kind, used, shorthand, is_mut_param });
        self.scopes.last_mut().expect("a scope is open").insert(name.name.clone(), id);
        id
    }

    fn use_name(&mut self, name: &str) {
        if let Some(id) = self.lookup(name) {
            self.bindings[id].used = true;
        }
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Binds every name a pattern introduces in the innermost scope. An alternative of `A(x) | B(x)` binds the same
    /// name again, which is one binding.
    fn bind_pattern(&mut self, p: &Pattern) {
        let mut seen: HashMap<String, usize> = HashMap::new();
        self.bind_into(p, &mut seen);
    }

    fn bind_into(&mut self, p: &Pattern, seen: &mut HashMap<String, usize>) {
        match &p.kind {
            // An uppercase name is a unit variant or a constant (`spec/grammar.md`, PatPath).
            PatternKind::Binding { name, .. } if starts_lowercase(&name.name) => self.bind_name(name, false, seen),
            PatternKind::At(name, inner) => {
                self.bind_name(name, false, seen);
                self.bind_into(inner, seen);
            }
            PatternKind::Typed { name: Some(name), .. } => self.bind_name(name, false, seen),
            PatternKind::Tuple(ps) | PatternKind::Or(ps) | PatternKind::TupleStruct(_, ps) => {
                ps.iter().for_each(|p| self.bind_into(p, seen));
            }
            PatternKind::Struct(_, fields, _) => {
                for f in fields {
                    match &f.pattern {
                        Some(p) => self.bind_into(p, seen),
                        None => self.bind_name(&f.name, true, seen),
                    }
                }
            }
            _ => {}
        }
    }

    fn bind_name(&mut self, name: &Ident, shorthand: bool, seen: &mut HashMap<String, usize>) {
        let scope = self.scopes.last_mut().expect("a scope is open");
        match seen.get(&name.name) {
            Some(&id) => {
                scope.insert(name.name.clone(), id);
            }
            None => {
                let id = self.declare(name, Kind::Variable, false, shorthand, false);
                seen.insert(name.name.clone(), id);
            }
        }
    }

    fn report(&self, out: &mut Vec<Finding>) {
        for b in &self.bindings {
            if b.used || b.name.starts_with('_') || b.name == "self" {
                continue;
            }
            match b.kind {
                Kind::Param if self.check_params => out.push(
                    Finding::new(Lint::UnusedParameters, b.span, format!("unused parameter `{}`", b.name)).note(format!(
                        "if the parameter is needed, prefix it with `_` (`_{}`); callers that pass it by name must change too",
                        b.name
                    )),
                ),
                Kind::Param => {}
                Kind::Variable => {
                    let mut f = Finding::new(Lint::UnusedVariables, b.span, format!("unused variable `{}`", b.name));
                    if b.shorthand {
                        f = f.note(format!("write `{0}: _{0}` to keep the field and drop the binding", b.name));
                    } else {
                        f = f.fix(
                            Span::new(b.span.file, b.span.lo, b.span.lo),
                            format!("prefix `{}` with `_` to mark it as unused", b.name),
                            "_",
                        );
                    }
                    out.push(f);
                }
            }
        }
    }
}

fn starts_lowercase(name: &str) -> bool {
    name.trim_start_matches('_').chars().next().is_some_and(|c| !c.is_uppercase())
}

impl Visit for Resolver {
    fn block(&mut self, b: &Block) {
        self.scopes.push(HashMap::new());
        walk_block(self, b);
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            // The initializer and the `else` block cannot see the names the pattern introduces.
            Stmt::Let { pattern, ty, init, else_block, .. } => {
                if let Some(ty) = ty {
                    self.ty(ty);
                }
                self.expr(init);
                if let Some(b) = else_block {
                    self.block(b);
                }
                self.bind_pattern(pattern);
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Name { name, args } => {
                self.use_name(&name.name);
                args.iter().for_each(|a| self.ty(a));
            }
            // Writing a local variable is not reading it, except for a `mut` parameter whose write is visible to the
            // caller; writing a part of a variable (`x.f = 1`, `x[i] = 1`) reads the variable to reach the part.
            ExprKind::Assign(target, value) => {
                if let ExprKind::Name { name, .. } = &target.kind {
                    if let Some(id) = self.lookup(&name.name)
                        && self.bindings[id].is_mut_param
                    {
                        self.bindings[id].used = true;
                    }
                } else {
                    self.expr(target);
                }
                self.expr(value);
            }
            // A compound assignment reads its variable as well as writing it, so the variable is used.
            ExprKind::CompoundAssign(_, target, value) => {
                self.expr(target);
                self.expr(value);
            }
            ExprKind::StructLit(_, _, fields, base) | ExprKind::TargetStruct(fields, base) => {
                for f in fields {
                    match &f.value {
                        Some(v) => self.expr(v),
                        None => self.use_name(&f.name.name),
                    }
                }
                if let Some(b) = base {
                    self.expr(b);
                }
            }
            ExprKind::Closure { params, ret, body } => {
                self.scopes.push(HashMap::new());
                for p in params {
                    if let Some(t) = &p.ty {
                        self.ty(t);
                    }
                    self.declare(&p.name, Kind::Variable, false, false, false);
                }
                if let Some(r) = ret {
                    self.ty(r);
                }
                self.expr(body);
                self.scopes.pop();
            }
            ExprKind::Match(scrutinee, arms) => {
                self.expr(scrutinee);
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    if let Some(p) = &arm.pattern {
                        self.bind_pattern(p);
                    }
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    self.expr(&arm.body);
                    self.scopes.pop();
                }
            }
            ExprKind::IfLet { pattern, scrutinee, then_block, else_expr } => {
                self.expr(scrutinee);
                self.scopes.push(HashMap::new());
                self.bind_pattern(pattern);
                self.block(then_block);
                self.scopes.pop();
                if let Some(o) = else_expr {
                    self.expr(o);
                }
            }
            ExprKind::WhileLet { pattern, scrutinee, body } => {
                self.expr(scrutinee);
                self.scopes.push(HashMap::new());
                self.bind_pattern(pattern);
                self.block(body);
                self.scopes.pop();
            }
            ExprKind::For { pattern, iter, body, .. } => {
                self.expr(iter);
                self.scopes.push(HashMap::new());
                self.bind_pattern(pattern);
                self.block(body);
                self.scopes.pop();
            }
            _ => walk_expr(self, e),
        }
    }

    // Patterns are bound by the constructs above, never by the generic walk.
    fn pattern(&mut self, _p: &Pattern) {}
}
