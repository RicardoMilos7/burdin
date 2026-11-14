//! Patterns.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

impl Parser<'_> {
    pub(super) fn pattern(&mut self) -> PResult<Pattern> {
        let first = self.at_pattern()?;
        if !self.at(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(&TokenKind::Pipe) {
            alts.push(self.at_pattern()?);
        }
        let span = alts[0].span.to(alts[alts.len() - 1].span);
        Ok(Pattern { kind: PatternKind::Or(alts), span })
    }

    pub(super) fn at_pattern(&mut self) -> PResult<Pattern> {
        if matches!(self.kind(), TokenKind::Ident(_)) && matches!(self.nth_kind(1), TokenKind::At) {
            let name = self.ident("a name")?;
            self.bump();
            let inner = self.primary_pattern()?;
            let span = name.span.to(inner.span);
            return Ok(Pattern { kind: PatternKind::At(name, Box::new(inner)), span });
        }
        self.primary_pattern()
    }

    pub(super) fn lit_pattern(&mut self) -> PResult<LitPat> {
        let negative = self.eat(&TokenKind::Minus);
        let lit = match self.kind().clone() {
            TokenKind::Int(value) => LitPat::Int { value, negative },
            _ if negative => return Err(self.unexpected("an integer literal")),
            TokenKind::Char(c) => LitPat::Char(c),
            TokenKind::Str(s) => LitPat::Str(s),
            TokenKind::True => LitPat::Bool(true),
            TokenKind::False => LitPat::Bool(false),
            _ => return Err(self.unexpected("a literal")),
        };
        self.bump();
        Ok(lit)
    }

    pub(super) fn primary_pattern(&mut self) -> PResult<Pattern> {
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
                let mut path = vec![self.ident("a name")?];
                while self.at(&TokenKind::Dot) {
                    self.bump();
                    path.push(self.ident("a name")?);
                }
                if self.at(&TokenKind::LParen) {
                    self.open(&TokenKind::LParen, false)?;
                    let mut elems = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        elems.push(self.pattern()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.close(&TokenKind::RParen)?;
                    let span = start.to(self.prev_span());
                    return Ok(Pattern { kind: PatternKind::TupleStruct(path, elems), span });
                }
                if self.at(&TokenKind::LBrace) && !self.nl() {
                    self.open(&TokenKind::LBrace, true)?;
                    let mut fields = Vec::new();
                    while !self.at(&TokenKind::RBrace) {
                        let name = self.ident("a field name")?;
                        let pattern = if self.eat(&TokenKind::Colon) { Some(self.pattern()?) } else { None };
                        fields.push(FieldPat { name, pattern });
                        if !self.eat(&TokenKind::Comma) && !self.tok().nl_before {
                            break;
                        }
                    }
                    self.close(&TokenKind::RBrace)?;
                    let span = start.to(self.prev_span());
                    return Ok(Pattern { kind: PatternKind::Struct(path, fields), span });
                }
                let span = start.to(self.prev_span());
                if path.len() > 1 {
                    return Ok(Pattern { kind: PatternKind::Path(path), span });
                }
                let name = path.pop().expect("a path has at least one segment");
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
                    return Ok(elems.pop().expect("one element"));
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
            | TokenKind::False => {
                let lo = self.lit_pattern()?;
                if self.eat(&TokenKind::DotDotEq) {
                    let hi = self.lit_pattern()?;
                    let span = start.to(self.prev_span());
                    return Ok(Pattern { kind: PatternKind::Range(lo, hi), span });
                }
                if self.at(&TokenKind::DotDot) {
                    return Err(self.error_here("range patterns must be inclusive (`..=`)".into()));
                }
                Ok(Pattern { kind: PatternKind::Lit(lo), span: start.to(self.prev_span()) })
            }
            _ => Err(self.unexpected("a pattern")),
        }
    }
}
