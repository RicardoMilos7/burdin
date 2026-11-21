//! Checking one function body: the typed expression tree is built here. `FnCx` is the state of one body (locals,
//! scopes, loops, module, and `unsafe` depth); its methods are split by what they check, one file per theme.

mod aggregate;
mod any;
mod call;
mod closure;
mod comptime;
mod control;
mod edition;
mod embed;
mod expr;
mod fnvalue;
mod format;
mod generic;
mod item;
mod iterate;
mod literals;
mod operators;
mod pattern;
mod reflect;
mod serial;
mod stmt;
mod suggest;

pub(crate) use item::{check_const, check_defaults, check_function};
pub(crate) use reflect::RESERVED_MEMBERS;

use crate::builtins::builtin_type;
use crate::generics::Bound;
use crate::globals::{Globals, TypeName};
use crate::types::compatible;
use burdin_diag::{Code, Diagnostics, SourceMap, Span};
use burdin_syntax::ast;
use burdin_tir::place::{place_root, through_view};
use burdin_tir::{self as tir, IntTy, LocalId, LocalKind, Pat, Ty};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// One entry on the stack of open loops while a body is checked. A loop with a label has two entries: the label's
/// (`label` set, `is_loop` false), which the labeled `break`s and `continue`s name, and the loop's own, which the
/// unlabeled ones name. `has_break` records whether a `break` leaves the entry's loop.
#[derive(Default)]
struct LoopScope {
    label: Option<String>,
    is_loop: bool,
    has_break: bool,
}

impl LoopScope {
    /// The entry of a loop body, which has no label of its own.
    fn loop_body() -> Self {
        LoopScope { label: None, is_loop: true, has_break: false }
    }
}

