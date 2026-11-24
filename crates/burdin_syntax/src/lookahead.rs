//! The parser's look-ahead decisions as functions over a token slice, so that every tool that reads Burdin source
//! (the parser, the formatter) answers them the same way. None of them consumes anything.

use crate::token::{Token, TokenKind};

fn kind_at(toks: &[Token], i: usize) -> Option<&TokenKind> {
    toks.get(i).map(|t| &t.kind)
}

/// Whether `kind` can appear inside a type argument list: the type grammar (`spec/grammar.md`, section 4), a const
/// argument, or the punctuation around them. Any other token (an operator, a string, a keyword) means the `<` before
/// it is a comparison, so `a < b && c > (d)` is never read as type arguments.
fn is_type_token(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Char(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Any
            | TokenKind::Impl
            | TokenKind::Fn
            | TokenKind::SelfType
            | TokenKind::Mut
            | TokenKind::Dot
            | TokenKind::Comma
            | TokenKind::Colon
            | TokenKind::Semi
            | TokenKind::Amp
            | TokenKind::Bang
            | TokenKind::Minus
            | TokenKind::LParen
            | TokenKind::RParen
            | TokenKind::LBracket
            | TokenKind::RBracket
    )
}

/// The index just past the `}` that closes the brace at `open`, or `None` when the braces do not close.
fn brace_end(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::LBrace => depth += 1,
            TokenKind::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            TokenKind::Eof => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The token index just past the `>` that closes the type argument list opening at `open` (a `<`), when every token
/// inside it is part of a type or a const argument and the brackets balance. `>>` closes two levels; `>=` closes none.
/// A `{ ... }` that starts an argument is a const expression (`Matrix<f64, { N + 1 }, C>`) and is skipped whole.
/// `None` when the list does not close cleanly or holds a token no type or const argument can contain.
fn type_list_end(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut nest = 0i32;
    let mut i = open;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt | TokenKind::Shr => {
                depth -= if t.kind == TokenKind::Gt { 1 } else { 2 };
                if depth == 0 {
                    return (nest == 0).then_some(i + 1);
                }
                if depth < 0 {
                    return None;
                }
            }
            TokenKind::LBrace if matches!(toks.get(i - 1).map(|p| &p.kind), Some(TokenKind::Lt | TokenKind::Comma)) => {
                i = brace_end(toks, i)?;
                continue;
            }
            TokenKind::LParen | TokenKind::LBracket => nest += 1,
            TokenKind::RParen | TokenKind::RBracket => {
                nest -= 1;
                if nest < 0 {
                    return None;
                }
            }
            kind if is_type_token(kind) => {}
            _ => return None,
        }
        i += 1;
    }
    None
}

/// Grammar rule 8.2: after an identifier, a `<` starts generic arguments when a type list parses (`type_list_end`)
/// and its closing `>` is followed by `.`, `(`, or `{`; otherwise the `<` is a comparison. An identifier that starts
/// with an uppercase letter may be followed by any of the three. A lowercase one (a function, as in `f<i32>(x)`) is
/// read only when a call's `(` follows. Two comparisons written as call arguments, `f(a < b, c > (d))`, match that
/// shape and read as type arguments; parenthesize the first comparison (`f((a < b), c > (d))`) to keep it. The test
/// is a scan ahead, so a comparison never leaves a `>>` split in its wake. `pos` is the identifier.
pub fn generic_args_ahead(toks: &[Token], pos: usize) -> bool {
    let Some(TokenKind::Ident(name)) = kind_at(toks, pos) else { return false };
    if !matches!(kind_at(toks, pos + 1), Some(TokenKind::Lt)) {
        return false;
    }
    let Some(end) = type_list_end(toks, pos + 1) else { return false };
    if name.starts_with(|c: char| c.is_ascii_uppercase()) {
        matches!(kind_at(toks, end), Some(TokenKind::Dot | TokenKind::LParen | TokenKind::LBrace))
    } else {
        matches!(kind_at(toks, end), Some(TokenKind::LParen))
    }
}

