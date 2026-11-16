//! Instances of generic types and functions: allocating the struct, enum, or function for one argument list.

use crate::generics::{Bound, Request, nowhere};
use crate::globals::Globals;
use crate::impls::MethodScope;
use crate::session::{FnItem, ImplTemplate, Sema};
use burdin_diag::{Code, Diagnostics, FileId};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AdtRef, Ty};
use std::rc::Rc;

/// The most functions and types a program may have, instances included. It stops a generic that instantiates
/// itself with ever larger types.
const MAX_ITEMS: usize = 4000;

impl<'t> Sema<'t> {
    /// Makes the struct or enum for a generic type and these arguments, and registers the methods of its impl blocks.
    pub(crate) fn allocate_adt(&mut self, template: usize, args: Vec<Ty>, diags: &mut Diagnostics) {
        let (gname, gmodule, is_enum, params, decl, adt_bounds, repr_c) = {
            let g = &self.globals.generic_adts[template];
            let repr_c = !g.is_enum && self.tpl.structs[g.decl].repr_c;
            (g.name.clone(), g.module, g.is_enum, g.params.clone(), g.decl, g.bounds.clone(), repr_c)
        };
        let symbolic = args.iter().any(|a| self.globals.ty_has_param(a));
        let identity = args.iter().zip(&params).all(|(a, p)| matches!(a, Ty::Param(n) if **n == **p));
        let name = Globals::instance_name(&gname, &args);
        let subst: Vec<(String, Ty)> = params.iter().cloned().zip(args.iter().cloned()).collect();
        let second_class = args.iter().any(Ty::is_second_class);
        let adt_ref = |id| AdtRef { id, name: Rc::from(name.as_str()), second_class };
        let (key, ty) = if is_enum {
            let id = self.globals.enums.len();
            self.globals.enums.push(tir::EnumDef {
                name: name.clone(),
                variants: Vec::new(),
                derives: tir::Derives::default(),
                drop_method: None,
            });
            let r = adt_ref(id);
            self.globals.enum_refs.push(r.clone());
            self.globals.enum_pkg.push(self.globals.package_of(gmodule));
            let private = self.globals.generic_adts[template].is_priv;
            self.globals.adt_priv.insert((true, id), private);
            self.src.enums.push(crate::session::AdtSrcEntry { decl: self.tpl.enums[decl], subst, module: gmodule });
            ((true, id), Ty::Enum(r))
        } else {
            let id = self.globals.structs.len();
            self.globals.structs.push(tir::StructDef {
                name: name.clone(),
                fields: Vec::new(),
                repr_c,
                derives: tir::Derives::default(),
                drop_method: None,
            });
            let r = adt_ref(id);
            self.globals.struct_refs.push(r.clone());
            self.globals.struct_pkg.push(self.globals.package_of(gmodule));
            let private = self.globals.generic_adts[template].is_priv;
            self.globals.adt_priv.insert((false, id), private);
            self.src.structs.push(crate::session::AdtSrcEntry { decl: self.tpl.structs[decl], subst, module: gmodule });
            ((false, id), Ty::Struct(r))
        };
        self.globals.adt_instances.insert((template, args.clone()), key);
        self.globals.adt_origin.insert(key, (template, args.clone()));
        let impls: Vec<ImplTemplate<'t>> = self.tpl.impls.iter().filter(|i| i.template == template).cloned().collect();
        for imp in &impls {
            // The block applies to the instances its arguments match (`spec/impls.md`, IM1).
            let Some(isubst) = self.globals.match_impl_args(&imp.params, &imp.args, &args) else { continue };
            let met = symbolic
                || imp.wheres.iter().all(|(n, bounds)| {
                    let arg = isubst.iter().find(|(m, _)| m == n).map(|(_, t)| t);
                    arg.is_none_or(|arg| bounds.iter().all(|b| self.globals.satisfies_with(arg, b, &[], &isubst)))
                });
            if !met {
                continue;
            }
            // A name the block gives a whole argument carries that parameter's declared bounds.
            let mut env: Vec<(String, Vec<Bound>)> = Vec::new();
            for (written, bounds) in imp.args.iter().zip(&adt_bounds) {
                if let Ty::Param(n) = written
                    && !env.iter().any(|(m, _)| m == &**n)
                {
                    env.push((n.to_string(), bounds.clone()));
                }
            }
            env.extend(imp.wheres.iter().cloned());
            // The instance an impl on specific arguments is declared for is the one that checks its bodies; the
            // type's identity instance checks the impls that cover every instance.
            let own = !crate::impl_match::is_plain_pattern(&imp.args) && imp.args == args;
            let check_body = !symbolic || identity || own;
            if imp.is_drop {
                let scope = MethodScope { subst: &isubst, bounds: &env, symbolic, check_body };
                self.register_generic_drop(imp.decl, imp.module, &ty, &scope, diags);
                continue;
            }
            let iface = imp.iface.map(|i| {
                let args: Vec<Ty> = imp.iface_args.iter().map(|a| self.globals.subst_ty(a, &isubst)).collect();
                (i, args)
            });
            let scope = MethodScope { subst: &isubst, bounds: &env, symbolic, check_body };
            self.register_methods(imp.decl, imp.module, iface, &ty, scope, diags);
        }
    }

