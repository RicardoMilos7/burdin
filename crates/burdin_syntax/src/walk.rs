//! A read-only walk over the syntax tree. [`Visit`] overrides only the nodes a tool cares about; every default
//! method walks into the node's children, in source order. The linter and the refactoring engine walk with it.

use crate::ast::*;
use burdin_diag::Span;

pub trait Visit {
    fn expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn block(&mut self, b: &Block) {
        walk_block(self, b);
    }
    fn stmt(&mut self, s: &Stmt) {
        walk_stmt(self, s);
    }
    fn pattern(&mut self, p: &Pattern) {
        walk_pattern(self, p);
    }
    fn ty(&mut self, t: &Type) {
        walk_type(self, t);
    }
}

pub fn walk_block<V: Visit + ?Sized>(v: &mut V, b: &Block) {
    for s in &b.stmts {
        v.stmt(s);
    }
}

pub fn walk_stmt<V: Visit + ?Sized>(v: &mut V, s: &Stmt) {
    match s {
        Stmt::Let { pattern, ty, init, else_block, .. } => {
            if let Some(ty) = ty {
                v.ty(ty);
            }
            v.expr(init);
            v.pattern(pattern);
            if let Some(b) = else_block {
                v.block(b);
            }
        }
        Stmt::Expr(e) => v.expr(e),
    }
}

pub fn walk_pattern<V: Visit + ?Sized>(v: &mut V, p: &Pattern) {
    match &p.kind {
        PatternKind::Wild
        | PatternKind::Binding { .. }
        | PatternKind::Lit(_)
        | PatternKind::Range(..)
        | PatternKind::Path(_) => {}
        PatternKind::Tuple(ps) | PatternKind::Or(ps) | PatternKind::TupleStruct(_, ps) => {
            ps.iter().for_each(|p| v.pattern(p));
        }
        PatternKind::Struct(_, fields) => {
            for f in fields {
                if let Some(p) = &f.pattern {
                    v.pattern(p);
                }
            }
        }
        PatternKind::At(_, p) => v.pattern(p),
    }
}

pub fn walk_type<V: Visit + ?Sized>(v: &mut V, t: &Type) {
    match &t.kind {
        TypeKind::Path(segments) => {
            for s in segments {
                s.args.iter().for_each(|a| v.ty(a));
            }
        }
        TypeKind::Any(t) | TypeKind::Impl(t) | TypeKind::Slice(t) => v.ty(t),
        TypeKind::Ref { ty, .. } => v.ty(ty),
        TypeKind::Fn { params, ret, .. } => {
            params.iter().for_each(|p| v.ty(p));
            if let Some(r) = ret {
                v.ty(r);
            }
        }
        TypeKind::Tuple(ts) => ts.iter().for_each(|t| v.ty(t)),
        TypeKind::Array(t, len) => {
            v.ty(t);
            v.expr(len);
        }
        TypeKind::Const(e) => v.expr(e),
        TypeKind::SelfType | TypeKind::Unit | TypeKind::Never => {}
    }
}

pub fn walk_interp<V: Visit + ?Sized>(v: &mut V, parts: &[InterpPart]) {
    for part in parts {
        if let InterpPart::Hole(Hole { arg: HoleArg::Expr(e), .. }) = part {
            v.expr(e);
        }
    }
}

