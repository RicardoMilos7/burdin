//! Exhaustiveness of patterns: the usefulness algorithm (Maranget, "Warnings for pattern matching") on a matrix of
//! patterns. A set of patterns is exhaustive when no value vector escapes every row.
//!
//! Integers and characters use ranges as constructors. A column of ranges is split at every range boundary into
//! segments that lie entirely inside or outside each range, and those segments are the constructors to try.

use burdin_tir::{FieldDef, Pat, Ty, TypeDefs, VariantShape};

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
    /// An array or slice of exactly this length. Only the lengths `seq_lengths` tries are ever used.
    Slice(usize),
}

#[derive(Clone, Debug)]
enum P {
    Wild,
    Ctor(Ctor, Vec<P>),
    Or(Vec<P>),
    /// An array or slice pattern: the elements before its rest, the elements after it, and whether it has a rest. A
    /// pattern without a rest has no suffix and names exactly its length (`spec/array-patterns.md`, AP8).
    Seq {
        prefix: Vec<P>,
        suffix: Vec<P>,
        rest: bool,
    },
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
        Pat::Product(fields) => P::Ctor(Ctor::Single, fields.iter().map(lower).collect()),
        Pat::Variant(i, fields) => P::Ctor(Ctor::Variant(*i), fields.iter().map(lower).collect()),
        Pat::Slice { prefix, rest, suffix } => P::Seq {
            prefix: prefix.iter().map(lower).collect(),
            suffix: suffix.iter().map(lower).collect(),
            rest: rest.is_some(),
        },
    }
}

/// Whether the patterns together match every value of `ty`.
pub fn exhaustive(ty: &Ty, pats: &[&Pat], adts: &TypeDefs) -> bool {
    let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![lower(p)]).collect();
    !missing(rows, std::slice::from_ref(ty), adts)
}

