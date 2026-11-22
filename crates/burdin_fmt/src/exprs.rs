//! Expressions (spec/grammar.md, section 6), by precedence, the same way the compiler's parser reads them.

use crate::cursor::{D, Fmt, R};
use crate::doc::{Doc, broken_group, concat, group, indent, space, text};
use crate::layout::{Delimited, Elem};
use burdin_syntax::lookahead;
use burdin_syntax::token::TokenKind;

/// An expression as a document, with what the layout of its parent needs to know about it.
pub struct Ex {
    pub doc: D,
    /// A chain of binary operators: the parent may break after `=` or `=>` to give it a line of its own.
    pub binary: bool,
    /// Ends with a block (`if`, `match`, a loop, a block): a match arm needs no comma after it.
    pub block_like: bool,
    /// Can overflow onto the line of a call: a closure, a struct literal, an array.
    pub hug: bool,
    /// A plain path (`a`, `a.b`), which a `{` after it may turn into a struct literal.
    pub path: bool,
}

impl Ex {
    pub fn plain(doc: D) -> Ex {
        Ex { doc, binary: false, block_like: false, hug: false, path: false }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Restriction {
    None,
    /// In the head of `if`, `while`, `for`, `match`: struct literals are not allowed.
    NoStructLiteral,
}

const LEVEL_OR: u8 = 4;
const LEVEL_AND: u8 = 5;
const LEVEL_CMP: u8 = 6;

/// One link of a postfix chain: `.name`, `(args)`, `[index]`, `?`.
struct Seg {
    doc: D,
    /// Starts with a `.`: a place where a long chain may break.
    dot: bool,
    /// Calls something or indexes.
    call: bool,
}

impl Fmt<'_> {
    pub fn expr(&mut self) -> R<Ex> {
        self.expr_r(Restriction::None)
    }

    /// The layout of the right side of `=`, `=>`, or a `let`: after a chain of binary operators it may move to a line
    /// of its own, anything else stays where it is and breaks inside itself.
    pub fn after_operator(&self, rhs: Ex) -> D {
        if rhs.binary {
            group(indent(concat(vec![Doc::Line, without_chain_indent(rhs.doc)])))
        } else {
            concat(vec![space(), rhs.doc])
        }
    }

    pub fn expr_r(&mut self, r: Restriction) -> R<Ex> {
        let lhs = self.orelse(r)?;
        if self.at(&TokenKind::Eq) && !self.nl() {
            let eq = self.tok_doc();
            let rhs = self.expr_r(r)?;
            let doc = concat(vec![lhs.doc, space(), eq, self.after_operator(rhs)]);
            return Ok(Ex::plain(doc));
        }
        Ok(lhs)
    }

    fn orelse(&mut self, r: Restriction) -> R<Ex> {
        let first = self.range(r)?;
        if !self.at(&TokenKind::Orelse) {
            return Ok(first);
        }
        let mut rest = Vec::new();
        while self.at(&TokenKind::Orelse) {
            let kw = self.tok_doc();
            let rhs = self.range(r)?;
            rest.push(concat(vec![Doc::Line, kw, space(), rhs.doc]));
        }
        Ok(Ex { binary: true, ..Ex::plain(group(concat(vec![first.doc, indent(concat(rest))]))) })
    }

    fn range(&mut self, r: Restriction) -> R<Ex> {
        let lo = self.binary(LEVEL_OR, r)?;
        if !matches!(self.kind(), TokenKind::DotDot | TokenKind::DotDotEq) || self.nl() {
            return Ok(lo);
        }
        let op = self.tok_doc();
        let hi = self.binary(LEVEL_OR, r)?;
        Ok(Ex::plain(concat(vec![lo.doc, op, hi.doc])))
    }

    /// The binary operator at the current position and its level, if it continues the expression.
    fn binop(&self) -> Option<u8> {
        use TokenKind as T;
        let level = match self.kind() {
            T::OrOr => LEVEL_OR,
            T::AndAnd => LEVEL_AND,
            T::EqEq | T::NotEq | T::Lt | T::Gt | T::Le | T::Ge => LEVEL_CMP,
            T::Pipe => 7,
            T::Caret => 8,
            T::Amp => 9,
            T::Shl | T::Shr => 10,
            T::Plus | T::Minus => 11,
            T::Star | T::Slash | T::Percent => 12,
            _ => return None,
        };
        // Only `&&` and `||` may start a continuation line.
        if self.nl() && !matches!(self.kind(), T::AndAnd | T::OrOr) {
            return None;
        }
        Some(level)
    }

    fn binary(&mut self, min: u8, r: Restriction) -> R<Ex> {
        let first = self.cast(r)?;
        // The operators of one level read left to right form one chain, laid out together; a different level starts
        // a new chain whose first operand is the chain so far.
        let mut chain_level = 0u8;
        let mut head = first;
        let mut rest: Vec<(D, Ex)> = Vec::new();
        while let Some(level) = self.binop() {
            if level < min {
                break;
            }
            if chain_level != level && !rest.is_empty() {
                head = chain(head, std::mem::take(&mut rest), chain_level);
            }
            chain_level = level;
            let op = self.tok_doc();
            let rhs = self.binary(level + 1, r)?;
            rest.push((op, rhs));
        }
        if rest.is_empty() { Ok(head) } else { Ok(chain(head, rest, chain_level)) }
    }

    fn cast(&mut self, r: Restriction) -> R<Ex> {
        let mut e = self.unary(r)?;
        while self.at(&TokenKind::As) {
            let kw = self.tok_doc();
            let ty = self.ty()?;
            e = Ex::plain(concat(vec![e.doc, space(), kw, space(), ty]));
        }
        Ok(e)
    }

    fn unary(&mut self, r: Restriction) -> R<Ex> {
        match self.kind() {
            TokenKind::Comptime => self.comptime_expr(r),
            TokenKind::Minus | TokenKind::Bang | TokenKind::Star | TokenKind::Amp => {
                let is_amp = self.at(&TokenKind::Amp);
                let op = self.tok_doc();
                let mut parts = vec![op];
                if is_amp && self.at(&TokenKind::Mut) {
                    parts.push(self.tok_doc());
                    parts.push(space());
                } else if is_amp && self.at(&TokenKind::Amp) {
                    // `& &x`: `&&` would be one token.
                    parts.push(space());
                }
                let operand = self.unary(r)?;
                parts.push(operand.doc);
                Ok(Ex::plain(concat(parts)))
            }
            _ => self.postfix(r),
        }
    }

    /// `comptime E`, `comptime if ...`, `comptime for ...`, and `comptime assert(condition[, message])`.
    fn comptime_expr(&mut self, r: Restriction) -> R<Ex> {
        let kw = self.tok_doc();
        if self.at_word("assert") && self.nth(1) == &TokenKind::LParen {
            let name = self.tok_doc();
            let args = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| Ok(f.expr()?.doc))?;
            let args = self.list_doc(args, false);
            return Ok(Ex::plain(concat(vec![kw, space(), name, args])));
        }
        let operand = self.unary(r)?;
        Ok(Ex { binary: false, ..Ex::plain(concat(vec![kw, space(), operand.doc])) })
    }

