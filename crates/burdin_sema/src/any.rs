//! `any Interface`: the interface instances, their object safety, the vtable layouts, and type identities
//! (spec/existentials.md). The checker builds these while it resolves types and checks bodies; `Sema::finish`
//! copies them into the program for code generation.

use crate::generics::Bound;
use crate::globals::{Globals, adt_key};
use burdin_diag::{Code, Diagnostics, Span};
use burdin_tir::{AnyIface, AnyMethod, AnyRef, AnyVTable, Signature, Ty};
use std::collections::HashMap;
use std::rc::Rc;

/// The existential state of one compilation. Interface instances are interned by `(interface, arguments)`;
/// their method lists are computed lazily, because the first `any` in a struct field is resolved before the
/// interfaces' own signatures are (lazy computation keeps the order of `resolve_all` irrelevant).
#[derive(Default)]
pub(crate) struct AnyState {
    /// The interface and its arguments of each instance, by [`AnyRef::id`].
    pub(crate) ifaces: Vec<(usize, Vec<Ty>)>,
    pub(crate) iface_index: HashMap<(usize, Vec<Ty>), usize>,
    /// The display name of each instance, by id.
    pub(crate) names: Vec<String>,
    /// The vtable method list of each instance, computed on first use.
    pub(crate) methods: Vec<Option<Vec<AnyMethod>>>,
    /// Every `any Interface` written in a type, with the span it was written at, for E0202.
    pub(crate) uses: Vec<(usize, Span)>,
    /// Types with a runtime identity: boxed concrete types and downcast targets.
    pub(crate) type_ids: Vec<Ty>,
    pub(crate) type_id_index: HashMap<Ty, usize>,
    /// One vtable per `(concrete type, interface instance)`.
    pub(crate) vtables: Vec<AnyVTable>,
    pub(crate) vtable_index: HashMap<(Ty, usize), usize>,
}

impl Globals {
    /// The interface instance for `(iface, args)`, creating it on first use. Object safety is checked by
    /// [`Globals::check_any_uses`], not here, so this is also safe to call while substituting a signature.
    pub(crate) fn any_iface(&self, iface: usize, args: Vec<Ty>) -> AnyRef {
        let key = (iface, args.clone());
        let mut state = self.any.borrow_mut();
        let id = match state.iface_index.get(&key) {
            Some(&id) => id,
            None => {
                let id = state.ifaces.len();
                let name = self.any_iface_name(iface, &args);
                state.iface_index.insert(key, id);
                state.ifaces.push((iface, args.clone()));
                state.names.push(name);
                state.methods.push(None);
                id
            }
        };
        let name = state.names[id].clone();
        AnyRef { id, name: Rc::from(name), iface, args }
    }

    /// `Shape`, or `Store<i32>` for a generic interface.
    fn any_iface_name(&self, iface: usize, args: &[Ty]) -> String {
        let mut name = self.interfaces[iface].name.clone();
        if !args.is_empty() {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            name.push('<');
            name.push_str(&args.join(", "));
            name.push('>');
        }
        name
    }

    /// The vtable slots of an interface instance: its own methods, then the methods of every interface it
    /// requires (transitively), deduplicated by name (AN2). `Self` is the boxed value itself.
    pub(crate) fn any_methods(&self, id: usize) -> Vec<AnyMethod> {
        if let Some(Some(methods)) = self.any.borrow().methods.get(id) {
            return methods.clone();
        }
        let (iface, args) = match self.any.borrow().ifaces.get(id) {
            Some(found) => found.clone(),
            None => return Vec::new(),
        };
        let name = self.any.borrow().names[id].clone();
        let anyref = AnyRef { id, name: Rc::from(name), iface, args: args.clone() };
        let mut methods = Vec::new();
        let mut seen = Vec::new();
        self.collect_any_methods(iface, &args, &anyref, &mut methods, &mut seen);
        self.any.borrow_mut().methods[id] = Some(methods.clone());
        methods
    }

