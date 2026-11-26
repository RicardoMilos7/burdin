//! Questions about types that need the struct and enum definitions: `Copy`, `Clone`, `Drop`.

use crate::defs::{Derives, EnumDef, LangItem, StructDef};
use crate::ty::{Alloc, Ty};

/// The struct and enum definitions of a program, for the questions a [`Ty`] cannot answer alone.
#[derive(Clone, Copy)]
pub struct TypeDefs<'a> {
    pub structs: &'a [StructDef],
    pub enums: &'a [EnumDef],
}

/// The two built-in interfaces that follow from a type's parts.
#[derive(Clone, Copy, PartialEq)]
enum Property {
    Copy,
    Clone,
}

impl Property {
    fn derived(self, derives: &Derives) -> bool {
        match self {
            Property::Copy => derives.copy,
            Property::Clone => derives.clone || derives.clone_method.is_some(),
        }
    }
}

impl TypeDefs<'_> {
    /// The payload type of an `Option` instance, `None` when `ty` is not one.
    pub fn option_payload(&self, ty: &Ty) -> Option<&Ty> {
        let Ty::Enum(a) = ty else { return None };
        let def = &self.enums[a.id];
        (def.lang == Some(LangItem::Option)).then(|| &def.variants[1].fields[0].ty)
    }

    /// The `Ok` and `Err` payload types of a `Result` instance, `None` when `ty` is not one.
    pub fn result_parts(&self, ty: &Ty) -> Option<(&Ty, &Ty)> {
        let Ty::Enum(a) = ty else { return None };
        let def = &self.enums[a.id];
        (def.lang == Some(LangItem::Result)).then(|| (&def.variants[0].fields[0].ty, &def.variants[1].fields[0].ty))
    }

    /// The element type and allocator of a `List` instance, `None` when `ty` is not one.
    pub fn list_parts(&self, ty: &Ty) -> Option<(&Ty, Alloc)> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        (def.lang == Some(LangItem::List)).then(|| Self::seq_parts(def, def.fields.len() - 1))
    }

    /// The key type, value type, and allocator of a `Map` instance, `None` when `ty` is not one.
    pub fn map_parts(&self, ty: &Ty) -> Option<(&Ty, &Ty, Alloc)> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        if def.lang != Some(LangItem::Map) {
            return None;
        }
        let Ty::CPtr(entry) = &def.fields[0].ty else { unreachable!("a map's first field is its buffer pointer") };
        let Ty::Tuple(parts) = &**entry else { unreachable!("a map's buffer holds (key, value, hash) entries") };
        Some((&parts[0], &parts[1], Alloc::of_field(&def.fields[def.fields.len() - 1].ty)))
    }

    /// The element type and allocator of a `ListIntoIter` instance, `None` when `ty` is not one.
    pub fn list_into_iter_parts(&self, ty: &Ty) -> Option<(&Ty, Alloc)> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        (def.lang == Some(LangItem::ListIntoIter)).then(|| Self::seq_parts(def, def.fields.len() - 1))
    }

    /// The endpoint type and whether the range is inclusive, when `ty` is a `Range` or a `RangeInclusive` instance.
    pub fn range_parts(&self, ty: &Ty) -> Option<(&Ty, bool)> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        match def.lang {
            Some(LangItem::Range) => Some((&def.fields[0].ty, false)),
            Some(LangItem::RangeInclusive) => Some((&def.fields[0].ty, true)),
            _ => None,
        }
    }

    /// The same for the iterator of a range (`RangeIter` or `RangeIterInclusive`).
    pub fn range_iter_parts(&self, ty: &Ty) -> Option<(&Ty, bool)> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        match def.lang {
            Some(LangItem::RangeIter) => Some((&def.fields[0].ty, false)),
            Some(LangItem::RangeIterInclusive) => Some((&def.fields[0].ty, true)),
            _ => None,
        }
    }

    /// The element type of an `ArrayIntoIter` instance, `None` when `ty` is not one.
    pub fn array_into_iter_elem(&self, ty: &Ty) -> Option<&Ty> {
        let Ty::Struct(a) = ty else { return None };
        let def = &self.structs[a.id];
        if def.lang != Some(LangItem::ArrayIntoIter) {
            return None;
        }
        let Ty::Struct(inner) = &def.fields[0].ty else { return None };
        match &self.structs[inner.id].fields[0].ty {
            Ty::Array(elem, _) => Some(elem),
            _ => None,
        }
    }

    fn seq_parts(def: &StructDef, alloc_field: usize) -> (&Ty, Alloc) {
        let Ty::CPtr(elem) = &def.fields[0].ty else { unreachable!("a list's first field is its buffer pointer") };
        (elem, Alloc::of_field(&def.fields[alloc_field].ty))
    }

    /// Whether values of `ty` are copied instead of moved (memory-rules.md, Copy types).
    pub fn is_copy(&self, ty: &Ty) -> bool {
        self.holds(ty, Property::Copy, &mut Vec::new())
    }

    /// Whether `x.clone()` applies to values of `ty` (the `Clone` requirement of derives, CP).
    pub fn is_clone(&self, ty: &Ty) -> bool {
        self.holds(ty, Property::Clone, &mut Vec::new())
    }

    /// Whether dropping a value of `ty` runs any code.
    pub fn needs_drop(&self, ty: &Ty) -> bool {
        self.needs_drop_seen(ty, &mut Vec::new())
    }

    /// Whether the type itself declares `Type: Drop { ... }` (fields cannot be moved out, DR8).
    pub fn has_drop_impl(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Struct(a) => self.structs[a.id].drop_method.is_some(),
            Ty::Enum(a) => self.enums[a.id].drop_method.is_some(),
            _ => false,
        }
    }

    /// Whether `ty` has the property: it is built from parts that have it, or it derives it and all its fields have
    /// it. `seen` holds the structs and enums being visited, so a recursive type does not loop.
    fn holds(&self, ty: &Ty, prop: Property, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => true,
            Ty::Chars => true,
            // A shared reference and a view are copied; a mutable reference is not (RF4). Cloning a view
            // yields the same view.
            Ty::Ref { mutable, .. } => prop == Property::Clone || !*mutable,
            Ty::Slice(_) => true,
            // A raw C pointer is a plain address.
            Ty::CPtr(_) => true,
            // An iterator advances through its state, so it is never copied or cloned. A map entry owns its key
            // and views its map, so it is moved into the operation that consumes it.
            Ty::MapIter(..)
            | Ty::IterMut(_)
            | Ty::MapIterMut(..)
            | Ty::MapIntoIter(..)
            | Ty::SliceChunks(..)
            | Ty::SliceWindows(..) => false,
            // A closure owns its capture record, so it is never copied or cloned (CL10).
            Ty::Fn { .. } => false,
            Ty::String => prop == Property::Clone,
            // A heap clones its allocator value and the pointee; the default allocator stores no value.
            Ty::Heap(_, a) => prop == Property::Clone && a.allocator_ty().is_none_or(|a| self.holds(a, prop, seen)),
            // A `Shared<T>` is cloned by sharing (a reference-count increase), whatever `T` is.
            // A list or a map owns its buffer, so it is never `Copy`; it can be deep-cloned when its elements and
            // its allocator value can.
            // A projection is opaque until the implementation's binding is substituted; it is never copied or
            // cloned on its own (`spec/associated-types.md`, AT3).
            Ty::Param(_) | Ty::Const(_) | Ty::Projection { .. } | Ty::TryFromError { .. } => false,
            // An `any` box owns a heap allocation and cannot be copied or cloned (AN1).
            Ty::Any(_) => false,
            Ty::Array(t, _) => self.holds(t, prop, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.holds(t, prop, seen)),
            Ty::Struct(a) => {
                let def = &self.structs[a.id];
                if let Some((elem, alloc)) = self.list_parts(ty) {
                    return prop == Property::Clone
                        && self.holds(elem, prop, seen)
                        && alloc.allocator_ty().is_none_or(|a| self.holds(a, prop, seen));
                }
                if let Some((k, v, alloc)) = self.map_parts(ty) {
                    return prop == Property::Clone
                        && self.holds(k, prop, seen)
                        && self.holds(v, prop, seen)
                        && alloc.allocator_ty().is_none_or(|a| self.holds(a, prop, seen));
                }
                if def.lang == Some(LangItem::ListIntoIter) {
                    return false;
                }
                self.derived_from_fields(false, a.id, prop, &def.derives, seen, || {
                    def.fields.iter().map(|f| &f.ty).collect()
                })
            }
            Ty::Enum(a) => {
                let def = &self.enums[a.id];
                self.derived_from_fields(true, a.id, prop, &def.derives, seen, || {
                    def.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)).collect()
                })
            }
        }
    }

    fn derived_from_fields<'t>(
        &self,
        is_enum: bool,
        id: usize,
        prop: Property,
        derives: &Derives,
        seen: &mut Vec<(bool, usize)>,
        fields: impl FnOnce() -> Vec<&'t Ty>,
    ) -> bool {
        if !prop.derived(derives) {
            return false;
        }
        if seen.contains(&(is_enum, id)) {
            return true;
        }
        seen.push((is_enum, id));
        fields().into_iter().all(|t| self.holds(t, prop, seen))
    }

    fn needs_drop_seen(&self, ty: &Ty, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => false,
            Ty::Ref { .. }
            | Ty::Slice(_)
            | Ty::Param(_)
            | Ty::Const(_)
            | Ty::Projection { .. }
            | Ty::TryFromError { .. }
            | Ty::CPtr(_)
            | Ty::Chars => false,
            Ty::MapIter(..) | Ty::IterMut(_) | Ty::MapIterMut(..) | Ty::SliceChunks(..) | Ty::SliceWindows(..) => false,
            // A closure's capture record is heap-allocated and may own its captures.
            Ty::Fn { .. } => true,
            // A box owns its heap allocation and the value inside it (AN1/AN7).
            Ty::Any(_) => true,
            // An owning iterator frees its buffer (or drops the elements it has not handed out).
            Ty::String | Ty::Heap(..) | Ty::MapIntoIter(..) => true,
            Ty::Array(t, n) => *n > 0 && self.needs_drop_seen(t, seen),
            Ty::Tuple(ts) => ts.iter().any(|t| self.needs_drop_seen(t, seen)),
            Ty::Struct(a) => {
                if seen.contains(&(false, a.id)) {
                    return false;
                }
                seen.push((false, a.id));
                let s = &self.structs[a.id];
                // Glue never drops the value of a `ManuallyDrop`.
                if s.lang == Some(LangItem::ManuallyDrop) {
                    return false;
                }
                // A map frees its table and drops its entries.
                if s.lang == Some(LangItem::Map) {
                    return true;
                }
                s.drop_method.is_some() || s.fields.iter().any(|f| self.needs_drop_seen(&f.ty, seen))
            }
            Ty::Enum(a) => {
                if seen.contains(&(true, a.id)) {
                    return false;
                }
                seen.push((true, a.id));
                let e = &self.enums[a.id];
                e.drop_method.is_some()
                    || e.variants.iter().any(|v| v.fields.iter().any(|f| self.needs_drop_seen(&f.ty, seen)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{FieldDef, VariantDef, VariantShape};
    use crate::ops::FuncId;
    use crate::ty::{AdtRef, Alloc, IntTy};
    use burdin_diag::{FileId, Span};

    fn span() -> Span {
        Span::new(FileId(0), 0, 0)
    }

    fn field(name: &str, ty: Ty) -> FieldDef {
        FieldDef { name: name.into(), ty, span: span(), is_priv: false }
    }

    fn struct_def(name: &str, fields: Vec<FieldDef>, derives: Derives, drop_method: Option<FuncId>) -> StructDef {
        StructDef { name: name.into(), fields, repr_c: false, derives, drop_method, lang: None }
    }

    fn derives(copy: bool, clone: bool) -> Derives {
        Derives { copy, clone, ..Derives::default() }
    }

    /// A `List<elem, alloc>` instance: the lang struct `std` declares, with its `Drop` impl.
    fn list_def(elem: Ty, alloc: Ty) -> StructDef {
        let mut def = struct_def(
            "List",
            vec![
                field("ptr", Ty::CPtr(Box::new(elem))),
                field("len", Ty::Int(IntTy::Usize)),
                field("cap", Ty::Int(IntTy::Usize)),
                field("alloc", alloc),
            ],
            Derives::default(),
            Some(FuncId(0)),
        );
        def.lang = Some(LangItem::List);
        def
    }

    fn map_def(key: Ty, value: Ty, alloc: Ty) -> StructDef {
        let entry = Ty::Tuple(vec![key, value, Ty::Int(IntTy::U64)]);
        let mut def = struct_def(
            "Map",
            vec![
                field("ptr", Ty::CPtr(Box::new(entry))),
                field("len", Ty::Int(IntTy::Usize)),
                field("cap", Ty::Int(IntTy::Usize)),
                field("alloc", alloc),
            ],
            Derives::default(),
            None,
        );
        def.lang = Some(LangItem::Map);
        def
    }

    fn global() -> Ty {
        Ty::Struct(AdtRef { id: 99, name: "std.alloc.Global".into(), second_class: false })
    }

    fn adt(name: &str, id: usize) -> AdtRef {
        AdtRef { id, name: name.into(), second_class: false }
    }

    fn int() -> Ty {
        Ty::Int(IntTy::I32)
    }

    #[test]
    fn copy_needs_the_derive_and_copy_fields() {
        let structs = [
            struct_def("Point", vec![field("x", int()), field("y", int())], derives(true, true), None),
            struct_def("Text", vec![field("s", Ty::String)], derives(true, true), None),
            struct_def("Plain", vec![field("x", int())], Derives::default(), None),
            list_def(int(), global()),
        ];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        assert!(defs.is_copy(&Ty::Struct(adt("Point", 0))));
        assert!(!defs.is_copy(&Ty::Struct(adt("Text", 1))), "String is not Copy");
        assert!(!defs.is_copy(&Ty::Struct(adt("Plain", 2))), "no derive");
        assert!(defs.is_copy(&int()));
        assert!(defs.is_copy(&Ty::Tuple(vec![int(), Ty::Bool])));
        assert!(defs.is_copy(&Ty::Array(Box::new(int()), 4)));
        assert!(defs.is_copy(&Ty::CPtr(Box::new(Ty::String))));
        assert!(defs.is_copy(&Ty::Ref { mutable: false, inner: Box::new(Ty::String) }));
        assert!(!defs.is_copy(&Ty::Ref { mutable: true, inner: Box::new(int()) }), "a &mut is not Copy");
        assert!(!defs.is_copy(&Ty::String));
        assert!(!defs.is_copy(&Ty::Struct(adt("List", 3))), "a list owns its buffer");
    }

    #[test]
    fn an_any_box_is_owned_droppable_and_not_copy_or_clone() {
        let defs = TypeDefs { structs: &[], enums: &[] };
        let any = Ty::Any(crate::ty::AnyRef { id: 0, name: "Shape".into(), iface: 0, args: Vec::new() });
        assert!(!defs.is_copy(&any));
        assert!(!defs.is_clone(&any));
        assert!(defs.needs_drop(&any));
        assert!(!defs.has_drop_impl(&any));
        assert!(!any.is_second_class(), "a box owns its value");
    }

    #[test]
    fn closures_own_their_record_and_are_never_copied_or_cloned() {
        let defs = TypeDefs { structs: &[], enums: &[] };
        let closure = Ty::Fn {
            sig: std::rc::Rc::new(crate::ty::FnSig { params: Vec::new(), ret: Box::new(Ty::Unit) }),
            kind: crate::ty::FnKind::Fn,
            borrowed: false,
            send: crate::ty::FnSend::unknown(),
        };
        assert!(!defs.is_copy(&closure));
        assert!(!defs.is_clone(&closure));
        assert!(defs.needs_drop(&closure));
        assert!(!defs.has_drop_impl(&closure));
    }

    #[test]
    fn clone_follows_the_parts() {
        let structs = [list_def(Ty::String, global())];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        assert!(defs.is_clone(&Ty::String));
        assert!(defs.is_clone(&Ty::Heap(Box::new(int()), crate::ty::Alloc::Global)));
        assert!(defs.is_clone(&Ty::Struct(adt("List", 0))));
        assert!(defs.is_clone(&Ty::Ref { mutable: true, inner: Box::new(int()) }));
    }

    #[test]
    fn a_stored_allocator_follows_the_clone_rules() {
        // A container with a stored allocator clones that value too (spec/allocators.md, AL7), so a non-Clone
        // allocator makes the container non-Clone, while `Global` stores nothing.
        let structs = [
            struct_def("Pool", vec![], derives(false, true), None),
            struct_def("Raw", vec![], derives(false, false), None),
            list_def(int(), Ty::Struct(adt("Pool", 0))),
            list_def(int(), Ty::Struct(adt("Raw", 1))),
            map_def(Ty::Str, int(), Ty::Struct(adt("Pool", 0))),
            map_def(Ty::Str, int(), Ty::Struct(adt("Raw", 1))),
        ];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        let pool = Alloc::Value(Box::new(Ty::Struct(adt("Pool", 0))));
        let raw = Alloc::Value(Box::new(Ty::Struct(adt("Raw", 1))));
        assert!(defs.is_clone(&Ty::Struct(adt("List", 2))));
        assert!(defs.is_clone(&Ty::Struct(adt("Map", 4))));
        assert!(defs.is_clone(&Ty::Heap(Box::new(int()), pool)));
        assert!(!defs.is_clone(&Ty::Struct(adt("List", 3))));
        assert!(!defs.is_clone(&Ty::Struct(adt("Map", 5))));
        assert!(!defs.is_clone(&Ty::Heap(Box::new(int()), raw)), "a heap clones its allocator too");
    }

    #[test]
    fn drop_follows_the_fields() {
        let structs = [
            struct_def("Owns", vec![field("s", Ty::String)], Derives::default(), None),
            struct_def("Empty", vec![field("x", int())], Derives::default(), None),
            struct_def("WithDrop", vec![], Derives::default(), Some(FuncId(3))),
            list_def(int(), global()),
            map_def(Ty::Str, int(), global()),
        ];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        assert!(defs.needs_drop(&Ty::String));
        assert!(!defs.needs_drop(&int()));
        assert!(defs.needs_drop(&Ty::Struct(adt("Owns", 0))));
        assert!(!defs.needs_drop(&Ty::Struct(adt("Empty", 1))));
        assert!(defs.needs_drop(&Ty::Struct(adt("WithDrop", 2))), "a Drop impl needs no fields");
        // An empty array holds no value to drop.
        assert!(!defs.needs_drop(&Ty::Array(Box::new(Ty::String), 0)));
        assert!(defs.needs_drop(&Ty::Array(Box::new(Ty::String), 1)));
        assert!(!defs.needs_drop(&Ty::Ref { mutable: false, inner: Box::new(Ty::String) }));
        assert!(defs.needs_drop(&Ty::Struct(adt("List", 3))));
        assert!(defs.needs_drop(&Ty::Struct(adt("Map", 4))));
    }

    #[test]
    fn drop_impl_question_is_separate_from_drop_glue() {
        let structs = [struct_def("WithDrop", vec![], Derives::default(), Some(FuncId(0)))];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        assert!(defs.has_drop_impl(&Ty::Struct(adt("WithDrop", 0))));
        assert!(!defs.has_drop_impl(&Ty::String));
    }

    #[test]
    fn recursive_types_terminate() {
        // `Node { next: Option<Node> }` is not a legal by-value type, but the questions must still terminate on
        // any cycle a definition could describe.
        let node = Ty::Struct(adt("Node", 0));
        let structs = [struct_def("Node", vec![field("next", node.clone())], derives(true, true), None)];
        let defs = TypeDefs { structs: &structs, enums: &[] };
        assert!(!defs.needs_drop(&node));
        assert!(defs.is_copy(&node));
        assert!(defs.is_clone(&node));
    }

    #[test]
    fn enum_questions_look_at_every_variant() {
        let with_string = EnumDef {
            name: "E".into(),
            variants: vec![
                VariantDef { name: "Unit".into(), shape: VariantShape::Unit, fields: vec![] },
                VariantDef { name: "Text".into(), shape: VariantShape::Tuple, fields: vec![field("s", Ty::String)] },
            ],
            derives: Derives { hash: true, ..Derives::default() },
            drop_method: None,
            lang: None,
        };
        let defs = TypeDefs { structs: &[], enums: &[with_string] };
        let ty = Ty::Enum(adt("E", 0));
        assert!(defs.needs_drop(&ty));
        assert!(!defs.is_copy(&ty));
        assert!(!defs.is_clone(&ty));
        assert!(!defs.has_drop_impl(&ty));
    }
}
