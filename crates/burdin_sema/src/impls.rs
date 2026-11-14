//! Impl blocks: `Type { ... }`, `Type: Drop { ... }`, and `Type: Interface { ... }`.

use crate::builtins::builtin_type;
use crate::generics::{BUILTIN_INTERFACES, Bound, Head, ImplInfo};
use crate::globals::{Globals, TypeName, adt_key, interface_name};
use crate::session::{FnItem, ImplTemplate, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::Ty;

impl<'t> Sema<'t> {
    /// Registers an impl block. Methods of an impl on a generic type are registered when each instance of the
    /// type is allocated.
    pub(crate) fn register_impl(&mut self, imp: &'t ast::ImplBlock, diags: &mut Diagnostics) {
        let Some(kind) = self.impl_kind(imp, diags) else { return };

        // An impl on a generic type: `Stack<T> { ... }`, `Wrapper<T>: Describe where T: Describe { ... }`.
        if let ast::TypeKind::Path(segs) = &imp.ty.kind
            && segs.len() == 1
            && let Some(&TypeName::GenericAdt(g)) = self.globals.types.get(&segs[0].name.name)
        {
            self.register_generic_impl(imp, g, kind, diags);
            return;
        }

        if !imp.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, imp.span, "`where` clauses are not implemented in this compiler yet");
        }
        let self_ty = self.globals.resolve_type(&imp.ty, None, diags);
        let Some(key) = adt_key(&self_ty) else {
            if matches!(kind, ImplKind::Builtin) && self_ty != Ty::Error {
                diags.error(
                    Code::ORPHAN_RULE,
                    imp.span,
                    format!("`{self_ty}` and the interface both come from the standard library, so this package cannot implement one for the other"),
                );
            } else if self_ty != Ty::Error {
                diags.error(
                    Code::TYPE_MISMATCH,
                    imp.ty.span,
                    format!("methods can only be added to structs and enums declared here, not to `{self_ty}`"),
                );
            }
            return;
        };
        match kind {
            ImplKind::Builtin => {
                diags.error(
                    Code::UNSUPPORTED,
                    imp.span,
                    "implementing a built-in interface by hand is not implemented in this compiler yet; use `derives`",
                );
            }
            ImplKind::Drop => self.register_drop_impl(imp, &self_ty, key, diags),
            ImplKind::Inherent | ImplKind::User(_) => {
                let iface = kind.user_interface();
                if let Some(i) = iface {
                    check_missing_methods(&self.globals, imp, i, diags);
                    self.globals.impls.push(ImplInfo {
                        head: Head::Plain(key),
                        iface: i,
                        params: Vec::new(),
                        wheres: Vec::new(),
                        span: imp.span,
                    });
                }
                self.register_methods(imp, iface, &self_ty, MethodScope::plain(), diags);
            }
        }
    }

    /// What kind of impl block this is, from the interface it names. `None` after reporting an unknown or
    /// unsupported interface.
    fn impl_kind(&self, imp: &ast::ImplBlock, diags: &mut Diagnostics) -> Option<ImplKind> {
        let Some(t) = &imp.interface else { return Some(ImplKind::Inherent) };
        let (name, has_args) = interface_name(t);
        if name == "Drop" {
            return Some(ImplKind::Drop);
        }
        if BUILTIN_INTERFACES.contains(&name.as_str()) {
            return Some(ImplKind::Builtin);
        }
        let Some(&i) = self.globals.iface_names.get(&name) else {
            diags.error(Code::UNKNOWN_NAME, t.span, format!("unknown interface `{name}`"));
            return None;
        };
        if has_args {
            diags.error(Code::UNSUPPORTED, t.span, "generic interfaces are not implemented in this compiler yet");
            return None;
        }
        Some(ImplKind::User(i))
    }

    /// An impl on a generic type, written with the names it gives the type's arguments: it becomes a template that
    /// is instantiated with every instance of the type.
    fn register_generic_impl(&mut self, imp: &'t ast::ImplBlock, g: usize, kind: ImplKind, diags: &mut Diagnostics) {
        let ast::TypeKind::Path(segs) = &imp.ty.kind else { return };
        let mut params = Vec::new();
        for arg in &segs[0].args {
            match &arg.kind {
                ast::TypeKind::Path(s)
                    if s.len() == 1
                        && s[0].args.is_empty()
                        && !self.globals.types.contains_key(&s[0].name.name)
                        && builtin_type(&s[0].name.name).is_none() =>
                {
                    params.push(s[0].name.name.clone());
                }
                _ => {
                    diags.error(
                        Code::UNSUPPORTED,
                        arg.span,
                        "an impl block for specific type arguments is not implemented in this compiler yet",
                    );
                    return;
                }
            }
        }
        let wanted = self.globals.generic_adts[g].params.len();
        if params.len() != wanted {
            diags.error(
                Code::TYPE_MISMATCH,
                imp.ty.span,
                format!("`{}` takes {wanted} type argument(s), but {} were given", segs[0].name.name, params.len()),
            );
            return;
        }
        if matches!(kind, ImplKind::Drop | ImplKind::Builtin) {
            diags.error(
                Code::UNSUPPORTED,
                imp.span,
                "implementing a built-in interface for a generic type is not implemented in this compiler yet",
            );
            return;
        }
        let wheres = self.impl_wheres(imp, &params, diags);
        let iface = kind.user_interface();
        if let Some(i) = iface {
            check_missing_methods(&self.globals, imp, i, diags);
            self.globals.impls.push(ImplInfo {
                head: Head::Generic(g),
                iface: i,
                params: params.clone(),
                wheres: wheres.clone(),
                span: imp.span,
            });
        }
        self.tpl.impls.push(ImplTemplate { decl: imp, template: g, params, wheres, iface });
    }

    /// The bounds an impl block's `where` clause puts on the names it gives the type's arguments.
    fn impl_wheres(
        &self,
        imp: &ast::ImplBlock,
        params: &[String],
        diags: &mut Diagnostics,
    ) -> Vec<(String, Vec<Bound>)> {
        let mut wheres = Vec::new();
        for pred in &imp.where_clause {
            let name = match &pred.ty.kind {
                ast::TypeKind::Path(s) if s.len() == 1 && s[0].args.is_empty() && params.contains(&s[0].name.name) => {
                    Some(s[0].name.name.clone())
                }
                _ => None,
            };
            let Some(name) = name else {
                diags.error(
                    Code::UNSUPPORTED,
                    pred.ty.span,
                    "a `where` clause on something other than a type parameter is not implemented in this compiler yet",
                );
                continue;
            };
            wheres.push((name, pred.bounds.iter().filter_map(|b| self.globals.resolve_bound(b, diags)).collect()));
        }
        wheres
    }

    /// `Type: Drop { fn drop(mut self) { ... } }`: the one method the drop glue calls.
    fn register_drop_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        self_ty: &Ty,
        key: (bool, usize),
        diags: &mut Diagnostics,
    ) {
        if imp.methods.len() != 1 {
            diags.error(Code::UNSUPPORTED, imp.span, "a `Drop` implementation must contain exactly one `drop` method");
            return;
        }
        let m = &imp.methods[0];
        if m.name.name != "drop" {
            diags.error(Code::UNSUPPORTED, m.name.span, "a `Drop` implementation must define `drop`");
            return;
        }
        let is_variant = match self_ty {
            Ty::Enum(a) => self.src.enums[a.id].0.variants.iter().any(|v| v.name.name == m.name.name),
            _ => false,
        };
        if self.globals.methods.contains_key(&(key, m.name.name.clone()))
            || is_variant
            || self.globals.drop_methods.contains_key(&key)
        {
            diags.error(
                Code::UNSUPPORTED,
                m.name.span,
                format!("`{self_ty}` already has a member named `{}`", m.name.name),
            );
            return;
        }
        let id = self.push_fn(FnItem::plain(m, format!("{self_ty}.drop"), Some(self_ty.clone())));
        self.globals.drop_methods.insert(key, id);
        match key.0 {
            false => self.globals.structs[key.1].drop_method = Some(id),
            true => self.globals.enums[key.1].drop_method = Some(id),
        }
        self.drop_checks.push((id, m.span));
    }
}

