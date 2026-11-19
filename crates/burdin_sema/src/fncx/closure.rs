//! Closures: capturing from the enclosing function, checking the body, and classifying how it uses its captures
//! (`spec/memory-rules.md` CL1-CL10, `spec/grammar.md` section 4).

use super::{FnCx, error_expr, mk};
use crate::globals::Globals;
use crate::types::compatible;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, CaptureMode, FnKind, FnSig, LocalId, LocalKind, Ty};
use std::rc::Rc;

/// A local of the enclosing function that a closure body may capture.
pub(super) struct OuterLocal {
    name: String,
    id: LocalId,
    ty: Ty,
    mutable: bool,
    kind: LocalKind,
}

/// The state of one closure body being checked: where captures come from, which ones were found, and whether the
/// context requires them to be moved in (CL2).
pub(super) struct CaptureCx {
    outer: Vec<OuterLocal>,
    /// The captures discovered so far, in capture-record order. `captures[i].local` is the body's local.
    captures: Vec<tir::Capture>,
    /// The enclosing local of each capture, parallel to `captures`.
    parents: Vec<LocalId>,
    /// The context stores or returns the closure, so every capture moves.
    move_captures: bool,
    /// A non-Copy capture would have to move out of a read-only place. The move checker reports E0101; the
    /// capability mismatch that follows from it is not reported a second time.
    illegal_move: bool,
}

