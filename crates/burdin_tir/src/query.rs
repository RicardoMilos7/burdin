//! Questions about types that need the struct and enum definitions: `Copy`, `Clone`, `Drop`.

use crate::defs::{Derives, EnumDef, StructDef};
use crate::ty::Ty;

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
            Property::Clone => derives.clone,
        }
    }
}

impl TypeDefs<'_> {
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
            // A range is two copied endpoints.
            Ty::Range(inner, _) => self.holds(inner, prop, seen),
            // A raw C pointer is a plain address; a `CString` owns its buffer and has no clone yet.
            Ty::CPtr(_) => true,
            // An iterator advances through its state, so it is never copied or cloned.
            Ty::ListIntoIter(_) | Ty::ArrayIntoIter(..) | Ty::MapIter(..) | Ty::MapIntoIter(..) | Ty::RangeIter(..) => {
                false
            }
            // A closure owns its capture record, so it is never copied or cloned (CL10).
            Ty::Fn { .. } => false,
            Ty::String | Ty::Heap(_) | Ty::CString => prop == Property::Clone,
            // A list or a map owns its buffer, so it is never `Copy`; it can be deep-cloned when its elements can.
            Ty::List(t) => prop == Property::Clone && self.holds(t, prop, seen),
            Ty::Map(k, v) => prop == Property::Clone && self.holds(k, prop, seen) && self.holds(v, prop, seen),
            Ty::Param(_) => false,
            Ty::Result(t, e) => self.holds(t, prop, seen) && self.holds(e, prop, seen),
            Ty::Array(t, _) | Ty::Option(t) => self.holds(t, prop, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.holds(t, prop, seen)),
            Ty::Struct(a) => {
                let def = &self.structs[a.id];
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
            Ty::Ref { .. } | Ty::Slice(_) | Ty::Param(_) | Ty::CPtr(_) | Ty::Chars => false,
            Ty::Range(..) | Ty::MapIter(..) | Ty::RangeIter(..) => false,
            // A closure's capture record is heap-allocated and may own its captures.
            Ty::Fn { .. } => true,
            // An owning iterator frees its buffer (or drops the elements it has not handed out).
            Ty::String
            | Ty::Heap(_)
            | Ty::CString
            | Ty::List(_)
            | Ty::Map(..)
            | Ty::ListIntoIter(_)
            | Ty::MapIntoIter(..) => true,
            Ty::ArrayIntoIter(t, n) => *n > 0 && self.needs_drop_seen(t, seen),
            Ty::Array(t, n) => *n > 0 && self.needs_drop_seen(t, seen),
            Ty::Option(t) => self.needs_drop_seen(t, seen),
            Ty::Result(t, e) => self.needs_drop_seen(t, seen) || self.needs_drop_seen(e, seen),
            Ty::Tuple(ts) => ts.iter().any(|t| self.needs_drop_seen(t, seen)),
            Ty::Struct(a) => {
                if seen.contains(&(false, a.id)) {
                    return false;
                }
                seen.push((false, a.id));
                let s = &self.structs[a.id];
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
    use crate::ty::{AdtRef, IntTy};
    use burdin_diag::{FileId, Span};

    fn span() -> Span {
        Span::new(FileId(0), 0, 0)
    }

    fn field(name: &str, ty: Ty) -> FieldDef {
        FieldDef { name: name.into(), ty, span: span(), is_priv: false }
    }

    fn struct_def(name: &str, fields: Vec<FieldDef>, derives: Derives, drop_method: Option<FuncId>) -> StructDef {
        StructDef { name: name.into(), fields, repr_c: false, derives, drop_method }
    }

    fn derives(copy: bool, clone: bool) -> Derives {
        Derives { copy, clone, ..Derives::default() }
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
        assert!(!defs.is_copy(&Ty::List(Box::new(int()))));
    }

    #[test]
    fn closures_own_their_record_and_are_never_copied_or_cloned() {
        let defs = TypeDefs { structs: &[], enums: &[] };
        let closure = Ty::Fn {
            sig: std::rc::Rc::new(crate::ty::FnSig { params: Vec::new(), ret: Box::new(Ty::Unit) }),
            kind: crate::ty::FnKind::Fn,
            borrowed: false,
        };
        assert!(!defs.is_copy(&closure));
        assert!(!defs.is_clone(&closure));
        assert!(defs.needs_drop(&closure));
        assert!(!defs.has_drop_impl(&closure));
    }

    #[test]
    fn clone_follows_the_parts() {
        let defs = TypeDefs { structs: &[], enums: &[] };
        assert!(defs.is_clone(&Ty::String));
        assert!(defs.is_clone(&Ty::Heap(Box::new(int()))));
        assert!(defs.is_clone(&Ty::List(Box::new(Ty::String))));
        assert!(defs.is_clone(&Ty::List(Box::new(Ty::CString))));
        assert!(defs.is_clone(&Ty::CString));
        assert!(defs.is_clone(&Ty::Ref { mutable: true, inner: Box::new(int()) }));
    }

    #[test]
    fn drop_follows_the_fields() {
        let structs = [
            struct_def("Owns", vec![field("s", Ty::String)], Derives::default(), None),
            struct_def("Empty", vec![field("x", int())], Derives::default(), None),
            struct_def("WithDrop", vec![], Derives::default(), Some(FuncId(3))),
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
        assert!(defs.needs_drop(&Ty::Option(Box::new(Ty::String))));
        assert!(!defs.needs_drop(&Ty::Ref { mutable: false, inner: Box::new(Ty::String) }));
        assert!(defs.needs_drop(&Ty::List(Box::new(int()))));
        assert!(defs.needs_drop(&Ty::Map(Box::new(Ty::Str), Box::new(int()))));
        assert!(defs.needs_drop(&Ty::CString));
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
        let structs =
            [struct_def("Node", vec![field("next", Ty::Option(Box::new(node.clone())))], derives(true, true), None)];
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
        };
        let defs = TypeDefs { structs: &[], enums: &[with_string] };
        let ty = Ty::Enum(adt("E", 0));
        assert!(defs.needs_drop(&ty));
        assert!(!defs.is_copy(&ty));
        assert!(!defs.is_clone(&ty));
        assert!(!defs.has_drop_impl(&ty));
    }
}