struct FnCx<'a> {
    globals: &'a Globals,
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Ty,
    /// The enclosing loops, innermost last (see `LoopScope`).
    loops: Vec<LoopScope>,
    /// What `Self` means: the type of the enclosing method block.
    self_ty: Option<Ty>,
    /// The `self` parameter of the method being checked.
    self_local: Option<LocalId>,
    /// What the type parameters in scope stand for.
    subst: Vec<(String, Ty)>,
    /// The bounds of the type parameters in scope (symbolic checks only).
    bounds: Vec<(String, Vec<Bound>)>,
    /// The module whose names the body resolves.
    module: usize,
    /// How many `unsafe` blocks (or `unsafe fn` bodies) enclose this expression.
    unsafe_depth: usize,
    /// The body is checked against bounds only and is discarded: it has no code, so it registers no vtable and
    /// no type identity (AN3, AN6).
    symbolic: bool,
    /// The closure bodies checked so far in this pass. Nested closures share the same list, so a closure
    /// expression's index is stable by the time the enclosing body is built.
    closures: Rc<RefCell<Vec<tir::Closure>>>,
    /// Set while checking a closure body: the enclosing function's visible locals, for capture-on-use, and the
    /// captures discovered so far.
    capture: Option<closure::CaptureCx>,
    /// Where the paths, lines, and columns of trace frames and typed assertions come from.
    sources: &'a SourceMap,
    /// The name of the function being checked, as a trace frame records it (`spec/debug.md`, DBG3).
    fn_name: String,
    /// Record the source location of a dynamic-error conversion (a debug build; `spec/errors.md`, ERR7).
    traces: bool,
    /// Set while checking a place that will be written: the left side of `=`, the operand of `&mut`, and the
    /// argument of a `mut` parameter. An index expression then lowers to `index_mut` (`spec/operators.md`, OP10).
    write_place: bool,
    /// The locals whose values are known at compile time: `comptime` parameters and `comptime for` bindings
    /// (`spec/comptime.md`, CT2).
    comptime_known: Vec<LocalId>,
    /// The loop depth just outside the innermost `comptime for` body, which `break`/`continue` cannot leave.
    comptime_loop_barrier: Option<usize>,
    /// The variables of the `comptime for` loops over `Type.fields` and `Type.variants` that are being unrolled, and
    /// the item each one stands for (`spec/reflection.md`, RFL4).
    reflect: reflect::ReflectBindings,
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
    // The arguments are the whole context of one body (the program, diagnostics, the signature, the enclosing
    // method type, the module, the flags, and the closure list); a struct would only move them around.
    #[allow(clippy::too_many_arguments)]
    fn new(
        globals: &'a Globals,
        diags: &'a mut Diagnostics,
        ret: Ty,
        self_ty: Option<Ty>,
        module: usize,
        is_unsafe: bool,
        symbolic: bool,
        closures: Rc<RefCell<Vec<tir::Closure>>>,
        sources: &'a SourceMap,
        fn_name: String,
        traces: bool,
    ) -> Self {
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
            module,
            unsafe_depth: usize::from(is_unsafe),
            symbolic,
            closures,
            capture: None,
            sources,
            fn_name,
            traces,
            write_place: false,
            comptime_known: Vec::new(),
            comptime_loop_barrier: None,
            reflect: HashMap::new(),
        }
    }

    /// The source path, line, and column of `span`, for a trace frame or a typed assertion's location.
    pub(super) fn location(&self, span: Span) -> (String, i32, i32) {
        self.globals.located.set(true);
        let file = self.sources.get(span.file);
        let (line, col) = file.line_col(span.lo);
        (file.path.clone(), line as i32, col as i32)
    }

    fn package(&self) -> usize {
        self.globals.package_of(self.module)
    }

    pub(super) fn resolve(&mut self, ty: &ast::Type) -> Ty {
        self.globals.resolve_type_in(ty, self.self_ty.as_ref(), &self.subst, self.module, self.diags)
    }

    /// Whether a type parameter's bounds provide `bound`.
    pub(super) fn param_has(&self, ty: &Ty, bound: Bound) -> bool {
        matches!(ty, Ty::Param(_)) && self.globals.satisfies(ty, &bound, &self.bounds)
    }

    pub(super) fn exhaustive(&self, ty: &Ty, pats: &[&Pat]) -> bool {
        crate::exhaust::exhaustive(ty, pats, &self.globals.defs())
    }

    /// If `e` names a type (and no value hides it), that type: the left side of `Point.new` or `Shape.Circle`.
    pub(super) fn type_path(&mut self, e: &ast::Expr) -> Option<PathTy> {
        let ast::ExprKind::Name { name, args } = &e.kind else { return None };
        if name.name == "Self" {
            if !args.is_empty() {
                self.diags.error(Code::TYPE_MISMATCH, name.span, "`Self` takes no generic arguments");
                return Some(PathTy::Ty(Ty::Error));
            }
            return Some(PathTy::Ty(match &self.self_ty {
                Some(ty) => ty.clone(),
                None => {
                    self.diags.error(Code::UNKNOWN_NAME, name.span, "`Self` is only available inside a method block");
                    Ty::Error
                }
            }));
        }
        if self.lookup(&name.name).is_some() || self.globals.value(self.module, &name.name).is_some() {
            return None;
        }
        if args.is_empty() {
            if let Some((_, t)) = self.subst.iter().find(|(n, _)| *n == name.name) {
                return Some(PathTy::Ty(t.clone()));
            }
            if let Some(TypeName::GenericAdt(g)) = self.globals.type_name(self.module, &name.name) {
                return Some(PathTy::Template(g));
            }
            return self
                .globals
                .named_type(self.module, &name.name)
                .or_else(|| builtin_type(&name.name))
                .map(PathTy::Ty);
        }
        let self_ty = self.self_ty.clone();
        Some(PathTy::Ty(self.globals.generic_type(name, args, self_ty.as_ref(), &self.subst, self.module, self.diags)))
    }

    /// The segments of a plain expression path (`math.add`, `geometry.shapes.Square`), or `None` when the
    /// expression has a call, index, or another non-path node in it.
    fn flat_path(e: &ast::Expr) -> Option<Vec<ast::Ident>> {
        match &e.kind {
            ast::ExprKind::Name { name, args } if args.is_empty() => Some(vec![name.clone()]),
            ast::ExprKind::Field(base, name) => {
                let mut path = FnCx::flat_path(base)?;
                path.push(name.clone());
                Some(path)
            }
            _ => None,
        }
    }

    /// Whether a path starts with a module rather than a local or a value.
    fn starts_with_module(&self, path: &[ast::Ident]) -> bool {
        path.first().is_some_and(|first| {
            // A local of the enclosing function that the closure captures shadows a module of the same name.
            let captured = self.captures_name(&first.name);
            self.lookup(&first.name).is_none() && !captured && self.globals.value(self.module, &first.name).is_none()
        })
    }

    /// A module-qualified item in expression position: `math.add`, `sh.area`. `recv` is the module part of the
    /// path and `name` is the item (the last field of a call or access).
    fn module_member(&self, recv: &ast::Expr, name: &ast::Ident) -> Option<(usize, ast::Ident)> {
        let prefix = FnCx::flat_path(recv)?;
        if !self.starts_with_module(&prefix) {
            return None;
        }
        let names: Vec<String> = prefix.iter().map(|i| i.name.clone()).collect();
        let target = self.globals.resolve_module_path(self.module, &names)?;
        Some((target, name.clone()))
    }

    /// A generic type named through a module path, as the whole path: `Wrapper`, `sh.Wrapper`. Returns the generic
    /// type and the module its name was found in.
    fn generic_adt_path(&self, path: &[ast::Ident]) -> Option<(usize, usize)> {
        for k in (0..path.len()).rev() {
            let target = if k == 0 {
                Some(self.module)
            } else {
                let names: Vec<String> = path[..k].iter().map(|i| i.name.clone()).collect();
                self.globals.resolve_module_path(self.module, &names)
            };
            if let Some(target) = target
                && k + 1 == path.len()
            {
                let found = if k == 0 {
                    self.globals.type_name(target, &path[k].name)
                } else {
                    self.globals.exported_type(target, &path[k].name)
                };
                if let Some(TypeName::GenericAdt(g)) = found {
                    return Some((g, target));
                }
            }
        }
        None
    }

    /// A module-qualified type in expression position: `sh.Square`, `geometry.shapes.Square`.
    fn qualified_type(&mut self, e: &ast::Expr) -> Option<Ty> {
        let path = FnCx::flat_path(e)?;
        if path.len() < 2 || !self.starts_with_module(&path) {
            return None;
        }
        let names: Vec<String> = path[..path.len() - 1].iter().map(|i| i.name.clone()).collect();
        let target = self.globals.resolve_module_path(self.module, &names)?;
        let last = path.last().expect("the path has a last segment");
        self.named_type_in(target, &last.name, last.span)
    }

    /// Reports E0150 when `name` of `target` is package-private and `target` is in another package.
    fn check_value_visibility(&mut self, target: usize, name: &str, span: Span) {
        if self.globals.package_of(target) != self.package() && self.globals.value_is_private(target, name) {
            self.diags.error(Code::PRIVATE_ITEM, span, format!("`{name}` is `priv` in another package"));
        }
    }

    /// Reports E0150 when a type name of `target` is package-private and `target` is in another package.
    fn check_type_visibility(&mut self, target: usize, name: &str, span: Span) {
        if self.globals.package_of(target) != self.package() && self.globals.type_is_private(target, name) {
            self.diags.error(Code::PRIVATE_ITEM, span, format!("type `{name}` is `priv` in another package"));
        }
    }

    /// The type `name` of `module` if it is one, without a diagnostic: a module-qualified path may name a module
    /// (`lib.helpers.f()`) or a value instead, and the caller tries those next.
    fn named_type_in(&mut self, module: usize, name: &str, span: Span) -> Option<Ty> {
        let found = self.globals.exported_type(module, name);
        if found.is_some() {
            self.check_type_visibility(module, name, span);
        }
        match found {
            // A generic type needs its arguments; a later, quieter diagnostic explains the construct.
            Some(TypeName::GenericAdt(_)) => None,
            Some(_) => self.globals.named_type(module, name),
            None => builtin_type(name),
        }
    }

    /// Resolves a path that names an ADT, with an optional variant: `Square`, `Shape.Circle`,
    /// `sh.Square`, `sh.Shape.Circle`. A generic type name resolves to the scrutinee's instance. The last value is
    /// the module whose namespace the type name came from.
    fn adt_path(&self, path: &[ast::Ident], scrutinee: &Ty) -> Option<(Ty, Option<ast::Ident>, usize, ast::Ident)> {
        // `Self.Variant` and `Self { ... }` in the impl block of the type.
        if path.first().is_some_and(|first| first.name == "Self") {
            if path.len() > 2 {
                return None;
            }
            let ty = self.self_ty.clone().unwrap_or(Ty::Error);
            return Some((ty, path.get(1).cloned(), self.module, path[0].clone()));
        }
        for k in (0..path.len()).rev() {
            let target = if k == 0 {
                Some(self.module)
            } else {
                let names: Vec<String> = path[..k].iter().map(|i| i.name.clone()).collect();
                self.globals.resolve_module_path(self.module, &names)
            };
            let Some(target) = target else { continue };
            let rest = &path[k..];
            if rest.len() > 2 {
                continue;
            }
            let name = &rest[0].name;
            let found =
                if k == 0 { self.globals.type_name(target, name) } else { self.globals.exported_type(target, name) };
            let ty = match found {
                Some(TypeName::GenericAdt(g)) => match self.globals.origin_of(scrutinee) {
                    Some((template, _)) if *template == g => Some(scrutinee.clone()),
                    _ => None,
                },
                Some(TypeName::Alias(_)) | Some(TypeName::Struct(_)) | Some(TypeName::Enum(_)) => {
                    self.globals.named_type(target, name)
                }
                Some(TypeName::GenericAlias(_)) => None,
                None => builtin_type(name),
            };
            if let Some(ty) = ty {
                return Some((ty, rest.get(1).cloned(), target, rest[0].clone()));
            }
        }
        None
    }

    /// A name that is not hidden by a local or a global: `Some`, `None`.
    pub(super) fn is_prelude(&self, name: &str) -> bool {
        self.lookup(name).is_none() && self.globals.value(self.module, name).is_none()
    }

    fn declare(&mut self, name: &str, ty: Ty, mutable: bool, kind: LocalKind, span: Span) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable, by_ref: false, kind, span });
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

    /// Checks a call argument for a parameter with the write-place mode its convention needs: the argument of a
    /// `mut` parameter may assign through an index, so its index expressions lower to `index_mut`.
    pub(super) fn expr_arg(&mut self, e: &ast::Expr, expect: Option<&Ty>, mutable: bool) -> tir::Expr {
        let saved = self.write_place;
        self.write_place = mutable;
        let checked = self.expr(e, expect);
        self.write_place = saved;
        checked
    }

    /// Whether the place `e` may be assigned to or borrowed with `&mut` (MU1, MU3).
    pub(super) fn place_mutable(&self, e: &tir::Expr) -> bool {
        match &e.kind {
            tir::ExprKind::Local(id) => self.locals[id.0].mutable,
            tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => self.place_mutable(base),
            tir::ExprKind::DerefRef(base) => matches!(base.ty, Ty::Ref { mutable: true, .. }),
            // Writing through a raw pointer writes C memory; it does not mutate the binding that holds the
            // pointer, and `unsafe` is what guards it (memory-rules.md, US; issue #72).
            tir::ExprKind::PtrDeref(_) => true,
            // Elements of an array or a list place are mutable with it; elements of a slice view are read-only.
            tir::ExprKind::Index(base, _) => {
                matches!(base.ty, Ty::Array(..) | Ty::List(_, _)) && self.place_mutable(base)
            }
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
            Some(root) => {
                matches!(self.locals[root.0].kind, LocalKind::Owned | LocalKind::OwnParam | LocalKind::Scrutinee)
            }
            None => true,
        }
    }

    pub(super) fn unsupported(&mut self, span: Span, what: &str) -> tir::Expr {
        self.diags.error(Code::UNSUPPORTED, span, format!("{what} is not implemented in this compiler yet")); // e9000: unsupported-helper
        error_expr(span)
    }

    pub(super) fn mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{expected}`, found `{found}`"));
    }

    /// Reports E0003 unless `e` has type `want` (or a type that converts to nothing at run time, like `!`).
    fn expect_ty(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if !compatible(&e.ty, want) && !self.param_meets_fn(&e.ty, want) {
            self.report_mismatch(e.span, want, &e.ty);
        }
        e
    }

    /// Whether `found` is a type parameter whose closure bound provides `want`: `F: FnOnce(): V` passed where a
    /// built-in method takes the closure type `FnOnce(): V` (`MapEntry::or_insert_with`). Each concrete instance
    /// checks its body again with `F` substituted, so a closure the bound does not allow is still rejected there.
    fn param_meets_fn(&self, found: &Ty, want: &Ty) -> bool {
        let Ty::Param(_) = found else { return false };
        let Ty::Fn { sig: want_sig, kind: want_kind, borrowed: false, .. } = want else { return false };
        self.globals
            .fn_call_sig(found, &self.bounds)
            .is_some_and(|(sig, kind)| sig == **want_sig && kind.at_least(*want_kind))
    }

    /// The type-mismatch diagnostic for `found` where `expected` is required. A closure mismatch gets the code
    /// that explains it: a borrowing closure where an owned one is required is E0171, and a closure whose body
    /// needs a lower capability than the expected type gives is E0141.
    pub(super) fn report_mismatch(&mut self, span: Span, want: &Ty, found: &Ty) {
        if let (Ty::Fn { sig: asig, kind: ak, borrowed: true, .. }, Ty::Fn { sig: esig, kind: ek, borrowed: false, .. }) =
            (found, want)
            && asig == esig
            && ak.at_least(*ek)
        {
            self.diags.error(
                Code::CLOSURE_ESCAPES,
                span,
                "this closure does not own everything it captures, so it cannot be used where an owned closure type is expected",
            );
            return;
        }
        if let (Ty::Fn { sig: asig, kind: ak, .. }, Ty::Fn { sig: esig, kind: ek, .. }) = (found, want)
            && asig == esig
            && !ak.at_least(*ek)
        {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("this closure is `{}`, which is not usable where `{}` is expected", ak.name(), ek.name()),
            );
            return;
        }
        self.mismatch(span, want, found);
    }

    /// Boxes `e` when it is used where `any I` is expected (AN3). Boxing produces an owned value, so unlike the
    /// `String`-to-`str` borrow it applies at every position with a known expected type. A view cannot be boxed
    /// (E0120), and the value must implement the interface (E0141).
    pub(super) fn coerce_any(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        let Ty::Any(any) = want else { return e };
        if matches!(e.ty, Ty::Error | Ty::Never) {
            return e;
        }
        if let Ty::Any(have) = &e.ty {
            if have == any {
                return e;
            }
            self.mismatch(e.span, want, &e.ty);
            // Type the value as the expected type: the program is rejected, and this avoids an E0003 cascade at
            // the enclosing position.
            return tir::Expr { ty: want.clone(), ..e };
        }
        let span = e.span;
        if e.ty.is_second_class() {
            self.diags.error(
                Code::SECOND_CLASS_POSITION,
                span,
                format!(
                    "`{}` is a view of other storage, so it cannot be boxed as `any {}`; the box would outlive \
                     the borrow",
                    e.ty, any.name
                ),
            );
            return tir::Expr { ty: want.clone(), ..e };
        }
        if !self.globals.satisfies(&e.ty, &Bound::User(any.iface, any.args.clone()), &self.bounds) {
            self.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!("`{}` does not implement `{}`, so it cannot be boxed as `any {}`", e.ty, any.name, any.name),
            );
            return tir::Expr { ty: want.clone(), ..e };
        }
        // A symbolic body has no code, so it registers neither a vtable nor a type identity.
        let vtable = if self.symbolic || self.globals.ty_has_param(&e.ty) {
            usize::MAX
        } else {
            match self.globals.any_vtable(&e.ty, any, span, self.diags) {
                Some(vtable) => vtable,
                None => return tir::Expr { ty: want.clone(), ..e },
            }
        };
        let ty = want.clone();
        mk(tir::ExprKind::BoxAny { value: Box::new(e), vtable }, ty, span)
    }

    /// Converts an error type into the dynamic `Error` where that type is expected (`spec/errors.md`, ERR4): the
    /// value is boxed as `any Error`, and a debug build records the conversion's source location (ERR7). A value
    /// that is not an error type is returned unchanged, so the caller reports the ordinary mismatch.
    pub(super) fn coerce_error(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        let Some(error_ty) = self.globals.error_ty.clone() else { return e };
        let Some(iface) = self.globals.error_iface else { return e };
        if !self.globals.is_error_ty(want) || matches!(e.ty, Ty::Error | Ty::Never) || e.ty == *want {
            return e;
        }
        if !self.globals.implements_error(&e.ty, &self.bounds) {
            return e;
        }
        let helper = if self.traces { self.globals.error_at } else { self.globals.error_plain };
        let Some(helper) = helper else { return e };
        let any = self.globals.any_iface(iface, Vec::new());
        let span = e.span;
        // A symbolic body has no code, so it registers neither a vtable nor a type identity.
        let vtable = if self.symbolic || self.globals.ty_has_param(&e.ty) {
            usize::MAX
        } else {
            match self.globals.any_vtable(&e.ty, &any, span, self.diags) {
                Some(vtable) => vtable,
                None => return e,
            }
        };
        let boxed = mk(tir::ExprKind::BoxAny { value: Box::new(e), vtable }, Ty::Any(any), span);
        let args = if self.traces {
            let (path, line, col) = self.location(span);
            vec![
                boxed,
                mk(tir::ExprKind::Str(self.fn_name.clone()), Ty::Str, span),
                mk(tir::ExprKind::Str(path), Ty::Str, span),
                mk(tir::ExprKind::Int(i128::from(line)), Ty::Int(IntTy::I32), span),
                mk(tir::ExprKind::Int(i128::from(col)), Ty::Int(IntTy::I32), span),
            ]
        } else {
            vec![boxed]
        };
        mk(tir::ExprKind::Call(helper, args), error_ty, span)
    }

    /// Like `expect_ty`, but a `String` argument is borrowed where a `str` is expected.
    pub(super) fn coerce_arg(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        if e.ty == Ty::String && *want == Ty::Str {
            let span = e.span;
            return mk(tir::ExprKind::StringAsStr(Box::new(e)), Ty::Str, span);
        }
        self.expect_ty(e, want)
    }

    /// Reports E0161 when `f` is an `unsafe fn` and this expression is not in an `unsafe` context.
    pub(super) fn check_unsafe_call(&mut self, f: tir::FuncId, name: &str, span: Span) {
        if self.unsafe_depth == 0 && self.globals.fn_unsafe[f.0] {
            self.diags.error(
                Code::UNSAFE_REQUIRED,
                span,
                format!("calling the unsafe function `{name}` requires an `unsafe` block"),
            );
        }
    }

    /// Reports E0150 when a member of a type from another package is package-private.
    pub(super) fn check_member_access(&mut self, ty: &Ty, key: (bool, usize), name: &ast::Ident) {
        let method_key = (key, name.name.clone());
        let owner = self
            .globals
            .method_pkg
            .get(&method_key)
            .copied()
            .unwrap_or_else(|| if key.0 { self.globals.enum_pkg[key.1] } else { self.globals.struct_pkg[key.1] });
        if owner != self.package() && self.globals.method_priv.get(&method_key).copied().unwrap_or(false) {
            self.diags.error(
                Code::PRIVATE_ITEM,
                name.span,
                format!("`{}` of `{ty}` is `priv` in another package", name.name),
            );
        }
    }
}
