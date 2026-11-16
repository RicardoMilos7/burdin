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
        traces,
    );
    cx.subst = item.subst.clone();
    cx.bounds = item.bounds.clone();
    let value = cx.expr(default, Some(want));
    let value = cx.expect_ty(value, want);
    (value.ty != Ty::Error).then_some(value)
}

/// Checks the initializer of a constant against its declared type. Any expression of a materializable type is
/// allowed: the comptime engine evaluates it after the memory checks (`spec/comptime.md`, CT7). The closures it
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
    let mut cx = FnCx::new(globals, diags, Ty::Unit, None, module, false, false, closures, sources, traces);
    let value = cx.expr(&decl.value, Some(ty));
    let value = cx.expect_ty(value, ty);
    if !globals.materializable(ty) {
        cx.diags.error(
            Code::NOT_MATERIALIZABLE,
            decl.ty.span,
            format!(
                "a constant cannot have type `{ty}`: its value is computed at compile time and copied into every \
                 use, so it must be a scalar, `str`, `String`, or a struct, enum, tuple, array, `Option`, or \
                 `Result` of those (spec/comptime.md, CT3)"
            ),
        );
    }
    let locals = std::mem::take(&mut cx.locals);
    tir::Const { name: globals.qualified(module, &decl.name.name), span: decl.name.span, value, locals }
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
        body: tir::Block { stmts: Vec::new(), tail: None },
        moved: Vec::new(),
        ret_origin: None,
        symbolic: true,
        exported: item.is_exported,
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
    if !item.check_body {
        return stub(item);
    }
    let f = item.decl;
    // A function generated by `derives (Debug)` has one span for every token. What is wrong with it is reported
    // at the offending field by the derive check (E0130), so its own diagnostics would only repeat that. Only the
    // derived `debug` is suppressed: the generated `Display` of an `error enum` needs its own message-template
    // diagnostics (E0193), and so does a generated `from`.
    let derived = f.name.span == f.body.span && f.name.name == "debug";
    let before = diags.len();
    let ret = sig.ret.clone();
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
        traces,
    );
    cx.subst = item.subst.clone();
    cx.bounds = item.bounds.clone();
    let params = cx.declare_params(f, sig);
    let (body, body_ty) = cx.block(&f.body, Some(&ret));
    if !compatible(&body_ty, &ret) {
        let span = match f.body.stmts.last() {
            Some(ast::Stmt::Expr(e)) => e.span,
            Some(ast::Stmt::Let { span, .. }) => *span,
            None => f.body.span,
        };
        // The same closure-aware codes as an annotated position: returning a borrowing closure is E0171, not a
        // bare type mismatch.
        cx.report_mismatch(span, &ret, &body_ty);
    }
    let locals = std::mem::take(&mut cx.locals);
    drop(cx);
    if derived {
        diags.truncate(before);
    }
    if item.symbolic {
        return stub(item);
    }
    tir::Function {
        name: item.name.clone(),
        span: f.span,
        params,
        ret,
        locals,
        body,
        moved: Vec::new(),
        ret_origin: None,
        symbolic: false,
        exported: item.is_exported,
    }
}

impl FnCx<'_> {
    /// Declares the parameters of `f` as locals, with the conventions its signature gave them.
    fn declare_params(&mut self, f: &ast::FnDecl, sig: &Signature) -> Vec<tir::LocalId> {
        let mut params = Vec::new();
        let mut sig_params = sig.params.iter();
        for p in &f.params {
            let (name, mutable) = match &p.kind {
                ast::ParamKind::Named { name, .. } => (name.name.as_str(), p.convention == ast::Convention::Mut),
                ast::ParamKind::SelfParam if sig.has_self && params.is_empty() => {
                    ("self", p.convention == ast::Convention::Mut)
                }
                ast::ParamKind::SelfParam => continue,
            };
            let Some(ps) = sig_params.next() else { break };
            let kind = if ps.own { LocalKind::OwnParam } else { LocalKind::BorrowedParam };
            let id = self.declare(name, ps.ty.clone(), mutable, kind);
            self.locals[id.0].by_ref = ps.by_ref;
            if ps.comptime {
                self.comptime_known.push(id);
            }
            if name == "self" {
                self.self_local = Some(id);
            }
            params.push(id);
        }
        params
    }
}
