//! The declaration phases: collect the names of types, interfaces, functions, and constants, then everything that
//! needs those names (interface requirements, bounds, impl blocks, the identity instances of generic types).

use crate::builtins::{builtin, builtin_type};
use crate::generics::{BUILTIN_INTERFACES, Bound, GenericAdt, GenericFn, IfaceMethod, InterfaceInfo, param_ty};
use crate::globals::{Global, Globals, TypeName};
use crate::session::{FnItem, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AdtRef, ConstId, Signature, Ty};
use std::rc::Rc;

impl<'t> Sema<'t> {
    /// Type and interface names first, so that every signature and field can refer to every type.
    pub(crate) fn declare_types(&mut self, file: &'t ast::File, diags: &mut Diagnostics) {
        for item in &file.items {
            if let ast::Item::Interface(i) = item {
                if !i.generics.is_empty() {
                    diags.error(
                        Code::UNSUPPORTED,
                        i.name.span,
                        "generic interfaces are not implemented in this compiler yet",
                    );
                }
                let name = &i.name.name;
                if self.globals.iface_names.contains_key(name)
                    || self.globals.types.contains_key(name)
                    || builtin_type(name).is_some()
                    || BUILTIN_INTERFACES.contains(&name.as_str())
                {
                    diags.error(Code::UNSUPPORTED, i.name.span, format!("`{name}` is defined more than once"));
                    continue;
                }
                self.globals.iface_names.insert(name.clone(), self.tpl.ifaces.len());
                self.globals.interfaces.push(InterfaceInfo {
                    name: name.clone(),
                    requires: Vec::new(),
                    methods: Vec::new(),
                });
                self.tpl.ifaces.push(i);
                continue;
            }
            let (name, generics) = match item {
                ast::Item::Struct(s) => (&s.name, &s.generics),
                ast::Item::Enum(e) => (&e.name, &e.generics),
                ast::Item::TypeAlias(a) => (&a.name, &a.generics),
                _ => continue,
            };
            if matches!(item, ast::Item::TypeAlias(_)) && !generics.is_empty() {
                diags.error(
                    Code::UNSUPPORTED,
                    name.span,
                    "generic type aliases are not implemented in this compiler yet",
                );
            }
            if self.globals.types.contains_key(&name.name)
                || builtin_type(&name.name).is_some()
                || self.globals.iface_names.contains_key(&name.name)
            {
                diags.error(Code::UNSUPPORTED, name.span, format!("type `{}` is defined more than once", name.name));
                continue;
            }
            let adt = |id| AdtRef { id, name: Rc::from(name.name.as_str()) };
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
            match item {
                ast::Item::Struct(s) if generics.is_empty() => {
                    let id = self.globals.structs.len();
                    self.globals.types.insert(name.name.clone(), TypeName::Struct(id));
                    self.globals.structs.push(tir::StructDef {
                        name: name.name.clone(),
                        fields: Vec::new(),
                        derives: tir::Derives::default(),
                        drop_method: None,
                    });
                    self.globals.struct_refs.push(adt(id));
                    self.src.structs.push((s, Vec::new()));
                }
                ast::Item::Enum(e) if generics.is_empty() => {
                    let id = self.globals.enums.len();
                    self.globals.types.insert(name.name.clone(), TypeName::Enum(id));
                    self.globals.enums.push(tir::EnumDef {
                        name: name.name.clone(),
                        variants: Vec::new(),
                        derives: tir::Derives::default(),
                        drop_method: None,
                    });
                    self.globals.enum_refs.push(adt(id));
                    self.src.enums.push((e, Vec::new()));
                }
                ast::Item::Struct(s) => {
                    let g = self.globals.generic_adts.len();
                    self.globals.types.insert(name.name.clone(), TypeName::GenericAdt(g));
                    self.globals.generic_adts.push(GenericAdt {
                        name: name.name.clone(),
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
                    self.globals.types.insert(name.name.clone(), TypeName::GenericAdt(g));
                    self.globals.generic_adts.push(GenericAdt {
                        name: name.name.clone(),
                        is_enum: true,
                        params,
                        bounds: Vec::new(),
                        decl: self.tpl.enums.len(),
                        identity: (false, 0),
                    });
                    self.tpl.enums.push(e);
                }
                ast::Item::TypeAlias(a) => {
                    self.globals.types.insert(name.name.clone(), TypeName::Alias(self.alias_decls.len()));
                    self.globals.aliases.push(Ty::Error);
                    self.alias_decls.push(a);
                }
                _ => unreachable!(),
            }
        }
    }

    /// What interfaces require, and the bounds of generic types.
    pub(crate) fn declare_bounds(&mut self, diags: &mut Diagnostics) {
        for i in 0..self.tpl.ifaces.len() {
            let decl = self.tpl.ifaces[i];
            let requires = decl.requires.iter().filter_map(|t| self.globals.resolve_bound(t, diags)).collect();
            let methods = decl
                .methods
                .iter()
                .map(|m| IfaceMethod {
                    name: m.decl.name.name.clone(),
                    sig: Signature::placeholder(),
                    has_default: m.has_body,
                })
                .collect();
            self.globals.interfaces[i].requires = requires;
            self.globals.interfaces[i].methods = methods;
        }
        for g in 0..self.globals.generic_adts.len() {
            let info = &self.globals.generic_adts[g];
            let generics =
                if info.is_enum { &self.tpl.enums[info.decl].generics } else { &self.tpl.structs[info.decl].generics };
            let bounds = generics
                .iter()
                .map(|p| p.bounds.iter().filter_map(|b| self.globals.resolve_bound(b, diags)).collect())
                .collect();
            self.globals.generic_adts[g].bounds = bounds;
        }
    }

    /// Functions, methods, constants, and impl blocks.
    pub(crate) fn declare_values(&mut self, file: &'t ast::File, diags: &mut Diagnostics) {
        for item in &file.items {
            match item {
                ast::Item::Fn(f) => {
                    if self.globals.names.contains_key(&f.name.name) || builtin(&f.name.name).is_some() {
                        diags.error(
                            Code::UNSUPPORTED,
                            f.name.span,
                            format!("`{}` is defined more than once", f.name.name),
                        );
                        continue;
                    }
                    if f.generics.is_empty() && f.where_clause.is_empty() {
                        let id = self.push_fn(FnItem::plain(f, f.name.name.clone(), None));
                        self.globals.names.insert(f.name.name.clone(), Global::Func(id));
                    } else {
                        let params: Vec<String> = f.generics.iter().map(|g| g.name.name.clone()).collect();
                        let bounds = fn_bounds(f, &self.globals, diags);
                        let env = params.iter().cloned().zip(bounds.iter().cloned()).collect();
                        let subst = params.iter().map(|p| (p.clone(), param_ty(p))).collect();
                        let identity = self.push_fn(FnItem {
                            decl: f,
                            name: f.name.name.clone(),
                            self_ty: None,
                            subst,
                            bounds: env,
                            symbolic: true,
                            check_body: true,
                        });
                        self.globals.names.insert(f.name.name.clone(), Global::Generic(self.globals.generic_fns.len()));
                        self.globals.generic_fns.push(GenericFn {
                            name: f.name.name.clone(),
                            params,
                            bounds,
                            decl: self.tpl.fns.len(),
                            identity,
                        });
                        self.tpl.fns.push(f);
                    }
                }
                ast::Item::Const(c) => {
                    if self.globals.names.contains_key(&c.name.name) || builtin(&c.name.name).is_some() {
                        diags.error(
                            Code::UNSUPPORTED,
                            c.name.span,
                            format!("`{}` is defined more than once", c.name.name),
                        );
                        continue;
                    }
                    self.globals.names.insert(c.name.name.clone(), Global::Const(ConstId(self.const_decls.len())));
                    self.globals.const_tys.push(Ty::Error);
                    self.const_decls.push(c);
                }
                ast::Item::Impl(imp) => self.register_impl(imp, diags),
                ast::Item::Struct(_) | ast::Item::Enum(_) | ast::Item::TypeAlias(_) | ast::Item::Interface(_) => {}
            }
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
            for m in decl.methods.iter().filter(|m| m.has_body) {
                let item = FnItem {
                    decl: &m.decl,
                    name: format!("{}.{}", decl.name.name, m.decl.name.name),
                    self_ty: Some(param_ty("Self")),
                    subst: Vec::new(),
                    bounds: vec![("Self".to_string(), vec![Bound::User(i)])],
                    symbolic: true,
                    check_body: true,
                };
                self.push_fn(item);
            }
        }
    }
}

/// The bounds of each type parameter of a function: from the parameter list and from `where` clauses.
pub(crate) fn fn_bounds(f: &ast::FnDecl, globals: &Globals, diags: &mut Diagnostics) -> Vec<Vec<Bound>> {
    let mut all: Vec<Vec<Bound>> =
        f.generics.iter().map(|p| p.bounds.iter().filter_map(|b| globals.resolve_bound(b, diags)).collect()).collect();
    for pred in &f.where_clause {
        let index = match &pred.ty.kind {
            ast::TypeKind::Path(segs) if segs.len() == 1 && segs[0].args.is_empty() => {
                f.generics.iter().position(|g| g.name.name == segs[0].name.name)
            }
            _ => None,
        };
        match index {
            Some(i) => {
                let extra: Vec<Bound> = pred.bounds.iter().filter_map(|b| globals.resolve_bound(b, diags)).collect();
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
