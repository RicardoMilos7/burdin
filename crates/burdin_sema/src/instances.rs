//! Instances of generic types and functions: allocating the struct, enum, or function for one argument list.

use crate::generics::{Bound, Request, nowhere};
use crate::globals::Globals;
use crate::impls::MethodScope;
use crate::session::{FnItem, ImplTemplate, Sema};
use burdin_diag::{Code, Diagnostics, FileId};
use burdin_tir::{self as tir, AdtRef, Ty};
use std::rc::Rc;

/// The most functions and types a program may have, instances included. It stops a generic that instantiates
/// itself with ever larger types.
const MAX_ITEMS: usize = 4000;

impl<'t> Sema<'t> {
    /// Makes the struct or enum for a generic type and these arguments, and registers the methods of its impl blocks.
    pub(crate) fn allocate_adt(&mut self, template: usize, args: Vec<Ty>, diags: &mut Diagnostics) {
        let (gname, is_enum, params, decl, adt_bounds) = {
            let g = &self.globals.generic_adts[template];
            (g.name.clone(), g.is_enum, g.params.clone(), g.decl, g.bounds.clone())
        };
        let symbolic = args.iter().any(|a| self.globals.ty_has_param(a));
        let identity = args.iter().zip(&params).all(|(a, p)| matches!(a, Ty::Param(n) if **n == **p));
        let name = Globals::instance_name(&gname, &args);
        let subst: Vec<(String, Ty)> = params.iter().cloned().zip(args.iter().cloned()).collect();
        let adt_ref = |id| AdtRef { id, name: Rc::from(name.as_str()) };
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
            self.src.enums.push((self.tpl.enums[decl], subst));
            ((true, id), Ty::Enum(r))
        } else {
            let id = self.globals.structs.len();
            self.globals.structs.push(tir::StructDef {
                name: name.clone(),
                fields: Vec::new(),
                derives: tir::Derives::default(),
                drop_method: None,
            });
            let r = adt_ref(id);
            self.globals.struct_refs.push(r.clone());
            self.src.structs.push((self.tpl.structs[decl], subst));
            ((false, id), Ty::Struct(r))
        };
        self.globals.adt_instances.insert((template, args.clone()), key);
        self.globals.adt_origin.insert(key, (template, args.clone()));
        let impls: Vec<ImplTemplate<'t>> = self.tpl.impls.iter().filter(|i| i.template == template).cloned().collect();
        for imp in &impls {
            let isubst: Vec<(String, Ty)> = imp.params.iter().cloned().zip(args.iter().cloned()).collect();
            let met = symbolic
                || imp.wheres.iter().all(|(n, bounds)| {
                    let arg = isubst.iter().find(|(m, _)| m == n).map(|(_, t)| t);
                    arg.is_none_or(|arg| bounds.iter().all(|b| self.globals.satisfies(arg, *b, &[])))
                });
            if !met {
                continue;
            }
            let mut env: Vec<(String, Vec<Bound>)> =
                imp.params.iter().cloned().zip(adt_bounds.iter().cloned()).collect();
            env.extend(imp.wheres.iter().cloned());
            let scope = MethodScope { subst: &isubst, bounds: &env, symbolic, check_body: !symbolic || identity };
            self.register_methods(imp.decl, imp.iface, &ty, scope, diags);
        }
    }

    /// Makes the function for a generic function and these arguments.
    fn allocate_fn(&mut self, template: usize, args: Vec<Ty>) {
        let g = &self.globals.generic_fns[template];
        let item = FnItem {
            decl: self.tpl.fns[g.decl],
            name: Globals::instance_name(&g.name, &args),
            self_ty: None,
            subst: g.params.iter().cloned().zip(args.iter().cloned()).collect(),
            bounds: Vec::new(),
            symbolic: false,
            check_body: true,
        };
        let id = self.push_fn(item);
        self.globals.fn_instances.insert((template, args), id);
    }

    /// Allocates what the last pass asked for. Returns whether anything was allocated, which means the pass has to
    /// be repeated.
    pub(crate) fn drain(&mut self, file: FileId, diags: &mut Diagnostics) -> bool {
        let requests = std::mem::take(&mut *self.globals.requests.borrow_mut());
        let mut any = false;
        for r in requests {
            if self.fn_items.len() + self.globals.structs.len() + self.globals.enums.len() > MAX_ITEMS {
                diags.error(
                    Code::UNSUPPORTED,
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
            }
        }
        any
    }
}
