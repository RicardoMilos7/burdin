//! Places: the expressions that name storage. Shared by the type checker, the move checker, and the borrow checker.

use crate::ops::LocalId;
use crate::tree::{Block, Expr, ExprKind, Stmt};
use crate::ty::Ty;

/// The local at the root of a place expression, looking through every field, index, and dereference.
pub fn place_root(e: &Expr) -> Option<LocalId> {
    match &e.kind {
        ExprKind::Local(id) => Some(*id),
        ExprKind::Index(base, _) | ExprKind::Field(base, _) | ExprKind::Deref(base) | ExprKind::DerefRef(base) => {
            place_root(base)
        }
        _ => None,
    }
}

/// Whether a place expression reaches through a reference or a slice, so its storage belongs to another value.
pub fn through_view(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::DerefRef(_) => true,
        ExprKind::Index(base, _) => through_view(base) || matches!(base.ty, Ty::Slice(_)),
        ExprKind::Field(base, _) => through_view(base),
        _ => false,
    }
}

/// Whether an expression is a place: a local followed by fields, indexing, and dereferences. A reference
/// dereference is always a place: it names the referent, whatever expression produced the reference.
pub fn is_place_expr(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) | ExprKind::DerefRef(_) => true,
        ExprKind::Field(base, _) | ExprKind::Index(base, _) | ExprKind::Deref(base) => is_place_expr(base),
        _ => false,
    }
}

/// A place as the memory checks compare them: the local at its root and the field path below it.
pub type Place = (LocalId, Vec<usize>);

/// The place an expression names: a root local and a field path, which stops at indexing, a `Heap`
/// dereference, or a reference dereference (memory-rules.md, place definition). The flag is false once the path
/// was cut, so a later field selection does not extend it.
pub fn tir_place(e: &Expr) -> Option<(LocalId, Vec<usize>, bool)> {
    match &e.kind {
        ExprKind::Local(id) => Some((*id, Vec::new(), true)),
        ExprKind::Field(base, i) => {
            let (root, mut path, direct) = tir_place(base)?;
            if direct {
                path.push(*i);
            }
            Some((root, path, direct))
        }
        ExprKind::Index(base, _) | ExprKind::Deref(base) | ExprKind::DerefRef(base) => {
            let (root, _, _) = tir_place(base)?;
            Some((root, Vec::new(), false))
        }
        _ => None,
    }
}

/// Every place an expression mentions (EX1). Index expressions and the parts of compound expressions are
/// visited too; a place on its own contributes its truncated place.
pub fn place_mentions(e: &Expr, out: &mut Vec<Place>) {
    use ExprKind as K;
    let record = |e: &Expr, out: &mut Vec<Place>| {
        if let Some((root, path, _)) = tir_place(e) {
            out.push((root, path));
        }
    };
    match &e.kind {
        K::Unit
        | K::Bool(_)
        | K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::Const(_)
        | K::None
        | K::Break
        | K::Continue => {}
        K::Local(_) => record(e, out),
        K::Field(base, _) | K::Deref(base) | K::DerefRef(base) => {
            record(e, out);
            if tir_place(base).is_none() {
                place_mentions(base, out);
            }
        }
        K::Index(base, index) => {
            record(e, out);
            place_mentions(index, out);
            if tir_place(base).is_none() {
                place_mentions(base, out);
            }
        }
        K::Ref { place, .. } => {
            record(place, out);
            if tir_place(place).is_none() {
                place_mentions(place, out);
            }
        }
        K::Slice { base, lo, hi } => {
            place_mentions(base, out);
            place_mentions(lo, out);
            place_mentions(hi, out);
        }
        K::StringAsStr(inner) | K::Some(inner) | K::Return(Some(inner)) | K::Cast(inner) => {
            place_mentions(inner, out);
        }
        K::Unary(_, inner) | K::ArrayRepeat(inner, _) => place_mentions(inner, out),
        K::Array(elems) | K::Tuple(elems) => elems.iter().for_each(|x| place_mentions(x, out)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, x)| place_mentions(x, out)),
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|x| place_mentions(x, out));
        }
        K::Binary(_, l, r) | K::Assign(l, r) => {
            place_mentions(l, out);
            place_mentions(r, out);
        }
        K::Block(b) => {
            for s in &b.stmts {
                match s {
                    Stmt::Let(_, init, _) => place_mentions(init, out),
                    Stmt::Expr(x) => place_mentions(x, out),
                }
            }
            if let Some(t) = &b.tail {
                place_mentions(t, out);
            }
        }
        K::If(c, then, other) => {
            place_mentions(c, out);
            place_mentions_block(then, out);
            if let Some(o) = other {
                place_mentions(o, out);
            }
        }
        K::While(c, body) => {
            place_mentions(c, out);
            place_mentions_block(body, out);
        }
        K::Loop(body) => place_mentions_block(body, out),
        K::ForRange { lo, hi, body, .. } => {
            place_mentions(lo, out);
            place_mentions(hi, out);
            place_mentions_block(body, out);
        }
        K::ForArray { array, body, .. } => {
            place_mentions(array, out);
            place_mentions_block(body, out);
        }
        K::ForSlice { slice, body, .. } => {
            place_mentions(slice, out);
            place_mentions_block(body, out);
        }
        K::Match(scrutinee, arms) => {
            place_mentions(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    place_mentions(g, out);
                }
                place_mentions(&arm.body, out);
            }
        }
        K::Return(None) => {}
    }
}

pub fn place_mentions_block(b: &Block, out: &mut Vec<Place>) {
    for s in &b.stmts {
        match s {
            Stmt::Let(_, init, _) => place_mentions(init, out),
            Stmt::Expr(x) => place_mentions(x, out),
        }
    }
    if let Some(t) = &b.tail {
        place_mentions(t, out);
    }
}

/// Two places overlap when one is a prefix of the other (EX2).
pub fn places_overlap(a: &(LocalId, Vec<usize>), b: &(LocalId, Vec<usize>)) -> bool {
    a.0 == b.0 && (a.1.starts_with(&b.1) || b.1.starts_with(&a.1))
}
