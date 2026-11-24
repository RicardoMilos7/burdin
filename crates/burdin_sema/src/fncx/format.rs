//! String interpolation and the format calls (`spec/format.md`).
//!
//! A string with placeholders, `format(...)`, and the output functions with arguments are lowered here to one
//! block that builds a `String` in a `Formatter` from the standard library:
//!
//! ```text
//! { let mut __f = Formatter.new()
//!   __f.write_str("literal")
//!   __f.set_spec(...)            // only when the placeholder has a specifier
//!   <value>.fmt(mut __f)         // `debug` for `{:?}`; integers, floats, text, and the built-in
//!   __f.reset_spec()             // types call the standard library's helpers instead
//!   __f.finish() }
//! ```
//!
//! Everything the checker needs to say about a placeholder is said here, before the block is built: a missing or
//! unused argument, a specifier the value cannot take, and a value that does not implement the interface.

use super::{FnCx, error_expr, mk};
use burdin_diag::{Code, Span};
use burdin_syntax::ast::{self, Align, SpecKind};
use burdin_tir::place::is_place_expr;
use burdin_tir::{self as tir, BindMode, IntTy, LocalKind, Pat, Ty};
use std::collections::HashMap;

/// The value of one placeholder: an expression still to be checked, or one that already has a type (the receiver
/// of `x.to_string()`).
enum Value<'a> {
    Ast(&'a ast::Expr),
    Typed(tir::Expr),
}

enum Piece<'a> {
    Lit(&'a str),
    Value(Box<PieceValue<'a>>),
}

struct PieceValue<'a> {
    value: Value<'a>,
    spec: ast::FormatSpec,
    span: Span,
}

impl<'a> Piece<'a> {
    fn value(value: Value<'a>, spec: ast::FormatSpec, span: Span) -> Piece<'a> {
        Piece::Value(Box::new(PieceValue { value, spec, span }))
    }
}

fn ident(name: &str, span: Span) -> ast::Ident {
    ast::Ident { name: name.to_string(), span }
}

fn expr(kind: ast::ExprKind, span: Span) -> ast::Expr {
    ast::Expr { kind, span }
}

fn arg(kind: ast::ExprKind, span: Span) -> ast::Arg {
    ast::Arg { name: None, mut_marker: false, mut_span: None, expr: expr(kind, span) }
}

impl FnCx<'_> {
    /// A string literal with placeholders that is not the format string of a call: every placeholder names an
    /// expression.
    pub(super) fn interp_expr(&mut self, parts: &[ast::InterpPart], span: Span) -> tir::Expr {
        self.format_parts(parts, &[], span)
    }

