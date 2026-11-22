//! Doc comments (`spec/doc.md`, DC1-DC4).
//!
//! The lexer drops comments, so doc comments are read from the source text: the run of `///` lines directly above
//! an item, a field, a variant, or a method is its documentation, and the run of `//!` lines at the top of a file is
//! the documentation of the file's module. `burdin doc` and the `missing_docs` lint both read them here, so they
//! agree about which item a comment belongs to.

use crate::ast::*;
use burdin_diag::{FileId, Span};

/// The documentation attached to one declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocComment {
    /// The Markdown text: the comment lines without their `///` (or `//!`) and the one space after it, joined by
    /// newlines.
    pub text: String,
    /// From the first marker to the end of the last comment line.
    pub span: Span,
    /// The source line of each line of `text` (from its marker to the end of the line), so a diagnostic about
    /// something inside the comment can point at the line it is on.
    pub lines: Vec<Span>,
}

/// The text of one `///` line (everything after the marker and one optional space), or `None` when the line is not a
/// doc comment. `////` and longer runs are ordinary comments, so a separator line is never documentation.
fn doc_line(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("///")?;
    if rest.starts_with('/') {
        return None;
    }
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// A suppression comment (`// lint: allow(...)`) may sit between a doc comment and its item.
fn is_lint_comment(trimmed: &str) -> bool {
    trimmed.strip_prefix("//").is_some_and(|rest| rest.trim_start().starts_with("lint:"))
}

/// The doc comment of the declaration that starts at byte `offset`, or `None`. The declaration must be the first
/// thing on its line (only modifiers such as `priv` or `unsafe` may come before it, so the second field of
/// `a: i32, b: i32` has no doc comment), and the comment must end on the line above it: a blank line or an ordinary
/// comment detaches it.
pub fn doc_before(file: FileId, text: &str, offset: u32) -> Option<DocComment> {
    let offset = offset as usize;
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let modifiers_only =
        text[line_start..offset].chars().all(|c| c.is_whitespace() || c.is_alphanumeric() || c == '_' || c == '"');
    if !modifiers_only {
        return None;
    }
    let mut lines: Vec<(usize, usize, &str)> = Vec::new();
    let mut end = line_start;
    while end > 0 {
        let start = text[..end - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = text[start..end - 1].trim_end_matches('\r');
        let trimmed = line.trim_start();
        if let Some(body) = doc_line(trimmed) {
            let lo = start + (line.len() - trimmed.len());
            lines.push((lo, start + line.len(), body));
        } else if !is_lint_comment(trimmed) {
            break;
        }
        end = start;
    }
    let (Some(first), Some(last)) = (lines.last(), lines.first()) else {
        return None;
    };
    let span = Span::new(file, first.0 as u32, last.1 as u32);
    let text = lines.iter().rev().map(|l| l.2).collect::<Vec<_>>().join("\n");
    let lines = lines.iter().rev().map(|l| Span::new(file, l.0 as u32, l.1 as u32)).collect();
    Some(DocComment { text, span, lines })
}

/// The documentation of the file's module: the `//!` lines at the top of the file, after any blank lines.
pub fn module_docs(file: FileId, text: &str) -> Option<DocComment> {
    let mut lines: Vec<(usize, usize, &str)> = Vec::new();
    let mut at = 0usize;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        let lo = at + (line.len() - trimmed.len());
        at += raw.len();
        if lines.is_empty() && trimmed.is_empty() {
            continue;
        }
        match trimmed.strip_prefix("//!") {
            Some(rest) => lines.push((lo, at - (raw.len() - line.len()), rest.strip_prefix(' ').unwrap_or(rest))),
            None => break,
        }
    }
    let (first, last) = (lines.first()?, lines.last()?);
    let span = Span::new(file, first.0 as u32, last.1 as u32);
    let text = lines.iter().map(|l| l.2).collect::<Vec<_>>().join("\n");
    let lines = lines.iter().map(|l| Span::new(file, l.0 as u32, l.1 as u32)).collect();
    Some(DocComment { text, span, lines })
}

/// A declaration that belongs to a package's public interface and can carry documentation.
#[derive(Clone, Debug)]
pub struct PublicItem {
    /// `function`, `struct`, `enum`, `error enum`, `interface`, `type alias`, `constant`, `derive`,
    /// `extern function`, `method`, `interface method`, or `extension method`.
    pub kind: &'static str,
    /// The name as a reader writes it: `Point`, `Point.new`, `String.shout`.
    pub name: String,
    /// Where the declaration's name is, for a diagnostic.
    pub span: Span,
    pub docs: Option<DocComment>,
}

/// Whether the parser made this item up (a derived `Debug`, the impls of an `error enum`, the `main` wrapper): every
/// token of generated code carries one identifier's span, so a written item is never a single token.
pub fn is_generated(text: &str, span: Span) -> bool {
    match text.get(span.lo as usize..span.hi as usize) {
        Some(slice) => !slice.is_empty() && !slice.contains(char::is_whitespace) && !slice.contains('{'),
        None => true,
    }
}

/// Whether the declaration written at `span` is an `error enum` (the parser lowers it to an ordinary enum).
pub fn is_error_enum(text: &str, span: Span) -> bool {
    let Some(slice) = text.get(span.lo as usize..span.hi as usize) else { return false };
    let slice = slice.strip_prefix("priv").map_or(slice, str::trim_start);
    slice.starts_with("error") && slice[5..].starts_with(char::is_whitespace)
}

/// The name a `Type`-like AST node is known by: the last segment of a path type.
pub fn type_name(ty: &Type) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path(segments) => segments.last().map(|s| s.name.name.as_str()),
        _ => None,
    }
}

