//! Calls, constructors, and literals of generic functions and types, where the type arguments are inferred.

use super::{FnCx, error_expr, mk};
use crate::generics::{Bound, BoundMethodGenerics, GenericMethod, Request, SYMBOLIC_CALL};
use crate::types::generic_args_missing;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{is_place_expr, place_root};
use burdin_tir::{self as tir, Builtin, FnKind, IntTy, Method, ParamSig, Ty};
use std::rc::Rc;

impl FnCx<'_> {
    /// A call to a generic function: the arguments are checked first, the type arguments are inferred from
    /// them, and the call goes to the instance for those arguments.
    pub(super) fn generic_call(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        explicit: &[ast::Type],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.generic_call_impl(g, id, args, None, explicit, span, expect)
    }

    /// A call to a generic function whose arguments are already checked, one expression per `args` entry (the
    /// typed `assert_eq`). The checked expressions drive the inference and the argument conventions; the AST
    /// entries are only placeholders for them.
    pub(super) fn generic_call_checked(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        checked: Vec<tir::Expr>,
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.generic_call_impl(g, id, args, Some(checked), &[], span, expect)
    }

    #[allow(clippy::too_many_arguments)]
    fn generic_call_impl(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        pre: Option<Vec<tir::Expr>>,
        explicit: &[ast::Type],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let globals = self.globals;
        let info = &globals.generic_fns[g];
        if info.is_unsafe && self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                id.span,
                format!("calling the unsafe function `{}` requires an `unsafe` block", id.name),
            );
        }
        let pattern = &globals.sigs[info.identity.0];
        // Named arguments are matched to parameters before inference (`spec/arguments.md`, AG4). A parameter
        // filled by its default takes no argument and contributes nothing to inference; a default never
        // depends on a type parameter (AG3).
        let Some(slots) = self.arg_slots(&id.name, &pattern.params, args, span) else {
            // The name or count error is already reported; check the expressions for their own errors.
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        };
        // Arguments are checked in parameter order: what an earlier argument binds (`max_of(3, 9)` gives
        // `T = i32`) specializes the hint for the later ones, which is what lets a closure see `Fn(&i32): U`
        // instead of `Fn(&T): U`.
        let mut checked: Vec<Option<tir::Expr>> = (0..args.len()).map(|_| None).collect();
        // Explicit type arguments are bindings from the start, so inference keeps them (TA3).
        let mut bindings = match self.explicit_bindings(&id.name, &info.params, &info.bounds, info.hidden, explicit) {
            Some(bindings) => bindings,
            None => return self.args_then_error(args, span),
        };
        for (i, p) in pattern.params.iter().enumerate() {
            let Some(ai) = slots[i] else { continue };
            let hint = generic_hint(globals, &p.ty, &info.params, &info.bounds, &bindings);
            let e = match pre.as_ref().and_then(|pre| pre.get(ai)) {
                Some(e) => e.clone(),
                None => self.expr_arg(&args[ai].expr, hint.as_ref(), p.mut_param),
            };
            globals.unify(&p.ty, &e.ty, &mut bindings);
            checked[ai] = Some(e);
        }
        // A type parameter that only appears in another parameter's `Fn` bound (`fn map<U, F: Fn(T): U>`) is
        // bound by matching that bound's signature with the closure it received. The hook takes the arguments
        // in parameter order, so a named argument written elsewhere still meets its own parameter.
        let per_param: Vec<Option<tir::Expr>> =
            slots.iter().map(|slot| slot.and_then(|ai| checked[ai].clone())).collect();
        bound_fn_inference(globals, &info.params, &info.bounds, &pattern.params, &per_param, &mut bindings);
        bound_iterator_inference(globals, &info.params, &info.bounds, &self.bounds, &mut bindings);
        if let Some(expected) = expect {
            globals.unify(&pattern.ret, expected, &mut bindings);
        }
        let mut targs = Vec::new();
        for pname in &info.params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => {
                    // A closure argument whose body already failed has no result type to bind, and an argument that
                    // failed (`map(Ok)` with no error type) binds nothing; the failure is reported where it
                    // happened, not again here.
                    if checked
                        .iter()
                        .flatten()
                        .any(|e| e.ty == Ty::Error || matches!(&e.ty, Ty::Fn { sig, .. } if *sig.ret == Ty::Error))
                    {
                        return error_expr(span);
                    }
                    let k = info.params.iter().position(|p| p == pname).unwrap_or(0);
                    let is_const =
                        info.bounds.get(k).is_some_and(|bs| crate::globals::Globals::const_param_ty(bs).is_some());
                    let (code, what) =
                        if is_const { (Code::CONST_INFER, "const argument ") } else { (Code::TYPE_MISMATCH, "") };
                    self.diags.error(
                        code,
                        span,
                        format!("cannot tell what {what}`{pname}` stands for in this call to `{}`", id.name),
                    );
                    return error_expr(span);
                }
            }
        }
        if targs.contains(&Ty::Error) {
            return error_expr(span);
        }
        let mut ok = true;
        let subst: Vec<(String, Ty)> = info.params.iter().cloned().zip(targs.iter().cloned()).collect();
        for ((pname, t), bounds) in info.params.iter().zip(&targs).zip(&info.bounds) {
            // A closure whose body already failed has no result type to check against its `Fn` bound.
            if matches!(t, Ty::Fn { sig, .. } if *sig.ret == Ty::Error) {
                continue;
            }
            for b in bounds {
                if !globals.satisfies_with(t, b, &self.bounds, &subst) {
                    let (code, message) =
                        globals.unmet_bound(t, b, &subst, &self.bounds, &format!("`{pname}` of `{}`", id.name));
                    self.diags.error(code, span, message);
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        let (params, ret, callee) = if self.symbolic || targs.iter().any(|t| globals.ty_has_param(t)) {
            // Inside symbolic code there is no instance to call; the types are all that matters.
            let env: Vec<(String, Ty)> = info.params.iter().cloned().zip(targs).collect();
            let params = pattern.params.iter().map(|p| globals.subst_param(p, &env)).collect();
            (params, globals.subst_ty(&pattern.ret, &env), SYMBOLIC_CALL)
        } else {
            match globals.fn_instances.get(&(g, targs.clone())) {
                Some(&f) => {
                    let sig = &globals.sigs[f.0];
                    (sig.params.clone(), sig.ret.clone(), f)
                }
                None => {
                    globals.requests.borrow_mut().push(Request::Func(g, targs));
                    return error_expr(span);
                }
            }
        };
        // The checked expressions, in written order, one per written argument; `args_pre` fills the defaults.
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let Some(checked) = self.args_pre(&id.name, &params, args, None, span, Some(written)) else {
            return error_expr(span);
        };
        mk(tir::ExprKind::Call(callee, checked), ret, span)
    }

    /// The bindings an explicit type-argument list gives a call (`f<T>(...)`, `spec/arguments.md`, TA1-TA4): one per
    /// type parameter, in declaration order, so inference has nothing left to decide. A callee with an `impl`
    /// parameter takes none (TA4). A list of the wrong length, or a type that does not resolve, is reported here and
    /// gives `None`.
    pub(super) fn explicit_bindings(
        &mut self,
        what: &str,
        params: &[String],
        bounds: &[Vec<Bound>],
        hidden: bool,
        explicit: &[ast::Type],
    ) -> Option<Vec<(String, Ty)>> {
        let (Some(first), Some(last)) = (explicit.first(), explicit.last()) else { return Some(Vec::new()) };
        let list = first.span.to(last.span);
        if hidden {
            self.diags.error(
                Code::TYPE_MISMATCH,
                list,
                format!("`{what}` has an `impl` parameter, whose type is inferred, so it takes no type arguments"),
            );
            return None;
        }
        if explicit.len() != params.len() {
            let plural = if params.len() == 1 { "" } else { "s" };
            let given = if explicit.len() == 1 { "was" } else { "were" };
            self.diags.error(
                Code::TYPE_MISMATCH,
                list,
                format!("`{what}` takes {} type argument{plural}, but {} {given} given", params.len(), explicit.len()),
            );
            return None;
        }
        let mut out = Vec::new();
        for ((name, bs), arg) in params.iter().zip(bounds).zip(explicit) {
            // A const parameter takes a value (CG2), not a type.
            let ty = match crate::globals::Globals::const_param_ty(bs) {
                Some(declared) => self.globals.eval_const_arg(arg, declared, &self.subst, self.module, self.diags),
                None => self.resolve(arg),
            };
            if ty == Ty::Error {
                return None;
            }
            out.push((name.clone(), ty));
        }
        Some(out)
    }

    /// A method call on a value whose type is a type parameter: only what its bounds provide.
    pub(super) fn param_method_call(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        if name.name == "clone" && args.is_empty() && self.param_has(&recv.ty, Bound::Clone) {
            let ty = recv.ty.clone();
            return mk(tir::ExprKind::Method(Method::Clone, vec![recv]), ty, span);
        }
        if name.name == "hash" && args.is_empty() && self.param_has(&recv.ty, Bound::Hash) {
            return mk(tir::ExprKind::Method(Method::Hash, vec![recv]), Ty::Int(IntTy::U64), span);
        }
        if name.name == "hash_from" && self.param_has(&recv.ty, Bound::Hash) {
            return self.hash_from_call(recv, args, span);
        }
        if name.name == "cmp"
            && args.len() == 1
            && self.param_has(&recv.ty, Bound::Ord)
            && let Some(g) = self.globals.std_generic("ord_cmp")
        {
            return self.generic_call_with_receiver(g, recv, "cmp", args, span);
        }
        // `sum`, `product`, `min`, `max`, `eq`, and `cmp` of an iterator are compiler-known or functions in `std`,
        // not interface methods, so they apply to a type parameter bounded by `Iterator` here.
        if (matches!(name.name.as_str(), "sum" | "product") || self.globals.iterator_item_fn(&name.name).is_some())
            && let Some(e) = self.iterator_method(recv.clone(), name, explicit, args, span, expect)
        {
            return e;
        }
        let found = self.globals.bound_method(&recv.ty, &name.name, &self.bounds);
        let Some((sig, _, is_unsafe)) = found.filter(|(sig, _, _)| sig.has_self) else {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                name.span,
                format!("`{}` has no method `{}` among the methods its bounds provide", recv.ty, name.name),
            );
            return self.args_then_error(args, span);
        };
        if is_unsafe && self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                name.span,
                format!("calling the unsafe method `{}` requires an `unsafe` block", name.name),
            );
        }
        let (receiver, rest, ret) = (sig.params[0].clone(), sig.params[1..].to_vec(), sig.ret.clone());
        let generic = self.globals.bound_method_generics(&recv.ty, &name.name, &self.bounds);
        let receiver_place = Some((&recv, receiver.exclusive));
        let checked = match generic {
            Some(generic) => self.generic_bound_args(
                &name.name,
                (&rest, ret),
                explicit,
                args,
                receiver_place,
                (span, expect),
                &generic,
            ),
            // A method that is not generic takes no type arguments (TA5).
            None if !explicit.is_empty() => return self.reject_type_args(&name.name, explicit, args, span),
            None => self.args(&name.name, &rest, args, receiver_place, span).map(|c| (c, ret)),
        };
        let Some((mut checked, ret)) = checked else { return error_expr(span) };
        checked.insert(0, recv);
        mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span)
    }

    /// The arguments of a call to a method that a bound provides and that has type parameters of its own
    /// (`C.from_iter(it)` with `C: FromIterator<T>`). Inside generic code there is no instance to call, so the
    /// method's parameters are matched against the argument types and replaced in the result type; the bounds of
    /// those parameters are checked where the call is instantiated.
    #[allow(clippy::too_many_arguments)]
    fn generic_bound_args(
        &mut self,
        name: &str,
        (params, ret): (&[ParamSig], Ty),
        explicit: &[ast::Type],
        args: &[ast::Arg],
        receiver_place: Option<(&tir::Expr, bool)>,
        (span, expect): (Span, Option<&Ty>),
        (own, bounds, base): &BoundMethodGenerics,
    ) -> Option<(Vec<tir::Expr>, Ty)> {
        if args.len() != params.len() {
            let _ = self.args(name, params, args, receiver_place, span);
            return None;
        }
        let globals = self.globals;
        // The bound's arguments already stand for the interface's parameters, so a closure bound written with them
        // (`F: FnMut(T): U`) is read with them too.
        let mut bindings: Vec<(String, Ty)> = base.clone();
        // Explicit type arguments are bindings from the start (TA3), and each one must meet the bounds of its
        // parameter, as a call on a concrete receiver checks them (`collect<C>` needs `C: FromIterator<T>`).
        if !explicit.is_empty() {
            let Some(given) = self.explicit_bindings(name, own, bounds, false, explicit) else {
                let _ = self.args(name, params, args, receiver_place, span);
                return None;
            };
            bindings.extend(given.iter().cloned());
            let mut ok = true;
            for ((pname, t), bs) in given.iter().zip(bounds) {
                for b in bs {
                    if !globals.satisfies_with(t, b, &self.bounds, &bindings) {
                        let (code, message) =
                            globals.unmet_bound(t, b, &bindings, &self.bounds, &format!("`{pname}` of `{name}`"));
                        self.diags.error(code, span, message);
                        ok = false;
                    }
                }
            }
            if !ok {
                let _ = self.args(name, params, args, receiver_place, span);
                return None;
            }
        }
        let mut written = Vec::new();
        for (a, p) in args.iter().zip(params) {
            let hint = generic_hint(globals, &p.ty, own, bounds, &bindings);
            let e = self.expr_arg(&a.expr, hint.as_ref(), p.mut_param);
            globals.unify(&p.ty, &e.ty, &mut bindings);
            written.push(e);
        }
        if let Some(expected) = expect {
            globals.unify(&ret, expected, &mut bindings);
        }
        // A method's type parameters that appear only in a closure bound (`fn pair<U, F: FnMut(T): U>`) or in an
        // `Iterator` bound are found from the arguments, as a call on a concrete receiver finds them.
        let per_arg: Vec<Option<tir::Expr>> = written.iter().cloned().map(Some).collect();
        bound_fn_inference(globals, own, bounds, params, &per_arg, &mut bindings);
        bound_iterator_inference(globals, own, bounds, &self.bounds, &mut bindings);
        for pname in own {
            if !bindings.iter().any(|(n, _)| n == pname) {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("cannot tell what `{pname}` stands for in this call to `{name}`"),
                );
                return None;
            }
        }
        let params: Vec<ParamSig> = params.iter().map(|p| globals.subst_param(p, &bindings)).collect();
        let ret = globals.subst_ty(&ret, &bindings);
        let checked = self.args_pre(name, &params, args, receiver_place, span, Some(written))?;
        Some((checked, ret))
    }

    /// `T.function(...)` on a type parameter.
    pub(super) fn param_assoc_call(
        &mut self,
        ty: &Ty,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        if name.name == "default" && args.is_empty() && self.param_has(ty, Bound::Default) {
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, Vec::new()), ty.clone(), span);
        }
        if name.name == "from"
            && args.len() == 1
            && let Some(call) = self.param_from_call(ty, args, span)
        {
            return call;
        }
        if name.name == "try_from"
            && args.len() == 1
            && let Some(call) = self.param_try_from_call(ty, args, span)
        {
            return call;
        }
        let found = self.globals.bound_method(ty, &name.name, &self.bounds);
        let Some((sig, _, is_unsafe)) = found.filter(|(sig, _, _)| !sig.has_self) else {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                name.span,
                format!("`{ty}` has no function `{}` among the functions its bounds provide", name.name),
            );
            return self.args_then_error(args, span);
        };
        if is_unsafe && self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                name.span,
                format!("calling the unsafe function `{}` requires an `unsafe` block", name.name),
            );
        }
        let generic = self.globals.bound_method_generics(ty, &name.name, &self.bounds);
        let checked = match generic {
            Some(generic) => {
                self.generic_bound_args(&name.name, (&sig.params, sig.ret), &[], args, None, (span, expect), &generic)
            }
            None => self.args(&name.name, &sig.params, args, None, span).map(|c| (c, sig.ret)),
        };
        let Some((checked, ret)) = checked else { return error_expr(span) };
        mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span)
    }

    /// `T.from(v)` in a generic body: `T` has a `From<S>` bound and `v` is of type `S` (FR7). The call is a placeholder
    /// typed `T`, as the bound's other functions are. `None` when `T` has no `From` bound, so the caller reports the
    /// missing function.
    fn param_from_call(&mut self, ty: &Ty, args: &[ast::Arg], span: Span) -> Option<tir::Expr> {
        let Ty::Param(param) = ty else { return None };
        let sources: Vec<Ty> = self
            .bounds
            .iter()
            .filter(|(name, _)| name.as_str() == &**param)
            .flat_map(|(_, bounds)| bounds.iter())
            .filter_map(|bound| match bound {
                Bound::From(source) => Some(source.clone()),
                _ => None,
            })
            .collect();
        if sources.is_empty() {
            return None;
        }
        let expect = match sources.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        };
        let value = self.expr(&args[0].expr, expect.as_ref());
        if value.ty == Ty::Error {
            return Some(error_expr(span));
        }
        if !sources.contains(&value.ty) {
            let listed: Vec<String> = sources.iter().map(|s| format!("`{s}`")).collect();
            let message = format!("`{ty}` converts from {}, not from `{}`", listed.join(" or "), value.ty);
            self.diags.error(Code::TYPE_MISMATCH, value.span, message);
            return Some(error_expr(span));
        }
        Some(mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![value]), ty.clone(), span))
    }

    /// `T.try_from(v)` in a generic body: `T` has a `TryFrom<S>` bound and `v` is of type `S` (TF7). The result is
    /// `Result<T, <T as TryFrom<S>>.Error>`, the error staying symbolic until `T` is concrete (TF9). `None` when `T`
    /// has no `TryFrom` bound, so the caller reports the missing function.
    fn param_try_from_call(&mut self, ty: &Ty, args: &[ast::Arg], span: Span) -> Option<tir::Expr> {
        let Ty::Param(param) = ty else { return None };
        let sources: Vec<Ty> = self
            .bounds
            .iter()
            .filter(|(name, _)| name.as_str() == &**param)
            .flat_map(|(_, bounds)| bounds.iter())
            .filter_map(|bound| match bound {
                Bound::TryFrom(source) => Some(source.clone()),
                _ => None,
            })
            .collect();
        if sources.is_empty() {
            return None;
        }
        let expect = match sources.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        };
        let value = self.expr(&args[0].expr, expect.as_ref());
        if value.ty == Ty::Error {
            return Some(error_expr(span));
        }
        if !sources.contains(&value.ty) {
            let listed: Vec<String> = sources.iter().map(|s| format!("`{s}`")).collect();
            let message = format!("`{ty}` converts from {}, not from `{}`", listed.join(" or "), value.ty);
            self.diags.error(Code::TYPE_MISMATCH, value.span, message);
            return Some(error_expr(span));
        }
        let err = Ty::TryFromError { self_ty: Box::new(ty.clone()), source: Box::new(value.ty.clone()) };
        let ret = Ty::Result(Box::new(ty.clone()), Box::new(err));
        Some(mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![value]), ret, span))
    }

    /// The instance of generic type `g` once the arguments are known: from `bindings`, and from the expected type
    /// for what is still open.
    pub(super) fn template_instance(
        &mut self,
        g: usize,
        mut bindings: Vec<(String, Ty)>,
        expect: Option<&Ty>,
        span: Span,
    ) -> Option<Ty> {
        let globals = self.globals;
        let info = &globals.generic_adts[g];
        if let Some(expected) = expect {
            let (is_enum, id) = info.identity;
            let identity = if is_enum {
                Ty::Enum(globals.enum_refs[id].clone())
            } else {
                Ty::Struct(globals.struct_refs[id].clone())
            };
            globals.unify(&identity, expected, &mut bindings);
        }
        let mut args = Vec::new();
        let mut env: Vec<(String, Ty)> = bindings.clone();
        for (i, p) in info.params.iter().enumerate() {
            let found = bindings.iter().find(|(n, _)| n == p).map(|(_, t)| t.clone());
            let ty = match found {
                Some(t) => t,
                None => match info.defaults.get(i).and_then(|d| d.clone()) {
                    Some(default) => {
                        let ty = globals.resolve_type_in(&default, None, &env, info.module, self.diags);
                        bindings.push((p.clone(), ty.clone()));
                        ty
                    }
                    None => {
                        // The parameters nothing supplies (not bound by the arguments or the expected type, and
                        // without a default), and every parameter the written form needs.
                        let has_default = |j: usize| info.defaults.get(j).is_some_and(Option::is_some);
                        let missing: Vec<&String> = info
                            .params
                            .iter()
                            .enumerate()
                            .filter(|(j, q)| !has_default(*j) && !bindings.iter().any(|(n, _)| n == *q))
                            .map(|(_, q)| q)
                            .collect();
                        let required: Vec<&String> =
                            info.params.iter().enumerate().filter(|(j, _)| !has_default(*j)).map(|(_, q)| q).collect();
                        generic_args_missing(&info.name, &missing, &required, span, self.diags);
                        return None;
                    }
                },
            };
            args.push(ty.clone());
            env.push((p.clone(), ty));
        }
        if !globals.check_adt_bounds(g, &args, span, self.diags) {
            return Some(Ty::Error);
        }
        Some(globals.adt_instance_ty(g, args))
    }

    /// `Stack.new()` and `Stack.Node(value, rest)` on a generic type written without arguments.
    pub(super) fn assoc_call_template(
        &mut self,
        g: usize,
        name: &ast::Ident,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let globals = self.globals;
        let (is_enum, id) = globals.generic_adts[g].identity;
        if is_enum && let Some(vi) = globals.enums[id].variants.iter().position(|v| v.name == name.name) {
            // A variant takes no type arguments (TA5).
            if !explicit.is_empty() {
                return self.reject_type_args(&name.name, explicit, args, span);
            }
            let v = &globals.enums[id].variants[vi];
            if v.shape != tir::VariantShape::Tuple {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("variant `{}` is not built with `(...)`", name.name),
                );
                return self.args_then_error(args, span);
            }
            if v.fields.len() != args.len() {
                let params: Vec<ParamSig> = v.fields.iter().map(|f| ParamSig::plain(f.ty.clone())).collect();
                let _ = self.args(&name.name, &params, args, None, span);
                return error_expr(span);
            }
            let mut checked = Vec::new();
            for (a, f) in args.iter().zip(&v.fields) {
                let hint = if globals.ty_has_param(&f.ty) {
                    let bound_param = match &f.ty {
                        Ty::Param(name) => globals.generic_adts[g].params.iter().position(|n| n == &**name),
                        _ => None,
                    };
                    bound_param
                        .and_then(|k| globals.generic_adts[g].bounds.get(k))
                        .and_then(|bounds| super::closure::bound_hint(globals, bounds))
                } else {
                    Some(f.ty.clone())
                };
                checked.push(self.expr(&a.expr, hint.as_ref()));
            }
            let mut bindings = Vec::new();
            for (e, f) in checked.iter().zip(&v.fields) {
                globals.unify(&f.ty, &e.ty, &mut bindings);
            }
            let Some(ty) = self.template_instance(g, bindings, expect, span) else { return error_expr(span) };
            let Ty::Enum(a) = &ty else { return error_expr(span) };
            let params: Vec<ParamSig> =
                globals.enums[a.id].variants[vi].fields.iter().map(|f| ParamSig::plain(f.ty.clone())).collect();
            let Some(checked) = self.args_pre(&name.name, &params, args, None, span, Some(checked)) else {
                return error_expr(span);
            };
            let fields = checked.into_iter().enumerate().collect();
            return mk(tir::ExprKind::Variant(vi, fields), ty, span);
        }
        // An associated function: its type arguments come from its own parameters first, then from the expected
        // type (issue #65).
        let (is_enum, id) = globals.generic_adts[g].identity;
        let identity = if is_enum { globals.enum_refs[id].clone() } else { globals.struct_refs[id].clone() };
        let key = (is_enum, id);
        let method = globals.methods.get(&(key, name.name.clone())).copied();
        // Only a method with type parameters of its own takes type arguments (TA5).
        if !explicit.is_empty() && !method.is_some_and(|f| globals.method_templates.contains_key(&f)) {
            return self.reject_type_args(&name.name, explicit, args, span);
        }
        let Some(f) = method else {
            let Some(ty) = self.template_instance(g, Vec::new(), expect, span) else {
                return self.args_then_error(args, span);
            };
            if ty == Ty::Error {
                return self.args_then_error(args, span);
            }
            return self.assoc_call(&ty, name, explicit, args, span, expect);
        };
        let pattern = globals.sigs[f.0].clone();
        if pattern.has_self {
            self.diags.error(
                Code::UNKNOWN_NAME,
                name.span,
                format!("`{}` has a `self` parameter; call it as `{}.{}(...)`", name.name, identity.name, name.name),
            );
            return self.args_then_error(args, span);
        }
        if args.len() != pattern.params.len() {
            let _ = self.args(&name.name, &pattern.params, args, None, span);
            return error_expr(span);
        }
        let mut checked = Vec::new();
        let info = &globals.generic_adts[g];
        for (a, p) in args.iter().zip(&pattern.params) {
            // A closure argument to a type parameter uses the type's `Fn` bound (CL9).
            let hint = if globals.ty_has_param(&p.ty) {
                let bound_param = match &p.ty {
                    Ty::Param(name) => info.params.iter().position(|n| n == &**name),
                    _ => None,
                };
                bound_param
                    .and_then(|k| info.bounds.get(k))
                    .and_then(|bounds| super::closure::bound_hint(globals, bounds))
            } else {
                Some(p.ty.clone())
            };
            checked.push(self.expr(&a.expr, hint.as_ref()));
        }
        let mut bindings = Vec::new();
        for (e, p) in checked.iter().zip(&pattern.params) {
            globals.unify(&p.ty, &e.ty, &mut bindings);
        }
        let Some(ty) = self.template_instance(g, bindings, expect, span) else { return error_expr(span) };
        if ty == Ty::Error {
            return error_expr(span);
        }
        let Some(actual) = crate::globals::adt_key(&ty) else { return error_expr(span) };
        let Some(&func) = globals.methods.get(&(actual, name.name.clone())) else { return error_expr(span) };
        if let Some(info) = globals.method_templates.get(&func).cloned() {
            return self.generic_assoc_method_call(info, &name.name, explicit, args, Some(checked), span, expect);
        }
        let sig = globals.sigs[func.0].clone();
        let Some(checked) = self.args_pre(&name.name, &sig.params, args, None, span, Some(checked)) else {
            return error_expr(span);
        };
        mk(tir::ExprKind::Call(func, checked), sig.ret, span)
    }

    /// `Wrapper { inner: value }` for a generic struct: the type arguments come from the field values.
    pub(super) fn generic_struct_literal(
        &mut self,
        g: usize,
        fields: &[ast::FieldInit],
        base: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let globals = self.globals;
        let info = &globals.generic_adts[g];
        if info.is_enum {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("`{}` is an enum, not a struct", info.name));
            return error_expr(span);
        }
        let defs = &globals.structs[info.identity.1].fields;
        let from_other_package = globals.package_of(info.module) != self.package();
        let mut given = vec![false; defs.len()];
        let mut checked: Vec<(usize, tir::Expr)> = Vec::new();
        for f in fields {
            let shorthand;
            let value = match &f.value {
                Some(v) => v,
                None => {
                    shorthand = ast::Expr {
                        kind: ast::ExprKind::Name { name: f.name.clone(), args: Vec::new() },
                        span: f.name.span,
                    };
                    &shorthand
                }
            };
            let Some(index) = defs.iter().position(|d| d.name == f.name.name) else {
                let available: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
                let refs: Vec<&str> = available.iter().map(String::as_str).collect();
                self.report_no_field(
                    format!("`{}` has no field `{}`", info.name, f.name.name),
                    &f.name.name,
                    f.name.span,
                    &refs,
                );
                self.expr(value, None);
                continue;
            };
            if std::mem::replace(&mut given[index], true) {
                self.diags.error(Code::TYPE_MISMATCH, f.name.span, format!("field `{}` is given twice", f.name.name));
            }
            if from_other_package && defs[index].is_priv {
                self.diags.error(
                    Code::PRIVATE_ITEM,
                    f.name.span,
                    format!("field `{}` of `{}` is `priv` in another package", f.name.name, info.name),
                );
            }
            let hint = if globals.ty_has_param(&defs[index].ty) {
                let bound_param = match &defs[index].ty {
                    Ty::Param(name) => info.params.iter().position(|n| n == &**name),
                    _ => None,
                };
                bound_param
                    .and_then(|k| info.bounds.get(k))
                    .and_then(|bounds| super::closure::bound_hint(globals, bounds))
            } else {
                Some(defs[index].ty.clone())
            };
            let v = self.expr(value, hint.as_ref());
            checked.push((index, v));
        }
        // A `String` value where the expected instance has a `str` field is borrowed as a `str` (IC1), so the
        // argument the field gives is `str`, not `String`.
        if let Some(Ty::Struct(want)) = expect {
            let wanted = &globals.structs[want.id].fields;
            for (i, v) in &mut checked {
                if v.ty == Ty::String
                    && wanted.len() == defs.len()
                    && wanted[*i].name == defs[*i].name
                    && wanted[*i].ty == Ty::Str
                {
                    let taken = std::mem::replace(v, error_expr(span));
                    *v = self.coerce_arg(taken, &Ty::Str);
                }
            }
        }
        let mut bindings = Vec::new();
        for (i, v) in &checked {
            globals.unify(&defs[*i].ty, &v.ty, &mut bindings);
        }
        let Some(ty) = self.template_instance(g, bindings, expect, span) else { return error_expr(span) };
        let Ty::Struct(a) = &ty else { return error_expr(span) };
        let actual = &globals.structs[a.id].fields;
        let checked: Vec<(usize, tir::Expr)> = checked
            .into_iter()
            .map(|(i, v)| {
                let v = self.coerce_arg(v, &actual[i].ty);
                (i, v)
            })
            .collect();
        if let Some(base) = base {
            let from_other_package = globals.package_of(info.module) != self.package();
            return self.struct_update(&ty, actual, checked, base, from_other_package, span);
        }
        let missing: Vec<&str> =
            actual.iter().zip(&given).filter(|(_, g)| !**g).map(|(d, _)| d.name.as_str()).collect();
        if !missing.is_empty() {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` is missing the field(s) `{}`", info.name, missing.join("`, `")),
            );
        }
        mk(tir::ExprKind::Struct(checked), ty, span)
    }

    /// `Shape.Rect { w: 2, h: 3 }` for a struct-like variant of a generic enum: the type arguments come from the field
    /// values and the expected type, as for a tuple variant (`assoc_call_template`). A name that is not a variant, or
    /// a variant without named fields, is reported and the fields are still checked.
    pub(super) fn generic_variant_literal(
        &mut self,
        g: usize,
        name: &ast::Ident,
        fields: &[ast::FieldInit],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let globals = self.globals;
        let info = &globals.generic_adts[g];
        let id = info.identity.1;
        let written = format!("{}.{}", info.name, name.name);
        let Some(vi) = globals.enums[id].variants.iter().position(|v| v.name == name.name) else {
            let variants: Vec<String> = globals.enums[id].variants.iter().map(|v| v.name.clone()).collect();
            let refs: Vec<&str> = variants.iter().map(String::as_str).collect();
            self.report_no_variant(format!("unknown variant `{written}`"), &name.name, name.span, &refs);
            self.check_field_values_untyped(fields);
            return error_expr(span);
        };
        if globals.enums[id].variants[vi].shape != tir::VariantShape::Struct {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("variant `{written}` has no named fields"));
            self.check_field_values_untyped(fields);
            return error_expr(span);
        }
        let defs: Vec<tir::FieldDef> = globals.enums[id].variants[vi].fields.clone();
        let from_other_package = globals.package_of(info.module) != self.package();
        let mut given = vec![false; defs.len()];
        let mut checked: Vec<(usize, tir::Expr)> = Vec::new();
        for f in fields {
            let shorthand;
            let value = match &f.value {
                Some(v) => v,
                None => {
                    shorthand = ast::Expr {
                        kind: ast::ExprKind::Name { name: f.name.clone(), args: Vec::new() },
                        span: f.name.span,
                    };
                    &shorthand
                }
            };
            let Some(index) = defs.iter().position(|d| d.name == f.name.name) else {
                let available: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
                let refs: Vec<&str> = available.iter().map(String::as_str).collect();
                self.report_no_field(
                    format!("`{written}` has no field `{}`", f.name.name),
                    &f.name.name,
                    f.name.span,
                    &refs,
                );
                self.expr(value, None);
                continue;
            };
            if std::mem::replace(&mut given[index], true) {
                self.diags.error(Code::TYPE_MISMATCH, f.name.span, format!("field `{}` is given twice", f.name.name));
            }
            if from_other_package && defs[index].is_priv {
                self.diags.error(
                    Code::PRIVATE_ITEM,
                    f.name.span,
                    format!("field `{}` of `{written}` is `priv` in another package", f.name.name),
                );
            }
            let hint = if globals.ty_has_param(&defs[index].ty) {
                let bound_param = match &defs[index].ty {
                    Ty::Param(name) => info.params.iter().position(|n| n == &**name),
                    _ => None,
                };
                bound_param
                    .and_then(|k| info.bounds.get(k))
                    .and_then(|bounds| super::closure::bound_hint(globals, bounds))
            } else {
                Some(defs[index].ty.clone())
            };
            let v = self.expr(value, hint.as_ref());
            checked.push((index, v));
        }
        let mut bindings = Vec::new();
        for (i, v) in &checked {
            globals.unify(&defs[*i].ty, &v.ty, &mut bindings);
        }
        let Some(ty) = self.template_instance(g, bindings, expect, span) else { return error_expr(span) };
        let Ty::Enum(a) = &ty else { return error_expr(span) };
        let actual = globals.enums[a.id].variants[vi].fields.clone();
        let checked: Vec<(usize, tir::Expr)> = checked
            .into_iter()
            .map(|(i, v)| {
                let v = self.coerce_arg(v, &actual[i].ty);
                (i, v)
            })
            .collect();
        let missing: Vec<&str> =
            actual.iter().zip(&given).filter(|(_, g)| !**g).map(|(d, _)| d.name.as_str()).collect();
        if !missing.is_empty() {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{written}` is missing the field(s) `{}`", missing.join("`, `")),
            );
        }
        mk(tir::ExprKind::Variant(vi, checked), ty, span)
    }

    /// Checks the values of a struct literal that cannot be typed (an unknown or non-struct target), so that their own
    /// errors are still reported. A shorthand field checks the local or name it names.
    fn check_field_values_untyped(&mut self, fields: &[ast::FieldInit]) {
        for f in fields {
            match &f.value {
                Some(v) => {
                    self.expr(v, None);
                }
                None => {
                    let shorthand = ast::Expr {
                        kind: ast::ExprKind::Name { name: f.name.clone(), args: Vec::new() },
                        span: f.name.span,
                    };
                    self.expr(&shorthand, None);
                }
            }
        }
    }

    /// The value of `Type.default()` for a type that derives `Default`.
    pub(super) fn default_expr(&self, ty: &Ty, span: Span) -> tir::Expr {
        use tir::ExprKind as K;
        let kind = match ty {
            Ty::Unit => K::Unit,
            Ty::Bool => K::Bool(false),
            Ty::Char => K::Char('\0'),
            Ty::Int(_) => K::Int(0),
            Ty::Float(_) => K::Float(0.0),
            Ty::Str => K::Str(String::new()),
            Ty::String => K::Builtin(Builtin::StringNew, Vec::new()),
            Ty::List(_, alloc) if alloc.is_global() => K::Builtin(Builtin::ListNew, Vec::new()),
            Ty::Map(_, _, alloc) if alloc.is_global() => K::Builtin(Builtin::MapNew, Vec::new()),
            Ty::Option(_) => K::None,
            Ty::Tuple(ts) => K::Tuple(ts.iter().map(|t| self.default_expr(t, span)).collect()),
            Ty::Struct(a) => K::Struct(
                self.globals.structs[a.id]
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (i, self.default_expr(&f.ty, span)))
                    .collect(),
            ),
            _ => return error_expr(span),
        };
        mk(kind, ty.clone(), span)
    }

    /// A call to a generic method (`it.map(|x| ...)`): infer its type arguments, check its bounds, and call the
    /// instance for them. `info` is the registered template; `recv` is the already-checked receiver.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn generic_method_call(
        &mut self,
        info: GenericMethod,
        recv: tir::Expr,
        name: &str,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let globals = self.globals;
        let pattern = globals.sigs[info.identity.0].clone();
        let params = info.params.clone();
        let bounds = info.bounds.clone();
        let Some(slots) = self.arg_slots(name, &pattern.params[1..], args, span) else {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        };
        let receiver = pattern.params[0].clone();
        if receiver.by_ref && receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{name}` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        if receiver.by_ref
            && receiver.exclusive
            && let Some(whose) = self.slice_receiver_problem(&recv)
        {
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{name}` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        // Explicit type arguments are bindings from the start, so inference keeps them (TA3).
        let mut bindings = match self.explicit_bindings(name, &params, &bounds, info.hidden, explicit) {
            Some(bindings) => bindings,
            None => return self.args_then_error(args, span),
        };
        globals.unify(&receiver.ty, &recv.ty, &mut bindings);
        let mut checked: Vec<Option<tir::Expr>> = (0..args.len()).map(|_| None).collect();
        for (i, p) in pattern.params[1..].iter().enumerate() {
            let Some(ai) = slots[i] else { continue };
            let hint = generic_hint(globals, &p.ty, &params, &bounds, &bindings);
            let e = self.expr_arg(&args[ai].expr, hint.as_ref(), p.mut_param);
            globals.unify(&p.ty, &e.ty, &mut bindings);
            checked[ai] = Some(e);
        }
        let per_param: Vec<Option<tir::Expr>> =
            slots.iter().map(|slot| slot.and_then(|ai| checked[ai].clone())).collect();
        bound_fn_inference(globals, &params, &bounds, &pattern.params[1..], &per_param, &mut bindings);
        if let Some(expected) = expect {
            globals.unify(&pattern.ret, expected, &mut bindings);
        }
        bound_iterator_inference(globals, &params, &bounds, &self.bounds, &mut bindings);
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let mut targs = Vec::new();
        for pname in &params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => return inference_hole(name, &params, &bounds, pname, span, self.diags),
            }
        }
        if targs.contains(&Ty::Error) {
            return error_expr(span);
        }
        let mut subst = bindings.clone();
        subst.extend(params.iter().cloned().zip(targs.iter().cloned()));
        let mut ok = true;
        for (k, t) in targs.iter().enumerate() {
            for b in &bounds[k] {
                if !globals.satisfies_with(t, b, &self.bounds, &subst) {
                    let (code, message) =
                        globals.unmet_bound(t, b, &subst, &self.bounds, &format!("`{}` of `{name}`", params[k]));
                    self.diags.error(code, span, message);
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        // Only the substituted signature decides: the identity's parameters are `Ty::Param` in every template.
        let symbolic = self.symbolic
            || targs.iter().any(|t| globals.ty_has_param(t))
            || globals.ty_has_param(&recv.ty)
            || globals.ty_has_param(&globals.subst_ty(&pattern.ret, &subst));
        if symbolic {
            let rest: Vec<ParamSig> = pattern.params[1..].iter().map(|p| globals.subst_param(p, &subst)).collect();
            let ret = globals.subst_ty(&pattern.ret, &subst);
            let receiver_place = Some((&recv, receiver.exclusive));
            let Some(mut checked) = self.args_pre(name, &rest, args, receiver_place, span, Some(written)) else {
                return error_expr(span);
            };
            checked.insert(0, recv);
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span);
        }
        let Some(&f) = globals.method_instances.get(&(info.template, targs.clone())) else {
            globals.requests.borrow_mut().push(Request::Method(info.template, targs));
            return error_expr(span);
        };
        let sig = globals.sigs[f.0].clone();
        let rest = sig.params[1..].to_vec();
        let receiver_place = Some((&recv, receiver.exclusive));
        let Some(mut checked) = self.args_pre(name, &rest, args, receiver_place, span, Some(written)) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Call(f, checked), sig.ret, span)
    }

    /// A call to an associated function that has type parameters of its own (`Set<i32>.from([1, 2])`,
    /// `Set<i32>.from_iter(it)`): the arguments are checked first, the type arguments come from them and then from the
    /// expected type, and the call goes to the instance for those arguments. `pre` holds arguments that are already
    /// checked, one per written argument.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn generic_assoc_method_call(
        &mut self,
        info: GenericMethod,
        name: &str,
        explicit: &[ast::Type],
        args: &[ast::Arg],
        pre: Option<Vec<tir::Expr>>,
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        let globals = self.globals;
        let pattern = globals.sigs[info.identity.0].clone();
        let params = info.params.clone();
        let bounds = info.bounds.clone();
        let Some(slots) = self.arg_slots(name, &pattern.params, args, span) else {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        };
        // Explicit type arguments are bindings from the start (TA3). When `pre` is set the arguments were checked.
        let mut bindings = match self.explicit_bindings(name, &params, &bounds, info.hidden, explicit) {
            Some(bindings) => bindings,
            None if pre.is_some() => return error_expr(span),
            None => return self.args_then_error(args, span),
        };
        let mut checked: Vec<Option<tir::Expr>> = (0..args.len()).map(|_| None).collect();
        for (i, p) in pattern.params.iter().enumerate() {
            let Some(ai) = slots[i] else { continue };
            let hint = generic_hint(globals, &p.ty, &params, &bounds, &bindings);
            let e = match pre.as_ref().and_then(|pre| pre.get(ai)) {
                Some(e) => e.clone(),
                None => self.expr_arg(&args[ai].expr, hint.as_ref(), p.mut_param),
            };
            globals.unify(&p.ty, &e.ty, &mut bindings);
            checked[ai] = Some(e);
        }
        let per_param: Vec<Option<tir::Expr>> =
            slots.iter().map(|slot| slot.and_then(|ai| checked[ai].clone())).collect();
        bound_fn_inference(globals, &params, &bounds, &pattern.params, &per_param, &mut bindings);
        if let Some(expected) = expect {
            globals.unify(&pattern.ret, expected, &mut bindings);
        }
        bound_iterator_inference(globals, &params, &bounds, &self.bounds, &mut bindings);
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let mut targs = Vec::new();
        for pname in &params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => return inference_hole(name, &params, &bounds, pname, span, self.diags),
            }
        }
        if targs.contains(&Ty::Error) {
            return error_expr(span);
        }
        let mut subst = bindings.clone();
        subst.extend(params.iter().cloned().zip(targs.iter().cloned()));
        let mut ok = true;
        for (k, t) in targs.iter().enumerate() {
            for b in &bounds[k] {
                if !globals.satisfies_with(t, b, &self.bounds, &subst) {
                    let (code, message) =
                        globals.unmet_bound(t, b, &subst, &self.bounds, &format!("`{}` of `{name}`", params[k]));
                    self.diags.error(code, span, message);
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        let symbolic = self.symbolic || targs.iter().any(|t| globals.ty_has_param(t));
        if symbolic {
            let ps: Vec<ParamSig> = pattern.params.iter().map(|p| globals.subst_param(p, &subst)).collect();
            let ret = globals.subst_ty(&pattern.ret, &subst);
            let Some(checked) = self.args_pre(name, &ps, args, None, span, Some(written)) else {
                return error_expr(span);
            };
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span);
        }
        let Some(&f) = globals.method_instances.get(&(info.template, targs.clone())) else {
            globals.requests.borrow_mut().push(Request::Method(info.template, targs));
            return error_expr(span);
        };
        let sig = globals.sigs[f.0].clone();
        let Some(checked) = self.args_pre(name, &sig.params, args, None, span, Some(written)) else {
            return error_expr(span);
        };
        mk(tir::ExprKind::Call(f, checked), sig.ret, span)
    }

    /// A call to a generic function whose first argument is an already-checked receiver (`xs.map(f)` calls the
    /// eager `List` helper `list_map(xs, f)`).
    pub(super) fn generic_call_with_receiver(
        &mut self,
        g: usize,
        recv: tir::Expr,
        name: &str,
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        self.generic_call_with_receiver_seeded(g, recv, name, args, &[], span)
    }

    /// Like [`Self::generic_call_with_receiver`], with type arguments the caller already knows. A parameter that
    /// appears only in a bound (`I: Iterator<T>`) cannot be inferred from the arguments, so the iterator
    /// helpers pass the item type here.
    pub(super) fn generic_call_with_receiver_seeded(
        &mut self,
        g: usize,
        recv: tir::Expr,
        name: &str,
        args: &[ast::Arg],
        seed: &[(String, Ty)],
        span: Span,
    ) -> tir::Expr {
        let globals = self.globals;
        let info = &globals.generic_fns[g];
        let pattern = globals.sigs[info.identity.0].clone();
        let params = info.params.clone();
        let bounds = info.bounds.clone();
        if info.is_unsafe && self.unsafe_depth == 0 {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                format!("calling the unsafe function `{name}` requires an `unsafe` block"),
            );
        }
        let Some(slots) = self.arg_slots(name, &pattern.params[1..], args, span) else {
            for a in args {
                self.expr(&a.expr, None);
            }
            return error_expr(span);
        };
        let receiver = pattern.params[0].clone();
        if receiver.by_ref && receiver.exclusive && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{name}` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        if receiver.by_ref
            && receiver.exclusive
            && let Some(whose) = self.slice_receiver_problem(&recv)
        {
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{name}` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        let mut bindings = seed.to_vec();
        globals.unify(&receiver.ty, &recv.ty, &mut bindings);
        let mut checked: Vec<Option<tir::Expr>> = (0..args.len()).map(|_| None).collect();
        for (i, p) in pattern.params[1..].iter().enumerate() {
            let Some(ai) = slots[i] else { continue };
            let hint = generic_hint(globals, &p.ty, &params, &bounds, &bindings);
            let e = self.expr_arg(&args[ai].expr, hint.as_ref(), p.mut_param);
            globals.unify(&p.ty, &e.ty, &mut bindings);
            checked[ai] = Some(e);
        }
        let per_param: Vec<Option<tir::Expr>> =
            slots.iter().map(|slot| slot.and_then(|ai| checked[ai].clone())).collect();
        bound_fn_inference(globals, &params, &bounds, &pattern.params[1..], &per_param, &mut bindings);
        bound_iterator_inference(globals, &params, &bounds, &self.bounds, &mut bindings);
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let mut targs = Vec::new();
        for pname in &params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => return inference_hole(name, &params, &bounds, pname, span, self.diags),
            }
        }
        if targs.contains(&Ty::Error) {
            return error_expr(span);
        }
        let mut subst = bindings.clone();
        subst.extend(params.iter().cloned().zip(targs.iter().cloned()));
        let mut ok = true;
        for (k, t) in targs.iter().enumerate() {
            for b in &bounds[k] {
                if !globals.satisfies_with(t, b, &self.bounds, &subst) {
                    let (code, message) =
                        globals.unmet_bound(t, b, &subst, &self.bounds, &format!("`{}` of `{name}`", params[k]));
                    self.diags.error(code, span, message);
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        // Only the substituted signature decides: the identity's parameters are `Ty::Param` in every template.
        let symbolic = self.symbolic
            || targs.iter().any(|t| globals.ty_has_param(t))
            || globals.ty_has_param(&recv.ty)
            || globals.ty_has_param(&globals.subst_ty(&pattern.ret, &subst));
        let receiver_place = Some((&recv, receiver.exclusive && !receiver.own));
        if symbolic {
            let rest: Vec<ParamSig> = pattern.params[1..].iter().map(|p| globals.subst_param(p, &subst)).collect();
            let ret = globals.subst_ty(&pattern.ret, &subst);
            let Some(mut checked) = self.args_pre(name, &rest, args, receiver_place, span, Some(written)) else {
                return error_expr(span);
            };
            checked.insert(0, recv);
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span);
        }
        let Some(&f) = globals.fn_instances.get(&(g, targs.clone())) else {
            globals.requests.borrow_mut().push(Request::Func(g, targs));
            return error_expr(span);
        };
        let sig = globals.sigs[f.0].clone();
        let rest = sig.params[1..].to_vec();
        let Some(mut checked) = self.args_pre(name, &rest, args, receiver_place, span, Some(written)) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Call(f, checked), sig.ret, span)
    }
}

