//! Resolving written types, and the questions about types the checker asks: `==`, `Copy`, `Clone`, and so on.

use crate::builtins::builtin_type;
use crate::globals::{Globals, TypeName};
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{Ty, TypeDefs};
use std::rc::Rc;

/// The type parameters a built-in generic type must be written with, in order (`K` and `V` for `Map`). The
/// allocator a container takes after them may be left out, so it is not listed. `None` for any other name.
pub(crate) fn builtin_generic_params(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "Option" | "Result" | "Heap" | "List" | "CPtr" | "Range" | "RangeInclusive" => &["T"],
        "Map" => &["K", "V"],
        _ => return None,
    })
}

/// The error for a generic type named without the type arguments it needs (E0003). `missing` are the parameters
/// the written type still lacks, and `required` are the ones it is written with, so the message names what is
/// missing and shows the form to write (`Map<K, V>`).
pub(crate) fn generic_args_missing<S: AsRef<str>>(
    name: &str,
    missing: &[S],
    required: &[S],
    span: Span,
    diags: &mut Diagnostics,
) {
    let join = |names: &[S]| names.iter().map(AsRef::as_ref).collect::<Vec<&str>>().join(", ");
    diags.error(
        Code::TYPE_MISMATCH,
        span,
        format!(
            "`{name}` is generic and needs type arguments for `{}`; write them, as in `{name}<{}>`",
            join(missing),
            join(required)
        ),
    );
}

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
        self.resolve_type_bounded(ty, self_ty, subst, &[], module, diags)
    }

    /// Like `resolve_type_in`, with the bounds of the type parameters in scope. They are what an unqualified
    /// associated type projection (`T.Item`) resolves through (`spec/associated-types.md`, AT3).
    pub(crate) fn resolve_type_bounded(
        &self,
        ty: &ast::Type,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
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
            // The parser desugars `impl` in a parameter list, and a return type is made by `crate::opaque`;
            // anywhere else it reaches here (AN4).
            ast::TypeKind::Impl(_) => {
                diags.error(
                    Code::IMPL_NOT_PARAMETER,
                    ty.span,
                    "`impl Interface` is only allowed on a parameter or a return type; use `any Interface` for a stored value",
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
            ast::TypeKind::Projection(p) => {
                let base = self.resolve_type_bounded(&p.self_ty, self_ty, subst, bounds, module, diags);
                self.resolve_assoc_projection(base, p.iface.as_deref(), &p.name, subst, bounds, module, diags)
            }
            ast::TypeKind::Fn { kind, params, ret } => {
                let params = params
                    .iter()
                    .map(|p| self.resolve_type_bounded(p, self_ty, subst, bounds, module, diags))
                    .collect();
                let ret = ret
                    .as_ref()
                    .map(|r| self.resolve_type_bounded(r, self_ty, subst, bounds, module, diags))
                    .unwrap_or(Ty::Unit);
                Ty::Fn {
                    sig: std::rc::Rc::new(burdin_tir::FnSig { params, ret: Box::new(ret) }),
                    kind: crate::generics::ast_kind(*kind),
                    borrowed: false,
                    send: burdin_tir::FnSend::unknown(),
                }
            }
            ast::TypeKind::Path(segments) if segments.len() == 1 => {
                self.resolve_path_segment(module, &segments[0], ty.span, self_ty, subst, bounds, diags)
            }
            // `geometry.shapes.Square`, through a module: the last segment is the type. When the prefix is not a
            // module, `T.Name` may be the shorthand for an associated type projection (`spec/associated-types.md`,
            // AT3).
            ast::TypeKind::Path(segments) => {
                let names: Vec<String> = segments[..segments.len() - 1].iter().map(|s| s.name.name.clone()).collect();
                let Some(target) = self.resolve_module_path(module, &names) else {
                    if let Some(ty) = self.resolve_shorthand_projection(segments, self_ty, subst, bounds, module, diags)
                    {
                        return ty;
                    }
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
                self.resolve_exported_segment(target, last, ty.span, self_ty, subst, bounds, diags)
            }
            ast::TypeKind::Ref { mutable, ty: inner } => {
                let inner = self.resolve_type_bounded(inner, self_ty, subst, bounds, module, diags);
                Ty::Ref { mutable: *mutable, inner: Box::new(inner) }
            }
            ast::TypeKind::Slice(elem) => {
                Ty::Slice(Box::new(self.resolve_type_bounded(elem, self_ty, subst, bounds, module, diags)))
            }
            ast::TypeKind::Tuple(elems) => Ty::Tuple(
                elems.iter().map(|e| self.resolve_type_bounded(e, self_ty, subst, bounds, module, diags)).collect(),
            ),
            ast::TypeKind::Array(elem, len) => {
                let elem = self.resolve_type_bounded(elem, self_ty, subst, bounds, module, diags);
                match self.eval_array_len(len, subst, module, diags) {
                    Some(n) => Ty::Array(Box::new(elem), n),
                    None => Ty::Error,
                }
            }
        }
    }

    /// `T.Name`, written as a type path whose prefix is not a module: a shorthand for the unqualified associated
    /// type projection (`spec/associated-types.md`, AT3). Returns `None` when the prefix is not a type at all.
    fn resolve_shorthand_projection(
        &self,
        segments: &[ast::PathSegment],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<Ty> {
        let [base, name] = segments else { return None };
        // The prefix is a type parameter or a type name; a module path was already tried and failed.
        let is_type = subst.iter().any(|(n, _)| *n == base.name.name)
            || self.type_name(module, &base.name.name).is_some()
            || crate::builtins::builtin_type(&base.name.name).is_some()
            || matches!(base.name.name.as_str(), "Option" | "Result" | "List" | "Map" | "Heap" | "CPtr");
        if !is_type || !base.args.is_empty() {
            return None;
        }
        let base_ty = self.resolve_path_segment(module, base, base.name.span, self_ty, subst, bounds, diags);
        if !name.args.is_empty() {
            diags.error(
                Code::TYPE_MISMATCH,
                name.name.span,
                format!("the associated type `{}` takes no type arguments", name.name.name),
            );
        }
        Some(self.resolve_assoc_projection(base_ty, None, &name.name, subst, bounds, module, diags))
    }

    /// `<T as TryFrom<S>>.Error` (`spec/stdlib.md`, TF9): the error of the conversion from `S` into `T`. A type
    /// parameter must have the `TryFrom<S>` bound and stays symbolic until it is concrete. A concrete type resolves
    /// now to the error of its conversion (`TryFromIntError` for an integer, else the block's `E`), or is E0141 when
    /// there is none. A type that still mentions a parameter stays symbolic.
    fn try_from_error_projection(
        &self,
        self_ty: Ty,
        source: Ty,
        assoc: &str,
        span: Span,
        bounds: &[(String, Vec<crate::generics::Bound>)],
        diags: &mut Diagnostics,
    ) -> Ty {
        use crate::generics::Bound;
        if assoc != "Error" {
            let message = format!("`TryFrom` declares one associated type, `Error`, not `{assoc}`");
            diags.error(Code::ASSOC_PROJECTION, span, message);
            return Ty::Error;
        }
        if let Ty::Param(p) = &self_ty {
            if !self.bound_in_env(p, &Bound::TryFrom(source.clone()), bounds) {
                let message = format!("`{p}` does not implement `TryFrom<{source}>`, so its `Error` is not available");
                diags.error(Code::BOUND_NOT_SATISFIED, span, message);
                return Ty::Error;
            }
            return Ty::TryFromError { self_ty: Box::new(self_ty), source: Box::new(source) };
        }
        if self_ty == Ty::Error || source == Ty::Error {
            return Ty::Error;
        }
        if self.ty_has_param(&self_ty) || self.ty_has_param(&source) {
            return Ty::TryFromError { self_ty: Box::new(self_ty), source: Box::new(source) };
        }
        match self.try_from_error_of(&self_ty, &source) {
            Some(ty) => ty,
            None => {
                let message = format!("`{self_ty}` does not convert from `{source}`, so it has no `TryFrom` error");
                diags.error(Code::BOUND_NOT_SATISFIED, span, message);
                Ty::Error
            }
        }
    }

    /// Resolves `self_ty.Name` or `<self_ty as Interface>.Name` (`spec/associated-types.md`, AT3).
    #[allow(clippy::too_many_arguments)]
    fn resolve_assoc_projection(
        &self,
        self_ty: Ty,
        iface_ast: Option<&ast::Type>,
        name: &ast::Ident,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        use crate::generics::Bound;
        if self_ty == Ty::Error {
            return Ty::Error;
        }
        let assoc = name.name.as_str();
        if let Some(iface_ast) = iface_ast {
            // `resolve_bound` reports an unknown or malformed interface itself; a built-in bound (`Copy`) is a
            // valid bound but not an interface with associated types.
            let Some(bound) = self.resolve_bound(iface_ast, subst, module, diags) else {
                return Ty::Error;
            };
            if let Bound::TryFrom(source) = &bound {
                let source = source.clone();
                return self.try_from_error_projection(self_ty, source, assoc, name.span, bounds, diags);
            }
            let Bound::User(iface, args) = bound else {
                diags.error(
                    Code::ASSOC_PROJECTION,
                    name.span,
                    format!("the type after `as` is not an interface, so it declares no associated type `{assoc}`"),
                );
                return Ty::Error;
            };
            let iface_name = &self.interfaces[iface].name;
            if !self.interfaces[iface].assoc.iter().any(|n| n == assoc) {
                diags.error(
                    Code::ASSOC_PROJECTION,
                    name.span,
                    format!("interface `{iface_name}` has no associated type `{assoc}`"),
                );
                return Ty::Error;
            }
            if let Ty::Param(p) = &self_ty {
                // A qualified projection on a type parameter needs the parameter to promise the interface; the
                // `requires` closure counts.
                let want = Bound::User(iface, args.clone());
                if !self.bound_in_env(p, &want, bounds) {
                    diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        name.span,
                        format!(
                            "`{p}` does not implement `{}`, so `{p}.{assoc}` is not available",
                            self.bound_name(&want)
                        ),
                    );
                    return Ty::Error;
                }
                return Ty::Projection {
                    self_ty: Box::new(self_ty),
                    iface,
                    iface_name: Rc::from(iface_name.as_str()),
                    args,
                    name: Rc::from(assoc),
                };
            }
            return match self.impl_assoc(iface, &args, &self_ty, assoc) {
                Some(Some(ty)) => ty,
                // The implementation exists but does not bind the type: E0144 was already reported.
                Some(None) => Ty::Error,
                None => {
                    diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        name.span,
                        format!(
                            "`{self_ty}` does not implement `{iface_name}`, so it has no associated type `{assoc}`"
                        ),
                    );
                    Ty::Error
                }
            };
        }
        // An unqualified projection (`Self.Name`, `T.Name`).
        if let Ty::Param(p) = &self_ty {
            let candidates = self.assoc_candidates_in_env(p, assoc, bounds);
            return match candidates.as_slice() {
                [] => {
                    diags.error(
                        Code::ASSOC_PROJECTION,
                        name.span,
                        format!("no interface among the bounds of `{p}` declares the associated type `{assoc}`"),
                    );
                    Ty::Error
                }
                [(iface, args)] => Ty::Projection {
                    self_ty: Box::new(self_ty),
                    iface: *iface,
                    iface_name: Rc::from(self.interfaces[*iface].name.as_str()),
                    args: args.clone(),
                    name: Rc::from(assoc),
                },
                many => {
                    let names: Vec<&str> = many.iter().map(|(i, _)| self.interfaces[*i].name.as_str()).collect();
                    diags.error(
                        Code::ASSOC_PROJECTION,
                        name.span,
                        format!(
                            "more than one interface among the bounds of `{p}` declares the associated type \
                             `{assoc}` (`{}`); name the interface to disambiguate, as in `<{p} as {}>.{assoc}`",
                            names.join("`, `"),
                            names[0]
                        ),
                    );
                    Ty::Error
                }
            };
        }
        // A concrete type, or an instance with parameters: the interfaces the program implements.
        let candidates = self.assoc_candidates_for(&self_ty, assoc);
        match candidates.as_slice() {
            [] => {
                diags.error(
                    Code::ASSOC_PROJECTION,
                    name.span,
                    format!("no interface implemented for `{self_ty}` declares the associated type `{assoc}`"),
                );
                Ty::Error
            }
            [(_, _, binding)] => match binding {
                Some(ty) => ty.clone(),
                // E0144 was already reported on the implementation.
                None => Ty::Error,
            },
            many => {
                let names: Vec<&str> = many.iter().map(|(i, _, _)| self.interfaces[*i].name.as_str()).collect();
                diags.error(
                    Code::ASSOC_PROJECTION,
                    name.span,
                    format!(
                        "more than one interface implemented for `{self_ty}` declares the associated type `{assoc}` \
                         (`{}`); name the interface to disambiguate, as in `<{self_ty} as {}>.{assoc}`",
                        names.join("`, `"),
                        names[0]
                    ),
                );
                Ty::Error
            }
        }
    }

    /// One segment of a type path, resolved in `module`: a type parameter, a user type, or a built-in type.
    #[allow(clippy::too_many_arguments)]
    fn resolve_path_segment(
        &self,
        module: usize,
        seg: &ast::PathSegment,
        span: Span,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
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
            && let Some(ty) = self.builtin_generic_type(name, &seg.args, self_ty, subst, bounds, module, diags)
        {
            return ty;
        }
        // A built-in generic type written with too few arguments (`Map`, or `Map<i32>`) names what it still needs.
        if self.type_name(module, name).is_none()
            && let Some(required) = builtin_generic_params(name)
            && seg.args.len() < required.len()
        {
            generic_args_missing(name, &required[seg.args.len()..], required, span, diags);
            return Ty::Error;
        }
        if let Some(TypeName::GenericAdt(g)) = self.type_name(module, name) {
            return self.generic_adt_type(g, span, &seg.args, self_ty, subst, bounds, module, diags);
        }
        if let Some(TypeName::GenericAlias(a)) = self.type_name(module, name) {
            return self.generic_alias_type(a, span, &seg.args, self_ty, subst, bounds, module, diags);
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
    #[allow(clippy::too_many_arguments)]
    fn resolve_exported_segment(
        &self,
        target: usize,
        seg: &ast::PathSegment,
        span: Span,
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
        diags: &mut Diagnostics,
    ) -> Ty {
        let name = seg.name.name.as_str();
        match self.exported_type(target, name) {
            Some(TypeName::GenericAdt(g)) => {
                self.generic_adt_type(g, span, &seg.args, self_ty, subst, bounds, target, diags)
            }
            Some(TypeName::GenericAlias(a)) => {
                self.generic_alias_type(a, span, &seg.args, self_ty, subst, bounds, target, diags)
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
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let info = &self.generic_adts[g];
        let missing_defaulted = args.len() <= info.params.len()
            && info.defaults[args.len().min(info.defaults.len())..].iter().all(Option::is_some);
        if args.len() < info.params.len() && !missing_defaulted {
            let has_default = |i: usize| info.defaults.get(i).is_some_and(Option::is_some);
            let missing: Vec<&String> = info
                .params
                .iter()
                .enumerate()
                .skip(args.len())
                .filter(|(i, _)| !has_default(*i))
                .map(|(_, p)| p)
                .collect();
            let required: Vec<&String> =
                info.params.iter().enumerate().filter(|(i, _)| !has_default(*i)).map(|(_, p)| p).collect();
            generic_args_missing(&info.name, &missing, &required, span, diags);
            return Ty::Error;
        }
        if args.len() > info.params.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes {} type argument(s), but {} were given", info.name, info.params.len(), args.len()),
            );
            return Ty::Error;
        }
        let mut resolved: Vec<Ty> = args
            .iter()
            .enumerate()
            .map(|(i, a)| match info.bounds.get(i).and_then(|pb| Globals::const_param_ty(pb)) {
                Some(declared) => self.eval_const_arg(a, declared, subst, module, diags),
                None => self.resolve_type_bounded(a, self_ty, subst, bounds, module, diags),
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
            let ty = self.resolve_type_bounded(default, self_ty, &env, bounds, info.module, diags);
            resolved.push(ty.clone());
            env.push((info.params[i].clone(), ty));
        }
        // A list holds its elements until it is dropped, so they must own their storage, and its allocator must be
        // one. These are checked where the type is written, as for the other containers.
        if Some(g) == self.lang_list || Some(g) == self.lang_list_into_iter {
            if let (Some(elem), Some(arg)) = (resolved.first(), args.first())
                && elem.is_second_class()
            {
                check_storable(elem, arg.span, "a `List` element", diags);
                return Ty::Error;
            }
            if let (Some(alloc), Some(arg)) = (resolved.get(1), args.get(1))
                && *alloc != Ty::Error
                && !self.ty_has_param(alloc)
                && !self.is_allocator(alloc, &[])
            {
                diags.error(
                    Code::NOT_AN_ALLOCATOR,
                    arg.span,
                    format!("`{alloc}` does not implement `Allocator`, so it cannot be a container's allocator"),
                );
                return Ty::Error;
            }
        }
        if !self.check_adt_bounds_upto(g, &resolved, args.len(), span, diags) {
            return Ty::Error;
        }
        // A channel holds the values sent through it until they are received, so they must own their storage. `T`
        // appears in no field of `Channel<T>` (the queue is behind a raw pointer), so the field check cannot see it
        // (spec/channels.md, CH4).
        if self.std_modules.contains(&info.module)
            && info.name == self.qualified(info.module, "Channel")
            && let (Some(ty), Some(arg)) = (resolved.first(), args.first())
            && ty.is_second_class()
        {
            check_storable(ty, arg.span, "a `Channel` value", diags);
            return Ty::Error;
        }
        self.adt_instance_ty(g, resolved)
    }

    pub(crate) fn defs(&self) -> TypeDefs<'_> {
        TypeDefs { structs: &self.structs, enums: &self.enums }
    }

    /// The type a path with generic arguments stands for, as written in an expression (`List<i32>.new()`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generic_type(
        &self,
        name: &ast::Ident,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Ty {
        let n = name.name.as_str();
        if let Some(TypeName::GenericAdt(g)) = self.type_name(module, n) {
            return self.generic_adt_type(g, name.span, args, self_ty, subst, bounds, module, diags);
        }
        if let Some(TypeName::GenericAlias(a)) = self.type_name(module, n) {
            return self.generic_alias_type(a, name.span, args, self_ty, subst, bounds, module, diags);
        }
        if self.type_name(module, n).is_some() {
            diags.error(Code::NOT_A_GENERIC_TYPE, name.span, format!("`{n}` is not a generic type"));
            return Ty::Error;
        }
        if let Some(required) = builtin_generic_params(n)
            && args.len() < required.len()
        {
            generic_args_missing(n, &required[args.len()..], required, name.span, diags);
            return Ty::Error;
        }
        self.builtin_generic_type(n, args, self_ty, subst, bounds, module, diags).unwrap_or_else(|| {
            diags.error(Code::UNKNOWN_NAME, name.span, format!("unknown type `{n}`"));
            Ty::Error
        })
    }

    /// The built-in generic types written with arguments (`Option<T>`, `Heap<T>`, and the standard-library
    /// containers), or `None` if `name` is not one of them with this many arguments.
    #[allow(clippy::too_many_arguments)]
    fn builtin_generic_type(
        &self,
        name: &str,
        args: &[ast::Type],
        self_ty: Option<&Ty>,
        subst: &[(String, Ty)],
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<Ty> {
        Some(match (name, args.len()) {
            ("Heap", 1) | ("Heap", 2) => {
                let inner = self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags);
                check_storable(&inner, args[0].span, "a `Heap` element", diags);
                let alloc = self.container_alloc(args.get(1), subst, bounds, module, diags);
                Ty::Heap(Box::new(inner), alloc)
            }
            // The iterator `map.iter()` returns, which yields `(&K, &V)`. `std` needs its name to write the
            // iterator of a type built on a map (`Set`); `MapIter` itself is the standard library's adapter.
            ("MapEntries", 2) => Ty::MapIter(
                Box::new(self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags)),
                Box::new(self.resolve_type_bounded(&args[1], self_ty, subst, bounds, module, diags)),
            ),
            // The owning iterator `map.into_iter()` returns, which yields `(K, V)`. `std` names it for the
            // `into_keys` and `into_values` views of issue #712.
            ("MapIntoEntries", 2) | ("MapIntoEntries", 3) => {
                let key = self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags);
                let value = self.resolve_type_bounded(&args[1], self_ty, subst, bounds, module, diags);
                let alloc = self.container_alloc(args.get(2), subst, bounds, module, diags);
                Ty::MapIntoIter(Box::new(key), Box::new(value), alloc)
            }
            // The mutable iterator `list.iter_mut()` returns; `std` names it to write `List.iter_mut`.
            ("IterMut", 1) => {
                Ty::IterMut(Box::new(self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags)))
            }
            ("CPtr", 1) => {
                Ty::CPtr(Box::new(self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags)))
            }
            ("Map", 2) | ("Map", 3) => {
                let key = self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags);
                let value = self.resolve_type_bounded(&args[1], self_ty, subst, bounds, module, diags);
                check_storable(&key, args[0].span, "a `Map` key", diags);
                check_storable(&value, args[1].span, "a `Map` value", diags);
                let alloc = self.container_alloc(args.get(2), subst, bounds, module, diags);
                Ty::Map(Box::new(key), Box::new(value), alloc)
            }
            // The type `map.entry(key)` returns (`spec/stdlib.md`, "Map entry operations"). The entry itself is
            // second-class; its key and value are the map's, so they must be storable.
            ("MapEntry", 2) | ("MapEntry", 3) => {
                let key = self.resolve_type_bounded(&args[0], self_ty, subst, bounds, module, diags);
                let value = self.resolve_type_bounded(&args[1], self_ty, subst, bounds, module, diags);
                check_storable(&key, args[0].span, "a `Map` key", diags);
                check_storable(&value, args[1].span, "a `Map` value", diags);
                let alloc = self.container_alloc(args.get(2), subst, bounds, module, diags);
                Ty::MapEntry(Box::new(key), Box::new(value), alloc)
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
        bounds: &[(String, Vec<crate::generics::Bound>)],
        module: usize,
        diags: &mut Diagnostics,
    ) -> burdin_tir::Alloc {
        let Some(arg) = arg else { return burdin_tir::Alloc::Global };
        let ty = self.resolve_type_bounded(arg, None, subst, bounds, module, diags);
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

    /// Whether `ty` is a struct or enum whose derived `Eq` holds: the derive is written and every field has `==`.
    pub(crate) fn has_derived_eq_in(&self, ty: &Ty, env: &crate::generics::Env) -> bool {
        match ty {
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.eq && s.fields.iter().all(|f| self.has_eq_in(&f.ty, env))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.eq && e.variants.iter().all(|v| v.fields.iter().all(|f| self.has_eq_in(&f.ty, env)))
            }
            _ => false,
        }
    }

    /// Whether `ty` can be a `Map` key or a `Set` element. Keys are hashed structurally, so a key whose `==` is
    /// hand-written must also derive `Hash`: that derive is the promise that equal values hash alike
    /// (`spec/derives.md`, issue #918).
    pub(crate) fn has_key_eq_in(&self, ty: &Ty, env: &crate::generics::Env) -> bool {
        self.has_eq_in(ty, env) && self.hand_written_eq_hashes(ty)
    }

    fn hand_written_eq_hashes(&self, ty: &Ty) -> bool {
        if let Some((t, _)) = self.list_parts(ty) {
            return self.hand_written_eq_hashes(&t);
        }
        match ty {
            Ty::Array(t, _) => self.hand_written_eq_hashes(t),
            Ty::Map(a, b, _) => self.hand_written_eq_hashes(a) && self.hand_written_eq_hashes(b),
            Ty::Tuple(ts) => ts.iter().all(|t| self.hand_written_eq_hashes(t)),
            Ty::Struct(a) => {
                let s = &self.structs[a.id];
                s.derives.hash || (s.derives.eq && s.fields.iter().all(|f| self.hand_written_eq_hashes(&f.ty)))
            }
            Ty::Enum(a) => {
                let e = &self.enums[a.id];
                e.derives.hash
                    || (e.derives.eq
                        && e.variants.iter().all(|v| v.fields.iter().all(|f| self.hand_written_eq_hashes(&f.ty))))
            }
            _ => true,
        }
    }

    /// Whether `==` and `!=` apply to values of `ty`, with the bounds of the type parameters in scope. A derived
    /// `Eq` on `Wrapper<T>` holds when the fields do, which is what a `T: Eq` bound provides (issue #69).
    pub(crate) fn has_eq_in(&self, ty: &Ty, env: &crate::generics::Env) -> bool {
        // A list compares by length and then element by element, in order (`spec/stdlib.md`, "List").
        if let Some((elem, _)) = self.list_parts(ty) {
            return self.has_eq_in(&elem, env);
        }
        match ty {
            Ty::Const(_) | Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Error | Ty::Never => true,
            // Text compares by value; `String` and `str` compare byte for byte.
            Ty::Str | Ty::String => true,
            Ty::Param(_) | Ty::Projection { .. } => self.env_provides(ty, &crate::generics::Bound::Eq, env),
            // A map compares by its entries, whatever the iteration order.
            Ty::Map(k, v, _) => self.has_eq_in(k, env) && self.has_eq_in(v, env),
            Ty::Tuple(ts) => ts.iter().all(|t| self.has_eq_in(t, env)),
            // A fixed-size array compares element by element.
            Ty::Array(t, _) => self.has_eq_in(t, env),
            // A derived `Eq` or a hand-written `Eq` implementation: a derive is the type's own implementation of the
            // interface (`spec/derives.md`, issue #918).
            Ty::Struct(_) | Ty::Enum(_) => {
                self.has_derived_eq_in(ty, env) || self.implements_eq_iface(ty, env) || self.eq_from_order(ty, env)
            }
            Ty::Heap(_, _)
            | Ty::CPtr(_)
            | Ty::MapEntry(..)
            | Ty::Chars
            | Ty::Fn { .. }
            | Ty::MapIter(..)
            | Ty::IterMut(_)
            | Ty::MapIterMut(..)
            | Ty::MapIntoIter(..)
            | Ty::SliceChunks(..)
            | Ty::SliceWindows(..)
            | Ty::TryFromError { .. }
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
        (Ty::Fn { sig: asig, kind: ak, borrowed: ab, .. }, Ty::Fn { sig: esig, kind: ek, borrowed: eb, .. }) => {
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
