//! The type rule comptime values follow (`spec/comptime.md`, CT3): which types can cross from compile-time
//! evaluation into the program.

use crate::globals::Globals;
use burdin_tir::Ty;

impl Globals {
    /// Whether a value of `ty` can be materialized as literals in the program (CT3): scalars, `str`, `String`,
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
            Ty::Array(t, _) | Ty::Option(t) | Ty::Range(t, _) => self.materializable_in(t, seen),
            Ty::Result(t, e) => self.materializable_in(t, seen) && self.materializable_in(e, seen),
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
            | Ty::List(_)
            | Ty::Map(..)
            | Ty::Chars
            | Ty::Heap(_)
            | Ty::Any(_)
            | Ty::CPtr(_)
            | Ty::CString
            | Ty::ListIntoIter(_)
            | Ty::ArrayIntoIter(..)
            | Ty::MapIter(..)
            | Ty::MapIntoIter(..)
            | Ty::RangeIter(..)
            | Ty::Param(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_tir::IntTy;

    #[test]
    fn scalars_and_their_aggregates_are_materializable() {
        let g = Globals::default();
        let i = Ty::Int(IntTy::I32);
        assert!(g.materializable(&i));
        assert!(g.materializable(&Ty::Array(Box::new(i.clone()), 4)));
        assert!(g.materializable(&Ty::Tuple(vec![Ty::Str, Ty::String, Ty::Bool])));
        assert!(g.materializable(&Ty::Result(Box::new(Ty::Option(Box::new(i.clone()))), Box::new(Ty::Char))));
    }

    #[test]
    fn heap_values_and_views_are_not() {
        let g = Globals::default();
        let i = Ty::Int(IntTy::I32);
        assert!(!g.materializable(&Ty::List(Box::new(i.clone()))));
        assert!(!g.materializable(&Ty::Heap(Box::new(i.clone()))));
        assert!(!g.materializable(&Ty::Slice(Box::new(i.clone()))));
        assert!(!g.materializable(&Ty::Ref { mutable: false, inner: Box::new(i.clone()) }));
        assert!(!g.materializable(&Ty::Option(Box::new(Ty::List(Box::new(i))))));
    }
}
