//! Compile-time conditions (`spec/const-eval.md`, CE4): an `if` whose condition is built only from literals, `const`
//! items, operators, enum variant paths, and the functions of the `target` module keeps only the taken branch in the
//! generated code. Checking only marks the condition; the interpreter evaluates it after the memory checks.

use super::{FnCx, mk};
use burdin_tir::{self as tir, ExprKind as K};

impl FnCx<'_> {
    /// Wraps `cond` in [`tir::ExprKind::ConstCond`] when it is a compile-time condition. Any other condition is
    /// returned as it is, and the `if` is an ordinary run-time `if`.
    pub(super) fn mark_const_condition(&self, cond: tir::Expr) -> tir::Expr {
        if !self.is_const_condition(&cond) {
            return cond;
        }
        let (ty, span) = (cond.ty.clone(), cond.span);
        mk(K::ConstCond(Box::new(cond)), ty, span)
    }

    /// Whether `e` is built only from the forms CE4 lists. A literal alone is one, so `if true { ... }` is too.
    fn is_const_condition(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Unit | K::Const(_) => true,
            K::Unary(_, x) | K::Cast(x) => self.is_const_condition(x),
            K::Binary(_, a, b) => self.is_const_condition(a) && self.is_const_condition(b),
            K::Variant(_, fields) => fields.iter().all(|(_, f)| self.is_const_condition(f)),
            K::Call(f, args, _) => self.globals.is_target_fn(*f) && args.iter().all(|a| self.is_const_condition(a)),
            _ => false,
        }
    }
}
