//! Follow-on errors of a program with syntax errors (`spec/check.md`, CK4).
//!
//! A file with a syntax error is not type checked: the diagnostics of its own checks are dropped, and the other
//! modules are checked against the declarations the parser did read. A declaration the parser could not read is
//! missing from that tree, so a use of it in another module would be reported as unknown. Such a diagnostic is
//! dropped when it mentions a name the unreadable declaration declares: when in doubt, the follow-on error goes.

use std::collections::BTreeSet;

use burdin_diag::{Diagnostic, Diagnostics, FileId, SourceMap, Span};
use burdin_syntax::lexer;
use burdin_syntax::token::{Token, TokenKind};

/// The names declared inside `regions` (the items the parser could not read, `ast::File::unparsed`): the name of
/// every item (`fn`, `struct`, `enum`, `error`, `interface`, `type`, `const`), the method names of every `fn`, the
/// interface and type named in an `impl` header, the variant names of an `enum`, and the names a `use` imports.
///
/// The names come from tokens, not from the tree, because a declaration that failed to parse is not in the tree at
/// all. Anything the tree kept is left out on purpose: its names resolve, so a diagnostic that mentions one is a
/// real error. Parameter names, field names, payload types, and the module path of an import are left out too.
pub(crate) fn declared_names(text: &str, file: FileId, regions: &[Span]) -> BTreeSet<String> {
    let tokens = lexer::lex(file, text, &mut Diagnostics::default()).tokens;
    let mut names = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if !regions.iter().any(|r| r.lo <= token.span.lo && token.span.lo < r.hi) {
            continue;
        }
        match &token.kind {
            // `fn name`, `fn Type.name` (an extension method), and every method of an `impl` or `interface`.
            TokenKind::Fn => {
                insert_ident(&tokens, index + 1, &mut names);
                if tokens.get(index + 2).is_some_and(|t| t.kind == TokenKind::Dot) {
                    insert_ident(&tokens, index + 3, &mut names);
                }
            }
            TokenKind::Struct | TokenKind::Error | TokenKind::Interface | TokenKind::Type | TokenKind::Const => {
                insert_ident(&tokens, index + 1, &mut names);
            }
            TokenKind::Enum => {
                insert_ident(&tokens, index + 1, &mut names);
                enum_variants(&tokens, index + 1, &mut names);
            }
            TokenKind::Impl => {
                for at in header(&tokens, index + 1, true) {
                    insert_ident(&tokens, at, &mut names);
                }
            }
            // Only the leaves of an import are names: `use a.b.{c, d}` declares `c` and `d`, not `a` or `b`.
            TokenKind::Use => {
                for at in header(&tokens, index + 1, false) {
                    let is_path = tokens.get(at + 1).is_some_and(|t| t.kind == TokenKind::Dot);
                    if !is_path {
                        insert_ident(&tokens, at, &mut names);
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// Whether `diagnostic` mentions one of `names`: an identifier in the text of its span, or in a backticked name of
/// its message (`` unknown name `oops` ``).
pub(crate) fn mentions(diagnostic: &Diagnostic, names: &BTreeSet<String>, sources: &SourceMap) -> bool {
    let span = diagnostic.span;
    let source = sources.get(span.file);
    let span_text = source.text.get(span.lo as usize..span.hi as usize).unwrap_or("");
    let quoted = diagnostic.message.split('`').skip(1).step_by(2);
    identifiers(span_text).chain(quoted.flat_map(identifiers)).any(|word| names.contains(word))
}

/// Keeps the diagnostics before index `from` (the syntax errors) and every later one that is not a follow-on: a
/// diagnostic in one of the `broken` files is dropped, and so is one that mentions one of `names`.
pub(crate) fn retain_checked(
    diags: Diagnostics,
    from: usize,
    broken: &[FileId],
    names: &BTreeSet<String>,
    sources: &SourceMap,
) -> Diagnostics {
    let mut kept = Diagnostics::default();
    for (index, diagnostic) in diags.into_vec().into_iter().enumerate() {
        let follow_on =
            index >= from && (broken.contains(&diagnostic.span.file) || mentions(&diagnostic, names, sources));
        if !follow_on {
            kept.push(diagnostic);
        }
    }
    kept
}

/// The identifiers of `text`: maximal runs of letters, digits, and `_`.
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|word| !word.is_empty())
}

fn insert_ident(tokens: &[Token], index: usize, names: &mut BTreeSet<String>) {
    if let Some(Token { kind: TokenKind::Ident(name), .. }) = tokens.get(index) {
        names.insert(name.clone());
    }
}

/// The token indices of a declaration's header from `from`: up to the end of its line at bracket depth zero (a `use`
/// may span lines inside its braces), and, with `stop_at_brace`, up to the `{` of an `impl`'s body.
fn header(tokens: &[Token], from: usize, stop_at_brace: bool) -> Vec<usize> {
    let mut found = Vec::new();
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(from) {
        if index > from && depth == 0 && (token.nl_before || token.kind == TokenKind::Eof) {
            break;
        }
        match &token.kind {
            TokenKind::LBrace if depth == 0 && stop_at_brace => break,
            TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = depth.saturating_sub(1),
            TokenKind::Ident(_) => found.push(index),
            _ => {}
        }
    }
    found
}

/// The variant names of an `enum` body: the identifiers that start an entry at the body's own depth. A payload's
/// types and a struct variant's fields are deeper and are not names of the enum.
fn enum_variants(tokens: &[Token], from: usize, names: &mut BTreeSet<String>) {
    // The body starts at the first `{` on the enum's line, after its name.
    let line = (from..tokens.len()).take_while(|&i| i == from || !tokens[i].nl_before);
    let Some(open) = line.into_iter().find(|&i| tokens[i].kind == TokenKind::LBrace) else {
        return;
    };
    let mut depth = 0usize;
    for index in open..tokens.len() {
        let token = &tokens[index];
        match &token.kind {
            TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            TokenKind::Ident(name) if depth == 1 => {
                let entry_start = matches!(tokens[index - 1].kind, TokenKind::LBrace | TokenKind::Comma);
                if entry_start {
                    names.insert(name.clone());
                }
            }
            TokenKind::Eof => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names of a whole file, which is one region.
    fn names_of(text: &str) -> Vec<String> {
        let whole = Span::new(FileId(0), 0, text.len() as u32);
        declared_names(text, FileId(0), &[whole]).into_iter().collect()
    }

    fn declares(text: &str, expected: &[&str]) {
        let names = names_of(text);
        for name in expected {
            assert!(names.contains(&name.to_string()), "{name} missing from {names:?}");
        }
    }

    #[test]
    fn items_and_their_methods_are_declared_but_parameters_and_fields_are_not() {
        let text = "fn area(p: Point) -> i32 { 0 }\nstruct Shape { x: i32 }\nconst LIMIT: i32 = 3\n";
        declares(text, &["area", "Shape", "LIMIT"]);
        let names = names_of(text);
        for left_out in ["p", "Point", "i32", "x"] {
            assert!(!names.contains(&left_out.to_string()), "{left_out} is declared in {names:?}");
        }
    }

    #[test]
    fn impl_headers_methods_and_extension_methods_count() {
        declares(
            "impl Shape for Circle {\n    fn radius(self) -> i32 { 1 }\n}\nfn Circle.area(self) {}\n",
            &["Shape", "Circle", "radius", "area"],
        );
    }

    #[test]
    fn enum_variants_count_but_payload_types_and_fields_do_not() {
        let names = names_of("enum Shape {\n    Circle(f64),\n    Square { side: i32 },\n    Empty,\n}\n");
        for expected in ["Shape", "Circle", "Square", "Empty"] {
            assert!(names.contains(&expected.to_string()), "{expected} missing from {names:?}");
        }
        assert!(!names.contains(&"f64".to_string()));
        assert!(!names.contains(&"side".to_string()));
    }

    #[test]
    fn imports_declare_their_leaves_not_their_module_path() {
        let names = names_of("pub use other.first\nuse third.{fourth, Fifth}\n");
        for expected in ["first", "fourth", "Fifth"] {
            assert!(names.contains(&expected.to_string()), "{expected} missing from {names:?}");
        }
        assert!(!names.contains(&"other".to_string()));
        assert!(!names.contains(&"third".to_string()));
    }

    #[test]
    fn only_the_unparsed_regions_declare_names() {
        // `fn fine` parsed, so only the region of the unreadable `fn broken` declares anything.
        let text = "fn broken( -> {\nfn fine() {}\n";
        let region = Span::new(FileId(0), 0, text.find("fn fine").unwrap() as u32);
        let names: Vec<String> = declared_names(text, FileId(0), &[region]).into_iter().collect();
        assert_eq!(names, ["broken"]);
    }

    #[test]
    fn a_mention_is_an_identifier_of_the_span_or_a_backticked_name() {
        let mut sources = SourceMap::default();
        let id = sources.add("a.bd".into(), "let v = area(2)\n".into());
        let names: BTreeSet<String> = ["area".to_string()].into_iter().collect();
        let call = Diagnostic::new(
            burdin_diag::Code::UNKNOWN_NAME,
            burdin_diag::Severity::Error,
            Span::new(id, 8, 12),
            "unknown function `area`",
        );
        assert!(mentions(&call, &names, &sources));
        let other = Diagnostic::new(
            burdin_diag::Code::TYPE_MISMATCH,
            burdin_diag::Severity::Error,
            Span::new(id, 0, 3),
            "expected `i32`, found `str`",
        );
        assert!(!mentions(&other, &names, &sources));
    }
}
