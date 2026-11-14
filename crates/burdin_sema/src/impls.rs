//! Impl blocks: `Type { ... }`, `Type: Drop { ... }`, and `Type: Interface { ... }`.

use crate::builtins::builtin_type;
use crate::generics::{BUILTIN_INTERFACES, Bound, Head, ImplInfo};
use crate::globals::{FromImpl, Globals, TypeName, adt_key};
use crate::session::{FnItem, ImplTemplate, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::Ty;

impl<'t> Sema<'t> {
    /// Registers the impl blocks of every module. This runs after imports, because the type and the interface it
    /// names may be imported.
    pub(crate) fn register_impls(&mut self, diags: &mut Diagnostics) {
        for module in 0..self.module_asts.len() {
            let ast = self.module_asts[module];
            for item in &ast.items {
                if let ast::Item::Impl(imp) = item {
                    self.register_impl(imp, module, diags);
                }
            }
        }
    }

    /// Registers an impl block. Methods of an impl on a generic type are registered when each instance of the
    /// type is allocated.
    fn register_impl(&mut self, imp: &'t ast::ImplBlock, module: usize, diags: &mut Diagnostics) {
        let Some(kind) = self.impl_kind(imp, module, diags) else { return };

        // An impl on a generic type, written plainly or through a module: `Stack<T> { ... }`,
        // `sh.Stack<T>: Describe where T: Describe { ... }`.
        if let ast::TypeKind::Path(segs) = &imp.ty.kind {
            let generic = if segs.len() == 1 {
                match self.globals.type_name(module, &segs[0].name.name) {
                    Some(TypeName::GenericAdt(g)) => Some((g, &segs[0])),
                    _ => None,
                }
            } else {
                let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
                self.globals.resolve_module_path(module, &names).and_then(|target| {
                    let seg = segs.last().expect("the path has a last segment");
                    match self.globals.exported_type(target, &seg.name.name) {
                        Some(TypeName::GenericAdt(g)) => Some((g, seg)),
                        _ => None,
                    }
                })
            };
            if let Some((g, seg)) = generic {
                let type_pkg = self.globals.package_of(self.globals.generic_adts[g].module);
                if !self.check_impl_coherence(&kind, type_pkg, imp, module, diags) {
                    return;
                }
                self.register_generic_impl(imp, module, g, seg, kind, diags);
                return;
            }
        }

        if !imp.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, imp.span, "`where` clauses are not implemented in this compiler yet");
        }
        let self_ty = self.globals.resolve_type(&imp.ty, None, module, diags);
        let Some(key) = adt_key(&self_ty) else {
            if matches!(kind, ImplKind::Builtin | ImplKind::From(_)) && self_ty != Ty::Error {
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
        let type_pkg = if key.0 { self.globals.enum_pkg[key.1] } else { self.globals.struct_pkg[key.1] };
        if !self.check_impl_coherence(&kind, type_pkg, imp, module, diags) {
            return;
        }
        match kind {
            ImplKind::Builtin => {
                diags.error(
                    Code::UNSUPPORTED,
                    imp.span,
                    "implementing a built-in interface by hand is not implemented in this compiler yet; use `derives`",
                );
            }
            ImplKind::From(source) => self.register_from_impl(imp, module, &self_ty, key, source, diags),
            ImplKind::Drop => self.register_drop_impl(imp, module, &self_ty, key, diags),
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
                self.register_methods(imp, module, iface, &self_ty, MethodScope::plain(), diags);
            }
        }
    }

    /// The orphan rule (E0142): inherent methods and `Drop` belong to the type's package, and an interface may be
    /// implemented for a foreign type only when this package declares the interface.
    fn check_impl_coherence(
        &self,
        kind: &ImplKind,
        type_pkg: usize,
        imp: &ast::ImplBlock,
        module: usize,
        diags: &mut Diagnostics,
    ) -> bool {
        let package = self.globals.package_of(module);
        let local_type = type_pkg == package;
        match kind {
            ImplKind::Inherent if !local_type => {
                diags.error(
                    Code::ORPHAN_RULE,
                    imp.span,
                    "inherent methods can only be added to a type declared in this package",
                );
                false
            }
            ImplKind::Drop if !local_type => {
                diags.error(
                    Code::ORPHAN_RULE,
                    imp.span,
                    "`Drop` can only be implemented for a type declared in this package",
                );
                false
            }
            ImplKind::From(_) if !local_type => {
                diags.error(
                    Code::ORPHAN_RULE,
                    imp.span,
                    "`From` can only be implemented for a type declared in this package",
                );
                false
            }
            ImplKind::User(i)
                if !local_type && self.globals.package_of(self.globals.interfaces[*i].module) != package =>
            {
                diags.error(Code::ORPHAN_RULE, imp.span, "this package declares neither the type nor the interface");
                false
            }
            _ => true,
        }
    }

    /// What kind of impl block this is, from the interface it names. `None` after reporting an unknown or
    /// unsupported interface.
    fn impl_kind(&self, imp: &ast::ImplBlock, module: usize, diags: &mut Diagnostics) -> Option<ImplKind> {
        let Some(t) = &imp.interface else { return Some(ImplKind::Inherent) };
        let ast::TypeKind::Path(segs) = &t.kind else {
            diags.error(
                Code::UNSUPPORTED,
                t.span,
                "this kind of interface path is not implemented in this compiler yet",
            );
            return None;
        };
        let (lookup, seg, qualified) = if segs.len() == 1 {
            (module, &segs[0], false)
        } else {
            let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
            let Some(target) = self.globals.resolve_module_path(module, &names) else {
                diags.error(Code::UNKNOWN_NAME, t.span, format!("unknown module `{}`", names.join(".")));
                return None;
            };
            (target, segs.last().expect("the path has a last segment"), true)
        };
        let name = &seg.name.name;
        if !qualified {
            if name == "Drop" {
                return Some(ImplKind::Drop);
            }
            if name == "From" {
                if seg.args.len() != 1 {
                    diags.error(
                        Code::ARG_COUNT,
                        t.span,
                        format!("`From` takes one type argument, but {} were given", seg.args.len()),
                    );
                    return None;
                }
                let source = self.globals.resolve_type(&seg.args[0], None, module, diags);
                return Some(ImplKind::From(source));
            }
            if BUILTIN_INTERFACES.contains(&name.as_str()) {
                return Some(ImplKind::Builtin);
            }
        }
        let found =
            if qualified { self.globals.exported_iface(lookup, name) } else { self.globals.iface_name(module, name) };
        let Some(i) = found else {
            diags.error(Code::UNKNOWN_NAME, t.span, format!("unknown interface `{name}`"));
            return None;
        };
        if qualified
            && self.globals.package_of(lookup) != self.globals.package_of(module)
            && self.globals.modules[lookup].iface_priv.get(name).copied().unwrap_or(false)
        {
            diags.error(Code::PRIVATE_ITEM, seg.name.span, format!("interface `{name}` is `priv` in another package"));
        }
        if !seg.args.is_empty() {
            diags.error(Code::UNSUPPORTED, t.span, "generic interfaces are not implemented in this compiler yet");
            return None;
        }
        Some(ImplKind::User(i))
    }

    /// An impl on a generic type, written with the names it gives the type's arguments: it becomes a template that
    /// is instantiated with every instance of the type.
    fn register_generic_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
        g: usize,
        seg: &ast::PathSegment,
        kind: ImplKind,
        diags: &mut Diagnostics,
    ) {
        let mut params = Vec::new();
        for arg in &seg.args {
            match &arg.kind {
                ast::TypeKind::Path(s)
                    if s.len() == 1
                        && s[0].args.is_empty()
                        && self.globals.type_name(module, &s[0].name.name).is_none()
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
                format!("`{}` takes {wanted} type argument(s), but {} were given", seg.name.name, params.len()),
            );
            return;
        }
        for (i, p) in params.iter().enumerate() {
            if params[..i].contains(p) {
                diags.error(Code::UNSUPPORTED, seg.args[i].span, format!("type parameter `{p}` is declared twice"));
            }
        }
        if matches!(kind, ImplKind::Drop | ImplKind::Builtin) {
            diags.error(
                Code::UNSUPPORTED,
                imp.span,
                "implementing a built-in interface for a generic type is not implemented in this compiler yet",
            );
            return;
        }
        let wheres = self.impl_wheres(imp, module, &params, diags);
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
        self.tpl.impls.push(ImplTemplate { decl: imp, module, template: g, params, wheres, iface });
    }

    /// The bounds an impl block's `where` clause puts on the names it gives the type's arguments.
    fn impl_wheres(
        &self,
        imp: &ast::ImplBlock,
        module: usize,
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
            wheres.push((
                name,
                pred.bounds.iter().filter_map(|b| self.globals.resolve_bound(b, module, diags)).collect(),
            ));
        }
        wheres
    }

    /// `Type: From<Source> { fn from(value: Source): Type }`: the conversion `?` applies to an error.
    fn register_from_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
        self_ty: &Ty,
        key: (bool, usize),
        source: Ty,
        diags: &mut Diagnostics,
    ) {
        let Some(m) = imp.methods.iter().find(|m| m.name.name == "from") else {
            diags.error(
                Code::MISSING_INTERFACE_METHOD,
                imp.span,
                "this implementation of `From` is missing the method `from`",
            );
            return;
        };
        if !m.generics.is_empty() {
            diags.error(Code::UNSUPPORTED, m.name.span, "generic methods are not implemented in this compiler yet");
            return;
        }
        self.register_methods(imp, module, None, self_ty, MethodScope::plain(), diags);
        let Some(&func) = self.globals.methods.get(&(key, "from".to_string())) else { return };
        self.globals.from_methods.push(func);
        self.globals.from_impls.push(FromImpl { target: self_ty.clone(), source, func });
    }

    /// `Type: Drop { fn drop(mut self) { ... } }`: the one method the drop glue calls.
    fn register_drop_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
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
            Ty::Enum(a) => self.src.enums[a.id].decl.variants.iter().any(|v| v.name.name == m.name.name),
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
        let id = self.push_fn(FnItem::plain(m, format!("{self_ty}.drop"), module, Some(self_ty.clone())));
        self.globals.drop_methods.insert(key, id);
        match key.0 {
            false => self.globals.structs[key.1].drop_method = Some(id),
            true => self.globals.enums[key.1].drop_method = Some(id),
        }
        self.drop_checks.push((id, m.span));
    }
}

