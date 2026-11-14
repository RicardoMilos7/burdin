//! Exhaustiveness of patterns: the usefulness algorithm (Maranget, "Warnings for pattern matching") on a matrix of
//! patterns. A set of patterns is exhaustive when no value vector escapes every row.
//!
//! Integers and characters use ranges as constructors. A column of ranges is split at every range boundary into
//! segments that lie entirely inside or outside each range, and those segments are the constructors to try.

use burdin_tir::{Pat, Ty, TypeDefs};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ctor {
    Bool(bool),
    /// An enum variant, or `None` (0) and `Some` (1) of `Option`.
    Variant(usize),
    /// The one constructor of a tuple, struct, or `()`.
    Single,
    /// An inclusive range of integers or character code points.
    Range(i128, i128),
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
        rows.iter().filter_map(|r| if let P::Ctor(c, _) = &r[0] { Some(*c) } else { None }).collect();
    match complete_ctors(first, &heads, adts) {
        Some(ctors) => ctors.into_iter().any(|c| {
            let mut sub = sub_tys(first, c, adts);
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

fn covers(outer: Ctor, inner: Ctor) -> bool {
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
            P::Ctor(rc, subs) if covers(*rc, c) => subs.clone(),
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
            let (min, max) = match ty {
                Ty::Int(i) => (i.min(), i.max()),
                _ => (0, 0x10FFFF),
            };
            let ranges: Vec<(i128, i128)> = heads
                .iter()
                .filter_map(|c| if let Ctor::Range(lo, hi) = c { Some((*lo, *hi)) } else { None })
                .collect();
            let mut points = vec![min, max + 1];
            for (lo, hi) in &ranges {
                points.push((*lo).clamp(min, max + 1));
                points.push((hi + 1).clamp(min, max + 1));
            }
            points.sort_unstable();
            points.dedup();
            let mut segments = Vec::new();
            for w in points.windows(2) {
                let (lo, hi) = (w[0], w[1] - 1);
                if !ranges.iter().any(|(a, b)| *a <= lo && hi <= *b) {
                    return None;
                }
                segments.push(Ctor::Range(lo, hi));
            }
            Some(segments)
        }
        _ => None,
    }
}
