//! The parser (spec/grammar.md). It reports E0001 for anything outside the grammar and recovers at statement and
//! item boundaries so that every syntax error in a file is reported.
//!
//! Constructs that are in the grammar but not yet implemented by this compiler are reported with
//! `Code::UNSUPPORTED`; `spec/e9000-audit.md` tracks the remaining sites.

mod expr;
mod format;
mod items;
mod pattern;
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
    };
    p.file()
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
        matches!(kind, Fn | Struct | Enum | Error | Interface | Type | Const | Use | Extern | Priv)
    }

    /// Skips to the start of the next item: an item keyword at the start of a line.
    fn recover_item(&mut self) {
        let mut depth = 0i32;
        let start = self.pos;
        while !self.at_eof() {
            let t = self.tok();
            // `test` is contextual, so it only starts an item when a string follows it.
            let test_item = matches!(&t.kind, TokenKind::Ident(name) if name == "test")
                && self.toks.get(self.pos + 1).is_some_and(|next| matches!(next.kind, TokenKind::Str(_)));
            let item_start =
                Self::starts_item_only(&t.kind) || (depth == 0 && t.kind == TokenKind::Unsafe) || test_item;
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
            "priv error enum E<T> derives (Debug) {\n\
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
}
