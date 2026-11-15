//! The lexer (spec/grammar.md, section 1).

use crate::token::{Token, TokenKind};
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

struct Lexer<'a> {
    file: FileId,
    src: &'a [u8],
    pos: usize,
    diags: &'a mut Diagnostics,
    tokens: Vec<Token>,
    nl_before: bool,
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
            TokenKind::keyword(word).unwrap_or_else(|| TokenKind::Ident(word.to_string()))
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
        // The source text is valid UTF-8 and escapes produce whole characters, so this cannot fail.
        let text = String::from_utf8(bytes).unwrap_or_default();
        self.push(TokenKind::Str(text), lo);
        Ok(())
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
            ("as", As),
            ("break", Break),
            ("const", Const),
            ("continue", Continue),
            ("else", Else),
            ("enum", Enum),
            ("extern", Extern),
            ("false", False),
            ("fn", Fn),
            ("for", For),
            ("if", If),
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
            ("any", Reserved("any".into())),
            ("comptime", Reserved("comptime".into())),
            ("derive", Reserved("derive".into())),
            ("impl", Reserved("impl".into())),
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
        let (tokens, d) = lex_all("'a\nb");
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
}
