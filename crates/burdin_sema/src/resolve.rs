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
        for id in 0..self.globals.structs.len() {
            let entry = &self.src.structs[id];
            let module = entry.module;
            let fields = entry
                .decl
                .fields
                .iter()
                .map(|f| {
                    let ty = self.globals.resolve_type_in(&f.ty, None, &entry.subst, module, diags);
                    check_storable(&ty, f.ty.span, "a struct field", diags);
                    tir::FieldDef { name: f.name.name.clone(), ty, span: f.name.span, is_priv: f.is_priv }
                })
                .collect();
            check_unique_fields(entry.decl.fields.iter().map(|f| &f.name), diags);
            let derives = parse_derives(&entry.decl.derives, diags);
            let s = &mut self.globals.structs[id];
            s.fields = fields;
            s.derives = derives;
        }
        for id in 0..self.globals.enums.len() {
            let entry = &self.src.enums[id];
            let module = entry.module;
            let mut variants: Vec<tir::VariantDef> = Vec::new();
            for v in &entry.decl.variants {
                if variants.iter().any(|o| o.name == v.name.name) {
                    diags.error(
                        Code::UNSUPPORTED,
                        v.name.span,
                        format!("variant `{}` is defined more than once", v.name.name),
                    );
                }
                let field = |ty: &ast::Type, name: String, span: Span, is_priv: bool, diags: &mut Diagnostics| {
                    let resolved = self.globals.resolve_type_in(ty, None, &entry.subst, module, diags);
                    check_storable(&resolved, ty.span, "an enum variant field", diags);
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
            let derives = parse_derives(&entry.decl.derives, diags);
            let e = &mut self.globals.enums[id];
            e.variants = variants;
            e.derives = derives;
        }
        check_infinite_size(&self.globals, &self.src, diags);
        for i in 0..self.globals.interfaces.len() {
            let module = self.globals.interfaces[i].module;
            for k in 0..self.globals.interfaces[i].methods.len() {
                let decl = &self.tpl.ifaces[i].methods[k].decl;
                let sig = fn_signature(decl, Some(&param_ty("Self")), &[], module, &self.globals, diags);
                self.globals.interfaces[i].methods[k].sig = sig;
            }
        }
        for (i, item) in self.fn_items.iter().enumerate() {
            let mut sig =
                fn_signature(item.decl, item.self_ty.as_ref(), &item.subst, item.module, &self.globals, diags);
            if self.globals.from_methods.contains(&tir::FuncId(i))
                && let Some(p) = sig.params.first_mut()
            {
                p.by_ref = false;
                p.own = true;
                p.exclusive = true;
            }
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
                        Code::UNSUPPORTED,
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
            diags.error(Code::UNSUPPORTED, decl.name.span, format!("type alias `{}` refers to itself", decl.name.name));
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
    alias_refs(&decl.ty, module, globals, &mut refs);
    for j in refs {
        resolve_alias(j, decls, globals, state, diags);
    }
    if state[i] == 1 {
        globals.aliases[i] = globals.resolve_type(&decl.ty, None, module, diags);
    }
    state[i] = 2;
}

/// The type aliases a written type mentions, whether by a plain name or through a module path.
fn alias_refs(ty: &ast::Type, module: usize, globals: &Globals, out: &mut Vec<usize>) {
    match &ty.kind {
        ast::TypeKind::Path(segments) if segments.len() == 1 => {
            let seg = &segments[0];
            if let Some(TypeName::Alias(j)) = globals.type_name(module, &seg.name.name) {
                out.push(j);
            }
            for arg in &seg.args {
                alias_refs(arg, module, globals, out);
            }
        }
        ast::TypeKind::Path(segments) => {
            let names: Vec<String> = segments[..segments.len() - 1].iter().map(|s| s.name.name.clone()).collect();
            if let Some(target) = globals.resolve_module_path(module, &names) {
                let last = segments.last().expect("the path has a last segment");
                if let Some(TypeName::Alias(j)) = globals.exported_type(target, &last.name.name) {
                    out.push(j);
                }
                for arg in &last.args {
                    alias_refs(arg, target, globals, out);
                }
            }
        }
        ast::TypeKind::Tuple(elems) => elems.iter().for_each(|e| alias_refs(e, module, globals, out)),
        ast::TypeKind::Array(e, _) | ast::TypeKind::Slice(e) | ast::TypeKind::Ref { ty: e, .. } => {
            alias_refs(e, module, globals, out)
        }
        ast::TypeKind::SelfType | ast::TypeKind::Unit | ast::TypeKind::Never => {}
    }
}

pub(crate) fn check_unique_fields<'a>(names: impl Iterator<Item = &'a ast::Ident>, diags: &mut Diagnostics) {
    let mut seen: Vec<&str> = Vec::new();
    for n in names {
        if seen.contains(&n.name.as_str()) {
            diags.error(Code::UNSUPPORTED, n.span, format!("field `{}` is declared more than once", n.name));
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
    diags: &mut Diagnostics,
) -> Signature {
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
                });
            }
            ast::ParamKind::Named { ty, .. } => {
                let ty = globals.resolve_type_in(ty, self_ty, subst, module, diags);
                params.push(match p.convention {
                    // A read-only parameter borrows the caller's place, so a view derived from it can be
                    // returned (RT1).
                    ast::Convention::Read => {
                        ParamSig { ty, by_ref: true, mut_param: false, own: false, exclusive: false }
                    }
                    // The callee sees the caller's place, so it is passed by address and the call site
                    // marks it `mut` (CV2).
                    ast::Convention::Mut => ParamSig { ty, by_ref: true, mut_param: true, own: false, exclusive: true },
                    ast::Convention::Own => {
                        ParamSig { ty, by_ref: false, mut_param: false, own: true, exclusive: true }
                    }
                });
            }
        }
    }
    let ret = f.ret.as_ref().map(|t| globals.resolve_type_in(t, self_ty, subst, module, diags)).unwrap_or(Ty::Unit);
    Signature { params, ret, has_self, ret_origin: None }
}

