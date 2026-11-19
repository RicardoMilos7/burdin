//! What one file declares and how it is spelled, read from its syntax tree and its tokens (`spec/lsp.md`, LS8): the
//! items with their names and spans, the `use` declarations, and the tokens. Outlines, folding, completion, and the
//! name-based parts of navigation read this; the typed answers come from the analysis.

use burdin_diag::{Diagnostics, FileId, Span};
use burdin_syntax::ast::{self, Item as AstItem};
use burdin_syntax::docs;
use burdin_syntax::lexer;
use burdin_syntax::token::{InterpPiece, Token, TokenKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Function,
    /// A method of an inherent or interface `impl` block, or an extension method.
    Method,
    Struct,
    Enum,
    Variant,
    Field,
    Interface,
    /// A method an interface declares.
    InterfaceMethod,
    Const,
    Alias,
    Test,
    /// `Type { ... }` or `Type: Interface { ... }`.
    Impl,
    Derive,
    /// A C function declared in an `extern` block.
    ExternFn,
}

impl Kind {
    /// Whether the item names a type.
    pub fn is_type(self) -> bool {
        matches!(self, Kind::Struct | Kind::Enum | Kind::Interface | Kind::Alias)
    }

    /// Whether the item is callable.
    pub fn is_function(self) -> bool {
        matches!(self, Kind::Function | Kind::Method | Kind::InterfaceMethod | Kind::ExternFn)
    }
}

/// One declaration of the file.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    /// The name as written.
    pub name_span: (u32, u32),
    /// The whole declaration.
    pub span: (u32, u32),
    /// The item this one is declared in: the struct of a field, the enum of a variant, the `impl` block or
    /// interface of a method.
    pub parent: Option<usize>,
    /// The type a method, field, or variant belongs to.
    pub owner: Option<String>,
    /// The interface an `impl` block (or a method in it) implements.
    pub interface: Option<String>,
    /// The interfaces an interface requires.
    pub requires: Vec<String>,
    /// The declaration as written, without its body: `fn add(a: i32, b: i32): i32`, `struct Rect`.
    pub detail: String,
    /// The `///` documentation above the declaration.
    pub docs: Option<String>,
    /// For a function or method: the byte range of its body block.
    pub body: Option<(u32, u32)>,
    /// A `pub use` or ordinary item that is not `priv`.
    pub is_priv: bool,
}

/// A `use` declaration.
#[derive(Clone, Debug)]
pub struct Use {
    /// The module or item path.
    pub path: Vec<(String, (u32, u32))>,
    /// The names of a `.{...}` list.
    pub items: Vec<(String, (u32, u32))>,
    pub alias: Option<(String, (u32, u32))>,
    pub span: (u32, u32),
}

/// A token, reduced to what the server looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tk {
    Ident(String),
    Keyword(&'static str),
    /// `self`.
    SelfValue,
    /// `Self`.
    SelfType,
    Str,
    Num,
    Dot,
    Comma,
    Colon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Lt,
    Gt,
    Other,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub kind: Tk,
    pub lo: u32,
    pub hi: u32,
}

impl Tok {
    pub fn ident(&self) -> Option<&str> {
        match &self.kind {
            Tk::Ident(name) => Some(name),
            _ => None,
        }
    }
}

/// Everything the server reads from one file's syntax.
#[derive(Clone, Default)]
pub struct FileIndex {
    pub items: Vec<Item>,
    pub uses: Vec<Use>,
    /// Every token in source order, the tokens inside `{...}` placeholders of strings included (after the string
    /// token itself).
    pub tokens: Vec<Tok>,
}

/// The keywords the lexer knows, as source spells them.
pub const KEYWORDS: &[&str] = &[
    "any",
    "as",
    "break",
    "comptime",
    "const",
    "continue",
    "else",
    "enum",
    "error",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "interface",
    "let",
    "loop",
    "match",
    "mut",
    "orelse",
    "own",
    "priv",
    "return",
    "self",
    "Self",
    "struct",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
];

