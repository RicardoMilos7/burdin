//! The token cursor the layout parser reads from, and how a token is turned into a document together with its
//! comments (spec/fmt.md, FM3).

use crate::doc::{Doc, concat, text};
use crate::trivia::{Comment, Tok};
use burdin_syntax::token::{Token, TokenKind};

pub type D = Doc;

/// The layout parser disagreed with the compiler's parser about the source. The source parsed without error, so
/// this is a formatter bug; it is reported and the file is left alone.
#[derive(Debug)]
pub struct FmtError(pub String);

pub type R<T> = Result<T, FmtError>;

/// A token as three documents: the comments before it, its text, and the comments after it.
pub struct Parts {
    pub lead: D,
    pub text: D,
    pub trail: D,
}

impl Parts {
    pub fn all(self) -> D {
        concat(vec![self.lead, self.text, self.trail])
    }
}

/// A closing bracket: the comments in front of it, its text, and the comments after it (which belong outside the
/// group the bracket closes).
pub struct Closer {
    pub lead: Vec<Comment>,
    pub text: D,
    pub trail: D,
}

/// Whether a token continues an expression onto the next line, so a line break after it is always allowed
/// (spec/grammar.md, section 2, rule 3).
fn continues_line(kind: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        Plus | Minus
            | Star
            | Slash
            | Percent
            | Amp
            | Pipe
            | Caret
            | Shl
            | Shr
            | AndAnd
            | OrOr
            | EqEq
            | NotEq
            | Lt
            | Gt
            | Le
            | Ge
            | Eq
            | Comma
            | Dot
            | Colon
            | FatArrow
            | LParen
            | LBracket
            | LBrace
            | DotDot
            | DotDotEq
            | Orelse
            | Priv
    )
}

pub struct Fmt<'a> {
    pub src: &'a str,
    pub toks: Vec<Tok>,
    /// The lexer's tokens, for the parser's look-ahead functions (`burdin_syntax::lookahead`).
    pub raw: Vec<Token>,
    pub pos: usize,
    /// Whether line breaks are significant, one entry per open bracket (spec/grammar.md, section 2).
    pub nl_stack: Vec<bool>,
    /// The nesting depth of braces, for the width left to a code block in a doc comment.
    pub depth: usize,
    /// The index of the first token of the statement being parsed.
    pub stmt_start: usize,
    /// How many comments have been emitted, so a caller can tell whether a construct contains any.
    pub comments: usize,
    pub width: usize,
    /// Whether the code blocks of doc comments are formatted (FM10).
    pub docs: bool,
}

