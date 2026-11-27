//! Checking one function body: the typed expression tree is built here. `FnCx` is the state of one body (locals,
//! scopes, loops, module, and `unsafe` depth); its methods are split by what they check, one file per theme.

mod aggregate;
mod any;
mod call;
mod closure;
mod constcond;
mod control;
pub(crate) mod conversion;
mod edition;
mod embed;
mod expr;
mod fnvalue;
mod format;
mod generic;
mod item;
mod iterate;
mod literals;
mod opaque;
mod operators;
mod pattern;
mod reflect;
mod serial;
mod stmt;
pub(crate) mod suggest;
mod target;

pub(crate) use item::{check_const, check_defaults, check_function, infer_return};
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

/// Which kind of loop a [`LoopScope`] is: only a `loop` takes a `break` value, because a `while` or `for` can end
/// normally and gives back `()`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopKind {
    Loop,
    While,
    For,
}

/// One entry on the stack of open loops while a body is checked. A loop with a label carries that label, which the
/// labeled `break`s and `continue`s name; the unlabeled ones name the innermost loop. `has_break` records whether a
/// `break` leaves the loop, and `break_ty` is the type its `break` values must have: `()` for a `while`/`for`, the
/// expected type of the `loop` when it is known, or `None` until the first break of a `loop` with no expected type.
struct LoopScope {
    label: Option<String>,
    kind: LoopKind,
    break_ty: Option<Ty>,
    has_break: bool,
}

impl LoopScope {
    /// The entry of a `loop`, which may take a `break` value of `expect`'s type.
    fn loop_body(label: Option<&ast::Ident>, expect: Option<&Ty>) -> Self {
        LoopScope {
            label: label.map(|label| label.name.clone()),
            kind: LoopKind::Loop,
            break_ty: expect.cloned(),
            has_break: false,
        }
    }

    /// The entry of a `while`, which always gives back `()`.
    fn while_body(label: Option<&ast::Ident>) -> Self {
        LoopScope::unit_loop(label, LoopKind::While)
    }

    /// The entry of a `for`, which always gives back `()`.
    fn for_body(label: Option<&ast::Ident>) -> Self {
        LoopScope::unit_loop(label, LoopKind::For)
    }

    fn unit_loop(label: Option<&ast::Ident>, kind: LoopKind) -> Self {
        LoopScope { label: label.map(|label| label.name.clone()), kind, break_ty: Some(Ty::Unit), has_break: false }
    }
}

