//! Items: structs, enums, aliases, functions, constants, interfaces, and impl blocks.

use super::{PResult, Parser, Reported};
use crate::ast::*;
use crate::token::TokenKind;
use burdin_diag::{Code, Span};

impl Parser<'_> {
    pub(super) fn file(&mut self) -> File {
        let mut file = File::default();
        while !self.at_eof() {
            self.nl_stack.clear();
            if self.at(&TokenKind::Use) {
                match self.use_decl() {
                    Ok(use_decl) => file.uses.push(use_decl),
                    Err(Reported) => self.recover_item(),
                }
                continue;
            }
            if self.at_test_item() {
                match self.test_decl() {
                    Ok(test) => file.tests.push(test),
                    Err(Reported) => self.recover_item(),
                }
                continue;
            }
            match self.item() {
                Ok(Some(item)) => file.items.push(item),
                Ok(None) => {}
                Err(Reported) => self.recover_item(),
            }
        }
        file
    }

    /// A `test` item: `test`, the contextual keyword, followed by a string. This is the only position where `test`
    /// is not an ordinary identifier.
    fn at_test_item(&self) -> bool {
        matches!(self.kind(), TokenKind::Ident(name) if name == "test") && matches!(self.nth_kind(1), TokenKind::Str(_))
    }

    pub(super) fn item(&mut self) -> PResult<Option<Item>> {
        let start = self.span();
        let is_priv = self.eat(&TokenKind::Priv);
        match self.kind() {
            TokenKind::Fn | TokenKind::Unsafe => Ok(Some(Item::Fn(self.fn_decl(is_priv, start)?))),
            TokenKind::Struct => Ok(Some(Item::Struct(self.struct_decl(is_priv, start)?))),
            TokenKind::Enum => Ok(Some(Item::Enum(self.enum_decl(is_priv, start)?))),
            TokenKind::Interface => Ok(Some(Item::Interface(self.interface_decl(is_priv, start)?))),
            TokenKind::Type => Ok(Some(Item::TypeAlias(self.type_alias(is_priv, start)?))),
            TokenKind::Const => Ok(Some(Item::Const(self.const_decl(is_priv, start)?))),
            TokenKind::Extern => Ok(Some(Item::Extern(self.extern_block(is_priv, start)?))),
            TokenKind::Ident(_) | TokenKind::SelfType if !is_priv => Ok(Some(Item::Impl(self.impl_block(start)?))),
            _ => Err(self.unexpected("an item")),
        }
    }

    /// `use path`, `use path as name`, or `use path.{item, item}`. Uses are collected separately from items,
    /// because they declare no item of their own.
    pub(super) fn use_decl(&mut self) -> PResult<UseDecl> {
        let start = self.span();
        self.expect(&TokenKind::Use)?;
        let mut path = vec![self.ident("a module or item name")?];
        let mut items = Vec::new();
        while self.eat(&TokenKind::Dot) {
            if self.at(&TokenKind::LBrace) {
                let brace_span = self.span();
                self.open(&TokenKind::LBrace, false)?;
                while !self.at(&TokenKind::RBrace) {
                    items.push(self.ident("an imported name")?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.close(&TokenKind::RBrace)?;
                if items.is_empty() {
                    self.diags.error(Code::SYNTAX, brace_span, "a `use` name list cannot be empty");
                }
                break;
            }
            path.push(self.ident("a module or item name")?);
        }
        let alias = if self.eat(&TokenKind::As) {
            let alias = self.ident("a name")?;
            if !items.is_empty() {
                self.diags.error(Code::SYNTAX, alias.span, "an imported name list cannot be combined with `as`");
            }
            Some(alias)
        } else {
            None
        };
        Ok(UseDecl { path, alias, items, span: start.to(self.prev_span()) })
    }

    /// `extern "C" { [priv] fn ... }`: declarations of C functions, which have no body.
    pub(super) fn extern_block(&mut self, is_priv: bool, start: Span) -> PResult<ExternBlock> {
        self.expect(&TokenKind::Extern)?;
        let abi_span = self.span();
        let abi = match self.kind().clone() {
            TokenKind::Str(s) => {
                self.bump();
                s
            }
            _ => return Err(self.unexpected("an ABI string, such as \"C\"")),
        };
        self.open(&TokenKind::LBrace, true)?;
        let mut decls = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            let fn_start = self.span();
            let fn_priv = self.eat(&TokenKind::Priv);
            if !matches!(self.kind(), TokenKind::Fn | TokenKind::Unsafe) {
                return Err(self.unexpected("a function signature"));
            }
            let (decl, has_body) = self.fn_decl_opt(fn_priv, fn_start, true)?;
            if has_body {
                self.diags.error(Code::SYNTAX, decl.body.span, "an `extern` function has no body");
            }
            decls.push(decl);
        }
        self.close(&TokenKind::RBrace)?;
        Ok(ExternBlock { abi, abi_span, is_priv, decls, span: start.to(self.prev_span()) })
    }

    /// Ends one entry of a list inside braces: a `,` or a line break separates entries (grammar section 2,
    /// rule 4). Returns `false` at the closing brace.
    pub(super) fn brace_list_sep(&mut self, what: &str) -> PResult<bool> {
        if self.eat(&TokenKind::Comma) {
            return Ok(!self.at(&TokenKind::RBrace));
        }
        if self.at(&TokenKind::RBrace) {
            return Ok(false);
        }
        if self.tok().nl_before && !self.at_eof() {
            return Ok(true);
        }
        Err(self.unexpected(&format!("`,`, a line break, or `}}` after the {what}")))
    }

    pub(super) fn derives(&mut self) -> PResult<Vec<Type>> {
        let mut derives = Vec::new();
        if matches!(self.kind(), TokenKind::Ident(n) if n == "derives") {
            self.bump();
            self.open(&TokenKind::LParen, false)?;
            while !self.at(&TokenKind::RParen) {
                derives.push(self.ty()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.close(&TokenKind::RParen)?;
        }
        Ok(derives)
    }

    pub(super) fn field_list(&mut self) -> PResult<Vec<FieldDecl>> {
        self.open(&TokenKind::LBrace, true)?;
        let mut fields = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            loop {
                let is_priv = self.eat(&TokenKind::Priv);
                let name = self.ident("a field name")?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.ty()?;
                fields.push(FieldDecl { is_priv, name, ty });
                if !self.brace_list_sep("field")? {
                    break;
                }
            }
        }
        self.close(&TokenKind::RBrace)?;
        Ok(fields)
    }

    pub(super) fn struct_decl(&mut self, is_priv: bool, start: Span) -> PResult<StructDecl> {
        self.expect(&TokenKind::Struct)?;
        let name = self.ident("a struct name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        let derives = self.derives()?;
        let fields = self.field_list()?;
        Ok(StructDecl { is_priv, name, generics, derives, fields, span: start.to(self.prev_span()) })
    }

    pub(super) fn enum_decl(&mut self, is_priv: bool, start: Span) -> PResult<EnumDecl> {
        self.expect(&TokenKind::Enum)?;
        let name = self.ident("an enum name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        let derives = self.derives()?;
        self.open(&TokenKind::LBrace, true)?;
        let mut variants = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            loop {
                let vname = self.ident("a variant name")?;
                let kind = if self.at(&TokenKind::LParen) {
                    self.open(&TokenKind::LParen, false)?;
                    let mut tys = Vec::new();
                    while !self.at(&TokenKind::RParen) {
                        tys.push(self.ty()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.close(&TokenKind::RParen)?;
                    VariantKind::Tuple(tys)
                } else if self.at(&TokenKind::LBrace) && !self.tok().nl_before {
                    VariantKind::Struct(self.field_list()?)
                } else {
                    VariantKind::Unit
                };
                variants.push(Variant { name: vname, kind });
                if !self.brace_list_sep("variant")? {
                    break;
                }
            }
        }
        self.close(&TokenKind::RBrace)?;
        Ok(EnumDecl { is_priv, name, generics, derives, variants, span: start.to(self.prev_span()) })
    }

    pub(super) fn type_alias(&mut self, is_priv: bool, start: Span) -> PResult<TypeAliasDecl> {
        self.expect(&TokenKind::Type)?;
        let name = self.ident("a type name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        self.expect(&TokenKind::Eq)?;
        let ty = self.ty()?;
        Ok(TypeAliasDecl { is_priv, name, generics, ty, span: start.to(self.prev_span()) })
    }

    pub(super) fn impl_block(&mut self, start: Span) -> PResult<ImplBlock> {
        let ty = self.ty()?;
        let interface = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
        let where_clause = if self.at(&TokenKind::Where) { self.where_clause()? } else { Vec::new() };
        self.open(&TokenKind::LBrace, true)?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            let fn_start = self.span();
            let is_priv = self.eat(&TokenKind::Priv);
            if !matches!(self.kind(), TokenKind::Fn | TokenKind::Unsafe) {
                return Err(self.unexpected("a method"));
            }
            methods.push(self.fn_decl(is_priv, fn_start)?);
        }
        self.close(&TokenKind::RBrace)?;
        Ok(ImplBlock { ty, interface, where_clause, methods, span: start.to(self.prev_span()) })
    }

    /// `test "name" { ... }` or `test "name" [attrs] { ... }`. The test is compiled as a function with no
    /// parameters that returns `Result<(), AssertError>`: its body is the block followed by `Ok(())`, so
    /// `assert(...)?` in the body propagates the failure to the runner. Tests are only declared and checked by
    /// `burdin test`.
    pub(super) fn test_decl(&mut self) -> PResult<TestDecl> {
        let start = self.span();
        self.bump(); // `test`
        let name = match self.kind().clone() {
            TokenKind::Str(name) => {
                self.bump();
                name
            }
            _ => return Err(self.unexpected("a test name string")),
        };
        let name_span = self.prev_span();
        let attrs = self.test_attrs()?;
        let body = self.block()?;
        let body_span = body.span;
        let index = self.test_count;
        self.test_count += 1;
        let span = start.to(body_span);

        let path = |name: &str, args: Vec<Type>| Type {
            kind: TypeKind::Path(vec![PathSegment { name: Ident { name: name.to_string(), span }, args }]),
            span,
        };
        let ret = path("Result", vec![Type { kind: TypeKind::Unit, span }, path("AssertError", Vec::new())]);
        let unit = Expr { kind: ExprKind::Unit, span };
        let ok = Expr { kind: ExprKind::Name { name: Ident { name: "Ok".to_string(), span }, args: Vec::new() }, span };
        let tail = Expr {
            kind: ExprKind::Call(Box::new(ok), vec![Arg { mut_marker: false, mut_span: None, expr: unit }]),
            span: body_span,
        };
        let mut stmts = body.stmts;
        stmts.push(Stmt::Expr(tail));
        let func = FnDecl {
            is_priv: true,
            is_unsafe: false,
            name: Ident { name: format!("__burdin_test_{index}"), span: name_span },
            generics: Vec::new(),
            params: Vec::new(),
            ret: Some(ret),
            where_clause: Vec::new(),
            body: Block { stmts, span: body_span },
            span,
        };
        Ok(TestDecl { name, name_span, attrs, func, span })
    }

    /// The optional attribute list of a test item: `[ignore, timeout(500), should_panic("boom")]`. The values are
    /// parsed but not checked here; `burdin_sema` validates the names, values, duplicates, and combinations (E0170).
    fn test_attrs(&mut self) -> PResult<Vec<TestAttr>> {
        let mut attrs = Vec::new();
        if !self.at(&TokenKind::LBracket) {
            return Ok(attrs);
        }
        self.open(&TokenKind::LBracket, true)?;
        while !self.at(&TokenKind::RBracket) {
            let name = self.ident("a test attribute")?;
            let value = if self.eat(&TokenKind::LParen) {
                let value = match self.kind().clone() {
                    TokenKind::Int(n) => {
                        self.bump();
                        TestAttrValue::Int(n)
                    }
                    TokenKind::Str(s) => {
                        self.bump();
                        TestAttrValue::Str(s)
                    }
                    _ => return Err(self.unexpected("an integer or a string")),
                };
                self.close(&TokenKind::RParen)?;
                Some(value)
            } else {
                None
            };
            attrs.push(TestAttr { name, value });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.close(&TokenKind::RBracket)?;
        Ok(attrs)
    }

    pub(super) fn interface_decl(&mut self, is_priv: bool, start: Span) -> PResult<InterfaceDecl> {
        self.expect(&TokenKind::Interface)?;
        let name = self.ident("an interface name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        let requires = if matches!(self.kind(), TokenKind::Ident(n) if n == "requires") {
            self.bump();
            self.bounds()?
        } else {
            Vec::new()
        };
        self.open(&TokenKind::LBrace, true)?;
        let mut methods = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.unexpected("`}`"));
            }
            let fn_start = self.span();
            let is_priv = self.eat(&TokenKind::Priv);
            if !matches!(self.kind(), TokenKind::Fn | TokenKind::Unsafe) {
                return Err(self.unexpected("a method"));
            }
            let (decl, has_body) = self.fn_decl_opt(is_priv, fn_start, true)?;
            methods.push(InterfaceFn { decl, has_body });
        }
        self.close(&TokenKind::RBrace)?;
        Ok(InterfaceDecl { is_priv, name, generics, requires, methods, span: start.to(self.prev_span()) })
    }

    pub(super) fn fn_decl(&mut self, is_priv: bool, start: Span) -> PResult<FnDecl> {
        Ok(self.fn_decl_opt(is_priv, start, false)?.0)
    }

    /// A function. With `body_optional`, a signature without a body is accepted (an interface method); the flag
    /// says whether a body was present.
    pub(super) fn fn_decl_opt(&mut self, is_priv: bool, start: Span, body_optional: bool) -> PResult<(FnDecl, bool)> {
        let is_unsafe = self.eat(&TokenKind::Unsafe);
        self.expect(&TokenKind::Fn)?;
        let name = self.ident("a function name")?;
        let generics = if self.at(&TokenKind::Lt) { self.generic_params()? } else { Vec::new() };
        self.open(&TokenKind::LParen, false)?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            params.push(self.param()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.close(&TokenKind::RParen)?;
        let ret = if self.eat(&TokenKind::Colon) { Some(self.ty()?) } else { None };
        let where_clause = if self.at(&TokenKind::Where) { self.where_clause()? } else { Vec::new() };
        if body_optional && !self.at(&TokenKind::LBrace) {
            let span = start.to(self.prev_span());
            let body = Block { stmts: Vec::new(), span };
            return Ok((FnDecl { is_priv, is_unsafe, name, generics, params, ret, where_clause, body, span }, false));
        }
        let body = self.block()?;
        let span = start.to(body.span);
        Ok((FnDecl { is_priv, is_unsafe, name, generics, params, ret, where_clause, body, span }, true))
    }

    pub(super) fn const_decl(&mut self, is_priv: bool, start: Span) -> PResult<ConstDecl> {
        self.expect(&TokenKind::Const)?;
        let name = self.ident("a constant name")?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.ty()?;
        self.expect(&TokenKind::Eq)?;
        let value = self.expr()?;
        let span = start.to(value.span);
        Ok(ConstDecl { is_priv, name, ty, value, span })
    }

    pub(super) fn param(&mut self) -> PResult<Param> {
        let start = self.span();
        let convention = if self.eat(&TokenKind::Mut) {
            Convention::Mut
        } else if self.eat(&TokenKind::Own) {
            Convention::Own
        } else {
            Convention::Read
        };
        if self.eat(&TokenKind::SelfValue) {
            return Ok(Param { convention, kind: ParamKind::SelfParam, span: start.to(self.prev_span()) });
        }
        let name = self.ident("a parameter name")?;
        self.expect(&TokenKind::Colon)?;
        let ty = self.ty()?;
        Ok(Param { convention, kind: ParamKind::Named { name, ty }, span: start.to(self.prev_span()) })
    }

    pub(super) fn generic_params(&mut self) -> PResult<Vec<GenericParam>> {
        self.open(&TokenKind::Lt, false)?;
        let mut params = Vec::new();
        loop {
            if self.eat_closing_angle() {
                break;
            }
            let name = self.ident("a generic parameter name")?;
            let bounds = if self.eat(&TokenKind::Colon) { self.bounds()? } else { Vec::new() };
            params.push(GenericParam { name, bounds });
            if !self.eat(&TokenKind::Comma) {
                if !self.eat_closing_angle() {
                    return Err(self.unexpected("`,` or `>`"));
                }
                break;
            }
        }
        self.nl_stack.pop();
        Ok(params)
    }

    pub(super) fn bounds(&mut self) -> PResult<Vec<Type>> {
        let mut bounds = vec![self.ty()?];
        while self.eat(&TokenKind::Plus) {
            bounds.push(self.ty()?);
        }
        Ok(bounds)
    }

    pub(super) fn where_clause(&mut self) -> PResult<Vec<WherePred>> {
        self.expect(&TokenKind::Where)?;
        let mut preds = Vec::new();
        loop {
            let ty = self.ty()?;
            self.expect(&TokenKind::Colon)?;
            let bounds = self.bounds()?;
            preds.push(WherePred { ty, bounds });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(preds)
    }
}
