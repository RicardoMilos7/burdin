//! Resolving every written type: aliases, fields, variants, signatures, constants, and extern declarations.

use crate::derives::parse_derives;
use crate::generics::param_ty;
use crate::globals::{Globals, TypeName};
use crate::session::{NamedSrc, Sema};
use crate::types::check_storable;
use crate::validate::check_infinite_size;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, ParamSig, Signature, Ty};

impl Sema<'_> {
    /// Resolves every written type: aliases, fields, variants, signatures, extern declarations, and constants. It
    /// runs again after each round of instances, because a type that mentioned an instance that did not exist yet
    /// resolved to `Error`.
    pub(crate) fn resolve_all(&mut self, diags: &mut Diagnostics) {
        let mut state = vec![0u8; self.alias_decls.len()];
        for i in 0..self.alias_decls.len() {
            resolve_alias(i, &self.alias_decls, &mut self.globals, &mut state, diags);
        }
        // An extension's receiver may name a generic type, whose instance appears only after the previous pass's
        // requests were allocated; resolve it before the signatures that use it (`spec/extensions.md`).
        self.refresh_extensions(diags);
        for id in 0..self.globals.structs.len() {
            let entry = &self.src.structs[id];
            let module = entry.module;
            let fields = entry
                .decl
                .fields
                .iter()
                .map(|f| {
                    let ty = self.globals.resolve_type_in(&f.ty, None, &entry.subst, module, diags);
                    // An instance of a generic type is second-class when an argument is (SC3), so its own field
                    // may hold the view; the check applies where the instance is stored instead.
                    if entry.subst.is_empty() {
                        check_storable(&ty, f.ty.span, "a struct field", diags);
                    }
                    tir::FieldDef { name: f.name.name.clone(), ty, span: f.name.span, is_priv: f.is_priv }
                })
                .collect();
            check_unique_fields(entry.decl.fields.iter().map(|f| &f.name), diags);
            let derives =
                parse_derives(&entry.decl.derives, &|n| self.globals.modules[module].derives.contains_key(n), diags);
            let s = &mut self.globals.structs[id];
            s.fields = fields;
            s.derives = derives;
        }
        for id in 0..self.globals.enums.len() {
            let entry = &self.src.enums[id];
            let module = entry.module;
            let mut variants: Vec<tir::VariantDef> = Vec::new();
            for v in &entry.decl.variants {
                if crate::fncx::RESERVED_MEMBERS.contains(&v.name.name.as_str()) {
                    diags.error(
                        Code::RESERVED_MEMBER,
                        v.name.span,
                        format!(
                            "`{}` is reserved for reflection, so `{}.{}` cannot be a variant; give it another name",
                            v.name.name, entry.decl.name.name, v.name.name
                        ),
                    );
                }
                if variants.iter().any(|o| o.name == v.name.name) {
                    diags.error(
                        Code::DUPLICATE_NAME,
                        v.name.span,
                        format!("variant `{}` is defined more than once", v.name.name),
                    );
                }
                let field = |ty: &ast::Type, name: String, span: Span, is_priv: bool, diags: &mut Diagnostics| {
                    let resolved = self.globals.resolve_type_in(ty, None, &entry.subst, module, diags);
                    // See the struct field case above (SC3).
                    if entry.subst.is_empty() {
                        check_storable(&resolved, ty.span, "an enum variant field", diags);
                    }
                    tir::FieldDef { name, ty: resolved, span, is_priv }
                };
                let (shape, fields) = match &v.kind {
                    ast::VariantKind::Unit => (tir::VariantShape::Unit, Vec::new()),
                    ast::VariantKind::Tuple(tys) => {
                        let fields = tys
                            .iter()
                            .enumerate()
                            .map(|(i, t)| field(t, i.to_string(), t.span, false, diags))
                            .collect();
                        (tir::VariantShape::Tuple, fields)
                    }
                    ast::VariantKind::Struct(fs) => {
                        check_unique_fields(fs.iter().map(|f| &f.name), diags);
                        let fields = fs
                            .iter()
                            .map(|f| field(&f.ty, f.name.name.clone(), f.name.span, f.is_priv, diags))
                            .collect();
                        (tir::VariantShape::Struct, fields)
                    }
                };
                variants.push(tir::VariantDef { name: v.name.name.clone(), shape, fields });
            }
            let derives =
                parse_derives(&entry.decl.derives, &|n| self.globals.modules[module].derives.contains_key(n), diags);
            let e = &mut self.globals.enums[id];
            e.variants = variants;
            e.derives = derives;
        }
        check_infinite_size(&self.globals, &self.src, diags);
        for i in 0..self.globals.interfaces.len() {
            let module = self.globals.interfaces[i].module;
            let params = self.globals.interfaces[i].params.clone();
            let subst: Vec<(String, Ty)> = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
            for k in 0..self.globals.interfaces[i].methods.len() {
                let decl = &self.tpl.ifaces[i].methods[k].decl;
                // A method-level type parameter is opaque in the interface's own signature.
                let mut method_subst = subst.clone();
                for g in &decl.generics {
                    method_subst.push((g.name.name.clone(), param_ty(&g.name.name)));
                }
                let sig =
                    fn_signature(decl, Some(&param_ty("Self")), &method_subst, module, &self.globals, false, diags);
                // A default argument is a call-site convenience of one concrete function; an interface's
                // signature is shared by every implementation, so it cannot declare one
                // (spec/arguments.md, AG5).
                for p in &decl.params {
                    if let Some(at) = p.comptime {
                        diags.error(
                            Code::INVALID_COMPTIME,
                            at,
                            "an interface method cannot take a `comptime` parameter: every implementation would need its own specializations",
                        );
                    }
                    if let Some(default) = &p.default {
                        diags.error(
                            Code::INVALID_DEFAULT,
                            default.span,
                            "an interface method cannot declare a default argument; only a function or method with one implementation can",
                        );
                    }
                }
                self.globals.interfaces[i].methods[k].sig = sig;
            }
        }
        for (i, item) in self.fn_items.iter().enumerate() {
            let mut sig = fn_signature(
                item.decl,
                item.self_ty.as_ref(),
                &item.subst,
                item.module,
                &self.globals,
                item.is_exported,
                diags,
            );
            if self.globals.from_methods.contains(&tir::FuncId(i))
                && let Some(p) = sig.params.first_mut()
            {
                p.by_ref = false;
                p.own = true;
                p.exclusive = true;
            }
            crate::fncx::check_defaults(&self.globals, item, &mut sig, self.sources, self.traces, diags);
            self.globals.sigs[i] = sig;
        }
        for (i, c) in self.const_decls.iter().enumerate() {
            self.globals.const_tys[i] = self.globals.resolve_type(&c.decl.ty, None, c.module, diags);
        }
        let externs: Vec<(usize, &ast::FnDecl, usize, String)> = self
            .extern_srcs
            .iter()
            .map(|s| (s.extern_id, s.decl, s.module, self.globals.externs[s.extern_id].symbol.clone()))
            .collect();
        let mut signatures: Vec<(usize, String, Signature)> = Vec::new();
        for (id, decl, module, symbol) in externs {
            let sig = extern_signature(decl, module, &self.globals, diags);
            match signatures.iter().find(|(other, _, _)| *other == id) {
                Some((_, _, first)) if !same_signature(first, &sig) => {
                    diags.error(
                        Code::INVALID_EXTERN,
                        decl.name.span,
                        format!("the C function `{symbol}` is declared with a different signature here"),
                    );
                }
                Some(_) => {}
                None => signatures.push((id, symbol, sig)),
            }
        }
        for (id, _, sig) in signatures {
            self.globals.externs[id].sig = sig;
        }
        // Object safety is checked once every interface signature is resolved, so the reasons are final
        // (spec/existentials.md, AN2).
        self.globals.check_any_uses(diags);
        // Duplicate declarations and shadowed extensions need the resolved receiver and every method in place
        // (`spec/extensions.md`, EX4 and EX5).
        self.check_extension_problems(diags);
    }
}

