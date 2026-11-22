//! The expression dispatcher.

use super::conversion::lossless_from;
use super::operators::can_cast;
use super::{FnCx, error_expr, mk};
use crate::types::compatible;
use burdin_diag::Code;
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};

impl FnCx<'_> {
    /// Checks an expression, then enforces that a raw C pointer is only used inside `unsafe` (memory-rules.md,
    /// US; issue #74). `CPtr` types in signatures and `extern` declarations are not values and are not affected.
    pub(super) fn expr(&mut self, e: &ast::Expr, expect: Option<&Ty>) -> tir::Expr {
        let checked = self.expr_inner(e, expect);
        // A value used where `any Interface` is expected is boxed (spec/existentials.md, AN3), and an error type
        // used where the dynamic `Error` is expected becomes one (spec/errors.md, ERR4). The dynamic error value
        // is a struct, not `Ty::Any`, so the two cases are separate.
        let checked = match expect {
            Some(want @ Ty::Any(_)) => self.coerce_any(checked, want),
            Some(want) if self.globals.is_error_ty(want) => self.coerce_error(checked, want),
            _ => checked,
        };
        // An `unsafe` block is checked with the depth raised; the block itself is the boundary and its result
        // may be a raw pointer.
        let boundary = matches!(e.kind, ast::ExprKind::UnsafeBlock(_));
        if !boundary && self.unsafe_depth == 0 && matches!(checked.ty, Ty::CPtr(_)) {
            self.diags.error(Code::UNSAFE_REQUIRED, checked.span, "using a raw C pointer requires an `unsafe` block");
        }
        checked
    }

    fn expr_inner(&mut self, e: &ast::Expr, expect: Option<&Ty>) -> tir::Expr {
        use ast::ExprKind as K;
        let span = e.span;
        match &e.kind {
            K::Int(v) => self.int_literal(*v, false, expect, span),
            K::Float(text) => self.float_literal(text, false, expect, span),
            K::Char(c) => mk(tir::ExprKind::Char(*c), Ty::Char, span),
            K::Str(s) => mk(tir::ExprKind::Str(s.clone()), Ty::Str, span),
            // An inline snapshot is only an argument of `assert_snapshot`, which reads it from the call itself.
            K::InlineSnapshot(_) => {
                self.diags.error(
                    Code::INVALID_SNAPSHOT,
                    span,
                    "an inline snapshot `@\"...\"` is only allowed as the second argument of `assert_snapshot`",
                );
                error_expr(span)
            }
            K::Interp(parts) => self.interp_expr(parts, span),
            K::Bool(b) => mk(tir::ExprKind::Bool(*b), Ty::Bool, span),
            K::Unit => mk(tir::ExprKind::Unit, Ty::Unit, span),
            K::Name { name, args } if args.is_empty() => self.name(name, expect),
            // A generic type path only means something as the left side of an associated call.
            K::Name { name, args: _ } => {
                self.diags.error(Code::TYPE_MISMATCH, span, format!("`{}` is a type, not a value", name.name));
                error_expr(span)
            }
            // `x.downcast<T>` is only meaningful when called; `call` handles the callee before reaching here.
            K::TypeArgs(base, _) => {
                self.expr(base, None);
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    span,
                    "type arguments on a member name are only allowed in a call, as in `x.downcast<T>()`",
                );
                error_expr(span)
            }
            K::Block(b) => {
                let (b, ty) = self.block(b, expect);
                mk(tir::ExprKind::Block(b), ty, span)
            }
            K::If { cond, then_block, else_expr } => self.if_expr(cond, then_block, else_expr.as_deref(), expect, span),
            K::While { cond, body } => self.while_expr(cond, body, None, span),
            K::WhileLet { pattern, scrutinee, body } => self.while_let(pattern, scrutinee, body, None, span),
            K::Loop(body) => self.loop_expr(body, None, expect, span),
            K::Labeled { label, body } => self.labeled_loop(label, body, expect, span),
            K::For { pattern, iter, mutable, body } => self.for_expr(pattern, iter, *mutable, body, None, span),
            K::Comptime(inner) => self.comptime_expr(inner, expect, span),
            K::ComptimeAssert(cond, message) => self.comptime_assert(cond, message.as_deref(), span),
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, expect, span),
            K::Break { label, value } => self.jump(label.as_ref(), true, value.as_deref(), span),
            K::Continue(label) => self.jump(label.as_ref(), false, None, span),
            K::Return(value) => {
                let ret = self.ret.clone();
                let value = match value {
                    Some(v) => {
                        let v = self.expr(v, Some(&ret));
                        Some(Box::new(self.expect_ty(v, &ret)))
                    }
                    None => {
                        if !compatible(&Ty::Unit, &ret) {
                            self.mismatch(span, &ret, &Ty::Unit);
                        }
                        None
                    }
                };
                mk(tir::ExprKind::Return(value), Ty::Never, span)
            }
            K::Assign(lhs, rhs) => self.assign(lhs, rhs, span),
            K::Binary(op, l, r) => self.binary(*op, l, r, expect, span),
            K::Unary(op, operand) => self.unary(*op, operand, expect, span),
            K::Cast(operand, ty) => {
                let target = self.resolve(ty);
                let operand = self.expr(operand, None);
                if !can_cast(&operand.ty, &target) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot convert `{}` to `{target}` with `as`", operand.ty),
                    );
                    return error_expr(span);
                }
                // A pair with a built-in `From` impl is that conversion (FR3); the others keep the cast.
                if lossless_from(&target, &operand.ty) {
                    return self.primitive_from_value(target, operand, span);
                }
                mk(tir::ExprKind::Cast(Box::new(operand)), target, span)
            }
            K::Call(callee, args) => self.call(callee, args, span, expect),
            K::Index(base, index) => self.index(base, index, span),
            K::Array(elems) => self.array(elems, expect, span),
            K::ArrayRepeat(value, count) => {
                let elem_expect = match expect {
                    Some(Ty::Array(t, _)) => Some(&**t),
                    _ => None,
                };
                let value = self.expr(value, elem_expect);
                let Some(n) = self.globals.eval_array_len(count, &self.subst, self.module, self.diags) else {
                    return error_expr(span);
                };
                let ty = Ty::Array(Box::new(value.ty.clone()), n);
                mk(tir::ExprKind::ArrayRepeat(Box::new(value), n), ty, span)
            }
            K::Field(base, name) => self.field(base, name, expect, span),
            K::DynField(base, name) => self.dyn_field(base, name, expect, span),
            K::TupleField(base, index, index_span) => {
                let base = self.expr(base, None);
                let base = self.auto_deref(base);
                let ty = match &base.ty {
                    Ty::Tuple(elems) if (*index as usize) < elems.len() => elems[*index as usize].clone(),
                    Ty::Error => return error_expr(span),
                    other => {
                        self.diags.error(
                            Code::UNKNOWN_NAME,
                            *index_span,
                            format!("no field `{index}` on type `{other}`"),
                        );
                        return error_expr(span);
                    }
                };
                mk(tir::ExprKind::Field(Box::new(base), *index as usize), ty, span)
            }
            K::Tuple(elems) => {
                let expects: Vec<Option<Ty>> = match expect {
                    Some(Ty::Tuple(ts)) if ts.len() == elems.len() => ts.iter().cloned().map(Some).collect(),
                    _ => vec![None; elems.len()],
                };
                let checked: Vec<tir::Expr> =
                    elems.iter().zip(&expects).map(|(e, t)| self.expr(e, t.as_ref())).collect();
                let ty = Ty::Tuple(checked.iter().map(|e| e.ty.clone()).collect());
                mk(tir::ExprKind::Tuple(checked), ty, span)
            }
            K::StructLit(path, fields) => self.struct_literal(path, fields, expect, span),
            K::IfLet { pattern, scrutinee, then_block, else_expr } => {
                self.if_let(pattern, scrutinee, then_block, else_expr.as_deref(), expect, span)
            }
            K::Range { lo, hi, inclusive } => self.range_expr(lo, hi, *inclusive, expect, span),
            K::OrElse(value, default) => self.orelse(value, default, span),
            K::Try(inner) => self.try_expr(inner, span),
            K::SelfValue => match self.self_local {
                Some(id) => mk(tir::ExprKind::Local(id), self.locals[id.0].ty.clone(), span),
                None => {
                    if let Some(e) = self.capture_name("self", span) {
                        return e;
                    }
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        span,
                        "`self` is only available in a method with a `self` parameter",
                    );
                    error_expr(span)
                }
            },
            K::Closure { params, ret, body } => self.closure(params, ret.as_ref(), body, expect, span),
            K::UnsafeBlock(b) => {
                self.unsafe_depth += 1;
                let (b, ty) = self.block(b, expect);
                self.unsafe_depth -= 1;
                mk(tir::ExprKind::Block(b), ty, span)
            }
        }
    }
}
