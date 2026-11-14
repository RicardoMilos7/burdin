//! Origin inference for returned views (memory-rules.md, RT).
//!
//! A function whose return type is second-class (`&T`, `&mut T`, `str`, `[T]`, or an aggregate containing one)
//! must return a view of exactly one of its parameters, or a static view. The origin set of an expression is
//! inferred by data flow through the body; a call with a second-class result maps the callee's signature origin
//! back to the argument. The check stays local: a body is analyzed against signatures only.
//!
//! The signature summaries are computed first, to a fixpoint, because a call needs its callee's origin. The
//! errors are reported on a final pass over the stable summaries.

use burdin_diag::{Code, Diagnostics, Span};
use burdin_tir::{self as tir, BindMode, LocalId, LocalKind, Pat, Signature};
use std::collections::BTreeSet;

/// The parameters whose storage a value may borrow from, and whether it borrows from something that does not
/// live past the return (a local, an owned parameter, or a temporary).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Origin {
    params: BTreeSet<usize>,
    local: bool,
}

impl Origin {
    fn param(index: usize) -> Origin {
        Origin { params: BTreeSet::from([index]), local: false }
    }

    /// A value that lives only in this function: a local, an owned parameter, or a temporary.
    fn local() -> Origin {
        Origin { params: BTreeSet::new(), local: true }
    }

    fn join(&mut self, other: &Origin) {
        self.params.extend(other.params.iter().copied());
        self.local |= other.local;
    }

    /// The parameter a second-class result derives from, if the signature can say it.
    fn summary(&self) -> Option<usize> {
        if self.local || self.params.len() != 1 {
            return None;
        }
        self.params.iter().next().copied()
    }
}

