//! The declaration phases: collect the names of types, interfaces, functions, constants, and extern functions,
//! then everything that needs those names (imports, interface requirements, bounds, impl blocks, the identity
//! instances of generic types).

use crate::builtins::{builtin, builtin_type};
use crate::generics::{BUILTIN_INTERFACES, Bound, GenericAdt, GenericFn, IfaceMethod, InterfaceInfo, param_ty};
use crate::globals::{ExternInfo, Global, Globals, TypeName};
use crate::session::{FnItem, NamedSrc, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AdtRef, ConstId, Signature, Ty};
use std::rc::Rc;

/// An `extern` declaration whose signature is still to be resolved.
pub(crate) struct ExternSrc<'t> {
    pub(crate) decl: &'t ast::FnDecl,
    pub(crate) module: usize,
    pub(crate) extern_id: usize,
}

impl<'t> Sema<'t> {
    /// The module's name and the package it belongs to.
    fn package(&self, module: usize) -> usize {
        self.globals.package_of(module)
    }

    /// Type and interface names first, so that every signature and field can refer to every type.
    pub(crate) fn declare_types(&mut self, diags: &mut Diagnostics) {
        for module in 0..self.module_asts.len() {
            for item in &self.module_asts[module].items {
                self.declare_type_item(module, item, diags);
            }
        }
    }

    fn declare_type_item(&mut self, module: usize, item: &'t ast::Item, diags: &mut Diagnostics) {
        if let ast::Item::Interface(i) = item {
            if !i.generics.is_empty() {
                diags.error(
                    Code::UNSUPPORTED,
                    i.name.span,
                    "generic interfaces are not implemented in this compiler yet",
                );
            }
            let name = &i.name.name;
            let module_info = &self.globals.modules[module];
            if module_info.ifaces.contains_key(name)
                || module_info.types.contains_key(name)
                || builtin_type(name).is_some()
                || BUILTIN_INTERFACES.contains(&name.as_str())
            {
                diags.error(Code::UNSUPPORTED, i.name.span, format!("`{name}` is defined more than once"));
                return;
            }
            let full = self.globals.qualified(module, name);
            let index = self.globals.interfaces.len();
            self.globals.modules[module].ifaces.insert(name.clone(), index);
            self.globals.modules[module].iface_priv.insert(name.clone(), i.is_priv);
            self.globals.interfaces.push(InterfaceInfo {
                name: full,
                module,
                requires: Vec::new(),
                methods: Vec::new(),
            });
            self.tpl.ifaces.push(i);
            return;
        }
        let (name, generics, is_priv) = match item {
            ast::Item::Struct(s) => (&s.name, &s.generics, s.is_priv),
            ast::Item::Enum(e) => (&e.name, &e.generics, e.is_priv),
            ast::Item::TypeAlias(a) => (&a.name, &a.generics, a.is_priv),
            _ => return,
        };
        if matches!(item, ast::Item::TypeAlias(_)) && !generics.is_empty() {
            diags.error(Code::UNSUPPORTED, name.span, "generic type aliases are not implemented in this compiler yet");
        }
        let module_info = &self.globals.modules[module];
        if module_info.types.contains_key(&name.name)
            || module_info.ifaces.contains_key(&name.name)
            || builtin_type(&name.name).is_some()
        {
            diags.error(Code::UNSUPPORTED, name.span, format!("type `{}` is defined more than once", name.name));
            return;
        }
        let full = self.globals.qualified(module, &name.name);
        let adt = |id| AdtRef { id, name: Rc::from(full.as_str()) };
        let params: Vec<String> = generics.iter().map(|g| g.name.name.clone()).collect();
        for (i, p) in params.iter().enumerate() {
            if params[..i].contains(p) {
                diags.error(
                    Code::UNSUPPORTED,
                    generics[i].name.span,
                    format!("type parameter `{p}` is declared twice"),
                );
            }
        }
        self.globals.modules[module].type_priv.insert(name.name.clone(), is_priv);
        let package = self.package(module);
        match item {
            ast::Item::Struct(s) if generics.is_empty() => {
                let id = self.globals.structs.len();
                self.globals.modules[module].types.insert(name.name.clone(), TypeName::Struct(id));
                self.globals.structs.push(tir::StructDef {
                    name: full.clone(),
                    fields: Vec::new(),
                    derives: tir::Derives::default(),
                    drop_method: None,
                });
                self.globals.struct_refs.push(adt(id));
                self.globals.struct_pkg.push(package);
                self.src.structs.push(crate::session::AdtSrcEntry { decl: s, subst: Vec::new(), module });
            }
            ast::Item::Enum(e) if generics.is_empty() => {
                let id = self.globals.enums.len();
                self.globals.modules[module].types.insert(name.name.clone(), TypeName::Enum(id));
                self.globals.enums.push(tir::EnumDef {
                    name: full.clone(),
                    variants: Vec::new(),
                    derives: tir::Derives::default(),
                    drop_method: None,
                });
                self.globals.enum_refs.push(adt(id));
                self.globals.enum_pkg.push(package);
                self.src.enums.push(crate::session::AdtSrcEntry { decl: e, subst: Vec::new(), module });
            }
            ast::Item::Struct(s) => {
                let g = self.globals.generic_adts.len();
                self.globals.modules[module].types.insert(name.name.clone(), TypeName::GenericAdt(g));
                self.globals.generic_adts.push(GenericAdt {
                    name: full,
                    module,
                    is_enum: false,
                    params,
                    bounds: Vec::new(),
                    decl: self.tpl.structs.len(),
                    identity: (false, 0),
                });
                self.tpl.structs.push(s);
            }
            ast::Item::Enum(e) => {
                let g = self.globals.generic_adts.len();
                self.globals.modules[module].types.insert(name.name.clone(), TypeName::GenericAdt(g));
                self.globals.generic_adts.push(GenericAdt {
                    name: full,
                    module,
                    is_enum: true,
                    params,
                    bounds: Vec::new(),
                    decl: self.tpl.enums.len(),
                    identity: (false, 0),
                });
                self.tpl.enums.push(e);
            }
            ast::Item::TypeAlias(a) => {
                let index = self.alias_decls.len();
                self.globals.modules[module].types.insert(name.name.clone(), TypeName::Alias(index));
                self.globals.aliases.push(Ty::Error);
                self.alias_decls.push(NamedSrc { decl: a, module });
            }
            _ => unreachable!(),
        }
    }

