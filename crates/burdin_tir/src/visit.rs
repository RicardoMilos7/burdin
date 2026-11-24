//! Walking the typed tree. [`Visit`] reads it and [`VisitMut`] rewrites it in place; each method's default walks
//! into the node's children, so an implementation overrides only the nodes it cares about.

use crate::tree::*;

pub trait Visit {
    fn expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn block(&mut self, b: &Block) {
        walk_block(self, b);
    }
    /// A pattern that binds locals: a `let`, a `match` arm, a `for` loop, or a `comptime for`.
    fn pat(&mut self, _p: &Pat) {}
}

pub fn walk_block<V: Visit + ?Sized>(v: &mut V, b: &Block) {
    for s in &b.stmts {
        match s {
            Stmt::Let(p, init, other) => {
                v.expr(init);
                v.pat(p);
                if let Some(other) = other {
                    v.block(other);
                }
            }
            Stmt::Expr(e) => v.expr(e),
        }
    }
    if let Some(t) = &b.tail {
        v.expr(t);
    }
}

pub fn walk_expr<V: Visit + ?Sized>(v: &mut V, e: &Expr) {
    use ExprKind as K;
    match &e.kind {
        K::Unit
        | K::Bool(_)
        | K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::Bytes(_)
        | K::Local(_)
        | K::Const(_)
        | K::Closure { .. }
        | K::Continue(_)
        | K::None
        | K::Return(None)
        | K::LayoutOf { .. } => {}
        K::Break { value, .. } => {
            if let Some(value) = value {
                v.expr(value);
            }
        }
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|a| v.expr(a));
        }
        K::Array(es) | K::Tuple(es) => es.iter().for_each(|a| v.expr(a)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, a)| v.expr(a)),
        K::ClosureCall { callee, args, .. } => {
            v.expr(callee);
            args.iter().for_each(|a| v.expr(a));
        }
        K::AnyCall { recv, args, .. } => {
            v.expr(recv);
            args.iter().for_each(|a| v.expr(a));
        }
        K::BoxAny { value: inner, .. }
        | K::AnyIs { value: inner, .. }
        | K::AnyDowncast { value: inner, .. }
        | K::StringAsStr(inner)
        | K::Unary(_, inner)
        | K::Cast(inner)
        | K::Deref(inner)
        | K::PtrDeref(inner)
        | K::DerefRef(inner)
        | K::Field(inner, _)
        | K::Some(inner)
        | K::Ok(inner)
        | K::Err(inner)
        | K::Return(Some(inner))
        | K::ArrayRepeat(inner, _)
        | K::Comptime(inner) => v.expr(inner),
        K::Ref { place, .. } => v.expr(place),
        K::Assign(a, b) | K::CompoundAssign(_, a, b) | K::Binary(_, a, b) | K::Index(a, b) => {
            v.expr(a);
            v.expr(b);
        }
        K::Range { lo, hi, .. } => {
            v.expr(lo);
            v.expr(hi);
        }
        K::Slice { base, lo, hi, .. } => {
            v.expr(base);
            v.expr(lo);
            v.expr(hi);
        }
        K::Block(b) | K::Loop(b) => v.block(b),
        K::Labeled(_, body) => v.expr(body),
        K::If(c, then, other) => {
            v.expr(c);
            v.block(then);
            if let Some(o) = other {
                v.expr(o);
            }
        }
        K::While(c, body) => {
            v.expr(c);
            v.block(body);
        }
        K::ForRange { var, lo, hi, body, .. } => {
            v.expr(lo);
            v.expr(hi);
            if let Some(var) = var {
                v.pat(&Pat::Binding(*var, BindMode::Copy));
            }
            v.block(body);
        }
        K::ForRangeValue { var, range, body } => {
            v.expr(range);
            if let Some(var) = var {
                v.pat(&Pat::Binding(*var, BindMode::Copy));
            }
            v.block(body);
        }
        K::ForArray { pat, array: src, body }
        | K::ForSlice { pat, slice: src, body }
        | K::ForList { pat, list: src, body }
        | K::ForMap { pat, map: src, body }
        | K::ForChars { pat, string: src, body }
        | K::ComptimeFor { pat, iter: src, body } => {
            v.expr(src);
            v.pat(pat);
            v.block(body);
        }
        K::Match(scrutinee, arms) => {
            v.expr(scrutinee);
            for arm in arms {
                v.pat(&arm.pat);
                if let Some(g) = &arm.guard {
                    v.expr(g);
                }
                v.expr(&arm.body);
            }
        }
    }
}

