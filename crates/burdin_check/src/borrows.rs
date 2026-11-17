//! The borrow checker (memory-rules.md, RF).
//!
//! Loans are created by `&`/`&mut`, by slicing and `as_str`, and by calling a function whose second-class result
//! is derived from an argument (RF5). A loan lives from its creation to the last use of the locals that hold it
//! (RF2); reusing the storage while the loan is live is E0111.
//!
//! Liveness is computed per statement: for every local, the highest top-level statement index that uses it. A
//! statement nested in a loop is part of the enclosing top-level statement, so a loan used inside a loop is live
//! for the whole loop. This is conservative for liveness and simple to keep sound, which is the right trade for
//! the stage 0 analysis.

use crate::Env;
use burdin_diag::{Code, Diagnostics, Span};
use burdin_tir::{self as tir, BindMode, LocalId, Pat, Ty};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    /// A read, and a shared borrow: only a live mutable loan conflicts.
    Read,
    /// A write: any live loan conflicts.
    Write,
    /// `&mut place`: any live loan conflicts.
    BorrowMut,
    /// A move: any live loan conflicts.
    Move,
}

struct Loan {
    root: LocalId,
    path: Vec<usize>,
    mutable: bool,
    /// The locals that hold the reference, a copy of it, or a view derived from it.
    holders: Vec<LocalId>,
    /// The top-level statement the loan was created in; with no holders it lives only there.
    start: usize,
}

struct Checker<'a> {
    env: Env<'a>,
    locals: &'a [tir::Local],
    last_use: Vec<Option<usize>>,
    loans: Vec<Loan>,
    /// The top-level statement being checked.
    stmt: usize,
    /// Set while a call under `*` is checked: its reference result is consumed by the dereference, so no result
    /// loan is recorded for it (`spec/operators.md`, OP10).
    skip_result_loan: bool,
    reported: Vec<Span>,
    diags: &'a mut Diagnostics,
}

/// Checks the borrows of one function body.
pub(crate) fn check(f: &tir::Function, env: Env, diags: &mut Diagnostics) {
    let mut last_use = vec![None; f.locals.len()];
    for (i, stmt) in f.body.stmts.iter().enumerate() {
        uses_stmt(stmt, i, &mut last_use);
    }
    if let Some(tail) = &f.body.tail {
        uses_expr(tail, f.body.stmts.len(), &mut last_use);
    }
    let mut cx = Checker {
        env,
        locals: &f.locals,
        last_use,
        loans: Vec::new(),
        stmt: 0,
        skip_result_loan: false,
        reported: Vec::new(),
        diags,
    };
    cx.body(&f.body);
}

impl<'a> Checker<'a> {
    fn error(&mut self, span: Span) {
        if self.reported.contains(&span) {
            return;
        }
        self.reported.push(span);
        self.diags.error(Code::BORROW_CONFLICT, span, "this access conflicts with a borrow that is still live");
    }

    fn is_copy(&self, ty: &Ty) -> bool {
        self.env.defs.is_copy(ty)
    }

    fn loan_live(&self, loan: &Loan, stmt: usize) -> bool {
        if loan.start > stmt {
            return false;
        }
        let mut end: Option<usize> = None;
        for holder in &loan.holders {
            if let Some(last) = self.last_use[holder.0] {
                end = Some(end.map_or(last, |e: usize| e.max(last)));
            }
        }
        match end {
            Some(end) => end >= stmt,
            None => loan.start == stmt,
        }
    }

    fn conflicts(&self, e: &tir::Expr, access: Access) -> bool {
        let Some((root, path, _)) = burdin_tir::place::tir_place(e) else { return false };
        self.loans.iter().any(|loan| {
            loan.root == root
                && (path.starts_with(&loan.path) || loan.path.starts_with(&path))
                && self.loan_live(loan, self.stmt)
                && match access {
                    Access::Read => loan.mutable,
                    Access::Write | Access::BorrowMut | Access::Move => true,
                }
        })
    }

