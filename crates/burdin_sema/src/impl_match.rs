//! Which instances an impl block applies to: `Stack<i32> { ... }`, `Pair<i32, B> { ... }`, `Holder<List<T>> { ... }`.
//!
//! An impl on a generic type writes the type's arguments as a *pattern*: a type with the impl's own parameter names
//! standing for what they match (`spec/impls.md`, IM1). Matching a pattern against an instance's arguments says
//! whether the impl applies and what its parameters stand for; comparing two patterns says whether the impls
//! overlap (IM4).

use crate::consteval::SYM_LEN;
use crate::globals::Globals;
use burdin_tir::Ty;

/// A parameter bound while two patterns are unified, with the side it belongs to: side 0 is the first pattern, side 1
/// the second, so `T` of one impl is a different name from `T` of the other.
type SideBinding = ((u8, String), (u8, Ty));

/// The part of a type that decides which type it is, and the types it is made of: `Option<i32>` is `("Option",
/// [i32])`, a generic struct instance is its template and its arguments. Types with no such structure (numbers,
/// `str`, a struct that is not generic) have none and compare by equality.
fn shape(globals: &Globals, ty: &Ty) -> Option<(String, Vec<Ty>)> {
    Some(match ty {
        Ty::Ref { mutable, inner } => (format!("ref{mutable}"), vec![(**inner).clone()]),
        Ty::Slice(t) => ("slice".into(), vec![(**t).clone()]),
        Ty::Heap(t, _) => ("heap".into(), vec![(**t).clone()]),
        Ty::CPtr(t) => ("cptr".into(), vec![(**t).clone()]),
        Ty::Map(k, v, _) => ("map".into(), vec![(**k).clone(), (**v).clone()]),
        Ty::MapEntry(k, v, _) => ("map_entry".into(), vec![(**k).clone(), (**v).clone()]),
        Ty::Tuple(ts) => (format!("tuple{}", ts.len()), ts.clone()),
        Ty::Array(t, n) if *n < SYM_LEN => (format!("array{n}"), vec![(**t).clone()]),
        Ty::Array(t, _) => ("array".into(), vec![(**t).clone()]),
        Ty::Struct(_) | Ty::Enum(_) => {
            let (template, args) = globals.origin_of(ty)?;
            (format!("adt{template}{}", if matches!(ty, Ty::Enum(_)) { "e" } else { "s" }), args.clone())
        }
        _ => return None,
    })
}

/// Whether a pattern is a list of distinct parameters (`Stack<T>`, `Pair<A, B>`): the impl applies to every instance
/// of the type, so it is checked once, on the type's identity instance.
pub(crate) fn is_plain_pattern(args: &[Ty]) -> bool {
    args.iter().enumerate().all(|(i, a)| matches!(a, Ty::Param(n) if !args[..i].contains(&Ty::Param(n.clone()))))
}

impl Globals {
    /// What the parameters of an impl stand for when its argument pattern is matched against the arguments of an
    /// instance, or `None` when the impl does not apply to the instance.
    pub(crate) fn match_impl_args(
        &self,
        params: &[String],
        pattern: &[Ty],
        actual: &[Ty],
    ) -> Option<Vec<(String, Ty)>> {
        if pattern.len() != actual.len() {
            return None;
        }
        let mut bindings = Vec::new();
        for (p, a) in pattern.iter().zip(actual) {
            if !self.match_pattern(p, a, &mut bindings) {
                return None;
            }
        }
        params.iter().map(|n| bindings.iter().find(|(m, _)| m == n).cloned()).collect()
    }

    fn match_pattern(&self, pat: &Ty, act: &Ty, bindings: &mut Vec<(String, Ty)>) -> bool {
        if let Ty::Param(n) = pat {
            return match bindings.iter().find(|(m, _)| m == &**n) {
                Some((_, bound)) => bound == act,
                None => {
                    bindings.push((n.to_string(), act.clone()));
                    true
                }
            };
        }
        if let (Ty::Array(p, pn), Ty::Array(a, an)) = (pat, act) {
            if *pn >= SYM_LEN {
                self.unify_len(*pn, *an, bindings);
            } else if pn != an {
                return false;
            }
            return self.match_pattern(p, a, bindings);
        }
        match (shape(self, pat), shape(self, act)) {
            (Some((ph, pc)), Some((ah, ac))) => {
                ph == ah && pc.len() == ac.len() && pc.iter().zip(&ac).all(|(p, a)| self.match_pattern(p, a, bindings))
            }
            (None, None) => pat == act,
            _ => false,
        }
    }

    /// Whether some instance matches both impls' argument patterns, so that both would apply to it. The `where`
    /// clauses are not looked at: as in Rust, two impls whose bounds differ still overlap.
    pub(crate) fn impl_patterns_overlap(&self, first: &[Ty], second: &[Ty]) -> bool {
        // A parameter is told apart by the pattern it belongs to: side 0 is `first`, side 1 is `second`.
        let mut bindings: Vec<SideBinding> = Vec::new();
        first.len() == second.len()
            && first.iter().zip(second).all(|(a, b)| self.unify_sides(a, 0, b, 1, &mut bindings))
    }

    fn resolve_side(&self, ty: &Ty, side: u8, bindings: &[SideBinding]) -> (Ty, u8) {
        let mut current = (ty.clone(), side);
        while let Ty::Param(n) = &current.0 {
            match bindings.iter().find(|(var, _)| var.0 == current.1 && var.1 == **n) {
                Some((_, next)) => current = (next.1.clone(), next.0),
                None => break,
            }
        }
        current
    }

