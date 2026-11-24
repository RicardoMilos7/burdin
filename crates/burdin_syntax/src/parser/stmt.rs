//! Blocks and statements.

use super::{PResult, Parser, Reported};
use crate::ast::*;
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn block(&mut self) -> PResult<Block> {
        let open = self.open(&TokenKind::LBrace, true)?;
        let depth = self.nl_stack.len();
        let mut stmts = Vec::new();
        loop {
            if self.at(&TokenKind::RBrace) {
                break;
            }
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            match self.stmt() {
                Ok(stmt) => {
                    stmts.push(stmt);
                    // A statement ends at a line break, `;`, or the closing brace.
                    if !(self.at(&TokenKind::RBrace) || self.eat(&TokenKind::Semi) || self.tok().nl_before) {
                        if self.at_eof() {
                            return Err(self.unexpected("`}`"));
                        }
                        self.unexpected("a line break or `}` after the statement");
                        self.recover_stmt(depth);
                    }
                }
                Err(Reported) => self.recover_stmt(depth),
            }
        }
        let close = self.close(&TokenKind::RBrace)?;
        Ok(Block { stmts, span: open.span.to(close.span) })
    }

    pub(super) fn stmt(&mut self) -> PResult<Stmt> {
        // A `static` in a body is reported (E0302, `spec/globals.md`, GL1) and then read as a `let`.
        if self.at(&TokenKind::Let) || self.at_static_decl() {
            let start = if self.at_static_decl() { self.static_in_body() } else { self.bump().span };
            let pattern = self.pattern()?;
            let ty = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
            self.expect(&TokenKind::Eq)?;
            let init = self.expr()?;
            let else_block = if self.at(&TokenKind::Else) {
                self.bump();
                Some(self.block()?)
            } else {
                None
            };
            let span = start.to(self.prev_span());
            return Ok(Stmt::Let { pattern, ty, init, else_block, span });
        }
        Ok(Stmt::Expr(self.expr()?))
    }
}
