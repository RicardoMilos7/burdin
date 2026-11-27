//! Functions that return `impl Interface` (spec/existentials.md, AN4).
//!
//! The concrete type a body returns is one type known at compile time, but a caller may use it only through the
//! interface. The return type is therefore a wrapper struct with one field, the concrete value, and no name a caller
//! can write. Its methods are the interface's: a caller's call goes to the concrete type's method, the wrapper only
//! lets it through (`fncx/opaque.rs`), and the wrapper satisfies the interface and nothing else.
//!
//! The concrete type is not in the signature, so it is found before the bodies are checked: each body is checked
//! once with no declared result, and the type its returns and tail agree on becomes the wrapper's field. A body
//! whose answer depends on another such function waits until that function's type is known, so the pass repeats
//! until nothing more is resolved.

use crate::generics::Bound;
use crate::globals::{Globals, OpaqueRet};
use crate::session::{AdtSrcEntry, Sema};
use burdin_diag::{Code, Diagnostics};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AdtRef, Ty};
use std::rc::Rc;

/// The `impl I` a declaration writes as its return type, if it writes one.
pub(crate) fn impl_return(decl: &ast::FnDecl) -> Option<&ast::Type> {
    decl.ret.as_ref().filter(|t| matches!(t.kind, ast::TypeKind::Impl(_)))
}

/// The return type of the function at `fn_index`, which is declared `-> impl I`: its wrapper struct, made the first
/// time the function is resolved. Every pass resolves the interface again, so its errors are reported by the pass
/// that is kept.
pub(crate) fn return_type<'t>(
    globals: &mut Globals,
    structs: &mut Vec<Option<AdtSrcEntry<'t, ast::StructDecl>>>,
    fn_index: usize,
    ty: &ast::Type,
    subst: &[(String, Ty)],
    module: usize,
    diags: &mut Diagnostics,
) -> Ty {
    let ast::TypeKind::Impl(inner) = &ty.kind else { unreachable!("an opaque return is an `impl` type") };
    let bound = match globals.resolve_bound(inner, subst, module, diags) {
        Some(bound @ Bound::User(..)) => bound,
        Some(_) => {
            diags.error(
                Code::NOT_AN_INTERFACE,
                ty.span,
                "`impl Interface` needs an interface; a built-in bound such as `Copy` cannot be returned as `impl`",
            );
            return Ty::Error;
        }
        None => return Ty::Error,
    };
    if let Some(o) = globals.opaques.iter().find(|o| o.fn_index == fn_index) {
        return Ty::Struct(globals.struct_refs[o.struct_id].clone());
    }
    let Bound::User(iface, _) = bound else { unreachable!("matched above") };
    let name = format!("impl {}", globals.interfaces[iface].name);
    let id = globals.structs.len();
    globals.structs.push(tir::StructDef {
        name: name.clone(),
        fields: Vec::new(),
        repr_c: false,
        derives: tir::Derives::default(),
        drop_method: None,
        lang: None,
    });
    let r = AdtRef { id, name: Rc::from(name.as_str()), second_class: false };
    globals.struct_refs.push(r.clone());
    let package = globals.package_of(module);
    globals.struct_pkg.push(package);
    globals.adt_priv.insert((false, id), false);
    // A wrapper has no declaration to resolve its fields from: `resolve_all` skips it, and the field is set once the
    // concrete type is known.
    structs.push(None);
    globals.opaques.push(OpaqueRet { fn_index, bound, span: ty.span, struct_id: id, concrete: None });
    Ty::Struct(r)
}

impl Globals {
    /// The opaque return whose wrapper struct is `id`.
    pub(crate) fn opaque_for_struct(&self, id: usize) -> Option<&OpaqueRet> {
        self.opaques.iter().find(|o| o.struct_id == id)
    }

    /// The opaque return a value of `ty` has, when `ty` is one of the wrappers.
    pub(crate) fn opaque_of(&self, ty: &Ty) -> Option<&OpaqueRet> {
        match ty {
            Ty::Struct(a) => self.opaque_for_struct(a.id),
            _ => None,
        }
    }

    /// Whether `ty` is an `impl Interface` return whose concrete type is not known: its function's error is already
    /// reported, so a mismatch against it is not reported again.
    pub(crate) fn opaque_unresolved(&self, ty: &Ty) -> bool {
        self.opaque_of(ty).is_some_and(|o| matches!(o.concrete, None | Some(Ty::Error)))
    }
}

