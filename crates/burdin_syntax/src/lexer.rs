//! The lexer (spec/grammar.md, section 1).

use crate::token::{HoleTokens, InterpPiece, Token, TokenKind};
use burdin_diag::{Code, Diagnostics, FileId, Span};

pub struct LexResult {
    pub tokens: Vec<Token>,
    /// The lexer stopped early at an unterminated string, char, or block comment. The final `Eof` token is not
    /// the real end of the file, so the parser stays silent about it.
    pub truncated: bool,
}

pub fn lex(file: FileId, text: &str, diags: &mut Diagnostics) -> LexResult {
    let mut lexer = Lexer { file, src: text.as_bytes(), pos: 0, diags, tokens: Vec::new(), nl_before: false };
    let truncated = lexer.run();
    let end = text.len() as u32;
    let nl_before = lexer.nl_before;
    lexer.tokens.push(Token { kind: TokenKind::Eof, span: Span::new(file, end, end), nl_before });
    LexResult { tokens: lexer.tokens, truncated }
}

/// Lexes only `text[lo..hi]`. The tokens' spans are offsets into the whole `text`, so what this returns is what
/// [`lex`] returns for that stretch, and the final `Eof` sits at `hi`. The query library uses it to reparse one
/// function body (`burdin_query`, `spec/query.md`, QL6).
pub fn lex_range(file: FileId, text: &str, lo: usize, hi: usize, diags: &mut Diagnostics) -> LexResult {
    let mut lexer = Lexer { file, src: &text.as_bytes()[..hi], pos: lo, diags, tokens: Vec::new(), nl_before: false };
    let truncated = lexer.run();
    let nl_before = lexer.nl_before;
    lexer.tokens.push(Token { kind: TokenKind::Eof, span: Span::new(file, hi as u32, hi as u32), nl_before });
    LexResult { tokens: lexer.tokens, truncated }
}

struct Lexer<'a> {
    file: FileId,
    src: &'a [u8],
    pos: usize,
    diags: &'a mut Diagnostics,
    tokens: Vec<Token>,
    nl_before: bool,
}

/// Where a placeholder's parts are, as byte offsets into the source.
struct HoleScan {
    expr_lo: usize,
    expr_hi: usize,
    spec: Option<(usize, usize)>,
    close: usize,
}

/// Stop lexing: the rest of the file belongs to an unterminated literal or comment.
struct Fatal;

