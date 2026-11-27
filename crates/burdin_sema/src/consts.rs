//! Constants: their values are computed at compile time and expanded at every use, so they must not depend on
//! themselves.

use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::visit::{Visit, walk_expr};
use burdin_tir::{self as tir, ConstId};

/// A constant is expanded at every use, so it must not refer to itself, directly or through other constants.
pub(crate) fn check_const_cycles(consts: &[tir::Const], decls: &[&ast::ConstDecl], diags: &mut Diagnostics) {
    // Every constant the initializer names, anywhere in it. A cycle through a function call is found when the
    // interpreter evaluates the constant (`spec/const-eval.md`, CE2).
    fn refs(e: &tir::Expr, out: &mut Vec<ConstId>) {
        struct Refs<'a>(&'a mut Vec<ConstId>);
        impl Visit for Refs<'_> {
            fn expr(&mut self, e: &tir::Expr) {
                if let tir::ExprKind::Const(id) = &e.kind {
                    self.0.push(*id);
                }
                walk_expr(self, e);
            }
        }
        Refs(out).expr(e);
    }
    let deps: Vec<Vec<ConstId>> = consts
        .iter()
        .map(|c| {
            let mut v = Vec::new();
            refs(&c.value, &mut v);
            v
        })
        .collect();
    for i in 0..decls.len() {
        if deps[i].iter().any(|d| reaches(d.0, i, &deps)) {
            diags.error(
                Code::CYCLIC_DECLARATION,
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