impl Sema<'_> {
    /// Finds the concrete type of every function returning `impl I`, so the bodies that call them can be checked.
    /// A body is inferred with scratch diagnostics; once it gives one type with no errors, that type is recorded
    /// and the rest of the pass sees it. Whatever is still unknown after no more progress is reported here, and what
    /// the concrete types must satisfy is checked for each resolved function.
    pub(crate) fn resolve_opaque_returns(&mut self, diags: &mut Diagnostics) {
        loop {
            let mut progress = false;
            for k in 0..self.globals.opaques.len() {
                if self.globals.opaques[k].concrete.is_some() {
                    continue;
                }
                let mut scratch = Diagnostics::default();
                let candidates = self.infer_opaque(k, &mut scratch);
                if scratch.is_empty()
                    && let Some(ty) = agreed(&candidates)
                    && ty != Ty::Error
                {
                    self.set_concrete(k, ty);
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        for k in 0..self.globals.opaques.len() {
            let span = self.globals.opaques[k].span;
            match self.globals.opaques[k].concrete.clone() {
                None => {
                    let mut scratch = Diagnostics::default();
                    let candidates = self.infer_opaque(k, &mut scratch);
                    // The body's own errors are reported by its check; the rest is about the return type itself.
                    if !scratch.is_empty() || candidates.contains(&Ty::Error) {
                        continue;
                    }
                    // A return value whose type is itself an unresolved `impl` return depends on this one (a cycle),
                    // and a body with no return value has no type to give.
                    let pending =
                        candidates.iter().any(|t| self.globals.opaque_of(t).is_some_and(|o| o.concrete.is_none()));
                    match candidates.first() {
                        Some(first) if !pending => {
                            if let Some(other) = candidates.iter().find(|t| *t != first) {
                                diags.error(
                                    Code::TYPE_MISMATCH,
                                    span,
                                    format!(
                                        "this `impl` function returns {} in one place and {} in another, which are different types",
                                        self.describe_return(first),
                                        self.describe_return(other)
                                    ),
                                );
                            }
                        }
                        _ => {
                            diags.error(
                                Code::OPAQUE_UNKNOWN,
                                span,
                                "the concrete type of this `impl` return cannot be found: its body returns a value whose type depends on this function, or never returns one",
                            );
                        }
                    }
                }
                Some(Ty::Error) => {}
                Some(concrete) => {
                    let name = self.globals.opaques[k].bound.clone();
                    let env = self.fn_items[self.globals.opaques[k].fn_index].bounds.clone();
                    if !self.globals.satisfies(&concrete, &name, &env) {
                        let Bound::User(iface, _) = name else { unreachable!("an opaque return names an interface") };
                        diags.error(
                            Code::BOUND_NOT_SATISFIED,
                            span,
                            format!(
                                "`{concrete}` does not implement `{}`, which this `impl {}` return promises",
                                self.globals.interfaces[iface].name, self.globals.interfaces[iface].name
                            ),
                        );
                    }
                }
            }
        }
    }

    /// The types the body of opaque `k` returns: its tail and every `return`. Empty when the function has no body to
    /// check.
    fn infer_opaque(&self, k: usize, diags: &mut Diagnostics) -> Vec<Ty> {
        let fi = self.globals.opaques[k].fn_index;
        let item = &self.fn_items[fi];
        if !item.check_body {
            return Vec::new();
        }
        crate::fncx::infer_return(&self.globals, item, &self.globals.sigs[fi], self.sources, self.traces, diags)
    }

    /// A return type for a diagnostic: an `impl` type names its concrete type, which is what tells two apart.
    fn describe_return(&self, ty: &Ty) -> String {
        match self.globals.opaque_of(ty) {
            Some(o) => match &o.concrete {
                Some(c) => format!("`{ty}` of `{c}`"),
                None => format!("`{ty}`"),
            },
            None => format!("`{ty}`"),
        }
    }

    /// Records the concrete type of opaque `k`: the wrapper's field, and whether the wrapper is `Copy`.
    fn set_concrete(&mut self, k: usize, ty: Ty) {
        let id = self.globals.opaques[k].struct_id;
        let span = self.globals.opaques[k].span;
        let copy = self.globals.is_copy(&ty);
        let s = &mut self.globals.structs[id];
        s.fields = vec![tir::FieldDef { name: "impl value".to_string(), ty: ty.clone(), span, is_priv: false }];
        s.derives.copy = copy;
        self.globals.opaques[k].concrete = Some(ty);
    }
}

/// The one type every candidate has, or None when there are none or they differ.
fn agreed(candidates: &[Ty]) -> Option<Ty> {
    let first = candidates.first()?;
    candidates.iter().all(|t| t == first).then(|| first.clone())
}
