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
        (self.declare(&name.name, ty.clone(), mutable, kind, name.span), mode)
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
            P::Typed { .. } => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    p.span,
                    "a type pattern is the whole pattern of a `match` arm on an `any` value",
                );
                Pat::Wild
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
                    Ty::Option(inner) => match self.rest_positions(elems, 1) {
                        Some(positions) => Pat::Some(Box::new(self.sub_pattern(positions[0], inner, owned))),
                        None => {
                            self.diags.error(Code::ARG_COUNT, p.span, "`Some` takes one pattern");
                            Pat::Wild
                        }
                    },
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
                    Ty::Result(t, e) => match self.rest_positions(elems, 1) {
                        Some(positions) => {
                            let payload = if ok { (**t).clone() } else { (**e).clone() };
                            let inner = self.sub_pattern(positions[0], &payload, owned);
                            if ok { Pat::Ok(Box::new(inner)) } else { Pat::Err(Box::new(inner)) }
                        }
                        None => {
                            let name = if ok { "Ok" } else { "Err" };
                            self.diags.error(Code::ARG_COUNT, p.span, format!("`{name}` takes one pattern"));
                            Pat::Wild
                        }
                    },
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
            P::Rest(_) => Pat::Wild, // only a list holds a rest; the list handles it
            P::Slice(elems) => self.slice_pattern(p.span, elems, ty, owned),
            P::Tuple(elems) => match ty {
                Ty::Tuple(ts) => match self.rest_positions(elems, ts.len()) {
                    Some(positions) => Pat::Product(
                        positions.into_iter().zip(ts).map(|(e, t)| self.sub_pattern(e, t, owned)).collect(),
                    ),
                    None => pat_mismatch(self, &format!("a tuple pattern with {} elements", elems.len())),
                },
                Ty::Unit => match self.rest_positions(elems, 0) {
                    Some(_) => Pat::Product(Vec::new()),
                    None => pat_mismatch(self, &format!("a tuple pattern with {} elements", elems.len())),
                },
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
                let positions =
                    if shape == tir::VariantShape::Tuple { self.rest_positions(elems, field_tys.len()) } else { None };
                let Some(positions) = positions else {
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
                };
                Pat::Variant(
                    index,
                    positions.into_iter().zip(&field_tys).map(|(e, t)| self.sub_pattern(e, t, owned)).collect(),
                )
            }
            P::Struct(path, fields, rest) => {
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
                let mut named = vec![false; defs.len()];
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
                    named[i] = true;
                    subs[i] = match &f.pattern {
                        Some(sub) => self.pattern(sub, &fty, owned),
                        None => {
                            let (id, mode) = self.declare_binding(&f.name, &fty, false, owned);
                            Pat::Binding(id, mode)
                        }
                    };
                }
                // A `..` that ignores no field is E0011 (RP4). A pattern without `..` may leave fields out: the
                // omitted fields are not bound and stay where they are (`spec/rest-patterns.md`, "Omitted fields").
                if let Some(span) = rest.filter(|_| named.iter().all(|n| *n)) {
                    self.diags.error(Code::REST_IGNORES_NOTHING, span, "this `..` stands for no field");
                }
                match variant {
                    Some(index) => Pat::Variant(index, subs),
                    None if *ty == Ty::Error => Pat::Wild,
                    None => Pat::Product(subs),
                }
            }
        }
    }

    /// `[p, ..., q]` against an array or a slice (`spec/array-patterns.md`, AP2-AP6). The first rest element counts;
    /// a later one was reported by the parser (E0010) and is skipped here. An array pattern without a rest names every
    /// element; with one, the rest stands for the elements the pattern does not name, and for an array that is none
    /// it is E0011 (AP4). A slice pattern with a rest matches every length from the named count up (AP5).
    fn slice_pattern(&mut self, span: Span, elems: &[ast::Pattern], ty: &Ty, owned: bool) -> Pat {
        use ast::PatternKind as P;
        let (elem, length) = match ty {
            Ty::Array(t, n) => ((**t).clone(), Some(*n)),
            Ty::Slice(t) => ((**t).clone(), None),
            _ => {
                if *ty != Ty::Error {
                    self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{ty}`, found an array pattern"));
                }
                // The names the pattern binds are still declared, so a use of them is not reported as unknown.
                for e in elems {
                    match &e.kind {
                        P::Rest(Some(name)) => {
                            self.declare_binding(name, &Ty::Error, false, owned);
                        }
                        _ => {
                            self.pattern(e, &Ty::Error, owned);
                        }
                    }
                }
                return Pat::Wild;
            }
        };
        let rest = elems.iter().position(|e| matches!(e.kind, P::Rest(_)));
        let named = elems.iter().filter(|e| !matches!(e.kind, P::Rest(_))).count();
        let (prefix_elems, suffix_elems): (&[ast::Pattern], Vec<&ast::Pattern>) = match rest {
            Some(r) => (&elems[..r], elems[r + 1..].iter().filter(|e| !matches!(e.kind, P::Rest(_))).collect()),
            None => (elems, Vec::new()),
        };
        let fits = match (length, rest) {
            (Some(n), Some(_)) => named as u64 <= n,
            (Some(n), None) => named as u64 == n,
            (None, _) => true,
        };
        if !fits {
            let n = length.unwrap_or(0);
            let expected = if rest.is_some() { format!("at most {n}") } else { n.to_string() };
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!(
                    "expected `{ty}`, found an array pattern with {named} element(s) where {expected} are expected"
                ),
            );
            for e in elems {
                self.pattern(e, &Ty::Error, owned);
            }
            return Pat::Wild;
        }
        // The rest's type: the elements it stands for. An array rest that stands for none ignores nothing (E0011).
        let stands = length.map(|n| n - named as u64);
        if let (Some(0), Some(r)) = (stands, rest) {
            self.diags.error(Code::REST_IGNORES_NOTHING, elems[r].span, "this `..` stands for no element");
        }
        let rest_ty = match stands {
            Some(k) => Ty::Array(Box::new(elem.clone()), k),
            None => Ty::Slice(Box::new(elem.clone())),
        };
        let prefix: Vec<Pat> = prefix_elems.iter().map(|e| self.sub_pattern(Some(e), &elem, owned)).collect();
        let rest_pat = rest.map(|r| match &elems[r].kind {
            P::Rest(Some(name)) => {
                let (id, mode) = self.declare_binding(name, &rest_ty, false, owned);
                Box::new(Pat::Binding(id, mode))
            }
            _ => Box::new(Pat::Wild),
        });
        let suffix: Vec<Pat> = suffix_elems.into_iter().map(|e| self.sub_pattern(Some(e), &elem, owned)).collect();
        Pat::Slice { prefix, rest: rest_pat, suffix }
    }

    /// The sub-pattern for one position of a tuple, variant, or `Some`/`Ok`/`Err` pattern: a wildcard where a `..`
    /// stands for the position (`spec/rest-patterns.md`, RP1), otherwise the written pattern.
    fn sub_pattern(&mut self, p: Option<&ast::Pattern>, ty: &Ty, owned: bool) -> Pat {
        match p {
            Some(p) => self.pattern(p, ty, owned),
            None => Pat::Wild,
        }
    }

    /// The positions of a tuple-like pattern with `arity` positions: one entry per position, `None` where a `..`
    /// stands for it. The first `..` is the one that counts; a later one was reported by the parser (E0010) and is
    /// skipped here. `None` when the number of named positions does not fit `arity`, which the caller reports. A
    /// `..` that stands for no position is E0011 (RP4).
    fn rest_positions<'a>(&mut self, elems: &'a [ast::Pattern], arity: usize) -> Option<Vec<Option<&'a ast::Pattern>>> {
        use ast::PatternKind as P;
        // A name on a rest is only allowed in an array or slice pattern (E0016): a tuple has no rest binding.
        for e in elems {
            if let P::Rest(Some(name)) = &e.kind {
                self.diags.error(
                    Code::NAMED_REST,
                    e.span,
                    format!("`..{}` binds a rest only in an array or slice pattern", name.name),
                );
            }
        }
        let Some(rest) = elems.iter().position(|e| matches!(e.kind, P::Rest(_))) else {
            return (elems.len() == arity).then(|| elems.iter().map(Some).collect());
        };
        let named = elems.iter().filter(|e| !matches!(e.kind, P::Rest(_))).count();
        if named > arity {
            return None;
        }
        let stands = arity - named;
        if stands == 0 {
            self.diags.error(Code::REST_IGNORES_NOTHING, elems[rest].span, "this `..` stands for no element");
        }
        let mut out: Vec<Option<&ast::Pattern>> = elems[..rest].iter().map(Some).collect();
        out.extend(std::iter::repeat_n(None, stands));
        out.extend(elems[rest + 1..].iter().filter(|e| !matches!(e.kind, P::Rest(_))).map(Some));
        Some(out)
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