struct FnCx<'a> {
    globals: &'a Globals,
    diags: &'a mut Diagnostics,
    locals: Vec<tir::Local>,
    scopes: Vec<HashMap<String, LocalId>>,
    ret: Ty,
    /// Span of the function's return type annotation, when written.
    pub(super) ret_span: Option<Span>,
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
    /// The body is the initializer of a `const` or a `static`, which is evaluated at compile time: it may not name
    /// a static (`spec/globals.md`, GL1).
    const_init: bool,
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
    /// The loop depth just outside the innermost unrolled reflection loop body, which `break`/`continue` cannot
    /// leave (`spec/reflection.md`, RFL4).
    reflect_loop_barrier: Option<usize>,
    /// The variables of the `for` loops over `Type.fields` and `Type.variants` that are being unrolled, and
    /// the item each one stands for (`spec/reflection.md`, RFL4).
    reflect: reflect::ReflectBindings,
    /// The body is checked to find the concrete type of an `impl Interface` return (`crate::opaque`): the result
    /// type is not declared, and the types of `return` values are collected in `returned`.
    infer_returns: bool,
    returned: Vec<Ty>,
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
            ret_span: None,
            loops: Vec::new(),
            self_ty,
            self_local: None,
            subst: Vec::new(),
            bounds: Vec::new(),
            module,
            unsafe_depth: usize::from(is_unsafe),
            const_init: false,
            symbolic,
            closures,
            capture: None,
            sources,
            fn_name,
            traces,
            write_place: false,
            reflect_loop_barrier: None,
            reflect: HashMap::new(),
            infer_returns: false,
            returned: Vec::new(),
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
        self.globals.resolve_type_bounded(ty, self.self_ty.as_ref(), &self.subst, &self.bounds, self.module, self.diags)
    }

    /// Whether a type parameter's bounds provide `bound`.
    pub(super) fn param_has(&self, ty: &Ty, bound: Bound) -> bool {
        matches!(ty, Ty::Param(_) | Ty::Projection { .. }) && self.globals.satisfies(ty, &bound, &self.bounds)
    }

    pub(super) fn exhaustive(&self, ty: &Ty, pats: &[&Pat]) -> bool {
        crate::exhaust::exhaustive(ty, pats, &self.globals.defs())
    }

    /// Reports E0006 with missing patterns and optional guard / missing-arm placeholder notes.
    pub(super) fn report_non_exhaustive(
        &mut self,
        ty: &Ty,
        pats: &[&Pat],
        span: Span,
        msg: String,
        guarded: usize,
        offer_arms: bool,
    ) {
        let missing = crate::exhaust::missing_patterns(ty, pats, &self.globals.defs());
        let insert_arm = if offer_arms && !missing.is_empty() {
            let text = &self.sources.get(span.file).text;
            let hi = span.hi as usize;
            (hi > 0 && hi <= text.len() && text.as_bytes()[hi - 1] == b'}').then(|| {
                let line_start = text[..hi - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let indent: String = text[line_start..hi - 1].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                let arm_indent = format!("{indent}    ");
                let arms: String = missing.iter().take(8).map(|p| format!("{arm_indent}{p} => todo(),\n")).collect();
                let at = Span::new(span.file, (hi - 1) as u32, (hi - 1) as u32);
                (at, arms)
            })
        } else {
            None
        };
        let d = self.diags.error(Code::NON_EXHAUSTIVE, span, msg);
        if !missing.is_empty() {
            let shown: Vec<String> = missing.iter().take(4).map(|p| format!("`{p}`")).collect();
            let extra = if missing.len() > 4 { format!(" (and {} more)", missing.len() - 4) } else { String::new() };
            d.label(span, format!("pattern(s) {} not covered{extra}", shown.join(", ")));
        } else {
            d.label(span, format!("scrutinee has type `{ty}`"));
        }
        if guarded > 0 {
            d.note_without_span(format!(
                "{guarded} arm(s) have an `if` guard, so they are not counted toward exhaustiveness"
            ));
        }
        if let Some((at, arms)) = insert_arm {
            d.suggest_with(at, "add the missing match arms", arms, burdin_diag::Applicability::HasPlaceholders);
        }
    }

    /// Returns `(decl_span, label)` for an immutable root local of `place`, when it has a source span.
    pub(super) fn root_decl_span(&self, place: &tir::Expr) -> Option<(Span, String)> {
        let root = burdin_tir::place::place_root(place)?;
        let local = &self.locals[root.0];
        (local.span.lo < local.span.hi && !local.mutable)
            .then(|| (local.span, format!("`{}` is declared here without `mut`", local.name)))
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
        Some(PathTy::Ty(self.globals.generic_type(
            name,
            args,
            self_ty.as_ref(),
            &self.subst,
            &self.bounds,
            self.module,
            self.diags,
        )))
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
        // `.Variant`: the enum is the scrutinee's own type, so no name is looked up (TT13).
        if path.first().is_some_and(|first| first.is_inferred()) {
            let Ty::Enum(a) = scrutinee else { return None };
            let type_name = ast::Ident { name: a.name.to_string(), span: path[0].span };
            return Some((scrutinee.clone(), path.get(1).cloned(), self.module, type_name));
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

    /// A name that is not hidden by a local or a global.
    pub(super) fn is_prelude(&self, name: &str) -> bool {
        self.lookup(name).is_none() && self.globals.value(self.module, name).is_none()
    }

    /// Whether `Option` or `Result` written alone means the built-in enum: nothing in scope and no type of the
    /// program has that name.
    pub(super) fn is_prelude_type(&self, name: &str) -> bool {
        self.is_prelude(name)
            && matches!(
                (self.globals.type_name(self.module, name), self.globals.lang_type_name(name)),
                (Some(a), Some(b)) if a == b
            )
    }

    /// `Option` or `Result` as the head of a path expression, as in `Option.Some(x)` or `Result.Ok(v)`.
    pub(super) fn prelude_head(&self, e: &ast::Expr) -> Option<&'static str> {
        let ast::ExprKind::Name { name, args } = &e.kind else { return None };
        if !args.is_empty() || !self.is_prelude_type(&name.name) {
            return None;
        }
        match name.name.as_str() {
            "Option" => Some("Option"),
            "Result" => Some("Result"),
            _ => None,
        }
    }

    /// Reports a bare `Some`, `None`, `Ok`, or `Err` (E0305). `Option.Some` means the same wherever the bare name
    /// did, so the fix is safe.
    pub(super) fn removed_prelude_name(&mut self, id: &ast::Ident) {
        let owner = if matches!(id.name.as_str(), "Some" | "None") { "Option" } else { "Result" };
        let d = self.diags.error(
            Code::REMOVED_PRELUDE_NAME,
            id.span,
            format!(
                "`{0}` is not a name of its own: write `{1}.{0}`, or `.{0}` where the type is known",
                id.name, owner
            ),
        );
        d.suggest_safe(id.span, format!("write `{owner}.{}`", id.name), format!("{owner}.{}", id.name));
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
            tir::ExprKind::Static(id) => self.globals.const_kinds[id.0] == tir::ConstKind::StaticMut,
            tir::ExprKind::Field(base, _) | tir::ExprKind::Deref(base) => self.place_mutable(base),
            tir::ExprKind::DerefRef(base) => matches!(base.ty, Ty::Ref { mutable: true, .. }),
            // Writing through a raw pointer writes C memory; it does not mutate the binding that holds the
            // pointer, and `unsafe` is what guards it (memory-rules.md, US; issue #72).
            tir::ExprKind::PtrDeref(_) => true,
            // Elements of an array or a list place are mutable with it; elements of a slice view are read-only.
            tir::ExprKind::Index(base, _) => {
                (matches!(base.ty, Ty::Array(..)) || self.globals.list_parts(&base.ty).is_some())
                    && self.place_mutable(base)
            }
            _ => false,
        }
    }

    /// The static a place is part of, when its root is one (`spec/globals.md`).
    pub(super) fn static_root(&self, e: &tir::Expr) -> Option<tir::ConstId> {
        match &e.kind {
            tir::ExprKind::Static(id) => Some(*id),
            tir::ExprKind::Field(base, _) | tir::ExprKind::Index(base, _) => self.static_root(base),
            _ => None,
        }
    }

    /// Reports E0304 when `place` is part of a `static mut` that is borrowed: no reference may alias it
    /// (`spec/globals.md`, GL4). Returns whether it did.
    pub(super) fn static_mut_borrowed(&mut self, place: &tir::Expr, span: Span) -> bool {
        let Some(id) = self.static_root(place) else { return false };
        if self.globals.const_kinds[id.0] != tir::ConstKind::StaticMut {
            return false;
        }
        let name = self.globals.const_names[id.0].clone();
        self.diags.error(
            Code::STATIC_MUT_BORROW,
            span,
            format!(
                "`{name}` is a `static mut`, which cannot be borrowed: no reference may alias it, so a read or a \
                 write of it never conflicts (spec/globals.md, GL4)"
            ),
        );
        true
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
        self.mismatch_with(span, expected, found, None);
    }

    pub(super) fn mismatch_with(&mut self, span: Span, expected: &Ty, found: &Ty, origin: Option<(Span, &str)>) {
        let snippet = self.span_snippet(span);
        let d = self.diags.error(Code::TYPE_MISMATCH, span, format!("expected `{expected}`, found `{found}`"));
        d.label(span, format!("found `{found}`"));
        if let Some((origin_span, origin_msg)) = origin
            && origin_span != span
        {
            d.label(origin_span, origin_msg.to_string());
        }
        Self::attach_type_conversion_hints(span, snippet.as_deref(), expected, found, d);
    }

    fn span_snippet(&self, span: Span) -> Option<String> {
        let text = &self.sources.get(span.file).text;
        let (lo, hi) = (span.lo as usize, span.hi as usize);
        (lo <= hi && hi <= text.len()).then(|| text[lo..hi].to_string())
    }

    fn attach_type_conversion_hints(
        span: Span,
        snippet: Option<&str>,
        expected: &Ty,
        found: &Ty,
        d: &mut burdin_diag::Diagnostic,
    ) {
        use burdin_diag::Applicability::MaybeIncorrect;
        let end = Span::new(span.file, span.hi, span.hi);
        let simple = snippet.is_some_and(|s| !s.is_empty() && !s.contains('\n') && !s.contains(' '));
        match (expected, found) {
            (Ty::Str, Ty::String) => {
                if simple {
                    d.suggest_with(end, "borrow as `str` with `.as_str()`", ".as_str()", MaybeIncorrect);
                } else {
                    d.help("borrow the `String` as `str` with `.as_str()`");
                }
            }
            (Ty::String, Ty::Str) => {
                if let Some(s) = snippet.filter(|s| !s.is_empty() && !s.contains('\n')) {
                    d.suggest_with(span, "convert to `String`", format!("String.from({s})"), MaybeIncorrect);
                } else {
                    d.help("convert to `String` with `String.from(...)` or `.to_string()`");
                }
            }
            (Ty::String, Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Char) => {
                if simple {
                    d.suggest_with(end, "convert with `.to_string()`", ".to_string()", MaybeIncorrect);
                } else {
                    d.help("convert to `String` with `.to_string()`");
                }
            }
            (Ty::Enum(adt), _) if adt.name.starts_with("Option<") => {
                if let Some(s) = snippet.filter(|s| !s.is_empty() && !s.contains('\n')) {
                    d.suggest_with(span, "wrap in `Option.Some(...)`", format!("Option.Some({s})"), MaybeIncorrect);
                } else {
                    d.help("wrap the value in `Option.Some(...)`");
                }
            }
            (Ty::Enum(adt), _) if adt.name.starts_with("Result<") => {
                if let Some(s) = snippet.filter(|s| !s.is_empty() && !s.contains('\n')) {
                    d.suggest_with(span, "wrap in `Result.Ok(...)`", format!("Result.Ok({s})"), MaybeIncorrect);
                } else {
                    d.help("wrap the value in `Result.Ok(...)`");
                }
            }
            (_, Ty::Enum(adt)) if adt.name.starts_with("Option<") => {
                d.help("unwrap the `Option` with `.unwrap()`, `.expect(...)`, or `match`");
            }
            (_, Ty::Enum(adt)) if adt.name.starts_with("Result<") => {
                d.help("propagate the error with `?` or unwrap with `.unwrap()` or `match`");
            }
            (Ty::Int(_) | Ty::Float(_), Ty::Int(_) | Ty::Float(_)) => {
                if simple {
                    d.suggest_with(
                        end,
                        format!("cast with `as {expected}`"),
                        format!(" as {expected}"),
                        MaybeIncorrect,
                    );
                } else {
                    d.help(format!("convert with `as {expected}`"));
                }
            }
            (Ty::Ref { mutable: false, inner }, _) if compatible(found, inner) => {
                if simple {
                    let start = Span::new(span.file, span.lo, span.lo);
                    d.suggest_with(start, "borrow with `&`", "&", MaybeIncorrect);
                } else {
                    d.help("borrow the value with `&`");
                }
            }
            (_, Ty::Ref { inner, .. }) if compatible(inner, expected) => {
                d.help("dereference with `*` or copy with `.clone()`");
            }
            _ => {}
        }
    }

    /// Reports E0003 unless `e` has type `want` (or a type that converts to nothing at run time, like `!`).
    fn expect_ty(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        self.expect_ty_from(e, want, None)
    }

    pub(super) fn expect_ty_from(&mut self, e: tir::Expr, want: &Ty, origin: Option<(Span, &str)>) -> tir::Expr {
        if !compatible(&e.ty, want)
            && !self.param_meets_fn(&e.ty, want)
            && !self.globals.opaque_unresolved(want)
            && !self.globals.opaque_unresolved(&e.ty)
        {
            self.report_mismatch_with(e.span, want, &e.ty, origin);
        }
        e
    }

    /// Whether `found` is a type parameter whose closure bound provides `want`: `F: FnOnce(): V` passed where a
    /// built-in method takes the closure type `FnOnce(): V`. Each concrete instance
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
    pub(super) fn report_mismatch_with(&mut self, span: Span, want: &Ty, found: &Ty, origin: Option<(Span, &str)>) {
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
        self.mismatch_with(span, want, found, origin);
    }

    /// Boxes `e` when it is used where `any I` is expected (AN3). Boxing produces an owned value, so unlike the
    /// `String`-to-`str` borrow it applies at every position with a known expected type. A view cannot be boxed
    /// (E0120), and the value must implement the interface (E0141).
    pub(super) fn coerce_any(&mut self, e: tir::Expr, want: &Ty) -> tir::Expr {
        let Ty::Any(any) = want else { return e };
        if matches!(e.ty, Ty::Error | Ty::Never) {
            return e;
        }
        // An `impl Interface` value is boxed through its concrete value, when the interface is one it names.
        let mut e = e;
        if self.globals.opaque_of(&e.ty).is_some() {
            e = match self.open_opaque_for_any(e, any) {
                Ok(inner) => inner,
                Err(typed) => return *typed,
            };
        }
        if let Ty::Any(have) = &e.ty {
            if have == any {
                return e;
            }
            // `any A` where `any B` is expected and `A requires B`: the same box with `B`'s table (AN13).
            let slot = self.globals.any_supers(have.id).iter().position(|s| s.id == any.id);
            if let Some(slot) = slot {
                let span = e.span;
                let from = have.id;
                return mk(tir::ExprKind::AnyUpcast { value: Box::new(e), from, slot }, want.clone(), span);
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

    /// Converts an error type into the dynamic `AnyError` where that type is expected (`spec/errors.md`, ERR4): the
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
