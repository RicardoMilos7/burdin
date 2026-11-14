//! Literals and names.

use super::{FnCx, error_expr, mk};
use crate::builtins::builtin;
use crate::globals::Global;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FloatTy, IntTy, Ty};

impl FnCx<'_> {
    pub(super) fn int_literal(&mut self, v: u128, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let ity = match expect {
            Some(Ty::Int(i)) => *i,
            _ => IntTy::I32,
        };
        match self.int_value(v, negative, ity, span) {
            Some(value) => mk(tir::ExprKind::Int(value), Ty::Int(ity), span),
            None => error_expr(span),
        }
    }

    /// The value of an integer literal of type `ity`, or E0008 if it does not fit.
    pub(super) fn int_value(&mut self, v: u128, negative: bool, ity: IntTy, span: Span) -> Option<i128> {
        let value = i128::try_from(v).ok().map(|v| if negative { -v } else { v });
        match value {
            Some(value) if value >= ity.min() && value <= ity.max() => Some(value),
            _ => {
                let sign = if negative { "-" } else { "" };
                self.diags.error(
                    Code::LITERAL_RANGE,
                    span,
                    format!("literal `{sign}{v}` is out of range for `{}`", ity.name()),
                );
                None
            }
        }
    }

    pub(super) fn float_literal(&mut self, text: &str, negative: bool, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let fty = match expect {
            Some(Ty::Float(f)) => *f,
            _ => FloatTy::F64,
        };
        let Ok(v) = text.parse::<f64>() else {
            self.diags.error(Code::SYNTAX, span, format!("invalid float literal `{text}`"));
            return error_expr(span);
        };
        let v = if negative { -v } else { v };
        if !v.is_finite() || (fty == FloatTy::F32 && !(v as f32).is_finite()) {
            self.diags.error(Code::LITERAL_RANGE, span, format!("literal `{text}` is out of range for its type"));
            return error_expr(span);
        }
        mk(tir::ExprKind::Float(v), Ty::Float(fty), span)
    }

    pub(super) fn name(&mut self, id: &ast::Ident, expect: Option<&Ty>) -> tir::Expr {
        if let Some(local) = self.lookup(&id.name) {
            return mk(tir::ExprKind::Local(local), self.locals[local.0].ty.clone(), id.span);
        }
        if id.name == "None" && self.is_prelude("None") {
            return match expect {
                Some(t @ Ty::Option(_)) => mk(tir::ExprKind::None, t.clone(), id.span),
                _ => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        id.span,
                        "cannot tell the type of this `None`; annotate the binding with an `Option` type",
                    );
                    error_expr(id.span)
                }
            };
        }
        match self.globals.names.get(&id.name) {
            Some(Global::Const(c)) => mk(tir::ExprKind::Const(*c), self.globals.const_tys[c.0].clone(), id.span),
            Some(Global::Func(_) | Global::Generic(_)) => self.unsupported(id.span, "using a function as a value"),
            None if builtin(&id.name).is_some() => self.unsupported(id.span, "using a function as a value"),
            None if id.name == "Some" => self.unsupported(id.span, "using `Some` as a function value"),
            None if self.globals.types.contains_key(&id.name) => {
                self.diags.error(Code::TYPE_MISMATCH, id.span, format!("`{}` is a type, not a value", id.name));
                error_expr(id.span)
            }
            None => {
                self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown name `{}`", id.name));
                error_expr(id.span)
            }
        }
    }
}
