//! The parser's look-ahead decisions as functions over a token slice, so that every tool that reads Burdin source
//! (the parser, the formatter) answers them the same way. None of them consumes anything.

use crate::token::{Token, TokenKind};

fn kind_at(toks: &[Token], i: usize) -> Option<&TokenKind> {
    toks.get(i).map(|t| &t.kind)
}

fn continues_after_angle(toks: &[Token], i: usize) -> bool {
    matches!(kind_at(toks, i), Some(TokenKind::Dot | TokenKind::LParen | TokenKind::LBrace))
}

/// Grammar rule 8.2: after an identifier that starts with an uppercase letter, a `<` starts generic arguments if a
/// type list parses and its closing `>` is followed by `.`, `(`, or `{`. Otherwise the `<` is a comparison. The
/// test is a scan ahead, so a comparison never leaves a `>>` split in its wake. `pos` is the identifier.
pub fn generic_args_ahead(toks: &[Token], pos: usize) -> bool {
    let Some(TokenKind::Ident(name)) = kind_at(toks, pos) else { return false };
    if !name.starts_with(|c: char| c.is_ascii_uppercase()) || !matches!(kind_at(toks, pos + 1), Some(TokenKind::Lt)) {
        return false;
    }
    let mut depth = 0i32;
    // Parentheses and brackets inside a type (`Fn(i32)`, `(i32, i32)`) do not end the scan; only a closer that is
    // not matched inside it can belong to the surrounding expression.
    let mut parens = 0i32;
    let mut i = pos + 1;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt => {
                depth -= 1;
                if depth <= 0 {
                    return depth == 0 && continues_after_angle(toks, i + 1);
                }
            }
            // `>>` can close two levels; `>=` can never close a generic argument list.
            TokenKind::Shr => {
                depth -= 2;
                if depth <= 0 {
                    return depth == 0 && continues_after_angle(toks, i + 1);
                }
            }
            // A `{ ... }` inside the list is a const argument (`Matrix<f64, { N + 1 }, C>`).
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => parens += 1,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                if parens == 0 {
                    return false;
                }
                parens -= 1;
            }
            TokenKind::Ge | TokenKind::Eof if parens == 0 => return false,
            _ => {}
        }
        i += 1;
    }
    false
}

/// Whether the `<` at `pos` (right after a member name) starts a type-argument list rather than a comparison: a
/// type list parses and its `>` is followed by `(`, `.`, or `{`. Comparison operators do not chain
/// (spec/grammar.md), so `a.b < c > (d)` is already invalid and cannot be made ambiguous by this rule.
pub fn member_type_args_ahead(toks: &[Token], pos: usize) -> bool {
    if !matches!(kind_at(toks, pos), Some(TokenKind::Lt)) {
        return false;
    }
    let mut depth = 0i32;
    let mut parens = 0i32;
    let mut i = pos;
    while let Some(t) = toks.get(i) {
        match &t.kind {
            TokenKind::Lt => depth += 1,
            TokenKind::Gt => {
                depth -= 1;
                if depth <= 0 {
                    return depth == 0 && continues_after_angle(toks, i + 1);
                }
            }
            TokenKind::Shr => {
                depth -= 2;
                if depth <= 0 {
                    return depth == 0 && continues_after_angle(toks, i + 1);
                }
            }
            TokenKind::LParen | TokenKind::LBracket => parens += 1,
            TokenKind::RParen | TokenKind::RBracket => {
                if parens == 0 {
                    return false;
                }
                parens -= 1;
            }
            TokenKind::Ge | TokenKind::Eof | TokenKind::RBrace if parens == 0 => return false,
            _ => {}
        }
        i += 1;
    }
    false
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
        Some(TokenKind::RBrace) => true,
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
    )
}