pub(crate) fn resolve_alias(
    i: usize,
    decls: &[NamedSrc<ast::TypeAliasDecl>],
    globals: &mut Globals,
    state: &mut [u8],
    diags: &mut Diagnostics,
) {
    let module = decls[i].module;
    let decl = decls[i].decl;
    match state[i] {
        2 => return,
        1 => {
            diags.error(
                Code::CYCLIC_DECLARATION,
                decl.name.span,
                format!("type alias `{}` refers to itself", decl.name.name),
            );
            state[i] = 3;
            return;
        }
        3 => return,
        _ => {}
    }
    state[i] = 1;
    // Resolve the aliases this alias names first, including the ones reached through a module path: aliases are
    // values of their own and would otherwise be read before they are resolved (in module order).
    let mut refs = Vec::new();
    alias_refs(&decl.ty, module, globals, 0, &mut refs);
    for j in refs {
        resolve_alias(j, decls, globals, state, diags);
    }
    if state[i] == 1 {
        globals.aliases[i] = globals.resolve_type(&decl.ty, None, module, diags);
    }
    state[i] = 2;
}

/// The type aliases a written type mentions, whether by a plain name or through a module path. A generic alias is
/// expanded where it is used, so the aliases its definition mentions count too (`depth` stops an alias that names
/// itself; the expansion reports that).
fn alias_refs(ty: &ast::Type, module: usize, globals: &Globals, depth: usize, out: &mut Vec<usize>) {
    match &ty.kind {
        ast::TypeKind::Path(segments) if segments.len() == 1 => {
            let seg = &segments[0];
            match globals.type_name(module, &seg.name.name) {
                Some(TypeName::Alias(j)) => out.push(j),
                Some(TypeName::GenericAlias(g)) if depth < 8 => {
                    let alias = &globals.generic_aliases[g];
                    alias_refs(&alias.decl.ty, alias.module, globals, depth + 1, out);
                }
                _ => {}
            }
            for arg in &seg.args {
                alias_refs(arg, module, globals, depth, out);
            }
        }
        ast::TypeKind::Path(segments) => {
            let names: Vec<String> = segments[..segments.len() - 1].iter().map(|s| s.name.name.clone()).collect();
            if let Some(target) = globals.resolve_module_path(module, &names) {
                let last = segments.last().expect("the path has a last segment");
                match globals.exported_type(target, &last.name.name) {
                    Some(TypeName::Alias(j)) => out.push(j),
                    Some(TypeName::GenericAlias(g)) if depth < 8 => {
                        let alias = &globals.generic_aliases[g];
                        alias_refs(&alias.decl.ty, alias.module, globals, depth + 1, out);
                    }
                    _ => {}
                }
                for arg in &last.args {
                    alias_refs(arg, module, globals, depth, out);
                }
            }
        }
        ast::TypeKind::Tuple(elems) => elems.iter().for_each(|e| alias_refs(e, module, globals, depth, out)),
        ast::TypeKind::Fn { params, ret, .. } => {
            params.iter().for_each(|e| alias_refs(e, module, globals, depth, out));
            if let Some(ret) = ret {
                alias_refs(ret, module, globals, depth, out);
            }
        }
        ast::TypeKind::Array(e, _) | ast::TypeKind::Slice(e) | ast::TypeKind::Ref { ty: e, .. } => {
            alias_refs(e, module, globals, depth, out)
        }
        // `any I` and a parameter `impl I` name an interface, not a type alias, but their arguments may name one.
        ast::TypeKind::Any(e) | ast::TypeKind::Impl(e) => alias_refs(e, module, globals, depth, out),
        ast::TypeKind::SelfType | ast::TypeKind::Unit | ast::TypeKind::Never | ast::TypeKind::Const(_) => {}
    }
}