    /// `format(fmt, args...)`, and the string part of `print(fmt, args...)`.
    pub(super) fn format_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        // Format placeholders name the call's arguments by position, not by parameter name.
        self.reject_named_args("format", args);
        let Some(first) = args.first() else {
            self.diags.error(Code::ARG_COUNT, span, "`format` needs a format string");
            return error_expr(span);
        };
        let rest: Vec<&ast::Expr> = args[1..].iter().map(|a| &a.expr).collect();
        match &first.expr.kind {
            ast::ExprKind::Interp(parts) => self.format_parts(parts, &rest, span),
            ast::ExprKind::Str(text) => {
                let parts = vec![ast::InterpPart::Lit(text.clone())];
                self.format_parts(&parts, &rest, span)
            }
            _ => {
                self.diags.error(
                    Code::FORMAT_NOT_LITERAL,
                    first.expr.span,
                    "the format string of a call with arguments must be a string literal",
                );
                for a in &rest {
                    self.expr(a, None);
                }
                error_expr(span)
            }
        }
    }

    /// `x.to_string()` for a value whose type implements `Display`.
    pub(super) fn display_to_string(&mut self, recv: tir::Expr, span: Span) -> tir::Expr {
        let spec = ast::FormatSpec::plain(span);
        let pieces = vec![Piece::value(Value::Typed(recv), spec, span)];
        self.format_pieces(pieces, Vec::new(), span)
    }

    fn format_parts(&mut self, parts: &[ast::InterpPart], positional: &[&ast::Expr], span: Span) -> tir::Expr {
        let mut uses = vec![0usize; positional.len()];
        let mut next = 0usize;
        for part in parts {
            if let ast::InterpPart::Hole(hole) = part {
                let index = match &hole.arg {
                    ast::HoleArg::Expr(_) => continue,
                    ast::HoleArg::Next => {
                        next += 1;
                        next - 1
                    }
                    ast::HoleArg::Index(n) => *n,
                };
                if index < uses.len() {
                    uses[index] += 1;
                }
            }
        }
        // An argument used by more than one placeholder is evaluated once, here, so that a call in it still runs
        // once. A place is borrowed, so the argument stays usable; a Copy value is copied.
        let mut shared: HashMap<usize, tir::Expr> = HashMap::new();
        let mut prelude = Vec::new();
        let mut broken: Vec<usize> = Vec::new();
        let mut pieces: Vec<Piece> = Vec::new();
        let mut next = 0usize;
        for part in parts {
            match part {
                ast::InterpPart::Lit(text) => pieces.push(Piece::Lit(text)),
                ast::InterpPart::Hole(hole) => {
                    let index = match &hole.arg {
                        ast::HoleArg::Expr(e) => {
                            pieces.push(Piece::value(Value::Ast(e), hole.spec, hole.span));
                            continue;
                        }
                        ast::HoleArg::Next => {
                            next += 1;
                            next - 1
                        }
                        ast::HoleArg::Index(n) => *n,
                    };
                    let Some(arg) = positional.get(index) else {
                        let what = if positional.is_empty() {
                            "this placeholder needs an argument, but there is none; a string literal outside a format call has only named placeholders such as `{name}`".to_string()
                        } else {
                            format!(
                                "this placeholder needs argument {index}, but the call has only {} (numbered from 0)",
                                positional.len()
                            )
                        };
                        self.diags.error(Code::FORMAT_ARG_MISSING, hole.span, what);
                        continue;
                    };
                    if broken.contains(&index) {
                        continue;
                    }
                    let value = if uses[index] == 1 {
                        Value::Ast(arg)
                    } else {
                        match shared.get(&index) {
                            Some(local) => Value::Typed(local.clone()),
                            None => {
                                let value = self.check_shared(index, arg, hole.span);
                                match value {
                                    Some((local, stmt)) => {
                                        shared.insert(index, local.clone());
                                        prelude.push(stmt);
                                        Value::Typed(local)
                                    }
                                    None => {
                                        broken.push(index);
                                        continue;
                                    }
                                }
                            }
                        }
                    };
                    pieces.push(Piece::value(value, hole.spec, hole.span));
                }
            }
        }
        for (index, count) in uses.iter().enumerate() {
            if *count == 0 {
                let arg = positional[index];
                self.expr(arg, None);
                self.diags.error(
                    Code::FORMAT_ARG_UNUSED,
                    arg.span,
                    format!("argument {index} is not used by the format string"),
                );
            }
        }
        self.format_pieces(pieces, prelude, span)
    }

    /// The local that holds an argument used more than once: a reference to it when it is a place, or a copy when
    /// it is `Copy`. `None` (with E0195) when the value can be had only once.
    fn check_shared(&mut self, index: usize, arg: &ast::Expr, span: Span) -> Option<(tir::Expr, tir::Stmt)> {
        let value = self.expr(arg, None);
        if value.ty == Ty::Error {
            return None;
        }
        // The argument is borrowed for each use, and a `static mut` may not be borrowed (spec/globals.md, GL4).
        if is_place_expr(&value) && self.static_mut_borrowed(&value, value.span) {
            return None;
        }
        let (bound, stmt) = if is_place_expr(&value) {
            let ty = value.ty.clone();
            let place = value;
            let at = place.span;
            let borrowed = mk(
                tir::ExprKind::Ref { place: Box::new(place), mutable: false },
                Ty::Ref { mutable: false, inner: Box::new(ty) },
                at,
            );
            let local = self.declare_hidden("__arg", borrowed.ty.clone(), false, LocalKind::Owned);
            let binding = tir::Stmt::Let(Pat::Binding(local, BindMode::View), borrowed, None);
            (local, binding)
        } else if self.globals.is_copy(&value.ty) {
            let local = self.declare_hidden("__arg", value.ty.clone(), false, LocalKind::Owned);
            let binding = tir::Stmt::Let(Pat::Binding(local, BindMode::Copy), value, None);
            (local, binding)
        } else {
            self.diags.error(
                Code::FORMAT_ARG_REUSED,
                span,
                format!("argument {index} is used more than once and is neither a place nor Copy; bind it to a variable first"),
            );
            return None;
        };
        let ty = self.locals[bound.0].ty.clone();
        Some((mk(tir::ExprKind::Local(bound), ty, span), stmt))
    }

    fn format_pieces(&mut self, pieces: Vec<Piece>, prelude: Vec<tir::Stmt>, span: Span) -> tir::Expr {
        let Some(formatter_ty) = self.globals.std_struct_ty("Formatter") else {
            self.diags.error(Code::UNKNOWN_NAME, span, "the standard library does not declare `Formatter`");
            return error_expr(span);
        };
        let f = self.declare_hidden("__f", formatter_ty.clone(), true, LocalKind::Owned);
        let mut scope = HashMap::new();
        scope.insert("__f".to_string(), f);
        self.scopes.push(scope);
        let f_expr = mk(tir::ExprKind::Local(f), formatter_ty.clone(), span);

        let mut stmts = Vec::new();
        let new = adt_method(self, &formatter_ty, "new");
        match new {
            Some(new) => {
                let init = mk(tir::ExprKind::Call(new, Vec::new()), formatter_ty.clone(), span);
                stmts.push(tir::Stmt::Let(Pat::Binding(f, BindMode::Move), init, None));
            }
            None => {
                self.scopes.pop();
                self.diags.error(Code::UNKNOWN_NAME, span, "the standard library's `Formatter` has no `new`");
                return error_expr(span);
            }
        }
        stmts.extend(prelude);
        for piece in pieces {
            match piece {
                Piece::Lit(text) => {
                    if !text.is_empty() {
                        let call = self.formatter_call(
                            "write_str",
                            vec![arg(ast::ExprKind::Str(text.to_string()), span)],
                            span,
                        );
                        stmts.push(tir::Stmt::Expr(call));
                    }
                }
                Piece::Value(piece) => {
                    let PieceValue { value, spec, span: hole } = *piece;
                    let e = match value {
                        Value::Ast(a) => self.expr(a, None),
                        Value::Typed(t) => t,
                    };
                    if e.ty == Ty::Error {
                        continue;
                    }
                    let set = !spec.is_default()
                        || matches!(
                            spec.kind,
                            SpecKind::LowerHex | SpecKind::UpperHex | SpecKind::Binary | SpecKind::Octal
                        );
                    if !self.spec_applies(&e.ty, &spec, hole) {
                        continue;
                    }
                    if set {
                        let call = self.set_spec_call(&spec, hole);
                        stmts.push(tir::Stmt::Expr(call));
                    }
                    let debug = spec.kind == SpecKind::Debug;
                    let call = self.format_value(e, debug, &f_expr, "__f", hole);
                    stmts.push(tir::Stmt::Expr(call));
                    if set {
                        let call = self.formatter_call("reset_spec", Vec::new(), hole);
                        stmts.push(tir::Stmt::Expr(call));
                    }
                }
            }
        }
        let tail = self.formatter_call("finish", Vec::new(), span);
        self.scopes.pop();
        let block = tir::Block { stmts, tail: Some(Box::new(tail)), close: span };
        mk(tir::ExprKind::Block(block), Ty::String, span)
    }

    /// `__f.name(args)` on the hidden formatter.
    fn formatter_call(&mut self, name: &str, args: Vec<ast::Arg>, span: Span) -> tir::Expr {
        let recv = expr(ast::ExprKind::Name { name: ident("__f", span), args: Vec::new() }, span);
        self.method_call(&recv, &ident(name, span), &[], &args, span, None)
    }

    fn set_spec_call(&mut self, spec: &ast::FormatSpec, span: Span) -> tir::Expr {
        use ast::ExprKind as K;
        let align = match spec.align {
            Align::Default => 0,
            Align::Left => 1,
            Align::Center => 2,
            Align::Right => 3,
        };
        let radix = match spec.kind {
            SpecKind::LowerHex => 16,
            SpecKind::UpperHex => 17,
            SpecKind::Binary => 2,
            SpecKind::Octal => 8,
            SpecKind::Display | SpecKind::Debug => 10,
        };
        let args = vec![
            arg(K::Char(spec.fill), span),
            arg(K::Int(align), span),
            arg(K::Bool(spec.plus), span),
            arg(K::Bool(spec.alt), span),
            arg(K::Bool(spec.zero), span),
            arg(K::Bool(spec.width.is_some()), span),
            arg(K::Int(spec.width.unwrap_or(0) as u128), span),
            arg(K::Bool(spec.precision.is_some()), span),
            arg(K::Int(spec.precision.unwrap_or(0) as u128), span),
            arg(K::Int(radix), span),
        ];
        self.formatter_call("set_spec", args, span)
    }

    /// Whether the specifier makes sense for the value; reports E0190 when it does not.
    fn spec_applies(&mut self, ty: &Ty, spec: &ast::FormatSpec, span: Span) -> bool {
        let mut inner = ty;
        while let Ty::Ref { inner: i, .. } = inner {
            inner = i;
        }
        let is_int = matches!(inner, Ty::Int(_));
        let is_float = matches!(inner, Ty::Float(_));
        let is_text = matches!(inner, Ty::Str | Ty::String | Ty::Char | Ty::Bool);
        let user = matches!(inner, Ty::Struct(_) | Ty::Enum(_) | Ty::Param(_));
        let radix = matches!(spec.kind, SpecKind::LowerHex | SpecKind::UpperHex | SpecKind::Binary | SpecKind::Octal);
        let problem = if radix && !is_int {
            Some(format!("`x`, `X`, `b`, and `o` format integers, not `{inner}`"))
        } else if spec.alt && !radix && !user {
            Some("the `#` flag needs `x`, `X`, `b`, or `o`".to_string())
        } else if spec.precision.is_some() && is_int {
            Some("a precision does not apply to integers".to_string())
        } else if (spec.plus || spec.zero) && !(is_int || is_float || user) {
            Some(format!("the `+` and `0` flags format numbers, not `{inner}`"))
        } else if spec.precision.is_some() && !(is_float || is_text || user) {
            Some(format!("a precision does not apply to `{inner}`"))
        } else {
            None
        };
        match problem {
            Some(message) => {
                self.diags.error(Code::BAD_FORMAT_SPEC, span, message);
                false
            }
            None => true,
        }
    }

    fn formattable(&self, ty: &Ty, debug: bool) -> bool {
        let iface = if debug { self.globals.debug_iface } else { self.globals.display_iface };
        let Some(iface) = iface else { return false };
        self.globals.satisfies(ty, &crate::generics::Bound::User(iface, Vec::new()), &self.bounds)
    }

    /// Whether `x.to_string()` can format a value of this type through `Display` (the integer, bool, char, and
    /// str `to_string` methods are built in and keep working as before).
    pub(super) fn to_string_via_display(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Float(_) => true,
            Ty::Struct(_) | Ty::Enum(_) | Ty::Param(_) | Ty::Any(_) => self.formattable(ty, false),
            _ => false,
        }
    }

    /// Writes `e` to the formatter `f` (a place of type `Formatter`) with `Display` or `Debug`.
    pub(super) fn format_value(
        &mut self,
        e: tir::Expr,
        debug: bool,
        f: &tir::Expr,
        f_name: &str,
        span: Span,
    ) -> tir::Expr {
        let e = self.auto_deref(e);
        if !self.formattable(&e.ty, debug) {
            let (interface, spec) = if debug { ("Debug", "{:?}") } else { ("Display", "{}") };
            let mut message = format!("`{}` does not implement `{interface}`, which `{spec}` needs", e.ty);
            if !debug && self.formattable(&e.ty, true) {
                message.push_str("; it implements `Debug`, so `{:?}` works");
            }
            self.diags.error(Code::NOT_FORMATTABLE, span, message);
            return error_expr(span);
        }
        let int = |bits: i128| mk(tir::ExprKind::Int(bits), Ty::Int(IntTy::I32), span);
        let boolean = |b: bool| mk(tir::ExprKind::Bool(b), Ty::Bool, span);
        let cast = |e: tir::Expr, to: IntTy| {
            let span = e.span;
            mk(tir::ExprKind::Cast(Box::new(e)), Ty::Int(to), span)
        };
        let f = f.clone();
        match e.ty.clone() {
            Ty::Int(IntTy::I128) => self.std_call("__fmt_i128", vec![e, int(128), f], span),
            Ty::Int(IntTy::U128) => self.std_call("__fmt_u128", vec![e, f], span),
            Ty::Int(i) if i.signed() => {
                let bits = i128::from(i.bits());
                self.std_call("__fmt_i64", vec![cast(e, IntTy::I64), int(bits), f], span)
            }
            Ty::Int(_) => self.std_call("__fmt_u64", vec![cast(e, IntTy::U64), f], span),
            Ty::Float(ft) => {
                let single = ft == tir::FloatTy::F32;
                let value = mk(tir::ExprKind::Cast(Box::new(e)), Ty::Float(tir::FloatTy::F64), span);
                self.std_call("__fmt_float", vec![value, boolean(single), boolean(debug), f], span)
            }
            Ty::Bool => self.std_call("__fmt_bool", vec![e, f], span),
            Ty::Char => self.std_call(if debug { "__debug_char" } else { "__fmt_char" }, vec![e, f], span),
            Ty::Str => self.std_call(if debug { "__debug_str" } else { "__fmt_str" }, vec![e, f], span),
            Ty::String => {
                let view = mk(tir::ExprKind::StringAsStr(Box::new(e)), Ty::Str, span);
                self.std_call(if debug { "__debug_str" } else { "__fmt_str" }, vec![view, f], span)
            }
            Ty::Unit => self.std_call("__debug_unit", vec![f], span),
            Ty::Option(_) => self.generic_format("__debug_option", e, f_name, span),
            Ty::Result(..) => self.generic_format("__debug_result", e, f_name, span),
            Ty::List(_, _) => self.generic_format("__debug_list", e, f_name, span),
            Ty::Map(_, _, _) => self.generic_format("__debug_map", e, f_name, span),
            Ty::Heap(_, _) => self.generic_format("__debug_heap", e, f_name, span),
            Ty::Tuple(ts) => {
                let name = format!("__debug_tuple{}", ts.len());
                self.generic_format(&name, e, f_name, span)
            }
            _ => {
                let method = if debug { "debug" } else { "fmt" };
                let mut_arg = mut_name_arg(f_name, span);
                self.method_call_on(e, &ident(method, span), &[mut_arg], span, None)
            }
        }
    }

    pub(super) fn std_call(&mut self, name: &str, args: Vec<tir::Expr>, span: Span) -> tir::Expr {
        match self.globals.std_func(name) {
            Some(f) => {
                let ret = self.globals.sigs[f.0].ret.clone();
                mk(tir::ExprKind::Call(f, args), ret, span)
            }
            None => {
                self.diags.error(Code::UNKNOWN_NAME, span, format!("the standard library does not declare `{name}`"));
                error_expr(span)
            }
        }
    }

    /// A call to one of the standard library's generic `__debug_*` helpers with the value as its first argument
    /// and the formatter as its second.
    fn generic_format(&mut self, name: &str, value: tir::Expr, f_name: &str, span: Span) -> tir::Expr {
        let Some(g) = self.globals.std_generic(name) else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("the standard library does not declare `{name}`"));
            return error_expr(span);
        };
        let mut_arg = mut_name_arg(f_name, span);
        self.generic_call_with_receiver(g, value, name, &[mut_arg], span)
    }

    /// `x.fmt(mut f)` or `x.debug(mut f)` written by the program (or by a derive) on a built-in type: the
    /// standard library's helpers format it.
    pub(super) fn builtin_format_method(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let debug = match name.name.as_str() {
            "fmt" => false,
            "debug" => true,
            _ => return None,
        };
        let formatter_ty = self.globals.std_struct_ty("Formatter")?;
        if args.len() != 1 || !args[0].mut_marker || !self.formattable(&recv.ty, debug) {
            return None;
        }
        let ast::ExprKind::Name { name: f_name, .. } = &args[0].expr.kind else { return None };
        let f = self.expr(&args[0].expr, Some(&formatter_ty));
        if f.ty != formatter_ty {
            return None;
        }
        Some(self.format_value(recv, debug, &f, &f_name.name, span))
    }
}

/// The argument `mut name`.
fn mut_name_arg(name: &str, span: Span) -> ast::Arg {
    let mut a = arg(ast::ExprKind::Name { name: ident(name, span), args: Vec::new() }, span);
    a.mut_marker = true;
    a.mut_span = Some(span);
    a
}

fn adt_method(cx: &FnCx<'_>, ty: &Ty, name: &str) -> Option<tir::FuncId> {
    let key = crate::globals::adt_key(ty)?;
    cx.globals.methods.get(&(key, name.to_string())).copied()
}