    fn occurs(&self, var: &(u8, String), ty: &Ty, side: u8, bindings: &[SideBinding]) -> bool {
        let (ty, side) = self.resolve_side(ty, side, bindings);
        if let Ty::Param(n) = &ty {
            return side == var.0 && **n == var.1;
        }
        match shape(self, &ty) {
            Some((_, children)) => children.iter().any(|c| self.occurs(var, c, side, bindings)),
            None => false,
        }
    }

    fn unify_sides(&self, a: &Ty, sa: u8, b: &Ty, sb: u8, bindings: &mut Vec<SideBinding>) -> bool {
        let (a, sa) = self.resolve_side(a, sa, bindings);
        let (b, sb) = self.resolve_side(b, sb, bindings);
        match (&a, &b) {
            (Ty::Param(x), Ty::Param(y)) if sa == sb && x == y => true,
            (Ty::Param(x), _) => {
                let var = (sa, x.to_string());
                if self.occurs(&var, &b, sb, bindings) {
                    return false;
                }
                bindings.push((var, (sb, b)));
                true
            }
            (_, Ty::Param(y)) => {
                let var = (sb, y.to_string());
                if self.occurs(&var, &a, sa, bindings) {
                    return false;
                }
                bindings.push((var, (sa, a)));
                true
            }
            // Two arrays may have a length no parameter fixes yet, so only two known lengths can tell them apart.
            (Ty::Array(x, xn), Ty::Array(y, yn)) => {
                (*xn >= SYM_LEN || *yn >= SYM_LEN || xn == yn) && self.unify_sides(x, sa, y, sb, bindings)
            }
            _ => match (shape(self, &a), shape(self, &b)) {
                (Some((ah, ac)), Some((bh, bc))) => {
                    ah == bh
                        && ac.len() == bc.len()
                        && ac.iter().zip(&bc).all(|(x, y)| self.unify_sides(x, sa, y, sb, bindings))
                }
                (None, None) => a == b,
                _ => false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generics::param_ty;
    use burdin_tir::IntTy;

    fn param(name: &str) -> Ty {
        param_ty(name)
    }

    fn i32_() -> Ty {
        Ty::Int(IntTy::I32)
    }

    fn list(ty: Ty) -> Ty {
        Ty::Heap(Box::new(ty), burdin_tir::Alloc::Global)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_concrete_pattern_matches_only_its_own_arguments() {
        let g = Globals::default();
        assert_eq!(g.match_impl_args(&[], &[i32_()], &[i32_()]), Some(vec![]));
        assert_eq!(g.match_impl_args(&[], &[i32_()], &[Ty::Bool]), None);
        assert_eq!(g.match_impl_args(&[], &[i32_()], &[param("T")]), None);
    }

    #[test]
    fn a_name_matches_anything_and_nested_names_bind() {
        let mut g = crate::testutil::lang_globals();
        assert_eq!(
            g.match_impl_args(&names(&["T"]), &[param("T")], &[Ty::Bool]),
            Some(vec![("T".to_string(), Ty::Bool)])
        );
        assert_eq!(
            g.match_impl_args(&names(&["T"]), &[list(param("T"))], &[list(i32_())]),
            Some(vec![("T".to_string(), i32_())])
        );
        let option = g.test_option(i32_());
        assert_eq!(g.match_impl_args(&names(&["T"]), &[list(param("T"))], &[option]), None);
    }

    #[test]
    fn mixed_patterns_bind_the_names_in_parameter_order() {
        let g = Globals::default();
        let pattern = [i32_(), param("B")];
        assert_eq!(
            g.match_impl_args(&names(&["B"]), &pattern, &[i32_(), Ty::Bool]),
            Some(vec![("B".to_string(), Ty::Bool)])
        );
        assert_eq!(g.match_impl_args(&names(&["B"]), &pattern, &[Ty::Bool, Ty::Bool]), None);
    }

    #[test]
    fn a_name_that_repeats_must_match_the_same_type() {
        let g = Globals::default();
        let pattern = [param("T"), param("T")];
        assert!(g.match_impl_args(&names(&["T"]), &pattern, &[i32_(), i32_()]).is_some());
        assert_eq!(g.match_impl_args(&names(&["T"]), &pattern, &[i32_(), Ty::Bool]), None);
    }

    #[test]
    fn plain_patterns_are_distinct_names() {
        assert!(is_plain_pattern(&[param("A"), param("B")]));
        assert!(!is_plain_pattern(&[param("A"), param("A")]));
        assert!(!is_plain_pattern(&[i32_(), param("B")]));
        assert!(!is_plain_pattern(&[list(param("T"))]));
    }

    #[test]
    fn patterns_overlap_when_one_instance_matches_both() {
        let mut g = crate::testutil::lang_globals();
        // `Pair<i32, B>` and `Pair<A, i32>` both apply to `Pair<i32, i32>`.
        assert!(g.impl_patterns_overlap(&[i32_(), param("B")], &[param("A"), i32_()]));
        // A name overlaps everything, a concrete type only itself.
        assert!(g.impl_patterns_overlap(&[param("T")], &[i32_()]));
        assert!(!g.impl_patterns_overlap(&[i32_()], &[Ty::Bool]));
        let option = g.test_option(param("T"));
        assert!(!g.impl_patterns_overlap(&[list(param("T"))], &[option]));
        assert!(g.impl_patterns_overlap(&[list(param("T"))], &[list(i32_())]));
    }

    #[test]
    fn a_name_cannot_contain_itself() {
        let g = Globals::default();
        // `(T, List<T>)` and `(U, U)` would need `T = List<T>`.
        assert!(!g.impl_patterns_overlap(&[param("T"), list(param("T"))], &[param("U"), param("U")]));
        // The same name in two impls is two different names.
        assert!(g.impl_patterns_overlap(&[param("T"), i32_()], &[i32_(), param("T")]));
    }
}
