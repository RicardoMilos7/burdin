//! `value.serialize(mut s)` and `T.deserialize(mut d)` on a built-in type (spec/serialization.md, SER14). The
//! standard library's helper for the type's shape does the work, as `builtin_format_method` does for formatting.

use super::FnCx;
use crate::globals::Globals;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};

impl FnCx<'_> {
    /// `value.serialize(mut s)` on a built-in type: the receiver is passed first to `__serialize_<shape>`, which
    /// writes it with the serializer of the same shape. `None` for any other receiver.
    pub(super) fn builtin_serialize_method(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        if name.name != "serialize" || args.len() != 1 {
            return None;
        }
        let iface = self.globals.serialize_iface?;
        if self.globals.builtin_serial(&recv.ty, iface, false, &self.bounds) != Some(true) {
            return None;
        }
        let helper = Globals::serialize_helper(&recv.ty)?;
        let Some(g) = self.globals.std_generic(&helper) else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("the standard library does not declare `{helper}`"));
            return Some(super::error_expr(span));
        };
        Some(self.generic_call_with_receiver(g, recv, "serialize", args, span))
    }

    /// `T.deserialize(mut d)` on a built-in type: the helper for its shape reads the value. The expected result is
    /// `Result<T, DeError>`, so the helper's type arguments come from `T` (`List<Leaf>.deserialize(mut d)`). `None`
    /// for any other type.
    pub(super) fn builtin_deserialize_call(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        if name.name != "deserialize" {
            return None;
        }
        let iface = self.globals.deserialize_iface?;
        if self.globals.builtin_serial(ty, iface, true, &self.bounds) != Some(true) {
            return None;
        }
        let helper = Globals::deserialize_helper(ty)?;
        let de_error = self.globals.de_error_ty.clone()?;
        let Some(g) = self.globals.std_generic(&helper) else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("the standard library does not declare `{helper}`"));
            return Some(super::error_expr(span));
        };
        let expect = Ty::Result(Box::new(ty.clone()), Box::new(de_error));
        Some(self.generic_call(g, name, args, &[], span, Some(&expect)))
    }
}
