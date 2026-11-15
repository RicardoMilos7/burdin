//! Patterns: bindings, variants, literals, and how they take their scrutinee.

use super::FnCx;
use crate::globals::Global;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, BindMode, LocalId, LocalKind, Pat, Ty};

impl FnCx<'_> {
    /// How a binding of type `ty` takes its value: a view when the scrutinee is borrowed, otherwise a move for a
    /// non-Copy type and a copy for a Copy one.
    pub(super) fn bind_mode(&self, ty: &Ty, owned: bool) -> BindMode {
        if !owned {
            BindMode::View
        } else if self.globals.is_copy(ty) {
            BindMode::Copy
        } else {
            BindMode::Move
        }
    }

    pub(super) fn declare_binding(
        &mut self,
        name: &ast::Ident,
        ty: &Ty,
        mutable: bool,
        owned: bool,
    ) -> (LocalId, BindMode) {
        let mode = self.bind_mode(ty, owned);
        let kind = if mode == BindMode::View { LocalKind::View } else { LocalKind::Owned };
        (self.declare(&name.name, ty.clone(), mutable, kind), mode)
    }

    pub(super) fn pattern(&mut self, p: &ast::Pattern, ty: &Ty, owned: bool) -> Pat {
        use ast::PatternKind as P;
        let pat_mismatch = |cx: &mut Self, what: &str| {
            if *ty != Ty::Error {
                cx.diags.error(Code::TYPE_MISMATCH, p.span, format!("expected `{ty}`, found {what}"));
            }
            Pat::Wild
        };
        match &p.kind {
            P::Wild => Pat::Wild,
            P::Binding { name, mutable } => {
                if name.name == "None" && !*mutable && self.is_prelude("None") {
                    return match ty {
                        Ty::Option(_) | Ty::Error => Pat::None,
                        _ => pat_mismatch(self, "`None`"),
                    };
                }
                if let Some(Global::Const(_)) = self.globals.value(self.module, &name.name) {
                    self.unsupported(p.span, "constants in patterns"); // e9000: pattern-constant
                    return Pat::Wild;
                }
                let (id, mode) = self.declare_binding(name, ty, *mutable, owned);
                Pat::Binding(id, mode)
            }
            P::At(name, inner) => {
                let inner = self.pattern(inner, ty, owned);
                let (id, mode) = self.declare_binding(name, ty, false, owned);
                Pat::At(id, mode, Box::new(inner))
            }
            P::Lit(lit) => self.lit_pattern(lit, ty, p.span).unwrap_or(Pat::Wild),
            P::Range(lo, hi) => {
                let (Some(lo), Some(hi)) = (self.lit_pattern(lo, ty, p.span), self.lit_pattern(hi, ty, p.span)) else {
                    return Pat::Wild;
                };
                let value = |p: &Pat| match p {
                    Pat::Int(v) => Some(*v),
                    Pat::Char(c) => Some(*c as i128),
                    _ => None,
                };
                match (value(&lo), value(&hi)) {
                    (Some(lo), Some(hi)) if lo <= hi => Pat::Range(lo, hi),
                    (Some(_), Some(_)) => {
                        self.diags.error(Code::TYPE_MISMATCH, p.span, "this range pattern is empty");
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, "a range pattern"),
                }
            }
            P::TupleStruct(path, elems) if path.len() == 1 && path[0].name == "Some" && self.is_prelude("Some") => {
                match ty {
                    Ty::Option(inner) if elems.len() == 1 => Pat::Some(Box::new(self.pattern(&elems[0], inner, owned))),
                    Ty::Option(_) => {
                        self.diags.error(Code::ARG_COUNT, p.span, "`Some` takes one pattern");
                        Pat::Wild
                    }
                    Ty::Error => {
                        elems.iter().for_each(|e| {
                            self.pattern(e, &Ty::Error, owned);
                        });
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, "`Some(...)`"),
                }
            }
            P::TupleStruct(path, elems)
                if path.len() == 1
                    && (path[0].name == "Ok" || path[0].name == "Err")
                    && self.is_prelude(&path[0].name) =>
            {
                let ok = path[0].name == "Ok";
                match ty {
                    Ty::Result(t, e) if elems.len() == 1 => {
                        let payload = if ok { (**t).clone() } else { (**e).clone() };
                        let inner = self.pattern(&elems[0], &payload, owned);
                        if ok { Pat::Ok(Box::new(inner)) } else { Pat::Err(Box::new(inner)) }
                    }
                    Ty::Result(..) => {
                        let name = if ok { "Ok" } else { "Err" };
                        self.diags.error(Code::ARG_COUNT, p.span, format!("`{name}` takes one pattern"));
                        Pat::Wild
                    }
                    Ty::Error => {
                        elems.iter().for_each(|e| {
                            self.pattern(e, &Ty::Error, owned);
                        });
                        Pat::Wild
                    }
                    _ => pat_mismatch(self, if ok { "`Ok(...)`" } else { "`Err(...)`" }),
                }
            }
            P::Or(alts) => {
                let before = self.locals.len();
                let alts: Vec<Pat> = alts.iter().map(|a| self.pattern(a, ty, owned)).collect();
                if self.locals.len() != before {
                    self.unsupported(p.span, "bindings inside `|` patterns"); // e9000: pattern-or-binding
                }
                Pat::Or(alts)
            }
            P::Tuple(elems) => match ty {
                Ty::Tuple(ts) if ts.len() == elems.len() => {
                    Pat::Product(elems.iter().zip(ts).map(|(e, t)| self.pattern(e, t, owned)).collect())
                }
                Ty::Unit if elems.is_empty() => Pat::Product(Vec::new()),
                Ty::Error => {
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    Pat::Wild
                }
                _ => pat_mismatch(self, &format!("a tuple pattern with {} elements", elems.len())),
            },
            P::Path(path) => {
                let Some((index, shape, _)) = self.variant_pattern(path, ty, p.span) else { return Pat::Wild };
                if shape != tir::VariantShape::Unit {
                    let how = if shape == tir::VariantShape::Tuple { "(...)" } else { " { ... }" };
                    let name = path.last().map(|n| n.name.as_str()).unwrap_or("");
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        p.span,
                        format!("variant `{name}` has fields; match it with `{name}{how}`"),
                    );
                    return Pat::Wild;
                }
                Pat::Variant(index, Vec::new())
            }
            P::TupleStruct(path, elems) => {
                let Some((index, shape, field_tys)) = self.variant_pattern(path, ty, p.span) else {
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    return Pat::Wild;
                };
                if shape != tir::VariantShape::Tuple || field_tys.len() != elems.len() {
                    let code = if shape == tir::VariantShape::Tuple { Code::ARG_COUNT } else { Code::TYPE_MISMATCH };
                    self.diags.error(
                        code,
                        p.span,
                        format!("this variant has {} field(s) and is not matched like this", field_tys.len()),
                    );
                    elems.iter().for_each(|e| {
                        self.pattern(e, &Ty::Error, owned);
                    });
                    return Pat::Wild;
                }
                Pat::Variant(index, elems.iter().zip(&field_tys).map(|(e, t)| self.pattern(e, t, owned)).collect())
            }
            P::Struct(path, fields) => {
                // A struct, or a variant with named fields.
                let owner;
                let (variant, defs): (Option<usize>, Vec<tir::FieldDef>) =
                    if let Some((target, variant, found, type_name)) = self.adt_path(path, ty) {
                        match variant {
                            None => {
                                // A variant pattern goes through `variant_pattern`, which checks visibility.
                                self.check_type_visibility(found, &type_name.name, type_name.span);
                                let Ty::Struct(a) = target else {
                                    if target == Ty::Error {
                                        return Pat::Wild;
                                    }
                                    return pat_mismatch(self, &format!("a `{}` pattern", path.last().unwrap().name));
                                };
                                if *ty != Ty::Struct(a.clone()) && *ty != Ty::Error {
                                    return pat_mismatch(self, &format!("a `{}` pattern", a.name));
                                }
                                owner = self.globals.struct_pkg[a.id];
                                (None, self.globals.structs[a.id].fields.clone())
                            }
                            Some(_) => {
                                let Some((index, shape, _)) = self.variant_pattern(path, ty, p.span) else {
                                    return Pat::Wild;
                                };
                                if shape != tir::VariantShape::Struct {
                                    return pat_mismatch(self, "a pattern with named fields");
                                }
                                let Ty::Enum(a) = ty else { return Pat::Wild };
                                owner = self.globals.enum_pkg[a.id];
                                (Some(index), self.globals.enums[a.id].variants[index].fields.clone())
                            }
                        }
                    } else if path.len() == 1 {
                        self.diags.error(Code::UNKNOWN_NAME, path[0].span, format!("unknown type `{}`", path[0].name));
                        return Pat::Wild;
                    } else {
                        let Some((index, shape, _)) = self.variant_pattern(path, ty, p.span) else {
                            return Pat::Wild;
                        };
                        if shape != tir::VariantShape::Struct {
                            return pat_mismatch(self, "a pattern with named fields");
                        }
                        let Ty::Enum(a) = ty else { return Pat::Wild };
                        owner = self.globals.enum_pkg[a.id];
                        (Some(index), self.globals.enums[a.id].variants[index].fields.clone())
                    };
                let from_other_package = owner != self.package();
                let mut subs: Vec<Pat> = defs.iter().map(|_| Pat::Wild).collect();
                for f in fields {
                    let Some(i) = defs.iter().position(|d| d.name == f.name.name) else {
                        self.diags.error(Code::UNKNOWN_NAME, f.name.span, format!("no field `{}` here", f.name.name));
                        if let Some(sub) = &f.pattern {
                            self.pattern(sub, &Ty::Error, owned);
                        }
                        continue;
                    };
                    if from_other_package && defs[i].is_priv {
                        self.diags.error(
                            Code::PRIVATE_ITEM,
                            f.name.span,
                            format!("field `{}` is `priv` in another package", f.name.name),
                        );
                    }
                    let fty = defs[i].ty.clone();
                    subs[i] = match &f.pattern {
                        Some(sub) => self.pattern(sub, &fty, owned),
                        None => {
                            let (id, mode) = self.declare_binding(&f.name, &fty, false, owned);
                            Pat::Binding(id, mode)
                        }
                    };
                }
                match variant {
                    Some(index) => Pat::Variant(index, subs),
                    None if *ty == Ty::Error => Pat::Wild,
                    None => Pat::Product(subs),
                }
            }
        }
    }

    /// Resolves `Enum.Variant` in a pattern against the scrutinee type: the variant's index, shape, and field
    /// types. Reports an error and returns `None` if the path is not a variant of `ty`.
    pub(super) fn variant_pattern(
        &mut self,
        path: &[ast::Ident],
        ty: &Ty,
        span: Span,
    ) -> Option<(usize, tir::VariantShape, Vec<Ty>)> {
        let text: Vec<&str> = path.iter().map(|p| p.name.as_str()).collect();
        let text = text.join(".");
        let Some((enum_ty, variant, found, type_name)) = self.adt_path(path, ty).filter(|(_, v, _, _)| v.is_some())
        else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not an enum variant"));
            return None;
        };
        self.check_type_visibility(found, &type_name.name, type_name.span);
        let variant = variant.expect("checked above");
        let Ty::Enum(a) = enum_ty else {
            self.diags.error(Code::UNKNOWN_NAME, span, format!("`{text}` is not an enum variant"));
            return None;
        };
        let def = &self.globals.enums[a.id];
        let Some(index) = def.variants.iter().position(|v| v.name == variant.name) else {
            self.diags.error(
                Code::UNKNOWN_NAME,
                variant.span,
                format!("`{}` has no variant `{}`", a.name, variant.name),
            );
            return None;
        };
        if *ty != Ty::Enum(a.clone()) {
            if *ty != Ty::Error {
                self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found a `{}` pattern", a.name));
            }
            return None;
        }
        let v = &def.variants[index];
        Some((index, v.shape, v.fields.iter().map(|f| f.ty.clone()).collect()))
    }

    pub(super) fn lit_pattern(&mut self, lit: &ast::LitPat, ty: &Ty, span: Span) -> Option<Pat> {
        match (lit, ty) {
            (_, Ty::Error) => None,
            (ast::LitPat::Int { value, negative }, Ty::Int(i)) => {
                self.int_value(*value, *negative, *i, span).map(Pat::Int)
            }
            (ast::LitPat::Bool(b), Ty::Bool) => Some(Pat::Bool(*b)),
            (ast::LitPat::Char(c), Ty::Char) => Some(Pat::Char(*c)),
            (ast::LitPat::Str(s), Ty::Str) => Some(Pat::Str(s.clone())),
            _ => {
                let what = match lit {
                    ast::LitPat::Int { .. } => "an integer literal",
                    ast::LitPat::Bool(_) => "a `bool` literal",
                    ast::LitPat::Char(_) => "a character literal",
                    ast::LitPat::Str(_) => "a string literal",
                };
                self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found {what}"));
                None
            }
        }
    }
}
