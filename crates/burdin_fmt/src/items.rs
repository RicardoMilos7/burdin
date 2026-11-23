//! Items: functions, structs, enums, interfaces, impl blocks, and the rest of a file (spec/grammar.md, section 3).

use crate::cursor::{D, Fmt, R};
use crate::doc::{Doc, concat, space};
use crate::exprs::Restriction;
use crate::layout::{Elem, Member};
use crate::trivia::Comment;
use crate::uses::UseEntry;
use burdin_syntax::lookahead;
use burdin_syntax::token::TokenKind;

/// One top-level item, `use` declaration, or test with the source range it came from.
pub struct Unit {
    /// The byte range of the item and its comments in the source.
    pub start: usize,
    pub end: usize,
    pub doc: D,
    /// The source had a blank line before it.
    pub blank_before: bool,
    /// Set for a `use` declaration.
    pub use_entry: Option<UseEntry>,
}

impl Fmt<'_> {
    /// Reads the whole file: its units and the comments after the last token.
    pub fn file(&mut self) -> R<(Vec<Unit>, Vec<Comment>)> {
        let mut units = Vec::new();
        while !self.at_eof() {
            self.nl_stack.clear();
            self.stmt_start = usize::MAX;
            let start = self.tok().start;
            let blank_before = self.blank_before();
            let (doc, use_entry) = if self.at_use() {
                let entry = self.use_entry()?;
                (Doc::Nil, Some(entry))
            } else {
                (self.item()?, None)
            };
            let end = self.toks[self.pos - 1].end;
            units.push(Unit { start, end, doc, blank_before, use_entry });
        }
        let eof_comments = std::mem::take(&mut self.toks[self.pos].lead);
        Ok((units, eof_comments))
    }

    fn at_test_item(&self) -> bool {
        self.at_word("test") && matches!(self.nth(1), TokenKind::Str(_))
    }

    fn at_bench_item(&self) -> bool {
        self.at_word("bench") && matches!(self.nth(1), TokenKind::Str(_))
    }

    /// `fuzz name(data: [u8]) { ... }` (`spec/fuzz.md`, FZ1).
    fn at_fuzz_item(&self) -> bool {
        self.at_word("fuzz")
            && matches!(self.nth(1), TokenKind::Ident(_))
            && matches!(self.nth(2), TokenKind::LParen | TokenKind::Lt)
    }

    fn item(&mut self) -> R<D> {
        let lead = self.start_of_line();
        if self.at_test_item() {
            let body = self.test_item()?;
            return Ok(concat(vec![lead, body]));
        }
        if self.at_bench_item() {
            let body = self.bench_item()?;
            return Ok(concat(vec![lead, body]));
        }
        if self.at_fuzz_item() {
            let body = self.fuzz_item()?;
            return Ok(concat(vec![lead, body]));
        }
        let mut parts = vec![lead];
        let is_priv = self.at(&TokenKind::Priv);
        if is_priv {
            parts.push(self.tok_doc());
            parts.push(space());
        }
        let body = match self.kind().clone() {
            TokenKind::Unsafe if !is_priv && matches!(self.nth(1), TokenKind::Ident(_) | TokenKind::SelfType) => {
                let kw = self.tok_doc();
                let block = self.impl_block()?;
                concat(vec![kw, space(), block])
            }
            TokenKind::Fn | TokenKind::Unsafe => self.fn_decl(false, false)?,
            TokenKind::Struct => self.struct_decl()?,
            TokenKind::Enum => self.enum_decl(false)?,
            TokenKind::Error => self.enum_decl(true)?,
            TokenKind::Interface => self.interface_decl()?,
            TokenKind::Type => self.type_alias()?,
            TokenKind::Const => self.const_decl()?,
            TokenKind::Comptime if !is_priv => self.comptime_item()?,
            TokenKind::Extern => self.extern_item()?,
            TokenKind::Reserved(word) if word == "derive" => self.derive_decl()?,
            TokenKind::Ident(_) | TokenKind::SelfType if !is_priv => self.impl_block()?,
            _ => return Err(self.error("expected an item")),
        };
        parts.push(body);
        Ok(concat(parts))
    }

    // ----- functions -------------------------------------------------------------------------------------------

    /// `[unsafe] fn name<T>(params): Type where ... { body }`, and the extension form `fn Type.name(...)`. With
    /// `body_optional`, an interface or `extern` signature may have no body. With `member`, the name is a method's
    /// name in an impl block, an interface, or a derive body, which may be a member name (issue #933).
    pub fn fn_decl(&mut self, body_optional: bool, member: bool) -> R<D> {
        let extension = lookahead::extension_ahead(&self.raw, self.pos);
        let mut parts = Vec::new();
        if self.at(&TokenKind::Unsafe) {
            parts.push(self.tok_doc());
            parts.push(space());
        }
        parts.push(self.take(&TokenKind::Fn)?);
        parts.push(space());
        if extension {
            if self.at(&TokenKind::LBracket) {
                parts.push(self.ty()?);
                parts.push(self.take(&TokenKind::Dot)?);
                parts.push(self.ident()?);
            } else {
                loop {
                    let name = self.ident()?;
                    if self.at(&TokenKind::Lt) {
                        let receiver_args = lookahead::generic_list_end(&self.raw, self.pos)
                            .is_some_and(|end| matches!(self.raw.get(end).map(|t| &t.kind), Some(TokenKind::Dot)));
                        if receiver_args {
                            parts.push(name);
                            parts.push(self.angle_args()?);
                            parts.push(self.take(&TokenKind::Dot)?);
                            continue;
                        }
                        parts.push(name);
                        break;
                    }
                    parts.push(name);
                    if self.at(&TokenKind::Dot) {
                        parts.push(self.tok_doc());
                        continue;
                    }
                    break;
                }
            }
        } else if member {
            parts.push(self.member_name()?);
        } else {
            parts.push(self.ident()?);
        }
        parts.push(self.fn_rest(body_optional)?);
        Ok(concat(parts))
    }

    /// The rest of a function after its name: generics, parameters, the return type, bounds, and the body.
    fn fn_rest(&mut self, body_optional: bool) -> R<D> {
        let mut parts = Vec::new();
        if self.at(&TokenKind::Lt) {
            parts.push(self.generic_params()?);
        }
        let params = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.param())?;
        parts.push(self.list_doc(params, false));
        if self.at(&TokenKind::Colon) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        let has_where = self.at(&TokenKind::Where);
        if has_where {
            parts.push(self.where_clause()?);
        }
        let has_body = self.at(&TokenKind::LBrace);
        if !has_body && !body_optional {
            return Err(self.error("expected a function body"));
        }
        if has_body && !has_where {
            parts.push(space());
        }
        if has_body {
            let block = self.block_parts()?;
            parts.push(self.block_doc(block, false));
        }
        Ok(concat(parts))
    }

    fn param(&mut self) -> R<D> {
        if self.at(&TokenKind::DotDotDot) {
            return Ok(self.tok_doc());
        }
        let mut parts = Vec::new();
        if self.at(&TokenKind::Comptime) {
            parts.push(self.tok_doc());
            parts.push(space());
        }
        if matches!(self.kind(), TokenKind::Mut | TokenKind::Own) {
            parts.push(self.tok_doc());
            parts.push(space());
        }
        if self.at(&TokenKind::SelfValue) {
            parts.push(self.tok_doc());
            return Ok(concat(parts));
        }
        parts.push(self.ident()?);
        parts.push(self.take(&TokenKind::Colon)?);
        parts.push(space());
        parts.push(self.ty()?);
        if self.at(&TokenKind::Eq) {
            let eq = self.tok_doc();
            let default = self.expr()?;
            parts.push(space());
            parts.push(eq);
            parts.push(self.after_operator(default));
        }
        Ok(concat(parts))
    }

    // ----- data ------------------------------------------------------------------------------------------------

    /// `{ [priv] name: Type, ... }`: fields, one per line.
    fn field_list(&mut self) -> R<D> {
        let open = self.open(&TokenKind::LBrace, true)?;
        self.depth += 1;
        let (members, close) = self.brace_entries(|f| {
            let lead = f.start_of_line();
            let mut parts = vec![lead];
            if f.at(&TokenKind::Priv) {
                parts.push(f.tok_doc());
                parts.push(space());
            }
            parts.push(f.ident()?);
            parts.push(f.take(&TokenKind::Colon)?);
            parts.push(space());
            parts.push(f.ty()?);
            Ok(concat(parts))
        })?;
        self.depth -= 1;
        Ok(self.members_doc(open, members, close, ","))
    }

    /// `: A, B` after a struct or enum name, when the next token is `:`: the interfaces the declaration implements.
    fn derives(&mut self) -> R<D> {
        if !self.at(&TokenKind::Colon) {
            return Ok(Doc::Nil);
        }
        let mut parts = vec![self.tok_doc(), space(), self.ty()?];
        while self.at(&TokenKind::Comma) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        Ok(concat(parts))
    }

    fn struct_decl(&mut self) -> R<D> {
        let mut parts = vec![self.tok_doc(), space(), self.ident()?];
        if self.at(&TokenKind::Lt) {
            parts.push(self.generic_params()?);
        }
        parts.push(self.derives()?);
        if self.at_word("repr") {
            let kw = self.tok_doc();
            let list = self.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.ident())?;
            parts.push(space());
            parts.push(kw);
            parts.push(self.list_doc(list, false));
        }
        parts.push(space());
        parts.push(self.field_list()?);
        Ok(concat(parts))
    }

    /// `enum Name { Variant, Variant(A, B), Variant { a: A } }`, and the `error enum` form whose variants carry a
    /// message.
    fn enum_decl(&mut self, is_error: bool) -> R<D> {
        let mut parts = vec![self.tok_doc(), space()];
        if is_error {
            parts.push(self.take(&TokenKind::Enum)?);
            parts.push(space());
        }
        parts.push(self.ident()?);
        if self.at(&TokenKind::Lt) {
            parts.push(self.generic_params()?);
        }
        parts.push(self.derives()?);
        parts.push(space());
        let open = self.open(&TokenKind::LBrace, true)?;
        self.depth += 1;
        let (members, close) = self.brace_entries(|f| {
            let lead = f.start_of_line();
            let mut parts = vec![lead, f.ident()?];
            if f.at(&TokenKind::LParen) {
                let list = f.delimited(&TokenKind::LParen, &TokenKind::RParen, |f| f.ty())?;
                parts.push(f.list_doc(list, false));
            } else if f.at(&TokenKind::LBrace) && !f.tok().nl_before {
                parts.push(space());
                parts.push(f.variant_fields()?);
            }
            if is_error {
                if f.at_word("from") {
                    parts.push(space());
                    parts.push(f.tok_doc());
                }
                parts.push(space());
                if !matches!(f.kind(), TokenKind::Str(_) | TokenKind::Interp(_)) {
                    return Err(f.error("expected a message string"));
                }
                parts.push(f.tok_doc());
            }
            Ok(concat(parts))
        })?;
        self.depth -= 1;
        parts.push(self.members_doc(open, members, close, ","));
        Ok(concat(parts))
    }

    /// `{ a: A, b: B }` after a variant name, on one line when it fits.
    fn variant_fields(&mut self) -> R<D> {
        let open = self.open(&TokenKind::LBrace, true)?;
        let mut elems = Vec::new();
        let mut trailing_comma = false;
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let mut parts = Vec::new();
            if self.at(&TokenKind::Priv) {
                parts.push(self.tok_doc());
                parts.push(space());
            }
            parts.push(self.ident()?);
            parts.push(self.take(&TokenKind::Colon)?);
            parts.push(space());
            parts.push(self.ty()?);
            let doc = concat(parts);
            if self.at(&TokenKind::Comma) {
                let after = self.dropped();
                elems.push(Elem { doc, after });
                trailing_comma = true;
            } else {
                elems.push(Elem { doc, after: Doc::Nil });
                trailing_comma = false;
                if !self.tok().nl_before && !self.at(&TokenKind::RBrace) {
                    return Err(self.error("expected `,`, a line break, or `}`"));
                }
            }
        }
        let keep_broken = trailing_comma && self.tok().nl_before;
        let close = self.close(&TokenKind::RBrace)?;
        Ok(self.inline_braces_doc(open, elems, keep_broken, close))
    }

    fn type_alias(&mut self) -> R<D> {
        let mut parts = vec![self.tok_doc(), space(), self.ident()?];
        if self.at(&TokenKind::Lt) {
            parts.push(self.generic_params()?);
        }
        parts.push(space());
        parts.push(self.take(&TokenKind::Eq)?);
        parts.push(space());
        parts.push(self.ty()?);
        Ok(concat(parts))
    }

    fn const_decl(&mut self) -> R<D> {
        let mut parts = vec![self.tok_doc(), space(), self.ident()?];
        parts.push(self.take(&TokenKind::Colon)?);
        parts.push(space());
        parts.push(self.ty()?);
        let eq = self.take(&TokenKind::Eq)?;
        let value = self.expr()?;
        parts.push(space());
        parts.push(eq);
        parts.push(self.after_operator(value));
        Ok(concat(parts))
    }

    /// `comptime assert(condition, "message")` or `comptime generate { ... }` at module scope. The block is laid
    /// out as a function body (`spec/generate.md`, GN9).
    fn comptime_item(&mut self) -> R<D> {
        if matches!(self.nth(1), TokenKind::Ident(name) if name == "generate") && self.nth(2) == &TokenKind::LBrace {
            let comptime = self.tok_doc();
            let generate = self.tok_doc();
            let block = self.block_parts()?;
            return Ok(concat(vec![comptime, space(), generate, space(), self.block_doc(block, false)]));
        }
        Ok(self.expr_r(Restriction::None)?.doc)
    }

    // ----- interfaces, impls, extern ---------------------------------------------------------------------------

    /// `{ [priv] [blocking] fn ... }`: methods, one per line, blank lines kept. With `body_optional` a method may be
    /// a bare signature; with `blocking_ok` the contextual `blocking` marker is accepted, as it is inside
    /// `extern "C" { ... }` (`spec/io.md`, IO4). With `member_names` a method name may be a member name (issue #933);
    /// an `extern` block's names may not.
    fn method_list(&mut self, body_optional: bool, blocking_ok: bool, member_names: bool) -> R<D> {
        let open = self.open(&TokenKind::LBrace, true)?;
        self.depth += 1;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at_eof() {
                return Err(self.error("expected `}`"));
            }
            let blank_before = self.blank_before();
            let lead = self.start_of_line();
            let mut parts = vec![lead];
            // An associated type: `type Name` in an interface, `type Name = Ty` in an implementation
            // (`spec/associated-types.md`, AT1-AT2).
            if self.at(&TokenKind::Type) {
                parts.push(self.tok_doc());
                parts.push(space());
                parts.push(self.ident()?);
                if self.at(&TokenKind::Eq) {
                    parts.push(space());
                    parts.push(self.tok_doc());
                    parts.push(space());
                    parts.push(self.ty()?);
                }
                members.push(Member { doc: concat(parts), after: Doc::Nil, blank_before });
                continue;
            }
            if self.at(&TokenKind::Priv) {
                parts.push(self.tok_doc());
                parts.push(space());
            }
            if blocking_ok && self.at_word("blocking") {
                parts.push(self.tok_doc());
                parts.push(space());
            }
            if !matches!(self.kind(), TokenKind::Fn | TokenKind::Unsafe) {
                return Err(self.error("expected a method"));
            }
            parts.push(self.fn_decl(body_optional, member_names)?);
            members.push(Member { doc: concat(parts), after: Doc::Nil, blank_before });
        }
        self.depth -= 1;
        let close = self.close(&TokenKind::RBrace)?;
        Ok(self.members_doc(open, members, close, ""))
    }

    fn interface_decl(&mut self) -> R<D> {
        let mut parts = vec![self.tok_doc(), space(), self.ident()?];
        if self.at(&TokenKind::Lt) {
            parts.push(self.generic_params()?);
        }
        if self.at_word("requires") {
            parts.push(space());
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.bounds()?);
        }
        parts.push(space());
        parts.push(self.method_list(true, false, true)?);
        Ok(concat(parts))
    }

    /// `Type { methods }` and `Type: Interface where ... { methods }`; the cursor is at the type.
    fn impl_block(&mut self) -> R<D> {
        let mut parts = vec![self.ty()?];
        if self.at(&TokenKind::Colon) {
            parts.push(self.tok_doc());
            parts.push(space());
            parts.push(self.ty()?);
        }
        if self.at(&TokenKind::Where) {
            parts.push(self.where_clause()?);
        } else {
            parts.push(space());
        }
        parts.push(self.method_list(false, false, true)?);
        Ok(concat(parts))
    }

    /// `derive Name { fn ... }`.
    fn derive_decl(&mut self) -> R<D> {
        let kw = self.tok_doc();
        let name = self.ident()?;
        let methods = self.method_list(false, false, true)?;
        Ok(concat(vec![kw, space(), name, space(), methods]))
    }

    /// `extern "C" { fn ... }` and `extern "C" fn ... { }`.
    fn extern_item(&mut self) -> R<D> {
        let kw = self.tok_doc();
        if !matches!(self.kind(), TokenKind::Str(_)) {
            return Err(self.error("expected an ABI string"));
        }
        let abi = self.tok_doc();
        let mut parts = vec![kw, space(), abi, space()];
        if self.at(&TokenKind::LBrace) {
            parts.push(self.method_list(true, true, false)?);
        } else {
            if self.at(&TokenKind::Priv) {
                parts.push(self.tok_doc());
                parts.push(space());
            }
            parts.push(self.fn_decl(false, false)?);
        }
        Ok(concat(parts))
    }

    /// `test "name" [attributes] { ... }`.
    fn test_item(&mut self) -> R<D> {
        let kw = self.tok_doc();
        let name = self.tok_doc();
        let mut parts = vec![kw, space(), name];
        if self.at(&TokenKind::LBracket) {
            let attrs = self.delimited(&TokenKind::LBracket, &TokenKind::RBracket, |f| {
                let mut parts = vec![f.ident()?];
                if f.at(&TokenKind::LParen) {
                    parts.push(f.tok_doc());
                    parts.push(f.tok_doc());
                    parts.push(f.take(&TokenKind::RParen)?);
                }
                Ok(concat(parts))
            })?;
            parts.push(space());
            parts.push(self.list_doc(attrs, false));
        }
        parts.push(space());
        let block = self.block_parts()?;
        parts.push(self.block_doc(block, false));
        Ok(concat(parts))
    }

    /// `bench "name" { ... }` (`spec/bench.md`, BN1): the keyword, the name, and the block. A benchmark takes no
    /// attributes in this stage.
    /// `fuzz name(params) { ... }`: the keyword, the name, and the rest of a function.
    fn fuzz_item(&mut self) -> R<D> {
        let kw = self.tok_doc();
        let name = self.ident()?;
        let rest = self.fn_rest(false)?;
        Ok(concat(vec![kw, space(), name, rest]))
    }

    fn bench_item(&mut self) -> R<D> {
        let kw = self.tok_doc();
        let name = self.tok_doc();
        let block = self.block_parts()?;
        Ok(concat(vec![kw, space(), name, space(), self.block_doc(block, false)]))
    }
}