    /// What interfaces require, and the bounds of generic types.
    pub(crate) fn declare_bounds(&mut self, diags: &mut Diagnostics) {
        for i in 0..self.tpl.ifaces.len() {
            let decl = self.tpl.ifaces[i];
            let module = self.globals.interfaces[i].module;
            let requires = decl.requires.iter().filter_map(|t| self.globals.resolve_bound(t, module, diags)).collect();
            let methods = decl
                .methods
                .iter()
                .map(|m| IfaceMethod {
                    name: m.decl.name.name.clone(),
                    sig: Signature::placeholder(),
                    has_default: m.has_body,
                    is_unsafe: m.decl.is_unsafe,
                })
                .collect();
            self.globals.interfaces[i].requires = requires;
            self.globals.interfaces[i].methods = methods;
        }
        // Generic functions were named before imports; their bounds may name imported interfaces.
        let pending = std::mem::take(&mut self.generic_fn_bounds);
        for (index, decl, module) in pending {
            let bounds = fn_bounds(decl, &self.globals, module, diags);
            let identity = self.globals.generic_fns[index].identity;
            self.fn_items[identity.0].bounds =
                self.globals.generic_fns[index].params.iter().cloned().zip(bounds.iter().cloned()).collect();
            self.globals.generic_fns[index].bounds = bounds;
        }
        for g in 0..self.globals.generic_adts.len() {
            let info = &self.globals.generic_adts[g];
            let module = info.module;
            let generics =
                if info.is_enum { &self.tpl.enums[info.decl].generics } else { &self.tpl.structs[info.decl].generics };
            let bounds = generics
                .iter()
                .map(|p| p.bounds.iter().filter_map(|b| self.globals.resolve_bound(b, module, diags)).collect())
                .collect();
            self.globals.generic_adts[g].bounds = bounds;
        }
    }

