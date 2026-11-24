//! Function values: a named function, a built-in function, or `Some` used where a closure value is expected.
//!
//! Each one becomes an ordinary closure with no captures whose body forwards to the function, so the call,
//! storage, and drop rules are the closure rules (CL11 in `spec/memory-rules.md`).

use super::{FnCx, error_expr, mk};
use crate::builtins::builtin_sig;
use crate::globals::adt_key;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Builtin, FnSig, FuncId, LocalKind, Ty};
use std::rc::Rc;

/// The prelude constructors that take one payload and can be used as a function value (CL11).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Ctor {
    Some,
    Ok,
    Err,
}

impl Ctor {
    fn name(self) -> &'static str {
        match self {
            Ctor::Some => "Some",
            Ctor::Ok => "Ok",
            Ctor::Err => "Err",
        }
    }
}

impl FnCx<'_> {
    /// A named, non-generic function used as a value. Its closure parameter list is the function's parameter
    /// types; every argument is passed by value (CL6) and then forwarded with the function's own conventions.
    pub(super) fn function_value(&mut self, f: FuncId, span: Span) -> tir::Expr {
        if self.globals.fn_unsafe[f.0] {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                "an `unsafe fn` cannot be used as a closure value; write a closure whose body calls it in an \
                 `unsafe` block",
            );
            return error_expr(span);
        }
        let sig = self.globals.sigs[f.0].clone();
        if sig.params.iter().any(|p| p.comptime) {
            self.diags.error(
                Code::INVALID_COMPTIME,
                span,
                "a function with a `comptime` parameter cannot be used as a value: each call is specialized for its                  compile-time arguments (spec/comptime.md, CT4)",
            );
            return error_expr(span);
        }
        if sig.ret.is_second_class() {
            self.diags.error(
                Code::CLOSURE_ESCAPES,
                span,
                "a function that returns a view cannot be used as a closure value; the result would point into \
                 the closure's own argument",
            );
            return error_expr(span);
        }
        let fn_sig =
            FnSig { params: sig.params.iter().map(|p| p.ty.clone()).collect(), ret: Box::new(sig.ret.clone()) };
        // A parameter the callee borrows exclusively (`mut x`, `mut self`) needs a mutable argument local.
        let mutable: Vec<bool> = sig.params.iter().map(|p| p.mut_param || (p.by_ref && p.exclusive)).collect();
        let ret = sig.ret.clone();
        self.value_closure(fn_sig, &mutable, span, |args| mk(tir::ExprKind::Call(f, args), ret, span))
    }

    /// A built-in function used as a value (`println`, `panic`, the runtime helpers). Its signature is fixed.
    pub(super) fn builtin_value(&mut self, b: Builtin, span: Span) -> tir::Expr {
        let sig = builtin_sig(b, self.globals);
        let fn_sig =
            FnSig { params: sig.params.iter().map(|p| p.ty.clone()).collect(), ret: Box::new(sig.ret.clone()) };
        let mutable: Vec<bool> = sig.params.iter().map(|p| p.mut_param).collect();
        let ret = sig.ret.clone();
        self.value_closure(fn_sig, &mutable, span, |args| mk(tir::ExprKind::Builtin(b, args), ret, span))
    }

    /// `Some`, `Ok`, or `Err` used as a value (CL11). A constructor has no signature of its own, so the expected
    /// closure type fixes its type arguments. The closure's one parameter is the payload; the expected result gives
    /// the argument the payload does not fix (`Err`'s `T`, `Ok`'s `E`), and `Some`'s result is built from the payload
    /// alone, so an open result (`map`'s `U`) is bound by the call. An argument the expected type leaves open is E0003.
    pub(super) fn constructor_value(&mut self, ctor: Ctor, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let name = ctor.name();
        let sig = match expect {
            Some(Ty::Fn { sig, .. }) => sig.clone(),
            Some(Ty::Error) => return error_expr(span),
            _ => return self.constructor_untyped(ctor, span),
        };
        let [payload] = sig.params.as_slice() else {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{name}` takes 1 argument, but the closure type has {} parameters", sig.params.len()),
            );
            return error_expr(span);
        };
        let payload = payload.clone();
        let ret = (*sig.ret).clone();
        if ret == Ty::Error || payload == Ty::Error {
            return error_expr(span);
        }
        if self.is_open(&payload) {
            return self.constructor_untyped(ctor, span);
        }
        let result = match ctor {
            Ctor::Some => Ty::Option(Box::new(payload.clone())),
            Ctor::Ok | Ctor::Err => {
                let Ty::Result(t, e) = &ret else {
                    if self.is_open(&ret) {
                        return self.constructor_untyped(ctor, span);
                    }
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("`{name}` used as a value must return a `Result`, but the closure type returns {ret}"),
                    );
                    return error_expr(span);
                };
                // `Ok` fixes `T` from the payload and takes `E` from the result; `Err` the reverse.
                let other = if ctor == Ctor::Ok { e } else { t };
                if other.as_ref() == &Ty::Error {
                    return error_expr(span);
                }
                if self.is_open(other) {
                    return self.constructor_untyped(ctor, span);
                }
                if ctor == Ctor::Ok {
                    Ty::Result(Box::new(payload.clone()), e.clone())
                } else {
                    Ty::Result(t.clone(), Box::new(payload.clone()))
                }
            }
        };
        let fn_sig = FnSig { params: vec![payload], ret: Box::new(result.clone()) };
        self.value_closure(fn_sig, &[false], span, |args| {
            let arg = args.into_iter().next().expect("a constructor takes one argument");
            let kind = match ctor {
                Ctor::Some => tir::ExprKind::Some(Box::new(arg)),
                Ctor::Ok => tir::ExprKind::Ok(Box::new(arg)),
                Ctor::Err => tir::ExprKind::Err(Box::new(arg)),
            };
            mk(kind, result, span)
        })
    }

    /// The error for a constructor used as a value where nothing says what its type arguments are.
    fn constructor_untyped(&mut self, ctor: Ctor, span: Span) -> tir::Expr {
        let (name, example) = match ctor {
            Ctor::Some => ("Some", "let f: Fn(i32): Option<i32> = Some"),
            Ctor::Ok => ("Ok", "let f: Fn(i32): Result<i32, String> = Ok"),
            Ctor::Err => ("Err", "let f: Fn(String): Result<i32, String> = Err"),
        };
        self.diags.error(
            Code::TYPE_MISMATCH,
            span,
            format!("cannot tell the type of `{name}` used as a value; write the closure type, as in `{example}`"),
        );
        error_expr(span)
    }

    /// The user method or associated function `name` of the struct or enum `ty`, if there is one. The member-access
    /// rules (`priv`) apply to a method used as a value as they do to a call.
    pub(super) fn user_method(&mut self, ty: &Ty, name: &ast::Ident) -> Option<FuncId> {
        let key = adt_key(ty)?;
        let f = *self.globals.methods.get(&(key, name.name.clone()))?;
        self.check_member_access(ty, key, name);
        Some(f)
    }

    /// The type of a method value's receiver when it is a name: a local of this function, an enclosing function's
    /// local seen from a closure body, or `self`. Any other receiver expression has no name to bind a method value to,
    /// so it gives `None`.
    pub(super) fn local_receiver_ty(&self, base: &ast::Expr) -> Option<Ty> {
        match &base.kind {
            ast::ExprKind::Name { name, args } if args.is_empty() => match self.lookup(&name.name) {
                Some(local) => Some(self.locals[local.0].ty.clone()),
                None => self.outer_local_ty(&name.name),
            },
            ast::ExprKind::SelfValue => match self.self_local {
                Some(local) => Some(self.locals[local.0].ty.clone()),
                None => self.outer_local_ty("self"),
            },
            _ => None,
        }
    }

    /// Whether the struct `ty` has a field named `name`. A field of that name wins over a method of the same name.
    pub(super) fn has_field(&self, ty: &Ty, name: &ast::Ident) -> bool {
        matches!(ty, Ty::Struct(a) if self.globals.structs[a.id].fields.iter().any(|f| f.name == name.name))
    }

    /// `Point.name` read without a call, where `name` is a user method or associated function with no type
    /// parameters of its own: a function value whose parameters are the declared ones (CL11).
    pub(super) fn method_function_value(&mut self, f: FuncId, name: &ast::Ident, span: Span) -> tir::Expr {
        if self.globals.method_templates.contains_key(&f) {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!(
                    "`{}` has type parameters of its own, so it cannot be a value; write a closure that calls it",
                    name.name
                ),
            );
            return error_expr(span);
        }
        self.function_value(f, span)
    }

    /// `recv.name` read without a call, where `recv` is a local and `name` is one of its methods: the closure
    /// `|args| recv.name(args)`, typed by the method's declared parameters (CL11). The closure borrows the receiver
    /// the way any closure does (CL2): an `own self` method moves it, and so does an owned closure type expected by
    /// the context. The closure is checked as written, so its captures and its kind come from its body.
    pub(super) fn bound_method_value(
        &mut self,
        recv: &ast::Expr,
        name: &ast::Ident,
        f: FuncId,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        if self.globals.method_templates.contains_key(&f) {
            return self.method_function_value(f, name, span);
        }
        let sig = self.globals.sigs[f.0].clone();
        if !sig.has_self {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("`{}` has no `self` parameter, so it is not a method value; write a closure", name.name),
            );
            return error_expr(span);
        }
        let owned = sig.params[0].own || matches!(expect, Some(Ty::Fn { borrowed: false, .. }));
        let rest = &sig.params[1..];
        // The generated parameters and arguments have no source text, so they take an empty span at the start of the
        // value: a cursor on the value then finds the method call, not one of these names.
        let empty = Span::new(span.file, span.lo, span.lo);
        let params: Vec<ast::ClosureParam> = (0..rest.len())
            .map(|i| ast::ClosureParam { name: ast::Ident { name: format!("arg{i}"), span: empty }, ty: None })
            .collect();
        let args: Vec<ast::Arg> = params
            .iter()
            .map(|p| ast::Arg {
                name: None,
                mut_marker: false,
                mut_span: None,
                expr: ast::Expr { kind: ast::ExprKind::Name { name: p.name.clone(), args: Vec::new() }, span: empty },
            })
            .collect();
        let callee = ast::Expr { kind: ast::ExprKind::Field(Box::new(recv.clone()), name.clone()), span };
        let body = ast::Expr { kind: ast::ExprKind::Call(Box::new(callee), args), span };
        let expected = Ty::Fn {
            sig: Rc::new(FnSig { params: rest.iter().map(|p| p.ty.clone()).collect(), ret: Box::new(sig.ret.clone()) }),
            kind: tir::FnKind::FnOnce,
            borrowed: !owned,
            send: tir::FnSend::unknown(),
        };
        self.closure(&params, None, &body, Some(&expected), span)
    }

    /// A type parameter the call has not bound yet (`U` in `fn map<U, F: Fn(T): U>`), as opposed to one of this
    /// function's own type parameters, which are in scope with their bounds.
    fn is_open(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Param(name) if !self.bounds.iter().any(|(p, _)| p.as_str() == name.as_ref()))
    }

    /// Builds a captureless closure value whose body `build` receives its parameters as locals, in order.
    fn value_closure(
        &mut self,
        fn_sig: FnSig,
        mutable: &[bool],
        span: Span,
        build: impl FnOnce(Vec<tir::Expr>) -> tir::Expr,
    ) -> tir::Expr {
        let index = self.closures.borrow().len();
        let mut locals = Vec::new();
        let mut params = Vec::new();
        let mut args = Vec::new();
        for (i, ty) in fn_sig.params.iter().enumerate() {
            let id = tir::LocalId(locals.len());
            locals.push(tir::Local {
                name: format!("arg{i}"),
                ty: ty.clone(),
                mutable: mutable.get(i).copied().unwrap_or(false),
                by_ref: false,
                kind: LocalKind::OwnParam,
                span: Span::new(span.file, span.lo, span.lo),
            });
            params.push(id);
            args.push(mk(tir::ExprKind::Local(id), ty.clone(), span));
        }
        let body = build(args);
        let ret = (*fn_sig.ret).clone();
        let func = tir::Function {
            name: format!("closure.{index}"),
            span,
            params,
            ret,
            locals,
            body: tir::Block { stmts: Vec::new(), tail: Some(Box::new(body)), close: span },
            moved: Vec::new(),
            debug_kills: Vec::new(),
            ret_origins: Vec::new(),
            symbolic: false,
            exported: false,
            events: Vec::new(),
        };
        let closure = tir::Closure {
            name: func.name.clone(),
            span,
            kind: tir::FnKind::Fn,
            sig: fn_sig.clone(),
            captures: Vec::new(),
            func,
        };
        self.closures.borrow_mut().push(closure);
        let ty = Ty::Fn { sig: Rc::new(fn_sig), kind: tir::FnKind::Fn, borrowed: false, send: tir::FnSend::yes() };
        mk(tir::ExprKind::Closure { closure: index, captures: Vec::new() }, ty, span)
    }
}
