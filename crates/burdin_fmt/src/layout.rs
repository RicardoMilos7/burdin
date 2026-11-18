//! Layouts shared by many constructs: comma lists in brackets, and lists of members in braces.

use crate::cursor::{Closer, D, Fmt, Parts, R};
use crate::doc::{Doc, broken_group, concat, group, if_break, indent, text};
use burdin_syntax::token::TokenKind;

/// One element of a comma list: its document and the comments that followed its comma.
pub struct Elem {
    pub doc: D,
    pub after: D,
}

/// A bracketed comma list as read from the source.
pub struct Delimited {
    pub open: Parts,
    pub elems: Vec<Elem>,
    /// A comma followed the last element.
    pub trailing_comma: bool,
    /// The magic trailing comma (spec/fmt.md, FM8): the list had a trailing comma and its closing bracket was on a
    /// later line than the opening one, so it stays one element per line.
    pub keep_broken: bool,
    pub close: Closer,
}

/// One entry of a brace list: the document, the comments after its comma, and whether the source had a blank line
/// before it.
pub struct Member {
    pub doc: D,
    pub after: D,
    pub blank_before: bool,
}

impl Fmt<'_> {
    /// Reads `open item, item, ... close`, where newlines inside the brackets do not matter. Each item is read by
    /// `item`.
    pub fn delimited(
        &mut self,
        open: &TokenKind,
        close: &TokenKind,
        mut item: impl FnMut(&mut Self) -> R<D>,
    ) -> R<Delimited> {
        let open = self.open(open, false)?;
        let mut elems = Vec::new();
        let mut trailing_comma = false;
        while !self.at(close) {
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
        let close = self.close(close)?;
        Ok(Delimited { open, elems, trailing_comma, keep_broken, close })
    }

    /// The comma list as a group: on one line when it fits, otherwise one element per line with a trailing comma.
    /// `always_comma` keeps the comma after the last element on one line too (a tuple of one element).
    pub fn list_doc(&mut self, d: Delimited, always_comma: bool) -> D {
        let Delimited { open, elems, close, keep_broken, .. } = d;
        let dangling = self.dangling(&close.lead, !elems.is_empty());
        let has_dangling = !close.lead.is_empty();
        if elems.is_empty() && !has_dangling {
            return concat(vec![open.lead, open.text, close.text, open.trail, close.trail]);
        }
        let mut inner = Vec::new();
        let last = elems.len().saturating_sub(1);
        for (i, e) in elems.into_iter().enumerate() {
            inner.push(e.doc);
            if i == last {
                inner.push(if always_comma { text(",") } else { if_break(text(","), Doc::Nil) });
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
            indent(concat(vec![Doc::Soft, concat(inner), dangling])),
            Doc::Soft,
            close.text,
        ]);
        concat(vec![open.lead, if keep_broken { broken_group(body) } else { group(body) }, close.trail])
    }

    /// A comma list with no trailing comma allowed (a `where` clause): joined with `, `, breaking one per line.
    pub fn plain_join(&self, docs: Vec<D>, sep: &str) -> D {
        let mut parts = Vec::new();
        let last = docs.len().saturating_sub(1);
        for (i, doc) in docs.into_iter().enumerate() {
            parts.push(doc);
            if i != last {
                parts.push(text(sep));
                parts.push(Doc::Line);
            }
        }
        concat(parts)
    }

    /// Entries between braces, one per line. `punct` follows each entry (`,` for fields and variants, nothing for
    /// methods). Blank lines the source had between entries stay, one at most.
    pub fn members_doc(&mut self, open: Parts, members: Vec<Member>, close: Closer, punct: &str) -> D {
        let dangling = self.dangling(&close.lead, !members.is_empty());
        let has_dangling = !close.lead.is_empty();
        if members.is_empty() && !has_dangling && open.trail.is_nil() {
            return concat(vec![open.lead, open.text, close.text, close.trail]);
        }
        let mut inner = Vec::new();
        for (i, m) in members.into_iter().enumerate() {
            inner.push(Doc::Hard);
            if i > 0 && m.blank_before {
                inner.push(Doc::Hard);
            }
            inner.push(m.doc);
            inner.push(text(punct));
            inner.push(m.after);
        }
        inner.push(dangling);
        concat(vec![open.lead, open.text, open.trail, indent(concat(inner)), Doc::Hard, close.text, close.trail])
    }

    /// Entries between braces that fit on one line when they are short: `{ a, b }`, as a struct literal or a
    /// struct pattern. Broken, one per line with a trailing comma.
    pub fn inline_braces_doc(&mut self, open: Parts, elems: Vec<Elem>, keep_broken: bool, close: Closer) -> D {
        let dangling = self.dangling(&close.lead, !elems.is_empty());
        let has_dangling = !close.lead.is_empty();
        if elems.is_empty() && !has_dangling {
            return concat(vec![open.lead, open.text, close.text, open.trail, close.trail]);
        }
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
            indent(concat(vec![Doc::Line, concat(inner), dangling])),
            Doc::Line,
            close.text,
        ]);
        concat(vec![open.lead, if keep_broken { broken_group(body) } else { group(body) }, close.trail])
    }

    /// Reads the entries of a brace list: `Name`, `Name: value`, ... separated by `,` or a line break, closing at
    /// `}`. Each entry is read by `entry`; the comma, when there is one, is dropped and its comments kept.
    pub fn brace_entries(&mut self, mut entry: impl FnMut(&mut Self) -> R<D>) -> R<(Vec<Member>, Closer)> {
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let blank_before = self.blank_before();
            let doc = entry(self)?;
            let after = if self.at(&TokenKind::Comma) { self.dropped() } else { Doc::Nil };
            members.push(Member { doc, after, blank_before });
        }
        let close = self.close(&TokenKind::RBrace)?;
        Ok((members, close))
    }
}
