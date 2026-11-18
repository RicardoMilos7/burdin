//! Blocks and statements, `if`, `match`, and `select` (spec/grammar.md, sections 5 and 6).

use crate::cursor::{Closer, D, Fmt, Parts, R};
use crate::doc::{Doc, concat, group, indent, space, text};
use crate::exprs::{Ex, Restriction};
use crate::layout::Member;
use burdin_syntax::token::TokenKind;

pub struct StmtDoc {
    doc: D,
    blank_before: bool,
    /// An expression statement, not a `let`.
    expr_only: bool,
    /// The statement has a comment in it, in front of it, or after it.
    commented: bool,
}

pub struct BlockParts {
    open: Parts,
    stmts: Vec<StmtDoc>,
    close: Closer,
}

impl BlockParts {
    /// A block that may stay on the line of its parent: exactly one expression statement, and no comments.
    fn is_short(&self) -> bool {
        self.stmts.len() == 1
            && self.stmts[0].expr_only
            && !self.stmts[0].commented
            && self.open.trail.is_nil()
            && self.close.lead.is_empty()
    }
}

impl Fmt<'_> {
    pub fn block_parts(&mut self) -> R<BlockParts> {
        let open = self.open(&TokenKind::LBrace, true)?;
        self.depth += 1;
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let blank_before = self.blank_before();
            let comments_before = self.comments;
            let lead = self.start_of_line();
            self.stmt_start = self.pos;
            let (body, is_let) = self.stmt()?;
            let mut doc = concat(vec![lead, body]);
            if self.at(&TokenKind::Semi) {
                // A `;` between statements is dropped, except before a statement that starts with `||`: on a new
                // line that `||` would continue the expression before it.
                let keep = self.nth(1) == &TokenKind::OrOr;
                let semi = if keep { self.tok_doc() } else { self.dropped() };
                doc = concat(vec![doc, semi]);
            }
            let commented = self.comments != comments_before;
            stmts.push(StmtDoc { doc, blank_before, expr_only: !is_let, commented });
        }
        self.depth -= 1;
        let close = self.close(&TokenKind::RBrace)?;
        Ok(BlockParts { open, stmts, close })
    }

    /// A block as a document. `soft` lets a short block (see [`BlockParts::is_short`]) stay on one line when its
    /// enclosing group fits; otherwise the statements go one per line.
    pub fn block_doc(&mut self, parts: BlockParts, soft: bool) -> D {
        let BlockParts { open, stmts, close } = parts;
        if stmts.is_empty() && close.lead.is_empty() && open.trail.is_nil() {
            return concat(vec![open.lead, open.text, close.text, close.trail]);
        }
        let short = soft && stmts.len() == 1 && stmts[0].expr_only && !stmts[0].commented;
        if short && open.trail.is_nil() && close.lead.is_empty() {
            let stmt = stmts.into_iter().next().expect("one statement");
            return concat(vec![
                open.lead,
                open.text,
                indent(concat(vec![Doc::Line, stmt.doc])),
                Doc::Line,
                close.text,
                close.trail,
            ]);
        }
        let dangling = self.dangling(&close.lead, !stmts.is_empty());
        let mut inner = Vec::new();
        for (i, s) in stmts.into_iter().enumerate() {
            inner.push(Doc::Hard);
            if i > 0 && s.blank_before {
                inner.push(Doc::Hard);
            }
            inner.push(s.doc);
        }
        inner.push(dangling);
        concat(vec![open.lead, open.text, open.trail, indent(concat(inner)), Doc::Hard, close.text, close.trail])
    }

    fn stmt(&mut self) -> R<(D, bool)> {
        if !self.at(&TokenKind::Let) {
            return Ok((self.expr()?.doc, false));
        }
        let kw = self.tok_doc();
        let pattern = self.pattern()?;
        let mut parts = vec![kw, space(), pattern];
        if self.at(&TokenKind::Colon) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        let eq = self.take(&TokenKind::Eq)?;
        let init = self.expr()?;
        parts.push(space());
        parts.push(eq);
        parts.push(self.after_operator(init));
        if self.at(&TokenKind::Else) {
            let kw = self.tok_doc();
            let block = self.block_parts()?;
            parts.push(space());
            parts.push(kw);
            parts.push(space());
            parts.push(self.block_doc(block, false));
        }
        Ok((concat(parts), true))
    }

    /// `if cond { } else if cond { } else { }`, and `if let pattern = value { }`. A chain that is a statement lays its
    /// blocks out one statement per line; anywhere else a chain of short blocks may stay on one line.
    pub fn if_expr(&mut self) -> R<Ex> {
        let at_statement = self.stmt_start == self.pos;
        let mut links: Vec<(D, BlockParts)> = Vec::new();
        let mut closing_else: Option<(D, BlockParts)> = None;
        let mut prefix = Doc::Nil;
        loop {
            let kw = self.tok_doc();
            let mut head = vec![prefix, kw, space()];
            if self.at(&TokenKind::Let) {
                head.push(self.tok_doc());
                head.push(space());
                head.push(self.pattern()?);
                head.push(space());
                head.push(self.take(&TokenKind::Eq)?);
                head.push(space());
            }
            let cond = self.expr_r(Restriction::NoStructLiteral)?;
            head.push(cond.doc);
            head.push(space());
            let block = self.block_parts()?;
            links.push((concat(head), block));
            if !self.at(&TokenKind::Else) {
                break;
            }
            let else_kw = self.tok_doc();
            if self.at(&TokenKind::If) {
                prefix = concat(vec![space(), else_kw, space()]);
                continue;
            }
            closing_else = Some((concat(vec![space(), else_kw, space()]), self.block_parts()?));
            break;
        }
        let soft = !at_statement
            && links.iter().all(|(_, b)| b.is_short())
            && closing_else.as_ref().is_none_or(|(_, b)| b.is_short());
        let mut parts = Vec::new();
        for (head, block) in links {
            parts.push(head);
            parts.push(self.block_doc(block, soft));
        }
        if let Some((kw, block)) = closing_else {
            parts.push(kw);
            parts.push(self.block_doc(block, soft));
        }
        Ok(Ex { block_like: true, ..Ex::plain(group(concat(parts))) })
    }

    pub fn match_expr(&mut self) -> R<Ex> {
        let kw = self.tok_doc();
        let scrutinee = self.expr_r(Restriction::NoStructLiteral)?;
        let open = self.open(&TokenKind::LBrace, true)?;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let blank_before = self.blank_before();
            let lead = self.start_of_line();
            let mut head = Vec::new();
            if self.at(&TokenKind::Else) {
                head.push(self.tok_doc());
            } else {
                head.push(self.pattern()?);
                if self.at(&TokenKind::If) {
                    head.push(space());
                    head.push(self.tok_doc());
                    head.push(space());
                    head.push(self.expr()?.doc);
                }
            }
            let arm = self.arm_tail(concat(head))?;
            members.push(Member { doc: concat(vec![lead, arm]), after: Doc::Nil, blank_before });
        }
        let close = self.close(&TokenKind::RBrace)?;
        let arms = self.members_doc(open, members, close, "");
        Ok(Ex { block_like: true, ..Ex::plain(concat(vec![kw, space(), scrutinee.doc, space(), arms])) })
    }

    /// `=> body` and the comma after it, for the arm of a `match` or a `select`. A block body needs no comma.
    fn arm_tail(&mut self, head: D) -> R<D> {
        let arrow = self.take(&TokenKind::FatArrow)?;
        let body = self.expr()?;
        let block_like = body.block_like;
        let body_doc = if block_like { concat(vec![space(), body.doc]) } else { self.after_operator(body) };
        let comma = if self.at(&TokenKind::Comma) { self.dropped() } else { Doc::Nil };
        let separator = if block_like { Doc::Nil } else { text(",") };
        Ok(concat(vec![head, space(), arrow, body_doc, separator, comma]))
    }

    /// `select { name = channel.recv() => body, default => body }`.
    pub fn select_expr(&mut self) -> R<Ex> {
        let kw = self.tok_doc();
        let open = self.open(&TokenKind::LBrace, true)?;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let blank_before = self.blank_before();
            let lead = self.start_of_line();
            let mut head = Vec::new();
            if self.at_word("default") && self.nth(1) == &TokenKind::FatArrow {
                head.push(self.tok_doc());
            } else {
                let has_binding = matches!(self.kind(), TokenKind::Underscore | TokenKind::Mut)
                    || (matches!(self.kind(), TokenKind::Ident(_)) && self.nth(1) == &TokenKind::Eq);
                if has_binding {
                    head.push(self.pattern()?);
                    head.push(space());
                    head.push(self.take(&TokenKind::Eq)?);
                    head.push(space());
                }
                head.push(self.expr()?.doc);
                if self.at(&TokenKind::If) {
                    head.push(space());
                    head.push(self.tok_doc());
                    head.push(space());
                    head.push(self.expr()?.doc);
                }
            }
            let arm = self.arm_tail(concat(head))?;
            members.push(Member { doc: concat(vec![lead, arm]), after: Doc::Nil, blank_before });
        }
        let close = self.close(&TokenKind::RBrace)?;
        let arms = self.members_doc(open, members, close, "");
        Ok(Ex { block_like: true, ..Ex::plain(concat(vec![kw, space(), arms])) })
    }
}
