//! Exhaustiveness of patterns: the usefulness algorithm (Maranget, "Warnings for pattern matching") on a matrix of
//! patterns. A set of patterns is exhaustive when no value vector escapes every row.
//!
//! Integers and characters use ranges as constructors. A column of ranges is split at every range boundary into
//! segments that lie entirely inside or outside each range, and those segments are the constructors to try.

use burdin_tir::{Pat, Ty, TypeDefs};

#[derive(Clone, Debug, PartialEq)]
enum Ctor {
    Bool(bool),
    /// An enum variant, or `None` (0) and `Some` (1) of `Option`.
    Variant(usize),
    /// The one constructor of a tuple, struct, or `()`.
    Single,
    /// An inclusive range of integers or character code points.
    Range(i128, i128),
    /// One string literal. The `str` domain is open, so its presence never makes a match exhaustive.
    Str(String),
}

#[derive(Clone, Debug)]
enum P {
    Wild,
    Ctor(Ctor, Vec<P>),
    Or(Vec<P>),
}

fn lower(p: &Pat) -> P {
    match p {
        Pat::Wild | Pat::Binding(..) => P::Wild,
        Pat::At(_, _, inner) => lower(inner),
        Pat::Bool(b) => P::Ctor(Ctor::Bool(*b), Vec::new()),
        Pat::Int(v) => P::Ctor(Ctor::Range(*v, *v), Vec::new()),
        Pat::Char(c) => P::Ctor(Ctor::Range(*c as i128, *c as i128), Vec::new()),
        Pat::Str(s) => P::Ctor(Ctor::Str(s.clone()), Vec::new()),
        Pat::Range(lo, hi) => P::Ctor(Ctor::Range(*lo, *hi), Vec::new()),
        Pat::Or(alts) => P::Or(alts.iter().map(lower).collect()),
        Pat::None => P::Ctor(Ctor::Variant(0), Vec::new()),
        Pat::Some(inner) => P::Ctor(Ctor::Variant(1), vec![lower(inner)]),
        Pat::Ok(inner) => P::Ctor(Ctor::Variant(0), vec![lower(inner)]),
        Pat::Err(inner) => P::Ctor(Ctor::Variant(1), vec![lower(inner)]),
        Pat::Product(fields) => P::Ctor(Ctor::Single, fields.iter().map(lower).collect()),
        Pat::Variant(i, fields) => P::Ctor(Ctor::Variant(*i), fields.iter().map(lower).collect()),
    }
}

/// Whether the patterns together match every value of `ty`.
pub fn exhaustive(ty: &Ty, pats: &[&Pat], adts: &TypeDefs) -> bool {
    let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![lower(p)]).collect();
    !missing(rows, std::slice::from_ref(ty), adts)
}

/// Whether some vector of values of types `tys` matches none of `rows`.
fn missing(rows: Vec<Vec<P>>, tys: &[Ty], adts: &TypeDefs) -> bool {
    let Some((first, rest)) = tys.split_first() else { return rows.is_empty() };
    let rows = expand_or(rows);
    let heads: Vec<Ctor> =
        rows.iter().filter_map(|r| if let P::Ctor(c, _) = &r[0] { Some(c.clone()) } else { None }).collect();
    match complete_ctors(first, &heads, adts) {
        Some(ctors) => ctors.into_iter().any(|c| {
            let mut sub = sub_tys(first, c.clone(), adts);
            let arity = sub.len();
            sub.extend_from_slice(rest);
            missing(specialize(&rows, c, arity), &sub, adts)
        }),
        None => {
            let default: Vec<Vec<P>> =
                rows.iter().filter(|r| matches!(r[0], P::Wild)).map(|r| r[1..].to_vec()).collect();
            missing(default, rest, adts)
        }
    }
}

/// Replaces every row whose first pattern is an or-pattern by one row per alternative.
fn expand_or(rows: Vec<Vec<P>>) -> Vec<Vec<P>> {
    let mut out = Vec::new();
    let mut work: Vec<Vec<P>> = rows.into_iter().rev().collect();
    while let Some(row) = work.pop() {
        match &row[0] {
            P::Or(alts) => {
                for alt in alts.iter().rev() {
                    let mut r = row.clone();
                    r[0] = alt.clone();
                    work.push(r);
                }
            }
            _ => out.push(row),
        }
    }
    out
}

fn covers(outer: &Ctor, inner: &Ctor) -> bool {
    match (outer, inner) {
        (Ctor::Range(a, b), Ctor::Range(x, y)) => a <= x && y <= b,
        _ => outer == inner,
    }
}