/// The values `pats` leave out, as patterns written in source form, in the order the type declares them: one for
/// each uncovered case (`Color.Green`, `Shape.Circle(_)`, `Option.Some(_)`, `(Color.Red, _)`). An open domain (an integer, a
/// string, a slice) that a pattern does not finish shows as `_`, so the list for a match on an integer is just `_`.
/// Empty when `pats` cover every value of `ty`.
pub fn missing_patterns(ty: &Ty, pats: &[&Pat], adts: &TypeDefs) -> Vec<String> {
    let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![lower(p)]).collect();
    let mut out: Vec<String> = Vec::new();
    for witness in witnesses(rows, std::slice::from_ref(ty), adts, true) {
        let text = witness.into_iter().next().unwrap_or_else(|| "_".to_string());
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// The vectors of patterns (one per type of `tys`) that match the values no row matches. `top` is whether `rows` is
/// the whole matrix: below the top, a matrix without rows is one wildcard per column, not every combination.
fn witnesses(rows: Vec<Vec<P>>, tys: &[Ty], adts: &TypeDefs, top: bool) -> Vec<Vec<String>> {
    let Some((first, rest)) = tys.split_first() else {
        return if rows.is_empty() { vec![Vec::new()] } else { Vec::new() };
    };
    let rows = expand_or(rows);
    if rows.is_empty() && !top && !matches!(first, Ty::Never | Ty::Error) {
        return vec![vec!["_".to_string(); tys.len()]];
    }
    if seq_lengths(first, &rows).is_some() {
        // Arrays and slices are not listed length by length: what is left out is one wildcard.
        return if missing(rows, tys, adts) { vec![vec!["_".to_string(); tys.len()]] } else { Vec::new() };
    }
    let heads: Vec<Ctor> =
        rows.iter().filter_map(|r| if let P::Ctor(c, _) = &r[0] { Some(c.clone()) } else { None }).collect();
    match finite_ctors(first, adts).or_else(|| complete_ctors(first, &heads, adts)) {
        Some(ctors) => {
            let mut out = Vec::new();
            for c in ctors {
                let mut sub = sub_tys(first, c.clone(), adts);
                let arity = sub.len();
                sub.extend_from_slice(rest);
                for mut w in witnesses(specialize(&rows, c.clone(), arity), &sub, adts, false) {
                    let tail = w.split_off(arity);
                    let mut full = vec![apply(first, &c, &w, adts)];
                    full.extend(tail);
                    out.push(full);
                }
            }
            out
        }
        None => {
            let default: Vec<Vec<P>> =
                rows.iter().filter(|r| matches!(r[0], P::Wild)).map(|r| r[1..].to_vec()).collect();
            let mut out = Vec::new();
            for mut w in witnesses(default, rest, adts, false) {
                w.insert(0, "_".to_string());
                out.push(w);
            }
            out
        }
    }
}

/// Every constructor of a type with a finite, listed set of them, in declaration order.
fn finite_ctors(ty: &Ty, adts: &TypeDefs) -> Option<Vec<Ctor>> {
    match ty {
        Ty::Unit | Ty::Tuple(_) | Ty::Struct(_) => Some(vec![Ctor::Single]),
        Ty::Never | Ty::Error => Some(Vec::new()),
        Ty::Bool => Some(vec![Ctor::Bool(false), Ctor::Bool(true)]),
        Ty::Enum(a) => Some((0..adts.enums[a.id].variants.len()).map(Ctor::Variant).collect()),
        _ => None,
    }
}

/// The last segment of a type's name, without type arguments: how a pattern names it.
fn pattern_name(name: &str) -> &str {
    let name = name.split('<').next().unwrap_or(name);
    name.rsplit('.').next().unwrap_or(name)
}

/// The pattern of constructor `c` of `ty` with `args` as its fields.
fn apply(ty: &Ty, c: &Ctor, args: &[String], adts: &TypeDefs) -> String {
    match (ty, c) {
        (Ty::Bool, Ctor::Bool(b)) => b.to_string(),
        (Ty::Tuple(_), Ctor::Single) => format!("({})", args.join(", ")),
        (Ty::Unit, Ctor::Single) => "()".to_string(),
        (Ty::Struct(a), Ctor::Single) => {
            let fields = &adts.structs[a.id].fields;
            format!("{} {{ {} }}", pattern_name(&a.name), named_fields(fields, args))
        }
        (Ty::Enum(a), Ctor::Variant(i)) => {
            let variant = &adts.enums[a.id].variants[*i];
            let name = format!("{}.{}", pattern_name(&a.name), variant.name);
            match variant.shape {
                VariantShape::Unit => name,
                VariantShape::Tuple => format!("{name}({})", args.join(", ")),
                VariantShape::Struct => format!("{name} {{ {} }}", named_fields(&variant.fields, args)),
            }
        }
        _ => "_".to_string(),
    }
}

fn named_fields(fields: &[FieldDef], args: &[String]) -> String {
    let named: Vec<String> = fields.iter().zip(args).map(|(f, a)| format!("{}: {a}", f.name)).collect();
    named.join(", ")
}

/// Whether some vector of values of types `tys` matches none of `rows`.
fn missing(rows: Vec<Vec<P>>, tys: &[Ty], adts: &TypeDefs) -> bool {
    let Some((first, rest)) = tys.split_first() else { return rows.is_empty() };
    let rows = expand_or(rows);
    if let Some(lengths) = seq_lengths(first, &rows) {
        return lengths.into_iter().any(|len| {
            let mut sub = sub_tys(first, Ctor::Slice(len), adts);
            sub.extend_from_slice(rest);
            missing(specialize_len(&rows, len), &sub, adts)
        });
    }
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

/// The lengths to try for an array or slice column, when its type is one (`spec/array-patterns.md`, AP8). An array has
/// one length. A slice is tried at every length up to one more than the longest pattern: a longer slice matches the
/// same rows as that last length, so it need not be tried on its own.
fn seq_lengths(ty: &Ty, rows: &[Vec<P>]) -> Option<Vec<usize>> {
    let seq_heads = rows.iter().any(|r| matches!(r[0], P::Seq { .. }));
    match ty {
        // An array is split into one column per element, so it is split only when some row matches it by a
        // sequence pattern. A column of wildcards (a large array no row destructures) takes the default path below.
        Ty::Array(_, n) => seq_heads.then(|| vec![*n as usize]),
        Ty::Slice(_) if !seq_heads => None,
        Ty::Slice(_) => {
            let longest = rows
                .iter()
                .filter_map(|r| match &r[0] {
                    P::Seq { prefix, suffix, .. } => Some(prefix.len() + suffix.len()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            Some((0..=longest + 1).collect())
        }
        _ => None,
    }
}

/// The subpatterns of an array or slice pattern at length `len`, or `None` when the pattern matches no such length.
fn seq_subs(prefix: &[P], suffix: &[P], rest: bool, len: usize) -> Option<Vec<P>> {
    if rest {
        let named = prefix.len() + suffix.len();
        if named > len {
            return None;
        }
        let mut out = prefix.to_vec();
        out.extend(std::iter::repeat_n(P::Wild, len - named));
        out.extend_from_slice(suffix);
        Some(out)
    } else {
        (prefix.len() == len).then(|| prefix.to_vec())
    }
}

/// The rows that match an array or slice of length `len`, with that length's subpatterns in place of the column.
fn specialize_len(rows: &[Vec<P>], len: usize) -> Vec<Vec<P>> {
    let mut out = Vec::new();
    for row in rows {
        let mut new_row = match &row[0] {
            P::Wild => vec![P::Wild; len],
            P::Seq { prefix, suffix, rest } => match seq_subs(prefix, suffix, *rest, len) {
                Some(subs) => subs,
                None => continue,
            },
            _ => continue,
        };
        new_row.extend_from_slice(&row[1..]);
        out.push(new_row);
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
        (Ty::Tuple(elems), Ctor::Single) => elems.clone(),
        (Ty::Struct(a), Ctor::Single) => adts.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
        (Ty::Array(t, _) | Ty::Slice(t), Ctor::Slice(len)) => vec![(**t).clone(); len],
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
        EnumDef { name: name.into(), variants, derives: Derives::default(), drop_method: None, lang: None }
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
            lang: None,
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

    /// A pattern `[prefix, .., suffix]` (or `[prefix, suffix]` without a rest) on an array or slice.
    fn seq(prefix: Vec<Pat>, rest: bool, suffix: Vec<Pat>) -> Pat {
        Pat::Slice { prefix, rest: rest.then(|| Box::new(Pat::Wild)), suffix }
    }

    #[test]
    fn slices_need_every_length() {
        let slice = Ty::Slice(Box::new(Ty::Bool));
        // `[]` alone leaves every longer slice out; `[..]` takes every length.
        assert!(!exhaustive_with(&slice, &[seq(vec![], false, vec![])]));
        assert!(exhaustive_with(&slice, &[seq(vec![], true, vec![])]));
        // `[_, ..]` and `[]` cover every length.
        assert!(exhaustive_with(&slice, &[seq(vec![Pat::Wild], true, vec![]), seq(vec![], false, vec![])]));
        // `[_]` and `[_, _, ..]` leave the empty slice out (AP8).
        let one = seq(vec![Pat::Wild], false, vec![]);
        let two_or_more = seq(vec![Pat::Wild, Pat::Wild], true, vec![]);
        assert!(!exhaustive_with(&slice, &[one.clone(), two_or_more.clone()]));
        assert!(exhaustive_with(&slice, &[seq(vec![], false, vec![]), one, two_or_more]));
    }

    #[test]
    fn a_slice_rest_with_named_elements_needs_the_shorter_lengths() {
        let slice = Ty::Slice(Box::new(Ty::Bool));
        // `[a, .., b]` matches only slices of two or more elements.
        let named = seq(vec![Pat::Bool(true)], true, vec![Pat::Bool(true)]);
        assert!(!exhaustive_with(&slice, std::slice::from_ref(&named)));
        // `[]` and `[_]` cover the short lengths, but `named` covers only some of the longer ones.
        assert!(!exhaustive_with(
            &slice,
            &[named.clone(), seq(vec![], false, vec![]), seq(vec![Pat::Wild], false, vec![])]
        ));
        // `[.., _]` matches every slice of one element or more, so with `[]` the whole type is covered.
        assert!(exhaustive_with(&slice, &[seq(vec![], false, vec![]), seq(vec![], true, vec![Pat::Wild])]));
    }

    #[test]
    fn an_array_pattern_names_its_length() {
        let array = Ty::Array(Box::new(Ty::Bool), 2);
        // The elements are checked one by one: `[true, _]` leaves `[false, _]` out.
        assert!(!exhaustive_with(&array, &[seq(vec![Pat::Bool(true), Pat::Wild], false, vec![])]));
        assert!(exhaustive_with(&array, &[seq(vec![Pat::Wild, Pat::Wild], false, vec![])]));
        // A rest on an array stands for the elements it does not name: `[true, ..]` and `[false, ..]` cover it.
        assert!(!exhaustive_with(&array, &[seq(vec![Pat::Bool(true)], true, vec![])]));
        assert!(exhaustive_with(
            &array,
            &[seq(vec![Pat::Bool(true)], true, vec![]), seq(vec![Pat::Bool(false)], true, vec![])],
        ));
        assert!(exhaustive_with(
            &array,
            &[
                seq(vec![Pat::Bool(true), Pat::Wild], false, vec![]),
                seq(vec![Pat::Bool(false), Pat::Wild], false, vec![]),
            ],
        ));
    }

    #[test]
    fn a_wildcard_column_of_a_large_array_is_not_split() {
        // A large array that no row destructures is not split into one column per element (`seq_lengths`): the
        // wildcard alone is exhaustive, and the length is never walked.
        let large = Ty::Array(Box::new(Ty::Bool), u64::MAX);
        assert!(exhaustive_with(&large, &[Pat::Wild]));
        assert!(!exhaustive_with(&large, &[]));
    }

    #[test]
    fn missing_patterns_name_each_uncovered_case() {
        let shape = enum_def(
            "Shape",
            vec![
                variant("Dot", vec![]),
                variant("Circle", vec![field("r", Ty::Int(IntTy::I32))]),
                variant("Rect", vec![field("w", Ty::Bool), field("h", Ty::Bool)]),
            ],
        );
        let enums = [shape];
        let defs = TypeDefs { structs: &[], enums: &enums };
        let ty = Ty::Enum(adt("Shape", 0));
        let dot = Pat::Variant(0, vec![]);
        assert_eq!(missing_patterns(&ty, &[&dot], &defs), ["Shape.Circle(_)", "Shape.Rect(_, _)"]);
        let rect_true = Pat::Variant(2, vec![Pat::Bool(true), Pat::Wild]);
        assert_eq!(
            missing_patterns(&ty, &[&rect_true], &defs),
            ["Shape.Dot", "Shape.Circle(_)", "Shape.Rect(false, _)"]
        );
        assert!(missing_patterns(&ty, &[&Pat::Wild], &defs).is_empty());
        let none: [&Pat; 0] = [];
        assert_eq!(missing_patterns(&Ty::Int(IntTy::I32), &none, &defs), ["_"]);
    }

    #[test]
    fn missing_patterns_of_tuples() {
        let defs = TypeDefs { structs: &[], enums: &[] };
        let pair = Ty::Tuple(vec![Ty::Bool, Ty::Bool]);
        let tt = Pat::Product(vec![Pat::Bool(true), Pat::Bool(true)]);
        assert_eq!(missing_patterns(&pair, &[&tt], &defs), ["(false, _)", "(true, false)"]);
    }
}