pub trait VisitMut {
    fn expr(&mut self, e: &mut Expr) {
        walk_expr_mut(self, e);
    }
    fn block(&mut self, b: &mut Block) {
        walk_block_mut(self, b);
    }
}

pub fn walk_block_mut<V: VisitMut + ?Sized>(v: &mut V, b: &mut Block) {
    for s in &mut b.stmts {
        match s {
            Stmt::Let(_, init, other) => {
                v.expr(init);
                if let Some(other) = other {
                    v.block(other);
                }
            }
            Stmt::Expr(e) => v.expr(e),
        }
    }
    if let Some(t) = &mut b.tail {
        v.expr(t);
    }
}

pub fn walk_expr_mut<V: VisitMut + ?Sized>(v: &mut V, e: &mut Expr) {
    use ExprKind as K;
    match &mut e.kind {
        K::Unit
        | K::Bool(_)
        | K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::Bytes(_)
        | K::Local(_)
        | K::Const(_)
        | K::Closure { .. }
        | K::Continue(_)
        | K::None
        | K::Return(None)
        | K::LayoutOf { .. } => {}
        K::Break { value, .. } => {
            if let Some(value) = value {
                v.expr(value);
            }
        }
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter_mut().for_each(|a| v.expr(a));
        }
        K::Array(es) | K::Tuple(es) => es.iter_mut().for_each(|a| v.expr(a)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter_mut().for_each(|(_, a)| v.expr(a)),
        K::ClosureCall { callee, args, .. } => {
            v.expr(callee);
            args.iter_mut().for_each(|a| v.expr(a));
        }
        K::AnyCall { recv, args, .. } => {
            v.expr(recv);
            args.iter_mut().for_each(|a| v.expr(a));
        }
        K::BoxAny { value: inner, .. }
        | K::AnyIs { value: inner, .. }
        | K::AnyDowncast { value: inner, .. }
        | K::StringAsStr(inner)
        | K::Unary(_, inner)
        | K::Cast(inner)
        | K::Deref(inner)
        | K::PtrDeref(inner)
        | K::DerefRef(inner)
        | K::Field(inner, _)
        | K::Some(inner)
        | K::Ok(inner)
        | K::Err(inner)
        | K::Return(Some(inner))
        | K::ArrayRepeat(inner, _)
        | K::Comptime(inner) => v.expr(inner),
        K::Ref { place, .. } => v.expr(place),
        K::Assign(a, b) | K::CompoundAssign(_, a, b) | K::Binary(_, a, b) | K::Index(a, b) => {
            v.expr(a);
            v.expr(b);
        }
        K::Range { lo, hi, .. } => {
            v.expr(lo);
            v.expr(hi);
        }
        K::Slice { base, lo, hi, .. } => {
            v.expr(base);
            v.expr(lo);
            v.expr(hi);
        }
        K::Block(b) | K::Loop(b) => v.block(b),
        K::Labeled(_, body) => v.expr(body),
        K::If(c, then, other) => {
            v.expr(c);
            v.block(then);
            if let Some(o) = other {
                v.expr(o);
            }
        }
        K::While(c, body) => {
            v.expr(c);
            v.block(body);
        }
        K::ForRange { lo, hi, body, .. } => {
            v.expr(lo);
            v.expr(hi);
            v.block(body);
        }
        K::ForRangeValue { range: src, body, .. }
        | K::ForArray { array: src, body, .. }
        | K::ForSlice { slice: src, body, .. }
        | K::ForList { list: src, body, .. }
        | K::ForMap { map: src, body, .. }
        | K::ForChars { string: src, body, .. }
        | K::ComptimeFor { iter: src, body, .. } => {
            v.expr(src);
            v.block(body);
        }
        K::Match(scrutinee, arms) => {
            v.expr(scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    v.expr(g);
                }
                v.expr(&mut arm.body);
            }
        }
    }
}