fn specialize(rows: &[Vec<P>], c: Ctor, arity: usize) -> Vec<Vec<P>> {
    let mut out = Vec::new();
    for row in rows {
        let mut new_row = match &row[0] {
            P::Wild => vec![P::Wild; arity],
            P::Ctor(rc, subs) if covers(rc, &c) => subs.clone(),
            _ => continue,
        };
        new_row.extend_from_slice(&row[1..]);
        out.push(new_row);
    }
    out
}

/// The types of the fields of constructor `c` of `ty`.
fn sub_tys(ty: &Ty, c: Ctor, adts: &TypeDefs) -> Vec<Ty> {
    match (ty, c) {
        (Ty::Option(inner), Ctor::Variant(1)) => vec![(**inner).clone()],
        (Ty::Result(ok, _), Ctor::Variant(0)) => vec![(**ok).clone()],
        (Ty::Result(_, err), Ctor::Variant(1)) => vec![(**err).clone()],
        (Ty::Tuple(elems), Ctor::Single) => elems.clone(),
        (Ty::Struct(a), Ctor::Single) => adts.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
        (Ty::Enum(a), Ctor::Variant(i)) => adts.enums[a.id].variants[i].fields.iter().map(|f| f.ty.clone()).collect(),
        _ => Vec::new(),
    }
}