/// The hint for one argument of a generic call: a `Fn` bound on a type parameter supplies the closure's
/// signature, and everything the earlier arguments bound is substituted in.
fn generic_hint(
    globals: &crate::globals::Globals,
    param: &Ty,
    params: &[String],
    bounds: &[Vec<Bound>],
    bindings: &[(String, Ty)],
) -> Option<Ty> {
    if let Ty::Param(name) = param
        && let Some(k) = params.iter().position(|p| p.as_str() == name.as_ref())
        && let Some((index, kind)) = bounds.get(k).and_then(|bs| {
            bs.iter().find_map(|b| match b {
                Bound::Fn(i, kind) => Some((*i, *kind)),
                _ => None,
            })
        })
    {
        return Some(Ty::Fn {
            sig: Rc::new(globals.bound_fn_sig(index, bindings)),
            kind,
            borrowed: kind != FnKind::FnOnce,
            send: tir::FnSend::unknown(),
        });
    }
    // A closure parameter written as a function type with an open return (`f: Fn(i32): U` in `fn apply<U>`)
    // still supplies the closure's parameter types. The closure checker treats an open return as an inference
    // hole, so only the return may stay open: an open parameter type cannot type the closure's parameter.
    if let Ty::Fn { .. } = param {
        let hint = globals.subst_ty(param, bindings);
        if let Ty::Fn { sig, .. } = &hint
            && !sig.params.iter().any(|t| globals.ty_has_param(t))
        {
            return Some(hint);
        }
    }
    if globals.ty_has_param(param) {
        let hint = globals.subst_ty(param, bindings);
        // A hint that still names one of the callee's own open parameters (`Option<U>` before `U` is known) would
        // make the argument's literals take `U` as their type, so it is no hint at all. The parameter names are
        // found in the type's text, which spells a parameter by its name.
        let text = param.to_string();
        let words: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
        let open = params.iter().any(|p| !bindings.iter().any(|(n, _)| n == p) && words.contains(&p.as_str()));
        if open {
            return None;
        }
        return Some(hint);
    }
    Some(param.clone())
}

