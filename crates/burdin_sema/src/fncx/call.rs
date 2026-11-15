//! Calls: functions, methods, associated functions, extern functions, and the argument conventions.

use super::{FnCx, PathTy, error_expr, mk};
use crate::builtins::{builtin, builtin_sig, method};
use crate::globals::{Callee, Global, adt_key};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{Place, is_place_expr, place_mentions, place_root, places_overlap};
use burdin_tir::{self as tir, Builtin, IntTy, Method, ParamSig, Ty};

impl FnCx<'_> {
    pub(super) fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span, expect: Option<&Ty>) -> tir::Expr {
        if let ast::ExprKind::Field(recv, name) = &callee.kind {
            return self.method_call(recv, name, args, span, expect);
        }
        // A bare name that resolves to a local, or to a variable captured from the enclosing function, is a
        // closure call; only a name with no value meaning is looked up as a function.
        if let ast::ExprKind::Name { name: id, args: type_args } = &callee.kind
            && type_args.is_empty()
        {
            let local = match self.lookup(&id.name) {
                Some(local) => Some(local),
                None if self.capture.is_some() => {
                    if let Some(e) = self.capture_name(&id.name, id.span) {
                        return self.closure_call(e, args, span);
                    }
                    None
                }
                None => None,
            };
            if let Some(local) = local {
                let ty = self.locals[local.0].ty.clone();
                let value = mk(tir::ExprKind::Local(local), ty, id.span);
                return self.closure_call(value, args, span);
            }
        }
        let (params, ret, target) = match &callee.kind {
            ast::ExprKind::Name { name: id, args: type_args }
                if type_args.is_empty() && self.lookup(&id.name).is_none() =>
            {
                match self.globals.value(self.module, &id.name) {
                    Some(Global::Func(f)) => {
                        self.check_unsafe_call(f, &id.name, id.span);
                        let sig = &self.globals.sigs[f.0];
                        (sig.params.clone(), sig.ret.clone(), Callee::User(f))
                    }
                    Some(Global::Generic(g)) => return self.generic_call(g, id, args, span, expect),
                    Some(Global::Extern(e)) => return self.extern_call(id, e, args, span),
                    Some(Global::Const(_)) => {
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            id.span,
                            format!("`{}` is a constant, not a function", id.name),
                        );
                        return self.args_then_error(args, span);
                    }
                    None if id.name == "swap" || id.name == "__swap" => return self.swap_call(args, span),
                    None if id.name == "replace" || id.name == "__replace" => {
                        return self.replace_call(args, span);
                    }
                    None => match builtin(&id.name) {
                        Some(Builtin::AssertEq) => return self.assert_eq(args, span),
                        Some(b) => {
                            let sig = builtin_sig(b);
                            (sig.params, sig.ret, Callee::Builtin(b))
                        }
                        None if id.name == "Some" => return self.some(args, expect, span),
                        None if id.name == "Ok" => return self.result_ctor(true, args, expect, span),
                        None if id.name == "Err" => return self.result_ctor(false, args, expect, span),
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown function `{}`", id.name));
                            return self.args_then_error(args, span);
                        }
                    },
                }
            }
            _ => {
                // A closure value: a local, a field access other than a method, or any expression that
                // evaluates to a closure interface.
                let value = self.expr(callee, None);
                if value.ty == Ty::Error {
                    return self.args_then_error(args, span);
                }
                return self.closure_call(value, args, span);
            }
        };
        let name = match &callee.kind {
            ast::ExprKind::Name { name, .. } => name.name.clone(),
            _ => String::new(),
        };
        let Some(checked) = self.args(&name, &params, args, None, span) else { return error_expr(span) };
        let kind = match target {
            Callee::User(f) => tir::ExprKind::Call(f, checked),
            Callee::Builtin(b) => tir::ExprKind::Builtin(b, checked),
        };
        mk(kind, ret, span)
    }

    /// A call to an `extern "C"` function: it always requires an `unsafe` context, and its arguments are passed
    /// by value.
    fn extern_call(&mut self, name: &ast::Ident, id: usize, args: &[ast::Arg], span: Span) -> tir::Expr {
        let info = &self.globals.externs[id];
        let sig = info.sig.clone();
        let vararg = info.vararg;
        let symbol = info.symbol.clone();
        // Visibility is checked where the name is bound (the `use` import, or a module-qualified call), so a
        // `priv` declaration in one module does not hide a public declaration of the same symbol in another.
        if self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                format!("calling the C function `{symbol}` requires an `unsafe` block"),
            );
        }
        let fixed = sig.params.len();
        if args.len() < fixed || (!vararg && args.len() != fixed) {
            for a in args {
                self.expr(&a.expr, None);
            }
            let expected = if vararg { format!("at least {fixed}") } else { fixed.to_string() };
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{symbol}` takes {expected} argument(s), but {} were given", args.len()),
            );
            return error_expr(span);
        }
        let (fixed_args, extra_args) = args.split_at(fixed);
        let Some(mut checked) = self.args(&name.name, &sig.params, fixed_args, None, span) else {
            return error_expr(span);
        };
        for a in extra_args {
            let value = self.expr(&a.expr, None);
            if value.ty != Ty::Error && !crate::resolve::is_c_abi_value(&value.ty) {
                self.diags.error(
                    Code::C_ABI,
                    value.span,
                    crate::resolve::c_abi_rejection(&value.ty, "passed through `...` to a C function"),
                );
            }
            checked.push(value);
        }
        mk(tir::ExprKind::ExternCall(id, checked), sig.ret, span)
    }

    /// `Some(value)`.
    pub(super) fn some(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        if args.len() != 1 {
            self.diags.error(Code::ARG_COUNT, span, format!("`Some` takes 1 argument, but {} were given", args.len()));
            return self.args_then_error(args, span);
        }
        let inner_expect = match expect {
            Some(Ty::Option(t)) => Some((**t).clone()),
            _ => None,
        };
        let v = self.expr(&args[0].expr, inner_expect.as_ref());
        let v = match &inner_expect {
            Some(t) => self.coerce_arg(v, t),
            None => v,
        };
        let ty = Ty::Option(Box::new(inner_expect.unwrap_or_else(|| v.ty.clone())));
        mk(tir::ExprKind::Some(Box::new(v)), ty, span)
    }

    /// `Ok(value)` and `Err(value)` of a `Result<T, E>`. The expected type gives both type arguments, so a
    /// `Result` built without one needs an annotation.
    pub(super) fn result_ctor(&mut self, ok: bool, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        if args.len() != 1 {
            let name = if ok { "Ok" } else { "Err" };
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes 1 argument, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let (payload_expect, result_ty) = match expect {
            Some(Ty::Result(t, e)) => {
                let payload = if ok { (**t).clone() } else { (**e).clone() };
                (payload, Ty::Result(t.clone(), e.clone()))
            }
            Some(Ty::Error) => (Ty::Error, Ty::Error),
            _ => {
                self.expr(&args[0].expr, None);
                let name = if ok { "Ok" } else { "Err" };
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("cannot infer the type of this `{name}`; give the binding a `Result` type"),
                );
                return error_expr(span);
            }
        };
        let value = self.expr(&args[0].expr, Some(&payload_expect));
        let value = self.coerce_arg(value, &payload_expect);
        let kind = if ok { tir::ExprKind::Ok(Box::new(value)) } else { tir::ExprKind::Err(Box::new(value)) };
        mk(kind, result_ty, span)
    }

    /// `assert_eq(a, b)`: both values must have the same `Eq` type.
    fn assert_eq(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`assert_eq` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let l = self.expr(&args[0].expr, None);
        let l = self.auto_deref(l);
        let r = self.expr(&args[1].expr, Some(&l.ty));
        let r = self.coerce_arg(r, &l.ty);
        if !self.globals.has_eq_in(&l.ty, &self.bounds) && !self.param_has(&l.ty, crate::generics::Bound::Eq) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`assert_eq` needs `Eq` values, but `{}` does not support `==`", l.ty),
            );
        }
        mk(tir::ExprKind::Builtin(Builtin::AssertEq, vec![l, r]), Ty::Unit, span)
    }

    /// `swap(mut a, mut b)`: two places of one type, both mutable and non-overlapping.
    fn swap_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        if args.len() != 2 {
            self.diags.error(Code::ARG_COUNT, span, format!("`swap` takes 2 arguments, but {} were given", args.len()));
            return self.args_then_error(args, span);
        }
        let a = self.expr(&args[0].expr, None);
        let b = self.expr(&args[1].expr, None);
        for e in [&a, &b] {
            if e.ty != Ty::Error && (!is_place_expr(e) || !self.place_mutable(e)) {
                self.diags.error(Code::MUT_OF_IMMUTABLE, e.span, "`swap` needs two mutable places");
            }
        }
        if !crate::types::compatible(&a.ty, &b.ty) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`swap` needs two values of the same type, found `{}` and `{}`", a.ty, b.ty),
            );
        }
        let mut pa = Vec::new();
        let mut pb = Vec::new();
        place_mentions(&a, &mut pa);
        place_mentions(&b, &mut pb);
        if pa.iter().any(|p| pb.iter().any(|q| places_overlap(p, q))) {
            self.diags.error(Code::EXCLUSIVITY, span, "`swap` needs two places that do not overlap");
        }
        mk(tir::ExprKind::Builtin(Builtin::Swap, vec![a, b]), Ty::Unit, span)
    }

    /// `replace(mut place, value)`: stores `value` and returns what `place` held.
    fn replace_call(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        if args.len() != 2 {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`replace` takes 2 arguments, but {} were given", args.len()),
            );
            return self.args_then_error(args, span);
        }
        let place = self.expr(&args[0].expr, None);
        if place.ty != Ty::Error && (!is_place_expr(&place) || !self.place_mutable(&place)) {
            self.diags.error(Code::MUT_OF_IMMUTABLE, place.span, "`replace` needs a mutable place");
        }
        let value = self.expr(&args[1].expr, Some(&place.ty));
        let value = self.coerce_arg(value, &place.ty);
        let ty = place.ty.clone();
        let mut pa = Vec::new();
        let mut pv = Vec::new();
        place_mentions(&place, &mut pa);
        place_mentions(&value, &mut pv);
        if pa.iter().any(|p| pv.iter().any(|q| places_overlap(p, q))) {
            self.diags.error(Code::EXCLUSIVITY, span, "`replace` needs a place that the new value does not mention");
        }
        mk(tir::ExprKind::Builtin(Builtin::Replace, vec![place, value]), ty, span)
    }

    /// Checks call arguments against the parameter conventions (CV1-CV3) and reports exclusivity violations
    /// (EX1). `receiver` is the method receiver and whether it is borrowed exclusively (`mut self`, or the
    /// receiver of `String.push_str`).
    pub(super) fn args(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
    ) -> Option<Vec<tir::Expr>> {
        self.args_pre(name, params, args, receiver, span, None)
    }

    /// Like `args`, for arguments that were already checked (a generic call checks them first to infer its type
    /// arguments).
    pub(super) fn args_pre(
        &mut self,
        name: &str,
        params: &[ParamSig],
        args: &[ast::Arg],
        receiver: Option<(&tir::Expr, bool)>,
        span: Span,
        pre: Option<Vec<tir::Expr>>,
    ) -> Option<Vec<tir::Expr>> {
        if params.len() != args.len() {
            if pre.is_none() {
                for a in args {
                    self.expr(&a.expr, None);
                }
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`{name}` takes {} argument(s), but {} were given", params.len(), args.len()),
            );
            return None;
        }
        let mut checked = Vec::new();
        let mut pre = pre.map(|v| v.into_iter());
        for (a, p) in args.iter().zip(params) {
            if p.mut_param && !a.mut_marker {
                let position = a.expr.span;
                self.diags
                    .error(
                        Code::MISSING_MUT_MARKER,
                        position,
                        format!("this parameter of `{name}` is `mut`; pass the argument as `mut ...`"),
                    )
                    .suggest(
                        Span::new(position.file, position.lo, position.lo),
                        "add `mut` before this argument",
                        "mut ",
                    );
            } else if !p.mut_param && a.mut_marker {
                let error = self.diags.error(
                    Code::UNNEEDED_MUT_MARKER,
                    a.expr.span,
                    format!("this parameter of `{name}` is not `mut`; the argument takes no `mut` marker"),
                );
                if let Some(mut_span) = a.mut_span {
                    // Delete the marker and the whitespace after it, so applying the edit leaves plain `name`.
                    error.suggest(Span::new(mut_span.file, mut_span.lo, a.expr.span.lo), "remove the `mut` marker", "");
                }
            }
            let arg = match pre.as_mut().and_then(|it| it.next()) {
                Some(arg) => arg,
                None => self.expr(&a.expr, Some(&p.ty)),
            };
            let arg = self.coerce_arg(arg, &p.ty);
            if p.mut_param && !matches!(arg.ty, Ty::Error) {
                if !is_place_expr(&arg) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "the argument of a `mut` parameter must be a place",
                    );
                } else if !self.place_mutable(&arg) {
                    self.diags.error(
                        Code::MUT_OF_IMMUTABLE,
                        arg.span,
                        "cannot pass this place as `mut`: it is not mutable",
                    );
                }
            }
            checked.push(arg);
        }
        self.exclusivity(receiver, params, &checked, span);
        Some(checked)
    }

    /// No argument may mention a place that overlaps one passed as `mut` (EX1, EX2).
    pub(super) fn exclusivity(
        &mut self,
        receiver: Option<(&tir::Expr, bool)>,
        params: &[ParamSig],
        args: &[tir::Expr],
        span: Span,
    ) {
        let mut entries: Vec<(bool, Span, Vec<Place>)> = Vec::new();
        if let Some((receiver, exclusive)) = receiver {
            let mut places = Vec::new();
            place_mentions(receiver, &mut places);
            entries.push((exclusive, receiver.span, places));
        }
        for (p, a) in params.iter().zip(args) {
            let mut places = Vec::new();
            place_mentions(a, &mut places);
            entries.push((p.mut_param, a.span, places));
        }
        for i in 0..entries.len() {
            if !entries[i].0 {
                continue;
            }
            for j in 0..entries.len() {
                if i != j && entries[i].2.iter().any(|p| entries[j].2.iter().any(|q| places_overlap(p, q))) {
                    let other = entries[j].1;
                    self.diags
                        .error(
                            Code::EXCLUSIVITY,
                            span,
                            "another argument of this call touches the same value as a `mut` argument",
                        )
                        .note(other, "this argument touches the same place");
                    return;
                }
            }
        }
    }

    pub(super) fn args_then_error(&mut self, args: &[ast::Arg], span: Span) -> tir::Expr {
        for a in args {
            self.expr(&a.expr, None);
        }
        error_expr(span)
    }

    pub(super) fn method_call(
        &mut self,
        recv: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        // The prelude modules: `env.args()` and `process.exit`.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && self.lookup(&id.name).is_none()
            && self.globals.value(self.module, &id.name).is_none()
        {
            if id.name == "env" && name.name == "args" {
                if !args.is_empty() {
                    self.diags.error(Code::ARG_COUNT, span, "`env.args` takes no arguments");
                    return self.args_then_error(args, span);
                }
                let ty = Ty::List(Box::new(Ty::String));
                return mk(tir::ExprKind::Builtin(Builtin::EnvArgs, Vec::new()), ty, span);
            }
            if id.name == "process" && name.name == "exit" {
                let sig = builtin_sig(Builtin::ProcessExit);
                let Some(checked) =
                    self.args("process.exit", &[ParamSig::plain(Ty::Int(IntTy::I32))], args, None, span)
                else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::ProcessExit, checked), sig.ret, span);
            }
        }
        // `Heap.new(value)`: the only associated function of a built-in type that is not written as a type path,
        // because `Heap` alone is not a type.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && id.name == "Heap"
            && name.name == "new"
            && self.lookup("Heap").is_none()
            && self.globals.value(self.module, "Heap").is_none()
        {
            return self.heap_new(args, expect, span);
        }
        // `CPtr.null()`, typed by the expected type.
        if let ast::ExprKind::Name { name: id, args: type_args } = &recv.kind
            && type_args.is_empty()
            && id.name == "CPtr"
            && name.name == "null"
            && self.lookup("CPtr").is_none()
            && self.globals.value(self.module, "CPtr").is_none()
        {
            return self.cptr_null(args, expect, span);
        }
        if let Some(path) = self.type_path(recv) {
            return match path {
                PathTy::Ty(ty) => self.assoc_call(&ty, name, args, span),
                PathTy::Template(g) => self.assoc_call_template(g, name, args, expect, span),
            };
        }
        // A module-qualified type: `sh.Square.new()`.
        if let Some(ty) = self.qualified_type(recv) {
            return self.assoc_call(&ty, name, args, span);
        }
        // A module-qualified function: `math.add(...)`, `sh.area(...)`.
        if let Some((target, item)) = self.module_member(recv, name) {
            return self.module_call(target, &item, args, span, expect);
        }
        let recv = self.expr(recv, None);
        if recv.ty == Ty::Error {
            return self.args_then_error(args, span);
        }
        // `x.clone()`: the deep copy the compiler derives (CP). It applies to the receiver as written, before
        // any auto-dereference, so cloning a shared reference clones the reference.
        if name.name == "clone" && args.is_empty() && self.globals.is_clone(&recv.ty) {
            let ty = recv.ty.clone();
            return mk(tir::ExprKind::Method(Method::Clone, vec![recv]), ty, span);
        }
        // Field access and method calls see through references (RF1).
        let recv = self.auto_deref(recv);
        if let Ty::Param(_) = recv.ty {
            return self.param_method_call(recv, name, args, span);
        }
        if let Some(key) = adt_key(&recv.ty)
            && let Some(&f) = self.globals.methods.get(&(key, name.name.clone()))
        {
            self.check_member_access(&recv.ty, key, name);
            self.check_unsafe_call(f, &name.name, name.span);
            // A method with its own type parameters is instantiated for the arguments this call infers.
            if let Some(info) = self.globals.method_templates.get(&f).cloned() {
                return self.generic_method_call(info, recv, &name.name, args, span);
            }
            let sig = &self.globals.sigs[f.0];
            if !sig.has_self {
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    name.span,
                    format!("`{}` has no `self` parameter; call it as `{}.{}(...)`", name.name, recv.ty, name.name),
                );
                return self.args_then_error(args, span);
            }
            let (receiver, rest, ret) = (sig.params[0].clone(), sig.params[1..].to_vec(), sig.ret.clone());
            // Only `mut self` borrows the receiver exclusively; `own self` takes it, and read-only `self`
            // only reads it.
            if receiver.by_ref && receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
                let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
                );
            }
            let receiver_place = Some((&recv, receiver.exclusive));
            let Some(mut checked) = self.args(&name.name, &rest, args, receiver_place, span) else {
                return error_expr(span);
            };
            checked.insert(0, recv);
            return mk(tir::ExprKind::Call(f, checked), ret, span);
        }
        if name.name == "hash" && args.is_empty() && self.globals.has_hash_in(&recv.ty, &self.bounds) {
            return mk(tir::ExprKind::Method(Method::Hash, vec![recv]), Ty::Int(IntTy::U64), span);
        }
        // A field that holds a closure is callable: `holder.f(x)` calls the closure in `f` when no method of that
        // name exists (CL6). The field wins only when its type is a closure interface.
        if let Some((index, field_ty)) = self.closure_field(&recv, &name.name) {
            let field = mk(tir::ExprKind::Field(Box::new(recv), index), field_ty, name.span);
            return self.closure_call(field, args, span);
        }
        // The iteration protocol: `next`, `sum`, the eager `List` helpers, and the `Iterator` default methods.
        if let Some(e) = self.iterator_method(recv.clone(), name, args, span) {
            return e;
        }
        let Some((m, params, ret)) = method(&name.name, &recv.ty) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on type `{}`", name.name, recv.ty));
            return self.args_then_error(args, span);
        };
        if m.mutates_receiver() && !matches!(recv.ty, Ty::Error) {
            if !is_place_expr(&recv) {
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, so the receiver must be a mutable place", name.name),
                );
            } else if !self.place_mutable(&recv) {
                let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
                let whose = local.unwrap_or_else(|| "this receiver".to_string());
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    recv.span,
                    format!("`{}` changes its receiver, but `{whose}` is not mutable", name.name),
                );
            }
        }
        match (&m, &recv.ty) {
            (Method::ListSort, Ty::List(elem)) => {
                if !self.globals.has_ord_in(elem, &self.bounds) {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!("cannot sort a `List<{elem}>`: `{elem}` does not support `Ord`"),
                    );
                }
            }
            (Method::ListContains, Ty::List(elem)) => {
                if !self.globals.has_eq_in(elem, &self.bounds) {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!("cannot search a `List<{elem}>`: `{elem}` does not support `==`"),
                    );
                }
            }
            (Method::MapGet | Method::MapRemove | Method::MapContainsKey | Method::MapInsert, Ty::Map(k, _))
                if !self.globals.has_eq_in(k, &self.bounds) =>
            {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("cannot use `{k}` as a map key: it does not support `==`"),
                );
            }
            _ => {}
        }
        // A mutating method borrows the receiver exclusively (EX1).
        let receiver_place = Some((&recv, m.mutates_receiver()));
        let Some(mut checked) = self.args(&name.name, &params, args, receiver_place, span) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(m, checked), ret, span)
    }

    /// A struct field of closure type named `name`, for calling `holder.f(x)`. A field whose type is a type
    /// parameter with an `Fn`-family bound is callable the same way (the adapters in `std` store one).
    fn closure_field(&self, recv: &tir::Expr, name: &str) -> Option<(usize, Ty)> {
        let Ty::Struct(a) = &recv.ty else { return None };
        let (index, field) = self.globals.structs[a.id].fields.iter().enumerate().find(|(_, f)| f.name == name)?;
        self.globals.fn_call_sig(&field.ty, &self.bounds).map(|_| (index, field.ty.clone()))
    }

    /// A function or constant found through a module path: `math.add(...)`, `sh.area(...)`.
    fn module_call(
        &mut self,
        target: usize,
        item: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.check_value_visibility(target, &item.name, item.span);
        match self.globals.exported_value(target, &item.name) {
            Some(Global::Func(f)) => {
                self.check_unsafe_call(f, &item.name, item.span);
                let sig = &self.globals.sigs[f.0];
                let (params, ret) = (sig.params.clone(), sig.ret.clone());
                let Some(checked) = self.args(&item.name, &params, args, None, span) else {
                    return error_expr(span);
                };
                mk(tir::ExprKind::Call(f, checked), ret, span)
            }
            Some(Global::Generic(g)) => self.generic_call(g, item, args, span, expect),
            Some(Global::Extern(e)) => self.extern_call(item, e, args, span),
            Some(Global::Const(_)) => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    item.span,
                    format!("`{}` is a constant, not a function", item.name),
                );
                self.args_then_error(args, span)
            }
            None => {
                if self.globals.type_name(target, &item.name).is_some() {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        item.span,
                        format!("`{}` is a type, not a function", item.name),
                    );
                } else {
                    self.diags.error(Code::UNKNOWN_NAME, item.span, format!("unknown function `{}`", item.name));
                }
                self.args_then_error(args, span)
            }
        }
    }

    /// `CPtr.null()`.
    fn cptr_null(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        if !args.is_empty() {
            self.diags.error(Code::ARG_COUNT, span, "`CPtr.null` takes no arguments");
            return self.args_then_error(args, span);
        }
        match expect {
            Some(Ty::CPtr(inner)) => {
                mk(tir::ExprKind::Builtin(Builtin::CPtrNull, Vec::new()), Ty::CPtr(inner.clone()), span)
            }
            _ => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    "cannot tell the type of this `CPtr.null()`; annotate the binding with a `CPtr` type",
                );
                error_expr(span)
            }
        }
    }

    /// `Heap.new(value)`.
    pub(super) fn heap_new(&mut self, args: &[ast::Arg], expect: Option<&Ty>, span: Span) -> tir::Expr {
        let inner = match expect {
            Some(Ty::Heap(t)) => Some((**t).clone()),
            _ => None,
        };
        if args.len() != 1 {
            for a in args {
                self.expr(&a.expr, None);
            }
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("`Heap.new` takes 1 argument, but {} were given", args.len()),
            );
            return error_expr(span);
        }
        let value = self.expr(&args[0].expr, inner.as_ref());
        let ty = Ty::Heap(Box::new(value.ty.clone()));
        mk(tir::ExprKind::Builtin(Builtin::HeapNew, vec![value]), ty, span)
    }

    /// `Type.function(...)`, `Enum.Variant(...)`, and the built-in types' associated functions.
    pub(super) fn assoc_call(&mut self, ty: &Ty, name: &ast::Ident, args: &[ast::Arg], span: Span) -> tir::Expr {
        // The type already failed to resolve; do not report a second error for its associated items.
        if *ty == Ty::Error {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        }
        if let Ty::Param(_) = ty {
            return self.param_assoc_call(ty, name, args, span);
        }
        // `Type.size()` and `Type.align()`: the C layout of a type, for `repr(C)` size assertions (issue #490).
        if matches!(name.name.as_str(), "size" | "align") && args.is_empty() {
            let user_method = crate::globals::adt_key(ty)
                .is_some_and(|key| self.globals.methods.contains_key(&(key, name.name.clone())));
            if !user_method {
                let alignment = name.name == "align";
                return mk(tir::ExprKind::LayoutOf { ty: ty.clone(), alignment }, Ty::Int(IntTy::Usize), span);
            }
        }
        // A raw pointer has `null`; a pointer to `CChar` is how `CStr` is written, so it also reads the
        // NUL-terminated bytes into a `String`.
        if let Ty::CPtr(inner) = ty {
            if name.name == "null" && args.is_empty() {
                return mk(tir::ExprKind::Builtin(Builtin::CPtrNull, Vec::new()), ty.clone(), span);
            }
            if name.name == "to_string" && **inner == Ty::Int(IntTy::I8) {
                let sig = builtin_sig(Builtin::CStrToString);
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::CStrToString, checked), sig.ret, span);
            }
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`CPtr` has no function `{}`", name.name));
            return self.args_then_error(args, span);
        }
        // `List<T>.new()` and `Map<K, V>.new()`.
        if let Ty::List(_) = ty
            && name.name == "new"
        {
            if !args.is_empty() {
                self.diags.error(Code::ARG_COUNT, span, "`List.new` takes no arguments");
                return self.args_then_error(args, span);
            }
            return mk(tir::ExprKind::Builtin(Builtin::ListNew, Vec::new()), ty.clone(), span);
        }
        if let Ty::Map(..) = ty
            && name.name == "new"
        {
            if !args.is_empty() {
                self.diags.error(Code::ARG_COUNT, span, "`Map.new` takes no arguments");
                return self.args_then_error(args, span);
            }
            return mk(tir::ExprKind::Builtin(Builtin::MapNew, Vec::new()), ty.clone(), span);
        }
        // `i32.parse(s)`: the number in the text, or an error.
        if let Ty::Int(i) = ty {
            if name.name != "parse" {
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    name.span,
                    format!("`{}` has no function `{}`", i.name(), name.name),
                );
                return self.args_then_error(args, span);
            }
            let Some(checked) = self.args("parse", &[ParamSig::plain(Ty::Str)], args, None, span) else {
                return error_expr(span);
            };
            let ret = Ty::Result(Box::new(ty.clone()), Box::new(Ty::Unit));
            return mk(tir::ExprKind::Builtin(Builtin::IntParse(*i), checked), ret, span);
        }
        // The associated functions of `String` (the only built-in owning type with any so far).
        if *ty == Ty::String && name.name != "default" {
            let b = match name.name.as_str() {
                "new" => Builtin::StringNew,
                "from" => Builtin::StringFrom,
                _ => {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        name.span,
                        format!("`String` has no function `{}`", name.name),
                    );
                    return self.args_then_error(args, span);
                }
            };
            let sig = builtin_sig(b);
            let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                return error_expr(span);
            };
            return mk(tir::ExprKind::Builtin(b, checked), sig.ret, span);
        }
        // The C string bridge.
        if *ty == Ty::CString {
            if name.name == "from" {
                let sig = builtin_sig(Builtin::CStringFrom);
                let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
                    return error_expr(span);
                };
                return mk(tir::ExprKind::Builtin(Builtin::CStringFrom, checked), sig.ret, span);
            }
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`CString` has no function `{}`", name.name));
            return self.args_then_error(args, span);
        }
        if let Ty::Enum(a) = ty
            && let Some(index) = self.globals.enums[a.id].variants.iter().position(|v| v.name == name.name)
        {
            let v = &self.globals.enums[a.id].variants[index];
            if v.shape != tir::VariantShape::Tuple {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("variant `{}` is not built with `(...)`", name.name),
                );
                return self.args_then_error(args, span);
            }
            let params: Vec<ParamSig> = v.fields.iter().map(|f| ParamSig::plain(f.ty.clone())).collect();
            let Some(checked) = self.args(&name.name, &params, args, None, span) else { return error_expr(span) };
            let fields = checked.into_iter().enumerate().collect();
            return mk(tir::ExprKind::Variant(index, fields), ty.clone(), span);
        }
        let method = adt_key(ty).and_then(|k| self.globals.methods.get(&(k, name.name.clone())).copied());
        let Some(f) = method else {
            if name.name == "default" && args.is_empty() && self.globals.has_default(ty) {
                return self.default_expr(ty, span);
            }
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("`{ty}` has no function or variant `{}`", name.name),
            );
            return self.args_then_error(args, span);
        };
        if let Some(key) = adt_key(ty) {
            self.check_member_access(ty, key, name);
        }
        self.check_unsafe_call(f, &name.name, name.span);
        let sig = &self.globals.sigs[f.0];
        let (params, ret) = (sig.params.clone(), sig.ret.clone());
        let Some(checked) = self.args(&name.name, &params, args, None, span) else { return error_expr(span) };
        mk(tir::ExprKind::Call(f, checked), ret, span)
    }
}