/// Every local a pattern binds.
pub fn pat_locals(p: &Pat, out: &mut Vec<crate::LocalId>) {
    match p {
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            pat_locals(inner, out);
        }
        Pat::Some(inner) | Pat::Ok(inner) | Pat::Err(inner) => pat_locals(inner, out),
        Pat::Or(ps) | Pat::Product(ps) | Pat::Variant(_, ps) => ps.iter().for_each(|q| pat_locals(q, out)),
        Pat::Slice { prefix, rest, suffix } => {
            prefix.iter().for_each(|q| pat_locals(q, out));
            if let Some(rest) = rest {
                pat_locals(rest, out);
            }
            suffix.iter().for_each(|q| pat_locals(q, out));
        }
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
    }
}

/// The locals `e` reads without binding them itself, each with the span of its first use: the locals of the
/// enclosing function a comptime expression would need (`spec/comptime.md`, CT2). A closure's captures count as
/// uses.
pub fn free_locals(e: &Expr) -> Vec<(crate::LocalId, burdin_diag::Span)> {
    struct Free {
        bound: Vec<crate::LocalId>,
        used: Vec<(crate::LocalId, burdin_diag::Span)>,
    }
    impl Visit for Free {
        fn expr(&mut self, e: &Expr) {
            match &e.kind {
                ExprKind::Local(id) => self.used.push((*id, e.span)),
                ExprKind::Closure { captures, .. } => {
                    self.used.extend(captures.iter().map(|c| (c.local, e.span)));
                }
                _ => walk_expr(self, e),
            }
        }
        fn pat(&mut self, p: &Pat) {
            pat_locals(p, &mut self.bound);
        }
    }
    let mut v = Free { bound: Vec::new(), used: Vec::new() };
    v.expr(e);
    let mut out: Vec<(crate::LocalId, burdin_diag::Span)> = Vec::new();
    for (id, span) in v.used {
        if !v.bound.contains(&id) && !out.iter().any(|(o, _)| *o == id) {
            out.push((id, span));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalId, Ty};
    use burdin_diag::{FileId, Span};

    fn e(kind: ExprKind) -> Expr {
        Expr { kind, ty: Ty::Unit, span: Span::new(FileId(0), 0, 0) }
    }

    #[test]
    fn a_local_bound_inside_is_not_free() {
        // { let a = b; a }
        let block = Block {
            stmts: vec![Stmt::Let(Pat::Binding(LocalId(0), BindMode::Copy), e(ExprKind::Local(LocalId(1))), None)],
            tail: Some(Box::new(e(ExprKind::Local(LocalId(0))))),
            close: Span::new(FileId(0), 0, 0),
        };
        let free: Vec<LocalId> = free_locals(&e(ExprKind::Block(block))).into_iter().map(|(l, _)| l).collect();
        assert_eq!(free, vec![LocalId(1)]);
    }

    #[test]
    fn a_match_binding_is_not_free() {
        let arm = Arm {
            pat: Pat::Some(Box::new(Pat::Binding(LocalId(3), BindMode::Copy))),
            guard: None,
            body: e(ExprKind::Local(LocalId(3))),
        };
        let m = e(ExprKind::Match(Box::new(e(ExprKind::Local(LocalId(2)))), vec![arm]));
        let free: Vec<LocalId> = free_locals(&m).into_iter().map(|(l, _)| l).collect();
        assert_eq!(free, vec![LocalId(2)]);
    }
}