/// The signature of an `extern "C"` function: every parameter is passed by value, and only C-compatible types may
/// cross the boundary.
fn extern_signature(f: &ast::FnDecl, module: usize, globals: &Globals, diags: &mut Diagnostics) -> Signature {
    let mut params = Vec::new();
    for p in &f.params {
        if matches!(p.kind, ast::ParamKind::SelfParam) {
            diags.error(Code::UNSUPPORTED, p.span, "an `extern` function cannot take `self`");
            continue;
        }
        if p.convention != ast::Convention::Read {
            diags.error(
                Code::UNSUPPORTED,
                p.span,
                "an `extern` function parameter is always passed by value; `mut` and `own` do not apply",
            );
        }
        let ast::ParamKind::Named { ty, .. } = &p.kind else { continue };
        let resolved = globals.resolve_type_in(ty, None, &[], module, diags);
        if resolved != Ty::Error && !is_c_abi_value(&resolved) {
            diags.error(
                Code::UNSUPPORTED,
                ty.span,
                format!("`{resolved}` cannot be passed to a C function; only integers, floats, and `CPtr` can"),
            );
        }
        // The parameter stays in the signature so that calls are still checked for arity, even though this
        // declaration already failed.
        params.push(ParamSig { ty: resolved, by_ref: false, mut_param: false, own: false, exclusive: false });
    }
    let ret = f.ret.as_ref().map(|t| globals.resolve_type_in(t, None, &[], module, diags)).unwrap_or(Ty::Unit);
    if ret != Ty::Unit && ret != Ty::Error && !is_c_abi_value(&ret) {
        diags.error(Code::UNSUPPORTED, f.name.span, format!("`{ret}` cannot be returned by a C function"));
    }
    Signature { params, ret, has_self: false, ret_origin: None }
}

/// Whether a type has the same representation in Burdin and in C: integers, floats, and raw pointers.
fn is_c_abi_value(ty: &Ty) -> bool {
    matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::CPtr(_))
}

/// Whether two `extern` declarations describe the same C function.
fn same_signature(a: &Signature, b: &Signature) -> bool {
    a.ret == b.ret && a.params.len() == b.params.len() && a.params.iter().zip(&b.params).all(|(x, y)| x.ty == y.ty)
}