impl FnCx<'_> {
    /// Whether the enclosing function has a local called `name` that this closure can capture.
    pub(super) fn captures_name(&self, name: &str) -> bool {
        self.capture.as_ref().is_some_and(|ctx| ctx.outer.iter().any(|o| o.name == name))
    }

    /// Resolves a name in a closure body: the closure's own bindings first, then the enclosing function's visible
    /// locals, which are captured on first use (CL1). `None` when the name is not an enclosing local.
    pub(super) fn capture_name(&mut self, name: &str, span: Span) -> Option<tir::Expr> {
        let ctx = self.capture.as_ref()?;
        let outer = ctx.outer.iter().find(|o| o.name == name)?;
        let outer_id = outer.id;
        let ty = outer.ty.clone();
        if let Some(index) = ctx.parents.iter().position(|p| *p == outer_id) {
            let local = ctx.captures[index].local;
            return Some(mk(tir::ExprKind::Local(local), ty, span));
        }
        let mutable = outer.mutable;
        let parent_kind = outer.kind;
        let move_captures = ctx.move_captures;
        let local = self.declare_capture(name, ty.clone(), mutable, parent_kind, move_captures, outer_id, span);
        Some(mk(tir::ExprKind::Local(local), ty, span))
    }

    #[allow(clippy::too_many_arguments)]
    fn declare_capture(
        &mut self,
        name: &str,
        ty: Ty,
        mutable: bool,
        parent_kind: LocalKind,
        move_captures: bool,
        parent: LocalId,
        span: Span,
    ) -> LocalId {
        // A move capture takes the value out of an enclosing place; the move checker reports E0101 when that place
        // is only borrowed (CL4).
        let _ = span;
        if move_captures
            && !self.globals.is_copy(&ty)
            && matches!(parent_kind, LocalKind::BorrowedParam | LocalKind::View)
            && let Some(ctx) = self.capture.as_mut()
        {
            ctx.illegal_move = true;
        }
        let mode = if move_captures { CaptureMode::Move } else { CaptureMode::Borrow };
        let kind = if move_captures { LocalKind::OwnParam } else { LocalKind::BorrowedParam };
        // A moved capture lives in the closure's own record, so the body may write it; a borrowed one is as
        // mutable as the place it borrows.
        let mutable = if move_captures { true } else { mutable };
        // The capture is a compiler-made copy of the enclosing local, so it has no span of its own.
        let id = self.declare(name, ty.clone(), mutable, kind, Span::new(span.file, span.lo, span.lo));
        self.locals[id.0].by_ref = !move_captures;
        let ctx = self.capture.as_mut().expect("a capture context is open");
        ctx.parents.push(parent);
        ctx.captures.push(tir::Capture { local: id, mode, ty });
        id
    }

    /// The expected closure signature and capability, and whether the context owns the closure. A written
    /// closure type is owned; a type parameter with an `Fn` bound keeps the concrete closure type. An `FnOnce`
    /// bound means the closure is consumed, so its captures move.
    fn expected_closure(&self, expect: Option<&Ty>) -> Option<(FnSig, FnKind, bool)> {
        match expect? {
            Ty::Fn { sig, kind, borrowed, .. } => Some(((**sig).clone(), *kind, !*borrowed)),
            Ty::Param(_) => {
                self.globals.fn_call_sig(expect?, &self.bounds).map(|(sig, kind)| (sig, kind, kind == FnKind::FnOnce))
            }
            _ => None,
        }
    }

    /// The locals visible where the closure is written, innermost first, for capture-on-use.
    fn visible_locals(&self) -> Vec<OuterLocal> {
        let mut out: Vec<OuterLocal> = Vec::new();
        for scope in self.scopes.iter().rev() {
            for (name, id) in scope {
                if out.iter().any(|o| &o.name == name) {
                    continue;
                }
                let local = &self.locals[id.0];
                out.push(OuterLocal {
                    name: name.clone(),
                    id: *id,
                    ty: local.ty.clone(),
                    mutable: local.mutable,
                    kind: local.kind,
                });
            }
        }
        out
    }

    /// Checks a closure expression and returns its typed form. `expect` supplies the parameter and return types
    /// when the closure has no annotations; it also decides whether the captures move (CL2).
    pub(super) fn closure(
        &mut self,
        params: &[ast::ClosureParam],
        ret: Option<&ast::Type>,
        body: &ast::Expr,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let expected = self.expected_closure(expect);
        let move_captures = expected.as_ref().is_some_and(|(_, _, own)| *own);
        let arity_ok = expected.as_ref().is_none_or(|(sig, _, _)| sig.params.len() == params.len());

        // Parameter types: written, or from the expected signature.
        let mut param_tys = Vec::new();
        for (i, p) in params.iter().enumerate() {
            match &p.ty {
                Some(t) => param_tys.push(self.resolve(t)),
                None => match expected.as_ref().and_then(|(sig, _, _)| sig.params.get(i)) {
                    Some(t) => param_tys.push(t.clone()),
                    None => {
                        if arity_ok {
                            self.diags.error(
                                Code::TYPE_MISMATCH,
                                p.name.span,
                                format!(
                                    "cannot tell the type of the closure parameter `{}`; annotate it, as in `|{}: i32|`",
                                    p.name.name, p.name.name
                                ),
                            );
                        }
                        param_tys.push(Ty::Error);
                    }
                },
            }
        }
        if let Some((sig, _, _)) = &expected
            && !arity_ok
        {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!(
                    "this closure takes {} parameter(s), but the expected closure type takes {}",
                    params.len(),
                    sig.params.len()
                ),
            );
        }

        // Return type: written, expected, or inferred from the body.
        let annotated = ret.map(|t| self.resolve(t));
        let expected_ret = expected.as_ref().map(|(sig, _, _)| (*sig.ret).clone());
        // An expected return type that is still a type parameter (`Fn(T): U`) is an inference hole: a written
        // annotation says what the parameter stands for, so it is checked by unifying, not as a mismatch.
        if let (Some(a), Some(e)) = (&annotated, &expected_ret)
            && !self.globals.ty_has_param(e)
            && !compatible(a, e)
        {
            self.mismatch(span, e, a);
        }
        let declared_ret = annotated.clone().or(expected_ret.clone());
        if declared_ret.is_none() && contains_return(body) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                "this closure's body uses `return`, so write its return type after the closing `|`",
            );
        }

        // Capture the body's free names before the child is created, so a nested closure's names are captured
        // through this closure's record (CL1). In a top-level function there is no capture context and the
        // child's snapshot of the visible locals is enough.
        for (name, use_span) in free_names(body, params) {
            self.capture_name(&name, use_span);
        }

        // The body is its own function: a fresh set of locals, its own return type and loops, and the
        // enclosing function's locals as capture sources.
        let outer = self.visible_locals();
        let closures = self.closures.clone();
        let subst = self.subst.clone();
        let bounds = self.bounds.clone();
        let self_ty = self.self_ty.clone();
        let module = self.module;
        let is_unsafe = self.unsafe_depth > 0;
        let child_ret = declared_ret.clone().unwrap_or(Ty::Error);
        let mut child = FnCx::new(
            self.globals,
            &mut *self.diags,
            child_ret,
            self_ty,
            module,
            is_unsafe,
            self.symbolic,
            closures,
            self.sources,
            self.fn_name.clone(),
            self.traces,
        );
        child.subst = subst;
        child.bounds = bounds;
        child.capture =
            Some(CaptureCx { outer, captures: Vec::new(), parents: Vec::new(), move_captures, illegal_move: false });
        let mut param_ids = Vec::new();
        for (p, ty) in params.iter().zip(&param_tys) {
            param_ids.push(child.declare(&p.name.name, ty.clone(), false, LocalKind::OwnParam, p.name.span));
        }

        // An expected return type that still mentions a type parameter (`Option<U>` in `F: Fn(T): Option<U>`) is
        // only partly known, so it cannot steer the body's literals: the body's type is what the parameter
        // stands for.
        let open_return = annotated.is_none() && declared_ret.as_ref().is_some_and(|t| self.globals.ty_has_param(t));
        let body_expect = if open_return { None } else { declared_ret.clone() };
        let (block, body_ty) = match &body.kind {
            ast::ExprKind::Block(b) => child.block(b, body_expect.as_ref()),
            _ => {
                let e = child.expr(body, body_expect.as_ref());
                let ty = e.ty.clone();
                (tir::Block { stmts: Vec::new(), tail: Some(Box::new(e)) }, ty)
            }
        };
        // An expected return type that is still a type parameter (`F: Fn(T): U` in a generic method) is an
        // inference hole: the body's value is what `U` stands for. A written annotation is always enforced.
        let ret_ty = match declared_ret.clone() {
            Some(_) if open_return && body_ty != Ty::Never => body_ty.clone(),
            Some(t) => t,
            None => {
                if body_ty == Ty::Never {
                    Ty::Unit
                } else {
                    body_ty.clone()
                }
            }
        };
        if !open_return && !compatible(&body_ty, &ret_ty) {
            child.mismatch(body.span, &ret_ty, &body_ty);
        }
        // A view returned by a closure would point into the closure's capture record, which is gone once the
        // call returns. Stage 1 closures return owned values (CL9).
        if ret_ty.is_second_class() {
            child.diags.error(
                Code::CLOSURE_ESCAPES,
                span,
                "a closure cannot return a view; return an owned value instead",
            );
        }

        // Classify the captures from how the body uses them (CL3, CL5).
        let ctx = child.capture.take().expect("the closure's capture context is open");
        let usage = analyze(&block, &ctx.captures, child.globals);
        let mut modes = Vec::new();
        for (i, _) in ctx.captures.iter().enumerate() {
            modes.push(if ctx.move_captures {
                CaptureMode::Move
            } else if usage.mutated[i] {
                CaptureMode::BorrowMut
            } else {
                CaptureMode::Borrow
            });
        }
        let illegal_move = ctx.illegal_move;
        let mut kind = if ctx
            .captures
            .iter()
            .enumerate()
            .any(|(i, c)| modes[i] == CaptureMode::Move && usage.moved[i] && !child.globals.is_copy(&c.ty))
        {
            FnKind::FnOnce
        } else if usage.mutated.iter().any(|m| *m) {
            FnKind::FnMut
        } else {
            FnKind::Fn
        };
        // A capture that cannot legally move has already been reported as E0101. Keep the closure at the
        // capability its context expects, so the same mistake is not reported again as a closure-type mismatch.
        // The body is still checked as the analyzed kind, so the illegal move is not reported twice there.
        let body_kind = kind;
        if illegal_move && let Some((_, expected_kind, _)) = &expected {
            kind = *expected_kind;
        }
        // The body's capture locals follow the final kind: an `FnOnce` body owns its captures (and drops the
        // remainder), while an `Fn`/`FnMut` body borrows them from the record for the call.
        for (i, capture) in ctx.captures.iter().enumerate() {
            let local = &mut child.locals[capture.local.0];
            let owned = modes[i] == CaptureMode::Move && body_kind == FnKind::FnOnce;
            local.kind = if owned { LocalKind::OwnParam } else { LocalKind::BorrowedParam };
            local.by_ref = !owned;
        }
        // A closure is second-class when it borrows a place, and also when a moved-in capture is itself a view
        // (`str`, `&T`, a slice): the record then holds a pointer to storage it does not own (CL3).
        let borrowed =
            modes.iter().any(|m| *m != CaptureMode::Move) || ctx.captures.iter().any(|c| c.ty.is_second_class());
        // Whether the closure may move to another task: each capture it moves must be `Send`, each one it only reads
        // must be `Sync` (a shared view of it crosses), and each one it writes must be `Send` (an exclusive view).
        let send = ctx
            .captures
            .iter()
            .zip(&modes)
            .find_map(|(c, mode)| {
                let sync = *mode == CaptureMode::Borrow;
                let why = child.globals.marker_violation(&c.ty, sync, &child.bounds)?;
                let (how, name) = match mode {
                    CaptureMode::Move => ("moves", "Send"),
                    CaptureMode::Borrow => ("borrows", "Sync"),
                    CaptureMode::BorrowMut => ("borrows mutably", "Send"),
                };
                Some(format!(
                    "the closure {how} `{}` of type `{}`, which is not `{name}`: {why}",
                    child.locals[c.local.0].name, c.ty
                ))
            })
            .map(tir::FnSend::no)
            .unwrap_or_else(tir::FnSend::yes);
        let sig = FnSig { params: param_tys, ret: Box::new(ret_ty.clone()) };
        if let Some((_, expected_kind, _)) = &expected
            && !kind.at_least(*expected_kind)
            && arity_ok
        {
            child.diags.error(
                Code::BOUND_NOT_SATISFIED,
                span,
                format!(
                    "this closure is `{}`, which cannot be used where `{}` is expected",
                    kind.name(),
                    expected_kind.name()
                ),
            );
        }

        let captures: Vec<tir::Capture> = ctx
            .captures
            .iter()
            .zip(&modes)
            .map(|(c, mode)| tir::Capture { local: c.local, mode: *mode, ty: c.ty.clone() })
            .collect();
        let index = self.closures.borrow().len();
        let func = tir::Function {
            name: format!("closure.{index}"),
            span,
            params: param_ids,
            ret: ret_ty.clone(),
            locals: child.locals,
            body: block,
            moved: Vec::new(),
            debug_kills: Vec::new(),
            ret_origins: Vec::new(),
            symbolic: false,
            exported: false,
        };
        let closure =
            tir::Closure { name: func.name.clone(), span, kind, sig: sig.clone(), captures: captures.clone(), func };
        self.closures.borrow_mut().push(closure);

        let expr_captures: Vec<tir::Capture> = ctx
            .parents
            .iter()
            .zip(&captures)
            .map(|(parent, c)| tir::Capture { local: *parent, mode: c.mode, ty: c.ty.clone() })
            .collect();
        // A closure with the wrong arity already reported E0004; giving it the expected type keeps the same
        // mistake from being reported again as a type mismatch by the enclosing position.
        let ty = match expect {
            Some(t @ Ty::Fn { .. }) if !arity_ok => t.clone(),
            _ => Ty::Fn { sig: Rc::new(sig), kind, borrowed, send },
        };
        mk(tir::ExprKind::Closure { closure: index, captures: expr_captures }, ty, span)
    }

    /// Checks a call whose callee is a closure value. A callee whose type is a type parameter needs an `Fn`-family
    /// bound (CL6, E0141).
    pub(super) fn closure_call(&mut self, callee: tir::Expr, args: &[ast::Arg], span: Span) -> tir::Expr {
        // A reference to a closure is called through the reference (RF1).
        let callee = self.auto_deref(callee);
        let Some((sig, kind)) = self.globals.fn_call_sig(&callee.ty, &self.bounds) else {
            let found = callee.ty.clone();
            if matches!(found, Ty::Param(_)) {
                self.diags.error(
                    Code::BOUND_NOT_SATISFIED,
                    span,
                    format!("`{found}` cannot be called; add an `Fn`-family bound"),
                );
            } else {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`{found}` is not callable; only a closure or a function can be called"),
                );
            }
            return self.args_then_error(args, span);
        };
        if kind == FnKind::FnMut && !matches!(callee.ty, Ty::Error) {
            // An `FnMut` call borrows the closure exclusively: the receiver must be a mutable place (CL6).
            if !burdin_tir::place::is_place_expr(&callee) || !self.place_mutable(&callee) {
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    callee.span,
                    "calling this `FnMut` closure changes it, so it must be a mutable place",
                );
            }
        }
        let params: Vec<tir::ParamSig> = sig
            .params
            .iter()
            .map(|ty| tir::ParamSig {
                ty: ty.clone(),
                by_ref: false,
                mut_param: false,
                own: true,
                exclusive: false,
                name: None,
                default: None,
                comptime: false,
            })
            .collect();
        let receiver = Some((&callee, kind != FnKind::Fn));
        let Some(checked) = self.args_pre("this closure", &params, args, receiver, span, None) else {
            return super::error_expr(span);
        };
        let ret = (*sig.ret).clone();
        mk(tir::ExprKind::ClosureCall { callee: Box::new(callee), args: checked, kind }, ret, span)
    }

    /// A call whose callee and arguments are already typed: the handler a `value orelse |e| ...` lowers to. There
    /// are no AST arguments to check, so only the arity and the `FnMut` receiver rule apply (CL6).
    pub(super) fn closure_call_typed(&mut self, callee: tir::Expr, args: Vec<tir::Expr>, span: Span) -> tir::Expr {
        let callee = self.auto_deref(callee);
        let Some((sig, kind)) = self.globals.fn_call_sig(&callee.ty, &self.bounds) else {
            self.diags.error(
                Code::TYPE_MISMATCH,
                span,
                format!("`{}` is not callable; only a closure or a function can be called", callee.ty),
            );
            return error_expr(span);
        };
        if kind == FnKind::FnMut && !matches!(callee.ty, Ty::Error) {
            // An `FnMut` call borrows the closure exclusively: the receiver must be a mutable place (CL6).
            if !burdin_tir::place::is_place_expr(&callee) || !self.place_mutable(&callee) {
                self.diags.error(
                    Code::MUT_OF_IMMUTABLE,
                    callee.span,
                    "calling this `FnMut` closure changes it, so it must be a mutable place",
                );
            }
        }
        if sig.params.len() != args.len() {
            self.diags.error(
                Code::ARG_COUNT,
                span,
                format!("this closure takes {} argument(s), but {} were given", sig.params.len(), args.len()),
            );
            return error_expr(span);
        }
        let ret = (*sig.ret).clone();
        mk(tir::ExprKind::ClosureCall { callee: Box::new(callee), args, kind }, ret, span)
    }
}

