//! The `Send` and `Sync` markers (`spec/shared.md`, SH4-SH6).
//!
//! `Send` says a value may move to another task or thread; `Sync` says a shared view of it (`&T`) may be used by
//! several tasks at once. Both are interfaces declared in `std.marker` with no methods. A type has one in three ways:
//!
//! * it is built from parts that have it: the rules below, applied to the type's fields, elements, and type
//!   arguments (this is "derived");
//! * it declares it with an `unsafe` impl block, which replaces the derived answer: `unsafe Handle: Send { }`. The
//!   standard library uses this for `Channel<T>`, `Mutex<T>`, and the other types that hold a raw pointer but
//!   synchronize access to it;
//! * it has neither: a raw `CPtr<T>`, an `any I`, and every type that holds one.

use crate::generics::{Bound, Env, Head};
use crate::globals::{Globals, adt_key};
use burdin_diag::Code;
use burdin_tir::{Alloc, Ty};

impl Globals {
    /// Which marker `iface` is: `Some(false)` for `Send`, `Some(true)` for `Sync`.
    pub(crate) fn marker_kind(&self, iface: usize) -> Option<bool> {
        if Some(iface) == self.send_iface {
            Some(false)
        } else if Some(iface) == self.sync_iface {
            Some(true)
        } else {
            None
        }
    }

    /// Whether a struct or enum has an impl block of `iface` of its own, whatever its `where` clauses say.
    pub(crate) fn has_impl_head(&self, ty: &Ty, iface: usize) -> bool {
        let Some(key) = adt_key(ty) else { return false };
        self.impls.iter().any(|info| {
            info.iface == iface
                && match info.head {
                    Head::Plain(k) => k == key,
                    Head::Generic(template) => self.adt_origin.get(&key).is_some_and(|(t, _)| *t == template),
                }
        })
    }

    /// Why `ty` is not `Send` (or, with `sync`, not `Sync`), or `None` when it is. The text names the offending part
    /// the way the diagnostic shows it: the field, then what is wrong with its type.
    pub(crate) fn marker_violation(&self, ty: &Ty, sync: bool, env: &Env) -> Option<String> {
        self.marker_why(ty, sync, env, &mut Vec::new())
    }

    fn marker_why(&self, ty: &Ty, sync: bool, env: &Env, seen: &mut Vec<(Ty, bool)>) -> Option<String> {
        let iface = if sync { self.sync_iface } else { self.send_iface }?;
        let name = if sync { "Sync" } else { "Send" };
        match ty {
            Ty::Unit
            | Ty::Bool
            | Ty::Char
            | Ty::Int(_)
            | Ty::Float(_)
            | Ty::Str
            | Ty::String
            | Ty::CString
            | Ty::Chars
            | Ty::Never
            | Ty::Const(_)
            // A projection is opaque; the concrete binding is checked when it is substituted.
            | Ty::Projection { .. }
            | Ty::TryFromError { .. }
            | Ty::Error => None,
            Ty::Param(p) => {
                if self.bound_in_env(p, &Bound::User(iface, Vec::new()), env) {
                    None
                } else {
                    Some(format!("the type parameter `{p}` has no `{name}` bound"))
                }
            }
            Ty::CPtr(_) => Some(format!("a raw pointer is never `{name}`")),
            Ty::Any(_) => Some(format!("an `any` value may hold a type that is not `{name}`")),
            Ty::Fn { send, kind, .. } => {
                // A closure type does not list its captures. The closure expression decided both answers from the
                // captures it found. Only an `Fn` closure is `Sync`: `FnMut` and `FnOnce` change or consume what
                // they capture, so they cannot be shared.
                if sync {
                    if *kind != burdin_tir::FnKind::Fn {
                        Some(format!("a closure used as `{}` is not `Sync`; only an `Fn` closure is", kind.name()))
                    } else if send.sync_ok {
                        None
                    } else {
                        Some(send.sync_why.as_deref().map(str::to_string).unwrap_or_else(|| {
                            "a closure type written as an annotation does not say what its closure captures".to_string()
                        }))
                    }
                } else if send.ok {
                    None
                } else {
                    Some(send.why.as_deref().map(str::to_string).unwrap_or_else(|| {
                        "a closure type written as an annotation does not say what its closure captures".to_string()
                    }))
                }
            }
            // A shared view crosses to another task when the target may be used by several; an exclusive view when
            // the target may move.
            Ty::Ref { mutable, inner } => {
                let target = if sync { true } else { !*mutable };
                self.part_why("the referent", inner, target, env, seen)
            }
            // A slice stands for a view of either kind, so it needs both.
            Ty::Slice(t) => {
                if sync {
                    self.part_why("the element", t, true, env, seen)
                } else {
                    self.part_why("the element", t, false, env, seen)
                        .or_else(|| self.part_why("the element", t, true, env, seen))
                }
            }
            Ty::Array(t, _) | Ty::Option(t) | Ty::Range(t, _) | Ty::RangeIter(t, _) | Ty::ArrayIntoIter(t, _) => {
                self.part_why("the element", t, sync, env, seen)
            }
            Ty::List(t, a) | Ty::Heap(t, a) | Ty::ListIntoIter(t, a) => {
                self.part_why("the element", t, sync, env, seen).or_else(|| self.alloc_why(a, sync, env, seen))
            }
            Ty::Map(k, v, a) | Ty::MapIntoIter(k, v, a) => self
                .part_why("the key", k, sync, env, seen)
                .or_else(|| self.part_why("the value", v, sync, env, seen))
                .or_else(|| self.alloc_why(a, sync, env, seen)),
            Ty::Result(t, e) => self
                .part_why("the `Ok` type", t, sync, env, seen)
                .or_else(|| self.part_why("the `Err` type", e, sync, env, seen)),
            Ty::Tuple(ts) => ts.iter().find_map(|t| self.part_why("an element", t, sync, env, seen)),
            // These point into another collection, so they behave like the shared or exclusive views they hold.
            Ty::MapEntry(k, v, _) | Ty::MapIter(k, v) | Ty::MapIterMut(k, v) => {
                self.part_why("the key", k, sync, env, seen).or_else(|| self.part_why("the value", v, sync, env, seen))
            }
            Ty::IterMut(t) | Ty::SliceChunks(t, _) | Ty::SliceWindows(t) => {
                self.part_why("the element", t, sync, env, seen)
            }
            // A handle to a cell several handles share: it moves when the value may move and be used from another
            // task, and it is shared when the value may be both.
            Ty::Shared(t) => self
                .part_why("the shared value", t, false, env, seen)
                .or_else(|| self.part_why("the shared value", t, true, env, seen)),
            Ty::Struct(_) | Ty::Enum(_) => self.adt_why(ty, iface, sync, env, seen),
        }
    }