/// Whether a name is one the compiler invented or one that is never documented (`main`, `__x`).
pub fn is_internal_name(name: &str) -> bool {
    name == "main"
        || name.starts_with("__")
        || name.starts_with("comptime assert")
        || name.starts_with("comptime generate")
}

/// The public items of one file, in source order: the functions, types, interfaces, aliases, constants, and derives
/// that are not `priv`; the C functions it declares or exports; the non-`priv` methods of its inherent `impl` blocks;
/// the methods of its public interfaces; and its non-`priv` extension methods. Generated items and `main` are left
/// out, and so are the members of a `priv` type (nothing outside the package can name them).
pub fn public_items(ast: &File, file: FileId, text: &str) -> Vec<PublicItem> {
    let mut out = Vec::new();
    let docs = |offset: u32| doc_before(file, text, offset);
    let mut private_types: Vec<&str> = Vec::new();
    for item in &ast.items {
        match item {
            Item::Struct(s) if s.is_priv => private_types.push(&s.name.name),
            Item::Enum(e) if e.is_priv => private_types.push(&e.name.name),
            Item::Interface(i) if i.is_priv => private_types.push(&i.name.name),
            Item::TypeAlias(a) if a.is_priv => private_types.push(&a.name.name),
            _ => {}
        }
    }
    for item in &ast.items {
        let mut push = |kind: &'static str, name: &Ident, span: Span, display: String| {
            if is_internal_name(&name.name) || is_generated(text, span) && span == name.span {
                return;
            }
            out.push(PublicItem { kind, name: display, span: name.span, docs: docs(span.lo) });
        };
        match item {
            Item::Fn(f) if !f.is_priv => push("function", &f.name, f.span, f.name.name.clone()),
            Item::Struct(s) if !s.is_priv => push("struct", &s.name, s.span, s.name.name.clone()),
            Item::Enum(e) if !e.is_priv => {
                let kind = if is_error_enum(text, e.span) { "error enum" } else { "enum" };
                push(kind, &e.name, e.span, e.name.name.clone());
            }
            Item::Interface(i) if !i.is_priv => {
                push("interface", &i.name, i.span, i.name.name.clone());
                for m in &i.methods {
                    push(
                        "interface method",
                        &m.decl.name,
                        m.decl.span,
                        format!("{}.{}", i.name.name, m.decl.name.name),
                    );
                }
            }
            Item::TypeAlias(a) if !a.is_priv => push("type alias", &a.name, a.span, a.name.name.clone()),
            Item::Const(c) if !c.is_priv => push("constant", &c.name, c.span, c.name.name.clone()),
            Item::Derive(d) if !d.is_priv => push("derive", &d.name, d.span, d.name.name.clone()),
            Item::Extern(b) if !b.is_priv => {
                for d in b.decls.iter().filter(|d| !d.is_priv) {
                    push("extern function", &d.name, d.span, d.name.name.clone());
                }
            }
            Item::ExternFn(x) if !x.decl.is_priv => {
                push("extern function", &x.decl.name, x.decl.span, x.decl.name.name.clone());
            }
            Item::Extension(x) if !x.func.is_priv => {
                let recv = type_name(&x.recv).unwrap_or("_");
                push("extension method", &x.func.name, x.func.span, format!("{recv}.{}", x.func.name.name));
            }
            Item::Impl(b) if b.interface.is_none() => {
                let recv = type_name(&b.ty).unwrap_or("_");
                if private_types.contains(&recv) || is_generated(text, b.span) && b.span == b.ty.span {
                    continue;
                }
                for m in b.methods.iter().filter(|m| !m.is_priv) {
                    push("method", &m.name, m.span, format!("{recv}.{}", m.name.name));
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: FileId = FileId(0);

    fn doc(text: &str, needle: &str) -> Option<String> {
        let at = text.find(needle).expect("needle in text") as u32;
        doc_before(F, text, at).map(|d| d.text)
    }

    #[test]
    fn a_run_of_lines_above_the_item() {
        let t = "/// First line.\n///\n/// Second paragraph.\nfn f() {}\n";
        assert_eq!(doc(t, "fn f").as_deref(), Some("First line.\n\nSecond paragraph."));
    }

    #[test]
    fn a_blank_line_or_plain_comment_detaches() {
        assert_eq!(doc("/// doc\n\nfn f() {}\n", "fn f"), None);
        assert_eq!(doc("/// doc\n// note\nfn f() {}\n", "fn f"), None);
        assert_eq!(doc("//// banner\nfn f() {}\n", "fn f"), None);
    }

    #[test]
    fn a_suppression_comment_may_sit_between() {
        assert_eq!(doc("/// doc\n// lint: allow(dead_code)\nfn f() {}\n", "fn f").as_deref(), Some("doc"));
    }

    #[test]
    fn only_the_first_declaration_of_a_line_has_docs() {
        let t = "struct P {\n    /// the x\n    x: i32, y: i32\n}\n";
        assert_eq!(doc(t, "x: i32").as_deref(), Some("the x"));
        assert_eq!(doc(t, "y: i32"), None);
        assert_eq!(doc("/// doc\npriv fn f() {}\n", "priv fn").as_deref(), Some("doc"));
    }

    #[test]
    fn crlf_lines() {
        assert_eq!(doc("/// doc\r\nfn f() {}\r\n", "fn f").as_deref(), Some("doc"));
    }

    #[test]
    fn module_docs_are_the_leading_bang_lines() {
        let t = "\n//! The module.\n//!\n//! More.\n\n/// Not this.\nfn f() {}\n";
        assert_eq!(module_docs(F, t).map(|d| d.text).as_deref(), Some("The module.\n\nMore."));
        assert!(module_docs(F, "fn f() {}\n//! late\n").is_none());
    }

    #[test]
    fn public_items_skip_private_and_generated() {
        let src = "/// A.\nfn a() {}\npriv fn b() {}\nstruct S: Debug {\n    x: i32\n}\nS {\n    fn new(): S { S { x: 1 } }\n    priv fn hide(self) {}\n}\nfn main() {}\n";
        let mut diags = burdin_diag::Diagnostics::default();
        let ast = crate::parse(F, src, &mut diags);
        assert!(!diags.has_errors());
        let names: Vec<_> =
            public_items(&ast, F, src).into_iter().map(|i| (i.kind, i.name, i.docs.is_some())).collect();
        assert_eq!(
            names,
            vec![
                ("function", "a".to_string(), true),
                ("struct", "S".to_string(), false),
                ("method", "S.new".to_string(), false)
            ]
        );
    }

    #[test]
    fn error_enums_are_recognized() {
        let src = "error enum E {\n    A from \"a\"\n}\npriv error enum F {\n    B from \"b\"\n}\nenum G { X }\n";
        let mut diags = burdin_diag::Diagnostics::default();
        let ast = crate::parse(F, src, &mut diags);
        let items = public_items(&ast, F, src);
        assert_eq!(
            items.iter().map(|i| (i.kind, i.name.as_str())).collect::<Vec<_>>(),
            vec![("error enum", "E"), ("enum", "G")]
        );
    }
}