    /// Makes the function for a generic function and these arguments.
    fn allocate_fn(&mut self, template: usize, args: Vec<Ty>) {
        let g = &self.globals.generic_fns[template];
        let subst: Vec<(String, Ty)> = g.params.iter().cloned().zip(args.iter().cloned()).collect();
        // A generic extension method is an ordinary function whose first parameter is `self`; its instance
        // takes the receiver type with the arguments substituted (`spec/extensions.md`, EX7).
        let self_ty = g.recv.as_ref().map(|recv| self.globals.subst_ty(recv, &subst));
        let item = FnItem {
            decl: self.tpl.fns[g.decl],
            name: Globals::instance_name(&g.name, &args),
            module: g.module,
            self_ty,
            subst,
            bounds: Vec::new(),
            symbolic: false,
            check_body: true,
            is_unsafe: g.is_unsafe,
            is_exported: false,
        };
        let id = self.push_fn(item);
        self.globals.fn_instances.insert((template, args), id);
    }

    /// Makes the function for one instantiation of a generic method.
    fn allocate_method(&mut self, template: usize, args: Vec<Ty>) {
        let t = &self.tpl.methods[template];
        let mut subst: Vec<(String, Ty)> = t.params.iter().cloned().zip(args.iter().cloned()).collect();
        subst.extend(t.base_subst.iter().cloned());
        let mut bounds: Vec<(String, Vec<Bound>)> = Vec::new();
        for (p, bs) in t.params.iter().zip(&t.bounds) {
            bounds.push((p.clone(), bs.clone()));
        }
        bounds.extend(t.base_bounds.iter().cloned());
        let symbolic = args.iter().any(|a| self.globals.ty_has_param(a))
            || t.base_subst.iter().any(|(_, ty)| self.globals.ty_has_param(ty))
            || t.self_ty.as_ref().is_some_and(|ty| self.globals.ty_has_param(ty));
        let name = Globals::instance_name(&t.name, &args);
        let item = FnItem {
            decl: t.decl,
            name,
            module: t.module,
            self_ty: t.self_ty.clone(),
            subst,
            bounds,
            symbolic,
            check_body: !symbolic,
            is_unsafe: t.is_unsafe,
            is_exported: false,
        };
        let id = self.push_fn(item);
        self.globals.method_instances.insert((template, args), id);
    }