    /// Functions, constants, extern functions, and the names of generic functions. Impl blocks are registered
    /// after imports resolve, by [`Sema::register_impls`].
    pub(crate) fn declare_values(&mut self, diags: &mut Diagnostics) {
        for module in 0..self.module_asts.len() {
            for item in &self.module_asts[module].items {
                match item {
                    ast::Item::Fn(f) => {
                        let name = &f.name.name;
                        if self.globals.modules[module].values.contains_key(name) || builtin(name).is_some() {
                            diags.error(Code::UNSUPPORTED, f.name.span, format!("`{name}` is defined more than once"));
                            continue;
                        }
                        let full = self.globals.qualified(module, name);
                        if f.generics.is_empty() && f.where_clause.is_empty() {
                            let id = self.push_fn(FnItem::plain(f, full, module, None));
                            self.globals.modules[module].values.insert(name.clone(), Global::Func(id));
                        } else {
                            let params: Vec<String> = f.generics.iter().map(|g| g.name.name.clone()).collect();
                            // The bounds are resolved after imports, which `declare_values` runs before.
                            let bounds: Vec<Vec<Bound>> = vec![Vec::new(); params.len()];
                            let env = params.iter().cloned().zip(bounds.iter().cloned()).collect();
                            let subst = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
                            let identity = self.push_fn(FnItem {
                                decl: f,
                                name: full,
                                module,
                                self_ty: None,
                                subst,
                                bounds: env,
                                symbolic: true,
                                check_body: true,
                                is_unsafe: f.is_unsafe,
                            });
                            let index = self.globals.generic_fns.len();
                            self.globals.modules[module].values.insert(name.clone(), Global::Generic(index));
                            self.globals.generic_fns.push(GenericFn {
                                name: self.globals.qualified(module, name),
                                module,
                                is_unsafe: f.is_unsafe,
                                params,
                                bounds,
                                decl: self.tpl.fns.len(),
                                identity,
                            });
                            self.generic_fn_bounds.push((index, f, module));
                            self.tpl.fns.push(f);
                        }
                        self.globals.modules[module].value_priv.insert(name.clone(), f.is_priv);
                    }
                    ast::Item::Const(c) => {
                        let name = &c.name.name;
                        if self.globals.modules[module].values.contains_key(name) || builtin(name).is_some() {
                            diags.error(Code::UNSUPPORTED, c.name.span, format!("`{name}` is defined more than once"));
                            continue;
                        }
                        self.globals.modules[module]
                            .values
                            .insert(name.clone(), Global::Const(ConstId(self.const_decls.len())));
                        self.globals.modules[module].value_priv.insert(name.clone(), c.is_priv);
                        self.globals.const_tys.push(Ty::Error);
                        self.const_decls.push(NamedSrc { decl: c, module });
                    }
                    ast::Item::Extern(block) => self.declare_extern(module, block, diags),
                    ast::Item::Struct(_)
                    | ast::Item::Enum(_)
                    | ast::Item::TypeAlias(_)
                    | ast::Item::Interface(_)
                    | ast::Item::Impl(_) => {}
                }
            }
        }
        if self.test_mode {
            self.declare_tests(diags);
        }
    }

    /// Declares the `test` blocks of the entry package as ordinary functions named by the parser
    /// (`__burdin_test_N`), each returning `Result<(), AssertError>`. Tests of dependency packages are not run, so
    /// they are not declared. A normal build ignores them entirely: they are stripped from the program. The
    /// attributes are validated here, before the bodies, so a bad attribute is reported even if the body is fine.
    fn declare_tests(&mut self, diags: &mut Diagnostics) {
        let root = self.globals.modules[self.entry].package;
        for module in 0..self.module_asts.len() {
            if self.globals.modules[module].package != root {
                continue;
            }
            let ast = self.module_asts[module];
            for test in &ast.tests {
                let name = test.func.name.name.clone();
                let attrs = check_test_attrs(test, diags);
                if self.globals.modules[module].values.contains_key(&name) || builtin(&name).is_some() {
                    diags.error(Code::UNSUPPORTED, test.name_span, format!("`{name}` is defined more than once"));
                    continue;
                }
                let full = self.globals.qualified(module, &name);
                let id = self.push_fn(FnItem::plain(&test.func, full, module, None));
                self.globals.modules[module].values.insert(name.clone(), Global::Func(id));
                self.globals.modules[module].value_priv.insert(name, true);
                self.tests.push(tir::TestEntry { name: test.name.clone(), func: id, attrs });
            }
        }
    }

