//! Comptime expressions, `comptime if`, `comptime for`, `comptime assert`, and the arguments of `comptime`
//! parameters (`spec/comptime.md`). Checking only builds the typed tree: the values are computed by
//! `burdin_comptime` after the memory checks.

use super::{FnCx, LoopScope, error_expr, mk};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::visit::pat_locals;
use burdin_tir::{self as tir, Builtin, Ty};
use std::collections::HashMap;

impl FnCx<'_> {
    /// `comptime E`, and the `comptime if` and `comptime for` forms it covers (CT2, CT5, CT6).
    pub(super) fn comptime_expr(&mut self, inner: &ast::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        match &inner.kind {
            ast::ExprKind::If { cond, then_block, else_expr } => {
                self.comptime_if(cond, then_block, else_expr.as_deref(), expect, span)
            }
            ast::ExprKind::For { pattern, iter, body, .. } => self.comptime_for(pattern, iter, body, span),
            _ => {
                let value = self.expr(inner, expect);
                self.comptime_value(value, span)
            }
        }
    }

    /// Wraps a checked expression as a comptime value: it may use only compile-time-known locals (CT2), and its
    /// type must be materializable (CT3).
    pub(super) fn comptime_value(&mut self, value: tir::Expr, span: Span) -> tir::Expr {
        if value.ty == Ty::Error {
            return value;
        }
        if !self.comptime_refs_ok(&value) {
            return error_expr(span);
        }
        if !self.globals.materializable(&value.ty) {
            self.diags.error(
                Code::NOT_MATERIALIZABLE,
                span,
                format!(
                    "a value of type `{}` cannot be computed at compile time and used in the program; comptime code \
                     may use it, but the result must be a scalar, `str`, `String`, or a struct, enum, tuple, array, \
                     `Option`, or `Result` of those (spec/comptime.md, CT3)",
                    value.ty
                ),
            );
            return error_expr(span);
        }
        let ty = value.ty.clone();
        mk(tir::ExprKind::Comptime(Box::new(value)), ty, span)
    }

    /// Reports every runtime local `e` uses (E0220). A comptime parameter and a `comptime for` binding are
    /// compile-time known; anything else of the enclosing function is not.
    fn comptime_refs_ok(&mut self, e: &tir::Expr) -> bool {
        let mut ok = true;
        for (local, at) in tir::visit::free_locals(e) {
            if self.comptime_known.contains(&local) {
                continue;
            }
            ok = false;
            let name = self.locals[local.0].name.clone();
            let what = if name == "self" { "`self`".to_string() } else { format!("the local `{name}`") };
            self.diags.error(
                Code::INVALID_COMPTIME,
                at,
                format!(
                    "{what} is a run-time value, so it cannot be used at compile time; only constants, `comptime` \
                     parameters, `comptime for` bindings, and locals declared inside the comptime expression can \
                     (spec/comptime.md, CT2)"
                ),
            );
        }
        ok
    }

    /// `comptime if`: the condition is a comptime value, and an `else if` chain is comptime throughout (CT5).
    fn comptime_if(
        &mut self,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let checked = self.expr(cond, Some(&Ty::Bool));
        let checked = self.expect_ty(checked, &Ty::Bool);
        let cond = self.comptime_value(checked, cond.span);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        let Some(else_expr) = else_expr else {
            return mk(tir::ExprKind::If(Box::new(cond), then, None), Ty::Unit, span);
        };
        let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
        let other = match &else_expr.kind {
            ast::ExprKind::If { cond, then_block, else_expr: rest } => {
                self.comptime_if(cond, then_block, rest.as_deref(), else_expect.as_ref(), else_expr.span)
            }
            _ => self.expr(else_expr, else_expect.as_ref()),
        };
        let ty = self.unify(then_ty, &other.ty, other.span, "`if` and `else` have different types");
        mk(tir::ExprKind::If(Box::new(cond), then, Some(Box::new(other))), ty, span)
    }

    /// `comptime for pattern in iter { body }` (CT6).
    fn comptime_for(&mut self, pattern: &ast::Pattern, iter: &ast::Expr, body: &ast::Block, span: Span) -> tir::Expr {
        // A loop over a type's fields or variants is unrolled here, where `x.{field.name}` can be resolved (RFL4).
        if let Some(unrolled) = self.comptime_for_reflect(pattern, iter, body, span) {
            return unrolled;
        }
        let checked = self.expr(iter, None);
        let item = match &checked.ty {
            Ty::Range(t, _) if t.is_int() => Some((**t).clone()),
            Ty::Array(t, _) | Ty::List(t, _) => Some((**t).clone()),
            Ty::Error => None,
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    iter.span,
                    format!("`comptime for` iterates over a range of integers, an array, or a `List`, not `{other}`"),
                );
                None
            }
        };
        let item_ty = item.clone().unwrap_or(Ty::Error);
        let mut iter_value = checked;
        if let Some(t) = &item {
            if !self.globals.materializable(t) {
                self.diags.error(
                    Code::NOT_MATERIALIZABLE,
                    iter.span,
                    format!("the items of a `comptime for` must be compile-time values, and `{t}` is not one (spec/comptime.md, CT3)"),
                );
                iter_value = error_expr(iter.span);
            } else {
                // The iterable itself may be a `List` (not materializable), so only the locals are checked here;
                // the items are what the unrolled bodies receive.
                if !self.comptime_refs_ok(&iter_value) {
                    iter_value = error_expr(iter.span);
                }
            }
        }
        self.scopes.push(HashMap::new());
        let pat = self.pattern(pattern, &item_ty, true);
        let mut bound = Vec::new();
        pat_locals(&pat, &mut bound);
        for &local in &bound {
            if self.locals[local.0].mutable {
                self.diags.error(
                    Code::INVALID_COMPTIME,
                    pattern.span,
                    "a `comptime for` binding cannot be `mut`: it is a compile-time value (spec/comptime.md, CT6)",
                );
            }
        }
        if item.is_some() && !self.exhaustive(&item_ty, &[&pat]) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                pattern.span,
                format!("this pattern does not match every value of `{item_ty}`"),
            );
        }
        self.comptime_known.extend(bound.iter().copied());
        // `break` and `continue` cannot leave the unrolled body: it is not a loop in the generated code.
        let saved_barrier = self.comptime_loop_barrier.replace(self.loops.len());
        self.loops.push(LoopScope::for_body(None));
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.comptime_loop_barrier = saved_barrier;
        self.scopes.pop();
        if iter_value.ty == Ty::Error {
            return error_expr(span);
        }
        mk(tir::ExprKind::ComptimeFor { pat, iter: Box::new(iter_value), body }, Ty::Unit, span)
    }

    /// `comptime assert(condition)` or `comptime assert(condition, "message")` (CT8): a comptime expression of
    /// type `()` that fails compilation when the condition is false.
    pub(super) fn comptime_assert(&mut self, cond: &ast::Expr, message: Option<&ast::Expr>, span: Span) -> tir::Expr {
        let checked = self.expr(cond, Some(&Ty::Bool));
        let checked = self.expect_ty(checked, &Ty::Bool);
        let text = match message.map(|m| (&m.kind, m.span)) {
            None => String::new(),
            Some((ast::ExprKind::Str(s), _)) => s.clone(),
            Some((_, at)) => {
                self.diags.error(
                    Code::INVALID_COMPTIME,
                    at,
                    "the message of `comptime assert` must be a string literal (spec/comptime.md, CT8)",
                );
                String::new()
            }
        };
        if checked.ty == Ty::Error {
            return error_expr(span);
        }
        let message = mk(tir::ExprKind::Str(text), Ty::Str, span);
        let call = mk(tir::ExprKind::Builtin(Builtin::ComptimeAssert, vec![checked, message]), Ty::Unit, span);
        self.comptime_value(call, span)
    }

    /// The argument of a `comptime` parameter is checked as `comptime argument` (CT4).
    pub(super) fn comptime_argument(&mut self, arg: tir::Expr) -> tir::Expr {
        if matches!(arg.kind, tir::ExprKind::Comptime(_)) {
            return arg;
        }
        let span = arg.span;
        self.comptime_value(arg, span)
    }

    /// `break` or `continue` directly inside a `comptime for` body (CT6): returns whether it was reported.
    /// Whether a `break` or `continue` that names the loop scope at `target` would leave the body of a `comptime for`
    /// (CT6). The body's own scope is the barrier; a scope inside it is a loop of the generated code, and a scope
    /// outside it is not.
    pub(super) fn leaves_comptime_for(&mut self, word: &str, span: Span, target: usize) -> bool {
        if self.comptime_loop_barrier.is_some_and(|barrier| target <= barrier) {
            self.diags.error(
                Code::INVALID_COMPTIME,
                span,
                format!(
                    "`{word}` cannot leave a `comptime for` body: the body is repeated in the generated code, not \
                     looped (spec/comptime.md, CT6)"
                ),
            );
            return true;
        }
        false
    }
}