    /// Creates the template and identity function of an interface default method for a built-in receiver type
    /// (a slice, `Chars`, a list iterator, ...), which has no impl block to register it.
    fn allocate_builtin_template(
        &mut self,
        iface: usize,
        method: usize,
        self_ty: Ty,
        iface_args: Vec<Ty>,
        diags: &mut Diagnostics,
    ) {
        let info = &self.globals.interfaces[iface];
        let name = info.name.clone();
        let module = info.module;
        let params = info.params.clone();
        let iface_bounds = info.bounds.clone();
        let decl: &'t ast::FnDecl = &self.tpl.ifaces[iface].methods[method].decl;
        let base_subst: Vec<(String, Ty)> = params.iter().cloned().zip(iface_args.iter().cloned()).collect();
        let mut base_bounds: Vec<(String, Vec<Bound>)> =
            vec![("Self".to_string(), vec![Bound::User(iface, iface_args.clone())])];
        for (p, bs) in params.iter().zip(&iface_bounds) {
            base_bounds.push((p.clone(), bs.iter().map(|b| self.globals.subst_bound(b, &base_subst)).collect()));
        }
        let method_params: Vec<String> = decl.generics.iter().map(|g| g.name.name.clone()).collect();
        let method_bounds = crate::declare::method_bounds(decl, &base_subst, &self.globals, module, diags);
        let mut subst: Vec<(String, Ty)> = Vec::new();
        for p in &method_params {
            subst.push((p.clone(), crate::generics::param_ty(p)));
        }
        subst.extend(base_subst.iter().cloned());
        let mut bounds: Vec<(String, Vec<Bound>)> = Vec::new();
        for (p, bs) in method_params.iter().zip(&method_bounds) {
            bounds.push((p.clone(), bs.clone()));
        }
        bounds.extend(base_bounds.iter().cloned());
        let item = FnItem {
            decl,
            name: format!("{name}.{}", decl.name.name),
            module,
            self_ty: Some(self_ty.clone()),
            subst,
            bounds,
            symbolic: true,
            check_body: true,
            is_unsafe: decl.is_unsafe,
            is_exported: false,
        };
        let identity = self.push_fn(item);
        let template = crate::session::MethodTemplate {
            decl,
            name: format!("{self_ty}.{}", decl.name.name),
            module,
            self_ty: Some(self_ty.clone()),
            base_subst,
            base_bounds,
            params: method_params,
            bounds: method_bounds,
            is_unsafe: decl.is_unsafe,
        };
        let index = self.tpl.methods.len();
        self.tpl.methods.push(template);
        self.globals.builtin_method_templates.insert(
            (iface, method, self_ty, iface_args),
            crate::generics::GenericMethod {
                template: index,
                identity,
                params: self.tpl.methods[index].params.clone(),
                bounds: self.tpl.methods[index].bounds.clone(),
            },
        );
    }

    /// Allocates what the last pass asked for. Returns whether anything was allocated, which means the pass has to
    /// be repeated.
    pub(crate) fn drain(&mut self, file: FileId, diags: &mut Diagnostics) -> bool {
        let requests = std::mem::take(&mut *self.globals.requests.borrow_mut());
        let mut any = false;
        for r in requests {
            if self.fn_items.len() + self.globals.structs.len() + self.globals.enums.len() > MAX_ITEMS {
                diags.error(
                    Code::UNSUPPORTED, // e9000: instances-limit
                    nowhere(file),
                    "too many generic instantiations; a generic type or function may instantiate itself without end",
                );
                return false;
            }
            match r {
                Request::Adt(template, args) => {
                    if !self.globals.adt_instances.contains_key(&(template, args.clone())) {
                        self.allocate_adt(template, args, diags);
                        any = true;
                    }
                }
                Request::Func(template, args) => {
                    if !self.globals.fn_instances.contains_key(&(template, args.clone())) {
                        self.allocate_fn(template, args);
                        any = true;
                    }
                }
                Request::Method(template, args) => {
                    if !self.globals.method_instances.contains_key(&(template, args.clone())) {
                        self.allocate_method(template, args);
                        any = true;
                    }
                }
                Request::BuiltinMethodTemplate { iface, method, self_ty, iface_args } => {
                    let key = (iface, method, self_ty.clone(), iface_args.clone());
                    if !self.globals.builtin_method_templates.contains_key(&key) {
                        self.allocate_builtin_template(iface, method, self_ty, iface_args, diags);
                        any = true;
                    }
                }
            }
        }
        any
    }
}