impl<'a> Fmt<'a> {
    pub fn new(src: &'a str, toks: Vec<Tok>, raw: Vec<Token>, width: usize) -> Fmt<'a> {
        Fmt {
            src,
            toks,
            raw,
            pos: 0,
            nl_stack: Vec::new(),
            depth: 0,
            stmt_start: usize::MAX,
            comments: 0,
            width,
            docs: true,
        }
    }

    // ----- reading ---------------------------------------------------------------------------------------------

    pub fn tok(&self) -> &Tok {
        &self.toks[self.pos]
    }

    pub fn kind(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    pub fn nth(&self, n: usize) -> &TokenKind {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].kind
    }

    pub fn at(&self, kind: &TokenKind) -> bool {
        self.kind() == kind
    }

    pub fn at_eof(&self) -> bool {
        self.at(&TokenKind::Eof)
    }

    pub fn at_word(&self, word: &str) -> bool {
        matches!(self.kind(), TokenKind::Ident(name) if name == word)
    }

    /// A line break before the current token that ends an expression here.
    pub fn nl(&self) -> bool {
        self.tok().nl_before && self.nl_stack.last().copied().unwrap_or(true)
    }

    pub fn nl_top(&self) -> bool {
        self.nl_stack.last().copied().unwrap_or(true)
    }

    pub fn error(&self, message: &str) -> FmtError {
        let t = self.tok();
        FmtError(format!("{message} at byte {} (`{}`)", t.lo, &self.src[t.lo..t.hi]))
    }

    pub fn expect_kind(&self, kind: &TokenKind) -> R<()> {
        if self.at(kind) { Ok(()) } else { Err(self.error(&format!("expected `{}`", kind.text()))) }
    }

    /// Whether a blank line comes before the current token (or before its first comment).
    pub fn blank_before(&self) -> bool {
        let t = self.tok();
        match t.lead.first() {
            Some(c) => c.nl_before >= 2,
            None => t.nls >= 2,
        }
    }

    // ----- emitting --------------------------------------------------------------------------------------------

    fn token_text(&self, index: usize) -> D {
        let t = &self.toks[index];
        let raw = &self.src[t.lo..t.hi];
        if raw.contains('\n') { Doc::Verbatim(raw.to_string()) } else { text(raw) }
    }

    /// Whether a line break may go in front of the token at `index` without changing the meaning.
    fn break_allowed_before(&self, index: usize) -> bool {
        !self.nl_top() || (index > 0 && continues_line(&self.toks[index - 1].kind))
    }

    /// The comments that lead a token. `own_line` says the token starts a line (a statement, an item, a member):
    /// then comments that were on their own lines stay there. Elsewhere a comment would need a line break the grammar
    /// may not allow, so an own-line comment is moved to the end of the line instead.
    fn lead_doc(&mut self, index: usize, comments: Vec<Comment>, own_line: bool) -> D {
        if comments.is_empty() {
            return Doc::Nil;
        }
        let comments = if own_line { self.with_doc_blocks(comments) } else { comments };
        self.comments += comments.len();
        let mut parts = Vec::new();
        for c in &comments {
            let verbatim = c.text.contains('\n');
            let body = if verbatim { Doc::Verbatim(c.text.clone()) } else { text(c.text.clone()) };
            if (c.nl_before == 0 || c.nl_after == 0) && !c.line {
                // An inline block comment sits where the layout puts the token; it keeps a space after it when it had
                // one. (The layout supplies any space in front.)
                parts.push(body);
                if c.space_after {
                    parts.push(text(" "));
                }
            } else if own_line || self.break_allowed_before(index) {
                parts.push(body);
                parts.push(Doc::Hard);
                if c.nl_after >= 2 {
                    parts.push(Doc::Hard);
                }
            } else {
                parts.push(Doc::Suffix(format!(" {}", c.text)));
            }
        }
        concat(parts)
    }

    fn trail_doc(&mut self, comments: &[Comment]) -> D {
        self.comments += comments.len();
        concat(comments.iter().map(|c| Doc::Suffix(format!(" {}", c.text))).collect())
    }

    /// Consumes the current token. `own_line`: see [`Fmt::lead_doc`].
    pub fn parts(&mut self, own_line: bool) -> Parts {
        let index = self.pos;
        let lead = std::mem::take(&mut self.toks[index].lead);
        let trail = std::mem::take(&mut self.toks[index].trail);
        let text = self.token_text(index);
        let lead = self.lead_doc(index, lead, own_line);
        let trail = self.trail_doc(&trail);
        if index + 1 < self.toks.len() {
            self.pos += 1;
        }
        Parts { lead, text, trail }
    }

    /// Consumes the current token and returns it with its comments.
    pub fn tok_doc(&mut self) -> D {
        self.parts(false).all()
    }

    /// The comments in front of the current token, which starts a line (a statement, an item, a member). They are
    /// taken out of the token, so the token's own document carries none.
    pub fn start_of_line(&mut self) -> D {
        let index = self.pos;
        let lead = std::mem::take(&mut self.toks[index].lead);
        self.lead_doc(index, lead, true)
    }

    /// Consumes a token the output drops (a `;` or a separator `,`) and returns only what to keep of it: its comments,
    /// which trail whatever comes before.
    pub fn dropped(&mut self) -> D {
        let index = self.pos;
        let lead = std::mem::take(&mut self.toks[index].lead);
        let trail = std::mem::take(&mut self.toks[index].trail);
        if index + 1 < self.toks.len() {
            self.pos += 1;
        }
        self.comments += lead.len() + trail.len();
        let mut parts = Vec::new();
        for c in lead.iter().chain(trail.iter()) {
            parts.push(Doc::Suffix(format!(" {}", c.text)));
        }
        concat(parts)
    }

    /// Consumes `kind` and returns its document.
    pub fn take(&mut self, kind: &TokenKind) -> R<D> {
        self.expect_kind(kind)?;
        Ok(self.tok_doc())
    }

    /// Opens a bracket and records whether line breaks matter inside it.
    pub fn open(&mut self, kind: &TokenKind, newlines_matter: bool) -> R<Parts> {
        self.expect_kind(kind)?;
        let parts = self.parts(false);
        self.nl_stack.push(newlines_matter);
        Ok(parts)
    }

    /// Closes a bracket. The comments in front of the closer are returned raw, for the caller to place inside the
    /// brackets with [`Fmt::dangling`].
    pub fn close(&mut self, kind: &TokenKind) -> R<Closer> {
        self.expect_kind(kind)?;
        let index = self.pos;
        let lead = std::mem::take(&mut self.toks[index].lead);
        let trail = std::mem::take(&mut self.toks[index].trail);
        let text = self.token_text(index);
        let trail = self.trail_doc(&trail);
        if index + 1 < self.toks.len() {
            self.pos += 1;
        }
        self.nl_stack.pop();
        Ok(Closer { lead, text, trail })
    }

    /// Consumes the `>` that closes generic arguments, splitting `>>`, `>=`, and `>>=`.
    pub fn closing_angle(&mut self) -> R<Parts> {
        let index = self.pos;
        let rest = match self.kind() {
            TokenKind::Gt => {
                self.nl_stack.pop();
                return Ok(self.parts(false));
            }
            TokenKind::Shr => TokenKind::Gt,
            TokenKind::Ge => TokenKind::Eq,
            TokenKind::ShrEq => TokenKind::Ge,
            _ => return Err(self.error("expected `>`")),
        };
        // The first `>` closes the list and takes the comments in front; the rest stays for the caller.
        let lead = std::mem::take(&mut self.toks[index].lead);
        let lead = self.lead_doc(index, lead, false);
        let t = &mut self.toks[index];
        t.kind = rest;
        t.lo += 1;
        t.nl_before = false;
        t.nls = 0;
        self.raw[index].kind = self.toks[index].kind.clone();
        self.nl_stack.pop();
        Ok(Parts { lead, text: text(">"), trail: Doc::Nil })
    }

    /// Comments in front of a closing token, one per line, for the inside of a block or a list. Each is preceded by a
    /// line break (so the group around it breaks), and a blank line in the source is kept between the comments and
    /// after the last element (`after_element`).
    pub fn dangling(&mut self, lead: &[Comment], after_element: bool) -> D {
        let mut parts = Vec::new();
        for (i, c) in lead.iter().enumerate() {
            parts.push(Doc::Hard);
            if (i > 0 || after_element) && c.nl_before >= 2 {
                parts.push(Doc::Hard);
            }
            parts.push(if c.text.contains('\n') { Doc::Verbatim(c.text.clone()) } else { text(c.text.clone()) });
        }
        self.comments += lead.len();
        concat(parts)
    }
}