/// What an impl block implements.
#[derive(Clone, Copy)]
enum ImplKind {
    /// `Type { ... }`: methods of the type itself.
    Inherent,
    /// `Type: Drop { ... }`.
    Drop,
    /// A built-in interface other than `Drop`, which is only derived for now.
    Builtin,
    /// `Type: Interface { ... }` for an interface declared in the program (an index into the interfaces).
    User(usize),
}

impl ImplKind {
    fn user_interface(self) -> Option<usize> {
        match self {
            ImplKind::User(i) => Some(i),
            _ => None,
        }
    }
}

/// E0140: an implementation must define every method of its interface that has no default.
pub(crate) fn check_missing_methods(globals: &Globals, imp: &ast::ImplBlock, iface: usize, diags: &mut Diagnostics) {
    let info = &globals.interfaces[iface];
    for m in info.methods.iter().filter(|m| !m.has_default) {
        if !imp.methods.iter().any(|d| d.name.name == m.name) {
            diags.error(
                Code::MISSING_INTERFACE_METHOD,
                imp.span,
                format!("this implementation of `{}` is missing the method `{}`", info.name, m.name),
            );
        }
    }
}

/// What the methods of one impl block see of the type parameters, and how they are checked.
pub(crate) struct MethodScope<'a> {
    pub(crate) subst: &'a [(String, Ty)],
    pub(crate) bounds: &'a [(String, Vec<Bound>)],
    /// Checked against bounds only (the impl of a generic type with its parameters still open).
    pub(crate) symbolic: bool,
    /// Whether the bodies are checked. A symbolic copy that exists only to be found by name is not.
    pub(crate) check_body: bool,
}

