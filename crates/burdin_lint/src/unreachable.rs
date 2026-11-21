//! `unreachable_code` (`spec/lint.md`, LN18): a statement that follows one that never completes. A statement never
//! completes when it is a `return`, `break`, or `continue`, a call that does not return (`panic`, `unreachable`,
//! `todo`, `unimplemented`, `process.exit`, or a function declared `: !`), a `loop` with no `break`, or a block, `if`,
//! or `match` whose every path does one of those.

use crate::walk::{Visit, for_each_fn, walk_block, walk_expr};
use crate::{Ctx, Finding, Lint};
use burdin_syntax::ast::*;
use std::collections::HashSet;

const NEVER_RETURNS: [&str; 5] = ["panic", "unreachable", "todo", "unimplemented", "exit"];

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    // Functions of the package declared to return `!`, by name.
    let mut never: HashSet<String> = HashSet::new();
    for source in &cx.input.files {
        for_each_fn(source.ast, &|span| cx.is_generated(span), &mut |_, f| {
            if f.ret.as_ref().is_some_and(|t| matches!(t.kind, TypeKind::Never)) {
                never.insert(f.name.name.clone());
            }
        });
    }
    for source in &cx.input.files {
        let mut finder = Finder { never: &never, out: &mut *out };
        for_each_fn(source.ast, &|span| cx.is_generated(span), &mut |_, f| finder.block(&f.body));
    }
}

struct Finder<'a> {
    never: &'a HashSet<String>,
    out: &'a mut Vec<Finding>,
}

impl Visit for Finder<'_> {
    fn block(&mut self, b: &Block) {
        if let Some(i) = b.stmts.iter().position(|s| self.stmt_diverges(s))
            && let Some(next) = b.stmts.get(i + 1)
        {
            let span_of = |s: &Stmt| match s {
                Stmt::Let { span, .. } => *span,
                Stmt::Expr(e) => e.span,
            };
            let span = span_of(next);
            // A statement the parser added (the `Ok(())` at the end of a `test` body) is anchored before the code that
            // precedes it; written code always follows what it follows.
            if span.lo < span_of(&b.stmts[i]).hi {
                walk_block(self, b);
                return;
            }
            self.out.push(
                Finding::new(Lint::UnreachableCode, span, "unreachable code")
                    .note("the statement before it never completes, so this one cannot run"),
            );
        }
        walk_block(self, b);
    }
}

impl Finder<'_> {
    fn stmt_diverges(&self, s: &Stmt) -> bool {
        match s {
            Stmt::Let { init, .. } => self.diverges(init),
            Stmt::Expr(e) => self.diverges(e),
        }
    }

    fn block_diverges(&self, b: &Block) -> bool {
        b.stmts.iter().any(|s| self.stmt_diverges(s))
    }

    fn diverges(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Return(_) | ExprKind::Break(_) | ExprKind::Continue(_) => true,
            ExprKind::Call(callee, _) => self.never_returns(callee),
            ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => self.block_diverges(b),
            ExprKind::If { then_block, else_expr: Some(other), .. }
            | ExprKind::IfLet { then_block, else_expr: Some(other), .. } => {
                self.block_diverges(then_block) && self.diverges(other)
            }
            ExprKind::Match(_, arms) => !arms.is_empty() && arms.iter().all(|a| self.diverges(&a.body)),
            ExprKind::Loop(b) => !contains_break(b, None),
            ExprKind::Labeled { label, body } => match &body.kind {
                ExprKind::Loop(b) => !contains_break(b, Some(&label.name)),
                _ => self.diverges(body),
            },
            _ => false,
        }
    }

    fn never_returns(&self, callee: &Expr) -> bool {
        match &callee.kind {
            ExprKind::Name { name, .. } => {
                NEVER_RETURNS.contains(&name.name.as_str()) || self.never.contains(&name.name)
            }
            ExprKind::Field(recv, name) => match &recv.kind {
                ExprKind::Name { name: module, .. } if module.name == "process" => name.name == "exit",
                _ => self.never.contains(&name.name),
            },
            _ => false,
        }
    }
}

/// Whether a `break` leaves this loop body: a `break` inside a nested loop or a closure leaves that one instead.
/// Whether a loop's body has a `break` that leaves the loop: an unlabeled one outside any nested loop, or one that
/// names the loop by `label`, from anywhere in the body.
fn contains_break(body: &Block, label: Option<&str>) -> bool {
    struct Breaks<'l> {
        label: Option<&'l str>,
        nested: usize,
        found: bool,
    }
    impl Visit for Breaks<'_> {
        fn expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Break(None) if self.nested == 0 => self.found = true,
                ExprKind::Break(Some(name)) if Some(name.name.as_str()) == self.label => self.found = true,
                ExprKind::Loop(_) | ExprKind::While { .. } | ExprKind::For { .. } => {
                    self.nested += 1;
                    walk_expr(self, e);
                    self.nested -= 1;
                }
                ExprKind::Closure { .. } => {}
                _ => walk_expr(self, e),
            }
        }
    }
    let mut breaks = Breaks { label, nested: 0, found: false };
    breaks.block(body);
    breaks.found
}
