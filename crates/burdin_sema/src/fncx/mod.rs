//! Checking one function body: the typed expression tree is built here. `FnCx` is the state of one body (locals,
//! scopes, loops); its methods are split by what they check, one file per theme.

mod aggregate;
mod call;
mod control;
mod expr;
mod generic;
mod item;
mod literals;
mod operators;
mod pattern;
mod stmt;

pub(crate) use item::{check_const, check_function};

use crate::builtins::builtin_type;
use crate::generics::Bound;
use crate::globals::{Globals, TypeName};
use crate::types::compatible;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_syntax::ast;
use burdin_tir::place::{place_root, through_view};
use burdin_tir::{self as tir, LocalId, LocalKind, Pat, Ty};
use std::collections::HashMap;

struct FnCx<'a> {
    globals: &'a Globals,
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Ty,
    /// One entry per enclosing loop: whether it contains a `break`.
    loops: Vec<bool>,
    /// What `Self` means: the type of the enclosing method block.
    self_ty: Option<Ty>,
    /// The `self` parameter of the method being checked.
    self_local: Option<LocalId>,
    /// What the type parameters in scope stand for.
    subst: Vec<(String, Ty)>,
    /// The bounds of the type parameters in scope (symbolic checks only).
    bounds: Vec<(String, Vec<Bound>)>,
}

/// What a path in expression position can name before its last segment: a type, or a generic type whose
/// arguments are still to be inferred.
pub(super) enum PathTy {
    Ty(Ty),
    Template(usize),
}

pub(super) fn mk(kind: tir::ExprKind, ty: Ty, span: Span) -> tir::Expr {
    tir::Expr { kind, ty, span }
}

pub(super) fn error_expr(span: Span) -> tir::Expr {
    mk(tir::ExprKind::Unit, Ty::Error, span)
}

impl<'a> FnCx<'a> {
    fn new(globals: &'a Globals, diags: &'a mut Diagnostics, ret: Ty, self_ty: Option<Ty>) -> Self {
        FnCx {
            globals,
            diags,
            locals: Vec::new(),
            scopes: vec![HashMap::new()],
            ret,
            loops: Vec::new(),
            self_ty,
            self_local: None,
            subst: Vec::new(),
            bounds: Vec::new(),
        }
    }

    pub(super) fn resolve(&mut self, ty: &ast::Type) -> Ty {
        self.globals.resolve_type_in(ty, self.self_ty.as_ref(), &self.subst, self.diags)
    }

    /// Whether a type parameter's bounds provide `bound`.
    pub(super) fn param_has(&self, ty: &Ty, bound: Bound) -> bool {
        matches!(ty, Ty::Param(_)) && self.globals.satisfies(ty, bound, &self.bounds)
    }

    pub(super) fn exhaustive(&self, ty: &Ty, pats: &[&Pat]) -> bool {
        crate::exhaust::exhaustive(ty, pats, &self.globals.defs())
    }

    /// If `e` names a type (and no value hides it), that type: the left side of `Point.new` or `Shape.Circle`.
    pub(super) fn type_path(&mut self, e: &ast::Expr) -> Option<PathTy> {
        let ast::ExprKind::Name { name, args } = &e.kind else { return None };
        if self.lookup(&name.name).is_some() || self.globals.names.contains_key(&name.name) {
            return None;
        }
        if args.is_empty() {
            if let Some((_, t)) = self.subst.iter().find(|(n, _)| *n == name.name) {
                return Some(PathTy::Ty(t.clone()));
            }
            if let Some(&TypeName::GenericAdt(g)) = self.globals.types.get(&name.name) {
                return Some(PathTy::Template(g));
            }
            return self.globals.named_type(&name.name).or_else(|| builtin_type(&name.name)).map(PathTy::Ty);
        }
        let self_ty = self.self_ty.clone();
        Some(PathTy::Ty(self.globals.generic_type(name, args, self_ty.as_ref(), &self.subst, self.diags)))
    }

    /// A name that is not hidden by a local or a global: `Some`, `None`.
    pub(super) fn is_prelude(&self, name: &str) -> bool {
        self.lookup(name).is_none() && !self.globals.names.contains_key(name)
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool, kind: LocalKind) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable, by_ref: false, kind });
        self.scopes.last_mut().expect("a scope is open").insert(name.to_string(), id);
        id
    }

    pub(super) fn lookup(&self, name: &str) -> Option<LocalId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Inserts the implicit dereferences of a receiver: field access and method calls go through `&T` (RF1).
    pub(super) fn auto_deref(&self, mut e: tir::Expr) -> tir::Expr {
        loop {
            let Ty::Ref { inner, .. } = &e.ty else { return e };
            let ty = (**inner).clone();
            let span = e.span;
            e = mk(tir::ExprKind::DerefRef(Box::new(e)), ty, span);
        }
    }

    /// Whether the place `e` may be assigned to or borrowed with `&mut` (MU1, MU3).
    pub(super) fn place_mutable(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            tir::ExprKind::Local(id) => self.locals[id.0].mutable,
            tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => self.place_mutable(base),
            tir::ExprKind::DerefRef(base) => matches!(base.ty, Ty::Ref { mutable: true, .. }),
            // Elements of an array place are mutable with it; elements of a slice view are read-only.
            tir::ExprKind::Index(base, _) => matches!(base.ty, Ty::Array(..)) && self.place_mutable(base),
            _ => false,
        }
    }

    /// Whether a pattern matching `e` takes ownership of it: a place rooted at an owned local, or a temporary.
    /// A place rooted at a read-only or `mut` parameter (or a view), or reached through a reference or a slice,
    /// is only borrowed (MV8).
    pub(super) fn owns_scrutinee(&self, e: &tir::Expr) -> bool {
        if through_view(e) {
            return false;
        }
        match place_root(e) {
            Some(root) => matches!(self.locals[root.0].kind, LocalKind::Owned | LocalKind::OwnParam),
            None => true,
        }
    }

    pub(super) fn unsupported(&mut self, span: Span, what: &str) -> tir::Expr {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet"));
        error_expr(span)
    }

    pub(super) fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{expected}`, found `{found}`"));
    }

    /// Reports E0003 unless `e` has type `want` (or a type that converts to nothing at run time, like `!`).
    fn expect_ty(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if !compatible(&e.ty, want) {
            self.mismatch(e.span, want, &e.ty);
        }
        e
    }

    /// Like `expect_ty`, but a `String` argument is borrowed where a `str` is expected.
    pub(super) fn coerce_arg(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if e.ty == Ty::String && *want == Ty::Str {
            let span = e.span;
            return mk(tir::ExprKind::StringAsStr(Box::new(e)), Ty::Str, span);
        }
        self.expect_ty(e, want)
    }
}
