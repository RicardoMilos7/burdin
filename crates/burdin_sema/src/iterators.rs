//! The iteration protocol (`spec/iterators.md`).
//!
//! `Iterator<T>` and `Iterable<T>` are ordinary generic interfaces declared by `std`. The built-in collections
//! implement the protocol in the compiler instead of with impl blocks: a `List<T>`, an array, a slice, a range,
//! `Chars`, and the compiler-known iterator types have a known item type, and `next`, `iter`, and `into_iter` are
//! built-in methods (see `builtins::method` and `fncx::call`).
//!
//! This module answers the questions the checker asks: what an iterator yields, what a collection iterates over,
//! and whether a type implements `Iterator` or `Iterable` without an impl block.

use crate::generics::Bound;
use crate::globals::{Globals, adt_key};
use burdin_tir::Ty;

impl Globals {
    /// The item type of one of the compiler-known iterators. A slice, an array, and a `List` yield their element
    /// by value when it is `Copy` and as a shared view otherwise; a map yields a `(&K, &V)` pair.
    pub(crate) fn builtin_iterator_item(&self, ty: &Ty, env: &crate::generics::Env) -> Option<Ty> {
        Some(match ty {
            Ty::Slice(elem) => Self::element_item(elem, env, self),
            Ty::Chars => Ty::Char,
            // `chunks`, `chunks_exact`, and `windows` yield views of the slice they were made from.
            Ty::SliceChunks(elem, _) | Ty::SliceWindows(elem) => Ty::Slice(elem.clone()),
            Ty::MapIter(key, value) => Ty::Tuple(vec![Self::shared_view(key), Self::shared_view(value)]),
            // A mutable iterator yields a writable view of each element; a map's keys stay read-only (IT15).
            Ty::IterMut(elem) => Self::mutable_view(elem),
            Ty::MapIterMut(key, value) => Ty::Tuple(vec![Self::shared_view(key), Self::mutable_view(value)]),
            Ty::MapIntoIter(key, value, _) => Ty::Tuple(vec![(**key).clone(), (**value).clone()]),
            _ => return None,
        })
    }

    /// The item type of one of the compiler-known iterators that can also take items from the back: a slice, a
    /// range iterator, and `Chars` (`spec/iterators.md`, IT14). The owning and map iterators cannot.
    pub(crate) fn builtin_double_ended_item(&self, ty: &Ty, env: &crate::generics::Env) -> Option<Ty> {
        match ty {
            Ty::Slice(_) | Ty::Chars => self.builtin_iterator_item(ty, env),
            _ => None,
        }
    }

    /// The item type of one of the compiler-known iterable collections: what `iter()` yields.
    pub(crate) fn builtin_iterable_item(&self, ty: &Ty, env: &crate::generics::Env) -> Option<Ty> {
        // A `List` iterates through the slice view of its elements.
        if let Some((elem, _)) = self.list_parts(ty) {
            return Some(Self::element_item(&elem, env, self));
        }
        if let Some((key, value, _)) = self.map_parts(ty) {
            return Some(Ty::Tuple(vec![Self::shared_view(&key), Self::shared_view(&value)]));
        }
        Some(match ty {
            Ty::Array(elem, _) | Ty::Slice(elem) => Self::element_item(elem, env, self),
            Ty::Chars => Ty::Char,
            _ => return None,
        })
    }

    /// The item type of `ty` as an `Iterator`: a built-in iterator's item, or the `T` of a user
    /// `Type: Iterator<T>` block.
    pub(crate) fn iterator_item(&self, ty: &Ty, env: &crate::generics::Env) -> Option<Ty> {
        if let Some(item) = self.builtin_iterator_item(ty, env) {
            return Some(item);
        }
        self.declared_item(ty, self.iterator_iface, env)
    }

    /// The item type of `ty` as an `Iterable`: what a `for` loop over it binds.
    pub(crate) fn iterable_item(&self, ty: &Ty, env: &crate::generics::Env) -> Option<Ty> {
        if let Some(item) = self.builtin_iterable_item(ty, env) {
            return Some(item);
        }
        self.declared_item(ty, self.iterable_iface, env)
    }