    /// `marker_why` for one part of a type, with the part named in front of the reason.
    fn part_why(&self, what: &str, ty: &Ty, sync: bool, env: &Env, seen: &mut Vec<(Ty, bool)>) -> Option<String> {
        let name = if sync { "Sync" } else { "Send" };
        self.marker_why(ty, sync, env, seen).map(|why| format!("{what} `{ty}` is not `{name}`: {why}"))
    }

    fn alloc_why(&self, alloc: &Alloc, sync: bool, env: &Env, seen: &mut Vec<(Ty, bool)>) -> Option<String> {
        alloc.allocator_ty().and_then(|t| self.part_why("the allocator", t, sync, env, seen))
    }

    /// A struct or enum: its own declaration if it has one, otherwise its fields.
    fn adt_why(&self, ty: &Ty, iface: usize, sync: bool, env: &Env, seen: &mut Vec<(Ty, bool)>) -> Option<String> {
        let name = if sync { "Sync" } else { "Send" };
        if self.has_impl_head(ty, iface) {
            return if self.impl_blocks_cover(ty, iface, &[], env) {
                None
            } else {
                Some(format!("its `{name}` declaration does not apply to these type arguments"))
            };
        }
        // A recursive type (`Node` holding `Option<Heap<Node>>`) is `Send` when its other parts are.
        let entry = (ty.clone(), sync);
        if seen.contains(&entry) {
            return None;
        }
        seen.push(entry);
        let why = match ty {
            Ty::Struct(a) => self.structs[a.id].fields.iter().find_map(|f| {
                self.marker_why(&f.ty, sync, env, seen)
                    .map(|why| format!("field `{}` of type `{}` is not `{name}`: {why}", f.name, f.ty))
            }),
            Ty::Enum(a) => self.enums[a.id].variants.iter().find_map(|v| {
                v.fields.iter().find_map(|f| {
                    self.marker_why(&f.ty, sync, env, seen).map(|why| {
                        format!(
                            "field `{}` of variant `{}` has type `{}`, which is not `{name}`: {why}",
                            f.name, v.name, f.ty
                        )
                    })
                })
            }),
            _ => None,
        };
        seen.pop();
        why
    }

    /// The code and text of an unmet bound at a call or a type use. `subject` says what asked for it ("`T` of
    /// `spawn`"). A missing `Send` or `Sync` is E0280 and explains which part is to blame; every other bound is E0141.
    pub(crate) fn bound_unmet(&self, ty: &Ty, bound: &Bound, env: &Env, subject: &str) -> (Code, String) {
        if let Bound::User(i, _) = bound
            && let Some(sync) = self.marker_kind(*i)
        {
            let name = if sync { "Sync" } else { "Send" };
            let why = self.marker_violation(ty, sync, env).unwrap_or_default();
            return (Code::NOT_SEND, format!("`{ty}` is not `{name}`, which {subject} requires: {why}"));
        }
        (
            Code::BOUND_NOT_SATISFIED,
            format!("`{ty}` does not implement `{}`, which {subject} requires", self.bound_name(bound)),
        )
    }

    /// `bound_unmet` for a bound of a declaration whose type parameters are replaced by `subst`. A projected bound
    /// (`where I.Item: Eq`) is about its projection under `subst`, so the message names the type the projection stands
    /// for, and the projection itself as the subject (spec/associated-types.md, AT7).
    pub(crate) fn unmet_bound(
        &self,
        ty: &Ty,
        bound: &Bound,
        subst: &[(String, Ty)],
        env: &Env,
        subject: &str,
    ) -> (Code, String) {
        if let Bound::Projected(projection, inner) = bound {
            let value = self.subst_ty(projection, subst);
            return self.bound_unmet(&value, inner, env, &format!("`{projection}`"));
        }
        self.bound_unmet(ty, bound, env, subject)
    }
}