/// Whether `name` is a valid identifier that is not a keyword: what a rename accepts.
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name != "_"
        && !KEYWORDS.contains(&name)
        && name != "derive"
}

impl FileIndex {
    pub fn new(ast: &ast::File, text: &str) -> FileIndex {
        let mut index = FileIndex { tokens: tokens(text), ..FileIndex::default() };
        let mut b = Builder { text, index: &mut index };
        for item in &ast.items {
            b.item(item);
        }
        for test in &ast.tests {
            let name_span = (test.name_span.lo, test.name_span.hi);
            b.push(Item {
                name: test.name.clone(),
                kind: Kind::Test,
                name_span,
                span: (test.span.lo, test.span.hi),
                parent: None,
                owner: None,
                interface: None,
                requires: Vec::new(),
                detail: format!("test \"{}\"", test.name),
                docs: docs::doc_before(FileId(0), text, test.span.lo).map(|d| d.text),
                body: Some((test.body_func.body.span.lo, test.body_func.body.span.hi)),
                is_priv: true,
            });
        }
        for u in &ast.uses {
            if docs::is_generated(text, u.span) {
                continue;
            }
            let ident = |i: &ast::Ident| (i.name.clone(), (i.span.lo, i.span.hi));
            index.uses.push(Use {
                path: u.path.iter().map(ident).collect(),
                items: u.items.iter().map(ident).collect(),
                alias: u.alias.as_ref().map(ident),
                span: (u.span.lo, u.span.hi),
            });
        }
        index.items.sort_by_key(|i| (i.span.0, std::cmp::Reverse(i.span.1)));
        // Sorting moved the items; parents are found again by containment.
        let spans: Vec<((u32, u32), Kind)> = index.items.iter().map(|i| (i.span, i.kind)).collect();
        for (n, item) in index.items.iter_mut().enumerate() {
            if item.parent.is_some() {
                item.parent = (0..n).rev().find(|&p| {
                    let (span, kind) = spans[p];
                    matches!(kind, Kind::Struct | Kind::Enum | Kind::Interface | Kind::Impl)
                        && span.0 <= item.span.0
                        && item.span.1 <= span.1
                });
            }
        }
        index
    }

    /// The token that contains a byte offset, or ends at it (a cursor just after a name is on the name).
    pub fn token_at(&self, offset: u32) -> Option<usize> {
        let at = self.tokens.partition_point(|t| t.hi < offset);
        let mut best = None;
        for (i, t) in self.tokens.iter().enumerate().skip(at).take(3) {
            if t.lo <= offset && offset <= t.hi {
                // Prefer an identifier when the offset is between two tokens.
                if best.is_none() || t.ident().is_some() {
                    best = Some(i);
                }
            }
        }
        best
    }

    /// The identifier token at a byte offset.
    pub fn ident_at(&self, offset: u32) -> Option<&Tok> {
        let i = self.token_at(offset)?;
        let t = &self.tokens[i];
        if t.ident().is_some() {
            return Some(t);
        }
        // `foo|(`: the identifier ends where the next token starts.
        self.tokens.get(i.wrapping_sub(1)).filter(|p| p.hi == offset && p.ident().is_some())
    }

    /// The item declared with its name at a byte offset.
    pub fn item_named_at(&self, offset: u32) -> Option<usize> {
        self.items.iter().position(|i| i.name_span.0 <= offset && offset <= i.name_span.1 && i.kind != Kind::Impl)
    }

    /// The innermost function, method, or test whose declaration contains a byte offset.
    pub fn function_around(&self, offset: u32) -> Option<&Item> {
        self.items
            .iter()
            .filter(|i| i.body.is_some() && i.span.0 <= offset && offset <= i.span.1)
            .min_by_key(|i| i.span.1 - i.span.0)
    }
}