pub fn walk_expr<V: Visit + ?Sized>(v: &mut V, e: &Expr) {
    use ExprKind as K;
    match &e.kind {
        K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::InlineSnapshot(_)
        | K::Bool(_)
        | K::Unit
        | K::SelfValue
        | K::Break(_)
        | K::Continue(_)
        | K::Return(None) => {}
        K::Labeled { body, .. } => v.expr(body),
        K::Interp(parts) => walk_interp(v, parts),
        K::Name { args, .. } => args.iter().for_each(|a| v.ty(a)),
        K::Tuple(es) | K::Array(es) => es.iter().for_each(|x| v.expr(x)),
        K::ArrayRepeat(a, b) | K::Assign(a, b) | K::Binary(_, a, b) | K::OrElse(a, b) | K::Index(a, b) => {
            v.expr(a);
            v.expr(b);
        }
        K::Block(b) | K::UnsafeBlock(b) | K::Loop(b) => v.block(b),
        K::If { cond, then_block, else_expr } => {
            v.expr(cond);
            v.block(then_block);
            if let Some(o) = else_expr {
                v.expr(o);
            }
        }
        K::While { cond, body } => {
            v.expr(cond);
            v.block(body);
        }
        K::For { pattern, iter, body, .. } => {
            v.expr(iter);
            v.pattern(pattern);
            v.block(body);
        }
        K::Return(Some(x)) | K::Unary(_, x) | K::Try(x) | K::Comptime(x) | K::Field(x, _) | K::TupleField(x, ..) => {
            v.expr(x)
        }
        K::Cast(x, t) => {
            v.expr(x);
            v.ty(t);
        }
        K::Range { lo, hi, .. } => {
            v.expr(lo);
            v.expr(hi);
        }
        K::DynField(a, b) => {
            v.expr(a);
            v.expr(b);
        }
        K::TypeArgs(x, ts) => {
            v.expr(x);
            ts.iter().for_each(|t| v.ty(t));
        }
        K::Call(callee, args) => {
            v.expr(callee);
            args.iter().for_each(|a| v.expr(&a.expr));
        }
        K::Match(scrutinee, arms) => {
            v.expr(scrutinee);
            for arm in arms {
                if let Some(p) = &arm.pattern {
                    v.pattern(p);
                }
                if let Some(g) = &arm.guard {
                    v.expr(g);
                }
                v.expr(&arm.body);
            }
        }
        K::Closure { params, ret, body } => {
            for p in params {
                if let Some(t) = &p.ty {
                    v.ty(t);
                }
            }
            if let Some(r) = ret {
                v.ty(r);
            }
            v.expr(body);
        }
        K::StructLit(_, fields) => {
            for f in fields {
                if let Some(x) = &f.value {
                    v.expr(x);
                }
            }
        }
        K::IfLet { pattern, scrutinee, then_block, else_expr } => {
            v.expr(scrutinee);
            v.pattern(pattern);
            v.block(then_block);
            if let Some(o) = else_expr {
                v.expr(o);
            }
        }
        K::ComptimeAssert(c, m) => {
            v.expr(c);
            if let Some(m) = m {
                v.expr(m);
            }
        }
    }
}

/// Where a function was declared, which decides what a lint may ask of its signature.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FnOrigin {
    /// A free function.
    Free,
    /// A method of an inherent `impl Type { }` block, or an extension method.
    Method,
    /// A method of `impl Type: Interface { }`: the interface dictates the signature.
    InterfaceImpl,
    /// A method declared in an `interface` (with or without a default body).
    InterfaceDecl,
    /// A method of a `derive` declaration: the derive's interface dictates the signature.
    DeriveMethod,
    /// A function exported with the C ABI.
    Exported,
    /// A `test` block's body.
    Test,
}

/// Calls `f` for every function-like item of a file with its origin: free functions, methods, extension methods,
/// interface methods, derive methods, exported functions, and test bodies. An impl the parser generated (a derived
/// `Debug`, a lowered `error enum`) is skipped: `generated` says whether a span belongs to one.
pub fn for_each_fn<'a>(file: &'a File, generated: &dyn Fn(Span) -> bool, f: &mut dyn FnMut(FnOrigin, &'a FnDecl)) {
    for item in &file.items {
        if generated(item_span(item)) {
            continue;
        }
        match item {
            Item::Fn(d) => f(FnOrigin::Free, d),
            Item::Extension(x) => f(FnOrigin::Method, &x.func),
            Item::ExternFn(x) => f(FnOrigin::Exported, &x.decl),
            Item::Impl(b) => {
                let origin = if b.interface.is_some() { FnOrigin::InterfaceImpl } else { FnOrigin::Method };
                b.methods.iter().for_each(|m| f(origin, m));
            }
            Item::Interface(i) => i.methods.iter().for_each(|m| f(FnOrigin::InterfaceDecl, &m.decl)),
            Item::Derive(d) => d.methods.iter().for_each(|m| f(FnOrigin::DeriveMethod, m)),
            Item::Const(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::ErrorEnum(_)
            | Item::TypeAlias(_)
            | Item::Extern(_) => {}
        }
    }
    for t in &file.tests {
        f(FnOrigin::Test, &t.body_func);
    }
}

/// The span an item carries as a whole.
pub fn item_span(item: &Item) -> Span {
    match item {
        Item::Fn(d) => d.span,
        Item::Extension(x) => x.func.span,
        Item::Const(c) => c.span,
        Item::Struct(s) => s.span,
        Item::Enum(e) => e.span,
        Item::ErrorEnum(e) => e.span,
        Item::TypeAlias(t) => t.span,
        Item::Impl(b) => b.span,
        Item::Interface(i) => i.span,
        Item::Extern(b) => b.span,
        Item::ExternFn(x) => x.decl.span,
        Item::Derive(d) => d.span,
    }
}
