//! Expressions, by precedence.

use super::{PResult, Parser, Restriction};
use crate::ast::*;
use crate::token::TokenKind;
use burdin_diag::Span;

impl Parser<'_> {
    pub fn expr(&mut self) -> PResult<Expr> {
        self.expr_r(Restriction::None)
    }

    pub(super) fn expr_r(&mut self, r: Restriction) -> PResult<Expr> {
        let lhs = self.orelse(r)?;
        if self.at(&TokenKind::Eq) && !self.nl() {
            self.bump();
            let rhs = self.expr_r(r)?;
            let span = lhs.span.to(rhs.span);
            return Ok(Expr { kind: ExprKind::Assign(Box::new(lhs), Box::new(rhs)), span });
        }
        Ok(lhs)
    }

    pub(super) fn orelse(&mut self, r: Restriction) -> PResult<Expr> {
        let mut lhs = self.range(r)?;
        while self.at(&TokenKind::Orelse) {
            self.bump();
            let rhs = self.range(r)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr { kind: ExprKind::OrElse(Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    pub(super) fn range(&mut self, r: Restriction) -> PResult<Expr> {
        let lo = self.binary(LEVEL_OR, r)?;
        let inclusive = match self.kind() {
            TokenKind::DotDot => false,
            TokenKind::DotDotEq => true,
            _ => return Ok(lo),
        };
        if self.nl() {
            return Ok(lo);
        }
        self.bump();
        let hi = self.binary(LEVEL_OR, r)?;
        let span = lo.span.to(hi.span);
        if matches!(self.kind(), TokenKind::DotDot | TokenKind::DotDotEq) && !self.nl() {
            return Err(self.error_here("range operators cannot be chained".into()));
        }
        Ok(Expr { kind: ExprKind::Range { lo: Box::new(lo), hi: Box::new(hi), inclusive }, span })
    }

    /// The binary operator at the current position and its level, if it continues the expression.
    pub(super) fn binop(&self) -> Option<(BinOp, u8)> {
        use TokenKind as T;
        let (op, level) = match self.kind() {
            T::OrOr => (BinOp::Or, LEVEL_OR),
            T::AndAnd => (BinOp::And, 5),
            T::EqEq => (BinOp::Eq, LEVEL_CMP),
            T::NotEq => (BinOp::Ne, LEVEL_CMP),
            T::Lt => (BinOp::Lt, LEVEL_CMP),
            T::Gt => (BinOp::Gt, LEVEL_CMP),
            T::Le => (BinOp::Le, LEVEL_CMP),
            T::Ge => (BinOp::Ge, LEVEL_CMP),
            T::Pipe => (BinOp::BitOr, 7),
            T::Caret => (BinOp::BitXor, 8),
            T::Amp => (BinOp::BitAnd, 9),
            T::Shl => (BinOp::Shl, 10),
            T::Shr => (BinOp::Shr, 10),
            T::Plus => (BinOp::Add, 11),
            T::Minus => (BinOp::Sub, 11),
            T::Star => (BinOp::Mul, 12),
            T::Slash => (BinOp::Div, 12),
            T::Percent => (BinOp::Rem, 12),
            _ => return None,
        };
        // Only `&&` and `||` may start a continuation line.
        if self.nl() && !matches!(op, BinOp::And | BinOp::Or) {
            return None;
        }
        Some((op, level))
    }

    pub(super) fn binary(&mut self, min: u8, r: Restriction) -> PResult<Expr> {
        let mut lhs = self.cast(r)?;
        while let Some((op, level)) = self.binop() {
            if level < min {
                break;
            }
            self.bump();
            let rhs = self.binary(level + 1, r)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr { kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span };
            if level == LEVEL_CMP && matches!(self.binop(), Some((_, LEVEL_CMP))) {
                return Err(self.error_here("comparison operators cannot be chained; use `&&`".into()));
            }
        }
        Ok(lhs)
    }

    pub(super) fn cast(&mut self, r: Restriction) -> PResult<Expr> {
        let mut e = self.unary(r)?;
        while self.at(&TokenKind::As) {
            self.bump();
            let ty = self.ty()?;
            let span = e.span.to(ty.span);
            e = Expr { kind: ExprKind::Cast(Box::new(e), ty), span };
        }
        Ok(e)
    }

    pub(super) fn unary(&mut self, r: Restriction) -> PResult<Expr> {
        let start = self.span();
        let op = match self.kind() {
            TokenKind::Minus => UnOp::Neg,
            TokenKind::Bang => UnOp::Not,
            TokenKind::Star => UnOp::Deref,
            TokenKind::Amp => UnOp::Ref,
            TokenKind::AndAnd => {
                return Err(
                    self.error_here("`&&` is not a prefix operator; write `& &` for a reference to a reference".into())
                );
            }
            _ => return self.postfix(r),
        };
        self.bump();
        let op = if op == UnOp::Ref && self.eat(&TokenKind::Mut) { UnOp::RefMut } else { op };
        let operand = self.unary(r)?;
        let span = start.to(operand.span);
        Ok(Expr { kind: ExprKind::Unary(op, Box::new(operand)), span })
    }

    pub(super) fn postfix(&mut self, r: Restriction) -> PResult<Expr> {
        let mut e = self.primary(r)?;
        loop {
            match self.kind().clone() {
                TokenKind::Question => {
                    let span = e.span.to(self.bump().span);
                    e = Expr { kind: ExprKind::Try(Box::new(e)), span };
                }
                TokenKind::Dot => {
                    self.bump();
                    match self.kind().clone() {
                        TokenKind::Ident(name) => {
                            let t = self.bump();
                            let span = e.span.to(t.span);
                            e = Expr { kind: ExprKind::Field(Box::new(e), Ident { name, span: t.span }), span };
                            if self.at_struct_literal(r)
                                && let Some(path) = expr_path(&e)
                            {
                                let start = e.span;
                                e = self.struct_literal(path, start)?;
                            }
                        }
                        TokenKind::Int(n) => {
                            let t = self.bump();
                            let span = e.span.to(t.span);
                            let index = u32::try_from(n).unwrap_or(u32::MAX);
                            e = Expr { kind: ExprKind::TupleField(Box::new(e), index, t.span), span };
                        }
                        _ => return Err(self.unexpected("a field name")),
                    }
                }
                TokenKind::LParen if !self.nl() => {
                    self.open(&TokenKind::LParen, false)?;
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        let mut_span = self.at(&TokenKind::Mut).then(|| self.bump().span);
                        let expr = self.expr()?;
                        args.push(Arg { mut_marker: mut_span.is_some(), mut_span, expr });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let close = self.close(&TokenKind::RParen)?;
                    let span = e.span.to(close.span);
                    e = Expr { kind: ExprKind::Call(Box::new(e), args), span };
                }
                TokenKind::LBracket if !self.nl() => {
                    self.open(&TokenKind::LBracket, false)?;
                    let index = self.expr()?;
                    let close = self.close(&TokenKind::RBracket)?;
                    let span = e.span.to(close.span);
                    e = Expr { kind: ExprKind::Index(Box::new(e), Box::new(index)), span };
                }
                _ => break,
            }
        }
        Ok(e)
    }

    pub(super) fn can_start_expr(kind: &TokenKind) -> bool {
        use TokenKind::*;
        matches!(
            kind,
            Ident(_)
                | Int(_)
                | Float(_)
                | Char(_)
                | Str(_)
                | True
                | False
                | SelfValue
                | SelfType
                | LParen
                | LBracket
                | LBrace
                | If
                | Match
                | Loop
                | While
                | For
                | Unsafe
                | Return
                | Break
                | Continue
                | Minus
                | Bang
                | Star
                | Amp
        )
    }

    pub(super) fn primary(&mut self, r: Restriction) -> PResult<Expr> {
        let start = self.span();
        let lit = |kind| Ok(Expr { kind, span: start });
        match self.kind().clone() {
            TokenKind::Int(v) => {
                self.bump();
                lit(ExprKind::Int(v))
            }
            TokenKind::Float(text) => {
                self.bump();
                lit(ExprKind::Float(text))
            }
            TokenKind::Char(c) => {
                self.bump();
                lit(ExprKind::Char(c))
            }
            TokenKind::Str(s) => {
                self.bump();
                lit(ExprKind::Str(s))
            }
            TokenKind::True => {
                self.bump();
                lit(ExprKind::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                lit(ExprKind::Bool(false))
            }
            TokenKind::SelfValue => {
                self.bump();
                lit(ExprKind::SelfValue)
            }
            // `Self` in expression position: the enclosing impl's type, as the left side of `Self.new()`,
            // `Self.Variant`, or `Self { ... }`.
            TokenKind::SelfType => {
                self.bump();
                let name = Ident { name: "Self".to_string(), span: start };
                if self.at_struct_literal(r) {
                    return self.struct_literal(vec![name], start);
                }
                Ok(Expr { kind: ExprKind::Name { name, args: Vec::new() }, span: start })
            }
            TokenKind::Ident(_) => {
                let generic = self.generic_args_ahead();
                let name = self.ident("an expression")?;
                let args = if generic { self.expr_generic_args()? } else { Vec::new() };
                if self.at_struct_literal(r) {
                    return self.struct_literal(vec![name], start);
                }
                let span = if args.is_empty() { name.span } else { start.to(self.prev_span()) };
                Ok(Expr { span, kind: ExprKind::Name { name, args } })
            }
            TokenKind::LParen => {
                self.open(&TokenKind::LParen, false)?;
                if self.at(&TokenKind::RParen) {
                    let close = self.close(&TokenKind::RParen)?;
                    return Ok(Expr { kind: ExprKind::Unit, span: start.to(close.span) });
                }
                let first = self.expr()?;
                if !self.at(&TokenKind::Comma) {
                    let close = self.close(&TokenKind::RParen)?;
                    return Ok(Expr { kind: first.kind, span: start.to(close.span) });
                }
                let mut elems = vec![first];
                while self.eat(&TokenKind::Comma) {
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    elems.push(self.expr()?);
                }
                let close = self.close(&TokenKind::RParen)?;
                Ok(Expr { kind: ExprKind::Tuple(elems), span: start.to(close.span) })
            }
            TokenKind::LBracket => {
                self.open(&TokenKind::LBracket, false)?;
                let mut elems = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    let first = self.expr()?;
                    if self.eat(&TokenKind::Semi) {
                        let count = self.expr()?;
                        let close = self.close(&TokenKind::RBracket)?;
                        let kind = ExprKind::ArrayRepeat(Box::new(first), Box::new(count));
                        return Ok(Expr { kind, span: start.to(close.span) });
                    }
                    elems.push(first);
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::RBracket) {
                            break;
                        }
                        elems.push(self.expr()?);
                    }
                }
                let close = self.close(&TokenKind::RBracket)?;
                Ok(Expr { kind: ExprKind::Array(elems), span: start.to(close.span) })
            }
            TokenKind::LBrace => {
                let b = self.block()?;
                Ok(Expr { span: b.span, kind: ExprKind::Block(b) })
            }
            TokenKind::Unsafe => {
                self.bump();
                let b = self.block()?;
                Ok(Expr { span: start.to(b.span), kind: ExprKind::UnsafeBlock(b) })
            }
            TokenKind::If => self.if_expr(),
            TokenKind::While => {
                self.bump();
                let cond = self.expr_r(Restriction::NoStructLiteral)?;
                let body = self.block()?;
                Ok(Expr { span: start.to(body.span), kind: ExprKind::While { cond: Box::new(cond), body } })
            }
            TokenKind::Loop => {
                self.bump();
                let body = self.block()?;
                Ok(Expr { span: start.to(body.span), kind: ExprKind::Loop(body) })
            }
            TokenKind::For => {
                self.bump();
                let pattern = self.pattern()?;
                self.expect(&TokenKind::In)?;
                let iter = self.expr_r(Restriction::NoStructLiteral)?;
                let body = self.block()?;
                Ok(Expr { span: start.to(body.span), kind: ExprKind::For { pattern, iter: Box::new(iter), body } })
            }
            TokenKind::Return => {
                self.bump();
                let value =
                    if !self.nl() && Self::can_start_expr(self.kind()) { Some(Box::new(self.expr()?)) } else { None };
                let span = start.to(self.prev_span());
                Ok(Expr { kind: ExprKind::Return(value), span })
            }
            TokenKind::Break => {
                self.bump();
                lit(ExprKind::Break)
            }
            TokenKind::Continue => {
                self.bump();
                lit(ExprKind::Continue)
            }
            TokenKind::Match => self.match_expr(),
            TokenKind::Pipe | TokenKind::OrOr => Err(self.error_here("closures are not part of stage 0".into())),
            TokenKind::Reserved(_) => {
                self.ident("an expression")?;
                unreachable!()
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    pub(super) fn at_struct_literal(&self, r: Restriction) -> bool {
        self.at(&TokenKind::LBrace) && !self.nl() && r == Restriction::None && looks_like_struct_literal(self)
    }

    pub(super) fn struct_literal(&mut self, path: Vec<Ident>, start: Span) -> PResult<Expr> {
        self.open(&TokenKind::LBrace, true)?;
        let mut fields = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            loop {
                let name = self.ident("a field name")?;
                let value = if self.eat(&TokenKind::Colon) { Some(self.expr()?) } else { None };
                fields.push(FieldInit { name, value });
                if !self.brace_list_sep("field")? {
                    break;
                }
            }
        }
        let close = self.close(&TokenKind::RBrace)?;
        Ok(Expr { kind: ExprKind::StructLit(path, fields), span: start.to(close.span) })
    }

    pub(super) fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&TokenKind::Match)?.span;
        let scrutinee = self.expr_r(Restriction::NoStructLiteral)?;
        self.open(&TokenKind::LBrace, true)?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            let arm_start = self.span();
            let pattern = if self.eat(&TokenKind::Else) { None } else { Some(self.pattern()?) };
            let guard = if pattern.is_some() && self.eat(&TokenKind::If) { Some(self.expr()?) } else { None };
            self.expect(&TokenKind::FatArrow)?;
            let body = self.expr()?;
            let span = arm_start.to(body.span);
            let block_body = body.is_block_like();
            arms.push(MatchArm { pattern, guard, body, span });
            // Arms are separated by `,` or a line break; none is needed after a block body.
            if !(self.eat(&TokenKind::Comma) || self.at(&TokenKind::RBrace) || self.tok().nl_before || block_body) {
                return Err(self.unexpected("`,`, a line break, or `}` after the match arm"));
            }
        }
        let close = self.close(&TokenKind::RBrace)?;
        Ok(Expr { kind: ExprKind::Match(Box::new(scrutinee), arms), span: start.to(close.span) })
    }

    pub(super) fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&TokenKind::If)?.span;
        let let_pattern = if self.eat(&TokenKind::Let) {
            let pattern = self.pattern()?;
            self.expect(&TokenKind::Eq)?;
            Some(pattern)
        } else {
            None
        };
        let cond = self.expr_r(Restriction::NoStructLiteral)?;
        let then_block = self.block()?;
        let mut span = start.to(then_block.span);
        let else_expr = if self.at(&TokenKind::Else) {
            self.bump();
            let e = if self.at(&TokenKind::If) {
                self.if_expr()?
            } else {
                let b = self.block()?;
                Expr { span: b.span, kind: ExprKind::Block(b) }
            };
            span = span.to(e.span);
            Some(Box::new(e))
        } else {
            None
        };
        let kind = match let_pattern {
            Some(pattern) => ExprKind::IfLet { pattern, scrutinee: Box::new(cond), then_block, else_expr },
            None => ExprKind::If { cond: Box::new(cond), then_block, else_expr },
        };
        Ok(Expr { kind, span })
    }
}

const LEVEL_OR: u8 = 4;
const LEVEL_CMP: u8 = 6;

/// The identifiers of a path expression such as `Shape.Rect`, or `None` if `e` is not a plain path.
fn expr_path(e: &Expr) -> Option<Vec<Ident>> {
    match &e.kind {
        ExprKind::Name { name, args } if args.is_empty() => Some(vec![name.clone()]),
        ExprKind::Field(base, id) => {
            let mut path = expr_path(base)?;
            path.push(id.clone());
            Some(path)
        }
        _ => None,
    }
}

/// `Name {` starts a struct literal when the brace is followed by `}` or by `ident` and then `:`, `,`, `}` or a
/// line break. A block after a name (as in `while x { ... }`) is excluded by the restriction instead.
fn looks_like_struct_literal(p: &Parser<'_>) -> bool {
    match p.nth_kind(1) {
        TokenKind::RBrace => true,
        TokenKind::Ident(_) => matches!(p.nth_kind(2), TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace),
        _ => false,
    }
}