pub(crate) fn check_unique_fields<'a>(names: impl Iterator<Item = &'a ast::Ident>, diags: &mut Diagnostics) {
    let mut seen: Vec<&str> = Vec::new();
    for n in names {
        if seen.contains(&n.name.as_str()) {
            diags.error(Code::DUPLICATE_NAME, n.span, format!("field `{}` is declared more than once", n.name));
        }
        seen.push(&n.name);
    }
}

pub(crate) fn fn_signature(
    f: &ast::FnDecl,
    self_ty: Option<&Ty>,
    subst: &[(String, Ty)],
    module: usize,
    globals: &Globals,
    is_exported: bool,
    diags: &mut Diagnostics,
) -> Signature {
    // An exported function is checked against the C ABI by `check_export_signatures`, which rejects `...`.
    if f.is_vararg && !is_exported {
        diags.error(Code::SYNTAX, f.name.span, "variadic parameters are only allowed in `extern` declarations");
    }
    let mut params = Vec::new();
    let mut has_self = false;
    for (i, p) in f.params.iter().enumerate() {
        match &p.kind {
            ast::ParamKind::SelfParam => {
                let Some(st) = self_ty.filter(|_| i == 0) else {
                    diags.error(Code::SYNTAX, p.span, "`self` can only be the first parameter of a method");
                    continue;
                };
                has_self = true;
                // `self` reads the caller's value (read-only, `mut`, or `own`): only `own self` takes it (CV4).
                params.push(ParamSig {
                    ty: st.clone(),
                    by_ref: p.convention != ast::Convention::Own,
                    mut_param: false,
                    own: p.convention == ast::Convention::Own,
                    exclusive: p.convention != ast::Convention::Read,
                    name: None,
                    default: None,
                    comptime: false,
                });
                if let Some(at) = p.comptime {
                    diags.error(Code::INVALID_COMPTIME, at, "`self` cannot be a `comptime` parameter");
                }
            }
            ast::ParamKind::Named { name, ty } => {
                let ty = globals.resolve_type_in(ty, self_ty, subst, module, diags);
                let name = Some(name.name.clone());
                let comptime = p.comptime.is_some();
                if let Some(at) = p.comptime {
                    if p.convention != ast::Convention::Read {
                        diags.error(
                            Code::INVALID_COMPTIME,
                            at,
                            "a `comptime` parameter is read-only: it cannot be `mut` or `own` (spec/comptime.md, CT4)",
                        );
                    } else if !globals.ty_has_param(&ty) && !globals.materializable(&ty) {
                        diags.error(
                            Code::NOT_MATERIALIZABLE,
                            p.span,
                            format!("a `comptime` parameter cannot have type `{ty}`: it is not a compile-time value (spec/comptime.md, CT3)"),
                        );
                    }
                }
                params.push(match p.convention {
                    // A read-only parameter borrows the caller's place, so a view derived from it can be
                    // returned (RT1).
                    ast::Convention::Read => ParamSig {
                        ty,
                        by_ref: true,
                        mut_param: false,
                        own: false,
                        exclusive: false,
                        name,
                        default: None,
                        comptime,
                    },
                    // The callee sees the caller's place, so it is passed by address and the call site
                    // marks it `mut` (CV2).
                    ast::Convention::Mut => ParamSig {
                        ty,
                        by_ref: true,
                        mut_param: true,
                        own: false,
                        exclusive: true,
                        name,
                        default: None,
                        comptime: false,
                    },
                    ast::Convention::Own => ParamSig {
                        ty,
                        by_ref: false,
                        mut_param: false,
                        own: true,
                        exclusive: true,
                        name,
                        default: None,
                        comptime: false,
                    },
                });
            }
        }
    }
    let ret = f.ret.as_ref().map(|t| globals.resolve_type_in(t, self_ty, subst, module, diags)).unwrap_or(Ty::Unit);
    Signature { params, ret, has_self, ret_origins: Vec::new() }
}

