//! Syntax highlighting for the line being typed (`spec/repl.md`, RP12): the same token classes as the editor
//! grammar (`editors/vscode/syntaxes/burdin.tmLanguage.json`), rendered as ANSI colors. It colors one line at a
//! time and never changes what is displayed, so the plain fallback (no terminal colors) is always readable.

use burdin_diag::{Diagnostics, FileId};
use burdin_syntax::token::TokenKind;

/// The token classes the grammar names, and the color each gets. The names in the comments are the grammar's
/// scope names, so the two stay comparable (`tests/check_lsp.py` checks the grammar is the lexer's).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// `keyword.control` and `keyword.declaration`.
    Keyword,
    /// `storage.modifier`.
    Modifier,
    /// `constant.language`.
    Constant,
    /// `variable.language.self`.
    SelfName,
    /// `constant.numeric.*`.
    Number,
    /// `string.quoted.*` and `constant.character`.
    Text,
    /// `comment.line.*` and `comment.block.*`.
    Comment,
    /// `support.type.primitive`.
    Primitive,
    /// `entity.name.type`.
    Type,
    /// `entity.name.function.call` and `entity.name.function`.
    Function,
    /// `invalid.illegal.reserved`.
    Reserved,
}

impl Class {
    fn color(self) -> &'static str {
        match self {
            Class::Keyword => "\x1b[94m",
            Class::Modifier => "\x1b[94m",
            Class::Constant => "\x1b[95m",
            Class::SelfName => "\x1b[96m",
            Class::Number => "\x1b[93m",
            Class::Text => "\x1b[92m",
            Class::Comment => "\x1b[90m",
            Class::Primitive => "\x1b[96m",
            Class::Type => "\x1b[36m",
            Class::Function => "\x1b[94m",
            Class::Reserved => "\x1b[91m",
        }
    }
}

const RESET: &str = "\x1b[0m";

/// The primitive type names the grammar colors (`support.type.primitive`).
const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128",
    "usize",
];

/// One line with its tokens colored. `text` is echoed as it is apart from the color codes, so the editor can
/// compute the cursor position from the uncolored width.
pub(crate) fn line(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    if comment_start(text) == Some(0) {
        return format!("{}{}{RESET}", Class::Comment.color(), text);
    }
    let mut diags = Diagnostics::default();
    let lexed = burdin_syntax::lexer::lex(FileId(0), text, &mut diags);
    let mut at = 0usize;
    for (index, token) in lexed.tokens.iter().enumerate() {
        if matches!(token.kind, TokenKind::Eof) {
            break;
        }
        let (lo, hi) = (token.span.lo as usize, token.span.hi as usize);
        if lo > text.len() || hi > text.len() || lo < at {
            continue;
        }
        if let Some(start) = comment_start(&text[at..]).map(|s| at + s).filter(|s| *s < lo) {
            out.push_str(&text[at..start]);
            out.push_str(Class::Comment.color());
            out.push_str(&text[start..]);
            out.push_str(RESET);
            return out;
        }
        out.push_str(&text[at..lo]);
        let class = class_of(&lexed.tokens, index);
        match class {
            Some(class) => {
                out.push_str(class.color());
                out.push_str(&text[lo..hi]);
                out.push_str(RESET);
            }
            None => out.push_str(&text[lo..hi]),
        }
        at = hi;
    }
    if let Some(start) = comment_start(&text[at.min(text.len())..]).map(|s| at + s) {
        out.push_str(&text[at..start]);
        out.push_str(Class::Comment.color());
        out.push_str(&text[start..]);
        out.push_str(RESET);
    } else {
        out.push_str(&text[at.min(text.len())..]);
    }
    out
}

