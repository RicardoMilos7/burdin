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
            // A shared reference and a view are copied; a mutable reference is not (RF4). Cloning a view
            // yields the same view.
            Ty::Ref { mutable, .. } => prop == Property::Clone || !*mutable,
            Ty::Slice(_) => true,
            // A raw C pointer is a plain address; a `CString` owns its buffer and has no clone yet.
            Ty::CPtr(_) => true,
            Ty::String | Ty::Heap(_) => prop == Property::Clone,
            Ty::CString | Ty::Param(_) => false,
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
            Ty::Ref { .. } | Ty::Slice(_) | Ty::Param(_) | Ty::CPtr(_) => false,
            Ty::String | Ty::Heap(_) | Ty::CString => true,
            Ty::Array(t, n) => *n > 0 && self.needs_drop_seen(t, seen),
            Ty::Option(t) => self.needs_drop_seen(t, seen),
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
