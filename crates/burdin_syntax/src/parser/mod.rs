//! The parser (spec/grammar.md). It reports E0001 for anything outside the grammar and recovers at statement and
//! item boundaries so that every syntax error in a file is reported.
//!
//! Constructs that are in the grammar but not yet implemented by this compiler are reported with
//! `Code::UNSUPPORTED`; `spec/e9000-audit.md` tracks the remaining sites.

mod expr;
mod format;
mod items;
mod pattern;
mod select;
mod stmt;
mod types;

use crate::ast::*;
use crate::lexer::LexResult;
use crate::token::{Token, TokenKind};
use burdin_diag::{Code, Diagnostics, FileId, Span};

/// The error has already been reported; the caller recovers.
pub struct Reported;

pub(super) type PResult<T> = Result<T, Reported>;

pub fn parse_file(file: FileId, lexed: LexResult, diags: &mut Diagnostics) -> File {
    let mut p = Parser {
        file,
        toks: lexed.tokens,
        pos: 0,
        diags,
        nl_stack: Vec::new(),
        truncated: lexed.truncated,
        test_count: 0,
        bench_count: 0,
        comptime_asserts: 0,
        select_count: 0,
        bodies: Vec::new(),
        generators: Vec::new(),
    };
    p.file()
}

/// Parses `lexed` as exactly one `{ ... }` block. `None` when the tokens are anything else, or when the parser had
/// to report an error (the caller then parses the whole file, which reports it in context).
pub fn parse_block(file: FileId, lexed: LexResult, diags: &mut Diagnostics) -> Option<Block> {
    if lexed.truncated {
        return None;
    }
    let before = diags.len();
    let mut p = Parser {
        file,
        toks: lexed.tokens,
        pos: 0,
        diags,
        nl_stack: Vec::new(),
        truncated: false,
        test_count: 0,
        bench_count: 0,
        comptime_asserts: 0,
        select_count: 0,
        bodies: Vec::new(),
        generators: Vec::new(),
    };
    let block = p.block().ok()?;
    let clean = p.at_eof() && p.diags.len() == before;
    clean.then_some(block)
}

pub(super) struct Parser<'a> {
    #[allow(dead_code)]
    file: FileId,
    toks: Vec<Token>,
    pos: usize,
    diags: &'a mut Diagnostics,
    /// Whether line breaks are significant, one entry per open bracket (spec/grammar.md, section 2).
    nl_stack: Vec<bool>,
    truncated: bool,
    /// How many `test` items this file has parsed, for their generated function names.
    test_count: usize,
    /// How many `bench` items this file has parsed, for their generated function names.
    bench_count: usize,
    /// How many module-scope `comptime assert` items this file has parsed, for their constant names.
    comptime_asserts: usize,
    /// How many `select` expressions this file has parsed, for the names of their generated variables.
    select_count: usize,
    /// The block of every function and test body the file wrote, in source order ([`File::bodies`]).
    bodies: Vec<Span>,
    /// The module-scope `comptime generate` items this file has parsed ([`File::generators`]).
    generators: Vec<Generator>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Restriction {
    None,
    /// In the head of `if`, `while`, `for`, `match`: struct literals are not allowed.
    NoStructLiteral,
}