struct Builder<'a> {
    text: &'a str,
    index: &'a mut FileIndex,
}

impl Builder<'_> {
    fn slice(&self, lo: u32, hi: u32) -> &str {
        self.text.get(lo as usize..hi as usize).unwrap_or("")
    }

    fn push(&mut self, item: Item) -> usize {
        self.index.items.push(item);
        self.index.items.len() - 1
    }

    fn generated(&self, span: Span) -> bool {
        docs::is_generated(self.text, span)
    }

    fn docs(&self, lo: u32) -> Option<String> {
        docs::doc_before(FileId(0), self.text, lo).map(|d| d.text)
    }

    /// The header of a declaration: its text up to the first `{`, on one line.
    fn header(&self, span: Span) -> String {
        let text = self.slice(span.lo, span.hi);
        let head = text.split('{').next().unwrap_or(text);
        head.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn fn_detail(&self, f: &ast::FnDecl) -> String {
        let end = if f.body.span.lo > f.span.lo && f.body.span.lo <= f.span.hi { f.body.span.lo } else { f.span.hi };
        self.slice(f.span.lo, end).split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn function(
        &mut self,
        f: &ast::FnDecl,
        kind: Kind,
        parent: Option<usize>,
        owner: Option<String>,
        iface: Option<String>,
    ) {
        if self.generated(f.span) || self.slice(f.name.span.lo, f.name.span.hi) != f.name.name {
            return;
        }
        let has_body = f.body.span.lo < f.body.span.hi && f.body.span.lo >= f.span.lo;
        self.push(Item {
            name: f.name.name.clone(),
            kind,
            name_span: (f.name.span.lo, f.name.span.hi),
            span: (f.span.lo, f.span.hi),
            parent,
            owner,
            interface: iface,
            requires: Vec::new(),
            detail: self.fn_detail(f),
            docs: self.docs(f.span.lo),
            body: has_body.then_some((f.body.span.lo, f.body.span.hi)),
            is_priv: f.is_priv,
        });
    }

    fn simple(&mut self, name: &ast::Ident, kind: Kind, span: Span, is_priv: bool) -> Option<usize> {
        if self.generated(span) {
            return None;
        }
        Some(self.push(Item {
            name: name.name.clone(),
            kind,
            name_span: (name.span.lo, name.span.hi),
            span: (span.lo, span.hi),
            parent: None,
            owner: None,
            interface: None,
            requires: Vec::new(),
            detail: self.header(span),
            docs: self.docs(span.lo),
            body: None,
            is_priv,
        }))
    }

    fn field(&mut self, f: &ast::FieldDecl, parent: usize, owner: &str) {
        let span = Span::new(f.name.span.file, f.name.span.lo, f.ty.span.hi.max(f.name.span.hi));
        self.push(Item {
            name: f.name.name.clone(),
            kind: Kind::Field,
            name_span: (f.name.span.lo, f.name.span.hi),
            span: (span.lo, span.hi),
            parent: Some(parent),
            owner: Some(owner.to_string()),
            interface: None,
            requires: Vec::new(),
            detail: self.slice(span.lo, span.hi).split_whitespace().collect::<Vec<_>>().join(" "),
            docs: self.docs(span.lo),
            body: None,
            is_priv: f.is_priv,
        });
    }

    fn variants(&mut self, variants: &[(&ast::Ident, &ast::VariantKind)], parent: usize, owner: &str) {
        for (name, kind) in variants {
            let hi = match kind {
                ast::VariantKind::Unit => name.span.hi,
                ast::VariantKind::Tuple(types) => types.last().map_or(name.span.hi, |t| t.span.hi + 1),
                ast::VariantKind::Struct(fields) => fields.last().map_or(name.span.hi, |f| f.ty.span.hi + 2),
            };
            let hi = hi.min(self.text.len() as u32);
            let variant = self.push(Item {
                name: name.name.clone(),
                kind: Kind::Variant,
                name_span: (name.span.lo, name.span.hi),
                span: (name.span.lo, hi),
                parent: Some(parent),
                owner: Some(owner.to_string()),
                interface: None,
                requires: Vec::new(),
                detail: format!(
                    "{owner}.{}",
                    self.slice(name.span.lo, hi).split_whitespace().collect::<Vec<_>>().join(" ")
                ),
                docs: self.docs(name.span.lo),
                body: None,
                is_priv: false,
            });
            if let ast::VariantKind::Struct(fields) = kind {
                for f in fields {
                    self.field(f, variant, owner);
                }
            }
        }
    }

    fn item(&mut self, item: &AstItem) {
        match item {
            AstItem::Fn(f) => self.function(f, Kind::Function, None, None, None),
            AstItem::ExternFn(e) => self.function(&e.decl, Kind::Function, None, None, None),
            AstItem::Extension(e) => {
                let owner = docs::type_name(&e.recv).map(str::to_string);
                self.function(&e.func, Kind::Method, None, owner, None);
            }
            AstItem::Const(c) => {
                self.simple(&c.name, Kind::Const, c.span, c.is_priv);
            }
            AstItem::TypeAlias(a) => {
                self.simple(&a.name, Kind::Alias, a.span, a.is_priv);
            }
            AstItem::Derive(d) => {
                if let Some(parent) = self.simple(&d.name, Kind::Derive, d.span, d.is_priv) {
                    for m in &d.methods {
                        self.function(m, Kind::Method, Some(parent), None, None);
                    }
                }
            }
            AstItem::Struct(s) => {
                if let Some(parent) = self.simple(&s.name, Kind::Struct, s.span, s.is_priv) {
                    for f in &s.fields {
                        self.field(f, parent, &s.name.name);
                    }
                }
            }
            AstItem::Enum(e) => {
                if let Some(parent) = self.simple(&e.name, Kind::Enum, e.span, e.is_priv) {
                    let vs: Vec<_> = e.variants.iter().map(|v| (&v.name, &v.kind)).collect();
                    self.variants(&vs, parent, &e.name.name);
                }
            }
            AstItem::ErrorEnum(e) => {
                if let Some(parent) = self.simple(&e.name, Kind::Enum, e.span, e.is_priv) {
                    let vs: Vec<_> = e.variants.iter().map(|v| (&v.name, &v.kind)).collect();
                    self.variants(&vs, parent, &e.name.name);
                }
            }
            AstItem::Interface(i) => {
                if let Some(parent) = self.simple(&i.name, Kind::Interface, i.span, i.is_priv) {
                    self.index.items[parent].requires =
                        i.requires.iter().filter_map(docs::type_name).map(str::to_string).collect();
                    for m in &i.methods {
                        self.function(&m.decl, Kind::InterfaceMethod, Some(parent), Some(i.name.name.clone()), None);
                    }
                }
            }
            AstItem::Impl(b) => {
                if self.generated(b.span) {
                    return;
                }
                let Some(owner) = docs::type_name(&b.ty).map(str::to_string) else { return };
                let iface = b.interface.as_ref().and_then(docs::type_name).map(str::to_string);
                let name = match &iface {
                    Some(i) => format!("{owner}: {i}"),
                    None => owner.clone(),
                };
                let parent = self.push(Item {
                    name,
                    kind: Kind::Impl,
                    name_span: (b.ty.span.lo, b.ty.span.hi),
                    span: (b.span.lo, b.span.hi),
                    parent: None,
                    owner: Some(owner.clone()),
                    interface: iface.clone(),
                    requires: Vec::new(),
                    detail: self.header(b.span),
                    docs: None,
                    body: None,
                    is_priv: false,
                });
                for m in &b.methods {
                    self.function(m, Kind::Method, Some(parent), Some(owner.clone()), iface.clone());
                }
            }
            AstItem::Extern(e) => {
                for d in &e.decls {
                    self.function(d, Kind::ExternFn, None, None, None);
                }
            }
        }
    }
}

/// The tokens of a text, the placeholders of interpolated strings flattened in after their string.
pub fn tokens(text: &str) -> Vec<Tok> {
    let mut diags = Diagnostics::default();
    let lexed = lexer::lex(FileId(0), text, &mut diags);
    let mut out = Vec::new();
    flatten(&lexed.tokens, &mut out);
    out
}

fn flatten(tokens: &[Token], out: &mut Vec<Tok>) {
    for t in tokens {
        let kind = match &t.kind {
            TokenKind::Eof => continue,
            TokenKind::Ident(name) => Tk::Ident(name.clone()),
            TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Char(_) => Tk::Num,
            TokenKind::Str(_) => Tk::Str,
            TokenKind::Interp(pieces) => {
                out.push(Tok { kind: Tk::Str, lo: t.span.lo, hi: t.span.hi });
                for piece in pieces {
                    if let InterpPiece::Hole(hole) = piece {
                        flatten(&hole.tokens, out);
                    }
                }
                continue;
            }
            TokenKind::SelfValue => Tk::SelfValue,
            TokenKind::SelfType => Tk::SelfType,
            TokenKind::Dot => Tk::Dot,
            TokenKind::Comma => Tk::Comma,
            TokenKind::Colon => Tk::Colon,
            TokenKind::LParen => Tk::LParen,
            TokenKind::RParen => Tk::RParen,
            TokenKind::LBrace => Tk::LBrace,
            TokenKind::RBrace => Tk::RBrace,
            TokenKind::LBracket => Tk::LBracket,
            TokenKind::RBracket => Tk::RBracket,
            TokenKind::Lt => Tk::Lt,
            TokenKind::Gt => Tk::Gt,
            TokenKind::Reserved(_) => Tk::Keyword("derive"),
            other => match keyword_text(other) {
                Some(word) => Tk::Keyword(word),
                None => Tk::Other,
            },
        };
        out.push(Tok { kind, lo: t.span.lo, hi: t.span.hi });
    }
}

fn keyword_text(kind: &TokenKind) -> Option<&'static str> {
    KEYWORDS.iter().copied().find(|word| TokenKind::keyword(word).as_ref() == Some(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(text: &str) -> FileIndex {
        let mut diags = Diagnostics::default();
        let ast = burdin_syntax::parse(FileId(0), text, &mut diags);
        FileIndex::new(&ast, text)
    }

    #[test]
    fn items_nest_under_their_declarations() {
        let text = "/// A point.\nstruct P {\n    x: i32\n}\n\nP {\n    fn len(self): i32 {\n        self.x\n    }\n}\n\nfn main() {\n    let s = \"{1 + 2}\"\n}\n";
        let ix = index(text);
        let names: Vec<(&str, Kind, Option<usize>)> =
            ix.items.iter().map(|i| (i.name.as_str(), i.kind, i.parent)).collect();
        assert_eq!(
            names,
            vec![
                ("P", Kind::Struct, None),
                ("x", Kind::Field, Some(0)),
                ("P", Kind::Impl, None),
                ("len", Kind::Method, Some(2)),
                ("main", Kind::Function, None)
            ]
        );
        assert_eq!(ix.items[0].docs.as_deref(), Some("A point."));
        assert_eq!(ix.items[3].detail, "fn len(self): i32");
        // The placeholder's tokens follow the string token.
        assert!(ix.tokens.iter().any(|t| t.kind == Tk::Num && text[t.lo as usize..t.hi as usize] == *"2"));
    }

    #[test]
    fn identifiers_reject_keywords() {
        assert!(is_identifier("area_2"));
        assert!(!is_identifier("fn"));
        assert!(!is_identifier("2x"));
        assert!(!is_identifier("_"));
    }
}
