//! Target-typed initializers: `.name`, `.name(args)`, and `.{ fields }` take their type from the expected type of
//! their position (`spec/target-typing.md`). Each one is checked as the spelling the expected type already has:
//! `.name(args)` as `T.name(args)`, `.name` as `T.name`, and `.{ fields }` as `T { fields }`.

use super::{FnCx, error_expr, mk};
use crate::globals::Globals;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};

impl FnCx<'_> {
    /// The type a target-typed form takes from its position, or `None` after the error for a position that gives
    /// it none or one it cannot use (E0003, TT3-TT5). An error type is `None` without a second error.
    fn target_ty(&mut self, expect: Option<&Ty>, form: &str, span: Span) -> Option<Ty> {
        let Some(ty) = expect else {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{form}` needs an expected type: write the type, as in `List<String>.new()`, or give the place a type"),
            );
            return None;
        };
        match ty {
            Ty::Error => None,
            Ty::Param(_) | Ty::Projection { .. } => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`{form}` needs a concrete type, but `{ty}` is a type parameter; write the type instead"),
                );
                None
            }
            Ty::Ref { .. }
            | Ty::Str
            | Ty::Slice(_)
            | Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Any(_)
            | Ty::Fn { .. }
            | Ty::Unit
            | Ty::Never => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`{form}` needs a named type, but the expected type `{ty}` has no members; write the type"),
                );
                None
            }
            _ => Some(ty.clone()),
        }
    }

    /// `.name(args)`: the associated function, variant constructor, or constructor the expected type has, called
    /// with `args` (TT1, TT6). `.Some(x)`, `.Ok(v)`, and `.Err(e)` are the prelude constructors.
    pub(super) fn target_call(
        &mut self,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let form = format!(".{}", name.name);
        let Some(ty) = self.target_ty(expect, &form, span) else { return self.args_then_error(args, span) };
        match (&ty, name.name.as_str()) {
            (Ty::Heap(..), "new") => return self.heap_new(args, expect, span),
            (Ty::Heap(..), "new_in") => return self.heap_new_in(args, span),
            _ => {}
        }
        self.assoc_call(&ty, name, &[], args, span, expect)
    }

    /// `.name` without a call: a unit variant, `None`, or a method value of the expected type (TT1, TT6).
    pub(super) fn target_value(&mut self, name: &ast::Ident, span: Span, expect: Option<&Ty>) -> tir::Expr {
        let form = format!(".{}", name.name);
        let Some(ty) = self.target_ty(expect, &form, span) else { return error_expr(span) };
        if self.globals.option_payload(&ty).is_some() && name.name == "None" {
            return Globals::mk_none(ty.clone(), span);
        }
        if (self.globals.option_payload(&ty).is_some() && name.name == "Some")
            || (self.globals.result_parts(&ty).is_some() && matches!(name.name.as_str(), "Ok" | "Err"))
        {
            self.diags.error(
                Code::TYPE_MISMATCH,
                name.span,
                format!("`{}` takes a value; write `{form}(value)`", name.name),
            );
            return error_expr(span);
        }
        let value = self.unit_variant(ty.clone(), name, span);
        // A name the expected type does not have keeps that type, so the editor can complete the name at the
        // dot (`spec/lsp.md`, LS12). The error is already reported; no other check reads this node's kind.
        if matches!(value.ty, Ty::Error) { mk(tir::ExprKind::Unit, ty.clone(), span) } else { value }
    }

    /// `.Variant { field: value, ... }`: the struct-like variant of the expected enum, checked like
    /// `Enum.Variant { ... }` (TT13). `path` is the inferred path, `.` and the variant's name.
    pub(super) fn inferred_variant_literal(
        &mut self,
        path: &[ast::Ident],
        text: &str,
        fields: &[ast::FieldInit],
        base: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let variant = path.get(1).cloned();
        let target = match self.target_ty(expect, text, span) {
            Some(Ty::Enum(a)) => self.literal_target(Ty::Enum(a), variant, span, text),
            Some(other) => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!(
                        "`{text} {{ ... }}` builds an enum variant, but the expected type `{other}` is not an enum"
                    ),
                );
                None
            }
            None => None,
        };
        self.struct_fields(target, text, fields, base, span)
    }

    /// `.{ field: value, ... }`: a struct literal of the expected type, checked like `Type { ... }` (TT7).
    pub(super) fn target_struct(
        &mut self,
        fields: &[ast::FieldInit],
        base: Option<&ast::Expr>,
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let Some(ty) = self.target_ty(expect, ".{ ... }", span) else {
            self.check_field_values(fields);
            if let Some(base) = base {
                self.expr(base, None);
            }
            return error_expr(span);
        };
        match ty {
            Ty::Struct(a) => {
                let owner = self.globals.struct_pkg[a.id];
                let text = Ty::Struct(a.clone()).to_string();
                self.struct_fields(Some((Ty::Struct(a), None, owner)), &text, fields, base, span)
            }
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`.{{ ... }}` builds a struct, but the expected type `{other}` is not a struct"),
                );
                self.check_field_values(fields);
                if let Some(base) = base {
                    self.expr(base, None);
                }
                error_expr(span)
            }
        }
    }

    /// Checks the values of a struct literal that cannot be built, so their own errors are still reported.
    fn check_field_values(&mut self, fields: &[ast::FieldInit]) {
        for f in fields {
            match &f.value {
                Some(value) => {
                    self.expr(value, None);
                }
                None => {
                    let shorthand = ast::Expr {
                        kind: ast::ExprKind::Name { name: f.name.clone(), args: Vec::new() },
                        span: f.name.span,
                    };
                    self.expr(&shorthand, None);
                }
            }
        }
    }
}