/// Computes every signature's return origin and reports E0121 and E0122.
pub(crate) fn check(sigs: &mut [Signature], functions: &mut [tir::Function], diags: &mut Diagnostics) {
    // The summary of one function can feed the next, so iterate until nothing changes.
    for _ in 0..=functions.len() {
        let mut changed = false;
        for i in 0..functions.len() {
            if functions[i].symbolic {
                continue;
            }
            let origin = {
                let mut analyzer = Analyzer::new(sigs, &functions[i], None);
                analyzer.body()
            };
            let summary = origin.summary();
            if sigs[i].ret_origin != summary {
                sigs[i].ret_origin = summary;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for i in 0..functions.len() {
        if functions[i].symbolic {
            continue;
        }
        let origin = {
            let mut analyzer = Analyzer::new(sigs, &functions[i], Some(diags));
            analyzer.body()
        };
        functions[i].ret_origin = sigs[i].ret_origin;
        if functions[i].ret.is_second_class() && origin.params.len() > 1 {
            let f = &functions[i];
            let names: Vec<String> = origin.params.iter().map(|p| f.locals[f.params[*p].0].name.clone()).collect();
            diags.error(
                Code::RETURN_MULTIPLE_ORIGINS,
                functions[i].span,
                format!(
                    "this returns views of more than one parameter (`{}`); a second-class result must derive from \
                     exactly one parameter",
                    names.join("`, `")
                ),
            );
        }
    }
}

struct Analyzer<'a> {
    sigs: &'a [Signature],
    f: &'a tir::Function,
    /// The origin of every local that is not a borrowed parameter; borrowed parameters are computed from the
    /// signature, and view bindings inherit the origin of their scrutinee.
    bindings: Vec<Option<Origin>>,
    /// The union of the origins of every returned expression.
    returns: Origin,
    /// `Some` on the reporting pass.
    report: Option<&'a mut Diagnostics>,
}

impl<'a> Analyzer<'a> {
    fn new(sigs: &'a [Signature], f: &'a tir::Function, report: Option<&'a mut Diagnostics>) -> Self {
        Analyzer { sigs, f, bindings: vec![None; f.locals.len()], returns: Origin::default(), report }
    }

    /// Analyzes the body and returns the union of the origins of all returned expressions.
    fn body(&mut self) -> Origin {
        let tail = self.block(&self.f.body);
        let span = self.f.body.tail.as_ref().map(|e| e.span);
        if let Some(span) = span {
            self.record_return(&tail, span);
        } else if self.f.ret.is_second_class() {
            self.record_return(&tail, self.f.span);
        }
        self.returns.clone()
    }

    /// The origin of a local's value: a borrowed parameter stands for its position in the signature.
    fn local_origin(&self, id: LocalId) -> Origin {
        match self.f.locals[id.0].kind {
            LocalKind::BorrowedParam => {
                let index = self.f.params.iter().position(|p| *p == id).unwrap_or(0);
                Origin::param(index)
            }
            _ => Origin::local(),
        }
    }

    fn local_value(&self, id: LocalId) -> Origin {
        match &self.bindings[id.0] {
            Some(origin) => origin.clone(),
            None => self.local_origin(id),
        }
    }

    fn record_return(&mut self, origin: &Origin, span: Span) {
        self.returns.join(origin);
        if !self.f.ret.is_second_class() || !origin.local {
            return;
        }
        if let Some(diags) = self.report.as_deref_mut() {
            diags.error(
                Code::RETURN_LOCAL_ORIGIN,
                span,
                "this view borrows from a local or an owned parameter, which does not live past the return",
            );
        }
    }

    fn set_binding(&mut self, id: LocalId, mode: BindMode, origin: &Origin) {
        let ty = &self.f.locals[id.0].ty;
        // A view binding, or any value of a view type, borrows from the same storage as the scrutinee.
        let origin = if mode == BindMode::View || ty.is_second_class() { origin.clone() } else { Origin::local() };
        self.bindings[id.0] = Some(origin);
    }

    fn bind_pattern(&mut self, pat: &Pat, origin: &Origin) {
        match pat {
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Range(..) | Pat::None => {}
            Pat::Binding(id, mode) => self.set_binding(*id, *mode, origin),
            Pat::At(id, mode, inner) => {
                self.set_binding(*id, *mode, origin);
                self.bind_pattern(inner, origin);
            }
            Pat::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.bind_pattern(first, origin);
                }
            }
            Pat::Some(inner) => self.bind_pattern(inner, origin),
            Pat::Product(subs) | Pat::Variant(_, subs) => {
                for sub in subs {
                    self.bind_pattern(sub, origin);
                }
            }
        }
    }

    fn block(&mut self, b: &tir::Block) -> Origin {
        for stmt in &b.stmts {
            match stmt {
                tir::Stmt::Let(pat, init, else_block) => {
                    let origin = self.expr(init);
                    if let Some(else_block) = else_block {
                        self.block(else_block);
                    }
                    self.bind_pattern(pat, &origin);
                }
                tir::Stmt::Expr(e) => {
                    self.expr(e);
                }
            }
        }
        match &b.tail {
            Some(tail) => self.expr(tail),
            None => Origin::default(),
        }
    }

    fn expr(&mut self, e: &tir::Expr) -> Origin {
        use tir::ExprKind as K;
        match &e.kind {
            K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Const(_) | K::None => {
                Origin::default()
            }
            K::Local(id) => self.local_value(*id),
            // A field, an element, a pointee, or a referent derives from the storage of its base.
            K::Field(base, _) | K::Index(base, _) | K::Deref(base) | K::DerefRef(base) => self.expr(base),
            K::Ref { place, .. } => self.expr(place),
            K::Slice { base, lo, hi } => {
                self.expr(lo);
                self.expr(hi);
                self.expr(base)
            }
            K::StringAsStr(inner) | K::Some(inner) | K::ArrayRepeat(inner, _) | K::Unary(_, inner) | K::Cast(inner) => {
                self.expr(inner)
            }
            K::Array(elems) | K::Tuple(elems) => {
                let mut origin = Origin::default();
                for x in elems {
                    origin.join(&self.expr(x));
                }
                origin
            }
            K::Struct(fields) | K::Variant(_, fields) => {
                let mut origin = Origin::default();
                for (_, x) in fields {
                    origin.join(&self.expr(x));
                }
                origin
            }
            // A C function returns a scalar or a raw pointer, never a view of Burdin storage.
            K::ExternCall(_, args) => {
                for arg in args {
                    self.expr(arg);
                }
                Origin::default()
            }
            K::Call(f, args) => {
                for arg in args {
                    self.expr(arg);
                }
                let sig = &self.sigs[f.0];
                if !sig.ret.is_second_class() {
                    return Origin::default();
                }
                match sig.ret_origin {
                    Some(index) if index < args.len() => self.expr(&args[index]),
                    // Static, or a signature that is already known to be invalid.
                    _ => Origin::default(),
                }
            }
            K::Builtin(_, args) => {
                for arg in args {
                    self.expr(arg);
                }
                Origin::local()
            }
            K::Method(m, args) => {
                for arg in args {
                    self.expr(arg);
                }
                match m {
                    // A view of the receiver, or a copy of it.
                    tir::Method::AsStr | tir::Method::Clone => self.expr(&args[0]),
                    _ => Origin::default(),
                }
            }
            K::Block(b) => self.block(b),
            K::If(cond, then, other) => {
                self.expr(cond);
                let mut origin = self.block(then);
                if let Some(other) = other {
                    origin.join(&self.expr(other));
                }
                origin
            }
            K::While(cond, body) => {
                self.expr(cond);
                self.block(body);
                Origin::default()
            }
            K::Loop(body) => {
                self.block(body);
                Origin::default()
            }
            K::ForRange { lo, hi, body, .. } => {
                self.expr(lo);
                self.expr(hi);
                self.block(body);
                Origin::default()
            }
            K::ForArray { pat, array, body } => {
                let origin = self.expr(array);
                self.bind_pattern(pat, &origin);
                self.block(body);
                Origin::default()
            }
            K::ForSlice { pat, slice, body } => {
                let origin = self.expr(slice);
                self.bind_pattern(pat, &origin);
                self.block(body);
                Origin::default()
            }
            K::Match(scrutinee, arms) => {
                let origin = self.expr(scrutinee);
                let mut result = Origin::default();
                for arm in arms {
                    self.bind_pattern(&arm.pat, &origin);
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    result.join(&self.expr(&arm.body));
                }
                result
            }
            K::Return(value) => {
                if let Some(value) = value {
                    let origin = self.expr(value);
                    self.record_return(&origin, value.span);
                }
                Origin::default()
            }
            K::Assign(place, value) => {
                self.expr(value);
                self.expr(place);
                Origin::default()
            }
            K::Binary(_, l, r) => {
                self.expr(l);
                self.expr(r);
                Origin::default()
            }
            K::Break | K::Continue => Origin::default(),
        }
    }
}