    /// Checks an access to the place `e`, then evaluates the parts of it that are not the place itself (indices,
    /// calls under a field).
    fn access_place(&mut self, e: &tir::Expr, access: Access) {
        if self.conflicts(e, access) {
            self.error(e.span);
        }
        self.place_subexpr(e);
    }

    fn place_subexpr(&mut self, e: &tir::Expr) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Local(_) => {}
            // A reference returned by a call and consumed by `*` lasts only for this access: its result loan is
            // skipped, and a re-borrow of the referent is recorded through the call's origin by `add_loan`.
            K::DerefRef(base) => {
                let saved = self.skip_result_loan;
                self.skip_result_loan = true;
                self.place_subexpr(base);
                self.skip_result_loan = saved;
            }
            K::Field(base, _) | K::Deref(base) | K::PtrDeref(base) => self.place_subexpr(base),
            K::Index(base, index) => {
                self.place_subexpr(base);
                self.expr(index, Mode::Use);
            }
            _ => self.expr(e, Mode::Use),
        }
    }

    /// Records a loan on the place `e`, if it is one. Its holders are attached by the enclosing `let`.
    fn add_loan(&mut self, e: &tir::Expr, mutable: bool) {
        let place =
            burdin_tir::place::tir_place(e).map(|(root, path, _)| (root, path)).or_else(|| self.reference_origin(e));
        if let Some((root, path)) = place {
            self.loans.push(Loan { root, path, mutable, holders: Vec::new(), start: self.stmt });
        }
    }

    /// The storage a borrowed expression views when it is not itself a plain place: a call's reference result
    /// derives from its origin argument (RF5). A user `Index` implementation lowers to exactly such a call, so a
    /// re-borrow of an element (`&mut v[i]`) is tracked against the container (`spec/operators.md`, OP10).
    fn reference_origin(&self, e: &tir::Expr) -> Option<(LocalId, Vec<usize>)> {
        use tir::ExprKind as K;
        match &e.kind {
            K::DerefRef(base) | K::Field(base, _) => self.reference_origin(base),
            K::Call(f, args) => {
                let sig = self.env.sigs.get(f.0)?;
                if !sig.ret.is_second_class() {
                    return None;
                }
                let arg = args.get(sig.ret_origin?)?;
                burdin_tir::place::tir_place(arg)
                    .map(|(root, path, _)| (root, path))
                    .or_else(|| self.reference_origin(arg))
            }
            // The entry operations hand out the map's storage from a temporary entry value, so a loan taken on
            // the entry belongs to the map the entry borrows. Other chainable views take their loan where their
            // receiver is a place, so they need no origin here (and `mut self` on a view must not borrow the
            // view's source exclusively).
            K::Method(
                tir::Method::MapEntry | tir::Method::MapEntryOrInsert | tir::Method::MapEntryOrInsertWith,
                args,
            ) => {
                let arg = args.first()?;
                burdin_tir::place::tir_place(arg)
                    .map(|(root, path, _)| (root, path))
                    .or_else(|| self.reference_origin(arg))
            }
            _ => None,
        }
    }

    // ----- statements ---------------------------------------------------------

    fn body(&mut self, b: &tir::Block) {
        for (i, stmt) in b.stmts.iter().enumerate() {
            self.stmt = i;
            self.statement(stmt);
        }
        self.stmt = b.stmts.len();
        if let Some(tail) = &b.tail {
            self.expr(tail, Mode::Use);
        }
    }

    fn block(&mut self, b: &tir::Block) {
        for stmt in &b.stmts {
            self.statement(stmt);
        }
        if let Some(tail) = &b.tail {
            self.expr(tail, Mode::Use);
        }
    }

    fn statement(&mut self, stmt: &tir::Stmt) {
        let start = self.loans.len();
        match stmt {
            tir::Stmt::Let(pat, init, else_block) => {
                // `let _ = e` only reads a place; any other pattern takes the value (moves are checked elsewhere).
                let mode = if matches!(pat, Pat::Wild) { Mode::Use } else { Mode::Move };
                self.expr(init, mode);
                if let Some(else_block) = else_block {
                    self.block(else_block);
                }
                self.attach_holders(pat, init, start);
            }
            tir::Stmt::Expr(e) => self.expr(e, Mode::Move),
        }
    }

    /// Gives the loans created by a `let` initializer to the bindings that hold them, and extends loans held by
    /// locals the initializer copies from.
    fn attach_holders(&mut self, pat: &Pat, init: &tir::Expr, start: usize) {
        let mut holders = Vec::new();
        view_bindings(pat, self.locals, &mut holders);
        let mut used = Vec::new();
        locals_used(init, &mut used);
        self.hold(&holders, &used, start);
    }

    /// Makes `holders` hold the loans created since `start` and the loans held by any of the locals in `used`.
    fn hold(&mut self, holders: &[LocalId], used: &[LocalId], start: usize) {
        if holders.is_empty() {
            return;
        }
        for (i, loan) in self.loans.iter_mut().enumerate() {
            if i >= start {
                // A loan created by this initializer belongs to the binding.
                for id in holders {
                    if !loan.holders.contains(id) {
                        loan.holders.push(*id);
                    }
                }
            } else if loan.holders.iter().any(|h| used.contains(h)) {
                // `let q = r`: q carries a copy of the reference or a view derived from it.
                for id in holders {
                    if !loan.holders.contains(id) {
                        loan.holders.push(*id);
                    }
                }
            }
        }
    }

    // ----- expressions --------------------------------------------------------

    fn expr(&mut self, e: &tir::Expr, mode: Mode) {
        use tir::ExprKind as K;
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
            K::Local(_) | K::Field(..) | K::Deref(..) | K::PtrDeref(..) | K::DerefRef(..) | K::Index(..) => {
                let access = if mode == Mode::Move && !self.is_copy(&e.ty) { Access::Move } else { Access::Read };
                self.access_place(e, access);
            }
            K::Ref { place, mutable } => {
                self.access_place(place, if *mutable { Access::BorrowMut } else { Access::Read });
                self.add_loan(place, *mutable);
            }
            K::Slice { base, lo, hi, .. } => {
                self.access_place(base, Access::Read);
                self.add_loan(base, false);
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
            }
            K::StringAsStr(inner) => {
                self.access_place(inner, Access::Read);
                self.add_loan(inner, false);
            }
            // Building a borrowing closure takes the same loan as `&`/`&mut` on each captured place (CL7); the
            // loan is held by the closure value (attached by the enclosing `let`) until its last use.
            K::Closure { captures, .. } => {
                for c in captures {
                    let place = tir::Expr { kind: tir::ExprKind::Local(c.local), ty: c.ty.clone(), span: e.span };
                    match c.mode {
                        tir::CaptureMode::Borrow => {
                            self.access_place(&place, Access::Read);
                            self.add_loan(&place, false);
                        }
                        tir::CaptureMode::BorrowMut => {
                            self.access_place(&place, Access::BorrowMut);
                            self.add_loan(&place, true);
                        }
                        tir::CaptureMode::Move => self.access_place(&place, Access::Move),
                    }
                }
            }
            K::ClosureCall { callee, args, kind } => {
                match kind {
                    tir::FnKind::Fn => self.access_place(callee, Access::Read),
                    tir::FnKind::FnMut => self.access_place(callee, Access::BorrowMut),
                    tir::FnKind::FnOnce => self.expr(callee, Mode::Move),
                }
                for arg in args {
                    self.expr(arg, Mode::Move);
                }
                // A view returned by the closure derives from the closure value it was called through.
                if e.ty.is_second_class() {
                    let mutable = matches!(e.ty, Ty::Ref { mutable: true, .. });
                    self.add_loan(callee, mutable);
                }
            }
            K::Call(f, args) => self.call(f.0, args),
            // C functions borrow nothing from Burdin values: their arguments are scalars and raw pointers.
            K::ExternCall(_, args) => {
                for arg in args {
                    self.expr(arg, Mode::Use);
                }
            }
            K::Builtin(b, args) => match b {
                tir::Builtin::HeapNew | tir::Builtin::SharedNew => self.expr(&args[0], Mode::Move),
                tir::Builtin::ListNewIn | tir::Builtin::MapNewIn => self.expr(&args[0], Mode::Move),
                tir::Builtin::HeapNewIn => {
                    self.expr(&args[0], Mode::Move);
                    self.expr(&args[1], Mode::Move);
                }
                tir::Builtin::Swap => {
                    self.access_place(&args[0], Access::BorrowMut);
                    self.access_place(&args[1], Access::BorrowMut);
                }
                tir::Builtin::Replace => {
                    self.access_place(&args[0], Access::BorrowMut);
                    self.expr(&args[1], Mode::Move);
                }
                _ => {
                    for arg in args {
                        self.expr(arg, Mode::Use);
                    }
                }
            },
            // Boxing moves (or copies) the value into the box; `downcast` consumes the box; `is` only reads it.
            K::BoxAny { value, .. } => self.expr(value, Mode::Move),
            K::AnyIs { value, .. } => self.expr(value, Mode::Use),
            K::AnyDowncast { value, .. } => self.expr(value, Mode::Move),
            // A dynamic call borrows the box: shared for `self`, exclusive for `mut self`. Its result is always
            // an owned type (object safety), so it holds no loan after the call.
            K::AnyCall { recv, iface, slot, params, args } => {
                let exclusive = self
                    .env
                    .any_ifaces
                    .get(*iface)
                    .and_then(|i| i.methods.get(*slot))
                    .and_then(|m| m.sig.params.first())
                    .is_some_and(|p| p.exclusive);
                if exclusive {
                    self.access_place(recv, Access::BorrowMut);
                } else {
                    self.access_place(recv, Access::Read);
                }
                for (a, p) in args.iter().zip(params) {
                    if p.own {
                        self.expr(a, Mode::Move);
                    } else if p.exclusive {
                        self.access_place(a, Access::BorrowMut);
                    } else {
                        self.expr(a, Mode::Use);
                    }
                }
            }
            K::Method(m, args) => {
                if m.mutates_receiver() {
                    self.access_place(&args[0], Access::BorrowMut);
                    // A mutable view of a map value, or a mutable iterator, keeps its collection exclusively borrowed (`Iterator.next` also
                    // mutates its receiver and returns a view, but the loan of its source is already held).
                    if matches!(
                        m,
                        tir::Method::MapGetMut
                            | tir::Method::MapGetOrInsert
                            | tir::Method::MapEntry
                            | tir::Method::IterMut
                    ) {
                        self.add_loan(&args[0], true);
                    }
                    for (i, arg) in args.iter().enumerate().skip(1) {
                        self.expr(arg, if m.owns_argument(i) { Mode::Move } else { Mode::Use });
                    }
                } else if m.result_borrows_receiver() || *m == tir::Method::AsStr {
                    // The entry operations consume the entry, value, or closure they are given; their result is a
                    // view of the storage the entry borrows, so that loan stays live for the result's holders.
                    if m.owns_argument(0) {
                        self.expr(&args[0], Mode::Move);
                    } else {
                        self.access_place(&args[0], Access::Read);
                    }
                    let mutable = matches!(e.ty, Ty::Ref { mutable: true, .. });
                    self.add_loan(&args[0], mutable);
                    for (i, arg) in args.iter().enumerate().skip(1) {
                        self.expr(arg, if m.owns_argument(i) { Mode::Move } else { Mode::Use });
                    }
                } else if *m == tir::Method::Clone {
                    self.expr(&args[0], Mode::Use);
                } else {
                    for (i, arg) in args.iter().enumerate() {
                        self.expr(arg, if m.owns_argument(i) { Mode::Move } else { Mode::Use });
                    }
                }
            }
            K::Block(b) => self.block(b),
            // A comptime expression only touches its own locals (spec/comptime.md, CT2); its value is a new one.
            K::Comptime(inner) => self.expr(inner, mode),
            // The items are owned values made at compile time, so the loop borrows nothing (CT6).
            K::ComptimeFor { iter, body, .. } => {
                self.expr(iter, Mode::Use);
                self.block(body);
            }
            K::If(cond, then, other) => {
                self.expr(cond, Mode::Use);
                self.block(then);
                if let Some(other) = other {
                    self.expr(other, Mode::Use);
                }
            }
            K::While(cond, body) => {
                self.expr(cond, Mode::Use);
                self.block(body);
            }
            K::Loop(body) => self.block(body),
            K::ForRange { lo, hi, body, .. } => {
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
                self.block(body);
            }
            K::ForRangeValue { range, body, .. } => {
                self.expr(range, Mode::Use);
                self.block(body);
            }
            K::ForArray { array, body, .. } => {
                // The loop borrows the collection for as long as it runs, so the body cannot change it.
                self.access_place(array, Access::Read);
                self.add_loan(array, false);
                self.block(body);
            }
            K::ForSlice { slice, body, .. } => {
                // The loop borrows the collection for as long as it runs, so the body cannot change it.
                self.access_place(slice, Access::Read);
                self.add_loan(slice, false);
                self.block(body);
            }
            K::ForList { list, body, .. } => {
                // The loop borrows the collection for as long as it runs, so the body cannot change it.
                self.access_place(list, Access::Read);
                self.add_loan(list, false);
                self.block(body);
            }
            K::ForMap { map, body, .. } => {
                // The loop borrows the collection for as long as it runs, so the body cannot change it.
                self.access_place(map, Access::Read);
                self.add_loan(map, false);
                self.block(body);
            }
            K::ForChars { string, body, .. } => {
                // The loop borrows the collection for as long as it runs, so the body cannot change it.
                self.access_place(string, Access::Read);
                self.add_loan(string, false);
                self.block(body);
            }
            K::Match(scrutinee, arms) => {
                let start = self.loans.len();
                self.expr(scrutinee, Mode::Use);
                let mut used = Vec::new();
                locals_used(scrutinee, &mut used);
                for arm in arms {
                    // A view binding holds the loans of the scrutinee, so a view copied out of the arm keeps the
                    // collection borrowed (`held = x`).
                    let mut holders = Vec::new();
                    view_bindings(&arm.pat, self.locals, &mut holders);
                    self.hold(&holders, &used, start);
                    if let Some(guard) = &arm.guard {
                        self.expr(guard, Mode::Use);
                    }
                    self.expr(&arm.body, Mode::Use);
                }
            }
            K::Return(value) => {
                if let Some(value) = value {
                    self.expr(value, Mode::Move);
                }
            }
            K::Assign(place, value) => {
                let start = self.loans.len();
                self.expr(value, Mode::Move);
                if let Some((root, path, _)) = burdin_tir::place::tir_place(place)
                    && path.is_empty()
                {
                    // `held = x` copies a view: the loans the value's locals hold are held by `held` too.
                    let mut used = Vec::new();
                    locals_used(value, &mut used);
                    let copied: Vec<usize> =
                        (0..start).filter(|&i| self.loans[i].holders.iter().any(|h| used.contains(h))).collect();
                    // The local no longer holds the old value: the loans it held may end here. A new
                    // reference or view stored in it is held by it (as in a `let`).
                    for loan in &mut self.loans {
                        loan.holders.retain(|h| *h != root);
                    }
                    if self.locals[root.0].ty.is_second_class() {
                        for (i, loan) in self.loans.iter_mut().enumerate() {
                            if (i >= start || copied.contains(&i)) && !loan.holders.contains(&root) {
                                loan.holders.push(root);
                            }
                        }
                    }
                }
                self.access_place(place, Access::Write);
            }
            K::Binary(_, l, r) => {
                self.expr(l, Mode::Use);
                self.expr(r, Mode::Use);
            }
            K::Range { lo, hi, .. } => {
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
            }
            // A reference converted to a raw pointer leaves the safe borrow domain: the pointer may only be used
            // inside `unsafe`, where the programmer is responsible for the pointer's validity (memory-rules.md,
            // RF; issue #76). The conversion still conflicts with live loans on the place.
            K::Cast(inner) if matches!(e.ty, Ty::CPtr(_)) => match &inner.kind {
                K::Ref { place, mutable } => {
                    let access = if *mutable { Access::BorrowMut } else { Access::Read };
                    self.access_place(place, access);
                }
                _ => self.expr(inner, Mode::Use),
            },
            K::Unary(_, inner) | K::Cast(inner) | K::ArrayRepeat(inner, _) => self.expr(inner, Mode::Use),
            K::Array(elems) | K::Tuple(elems) => {
                for el in elems {
                    self.expr(el, Mode::Move);
                }
            }
            K::Struct(fields) | K::Variant(_, fields) => {
                for (_, v) in fields {
                    self.expr(v, Mode::Move);
                }
            }
            K::Some(inner) | K::Ok(inner) | K::Err(inner) => self.expr(inner, Mode::Move),
        }
    }

    fn call(&mut self, f: usize, args: &[tir::Expr]) {
        let sig = &self.env.sigs[f];
        for (i, arg) in args.iter().enumerate() {
            let Some(param) = sig.params.get(i) else {
                self.expr(arg, Mode::Use);
                continue;
            };
            if param.own {
                self.expr(arg, Mode::Move);
            } else if param.exclusive {
                // An exclusive borrow for the call (`mut` parameter or `mut self`). The exclusion between
                // arguments of the same call is checked in sema (EX1).
                self.access_place(arg, Access::BorrowMut);
            } else {
                self.expr(arg, Mode::Use);
            }
        }
        // A second-class result derived from an argument keeps that argument borrowed until the last use of the
        // result (RF5).
        if !self.skip_result_loan
            && sig.ret.is_second_class()
            && let Some(index) = sig.ret_origin
            && let Some(param) = sig.params.get(index)
            // A result that derives from an `own` parameter borrows from a value the callee drops, which the
            // origin check already rejects (RT2).
            && !param.own
            && let Some(arg) = args.get(index)
        {
            let mutable = param.exclusive || matches!(sig.ret, Ty::Ref { mutable: true, .. });
            self.add_loan(arg, mutable);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Move,
    Use,
}

/// The bindings of a pattern whose value is a view of the initializer: view bindings, and bindings of a
/// second-class type (which are copies of a reference).
fn view_bindings(pat: &Pat, locals: &[tir::Local], out: &mut Vec<LocalId>) {
    match pat {
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
        Pat::Binding(id, mode) => {
            if *mode == BindMode::View || locals[id.0].ty.is_second_class() {
                out.push(*id);
            }
        }
        Pat::At(id, mode, inner) => {
            if *mode == BindMode::View || locals[id.0].ty.is_second_class() {
                out.push(*id);
            }
            view_bindings(inner, locals, out);
        }
        Pat::Or(alts) => {
            if let Some(first) = alts.first() {
                view_bindings(first, locals, out);
            }
        }
        Pat::Some(inner) => view_bindings(inner, locals, out),
        Pat::Ok(inner) | Pat::Err(inner) => view_bindings(inner, locals, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => {
            for sub in subs {
                view_bindings(sub, locals, out);
            }
        }
    }
}

/// Every local an expression mentions, for the holder transfer of `let q = r` and for liveness.
fn locals_used(e: &tir::Expr, out: &mut Vec<LocalId>) {
    use tir::ExprKind as K;
    match &e.kind {
        K::Local(id) => out.push(*id),
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
        | K::LayoutOf { .. }
        | K::Return(None) => {}
        K::Field(base, _)
        | K::Deref(base)
        | K::PtrDeref(base)
        | K::DerefRef(base)
        | K::Unary(_, base)
        | K::Cast(base) => locals_used(base, out),
        K::Index(base, index) => {
            locals_used(base, out);
            locals_used(index, out);
        }
        K::Ref { place, .. } => locals_used(place, out),
        K::Slice { base, lo, hi, .. } => {
            locals_used(base, out);
            locals_used(lo, out);
            locals_used(hi, out);
        }
        K::StringAsStr(inner)
        | K::Some(inner)
        | K::Ok(inner)
        | K::Err(inner)
        | K::Return(Some(inner))
        | K::ArrayRepeat(inner, _)
        | K::BoxAny { value: inner, .. }
        | K::AnyIs { value: inner, .. }
        | K::AnyDowncast { value: inner, .. }
        | K::Comptime(inner) => locals_used(inner, out),
        K::Array(elems) | K::Tuple(elems) => elems.iter().for_each(|x| locals_used(x, out)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, x)| locals_used(x, out)),
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|x| locals_used(x, out));
        }
        K::AnyCall { recv, args, .. } => {
            locals_used(recv, out);
            args.iter().for_each(|x| locals_used(x, out));
        }
        K::Closure { captures, .. } => out.extend(captures.iter().map(|c| c.local)),
        K::ClosureCall { callee, args, .. } => {
            locals_used(callee, out);
            args.iter().for_each(|x| locals_used(x, out));
        }
        K::Binary(_, l, r) | K::Assign(l, r) => {
            locals_used(l, out);
            locals_used(r, out);
        }
        K::Range { lo, hi, .. } => {
            locals_used(lo, out);
            locals_used(hi, out);
        }
        K::Block(b) => locals_used_block(b, out),
        K::If(cond, then, other) => {
            locals_used(cond, out);
            locals_used_block(then, out);
            if let Some(other) = other {
                locals_used(other, out);
            }
        }
        K::While(cond, body) => {
            locals_used(cond, out);
            locals_used_block(body, out);
        }
        K::Loop(body) => locals_used_block(body, out),
        K::ForRange { lo, hi, body, .. } => {
            locals_used(lo, out);
            locals_used(hi, out);
            locals_used_block(body, out);
        }
        K::ForRangeValue { range, body, .. } => {
            locals_used(range, out);
            locals_used_block(body, out);
        }
        K::ForArray { array, body, .. } | K::ComptimeFor { iter: array, body, .. } => {
            locals_used(array, out);
            locals_used_block(body, out);
        }
        K::ForSlice { slice, body, .. } => {
            locals_used(slice, out);
            locals_used_block(body, out);
        }
        K::ForList { list, body, .. } => {
            locals_used(list, out);
            locals_used_block(body, out);
        }
        K::ForMap { map, body, .. } => {
            locals_used(map, out);
            locals_used_block(body, out);
        }
        K::ForChars { string, body, .. } => {
            locals_used(string, out);
            locals_used_block(body, out);
        }
        K::Match(scrutinee, arms) => {
            locals_used(scrutinee, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    locals_used(guard, out);
                }
                locals_used(&arm.body, out);
            }
        }
    }
}