    /// Registers the functions of an `extern "C"` block. A symbol is declared once: a second declaration with the
    /// same name and signature reuses it (and is what a second module binds its own name to).
    fn declare_extern(&mut self, module: usize, block: &'t ast::ExternBlock, diags: &mut Diagnostics) {
        if block.abi != "C" {
            diags.error(
                Code::UNSUPPORTED,
                block.abi_span,
                format!("the `{}` ABI is not implemented in this compiler yet; only \"C\" is", block.abi),
            );
        }
        for decl in &block.decls {
            let symbol = decl.name.name.clone();
            let is_priv = block.is_priv || decl.is_priv;
            if self.globals.modules[module].values.contains_key(&symbol) || builtin(&symbol).is_some() {
                diags.error(Code::UNSUPPORTED, decl.name.span, format!("`{symbol}` is defined more than once"));
                continue;
            }
            if symbol == "main" || symbol.starts_with("burdin_rt_") || symbol.starts_with("burdin.") {
                diags.error(
                    Code::UNSUPPORTED,
                    decl.name.span,
                    format!("`{symbol}` is reserved by the runtime and cannot be declared as an `extern` function"),
                );
                continue;
            }
            if !decl.generics.is_empty() || !decl.where_clause.is_empty() {
                diags.error(Code::UNSUPPORTED, decl.name.span, "an `extern` function cannot be generic");
                continue;
            }
            let id = match self.globals.extern_symbols.get(&symbol).copied() {
                Some(id) => id,
                None => {
                    let id = self.globals.externs.len();
                    self.globals.externs.push(ExternInfo { symbol: symbol.clone(), sig: Signature::placeholder() });
                    self.globals.extern_symbols.insert(symbol.clone(), id);
                    id
                }
            };
            // Every declaration is resolved, so a second one with a different signature is reported instead of
            // silently linking to the wrong ABI.
            self.extern_srcs.push(ExternSrc { decl, module, extern_id: id });
            self.globals.modules[module].values.insert(symbol.clone(), Global::Extern(id));
            self.globals.modules[module].value_priv.insert(symbol, is_priv);
        }
    }

    /// The identity instance of every generic type (its arguments are its own parameters) is what struct literals
    /// and variant constructors are matched against, and its methods are checked against the declared bounds.
    /// Default methods of interfaces are checked once with `Self` as a type parameter.
    pub(crate) fn declare_identities(&mut self, diags: &mut Diagnostics) {
        for g in 0..self.globals.generic_adts.len() {
            let args: Vec<Ty> = self.globals.generic_adts[g].params.iter().map(|p| param_ty(p)).collect();
            self.allocate_adt(g, args.clone(), diags);
            self.globals.generic_adts[g].identity = self.globals.adt_instances[&(g, args)];
        }
        // Default methods of interfaces are checked once with `Self` as a type parameter.
        for (i, decl) in self.tpl.ifaces.clone().into_iter().enumerate() {
            let module = self.globals.interfaces[i].module;
            let iface_name = self.globals.interfaces[i].name.clone();
            for m in decl.methods.iter().filter(|m| m.has_body) {
                let item = FnItem {
                    decl: &m.decl,
                    name: format!("{iface_name}.{}", m.decl.name.name),
                    module,
                    self_ty: Some(param_ty("Self")),
                    subst: Vec::new(),
                    bounds: vec![("Self".to_string(), vec![Bound::User(i)])],
                    symbolic: true,
                    check_body: true,
                    is_unsafe: m.decl.is_unsafe,
                };
                self.push_fn(item);
            }
        }
    }
}

