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
            TypeName::GenericAdt(_) | TypeName::GenericAlias(_) => return None,
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
            // A value where a type is expected: the const arguments of a generic type are read by its path, not here.
            ast::TypeKind::Const(_) => {
                diags.error(
                    Code::INVALID_CONST_GENERIC,
                    ty.span,
                    "a value is not a type; it can only be a const generic argument",
                );
                Ty::Error
            }
            // `any Interface`: the owned boxed existential (spec/existentials.md, AN1).
            ast::TypeKind::Any(inner) => self.resolve_any(inner, ty.span, subst, module, diags),
            // The parser desugars `impl` in a parameter list; anywhere else it reaches here (AN4).
            ast::TypeKind::Impl(_) => {
                diags.error(
                    Code::IMPL_NOT_PARAMETER,
                    ty.span,
                    "`impl Interface` is only allowed on a parameter; use `any Interface` for a stored value",
                );
                Ty::Error
            }
            ast::TypeKind::SelfType => match self_ty {
                Some(t) => t.clone(),
                None => {
                    diags.error(Code::UNKNOWN_NAME, ty.span, "`Self` is only available inside a method block");
                    Ty::Error
                }
            },
            ast::TypeKind::Fn { kind, params, ret } => {
                let params = params.iter().map(|p| self.resolve_type_in(p, self_ty, subst, module, diags)).collect();
                let ret =
                    ret.as_ref().map(|r| self.resolve_type_in(r, self_ty, subst, module, diags)).unwrap_or(Ty::Unit);
                Ty::Fn {
                    sig: std::rc::Rc::new(burdin_tir::FnSig { params, ret: Box::new(ret) }),
                    kind: crate::generics::ast_kind(*kind),
                    borrowed: false,
                }
            }
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
                match self.eval_array_len(len, subst, module, diags) {
                    Some(n) => Ty::Array(Box::new(elem), n),
                    None => Ty::Error,
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
        if let Some(TypeName::GenericAlias(a)) = self.type_name(module, name) {
            return self.generic_alias_type(a, span, &seg.args, self_ty, subst, module, diags);
        }
        if !seg.args.is_empty() {
            diags.error(Code::NOT_A_GENERIC_TYPE, span, format!("`{name}` is not a generic type"));
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
            Some(TypeName::GenericAlias(a)) => {
                self.generic_alias_type(a, span, &seg.args, self_ty, subst, target, diags)
            }
            Some(TypeName::Struct(_) | TypeName::Enum(_) | TypeName::Alias(_)) => {
                if !seg.args.is_empty() {
                    diags.error(Code::NOT_A_GENERIC_TYPE, span, format!("`{name}` is not a generic type"));
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
        let missing_defaulted = args.len() <= info.params.len()
            && info.defaults[args.len().min(info.defaults.len())..].iter().all(Option::is_some);
        if args.len() > info.params.len() || (args.len() < info.params.len() && !missing_defaulted) {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes {} type argument(s), but {} were given", info.name, info.params.len(), args.len()),
            );
            return Ty::Error;
        }
        let mut resolved: Vec<Ty> = args
            .iter()
            .zip(&info.bounds)
            .map(|(a, bounds)| match Globals::const_param_ty(bounds) {
                Some(declared) => self.eval_const_arg(a, declared, subst, module, diags),
                None => self.resolve_type_in(a, self_ty, subst, module, diags),
            })
            .collect();
        // A trailing type argument left out takes the parameter's default (`spec/allocators.md`, AL5). The
        // default is resolved where the type is declared, with the preceding parameters in scope.
        let mut env: Vec<(String, Ty)> = subst.to_vec();
        for (name, ty) in info.params.iter().zip(&resolved) {
            env.push((name.clone(), ty.clone()));
        }
        for i in resolved.len()..info.params.len() {
            let Some(default) = &info.defaults[i] else { break };
            let ty = self.resolve_type_in(default, self_ty, &env, info.module, diags);
            resolved.push(ty.clone());
            env.push((info.params[i].clone(), ty));
        }
        self.check_adt_bounds(g, &resolved, span, diags);
        self.adt_instance_ty(g, resolved)
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
        if let Some(TypeName::GenericAlias(a)) = self.type_name(module, n) {
            return self.generic_alias_type(a, name.span, args, self_ty, subst, module, diags);
        }
        if self.type_name(module, n).is_some() {
            diags.error(Code::NOT_A_GENERIC_TYPE, name.span, format!("`{n}` is not a generic type"));
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
            // `Result<T>` is the default-error form: `Result<T, Error>` (`spec/errors.md`, ERR5).
            ("Result", 1) => Ty::Result(
                Box::new(self.resolve_type_in(&args[0], self_ty, subst, module, diags)),
                Box::new(self.error_ty.clone().unwrap_or(Ty::Error)),
            ),
            ("Heap", 1) | ("Heap", 2) => {
                let inner = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                check_storable(&inner, args[0].span, "a `Heap` element", diags);
                let alloc = self.container_alloc(args.get(1), subst, module, diags);
                Ty::Heap(Box::new(inner), alloc)
            }
            ("Shared", 1) => {
                let inner = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                check_storable(&inner, args[0].span, "a `Shared` value", diags);
                Ty::Shared(Box::new(inner))
            }
            ("CPtr", 1) => Ty::CPtr(Box::new(self.resolve_type_in(&args[0], self_ty, subst, module, diags))),
            ("Range", 1) | ("RangeInclusive", 1) => {
                let inner = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                if !matches!(inner, Ty::Int(_) | Ty::Error) {
                    diags.error(
                        Code::TYPE_MISMATCH,
                        args[0].span,
                        format!("a range's endpoints must be integers, so `{inner}` cannot be a range element"),
                    );
                    return Some(Ty::Error);
                }
                Ty::Range(Box::new(inner), name == "RangeInclusive")
            }
            ("List", 1) | ("List", 2) => {
                let elem = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                check_storable(&elem, args[0].span, "a `List` element", diags);
                let alloc = self.container_alloc(args.get(1), subst, module, diags);
                Ty::List(Box::new(elem), alloc)
            }
            ("Map", 2) | ("Map", 3) => {
                let key = self.resolve_type_in(&args[0], self_ty, subst, module, diags);
                let value = self.resolve_type_in(&args[1], self_ty, subst, module, diags);
                check_storable(&key, args[0].span, "a `Map` key", diags);
                check_storable(&value, args[1].span, "a `Map` value", diags);
                let alloc = self.container_alloc(args.get(2), subst, module, diags);
                Ty::Map(Box::new(key), Box::new(value), alloc)
            }
            _ => return None,
        })
    }

    /// The allocator a container type argument names (`spec/allocators.md`, AL3). A written `Global` is the
    /// default (no value is stored), and an allocator that does not implement `Allocator` is E0270.
    fn container_alloc(
        &self,
        arg: Option<&ast::Type>,
        subst: &[(String, Ty)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> burdin_tir::Alloc {
        let Some(arg) = arg else { return burdin_tir::Alloc::Global };
        let ty = self.resolve_type_in(arg, None, subst, module, diags);
        if ty == Ty::Error {
            return burdin_tir::Alloc::Global;
        }
        check_storable(&ty, arg.span, "an allocator", diags);
        let alloc = self.alloc_of(ty.clone());
        // A type parameter is checked where its bounds are known (the declaration or the call site); a concrete
        // allocator needs a visible `Allocator` implementation now.
        if let burdin_tir::Alloc::Value(a) = &alloc
            && !self.ty_has_param(a)
            && !self.is_allocator(a, &[])
        {
            diags.error(
                Code::NOT_AN_ALLOCATOR,
                arg.span,
                format!("`{a}` does not implement `Allocator`, so it cannot be a container's allocator"),
            );
        }
        alloc
    }

    /// Whether `==` and `!=` apply to values of `ty`.
    pub(crate) fn has_eq(&self, ty: &Ty) -> bool {
        self.has_eq_in(ty, &[])
    }

    /// Whether `==` and `!=` apply to values of `ty`, with the bounds of the type parameters in scope. A derived
    /// `Eq` on `Wrapper<T>` holds when the fields do, which is what a `T: Eq` bound provides (issue #69).
    pub(crate) fn has_eq_in(&self, ty: &Ty, env: &crate::generics::Env) -> bool {
        match ty {
            Ty::Const(_) | Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Error | Ty::Never => true,
            // Text compares by value; `String` and `str` compare byte for byte.
            Ty::Str | Ty::String => true,
            Ty::Param(name) => self.bound_in_env(name, &crate::generics::Bound::Eq, env),
            Ty::Option(t) => self.has_eq_in(t, env),
            Ty::Range(t, _) => self.has_eq_in(t, env),
            Ty::Result(t, e) => self.has_eq_in(t, env) && self.has_eq_in(e, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_eq_in(t, env)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.eq && s.fields.iter().all(|f| self.has_eq_in(&f.ty, env))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.eq && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_eq_in(&f.ty, env)))
            }
            Ty::Array(..)
            | Ty::Heap(_, _)
            | Ty::CPtr(_)
            | Ty::CString
            | Ty::List(_, _)
            | Ty::Map(..)
            | Ty::Shared(_)
            | Ty::Chars
            | Ty::Fn { .. }
            | Ty::ListIntoIter(..)
            | Ty::ArrayIntoIter(..)
            | Ty::MapIter(..)
            | Ty::MapIntoIter(..)
            | Ty::RangeIter(..)
            | Ty::Any(_) => false,
            Ty::Ref { .. } | Ty::Slice(_) => false,
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
    actual == expected
        || *actual == Ty::Error
        || *expected == Ty::Error
        || *actual == Ty::Never
        || fn_compatible(actual, expected)
}

/// Whether a closure value fits a closure type: the same signature and capture mode, at a capability the value
/// provides (CL5). A closure value may be used where a weaker call capability is expected.
pub(crate) fn fn_compatible(actual: &Ty, expected: &Ty) -> bool {
    match (actual, expected) {
        (Ty::Fn { sig: asig, kind: ak, borrowed: ab }, Ty::Fn { sig: esig, kind: ek, borrowed: eb }) => {
            asig == esig && ab == eb && ak.at_least(*ek)
        }
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::FileId;
    use burdin_tir::IntTy;

    #[test]
    fn compatible_allows_error_and_never_but_not_mismatches() {
        let i32_ = Ty::Int(IntTy::I32);
        assert!(compatible(&i32_, &i32_));
        assert!(compatible(&Ty::Error, &i32_));
        assert!(compatible(&i32_, &Ty::Error));
        assert!(compatible(&Ty::Never, &i32_));
        assert!(!compatible(&i32_, &Ty::Never), "only an actual `!` adapts");
        assert!(!compatible(&Ty::Bool, &i32_));
        assert!(!compatible(&Ty::Str, &Ty::String), "String coerces to str, not the other way");
    }

    #[test]
    fn storable_rejects_view_types() {
        let span = Span::new(FileId(0), 0, 0);
        let mut diags = Diagnostics::default();
        check_storable(&Ty::Str, span, "a field", &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags.iter().next().unwrap().code, Code::SECOND_CLASS_POSITION);
        check_storable(&Ty::String, span, "a field", &mut diags);
        assert_eq!(diags.len(), 1, "an owned type is storable");
    }
}
