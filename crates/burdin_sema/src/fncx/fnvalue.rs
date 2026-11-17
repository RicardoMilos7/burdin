//! Function values: a named function, a built-in function, or `Some` used where a closure value is expected.
//!
//! Each one becomes an ordinary closure with no captures whose body forwards to the function, so the call,
//! storage, and drop rules are the closure rules (CL11 in `spec/memory-rules.md`).

use super::{FnCx, error_expr, mk};
use crate::builtins::builtin_sig;
use burdin_diag::{Code, Span};
use burdin_tir::{self as tir, Builtin, FnSig, FuncId, LocalKind, Ty};
use std::rc::Rc;

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
        if sig.has_self {
            self.diags.error(
                Code::UNKNOWN_NAME,
                span,
                "a method cannot be used as a value; write a closure that calls it instead",
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
        let mutable: Vec<bool> = sig.params.iter().map(|p| p.mut_param).collect();
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

    /// `Some` used as a value. A constructor has no signature of its own, so the expected closure type fixes the
    /// element: its one parameter and the `Option` result must name the same type.
    pub(super) fn some_value(&mut self, expect: Option<&Ty>, span: Span) -> tir::Expr {
        let elem = match expect {
            Some(Ty::Fn { sig, .. })
                if sig.params.len() == 1 && matches!(&*sig.ret, Ty::Option(t) if **t == sig.params[0]) =>
            {
                sig.params[0].clone()
            }
            _ => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "cannot tell the type of `Some` used as a value; write the closure type, as in \
                     `let f: Fn(i32): Option<i32> = Some`",
                );
                return error_expr(span);
            }
        };
        let fn_sig = FnSig { params: vec![elem.clone()], ret: Box::new(Ty::Option(Box::new(elem.clone()))) };
        self.value_closure(fn_sig, &[false], span, |args| {
            let arg = args.into_iter().next().expect("`Some` takes one argument");
            mk(tir::ExprKind::Some(Box::new(arg)), Ty::Option(Box::new(elem)), span)
        })
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
            body: tir::Block { stmts: Vec::new(), tail: Some(Box::new(body)) },
            moved: Vec::new(),
            ret_origins: Vec::new(),
            symbolic: false,
            exported: false,
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
