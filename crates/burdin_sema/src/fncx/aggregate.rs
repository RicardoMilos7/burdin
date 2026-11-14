//! Fields, struct literals, indexing, and arrays.

use super::{FnCx, PathTy, error_expr, mk};
use crate::globals::TypeName;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, IntTy, Ty};

impl FnCx<'_> {
    /// `base.name`: a field of a struct, or a unit variant (`Color.Red`).
    pub(super) fn field(&mut self, base: &ast::Expr, name: &ast::Ident, expect: Option<&Ty>, span: Span) -> tir::Expr {
        if let Some(path) = self.type_path(base) {
            let ty = match path {
                PathTy::Ty(ty) => ty,
                PathTy::Template(g) => match self.template_instance(g, Vec::new(), expect, span) {
                    Some(ty) => ty,
                    None => return error_expr(span),
                },
            };
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
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("`{ty}` has no variant `{}`", name.name));
            return error_expr(span);
        }
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        let field = match &base.ty {
            Ty::Struct(a) => self.globals.structs[a.id]
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| f.name == name.name)
                .map(|(i, f)| (i, f.ty.clone())),
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
        let Some((index, ty)) = field else {
            self.diags.error(Code::UNKNOWN_NAME, name.span, format!("no field `{}` on type `{}`", name.name, base.ty));
            return error_expr(span);
        };
        mk(tir::ExprKind::Field(Box::new(base), index), ty, span)
    }

    /// `Point { x: 1, y }` or `Shape.Rect { w: 1, h: 2 }`.
    pub(super) fn struct_literal(
        &mut self,
        path: &[ast::Ident],
        fields: &[ast::FieldInit],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        if let [name] = path
            && let Some(&TypeName::GenericAdt(g)) = self.globals.types.get(&name.name)
        {
            return self.generic_struct_literal(g, fields, expect, span);
        }
        let text: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
        let text = text.join(".");
        let target = match path {
            [name] => match self.globals.named_type(&name.name) {
                Some(ty @ Ty::Struct(_)) => Some((ty, None)),
                Some(Ty::Error) => None,
                Some(other) => {
                    self.diags.error(Code::TYPE_MISMATCH, name.span, format!("`{other}` is not a struct"));
                    None
                }
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{}`", name.name));
                    None
                }
            },
            [enum_name, variant] => match self.globals.named_type(&enum_name.name) {
                Some(ty @ Ty::Enum(_)) => {
                    let Ty::Enum(a) = &ty else { unreachable!() };
                    match self.globals.enums[a.id].variants.iter().position(|v| v.name == variant.name) {
                        Some(i) if self.globals.enums[a.id].variants[i].shape == tir::VariantShape::Struct => {
                            Some((ty, Some(i)))
                        }
                        Some(_) => {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                span,
                                format!("variant `{text}` has no named fields"),
                            );
                            None
                        }
                        None => {
                            self.diags.error(Code::UNKNOWN_NAME, variant.span, format!("unknown variant `{text}`"));
                            None
                        }
                    }
                }
                _ => {
                    self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
                    None
                }
            },
            _ => {
                self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not a struct or an enum variant"));
                None
            }
        };
        let defs: Vec<(String, Ty)> = match &target {
            Some((Ty::Struct(a), _)) => {
                self.globals.structs[a.id].fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect()
            }
            Some((Ty::Enum(a), Some(i))) => {
                self.globals.enums[a.id].variants[*i].fields.iter().map(|f| (f.name.clone(), f.ty.clone())).collect()
            }
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
            let index = defs.iter().position(|(n, _)| *n == f.name.name);
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
            let fty = defs[index].1.clone();
            let v = self.expr(value, Some(&fty));
            let v = self.coerce_arg(v, &fty);
            checked.push((index, v));
        }
        let Some((ty, variant)) = target else { return error_expr(span) };
        let missing: Vec<&str> = defs.iter().zip(&given).filter(|(_, g)| !**g).map(|((n, _), _)| n.as_str()).collect();
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
        let base = self.expr(base, None);
        let base = self.auto_deref(base);
        if let ast::ExprKind::Range { lo, hi, inclusive } = &index.kind {
            if *inclusive {
                return self.unsupported(index.span, "an inclusive slice range");
            }
            let usize_ty = Ty::Int(IntTy::Usize);
            let lo = self.expr(lo, Some(&usize_ty));
            let lo = self.expect_ty(lo, &usize_ty);
            let hi = self.expr(hi, Some(&usize_ty));
            let hi = self.expect_ty(hi, &usize_ty);
            let ty = match &base.ty {
                Ty::Str | Ty::String => Ty::Str,
                Ty::Slice(_) => base.ty.clone(),
                Ty::Array(elem, _) => Ty::Slice(elem.clone()),
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
            let kind = tir::ExprKind::Slice { base: Box::new(base), lo: Box::new(lo), hi: Box::new(hi) };
            return mk(kind, ty, span);
        }
        let index = self.expr(index, Some(&Ty::Int(IntTy::Usize)));
        let index = self.expect_ty(index, &Ty::Int(IntTy::Usize));
        let ty = match &base.ty {
            Ty::Array(elem, _) | Ty::Slice(elem) => (**elem).clone(),
            Ty::Error => Ty::Error,
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    base.span,
                    format!("cannot index into a value of type `{other}`"),
                );
                return error_expr(span);
            }
        };
        mk(tir::ExprKind::Index(Box::new(base), Box::new(index)), ty, span)
    }

    pub(super) fn array(&mut self, elems: &[ast::Expr], expect: Option<&Ty>, span: Span) -> tir::Expr {
        let mut elem_ty = match expect {
            Some(Ty::Array(t, _)) => Some((**t).clone()),
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
