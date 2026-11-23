//! Conversions between scalar types: the built-in `From` pairs (spec/stdlib.md, "Numeric conversions: `From`, `into`,
//! and `as`", FR1-FR5). `as` on a pair, `T.from(v)`, and `v.into()` all lower through `primitive_from_value`, so the
//! three cannot disagree. The blocks a program writes (`Target: From<Source>`) are found through the same conversion
//! registry as `TryFrom` blocks (`globals.conversions_into`).

use super::{FnCx, error_expr, mk};
use crate::generics::SYMBOLIC_CALL;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FloatTy, FuncId, IntTy, Ty};

/// Whether every value of `from` converts to `to` exactly, so that `as` from `from` to `to` gives back the same
/// value (FR1). `isize` and `usize` are 64 bits (`POINTER_BITS`).
pub(crate) fn lossless_from(to: &Ty, from: &Ty) -> bool {
    match (from, to) {
        (Ty::Int(f), Ty::Int(t)) => t.min() <= f.min() && f.max() <= t.max(),
        // Every integer whose magnitude is at most 2^p is exact in a float with a p-bit significand.
        (Ty::Int(f), Ty::Float(t)) => {
            let limit = 1i128 << significand_bits(*t);
            f.min().abs().max(f.max()) <= limit
        }
        (Ty::Float(FloatTy::F32), Ty::Float(FloatTy::F64)) => true,
        (Ty::Bool, Ty::Int(_)) => true,
        (Ty::Char, Ty::Int(t)) => t.min() <= 0 && LAST_SCALAR <= t.max(),
        (Ty::Int(IntTy::U8), Ty::Char) => true,
        _ => from == to && matches!(from, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char),
    }
}

/// The largest Unicode scalar value, as a `char` holds it.
const LAST_SCALAR: i128 = 0x10_FFFF;

/// Whether `e` is an integer literal, with or without the minus sign before it (FR2). An integer literal has no
/// suffix, so its type is the one its context gives it.
fn is_int_literal(e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::Int(_) => true,
        ast::ExprKind::Unary(ast::UnOp::Neg, operand) => matches!(operand.kind, ast::ExprKind::Int(_)),
        _ => false,
    }
}

/// The significand precision of a float type, counting the implicit bit.
fn significand_bits(t: FloatTy) -> u32 {
    match t {
        FloatTy::F32 => 24,
        FloatTy::F64 => 53,
    }
}

