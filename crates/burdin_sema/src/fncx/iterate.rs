//! The iteration protocol: the compiler-known methods of the built-in iterators, the `Iterator<T>` default
//! methods of built-in receivers, `sum`, the eager `List` helpers, and the lowering of `for` over a user type or
//! an iterator value (`spec/iterators.md`).

use super::{FnCx, error_expr, mk};
use crate::generics::{Bound, Request, SYMBOLIC_CALL};
use crate::globals::adt_key;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{is_place_expr, place_root};
use burdin_tir::{self as tir, BinOp, LocalId, LocalKind, Method, Pat, Ty};

impl FnCx<'_> {
    /// The protocol's methods on a receiver that is not an ordinary method of a user type: `next` on a built-in
    /// iterator, `sum`, the eager `List` helpers, and the `Iterator<T>` default methods of any iterator. `None`
    /// when none of them applies, so the ordinary built-in method table runs next.
    pub(super) fn iterator_method(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let recv_ty = recv.ty.clone();
        let builtin_item = self.globals.builtin_iterator_item(&recv_ty, &self.bounds);
        // `.any(...)` is the adapter's spelling; `any` is reserved as an identifier, so `std` declares the
        // method as `any_item` (spec/iterators.md, IT9).
        let lookup = if name.name == "any" { "any_item" } else { name.name.as_str() };
        if name.name == "next" && args.is_empty() {
            let item = builtin_item?;
            if is_place_expr(&recv) && !self.place_mutable(&recv) {
                let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`next` changes its receiver, but `{whose}` is not mutable"),
                );
            }
            return Some(mk(tir::ExprKind::Method(Method::IterNext, vec![recv]), Ty::Option(Box::new(item)), span));
        }
        if name.name == "sum"
            && args.is_empty()
            && let Some(item) = self.globals.iterator_item(&recv.ty, &self.bounds)
        {
            return Some(self.iterator_sum(recv, &item, span));
        }
        // The eager `List` methods are generic functions in `std`; the receiver is their first argument.
        if let Ty::List(_, _) = &recv_ty
            && let Some(g) = self.globals.eager_list_fn(&name.name)
        {
            return Some(self.generic_call_with_receiver(g, recv, &name.name, args, span));
        }
        // An iterator has no impl block for the interface's defaults, whether it is built in or a user type, so
        // the template is instantiated on demand. Registering the defaults on every instance instead would
        // instantiate each adapter's own adapters without end (`take(1)` returns a `TakeIter` whose `take`
        // returns another, and so on).
        let item = self.globals.iterator_item(&recv_ty, &self.bounds)?;
        let iface = self.globals.iterator_iface?;
        let method = self.globals.interfaces[iface].methods.iter().position(|m| m.name == lookup)?;
        if !self.globals.interfaces[iface].methods[method].has_default {
            return None;
        }
        let key = (iface, method, recv_ty.clone(), vec![item.clone()]);
        if let Some(info) = self.globals.builtin_method_templates.get(&key).cloned() {
            return Some(self.generic_method_call(info, recv, &name.name, args, span));
        }
        // The template is created on the next pass; still check the arguments so their own errors show now.
        for a in args {
            self.expr(&a.expr, None);
        }
        self.globals.requests.borrow_mut().push(Request::BuiltinMethodTemplate {
            iface,
            method,
            self_ty: recv_ty,
            iface_args: vec![item],
        });
        Some(error_expr(span))
    }

    /// `it.sum()`: the sum of an iterator of integers, written as an ordinary loop by the compiler because
    /// stage 1 has no arithmetic bound to express it in `std`.
    fn iterator_sum(&mut self, recv: tir::Expr, item: &Ty, span: Span) -> tir::Expr {
        let int = match item {
            Ty::Int(i) => Ty::Int(*i),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Int(_)) => (**inner).clone(),
            _ => {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("`sum` needs an iterator of integers, but this one yields `{item}`"),
                );
                return error_expr(span);
            }
        };
        let iter_ty = recv.ty.clone();
        let it = self.declare_hidden("__iter", iter_ty.clone(), true, LocalKind::Owned);
        let sum = self.declare_hidden("__sum", int.clone(), true, LocalKind::Owned);
        let x = self.declare_hidden("__item", item.clone(), false, LocalKind::Owned);
        let mode = self.bind_mode(item, true);
        let next = self.iterator_next_call(mk(tir::ExprKind::Local(it), iter_ty.clone(), span), item, span);
        let value = if item.is_ref() {
            mk(tir::ExprKind::DerefRef(Box::new(mk(tir::ExprKind::Local(x), item.clone(), span))), int.clone(), span)
        } else {
            mk(tir::ExprKind::Local(x), item.clone(), span)
        };
        let added = mk(
            tir::ExprKind::Binary(
                BinOp::Add,
                Box::new(mk(tir::ExprKind::Local(sum), int.clone(), span)),
                Box::new(value),
            ),
            int.clone(),
            span,
        );
        let assign = mk(
            tir::ExprKind::Assign(Box::new(mk(tir::ExprKind::Local(sum), int.clone(), span)), Box::new(added)),
            Ty::Unit,
            span,
        );
        let body =
            mk(tir::ExprKind::Block(tir::Block { stmts: vec![tir::Stmt::Expr(assign)], tail: None }), Ty::Unit, span);
        let arms = vec![
            tir::Arm { pat: Pat::Some(Box::new(Pat::Binding(x, mode))), guard: None, body },
            tir::Arm { pat: Pat::None, guard: None, body: mk(tir::ExprKind::Break, Ty::Never, span) },
        ];
        let matched = mk(tir::ExprKind::Match(Box::new(next), arms), Ty::Unit, span);
        let looped =
            mk(tir::ExprKind::Loop(tir::Block { stmts: Vec::new(), tail: Some(Box::new(matched)) }), Ty::Unit, span);
        let init = tir::Stmt::Let(Pat::Binding(it, self.bind_mode(&iter_ty, true)), recv, None);
        let zero_mode = self.bind_mode(&int, true);
        let zero = tir::Stmt::Let(Pat::Binding(sum, zero_mode), mk(tir::ExprKind::Int(0), int.clone(), span), None);
        let block = tir::Block {
            stmts: vec![init, zero, tir::Stmt::Expr(looped)],
            tail: Some(Box::new(mk(tir::ExprKind::Local(sum), int.clone(), span))),
        };
        mk(tir::ExprKind::Block(block), int, span)
    }

    /// A compiler-made local (no user name can collide with `__iter`).
    pub(super) fn declare_hidden(&mut self, name: &str, ty: Ty, mutable: bool, kind: LocalKind) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable, by_ref: false, kind });
        id
    }

    /// `it.next()` for an iterator value: the built-in method, the user's `next`, or a symbolic call when the
    /// receiver is a type parameter.
    pub(super) fn iterator_next_call(&mut self, it: tir::Expr, item: &Ty, span: Span) -> tir::Expr {
        let ret = Ty::Option(Box::new(item.clone()));
        if self.globals.builtin_iterator_item(&it.ty, &self.bounds).is_some() && adt_key(&it.ty).is_none() {
            return mk(tir::ExprKind::Method(Method::IterNext, vec![it]), ret, span);
        }
        if let Ty::Param(_) = &it.ty {
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![it]), ret, span);
        }
        match adt_key(&it.ty).and_then(|key| self.globals.methods.get(&(key, "next".to_string())).copied()) {
            Some(f) => mk(tir::ExprKind::Call(f, vec![it]), ret, span),
            None => {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("`{}` does not provide the `next` method of `Iterator`", it.ty),
                );
                error_expr(span)
            }
        }
    }

    /// `for pattern in iterable` through the protocol: bind the iterator to a mutable local and loop over
    /// `next()`. The body keeps the stage 0 loop semantics: its bindings drop each iteration, and `break` and
    /// `continue` target the loop.
    pub(super) fn protocol_for(
        &mut self,
        pattern: &ast::Pattern,
        iterable: tir::Expr,
        item: Ty,
        body: &ast::Block,
        span: Span,
    ) -> tir::Expr {
        let iter_ty = iterable.ty.clone();
        let it = self.declare_hidden("__iter", iter_ty.clone(), true, LocalKind::Owned);
        self.scopes.push(Default::default());
        self.loops.push(false);
        let pat = self.pattern(pattern, &item, true);
        if !self.exhaustive(&item, &[&pat]) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                pattern.span,
                format!("this pattern does not match every value of `{item}`"),
            );
        }
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.scopes.pop();
        let next = self.iterator_next_call(mk(tir::ExprKind::Local(it), iter_ty.clone(), span), &item, span);
        let arms = vec![
            tir::Arm {
                pat: Pat::Some(Box::new(pat)),
                guard: None,
                body: mk(tir::ExprKind::Block(body), Ty::Unit, span),
            },
            tir::Arm { pat: Pat::None, guard: None, body: mk(tir::ExprKind::Break, Ty::Never, span) },
        ];
        let matched = mk(tir::ExprKind::Match(Box::new(next), arms), Ty::Unit, span);
        let looped =
            mk(tir::ExprKind::Loop(tir::Block { stmts: Vec::new(), tail: Some(Box::new(matched)) }), Ty::Unit, span);
        let init = tir::Stmt::Let(Pat::Binding(it, self.bind_mode(&iter_ty, true)), iterable, None);
        mk(tir::ExprKind::Block(tir::Block { stmts: vec![init], tail: Some(Box::new(looped)) }), Ty::Unit, span)
    }

    /// `for pattern in mut e`: the loop calls `e.iter_mut()` and walks the iterator it returns, whose items are
    /// writable views (IT13). The collection is borrowed exclusively until the loop ends. The method is a
    /// compiler-known one for `List`, arrays, and `Map`, and an ordinary `iter_mut(mut self)` method for a user
    /// type; either way the result must be an `Iterator`.
    pub(super) fn for_mut(
        &mut self,
        pattern: &ast::Pattern,
        iter: &ast::Expr,
        body: &ast::Block,
        span: Span,
    ) -> tir::Expr {
        let name = ast::Ident { name: "iter_mut".to_string(), span: iter.span };
        let iterator = self.method_call(iter, &name, &[], span, None);
        if iterator.ty == Ty::Error {
            // The iterable did not resolve; the body is still checked for its own errors.
            self.scopes.push(Default::default());
            self.loops.push(false);
            let _ = self.pattern(pattern, &Ty::Error, true);
            let _ = self.block(body, None);
            self.loops.pop();
            self.scopes.pop();
            return error_expr(span);
        }
        let Some(item) = self.globals.iterator_item(&iterator.ty, &self.bounds) else {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`iter_mut()` returns `{}`, which is not an `Iterator`", iterator.ty),
            );
            return error_expr(span);
        };
        self.protocol_for(pattern, iterator, item, body, span)
    }

    /// The iterator a `for` over an `Iterable` starts: `value.iter()`, or a symbolic iterator for a type
    /// parameter with the bound.
    pub(super) fn for_iterator(&mut self, iter: &ast::Expr, iterable: tir::Expr, item: &Ty, span: Span) -> tir::Expr {
        let param_name = match &iterable.ty {
            Ty::Param(p) => Some(p.to_string()),
            _ => None,
        };
        if let Some(pname) = param_name {
            // Inside generic code the iterator's type cannot be named; an opaque type parameter carrying the
            // `Iterator<T>` bound stands for it. The symbolic check discards the code.
            let Some(iface) = self.globals.iterator_iface else {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "`for` over a type parameter needs the standard library's `Iterator` interface",
                );
                return error_expr(span);
            };
            let pseudo = format!("{pname}::iterator");
            let ty = Ty::Param(pseudo.as_str().into());
            self.bounds.push((pseudo, vec![Bound::User(iface, vec![item.clone()])]));
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![iterable]), ty, span);
        }
        if let Some(key) = adt_key(&iterable.ty)
            && !self.globals.methods.contains_key(&(key, "iter".to_string()))
        {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`{}` implements `Iterable` but has no `iter` method", iterable.ty),
            );
            return error_expr(span);
        }
        let name = ast::Ident { name: "iter".to_string(), span };
        self.method_call(iter, &name, &[], span, None)
    }
}
