//! Calls, constructors, and literals of generic functions and types, where the type arguments are inferred.

use super::{FnCx, error_expr, mk};
use crate::generics::{Bound, GenericMethod, Request, SYMBOLIC_CALL};
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
        span: Span,
        expect: Option<&Ty>,
    ) -> tir::Expr {
        self.generic_call_impl(g, id, args, None, span, expect)
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
        self.generic_call_impl(g, id, args, Some(checked), span, expect)
    }

    fn generic_call_impl(
        &mut self,
        g: usize,
        id: &ast::Ident,
        args: &[ast::Arg],
        pre: Option<Vec<tir::Expr>>,
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
        let mut bindings = Vec::new();
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
        if let Some(expected) = expect {
            globals.unify(&pattern.ret, expected, &mut bindings);
        }
        let mut targs = Vec::new();
        for pname in &info.params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => {
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
            for b in bounds {
                if !globals.satisfies_with(t, b, &self.bounds, &subst) {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{t}` does not implement `{}`, which `{pname}` of `{}` requires",
                            globals.bound_name(b),
                            id.name
                        ),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        let (params, ret, callee) = if targs.iter().any(|t| globals.ty_has_param(t)) {
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

    /// A method call on a value whose type is a type parameter: only what its bounds provide.
    pub(super) fn param_method_call(
        &mut self,
        recv: tir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> tir::Expr {
        if name.name == "clone" && args.is_empty() && self.param_has(&recv.ty, Bound::Clone) {
            let ty = recv.ty.clone();
            return mk(tir::ExprKind::Method(Method::Clone, vec![recv]), ty, span);
        }
        if name.name == "hash" && args.is_empty() && self.param_has(&recv.ty, Bound::Hash) {
            return mk(tir::ExprKind::Method(Method::Hash, vec![recv]), Ty::Int(IntTy::U64), span);
        }
        if name.name == "cmp"
            && args.len() == 1
            && self.param_has(&recv.ty, Bound::Ord)
            && let Some(g) = self.globals.std_generic("__ord_cmp")
        {
            return self.generic_call_with_receiver(g, recv, "cmp", args, span);
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
        let receiver_place = Some((&recv, receiver.exclusive));
        let Some(mut checked) = self.args(&name.name, &rest, args, receiver_place, span) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), ret, span)
    }

    /// `T.function(...)` on a type parameter.
    pub(super) fn param_assoc_call(&mut self, ty: &Ty, name: &ast::Ident, args: &[ast::Arg], span: Span) -> tir::Expr {
        if name.name == "default" && args.is_empty() && self.param_has(ty, Bound::Default) {
            return mk(tir::ExprKind::Call(SYMBOLIC_CALL, Vec::new()), ty.clone(), span);
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
        let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else { return error_expr(span) };
        mk(tir::ExprKind::Call(SYMBOLIC_CALL, checked), sig.ret, span)
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
                        self.diags.error(
                            Code::TYPE_MISMATCH,
                            span,
                            format!(
                                "cannot tell the type arguments of `{}` here; write them, as in `{}<...>`",
                                info.name, info.name
                            ),
                        );
                        return None;
                    }
                },
            };
            args.push(ty.clone());
            env.push((p.clone(), ty));
        }
        globals.check_adt_bounds(g, &args, span, self.diags);
        Some(globals.adt_instance_ty(g, args))
    }

    /// `Stack.new()` and `Stack.Node(value, rest)` on a generic type written without arguments.
    pub(super) fn assoc_call_template(
        &mut self,
        g: usize,
        name: &ast::Ident,
        args: &[ast::Arg],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let globals = self.globals;
        let (is_enum, id) = globals.generic_adts[g].identity;
        if is_enum && let Some(vi) = globals.enums[id].variants.iter().position(|v| v.name == name.name) {
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
        let Some(f) = method else {
            let Some(ty) = self.template_instance(g, Vec::new(), expect, span) else {
                return self.args_then_error(args, span);
            };
            if ty == Ty::Error {
                return self.args_then_error(args, span);
            }
            return self.assoc_call(&ty, name, args, span);
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
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    f.name.span,
                    format!("`{}` has no field `{}`", info.name, f.name.name),
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
    pub(super) fn generic_method_call(
        &mut self,
        info: GenericMethod,
        recv: tir::Expr,
        name: &str,
        args: &[ast::Arg],
        span: Span,
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
        let mut bindings = Vec::new();
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
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let mut targs = Vec::new();
        for pname in &params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot tell what `{pname}` stands for in this call to `{name}`"),
                    );
                    return error_expr(span);
                }
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
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{t}` does not implement `{}`, which `{}` of `{name}` requires",
                            globals.bound_name(b),
                            params[k]
                        ),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        // Only the substituted signature decides: the identity's parameters are `Ty::Param` in every template.
        let symbolic = targs.iter().any(|t| globals.ty_has_param(t))
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

    /// A call to a generic function whose first argument is an already-checked receiver (`xs.map(f)` calls the
    /// eager `List` helper `__list_map(xs, f)`).
    pub(super) fn generic_call_with_receiver(
        &mut self,
        g: usize,
        recv: tir::Expr,
        name: &str,
        args: &[ast::Arg],
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
        let mut bindings = Vec::new();
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
        let written: Vec<tir::Expr> = checked.into_iter().flatten().collect();
        let mut targs = Vec::new();
        for pname in &params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot tell what `{pname}` stands for in this call to `{name}`"),
                    );
                    return error_expr(span);
                }
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
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{t}` does not implement `{}`, which `{}` of `{name}` requires",
                            globals.bound_name(b),
                            params[k]
                        ),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return error_expr(span);
        }
        // Only the substituted signature decides: the identity's parameters are `Ty::Param` in every template.
        let symbolic = targs.iter().any(|t| globals.ty_has_param(t))
            || globals.ty_has_param(&recv.ty)
            || globals.ty_has_param(&globals.subst_ty(&pattern.ret, &subst));
        let receiver_place = Some((&recv, receiver.exclusive));
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
        });
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