/// How a closure body uses its captures.
struct Usage {
    moved: Vec<bool>,
    mutated: Vec<bool>,
}

/// Whether an expression is a capture of the closure being checked, by its root local.
fn capture_of(e: &tir::Expr, captures: &[tir::Capture]) -> Option<usize> {
    use tir::ExprKind as K;
    match &e.kind {
        K::Local(id) => captures.iter().position(|c| c.local == *id),
        K::Field(base, _) | K::Deref(base) | K::PtrDeref(base) | K::DerefRef(base) => capture_of(base, captures),
        K::Index(base, _) => capture_of(base, captures),
        _ => None,
    }
}

/// Walks the typed body once and records which captures it moves and which it writes. This is the sema-side
/// classification that decides `Fn`/`FnMut`/`FnOnce`; the move checker later enforces the move rules themselves.
fn analyze(block: &tir::Block, captures: &[tir::Capture], globals: &Globals) -> Usage {
    let mut usage = Usage { moved: vec![false; captures.len()], mutated: vec![false; captures.len()] };
    walk_block(block, captures, globals, &mut usage);
    usage
}

fn walk_block(block: &tir::Block, captures: &[tir::Capture], globals: &Globals, usage: &mut Usage) {
    for stmt in &block.stmts {
        match stmt {
            tir::Stmt::Let(pat, init, else_block) => {
                walk(init, captures, globals, usage, Mode::Move);
                if let Some(else_block) = else_block {
                    walk_block(else_block, captures, globals, usage);
                }
                let _ = pat;
            }
            tir::Stmt::Expr(e) => walk(e, captures, globals, usage, Mode::Move),
        }
    }
    if let Some(tail) = &block.tail {
        walk(tail, captures, globals, usage, Mode::Move);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Move,
    Use,
}

fn walk(e: &tir::Expr, captures: &[tir::Capture], globals: &Globals, usage: &mut Usage, mode: Mode) {
    use tir::ExprKind as K;
    let moved_place = |e: &tir::Expr, globals: &Globals, usage: &mut Usage| {
        if mode == Mode::Move
            && !globals.is_copy(&e.ty)
            && let Some(i) = capture_of(e, captures)
        {
            usage.moved[i] = true;
        }
    };
    let mutated_place = |e: &tir::Expr, usage: &mut Usage| {
        if let Some(i) = capture_of(e, captures) {
            usage.mutated[i] = true;
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
        K::Local(_) => moved_place(e, globals, usage),
        // The base may be a call: a user `Index`/`IndexMut` is lowered to one, and the call's conventions decide
        // whether the captured receiver is mutated (`spec/operators.md`, OP10).
        K::Field(base, _) | K::Deref(base) | K::PtrDeref(base) | K::DerefRef(base) => {
            moved_place(e, globals, usage);
            walk(base, captures, globals, usage, Mode::Use);
        }
        K::Index(base, index) => {
            moved_place(e, globals, usage);
            walk(base, captures, globals, usage, Mode::Use);
            walk(index, captures, globals, usage, Mode::Use);
        }
        K::Ref { place, mutable } => {
            if *mutable {
                mutated_place(place, usage);
            }
            walk(place, captures, globals, usage, Mode::Use);
        }
        K::Slice { base, lo, hi, .. } => {
            for x in [base.as_ref(), lo.as_ref(), hi.as_ref()] {
                walk(x, captures, globals, usage, Mode::Use);
            }
        }
        K::StringAsStr(inner) => walk(inner, captures, globals, usage, Mode::Use),
        // Boxing moves (or copies) the value into the box; `downcast` consumes the box; `is` only reads it.
        K::BoxAny { value, .. } => walk(value, captures, globals, usage, Mode::Move),
        K::AnyIs { value, .. } => walk(value, captures, globals, usage, Mode::Use),
        K::AnyDowncast { value, .. } => walk(value, captures, globals, usage, Mode::Move),
        K::AnyCall { recv, params, args, .. } => {
            walk(recv, captures, globals, usage, Mode::Use);
            for (a, p) in args.iter().zip(params) {
                walk(a, captures, globals, usage, if p.own { Mode::Move } else { Mode::Use });
            }
        }
        K::ExternCall(_, args) => {
            for a in args {
                walk(a, captures, globals, usage, Mode::Use);
            }
        }
        K::Call(f, args) => {
            // The callee's conventions decide whether a capture moves (`own`) or is only borrowed.
            let sig = globals.sigs.get(f.0);
            for (i, a) in args.iter().enumerate() {
                let param = sig.and_then(|s| s.params.get(i));
                if param.is_some_and(|p| p.mut_param) {
                    mutated_place(a, usage);
                }
                let mode = if param.is_some_and(|p| p.own) { Mode::Move } else { Mode::Use };
                walk(a, captures, globals, usage, mode);
            }
        }
        K::ClosureCall { callee, args, kind } => {
            // The callee is moved for `FnOnce`, exclusively borrowed for `FnMut`, and read for `Fn`.
            let callee_mode = if *kind == FnKind::FnOnce { Mode::Move } else { Mode::Use };
            if *kind == FnKind::FnMut {
                mutated_place(callee, usage);
            }
            walk(callee, captures, globals, usage, callee_mode);
            for a in args {
                walk(a, captures, globals, usage, Mode::Move);
            }
        }
        K::Builtin(b, args) => match b {
            tir::Builtin::HeapNew | tir::Builtin::BlackBox => walk(&args[0], captures, globals, usage, Mode::Move),
            // `with_capacity_in(n, alloc)` moves the allocator value into the list it makes.
            tir::Builtin::ListWithCapacityIn => {
                walk(&args[0], captures, globals, usage, Mode::Use);
                walk(&args[1], captures, globals, usage, Mode::Move);
            }
            tir::Builtin::Replace => {
                mutated_place(&args[0], usage);
                walk(&args[0], captures, globals, usage, Mode::Use);
                walk(&args[1], captures, globals, usage, Mode::Move);
            }
            tir::Builtin::Swap => {
                for a in args {
                    mutated_place(a, usage);
                    walk(a, captures, globals, usage, Mode::Use);
                }
            }
            _ => {
                for a in args {
                    walk(a, captures, globals, usage, Mode::Use);
                }
            }
        },
        K::Method(m, args) => {
            for (i, a) in args.iter().enumerate() {
                if i == 0 && m.mutates_receiver() {
                    mutated_place(a, usage);
                    walk(a, captures, globals, usage, Mode::Use);
                } else if m.owns_argument(i) {
                    walk(a, captures, globals, usage, Mode::Move);
                } else {
                    walk(a, captures, globals, usage, Mode::Use);
                }
            }
        }
        K::Closure { captures: inner, .. } => {
            // A nested closure captures places of this closure's body: a move capture moves them, a mutable
            // borrow writes them. It may be called later, so any mutation is attributed to this body too.
            for c in inner {
                if let Some(i) = captures.iter().position(|outer| outer.local == c.local) {
                    match c.mode {
                        CaptureMode::Move if !globals.is_copy(&c.ty) => usage.moved[i] = true,
                        CaptureMode::BorrowMut => usage.mutated[i] = true,
                        _ => {}
                    }
                }
            }
        }
        K::Block(b) => walk_block(b, captures, globals, usage),
        K::If(cond, then, other) => {
            walk(cond, captures, globals, usage, Mode::Use);
            walk_block(then, captures, globals, usage);
            if let Some(other) = other {
                // The `else` expression is the value of the `if`, so it is used the way the `if` is.
                walk(other, captures, globals, usage, mode);
            }
        }
        K::While(cond, body) => {
            walk(cond, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::Loop(body) => walk_block(body, captures, globals, usage),
        K::ForRange { lo, hi, body, .. } => {
            walk(lo, captures, globals, usage, Mode::Use);
            walk(hi, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::ForRangeValue { range, body, .. } => {
            walk(range, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::ForArray { array, body, .. } | K::ComptimeFor { iter: array, body, .. } => {
            walk(array, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        // A comptime expression cannot use a capture (spec/comptime.md, CT2), but it is walked for its own code.
        K::Comptime(inner) => walk(inner, captures, globals, usage, mode),
        K::ForSlice { slice, body, .. } => {
            walk(slice, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::ForList { list, body, .. } => {
            walk(list, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::ForMap { map, body, .. } => {
            walk(map, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::ForChars { string, body, .. } => {
            walk(string, captures, globals, usage, Mode::Use);
            walk_block(body, captures, globals, usage);
        }
        K::Match(scrutinee, arms) => {
            walk(scrutinee, captures, globals, usage, Mode::Use);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk(guard, captures, globals, usage, Mode::Use);
                }
                // An arm's body is the value of the `match`, so it is used the way the `match` is.
                walk(&arm.body, captures, globals, usage, mode);
            }
        }
        K::Return(value) => {
            if let Some(v) = value {
                walk(v, captures, globals, usage, Mode::Move);
            }
        }
        K::Assign(place, value) => {
            walk(value, captures, globals, usage, Mode::Move);
            mutated_place(place, usage);
            walk(place, captures, globals, usage, Mode::Use);
        }
        K::Binary(_, l, r) => {
            walk(l, captures, globals, usage, Mode::Use);
            walk(r, captures, globals, usage, Mode::Use);
        }
        K::Range { lo, hi, .. } => {
            walk(lo, captures, globals, usage, Mode::Move);
            walk(hi, captures, globals, usage, Mode::Move);
        }
        K::Unary(_, inner) | K::Cast(inner) | K::ArrayRepeat(inner, _) => {
            walk(inner, captures, globals, usage, Mode::Use)
        }
        K::Array(elems) | K::Tuple(elems) => {
            for x in elems {
                walk(x, captures, globals, usage, Mode::Move);
            }
        }
        K::Struct(fields) | K::Variant(_, fields) => {
            for (_, x) in fields {
                walk(x, captures, globals, usage, Mode::Move);
            }
        }
        K::Some(inner) | K::Ok(inner) | K::Err(inner) => walk(inner, captures, globals, usage, Mode::Move),
    }
}

/// Every name the body uses but does not bind itself, in first-use order. Nested closures' parameters count as
/// bindings for their own bodies; their free names are free in this body too. Used to capture eagerly, so a
/// nested closure captures through the enclosing closure's record (CL1).
pub(super) fn free_names(body: &ast::Expr, params: &[ast::ClosureParam]) -> Vec<(String, Span)> {
    let mut walker = FreeNames { scopes: vec![params.iter().map(|p| p.name.name.clone()).collect()], out: Vec::new() };
    walker.expr(body);
    walker.out
}

struct FreeNames {
    scopes: Vec<Vec<String>>,
    out: Vec<(String, Span)>,
}

impl FreeNames {
    fn bound(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.iter().any(|n| n == name))
    }

    fn use_name(&mut self, name: &str, span: Span) {
        if !self.bound(name) && !self.out.iter().any(|(n, _)| n == name) {
            self.out.push((name.to_string(), span));
        }
    }

    fn bind(&mut self, pat: &ast::Pattern) {
        let mut names = Vec::new();
        pattern_names(pat, &mut names);
        self.scopes.last_mut().expect("a scope is open").extend(names);
    }

    fn block(&mut self, b: &ast::Block) {
        self.scopes.push(Vec::new());
        for stmt in &b.stmts {
            match stmt {
                ast::Stmt::Let { pattern, init, else_block, .. } => {
                    self.expr(init);
                    if let Some(else_block) = else_block {
                        self.block(else_block);
                    }
                    self.bind(pattern);
                }
                ast::Stmt::Expr(e) => self.expr(e),
            }
        }
        // The block's value is its last expression statement; scanning statements already covered it.
        self.scopes.pop();
    }

    fn expr(&mut self, e: &ast::Expr) {
        use ast::ExprKind as K;
        match &e.kind {
            K::Name { name, .. } => self.use_name(&name.name, name.span),
            K::TypeArgs(base, _) => self.expr(base),
            K::DynField(base, name) => {
                self.expr(base);
                self.expr(name);
            }
            K::SelfValue => self.use_name("self", e.span),
            K::Unit | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Bool(_) | K::Break | K::Continue => {}
            K::Interp(parts) => {
                for part in parts {
                    if let ast::InterpPart::Hole(ast::Hole { arg: ast::HoleArg::Expr(e), .. }) = part {
                        self.expr(e);
                    }
                }
            }
            K::Tuple(es) | K::Array(es) => es.iter().for_each(|x| self.expr(x)),
            K::ArrayRepeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            K::Block(b) | K::UnsafeBlock(b) | K::Loop(b) => self.block(b),
            K::If { cond, then_block, else_expr } => {
                self.expr(cond);
                self.block(then_block);
                if let Some(other) = else_expr {
                    self.expr(other);
                }
            }
            K::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            K::For { pattern, iter, body, .. } => {
                self.expr(iter);
                self.scopes.push(Vec::new());
                self.bind(pattern);
                self.block_body(body);
                self.scopes.pop();
            }
            K::Comptime(inner) => self.expr(inner),
            K::ComptimeAssert(cond, message) => {
                self.expr(cond);
                if let Some(m) = message {
                    self.expr(m);
                }
            }
            K::Return(value) => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            K::Assign(a, b) | K::Binary(_, a, b) | K::OrElse(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            K::Unary(_, a) | K::Cast(a, _) | K::Try(a) | K::Field(a, _) | K::TupleField(a, _, _) => self.expr(a),
            K::Range { lo, hi, .. } => {
                self.expr(lo);
                self.expr(hi);
            }
            K::Call(callee, args) => {
                self.expr(callee);
                args.iter().for_each(|a| self.expr(&a.expr));
            }
            K::Index(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee);
                for arm in arms {
                    self.scopes.push(Vec::new());
                    if let Some(pat) = &arm.pattern {
                        self.bind(pat);
                    }
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&arm.body);
                    self.scopes.pop();
                }
            }
            K::Closure { params, body, .. } => {
                self.scopes.push(params.iter().map(|p| p.name.name.clone()).collect());
                self.expr(body);
                self.scopes.pop();
            }
            K::StructLit(_, fields) => {
                for f in fields {
                    if let Some(v) = &f.value {
                        self.expr(v);
                    } else {
                        self.use_name(&f.name.name, f.name.span);
                    }
                }
            }
            K::IfLet { pattern, scrutinee, then_block, else_expr } => {
                self.expr(scrutinee);
                self.scopes.push(Vec::new());
                self.bind(pattern);
                self.block_body(then_block);
                self.scopes.pop();
                if let Some(other) = else_expr {
                    self.expr(other);
                }
            }
        }
    }

    /// A block that already has an enclosing scope for its bindings (`for`, `if let`).
    fn block_body(&mut self, b: &ast::Block) {
        for stmt in &b.stmts {
            match stmt {
                ast::Stmt::Let { pattern, init, else_block, .. } => {
                    self.expr(init);
                    if let Some(else_block) = else_block {
                        self.block(else_block);
                    }
                    self.bind(pattern);
                }
                ast::Stmt::Expr(e) => self.expr(e),
            }
        }
    }
}

/// The names a pattern binds.
fn pattern_names(pat: &ast::Pattern, out: &mut Vec<String>) {
    match &pat.kind {
        ast::PatternKind::Wild | ast::PatternKind::Lit(_) | ast::PatternKind::Range(..) | ast::PatternKind::Path(_) => {
        }
        ast::PatternKind::Binding { name, .. } => out.push(name.name.clone()),
        ast::PatternKind::Tuple(subs) | ast::PatternKind::TupleStruct(_, subs) => {
            subs.iter().for_each(|p| pattern_names(p, out))
        }
        ast::PatternKind::Struct(_, fields) => {
            for f in fields {
                match &f.pattern {
                    Some(p) => pattern_names(p, out),
                    None => out.push(f.name.name.clone()),
                }
            }
        }
        ast::PatternKind::Or(alts) => alts.iter().for_each(|p| pattern_names(p, out)),
        ast::PatternKind::At(name, inner) => {
            out.push(name.name.clone());
            pattern_names(inner, out);
        }
    }
}

/// Whether the closure's body uses `return` at its own level. A nested closure's `return` belongs to that
/// closure, not this one.
fn contains_return(e: &ast::Expr) -> bool {
    use ast::ExprKind as K;
    match &e.kind {
        K::Closure { .. } => false,
        K::Return(_) => true,
        K::Unit
        | K::Int(_)
        | K::Float(_)
        | K::Char(_)
        | K::Str(_)
        | K::Bool(_)
        | K::Name { .. }
        | K::SelfValue
        | K::Break
        | K::Continue => false,
        K::Interp(parts) => parts.iter().any(|part| {
            matches!(part, ast::InterpPart::Hole(ast::Hole { arg: ast::HoleArg::Expr(e), .. }) if contains_return(e))
        }),
        K::Tuple(es) | K::Array(es) => es.iter().any(contains_return),
        K::ArrayRepeat(a, b) => contains_return(a) || contains_return(b),
        K::TypeArgs(base, _) => contains_return(base),
        K::DynField(a, b) => contains_return(a) || contains_return(b),
        K::Block(b) | K::UnsafeBlock(b) | K::Loop(b) => block_has_return(b),
        K::If { cond, then_block, else_expr } => {
            contains_return(cond) || block_has_return(then_block) || else_expr.as_deref().is_some_and(contains_return)
        }
        K::While { cond, body } => contains_return(cond) || block_has_return(body),
        K::For { iter, body, .. } => contains_return(iter) || block_has_return(body),
        K::Comptime(inner) => contains_return(inner),
        K::ComptimeAssert(cond, message) => contains_return(cond) || message.as_deref().is_some_and(contains_return),
        K::Assign(a, b) | K::Binary(_, a, b) | K::OrElse(a, b) => contains_return(a) || contains_return(b),
        K::Unary(_, a) | K::Cast(a, _) | K::Try(a) | K::Field(a, _) | K::TupleField(a, _, _) => contains_return(a),
        K::Range { lo, hi, .. } => contains_return(lo) || contains_return(hi),
        K::Call(callee, args) => contains_return(callee) || args.iter().any(|a| contains_return(&a.expr)),
        K::Index(a, b) => contains_return(a) || contains_return(b),
        K::Match(scrutinee, arms) => {
            contains_return(scrutinee)
                || arms.iter().any(|arm| arm.guard.as_ref().is_some_and(contains_return) || contains_return(&arm.body))
        }
        K::StructLit(_, fields) => fields.iter().any(|f| f.value.as_ref().is_some_and(contains_return)),
        K::IfLet { scrutinee, then_block, else_expr, .. } => {
            contains_return(scrutinee)
                || block_has_return(then_block)
                || else_expr.as_deref().is_some_and(contains_return)
        }
    }
}

fn block_has_return(b: &ast::Block) -> bool {
    b.stmts.iter().any(|s| match s {
        ast::Stmt::Let { init, else_block, .. } => {
            contains_return(init) || else_block.as_ref().is_some_and(block_has_return)
        }
        ast::Stmt::Expr(e) => contains_return(e),
    })
}

/// The closure hint for an argument whose type parameter has these bounds: the first `Fn`-family bound, or `None`.
/// An owned closure type selects move captures; an `Fn`/`FnMut` bound leaves the concrete capture mode in the type
/// parameter (CL9).
pub(super) fn bound_hint(globals: &Globals, bounds: &[crate::generics::Bound]) -> Option<Ty> {
    bounds.iter().find_map(|b| match b {
        crate::generics::Bound::Fn(i, kind) => Some(Ty::Fn {
            sig: Rc::new(globals.bound_fn_sig(*i, &[])),
            kind: *kind,
            borrowed: *kind != FnKind::FnOnce,
            send: tir::FnSend::unknown(),
        }),
        _ => None,
    })
}
