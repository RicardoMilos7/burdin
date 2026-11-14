//! Resolving every written type: aliases, fields, variants, signatures, and constants.

use crate::derives::parse_derives;
use crate::generics::param_ty;
use crate::globals::{Globals, TypeName};
use crate::session::Sema;
use crate::types::check_storable;
use crate::validate::check_infinite_size;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, ParamSig, Signature, Ty};

impl Sema<'_> {
    /// Resolves every written type: aliases, fields, variants, signatures, and constants. It runs again after each
    /// round of instances, because a type that mentioned an instance that did not exist yet resolved to `Error`.
    pub(crate) fn resolve_all(&mut self, diags: &mut Diagnostics) {
        let mut state = vec![0u8; self.alias_decls.len()];
        for i in 0..self.alias_decls.len() {
            resolve_alias(i, &self.alias_decls, &mut self.globals, &mut state, diags);
        }
        for id in 0..self.globals.structs.len() {
            let (decl, subst) = &self.src.structs[id];
            let fields = decl
                .fields
                .iter()
                .map(|f| {
                    let ty = self.globals.resolve_type_in(&f.ty, None, subst, diags);
                    check_storable(&ty, f.ty.span, "a struct field", diags);
                    tir::FieldDef { name: f.name.name.clone(), ty, span: f.name.span }
                })
                .collect();
            check_unique_fields(decl.fields.iter().map(|f| &f.name), diags);
            let derives = parse_derives(&decl.derives, diags);
            let s = &mut self.globals.structs[id];
            s.fields = fields;
            s.derives = derives;
        }
        for id in 0..self.globals.enums.len() {
            let (decl, subst) = &self.src.enums[id];
            let mut variants: Vec<tir::VariantDef> = Vec::new();
            for v in &decl.variants {
                if variants.iter().any(|o| o.name == v.name.name) {
                    diags.error(
                        Code::UNSUPPORTED,
                        v.name.span,
                        format!("variant `{}` is defined more than once", v.name.name),
                    );
                }
                let field = |ty: &ast::Type, name: String, span: Span, diags: &mut Diagnostics| {
                    let resolved = self.globals.resolve_type_in(ty, None, subst, diags);
                    check_storable(&resolved, ty.span, "an enum variant field", diags);
                    tir::FieldDef { name, ty: resolved, span }
                };
                let (shape, fields) = match &v.kind {
                    ast::VariantKind::Unit => (tir::VariantShape::Unit, Vec::new()),
                    ast::VariantKind::Tuple(tys) => {
                        let fields =
                            tys.iter().enumerate().map(|(i, t)| field(t, i.to_string(), t.span, diags)).collect();
                        (tir::VariantShape::Tuple, fields)
                    }
                    ast::VariantKind::Struct(fs) => {
                        check_unique_fields(fs.iter().map(|f| &f.name), diags);
                        let fields = fs.iter().map(|f| field(&f.ty, f.name.name.clone(), f.name.span, diags)).collect();
                        (tir::VariantShape::Struct, fields)
                    }
                };
                variants.push(tir::VariantDef { name: v.name.name.clone(), shape, fields });
            }
            let derives = parse_derives(&decl.derives, diags);
            let e = &mut self.globals.enums[id];
            e.variants = variants;
            e.derives = derives;
        }
        check_infinite_size(&self.globals, &self.src, diags);
        for i in 0..self.globals.interfaces.len() {
            for k in 0..self.globals.interfaces[i].methods.len() {
                let decl = &self.tpl.ifaces[i].methods[k].decl;
                let sig = fn_signature(decl, Some(&param_ty("Self")), &[], &self.globals, diags);
                self.globals.interfaces[i].methods[k].sig = sig;
            }
        }
        for (i, item) in self.fn_items.iter().enumerate() {
            let sig = fn_signature(item.decl, item.self_ty.as_ref(), &item.subst, &self.globals, diags);
            self.globals.sigs[i] = sig;
        }
        for (i, c) in self.const_decls.iter().enumerate() {
            self.globals.const_tys[i] = self.globals.resolve_type(&c.ty, None, diags);
        }
    }
}

pub(crate) fn resolve_alias(
    i: usize,
    decls: &[&ast::TypeAliasDecl],
    globals: &mut Globals,
    state: &mut [u8],
    diags: &mut Diagnostics,
) {
    match state[i] {
        2 => return,
        1 => {
            diags.error(
                Code::UNSUPPORTED,
                decls[i].name.span,
                format!("type alias `{}` refers to itself", decls[i].name.name),
            );
            state[i] = 3;
            return;
        }
        3 => return,
        _ => {}
    }
    state[i] = 1;
    let mut names = Vec::new();
    type_names(&decls[i].ty, &mut names);
    for n in names {
        if let Some(TypeName::Alias(j)) = globals.types.get(&n).copied() {
            resolve_alias(j, decls, globals, state, diags);
        }
    }
    if state[i] == 1 {
        globals.aliases[i] = globals.resolve_type(&decls[i].ty, None, diags);
    }
    state[i] = 2;
}

/// The names used in a type, for ordering the type aliases.
pub(crate) fn type_names(ty: &ast::Type, out: &mut Vec<String>) {
    match &ty.kind {
        ast::TypeKind::Path(segments) => {
            for s in segments {
                out.push(s.name.name.clone());
                s.args.iter().for_each(|a| type_names(a, out));
            }
        }
        ast::TypeKind::Tuple(elems) => elems.iter().for_each(|e| type_names(e, out)),
        ast::TypeKind::Array(e, _) | ast::TypeKind::Slice(e) | ast::TypeKind::Ref { ty: e, .. } => type_names(e, out),
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
    globals: &Globals,
    diags: &mut Diagnostics,
) -> Signature {
    if f.is_unsafe {
        diags.error(Code::UNSUPPORTED, f.name.span, "`unsafe fn` is not implemented in this compiler yet");
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
                });
            }
            ast::ParamKind::Named { ty, .. } => {
                let ty = globals.resolve_type_in(ty, self_ty, subst, diags);
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
    let ret = f.ret.as_ref().map(|t| globals.resolve_type_in(t, self_ty, subst, diags)).unwrap_or(Ty::Unit);
    Signature { params, ret, has_self, ret_origin: None }
}
