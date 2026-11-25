//! The entry points of body checking: one constant, or one function.

use super::FnCx;
use crate::consts::is_const_expr;
use crate::globals::Globals;
use crate::session::FnItem;
use crate::types::compatible;
use burdin_diag::{Code, Diagnostics, SourceMap};
use burdin_syntax::ast;
use burdin_tir::{self as tir, LocalKind, Signature, Ty};

/// Checks the default argument of every parameter written `name: Type = expr` and stores the checked constant
/// expression in `sig` (`spec/arguments.md`, AG2). A default is checked once, in the function's declaration
/// context, with no parameters or locals in scope; every call that omits the argument inserts the value. The
/// invalid forms (before a required parameter, on a `mut` parameter, depending on a type parameter, or not a
/// constant expression) are E0213.
pub(crate) fn check_defaults(
    globals: &Globals,
    item: &FnItem,
    sig: &mut Signature,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
) {
    let f = item.decl;
    // A default may not precede a required parameter: a positional call could not tell which one an argument
    // fills, so the parameters with defaults must come last.
    let mut first_default: Option<&ast::Param> = None;
    for p in &f.params {
        let ast::ParamKind::Named { name, .. } = &p.kind else { continue };
        match (&p.default, first_default) {
            (None, Some(defaulted)) => {
                let note = defaulted.default.as_ref().map(|d| d.span).unwrap_or(defaulted.span);
                diags
                    .error(
                        Code::INVALID_DEFAULT,
                        name.span,
                        format!(
                            "parameter `{}` has no default but follows one; the parameters with defaults must come last",
                            name.name
                        ),
                    )
                    .note(note, "this parameter has a default");
            }
            (Some(_), None) => first_default = Some(p),
            _ => {}
        }
    }
    for (p, ps) in f
        .params
        .iter()
        .filter(|p| matches!(p.kind, ast::ParamKind::Named { .. }))
        .zip(sig.params.iter_mut().filter(|p| p.name.is_some()))
    {
        let Some(default) = &p.default else { continue };
        if ps.ty == Ty::Error {
            continue;
        }
        if p.convention == ast::Convention::Mut {
            diags.error(
                Code::INVALID_DEFAULT,
                default.span,
                "a `mut` parameter cannot have a default value: the callee writes to the caller's place, and a default has no place",
            );
            continue;
        }
        if globals.ty_has_param(&ps.ty) {
            diags.error(
                Code::INVALID_DEFAULT,
                default.span,
                "a default value cannot depend on a type parameter; give the parameter a concrete type",
            );
            continue;
        }
        let Some(value) = check_one(globals, item, default, &ps.ty, sources, traces, diags) else { continue };
        if !is_const_expr(&value) {
            diags.error(
                Code::INVALID_DEFAULT,
                default.span,
                "a default value must be a constant expression: literals, other constants, operators, and casts",
            );
            continue;
        }
        ps.default = Some(value);
    }
}

/// Checks one default expression in the function's declaration context, where no parameter or local is in
/// scope. Returns `None` when the expression's type is unknown.
fn check_one(
    globals: &Globals,
    item: &FnItem,
    default: &ast::Expr,
    want: &Ty,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
) -> Option<tir::Expr> {
    let closures = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut cx = FnCx::new(
        globals,
        diags,
        Ty::Unit,
        item.self_ty.clone(),
        item.module,
        false,
        item.symbolic,
        closures,
        sources,
        item.name.clone(),
        traces,
    );
    cx.subst = item.subst.clone();
    cx.bounds = item.bounds.clone();
    let value = cx.expr(default, Some(want));
    let value = cx.expect_ty(value, want);
    (value.ty != Ty::Error).then_some(value)
}

