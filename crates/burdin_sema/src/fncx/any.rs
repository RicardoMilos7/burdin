//! `any Interface`: method calls through the vtable, and the compiler-known `is`/`downcast` methods
//! (spec/existentials.md, AN5/AN6).

use super::{FnCx, error_expr, mk};
use crate::generics::Bound;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::is_place_expr;
use burdin_tir::{self as tir, AnyRef, Ty};

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
        let target = self.resolve(&type_args[0]);
        if target == Ty::Error {
            return error_expr(span);
        }
        if target.is_second_class() {
            self.diags.error(
                Code::SECOND_CLASS_POSITION,
                type_args[0].span,
                format!("`{target}` is a view of other storage, so it cannot be a downcast target"),
            );
            return error_expr(span);
        }
        if !self.globals.satisfies(&target, &Bound::User(any.iface, any.args.clone()), &self.bounds) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                type_args[0].span,
                format!("`{target}` does not implement `{}`, so it can never be inside `any {}`", any.name, any.name),
            );
            return error_expr(span);
        }
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
