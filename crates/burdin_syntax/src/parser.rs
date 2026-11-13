//! The parser (spec/grammar.md). It reports E0001 for anything outside the grammar and recovers at statement and
//! item boundaries so that every syntax error in a file is reported.
//!
//! Constructs that are in the grammar but not yet implemented by this compiler are reported with
//! `Code::UNSUPPORTED`.

use crate::ast::*;
use crate::lexer::LexResult;
use crate::token::{Token, TokenKind};
use burdin_diag::{Code, Diagnostics, FileId, Span};

/// The error has already been reported; the caller recovers.
pub struct Reported;

type PResult<T> = Result<T, Reported>;

pub fn parse_file(file: FileId, lexed: LexResult, diags: &mut Diagnostics) -> File {
    let mut p = Parser { file, toks: lexed.tokens, pos: 0, diags, nl_stack: Vec::new(), truncated: lexed.truncated };
    p.file()
}

struct Parser<'a> {
    #[allow(dead_code)]
    file: FileId,
    toks: Vec<Token>,
    pos: usize,
    diags: &'a mut Diagnostics,
    /// Whether line breaks are significant, one entry per open bracket (spec/grammar.md, section 2).
    nl_stack: Vec<bool>,
    truncated: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Restriction {
    None,
    /// In the head of `if`, `while`, `for`, `match`: struct literals are not allowed.
    NoStructLiteral,
}

