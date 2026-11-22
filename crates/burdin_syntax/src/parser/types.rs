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
            // `Self`, or `Self.Name` for an associated type of the interface or impl block
            // (`spec/associated-types.md`, AT3).
            TokenKind::SelfType => {
                self.bump();
                if self.at(&TokenKind::Dot) && matches!(self.nth_kind(1), TokenKind::Ident(_)) {
                    self.bump();
                    let name = self.ident("an associated type name")?;
                    let span = start.to(name.span);
                    let self_ty = Type { kind: TypeKind::SelfType, span: start };
                    return Ok(Type {
                        kind: TypeKind::Projection(Box::new(ProjectionType {
                            self_ty: Box::new(self_ty),
                            iface: None,
                            name,
                        })),
                        span,
                    });
                }
                Ok(Type { kind: TypeKind::SelfType, span: start })
            }
            // `<T as Interface>.Name`: a qualified associated type projection.
            TokenKind::Lt => {
                self.open(&TokenKind::Lt, false)?;
                let self_ty = self.ty()?;
                self.expect(&TokenKind::As)?;
                let iface = self.ty()?;
                if !self.eat_closing_angle() {
                    return Err(self.unexpected("`>`"));
                }
                self.nl_stack.pop();
                self.expect(&TokenKind::Dot)?;
                let name = self.ident("an associated type name")?;
                let span = start.to(name.span);
                Ok(Type {
                    kind: TypeKind::Projection(Box::new(ProjectionType {
                        self_ty: Box::new(self_ty),
                        iface: Some(Box::new(iface)),
                        name,
                    })),
                    span,
                })
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

    /// Grammar rule 8.2 (`crate::lookahead::generic_args_ahead`).
    pub(super) fn generic_args_ahead(&self) -> bool {
        crate::lookahead::generic_args_ahead(&self.toks, self.pos)
    }

    /// One generic argument: a type, or a const argument (an integer literal or a `{ expr }`;
    /// `spec/const-generics.md`, CG3).
    pub(super) fn generic_arg(&mut self) -> PResult<Type> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::Int(n) => {
                self.bump();
                let e = Expr { kind: ExprKind::Int(n), span: start };
                Ok(Type { kind: TypeKind::Const(Box::new(e)), span: start })
            }
            TokenKind::Minus if matches!(self.nth_kind(1), TokenKind::Int(_)) => {
                self.bump();
                let TokenKind::Int(n) = self.kind().clone() else { unreachable!("checked above") };
                let lit = self.bump().span;
                let inner = Expr { kind: ExprKind::Int(n), span: lit };
                let e = Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(inner)), span: start.to(lit) };
                Ok(Type { kind: TypeKind::Const(Box::new(e)), span: start.to(lit) })
            }
            kind @ (TokenKind::True | TokenKind::False) => {
                self.bump();
                let e = Expr { kind: ExprKind::Bool(kind == TokenKind::True), span: start };
                Ok(Type { kind: TypeKind::Const(Box::new(e)), span: start })
            }
            TokenKind::Char(c) => {
                self.bump();
                let e = Expr { kind: ExprKind::Char(c), span: start };
                Ok(Type { kind: TypeKind::Const(Box::new(e)), span: start })
            }
            TokenKind::LBrace => {
                self.open(&TokenKind::LBrace, false)?;
                let e = self.expr()?;
                let close = self.close(&TokenKind::RBrace)?;
                Ok(Type { kind: TypeKind::Const(Box::new(e)), span: start.to(close.span) })
            }
            _ => self.ty(),
        }
    }

    /// Parses `<` Type { `,` Type } `>` after the identifier of a path expression.
    pub(super) fn expr_generic_args(&mut self) -> PResult<Vec<Type>> {
        self.open(&TokenKind::Lt, false)?;
        let mut args = Vec::new();
        loop {
            if self.eat_closing_angle() {
                break;
            }
            args.push(self.generic_arg()?);
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
                args.push(self.generic_arg()?);
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
