//! Types, and the generic arguments that can follow a name.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn ty(&mut self) -> PResult<Type> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Ident(name) => {
                // `Fn(...)`, `FnMut(...)`, and `FnOnce(...)` are the compiler-known closure types. Only the
                // parenthesized form is special, so a type named `Fn` still works.
                let kind = match name.as_str() {
                    "Fn" => Some(FnKind::Fn),
                    "FnMut" => Some(FnKind::FnMut),
                    "FnOnce" => Some(FnKind::FnOnce),
                    _ => None,
                };
                if let Some(kind) = kind
                    && self.nth_kind(1) == &TokenKind::LParen
                {
                    self.bump();
                    self.open(&TokenKind::LParen, false)?;
                    let mut params = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        params.push(self.ty()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.close(&TokenKind::RParen)?;
                    let ret = if self.eat(&TokenKind::Colon) { Some(Box::new(self.ty()?)) } else { None };
                    return Ok(Type { kind: TypeKind::Fn { kind, params, ret }, span: start.to(self.prev_span()) });
                }
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
            // `any Interface` and `impl Interface`: the interface is written after the keyword, as a type so
            // that a wrong form gets a semantic diagnostic rather than a parse error.
            TokenKind::Any | TokenKind::Impl => {
                let any = self.at(&TokenKind::Any);
                self.bump();
                let inner = self.ty()?;
                let span = start.to(inner.span);
                let kind = if any { TypeKind::Any(Box::new(inner)) } else { TypeKind::Impl(Box::new(inner)) };
                Ok(Type { kind, span })
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

    /// Grammar rule 8.2: after an identifier that starts with an uppercase letter, a `<` starts generic arguments
    /// if a type list parses and its closing `>` is followed by `.`, `(`, or `{`. Otherwise the `<` is a comparison.
    /// The test is a scan ahead, so a comparison never leaves a `>>` split in its wake.
    pub(super) fn generic_args_ahead(&self) -> bool {
        let TokenKind::Ident(name) = self.kind() else { return false };
        if !name.starts_with(|c: char| c.is_ascii_uppercase()) || !matches!(self.nth_kind(1), TokenKind::Lt) {
            return false;
        }
        let continues = |i: usize| {
            matches!(self.toks.get(i).map(|t| &t.kind), Some(TokenKind::Dot | TokenKind::LParen | TokenKind::LBrace))
        };
        let mut depth = 0i32;
        // Parentheses and brackets inside a type (`Fn(i32)`, `(i32, i32)`) do not end the scan; only a closer
        // that is not matched inside it can belong to the surrounding expression.
        let mut parens = 0i32;
        let mut i = self.pos + 1;
        while let Some(t) = self.toks.get(i) {
            match &t.kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => {
                    depth -= 1;
                    if depth <= 0 {
                        return depth == 0 && continues(i + 1);
                    }
                }
                // `>>` can close two levels; `>=` can never close a generic argument list.
                TokenKind::Shr => {
                    depth -= 2;
                    if depth <= 0 {
                        return depth == 0 && continues(i + 1);
                    }
                }
                TokenKind::LParen | TokenKind::LBracket => parens += 1,
                TokenKind::RParen | TokenKind::RBracket => {
                    if parens == 0 {
                        return false;
                    }
                    parens -= 1;
                }
                TokenKind::Ge | TokenKind::Eof | TokenKind::RBrace if parens == 0 => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Parses `<` Type { `,` Type } `>` after the identifier of a path expression.
    pub(super) fn expr_generic_args(&mut self) -> PResult<Vec<Type>> {
        self.open(&TokenKind::Lt, false)?;
        let mut args = Vec::new();
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
        Ok(args)
    }

    pub(super) fn path_segment(&mut self) -> PResult<PathSegment> {
        let name = self.ident("a type name")?;
        if self.at(&TokenKind::Lt) { self.path_segment_args(name) } else { Ok(PathSegment { name, args: Vec::new() }) }
    }

    /// A path segment whose name is already read, with its `<...>` type arguments when they follow.
    pub(super) fn path_segment_args(&mut self, name: Ident) -> PResult<PathSegment> {
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
}
