//! Items: structs, enums, aliases, functions, constants, interfaces, and impl blocks.

use super::{PResult, Parser, Reported};
use crate::ast::*;
use crate::token::TokenKind;
use burdin_diag::Span;

impl Parser<'_> {
    pub(super) fn file(&mut self) -> File {
        let mut file = File::default();
        while !self.at_eof() {
            self.nl_stack.clear();
            match self.item() {
                Ok(Some(item)) => file.items.push(item),
                Ok(None) => {}
                Err(Reported) => self.recover_item(),
            }
        }
        file
    }

    pub(super) fn item(&mut self) -> PResult<Option<Item>> {
        let start = self.span();
        if let TokenKind::Ident(name) = self.kind()
            && name == "test"
            && matches!(self.nth_kind(1), TokenKind::Str(_))
        {
            return self.skip_unsupported_item(start, "`test` blocks");
        }
        let is_priv = self.eat(&TokenKind::Priv);
        match self.kind() {
            TokenKind::Fn | TokenKind::Unsafe => Ok(Some(Item::Fn(self.fn_decl(is_priv, start)?))),
            TokenKind::Struct => Ok(Some(Item::Struct(self.struct_decl(is_priv, start)?))),
            TokenKind::Enum => Ok(Some(Item::Enum(self.enum_decl(is_priv, start)?))),
            TokenKind::Interface => Ok(Some(Item::Interface(self.interface_decl(is_priv, start)?))),
            TokenKind::Type => Ok(Some(Item::TypeAlias(self.type_alias(is_priv, start)?))),
            TokenKind::Const => Ok(Some(Item::Const(self.const_decl(is_priv, start)?))),
            TokenKind::Use => self.skip_unsupported_item(start, "`use`"),
            TokenKind::Extern => self.skip_unsupported_item(start, "`extern` blocks"),
            TokenKind::Ident(_) | TokenKind::SelfType if !is_priv => Ok(Some(Item::Impl(self.impl_block(start)?))),
            _ => Err(self.unexpected("an item")),
        }
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

    /// Reports an item this compiler cannot compile yet and skips it whole: through its first balanced `{ ... }`,
    /// or to the end of the line for items without a body. This keeps the methods inside it from being parsed as
    /// separate items.
    pub(super) fn skip_unsupported_item(&mut self, start: Span, what: &str) -> PResult<Option<Item>> {
        self.unsupported(start, what);
        let first = self.pos;
        let mut depth = 0i32;
        let mut seen_brace = false;
        while !self.at_eof() {
            let t = self.tok();
            if depth == 0 && !seen_brace && self.pos > first && t.nl_before {
                break;
            }
            match t.kind {
                TokenKind::LBrace => {
                    depth += 1;
                    seen_brace = true;
                }
                TokenKind::LParen | TokenKind::LBracket => depth += 1,
                TokenKind::RParen | TokenKind::RBracket => depth = (depth - 1).max(0),
                TokenKind::RBrace => {
                    depth = (depth - 1).max(0);
                    if depth == 0 && seen_brace {
                        self.bump();
                        break;
                    }
                }
                _ => {}
            }
            self.bump();
        }
        Ok(None)
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