impl MethodScope<'_> {
    /// An impl of a plain type: nothing to substitute, checked as written.
    pub(crate) fn plain() -> MethodScope<'static> {
        MethodScope { subst: &[], bounds: &[], symbolic: false, check_body: true }
    }
}

impl<'t> Sema<'t> {
    /// Registers the methods of an impl block for one type, and the default methods of its interface that the
    /// block does not override.
    pub(crate) fn register_methods(
        &mut self,
        imp: &'t ast::ImplBlock,
        iface: Option<usize>,
        self_ty: &Ty,
        scope: MethodScope,
        diags: &mut Diagnostics,
    ) {
        let Some(key) = adt_key(self_ty) else { return };
        for m in &imp.methods {
            self.add_method(key, self_ty, m, &scope, diags);
        }
        if let Some(i) = iface {
            let interface: &'t ast::InterfaceDecl = self.tpl.ifaces[i];
            for im in &interface.methods {
                if im.has_body && !imp.methods.iter().any(|m| m.name.name == im.decl.name.name) {
                    self.add_method(key, self_ty, &im.decl, &scope, diags);
                }
            }
        }
    }

    fn add_method(
        &mut self,
        key: (bool, usize),
        self_ty: &Ty,
        decl: &'t ast::FnDecl,
        scope: &MethodScope,
        diags: &mut Diagnostics,
    ) {
        if !decl.generics.is_empty() || !decl.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, decl.name.span, "generic methods are not implemented in this compiler yet");
            return;
        }
        let clashes_with_variant = match self_ty {
            Ty::Enum(a) => self.src.enums[a.id].0.variants.iter().any(|v| v.name.name == decl.name.name),
            _ => false,
        };
        let method_key = (key, decl.name.name.clone());
        if self.globals.methods.contains_key(&method_key) || clashes_with_variant {
            diags.error(
                Code::UNSUPPORTED,
                decl.name.span,
                format!("`{self_ty}` already has a member named `{}`", decl.name.name),
            );
            return;
        }
        let item = FnItem {
            decl,
            name: format!("{self_ty}.{}", decl.name.name),
            self_ty: Some(self_ty.clone()),
            subst: scope.subst.to_vec(),
            bounds: scope.bounds.to_vec(),
            symbolic: scope.symbolic,
            check_body: scope.check_body,
        };
        let id = self.push_fn(item);
        self.globals.methods.insert(method_key, id);
    }
}
