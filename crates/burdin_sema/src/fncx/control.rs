//! Control flow: `if`, `for`, `match`, and `if let`.

use super::{FnCx, mk};
use crate::types::compatible;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, LocalKind, Pat, Ty};
use std::collections::HashMap;

impl FnCx<'_> {
    pub(super) fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let cond = self.expr(cond, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        let Some(else_expr) = else_expr else {
            return mk(tir::ExprKind::If(Box::new(cond), then, None), Ty::Unit, span);
        };
        let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
        let other = self.expr(else_expr, else_expect.as_ref());
        let ty = self.unify(then_ty, &other.ty, other.span, "`if` and `else` have different types");
        mk(tir::ExprKind::If(Box::new(cond), then, Some(Box::new(other))), ty, span)
    }

    /// The type of two branches that must agree; reports E0003 at `span` when they do not.
    pub(super) fn unify(&mut self, first: Ty, second: &Ty, span: Span, what: &str) -> Ty {
        if first == Ty::Never || first == Ty::Error {
            return second.clone();
        }
        if !compatible(second, &first) {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("{what}: `{first}` and `{second}`"));
        }
        first
    }

    pub(super) fn for_expr(
        &mut self,
        pattern: &ast::Pattern,
        iter: &ast::Expr,
        body: &ast::Block,
        span: Span,
    ) -> tir::Expr {
        let ast::ExprKind::Range { lo, hi, inclusive } = &iter.kind else {
            let array = self.expr(iter, None);
            let elem = match &array.ty {
                Ty::Array(t, _) | Ty::Slice(t) => (**t).clone(),
                Ty::Error | Ty::Never => Ty::Error,
                other => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        iter.span,
                        format!("cannot iterate over a value of type `{other}`"),
                    );
                    Ty::Error
                }
            };
            self.scopes.push(HashMap::new());
            // `for` borrows the collection, so its elements are read-only views.
            let pat = self.pattern(pattern, &elem, false);
            if !self.exhaustive(&elem, &[&pat]) {
                self.diags.error(
                    Code::NON_EXHAUSTIVE,
                    pattern.span,
                    format!("this pattern does not match every value of `{elem}`"),
                );
            }
            self.loops.push(false);
            let (body, _) = self.block(body, None);
            self.loops.pop();
            self.scopes.pop();
            let kind = if matches!(array.ty, Ty::Slice(_)) {
                tir::ExprKind::ForSlice { pat, slice: Box::new(array), body }
            } else {
                tir::ExprKind::ForArray { pat, array: Box::new(array), body }
            };
            return mk(kind, Ty::Unit, span);
        };
        let (lo, hi) = self.operands(lo, hi, None);
        let ty = if lo.ty == Ty::Never { hi.ty.clone() } else { lo.ty.clone() };
        let hi = self.expect_ty(hi, &ty);
        if !matches!(ty, Ty::Int(_) | Ty::Error) {
            self.diags.error(Code::TYPE_MISMATCH, iter.span, format!("cannot iterate over a range of `{ty}`"));
        }
        self.scopes.push(HashMap::new());
        let var = match &pattern.kind {
            ast::PatternKind::Binding { name, mutable } => {
                Some(self.declare(&name.name, ty, *mutable, LocalKind::Owned))
            }
            ast::PatternKind::Wild => None,
            _ => {
                self.scopes.pop();
                return self.unsupported(pattern.span, "this `for` pattern");
            }
        };
        self.loops.push(false);
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.scopes.pop();
        let kind = tir::ExprKind::ForRange { var, lo: Box::new(lo), hi: Box::new(hi), inclusive: *inclusive, body };
        mk(kind, Ty::Unit, span)
    }

    pub(super) fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let scrutinee = self.expr(scrutinee, None);
        let owned = self.owns_scrutinee(&scrutinee);
        let mut ty = Ty::Never;
        let mut checked = Vec::new();
        for arm in arms {
            self.scopes.push(HashMap::new());
            let pat = match &arm.pattern {
                Some(p) => self.pattern(p, &scrutinee.ty, owned),
                None => Pat::Wild,
            };
            let guard = arm.guard.as_ref().map(|g| {
                let g = self.expr(g, Some(&Ty::Bool));
                self.expect_ty(g, &Ty::Bool)
            });
            let arm_expect = if ty == Ty::Never { expect.cloned() } else { Some(ty.clone()) };
            let body = self.expr(&arm.body, arm_expect.as_ref());
            ty = self.unify(ty, &body.ty, body.span, "match arms have different types");
            self.scopes.pop();
            checked.push(tir::Arm { pat, guard, body });
        }
        let unguarded: Vec<&Pat> = checked.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
        if !self.exhaustive(&scrutinee.ty, &unguarded) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                span,
                format!("this `match` does not cover every value of `{}`", scrutinee.ty),
            );
        }
        mk(tir::ExprKind::Match(Box::new(scrutinee), checked), ty, span)
    }

    /// `if let pattern = scrutinee { ... } else { ... }`, checked as a `match` with a catch-all arm.
    pub(super) fn if_let(
        &mut self,
        pattern: &ast::Pattern,
        scrutinee: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let scrutinee = self.expr(scrutinee, None);
        let owned = self.owns_scrutinee(&scrutinee);
        self.scopes.push(HashMap::new());
        let pat = self.pattern(pattern, &scrutinee.ty, owned);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        self.scopes.pop();
        let then = mk(tir::ExprKind::Block(then), then_ty.clone(), then_block.span);
        let (other, ty) = match else_expr {
            Some(e) => {
                let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
                let other = self.expr(e, else_expect.as_ref());
                let ty = self.unify(then_ty, &other.ty, other.span, "`if let` and `else` have different types");
                (other, ty)
            }
            None => (mk(tir::ExprKind::Unit, Ty::Unit, span), Ty::Unit),
        };
        let arms =
            vec![tir::Arm { pat, guard: None, body: then }, tir::Arm { pat: Pat::Wild, guard: None, body: other }];
        mk(tir::ExprKind::Match(Box::new(scrutinee), arms), ty, span)
    }
}
