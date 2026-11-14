//! Calls, constructors, and literals of generic functions and types, where the type arguments are inferred.

use super::{FnCx, error_expr, mk};
use crate::generics::{Bound, Request, SYMBOLIC_CALL};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, Builtin, IntTy, Method, ParamSig, Ty};

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
        if pattern.params.len() != args.len() {
            let _ = self.args(&id.name, &pattern.params, args, None, span);
            return error_expr(span);
        }
        let mut checked = Vec::new();
        for (a, p) in args.iter().zip(&pattern.params) {
            let hint = if globals.ty_has_param(&p.ty) { None } else { Some(&p.ty) };
            checked.push(self.expr(&a.expr, hint));
        }
        let mut bindings = Vec::new();
        for (e, p) in checked.iter().zip(&pattern.params) {
            globals.unify(&p.ty, &e.ty, &mut bindings);
        }
        if let Some(expected) = expect {
            globals.unify(&pattern.ret, expected, &mut bindings);
        }
        let mut targs = Vec::new();
        for pname in &info.params {
            match bindings.iter().find(|(n, _)| n == pname) {
                Some((_, t)) => targs.push(t.clone()),
                None => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("cannot tell what `{pname}` stands for in this call to `{}`", id.name),
                    );
                    return error_expr(span);
                }
            }
        }
        if targs.contains(&Ty::Error) {
            return error_expr(span);
        }
        let mut ok = true;
        for ((pname, t), bounds) in info.params.iter().zip(&targs).zip(&info.bounds) {
            for b in bounds {
                if !globals.satisfies(t, *b, &self.bounds) {
                    self.diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{t}` does not implement `{}`, which `{pname}` of `{}` requires",
                            globals.bound_name(*b),
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
        let Some(checked) = self.args_pre(&id.name, &params, args, None, span, Some(checked)) else {
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
        for p in &info.params {
            match bindings.iter().find(|(n, _)| n == p) {
                Some((_, t)) => args.push(t.clone()),
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
            }
        }
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
                let hint = if globals.ty_has_param(&f.ty) { None } else { Some(&f.ty) };
                checked.push(self.expr(&a.expr, hint));
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
        // Anything else is an associated function; the type arguments can only come from the expected type.
        let Some(ty) = self.template_instance(g, Vec::new(), expect, span) else {
            return self.args_then_error(args, span);
        };
        if ty == Ty::Error {
            return self.args_then_error(args, span);
        }
        self.assoc_call(&ty, name, args, span)
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
            let hint = if globals.ty_has_param(&defs[index].ty) { None } else { Some(&defs[index].ty) };
            let v = self.expr(value, hint);
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
}
