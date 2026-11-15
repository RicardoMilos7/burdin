//! Places: the expressions that name storage. Shared by the type checker, the move checker, and the borrow checker.

use crate::ops::LocalId;
use crate::tree::{Block, Expr, ExprKind, Stmt};
use crate::ty::Ty;

/// The local at the root of a place expression, looking through every field, index, and dereference.
pub fn place_root(e: &Expr) -> Option<LocalId> {
    match &e.kind {
        ExprKind::Local(id) => Some(*id),
        ExprKind::Index(base, _)
        | ExprKind::Field(base, _)
        | ExprKind::Deref(base)
        | ExprKind::PtrDeref(base)
        | ExprKind::DerefRef(base) => place_root(base),
        _ => None,
    }
}

/// Whether a place expression reaches through a reference or a slice, so its storage belongs to another value.
pub fn through_view(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::DerefRef(_) | ExprKind::PtrDeref(_) => true,
        ExprKind::Index(base, _) => through_view(base) || matches!(base.ty, Ty::Slice(_)),
        ExprKind::Field(base, _) => through_view(base),
        _ => false,
    }
}

/// Whether an expression is a place: a local followed by fields, indexing, and dereferences. A reference
/// dereference is always a place: it names the referent, whatever expression produced the reference.
pub fn is_place_expr(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Local(_) | ExprKind::DerefRef(_) | ExprKind::PtrDeref(_) => true,
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
        ExprKind::Index(base, _) | ExprKind::Deref(base) | ExprKind::PtrDeref(base) | ExprKind::DerefRef(base) => {
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
        | K::Continue
        | K::LayoutOf { .. } => {}
        K::Local(_) => record(e, out),
        K::Field(base, _) | K::Deref(base) | K::PtrDeref(base) | K::DerefRef(base) => {
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
        K::Slice { base, lo, hi, .. } => {
            place_mentions(base, out);
            place_mentions(lo, out);
            place_mentions(hi, out);
        }
        K::StringAsStr(inner)
        | K::Some(inner)
        | K::Ok(inner)
        | K::Err(inner)
        | K::Return(Some(inner))
        | K::Cast(inner) => {
            place_mentions(inner, out);
        }
        K::Unary(_, inner) | K::ArrayRepeat(inner, _) => place_mentions(inner, out),
        K::Array(elems) | K::Tuple(elems) => elems.iter().for_each(|x| place_mentions(x, out)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, x)| place_mentions(x, out)),
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|x| place_mentions(x, out));
        }
        // A closure mentions the places it captures; a closure call mentions its callee and arguments.
        K::Closure { captures, .. } => {
            for c in captures {
                out.push((c.local, Vec::new()));
            }
        }
        K::ClosureCall { callee, args, .. } => {
            place_mentions(callee, out);
            args.iter().for_each(|x| place_mentions(x, out));
        }
        K::Binary(_, l, r) | K::Assign(l, r) => {
            place_mentions(l, out);
            place_mentions(r, out);
        }
        K::Range { lo, hi, .. } => {
            place_mentions(lo, out);
            place_mentions(hi, out);
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
        K::ForRangeValue { range, body, .. } => {
            place_mentions(range, out);
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
        K::ForList { list, body, .. } => {
            place_mentions(list, out);
            place_mentions_block(body, out);
        }
        K::ForMap { map, body, .. } => {
            place_mentions(map, out);
            place_mentions_block(body, out);
        }
        K::ForChars { string, body, .. } => {
            place_mentions(string, out);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::FuncId;
    use crate::ty::IntTy;
    use burdin_diag::{FileId, Span};

    fn span() -> Span {
        Span::new(FileId(0), 0, 0)
    }

    fn i32_() -> Ty {
        Ty::Int(IntTy::I32)
    }

    fn local(id: usize, ty: Ty) -> Expr {
        Expr { kind: ExprKind::Local(LocalId(id)), ty, span: span() }
    }

    fn field(base: Expr, index: usize) -> Expr {
        let ty = base.ty.clone();
        Expr { kind: ExprKind::Field(Box::new(base), index), ty, span: span() }
    }

    fn index(base: Expr, index: Expr) -> Expr {
        let ty = match &base.ty {
            Ty::Array(t, _) | Ty::Slice(t) => (**t).clone(),
            _ => Ty::Error,
        };
        Expr { kind: ExprKind::Index(Box::new(base), Box::new(index)), ty, span: span() }
    }

    fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem), 3)
    }

    fn deref(base: Expr) -> Expr {
        Expr { kind: ExprKind::Deref(Box::new(base)), ty: i32_(), span: span() }
    }

    fn deref_ref(base: Expr) -> Expr {
        Expr { kind: ExprKind::DerefRef(Box::new(base)), ty: i32_(), span: span() }
    }

    fn call(args: Vec<Expr>) -> Expr {
        Expr { kind: ExprKind::Call(FuncId(0), args), ty: Ty::Unit, span: span() }
    }

    #[test]
    fn place_roots_look_through_fields_indexes_and_derefs() {
        assert_eq!(place_root(&local(0, i32_())), Some(LocalId(0)));
        assert_eq!(place_root(&field(field(local(0, i32_()), 1), 2)), Some(LocalId(0)));
        assert_eq!(place_root(&index(local(1, array(i32_())), local(2, i32_()))), Some(LocalId(1)));
        assert_eq!(place_root(&deref(local(3, Ty::CPtr(Box::new(i32_()))))), Some(LocalId(3)));
        assert_eq!(
            place_root(&deref_ref(local(4, Ty::Ref { mutable: false, inner: Box::new(i32_()) }))),
            Some(LocalId(4))
        );
        assert_eq!(place_root(&call(vec![local(5, i32_())])), None);
    }

    #[test]
    fn tir_places_are_a_root_and_a_direct_field_path() {
        assert_eq!(tir_place(&local(0, i32_())), Some((LocalId(0), vec![], true)));
        assert_eq!(tir_place(&field(field(local(0, i32_()), 2), 1)), Some((LocalId(0), vec![2, 1], true)));
        // Indexing and dereferences cut the path: a later field does not extend it.
        let indexed = index(local(0, array(i32_())), local(1, i32_()));
        assert_eq!(tir_place(&indexed), Some((LocalId(0), vec![], false)));
        assert_eq!(tir_place(&field(indexed, 3)), Some((LocalId(0), vec![], false)));
        assert_eq!(tir_place(&deref(local(2, Ty::CPtr(Box::new(i32_()))))), Some((LocalId(2), vec![], false)));
        assert_eq!(
            tir_place(&deref_ref(local(3, Ty::Ref { mutable: true, inner: Box::new(i32_()) }))),
            Some((LocalId(3), vec![], false))
        );
        assert_eq!(tir_place(&call(vec![])), None);
    }

    #[test]
    fn place_expressions_are_locals_fields_indexes_and_derefs() {
        assert!(is_place_expr(&local(0, array(i32_()))));
        assert!(is_place_expr(&field(local(0, array(i32_())), 0)));
        assert!(is_place_expr(&index(local(0, array(i32_())), local(1, i32_()))));
        assert!(is_place_expr(&deref(local(0, array(i32_())))));
        assert!(is_place_expr(&deref_ref(local(0, array(i32_())))));
        assert!(!is_place_expr(&call(vec![local(0, i32_())])));
        // `&place` produces a reference value; it does not itself name storage.
        let borrowed = Expr {
            kind: ExprKind::Ref { place: Box::new(local(0, array(i32_()))), mutable: false },
            ty: Ty::Ref { mutable: false, inner: Box::new(array(i32_())) },
            span: span(),
        };
        assert!(!is_place_expr(&borrowed));
    }

    #[test]
    fn views_reach_through_references_and_slices_only() {
        assert!(!through_view(&local(0, i32_())));
        assert!(!through_view(&deref(local(0, i32_()))));
        assert!(through_view(&deref_ref(local(0, i32_()))));
        assert!(through_view(&field(deref_ref(local(0, i32_())), 0)));
        assert!(!through_view(&index(local(2, array(i32_())), local(1, i32_()))));
        assert!(through_view(&index(local(3, Ty::Slice(Box::new(i32_()))), local(1, i32_()))));
    }

    #[test]
    fn overlap_is_prefix_based_within_one_local() {
        assert!(places_overlap(&(LocalId(0), vec![]), &(LocalId(0), vec![1])));
        assert!(places_overlap(&(LocalId(0), vec![1]), &(LocalId(0), vec![1, 2])));
        assert!(places_overlap(&(LocalId(0), vec![2]), &(LocalId(0), vec![2])));
        assert!(!places_overlap(&(LocalId(0), vec![1]), &(LocalId(0), vec![2])));
        assert!(!places_overlap(&(LocalId(0), vec![]), &(LocalId(1), vec![])));
    }

    #[test]
    fn place_mentions_collects_every_place_an_expression_names() {
        let arg = field(local(0, i32_()), 1);
        let indexed = index(local(1, array(i32_())), local(2, i32_()));
        let mut out = Vec::new();
        place_mentions(&call(vec![arg, indexed]), &mut out);
        assert_eq!(out, vec![(LocalId(0), vec![1]), (LocalId(1), vec![]), (LocalId(2), vec![])]);
    }
}