/// The signature of an `extern "C"` function: every parameter is passed by value, and only C-compatible types may
/// cross the boundary.
fn extern_signature(f: &ast::FnDecl, module: usize, globals: &Globals, diags: &mut Diagnostics) -> Signature {
    let mut params = Vec::new();
    for p in &f.params {
        if matches!(p.kind, ast::ParamKind::SelfParam) {
            diags.error(Code::C_ABI, p.span, "an `extern` function cannot take `self`");
            continue;
        }
        if p.convention != ast::Convention::Read {
            diags.error(
                Code::C_ABI,
                p.span,
                "an `extern` function parameter is always passed by value; `mut` and `own` do not apply",
            );
        }
        let ast::ParamKind::Named { ty, .. } = &p.kind else { continue };
        if let Some(default) = &p.default {
            diags.error(
                Code::INVALID_DEFAULT,
                default.span,
                "an `extern` declaration takes no default arguments; the C function always receives every parameter",
            );
        }
        let resolved = globals.resolve_type_in(ty, None, &[], module, diags);
        if resolved != Ty::Error && !is_c_abi_value(&resolved) {
            diags.error(Code::C_ABI, ty.span, c_abi_rejection(&resolved, "passed to a C function"));
        }
        // The parameter stays in the signature so that calls are still checked for arity, even though this
        // declaration already failed.
        if let Some(at) = p.comptime {
            diags.error(Code::INVALID_COMPTIME, at, "an `extern` declaration cannot take a `comptime` parameter");
        }
        params.push(ParamSig {
            ty: resolved,
            by_ref: false,
            mut_param: false,
            own: false,
            exclusive: false,
            name: None,
            default: None,
            comptime: false,
        });
    }
    let ret = f.ret.as_ref().map(|t| globals.resolve_type_in(t, None, &[], module, diags)).unwrap_or(Ty::Unit);
    if ret != Ty::Unit && ret != Ty::Error && !is_c_abi_value(&ret) {
        diags.error(Code::C_ABI, f.name.span, c_abi_rejection(&ret, "returned by a C function"));
    }
    Signature { params, ret, has_self: false, ret_origins: Vec::new() }
}