impl Parser<'_> {
    // ----- token helpers ---------------------------------------------------

    fn tok(&self) -> &Token {
        &self.toks[self.pos]
    }

    fn kind(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    fn nth_kind(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].kind
    }

    fn span(&self) -> Span {
        self.tok().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 { self.span() } else { self.toks[self.pos - 1].span }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.kind() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(&TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// A line break before the current token that ends an expression here.
    fn nl(&self) -> bool {
        self.tok().nl_before && self.nl_stack.last().copied().unwrap_or(true)
    }

    fn error_here(&mut self, msg: String) -> Reported {
        // Past an unterminated literal the rest of the file is gone; the lexer already reported it.
        if !(self.truncated && self.at_eof()) {
            let span = self.span();
            self.diags.error(Code::SYNTAX, span, msg);
        }
        Reported
    }

    fn unexpected(&mut self, expected: &str) -> Reported {
        let found = self.kind().describe();
        self.error_here(format!("expected {expected}, found {found}"))
    }

    fn unsupported(&mut self, span: Span, what: &str) -> Reported {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet"));
        Reported
    }

    fn expect(&mut self, kind: &TokenKind) -> PResult<Token> {
        if self.at(kind) { Ok(self.bump()) } else { Err(self.unexpected(&format!("`{}`", kind.text()))) }
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        match self.kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            TokenKind::Reserved(word) => Err(self.error_here(format!(
                "`{word}` is reserved for a later version of Burdin and cannot be used as a name"
            ))),
            _ => Err(self.unexpected(what)),
        }
    }

    /// Opens a bracket and records whether line breaks matter inside it.
    fn open(&mut self, kind: &TokenKind, newlines_matter: bool) -> PResult<Token> {
        let t = self.expect(kind)?;
        self.nl_stack.push(newlines_matter);
        Ok(t)
    }

    fn close(&mut self, kind: &TokenKind) -> PResult<Token> {
        let t = self.expect(kind)?;
        self.nl_stack.pop();
        Ok(t)
    }

    /// Consumes a `>` that closes generic arguments, splitting `>>` and `>=`.
    fn eat_closing_angle(&mut self) -> bool {
        let span = self.span();
        let rest = match self.kind() {
            TokenKind::Gt => {
                self.bump();
                return true;
            }
            TokenKind::Shr => TokenKind::Gt,
            TokenKind::Ge => TokenKind::Eq,
            _ => return false,
        };
        let t = &mut self.toks[self.pos];
        t.kind = rest;
        t.span = Span::new(span.file, span.lo + 1, span.hi);
        t.nl_before = false;
        true
    }

    // ----- recovery --------------------------------------------------------

    /// Keywords that only start items (stage 0 has no nested items), so they end recovery even inside unbalanced
    /// brackets.
    fn starts_item_only(kind: &TokenKind) -> bool {
        use TokenKind::*;
        matches!(kind, Fn | Struct | Enum | Interface | Type | Const | Use | Extern | Priv)
    }

    /// Skips to the start of the next item: an item keyword at the start of a line.
    fn recover_item(&mut self) {
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            let item_start = Self::starts_item_only(&t.kind) || (depth == 0 && t.kind == TokenKind::Unsafe);
            if self.pos > start && t.nl_before && item_start {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                // A stray closer (the one that caused the error) must not unbalance the count.
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                _ => {}
            }
            self.bump();
        }
        self.nl_stack.clear();
    }

    /// Skips to the end of the current statement: a line break or `}` at the block's own depth.
    fn recover_stmt(&mut self, block_depth: usize) {
        self.nl_stack.truncate(block_depth);
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            if depth == 0 && (matches!(t.kind, TokenKind::RBrace) || (self.pos > start && t.nl_before)) {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                // A stray closer (the one that caused the error) must not unbalance the count.
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                _ => {}
            }
            self.bump();
        }
    }

    // ----- items -----------------------------------------------------------

    fn file(&mut self) -> File {
        let mut file = File::default();
        while !self.at_eof() {
            self.nl_stack.clear();
            match self.item() {
                Ok(Some(item)) => file.items.push(item),
                Ok(None) => {}
                Err(Reported) => self.recover_item(),
            }
        }
        file
    }

    fn item(&mut self) -> PResult<Option<Item>> {
        let start = self.span();
        if let TokenKind::Ident(name) = self.kind()
            && name == "test"
            && matches!(self.nth_kind(1), TokenKind::Str(_))
        {
            return self.skip_unsupported_item(start, "`test` blocks");
        }
        let is_priv = self.eat(&TokenKind::Priv);
        match self.kind() {
            TokenKind::Fn | TokenKind::Unsafe => Ok(Some(Item::Fn(self.fn_decl(is_priv, start)?))),
            TokenKind::Struct => self.skip_unsupported_item(start, "`struct`"),
            TokenKind::Enum => self.skip_unsupported_item(start, "`enum`"),
            TokenKind::Interface => self.skip_unsupported_item(start, "`interface`"),
            TokenKind::Type => self.skip_unsupported_item(start, "`type` aliases"),
            TokenKind::Const => self.skip_unsupported_item(start, "`const`"),
            TokenKind::Use => self.skip_unsupported_item(start, "`use`"),
            TokenKind::Extern => self.skip_unsupported_item(start, "`extern` blocks"),
            TokenKind::Ident(_) | TokenKind::SelfType if !is_priv => self.skip_unsupported_item(start, "method blocks"),
            _ => Err(self.unexpected("an item")),
        }
    }

    /// Reports an item this compiler cannot compile yet and skips it whole: through its first balanced `{ ... }`,
    /// or to the end of the line for items without a body. This keeps the methods inside it from being parsed as
    /// separate items.
    fn skip_unsupported_item(&mut self, start: Span, what: &str) -> PResult<Option<Item>> {
        self.unsupported(start, what);
        let first = self.pos;
        let mut depth = 0i32;
        let mut seen_brace = false;
        while !self.at_eof() {
            let t = self.tok();
            if depth == 0 && !seen_brace && self.pos > first && t.nl_before {
                break;
            }
            match t.kind {
                TokenKind::LBrace => {
                    depth += 1;
                    seen_brace = true;
                }
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                TokenKind::RBrace => {
                    depth = (depth - 1).max(0);
                    if depth == 0 && seen_brace {
                        self.bump();
                        break;
                    }
                }
                _ => {}
            }
            self.bump();
        }
        Ok(None)
    }

    fn fn_decl(&mut self, is_priv: bool, start: Span) -> PResult<FnDecl> {
        let is_unsafe = self.eat(&TokenKind::Unsafe);
        self.expect(&TokenKind::Fn)?;
        let name = self.ident("a function name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        self.open(&TokenKind::LParen, false)?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            params.push(self.param()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.close(&TokenKind::RParen)?;
        let ret = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
        let where_clause = if self.at(&TokenKind::Where) { self.where_clause()? } else { Vec::new() };
        let body = self.block()?;
        let span = start.to(body.span);
        Ok(FnDecl { is_priv, is_unsafe, name, generics, params, ret, where_clause, body, span })
    }

    fn param(&mut self) -> PResult<Param> {
        let start = self.span();
        let convention = if self.eat(&TokenKind::Mut) {
            Convention::Mut
        } else if self.eat(&TokenKind::Own) {
            Convention::Own
        } else {
            Convention::Read
        };
        if self.eat(&TokenKind::SelfValue) {
            return Ok(Param { convention, kind: ParamKind::SelfParam, span: start.to(self.prev_span()) });
        }
        let name = self.ident("a parameter name")?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.ty()?;
        Ok(Param { convention, kind: ParamKind::Named { name, ty }, span: start.to(self.prev_span()) })
    }

    fn generic_params(&mut self) -> PResult<Vec<GenericParam>> {
        self.open(&TokenKind::Lt, false)?;
        let mut params = Vec::new();
        loop {
            if self.eat_closing_angle() {
                break;
            }
            let name = self.ident("a generic parameter name")?;
            let bounds = if self.eat(&TokenKind::Colon) { self.bounds()? } else { Vec::new() };
            params.push(GenericParam { name, bounds });
            if !self.eat(&TokenKind::Comma) {
                if !self.eat_closing_angle() {
                    return Err(self.unexpected("`,` or `>`"));
                }
                break;
            }
        }
        self.nl_stack.pop();
        Ok(params)
    }

    fn bounds(&mut self) -> PResult<Vec<Type>> {
        let mut bounds = vec![self.ty()?];
        while self.eat(&TokenKind::Plus) {
            bounds.push(self.ty()?);
        }
        Ok(bounds)
    }

    fn where_clause(&mut self) -> PResult<Vec<WherePred>> {
        self.expect(&TokenKind::Where)?;
        let mut preds = Vec::new();
        loop {
            let ty = self.ty()?;
            self.expect(&TokenKind::Colon)?;
            let bounds = self.bounds()?;
            preds.push(WherePred { ty, bounds });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(preds)
    }

    // ----- types -----------------------------------------------------------

    fn ty(&mut self) -> PResult<Type> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Ident(_) => {
                let mut segments = vec![self.path_segment()?];
                while self.at(&TokenKind::Dot) && matches!(self.nth_kind(1), TokenKind::Ident(_)) {
                    self.bump();
                    segments.push(self.path_segment()?);
                }
                Ok(Type { kind: TypeKind::Path(segments), span: start.to(self.prev_span()) })
            }
            TokenKind::SelfType => {
                self.bump();
                Ok(Type { kind: TypeKind::SelfType, span: start })
            }
            TokenKind::LParen => {
                self.open(&TokenKind::LParen, false)?;
                if self.at(&TokenKind::RParen) {
                    self.close(&TokenKind::RParen)?;
                    return Ok(Type { kind: TypeKind::Unit, span: start.to(self.prev_span()) });
                }
                let first = self.ty()?;
                if !self.at(&TokenKind::Comma) {
                    // `(T)` is just `T`.
                    self.close(&TokenKind::RParen)?;
                    return Ok(first);
                }
                let mut elems = vec![first];
                while self.eat(&TokenKind::Comma) {
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    elems.push(self.ty()?);
                }
                self.close(&TokenKind::RParen)?;
                Ok(Type { kind: TypeKind::Tuple(elems), span: start.to(self.prev_span()) })
            }
            TokenKind::LBracket => {
                self.open(&TokenKind::LBracket, false)?;
                let elem = self.ty()?;
                let kind = if self.eat(&TokenKind::Semi) {
                    let len = self.expr()?;
                    TypeKind::Array(Box::new(elem), Box::new(len))
                } else {
                    TypeKind::Slice(Box::new(elem))
                };
                self.close(&TokenKind::RBracket)?;
                Ok(Type { kind, span: start.to(self.prev_span()) })
            }
            TokenKind::Amp => {
                self.bump();
                let mutable = self.eat(&TokenKind::Mut);
                let inner = self.ty()?;
                let span = start.to(inner.span);
                Ok(Type { kind: TypeKind::Ref { mutable, ty: Box::new(inner) }, span })
            }
            TokenKind::Bang => {
                self.bump();
                Ok(Type { kind: TypeKind::Never, span: start })
            }
            TokenKind::Reserved(_) => {
                self.ident("a type")?;
                unreachable!()
            }
            _ => Err(self.unexpected("a type")),
        }
    }

    fn path_segment(&mut self) -> PResult<PathSegment> {
        let name = self.ident("a type name")?;
        let mut args = Vec::new();
        if self.at(&TokenKind::Lt) {
            self.open(&TokenKind::Lt, false)?;
            loop {
                if self.eat_closing_angle() {
                    break;
                }
                args.push(self.ty()?);
                if !self.eat(&TokenKind::Comma) {
                    if !self.eat_closing_angle() {
                        return Err(self.unexpected("`,` or `>`"));
                    }
                    break;
                }
            }
            self.nl_stack.pop();
        }
        Ok(PathSegment { name, args })
    }

    // ----- statements ------------------------------------------------------

    fn block(&mut self) -> PResult<Block> {
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

    fn stmt(&mut self) -> PResult<Stmt> {
        if self.at(&TokenKind::Let) {
            let start = self.bump().span;
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

    fn pattern(&mut self) -> PResult<Pattern> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Underscore => {
                self.bump();
                Ok(Pattern { kind: PatternKind::Wild, span: start })
            }
            TokenKind::Mut => {
                self.bump();
                let name = self.ident("a name")?;
                let span = start.to(name.span);
                Ok(Pattern { kind: PatternKind::Binding { name, mutable: true }, span })
            }
            TokenKind::Ident(_) => {
                let name = self.ident("a name")?;
                if matches!(self.kind(), TokenKind::Dot | TokenKind::LParen | TokenKind::LBrace | TokenKind::At) {
                    return Err(self.unsupported(start, "this pattern"));
                }
                let span = name.span;
                Ok(Pattern { kind: PatternKind::Binding { name, mutable: false }, span })
            }
            TokenKind::LParen => {
                self.open(&TokenKind::LParen, false)?;
                let mut elems = Vec::new();
                let mut trailing_comma = false;
                while !self.at(&TokenKind::RParen) {
                    elems.push(self.pattern()?);
                    trailing_comma = self.eat(&TokenKind::Comma);
                    if !trailing_comma {
                        break;
                    }
                }
                self.close(&TokenKind::RParen)?;
                let span = start.to(self.prev_span());
                if elems.len() == 1 && !trailing_comma {
                    return Ok(elems.pop().unwrap());
                }
                Ok(Pattern { kind: PatternKind::Tuple(elems), span })
            }
            TokenKind::Reserved(_) => {
                self.ident("a pattern")?;
                unreachable!()
            }
            TokenKind::Int(_)
            | TokenKind::Minus
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::True
            | TokenKind::False => Err(self.unsupported(start, "this pattern")),
            _ => Err(self.unexpected("a pattern")),
        }
    }

    // ----- expressions -----------------------------------------------------

    pub fn expr(&mut self) -> PResult<Expr> {
        self.expr_r(Restriction::None)
    }

    fn expr_r(&mut self, r: Restriction) -> PResult<Expr> {
        let lhs = self.orelse(r)?;
        if self.at(&TokenKind::Eq) && !self.nl() {
            self.bump();
            let rhs = self.expr_r(r)?;
            let span = lhs.span.to(rhs.span);
            return Ok(Expr { kind: ExprKind::Assign(Box::new(lhs), Box::new(rhs)), span });
        }
        Ok(lhs)
    }

    fn orelse(&mut self, r: Restriction) -> PResult<Expr> {
        let mut lhs = self.range(r)?;
        while self.at(&TokenKind::Orelse) {
            self.bump();
            let rhs = self.range(r)?;
            let span = lhs.span.to(rhs.span);
            lhs = Expr { kind: ExprKind::OrElse(Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    fn range(&mut self, r: Restriction) -> PResult<Expr> {
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
    fn binop(&self) -> Option<(BinOp, u8)> {
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

    fn binary(&mut self, min: u8, r: Restriction) -> PResult<Expr> {
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

    fn cast(&mut self, r: Restriction) -> PResult<Expr> {
        let mut e = self.unary(r)?;
        while self.at(&TokenKind::As) {
            self.bump();
            let ty = self.ty()?;
            let span = e.span.to(ty.span);
            e = Expr { kind: ExprKind::Cast(Box::new(e), ty), span };
        }
        Ok(e)
    }

    fn unary(&mut self, r: Restriction) -> PResult<Expr> {
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

    fn postfix(&mut self, r: Restriction) -> PResult<Expr> {
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
                        let mut_marker = self.eat(&TokenKind::Mut);
                        let expr = self.expr()?;
                        args.push(Arg { mut_marker, expr });
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

    fn can_start_expr(kind: &TokenKind) -> bool {
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

    fn primary(&mut self, r: Restriction) -> PResult<Expr> {
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
            TokenKind::Ident(_) => {
                let name = self.ident("an expression")?;
                if self.at(&TokenKind::LBrace)
                    && !self.nl()
                    && r == Restriction::None
                    && looks_like_struct_literal(self)
                {
                    return Err(self.unsupported(start, "struct literals"));
                }
                Ok(Expr { span: name.span, kind: ExprKind::Name(name) })
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
            TokenKind::Match => Err(self.unsupported(start, "`match`")),
            TokenKind::Pipe | TokenKind::OrOr => Err(self.error_here("closures are not part of stage 0".into())),
            TokenKind::Reserved(_) => {
                self.ident("an expression")?;
                unreachable!()
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&TokenKind::If)?.span;
        if self.at(&TokenKind::Let) {
            return Err(self.unsupported(start, "`if let`"));
        }
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
        Ok(Expr { kind: ExprKind::If { cond: Box::new(cond), then_block, else_expr }, span })
    }
}

const LEVEL_OR: u8 = 4;
const LEVEL_CMP: u8 = 6;

/// `Name {` starts a struct literal when the brace is followed by `}` or by `ident` and then `:`, `,`, `}` or a
/// line break. A block after a name (as in `while x { ... }`) is excluded by the restriction instead.
fn looks_like_struct_literal(p: &Parser<'_>) -> bool {
    match p.nth_kind(1) {
        TokenKind::RBrace => true,
        TokenKind::Ident(_) => matches!(p.nth_kind(2), TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace),
        _ => false,
    }
}