    /// The interface arguments a built-in type satisfies for `iface` (the iterator protocol only).
    pub(crate) fn builtin_iface_args(&self, ty: &Ty, iface: usize, env: &crate::generics::Env) -> Option<Vec<Ty>> {
        if Some(iface) == self.iterator_iface {
            return self.builtin_iterator_item(ty, env).map(|item| vec![item]);
        }
        if Some(iface) == self.iterable_iface {
            return self.builtin_iterable_item(ty, env).map(|item| vec![item]);
        }
        if Some(iface) == self.double_ended_iface {
            return self.builtin_double_ended_item(ty, env).map(|item| vec![item]);
        }
        None
    }

    /// Whether a built-in type implements `FromIterator<item>` without an impl block, or `None` for a struct or an
    /// enum, which implements it with a block (`spec/iterators.md`, IT18). A `List` and a `Map` are built with the
    /// default allocator only, so one that stores another allocator never implements it.
    pub(crate) fn builtin_from_iterator(&self, ty: &Ty, args: &[Ty], env: &crate::generics::Env) -> Option<bool> {
        let [item] = args else { return None };
        let iface = self.from_iterator_iface?;
        let from_items =
            |target: &Ty, items: Ty| self.satisfies_with(target, &Bound::User(iface, vec![items]), env, &[]);
        if let Some((elem, alloc)) = self.list_parts(ty) {
            return Some(alloc.is_global() && elem == *item && !elem.is_second_class());
        }
        if let Some((key, value, alloc)) = self.map_parts(ty) {
            return Some(
                alloc.is_global()
                    && *item == Ty::Tuple(vec![key.clone(), value.clone()])
                    && !key.is_second_class()
                    && !value.is_second_class()
                    && self.has_key_eq_in(&key, env),
            );
        }
        Some(match ty {
            Ty::String => match item {
                Ty::Char | Ty::Str | Ty::String => true,
                Ty::Ref { inner, .. } => matches!(**inner, Ty::String | Ty::Str),
                _ => false,
            },
            _ => {
                if let Some(target) = self.option_payload(ty) {
                    match self.option_payload(item) {
                        Some(inner) => from_items(&target, inner),
                        None => false,
                    }
                } else if let Some((target, error)) = self.result_parts(ty) {
                    match self.result_parts(item) {
                        Some((inner, item_error)) => error == item_error && from_items(&target, inner),
                        None => false,
                    }
                } else {
                    return None;
                }
            }
        })
    }

    /// The item type recorded by a user `Type: Iterator<T>` or `Type: Iterable<T>` block, for a concrete
    /// instance or a type parameter with the bound in scope.
    pub(crate) fn declared_item(&self, ty: &Ty, iface: Option<usize>, env: &crate::generics::Env) -> Option<Ty> {
        let iface = iface?;
        // An `impl Interface` value has the item its interface names (`crate::opaque`).
        if let Some(o) = self.opaque_of(ty) {
            return self.bound_closure(&o.bound).into_iter().find_map(|c| match c {
                Bound::User(i, args) if i == iface && args.len() == 1 => Some(args[0].clone()),
                _ => None,
            });
        }
        match ty {
            Ty::Param(name) => env.iter().find(|(n, _)| n == &**name).and_then(|(_, bounds)| {
                bounds.iter().find_map(|b| {
                    match self
                        .bound_closure(b)
                        .into_iter()
                        .find(|c| matches!(c, Bound::User(i, args) if *i == iface && args.len() == 1))
                    {
                        Some(Bound::User(_, args)) => Some(args[0].clone()),
                        _ => None,
                    }
                })
            }),
            _ => {
                let key = adt_key(ty)?;
                self.impls.iter().find_map(|info| {
                    if info.iface != iface {
                        return None;
                    }
                    let subst: Vec<(String, Ty)> = match info.head {
                        crate::generics::Head::Plain(k) => {
                            if k != key {
                                return None;
                            }
                            Vec::new()
                        }
                        crate::generics::Head::Generic(template) => match self.adt_origin.get(&key) {
                            Some((t, args)) if *t == template => {
                                self.match_impl_args(&info.params, &info.args, args)?
                            }
                            _ => return None,
                        },
                    };
                    info.iface_args.first().map(|a| self.subst_ty(a, &subst))
                })
            }
        }
    }

