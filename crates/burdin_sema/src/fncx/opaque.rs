//! Values of a function's `impl Interface` return type (spec/existentials.md, AN4; `crate::opaque`).
//!
//! The body returns the concrete value, which is wrapped into the return type's struct. A caller holds the wrapper:
//! it calls the interface's methods, which go to the concrete value, it boxes the value as `any` of the interface,
//! and it passes it where the interface is required. Nothing else is visible through it.

use super::{FnCx, error_expr, mk};
use crate::generics::Bound;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AnyRef, Ty};

impl FnCx<'_> {
    /// Wraps `e` when it is the concrete value of the `impl Interface` type `want`. A value of another type is
    /// returned unchanged, so the caller reports the ordinary mismatch.
    pub(super) fn coerce_opaque(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        let Some(concrete) = self.globals.opaque_of(want).and_then(|o| o.concrete.clone()) else { return e };
        if matches!(e.ty, Ty::Error | Ty::Never) || e.ty == *want || e.ty != concrete {
            return e;
        }
        let span = e.span;
        mk(tir::ExprKind::Struct(vec![(0, e)]), want.clone(), span)
    }

    /// A method called on an `impl Interface` value: only a method of the interface (or one it requires) is found,
    /// and the call goes to the concrete value's method.
    pub(super) fn opaque_method_call(
        &mut self,
        recv: tir::Expr,
        explicit: &[ast::Type],
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let (bound, concrete) = match self.globals.opaque_of(&recv.ty) {
            Some(o) => (o.bound.clone(), o.concrete.clone()),
            None => unreachable!("the caller checked for an opaque receiver"),
        };
        let declared = self.globals.bound_closure(&bound).iter().any(|b| {
            matches!(b, Bound::User(i, _) if self.globals.interfaces[*i].methods.iter().any(|m| m.name == name.name))
        });
        if !declared {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on `{}`", name.name, recv.ty));
            return self.args_then_error(args, span);
        }
        let Some(concrete) = concrete else { return error_expr(span) };
        let field = mk(tir::ExprKind::Field(Box::new(recv), 0), concrete, span);
        self.method_call_on_with(field, name, explicit, args, span, expect)
    }

    /// Unwraps an `impl Interface` value for a `for` loop over it, which runs the iteration protocol on the concrete
    /// value. The wrapper's interface must be an `Iterator` or an `Iterable` (spec/iterators.md) for this to apply;
    /// any other value is returned unchanged and the loop reports it.
    pub(super) fn open_opaque_for_loop(&mut self, e: tir::Expr) -> tir::Expr {
        let Some(concrete) = self.globals.opaque_of(&e.ty).and_then(|o| o.concrete.clone()) else { return e };
        if self.globals.iterator_item(&e.ty, &self.bounds).is_none()
            && self.globals.iterable_item(&e.ty, &self.bounds).is_none()
        {
            return e;
        }
        let span = e.span;
        mk(tir::ExprKind::Field(Box::new(e), 0), concrete, span)
    }

    /// Unwraps an `impl Interface` value that is boxed as `any J` (AN3). The wrapper must satisfy `J` (AN4): an
    /// interface the return type does not name is not visible, even when the concrete type implements it. The
    /// boxed value is the concrete one. `Err` is the value to return as the boxed result, after the error (boxed, so
    /// the `Ok` path stays small).
    pub(super) fn open_opaque_for_any(&mut self, e: tir::Expr, any: &AnyRef) -> Result<tir::Expr, Box<tir::Expr>> {
        let span = e.span;
        let bound = Bound::User(any.iface, any.args.clone());
        if !self.globals.satisfies(&e.ty, &bound, &self.bounds) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`{}` does not implement `{}`, so it cannot be boxed as `any {}`", e.ty, any.name, any.name),
            );
            return Err(Box::new(tir::Expr { ty: Ty::Any(any.clone()), ..e }));
        }
        let Some(concrete) = self.globals.opaque_of(&e.ty).and_then(|o| o.concrete.clone()) else {
            return Err(Box::new(tir::Expr { ty: Ty::Any(any.clone()), ..e }));
        };
        Ok(mk(tir::ExprKind::Field(Box::new(e), 0), concrete, span))
    }
}