    fn postfix(&mut self, r: Restriction) -> R<Ex> {
        let base = self.primary(r)?;
        let mut head = base.doc;
        let mut path = base.path;
        let mut hug = base.hug;
        let mut block_like = base.block_like;
        let mut segs: Vec<Seg> = Vec::new();
        loop {
            match self.kind().clone() {
                TokenKind::Question => {
                    segs.push(Seg { doc: self.tok_doc(), dot: false, call: false });
                    path = false;
                    hug = false;
                    block_like = false;
                }
                TokenKind::Dot => {
                    let after_int = self.pos > 0 && matches!(self.toks[self.pos - 1].kind, TokenKind::Int(_));
                    let dot = self.tok_doc();
                    hug = false;
                    block_like = false;
                    match self.kind().clone() {
                        TokenKind::Ident(_) => {
                            let name = self.tok_doc();
                            let mut doc = concat(vec![dot, name]);
                            if self.at(&TokenKind::Lt) && lookahead::member_type_args_ahead(&self.raw, self.pos) {
                                let args = self.angle_args()?;
                                doc = concat(vec![doc, args]);
                                path = false;
                            } else if path && self.at_struct_literal(r) {
                                // `Shape.Rect { ... }`: what came before is the path of a struct literal.
                                let path_doc = concat(vec![head, concat(segs.drain(..).map(|s| s.doc).collect()), doc]);
                                let lit = self.struct_literal_body()?;
                                head = concat(vec![path_doc, space(), lit]);
                                path = false;
                                hug = true;
                                continue;
                            }
                            segs.push(Seg { doc, dot: true, call: false });
                        }
                        TokenKind::Int(_) => {
                            let n = self.tok_doc();
                            // `1.2` would read as a float: after an integer literal the field stays on its own line.
                            let dot = if after_int { concat(vec![dot, Doc::Hard]) } else { dot };
                            segs.push(Seg { doc: concat(vec![dot, n]), dot: true, call: false });
                            path = false;
                        }
                        TokenKind::LBrace => {
                            let open = self.open(&TokenKind::LBrace, false)?;
                            let name = self.expr()?;
                            let close = self.close(&TokenKind::RBrace)?;
                            let dangling = self.dangling(&close.lead, true);
                            let doc = concat(vec![
                                dot,
                                open.lead,
                                open.text,
                                open.trail,
                                name.doc,
                                dangling,
                                close.text,
                                close.trail,
                            ]);
                            segs.push(Seg { doc, dot: true, call: false });
                            path = false;
                        }
                        _ => return Err(self.error("expected a field name")),
                    }
                }
                TokenKind::LParen if !self.nl() => {
                    let args = self.call_args()?;
                    segs.push(Seg { doc: args, dot: false, call: true });
                    path = false;
                    hug = false;
                    block_like = false;
                }
                TokenKind::LBracket if !self.nl() => {
                    let open = self.open(&TokenKind::LBracket, false)?;
                    let index = self.expr()?;
                    let close = self.close(&TokenKind::RBracket)?;
                    let dangling = self.dangling(&close.lead, true);
                    let doc = concat(vec![
                        open.lead,
                        group(concat(vec![
                            open.text,
                            open.trail,
                            indent(concat(vec![Doc::Soft, index.doc, dangling])),
                            Doc::Soft,
                            close.text,
                        ])),
                        close.trail,
                    ]);
                    segs.push(Seg { doc, dot: false, call: true });
                    path = false;
                    hug = false;
                    block_like = false;
                }
                _ => break,
            }
        }
        if segs.is_empty() {
            return Ok(Ex { doc: head, binary: false, block_like, hug, path });
        }
        Ok(Ex { doc: assemble_chain(head, segs), binary: false, block_like: false, hug: false, path })
    }

