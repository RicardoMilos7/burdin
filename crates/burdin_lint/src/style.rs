//! The small style lints (`spec/lint.md`, LN34-LN39): `needless_return`, `bool_comparison`, `needless_bool`,
//! `empty_else`, `redundant_parens`, and `let_and_return`. Each reads only how the code is written, so it walks the
//! syntax tree; none needs a type. A finding gets a `--fix` edit only when the edit cannot change what the program
//! does, which means no comment is lost and no line break is removed from inside parentheses.

use crate::walk::{Visit, for_each_fn, walk_block, walk_expr, walk_stmt};
use crate::{Ctx, Finding, Lint};
use burdin_diag::Span;
use burdin_syntax::ast::*;

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    for source in &cx.input.files {
        let mut finder = Finder { cx, out: &mut *out };
        for_each_fn(source.ast, &|span| cx.is_generated(span), &mut |_, f| {
            finder.tail_returns(&f.body);
            finder.block(&f.body);
        });
    }
}

struct Finder<'a, 'c> {
    cx: &'a Ctx<'c>,
    out: &'a mut Vec<Finding>,
}

/// Whether the text has a comment, a string, or a character, which the text scans below do not read.
fn is_plain(text: &str) -> bool {
    !(text.contains("//") || text.contains("/*") || text.contains('"') || text.contains('\''))
}

impl Finder<'_, '_> {
    fn text(&self, span: Span) -> &str {
        self.cx.slice(span)
    }

    // --- needless_return -----------------------------------------------------------------------------------------

    /// Reports a `return` in the tail position of a function body: the last statement, or the last statement of the
    /// branches of a tail `if`/`else`, `match`, or block.
    fn tail_returns(&mut self, body: &Block) {
        if let Some(Stmt::Expr(last)) = body.stmts.last() {
            self.tail_expr(last);
        }
    }