/// If the constructors in `heads` cover every value of `ty`, the constructors to try (all of them, or the
/// segments of the split ranges). `None` when some value is left out, so only the default matrix matters.
fn complete_ctors(ty: &Ty, heads: &[Ctor], adts: &TypeDefs) -> Option<Vec<Ctor>> {
    let has = |c: Ctor| heads.contains(&c);
    match ty {
        Ty::Unit | Ty::Tuple(_) | Ty::Struct(_) => Some(vec![Ctor::Single]),
        // No values: nothing can be missing.
        Ty::Never | Ty::Error => Some(Vec::new()),
        Ty::Bool => {
            (has(Ctor::Bool(false)) && has(Ctor::Bool(true))).then(|| vec![Ctor::Bool(false), Ctor::Bool(true)])
        }
        Ty::Option(_) => {
            (has(Ctor::Variant(0)) && has(Ctor::Variant(1))).then(|| vec![Ctor::Variant(0), Ctor::Variant(1)])
        }
        Ty::Result(..) => {
            (has(Ctor::Variant(0)) && has(Ctor::Variant(1))).then(|| vec![Ctor::Variant(0), Ctor::Variant(1)])
        }
        Ty::Enum(a) => {
            let n = adts.enums[a.id].variants.len();
            (0..n).all(|i| has(Ctor::Variant(i))).then(|| (0..n).map(Ctor::Variant).collect())
        }
        Ty::Int(_) | Ty::Char => {
            // The sweep works on keys, which sort like the values (`IntTy::key`: `u128` bit patterns are not in
            // order as they are). A segment starts at the domain's start, at a range's start, or just after a
            // range's end, and it runs to the next start or to the domain's end. Keys are never added to past
            // the domain's end, so the top of `i128` needs no wider arithmetic.
            let key = |v: i128| match ty {
                Ty::Int(i) => i.key(v),
                _ => v,
            };
            let (min, max) = match ty {
                Ty::Int(i) => (i.key(i.min()), i.key(i.max())),
                _ => (0, 0x10FFFF),
            };
            let ranges: Vec<(i128, i128)> = heads
                .iter()
                .filter_map(|c| if let Ctor::Range(lo, hi) = c { Some((key(*lo), key(*hi))) } else { None })
                .collect();
            let mut starts = vec![min];
            for (lo, hi) in &ranges {
                starts.push((*lo).clamp(min, max));
                if *hi < max {
                    starts.push(hi + 1);
                }
            }
            starts.sort_unstable();
            starts.dedup();
            let mut segments = Vec::new();
            for (k, &lo) in starts.iter().enumerate() {
                let hi = starts.get(k + 1).map_or(max, |next| next - 1);
                if !ranges.iter().any(|(a, b)| *a <= lo && hi <= *b) {
                    return None;
                }
                // `key` is its own inverse, so the segment is in the value space again.
                segments.push(Ctor::Range(key(lo), key(hi)));
            }
            Some(segments)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::{FileId, Span};
    use burdin_tir::{AdtRef, Derives, EnumDef, FieldDef, IntTy, StructDef, VariantDef, VariantShape};

    fn span() -> Span {
        Span::new(FileId(0), 0, 0)
    }

    fn field(name: &str, ty: Ty) -> FieldDef {
        FieldDef { name: name.into(), ty, span: span(), is_priv: false }
    }

    fn variant(name: &str, fields: Vec<FieldDef>) -> VariantDef {
        let shape = if fields.is_empty() { VariantShape::Unit } else { VariantShape::Tuple };
        VariantDef { name: name.into(), shape, fields }
    }

    fn enum_def(name: &str, variants: Vec<VariantDef>) -> EnumDef {
        EnumDef { name: name.into(), variants, derives: Derives::default(), drop_method: None }
    }

    fn adt(name: &str, id: usize) -> AdtRef {
        AdtRef { id, name: name.into(), second_class: false }
    }

    fn exhaustive_with(ty: &Ty, pats: &[Pat]) -> bool {
        exhaustive_with_defs(ty, pats, &[], &[])
    }

    fn exhaustive_with_defs(ty: &Ty, pats: &[Pat], structs: &[StructDef], enums: &[EnumDef]) -> bool {
        let refs: Vec<&Pat> = pats.iter().collect();
        exhaustive(ty, &refs, &TypeDefs { structs, enums })
    }

    #[test]
    fn bool_needs_both_values() {
        assert!(!exhaustive_with(&Ty::Bool, &[Pat::Bool(true)]));
        assert!(!exhaustive_with(&Ty::Bool, &[Pat::Bool(false)]));
        assert!(exhaustive_with(&Ty::Bool, &[Pat::Bool(true), Pat::Bool(false)]));
        assert!(exhaustive_with(&Ty::Bool, &[Pat::Wild]));
    }

    #[test]
    fn option_needs_none_and_some() {
        let opt_bool = Ty::Option(Box::new(Ty::Bool));
        assert!(!exhaustive_with(&opt_bool, &[Pat::Some(Box::new(Pat::Wild))]));
        assert!(!exhaustive_with(&opt_bool, &[Pat::None]));
        assert!(exhaustive_with(&opt_bool, &[Pat::None, Pat::Some(Box::new(Pat::Wild))]));
    }

    #[test]
    fn nested_option_payloads_are_checked_too() {
        let opt_opt = Ty::Option(Box::new(Ty::Option(Box::new(Ty::Bool))));
        let some_some = || Pat::Some(Box::new(Pat::Some(Box::new(Pat::Wild))));
        // `Some(Some(_))` leaves `Some(None)` out.
        assert!(!exhaustive_with(&opt_opt, &[Pat::None, some_some()]));
        assert!(exhaustive_with(&opt_opt, &[Pat::None, Pat::Some(Box::new(Pat::None)), some_some()]));
    }

    #[test]
    fn result_needs_ok_and_err_with_every_payload() {
        let result = Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Int(IntTy::I32)));
        assert!(!exhaustive_with(&result, &[Pat::Ok(Box::new(Pat::Wild))]));
        assert!(exhaustive_with(&result, &[Pat::Ok(Box::new(Pat::Wild)), Pat::Err(Box::new(Pat::Wild))]));
        assert!(!exhaustive_with(&result, &[Pat::Ok(Box::new(Pat::Bool(true))), Pat::Err(Box::new(Pat::Wild))]));
        assert!(exhaustive_with(
            &result,
            &[Pat::Ok(Box::new(Pat::Bool(true))), Pat::Ok(Box::new(Pat::Bool(false))), Pat::Err(Box::new(Pat::Wild))]
        ));
    }

    #[test]
    fn integer_ranges_split_at_their_boundaries() {
        let i8_ = Ty::Int(IntTy::I8);
        assert!(!exhaustive_with(&i8_, &[Pat::Range(0, 127)]));
        assert!(exhaustive_with(&i8_, &[Pat::Range(-128, -1), Pat::Range(0, 127)]));
        assert!(exhaustive_with(&i8_, &[Pat::Range(-128, 127)]));
        // Overlapping ranges still cover every value.
        assert!(exhaustive_with(&i8_, &[Pat::Range(-128, 10), Pat::Range(5, 127)]));
        // One missing value is enough to make the match non-exhaustive.
        assert!(!exhaustive_with(&i8_, &[Pat::Range(-128, -1), Pat::Range(1, 127)]));
        let u8_ = Ty::Int(IntTy::U8);
        assert!(!exhaustive_with(&u8_, &[Pat::Range(0, 254)]));
        assert!(exhaustive_with(&u8_, &[Pat::Range(0, 255)]));
        // A wildcard covers whatever the ranges leave out.
        assert!(exhaustive_with(&i8_, &[Pat::Int(0), Pat::Wild]));
    }

    #[test]
    fn char_ranges_cover_the_code_point_space() {
        assert!(!exhaustive_with(&Ty::Char, &[Pat::Char('a')]));
        assert!(exhaustive_with(&Ty::Char, &[Pat::Range(0, 0x60), Pat::Range(0x61, 0x10FFFF)]));
        // 0x61 is 'a', so this leaves exactly one character out.
        assert!(!exhaustive_with(&Ty::Char, &[Pat::Range(0, 0x60), Pat::Range(0x62, 0x10FFFF)]));
        assert!(exhaustive_with(&Ty::Char, &[Pat::Char('a'), Pat::Wild]));
    }

    #[test]
    fn enum_variants_must_all_appear() {
        let color = enum_def("Color", vec![variant("Red", vec![]), variant("Green", vec![]), variant("Blue", vec![])]);
        let enums = [color];
        let ty = Ty::Enum(adt("Color", 0));
        assert!(!exhaustive_with_defs(&ty, &[Pat::Variant(0, vec![]), Pat::Variant(1, vec![])], &[], &enums));
        assert!(exhaustive_with_defs(
            &ty,
            &[Pat::Variant(0, vec![]), Pat::Variant(1, vec![]), Pat::Variant(2, vec![])],
            &[],
            &enums
        ));
        assert!(exhaustive_with_defs(&ty, &[Pat::Wild], &[], &enums));
    }

    #[test]
    fn enum_payloads_are_checked_per_variant() {
        let msg = enum_def(
            "Msg",
            vec![variant("Quit", vec![]), variant("Move", vec![field("x", Ty::Bool), field("y", Ty::Bool)])],
        );
        let enums = [msg];
        let ty = Ty::Enum(adt("Msg", 0));
        // `Move(true, _)` leaves `Move(false, _)` out.
        assert!(!exhaustive_with_defs(
            &ty,
            &[Pat::Variant(0, vec![]), Pat::Variant(1, vec![Pat::Bool(true), Pat::Wild])],
            &[],
            &enums
        ));
        assert!(exhaustive_with_defs(
            &ty,
            &[
                Pat::Variant(0, vec![]),
                Pat::Variant(1, vec![Pat::Bool(true), Pat::Wild]),
                Pat::Variant(1, vec![Pat::Bool(false), Pat::Wild])
            ],
            &[],
            &enums
        ));
    }

    #[test]
    fn tuples_need_every_combination() {
        let pair = Ty::Tuple(vec![Ty::Bool, Ty::Bool]);
        assert!(!exhaustive_with(&pair, &[Pat::Product(vec![Pat::Bool(true), Pat::Wild])]));
        assert!(exhaustive_with(
            &pair,
            &[Pat::Product(vec![Pat::Bool(true), Pat::Wild]), Pat::Product(vec![Pat::Bool(false), Pat::Wild])]
        ));
        assert!(exhaustive_with(&pair, &[Pat::Product(vec![Pat::Wild, Pat::Wild])]));
        // The rows above cover `(false, _)` and `(true, true)` only.
        assert!(!exhaustive_with(
            &pair,
            &[Pat::Product(vec![Pat::Bool(true), Pat::Bool(true)]), Pat::Product(vec![Pat::Bool(false), Pat::Wild])]
        ));
    }

    #[test]
    fn struct_fields_are_checked_like_a_tuple() {
        let def = StructDef {
            name: "S".into(),
            fields: vec![field("a", Ty::Bool), field("b", Ty::Bool)],
            derives: Derives::default(),
            drop_method: None,
            repr_c: false,
        };
        let structs = [def];
        let ty = Ty::Struct(adt("S", 0));
        assert!(!exhaustive_with_defs(&ty, &[Pat::Product(vec![Pat::Bool(true), Pat::Wild])], &structs, &[]));
        assert!(exhaustive_with_defs(&ty, &[Pat::Product(vec![Pat::Wild, Pat::Wild])], &structs, &[]));
    }

    #[test]
    fn or_patterns_expand_to_one_row_per_alternative() {
        let i8_ = Ty::Int(IntTy::I8);
        assert!(exhaustive_with(&i8_, &[Pat::Or(vec![Pat::Range(-128, -1), Pat::Range(0, 127)])]));
        assert!(!exhaustive_with(&i8_, &[Pat::Or(vec![Pat::Int(1), Pat::Int(2)])]));
        assert!(exhaustive_with(&Ty::Bool, &[Pat::Or(vec![Pat::Bool(true), Pat::Bool(false)])]));
    }

    #[test]
    fn strings_need_a_wildcard() {
        assert!(!exhaustive_with(&Ty::Str, &[Pat::Str("a".into())]));
        assert!(!exhaustive_with(&Ty::Str, &[Pat::Str("a".into()), Pat::Str("b".into())]));
        assert!(exhaustive_with(&Ty::Str, &[Pat::Str("a".into()), Pat::Wild]));
    }

    #[test]
    fn empty_types_and_the_unit_type() {
        // A `!` or an error-typed value has no values, so a match on it needs no arms.
        assert!(exhaustive_with(&Ty::Never, &[]));
        assert!(exhaustive_with(&Ty::Error, &[]));
        // `()` has exactly one value, which the empty product pattern names.
        assert!(!exhaustive_with(&Ty::Unit, &[]));
        assert!(exhaustive_with(&Ty::Unit, &[Pat::Product(vec![])]));
        assert!(exhaustive_with(&Ty::Unit, &[Pat::Wild]));
    }
}
