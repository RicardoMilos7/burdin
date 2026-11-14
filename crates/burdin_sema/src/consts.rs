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