/// Checks the initializer of a constant against its declared type. Any expression of a materializable type is
/// allowed: the interpreter evaluates it after the memory checks (`spec/const-eval.md`, CE2). The closures it
/// creates go to the program's list, because the engine may call them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_const(
    globals: &Globals,
    decl: &ast::ConstDecl,
    module: usize,
    ty: &Ty,
    closures: std::rc::Rc<std::cell::RefCell<Vec<tir::Closure>>>,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
) -> tir::Const {
    let mut cx = FnCx::new(
        globals,
        diags,
        Ty::Unit,
        None,
        module,
        false,
        false,
        closures,
        sources,
        decl.name.name.clone(),
        traces,
    );
    // The initializer is evaluated at compile time, so it may not name a static (`spec/globals.md`, GL1).
    cx.const_init = true;
    let value = cx.expr(&decl.value, Some(ty));
    let value = cx.expect_ty(value, ty);
    if !globals.materializable(ty) {
        cx.diags.error(
            Code::NOT_MATERIALIZABLE,
            decl.ty.span,
            format!(
                "a `const` of type `{ty}` cannot be evaluated at compile time: its value is computed by the \
                 compiler and copied into every use, so it must be a scalar, `str`, `String`, or a struct, enum, \
                 tuple, array, `Option`, or `Result` of those (spec/const-eval.md, CE3)"
            ),
        );
    } else if decl.kind != ast::ConstKind::Const {
        // A static is never dropped, so it must be plain data (GL2). Every plain, materializable type is `Sync`, so a
        // static needs no `Sync` check of its own (GL3).
        if !globals.plain(ty) {
            cx.diags.error(
                Code::STATIC_NOT_PLAIN,
                decl.ty.span,
                format!(
                    "a static cannot have type `{ty}`: it is never dropped, so its type must own no memory and have \
                     no destructor (a scalar, `str`, or a struct, enum, tuple, array, `Option`, or `Result` of those) \
                     (spec/globals.md, GL2)"
                ),
            );
        }
    }
    let kind = match decl.kind {
        ast::ConstKind::Const => tir::ConstKind::Const,
        ast::ConstKind::Static => tir::ConstKind::Static,
        ast::ConstKind::StaticMut => tir::ConstKind::StaticMut,
    };
    let locals = std::mem::take(&mut cx.locals);
    tir::Const { name: globals.qualified(module, &decl.name.name), kind, span: decl.name.span, value, locals }
}

/// A function without code. A symbolic function is checked against its bounds and then replaced by one of these:
/// only its diagnostics matter.
fn stub(item: &FnItem) -> tir::Function {
    tir::Function {
        name: item.name.clone(),
        span: item.decl.span,
        params: Vec::new(),
        ret: Ty::Unit,
        locals: Vec::new(),
        body: tir::Block { stmts: Vec::new(), tail: None, close: item.decl.span },
        moved: Vec::new(),
        debug_kills: Vec::new(),
        ret_origins: Vec::new(),
        symbolic: true,
        exported: item.is_exported,
        events: Vec::new(),
        internal: false,
    }
}

/// Checks the body of one function against its signature.
pub(crate) fn check_function(
    globals: &Globals,
    item: &FnItem,
    sig: &Signature,
    closures: std::rc::Rc<std::cell::RefCell<Vec<tir::Closure>>>,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
) -> tir::Function {
    check_function_with(globals, item, sig, closures, sources, traces, diags, false).0
}

/// The types a function with `impl Interface` as its return type returns: its tail and every `return`, with the
/// declared result not checked (`crate::opaque`). Empty when the function has no body to check.
pub(crate) fn infer_return(
    globals: &Globals,
    item: &FnItem,
    sig: &Signature,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
) -> Vec<Ty> {
    let closures = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    check_function_with(globals, item, sig, closures, sources, traces, diags, true).1
}