impl Lexer<'_> {
    fn peek(&self, n: usize) -> u8 {
        self.src.get(self.pos + n).copied().unwrap_or(0)
    }

    fn span(&self, lo: usize) -> Span {
        Span::new(self.file, lo as u32, self.pos as u32)
    }

    fn error(&mut self, lo: usize, hi: usize, msg: impl Into<String>) {
        self.diags.error(Code::SYNTAX, Span::new(self.file, lo as u32, hi as u32), msg);
    }

    fn push(&mut self, kind: TokenKind, lo: usize) {
        let span = self.span(lo);
        self.tokens.push(Token { kind, span, nl_before: self.nl_before });
        self.nl_before = false;
    }

    fn after_dot(&self) -> bool {
        matches!(self.tokens.last(), Some(t) if t.kind == TokenKind::Dot)
    }

    /// Returns true if lexing stopped early.
    fn run(&mut self) -> bool {
        loop {
            if self.skip_trivia().is_err() {
                return true;
            }
            if self.pos >= self.src.len() {
                return false;
            }
            if self.token().is_err() {
                return true;
            }
        }
    }

    fn skip_trivia(&mut self) -> Result<(), Fatal> {
        loop {
            match self.peek(0) {
                b'\n' => {
                    self.nl_before = true;
                    self.pos += 1;
                }
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'#' if self.peek(1) == b'!' && self.pos == 0 => {
                    // A script's interpreter line (`spec/scripts.md`, SC2): `#!...` at byte 0 is skipped like a
                    // comment, so `#!/usr/bin/env burdin run` is not part of the program.
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'#' if self.peek(1) == b'!' => {
                    // Anywhere but the first line, a shebang is a mistake worth its own explanation.
                    let lo = self.pos;
                    self.diags.error(
                        Code::SHEBANG_PLACEMENT,
                        Span::new(self.file, lo as u32, (lo + 2) as u32),
                        "a `#!` shebang line is only allowed as the first line of a file \
                         (spec/scripts.md, SC2)",
                    );
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.peek(1) == b'/' => {
                    while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.peek(1) == b'*' => {
                    let lo = self.pos;
                    self.pos += 2;
                    loop {
                        if self.pos >= self.src.len() {
                            self.error(lo, lo + 2, "unterminated block comment");
                            return Err(Fatal);
                        }
                        if self.src[self.pos] == b'*' && self.peek(1) == b'/' {
                            self.pos += 2;
                            break;
                        }
                        if self.src[self.pos] == b'\n' {
                            self.nl_before = true;
                        }
                        self.pos += 1;
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    fn token(&mut self) -> Result<(), Fatal> {
        let lo = self.pos;
        let c = self.peek(0);
        if c == b'r' && self.raw_string(lo)? {
            return Ok(());
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            self.ident(lo);
            return Ok(());
        }
        if c.is_ascii_digit() {
            self.number(lo);
            return Ok(());
        }
        if c == b'"' {
            return self.string(lo);
        }
        if c == b'\'' {
            if self.label(lo) {
                return Ok(());
            }
            return self.char_lit(lo);
        }
        if let Some((kind, len)) = self.punct() {
            self.pos += len;
            self.push(kind, lo);
            return Ok(());
        }
        // Unknown character: report it and skip the whole UTF-8 sequence.
        let ch = std::str::from_utf8(&self.src[lo..]).ok().and_then(|s| s.chars().next());
        let len = ch.map(|c| c.len_utf8()).unwrap_or(1);
        self.pos += len;
        let shown = ch.map(|c| format!("`{c}`")).unwrap_or_else(|| "byte".into());
        self.error(lo, self.pos, format!("unexpected character {shown}"));
        Ok(())
    }

    fn ident(&mut self, lo: usize) {
        while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
            self.pos += 1;
        }
        let word = std::str::from_utf8(&self.src[lo..self.pos]).unwrap();
        let kind = if word == "_" {
            TokenKind::Underscore
        } else {
            match TokenKind::keyword(word) {
                // A member name after `.` is its own namespace: `.any()` stays spellable even though `any` is a
                // keyword (spec/grammar.md, "Keywords").
                Some(
                    TokenKind::Reserved(_) | TokenKind::Any | TokenKind::Impl | TokenKind::Error | TokenKind::Comptime,
                ) if self.after_dot() => TokenKind::Ident(word.to_string()),
                Some(keyword) => keyword,
                None => TokenKind::Ident(word.to_string()),
            }
        };
        self.push(kind, lo);
    }

    fn number(&mut self, lo: usize) {
        let radix = match (self.peek(0), self.peek(1)) {
            (b'0', b'x') => 16,
            (b'0', b'o') => 8,
            (b'0', b'b') => 2,
            _ => 10,
        };
        if radix != 10 {
            self.pos += 2;
        }
        let digits_start = self.pos;
        while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
            // In decimal, an `e` may start an exponent; stop there and look again below.
            if radix == 10 && matches!(self.peek(0), b'e' | b'E') {
                break;
            }
            if radix == 10 && !self.peek(0).is_ascii_digit() && self.peek(0) != b'_' {
                break;
            }
            self.pos += 1;
        }
        let mut is_float = false;
        // Immediately after `.`, a number is plain digits (`t.0.1`).
        if radix == 10 && !self.after_dot() {
            if self.peek(0) == b'.' && self.peek(1).is_ascii_digit() {
                is_float = true;
                self.pos += 1;
                while self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
                    self.pos += 1;
                }
            }
            if matches!(self.peek(0), b'e' | b'E') {
                let sign = usize::from(matches!(self.peek(1), b'+' | b'-'));
                if self.peek(1 + sign).is_ascii_digit() {
                    is_float = true;
                    self.pos += 1 + sign;
                    while self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
                        self.pos += 1;
                    }
                }
            }
        }
        // A suffix or stray letters right after a number are an error (integer literals have no suffixes).
        let mut bad_tail = false;
        while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
            self.pos += 1;
            bad_tail = true;
        }
        let text: String =
            std::str::from_utf8(&self.src[lo..self.pos]).unwrap().chars().filter(|&c| c != '_').collect();
        if bad_tail {
            self.error(lo, self.pos, format!("invalid number literal `{text}`"));
            self.push(TokenKind::Int(0), lo);
            return;
        }
        if is_float {
            self.push(TokenKind::Float(text), lo);
            return;
        }
        let digits: String =
            std::str::from_utf8(&self.src[digits_start..self.pos]).unwrap().chars().filter(|&c| c != '_').collect();
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            self.error(lo, self.pos, format!("invalid number literal `{text}`"));
            self.push(TokenKind::Int(0), lo);
            return;
        }
        match u128::from_str_radix(&digits, radix) {
            Ok(v) => self.push(TokenKind::Int(v), lo),
            Err(_) => {
                self.diags.error(
                    Code::LITERAL_RANGE,
                    Span::new(self.file, lo as u32, self.pos as u32),
                    format!("integer literal `{text}` is too large"),
                );
                self.push(TokenKind::Int(0), lo);
            }
        }
    }

    /// Reads one escape sequence after the backslash. `quote` is the delimiter that may be escaped.
    fn escape(&mut self, quote: u8) -> Option<char> {
        let esc_lo = self.pos - 1;
        let c = self.peek(0);
        self.pos += 1;
        let value = match c {
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'\\' => '\\',
            b'0' => '\0',
            c if c == quote => quote as char,
            _ => {
                let hi = (self.pos).min(self.src.len());
                let shown = std::str::from_utf8(&self.src[esc_lo..hi]).unwrap_or("\\?").to_string();
                self.error(esc_lo, hi, format!("unknown escape sequence `{shown}`"));
                return None;
            }
        };
        Some(value)
    }

    fn string(&mut self, lo: usize) -> Result<(), Fatal> {
        self.pos += 1;
        let mut bytes = Vec::new();
        let mut pieces: Vec<InterpPiece> = Vec::new();
        // Brace errors are reported once the string is known to end, so an unterminated string reports only itself.
        let mut brace_errors: Vec<(usize, &'static str)> = Vec::new();
        loop {
            if self.pos >= self.src.len() {
                self.error(lo, lo + 1, "unterminated string literal");
                return Err(Fatal);
            }
            let c = self.src[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.src.len() {
                        continue;
                    }
                    if let Some(ch) = self.escape(b'"') {
                        let mut buf = [0u8; 4];
                        bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
                b'{' if self.peek(1) == b'{' => {
                    bytes.push(b'{');
                    self.pos += 2;
                }
                b'}' if self.peek(1) == b'}' => {
                    bytes.push(b'}');
                    self.pos += 2;
                }
                b'}' => {
                    brace_errors.push((self.pos, "unmatched `}` in a string literal; write `}}` for a literal brace"));
                    bytes.push(b'}');
                    self.pos += 1;
                }
                b'{' => match self.scan_hole(self.pos) {
                    Some(scan) => {
                        if !bytes.is_empty() {
                            let lit = String::from_utf8(std::mem::take(&mut bytes)).unwrap_or_default();
                            pieces.push(InterpPiece::Lit(lit));
                        }
                        let tokens = self.lex_range(scan.expr_lo, scan.expr_hi);
                        let spec = scan.spec.map(|(slo, shi)| {
                            let text = String::from_utf8_lossy(&self.src[slo..shi]).into_owned();
                            (text, Span::new(self.file, slo as u32, shi as u32))
                        });
                        let span = Span::new(self.file, self.pos as u32, (scan.close + 1) as u32);
                        pieces.push(InterpPiece::Hole(Box::new(HoleTokens { tokens, spec, span })));
                        self.pos = scan.close + 1;
                    }
                    None => {
                        brace_errors
                            .push((self.pos, "unterminated `{` in a string literal; write `{{` for a literal brace"));
                        bytes.push(b'{');
                        self.pos += 1;
                    }
                },
                b'\r' if self.peek(1) == b'\n' => {
                    // Normalize CRLF inside string literals so programs behave the same on every system.
                    self.pos += 1;
                }
                _ => {
                    bytes.push(c);
                    self.pos += 1;
                }
            }
        }
        for (at, message) in brace_errors {
            self.error(at, at + 1, message);
        }
        // The source text is valid UTF-8 and escapes produce whole characters, so this cannot fail.
        let text = String::from_utf8(bytes).unwrap_or_default();
        if pieces.is_empty() {
            self.push(TokenKind::Str(text), lo);
        } else {
            if !text.is_empty() {
                pieces.push(InterpPiece::Lit(text));
            }
            self.push(TokenKind::Interp(pieces), lo);
        }
        Ok(())
    }

    /// Finds the end of the placeholder that starts with the `{` at `open`. A placeholder stays on one line; its
    /// expression may contain brackets, character literals, and string literals, and the specifier starts at the
    /// first `:` outside any bracket. `None` when no closing `}` is found.
    fn scan_hole(&self, open: usize) -> Option<HoleScan> {
        let mut i = open + 1;
        let mut depth = 0usize;
        let mut colon: Option<usize> = None;
        while i < self.src.len() {
            match self.src[i] {
                b'\n' => return None,
                b'"' if colon.is_none() => {
                    i += 1;
                    loop {
                        match self.src.get(i) {
                            None | Some(b'\n') => return None,
                            Some(b'\\') => i += 2,
                            Some(b'"') => break,
                            Some(_) => i += 1,
                        }
                    }
                }
                b'\'' if colon.is_none() => {
                    // A character literal; a quote that does not start one is an ordinary byte.
                    let end = self.src.len().min(i + 9);
                    let rest = match std::str::from_utf8(&self.src[i + 1..end]) {
                        Ok(text) => text,
                        Err(e) => std::str::from_utf8(&self.src[i + 1..i + 1 + e.valid_up_to()]).unwrap_or(""),
                    };
                    let mut chars = rest.chars();
                    let len = match chars.next() {
                        Some('\\') => chars.next().map(|c| 1 + c.len_utf8()),
                        Some(c) => Some(c.len_utf8()),
                        None => None,
                    };
                    if let Some(len) = len
                        && self.src.get(i + 1 + len) == Some(&b'\'')
                    {
                        i += len + 1;
                    }
                }
                b'(' | b'[' | b'{' if colon.is_none() => depth += 1,
                b')' | b']' if colon.is_none() => depth = depth.saturating_sub(1),
                b'}' if colon.is_none() && depth > 0 => depth -= 1,
                b'}' => {
                    let (expr_hi, spec) = match colon {
                        Some(c) => (c, Some((c + 1, i))),
                        None => (i, None),
                    };
                    return Some(HoleScan { expr_lo: open + 1, expr_hi, spec, close: i });
                }
                b':' if colon.is_none() && depth == 0 => colon = Some(i),
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Lexes `src[lo..hi]` as the tokens of a placeholder's expression, ending with `Eof`.
    fn lex_range(&mut self, lo: usize, hi: usize) -> Vec<Token> {
        let mut sub = Lexer {
            file: self.file,
            src: &self.src[..hi],
            pos: lo,
            diags: &mut *self.diags,
            tokens: Vec::new(),
            nl_before: false,
        };
        sub.run();
        let mut tokens = sub.tokens;
        tokens.push(Token { kind: TokenKind::Eof, span: Span::new(self.file, hi as u32, hi as u32), nl_before: false });
        tokens
    }

    /// `r"..."` or `r#"..."#`: a string with no escapes and no placeholders. Returns false when the `r` starts an
    /// ordinary identifier.
    fn raw_string(&mut self, lo: usize) -> Result<bool, Fatal> {
        let mut hashes = 0;
        while self.peek(1 + hashes) == b'#' {
            hashes += 1;
        }
        if self.peek(1 + hashes) != b'"' {
            return Ok(false);
        }
        self.pos += 2 + hashes;
        let start = self.pos;
        loop {
            if self.pos >= self.src.len() {
                self.error(lo, lo + 1, "unterminated raw string literal");
                return Err(Fatal);
            }
            if self.src[self.pos] == b'"' && (0..hashes).all(|k| self.peek(1 + k) == b'#') {
                break;
            }
            self.pos += 1;
        }
        let text = String::from_utf8_lossy(&self.src[start..self.pos]).replace("\r\n", "\n");
        self.pos += 1 + hashes;
        self.push(TokenKind::Str(text), lo);
        Ok(true)
    }

    /// A loop label: a quote, an identifier, and no closing quote after it (`'outer:`). A quote that closes after
    /// one character or name (`'a'`, `'_'`) starts a character literal instead, so this returns false for it.
    fn label(&mut self, lo: usize) -> bool {
        let start = lo + 1;
        let first = self.src.get(start).copied().unwrap_or(0);
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return false;
        }
        let mut end = start;
        while end < self.src.len() && (self.src[end].is_ascii_alphanumeric() || self.src[end] == b'_') {
            end += 1;
        }
        if self.src.get(end) == Some(&b'\'') {
            return false;
        }
        let name = std::str::from_utf8(&self.src[start..end]).unwrap().to_string();
        self.pos = end;
        self.push(TokenKind::Label(name), lo);
        true
    }

    fn char_lit(&mut self, lo: usize) -> Result<(), Fatal> {
        self.pos += 1;
        let mut chars = Vec::new();
        loop {
            if self.pos >= self.src.len() || self.src[self.pos] == b'\n' {
                self.error(lo, lo + 1, "unterminated character literal");
                // A character literal cannot span lines, so lexing can go on from the next line.
                self.push(TokenKind::Char('\0'), lo);
                return Ok(());
            }
            match self.src[self.pos] {
                b'\'' => {
                    self.pos += 1;
                    break;
                }
                b'\\' => {
                    self.pos += 1;
                    if let Some(ch) = self.escape(b'\'') {
                        chars.push(ch);
                    } else {
                        chars.push('\0');
                    }
                }
                _ => {
                    let rest = std::str::from_utf8(&self.src[self.pos..]).unwrap_or("");
                    let ch = rest.chars().next().unwrap_or('\0');
                    self.pos += ch.len_utf8().max(1);
                    chars.push(ch);
                }
            }
        }
        if chars.len() != 1 {
            self.error(lo, self.pos, "a character literal must contain exactly one character");
        }
        self.push(TokenKind::Char(chars.first().copied().unwrap_or('\0')), lo);
        Ok(())
    }

    fn punct(&self) -> Option<(TokenKind, usize)> {
        use TokenKind::*;
        let (a, b, c) = (self.peek(0), self.peek(1), self.peek(2));
        let three = match (a, b, c) {
            (b'.', b'.', b'.') => Some(DotDotDot),
            (b'.', b'.', b'=') => Some(DotDotEq),
            (b'<', b'<', b'=') => Some(ShlEq),
            (b'>', b'>', b'=') => Some(ShrEq),
            _ => None,
        };
        if let Some(k) = three {
            return Some((k, 3));
        }
        let two = match (a, b) {
            (b'.', b'.') => Some(DotDot),
            (b'=', b'>') => Some(FatArrow),
            (b'<', b'<') => Some(Shl),
            (b'>', b'>') => Some(Shr),
            (b'&', b'&') => Some(AndAnd),
            (b'|', b'|') => Some(OrOr),
            (b'=', b'=') => Some(EqEq),
            (b'!', b'=') => Some(NotEq),
            (b'<', b'=') => Some(Le),
            (b'>', b'=') => Some(Ge),
            (b'+', b'=') => Some(PlusEq),
            (b'-', b'=') => Some(MinusEq),
            (b'*', b'=') => Some(StarEq),
            (b'/', b'=') => Some(SlashEq),
            (b'%', b'=') => Some(PercentEq),
            (b'&', b'=') => Some(AmpEq),
            (b'|', b'=') => Some(PipeEq),
            (b'^', b'=') => Some(CaretEq),
            _ => None,
        };
        if let Some(k) = two {
            return Some((k, 2));
        }
        let one = match a {
            b'(' => LParen,
            b')' => RParen,
            b'[' => LBracket,
            b']' => RBracket,
            b'{' => LBrace,
            b'}' => RBrace,
            b',' => Comma,
            b':' => Colon,
            b';' => Semi,
            b'.' => Dot,
            b'@' => At,
            b'?' => Question,
            b'=' => Eq,
            b'+' => Plus,
            b'-' => Minus,
            b'*' => Star,
            b'/' => Slash,
            b'%' => Percent,
            b'&' => Amp,
            b'|' => Pipe,
            b'^' => Caret,
            b'!' => Bang,
            b'<' => Lt,
            b'>' => Gt,
            _ => return None,
        };
        Some((one, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TokenKind::*;

    fn lex_all(src: &str) -> (Vec<Token>, Diagnostics) {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), src, &mut d);
        (r.tokens, d)
    }

    fn kinds(src: &str) -> Vec<TokenKind> {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), src, &mut d);
        assert!(!d.has_errors(), "unexpected errors for {src:?}");
        r.tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn every_keyword_has_its_own_token() {
        let cases: &[(&str, TokenKind)] = &[
            ("any", Any),
            ("as", As),
            ("break", Break),
            ("const", Const),
            ("continue", Continue),
            ("else", Else),
            ("enum", Enum),
            ("error", Error),
            ("extern", Extern),
            ("false", False),
            ("fn", Fn),
            ("for", For),
            ("if", If),
            ("impl", Impl),
            ("in", In),
            ("interface", Interface),
            ("let", Let),
            ("loop", Loop),
            ("match", Match),
            ("mut", Mut),
            ("orelse", Orelse),
            ("own", Own),
            ("priv", Priv),
            ("return", Return),
            ("self", SelfValue),
            ("Self", SelfType),
            ("struct", Struct),
            ("true", True),
            ("type", Type),
            ("unsafe", Unsafe),
            ("use", Use),
            ("where", Where),
            ("while", While),
            ("comptime", Comptime),
            ("derive", Reserved("derive".into())),
        ];
        for (word, want) in cases {
            let mut d = Diagnostics::default();
            let r = lex(FileId(0), word, &mut d);
            assert!(!d.has_errors(), "unexpected errors for {word:?}");
            assert_eq!(r.tokens.len(), 2, "{word:?}");
            assert_eq!(&r.tokens[0].kind, want, "{word:?}");
            assert_eq!(r.tokens[0].span, Span::new(FileId(0), 0, word.len() as u32), "{word:?}");
            assert_eq!(r.tokens[1].kind, Eof, "{word:?}");
        }
    }

    #[test]
    fn punctuation_uses_the_longest_token() {
        let cases: &[(&str, TokenKind)] = &[
            ("...", DotDotDot),
            ("..=", DotDotEq),
            ("..", DotDot),
            (".", Dot),
            ("=>", FatArrow),
            ("<<", Shl),
            (">>", Shr),
            ("<<=", ShlEq),
            (">>=", ShrEq),
            ("+=", PlusEq),
            ("-=", MinusEq),
            ("*=", StarEq),
            ("/=", SlashEq),
            ("%=", PercentEq),
            ("&=", AmpEq),
            ("|=", PipeEq),
            ("^=", CaretEq),
            ("&&", AndAnd),
            ("||", OrOr),
            ("==", EqEq),
            ("!=", NotEq),
            ("<=", Le),
            (">=", Ge),
            ("(", LParen),
            (")", RParen),
            ("[", LBracket),
            ("]", RBracket),
            ("{", LBrace),
            ("}", RBrace),
            (",", Comma),
            (":", Colon),
            (";", Semi),
            ("@", At),
            ("?", Question),
            ("=", Eq),
            ("+", Plus),
            ("-", Minus),
            ("*", Star),
            ("/", Slash),
            ("%", Percent),
            ("&", Amp),
            ("|", Pipe),
            ("^", Caret),
            ("!", Bang),
            ("<", Lt),
            (">", Gt),
        ];
        for (src, want) in cases {
            assert_eq!(kinds(src), vec![want.clone(), Eof], "{src:?}");
        }
    }

    #[test]
    fn identifiers_and_the_underscore() {
        assert_eq!(kinds("_"), vec![Underscore, Eof]);
        assert_eq!(
            kinds("_x x_1 Self self test"),
            vec![
                Ident("_x".into()),
                Ident("x_1".into()),
                SelfType,
                SelfValue,
                // `test` is contextual: it only starts an item when a string follows it.
                Ident("test".into()),
                Eof
            ]
        );
    }

    #[test]
    fn a_keyword_after_a_dot_is_a_member_name() {
        assert_eq!(kinds("x.any"), vec![Ident("x".into()), Dot, Ident("any".into()), Eof]);
        assert_eq!(kinds("x.impl"), vec![Ident("x".into()), Dot, Ident("impl".into()), Eof]);
        assert_eq!(kinds("x.error"), vec![Ident("x".into()), Dot, Ident("error".into()), Eof]);
        // Everywhere else the words stay keywords.
        assert_eq!(kinds("any"), vec![Any, Eof]);
        assert_eq!(kinds("impl"), vec![Impl, Eof]);
        assert_eq!(kinds("error"), vec![Error, Eof]);
        assert_eq!(kinds("any.x"), vec![Any, Dot, Ident("x".into()), Eof]);
        // A word reserved for a later version after a dot is a member name too.
        assert_eq!(kinds("x.derive"), vec![Ident("x".into()), Dot, Ident("derive".into()), Eof]);
    }

    #[test]
    fn non_ascii_identifiers_are_reported_and_skipped() {
        let (tokens, d) = lex_all("héllo");
        assert_eq!(
            tokens.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
            vec![Ident("h".into()), Ident("llo".into()), Eof]
        );
        assert_eq!(d.len(), 1);
        assert_eq!(d.iter().next().unwrap().span, Span::new(FileId(0), 1, 3));
        assert!(d.iter().next().unwrap().message.contains('é'));
    }

    #[test]
    fn integer_literals_take_their_radix() {
        assert_eq!(
            kinds("0 42 0xFF 0o17 0b1010 1_000_000"),
            vec![Int(0), Int(42), Int(255), Int(15), Int(10), Int(1_000_000), Eof]
        );
    }

    #[test]
    fn float_literals_keep_their_spelling() {
        assert_eq!(
            kinds("1.5 1e3 1E+3 1.5e-2 1_0.2_5"),
            vec![
                Float("1.5".into()),
                Float("1e3".into()),
                Float("1E+3".into()),
                Float("1.5e-2".into()),
                Float("10.25".into()),
                Eof
            ]
        );
    }

    #[test]
    fn malformed_numbers_report_once_and_still_produce_a_token() {
        for (src, message) in [
            ("1u8", "invalid number literal `1u8`"),
            ("0x", "invalid number literal `0x`"),
            ("0b2", "invalid number literal `0b2`"),
            ("0xFFg", "invalid number literal `0xFFg`"),
            ("1e", "invalid number literal `1e`"),
        ] {
            let (tokens, d) = lex_all(src);
            assert_eq!(d.len(), 1, "{src:?}");
            assert_eq!(d.iter().next().unwrap().message, message, "{src:?}");
            assert_eq!(tokens[0].kind, Int(0), "{src:?}");
        }
    }

    #[test]
    fn too_large_integer_literals_are_literal_range_errors() {
        assert_eq!(kinds(&u128::MAX.to_string()), vec![Int(u128::MAX), Eof]);
        let too_big = "340282366920938463463374607431768211456";
        let (tokens, d) = lex_all(too_big);
        assert_eq!(tokens[0].kind, Int(0));
        assert_eq!(d.len(), 1);
        let diag = d.iter().next().unwrap();
        assert_eq!(diag.code, Code::LITERAL_RANGE);
        assert_eq!(diag.span, Span::new(FileId(0), 0, too_big.len() as u32));
    }

    #[test]
    fn numbers_after_a_dot_are_plain_digits() {
        // `t.0.1` is three tokens plus a field access, not a float; `1.5` is a float.
        assert_eq!(kinds("t.0.1 1.5"), vec![Ident("t".into()), Dot, Int(0), Dot, Int(1), Float("1.5".into()), Eof]);
    }

    #[test]
    fn string_literals_unescape_and_normalize_crlf() {
        assert_eq!(
            kinds(r#""tab\t nl\n nul\0 quote\" slash\\""#),
            vec![Str("tab\t nl\n nul\0 quote\" slash\\".into()), Eof]
        );
        assert_eq!(kinds("\"a\r\nb\""), vec![Str("a\nb".into()), Eof]);
        assert_eq!(kinds("\"é\""), vec![Str("é".into()), Eof]);
    }

    #[test]
    fn unknown_escape_is_reported_once_and_dropped() {
        let (tokens, d) = lex_all(r#""a\qb""#);
        assert_eq!(tokens[0].kind, Str("ab".into()));
        assert_eq!(d.len(), 1);
        assert!(d.iter().next().unwrap().message.contains("unknown escape sequence"));
    }

    #[test]
    fn char_literals_handle_escapes_and_unicode() {
        assert_eq!(
            kinds(r"'a' '\n' '\\' '\'' 'é' '\0'"),
            vec![Char('a'), Char('\n'), Char('\\'), Char('\''), Char('é'), Char('\0'), Eof]
        );
    }

    #[test]
    fn malformed_char_literals_report_exactly_one_character() {
        let (tokens, d) = lex_all("'' 'ab'");
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|x| x.message.contains("exactly one character")));
        assert_eq!(tokens[0].kind, Char('\0'));
        assert_eq!(tokens[1].kind, Char('a'));
    }

    #[test]
    fn unterminated_char_literal_recovers_at_the_newline() {
        // `'a` with no closing quote is a loop label now (issue #275), so the quote must be followed by something
        // other than an identifier to be an unterminated character literal.
        let (tokens, d) = lex_all("'\nb");
        assert_eq!(d.len(), 1);
        assert!(d.iter().next().unwrap().message.contains("unterminated character literal"));
        assert_eq!(tokens[1].kind, Ident("b".into()));
        assert_eq!(tokens[2].kind, Eof);
    }

    #[test]
    fn token_spans_and_newlines_are_recorded() {
        let (tokens, d) = lex_all("a + 22\nbb");
        assert!(!d.has_errors());
        let seen: Vec<(TokenKind, u32, u32, bool)> =
            tokens.iter().map(|t| (t.kind.clone(), t.span.lo, t.span.hi, t.nl_before)).collect();
        assert_eq!(
            seen,
            vec![
                (Ident("a".into()), 0, 1, false),
                (Plus, 2, 3, false),
                (Int(22), 4, 6, false),
                (Ident("bb".into()), 7, 9, true),
                (Eof, 9, 9, false),
            ]
        );
        // A CRLF is one line break, not two.
        let (tokens, _) = lex_all("a\r\nb");
        assert!(tokens[1].nl_before);
    }

    #[test]
    fn comments_produce_no_tokens() {
        assert_eq!(
            kinds("a // b c\nd /* e\nf */ g"),
            vec![Ident("a".into()), Ident("d".into()), Ident("g".into()), Eof]
        );
    }

    #[test]
    fn unknown_characters_are_skipped_whole() {
        let (tokens, d) = lex_all("# é @");
        assert_eq!(tokens.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(), vec![At, Eof]);
        let messages: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(messages, ["unexpected character `#`", "unexpected character `é`"]);
    }

    #[test]
    fn unterminated_comment_truncates_the_file() {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), "a /* no end", &mut d);
        assert!(r.truncated);
        assert_eq!(r.tokens.len(), 2);
        assert_eq!(r.tokens[1].kind, Eof);
        assert_eq!(d.len(), 1);
        assert_eq!(d.iter().next().unwrap().message, "unterminated block comment");
    }

    #[test]
    fn numbers_and_dots() {
        assert_eq!(kinds("0..3"), vec![Int(0), DotDot, Int(3), Eof]);
        assert_eq!(kinds("t.0.1"), vec![Ident("t".into()), Dot, Int(0), Dot, Int(1), Eof]);
        assert_eq!(
            kinds("2.5 1e3 0xFF 1_000"),
            vec![Float("2.5".into()), Float("1e3".into()), Int(255), Int(1000), Eof]
        );
    }

    #[test]
    fn strings_and_escapes() {
        assert_eq!(kinds(r#""a\"b\\c\n""#), vec![Str("a\"b\\c\n".into()), Eof]);
        assert_eq!(kinds(r"'\''"), vec![Char('\''), Eof]);
    }

    #[test]
    fn newlines_are_recorded() {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), "a /* x\n */ b\nc", &mut d);
        let nl: Vec<bool> = r.tokens.iter().map(|t| t.nl_before).collect();
        assert_eq!(nl, vec![false, true, true, false]);
    }

    #[test]
    fn unterminated_string_stops() {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), "f(\"abc)\n}\n", &mut d);
        assert!(r.truncated);
        assert_eq!(d.len(), 1);
    }

    /// The literal text and holes of one interpolated string, with each hole's expression tokens and specifier.
    fn interp(src: &str) -> Vec<(String, Option<String>)> {
        let tokens = kinds(src);
        let Interp(pieces) = &tokens[0] else { panic!("not an interpolated string: {tokens:?}") };
        pieces
            .iter()
            .map(|piece| match piece {
                InterpPiece::Lit(text) => (format!("lit {text}"), None),
                InterpPiece::Hole(hole) => {
                    let expr: Vec<String> = hole.tokens.iter().map(|t| format!("{:?}", t.kind)).collect();
                    (expr.join(" "), hole.spec.as_ref().map(|(text, _)| text.clone()))
                }
            })
            .collect()
    }

    #[test]
    fn interpolated_strings_split_into_text_and_holes() {
        let pieces = interp(r#""a {name} b""#);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0], ("lit a ".to_string(), None));
        assert_eq!(pieces[1].0, r#"Ident("name") Eof"#);
        assert_eq!(pieces[2], ("lit  b".to_string(), None));

        // `{{` and `}}` are text, a hole holds any expression, and a nested string is one token of it.
        assert_eq!(kinds(r#""{{x}} and {{y}}""#)[0], Str("{x} and {y}".into()));
        let pieces = interp(r#""{a + b:>8}|{"x"}""#);
        assert_eq!(pieces[0].0, r#"Ident("a") Plus Ident("b") Eof"#);
        assert_eq!(pieces[0].1, Some(">8".to_string()));
        assert_eq!(pieces[1], ("lit |".to_string(), None));
        assert_eq!(pieces[2].0, r#"Str("x") Eof"#);

        // A colon inside brackets belongs to the expression; the first one outside starts the specifier.
        let pieces = interp(r#""{m[0]:?}""#);
        assert_eq!(pieces[0].0, r#"Ident("m") LBracket Int(0) RBracket Eof"#);
        assert_eq!(pieces[0].1, Some("?".to_string()));
    }

    #[test]
    fn raw_strings_have_no_escapes_or_holes() {
        assert_eq!(kinds(r#"r"a\n{x}""#)[0], Str(r"a\n{x}".into()));
        assert_eq!(kinds(r##"r#"a "q" {x}"#"##)[0], Str(r#"a "q" {x}"#.into()));
        // `r` followed by anything but `#`s and a quote is an ordinary identifier.
        assert_eq!(kinds("r + x"), vec![Ident("r".into()), Plus, Ident("x".into()), Eof]);
    }

    #[test]
    fn brace_errors_are_reported_once_the_string_ends() {
        let mut d = Diagnostics::default();
        let r = lex(FileId(0), r#""a {x""#, &mut d);
        assert!(!r.truncated);
        let rendered = d.render(&{
            let mut map = burdin_diag::SourceMap::default();
            map.add("t".into(), r#""a {x""#.into());
            map
        });
        assert!(rendered.contains("unterminated `{`"), "{rendered}");
        let mut d = Diagnostics::default();
        lex(FileId(0), r#""a } b""#, &mut d);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn a_quote_starts_a_loop_label_or_a_character_literal() {
        // A quote that closes after one character or name is a character literal; any other quote is a label.
        assert_eq!(kinds("'outer: loop"), vec![Label("outer".into()), Colon, Loop, Eof]);
        assert_eq!(kinds("break 'a"), vec![Break, Label("a".into()), Eof]);
        assert_eq!(kinds("'a'"), vec![Char('a'), Eof]);
        assert_eq!(kinds("'_'"), vec![Char('_'), Eof]);
        assert_eq!(kinds("'\\n'"), vec![Char('\n'), Eof]);
        assert_eq!(kinds("'x_1: for"), vec![Label("x_1".into()), Colon, For, Eof]);
    }
}