impl FnCx<'_> {
    /// The built-in conversion of `value` into `to`, for a pair `lossless_from` accepts (FR3). It is the cast: `as`,
    /// `T.from(v)`, and `v.into()` all produce this expression.
    pub(super) fn primitive_from_value(&mut self, to: Ty, value: tir::Expr, span: Span) -> tir::Expr {
        if value.ty == to {
            return value;
        }
        mk(tir::ExprKind::Cast(Box::new(value)), to, span)
    }

    /// `T.from(v)` for a scalar `T` (FR2). An integer literal takes an integer `T` as its type, as it takes its expected
    /// type everywhere else; any other argument is checked without an expected type, so it keeps its own type.
    pub(super) fn primitive_from_call(&mut self, to: &Ty, args: &[ast::Arg], span: Span) -> tir::Expr {
        if args.len() != 1 {
            let message = format!("`{to}.from` takes 1 argument, but {} were given", args.len());
            self.diags.error(Code::ARG_COUNT, span, message);
            return self.args_then_error(args, span);
        }
        let expect = (matches!(to, Ty::Int(_)) && is_int_literal(&args[0].expr)).then_some(to);
        let value = self.expr(&args[0].expr, expect);
        if value.ty == Ty::Error {
            return error_expr(span);
        }
        if !lossless_from(to, &value.ty) {
            let message = format!("`{to}` has no conversion from `{}`", value.ty);
            self.diags.error(Code::TYPE_MISMATCH, value.span, message);
            return error_expr(span);
        }
        self.primitive_from_value(to.clone(), value, span)
    }

    /// `v.into()`: the conversion of `v` into the type the context expects (FR4, FR5). The target is a built-in pair
    /// or a `From` block of the target type; any other pair is E0003.
    pub(super) fn convert_into(&mut self, recv: tir::Expr, expect: Option<&Ty>, span: Span) -> tir::Expr {
        if recv.ty == Ty::Error {
            return error_expr(span);
        }
        let Some(target) = expect.filter(|t| **t != Ty::Error).cloned() else {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                "cannot infer the target of `into`; give the type it converts to",
            );
            return error_expr(span);
        };
        if lossless_from(&target, &recv.ty) {
            return self.primitive_from_value(target, recv, span);
        }
        // A type parameter with a `From<S>` bound converts from `S` (FR7). Its body is checked against the bound
        // here; the instances that replace the parameter convert for real.
        if matches!(target, Ty::Param(_)) && self.globals.converts_into(&target, &recv.ty, &self.bounds) {
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![recv]), target, span);
        }
        let candidates: Vec<(Ty, FuncId)> =
            self.globals.conversions_into(&target, "from").iter().map(|c| (c.source.clone(), c.func)).collect();
        // A `String` is borrowed as a `str` where the block takes a `str` (IC1), as for `T.from(s)`.
        let exact = candidates.iter().find(|(source, _)| *source == recv.ty).cloned();
        let borrowed = candidates.iter().find(|(source, _)| *source == Ty::Str).cloned();
        let found = match (exact, borrowed) {
            (Some(hit), _) => Some(hit),
            (None, Some(hit)) if recv.ty == Ty::String => Some(hit),
            _ => None,
        };
        let Some((source, func)) = found else {
            let message = if candidates.is_empty() {
                format!("`{target}` has no conversion from `{}`", recv.ty)
            } else {
                let sources: Vec<String> = candidates.iter().map(|(source, _)| format!("`{source}`")).collect();
                format!("`{target}` converts from {}, not from `{}`", sources.join(" or "), recv.ty)
            };
            self.diags.error(Code::TYPE_MISMATCH, span, message);
            return error_expr(span);
        };
        let sig = self.globals.sigs[func.0].clone();
        // `into` cannot write the `mut` a `mut` parameter asks for, so such a block is called as `T.from(mut v)`.
        if sig.params.first().is_some_and(|p| p.mut_param) {
            let message =
                format!("`into` cannot pass the `mut` parameter of `{target}.from`; call `{target}.from(mut v)`");
            self.diags.error(Code::MISSING_MUT_MARKER, span, message);
            return error_expr(span);
        }
        let value = self.coerce_arg(recv, &source);
        mk(tir::ExprKind::Call(func, vec![value]), sig.ret, span)
    }
}

#[cfg(test)]
mod tests {
    use super::lossless_from;
    use burdin_tir::{FloatTy, IntTy, Ty};

    /// `(to, from, lossless)`: whether `as` from `from` to `to` gives back every value (FR1).
    #[test]
    fn built_in_pairs_are_the_exact_ones() {
        let int = Ty::Int;
        let float = Ty::Float;
        let cases = [
            (int(IntTy::I64), int(IntTy::I32), true),
            (int(IntTy::I32), int(IntTy::I64), false),
            (int(IntTy::Usize), int(IntTy::U32), true),
            (int(IntTy::Isize), int(IntTy::U32), true),
            (int(IntTy::U64), int(IntTy::I32), false),
            (float(FloatTy::F64), int(IntTy::I32), true),
            (float(FloatTy::F64), int(IntTy::I64), false),
            (float(FloatTy::F32), int(IntTy::I16), true),
            (float(FloatTy::F32), int(IntTy::I32), false),
            (float(FloatTy::F64), float(FloatTy::F32), true),
            (float(FloatTy::F32), float(FloatTy::F64), false),
            (int(IntTy::I32), Ty::Bool, true),
            (Ty::Char, int(IntTy::U8), true),
            (Ty::Char, int(IntTy::U32), false),
            (int(IntTy::I32), Ty::Char, true),
            (int(IntTy::U8), Ty::Char, false),
            (int(IntTy::I16), Ty::Char, false),
            (int(IntTy::U32), Ty::Char, true),
            (Ty::Char, Ty::Char, true),
            (Ty::Bool, Ty::Char, false),
            (float(FloatTy::F64), float(FloatTy::F64), true),
            (int(IntTy::U8), int(IntTy::U8), true),
        ];
        for (to, from, expected) in cases {
            assert_eq!(lossless_from(&to, &from), expected, "{from} to {to}");
        }
    }
}