/// Checks one function body. With `infer`, the result is not declared, and the second value is the types the body
/// returns (the tail and each `return`, without `!`).
#[allow(clippy::too_many_arguments)]
fn check_function_with(
    globals: &Globals,
    item: &FnItem,
    sig: &Signature,
    closures: std::rc::Rc<std::cell::RefCell<Vec<tir::Closure>>>,
    sources: &SourceMap,
    traces: bool,
    diags: &mut Diagnostics,
    infer: bool,
) -> (tir::Function, Vec<Ty>) {
    if !item.check_body || !item.decl.has_body {
        return (stub(item), Vec::new());
    }
    let f = item.decl;
    // A function generated by `derives (Debug)` has one span for every token. What is wrong with it is reported
    // at the offending field by the derive check (E0130), so its own diagnostics would only repeat that. Only the
    // derived `debug` is suppressed: the generated `Display` of an `error enum` needs its own message-template
    // diagnostics (E0193), and so does a generated `from`.
    let derived = f.name.span == f.body.span && f.name.name == "debug";
    let before = diags.len();
    let ret = if infer { Ty::Error } else { sig.ret.clone() };
    let mut cx = FnCx::new(
        globals,
        diags,
        ret.clone(),
        item.self_ty.clone(),
        item.module,
        item.is_unsafe,
        item.symbolic,
        closures,
        sources,
        f.name.name.clone(),
        traces,
    );
    cx.infer_returns = infer;
    cx.subst = item.subst.clone();
    cx.bounds = item.bounds.clone();
    let params = cx.declare_params(f, sig);
    let (mut body, mut body_ty) = cx.block(&f.body, Some(&ret));
    // A `String` tail where the return type is `str` is borrowed, as `return` borrows it.
    if body_ty == Ty::String
        && ret == Ty::Str
        && let Some(tail) = body.tail.take()
    {
        body.tail = Some(Box::new(cx.coerce_arg(*tail, &ret)));
        body_ty = Ty::Str;
    }
    // An `impl Interface` return whose type is not known has its error reported where it is found.
    let unresolved = globals.opaque_unresolved(&ret) || globals.opaque_unresolved(&body_ty);
    if !compatible(&body_ty, &ret) && !unresolved {
        let span = match f.body.stmts.last() {
            Some(ast::Stmt::Expr(e)) => e.span,
            Some(ast::Stmt::Let { span, .. }) => *span,
            None => f.body.span,
        };
        // The same closure-aware codes as an annotated position: returning a borrowing closure is E0171, not a
        // bare type mismatch.
        cx.report_mismatch(span, &ret, &body_ty);
    }
    let mut candidates = std::mem::take(&mut cx.returned);
    if body_ty != Ty::Never {
        candidates.push(body_ty);
    }
    let locals = std::mem::take(&mut cx.locals);
    drop(cx);
    if derived {
        diags.truncate(before);
    }
    if item.symbolic {
        return (stub(item), candidates);
    }
    let function = tir::Function {
        name: item.name.clone(),
        span: f.span,
        params,
        ret,
        locals,
        body,
        moved: Vec::new(),
        debug_kills: Vec::new(),
        ret_origins: Vec::new(),
        symbolic: false,
        exported: item.is_exported,
        events: Vec::new(),
        internal: item.decl.is_priv && globals.std_modules.contains(&item.module),
    };
    (function, candidates)
}

impl FnCx<'_> {
    /// Declares the parameters of `f` as locals, with the conventions its signature gave them.
    fn declare_params(&mut self, f: &ast::FnDecl, sig: &Signature) -> Vec<tir::LocalId> {
        let mut params = Vec::new();
        let mut sig_params = sig.params.iter();
        for p in &f.params {
            let (name, mutable, name_span) = match &p.kind {
                ast::ParamKind::Named { name, .. } => {
                    (name.name.as_str(), p.convention == ast::Convention::Mut, name.span)
                }
                ast::ParamKind::SelfParam if sig.has_self && params.is_empty() => {
                    ("self", p.convention == ast::Convention::Mut, p.span)
                }
                ast::ParamKind::SelfParam => continue,
            };
            let Some(ps) = sig_params.next() else { break };
            let kind = if ps.own { LocalKind::OwnParam } else { LocalKind::BorrowedParam };
            let id = self.declare(name, ps.ty.clone(), mutable, kind, name_span);
            self.locals[id.0].by_ref = ps.by_ref;
            if name == "self" {
                self.self_local = Some(id);
            }
            params.push(id);
        }
        params
    }
}
