//! The type rule `const` values follow (`spec/const-eval.md`, CE3): which types can cross from compile-time
//! evaluation into the program.

use crate::globals::Globals;
use burdin_tir::Ty;

impl Globals {
    /// Whether a value of `ty` can be materialized as literals in the program (CE3): scalars, `str`, `String`,
    /// and tuples, arrays, structs, enums, `Option`, `Result`, and ranges built from them. `Never` and `Error`
    /// are accepted so that a diverging or already-reported expression adds no second error.
    pub(crate) fn materializable(&self, ty: &Ty) -> bool {
        self.materializable_in(ty, &mut Vec::new())
    }

    fn materializable_in(&self, ty: &Ty, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit
            | Ty::Bool
            | Ty::Char
            | Ty::Int(_)
            | Ty::Float(_)
            | Ty::Str
            | Ty::String
            | Ty::Never
            | Ty::Error => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.materializable_in(t, seen)),
            Ty::Array(t, _) => self.materializable_in(t, seen),
            Ty::Struct(a) | Ty::Enum(a) => {
                let key = (matches!(ty, Ty::Enum(_)), a.id);
                // A type that reaches itself does so through a `Heap` or a `List`, which is rejected there.
                if seen.contains(&key) {
                    return true;
                }
                seen.push(key);
                let ok = if key.0 {
                    self.enums[a.id]
                        .variants
                        .iter()
                        .all(|v| v.fields.iter().all(|f| self.materializable_in(&f.ty, seen)))
                } else {
                    self.structs[a.id].fields.iter().all(|f| self.materializable_in(&f.ty, seen))
                };
                seen.pop();
                ok
            }
            Ty::Fn { .. }
            | Ty::Ref { .. }
            | Ty::Slice(_)
            | Ty::Map(..)
            | Ty::MapEntry(..)
            | Ty::Chars
            | Ty::Heap(_, _)
            | Ty::Any(_)
            | Ty::CPtr(_)
            | Ty::MapIter(..)
            | Ty::IterMut(_)
            | Ty::MapIterMut(..)
            | Ty::MapIntoIter(..)
            | Ty::SliceChunks(..)
            | Ty::SliceWindows(..)
            | Ty::Param(_)
            | Ty::Const(_)
            | Ty::Projection { .. }
            | Ty::TryFromError { .. } => false,
        }
    }
}

impl Globals {
    /// Whether a value of `ty` is plain data (`spec/globals.md`, GL2): it owns no memory and has no destructor, so a
    /// static of this type is never dropped. A `String`, a collection, a handle, and a closure are not plain.
    pub(crate) fn plain(&self, ty: &Ty) -> bool {
        self.plain_in(ty, &mut Vec::new())
    }

    fn plain_in(&self, ty: &Ty, seen: &mut Vec<(bool, usize)>) -> bool {
        match ty {
            Ty::Unit | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Str | Ty::Never | Ty::Error => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.plain_in(t, seen)),
            Ty::Array(t, _) => self.plain_in(t, seen),
            Ty::Struct(a) | Ty::Enum(a) => {
                let is_enum = matches!(ty, Ty::Enum(_));
                let key = (is_enum, a.id);
                // A recursive type reaches itself through a `Heap` or a `List`, which are not plain already.
                if seen.contains(&key) {
                    return true;
                }
                let has_drop = if is_enum {
                    self.enums[a.id].drop_method.is_some()
                } else {
                    self.structs[a.id].drop_method.is_some()
                };
                if has_drop {
                    return false;
                }
                seen.push(key);
                let ok = if is_enum {
                    self.enums[a.id].variants.iter().all(|v| v.fields.iter().all(|f| self.plain_in(&f.ty, seen)))
                } else {
                    self.structs[a.id].fields.iter().all(|f| self.plain_in(&f.ty, seen))
                };
                seen.pop();
                ok
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_tir::IntTy;

    #[test]
    fn scalars_and_their_aggregates_are_materializable() {
        let mut g = crate::testutil::lang_globals();
        let i = Ty::Int(IntTy::I32);
        assert!(g.materializable(&i));
        assert!(g.materializable(&Ty::Array(Box::new(i.clone()), 4)));
        assert!(g.materializable(&Ty::Tuple(vec![Ty::Str, Ty::String, Ty::Bool])));
        let option = g.test_option(i.clone());
        let result = g.test_result(option, Ty::Char);
        assert!(g.materializable(&result));
    }

    #[test]
    fn heap_values_and_views_are_not() {
        let mut g = crate::testutil::lang_globals();
        let i = Ty::Int(IntTy::I32);
        assert!(!g.materializable(&Ty::Heap(Box::new(i.clone()), burdin_tir::Alloc::Global)));
        assert!(!g.materializable(&Ty::Slice(Box::new(i.clone()))));
        assert!(!g.materializable(&Ty::Ref { mutable: false, inner: Box::new(i.clone()) }));
        let option = g.test_option(Ty::Heap(Box::new(i), burdin_tir::Alloc::Global));
        assert!(!g.materializable(&option));
    }
}
