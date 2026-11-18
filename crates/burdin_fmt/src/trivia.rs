//! Tokens with their comments. The lexer drops comments; the formatter must keep every one of them, so this pass
//! reads the text between two tokens (only white space and comments live there) and gives each comment to a
//! token: a comment on the same line as the token before it trails that token, every other comment leads the token
//! after it (spec/fmt.md, FM3).

use burdin_diag::{Diagnostics, FileId};
use burdin_syntax::lexer;
use burdin_syntax::token::{Token, TokenKind};

#[derive(Clone, Debug)]
pub struct Comment {
    /// The comment as written, `//` or `/* */` included. A line comment has no trailing white space.
    pub text: String,
    /// A `//` comment (which includes `///`); otherwise a `/* */` block.
    pub line: bool,
    /// Byte offsets in the source.
    pub lo: usize,
    pub hi: usize,
    /// Line breaks between the thing before the comment (a token or another comment) and the comment.
    pub nl_before: u32,
    /// Line breaks between the comment and the thing after it.
    pub nl_after: u32,
    /// Whether white space follows the comment, which an inline block comment keeps.
    pub space_after: bool,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub kind: TokenKind,
    pub lo: usize,
    pub hi: usize,
    /// The lexer's flag: a line break comes between the previous token and this one (spec/grammar.md, section 2).
    pub nl_before: bool,
    /// Line breaks between the thing before this token (the previous token or a comment) and the token.
    pub nls: u32,
    /// Comments that lead this token, in order.
    pub lead: Vec<Comment>,
    /// Comments on the same line after this token.
    pub trail: Vec<Comment>,
    /// Where the token and its comments start and end in the source: from its first leading comment to its last
    /// trailing one.
    pub start: usize,
    pub end: usize,
}

pub struct Lexed {
    pub toks: Vec<Tok>,
    /// The lexer's own tokens, which the parser's look-ahead functions read.
    pub raw: Vec<Token>,
}

/// Lexes `src` and attaches the comments. The source must be free of lexical errors.
pub fn lex(src: &str) -> Result<Lexed, String> {
    let mut diags = Diagnostics::default();
    let result = lexer::lex(FileId(0), src, &mut diags);
    if diags.has_errors() || result.truncated {
        return Err("the source has lexical errors".to_string());
    }
    let raw = result.tokens.clone();
    Ok(Lexed { toks: attach(src, result.tokens), raw })
}

fn attach(src: &str, tokens: Vec<Token>) -> Vec<Tok> {
    let mut toks: Vec<Tok> = Vec::with_capacity(tokens.len());
    let mut previous_end = 0usize;
    for (index, token) in tokens.into_iter().enumerate() {
        let lo = token.span.lo as usize;
        let hi = token.span.hi as usize;
        let at_eof = token.kind == TokenKind::Eof;
        let scan = scan_gap(src, previous_end, lo);
        let mut comments = scan.comments;
        // Comments on the line of the previous token trail it, provided a line break follows them.
        let mut trailing = 0;
        if index > 0 && (scan.has_newline || at_eof) {
            while trailing < comments.len() && comments[trailing].nl_before == 0 {
                trailing += 1;
            }
        }
        let lead = comments.split_off(trailing);
        if let Some(previous) = toks.last_mut() {
            if let Some(last) = comments.last() {
                previous.end = last.hi;
            }
            previous.trail = comments;
        }
        toks.push(Tok {
            kind: token.kind,
            lo,
            hi,
            nl_before: token.nl_before,
            nls: scan.newlines_at_end,
            start: lead.first().map_or(lo, |c| c.lo),
            lead,
            trail: Vec::new(),
            end: hi,
        });
        previous_end = hi;
    }
    toks
}

struct Gap {
    comments: Vec<Comment>,
    has_newline: bool,
    /// Line breaks after the last comment, or in the whole gap when it holds no comment.
    newlines_at_end: u32,
}

fn scan_gap(src: &str, lo: usize, hi: usize) -> Gap {
    let bytes = src.as_bytes();
    let mut comments: Vec<Comment> = Vec::new();
    let mut has_newline = false;
    let mut newlines = 0u32;
    let mut i = lo;
    while i < hi {
        match bytes[i] {
            b'\n' => {
                has_newline = true;
                newlines += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let start = i;
                while i < hi && bytes[i] != b'\n' {
                    i += 1;
                }
                let text = src[start..i].trim_end().to_string();
                push_comment(&mut comments, text, true, start, i, newlines);
                newlines = 0;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i + 1 < hi && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(hi);
                push_comment(&mut comments, src[start..i].to_string(), false, start, i, newlines);
                newlines = 0;
            }
            _ => i += 1,
        }
    }
    // The line breaks after each comment are the line breaks before the next one; the last count is the gap's tail.
    for k in 0..comments.len() {
        comments[k].nl_after = comments.get(k + 1).map_or(newlines, |next| next.nl_before);
        let after = comments[k].hi;
        comments[k].space_after = after >= hi || matches!(bytes[after], b' ' | b'\t' | b'\r' | b'\n');
    }
    Gap { comments, has_newline, newlines_at_end: newlines }
}

fn push_comment(comments: &mut Vec<Comment>, text: String, line: bool, lo: usize, hi: usize, nl_before: u32) {
    comments.push(Comment { text, line, lo, hi, nl_before, nl_after: 0, space_after: true });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).expect("lexes").toks
    }

    #[test]
    fn a_comment_on_the_line_of_a_token_trails_it() {
        let toks = toks("a // one\n// two\nb\n");
        assert_eq!(toks[0].trail.len(), 1);
        assert_eq!(toks[0].trail[0].text, "// one");
        assert_eq!(toks[1].lead.len(), 1);
        assert_eq!(toks[1].lead[0].text, "// two");
        assert_eq!(toks[1].lead[0].nl_before, 1);
        assert_eq!(toks[1].nls, 1);
    }

    #[test]
    fn a_block_comment_between_tokens_on_one_line_leads_the_next() {
        let toks = toks("a /* x */ b\n");
        assert!(toks[0].trail.is_empty());
        assert_eq!(toks[1].lead[0].text, "/* x */");
        assert!(toks[1].lead[0].space_after);
    }

    #[test]
    fn blank_lines_are_counted() {
        let toks = toks("a\n\n// c\n\nb\n");
        assert_eq!(toks[1].lead[0].nl_before, 2);
        assert_eq!(toks[1].lead[0].nl_after, 2);
        assert_eq!(toks[1].nls, 2);
    }

    #[test]
    fn comments_at_the_end_of_the_file_belong_to_the_end() {
        let toks = toks("a // end\n// tail\n");
        assert_eq!(toks[0].trail[0].text, "// end");
        let eof = toks.last().unwrap();
        assert_eq!(eof.lead[0].text, "// tail");
    }
}