/// Reports a type or const argument of a call that nothing determined. A const parameter that nothing determines
/// is E0252 (`spec/const-generics.md`, CG6); a type parameter is E0003. The diagnostic's span is the call.
fn inference_hole(
    name: &str,
    params: &[String],
    bounds: &[Vec<Bound>],
    pname: &str,
    span: Span,
    diags: &mut burdin_diag::Diagnostics,
) -> tir::Expr {
    let k = params.iter().position(|p| p == pname).unwrap_or(0);
    let is_const = bounds.get(k).is_some_and(|bs| crate::globals::Globals::const_param_ty(bs).is_some());
    let (code, what) = if is_const { (Code::CONST_INFER, "const argument ") } else { (Code::TYPE_MISMATCH, "") };
    diags.error(code, span, format!("cannot tell what {what}`{pname}` stands for in this call to `{name}`"));
    error_expr(span)
}

/// Infers the method type parameters that appear only in a closure bound (`fn map<U, F: Fn(T): U>`) by matching
/// that bound against the closure the call received. The parameter that owns the bound (`F`) is already bound to
/// the closure's type, so only the parameters inside the bound (`U`) are still open.
fn bound_fn_inference(
    globals: &crate::globals::Globals,
    params: &[String],
    bounds: &[Vec<Bound>],
    pattern: &[ParamSig],
    checked: &[Option<tir::Expr>],
    bindings: &mut Vec<(String, Ty)>,
) {
    for (k, pname) in params.iter().enumerate() {
        let Some(index) = bounds.get(k).and_then(|bs| {
            bs.iter().find_map(|b| match b {
                Bound::Fn(i, _) => Some(*i),
                _ => None,
            })
        }) else {
            continue;
        };
        for (e, p) in checked.iter().zip(pattern) {
            let Some(e) = e else { continue };
            if !matches!(&p.ty, Ty::Param(n) if n.as_ref() == pname.as_str()) {
                continue;
            }
            let Ty::Fn { sig: actual, .. } = &e.ty else { continue };
            let want = globals.bound_fn_sig(index, bindings);
            for (wp, ap) in want.params.iter().zip(&actual.params) {
                globals.unify(wp, ap, bindings);
            }
            globals.unify(&want.ret, &actual.ret, bindings);
        }
    }
}