/// Validates the attributes of a `test` item and returns the runner-visible form. The accepted attributes are
/// `ignore`, `timeout(ms)`, `should_panic`, `should_panic("text")`, and `should_fail` (spec/test-runner.md). An
/// unknown name, a repeated attribute, a value an attribute does not take, or `should_panic` together with
/// `should_fail` is E0170. A bad attribute never removes the test from the program, so one report does not turn
/// into follow-on diagnostics.
fn check_test_attrs(test: &ast::TestDecl, diags: &mut Diagnostics) -> tir::TestAttrs {
    let mut attrs = tir::TestAttrs::default();
    let mut seen: Vec<&str> = Vec::new();
    let mut should_fail_span = None;
    let mut should_panic_span = None;
    for attr in &test.attrs {
        let name = attr.name.name.as_str();
        if seen.contains(&name) {
            diags.error(Code::TEST_ATTRIBUTE, attr.name.span, format!("the test attribute `{name}` is repeated"));
            continue;
        }
        seen.push(name);
        let error = |diags: &mut Diagnostics, message: String| {
            diags.error(Code::TEST_ATTRIBUTE, attr.name.span, message);
        };
        match name {
            "ignore" => match attr.value {
                None => attrs.ignore = true,
                Some(_) => error(diags, "`ignore` takes no value; write `ignore`".to_string()),
            },
            "should_fail" => {
                should_fail_span = Some(attr.name.span);
                match attr.value {
                    None => attrs.should_fail = true,
                    Some(_) => error(diags, "`should_fail` takes no value; write `should_fail`".to_string()),
                }
            }
            "timeout" => match &attr.value {
                Some(ast::TestAttrValue::Int(ms)) if *ms > u64::MAX as u128 => error(
                    diags,
                    "the `timeout` value is too large; it is a number of milliseconds that must fit in 64 bits"
                        .to_string(),
                ),
                Some(ast::TestAttrValue::Int(ms)) => attrs.timeout_ms = Some(*ms as u64),
                Some(ast::TestAttrValue::Str(_)) => {
                    error(diags, "`timeout` takes a number of milliseconds, as in `timeout(500)`".to_string())
                }
                None => error(diags, "`timeout` needs a number of milliseconds, as in `timeout(500)`".to_string()),
            },
            "should_panic" => {
                should_panic_span = Some(attr.name.span);
                match &attr.value {
                    None => attrs.should_panic = true,
                    Some(ast::TestAttrValue::Str(text)) => {
                        attrs.should_panic = true;
                        attrs.should_panic_message = Some(text.clone());
                    }
                    Some(ast::TestAttrValue::Int(_)) => error(
                        diags,
                        "`should_panic` takes an optional message string, as in `should_panic(\"boom\")`".to_string(),
                    ),
                }
            }
            _ => error(
                diags,
                format!(
                    "unknown test attribute `{name}`; expected `ignore`, `timeout`, `should_panic`, or `should_fail`"
                ),
            ),
        }
    }
    if attrs.should_panic && attrs.should_fail {
        let span = match (should_panic_span, should_fail_span) {
            (Some(p), Some(f)) if p.lo > f.lo => p,
            (_, Some(f)) => f,
            (Some(p), None) => p,
            (None, None) => test.name_span,
        };
        diags.error(Code::TEST_ATTRIBUTE, span, "`should_panic` and `should_fail` cannot be combined");
    }
    attrs
}

/// The bounds of each type parameter of a function: from the parameter list and from `where` clauses.
pub(crate) fn fn_bounds(f: &ast::FnDecl, globals: &Globals, module: usize, diags: &mut Diagnostics) -> Vec<Vec<Bound>> {
    let mut all: Vec<Vec<Bound>> = f
        .generics
        .iter()
        .map(|p| p.bounds.iter().filter_map(|b| globals.resolve_bound(b, module, diags)).collect())
        .collect();
    for pred in &f.where_clause {
        let index = match &pred.ty.kind {
            ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => {
                f.generics.iter().position(|g| g.name.name == segs[0].name.name)
            }
            _ => None,
        };
        match index {
            Some(i) => {
                let extra: Vec<Bound> =
                    pred.bounds.iter().filter_map(|b| globals.resolve_bound(b, module, diags)).collect();
                all[i].extend(extra);
            }
            None => {
                diags.error(
                    Code::UNSUPPORTED,
                    pred.ty.span,
                    "a `where` clause on something other than a type parameter is not implemented in this compiler yet",
                );
            }
        }
    }
    all
}