impl Parser<'_> {
    // ----- token helpers ---------------------------------------------------

    fn tok(&self) -> &Token {
        &self.toks[self.pos]
    }

    fn kind(&self) -> &TokenKind {
        &self.toks[self.pos].kind
    }

    fn nth_kind(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].kind
    }

    fn span(&self) -> Span {
        self.tok().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 { self.span() } else { self.toks[self.pos - 1].span }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.kind() == kind
    }

    fn at_eof(&self) -> bool {
        self.at(&TokenKind::Eof)
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// A line break before the current token that ends an expression here.
    fn nl(&self) -> bool {
        self.tok().nl_before && self.nl_stack.last().copied().unwrap_or(true)
    }

    fn error_here(&mut self, msg: String) -> Reported {
        // Past an unterminated literal the rest of the file is gone; the lexer already reported it.
        if !(self.truncated && self.at_eof()) {
            let span = self.span();
            self.diags.error(Code::SYNTAX, span, msg);
        }
        Reported
    }

    fn error_at(&mut self, span: Span, msg: String) -> Reported {
        self.diags.error(Code::SYNTAX, span, msg);
        Reported
    }

    fn unexpected(&mut self, expected: &str) -> Reported {
        let found = self.kind().describe();
        self.error_here(format!("expected {expected}, found {found}"))
    }

    fn expect(&mut self, kind: &TokenKind) -> PResult<Token> {
        if self.at(kind) { Ok(self.bump()) } else { Err(self.unexpected(&format!("`{}`", kind.text()))) }
    }

    fn ident(&mut self, what: &str) -> PResult<Ident> {
        match self.kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            TokenKind::Reserved(word) => Err(self.error_here(format!(
                "`{word}` is reserved for a later version of Burdin and cannot be used as a name"
            ))),
            _ => Err(self.unexpected(what)),
        }
    }

    /// Opens a bracket and records whether line breaks matter inside it.
    fn open(&mut self, kind: &TokenKind, newlines_matter: bool) -> PResult<Token> {
        let t = self.expect(kind)?;
        self.nl_stack.push(newlines_matter);
        Ok(t)
    }

    fn close(&mut self, kind: &TokenKind) -> PResult<Token> {
        let t = self.expect(kind)?;
        self.nl_stack.pop();
        Ok(t)
    }

    /// Consumes a `>` that closes generic arguments, splitting `>>` and `>=`.
    fn eat_closing_angle(&mut self) -> bool {
        let span = self.span();
        let rest = match self.kind() {
            TokenKind::Gt => {
                self.bump();
                return true;
            }
            TokenKind::Shr => TokenKind::Gt,
            TokenKind::Ge => TokenKind::Eq,
            _ => return false,
        };
        let t = &mut self.toks[self.pos];
        t.kind = rest;
        t.span = Span::new(span.file, span.lo + 1, span.hi);
        t.nl_before = false;
        true
    }

    // ----- recovery --------------------------------------------------------

    /// Keywords that only start items (stage 0 has no nested items), so they end recovery even inside unbalanced
    /// brackets.
    fn starts_item_only(kind: &TokenKind) -> bool {
        use TokenKind::*;
        matches!(kind, Fn | Struct | Enum | Error | Interface | Type | Const | Use | Extern | Priv | Comptime)
    }

    /// Recovers from an item that did not parse, which began at token `start`: skips to the next item, and records
    /// the whole stretch (from the item's first token to the last one skipped) in `unparsed`.
    fn recover_unparsed_item(&mut self, start: usize, unparsed: &mut Vec<Span>) {
        self.recover_item();
        let first = self.toks[start].span;
        let span = if self.pos > start { first.to(self.toks[self.pos - 1].span) } else { first };
        unparsed.push(span);
    }

    /// Skips to the start of the next item: an item keyword at the start of a line.
    fn recover_item(&mut self) {
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            // `test` and `bench` are contextual, so they only start an item when a string follows them, and `fuzz`
            // when a name and a parameter list do.
            let contextual_item = (matches!(&t.kind, TokenKind::Ident(name) if name == "test" || name == "bench")
                && self.toks.get(self.pos + 1).is_some_and(|next| matches!(next.kind, TokenKind::Str(_))))
                || (matches!(&t.kind, TokenKind::Ident(name) if name == "fuzz")
                    && self.toks.get(self.pos + 1).is_some_and(|next| matches!(next.kind, TokenKind::Ident(_)))
                    && self.toks.get(self.pos + 2).is_some_and(|next| next.kind == TokenKind::LParen));
            let item_start =
                Self::starts_item_only(&t.kind) || (depth == 0 && t.kind == TokenKind::Unsafe) || contextual_item;
            if self.pos > start && t.nl_before && item_start {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                // A stray closer (the one that caused the error) must not unbalance the count.
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                _ => {}
            }
            self.bump();
        }
        self.nl_stack.clear();
    }

    /// Skips to the end of the current statement: a line break or `}` at the block's own depth.
    fn recover_stmt(&mut self, block_depth: usize) {
        self.nl_stack.truncate(block_depth);
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            if depth == 0 && (matches!(t.kind, TokenKind::RBrace) || (self.pos > start && t.nl_before)) {
                break;
            }
            match t.kind {
                TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                // A stray closer (the one that caused the error) must not unbalance the count.
                TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                _ => {}
            }
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::SourceMap;

    fn parse_str(src: &str) -> (File, Diagnostics) {
        let mut diags = Diagnostics::default();
        let lexed = crate::lexer::lex(FileId(0), src, &mut diags);
        let file = parse_file(FileId(0), lexed, &mut diags);
        (file, diags)
    }

    fn render(src: &str) -> (File, String) {
        let (file, diags) = parse_str(src);
        let mut sources = SourceMap::default();
        let id = sources.add("t.bd".into(), src.into());
        assert_eq!(id, FileId(0));
        (file, diags.render(&sources))
    }

    fn fn_names(file: &File) -> Vec<&str> {
        file.items
            .iter()
            .filter_map(|item| match item {
                Item::Fn(f) => Some(f.name.name.as_str()),
                _ => None,
            })
            .collect()
    }

    fn binding_names(block: &Block) -> Vec<&str> {
        block
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Let { pattern, .. } => match &pattern.kind {
                    PatternKind::Binding { name, .. } => Some(name.name.as_str()),
                    _ => None,
                },
                Stmt::Expr(_) => None,
            })
            .collect()
    }

    #[test]
    fn slice_extension_receivers_parse() {
        // A slice receiver is a type after `fn`: `fn [T].name(...)` and nested forms (`spec/extensions.md`, EX1).
        let (file, out) = render(
            "fn [T].first(self): Option<&T> { None }\n\
             fn [[T]].concat(self): List<T> { List<T>.new() }\n\
             fn [u8].is_ascii(self): bool { true }\n\
             fn plain() {}\n",
        );
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["plain"]);
        let extensions: Vec<&ExtensionDecl> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Extension(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(extensions.len(), 3);

        let first = extensions[0];
        assert_eq!(first.func.name.name, "first");
        let TypeKind::Slice(elem) = &first.recv.kind else { panic!("the receiver is a slice") };
        assert!(matches!(elem.kind, TypeKind::Path(_)));
        assert!(matches!(first.func.params[0].kind, ParamKind::SelfParam));

        let concat = extensions[1];
        let TypeKind::Slice(inner) = &concat.recv.kind else { panic!("the receiver is a slice") };
        assert!(matches!(inner.kind, TypeKind::Slice(_)), "a slice of slices is a receiver");

        let ascii = extensions[2];
        let TypeKind::Slice(elem) = &ascii.recv.kind else { panic!("the receiver is a slice") };
        let TypeKind::Path(segments) = &elem.kind else { panic!("the element is a path") };
        assert_eq!(segments[0].name.name, "u8");
        assert!(ascii.func.generics.is_empty(), "`u8` is a builtin type, not a receiver parameter");
    }

    #[test]
    fn for_loops_take_an_optional_mut_marker() {
        // `for x in mut xs` is the mutable form (spec/iterators.md, IT15); the marker is part of the header, so the
        // iterated expression is still just `xs` (and a struct literal is not allowed there).
        let (file, out) = render("fn f() {\nfor x in mut xs { }\nfor y in xs { }\nfor z in mut a.b { }\n}\n");
        assert_eq!(out, "");
        let Some(Item::Fn(f)) = file.items.first() else { panic!("expected a function") };
        let marks: Vec<bool> = f
            .body
            .stmts
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Expr(Expr { kind: ExprKind::For { mutable, .. }, .. }) => Some(*mutable),
                _ => None,
            })
            .collect();
        assert_eq!(marks, [true, false, true]);
        let Stmt::Expr(Expr { kind: ExprKind::For { iter, .. }, .. }) = &f.body.stmts[0] else { panic!() };
        assert!(matches!(&iter.kind, ExprKind::Name { .. }), "{:?}", iter.kind);
    }

    #[test]
    fn extension_declarations_parse() {
        // `fn Type.name(...)`: the receiver is the type path before the last dot, including its type
        // arguments, and a `<...>` after the method name is the method's own generic list (spec/extensions.md).
        let (file, out) = render(
            "fn String.shout(self): String { self }\n\
             fn List<T>.largest(self): T where T: Copy + Ord { self[0] }\n\
             fn sh.Square.area(self): i32 { 1 }\n\
             fn Holder.pair<U>(self, own other: U): (i32, U) { (0, other) }\n\
             fn plain() {}\n",
        );
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["plain"]);
        let extensions: Vec<&ExtensionDecl> = file
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Extension(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(extensions.len(), 4);

        let shout = extensions[0];
        assert_eq!(shout.func.name.name, "shout");
        let TypeKind::Path(segments) = &shout.recv.kind else { panic!("the receiver is a path") };
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].name.name, "String");
        assert!(matches!(shout.func.params[0].kind, ParamKind::SelfParam));

        let largest = extensions[1];
        let TypeKind::Path(segments) = &largest.recv.kind else { panic!("the receiver is a path") };
        assert_eq!(segments[0].name.name, "List");
        assert_eq!(segments[0].args.len(), 1, "`List<T>` declares the receiver's parameter");
        assert!(largest.func.generics.is_empty(), "`T` is not a method parameter");
        assert_eq!(largest.func.where_clause.len(), 1);

        let area = extensions[2];
        let TypeKind::Path(segments) = &area.recv.kind else { panic!("the receiver is a path") };
        assert_eq!(segments.iter().map(|s| s.name.name.as_str()).collect::<Vec<_>>(), ["sh", "Square"]);

        let pair = extensions[3];
        let TypeKind::Path(segments) = &pair.recv.kind else { panic!("the receiver is a path") };
        assert!(segments[0].args.is_empty());
        assert_eq!(
            pair.func.generics.iter().map(|g| g.name.name.as_str()).collect::<Vec<_>>(),
            ["U"],
            "`<U>` after the method name is the method's generic list"
        );
        assert!(matches!(pair.func.params[1].convention, Convention::Own));
    }

    #[test]
    fn recovers_at_the_next_statement() {
        let (file, out) = render("fn main() {\n    let = 5\n    let x = 1\n    let y = x + 1\n}\n");
        assert_eq!(out, "t.bd:2:9: error[E0001]: expected a pattern, found `=`\n");
        assert_eq!(fn_names(&file), ["main"]);
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(binding_names(&main.body), ["x", "y"]);
    }

    #[test]
    fn recovers_at_the_next_item() {
        let (file, out) = render("fn broken( { }\nfn good() {}\n");
        assert_eq!(out, "t.bd:1:12: error[E0001]: expected a parameter name, found `{`\n");
        assert_eq!(fn_names(&file), ["good"]);
    }

    #[test]
    fn an_unreadable_item_is_recorded_from_its_first_token() {
        // The unreadable `fn broken` is one region from its `fn` to the token recovery stopped at; the good items
        // parse and record nothing.
        let src = "fn broken( { }
fn good() {}
struct Bad { x i32 }
";
        let (file, _) = render(src);
        let texts: Vec<&str> = file.unparsed.iter().map(|span| &src[span.lo as usize..span.hi as usize]).collect();
        assert_eq!(texts, ["fn broken( { }", "struct Bad { x i32 }"]);
        assert_eq!(fn_names(&file), ["good"]);
    }

    #[test]
    fn a_readable_file_records_no_unreadable_item() {
        let (file, out) = render(
            "fn main() {}
struct Point { x: i32 }
",
        );
        assert_eq!(out, "");
        assert!(file.unparsed.is_empty());
    }

    #[test]
    fn recovers_from_an_unclosed_bracket_at_the_block_end() {
        let (file, out) = render("fn main() {\n    g(1, 2\n}\nfn good() {}\n");
        assert_eq!(out, "t.bd:3:1: error[E0001]: expected `)`, found `}`\n");
        assert_eq!(fn_names(&file), ["main", "good"]);
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(main.body.stmts.len(), 0);
    }

    #[test]
    fn item_recovery_ignores_unbalanced_brackets() {
        // `starts_item_only` ends recovery at the next item keyword even with unclosed brackets, so one broken
        // signature cannot swallow the rest of the file.
        let (file, out) = render("fn broken( ( (\nfn good() {}\n");
        assert_eq!(out, "t.bd:1:12: error[E0001]: expected a parameter name, found `(`\n");
        assert_eq!(fn_names(&file), ["good"]);
    }

    #[test]
    fn missing_statement_separator_is_reported_and_recovered() {
        let (file, out) = render("fn main() {\n    let x = 1 let y = 2\n}\n");
        assert_eq!(out, "t.bd:2:15: error[E0001]: expected a line break or `}` after the statement, found `let`\n");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(binding_names(&main.body), ["x"]);
    }

    #[test]
    fn a_newline_ends_an_expression_except_before_and_or() {
        let (file, out) = render("fn main() {\n    let ok = true\n    && false\n    let x = 1\n    + 2\n}\n");
        assert_eq!(out, "t.bd:5:5: error[E0001]: expected an expression, found `+`\n");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(binding_names(&main.body), ["ok", "x"]);
        let Stmt::Let { init, .. } = &main.body.stmts[0] else { unreachable!() };
        assert!(matches!(init.kind, ExprKind::Binary(BinOp::And, _, _)));
        let Stmt::Let { init, .. } = &main.body.stmts[1] else { unreachable!() };
        assert!(matches!(init.kind, ExprKind::Int(1)));
    }

    #[test]
    fn closing_angles_are_split_out_of_shifts_and_comparisons() {
        let (file, out) =
            render("struct Stack<T> { items: T }\nfn f(x: Stack<Stack<i32>>) {}\nfn g(a: bool) { let b = a >= a }\n");
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["f", "g"]);
    }

    #[test]
    fn errors_point_at_the_offending_token() {
        let (_, out) = render("fn main() {\n    let x = )\n}\n");
        assert_eq!(out, "t.bd:2:13: error[E0001]: expected an expression, found `)`\n");
    }

    #[test]
    fn a_truncated_lexer_adds_no_parser_error() {
        let (_, out) = render("fn main() {\n    /* no end");
        assert_eq!(out, "t.bd:2:5: error[E0001]: unterminated block comment\n");
    }

    #[test]
    fn closure_expressions_parse() {
        let (file, out) = render(
            "fn main() {\n    let f = |a: i32, b| a + b\n    let g = || 1\n    let h = |x: i32|: i32 { x }\n}\n",
        );
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let inits: Vec<&Expr> = main
            .body
            .stmts
            .iter()
            .filter_map(|s| match s {
                Stmt::Let { init, .. } => Some(init),
                Stmt::Expr(_) => None,
            })
            .collect();
        assert_eq!(inits.len(), 3);
        let ExprKind::Closure { params, ret, .. } = &inits[0].kind else { panic!("not a closure") };
        assert_eq!(params.len(), 2);
        assert!(params[0].ty.is_some());
        assert!(params[1].ty.is_none());
        assert!(ret.is_none());
        let ExprKind::Closure { params, .. } = &inits[1].kind else { panic!("not a closure") };
        assert!(params.is_empty());
        let ExprKind::Closure { ret, .. } = &inits[2].kind else { panic!("not a closure") };
        assert!(matches!(ret.as_ref().map(|t| &t.kind), Some(TypeKind::Path(_))));
    }

    #[test]
    fn closure_types_parse_in_signatures_and_bounds() {
        let (file, out) = render(
            "fn apply(f: Fn(i32): i32) {}\n\
             fn twice<F: FnMut(i32, i32): i32>(f: F) {}\n\
             fn zero(f: FnOnce(): ()) {}\n",
        );
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["apply", "twice", "zero"]);
        let Item::Fn(apply) = &file.items[0] else { unreachable!() };
        let ParamKind::Named { ty, .. } = &apply.params[0].kind else { unreachable!() };
        assert!(
            matches!(&ty.kind, TypeKind::Fn { kind: FnKind::Fn, params, ret } if params.len() == 1 && ret.is_some())
        );
        let Item::Fn(twice) = &file.items[1] else { unreachable!() };
        assert!(matches!(
            &twice.generics[0].bounds[0].kind,
            TypeKind::Fn { kind: FnKind::FnMut, params, .. } if params.len() == 2
        ));
    }

    #[test]
    fn closure_and_parenthesized_generic_arguments_scan_ahead() {
        let (file, out) =
            render("fn main() {\n    let a = List<Fn(i32): i32>.new()\n    let b = List<(i32, i32)>.new()\n}\n");
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(main.body.stmts.len(), 2);
    }

    #[test]
    fn test_items_survive_recovery() {
        let (file, out) = render("fn broken( { }\ntest \"one\" { assert(true) }\nfn good() {}\n");
        assert_eq!(out.lines().count(), 1, "{out}");
        assert_eq!(fn_names(&file), ["good"]);
        assert_eq!(file.tests.len(), 1);
        assert_eq!(file.tests[0].name, "one");
    }

    #[test]
    fn test_attributes_are_parsed() {
        let (file, out) = render(
            "test \"one\" [ignore, timeout(500), should_panic(\"boom\"), should_fail] { }\n\
             test \"two\" { }\n",
        );
        assert_eq!(out, "");
        assert_eq!(file.tests.len(), 2);
        let attrs = &file.tests[0].attrs;
        assert_eq!(attrs.len(), 4);
        assert_eq!(attrs[0].name.name, "ignore");
        assert!(attrs[0].value.is_none());
        assert_eq!(attrs[1].name.name, "timeout");
        assert!(matches!(attrs[1].value, Some(TestAttrValue::Int(500))));
        assert_eq!(attrs[2].name.name, "should_panic");
        assert!(matches!(&attrs[2].value, Some(TestAttrValue::Str(text)) if text == "boom"));
        assert_eq!(attrs[3].name.name, "should_fail");
        assert!(file.tests[1].attrs.is_empty());
    }

    #[test]
    fn malformed_test_attributes_are_syntax_errors() {
        for src in [
            "test \"a\" [ignore { }\n",
            "test \"a\" [timeout(] { }\n",
            "test \"a\" [ignore timeout(1)] { }\n",
            "test \"a\" [ignore]\n",
        ] {
            let (_, out) = render(src);
            assert!(out.contains("E0001"), "{src}: {out}");
        }
    }

    #[test]
    fn bench_items_parse_and_stay_contextual() {
        // A bench item is `bench`, a string, and a block; it becomes `__burdin_bench_N` and is kept out of the
        // ordinary items, like a test (`spec/bench.md`, BN1).
        let (file, out) = render("bench \"one\" { let mut x = 0\nx = x + 1 }\nbench \"two\" { }\nfn good() {}\n");
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["good"]);
        assert_eq!(file.benches.len(), 2);
        assert_eq!(file.benches[0].name, "one");
        assert_eq!(file.benches[1].func.name.name, "__burdin_bench_1");
        assert!(file.benches[0].func.ret.is_none());
        assert!(file.benches[0].func.is_priv);

        // `bench` stays an ordinary identifier everywhere else, so this is not a bench item.
        let (file, out) = render("fn f() { let bench = 1\nbench }\n");
        assert_eq!(out, "");
        assert!(file.benches.is_empty());
        assert_eq!(fn_names(&file), ["f"]);

        // A bench takes no attributes in this stage; the block must follow the name.
        for src in ["bench \"a\" [ignore] { }\n", "bench [ignore] { }\n"] {
            let (_, out) = render(src);
            assert!(out.contains("E0001"), "{src}: {out}");
        }
    }

    #[test]
    fn bench_items_survive_recovery() {
        let (file, out) = render("fn broken( { }\nbench \"one\" { assert(true) }\nfn good() {}\n");
        assert_eq!(out.lines().count(), 1, "{out}");
        assert_eq!(fn_names(&file), ["good"]);
        assert_eq!(file.benches.len(), 1);
        assert_eq!(file.benches[0].name, "one");
    }

    #[test]
    fn generate_items_are_functions_returning_a_string() {
        // `comptime generate { ... }` at module scope is a private function with no parameters and the return type
        // `String`, named so that no written name can refer to it, and recorded in `generators` for the front end
        // (`spec/generate.md`, GN2). Its block is not one of the file's `bodies`.
        let src = "comptime generate {\n    String.new()\n}\ncomptime generate { String.new() }\nfn good() {}\n";
        let (file, out) = render(src);
        assert_eq!(out, "");
        assert_eq!(fn_names(&file), ["comptime generate 1", "comptime generate 2", "good"]);
        assert_eq!(file.generators.len(), 2);
        let Item::Fn(generator) = &file.items[0] else { panic!("expected a function") };
        assert!(generator.is_priv && generator.params.is_empty());
        assert!(
            matches!(generator.ret.as_ref().map(|t| &t.kind), Some(TypeKind::Path(p)) if p[0].name.name == "String")
        );
        assert_eq!(file.generators[0].name, "comptime generate 1");
        assert_eq!(file.generators[0].span, generator.span);
        let keyword = &src[file.generators[0].keyword.lo as usize..file.generators[0].keyword.hi as usize];
        assert_eq!(keyword, "comptime generate");
        assert_eq!(file.bodies.len(), 1, "only `good`'s body is a function body");

        // `generate` is a keyword only right after `comptime`.
        let (_, out) = render("fn f(): i32 {\n    let generate = 1\n    generate\n}\n");
        assert_eq!(out, "");
    }

    #[test]
    fn generate_outside_module_scope_is_e0243_and_the_rest_parses() {
        let (file, out) = render(
            "fn main() {\n    comptime generate {\n        String.new()\n    }\n    let x = 1\n}\n\
             Point: Display {\n    comptime generate { String.new() }\n    fn f(self) {}\n}\n\
             fn good() {}\n",
        );
        let codes: Vec<&str> = out.lines().filter(|l| l.contains("error[")).collect();
        assert_eq!(codes.len(), 2, "{out}");
        assert!(codes.iter().all(|l| l.contains("E0243")), "{out}");
        assert!(out.contains("t.bd:2:5:") && out.contains("t.bd:8:5:"), "{out}");
        assert_eq!(fn_names(&file), ["main", "good"]);
        assert!(file.generators.is_empty());
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert!(binding_names(&main.body).contains(&"x"), "the statements after the block still parse");
        let Item::Impl(block) = &file.items[1] else { panic!("expected an impl block") };
        assert_eq!(block.methods.len(), 1, "the method after the block still parses");
    }

    #[test]
    fn malformed_input_never_panics() {
        let snippets = [
            "",
            "fn",
            "fn f(",
            "fn f() {",
            "fn f() { let",
            "fn f() { let x =",
            "fn f() { x = (",
            "struct",
            "struct S {",
            "enum E {",
            "error",
            "error enum",
            "error enum E {",
            "error enum E { V }",
            "error enum E { V from }",
            "fn f() { match x {",
            "fn f() { if",
            "fn f() { for",
            "fn f() { 1 +",
            "test",
            "test \"a\"",
            "bench",
            "bench \"a\"",
            "use",
            "use a.",
            "impl",
            "extern",
            "extern \"C\" {",
            "fn f() { g(] }",
            "fn f() { a.b. }",
            "fn f() { let (a, = 1 }",
            ">>=",
            "@@@",
            "fn f() { | }",
            "fn f() { |x }",
            "fn f() { |x| }",
            "fn f() { || }",
            "fn f() { let g = |x: | 1 }",
            "fn f() { let g = |x|: }",
            "fn f(g: Fn(i32) {}",
        ];
        for src in snippets {
            let (_, out) = render(src);
            assert!(!out.is_empty() || src.is_empty(), "no diagnosis for {src:?}");
        }
    }

    #[test]
    fn error_enums_parse() {
        // `parse_file` does not lower: the error declaration stays its own item until `parse` lowers it.
        let (file, out) = render(
            "priv error enum E<T>: Debug {\n\
             Unit \"u\"\n\
             Tuple(i32, T) from \"t {0} {1}\"\n\
             Struct { line: i32 } \"s {line}\"\n\
             }\n",
        );
        assert_eq!(out, "");
        let Item::ErrorEnum(e) = &file.items[0] else { panic!("not an error enum") };
        assert!(e.is_priv);
        assert_eq!(e.name.name, "E");
        assert_eq!(e.generics.len(), 1);
        assert_eq!(e.derives.len(), 1);
        assert_eq!(e.variants.len(), 3);
        assert!(e.variants[0].from.is_none());
        assert!(e.variants[1].from.is_some());
        assert!(matches!(e.variants[1].kind, VariantKind::Tuple(ref tys) if tys.len() == 2));
        assert!(matches!(e.variants[2].kind, VariantKind::Struct(ref fields) if fields.len() == 1));
        assert_eq!(e.variants[2].message.len(), 2);
        assert!(matches!(e.variants[2].message[1], InterpPart::Hole(_)));
    }

    #[test]
    fn an_error_variant_needs_a_message() {
        let (_, out) = render("error enum E {\n    A\n}\n");
        assert!(out.contains("E0001"), "{out}");
        assert!(out.contains("a message string"), "{out}");
    }

    #[test]
    fn semicolons_still_separate_statements() {
        // The parser resumes at the next line; the AST keeps only the statements that parsed, with their spans.
        let (file, out) = render("fn main() {\n    let a = 1;\n    let b = 2;\n}\n");
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        assert_eq!(binding_names(&main.body), ["a", "b"]);
        let Stmt::Let { span: first, .. } = &main.body.stmts[0] else { unreachable!() };
        let Stmt::Let { span: second, .. } = &main.body.stmts[1] else { unreachable!() };
        assert!(first.lo < second.lo && first.hi <= second.lo);
    }

    #[test]
    fn parameters_carry_their_defaults() {
        let (file, out) = render("fn f(a: i32, b: i32 = 1, c: str = \"x\") {}\n");
        assert_eq!(out, "");
        let Item::Fn(f) = &file.items[0] else { unreachable!() };
        assert_eq!(f.params.len(), 3);
        assert!(f.params[0].default.is_none());
        assert!(matches!(f.params[1].default.as_ref().map(|e| &e.kind), Some(ExprKind::Int(1))));
        assert!(matches!(f.params[2].default.as_ref().map(|e| &e.kind), Some(ExprKind::Str(s)) if s == "x"));
    }

    #[test]
    fn call_arguments_may_be_named_and_marked() {
        let (file, out) = render("fn main() {\n    f(mut a: x, b: 2, 3)\n}\n");
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let Stmt::Expr(call) = &main.body.stmts[0] else { unreachable!() };
        let ExprKind::Call(_, args) = &call.kind else { unreachable!() };
        assert_eq!(args.len(), 3);
        assert!(args[0].mut_marker);
        assert_eq!(args[0].name.as_ref().map(|n| n.name.as_str()), Some("a"));
        assert_eq!(args[1].name.as_ref().map(|n| n.name.as_str()), Some("b"));
        assert!(args[2].name.is_none());
    }

    #[test]
    fn a_colon_makes_a_name_only_in_an_argument_list() {
        // A struct literal's field is not a named argument, and a path is not a name.
        let (file, out) = render("fn main() {\n    f(P { a: 1 }, g.b)\n}\n");
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let Stmt::Expr(call) = &main.body.stmts[0] else { unreachable!() };
        let ExprKind::Call(_, args) = &call.kind else { unreachable!() };
        assert_eq!(args.len(), 2);
        assert!(args[0].name.is_none());
        assert!(matches!(&args[0].expr.kind, ExprKind::StructLit(..)));
        assert!(args[1].name.is_none());
    }

    #[test]
    fn a_derive_item_holds_methods() {
        let (file, out) = render(
            "derive Describe {
    fn describe(self): String { x }
    priv fn helper(self) {}
}
",
        );
        assert_eq!(out, "");
        let Item::Derive(d) = &file.items[0] else { panic!("expected a derive item") };
        assert_eq!(d.name.name, "Describe");
        assert_eq!(d.methods.len(), 2);
        assert!(!d.is_priv);
        assert!(d.methods[1].is_priv);
    }

    #[test]
    fn const_parameters_have_a_type() {
        let (file, out) = render(
            "struct M<T, const R: usize, const OK: bool> { a: i32 }
",
        );
        assert_eq!(out, "");
        let Item::Struct(s) = &file.items[0] else { unreachable!() };
        assert!(s.generics[0].const_ty.is_none());
        assert!(s.generics[1].const_ty.is_some());
        assert!(s.generics[2].const_ty.is_some());
    }

    #[test]
    fn type_parameters_take_a_default() {
        // `A: Allocator = Global`: a trailing type parameter's default (spec/allocators.md, AL5). It is a type,
        // parsed after the bounds, and a use may omit the argument.
        let (file, out) = render(
            "struct Buffer<A: Allocator = Global> { items: List<i32, A> }
",
        );
        assert_eq!(out, "");
        let Item::Struct(s) = &file.items[0] else { unreachable!() };
        let param = &s.generics[0];
        assert_eq!(param.name.name, "A");
        assert_eq!(param.bounds.len(), 1);
        assert!(param.const_ty.is_none());
        let default = param.default.as_ref().expect("the default is parsed");
        let TypeKind::Path(segs) = &default.kind else { panic!("the default is a type path") };
        assert_eq!(segs[0].name.name, "Global");

        let (file, out) = render("struct Pair<T, U = List<T>> { a: T, b: U }\n");
        assert_eq!(out, "");
        let Item::Struct(s) = &file.items[0] else { unreachable!() };
        assert!(s.generics[0].default.is_none());
        assert!(s.generics[1].default.is_some(), "the default may name an earlier parameter");
    }

    #[test]
    fn const_arguments_are_values_not_types() {
        let (file, out) = render(
            "fn main() {
    let m: M<i32, 2, -1, true, { 1 + 2 }, 'a', N> = x
}
",
        );
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let Stmt::Let { ty, .. } = &main.body.stmts[0] else { unreachable!() };
        let TypeKind::Path(segs) = &ty.as_ref().expect("annotated").kind else { unreachable!() };
        let kinds: Vec<bool> = segs[0].args.iter().map(|a| matches!(a.kind, TypeKind::Const(_))).collect();
        // `i32` and `N` are types; the other five arguments are values.
        assert_eq!(kinds, vec![false, true, true, true, true, true, false]);
    }

    #[test]
    fn a_computed_field_access_takes_an_expression_in_braces() {
        let (file, out) = render(
            "fn main() {
    a.{\"x\"} = b.{n}.c
}
",
        );
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let Stmt::Expr(assign) = &main.body.stmts[0] else { unreachable!() };
        let ExprKind::Assign(lhs, rhs) = &assign.kind else { unreachable!() };
        assert!(matches!(lhs.kind, ExprKind::DynField(..)));
        let ExprKind::Field(inner, _) = &rhs.kind else { unreachable!() };
        assert!(matches!(inner.kind, ExprKind::DynField(..)));
    }

    #[test]
    fn braces_inside_generic_arguments_do_not_end_the_scan() {
        // `M<{ 1 }>.new()` is a generic path, not a comparison.
        let (file, out) = render(
            "fn main() {
    M<{ 1 }>.new()
}
",
        );
        assert_eq!(out, "");
        let Item::Fn(main) = &file.items[0] else { unreachable!() };
        let Stmt::Expr(call) = &main.body.stmts[0] else { unreachable!() };
        assert!(matches!(call.kind, ExprKind::Call(..)));
    }
}