    fn tail_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Return(value) => {
                let mut finding =
                    Finding::new(Lint::NeedlessReturn, e.span, "unneeded `return` at the end of a function")
                        .note("the last expression of a block is its value, so `return` adds nothing");
                if let Some(value) = value {
                    // `return (a * 2)` becomes `a * 2`: the parentheses, when they hold one line, are redundant too.
                    let written = self.text(value.span);
                    let text =
                        unwrap_parens(written).filter(|inner| !inner.contains('\n')).unwrap_or(written).to_string();
                    let plain = is_plain(self.text(e.span));
                    if plain && !text.is_empty() {
                        finding = finding.fix(e.span, "remove `return`", text);
                    }
                }
                self.out.push(finding);
            }
            ExprKind::Block(b) | ExprKind::UnsafeBlock(b) => self.tail_returns(b),
            ExprKind::If { then_block, else_expr: Some(other), .. }
            | ExprKind::IfLet { then_block, else_expr: Some(other), .. } => {
                self.tail_returns(then_block);
                self.tail_expr(other);
            }
            ExprKind::Match(_, arms) => arms.iter().for_each(|a| self.tail_expr(&a.body)),
            _ => {}
        }
    }

    // --- bool_comparison -----------------------------------------------------------------------------------------

    fn bool_comparison(&mut self, e: &Expr, op: BinOp, lhs: &Expr, rhs: &Expr) {
        if !matches!(op, BinOp::Eq | BinOp::Ne) {
            return;
        }
        let (literal, other) = match (&lhs.kind, &rhs.kind) {
            (_, ExprKind::Bool(b)) => (*b, lhs),
            (ExprKind::Bool(b), _) => (*b, rhs),
            _ => return,
        };
        // `x == true` and `x != false` are `x`; `x == false` and `x != true` are `!x`.
        let negate = (op == BinOp::Eq) != literal;
        let operand = self.text(other.span).to_string();
        let wrapped = if negate {
            if is_atom(self.cx, other) { format!("!{operand}") } else { format!("!({operand})") }
        } else if matches!(other.kind, ExprKind::Binary(..)) && unwrap_parens(&operand).is_none() {
            format!("({operand})")
        } else {
            operand.clone()
        };
        let literal_text = if literal { "true" } else { "false" };
        let mut finding =
            Finding::new(Lint::BoolComparison, e.span, format!("comparison with the boolean literal `{literal_text}`"))
                .note(if negate { "write `!x` instead of comparing with a literal" } else { "use the value itself" });
        if is_plain(self.text(e.span)) && !operand.is_empty() {
            finding = finding.fix(e.span, "remove the comparison", wrapped);
        }
        self.out.push(finding);
    }

    // --- needless_bool -------------------------------------------------------------------------------------------

    /// `if c { true } else { false }` and its `return` form. The branch values are the booleans, or `return` of one.
    fn needless_bool(&mut self, e: &Expr, cond: &Expr, then_block: &Block, other: &Expr) {
        let ExprKind::Block(else_block) = &other.kind else { return };
        let Some((then_value, then_returns)) = bool_branch(then_block) else { return };
        let Some((else_value, else_returns)) = bool_branch(else_block) else { return };
        if then_value == else_value || then_returns != else_returns {
            return;
        }
        let condition = self.text(cond.span).to_string();
        let value = if then_value {
            condition.clone()
        } else if is_atom(self.cx, cond) {
            format!("!{condition}")
        } else {
            format!("!({condition})")
        };
        let text = if then_returns { format!("return {value}") } else { value };
        let mut finding = Finding::new(Lint::NeedlessBool, e.span, "this `if` only produces a boolean literal")
            .note(if then_value { "use the condition itself" } else { "use the negated condition" });
        if is_plain(self.text(e.span)) && !condition.is_empty() {
            finding = finding.fix(e.span, "use the condition directly", text);
        }
        self.out.push(finding);
    }

    // --- empty_else ----------------------------------------------------------------------------------------------

    fn empty_else(&mut self, then_block: &Block, other: &Expr) {
        let ExprKind::Block(b) = &other.kind else { return };
        if !b.stmts.is_empty() {
            return;
        }
        let whole = Span::new(then_block.span.file, then_block.span.hi, other.span.hi);
        let mut finding = Finding::new(Lint::EmptyElse, other.span, "this `else` block is empty")
            .note("an `if` without an `else` does the same");
        if is_plain(self.text(whole)) {
            finding = finding.fix(whole, "remove the empty `else`", "");
        }
        self.out.push(finding);
    }

    // --- redundant_parens ----------------------------------------------------------------------------------------

    /// Reports parentheses around the whole of `e`, in a place that already takes a full expression. `restricted` is
    /// true for the head of an `if`, `while`, `match`, or `for`, where a struct literal needs its parentheses.
    fn redundant_parens(&mut self, e: &Expr, restricted: bool) {
        if matches!(e.kind, ExprKind::Tuple(_) | ExprKind::Unit) {
            return;
        }
        let text = self.text(e.span);
        let Some(inner) = unwrap_parens(text) else { return };
        if restricted && (inner.contains('{') || matches!(e.kind, ExprKind::StructLit(..))) {
            return;
        }
        let inner = inner.to_string();
        let mut finding = Finding::new(Lint::RedundantParens, e.span, "redundant parentheses")
            .note("this position takes a whole expression, so the parentheses change nothing");
        // Inside parentheses a line break does not end a statement, outside it does.
        if !inner.contains('\n') {
            finding = finding.fix(e.span, "remove the parentheses", inner);
        }
        self.out.push(finding);
    }

    // --- let_and_return ------------------------------------------------------------------------------------------

    fn let_and_return(&mut self, b: &Block) {
        let [.., Stmt::Let { pattern, ty, init, else_block: None, span: let_span }, Stmt::Expr(tail)] =
            b.stmts.as_slice()
        else {
            return;
        };
        let (PatternKind::Binding { name, .. }, ExprKind::Name { name: used, args }) = (&pattern.kind, &tail.kind)
        else {
            return;
        };
        if name.name != used.name
            || !args.is_empty()
            || name.name.starts_with(char::is_uppercase)
            || let_span.lo >= tail.span.lo
        {
            return;
        }
        let mut finding =
            Finding::new(Lint::LetAndReturn, *let_span, format!("`{}` is bound and returned at once", name.name))
                .note("return the value directly");
        let whole = Span::new(let_span.file, let_span.lo, tail.span.hi);
        // A written type steers inference, and a comment between the two lines would be lost.
        if ty.is_none() && is_plain(self.text(whole)) {
            let value = self.text(init.span).to_string();
            if !value.is_empty() {
                finding = finding.fix(whole, "return the value directly", value);
            }
        }
        self.out.push(finding);
    }
}