fn locals_used_block(b: &tir::Block, out: &mut Vec<LocalId>) {
    for stmt in &b.stmts {
        match stmt {
            tir::Stmt::Let(_, init, _) => locals_used(init, out),
            tir::Stmt::Expr(e) => locals_used(e, out),
        }
    }
    if let Some(tail) = &b.tail {
        locals_used(tail, out);
    }
}

/// The highest top-level statement index that uses each local.
fn uses_expr(e: &tir::Expr, stmt: usize, last: &mut [Option<usize>]) {
    use tir::ExprKind as K;
    match &e.kind {
        K::Local(id) => last[id.0] = Some(last[id.0].map_or(stmt, |s: usize| s.max(stmt))),
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
        | K::LayoutOf { .. }
        | K::Return(None) => {}
        K::Field(base, _)
        | K::Deref(base)
        | K::PtrDeref(base)
        | K::DerefRef(base)
        | K::Unary(_, base)
        | K::Cast(base) => uses_expr(base, stmt, last),
        K::Index(base, index) => {
            uses_expr(base, stmt, last);
            uses_expr(index, stmt, last);
        }
        K::Ref { place, .. } => uses_expr(place, stmt, last),
        K::Slice { base, lo, hi, .. } => {
            uses_expr(base, stmt, last);
            uses_expr(lo, stmt, last);
            uses_expr(hi, stmt, last);
        }
        K::StringAsStr(inner)
        | K::Some(inner)
        | K::Ok(inner)
        | K::Err(inner)
        | K::Return(Some(inner))
        | K::ArrayRepeat(inner, _)
        | K::BoxAny { value: inner, .. }
        | K::AnyIs { value: inner, .. }
        | K::AnyDowncast { value: inner, .. }
        | K::Comptime(inner) => uses_expr(inner, stmt, last),
        K::Array(elems) | K::Tuple(elems) => elems.iter().for_each(|x| uses_expr(x, stmt, last)),
        K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, x)| uses_expr(x, stmt, last)),
        K::Call(_, args) | K::ExternCall(_, args) | K::Builtin(_, args) | K::Method(_, args) => {
            args.iter().for_each(|x| uses_expr(x, stmt, last));
        }
        K::AnyCall { recv, args, .. } => {
            uses_expr(recv, stmt, last);
            args.iter().for_each(|x| uses_expr(x, stmt, last));
        }
        K::Closure { captures, .. } => {
            for c in captures {
                last[c.local.0] = Some(last[c.local.0].map_or(stmt, |s: usize| s.max(stmt)));
            }
        }
        K::ClosureCall { callee, args, .. } => {
            uses_expr(callee, stmt, last);
            args.iter().for_each(|x| uses_expr(x, stmt, last));
        }
        K::Binary(_, l, r) | K::Assign(l, r) => {
            uses_expr(l, stmt, last);
            uses_expr(r, stmt, last);
        }
        K::Range { lo, hi, .. } => {
            uses_expr(lo, stmt, last);
            uses_expr(hi, stmt, last);
        }
        K::Block(b) => uses_block(b, stmt, last),
        K::If(cond, then, other) => {
            uses_expr(cond, stmt, last);
            uses_block(then, stmt, last);
            if let Some(other) = other {
                uses_expr(other, stmt, last);
            }
        }
        K::While(cond, body) => {
            uses_expr(cond, stmt, last);
            uses_block(body, stmt, last);
        }
        K::Loop(body) => uses_block(body, stmt, last),
        K::ForRange { lo, hi, body, .. } => {
            uses_expr(lo, stmt, last);
            uses_expr(hi, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForRangeValue { range, body, .. } => {
            uses_expr(range, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForArray { array, body, .. } | K::ComptimeFor { iter: array, body, .. } => {
            uses_expr(array, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForSlice { slice, body, .. } => {
            uses_expr(slice, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForList { list, body, .. } => {
            uses_expr(list, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForMap { map, body, .. } => {
            uses_expr(map, stmt, last);
            uses_block(body, stmt, last);
        }
        K::ForChars { string, body, .. } => {
            uses_expr(string, stmt, last);
            uses_block(body, stmt, last);
        }
        K::Match(scrutinee, arms) => {
            uses_expr(scrutinee, stmt, last);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    uses_expr(guard, stmt, last);
                }
                uses_expr(&arm.body, stmt, last);
            }
        }
    }
}

fn uses_block(b: &tir::Block, stmt: usize, last: &mut [Option<usize>]) {
    for s in &b.stmts {
        match s {
            tir::Stmt::Let(_, init, _) => uses_expr(init, stmt, last),
            tir::Stmt::Expr(e) => uses_expr(e, stmt, last),
        }
    }
    if let Some(tail) = &b.tail {
        uses_expr(tail, stmt, last);
    }
}

fn uses_stmt(stmt: &tir::Stmt, index: usize, last: &mut [Option<usize>]) {
    match stmt {
        tir::Stmt::Let(_, init, _) => uses_expr(init, index, last),
        tir::Stmt::Expr(e) => uses_expr(e, index, last),
    }
}
