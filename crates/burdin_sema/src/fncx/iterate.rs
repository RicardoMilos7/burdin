//! The iteration protocol: the compiler-known methods of the built-in iterators, the `Iterator<T>` default
//! methods of built-in receivers, `sum`, the eager `List` helpers, and the lowering of `for` over a user type or
//! an iterator value (`spec/iterators.md`).

use super::{FnCx, LoopScope, error_expr, mk};
use crate::generics::{Bound, Request, SYMBOLIC_CALL};
use crate::globals::adt_key;
use crate::types::check_storable;
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
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> Option<tir::Expr> {
        let recv_ty = recv.ty.clone();
        // A method of the receiver type's own table wins over an `Iterator` default of the same name
        // (`spec/extensions.md`, EX3): `[T].last()` and `[T].to_list()` are the slice's own methods.
        if crate::builtins::method(self.globals, &name.name, &recv_ty).is_some() {
            return None;
        }
        let builtin_item = self.globals.builtin_iterator_item(&recv_ty, &self.bounds);
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
            return Some(mk(tir::ExprKind::Method(Method::IterNext, vec![recv]), self.globals.option_ty(item), span));
        }
        if name.name == "next_back" && args.is_empty() {
            let item = self.globals.builtin_double_ended_item(&recv_ty, &self.bounds)?;
            if is_place_expr(&recv) && !self.place_mutable(&recv) {
                let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`next_back` changes its receiver, but `{whose}` is not mutable"),
                );
            }
            return Some(mk(
                tir::ExprKind::Method(Method::IterNextBack, vec![recv]),
                self.globals.option_ty(item),
                span,
            ));
        }
        if (name.name == "sum" || name.name == "product")
            && args.is_empty()
            && let Some(item) = self.globals.iterator_item(&recv.ty, &self.bounds)
        {
            return Some(self.iterator_sum(recv, &item, name.name == "product", span));
        }
        // The eager `List` methods are generic functions in `std`; the receiver is their first argument.
        if self.globals.list_parts(&recv_ty).is_some()
            && let Some(g) = self.globals.eager_list_fn(&name.name)
        {
            return Some(self.generic_call_with_receiver(g, recv, &name.name, args, span));
        }
        // `min`, `max`, `eq`, and `cmp` need `T: Ord` (or `Eq`) on the interface's own item type, so they are
        // functions in `std` that take the iterator; the compiler hands them the item type.
        if let Some(item) = self.globals.iterator_item(&recv_ty, &self.bounds)
            && let Some(g) = self.globals.iterator_item_fn(&name.name)
        {
            let seed = [("T".to_string(), item)];
            return Some(self.generic_call_with_receiver_seeded(g, recv, &name.name, args, &seed, span));
        }
        // The adapters that need items that are iterators or views, or a clonable source, are generic functions in
        // `std` that take the iterator as their first argument (IT13).
        if args.is_empty()
            && self.globals.iterator_item(&recv_ty, &self.bounds).is_some()
            && let Some(g) = self.globals.iterator_adapter_fn(&name.name)
        {
            return Some(self.generic_call_with_receiver(g, recv, &name.name, args, span));
        }
        // An iterator has no impl block for the interface's defaults, whether it is built in or a user type, so
        // the template is instantiated on demand. Registering the defaults on every instance instead would
        // instantiate each adapter's own adapters without end (`take(1)` returns a `TakeIter` whose `take`
        // returns another, and so on).
        // A parallel-pipeline default is on demand for the same reason (spec/parallel.md, PR13): the interface's
        // default of that name, for this receiver and its item type.
        if let Some((iface, method, item)) = self.parallel_default(&recv_ty, &name.name) {
            let key = (iface, method, recv_ty.clone(), vec![item.clone()]);
            if let Some(info) = self.globals.builtin_method_templates.get(&key).cloned() {
                return Some(self.generic_method_call(info, recv, &name.name, explicit, args, span, expect));
            }
            for a in args {
                self.expr(&a.expr, None);
            }
            self.globals.requests.borrow_mut().push(Request::BuiltinMethodTemplate {
                iface,
                method,
                self_ty: recv_ty,
                iface_args: vec![item],
            });
            return Some(error_expr(span));
        }
        let item = self.globals.iterator_item(&recv_ty, &self.bounds)?;
        let iface = self.globals.iterator_iface?;
        let method = self.globals.interfaces[iface].methods.iter().position(|m| m.name == name.name)?;
        if !self.globals.interfaces[iface].methods[method].has_default {
            return None;
        }
        let key = (iface, method, recv_ty.clone(), vec![item.clone()]);
        if let Some(info) = self.globals.builtin_method_templates.get(&key).cloned() {
            return Some(self.generic_method_call(info, recv, &name.name, explicit, args, span, expect));
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

    /// The parallel-pipeline interface that declares a default method `name` for the concrete pipeline value `ty`,
    /// with the method's index and the item type the value's impl names (spec/parallel.md, PR13). A type parameter is
    /// not one: a call on it goes through its bound. `None` when no such default applies.
    fn parallel_default(&self, ty: &Ty, name: &str) -> Option<(usize, usize, Ty)> {
        if matches!(ty, Ty::Param(_)) {
            return None;
        }
        self.globals.parallel_ifaces.iter().find_map(|&iface| {
            let method = self.globals.interfaces[iface].methods.iter().position(|m| m.name == name)?;
            if !self.globals.interfaces[iface].methods[method].has_default {
                return None;
            }
            let item = self.globals.declared_item(ty, Some(iface), &self.bounds)?;
            Some((iface, method, item))
        })
    }

    /// Whether `name` on a value of type `ty` (through references) is the `collect` of an iterator, which takes its
    /// target as an explicit type argument (`spec/iterators.md`, IT17). The value must be an iterator the compiler
    /// knows, and no method of its own may have that name, so the call reaches the `Iterator<T>` default.
    pub(super) fn iterator_collect(&self, ty: &Ty, name: &str) -> bool {
        if name != "collect" {
            return false;
        }
        let mut base = ty;
        while let Ty::Ref { inner, .. } = base {
            base = inner;
        }
        if matches!(base, Ty::Any(_) | Ty::Error) || crate::builtins::method(self.globals, name, base).is_some() {
            return false;
        }
        if adt_key(base).is_some_and(|key| self.globals.methods.contains_key(&(key, name.to_string()))) {
            return false;
        }
        self.globals.iterator_item(base, &self.bounds).is_some()
    }

    /// `from_iter` on a built-in collection (`List`, `Map`, `String`, `Option`, `Result`) and `from` on a `List` or a
    /// `Map` (`spec/iterators.md`, IT18): a call to the `std` function that builds it. `None` when `ty` and `name`
    /// are not one of those, so the ordinary associated-function lookup runs next. The target is `ty`, so the
    /// function's type arguments come from it and from the argument.
    pub(super) fn builtin_collection_call(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<tir::Expr> {
        let from_array = name.name == "from";
        if name.name != "from_iter" && !from_array {
            return None;
        }
        let std_name = match (ty, from_array) {
            (t, false) if self.globals.list_parts(t).is_some() => "list_from_iter",
            (t, true) if self.globals.list_parts(t).is_some() => "list_from",
            (t, false) if self.globals.map_parts(t).is_some() => "map_from_iter",
            (t, true) if self.globals.map_parts(t).is_some() => "map_from",
            (t, false) if self.globals.option_payload(t).is_some() => "option_from_iter",
            (t, false) if self.globals.result_parts(t).is_some() => "result_from_iter",
            (Ty::String, false) => "",
            _ => return None,
        };
        let stored = match (self.globals.list_parts(ty), self.globals.map_parts(ty)) {
            (Some((_, alloc)), _) => Some(alloc),
            (None, Some((_, _, alloc))) => Some(alloc),
            _ => None,
        };
        if let Some(alloc) = stored
            && !alloc.is_global()
        {
            self.diags.error(
                Code::MISSING_ALLOCATOR,
                span,
                format!(
                    "`{}` builds a `{ty}` with the default allocator, which this type does not use; make it with `new_in` and add the items with `extend_iter`",
                    name.name
                ),
            );
            return Some(self.args_then_error(args, span));
        }
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{}` takes 1 argument, but {} were given", name.name, args.len()),
            );
            return Some(self.args_then_error(args, span));
        }
        if let Some((key, value, _)) = self.globals.map_parts(ty) {
            check_storable(&key, span, "a `Map` key", self.diags);
            check_storable(&value, span, "a `Map` value", self.diags);
        }
        let hint = if from_array { self.array_hint(ty) } else { None };
        let arg = self.expr(&args[0].expr, hint.as_ref());
        Some(self.collection_from(ty, std_name, name, args, arg, span))
    }

    /// The element type hint for the array `List<T>.from` and `Map<K, V>.from` take: an array of `T`, or of `(K, V)`.
    fn array_hint(&self, ty: &Ty) -> Option<Ty> {
        if let Some((elem, _)) = self.globals.list_parts(ty) {
            return Some(Ty::Array(Box::new(elem), 0));
        }
        self.globals.map_parts(ty).map(|(key, value, _)| Ty::Array(Box::new(Ty::Tuple(vec![key, value])), 0))
    }

    /// `List.from(array)` and `Map.from(array)` written without type arguments: the types come from the array and
    /// from the expected type.
    pub(super) fn untyped_collection_from(
        &mut self,
        collection: &str,
        name: &ast::Ident,
        args: &[ast::Arg],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        if args.len() != 1 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{collection}.from` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let hint = expect.and_then(|ty| self.array_hint(ty));
        let arg = self.expr(&args[0].expr, hint.as_ref());
        if collection == "Map"
            && let Ty::Array(pair, _) = &arg.ty
            && let Ty::Tuple(parts) = &**pair
            && let [key, value] = parts.as_slice()
        {
            check_storable(key, span, "a `Map` key", self.diags);
            check_storable(value, span, "a `Map` value", self.diags);
        }
        let std_name = if collection == "List" { "list_from" } else { "map_from" };
        let ty = expect.filter(|ty| {
            (collection == "List" && self.globals.list_parts(ty).is_some())
                || (collection == "Map" && self.globals.map_parts(ty).is_some())
        });
        let Some(g) = self.globals.std_generic(std_name) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`{collection}.from` needs the standard library"));
            return error_expr(span);
        };
        self.generic_call_checked(g, name, args, vec![arg], span, ty)
    }

    /// The call of the `std` function `std_name` with the checked argument `arg`; a `String` picks the function by
    /// the item type of the iterator.
    fn collection_from(
        &mut self,
        ty: &Ty,
        std_name: &str,
        name: &ast::Ident,
        args: &[ast::Arg],
        arg: tir::Expr,
        span: Span,
    ) -> tir::Expr {
        let std_name = if *ty == Ty::String {
            let Some(item) = self.globals.iterator_item(&arg.ty, &self.bounds) else {
                if arg.ty != Ty::Error {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!("`{}` needs an iterator, but this is a `{}`", name.name, arg.ty),
                    );
                }
                return error_expr(span);
            };
            match &item {
                Ty::Char => "string_from_chars",
                Ty::Str => "string_from_strs",
                Ty::String => "string_from_strings",
                Ty::Ref { inner, .. } if **inner == Ty::String => "string_from_string_views",
                _ => {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!("a `String` is built from `char`, `str`, or `String` items, but this iterator yields `{item}`"),
                    );
                    return error_expr(span);
                }
            }
        } else {
            std_name
        };
        let Some(g) = self.globals.std_generic(std_name) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`{}` needs the standard library", name.name));
            return error_expr(span);
        };
        self.generic_call_checked(g, name, args, vec![arg], span, Some(ty))
    }

    /// `it.sum()` and `it.product()`: the sum or product of an iterator of integers or floats, written as an
    /// ordinary loop by the compiler because stage 1 has no arithmetic bound to express it in `std`. The loop uses
    /// the ordinary `+` or `*`, so an integer overflow behaves as it does there (spec/iterators.md, IT9).
    fn iterator_sum(&mut self, recv: tir::Expr, item: &Ty, product: bool, span: Span) -> tir::Expr {
        let method = if product { "product" } else { "sum" };
        let int = match item {
            Ty::Int(_) | Ty::Float(_) => item.clone(),
            Ty::Ref { inner, .. } if matches!(**inner, Ty::Int(_) | Ty::Float(_)) => (**inner).clone(),
            _ => {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("`{method}` needs an iterator of integers or floats, but this one yields `{item}`"),
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
                if product { BinOp::Mul } else { BinOp::Add },
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
        let body = mk(
            tir::ExprKind::Block(tir::Block { stmts: vec![tir::Stmt::Expr(assign)], tail: None, close: span }),
            Ty::Unit,
            span,
        );
        let arms = vec![
            tir::Arm { pat: Pat::Variant(1, vec![Pat::Binding(x, mode)]), guard: None, body },
            tir::Arm {
                pat: Pat::Variant(0, Vec::new()),
                guard: None,
                body: mk(tir::ExprKind::Break { label: None, value: None }, Ty::Never, span),
            },
        ];
        let matched = mk(tir::ExprKind::Match(Box::new(next), arms), Ty::Unit, span);
        let looped = mk(
            tir::ExprKind::Loop(tir::Block { stmts: Vec::new(), tail: Some(Box::new(matched)), close: span }),
            Ty::Unit,
            span,
        );
        let init = tir::Stmt::Let(Pat::Binding(it, self.bind_mode(&iter_ty, true)), recv, None);
        let zero_mode = self.bind_mode(&int, true);
        let start = match (&int, product) {
            (Ty::Float(_), _) => tir::ExprKind::Float(if product { 1.0 } else { 0.0 }),
            (_, _) => tir::ExprKind::Int(i128::from(product)),
        };
        let zero = tir::Stmt::Let(Pat::Binding(sum, zero_mode), mk(start, int.clone(), span), None);
        let block = tir::Block {
            stmts: vec![init, zero, tir::Stmt::Expr(looped)],
            tail: Some(Box::new(mk(tir::ExprKind::Local(sum), int.clone(), span))),
            close: span,
        };
        mk(tir::ExprKind::Block(block), int, span)
    }

    /// A compiler-made local (no user name can collide with `__iter`).
    pub(super) fn declare_hidden(&mut self, name: &str, ty: Ty, mutable: bool, kind: LocalKind) -> LocalId {
        let id = LocalId(self.locals.len());
        let span = Span::new(burdin_diag::FileId(0), 0, 0);
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable, by_ref: false, kind, span });
        id
    }

    /// `it.next()` for an iterator value: the built-in method, the user's `next`, or a symbolic call when the
    /// receiver is a type parameter.
    pub(super) fn iterator_next_call(&mut self, it: tir::Expr, item: &Ty, span: Span) -> tir::Expr {
        let ret = self.globals.option_ty(item.clone());
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
        label: Option<&ast::Ident>,
        span: Span,
    ) -> tir::Expr {
        let iter_ty = iterable.ty.clone();
        let it = self.declare_hidden("__iter", iter_ty.clone(), true, LocalKind::Owned);
        self.scopes.push(Default::default());
        self.loops.push(LoopScope::for_body(label));
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
                pat: Pat::Variant(1, vec![pat]),
                guard: None,
                body: mk(tir::ExprKind::Block(body), Ty::Unit, span),
            },
            tir::Arm {
                pat: Pat::Variant(0, Vec::new()),
                guard: None,
                body: mk(tir::ExprKind::Break { label: None, value: None }, Ty::Never, span),
            },
        ];
        let matched = mk(tir::ExprKind::Match(Box::new(next), arms), Ty::Unit, span);
        // The label goes on the generated `loop`, the node the user's `break 'label`/`continue 'label` target;
        // wrapping the whole block would leave codegen and the move checker without a loop to attach it to.
        let looped = Self::with_label(
            label,
            mk(
                tir::ExprKind::Loop(tir::Block { stmts: Vec::new(), tail: Some(Box::new(matched)), close: span }),
                Ty::Unit,
                span,
            ),
        );
        let init = tir::Stmt::Let(Pat::Binding(it, self.bind_mode(&iter_ty, true)), iterable, None);
        mk(
            tir::ExprKind::Block(tir::Block { stmts: vec![init], tail: Some(Box::new(looped)), close: span }),
            Ty::Unit,
            span,
        )
    }

    /// `for pattern in mut e`: the loop calls `e.iter_mut()` and walks the iterator it returns, whose items are
    /// writable views (IT15). The collection is borrowed exclusively until the loop ends. The method is a
    /// compiler-known one for `List`, arrays, and `Map`, and an ordinary `iter_mut(mut self)` method for a user
    /// type; either way the result must be an `Iterator`.
    pub(super) fn for_mut(
        &mut self,
        pattern: &ast::Pattern,
        iter: &ast::Expr,
        body: &ast::Block,
        label: Option<&ast::Ident>,
        span: Span,
    ) -> tir::Expr {
        let name = ast::Ident { name: "iter_mut".to_string(), span: iter.span };
        let iterator = self.method_call(iter, &name, &[], &[], span, None);
        if iterator.ty == Ty::Error {
            // The iterable did not resolve; the body is still checked for its own errors.
            self.scopes.push(Default::default());
            self.loops.push(LoopScope::for_body(label));
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
        self.protocol_for(pattern, iterator, item, body, label, span)
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
        self.method_call(iter, &name, &[], &[], span, None)
    }
}
