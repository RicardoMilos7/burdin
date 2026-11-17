//! Interpolated string literals: the placeholders the lexer found, and the format specifier after a `:`
//! (spec/format.md).

use super::Parser;
use crate::ast::*;
use crate::token::{HoleTokens, InterpPiece, TokenKind};
use burdin_diag::{Code, Span};

impl Parser<'_> {
    /// Turns the lexer's pieces into AST parts: each placeholder's tokens are parsed as an expression.
    pub(super) fn interp_parts(&mut self, pieces: Vec<InterpPiece>) -> Vec<InterpPart> {
        pieces
            .into_iter()
            .filter_map(|piece| match piece {
                InterpPiece::Lit(text) => Some(InterpPart::Lit(text)),
                InterpPiece::Hole(hole) => self.hole(*hole).map(InterpPart::Hole),
            })
            .collect()
    }

    fn hole(&mut self, hole: HoleTokens) -> Option<Hole> {
        let spec = match &hole.spec {
            Some((text, span)) => parse_spec(text, *span, self.diags),
            None => FormatSpec::plain(hole.span),
        };
        let kinds: Vec<&TokenKind> = hole.tokens.iter().map(|t| &t.kind).collect();
        let arg = match kinds.as_slice() {
            [TokenKind::Eof] => HoleArg::Next,
            [TokenKind::Int(n), TokenKind::Eof] => HoleArg::Index(usize::try_from(*n).unwrap_or(usize::MAX)),
            _ => {
                let mut sub = Parser {
                    file: self.file,
                    toks: hole.tokens,
                    pos: 0,
                    diags: &mut *self.diags,
                    nl_stack: vec![false],
                    truncated: false,
                    test_count: 0,
                    comptime_asserts: 0,
                    select_count: 0,
                };
                let expr = sub.expr().ok()?;
                if !sub.at_eof() {
                    sub.unexpected("`}` or `:` after the expression");
                    return None;
                }
                HoleArg::Expr(Box::new(expr))
            }
        };
        Some(Hole { arg, spec, span: hole.span })
    }
}

/// Parses `[[fill]align][+][#][0][width][.precision][type]`. A malformed specifier is E0190 and formats like the
/// plain placeholder.
pub(super) fn parse_spec(text: &str, span: Span, diags: &mut burdin_diag::Diagnostics) -> FormatSpec {
    let mut spec = FormatSpec::plain(span);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let align_of = |c: char| match c {
        '<' => Some(Align::Left),
        '^' => Some(Align::Center),
        '>' => Some(Align::Right),
        _ => None,
    };
    if chars.len() >= 2
        && let Some(a) = align_of(chars[1])
    {
        spec.fill = chars[0];
        spec.align = a;
        i = 2;
    } else if let Some(a) = chars.first().copied().and_then(align_of) {
        spec.align = a;
        i = 1;
    }
    if chars.get(i) == Some(&'+') {
        spec.plus = true;
        i += 1;
    }
    if chars.get(i) == Some(&'#') {
        spec.alt = true;
        i += 1;
    }
    if chars.get(i) == Some(&'0') {
        spec.zero = true;
        i += 1;
    }
    let number = |i: &mut usize| -> Option<usize> {
        let start = *i;
        while chars.get(*i).is_some_and(|c| c.is_ascii_digit()) {
            *i += 1;
        }
        if *i == start {
            return None;
        }
        let digits: String = chars[start..*i].iter().collect();
        Some(digits.parse().unwrap_or(usize::MAX))
    };
    spec.width = number(&mut i);
    if chars.get(i) == Some(&'.') {
        i += 1;
        spec.precision = number(&mut i);
        if spec.precision.is_none() {
            diags.error(Code::BAD_FORMAT_SPEC, span, "a `.` in a format specifier needs a number of digits after it");
            return FormatSpec::plain(span);
        }
    }
    let rest: String = chars[i..].iter().collect();
    spec.kind = match rest.as_str() {
        "" => SpecKind::Display,
        "?" => SpecKind::Debug,
        "x" => SpecKind::LowerHex,
        "X" => SpecKind::UpperHex,
        "b" => SpecKind::Binary,
        "o" => SpecKind::Octal,
        other => {
            diags.error(
                Code::BAD_FORMAT_SPEC,
                span,
                format!("unknown format specifier `{other}`; the type letters are `?`, `x`, `X`, `b`, and `o`"),
            );
            return FormatSpec::plain(span);
        }
    };
    if spec.width.is_some_and(|w| w > 4096) || spec.precision.is_some_and(|p| p > 4096) {
        diags.error(Code::BAD_FORMAT_SPEC, span, "a width or precision is at most 4096");
        return FormatSpec::plain(span);
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::{Diagnostics, FileId};

    fn spec(text: &str) -> (FormatSpec, usize) {
        let mut diags = Diagnostics::default();
        let s = parse_spec(text, Span::new(FileId(0), 0, 0), &mut diags);
        (s, diags.iter().count())
    }

    #[test]
    fn parses_every_part() {
        let (s, errors) = spec("*^+#012.3x");
        assert_eq!(errors, 0);
        assert_eq!((s.fill, s.align, s.plus, s.alt, s.zero), ('*', Align::Center, true, true, true));
        assert_eq!((s.width, s.precision, s.kind), (Some(12), Some(3), SpecKind::LowerHex));
    }

    #[test]
    fn align_without_fill_and_debug() {
        let (s, errors) = spec(">8?");
        assert_eq!(errors, 0);
        assert_eq!((s.fill, s.align, s.width, s.kind), (' ', Align::Right, Some(8), SpecKind::Debug));
    }

    #[test]
    fn empty_spec_is_plain() {
        let (s, errors) = spec("");
        assert_eq!(errors, 0);
        assert!(s.is_default());
    }

    #[test]
    fn bad_specs_are_reported() {
        assert_eq!(spec("q").1, 1);
        assert_eq!(spec(".").1, 1);
        assert_eq!(spec("99999").1, 1);
    }
}
