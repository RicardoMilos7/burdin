//! Fields, struct literals, indexing, and arrays.

use super::{FnCx, PathTy, error_expr, mk};
use crate::generics::{Bound, SYMBOLIC_CALL};
use crate::globals::{Global, adt_key};
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::place::{is_place_expr, place_root};
use burdin_tir::{self as tir, IntTy, Ty};

impl FnCx<'_> {
    /// `base.name`: a field of a struct, a unit variant (`Color.Red`), or a constant of a module.
    pub(super) fn field(&mut self, base: &ast::Expr, name: &ast::Ident, expect: Option<&Ty>, span: Span) -> tir::Expr {
        if let Some(literal) = self.reflect_member_of_loop_variable(base, name, span) {
            return literal;
        }
        if let Some(path) = self.type_path(base) {
            let ty = match path {
                PathTy::Ty(ty) => ty,
                PathTy::Template(g) => match self.template_instance(g, Vec::new(), expect, span) {
                    Some(ty) => ty,
                    None => return error_expr(span),
                },
            };
            if let Some(member) = self.reflect_member(&ty, name, span) {
                return member;
            }
            return self.unit_variant(ty, name, span);
        }
        if let Some(ty) = self.qualified_type(base) {
            if let Some(member) = self.reflect_member(&ty, name, span) {
                return member;
            }
            return self.unit_variant(ty, name, span);
        }
        if let Some((target, item)) = self.module_member(base, name) {
            self.check_value_visibility(target, &item.name, item.span);
            match self.globals.exported_value(target, &item.name) {
                Some(Global::Const(c)) => {
                    return mk(tir::ExprKind::Const(c), self.globals.const_tys[c.0].clone(), span);
                }
                Some(_) => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        item.span,
                        format!("`{}` is a function, not a value", item.name),
                    );
                    return error_expr(span);
                }
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, item.span, format!("unknown name `{}`", item.name));
                    return error_expr(span);
                }
            }
        }
        // `local.name` read without a call, where `name` is a method of the local's type: a method value (CL11).
        if let Some(ty) = self.local_receiver_ty(base)
            && !self.has_field(&ty, name)
            && let Some(f) = self.user_method(&ty, name)
        {
            return self.bound_method_value(base, name, f, expect, span);
        }
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        let field = match &base.ty {
            Ty::Struct(a) => self.globals.structs[a.id]
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == name.name)
                .map(|(i, f)| (i, f.ty.clone(), f.is_priv, self.globals.struct_pkg[a.id])),
            Ty::Error => return error_expr(span),
            Ty::Param(_) => {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    name.span,
                    format!("`{}` is a type parameter, so its bounds give it no field `{}`", base.ty, name.name),
                );
                return error_expr(span);
            }
            _ => None,
        };
        let Some((index, ty, is_priv, owner)) = field else {
            if self.user_method(&base.ty, name).is_some() {
                self.diags.error(
                    Code::UNKNOWN_NAME,
                    name.span,
                    format!(
                        "`{}` is a method of `{}`, and a method value needs its receiver in a local name; bind the                          receiver to a name first, or write a closure",
                        name.name, base.ty
                    ),
                );
                return error_expr(span);
            }
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no field `{}` on type `{}`", name.name, base.ty));
            return error_expr(span);
        };
        if is_priv && owner != self.package() {
            self.diags.error(
                Code::PRIVATE_ITEM,
                name.span,
                format!("field `{}` of `{}` is `priv` in another package", name.name, base.ty),
            );
        }
        mk(tir::ExprKind::Field(Box::new(base), index), ty, span)
    }

    /// `Color.Red`: a unit variant of an enum type.
    fn unit_variant(&mut self, ty: Ty, name: &ast::Ident, span: Span) -> tir::Expr {
        // `u32.MAX`: an integer type's constants are `std` constants named `__<type>_<name>`.
        if let Ty::Int(i) = &ty
            && let Some(Global::Const(c)) = self.globals.std_value(&format!("__{}_{}", i.name(), name.name))
        {
            return mk(tir::ExprKind::Const(c), self.globals.const_tys[c.0].clone(), span);
        }
        // `f64.MAX`: so are a float type's constants.
        if let Ty::Float(_) = &ty
            && let Some(Global::Const(c)) = self.globals.std_value(&format!("__{}_{}", ty, name.name))
        {
            return mk(tir::ExprKind::Const(c), self.globals.const_tys[c.0].clone(), span);
        }
        if let Ty::Enum(a) = &ty
            && let Some(index) = self.globals.enums[a.id].variants.iter().position(|v| v.name == name.name)
        {
            if self.globals.enums[a.id].variants[index].shape != tir::VariantShape::Unit {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("variant `{}` has fields; give them to build a value", name.name),
                );
                return error_expr(span);
            }
            return mk(tir::ExprKind::Variant(index, Vec::new()), ty, span);
        }
        // `Point.double` or `Point.new`: a method or associated function named through its type (CL11).
        if let Some(f) = self.user_method(&ty, name) {
            return self.method_function_value(f, name, span);
        }
        self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`{ty}` has no variant `{}`", name.name));
        error_expr(span)
    }

    /// `Point { x: 1, y }` or `Shape.Rect { w: 1, h: 2 }`.
    pub(super) fn struct_literal(
        &mut self,
        path: &[ast::Ident],
        fields: &[ast::FieldInit],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        // A generic type, plain or module-qualified: the arguments come from the field values and the expected type.
        if let Some((g, found)) = self.generic_adt_path(path) {
            if let Some(name) = path.last() {
                self.check_type_visibility(found, &name.name, name.span);
            }
            return self.generic_struct_literal(g, fields, expect, span);
        }
        let text: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
        let text = text.join(".");
        // `(type, variant, package that declares the type)`.
        let mut target: Option<(Ty, Option<usize>, usize)> = None;
        if path.len() == 1 && path[0].name == "Self" {
            let name = &path[0];
            match self.self_ty.clone() {
                Some(Ty::Struct(a)) => target = Some((Ty::Struct(a.clone()), None, self.globals.struct_pkg[a.id])),
                Some(Ty::Error) => {}
                Some(other) => {
                    self.diags.error(Code::TYPE_MISMATCH, name.span, format!("`{other}` is not a struct"));
                }
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, name.span, "`Self` is only available inside a method block");
                }
            }
        } else if path.len() == 1 {
            let name = &path[0];
            match self.globals.named_type(self.module, &name.name) {
                Some(ty) => match ty {
                    Ty::Struct(a) => {
                        target = Some((Ty::Struct(a.clone()), None, self.globals.struct_pkg[a.id]));
                    }
                    Ty::Error => {}
                    other => {
                        self.diags.error(Code::TYPE_MISMATCH, name.span, format!("`{other}` is not a struct"));
                    }
                },
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{}`", name.name));
                }
            }
        } else if let Some((ty, variant, found, type_name)) = self.adt_path(path, &Ty::Error) {
            self.check_type_visibility(found, &type_name.name, type_name.span);
            match (&ty, &variant) {
                (Ty::Struct(a), None) => {
                    target = Some((ty.clone(), None, self.globals.struct_pkg[a.id]));
                }
                (Ty::Enum(a), Some(v)) => {
                    match self.globals.enums[a.id].variants.iter().position(|x| x.name == v.name) {
                        Some(i) if self.globals.enums[a.id].variants[i].shape == tir::VariantShape::Struct => {
                            target = Some((ty.clone(), Some(i), self.globals.enum_pkg[a.id]));
                        }
                        Some(_) => {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                span,
                                format!("variant `{text}` has no named fields"),
                            );
                        }
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, v.span, format!("unknown variant `{text}`"));
                        }
                    }
                }
                (Ty::Error, _) => {}
                _ => {
                    self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
                }
            }
        } else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
        }
        let defs: Vec<tir::FieldDef> = match &target {
            Some((Ty::Struct(a), _, _)) => self.globals.structs[a.id].fields.clone(),
            Some((Ty::Enum(a), Some(i), _)) => self.globals.enums[a.id].variants[*i].fields.clone(),
            _ => Vec::new(),
        };
        let mut given = vec![false; defs.len()];
        let mut checked = Vec::new();
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
            let index = defs.iter().position(|d| d.name == f.name.name);
            let Some(index) = index.filter(|_| target.is_some()) else {
                if target.is_some() {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        f.name.span,
                        format!("`{text}` has no field `{}`", f.name.name),
                    );
                }
                self.expr(value, None);
                continue;
            };
            if std::mem::replace(&mut given[index], true) {
                self.diags.error(Code::TYPE_MISMATCH, f.name.span, format!("field `{}` is given twice", f.name.name));
            }
            if let Some((_, _, owner)) = &target
                && defs[index].is_priv
                && *owner != self.package()
            {
                self.diags.error(
                    Code::PRIVATE_ITEM,
                    f.name.span,
                    format!("field `{}` of `{text}` is `priv` in another package", f.name.name),
                );
            }
            let fty = defs[index].ty.clone();
            let v = self.expr(value, Some(&fty));
            let v = self.coerce_arg(v, &fty);
            checked.push((index, v));
        }
        let Some((ty, variant, _)) = target else { return error_expr(span) };
        let missing: Vec<&str> = defs.iter().zip(&given).filter(|(_, g)| !**g).map(|(d, _)| d.name.as_str()).collect();
        if !missing.is_empty() {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{text}` is missing the field(s) `{}`", missing.join("`, `")),
            );
        }
        let kind = match variant {
            Some(i) => tir::ExprKind::Variant(i, checked),
            None => tir::ExprKind::Struct(checked),
        };
        mk(kind, ty, span)
    }

    pub(super) fn index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> tir::Expr {
        // The surrounding place may be written (`x[i] = v`, `&mut x[i]`, `mut x[i]`); saving the mode here lets
        // the base of a nested index keep it while the index operand is an ordinary value expression.
        let mutable = self.write_place;
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        if let ast::ExprKind::Range { lo, hi, inclusive } = &index.kind {
            let saved = self.write_place;
            self.write_place = false;
            let usize_ty = Ty::Int(IntTy::Usize);
            let lo = self.expr(lo, Some(&usize_ty));
            let lo = self.expect_ty(lo, &usize_ty);
            let hi = self.expr(hi, Some(&usize_ty));
            let hi = self.expect_ty(hi, &usize_ty);
            self.write_place = saved;
            let ty = match &base.ty {
                Ty::Str | Ty::String => Ty::Str,
                Ty::Slice(_) => base.ty.clone(),
                Ty::Array(elem, _) | Ty::List(elem, _) => Ty::Slice(elem.clone()),
                Ty::Error => Ty::Error,
                other => {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        base.span,
                        format!("cannot take a slice of a value of type `{other}`"),
                    );
                    return error_expr(span);
                }
            };
            let kind = tir::ExprKind::Slice {
                base: Box::new(base),
                lo: Box::new(lo),
                hi: Box::new(hi),
                inclusive: *inclusive,
            };
            return mk(kind, ty, span);
        }
        let saved = self.write_place;
        self.write_place = false;
        let index = self.expr(index, Some(&Ty::Int(IntTy::Usize)));
        self.write_place = saved;
        let index = self.expect_ty(index, &Ty::Int(IntTy::Usize));
        let ty = match &base.ty {
            Ty::Array(elem, _) | Ty::Slice(elem) | Ty::List(elem, _) => (**elem).clone(),
            Ty::Error => return error_expr(span),
            // A user type takes part through `Index`/`IndexMut` (`spec/operators.md`, OP9-OP10).
            _ => return self.overloaded_index(base, index, mutable, span),
        };
        mk(tir::ExprKind::Index(Box::new(base), Box::new(index)), ty, span)
    }

    /// Lowers `x[i]` (or `x[i] = v`, with `mutable`) on a user type to its `Index`/`IndexMut` call and the
    /// referent of the returned reference. The built-in array, slice, and `List` indexing never reaches here.
    fn overloaded_index(&mut self, base: tir::Expr, index: tir::Expr, mutable: bool, span: Span) -> tir::Expr {
        let Some(ops) = self.globals.op_ifaces else {
            return self.index_missing(base.ty, mutable, span);
        };
        let (iface, method) = if mutable { (ops.index_mut, "index_mut") } else { (ops.index, "index") };
        match base.ty.clone() {
            Ty::Param(_) => {
                let Some((sig, _, _)) = self.globals.bound_method(&base.ty, method, &self.bounds) else {
                    return self.index_missing(base.ty, mutable, span);
                };
                let Ty::Ref { mutable: through, inner } = sig.ret.clone() else {
                    return self.index_missing(base.ty, mutable, span);
                };
                if sig.params.len() != 2 || !sig.has_self || through != mutable {
                    return self.index_missing(base.ty, mutable, span);
                }
                self.check_index_receiver(&base, &sig.params[0], method);
                let call = mk(tir::ExprKind::Call(SYMBOLIC_CALL, vec![base, index]), sig.ret.clone(), span);
                mk(tir::ExprKind::DerefRef(Box::new(call)), (*inner).clone(), span)
            }
            ty @ (Ty::Struct(_) | Ty::Enum(_)) => {
                let key = adt_key(&ty).expect("a struct or enum has a key");
                let Some(&f) = self.globals.methods.get(&(key, method.to_string())) else {
                    return self.index_missing(ty, mutable, span);
                };
                let sig = self.globals.sigs[f.0].clone();
                let Ty::Ref { mutable: through, inner } = sig.ret.clone() else {
                    return self.index_missing(ty, mutable, span);
                };
                if sig.params.len() != 2 || !sig.has_self || through != mutable {
                    return self.index_missing(ty, mutable, span);
                }
                if !self.globals.satisfies(&ty, &Bound::User(iface, vec![(*inner).clone()]), &self.bounds) {
                    return self.index_missing(ty, mutable, span);
                }
                self.check_index_receiver(&base, &sig.params[0], method);
                let call = mk(tir::ExprKind::Call(f, vec![base, index]), sig.ret.clone(), span);
                mk(tir::ExprKind::DerefRef(Box::new(call)), (*inner).clone(), span)
            }
            ty => self.index_missing(ty, mutable, span),
        }
    }

    /// `x[i]` on a type that does not implement `Index`, or `x[i] = v` without `IndexMut` (E0141); a type that
    /// has no element access at all keeps the stage 0 E0003.
    fn index_missing(&mut self, ty: Ty, mutable: bool, span: Span) -> tir::Expr {
        match ty {
            Ty::Param(_) => {
                let (what, iface) =
                    if mutable { ("assign through an index of", "IndexMut") } else { ("index into", "Index") };
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("cannot {what} `{ty}`: its bounds do not provide `{iface}`"),
                );
            }
            Ty::Struct(_) | Ty::Enum(_) => {
                let (what, iface) =
                    if mutable { ("assign through an index of", "IndexMut") } else { ("index into", "Index") };
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("cannot {what} `{ty}`: it does not implement `{iface}`"),
                );
            }
            other => {
                self.diags.error(Code::TYPE_MISMATCH, span, format!("cannot index into a value of type `{other}`"));
            }
        }
        error_expr(span)
    }

    /// A `mut self` receiver of `index_mut` must be a mutable place (MU3); a temporary or a place behind a
    /// mutable reference is already writable.
    fn check_index_receiver(&mut self, recv: &tir::Expr, receiver: &tir::ParamSig, method: &str) {
        if receiver.by_ref && receiver.exclusive && is_place_expr(recv) && !self.place_mutable(recv) {
            let local = place_root(recv).map(|root| self.locals[root.0].name.clone());
            let whose = local.unwrap_or_else(|| "this value".to_string());
            self.diags.error(
                Code::MUT_OF_IMMUTABLE,
                recv.span,
                format!("`{method}` changes its receiver, but `{whose}` is not mutable"),
            );
        }
    }

    pub(super) fn array(&mut self, elems: &[ast::Expr], expect: Option<&Ty>, span: Span) -> tir::Expr {
        // The expected element type of a generic call's parameter (`[T; N]`) can be a parameter of the callee that
        // is not bound yet; it says nothing about the elements, which then decide `T` themselves.
        let mut elem_ty = match expect {
            Some(Ty::Array(t, _)) if !self.is_foreign_param(t) => Some((**t).clone()),
            _ => None,
        };
        let mut checked = Vec::new();
        for e in elems {
            let c = self.expr(e, elem_ty.as_ref());
            let c = match &elem_ty {
                Some(t) => self.expect_ty(c, t),
                None => {
                    elem_ty = Some(c.ty.clone());
                    c
                }
            };
            checked.push(c);
        }
        let Some(elem_ty) = elem_ty else {
            self.diags.error(Code::TYPE_MISMATCH, span, "cannot infer the element type of an empty array");
            return error_expr(span);
        };
        let ty = Ty::Array(Box::new(elem_ty), checked.len() as u64);
        mk(tir::ExprKind::Array(checked), ty, span)
    }
}

impl FnCx<'_> {
    /// Whether `ty` is a type parameter that the function being checked does not declare: one of a callee whose
    /// call is still being inferred.
    fn is_foreign_param(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Param(name) if !self.bounds.iter().any(|(n, _)| n == &**name))
    }
}