/// The class of the token at `index`, decided like the grammar's patterns: keywords directly, a name before `(`
/// as a call, a capitalized name or a primitive as a type, and a name after `fn` as a declaration.
fn class_of(tokens: &[burdin_syntax::token::Token], index: usize) -> Option<Class> {
    let kind = &tokens[index].kind;
    let class = match kind {
        TokenKind::Break
        | TokenKind::Continue
        | TokenKind::Else
        | TokenKind::For
        | TokenKind::If
        | TokenKind::In
        | TokenKind::Loop
        | TokenKind::Match
        | TokenKind::Orelse
        | TokenKind::Return
        | TokenKind::While
        | TokenKind::Const
        | TokenKind::Enum
        | TokenKind::Error
        | TokenKind::Extern
        | TokenKind::Fn
        | TokenKind::Impl
        | TokenKind::Interface
        | TokenKind::Let
        | TokenKind::Struct
        | TokenKind::Type
        | TokenKind::Use => Class::Keyword,
        TokenKind::Any | TokenKind::Mut | TokenKind::Own | TokenKind::Priv | TokenKind::Unsafe | TokenKind::Where => {
            Class::Modifier
        }
        TokenKind::As => Class::Keyword,
        TokenKind::False | TokenKind::True => Class::Constant,
        TokenKind::SelfValue | TokenKind::SelfType => Class::SelfName,
        TokenKind::Int(_) | TokenKind::Float(_) => Class::Number,
        TokenKind::Str(_) | TokenKind::Interp(_) | TokenKind::Char(_) => Class::Text,
        TokenKind::Reserved(_) => Class::Reserved,
        // A loop label reads as a keyword, the way `break` does.
        TokenKind::Label(_) => Class::Keyword,
        TokenKind::Ident(name) => {
            let next = tokens.get(index + 1).map(|t| &t.kind);
            let previous = index.checked_sub(1).map(|i| &tokens[i].kind);
            if PRIMITIVES.contains(&name.as_str()) {
                Class::Primitive
            } else if previous.is_some_and(|p| matches!(p, TokenKind::Fn)) {
                Class::Function
            } else if previous.is_some_and(|p| {
                matches!(
                    p,
                    TokenKind::Struct | TokenKind::Enum | TokenKind::Type | TokenKind::Interface | TokenKind::Error
                )
            }) {
                Class::Type
            } else if next.is_some_and(|n| matches!(n, TokenKind::LParen)) {
                Class::Function
            } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                Class::Type
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(class)
}

/// Where a `//` comment starts, if one does: outside a string, a character literal, or a raw string.
fn comment_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => return Some(i),
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => return Some(i),
            b'"' => i = skip_string(text, i),
            b'\'' => i = skip_char(text, i),
            b'r' if i + 1 < bytes.len() && (bytes[i + 1] == b'"' || bytes[i + 1] == b'#') => {
                i = skip_raw_string(text, i)
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_string(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn skip_char(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\'' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// `r"..."` and `r#"..."#`.
fn skip_raw_string(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut hashes = 0;
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] == b'#' {
        hashes += 1;
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return start + 1;
    }
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let end = i + 1 + hashes;
            if end <= bytes.len() && bytes[i + 1..end].iter().all(|b| *b == b'#') {
                return end;
            }
        }
        i += 1;
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_added_without_changing_the_text() {
        let line = "let x = 1 + 2 // a note";
        let colored = line_plain(line);
        assert_eq!(colored, line, "the visible text must be the source line");
    }

    /// The colored line with its escape sequences removed.
    fn line_plain(text: &str) -> String {
        let mut out = String::new();
        let mut in_escape = false;
        for c in line(text).chars() {
            if in_escape {
                if c == 'm' {
                    in_escape = false;
                }
            } else if c == '\x1b' {
                in_escape = true;
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn comments_are_found_outside_literals() {
        assert_eq!(comment_start("let x = 1 // note"), Some(10));
        assert_eq!(comment_start("let s = \"a // b\""), None);
        assert_eq!(comment_start("let c = '/'"), None);
        assert_eq!(comment_start("let s = r#\"//\"# // note"), Some(16));
        assert_eq!(comment_start("// whole line"), Some(0));
    }
}
