//! `any Interface`: method calls through the vtable, and the compiler-known `is`/`downcast` methods
//! (spec/existentials.md, AN5/AN6).

use super::{FnCx, error_expr, mk};
use crate::generics::Bound;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::is_place_expr;
use burdin_tir::{self as tir, AnyRef, LocalKind, Pat, Ty};
use std::collections::HashMap;

impl FnCx<'_> {
    /// `recv.name(args)` where `recv` has type `any I`: the method's slot in the vtable, with the arguments
    /// checked against the interface's conventions (AN5).
    pub(super) fn any_method_call(
        &mut self,
        recv: tir::Expr,
        any: &AnyRef,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        let methods = self.globals.any_methods(any.id);
        if (name.name == "is" || name.name == "downcast") && !methods.iter().any(|m| m.name == name.name) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                name.span,
                format!("`{}` needs a type argument: `x.{}(T)`", name.name, name.name),
            );
            return self.args_then_error(args, span);
        }
        let Some(slot) = methods.iter().position(|m| m.name == name.name) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on `any {}`", name.name, any.name));
            return self.args_then_error(args, span);
        };
        let method = &methods[slot];
        if method.is_unsafe && self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                name.span,
                format!("calling the unsafe method `{}` requires an `unsafe` block", name.name),
            );
        }
        let sig = method.sig.clone();
        let receiver = sig.params[0].clone();
        // Only `mut self` borrows the box exclusively; object safety rejects `own self`, and read-only `self`
        // only reads it.
        if receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = burdin_tir::place::place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
            );
        }
        let params = sig.params[1..].to_vec();
        let Some(checked) = self.args(&name.name, &params, args, Some((&recv, receiver.exclusive)), span) else {
            return error_expr(span);
        };
        mk(tir::ExprKind::AnyCall { recv: Box::new(recv), iface: any.id, slot, params, args: checked }, sig.ret, span)
    }

    /// The type `T` of `is<T>`, `downcast<T>`, or a type pattern on an `any` value: it must be first-class and implement
    /// the interface, so the box can hold it (AN6, AN15). Reports the problem and returns `None` otherwise.
    fn any_target(&mut self, any: &AnyRef, ty: &ast::Type) -> Option<Ty> {
        let target = self.resolve(ty);
        if target == Ty::Error {
            return None;
        }
        if target.is_second_class() {
            self.diags.error(
                Code::SECOND_CLASS_POSITION,
                ty.span,
                format!("`{target}` is a view of other storage, so it cannot be a downcast target"),
            );
            return None;
        }
        if !self.globals.satisfies(&target, &Bound::User(any.iface, any.args.clone()), &self.bounds) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                ty.span,
                format!("`{target}` does not implement `{}`, so it can never be inside `any {}`", any.name, any.name),
            );
            return None;
        }
        Some(target)
    }

    /// The `any I` a `match` with type patterns tests: the scrutinee itself, or the box inside an `AnyError`.
    fn matched_any(&self, ty: &Ty) -> Option<AnyRef> {
        if let Ty::Any(any) = ty {
            return Some(any.clone());
        }
        if self.globals.is_error_ty(ty)
            && let Ty::Struct(a) = ty
            && let Some(field) = self.globals.structs[a.id].fields.iter().find(|f| f.name == "value")
            && let Ty::Any(any) = &field.ty
        {
            return Some(any.clone());
        }
        None
    }

    /// `match scrutinee { name: T => ..., other => ... }` on an `any I` or an `AnyError` (AN15). The scrutinee moves
    /// into a hidden local, and the arms become the `is<T>`/`downcast<T>` chain they stand for: a type arm tests with
    /// `is<T>` and takes the value with `downcast<T>`, so only the arm taken consumes the box. The catch-all is the
    /// final `else`.
    pub(super) fn any_match(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        use ast::PatternKind as P;
        let value = self.expr(scrutinee, None);
        let Some(any) = self.matched_any(&value.ty) else {
            if value.ty != Ty::Error {
                let msg = format!("a type pattern needs an `any` value to match, found `{}`", value.ty);
                self.diags.error(Code::TYPE_MISMATCH, scrutinee.span, msg);
            }
            return error_expr(span);
        };
        let mut valid = true;
        let mut catch_all = false;
        for arm in arms {
            let kind = arm.pattern.as_ref().map(|p| &p.kind);
            if catch_all {
                self.diags.error(Code::TYPE_MISMATCH, arm.span, "no arm may follow the catch-all arm of this `match`");
                valid = false;
            } else if let Some(P::Typed { ty, .. }) = kind {
                valid &= self.any_target(&any, ty).is_some();
                if arm.guard.is_some() {
                    let msg = "a type pattern takes no guard: the value leaves the box when its type matches";
                    self.diags.error(Code::TYPE_MISMATCH, arm.span, msg);
                    valid = false;
                }
            } else if matches!(kind, None | Some(P::Wild) | Some(P::Binding { .. })) {
                catch_all = true;
                if arm.guard.is_some() {
                    self.diags.error(Code::TYPE_MISMATCH, arm.span, "the catch-all arm of this `match` takes no guard");
                    valid = false;
                }
            } else {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    arm.span,
                    format!("this `match` is on an `any {}`: an arm is a type pattern or a final catch-all", any.name),
                );
                valid = false;
            }
        }
        if !catch_all && valid {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                span,
                format!("this `match` does not cover every value of `any {}`: add a catch-all arm", any.name),
            );
            valid = false;
        }
        if !valid {
            return error_expr(span);
        }
        self.scopes.push(HashMap::new());
        let id = self.declare("__any", value.ty.clone(), false, LocalKind::Scrutinee, scrutinee.span);
        let chain = self.any_chain(arms, expect);
        self.scopes.pop();
        let ty = chain.ty.clone();
        let stmt = tir::Stmt::Let(Pat::Binding(id, tir::BindMode::Move), value, None);
        let block = tir::Block { stmts: vec![stmt], tail: Some(Box::new(chain)), close: span };
        mk(tir::ExprKind::Block(block), ty, span)
    }

    /// Checks the `if __any.is<T>() { let name = __any.downcast<T>().unwrap() ... } else ...` chain of a validated
    /// `match` on an `any` value.
    fn any_chain(&mut self, arms: &[ast::MatchArm], expect: Option<&Ty>) -> tir::Expr {
        use ast::{ExprKind as E, PatternKind as P};
        let ident = |name: &str, span: Span| ast::Ident { name: name.to_string(), span };
        let expr = |kind: E, span: Span| ast::Expr { kind, span };
        // `__any.member<T>()`
        let member = |name: &str, ty: &ast::Type, span: Span| {
            let hidden = expr(E::Name { name: ident("__any", span), args: Vec::new() }, span);
            let field = expr(E::Field(Box::new(hidden), ident(name, span)), span);
            let typed = expr(E::TypeArgs(Box::new(field), vec![ty.clone()]), span);
            expr(E::Call(Box::new(typed), Vec::new()), span)
        };
        let let_stmt = |name: ast::Ident, mutable: bool, init: ast::Expr, span: Span| ast::Stmt::Let {
            pattern: ast::Pattern { kind: P::Binding { name, mutable }, span },
            ty: None,
            init,
            else_block: None,
            span,
        };
        let mut rest: Option<ast::Expr> = None;
        for arm in arms.iter().rev() {
            let span = arm.span;
            let body = arm.body.clone();
            let block = |stmts: Vec<ast::Stmt>| ast::Block { stmts, span };
            let next = match arm.pattern.as_ref().map(|p| &p.kind) {
                Some(P::Typed { name, ty }) => {
                    let taken = expr(E::Field(Box::new(member("downcast", ty, span)), ident("unwrap", span)), span);
                    let unwrap = expr(E::Call(Box::new(taken), Vec::new()), span);
                    let name = name.clone().unwrap_or_else(|| ident("__dropped", span));
                    let then_block = block(vec![let_stmt(name, false, unwrap, span), ast::Stmt::Expr(body)]);
                    let else_expr = rest.take().map(Box::new);
                    expr(E::If { cond: Box::new(member("is", ty, span)), then_block, else_expr }, span)
                }
                Some(P::Binding { name, mutable }) => {
                    let hidden = expr(E::Name { name: ident("__any", span), args: Vec::new() }, span);
                    expr(
                        E::Block(block(vec![let_stmt(name.clone(), *mutable, hidden, span), ast::Stmt::Expr(body)])),
                        span,
                    )
                }
                _ => expr(E::Block(block(vec![ast::Stmt::Expr(body)])), span),
            };
            rest = Some(next);
        }
        let chain = rest.expect("an `any` match has an arm");
        self.expr(&chain, expect)
    }

    /// `recv.name<T>(args)`: the compiler-known `is` and `downcast` methods of an `any` value (AN6). Any other
    /// member type arguments are E0002.
    pub(super) fn member_type_args_call(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        type_args: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        self.reject_named_args(&name.name, args);
        let recv = self.auto_deref(recv);
        // The dynamic `AnyError` is a struct wrapping `any Error`: `is` and `downcast` inspect the box
        // (spec/errors.md, ERR4), so they delegate to the field.
        if self.globals.is_error_ty(&recv.ty)
            && (name.name == "is" || name.name == "downcast")
            && let Ty::Struct(a) = &recv.ty
            && let Some((index, field)) =
                self.globals.structs[a.id].fields.iter().enumerate().find(|(_, f)| f.name == "value")
        {
            let field_ty = field.ty.clone();
            let value = mk(tir::ExprKind::Field(Box::new(recv), index), field_ty, name.span);
            return self.member_type_args_call(value, name, type_args, args, span);
        }
        let Ty::Any(any) = recv.ty.clone() else {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("`{}` takes type arguments only on an `any` value", name.name),
            );
            return self.args_then_error(args, span);
        };
        if name.name != "is" && name.name != "downcast" {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("no method `{}` takes type arguments on `any {}`", name.name, any.name),
            );
            return self.args_then_error(args, span);
        }
        if !args.is_empty() {
            self.diags.error(Code::ARG_COUNT, span, format!("`{}` takes no value arguments", name.name));
            return self.args_then_error(args, span);
        }
        if type_args.len() != 1 {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes one type argument, but {} were given", name.name, type_args.len()),
            );
            return error_expr(span);
        }
        let Some(target) = self.any_target(&any, &type_args[0]) else {
            return error_expr(span);
        };
        // A symbolic body has no code, so it has no runtime identity.
        let type_id = if self.symbolic || self.globals.ty_has_param(&target) {
            usize::MAX
        } else {
            self.globals.any_type_id(&target)
        };
        if name.name == "is" {
            return mk(tir::ExprKind::AnyIs { value: Box::new(recv), target, type_id }, Ty::Bool, span);
        }
        let ty = Ty::Option(Box::new(target.clone()));
        mk(tir::ExprKind::AnyDowncast { value: Box::new(recv), target, type_id }, ty, span)
    }
}
