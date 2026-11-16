//! Impl blocks: `Type { ... }`, `Type: Drop { ... }`, and `Type: Interface { ... }`.

use crate::builtins::builtin_type;
use crate::generics::{BUILTIN_INTERFACES, Bound, Head, ImplInfo, param_ty};
use crate::globals::{FromImpl, Globals, TypeName, adt_key};
use crate::session::{FnItem, ImplTemplate, Sema};
use burdin_diag::{Code, Diagnostics, Span};
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
    pub(crate) fn register_impl(&mut self, imp: &'t ast::ImplBlock, module: usize, diags: &mut Diagnostics) {
        let Some((kind, iface_seg)) = self.impl_kind(imp, module, diags) else { return };

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
                self.register_generic_impl(imp, module, g, seg, kind, iface_seg, diags);
                return;
            }
        }

        if !imp.where_clause.is_empty() {
            diags.error(Code::UNSUPPORTED, imp.span, "`where` clauses are not implemented in this compiler yet"); // e9000: impl-where-concrete
        }
        // Aliases are resolved after the impl blocks are registered, so methods cannot be added through one.
        if let Some(name) = self.impl_target_alias(imp, module) {
            diags.error(
                Code::TYPE_MISMATCH,
                imp.ty.span,
                format!("`{name}` is a type alias: an impl block names the type itself, not an alias of it"),
            );
            return;
        }
        let self_ty = self.globals.resolve_type(&imp.ty, None, module, diags);
        let Some(key) = adt_key(&self_ty) else {
            if matches!(kind, ImplKind::Builtin | ImplKind::From) && self_ty != Ty::Error {
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
        let iface = match &kind {
            ImplKind::User(i) => Some((*i, self.resolve_iface_args(*i, iface_seg, &[], &[], module, imp.span, diags))),
            _ => None,
        };
        match kind {
            ImplKind::Builtin => builtin_by_hand(imp.span, diags),
            ImplKind::From => {
                let source = iface_seg
                    .and_then(|s| s.args.first())
                    .map(|t| self.globals.resolve_type_in(t, None, &[], module, diags))
                    .unwrap_or(Ty::Error);
                self.register_from_impl(imp, module, &self_ty, key, source, diags);
            }
            ImplKind::Drop => self.register_drop_impl(imp, module, &self_ty, key, &MethodScope::plain(), diags),
            ImplKind::Inherent | ImplKind::User(_) => {
                let iface_args = iface.as_ref().map(|(_, args)| args.clone()).unwrap_or_default();
                if let Some((i, args)) = iface {
                    check_missing_methods(&self.globals, imp, i, diags);
                    self.globals.impls.push(ImplInfo {
                        head: Head::Plain(key),
                        iface: i,
                        iface_args: args,
                        params: Vec::new(),
                        args: Vec::new(),
                        wheres: Vec::new(),
                        span: imp.span,
                    });
                }
                let iface = kind.user_interface().map(|i| (i, iface_args));
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
            ImplKind::From if !local_type => {
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

    /// What kind of impl block this is, from the interface it names, and the interface segment that carries its
    /// type arguments. `None` after reporting an unknown or unsupported interface.
    fn impl_kind<'a>(
        &self,
        imp: &'a ast::ImplBlock,
        module: usize,
        diags: &mut Diagnostics,
    ) -> Option<(ImplKind, Option<&'a ast::PathSegment>)> {
        let Some(t) = &imp.interface else { return Some((ImplKind::Inherent, None)) };
        let ast::TypeKind::Path(segs) = &t.kind else {
            diags.error(
                Code::UNSUPPORTED, // e9000: impl-interface-path-kind
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
                return Some((ImplKind::Drop, None));
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
                return Some((ImplKind::From, Some(seg)));
            }
            if BUILTIN_INTERFACES.contains(&name.as_str()) {
                if !seg.args.is_empty() {
                    diags.error(Code::TYPE_MISMATCH, t.span, format!("`{name}` takes no type arguments"));
                    return None;
                }
                return Some((ImplKind::Builtin, None));
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
        Some((ImplKind::User(i), Some(seg)))
    }

    /// Resolves a generic interface's type arguments for an impl (`Type: Iterator<i32>`), checks their number and
    /// the interface's bounds on its parameters, and returns them. `env` holds the impl's `where` bounds.
    #[allow(clippy::too_many_arguments)]
    fn resolve_iface_args(
        &self,
        iface: usize,
        seg: Option<&ast::PathSegment>,
        subst: &[(String, Ty)],
        env: &[(String, Vec<Bound>)],
        module: usize,
        span: Span,
        diags: &mut Diagnostics,
    ) -> Vec<Ty> {
        let info = &self.globals.interfaces[iface];
        let args = seg.map(|s| s.args.as_slice()).unwrap_or(&[]);
        if args.len() != info.params.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` takes {} type argument(s), but {} were given", info.name, info.params.len(), args.len()),
            );
            return vec![Ty::Error; info.params.len()];
        }
        let resolved: Vec<Ty> =
            args.iter().map(|a| self.globals.resolve_type_in(a, None, subst, module, diags)).collect();
        for (k, arg) in resolved.iter().enumerate() {
            for bound in info.bounds.get(k).into_iter().flatten() {
                if !self.globals.satisfies_with(arg, bound, env, subst) {
                    let span = args[k].span;
                    diags.error(
                        Code::BOUND_NOT_SATISFIED,
                        span,
                        format!(
                            "`{arg}` does not implement `{}`, which the parameter of `{}` requires",
                            self.globals.bound_name(bound),
                            info.name
                        ),
                    );
                }
            }
        }
        resolved
    }

    /// An impl on a generic type: it becomes a template that is instantiated with every instance its arguments
    /// match, which are concrete (`Stack<i32>`), names (`Pair<i32, B>`), or both (`Holder<List<T>>`).
    #[allow(clippy::too_many_arguments)]
    fn register_generic_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
        g: usize,
        seg: &ast::PathSegment,
        kind: ImplKind,
        iface_seg: Option<&ast::PathSegment>,
        diags: &mut Diagnostics,
    ) {
        // The names the impl gives the type's arguments: every name in them that is not a type. `Stack<T>` has
        // one, `Pair<i32, B>` has one, and `Stack<i32>` has none (`spec/impls.md`, IM1).
        let mut params = Vec::new();
        let mut repeated = false;
        for arg in &seg.args {
            repeated |= !self.collect_impl_params(arg, module, &mut params, diags);
        }
        if repeated {
            return;
        }
        let adt_bounds = self.globals.generic_adts[g].bounds.clone();
        if seg.args.len() != adt_bounds.len() {
            diags.error(
                Code::TYPE_MISMATCH,
                imp.ty.span,
                format!(
                    "`{}` takes {} type argument(s), but {} were given",
                    seg.name.name,
                    adt_bounds.len(),
                    seg.args.len()
                ),
            );
            return;
        }
        let subst: Vec<(String, Ty)> = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
        let pattern: Vec<Ty> = seg
            .args
            .iter()
            .zip(&adt_bounds)
            .map(|(a, bounds)| match Globals::const_param_ty(bounds) {
                Some(declared) => self.globals.eval_const_arg(a, declared, &subst, module, diags),
                None => self.globals.resolve_type_in(a, None, &subst, module, diags),
            })
            .collect();
        if pattern.contains(&Ty::Error) {
            return;
        }
        // The arguments meet the type's bounds, as at any other use of it (E0141).
        self.globals.check_adt_bounds(g, &pattern, imp.ty.span, diags);
        let plain = crate::impl_match::is_plain_pattern(&pattern);
        if matches!(kind, ImplKind::Builtin) {
            builtin_by_hand(imp.span, diags);
            return;
        }
        // A `Drop` implementation covers every instance: the drop glue of one instance cannot differ from the
        // type's others (`spec/impls.md`, IM7).
        if matches!(kind, ImplKind::Drop) && (!plain || !imp.where_clause.is_empty()) {
            diags.error(
                Code::INVALID_DROP_IMPL,
                imp.span,
                "a `Drop` implementation covers every instance of its type, so it cannot name specific type \
                 arguments or have a `where` clause",
            );
            return;
        }
        let wheres = self.impl_wheres(imp, module, &params, diags);
        let iface = kind.user_interface();
        let iface_args = match iface {
            Some(i) => {
                check_missing_methods(&self.globals, imp, i, diags);
                let args = self.resolve_iface_args(i, iface_seg, &subst, &wheres, module, imp.span, diags);
                // The later of two overlapping impls is left out, so its methods do not also clash with the
                // earlier one's on the instances both apply to.
                if self.check_impl_overlap(g, i, &pattern, &args, imp.span, diags) {
                    return;
                }
                self.globals.impls.push(ImplInfo {
                    head: Head::Generic(g),
                    iface: i,
                    iface_args: args.clone(),
                    params: params.clone(),
                    args: pattern.clone(),
                    wheres: wheres.clone(),
                    span: imp.span,
                });
                args
            }
            None => Vec::new(),
        };
        // The instance an impl on specific arguments is checked on: the one whose arguments are its pattern. It is
        // symbolic when the pattern has names left (`Pair<i32, B>`), and its methods' bodies are checked there.
        if !plain {
            self.globals.requests.borrow_mut().push(crate::generics::Request::Adt(g, pattern.clone()));
        }
        let is_drop = matches!(kind, ImplKind::Drop);
        self.tpl.impls.push(ImplTemplate {
            decl: imp,
            module,
            template: g,
            params,
            args: pattern,
            wheres,
            iface,
            iface_args,
            is_drop,
        });
    }

    /// The type alias an impl block's type is written through, if it is one.
    fn impl_target_alias(&self, imp: &ast::ImplBlock, module: usize) -> Option<String> {
        let ast::TypeKind::Path(segs) = &imp.ty.kind else { return None };
        let last = segs.last()?;
        let found = if segs.len() == 1 {
            self.globals.type_name(module, &last.name.name)
        } else {
            let names: Vec<String> = segs[..segs.len() - 1].iter().map(|s| s.name.name.clone()).collect();
            let target = self.globals.resolve_module_path(module, &names)?;
            self.globals.exported_type(target, &last.name.name)
        };
        match found {
            Some(TypeName::Alias(_) | TypeName::GenericAlias(_)) => Some(last.name.name.clone()),
            _ => None,
        }
    }

    /// The names in an impl's type arguments that no type goes by: they stand for whatever the argument is. A name
    /// stands for one argument, so a name written twice is E0007 (`spec/impls.md`, IM1); the result is false then.
    fn collect_impl_params(
        &self,
        ty: &ast::Type,
        module: usize,
        out: &mut Vec<String>,
        diags: &mut Diagnostics,
    ) -> bool {
        match &ty.kind {
            ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => {
                let name = &segs[0].name.name;
                if self.globals.type_name(module, name).is_some() || builtin_type(name).is_some() {
                    return true;
                }
                if out.contains(name) {
                    diags.error(Code::DUPLICATE_NAME, ty.span, format!("type parameter `{name}` is declared twice"));
                    return false;
                }
                out.push(name.clone());
                true
            }
            ast::TypeKind::Path(segs) => {
                let mut ok = true;
                for arg in segs.iter().flat_map(|seg| &seg.args) {
                    ok = self.collect_impl_params(arg, module, out, diags) && ok;
                }
                ok
            }
            ast::TypeKind::Tuple(items) => {
                let mut ok = true;
                for t in items {
                    ok = self.collect_impl_params(t, module, out, diags) && ok;
                }
                ok
            }
            ast::TypeKind::Slice(t) | ast::TypeKind::Array(t, _) | ast::TypeKind::Ref { ty: t, .. } => {
                self.collect_impl_params(t, module, out, diags)
            }
            _ => true,
        }
    }

    /// E0143: two impls of one interface for one generic type apply to a common instance when their argument
    /// patterns and interface arguments unify (`spec/impls.md`, IM4). `where` clauses do not tell them apart.
    /// Returns whether the impl overlaps an earlier one.
    fn check_impl_overlap(
        &self,
        template: usize,
        iface: usize,
        pattern: &[Ty],
        iface_args: &[Ty],
        span: Span,
        diags: &mut Diagnostics,
    ) -> bool {
        let mut mine = pattern.to_vec();
        mine.extend(iface_args.iter().cloned());
        let earlier = self.globals.impls.iter().find(|info| {
            info.iface == iface && info.head == Head::Generic(template) && {
                let mut theirs = info.args.clone();
                theirs.extend(info.iface_args.iter().cloned());
                self.globals.impl_patterns_overlap(&mine, &theirs)
            }
        });
        if earlier.is_none() {
            return false;
        }
        diags.error(
            Code::OVERLAPPING_IMPLS,
            span,
            format!(
                "this implementation of `{}` overlaps an earlier one: some instance of the type would have both",
                self.globals.interfaces[iface].name
            ),
        );
        true
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
                    Code::UNSUPPORTED, // e9000: impl-where-non-param
                    pred.ty.span,
                    "a `where` clause on something other than a type parameter is not implemented in this compiler yet",
                );
                continue;
            };
            let subst: Vec<(String, Ty)> = params.iter().map(|p| (p.clone(), crate::generics::param_ty(p))).collect();
            wheres.push((
                name,
                pred.bounds.iter().filter_map(|b| self.globals.resolve_bound(b, &subst, module, diags)).collect(),
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
        if !m.generics.is_empty() || !m.where_clause.is_empty() {
            diags.error(
                Code::TYPE_MISMATCH,
                m.name.span,
                "a `From` conversion cannot be generic: `?` calls it with exactly one argument",
            );
            return;
        }
        // Each target converts from a source type once (`spec/errors.md`, ERR3). The two generated blocks are
        // separate impl blocks, so the duplicate is detected here rather than as a duplicate member: a source
        // written through a qualified path (`std.IoError` and `IoError`) is the same type here but not in the
        // source text the syntax check compares.
        if self.globals.from_impls.iter().any(|i| i.target == *self_ty && i.source == source) {
            diags.error(
                Code::DUPLICATE_FROM,
                imp.span,
                format!("`{self_ty}` already converts from `{source}`; a type converts from each source type once"),
            );
            return;
        }
        self.register_methods(imp, module, None, self_ty, MethodScope::plain(), diags);
        let Some(&func) = self.globals.methods.get(&(key, "from".to_string())) else { return };
        self.globals.from_methods.push(func);
        self.globals.from_impls.push(FromImpl { target: self_ty.clone(), source, func });
    }

    /// `Type<T>: Drop { ... }` for one instance of the generic type, with what the block's names stand for.
    pub(crate) fn register_generic_drop(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
        self_ty: &Ty,
        scope: &MethodScope,
        diags: &mut Diagnostics,
    ) {
        let Some(key) = adt_key(self_ty) else { return };
        self.register_drop_impl(imp, module, self_ty, key, scope, diags);
    }

    /// `Type: Drop { fn drop(mut self) { ... } }`: the one method the drop glue calls.
    fn register_drop_impl(
        &mut self,
        imp: &'t ast::ImplBlock,
        module: usize,
        self_ty: &Ty,
        key: (bool, usize),
        scope: &MethodScope,
        diags: &mut Diagnostics,
    ) {
        if imp.methods.len() != 1 {
            diags.error(
                Code::INVALID_DROP_IMPL,
                imp.span,
                "a `Drop` implementation must contain exactly one `drop` method",
            );
            return;
        }
        let m = &imp.methods[0];
        if m.name.name != "drop" {
            diags.error(Code::INVALID_DROP_IMPL, m.name.span, "a `Drop` implementation must define `drop`");
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
                Code::DUPLICATE_NAME,
                m.name.span,
                format!("`{self_ty}` already has a member named `{}`", m.name.name),
            );
            return;
        }
        let mut item = FnItem::plain(m, format!("{self_ty}.drop"), module, Some(self_ty.clone()));
        item.subst = scope.subst.to_vec();
        item.bounds = scope.bounds.to_vec();
        item.symbolic = scope.symbolic;
        item.check_body = scope.check_body;
        let id = self.push_fn(item);
        self.globals.drop_methods.insert(key, id);
        match key.0 {
            false => self.globals.structs[key.1].drop_method = Some(id),
            true => self.globals.enums[key.1].drop_method = Some(id),
        }
        self.drop_checks.push((id, m.span));
    }
}

/// A built-in interface other than `Drop` written by hand (`Type: Eq { ... }`), for a generic type or not.
fn builtin_by_hand(span: Span, diags: &mut Diagnostics) {
    diags.error(
        Code::UNSUPPORTED, // e9000: impl-builtin-hand
        span,
        "implementing a built-in interface by hand is not implemented in this compiler yet; use `derives`",
    );
}

/// What an impl block implements.
#[derive(Clone)]
enum ImplKind {
    /// `Type { ... }`: methods of the type itself.
    Inherent,
    /// `Type: Drop { ... }`.
    Drop,
    /// `Type: From<Source> { fn from(value: Source): Type }`.
    From,
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
        iface: Option<(usize, Vec<Ty>)>,
        self_ty: &Ty,
        scope: MethodScope,
        diags: &mut Diagnostics,
    ) {
        let Some(key) = adt_key(self_ty) else { return };
        for m in &imp.methods {
            self.add_method(key, self_ty, m, module, &scope, diags);
        }
        if let Some((i, args)) = iface {
            // A symbolic instance exists only while a generic body is checked against its bounds; its calls go
            // through the bound's methods, not through copies of the interface's defaults.
            if scope.symbolic {
                return;
            }
            // The iteration protocol's defaults are instantiated on demand at each call (fncx::iterate): a
            // default that returns an adapter of `Self` would otherwise instantiate adapters without end.
            if Some(i) == self.globals.iterator_iface {
                return;
            }
            let interface: &'t ast::InterfaceDecl = self.tpl.ifaces[i];
            let iface_module = self.globals.interfaces[i].module;
            let params = self.globals.interfaces[i].params.clone();
            let iface_bounds = self.globals.interfaces[i].bounds.clone();
            // A default method is written in the interface's module: only `Self`, the interface's parameters,
            // and the method's own parameters are names in it. The impl's parameters are not, so they cannot
            // shadow them; their bounds still join the environment last, for the types the impl substituted.
            let subst: Vec<(String, Ty)> = params.iter().cloned().zip(args.iter().cloned()).collect();
            let mut bounds: Vec<(String, Vec<Bound>)> = Vec::new();
            for (p, bs) in params.iter().zip(&iface_bounds) {
                let bs: Vec<Bound> = bs.iter().map(|b| self.globals.subst_bound(b, &subst)).collect();
                bounds.push((p.clone(), bs));
            }
            bounds.extend(scope.bounds.iter().cloned());
            let defaults: Vec<&'t ast::FnDecl> = interface
                .methods
                .iter()
                .filter(|im| im.has_body && !imp.methods.iter().any(|m| m.name.name == im.decl.name.name))
                .map(|im| &im.decl)
                .collect();
            let default_scope =
                MethodScope { subst: &subst, bounds: &bounds, symbolic: scope.symbolic, check_body: scope.check_body };
            for decl in defaults {
                self.add_method(key, self_ty, decl, iface_module, &default_scope, diags);
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
        // `Type.name`, `.size`, `.align`, `.fields`, and `.variants` are the reflection members, so a function
        // without `self` cannot take one of those names (`spec/reflection.md`, RFL3). A method with `self` is
        // called on a value and does not conflict.
        let has_self = matches!(decl.params.first().map(|p| &p.kind), Some(ast::ParamKind::SelfParam));
        if !has_self && crate::fncx::RESERVED_MEMBERS.contains(&decl.name.name.as_str()) {
            diags.error(
                Code::RESERVED_MEMBER,
                decl.name.span,
                format!(
                    "`{}` is reserved for reflection, so `{self_ty}.{}` cannot be a function; give it another name",
                    decl.name.name, decl.name.name
                ),
            );
            return;
        }
        let clashes_with_variant = match self_ty {
            Ty::Enum(a) => self.src.enums[a.id].decl.variants.iter().any(|v| v.name.name == decl.name.name),
            _ => false,
        };
        let method_key = (key, decl.name.name.clone());
        if self.globals.methods.contains_key(&method_key) || clashes_with_variant {
            diags.error(
                Code::DUPLICATE_NAME,
                decl.name.span,
                format!("`{self_ty}` already has a member named `{}`", decl.name.name),
            );
            return;
        }
        // A method-level type parameter is opaque in the identity function; each call instantiates it. It comes
        // first so that the method's own `F` wins over an impl parameter with the same name in a default method.
        let mut subst: Vec<(String, Ty)> = Vec::new();
        for g in &decl.generics {
            subst.push((g.name.name.clone(), param_ty(&g.name.name)));
        }
        subst.extend(scope.subst.iter().cloned());
        let method_bounds = crate::declare::method_bounds(decl, scope.subst, &self.globals, module, diags);
        let mut bounds: Vec<(String, Vec<Bound>)> = Vec::new();
        for (g, bs) in decl.generics.iter().zip(&method_bounds) {
            bounds.push((g.name.name.clone(), bs.clone()));
        }
        bounds.extend(scope.bounds.iter().cloned());
        let generics: Vec<String> = decl.generics.iter().map(|g| g.name.name.clone()).collect();
        let item = FnItem {
            decl,
            name: format!("{self_ty}.{}", decl.name.name),
            module,
            self_ty: Some(self_ty.clone()),
            subst,
            bounds,
            symbolic: scope.symbolic || !generics.is_empty(),
            check_body: scope.check_body,
            is_unsafe: decl.is_unsafe,
            is_exported: false,
        };
        let id = self.push_fn(item);
        self.globals.methods.insert(method_key.clone(), id);
        self.globals.method_priv.insert(method_key.clone(), decl.is_priv);
        self.globals.method_spans.insert(method_key.clone(), decl.name.span);
        self.globals.method_pkg.insert(method_key, self.globals.package_of(module));
        if !generics.is_empty() {
            let template = crate::session::MethodTemplate {
                decl,
                name: format!("{self_ty}.{}", decl.name.name),
                module,
                self_ty: Some(self_ty.clone()),
                base_subst: scope.subst.to_vec(),
                base_bounds: scope.bounds.to_vec(),
                params: generics,
                bounds: method_bounds,
                is_unsafe: decl.is_unsafe,
            };
            let index = self.tpl.methods.len();
            self.tpl.methods.push(template);
            self.globals.method_templates.insert(
                id,
                crate::generics::GenericMethod {
                    template: index,
                    identity: id,
                    params: self.tpl.methods[index].params.clone(),
                    bounds: self.tpl.methods[index].bounds.clone(),
                },
            );
        }
    }
}
