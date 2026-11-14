//! Constants: they are expanded at every use, so they must be simple and must not depend on themselves.

use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::{self as tir, ConstId, Ty};

/// A constant is expanded at every use, so it must not refer to itself, directly or through other constants.
pub(crate) fn check_const_cycles(consts: &[tir::Const], decls: &[&ast::ConstDecl], diags: &mut Diagnostics) {
    fn refs(e: &tir::Expr, out: &mut Vec<ConstId>) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Const(id) => out.push(*id),
            K::Unary(_, a) | K::Cast(a) => refs(a, out),
            K::Binary(_, a, b) => {
                refs(a, out);
                refs(b, out);
            }
            K::If(c, then, other) => {
                refs(c, out);
                if let Some(t) = &then.tail {
                    refs(t, out);
                }
                if let Some(o) = other {
                    refs(o, out);
                }
            }
            K::Block(b) => {
                if let Some(t) = &b.tail {
                    refs(t, out);
                }
            }
            _ => {}
        }
    }
    let deps: Vec<Vec<ConstId>> = consts
        .iter()
        .map(|c| {
            let mut v = Vec::new();
            refs(&c.value, &mut v);
            v
        })
        .collect();
    for i in 0..consts.len() {
        if deps[i].iter().any(|d| reaches(d.0, i, &deps)) {
            diags.error(
                Code::UNSUPPORTED,
                decls[i].name.span,
                format!("constant `{}` depends on itself", decls[i].name.name),
            );
        }
    }
    fn reaches(from: usize, to: usize, deps: &[Vec<ConstId>]) -> bool {
        let mut seen = vec![false; deps.len()];
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(deps[n].iter().map(|d| d.0));
            }
        }
        false
    }
}

pub(crate) fn is_const_expr(e: &tir::Expr) -> bool {
    use tir::ExprKind as K;
    match &e.kind {
        K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Const(_) => true,
        K::Unary(_, a) | K::Cast(a) => is_const_expr(a),
        K::Binary(_, a, b) => is_const_expr(a) && is_const_expr(b),
        _ => e.ty == Ty::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::{FileId, Span};

    fn expr(kind: tir::ExprKind, ty: Ty) -> tir::Expr {
        tir::Expr { kind, ty, span: Span::new(FileId(0), 0, 0) }
    }

    fn int(value: i128) -> tir::Expr {
        expr(tir::ExprKind::Int(value), Ty::Int(tir::IntTy::I32))
    }

    #[test]
    fn literals_and_operators_on_them_are_constants() {
        assert!(is_const_expr(&int(1)));
        assert!(is_const_expr(&expr(tir::ExprKind::Bool(true), Ty::Bool)));
        assert!(is_const_expr(&expr(tir::ExprKind::Str("x".into()), Ty::Str)));
        assert!(is_const_expr(&expr(
            tir::ExprKind::Binary(tir::BinOp::Add, Box::new(int(1)), Box::new(int(2))),
            Ty::Int(tir::IntTy::I32)
        )));
        assert!(is_const_expr(&expr(tir::ExprKind::Unary(tir::UnOp::Neg, Box::new(int(1))), Ty::Int(tir::IntTy::I32))));
        assert!(is_const_expr(&expr(tir::ExprKind::Cast(Box::new(int(1))), Ty::Int(tir::IntTy::I64))));
    }

    #[test]
    fn calls_and_locals_are_not_constants() {
        let i32_ = Ty::Int(tir::IntTy::I32);
        assert!(!is_const_expr(&expr(tir::ExprKind::Local(tir::LocalId(0)), i32_.clone())));
        assert!(!is_const_expr(&expr(tir::ExprKind::Call(tir::FuncId(0), vec![]), i32_.clone())));
        // A binary expression is only constant when both sides are.
        assert!(!is_const_expr(&expr(
            tir::ExprKind::Binary(
                tir::BinOp::Add,
                Box::new(int(1)),
                Box::new(expr(tir::ExprKind::Local(tir::LocalId(0)), i32_.clone()))
            ),
            i32_
        )));
    }

    #[test]
    fn an_error_typed_expression_is_accepted_to_avoid_cascades() {
        assert!(is_const_expr(&expr(tir::ExprKind::Call(tir::FuncId(0), vec![]), Ty::Error)));
    }
}
