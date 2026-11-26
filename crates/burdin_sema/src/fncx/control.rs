//! Control flow: `if`, `for`, `match`, and `if let`.

use super::{FnCx, LoopKind, LoopScope, error_expr, mk};
use crate::globals::Globals;
use crate::types::compatible;
use burdin_diag::{Code, Span};
use burdin_syntax::ast;
use burdin_tir::{self as tir, FnKind, FnSig, IntTy, LocalId, LocalKind, Pat, Ty};
use std::collections::HashMap;
use std::rc::Rc;

/// Which kind of `for` loop a non-literal-range iterable produces.
enum ForKind {
    Array,
    Slice,
    List,
    Map,
    Chars,
    /// A `Range<T>` or `RangeInclusive<T>` value, evaluated once.
    Range,
}

impl FnCx<'_> {
    pub(super) fn if_expr(
        &mut self,
        cond: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let cond = self.expr(cond, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        let cond = self.mark_const_condition(cond);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        let Some(else_expr) = else_expr else {
            return mk(tir::ExprKind::If(Box::new(cond), then, None), Ty::Unit, span);
        };
        let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
        let other = self.expr(else_expr, else_expect.as_ref());
        let ty = self.unify(then_ty, &other.ty, other.span, "`if` and `else` have different types");
        mk(tir::ExprKind::If(Box::new(cond), then, Some(Box::new(other))), ty, span)
    }

    /// A loop with a label (`'label: while ...`). The label and the loop are one construct, so the loop's own
    /// scope carries the label: a labeled `break`/`continue` names it by that name, and an unlabeled one still
    /// names the innermost loop.
    pub(super) fn labeled_loop(
        &mut self,
        label: &ast::Ident,
        body: &ast::Expr,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        if self.loops.iter().any(|scope| scope.label.as_deref() == Some(label.name.as_str())) {
            self.diags.error(
                Code::DUPLICATE_NAME,
                label.span,
                format!("the label `'{}` is already used by an enclosing loop", label.name),
            );
        }
        match &body.kind {
            ast::ExprKind::Loop(b) => self.loop_expr(b, Some(label), expect, span),
            ast::ExprKind::While { cond, body: b } => self.while_expr(cond, b, Some(label), span),
            ast::ExprKind::WhileLet { pattern, scrutinee, body: b } => {
                self.while_let(pattern, scrutinee, b, Some(label), span)
            }
            ast::ExprKind::For { pattern, iter, mutable, body: b } => {
                self.for_expr(pattern, iter, *mutable, b, Some(label), span)
            }
            _ => unreachable!("the parser admits only a loop after a label"),
        }
    }

    /// Wraps `inner` in the label's `Labeled` when there is one. A label goes on the loop node itself, never on
    /// the block a user `for` lowers to, so the checks and codegen find a loop (IT4). The loops that lower to
    /// several nodes wrap the node the jumps target.
    pub(super) fn with_label(label: Option<&ast::Ident>, inner: tir::Expr) -> tir::Expr {
        let Some(label) = label else { return inner };
        let ty = inner.ty.clone();
        let span = inner.span;
        mk(tir::ExprKind::Labeled(label.name.clone(), Box::new(inner)), ty, span)
    }

    /// `loop { ... }`: the only loop that gives back a value. Its type is the type every `break` value has, the
    /// expected type when one is known, and `!` when nothing breaks out of it.
    pub(super) fn loop_expr(
        &mut self,
        body: &ast::Block,
        label: Option<&ast::Ident>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        self.loops.push(LoopScope::loop_body(label, expect));
        let (body, _) = self.block(body, None);
        let scope = self.loops.pop().expect("pushed above");
        let ty = if scope.has_break { scope.break_ty.unwrap_or(Ty::Unit) } else { Ty::Never };
        Self::with_label(label, mk(tir::ExprKind::Loop(body), ty, span))
    }

    /// `while cond { ... }`: it can end by its condition, so it always gives back `()`.
    pub(super) fn while_expr(
        &mut self,
        cond: &ast::Expr,
        body: &ast::Block,
        label: Option<&ast::Ident>,
        span: Span,
    ) -> tir::Expr {
        let cond = self.expr(cond, Some(&Ty::Bool));
        let cond = self.expect_ty(cond, &Ty::Bool);
        self.loops.push(LoopScope::while_body(label));
        let (body, _) = self.block(body, None);
        self.loops.pop();
        Self::with_label(label, mk(tir::ExprKind::While(Box::new(cond), body), Ty::Unit, span))
    }

    /// `while let pattern = value { ... }`: a `while` that repeats while the value matches the pattern. It is lowered
    /// to a `loop` whose body matches the value once per pass, with a catch-all arm that breaks, so the pattern binds
    /// in the body only, as in `if let`, and the value is evaluated again on every pass. The loop's scope is a
    /// `while`'s, so a `break` with a value is E0003 (`spec/grammar.md`, rule 8.11).
    pub(super) fn while_let(
        &mut self,
        pattern: &ast::Pattern,
        scrutinee: &ast::Expr,
        body: &ast::Block,
        label: Option<&ast::Ident>,
        span: Span,
    ) -> tir::Expr {
        self.loops.push(LoopScope::while_body(label));
        let value = self.expr(scrutinee, None);
        let (scrutinee, owned_temp) =
            if pattern_has_binding(pattern) { self.own_scrutinee_temp(value) } else { (value, None) };
        let owned = self.owns_scrutinee(&scrutinee);
        self.scopes.push(HashMap::new());
        let pat = self.pattern(pattern, &scrutinee.ty, owned);
        // A pattern that always matches never ends the loop, so it is refused. `let ... else` refuses an `else` on
        // such a pattern the same way.
        if self.exhaustive(&scrutinee.ty, &[&pat]) {
            self.diags.error(
                Code::TYPE_MISMATCH,
                pattern.span,
                "this pattern always matches, so `while let` never ends; use `while` or `loop` instead",
            );
        }
        let (then, _) = self.block(body, None);
        self.scopes.pop();
        self.loops.pop();
        let then = mk(tir::ExprKind::Block(then), Ty::Unit, body.span);
        // The catch-all arm leaves the loop. It is unlabeled, so it names this loop, the innermost one.
        let leave = mk(tir::ExprKind::Break { label: None, value: None }, Ty::Never, span);
        let arms =
            vec![tir::Arm { pat, guard: None, body: then }, tir::Arm { pat: Pat::Wild, guard: None, body: leave }];
        let matched = mk(tir::ExprKind::Match(Box::new(scrutinee), arms), Ty::Unit, span);
        let matched = self.wrap_scrutinee(matched, owned_temp, span);
        let pass = tir::Block { stmts: Vec::new(), tail: Some(Box::new(matched)), close: span };
        Self::with_label(label, mk(tir::ExprKind::Loop(pass), Ty::Unit, span))
    }

    /// `break` or `continue`, with or without a label. An unlabeled one names the innermost loop; a labeled one
    /// names the enclosing loop with that label. A label is looked for only in this body: a closure starts with no
    /// loops, so it cannot name one outside (CL8).
    pub(super) fn jump(
        &mut self,
        label: Option<&ast::Ident>,
        is_break: bool,
        value: Option<&ast::Expr>,
        span: Span,
    ) -> tir::Expr {
        let word = if is_break { "break" } else { "continue" };
        let target = match label {
            None => self.loops.len().checked_sub(1),
            Some(label) => self.loops.iter().rposition(|scope| scope.label.as_deref() == Some(label.name.as_str())),
        };
        let Some(target) = target else {
            // The value is still checked for its own errors before the jump is dropped.
            if let Some(value) = value {
                self.expr(value, None);
            }
            match label {
                None => {
                    self.diags.error(Code::SYNTAX, span, format!("`{word}` outside of a loop"));
                }
                Some(label) => {
                    self.diags.error(
                        Code::UNKNOWN_NAME,
                        label.span,
                        format!("no enclosing loop is labeled `'{}`", label.name),
                    );
                }
            }
            return error_expr(span);
        };
        if self.leaves_reflect_loop(word, span, target) {
            if let Some(value) = value {
                self.expr(value, None);
            }
            return error_expr(span);
        }
        if !is_break {
            return mk(tir::ExprKind::Continue(label.map(|label| label.name.clone())), Ty::Never, span);
        }
        let value = self.break_value(target, value, span);
        mk(tir::ExprKind::Break { label: label.map(|label| label.name.clone()), value }, Ty::Never, span)
    }

    /// The value of a `break` in the loop at `target` (an index into `self.loops`), checked against the loop's
    /// break type and returning whether it has one. A `for` or `while` can end normally, so it takes no value:
    /// E0003, and the value is still checked for its own errors.
    fn break_value(&mut self, target: usize, value: Option<&ast::Expr>, span: Span) -> Option<Box<tir::Expr>> {
        let kind = self.loops[target].kind;
        if kind != LoopKind::Loop {
            let loop_word = if kind == LoopKind::For { "for" } else { "while" };
            if let Some(value) = value {
                self.expr(value, None);
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("a `{loop_word}` loop can end without a `break`, so it gives back `()`, not a value"),
                );
            }
            self.loops[target].has_break = true;
            self.loops[target].break_ty = Some(Ty::Unit);
            return None;
        }
        match (&self.loops[target].break_ty.clone(), value) {
            (Some(want), Some(value)) => {
                let checked = self.expr(value, Some(want));
                let checked = self.expect_ty(checked, want);
                self.loops[target].has_break = true;
                Some(Box::new(checked))
            }
            (Some(want), None) => {
                if !compatible(&Ty::Unit, want) {
                    self.diags.error(
                        Code::TYPE_MISMATCH,
                        span,
                        format!("this `break` gives no value, but the loop gives back `{want}`"),
                    );
                }
                self.loops[target].has_break = true;
                None
            }
            (None, Some(value)) => {
                let checked = self.expr(value, None);
                self.loops[target].break_ty = Some(checked.ty.clone());
                self.loops[target].has_break = true;
                Some(Box::new(checked))
            }
            (None, None) => {
                self.loops[target].break_ty = Some(Ty::Unit);
                self.loops[target].has_break = true;
                None
            }
        }
    }

    pub(super) fn unify(&mut self, first: Ty, second: &Ty, span: Span, what: &str) -> Ty {
        if first == Ty::Never || first == Ty::Error {
            return second.clone();
        }
        if !compatible(second, &first) {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("{what}: `{first}` and `{second}`"));
        }
        first
    }

    pub(super) fn for_expr(
        &mut self,
        pattern: &ast::Pattern,
        iter: &ast::Expr,
        mutable: bool,
        body: &ast::Block,
        label: Option<&ast::Ident>,
        span: Span,
    ) -> tir::Expr {
        if mutable {
            return self.for_mut(pattern, iter, body, label, span);
        }
        // A loop over `Type.fields` or `Type.variants` is unrolled while it is checked (`spec/reflection.md`, RFL4).
        if label.is_none()
            && let Some(unrolled) = self.for_reflect(pattern, iter, body, span)
        {
            return unrolled;
        }
        if let ast::ExprKind::Range { lo, hi, inclusive } = &iter.kind {
            let (lo, hi) = self.operands(lo, hi, None);
            let ty = if lo.ty == Ty::Never { hi.ty.clone() } else { lo.ty.clone() };
            let hi = self.expect_ty(hi, &ty);
            if !matches!(ty, Ty::Int(_) | Ty::Error) {
                self.diags.error(Code::TYPE_MISMATCH, iter.span, format!("cannot iterate over a range of `{ty}`"));
            }
            // A full pattern (a tuple or a literal) matches through the iteration protocol, like `.iter()`.
            if matches!(ty, Ty::Int(_))
                && !matches!(pattern.kind, ast::PatternKind::Binding { .. } | ast::PatternKind::Wild)
            {
                let range_ty = self.globals.range_ty(ty.clone(), *inclusive);
                let value = mk(
                    tir::ExprKind::Range { lo: Box::new(lo), hi: Box::new(hi), inclusive: *inclusive },
                    range_ty,
                    span,
                );
                let iter = ast::Ident { name: "iter".to_string(), span };
                let iterator = self.method_call_on(value, &iter, &[], span, None);
                return self.protocol_for(pattern, iterator, ty, body, label, span);
            }
            self.scopes.push(HashMap::new());
            let var = match &pattern.kind {
                ast::PatternKind::Binding { name, mutable } => {
                    Some(self.declare(&name.name, ty, *mutable, LocalKind::Owned, name.span))
                }
                ast::PatternKind::Wild => None,
                _ => {
                    self.scopes.pop();
                    // The endpoints did not produce an integer type, so no element exists to match; the body is
                    // still checked for its own errors.
                    self.loops.push(LoopScope::for_body(label));
                    let _ = self.block(body, None);
                    self.loops.pop();
                    return error_expr(span);
                }
            };
            self.loops.push(LoopScope::for_body(label));
            let (body, _) = self.block(body, None);
            self.loops.pop();
            self.scopes.pop();
            let kind = tir::ExprKind::ForRange { var, lo: Box::new(lo), hi: Box::new(hi), inclusive: *inclusive, body };
            return Self::with_label(label, mk(kind, Ty::Unit, span));
        }
        let iterable = self.expr(iter, None);
        // An `impl Interface` value iterates through its concrete value (`fncx/opaque.rs`).
        let iterable = self.open_opaque_for_loop(iterable);
        // A user type (or a type parameter) iterates through the protocol: an iterator value directly, an
        // `Iterable` through `iter()` (spec/iterators.md, IT4).
        let list_elem = self.globals.list_parts(&iterable.ty).map(|(elem, _)| elem);
        let range_elem = self.globals.range_parts(&iterable.ty);
        let builtin = list_elem.is_some()
            || range_elem.is_some()
            || matches!(iterable.ty, Ty::Array(..) | Ty::Slice(_) | Ty::Map(..) | Ty::Chars | Ty::Error | Ty::Never);
        if !builtin {
            // `for` borrows its source (IT1), so a type that is both an `Iterable` and an `Iterator` is iterated
            // through `iter()`, not consumed as an iterator value.
            if let Some(item) = self.globals.iterable_item(&iterable.ty, &self.bounds) {
                let iterator = self.for_iterator(iter, iterable.clone(), &item, span);
                if iterator.ty == Ty::Error {
                    return iterator;
                }
                match self.globals.iterator_item(&iterator.ty, &self.bounds) {
                    Some(got) if got == item => {}
                    _ => {
                        let got = iterator.ty.clone();
                        self.diags.error(
                            Code::BOUND_NOT_SATISFIED,
                            span,
                            format!(
                                "`iter()` of `{}` returns `{got}`, which is not an `Iterator<{item}>`",
                                iterable.ty
                            ),
                        );
                        return error_expr(span);
                    }
                }
                return self.protocol_for(pattern, iterator, item, body, label, span);
            }
            if let Some(item) = self.globals.iterator_item(&iterable.ty, &self.bounds) {
                return self.protocol_for(pattern, iterable, item, body, label, span);
            }
            self.diags.error(
                Code::TYPE_MISMATCH,
                iter.span,
                format!("cannot iterate over a value of type `{}`", iterable.ty),
            );
            return error_expr(span);
        }
        let (elem, kind) = match &iterable.ty {
            _ if list_elem.is_some() => (list_elem.clone().expect("checked above"), ForKind::List),
            _ if range_elem.is_some() => (range_elem.clone().expect("checked above").0, ForKind::Range),
            Ty::Array(t, _) => ((**t).clone(), ForKind::Array),
            Ty::Slice(t) => ((**t).clone(), ForKind::Slice),
            Ty::Map(k, v, _) => (Ty::Tuple(vec![(**k).clone(), (**v).clone()]), ForKind::Map),
            Ty::Error | Ty::Never => (Ty::Error, ForKind::Array),
            Ty::Chars => (Ty::Char, ForKind::Chars),
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    iter.span,
                    format!("cannot iterate over a value of type `{other}`"),
                );
                (Ty::Error, ForKind::Array)
            }
        };
        self.scopes.push(HashMap::new());
        if matches!(kind, ForKind::Range) {
            // A full pattern matches through the iteration protocol, like `.iter()` on the range value.
            if matches!(elem, Ty::Int(_))
                && !matches!(pattern.kind, ast::PatternKind::Binding { .. } | ast::PatternKind::Wild)
            {
                self.scopes.pop();
                let iter = ast::Ident { name: "iter".to_string(), span };
                let iterator = self.method_call_on(iterable, &iter, &[], span, None);
                return self.protocol_for(pattern, iterator, elem, body, label, span);
            }
            // A range value's endpoints are integers, so the loop binding is a plain owned integer like the
            // literal form.
            let var = match &pattern.kind {
                ast::PatternKind::Binding { name, mutable } => {
                    Some(self.declare(&name.name, elem, *mutable, LocalKind::Owned, name.span))
                }
                ast::PatternKind::Wild => None,
                _ => {
                    self.scopes.pop();
                    // An invalid range type was already reported; check the body for its own errors.
                    self.loops.push(LoopScope::for_body(label));
                    let _ = self.block(body, None);
                    self.loops.pop();
                    return error_expr(span);
                }
            };
            self.loops.push(LoopScope::for_body(label));
            let (body, _) = self.block(body, None);
            self.loops.pop();
            self.scopes.pop();
            let kind = tir::ExprKind::ForRangeValue { var, range: Box::new(iterable), body };
            return Self::with_label(label, mk(kind, Ty::Unit, span));
        }
        // `for` borrows the collection, so its elements are read-only views.
        let pat = self.pattern(pattern, &elem, false);
        if !self.exhaustive(&elem, &[&pat]) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                pattern.span,
                format!("this pattern does not match every value of `{elem}`"),
            );
        }
        self.loops.push(LoopScope::for_body(label));
        let (body, _) = self.block(body, None);
        self.loops.pop();
        self.scopes.pop();
        let array = Box::new(iterable);
        let kind = match kind {
            ForKind::Array => tir::ExprKind::ForArray { pat, array, body },
            ForKind::Slice => tir::ExprKind::ForSlice { pat, slice: array, body },
            ForKind::List => tir::ExprKind::ForList { pat, list: array, body },
            ForKind::Map => tir::ExprKind::ForMap { pat, map: array, body },
            ForKind::Chars => tir::ExprKind::ForChars { pat, string: array, body },
            ForKind::Range => unreachable!("handled above"),
        };
        Self::with_label(label, mk(kind, Ty::Unit, span))
    }

    /// `lo..hi` or `lo..=hi`: an ordinary value of `Range<T>` or `RangeInclusive<T>`. The endpoints are integers;
    /// the type comes from the left operand, from the right when the left is `!`, or from the expected type.
    pub(super) fn range_expr(
        &mut self,
        lo: &ast::Expr,
        hi: &ast::Expr,
        inclusive: bool,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let hint = match expect {
            Some(t) => self.globals.range_parts(t).map(|(elem, _)| elem),
            None => None,
        };
        let (lo, hi) = self.operands(lo, hi, hint.as_ref());
        let ty = if lo.ty == Ty::Never { hi.ty.clone() } else { lo.ty.clone() };
        let hi = self.expect_ty(hi, &ty);
        if !matches!(ty, Ty::Int(_) | Ty::Error) {
            self.diags.error(Code::TYPE_MISMATCH, span, format!("a range's endpoints must be integers, found `{ty}`"));
            return error_expr(span);
        }
        let ty = if ty == Ty::Error { Ty::Error } else { self.globals.range_ty(ty, inclusive) };
        mk(tir::ExprKind::Range { lo: Box::new(lo), hi: Box::new(hi), inclusive }, ty, span)
    }

    pub(super) fn match_expr(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        if arms.iter().any(|a| matches!(a.pattern.as_ref().map(|p| &p.kind), Some(ast::PatternKind::Typed { .. }))) {
            return self.any_match(scrutinee, arms, expect, span);
        }
        let value = self.expr(scrutinee, None);
        let moves = arms.iter().any(|a| a.pattern.as_ref().is_some_and(pattern_has_binding));
        let (scrutinee, owned_temp) = if moves { self.own_scrutinee_temp(value) } else { (value, None) };
        let owned = self.owns_scrutinee(&scrutinee);
        let mut ty = Ty::Never;
        let mut checked = Vec::new();
        for arm in arms {
            self.scopes.push(HashMap::new());
            let pat = match &arm.pattern {
                Some(p) => self.pattern(p, &scrutinee.ty, owned),
                None => Pat::Wild,
            };
            let guard = arm.guard.as_ref().map(|g| {
                let g = self.expr(g, Some(&Ty::Bool));
                self.expect_ty(g, &Ty::Bool)
            });
            let arm_expect = if ty == Ty::Never { expect.cloned() } else { Some(ty.clone()) };
            let body = self.expr(&arm.body, arm_expect.as_ref());
            ty = self.unify(ty, &body.ty, body.span, "match arms have different types");
            self.scopes.pop();
            checked.push(tir::Arm { pat, guard, body });
        }
        let unguarded: Vec<&Pat> = checked.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
        if !self.exhaustive(&scrutinee.ty, &unguarded) {
            self.diags.error(
                Code::NON_EXHAUSTIVE,
                span,
                format!("this `match` does not cover every value of `{}`", scrutinee.ty),
            );
        }
        let matched = mk(tir::ExprKind::Match(Box::new(scrutinee), checked), ty, span);
        self.wrap_scrutinee(matched, owned_temp, span)
    }

    /// `if let pattern = scrutinee { ... } else { ... }`, checked as a `match` with a catch-all arm.
    /// `expr?`: unwraps an `Option` or a `Result`, returning early through the enclosing function.
    pub(super) fn try_expr(&mut self, operand: &ast::Expr, span: Span) -> tir::Expr {
        let value = self.expr(operand, None);
        let (value, owned_temp) = self.own_scrutinee_temp(value);
        let value_ty = value.ty.clone();
        let ret = self.ret.clone();
        let ret_is_option = self.globals.option_payload(&ret).is_some();
        let ret_result_err = self.globals.result_parts(&ret).map(|(_, e)| e);
        match (&ret, &value_ty) {
            (Ty::Error, _) | (_, Ty::Error) => error_expr(span),
            _ if ret_is_option && self.globals.option_payload(&value_ty).is_some() => {
                let inner = self.globals.option_payload(&value_ty).unwrap();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", inner.clone(), owned);
                let some = Pat::Variant(1, vec![Pat::Binding(id, mode)]);
                let body = mk(tir::ExprKind::Local(id), inner.clone(), span);
                let none = Globals::mk_none(ret.clone(), span);
                let early = mk(tir::ExprKind::Return(Some(Box::new(none))), Ty::Never, span);
                let arms = vec![
                    tir::Arm { pat: some, guard: None, body },
                    tir::Arm { pat: Pat::Variant(0, Vec::new()), guard: None, body: early },
                ];
                let matched = mk(tir::ExprKind::Match(Box::new(value), arms), inner, span);
                self.wrap_scrutinee(matched, owned_temp, span)
            }
            _ if ret_result_err.is_some() && self.globals.result_parts(&value_ty).is_some() => {
                let (ok, err) = self.globals.result_parts(&value_ty).unwrap();
                let ret_err = ret_result_err.unwrap();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", ok.clone(), owned);
                let (eid, emode) = self.payload_local("err", err.clone(), owned);
                let body = mk(tir::ExprKind::Local(id), ok.clone(), span);
                let error = mk(tir::ExprKind::Local(eid), err.clone(), span);
                let converted = self.convert_error(error, &err, &ret_err, span, true);
                let err_value = Globals::mk_result(false, converted, ret.clone(), span);
                let early = mk(tir::ExprKind::Return(Some(Box::new(err_value))), Ty::Never, span);
                let arms = vec![
                    tir::Arm { pat: Pat::Variant(0, vec![Pat::Binding(id, mode)]), guard: None, body },
                    tir::Arm { pat: Pat::Variant(1, vec![Pat::Binding(eid, emode)]), guard: None, body: early },
                ];
                let matched = mk(tir::ExprKind::Match(Box::new(value), arms), ok, span);
                self.wrap_scrutinee(matched, owned_temp, span)
            }
            _ if ret_is_option || ret_result_err.is_some() => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`?` unwraps an `Option` or a `Result`, but this value is `{value_ty}`"),
                );
                error_expr(span)
            }
            _ => {
                self.diags.error(
                    Code::QUESTION_OUTSIDE,
                    span,
                    format!("`?` is only allowed in a function that returns `Option` or `Result`, but this one returns `{ret}`"),
                );
                error_expr(span)
            }
        }
    }

    /// The error conversion of `?` (`spec/errors.md`, ERR3-ERR5):
    ///
    /// - the same type passes through, and a `?` that propagates the dynamic `AnyError` records a trace frame when
    ///   tracing is on (ERR7);
    /// - a user `Type: From<Source>` block converts one error type into another (ERR3);
    /// - an error type becomes the dynamic `AnyError`, boxed at this creation site (ERR4);
    /// - anything else is E0212.
    fn convert_error(&mut self, value: tir::Expr, from: &Ty, to: &Ty, span: Span, propagate: bool) -> tir::Expr {
        if from == to {
            if !(propagate && self.traces && self.globals.is_error_ty(to)) {
                return value;
            }
            let Some(frame) = self.globals.error_frame else { return value };
            // A propagation frame points at the `?` itself, not at the start of the operand expression.
            let at = if span.hi > span.lo { burdin_diag::Span::new(span.file, span.hi - 1, span.hi) } else { span };
            let (path, line, col) = self.location(at);
            let args = vec![
                value,
                mk(tir::ExprKind::Str(self.fn_name.clone()), Ty::Str, span),
                mk(tir::ExprKind::Str(path), Ty::Str, span),
                mk(tir::ExprKind::Int(i128::from(line)), Ty::Int(IntTy::I32), span),
                mk(tir::ExprKind::Int(i128::from(col)), Ty::Int(IntTy::I32), span),
            ];
            return mk(tir::ExprKind::Call(frame, args), to.clone(), span);
        }
        // The enclosing function's return type did not resolve; its own diagnostic is enough.
        if matches!(from, Ty::Error) || matches!(to, Ty::Error) {
            return value;
        }
        if let Some(f) = self.globals.conversion(from, to) {
            return mk(tir::ExprKind::Call(f, vec![value]), to.clone(), span);
        }
        if self.globals.is_error_ty(to) && self.globals.implements_error(from, &self.bounds) {
            return self.coerce_error(value, to);
        }
        self.diags.error(
            Code::ERROR_CONVERSION,
            span,
            format!(
                "the error type `{from}` does not convert into `{to}`; add a `{to}: From<{from}>` implementation, \
                 or make `{from}` an error type"
            ),
        );
        error_expr(span)
    }

    /// `value orelse default`: the payload of an `Option` or a `Result`, or `default` when it is empty.
    pub(super) fn orelse(&mut self, value: &ast::Expr, default: &ast::Expr, span: Span) -> tir::Expr {
        let value = self.expr(value, None);
        let (value, owned_temp) = self.own_scrutinee_temp(value);
        let matched = self.orelse_match(value, default, span);
        self.wrap_scrutinee(matched, owned_temp, span)
    }

    /// The `match` an `orelse` lowers to.
    fn orelse_match(&mut self, value: tir::Expr, default: &ast::Expr, span: Span) -> tir::Expr {
        let value_ty = value.ty.clone();
        match value_ty {
            ref t if self.globals.option_payload(t).is_some() => {
                let inner = self.globals.option_payload(t).unwrap();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("some", inner.clone(), owned);
                // `value orelse || ...` is a handler: the closure replaces the empty case (`issue #488`).
                let other = if matches!(default.kind, ast::ExprKind::Closure { .. }) {
                    let expected = Ty::Fn {
                        sig: Rc::new(FnSig { params: Vec::new(), ret: Box::new(inner.clone()) }),
                        kind: FnKind::FnOnce,
                        borrowed: false,
                        send: tir::FnSend::unknown(),
                    };
                    let handler = self.expr(default, Some(&expected));
                    self.closure_call_typed(handler, Vec::new(), span)
                } else {
                    let other = self.expr(default, Some(&inner));
                    self.expect_ty(other, &inner)
                };
                let body = mk(tir::ExprKind::Local(id), inner.clone(), span);
                let arms = vec![
                    tir::Arm { pat: Pat::Variant(1, vec![Pat::Binding(id, mode)]), guard: None, body },
                    tir::Arm { pat: Pat::Variant(0, Vec::new()), guard: None, body: other },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), inner, span)
            }
            ref t if self.globals.result_parts(t).is_some() => {
                let (ok, err) = self.globals.result_parts(t).unwrap();
                let owned = self.owns_scrutinee(&value);
                let (id, mode) = self.payload_local("ok", ok.clone(), owned);
                // The error is bound so that it is dropped when the arm ends.
                let (eid, emode) = self.payload_local("err", err.clone(), owned);
                // `value orelse |e| ...` is a handler that receives the error (`issue #488`).
                let other = if matches!(default.kind, ast::ExprKind::Closure { .. }) {
                    let expected = Ty::Fn {
                        sig: Rc::new(FnSig { params: vec![err.clone()], ret: Box::new(ok.clone()) }),
                        kind: FnKind::FnOnce,
                        borrowed: false,
                        send: tir::FnSend::unknown(),
                    };
                    let handler = self.expr(default, Some(&expected));
                    let error = mk(tir::ExprKind::Local(eid), err, span);
                    self.closure_call_typed(handler, vec![error], span)
                } else {
                    let other = self.expr(default, Some(&ok));
                    self.expect_ty(other, &ok)
                };
                let body = mk(tir::ExprKind::Local(id), ok.clone(), span);
                let arms = vec![
                    tir::Arm { pat: Pat::Variant(0, vec![Pat::Binding(id, mode)]), guard: None, body },
                    tir::Arm { pat: Pat::Variant(1, vec![Pat::Binding(eid, emode)]), guard: None, body: other },
                ];
                mk(tir::ExprKind::Match(Box::new(value), arms), ok, span)
            }
            Ty::Error => {
                self.expr(default, None);
                error_expr(span)
            }
            other => {
                self.diags.error(
                    Code::TYPE_MISMATCH,
                    span,
                    format!("`orelse` needs an `Option` or a `Result`, but this value is `{other}`"),
                );
                self.expr(default, None);
                error_expr(span)
            }
        }
    }

    /// A local a synthesized `match` binds. It is not in any user scope, so the name cannot be observed.
    pub(super) fn payload_local(&mut self, name: &str, ty: Ty, owned: bool) -> (LocalId, tir::BindMode) {
        let mode = self.bind_mode(&ty, owned);
        let kind = if mode == tir::BindMode::View { LocalKind::View } else { LocalKind::Owned };
        let id = LocalId(self.locals.len());
        let span = Span::new(burdin_diag::FileId(0), 0, 0);
        self.locals.push(tir::Local { name: name.to_string(), ty, mutable: false, by_ref: false, kind, span });
        (id, mode)
    }

    /// `if let pattern = scrutinee { ... } else { ... }`, checked as a `match` with a catch-all arm.
    pub(super) fn if_let(
        &mut self,
        pattern: &ast::Pattern,
        scrutinee: &ast::Expr,
        then_block: &ast::Block,
        else_expr: Option<&ast::Expr>,
        expect: Option<&Ty>,
        span: Span,
    ) -> tir::Expr {
        let value = self.expr(scrutinee, None);
        let (scrutinee, owned_temp) =
            if pattern_has_binding(pattern) { self.own_scrutinee_temp(value) } else { (value, None) };
        let owned = self.owns_scrutinee(&scrutinee);
        self.scopes.push(HashMap::new());
        let pat = self.pattern(pattern, &scrutinee.ty, owned);
        let (then, then_ty) = self.block(then_block, if else_expr.is_some() { expect } else { None });
        self.scopes.pop();
        let then = mk(tir::ExprKind::Block(then), then_ty.clone(), then_block.span);
        let (other, ty) = match else_expr {
            Some(e) => {
                let else_expect = if then_ty == Ty::Never { expect.cloned() } else { Some(then_ty.clone()) };
                let other = self.expr(e, else_expect.as_ref());
                let ty = self.unify(then_ty, &other.ty, other.span, "`if let` and `else` have different types");
                (other, ty)
            }
            None => (mk(tir::ExprKind::Unit, Ty::Unit, span), Ty::Unit),
        };
        let arms =
            vec![tir::Arm { pat, guard: None, body: then }, tir::Arm { pat: Pat::Wild, guard: None, body: other }];
        let matched = mk(tir::ExprKind::Match(Box::new(scrutinee), arms), ty, span);
        self.wrap_scrutinee(matched, owned_temp, span)
    }

    /// Moves a temporary match scrutinee into a compiler-made local, so that moving bindings can track its parts
    /// and the rest of the value still drops once. Returns the local expression and the initializer to bind.
    pub(super) fn own_scrutinee_temp(&mut self, value: tir::Expr) -> (tir::Expr, Option<(LocalId, tir::Expr)>) {
        if burdin_tir::place::place_root(&value).is_some() {
            return (value, None);
        }
        let ty = value.ty.clone();
        let span = value.span;
        let id = LocalId(self.locals.len());
        self.locals.push(tir::Local {
            name: "__match".to_string(),
            ty: ty.clone(),
            mutable: false,
            by_ref: false,
            kind: LocalKind::Scrutinee,
            span: Span::new(span.file, span.lo, span.lo),
        });
        (mk(tir::ExprKind::Local(id), ty, span), Some((id, value)))
    }

    /// Wraps a lowered `match` in the block that declares its scrutinee local, when there is one.
    pub(super) fn wrap_scrutinee(
        &self,
        matched: tir::Expr,
        owned_temp: Option<(LocalId, tir::Expr)>,
        span: Span,
    ) -> tir::Expr {
        let Some((id, value)) = owned_temp else { return matched };
        let ty = matched.ty.clone();
        let stmt = tir::Stmt::Let(Pat::Binding(id, tir::BindMode::Move), value, None);
        let block = tir::Block { stmts: vec![stmt], tail: Some(Box::new(matched)), close: span };
        mk(tir::ExprKind::Block(block), ty, span)
    }
}

/// Whether an AST pattern binds any name, so it can move parts of a temporary scrutinee.
fn pattern_has_binding(pattern: &ast::Pattern) -> bool {
    match &pattern.kind {
        ast::PatternKind::Wild | ast::PatternKind::Lit(_) | ast::PatternKind::Range(..) | ast::PatternKind::Path(_) => {
            false
        }
        ast::PatternKind::Rest(name) => name.is_some(),
        ast::PatternKind::Binding { .. } | ast::PatternKind::At(..) | ast::PatternKind::Typed { .. } => true,
        ast::PatternKind::Tuple(subs) | ast::PatternKind::Slice(subs) => subs.iter().any(pattern_has_binding),
        ast::PatternKind::TupleStruct(_, subs) => subs.iter().any(pattern_has_binding),
        ast::PatternKind::Struct(_, fields, _) => {
            fields.iter().any(|f| f.pattern.as_ref().is_none_or(pattern_has_binding))
        }
        ast::PatternKind::Or(alts) => alts.iter().any(pattern_has_binding),
    }
}
