//! Types, generic arguments and parameters, and patterns (spec/grammar.md, sections 4 and 7).

use crate::cursor::{D, Fmt, R};
use crate::doc::{Doc, broken_group, concat, group, if_break, indent, space, text};
use crate::layout::Elem;
use burdin_syntax::token::TokenKind;

impl Fmt<'_> {
    pub fn ty(&mut self) -> R<D> {
        match self.kind().clone() {
            TokenKind::Ident(name) => {
                let closure = matches!(name.as_str(), "Fn" | "FnMut" | "FnOnce") && self.nth(1) == &TokenKind::LParen;
                if closure {
                    let head = self.tok_doc();
                    let params = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.ty())?;
                    let params = self.list_doc(params, false);
                    let mut parts = vec![head, params];
                    if self.at(&TokenKind::Colon) {
                        parts.push(self.tok_doc());
                        parts.push(space());
                        parts.push(self.ty()?);
                    }
                    return Ok(concat(parts));
                }
                self.type_path()
            }
            // `Self` or `Self.Name`: an associated type of the enclosing interface or impl block
            // (`spec/associated-types.md`, AT3).
            TokenKind::SelfType => {
                let mut parts = vec![self.tok_doc()];
                if self.at(&TokenKind::Dot) && matches!(self.nth(1), TokenKind::Ident(_)) {
                    parts.push(self.tok_doc());
                    parts.push(self.ident()?);
                }
                Ok(concat(parts))
            }
            // `<T as Interface>.Name`: a qualified associated type projection.
            TokenKind::Lt => {
                let open = self.open(&TokenKind::Lt, false)?;
                let mut parts = vec![open.lead, open.text, self.ty()?];
                parts.push(space());
                parts.push(self.take(&TokenKind::As)?);
                parts.push(space());
                parts.push(self.ty()?);
                let close = self.closing_angle()?;
                parts.push(close.lead);
                parts.push(close.text);
                parts.push(close.trail);
                parts.push(self.take(&TokenKind::Dot)?);
                parts.push(self.ident()?);
                Ok(concat(parts))
            }
            TokenKind::Any | TokenKind::Impl => {
                let kw = self.tok_doc();
                let inner = self.ty()?;
                Ok(concat(vec![kw, space(), inner]))
            }
            TokenKind::LParen => {
                let d = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.ty())?;
                if d.elems.len() == 1 && !d.trailing_comma {
                    // `(T)` is `T`: a comma after it would make a tuple.
                    return Ok(self.paren_doc(d));
                }
                let single_with_comma = d.elems.len() == 1;
                Ok(self.list_doc(d, single_with_comma))
            }
            TokenKind::LBracket => {
                let open = self.open(&TokenKind::LBracket, false)?;
                let elem = self.ty()?;
                let mut parts = vec![open.lead, open.text, open.trail, elem];
                if self.at(&TokenKind::Semi) {
                    parts.push(self.tok_doc());
                    parts.push(space());
                    parts.push(self.expr()?.doc);
                }
                let close = self.close(&TokenKind::RBracket)?;
                let dangling = self.dangling(&close.lead, true);
                parts.push(dangling);
                parts.push(close.text);
                parts.push(close.trail);
                Ok(concat(parts))
            }
            TokenKind::Amp => {
                let amp = self.tok_doc();
                let mut parts = vec![amp];
                if self.at(&TokenKind::Mut) {
                    parts.push(self.tok_doc());
                    parts.push(space());
                }
                parts.push(self.ty()?);
                Ok(concat(parts))
            }
            TokenKind::Bang => Ok(self.tok_doc()),
            _ => Err(self.error("expected a type")),
        }
    }

    /// `Name<args>.Name<args>`.
    pub fn type_path(&mut self) -> R<D> {
        let mut parts = vec![self.path_segment()?];
        while self.at(&TokenKind::Dot) && matches!(self.nth(1), TokenKind::Ident(_)) {
            parts.push(self.tok_doc());
            parts.push(self.path_segment()?);
        }
        Ok(concat(parts))
    }

    pub fn path_segment(&mut self) -> R<D> {
        let name = self.ident()?;
        if self.at(&TokenKind::Lt) {
            let args = self.angle_args()?;
            Ok(concat(vec![name, args]))
        } else {
            Ok(name)
        }
    }

    pub fn ident(&mut self) -> R<D> {
        match self.kind() {
            TokenKind::Ident(_) => Ok(self.tok_doc()),
            _ => Err(self.error("expected a name")),
        }
    }

    pub fn is_closing_angle(&self) -> bool {
        matches!(self.kind(), TokenKind::Gt | TokenKind::Shr | TokenKind::Ge)
    }

    /// `<A, B>`: generic arguments after a name.
    pub fn angle_args(&mut self) -> R<D> {
        self.angle_list(|f| f.generic_arg())
    }

    /// `<` items `>`, where the items are read by `item`.
    fn angle_list(&mut self, mut item: impl FnMut(&mut Self) -> R<D>) -> R<D> {
        let open = self.open(&TokenKind::Lt, false)?;
        let mut elems = Vec::new();
        let mut trailing_comma = false;
        while !self.is_closing_angle() {
            let doc = item(self)?;
            if self.at(&TokenKind::Comma) {
                let after = self.dropped();
                elems.push(Elem { doc, after });
                trailing_comma = true;
            } else {
                elems.push(Elem { doc, after: Doc::Nil });
                trailing_comma = false;
                break;
            }
        }
        let keep_broken = trailing_comma && self.tok().nl_before;
        let close = self.closing_angle()?;
        Ok(self.angle_doc(open, elems, keep_broken, close))
    }

    fn angle_doc(
        &mut self,
        open: crate::cursor::Parts,
        elems: Vec<Elem>,
        keep_broken: bool,
        close: crate::cursor::Parts,
    ) -> D {
        let mut inner = Vec::new();
        let last = elems.len().saturating_sub(1);
        for (i, e) in elems.into_iter().enumerate() {
            inner.push(e.doc);
            if i == last {
                inner.push(if_break(text(","), Doc::Nil));
                inner.push(e.after);
            } else {
                inner.push(text(","));
                inner.push(e.after);
                inner.push(Doc::Line);
            }
        }
        let body = concat(vec![
            open.text,
            open.trail,
            indent(concat(vec![Doc::Soft, concat(inner)])),
            Doc::Soft,
            close.lead,
            close.text,
        ]);
        concat(vec![open.lead, if keep_broken { broken_group(body) } else { group(body) }, close.trail])
    }

    /// One generic argument: a type, or a const argument (an integer, `-` integer, `true`, `false`, a character, or
    /// `{ expr }`).
    pub fn generic_arg(&mut self) -> R<D> {
        match self.kind() {
            TokenKind::Int(_) | TokenKind::True | TokenKind::False | TokenKind::Char(_) => Ok(self.tok_doc()),
            TokenKind::Minus if matches!(self.nth(1), TokenKind::Int(_)) => {
                let minus = self.tok_doc();
                let int = self.tok_doc();
                Ok(concat(vec![minus, int]))
            }
            TokenKind::LBrace => {
                let open = self.open(&TokenKind::LBrace, false)?;
                let e = self.expr()?;
                let close = self.close(&TokenKind::RBrace)?;
                let dangling = self.dangling(&close.lead, true);
                Ok(concat(vec![
                    open.lead,
                    open.text,
                    open.trail,
                    space(),
                    e.doc,
                    dangling,
                    space(),
                    close.text,
                    close.trail,
                ]))
            }
            _ => self.ty(),
        }
    }

    /// `<T: A + B, const N: usize>`: the generic parameters of a declaration.
    pub fn generic_params(&mut self) -> R<D> {
        self.angle_list(|f| f.generic_param())
    }

    fn generic_param(&mut self) -> R<D> {
        let mut parts = Vec::new();
        if self.at(&TokenKind::Const) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ident()?);
            parts.push(self.take(&TokenKind::Colon)?);
            parts.push(space());
            parts.push(self.ty()?);
            if self.at(&TokenKind::Eq) {
                parts.push(space());
                parts.push(self.tok_doc());
                parts.push(space());
                parts.push(self.generic_arg()?);
            }
            return Ok(concat(parts));
        }
        parts.push(self.ident()?);
        if self.at(&TokenKind::Colon) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.bounds()?);
        }
        if self.at(&TokenKind::Eq) {
            parts.push(space());
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        Ok(concat(parts))
    }

    /// `A + B`.
    pub fn bounds(&mut self) -> R<D> {
        let mut parts = vec![self.ty()?];
        while self.at(&TokenKind::Plus) {
            parts.push(space());
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        Ok(concat(parts))
    }

    /// `where T: A, U: B`, as the document of the whole clause: on the line of the signature when it fits, otherwise
    /// `where` on a line of its own with one predicate per line. `body` is what follows (` {`).
    pub fn where_clause(&mut self) -> R<D> {
        let kw = self.tok_doc();
        let mut preds = Vec::new();
        loop {
            let ty = self.ty()?;
            let colon = self.take(&TokenKind::Colon)?;
            let bounds = self.bounds()?;
            let mut pred = concat(vec![ty, colon, space(), bounds]);
            if self.at(&TokenKind::Comma) {
                let after = self.dropped();
                pred = concat(vec![pred, after]);
                preds.push(pred);
            } else {
                preds.push(pred);
                break;
            }
        }
        let joined = self.plain_join(preds, ",");
        let end = if self.at(&TokenKind::LBrace) { Doc::Line } else { Doc::Nil };
        Ok(group(concat(vec![Doc::Line, kw, indent(concat(vec![Doc::Line, joined])), end])))
    }

    // ----- patterns --------------------------------------------------------------------------------------------

    pub fn pattern(&mut self) -> R<D> {
        let first = self.at_pattern()?;
        if !self.at(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut rest = Vec::new();
        while self.at(&TokenKind::Pipe) {
            let pipe = self.tok_doc();
            let alt = self.at_pattern()?;
            rest.push(concat(vec![Doc::Line, pipe, space(), alt]));
        }
        Ok(group(concat(vec![first, indent(concat(rest))])))
    }

    fn at_pattern(&mut self) -> R<D> {
        if matches!(self.kind(), TokenKind::Ident(_)) && self.nth(1) == &TokenKind::At {
            let name = self.tok_doc();
            let at = self.tok_doc();
            let inner = self.primary_pattern()?;
            return Ok(concat(vec![name, at, inner]));
        }
        self.primary_pattern()
    }

    fn lit_pattern(&mut self) -> R<D> {
        let mut parts = Vec::new();
        if self.at(&TokenKind::Minus) {
            parts.push(self.tok_doc());
        }
        match self.kind() {
            TokenKind::Int(_) | TokenKind::Char(_) | TokenKind::Str(_) | TokenKind::True | TokenKind::False => {
                parts.push(self.tok_doc())
            }
            _ => return Err(self.error("expected a literal")),
        }
        Ok(concat(parts))
    }

    fn primary_pattern(&mut self) -> R<D> {
        match self.kind() {
            TokenKind::Underscore => Ok(self.tok_doc()),
            TokenKind::Mut => {
                let kw = self.tok_doc();
                let name = self.ident()?;
                Ok(concat(vec![kw, space(), name]))
            }
            TokenKind::Ident(_) | TokenKind::SelfType => {
                let mut parts = vec![self.tok_doc()];
                while self.at(&TokenKind::Dot) {
                    parts.push(self.tok_doc());
                    parts.push(self.ident()?);
                }
                let path = concat(parts);
                if self.at(&TokenKind::LParen) {
                    let list = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.pattern())?;
                    let list = self.list_doc(list, false);
                    return Ok(concat(vec![path, list]));
                }
                if self.at(&TokenKind::LBrace) && !self.nl() {
                    let braces = self.struct_pattern_fields()?;
                    return Ok(concat(vec![path, space(), braces]));
                }
                Ok(path)
            }
            TokenKind::LParen => {
                let list = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.pattern())?;
                if list.elems.len() == 1 && !list.trailing_comma {
                    return Ok(self.paren_doc(list));
                }
                let single_with_comma = list.elems.len() == 1;
                Ok(self.list_doc(list, single_with_comma))
            }
            TokenKind::Int(_)
            | TokenKind::Minus
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::True
            | TokenKind::False => {
                let lo = self.lit_pattern()?;
                if self.at(&TokenKind::DotDotEq) {
                    let op = self.tok_doc();
                    let hi = self.lit_pattern()?;
                    return Ok(concat(vec![lo, op, hi]));
                }
                Ok(lo)
            }
            _ => Err(self.error("expected a pattern")),
        }
    }

    /// `{ a, b: pattern }`: fields separated by `,` or a line break.
    fn struct_pattern_fields(&mut self) -> R<D> {
        let open = self.open(&TokenKind::LBrace, true)?;
        let mut elems = Vec::new();
        let mut trailing_comma = false;
        while !self.at(&TokenKind::RBrace) {
            let name = self.ident()?;
            let mut doc = name;
            if self.at(&TokenKind::Colon) {
                let colon = self.tok_doc();
                let p = self.pattern()?;
                doc = concat(vec![doc, colon, space(), p]);
            }
            if self.at(&TokenKind::Comma) {
                let after = self.dropped();
                elems.push(Elem { doc, after });
                trailing_comma = true;
            } else {
                elems.push(Elem { doc, after: Doc::Nil });
                trailing_comma = false;
                if !self.tok().nl_before {
                    break;
                }
            }
        }
        let keep_broken = trailing_comma && self.tok().nl_before;
        let close = self.close(&TokenKind::RBrace)?;
        Ok(self.inline_braces_doc(open, elems, keep_broken, close))
    }
}