    /// `(args)`: the arguments of a call. The last one may stay on the line of the call when it is a closure or a
    /// literal that breaks inside itself.
    fn call_args(&mut self) -> R<D> {
        let mut hugs: Vec<bool> = Vec::new();
        let list = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| {
            let mut parts = Vec::new();
            if f.at(&TokenKind::Mut) {
                parts.push(f.tok_doc());
                parts.push(space());
            }
            if matches!(f.kind(), TokenKind::Ident(_)) && f.nth(1) == &TokenKind::Colon {
                parts.push(f.tok_doc());
                parts.push(f.tok_doc());
                parts.push(space());
            }
            let value = f.expr()?;
            hugs.push(value.hug);
            parts.push(value.doc);
            Ok(concat(parts))
        })?;
        Ok(self.call_list(list, &hugs))
    }

    fn call_list(&mut self, list: Delimited, hugs: &[bool]) -> D {
        let n = list.elems.len();
        let plain = list.open.lead.is_nil()
            && list.open.trail.is_nil()
            && list.close.lead.is_empty()
            && list.elems.iter().all(|e| e.after.is_nil());
        let can_hug = n > 0
            && hugs[n - 1]
            && !list.keep_broken
            && plain
            && list.elems[..n - 1].iter().all(|e| !e.doc.has_forced_break());
        if !can_hug {
            return self.list_doc(list, false);
        }
        let docs: Vec<D> = list.elems.iter().map(|e| e.doc.clone()).collect();
        let join = |docs: &[D], last: D| -> D {
            let mut parts = vec![list.open.text.clone()];
            for (i, doc) in docs[..n - 1].iter().enumerate() {
                if i > 0 {
                    parts.push(text(", "));
                }
                parts.push(doc.clone());
            }
            if n > 1 {
                parts.push(text(", "));
            }
            parts.push(last);
            parts.push(list.close.text.clone());
            concat(parts)
        };
        let flat = join(&docs, docs[n - 1].clone());
        let hugged = join(&docs, force_last_group(docs[n - 1].clone()));
        let close_trail = list.close.trail.clone();
        let expanded = self.list_doc(list, false);
        concat(vec![Doc::Alt(vec![flat, hugged, expanded]), close_trail])
    }

    // ----- primary expressions ---------------------------------------------------------------------------------

    fn at_struct_literal(&self, r: Restriction) -> bool {
        self.at(&TokenKind::LBrace)
            && !self.nl()
            && r == Restriction::None
            && lookahead::looks_like_struct_literal(&self.raw, self.pos)
    }

    fn at_select(&self, r: Restriction) -> bool {
        r == Restriction::None
            && self.at_word("select")
            && self.nth(1) == &TokenKind::LBrace
            && !self.raw[(self.pos + 1).min(self.raw.len() - 1)].nl_before
    }

    /// `{ name: value, name }`: the fields of a struct literal, separated by `,` or a line break.
    fn struct_literal_body(&mut self) -> R<D> {
        let open = self.open(&TokenKind::LBrace, true)?;
        let mut elems = Vec::new();
        let mut trailing_comma = false;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let mut doc = self.ident()?;
            if self.at(&TokenKind::Colon) {
                let colon = self.tok_doc();
                let value = self.expr()?;
                doc = concat(vec![doc, colon, space(), value.doc]);
            }
            if self.at(&TokenKind::Comma) {
                let after = self.dropped();
                elems.push(Elem { doc, after });
                trailing_comma = true;
            } else {
                elems.push(Elem { doc, after: Doc::Nil });
                trailing_comma = false;
                if !self.tok().nl_before && !self.at(&TokenKind::RBrace) {
                    return Err(self.error("expected `,`, a line break, or `}`"));
                }
            }
        }
        let keep_broken = trailing_comma && self.tok().nl_before;
        let close = self.close(&TokenKind::RBrace)?;
        Ok(self.inline_braces_doc(open, elems, keep_broken, close))
    }

    fn primary(&mut self, r: Restriction) -> R<Ex> {
        match self.kind().clone() {
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Char(_)
            | TokenKind::Str(_)
            | TokenKind::Interp(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::SelfValue => Ok(Ex::plain(self.tok_doc())),
            // An inline snapshot, `@"..."`: the marker is written against its literal, which keeps its text.
            TokenKind::At if matches!(self.nth(1), TokenKind::Str(_)) => {
                let at = self.tok_doc();
                let literal = self.tok_doc();
                Ok(Ex::plain(concat(vec![at, literal])))
            }
            TokenKind::SelfType => {
                let name = self.tok_doc();
                if self.at_struct_literal(r) {
                    let lit = self.struct_literal_body()?;
                    return Ok(Ex { hug: true, ..Ex::plain(concat(vec![name, space(), lit])) });
                }
                Ok(Ex { path: true, ..Ex::plain(name) })
            }
            TokenKind::Ident(_) if self.at_select(r) => self.select_expr(),
            TokenKind::Ident(_) => {
                let generic = lookahead::generic_args_ahead(&self.raw, self.pos);
                let name = self.tok_doc();
                let mut doc = name;
                if generic {
                    let args = self.angle_args()?;
                    doc = concat(vec![doc, args]);
                }
                if self.at_struct_literal(r) {
                    let lit = self.struct_literal_body()?;
                    return Ok(Ex { hug: true, ..Ex::plain(concat(vec![doc, space(), lit])) });
                }
                Ok(Ex { path: !generic, ..Ex::plain(doc) })
            }
            TokenKind::LParen => {
                let list = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| Ok(f.expr()?.doc))?;
                if list.elems.len() == 1 && !list.trailing_comma {
                    return Ok(Ex::plain(self.paren_doc(list)));
                }
                let single = list.elems.len() == 1;
                Ok(Ex::plain(self.list_doc(list, single)))
            }
            TokenKind::LBracket => self.array_expr(),
            TokenKind::LBrace => {
                let first = self.pos;
                let parts = self.block_parts()?;
                let soft = self.stmt_start != first;
                let doc = self.block_doc(parts, soft);
                Ok(Ex { block_like: true, ..Ex::plain(group(doc)) })
            }
            TokenKind::Unsafe => {
                let first = self.pos;
                let kw = self.tok_doc();
                let parts = self.block_parts()?;
                let soft = self.stmt_start != first;
                let doc = self.block_doc(parts, soft);
                Ok(Ex { block_like: true, ..Ex::plain(group(concat(vec![kw, space(), doc]))) })
            }
            TokenKind::If => self.if_expr(),
            TokenKind::While => {
                let kw = self.tok_doc();
                let mut head = vec![kw, space()];
                // `while let pattern = value { ... }`: the same head as `if let`.
                if self.at(&TokenKind::Let) {
                    head.push(self.tok_doc());
                    head.push(space());
                    head.push(self.pattern()?);
                    head.push(space());
                    head.push(self.take(&TokenKind::Eq)?);
                    head.push(space());
                }
                let cond = self.expr_r(Restriction::NoStructLiteral)?;
                let body = self.block_parts()?;
                let body = self.block_doc(body, false);
                head.push(cond.doc);
                head.push(space());
                head.push(body);
                Ok(Ex { block_like: true, ..Ex::plain(concat(head)) })
            }
            TokenKind::Loop => {
                let kw = self.tok_doc();
                let body = self.block_parts()?;
                let body = self.block_doc(body, false);
                Ok(Ex { block_like: true, ..Ex::plain(concat(vec![kw, space(), body])) })
            }
            TokenKind::For => {
                let kw = self.tok_doc();
                let pattern = self.pattern()?;
                let in_kw = self.take(&TokenKind::In)?;
                let mut parts = vec![kw, space(), pattern, space(), in_kw, space()];
                if self.at(&TokenKind::Mut) {
                    parts.push(self.tok_doc());
                    parts.push(space());
                }
                let iter = self.expr_r(Restriction::NoStructLiteral)?;
                parts.push(iter.doc);
                parts.push(space());
                let body = self.block_parts()?;
                parts.push(self.block_doc(body, false));
                Ok(Ex { block_like: true, ..Ex::plain(concat(parts)) })
            }
            TokenKind::Return => {
                let kw = self.tok_doc();
                if !self.nl() && lookahead::can_start_expr(self.kind()) {
                    let value = self.expr()?;
                    return Ok(Ex::plain(concat(vec![kw, space(), value.doc])));
                }
                Ok(Ex::plain(kw))
            }
            TokenKind::Break => {
                let kw = self.tok_doc();
                let mut parts = vec![kw];
                // A label on the same line names the loop the jump leaves.
                if !self.nl() && matches!(self.kind(), TokenKind::Label(_)) {
                    parts.push(space());
                    parts.push(self.tok_doc());
                }
                // So does a value (`break 42`); `continue` takes none.
                if !self.nl() && lookahead::can_start_expr(self.kind()) {
                    let value = self.expr()?;
                    parts.push(space());
                    parts.push(value.doc);
                }
                Ok(Ex::plain(concat(parts)))
            }
            TokenKind::Continue => {
                let kw = self.tok_doc();
                // A label on the same line names the loop the jump goes on with.
                if !self.nl() && matches!(self.kind(), TokenKind::Label(_)) {
                    let label = self.tok_doc();
                    return Ok(Ex::plain(concat(vec![kw, space(), label])));
                }
                Ok(Ex::plain(kw))
            }
            // `'label: loop`, `'label: while`, or `'label: for`: the label, its colon, and the loop.
            TokenKind::Label(_) => {
                let label = self.tok_doc();
                let colon = self.take(&TokenKind::Colon)?;
                let body = self.primary(r)?;
                Ok(Ex { block_like: body.block_like, ..Ex::plain(concat(vec![label, colon, space(), body.doc])) })
            }
            TokenKind::Match => self.match_expr(),
            TokenKind::Pipe | TokenKind::OrOr => self.closure_expr(),
            _ => Err(self.error("expected an expression")),
        }
    }

    /// `(expr)`: a parenthesized expression, which may break inside its parentheses but never gets a trailing comma.
    pub fn paren_doc(&mut self, list: Delimited) -> D {
        let Delimited { open, mut elems, close, .. } = list;
        let only = elems.pop().expect("one element");
        let dangling = self.dangling(&close.lead, true);
        concat(vec![
            open.lead,
            group(concat(vec![
                open.text,
                open.trail,
                indent(concat(vec![Doc::Soft, only.doc, only.after, dangling])),
                Doc::Soft,
                close.text,
            ])),
            close.trail,
        ])
    }

    /// `[a, b]`, `[]`, or `[value; count]`.
    fn array_expr(&mut self) -> R<Ex> {
        if self.is_repeat_array() {
            let open = self.open(&TokenKind::LBracket, false)?;
            let value = self.expr()?;
            let semi = self.take(&TokenKind::Semi)?;
            let count = self.expr()?;
            let close = self.close(&TokenKind::RBracket)?;
            let dangling = self.dangling(&close.lead, true);
            let doc = concat(vec![
                open.lead,
                group(concat(vec![
                    open.text,
                    open.trail,
                    indent(concat(vec![Doc::Soft, value.doc, semi, Doc::Line, count.doc, dangling])),
                    Doc::Soft,
                    close.text,
                ])),
                close.trail,
            ]);
            return Ok(Ex::plain(doc));
        }
        let list = self.delimited(&TokenKind::LBracket, &TokenKind::RBracket, |f| Ok(f.expr()?.doc))?;
        let hug = !list.elems.is_empty();
        Ok(Ex { hug, ..Ex::plain(self.list_doc(list, false)) })
    }

    /// Whether the `[` at the cursor opens `[value; count]`: a `;` before the matching `]`, outside any bracket.
    fn is_repeat_array(&self) -> bool {
        let mut depth = 0i32;
        for t in &self.raw[self.pos..] {
            match t.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                TokenKind::Semi if depth == 1 => return true,
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    /// `|a, b| body`, `|| body`, or `|a: i32|: bool body`.
    fn closure_expr(&mut self) -> R<Ex> {
        let mut parts = Vec::new();
        if self.at(&TokenKind::OrOr) {
            parts.push(self.tok_doc());
        } else {
            parts.push(self.open(&TokenKind::Pipe, false)?.all());
            let mut first = true;
            while !self.at(&TokenKind::Pipe) {
                if !first {
                    parts.push(space());
                }
                first = false;
                parts.push(self.ident()?);
                if self.at(&TokenKind::Colon) {
                    parts.push(self.tok_doc());
                    parts.push(space());
                    parts.push(self.ty()?);
                }
                if self.at(&TokenKind::Comma) {
                    parts.push(self.tok_doc());
                } else {
                    break;
                }
            }
            let close = self.close(&TokenKind::Pipe)?;
            let dangling = self.dangling(&close.lead, true);
            parts.push(dangling);
            parts.push(close.text);
            parts.push(close.trail);
        }
        if self.at(&TokenKind::Colon) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        let body = self.expr()?;
        parts.push(self.after_operator(body));
        Ok(Ex { hug: true, ..Ex::plain(concat(parts)) })
    }
}

/// A chain of binary operators that moved to a line of its own after `=` needs no indentation of its own: its
/// continuation lines line up with its first line.
fn without_chain_indent(doc: D) -> D {
    if let Doc::Group(inner, broken) = doc {
        return match *inner {
            Doc::Concat(mut parts) if parts.len() == 2 && matches!(parts[1], Doc::Indent(_)) => {
                let Some(Doc::Indent(rest)) = parts.pop() else { unreachable!("checked above") };
                parts.push(*rest);
                Doc::Group(Box::new(Doc::Concat(parts)), broken)
            }
            other => Doc::Group(Box::new(other), broken),
        };
    }
    doc
}

/// The operands of one level of binary operators as a group. `&&` and `||` start the lines they break onto; the
/// other operators end them.
fn chain(head: Ex, rest: Vec<(D, Ex)>, level: u8) -> Ex {
    let leading = level == LEVEL_OR || level == LEVEL_AND;
    let mut parts = Vec::new();
    for (op, operand) in rest {
        if leading {
            parts.extend([Doc::Line, op, space(), operand.doc]);
        } else {
            parts.extend([space(), op, Doc::Line, operand.doc]);
        }
    }
    Ex { binary: true, ..Ex::plain(group(concat(vec![head.doc, indent(concat(parts))]))) }
}

/// Lays a head and its postfix links out: on one line when it fits; a chain of two or more calls breaks before each
/// `.` otherwise. Field accesses at the start stay with the head.
fn assemble_chain(head: D, segs: Vec<Seg>) -> D {
    // Links: each begins at a `.`; what comes before the first `.` belongs to the head.
    let mut head_parts = vec![head];
    let mut links: Vec<Vec<Seg>> = Vec::new();
    for seg in segs {
        if seg.dot {
            links.push(vec![seg]);
        } else if let Some(link) = links.last_mut() {
            link.push(seg);
        } else {
            head_parts.push(seg.doc);
        }
    }
    // Leading links without a call stay with the head: `self.items.iter()` breaks before `.iter()` only.
    let mut absorbed = 0;
    while absorbed < links.len() && links[absorbed].iter().all(|s| !s.call) {
        absorbed += 1;
    }
    let rest = links.split_off(absorbed);
    for link in links {
        head_parts.extend(link.into_iter().map(|s| s.doc));
    }
    if rest.len() < 2 {
        let tail = rest.into_iter().flatten().map(|s| s.doc).collect();
        head_parts.push(concat(tail));
        return concat(head_parts);
    }
    let last = rest.len() - 1;
    let mut forced = false;
    let mut broken = Vec::new();
    for (i, link) in rest.into_iter().enumerate() {
        let doc = concat(link.into_iter().map(|s| s.doc).collect());
        if i != last && doc.has_forced_break() {
            forced = true;
        }
        broken.push(concat(vec![Doc::Soft, doc]));
    }
    let body = concat(vec![concat(head_parts), indent(concat(broken))]);
    if forced { broken_group(body) } else { group(body) }
}

/// Makes the last group inside `doc` a broken one, so that the last argument of a call lays out across lines.
fn force_last_group(doc: D) -> D {
    match doc {
        Doc::Concat(mut parts) => {
            if let Some(at) = parts.iter().rposition(|p| !matches!(p, Doc::Nil | Doc::Suffix(_) | Doc::Text(_))) {
                let part = std::mem::replace(&mut parts[at], Doc::Nil);
                parts[at] = force_last_group(part);
            }
            Doc::Concat(parts)
        }
        Doc::Group(inner, _) => Doc::Group(inner, true),
        other => other,
    }
}
