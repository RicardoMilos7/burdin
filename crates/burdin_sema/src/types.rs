//! Resolving written types, and the questions about types the checker asks: `==`, `Copy`, `Clone`, and so on.

use crate::builtins::builtin_type;
use crate::globals::{Globals, TypeName};
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{Ty, TypeDefs};

impl Globals {
    /// The type a user-declared type name stands for, as seen from `module`.
    pub(crate) fn named_type(&self, module: usize, name: &str) -> Option<Ty> {
        Some(match self.type_name(module, name)? {
            TypeName::Struct(i) => Ty::Struct(self.struct_refs[i].clone()),
            TypeName::Enum(i) => Ty::Enum(self.enum_refs[i].clone()),
            TypeName::Alias(i) => self.aliases[i].clone(),
            TypeName::GenericAdt(_) => return None,
        })
    }

    /// Resolves a written type. `self_ty` is what `Self` means here (inside a method block), and `module` is the
    /// module whose names and imports apply.
    pub(crate) fn resolve_type(
        &self,
        ty: &ast::Type,
        self_ty: Option<&Ty>,
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        self.resolve_type_in(ty, self_ty, &[], module, diags)
    }

    /// Like `resolve_type`, with what the type parameters in scope stand for.
    pub(crate) fn resolve_type_in(
        &self,
        ty: &ast::Type,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        match &ty.kind {
            ast::TypeKind::Unit => Ty::Unit,
            ast::TypeKind::Never => Ty::Never,
            ast::TypeKind::SelfType => match self_ty {
                Some(t) => t.clone(),
                None => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, "`Self` is only available inside a method block");
                    Ty::Error
                }
            },
            ast::TypeKind::Path(segments) if segments.len() == 1 => {
                self.resolve_path_segment(module, &segments[0], ty.span, self_ty, subst, diags)
            }
            // `geometry.shapes.Square`, through a module: the last segment is the type.
            ast::TypeKind::Path(segments) => {
                let names: Vec<String> = segments[..segments.len() - 1].iter().map(|s| s.name.name.clone()).collect();
                let Some(target) = self.resolve_module_path(module, &names) else {
                    diags.error(Code::UNKNOWN_NAME, ty.span, format!("unknown module `{}`", names.join(".")));
                    return Ty::Error;
                };
                let last = segments.last().expect("the path has a last segment");
                if self.package_of(target) != self.package_of(module) && self.type_is_private(target, &last.name.name) {
                    diags.error(
                        Code::PRIVATE_ITEM,
                        ty.span,
                        format!("type `{}` is `priv` in another package", last.name.name),
                    );
                }
                self.resolve_exported_segment(target, last, ty.span, self_ty, subst, diags)
            }
            ast::TypeKind::Ref { mutable, ty: inner } => {
                let inner = self.resolve_type_in(inner, self_ty, subst, module, diags);
                Ty::Ref { mutable: *mutable, inner: Box::new(inner) }
            }
            ast::TypeKind::Slice(elem) => {
                Ty::Slice(Box::new(self.resolve_type_in(elem, self_ty, subst, module, diags)))
            }
            ast::TypeKind::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|e| self.resolve_type_in(e, self_ty, subst, module, diags)).collect())
            }
            ast::TypeKind::Array(elem, len) => {
                let elem = self.resolve_type_in(elem, self_ty, subst, module, diags);
                match len.kind {
                    ast::ExprKind::Int(n) if n <= u128::from(u32::MAX) => Ty::Array(Box::new(elem), n as u64),
                    _ => {
                        diags.error(
                            Code::UNSUPPORTED,
                            len.span,
                            "array lengths other than integer literals are not implemented in this compiler yet",
                        );
                        Ty::Error
                    }
                }
            }
        }
    }

    /// One segment of a type path, resolved in `module`: a type parameter, a user type, or a built-in type.
    fn resolve_path_segment(
        &self,
        module: usize,
        seg: &ast::PathSegment,
        span: Span,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        diags: &mut Diagnostics,
    ) -> Ty {
        let name = seg.name.name.as_str();
        if let Some((_, t)) = subst.iter().find(|(n, _)| n == name) {
            if !seg.args.is_empty() {
                diags.error(Code::TYPE_MISMATCH, span, format!("`{name}` is a type parameter and takes no arguments"));
                return Ty::Error;
            }
            return t.clone();
        }
        if self.type_name(module, name).is_none()
            && let Some(ty) = self.builtin_generic_type(name, &seg.args, self_ty, subst, module, diags)
        {
            return ty;
        }
        if let Some(TypeName::GenericAdt(g)) = self.type_name(module, name) {
            return self.generic_adt_type(g, span, &seg.args, self_ty, subst, module, diags);
        }
        if !seg.args.is_empty() {
            diags.error(Code::UNSUPPORTED, span, format!("`{name}` is not a generic type"));
            return Ty::Error;
        }
        if let Some(t) = self.named_type(module, name).or_else(|| builtin_type(name)) {
            return t;
        }
        diags.error(Code::UNKNOWN_NAME, span, format!("unknown type `{name}`"));
        Ty::Error
    }

    /// One segment of a type path that is qualified by a module: only the names the target module *declares* are
    /// visible (a module's `use` items are private to it).
    fn resolve_exported_segment(
        &self,
        target: usize,
        seg: &ast::PathSegment,
        span: Span,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        diags: &mut Diagnostics,
    ) -> Ty {
        let name = seg.name.name.as_str();
        match self.exported_type(target, name) {
            Some(TypeName::GenericAdt(g)) => self.generic_adt_type(g, span, &seg.args, self_ty, subst, target, diags),
            Some(TypeName::Struct(_) | TypeName::Enum(_) | TypeName::Alias(_)) => {
                if !seg.args.is_empty() {
                    diags.error(Code::UNSUPPORTED, span, format!("`{name}` is not a generic type"));
                    return Ty::Error;
                }
                self.named_type(target, name).unwrap_or(Ty::Error)
            }
            None => {
                diags.error(Code::UNKNOWN_NAME, span, format!("unknown type `{name}`"));
                Ty::Error
            }
        }
    }

    /// `Stack<i32>`: the instance of generic type `g` for these written arguments.
    // The arguments are the whole resolution context (what `Self` means, the type parameters in scope, the module,
    // and the diagnostics). Splitting them into a struct would only move the same values around.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generic_adt_type(
        &self,
        g: usize,
        span: Span,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let info = &self.generic_adts[g];
        if args.len() != info.params.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes {} type argument(s), but {} were given", info.name, info.params.len(), args.len()),
            );
            return Ty::Error;
        }
        let args: Vec<Ty> = args.iter().map(|a| self.resolve_type_in(a, self_ty, subst, module, diags)).collect();
        self.adt_instance_ty(g, args)
    }

    pub(crate) fn defs(&self) -> TypeDefs<'_> {
        TypeDefs { structs: &self.structs, enums: &self.enums }
    }

    /// The type a path with generic arguments stands for, as written in an expression (`List<i32>.new()`).
    pub(crate) fn generic_type(
        &self,
        name: &ast::Ident,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let n = name.name.as_str();
        if let Some(TypeName::GenericAdt(g)) = self.type_name(module, n) {
            return self.generic_adt_type(g, name.span, args, self_ty, subst, module, diags);
        }
        if self.type_name(module, n).is_some() {
            diags.error(Code::UNSUPPORTED, name.span, format!("`{n}` is not a generic type"));
            return Ty::Error;
        }
        self.builtin_generic_type(n, args, self_ty, subst, module, diags).unwrap_or_else(|| {
            diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{n}`"));
            Ty::Error
        })
    }

    /// The built-in generic types written with arguments (`Option<T>`, `Heap<T>`, and the standard-library
    /// containers), or `None` if `name` is not one of them with this many arguments.
    fn builtin_generic_type(
        &self,
        name: &str,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<Ty> {
        Some(match (name, args.len()) {
            ("Option", 1) => Ty::Option(Box::new(self.resolve_type_in(&args[0], self_ty, subst, module, diags))),
            ("Result", 2) => Ty::Result(
                Box::new(self.resolve_type_in(&args[0], self_ty, subst, module, diags)),
                Box::new(self.resolve_type_in(&args[1], self_ty, subst, module, diags)),
            ),
            ("Heap", 1) => {
                let inner = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                check_storable(&inner, args[0].span, "a `Heap` element", diags);
                Ty::Heap(Box::new(inner))
            }
            ("CPtr", 1) => Ty::CPtr(Box::new(self.resolve_type_in(&args[0], self_ty, subst, module, diags))),
            ("List", 1) => {
                let elem = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                check_storable(&elem, args[0].span, "a `List` element", diags);
                Ty::List(Box::new(elem))
            }
            ("Map", 2) => {
                let key = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                let value = self.resolve_type_in(&args[1], self_ty, subst, module, diags);
                check_storable(&key, args[0].span, "a `Map` key", diags);
                check_storable(&value, args[1].span, "a `Map` value", diags);
                Ty::Map(Box::new(key), Box::new(value))
            }
            _ => return None,
        })
    }

    /// Whether `==` and `!=` apply to values of `ty`.
    pub(crate) fn has_eq(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Error | Ty::Never => true,
            // Text compares by value; `String` and `str` compare byte for byte.
            Ty::Str | Ty::String => true,
            Ty::Option(t) => self.has_eq(t),
            Ty::Result(t, e) => self.has_eq(t) && self.has_eq(e),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_eq(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.eq && s.fields.iter().all(|f| self.has_eq(&f.ty))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.eq && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_eq(&f.ty)))
            }
            Ty::Array(..) | Ty::Heap(_) | Ty::CPtr(_) | Ty::CString | Ty::List(_) | Ty::Map(..) | Ty::Chars => false,
            Ty::Ref { .. } | Ty::Slice(_) | Ty::Param(_) => false,
        }
    }

    /// Whether values of `ty` are copied instead of moved (memory-rules.md, Copy types).
    pub(crate) fn is_copy(&self, ty: &Ty) -> bool {
        self.defs().is_copy(ty)
    }

    /// Whether `x.clone()` applies to values of `ty` (the `Clone` requirement of derives, CP).
    pub(crate) fn is_clone(&self, ty: &Ty) -> bool {
        self.defs().is_clone(ty)
    }

    /// Whether the type itself declares `Type: Drop { ... }` (fields cannot be moved out, DR8).
    pub(crate) fn has_drop_impl(&self, ty: &Ty) -> bool {
        self.defs().has_drop_impl(ty)
    }
}

pub(crate) fn compatible(actual: &Ty, expected: &Ty) -> bool {
    actual == expected || *actual == Ty::Error || *expected == Ty::Error || *actual == Ty::Never
}

/// Reports E0120 when a view type is used where a stored type is required (memory-rules.md, SC2).
pub(crate) fn check_storable(ty: &Ty, span: Span, what: &str, diags: &mut Diagnostics) {
    if ty.is_second_class() {
        diags.error(
            Code::SECOND_CLASS_POSITION,
            span,
            format!("`{ty}` is a view of other storage, so it cannot be used as {what}"),
        );
    }
}