    /// An element yielded by a borrowing collection: the value when it is `Copy`, a shared view otherwise. This
    /// keeps `for` and `.iter()` alike comfortable with numbers while non-`Copy` elements stay borrowed.
    fn element_item(elem: &Ty, env: &crate::generics::Env, globals: &Globals) -> Ty {
        let copy = match elem {
            Ty::Param(name) => globals.bound_in_env(name, &Bound::Copy, env),
            _ => globals.is_copy(elem),
        };
        if copy { elem.clone() } else { Self::shared_view(elem) }
    }

    fn mutable_view(ty: &Ty) -> Ty {
        Ty::Ref { mutable: true, inner: Box::new(ty.clone()) }
    }

    fn shared_view(ty: &Ty) -> Ty {
        Ty::Ref { mutable: false, inner: Box::new(ty.clone()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generics::param_ty;
    use burdin_tir::IntTy;

    fn i32_() -> Ty {
        Ty::Int(IntTy::I32)
    }

    #[test]
    fn builtin_collections_have_an_item_type() {
        let mut g = crate::testutil::lang_globals();
        let map = g.test_map(Ty::Str, i32_());
        let item = g.builtin_iterable_item(&map, &[]).expect("a map iterates");
        assert_eq!(
            item,
            Ty::Tuple(vec![
                Ty::Ref { mutable: false, inner: Box::new(Ty::Str) },
                Ty::Ref { mutable: false, inner: Box::new(i32_()) }
            ])
        );
        assert_eq!(g.builtin_iterable_item(&Ty::Chars, &[]), Some(Ty::Char));
    }

    #[test]
    fn iterators_report_their_item() {
        let g = Globals::default();
        assert_eq!(g.builtin_iterator_item(&Ty::Slice(Box::new(i32_())), &[]), Some(i32_()));
        assert_eq!(g.builtin_iterator_item(&Ty::Chars, &[]), Some(Ty::Char));
        let map = Ty::MapIter(Box::new(Ty::Str), Box::new(i32_()));
        assert!(g.builtin_iterator_item(&map, &[]).is_some());
    }

    fn globals_with_from_iterator() -> Globals {
        let mut g = Globals { from_iterator_iface: Some(0), ..Default::default() };
        g.interfaces.push(crate::generics::InterfaceInfo {
            name: "std.FromIterator".into(),
            module: 0,
            params: vec!["T".into()],
            bounds: vec![Vec::new()],
            requires: Vec::new(),
            assoc: Vec::new(),
            methods: Vec::new(),
        });
        g
    }

    #[test]
    fn builtin_collections_are_built_from_their_items() {
        let mut g = globals_with_from_iterator();
        g.lang_map = Some(2);
        let pair = Ty::Tuple(vec![Ty::String, i32_()]);
        let map = g.test_map(Ty::String, i32_());
        assert_eq!(g.builtin_from_iterator(&map, &[pair], &[]), Some(true));
        assert_eq!(g.builtin_from_iterator(&map, &[Ty::String], &[]), Some(false), "a map is built from pairs");
    }

    #[test]
    fn a_string_is_built_from_text_items_only() {
        let g = globals_with_from_iterator();
        for item in [Ty::Char, Ty::Str, Ty::String, Ty::Ref { mutable: false, inner: Box::new(Ty::String) }] {
            assert_eq!(g.builtin_from_iterator(&Ty::String, &[item], &[]), Some(true));
        }
        assert_eq!(g.builtin_from_iterator(&Ty::String, &[i32_()], &[]), Some(false));
    }

    #[test]
    fn option_and_result_collect_through_their_payload() {
        let mut g = globals_with_from_iterator();
        g.lang_option = Some(0);
        g.lang_result = Some(1);
        let payload = Ty::String;
        let option = g.test_option(payload.clone());
        let option_item = g.test_option(Ty::Char);
        assert_eq!(g.builtin_from_iterator(&option, &[option_item], &[]), Some(true));
        assert_eq!(g.builtin_from_iterator(&option, &[Ty::Char], &[]), Some(false));
        let result = g.test_result(payload, Ty::String);
        let ok_item = g.test_result(Ty::Char, Ty::String);
        assert_eq!(g.builtin_from_iterator(&result, &[ok_item], &[]), Some(true));
        let other_error = g.test_result(Ty::Char, Ty::Bool);
        assert_eq!(g.builtin_from_iterator(&result, &[other_error], &[]), Some(false), "the errors must agree");
        // A struct or an enum answers through its impl blocks, so the built-in check has no opinion.
        assert_eq!(g.builtin_from_iterator(&i32_(), &[i32_()], &[]), None);
    }

    #[test]
    fn mutable_iterators_yield_writable_views() {
        let g = Globals::default();
        let writable = |t: Ty| Ty::Ref { mutable: true, inner: Box::new(t) };
        let shared = |t: Ty| Ty::Ref { mutable: false, inner: Box::new(t) };
        // Every element is a `&mut T`, even a `Copy` one: that is what makes it writable (IT15).
        assert_eq!(g.builtin_iterator_item(&Ty::IterMut(Box::new(i32_())), &[]), Some(writable(i32_())));
        assert_eq!(g.builtin_iterator_item(&Ty::IterMut(Box::new(Ty::String)), &[]), Some(writable(Ty::String)));
        // A map yields its keys read-only and its values writable.
        let map = Ty::MapIterMut(Box::new(i32_()), Box::new(Ty::String));
        assert_eq!(g.builtin_iterator_item(&map, &[]), Some(Ty::Tuple(vec![shared(i32_()), writable(Ty::String)])));
        // They are iterators, not iterables: `for` reaches them through `iter_mut()`.
        assert_eq!(g.builtin_iterable_item(&Ty::IterMut(Box::new(i32_())), &[]), None);
    }

    #[test]
    fn only_slices_and_chars_walk_backwards() {
        let g = Globals::default();
        assert_eq!(g.builtin_double_ended_item(&Ty::Slice(Box::new(i32_())), &[]), Some(i32_()));
        assert_eq!(g.builtin_double_ended_item(&Ty::Chars, &[]), Some(Ty::Char));
        // The map iterators yield from the front only.
        assert_eq!(g.builtin_double_ended_item(&Ty::MapIter(Box::new(Ty::Str), Box::new(i32_())), &[]), None);
    }

    #[test]
    fn the_double_ended_interface_has_its_own_builtin_arguments() {
        let g = Globals { iterator_iface: Some(0), double_ended_iface: Some(1), ..Default::default() };
        let slice = Ty::Slice(Box::new(i32_()));
        assert_eq!(g.builtin_iface_args(&slice, 1, &[]), Some(vec![i32_()]));
        let map = Ty::MapIter(Box::new(Ty::Str), Box::new(i32_()));
        assert_eq!(g.builtin_iface_args(&map, 1, &[]), None, "a map iterator is not double-ended");
    }

    #[test]
    fn slice_window_iterators_yield_slices() {
        let g = Globals::default();
        let i = Ty::Int(IntTy::I32);
        let item = Ty::Slice(Box::new(i.clone()));
        assert_eq!(g.builtin_iterator_item(&Ty::SliceChunks(Box::new(i.clone()), false), &[]), Some(item.clone()));
        assert_eq!(g.builtin_iterator_item(&Ty::SliceChunks(Box::new(i.clone()), true), &[]), Some(item.clone()));
        assert_eq!(g.builtin_iterator_item(&Ty::SliceWindows(Box::new(i.clone())), &[]), Some(item));
        // They are iterators only: a `for` loop enters them through the protocol, and `iter()` is not theirs.
        assert_eq!(g.builtin_iterable_item(&Ty::SliceChunks(Box::new(Ty::Int(IntTy::I32)), false), &[]), None);
    }

    #[test]
    fn a_type_parameter_needs_the_bound() {
        let mut g = Globals { iterator_iface: Some(0), ..Default::default() };
        g.interfaces.push(crate::generics::InterfaceInfo {
            name: "std.Iterator".into(),
            module: 0,
            params: vec!["T".into()],
            bounds: vec![Vec::new()],
            requires: Vec::new(),
            assoc: Vec::new(),
            methods: Vec::new(),
        });
        let env: &[(String, Vec<Bound>)] = &[("I".into(), vec![Bound::User(0, vec![i32_()])])];
        assert_eq!(g.iterator_item(&param_ty("I"), env), Some(i32_()));
        assert_eq!(g.iterator_item(&param_ty("J"), env), None);
    }
}