/// Infers the type parameters that appear only in an `Iterator` bound (`fn zip<U, J: Iterator<U>>`): once the
/// parameter that owns the bound (`J`) is bound to a concrete type, the item type of that type's `Iterator`
/// implementation (built in or declared) fixes the parameters inside the bound (`U`). A bound that names a
/// parameter already bound is left to the ordinary bound check.
fn bound_iterator_inference(
    globals: &crate::globals::Globals,
    params: &[String],
    bounds: &[Vec<Bound>],
    env: &crate::generics::Env,
    bindings: &mut Vec<(String, Ty)>,
) {
    let Some(iterator) = globals.iterator_iface else { return };
    // One bound can bind the owner of the next (`flatten`: `I: Iterator<J>` binds `J`, then `J: Iterator<U>` binds
    // `U`), so go around until a pass binds nothing new.
    for _ in 0..=params.len() {
        let before = bindings.len();
        for (k, pname) in params.iter().enumerate() {
            let Some(owner) = bindings.iter().find(|(n, _)| n == pname).map(|(_, t)| t.clone()) else { continue };
            let Some(bs) = bounds.get(k) else { continue };
            for b in bs {
                let Bound::User(iface, args) = b else { continue };
                if *iface != iterator || args.len() != 1 {
                    continue;
                }
                let Some(item) = globals.iterator_item(&owner, env) else { continue };
                globals.unify(&args[0], &item, bindings);
            }
        }
        if bindings.len() == before {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generics::param_ty;
    use burdin_tir::{FnSig, IntTy};

    fn i32_() -> Ty {
        Ty::Int(IntTy::I32)
    }

    fn globals() -> crate::globals::Globals {
        crate::globals::Globals { iterator_iface: Some(0), ..Default::default() }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn fn_ty(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::Fn {
            sig: Rc::new(FnSig { params, ret: Box::new(ret) }),
            kind: FnKind::Fn,
            borrowed: true,
            send: tir::FnSend::unknown(),
        }
    }

    #[test]
    fn a_closure_parameter_with_an_open_return_still_supplies_its_parameters() {
        // `fn apply<U>(self, f: Fn(i32): U)`: `U` is open, but the closure's parameter type is known.
        let param = fn_ty(vec![i32_()], param_ty("U"));
        let hint = generic_hint(&globals(), &param, &names(&["U"]), &[Vec::new()], &[]);
        let Some(Ty::Fn { sig, .. }) = hint else { panic!("expected a closure hint") };
        assert_eq!(sig.params, vec![i32_()]);
        assert_eq!(*sig.ret, param_ty("U"));
    }

    #[test]
    fn a_closure_hint_substitutes_an_impl_parameter() {
        // `Box<A> { fn apply<U>(self, f: Fn(A): U) }`: `A` comes from the receiver, `U` stays open.
        let param = fn_ty(vec![param_ty("A")], param_ty("U"));
        let bindings = vec![("A".to_string(), i32_())];
        let hint = generic_hint(&globals(), &param, &names(&["U"]), &[Vec::new()], &bindings);
        let Some(Ty::Fn { sig, .. }) = hint else { panic!("expected a closure hint") };
        assert_eq!(sig.params, vec![i32_()]);
        assert_eq!(*sig.ret, param_ty("U"));
    }

    #[test]
    fn a_closure_parameter_with_an_open_parameter_type_is_no_hint() {
        // `fn apply<U>(f: Fn(U): i32)`: `U` cannot type the closure's parameter, so the closure gets no hint.
        let param = fn_ty(vec![param_ty("U")], i32_());
        assert_eq!(generic_hint(&globals(), &param, &names(&["U"]), &[Vec::new()], &[]), None);
    }

    #[test]
    fn an_open_non_closure_type_is_still_no_hint() {
        // `fn f<U>(x: Option<U>)`: a literal must not adopt the open parameter as its type.
        let param = Ty::Option(Box::new(param_ty("U")));
        assert_eq!(generic_hint(&globals(), &param, &names(&["U"]), &[Vec::new()], &[]), None);
    }

    #[test]
    fn an_iterator_bound_binds_the_item_type() {
        // `fn zip<U, J: Iterator<U>>` called with a slice for `J`: `U` is the slice's item.
        let params = names(&["U", "J"]);
        let bounds = vec![Vec::new(), vec![Bound::User(0, vec![param_ty("U")])]];
        let mut bindings = vec![("J".to_string(), Ty::Slice(Box::new(i32_())))];
        bound_iterator_inference(&globals(), &params, &bounds, &[], &mut bindings);
        assert_eq!(bindings.iter().find(|(n, _)| n == "U").map(|(_, t)| t.clone()), Some(i32_()));
    }

    #[test]
    fn one_bound_can_feed_the_next() {
        // `fn flatten<I: Iterator<J>, J: Iterator<U>, U>`: `I` fixes `J`, and then `J` fixes `U`, even though `J` is
        // listed before `I`.
        let params = names(&["J", "I", "U"]);
        let bounds =
            vec![vec![Bound::User(0, vec![param_ty("U")])], vec![Bound::User(0, vec![param_ty("J")])], Vec::new()];
        let items = Ty::Slice(Box::new(Ty::Slice(Box::new(Ty::Char))));
        let mut bindings = vec![("I".to_string(), items)];
        bound_iterator_inference(&globals(), &params, &bounds, &[], &mut bindings);
        let found = |name: &str| bindings.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone());
        // A slice is `Copy`, so a slice of slices yields the inner slice itself, and that is `J`.
        assert!(found("J").is_some());
        assert_eq!(found("U"), Some(Ty::Char));
    }

    #[test]
    fn a_bound_on_something_that_is_not_an_iterator_binds_nothing() {
        let params = names(&["U", "J"]);
        let bounds = vec![Vec::new(), vec![Bound::User(0, vec![param_ty("U")])]];
        let mut bindings = vec![("J".to_string(), i32_())];
        bound_iterator_inference(&globals(), &params, &bounds, &[], &mut bindings);
        assert!(bindings.iter().all(|(n, _)| n != "U"), "an integer has no items");
    }
}
