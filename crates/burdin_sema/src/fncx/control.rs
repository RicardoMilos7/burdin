//! Control flow: `if`, `for`, `match`, and `if let`.

use super::{FnCx, error_expr, mk};
use crate::types::compatible;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, LocalId, LocalKind, Pat, Ty};
use std::collections::HashMap;

/// Which kind of `for` loop a non-range iterable produces.
enum ForKind {
    Array,
    Slice,
    List,
    Map,
    Chars,
}

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
            let (elem, kind) = match &array.ty {
                Ty::Array(t, _) => ((**t).clone(), ForKind::Array),
                Ty::Slice(t) => ((**t).clone(), ForKind::Slice),
                Ty::List(t) => ((**t).clone(), ForKind::List),
                Ty::Map(k, v) => (Ty::Tuple(vec![(**k).clone(), (**v).clone()]), ForKind::Map),
                Ty::Error | Ty::Never => (Ty::Error, ForKind::Array),
                Ty::Chars => (Ty::Char, ForKind::Chars),
                other => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        iter.span,
                        format!("cannot iterate over a value of type `{other}`"),
                    );
                    (Ty::Error, ForKind::Array)
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
            let array = Box::new(array);
            let kind = match kind {
                ForKind::Array => tir::ExprKind::ForArray { pat, array, body },
                ForKind::Slice => tir::ExprKind::ForSlice { pat, slice: array, body },
                ForKind::List => tir::ExprKind::ForList { pat, list: array, body },
                ForKind::Map => tir::ExprKind::ForMap { pat, map: array, body },
                ForKind::Chars => tir::ExprKind::ForChars { pat, string: array, body },
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
    /// `expr?`: unwraps an `Option` or a `Result`, returning early through the enclosing function.
    pub(super) fn try_expr(&mut self, operand: &ast::Expr, span: Span) -> tir::Expr {
        let value = self.expr(operand, None);
        let value_ty = value.ty.clone();
        let ret = self.ret.clone();
        match (&ret, &value_ty) {
            (Ty::Error, _) | (_, Ty::Error) => error_expr(span),
            (Ty::Option(ret_inner), Ty::Option(inner)) => {
                if !compatible(inner, ret_inner) {
                    self.mismatch(span, ret_inner, inner);
                }
                let inner = (**inner).clone();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", inner.clone(), owned);
                let some = Pat::Some(Box::new(Pat::Binding(id, mode)));
                let body = mk(tir::ExprKind::Local(id), inner.clone(), span);
                let none = mk(tir::ExprKind::None, ret.clone(), span);
                let early = mk(tir::ExprKind::Return(Some(Box::new(none))), Ty::Never, span);
                let arms = vec![
                    tir::Arm { pat: some, guard: None, body },
                    tir::Arm { pat: Pat::None, guard: None, body: early },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), inner, span)
            }
            (Ty::Result(ret_ok, ret_err), Ty::Result(ok, err)) => {
                if !compatible(ok, ret_ok) {
                    self.mismatch(span, ret_ok, ok);
                }
                let ok = (**ok).clone();
                let err = (**err).clone();
                let ret_err = (**ret_err).clone();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", ok.clone(), owned);
                let (eid, emode) = self.payload_local("err", err.clone(), owned);
                let body = mk(tir::ExprKind::Local(id), ok.clone(), span);
                let converted = if err == ret_err {
                    mk(tir::ExprKind::Local(eid), err.clone(), span)
                } else if let Some(f) = self.globals.conversion(&err, &ret_err) {
                    mk(
                        tir::ExprKind::Call(f, vec![mk(tir::ExprKind::Local(eid), err.clone(), span)]),
                        ret_err.clone(),
                        span,
                    )
                } else {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("the error type `{err}` does not convert into `{ret_err}`; add a `{ret_err}: From<{err}>` implementation"),
                    );
                    return error_expr(span);
                };
                let err_value = mk(tir::ExprKind::Err(Box::new(converted)), ret.clone(), span);
                let early = mk(tir::ExprKind::Return(Some(Box::new(err_value))), Ty::Never, span);
                let arms = vec![
                    tir::Arm { pat: Pat::Ok(Box::new(Pat::Binding(id, mode))), guard: None, body },
                    tir::Arm { pat: Pat::Err(Box::new(Pat::Binding(eid, emode))), guard: None, body: early },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), ok, span)
            }
            (Ty::Option(_) | Ty::Result(..), _) => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`?` unwraps an `Option` or a `Result`, but this value is `{value_ty}`"),
                );
                error_expr(span)
            }
            _ => {
                self.diags.error(
                    Code::QUESTION_OUTSIDE,
                    span,
                    format!("`?` is only allowed in a function that returns `Option` or `Result`, but this one returns `{ret}`"),
                );
                error_expr(span)
            }
        }
    }

    /// `value orelse default`: the payload of an `Option` or a `Result`, or `default` when it is empty.
    pub(super) fn orelse(&mut self, value: &ast::Expr, default: &ast::Expr, span: Span) -> tir::Expr {
        let value = self.expr(value, None);
        match value.ty.clone() {
            Ty::Option(inner) => {
                let inner = (*inner).clone();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("some", inner.clone(), owned);
                let other = self.expr(default, Some(&inner));
                let other = self.expect_ty(other, &inner);
                let body = mk(tir::ExprKind::Local(id), inner.clone(), span);
                let arms = vec![
                    tir::Arm { pat: Pat::Some(Box::new(Pat::Binding(id, mode))), guard: None, body },
                    tir::Arm { pat: Pat::None, guard: None, body: other },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), inner, span)
            }
            Ty::Result(ok, err) => {
                let ok = (*ok).clone();
                let err = (*err).clone();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", ok.clone(), owned);
                // The error is bound so that it is dropped when the arm ends.
                let (eid, emode) = self.payload_local("err", err, owned);
                let other = self.expr(default, Some(&ok));
                let other = self.expect_ty(other, &ok);
                let body = mk(tir::ExprKind::Local(id), ok.clone(), span);
                let arms = vec![
                    tir::Arm { pat: Pat::Ok(Box::new(Pat::Binding(id, mode))), guard: None, body },
                    tir::Arm { pat: Pat::Err(Box::new(Pat::Binding(eid, emode))), guard: None, body: other },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), ok, span)
            }
            Ty::Error => {
                self.expr(default, None);
                error_expr(span)
            }
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`orelse` needs an `Option` or a `Result`, but this value is `{other}`"),
                );
                self.expr(default, None);
                error_expr(span)
            }
        }
    }

    /// A local a synthesized `match` binds. It is not in any user scope, so the name cannot be observed.
    fn payload_local(&mut self, name: &str, ty: Ty, owned: bool) -> (LocalId, tir::BindMode) {
        let mode = self.bind_mode(&ty, owned);
        let kind = if mode == tir::BindMode::View { LocalKind::View } else { LocalKind::Owned };
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable: false, by_ref: false, kind });
        (id, mode)
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