    fn collect_any_methods(
        &self,
        iface: usize,
        args: &[Ty],
        anyref: &AnyRef,
        out: &mut Vec<AnyMethod>,
        seen: &mut Vec<usize>,
    ) {
        if seen.contains(&iface) {
            return;
        }
        seen.push(iface);
        let info = &self.interfaces[iface];
        let mut subst: Vec<(String, Ty)> = vec![("Self".to_string(), Ty::Any(anyref.clone()))];
        subst.extend(info.params.iter().cloned().zip(args.iter().cloned()));
        for m in &info.methods {
            if out.iter().any(|have| have.name == m.name) {
                continue;
            }
            let sig = Signature {
                params: m.sig.params.iter().map(|p| self.subst_param(p, &subst)).collect(),
                ret: self.subst_ty(&m.sig.ret, &subst),
                has_self: m.sig.has_self,
                ret_origins: Vec::new(),
            };
            out.push(AnyMethod { name: m.name.clone(), sig, is_unsafe: m.is_unsafe });
        }
        // A required interface's arguments are written with the outer interface's type parameters.
        let requires = info.requires.clone();
        for bound in requires {
            if let Bound::User(j, rargs) = bound {
                let rargs: Vec<Ty> = rargs.iter().map(|a| self.subst_ty(a, &subst)).collect();
                self.collect_any_methods(j, &rargs, anyref, out, seen);
            }
        }
    }

    /// Why `any I` is not allowed, or `None` when the interface is object-safe (AN2).
    pub(crate) fn any_not_object_safe(&self, iface: usize, args: &[Ty]) -> Option<String> {
        self.any_not_object_safe_in(iface, args, &mut Vec::new())
    }

    fn any_not_object_safe_in(&self, iface: usize, args: &[Ty], seen: &mut Vec<usize>) -> Option<String> {
        if seen.contains(&iface) {
            return None;
        }
        seen.push(iface);
        let info = &self.interfaces[iface];
        let name = self.any_iface_name(iface, args);
        // A vtable is a fixed set of function slots; it cannot answer which type an associated type is for a
        // concrete value (`spec/associated-types.md`, AT5).
        if let Some(first) = info.assoc.first() {
            return Some(format!(
                "interface `{name}` has the associated type `{first}`, so it cannot be used as `any`"
            ));
        }
        let subst: Vec<(String, Ty)> = info.params.iter().cloned().zip(args.iter().cloned()).collect();
        for m in &info.methods {
            if m.generic {
                return Some(format!(
                    "interface `{name}` has the generic method `{}`, so it cannot be used as `any`",
                    m.name
                ));
            }
            if !m.sig.has_self {
                return Some(format!(
                    "interface `{name}` has the method `{}` without a `self` receiver, so it cannot be used as `any`",
                    m.name
                ));
            }
            if m.sig.params.first().is_some_and(|p| p.own) {
                return Some(format!(
                    "the method `{}` of interface `{name}` takes `own self`, so the interface cannot be used as `any`",
                    m.name
                ));
            }
            let mentions_self =
                m.sig.params.iter().skip(1).any(|p| ty_mentions_self(&p.ty)) || ty_mentions_self(&m.sig.ret);
            if mentions_self {
                return Some(format!(
                    "the method `{}` of interface `{name}` mentions `Self` outside its receiver, so the interface \
                     cannot be used as `any`",
                    m.name
                ));
            }
            let ret = self.subst_ty(&m.sig.ret, &subst);
            if ret.is_second_class() {
                return Some(format!(
                    "the method `{}` of interface `{name}` returns `{}`, a view, so the interface cannot be used \
                     as `any`",
                    m.name, m.sig.ret
                ));
            }
        }
        let requires = info.requires.clone();
        for bound in requires {
            if let Bound::User(j, rargs) = bound {
                let rargs: Vec<Ty> = rargs.iter().map(|a| self.subst_ty(a, &subst)).collect();
                if let Some(reason) = self.any_not_object_safe_in(j, &rargs, seen) {
                    return Some(reason);
                }
            }
        }
        None
    }

    /// Records that `any I` was written at `span`, for the deferred object-safety check.
    pub(crate) fn record_any_use(&self, any: &AnyRef, span: Span) {
        let mut state = self.any.borrow_mut();
        let entry = (any.id, span);
        if !state.uses.contains(&entry) {
            state.uses.push(entry);
        }
    }

    /// Reports E0202 for every recorded `any Interface` whose interface is not object-safe. It runs after all
    /// interface signatures are resolved, so the reasons are the final ones.
    pub(crate) fn check_any_uses(&self, diags: &mut Diagnostics) {
        let uses = self.any.borrow().uses.clone();
        for (id, span) in uses {
            let Some((iface, args)) = self.any.borrow().ifaces.get(id).cloned() else { continue };
            if let Some(reason) = self.any_not_object_safe(iface, &args) {
                diags.error(Code::NOT_OBJECT_SAFE, span, reason);
            }
        }
    }

