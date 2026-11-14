//! Calls: functions, methods, associated functions, extern functions, and the argument conventions.

use super::{FnCx, PathTy, error_expr, mk};
use crate::builtins::{builtin, builtin_sig, method, method_sig};
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
                    None => match builtin(&id.name) {
                        Some(b) => {
                            let sig = builtin_sig(b);
                            (sig.params, sig.ret, Callee::Builtin(b))
                        }
                        None if id.name == "Some" => return self.some(args, expect, span),
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, id.span, format!("unknown function `{}`", id.name));
                            return self.args_then_error(args, span);
                        }
                    },
                }
            }
            _ => {
                self.unsupported(callee.span, "calling this expression");
                return self.args_then_error(args, span);
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
        let Some(checked) = self.args(&name.name, &sig.params, args, None, span) else {
            return error_expr(span);
        };
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
                self.diags.error(
                    Code::MISSING_MUT_MARKER,
                    a.expr.span,
                    format!("this parameter of `{name}` is `mut`; pass the argument as `mut ...`"),
                );
            } else if !p.mut_param && a.mut_marker {
                self.diags.error(
                    Code::UNNEEDED_MUT_MARKER,
                    a.expr.span,
                    format!("this parameter of `{name}` is not `mut`; the argument takes no `mut` marker"),
                );
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
        let mut entries: Vec<(bool, Vec<Place>)> = Vec::new();
        if let Some((receiver, exclusive)) = receiver {
            let mut places = Vec::new();
            place_mentions(receiver, &mut places);
            entries.push((exclusive, places));
        }
        for (p, a) in params.iter().zip(args) {
            let mut places = Vec::new();
            place_mentions(a, &mut places);
            entries.push((p.mut_param, places));
        }
        for i in 0..entries.len() {
            if !entries[i].0 {
                continue;
            }
            for j in 0..entries.len() {
                if i != j && entries[i].1.iter().any(|p| entries[j].1.iter().any(|q| places_overlap(p, q))) {
                    self.diags.error(
                        Code::EXCLUSIVITY,
                        span,
                        "another argument of this call touches the same value as a `mut` argument",
                    );
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
            return self.cptr_null(expect, span);
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
        if name.name == "hash" && args.is_empty() && self.globals.has_hash(&recv.ty) {
            return mk(tir::ExprKind::Method(Method::Hash, vec![recv]), Ty::Int(IntTy::U64), span);
        }
        let Some((params, ret)) = method(&name.name).and_then(|m| method_sig(m, &recv.ty)) else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no method `{}` on type `{}`", name.name, recv.ty));
            return self.args_then_error(args, span);
        };
        let m = method(&name.name).expect("checked above");
        if m == Method::PushStr && is_place_expr(&recv) && !self.place_mutable(&recv) {
            let local = place_root(&recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this receiver".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`push_str` changes its receiver, but `{whose}` is not mutable"),
            );
        }
        let params: Vec<ParamSig> = params.into_iter().map(ParamSig::plain).collect();
        // `push_str` mutates the receiver, so no other argument may touch it (EX1).
        let receiver_place = Some((&recv, m == Method::PushStr));
        let Some(mut checked) = self.args(&name.name, &params, args, receiver_place, span) else {
            return error_expr(span);
        };
        checked.insert(0, recv);
        mk(tir::ExprKind::Method(m, checked), ret, span)
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
    fn cptr_null(&mut self, expect: Option<&Ty>, span: Span) -> tir::Expr {
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
        // A raw pointer has only `null`.
        if let Ty::CPtr(_) = ty {
            if name.name == "null" && args.is_empty() {
                return mk(tir::ExprKind::Builtin(Builtin::CPtrNull, Vec::new()), ty.clone(), span);
            }
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`CPtr` has no function `{}`", name.name));
            return self.args_then_error(args, span);
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