/// What an impl block implements.
#[derive(Clone)]
enum ImplKind {
    /// `Type { ... }`: methods of the type itself.
    Inherent,
    /// `Type: Drop { ... }`.
    Drop,
    /// `Type: From<Source> { fn from(value: Source): Type }`.
    From(Ty),
    /// A built-in interface other than `Drop`, which is only derived for now.
    Builtin,
    /// `Type: Interface { ... }` for an interface declared in the program (an index into the interfaces).
    User(usize),
}

impl ImplKind {
    fn user_interface(&self) -> Option<usize> {
        match self {
            ImplKind::User(i) => Some(*i),
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
        module: usize,
        iface: Option<usize>,
        self_ty: &Ty,
        scope: MethodScope,
        diags: &mut Diagnostics,
    ) {
        let Some(key) = adt_key(self_ty) else { return };
        for m in &imp.methods {
            self.add_method(key, self_ty, m, module, &scope, diags);
        }
        if let Some(i) = iface {
            let interface: &'t ast::InterfaceDecl = self.tpl.ifaces[i];
            let iface_module = self.globals.interfaces[i].module;
            for im in &interface.methods {
                if im.has_body && !imp.methods.iter().any(|m| m.name.name == im.decl.name.name) {
                    self.add_method(key, self_ty, &im.decl, iface_module, &scope, diags);
                }
            }
        }
    }

    fn add_method(
        &mut self,
        key: (bool, usize),
        self_ty: &Ty,
        decl: &'t ast::FnDecl,
        module: usize,
        scope: &MethodScope,
        diags: &mut Diagnostics,
    ) {
        if !decl.generics.is_empty() || !decl.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, decl.name.span, "generic methods are not implemented in this compiler yet");
            return;
        }
        let clashes_with_variant = match self_ty {
            Ty::Enum(a) => self.src.enums[a.id].decl.variants.iter().any(|v| v.name.name == decl.name.name),
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
            module,
            self_ty: Some(self_ty.clone()),
            subst: scope.subst.to_vec(),
            bounds: scope.bounds.to_vec(),
            symbolic: scope.symbolic,
            check_body: scope.check_body,
            is_unsafe: decl.is_unsafe,
        };
        let id = self.push_fn(item);
        self.globals.methods.insert(method_key.clone(), id);
        self.globals.method_priv.insert(method_key.clone(), decl.is_priv);
        self.globals.method_pkg.insert(method_key, self.globals.package_of(module));
    }
}