/// Whether the `<` at `pos` (right after a member name) starts a type-argument list rather than a comparison: a
/// type list parses and its `>` is followed by `(`, `.`, or `{`. Comparison operators do not chain
/// (spec/grammar.md), so `a.b < c > (d)` is already invalid and cannot be made ambiguous by this rule.
pub fn member_type_args_ahead(toks: &[Token], pos: usize) -> bool {
    if !matches!(kind_at(toks, pos), Some(TokenKind::Lt)) {
        return false;
    }
    type_list_end(toks, pos)
        .is_some_and(|end| matches!(kind_at(toks, end), Some(TokenKind::Dot | TokenKind::LParen | TokenKind::LBrace)))
}

/// The token index just past the `]` that closes the bracket opening at `start`. `None` when it does not close
/// cleanly.
pub fn bracket_list_end(toks: &[Token], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::LBracket => depth += 1,
            TokenKind::RBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
                if depth < 0 {
                    return None;
                }
            }
            TokenKind::Eof => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The token index just past the `>` that closes the generic list opening at `start` (a `<`). `>>` closes two
/// levels; `>=` closes none. `None` when the list does not close cleanly.
pub fn generic_list_end(toks: &[Token], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut parens = 0i32;
    let mut i = start;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
                if depth < 0 {
                    return None;
                }
            }
            TokenKind::Shr => {
                depth -= 2;
                if depth == 0 {
                    return Some(i + 1);
                }
                if depth < 0 {
                    return None;
                }
            }
            TokenKind::LParen | TokenKind::LBracket => parens += 1,
            TokenKind::RParen | TokenKind::RBracket => {
                if parens == 0 {
                    return None;
                }
                parens -= 1;
            }
            TokenKind::Ge | TokenKind::Eof | TokenKind::RBrace if parens == 0 => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether the `fn` (or `unsafe fn`) at `pos` is an extension method: an identifier, then either a `.` or a generic
/// argument list followed by a `.`, as in `fn String.shout(...)` or `fn List<T>.sum(...)`, or a slice type followed
/// by a `.`, as in `fn [T].first(...)`.
pub fn extension_ahead(toks: &[Token], pos: usize) -> bool {
    let mut i = pos;
    if matches!(kind_at(toks, i), Some(TokenKind::Unsafe)) {
        i += 1;
    }
    if !matches!(kind_at(toks, i), Some(TokenKind::Fn)) {
        return false;
    }
    i += 1;
    if matches!(kind_at(toks, i), Some(TokenKind::LBracket)) {
        return bracket_list_end(toks, i).is_some_and(|end| matches!(kind_at(toks, end), Some(TokenKind::Dot)));
    }
    if !matches!(kind_at(toks, i), Some(TokenKind::Ident(_))) {
        return false;
    }
    i += 1;
    match kind_at(toks, i) {
        Some(TokenKind::Dot) => true,
        Some(TokenKind::Lt) => {
            generic_list_end(toks, i).is_some_and(|end| matches!(kind_at(toks, end), Some(TokenKind::Dot)))
        }
        _ => false,
    }
}

/// `Name {` starts a struct literal when the brace (at `brace`) is followed by `}` or by `ident` and then `:`, `,`,
/// `}` or a line break. A block after a name (as in `while x { ... }`) is excluded by the restriction instead.
pub fn looks_like_struct_literal(toks: &[Token], brace: usize) -> bool {
    let nth = |n: usize| toks.get((brace + n).min(toks.len().saturating_sub(1))).map(|t| &t.kind);
    match nth(1) {
        Some(TokenKind::RBrace | TokenKind::DotDot) => true,
        Some(TokenKind::Ident(_)) => matches!(nth(2), Some(TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace)),
        _ => false,
    }
}

/// Whether `kind` can start an expression, which decides whether `return` has a value.
pub fn can_start_expr(kind: &TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        Ident(_)
            | Label(_)
            | Int(_)
            | Float(_)
            | Char(_)
            | Str(_)
            | Interp(_)
            | True
            | False
            | SelfValue
            | SelfType
            | LParen
            | LBracket
            | LBrace
            | If
            | Match
            | Loop
            | While
            | For
            | Unsafe
            | Comptime
            | Return
            | Break
            | Continue
            | Minus
            | Bang
            | Star
            | Amp
            | Pipe
            | OrOr
            | Dot
    )
}