    /// The runtime identity of `ty`: one integer per monomorphized type (AN6).
    pub(crate) fn any_type_id(&self, ty: &Ty) -> usize {
        let mut state = self.any.borrow_mut();
        if let Some(&id) = state.type_id_index.get(ty) {
            return id;
        }
        let id = state.type_ids.len();
        state.type_ids.push(ty.clone());
        state.type_id_index.insert(ty.clone(), id);
        id
    }

    /// The vtable index for boxing `concrete` as `iface`, creating it on first use. The concrete type must have a
    /// callable implementation of every slot; a compiler-provided implementation (Display for a primitive) has no
    /// function to dispatch to and is E0204 (AN3). An interface with no methods needs no slots at all, so any type
    /// that satisfies it can be boxed (a marker such as `Iterable<i32>`).
    pub(crate) fn any_vtable(
        &self,
        concrete: &Ty,
        iface: &AnyRef,
        span: Span,
        diags: &mut Diagnostics,
    ) -> Option<usize> {
        let key = (concrete.clone(), iface.id);
        if let Some(&id) = self.any.borrow().vtable_index.get(&key) {
            return Some(id);
        }
        let methods = self.any_methods(iface.id);
        let mut entries = Vec::new();
        if !methods.is_empty() {
            let Some(key_adt) = adt_key(concrete) else {
                diags.error(
                    Code::BUILT_IN_IMPL,
                    span,
                    format!(
                        "`{concrete}` implements `{}` in the compiler itself, so it has no function to dispatch \
                         to through `any {}`",
                        iface.name, iface.name
                    ),
                );
                return None;
            };
            for method in &methods {
                match self.methods.get(&(key_adt, method.name.clone())) {
                    Some(&f) => entries.push(f),
                    None => {
                        diags.error(
                            Code::BUILT_IN_IMPL,
                            span,
                            format!(
                                "`{concrete}` implements `{}` in the compiler itself, so it has no function to \
                                 dispatch to through `any {}`",
                                iface.name, iface.name
                            ),
                        );
                        return None;
                    }
                }
            }
        }
        let type_id = self.any_type_id(concrete);
        let mut state = self.any.borrow_mut();
        // Another boxing site may have created it while the methods were looked up.
        if let Some(&id) = state.vtable_index.get(&key) {
            return Some(id);
        }
        let id = state.vtables.len();
        state.vtables.push(AnyVTable { concrete: concrete.clone(), type_id, iface: iface.id, entries });
        state.vtable_index.insert(key, id);
        Some(id)
    }

    /// The interface instances, with their method lists, for the program.
    pub(crate) fn any_program_ifaces(&self) -> (Vec<AnyIface>, Vec<AnyVTable>, Vec<Ty>) {
        let state = self.any.borrow();
        let ifaces: Vec<AnyIface> =
            (0..state.ifaces.len()).map(|id| AnyIface { name: state.names[id].clone(), methods: Vec::new() }).collect();
        drop(state);
        let ifaces: Vec<AnyIface> = ifaces
            .into_iter()
            .enumerate()
            .map(|(id, mut iface)| {
                iface.methods = self.any_methods(id);
                iface
            })
            .collect();
        let state = self.any.borrow();
        (ifaces, state.vtables.clone(), state.type_ids.clone())
    }
}

/// Whether a written type names `Self` anywhere.
fn ty_mentions_self(ty: &Ty) -> bool {
    match ty {
        Ty::Param(name) => &**name == "Self",
        Ty::Ref { inner, .. }
        | Ty::Slice(inner)
        | Ty::Array(inner, _)
        | Ty::Option(inner)
        | Ty::Result(inner, _)
        | Ty::List(inner, _)
        | Ty::Heap(inner, _)
        | Ty::CPtr(inner)
        | Ty::Range(inner, _) => ty_mentions_self(inner),
        Ty::Map(k, v, _) | Ty::MapEntry(k, v, _) => ty_mentions_self(k) || ty_mentions_self(v),
        Ty::Any(a) => a.args.iter().any(ty_mentions_self),
        Ty::Projection { self_ty, args, .. } => ty_mentions_self(self_ty) || args.iter().any(ty_mentions_self),
        Ty::Fn { sig, .. } => sig.params.iter().any(ty_mentions_self) || ty_mentions_self(&sig.ret),
        Ty::Tuple(ts) => ts.iter().any(ty_mentions_self),
        _ => false,
    }
}