/// `{ true }`, `{ false }`, `{ return true }`, or `{ return false }`: the boolean and whether it is returned.
fn bool_branch(b: &Block) -> Option<(bool, bool)> {
    let [Stmt::Expr(e)] = b.stmts.as_slice() else { return None };
    match &e.kind {
        ExprKind::Bool(v) => Some((*v, false)),
        ExprKind::Return(Some(inner)) => match inner.kind {
            ExprKind::Bool(v) => Some((v, true)),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `!` applies to the expression without parentheses: an atom, or an expression that already has them.
fn is_atom(cx: &Ctx, e: &Expr) -> bool {
    unwrap_parens(cx.slice(e.span)).is_some()
        || matches!(
            e.kind,
            ExprKind::Name { .. }
                | ExprKind::SelfValue
                | ExprKind::Bool(_)
                | ExprKind::Call(..)
                | ExprKind::Field(..)
                | ExprKind::TupleField(..)
                | ExprKind::Index(..)
                | ExprKind::Unary(UnOp::Not, _)
        )
}

/// The text between a pair of parentheses that encloses all of `text`, trimmed; `None` when the text is not one such
/// pair (`(a) + (b)`), or holds a comment, a string, or a character that this scan does not read.
fn unwrap_parens(text: &str) -> Option<&str> {
    if !text.starts_with('(') || !text.ends_with(')') || !is_plain(text) {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (i + 1 == text.len()).then(|| text[1..i].trim()).filter(|s| !s.is_empty());
                }
            }
            _ => {}
        }
    }
    None
}

impl Visit for Finder<'_, '_> {
    fn block(&mut self, b: &Block) {
        self.let_and_return(b);
        walk_block(self, b);
    }

    fn stmt(&mut self, s: &Stmt) {
        if let Stmt::Let { init, .. } = s {
            self.redundant_parens(init, false);
        }
        walk_stmt(self, s);
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Binary(op, lhs, rhs) => self.bool_comparison(e, *op, lhs, rhs),
            ExprKind::If { cond, then_block, else_expr } => {
                self.redundant_parens(cond, true);
                if let Some(other) = else_expr {
                    self.needless_bool(e, cond, then_block, other);
                    self.empty_else(then_block, other);
                }
            }
            ExprKind::IfLet { scrutinee, then_block, else_expr, .. } => {
                self.redundant_parens(scrutinee, true);
                if let Some(other) = else_expr {
                    self.empty_else(then_block, other);
                }
            }
            ExprKind::While { cond, .. } => self.redundant_parens(cond, true),
            ExprKind::WhileLet { scrutinee, .. } => self.redundant_parens(scrutinee, true),
            ExprKind::Match(scrutinee, _) => self.redundant_parens(scrutinee, true),
            ExprKind::For { iter, .. } => self.redundant_parens(iter, true),
            ExprKind::Return(Some(value)) | ExprKind::Break { value: Some(value), .. } => {
                self.redundant_parens(value, false)
            }
            ExprKind::Assign(_, value) | ExprKind::CompoundAssign(_, _, value) => self.redundant_parens(value, false),
            ExprKind::Call(_, args) => args.iter().for_each(|a| self.redundant_parens(&a.expr, false)),
            _ => {}
        }
        walk_expr(self, e);
    }
}