/// Whether a type has the same representation in Burdin and in C: integers, floats, and raw pointers. A Burdin
/// `bool` is a one-bit value, while C `_Bool` is a one-byte integer, so it is passed as `u8` instead. Aggregates
/// cross the C ABI through `CPtr<T>` only: the platform by-value ABI coercions (packing a small struct into a
/// register, `sret`, and so on) are not implemented (issues #73 and #490).
pub(crate) fn is_c_abi_value(ty: &Ty) -> bool {
    matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::CPtr(_) | Ty::Unit | Ty::Never | Ty::Error)
}

/// The E0162 text for a type that cannot cross the C ABI.
pub(crate) fn c_abi_rejection(ty: &Ty, direction: &str) -> String {
    match ty {
        Ty::Bool => {
            format!("`bool` cannot be {direction}; C `_Bool` is a one-byte integer, so declare it `u8` (0 or 1)")
        }
        Ty::Struct(_) | Ty::Enum(_) | Ty::Tuple(_) | Ty::Array(..) => {
            format!("`{ty}` cannot be {direction} by value; an aggregate crosses the C ABI through `CPtr<{ty}>`")
        }
        _ => format!("`{ty}` cannot be {direction}; only integers, floats, and `CPtr` can"),
    }
}

/// Whether two `extern` declarations describe the same C function.
fn same_signature(a: &Signature, b: &Signature) -> bool {
    a.ret == b.ret && a.params.len() == b.params.len() && a.params.iter().zip(&b.params).all(|(x, y)| x.ty == y.ty)
}
