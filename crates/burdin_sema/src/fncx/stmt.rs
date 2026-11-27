//! Blocks and statements.

use super::FnCx;
use burdin_diag::Code;
use burdin_syntax::ast;
use burdin_tir::{self as tir, Ty};
use std::collections::HashMap;

impl FnCx<'_> {
    pub(super) fn block(&mut self, b: &ast::Block, expect: Option<&Ty>) -> (tir::Block, Ty) {
        self.scopes.push(HashMap::new());
        let mut stmts = Vec::new();
        let mut tail = None;
        let mut diverges = false;
        for (i, s) in b.stmts.iter().enumerate() {
            let last = i + 1 == b.stmts.len();
            match s {
                ast::Stmt::Let { pattern, ty, init, else_block, .. } => {
                    let declared = ty.as_ref().map(|t| self.resolve(t));
                    let init = self.expr(init, declared.as_ref());
                    let init = match &declared {
                        Some(d) => {
                            let init = self.coerce_arg(init, d);
                            let origin_msg = format!("expected `{d}` due to this type annotation");
                            let origin = ty.as_ref().map(|t| (t.span, origin_msg.as_str()));
                            self.expect_ty_from(init, d, origin)
                        }
                        None => init,
                    };
                    diverges |= init.ty == Ty::Never;
                    let local_ty = declared.unwrap_or_else(|| init.ty.clone());
                    // The `else` block runs when the pattern fails, so the pattern's bindings are not in scope there.
                    let else_checked = else_block.as_ref().map(|b| {
                        let (checked, ty) = self.block(b, None);
                        if !matches!(ty, Ty::Never | Ty::Error) {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                b.span,
                                "the `else` block of `let ... else` must not finish normally; \
                                 end it with `return`, `break`, `continue`, or `panic`",
                            );
                        }
                        checked
                    });
                    let pat = self.pattern(pattern, &local_ty, true);
                    let refutable = !self.exhaustive(&local_ty, &[&pat]);
                    if refutable && else_checked.is_none() {
                        self.report_non_exhaustive(
                            &local_ty,
                            &[&pat],
                            pattern.span,
                            format!("this pattern does not match every value of `{local_ty}`; add an `else` block"),
                            0,
                            false,
                        );
                    } else if !refutable && else_checked.is_some() {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            pattern.span,
                            "this pattern always matches, so `let` takes no `else` block",
                        );
                    }
                    stmts.push(tir::Stmt::Let(pat, init, else_checked));
                }
                ast::Stmt::Expr(e) => {
                    let e = self.expr(e, if last { expect } else { None });
                    diverges |= e.ty == Ty::Never;
                    if last {
                        tail = Some(Box::new(e));
                    } else {
                        stmts.push(tir::Stmt::Expr(e));
                    }
                }
            }
        }
        self.scopes.pop();
        let ty = match &tail {
            Some(e) if !(diverges && e.ty == Ty::Unit) => e.ty.clone(),
            _ if diverges => Ty::Never,
            _ => Ty::Unit,
        };
        let close = burdin_diag::Span::new(b.span.file, b.span.hi.saturating_sub(1), b.span.hi);
        (tir::Block { stmts, tail, close }, ty)
    }
}
