//! The move checker: enforces the move rules of `spec/memory-rules.md` (MV) on one function body and records
//! every place read in a move context, so that the code generator knows which drop flags to keep.
//!
//! The analysis walks the typed tree in source order and keeps an initialization state for every local, as a
//! map from field paths to [`Status`]. A path with no entry of its own inherits its parent's state. `if`,
//! `match`, and loops merge the states of their branches; a place that is moved on some paths becomes
//! `Maybe`, which is what a runtime drop flag resolves.

use crate::Globals;
use crate::tir::{self, BindMode, LocalId, LocalKind, Pat, Ty};
use burdin_diag::{Code, Diagnostics, Span};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Init,
    Moved,
    Maybe,
}

impl Status {
    fn merge(self, other: Status) -> Status {
        if self == other { self } else { Status::Maybe }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    status: Status,
    /// Where the last move happened, for E0104.
    moved_at: Option<Span>,
}

/// The initialization state of one local.
#[derive(Clone, Default)]
struct LocalState {
    paths: HashMap<Vec<usize>, Entry>,
}

impl LocalState {
    /// The state of `path`, inheriting from its ancestors. Absent means initialized.
    fn get(&self, path: &[usize]) -> Status {
        if let Some(e) = self.paths.get(path) {
            return e.status;
        }
        match path.split_last() {
            Some((_, parent)) => self.get(parent),
            None => Status::Init,
        }
    }

    fn moved_span(&self, path: &[usize]) -> Option<Span> {
        if let Some(e) = self.paths.get(path) {
            return e.moved_at;
        }
        path.split_last().and_then(|(_, parent)| self.moved_span(parent))
    }

    /// Marks `path` as moved; every tracked descendant is moved with it.
    fn move_out(&mut self, path: &[usize], span: Span) {
        self.paths.insert(path.to_vec(), Entry { status: Status::Moved, moved_at: Some(span) });
        let descendants: Vec<Vec<usize>> =
            self.paths.keys().filter(|k| k.len() > path.len() && k.starts_with(path)).cloned().collect();
        for d in descendants {
            self.paths.insert(d, Entry { status: Status::Moved, moved_at: Some(span) });
        }
    }

    /// Marks `path` as initialized again; its descendants are initialized too.
    fn reinit(&mut self, path: &[usize]) {
        self.paths.retain(|k, _| !(k.as_slice().starts_with(path) && k.len() >= path.len()));
        self.paths.insert(path.to_vec(), Entry { status: Status::Init, moved_at: None });
    }

    /// Whether any tracked place strictly inside `path` is moved or maybe moved.
    fn has_moved_inside(&self, path: &[usize]) -> bool {
        self.paths.iter().any(|(k, e)| k.len() > path.len() && k.starts_with(path) && e.status != Status::Init)
    }

    fn merge_with(&mut self, other: &LocalState) {
        let mut keys: Vec<Vec<usize>> = self.paths.keys().cloned().collect();
        for k in other.paths.keys() {
            if !self.paths.contains_key(k) {
                keys.push(k.clone());
            }
        }
        let merged: Vec<(Vec<usize>, Entry)> = keys
            .into_iter()
            .filter_map(|k| {
                let a = self.get(&k);
                let b = other.get(&k);
                let status = a.merge(b);
                if status == Status::Init {
                    return None;
                }
                let moved_at =
                    if status == Status::Moved { self.moved_span(&k).or_else(|| other.moved_span(&k)) } else { None };
                Some((k, Entry { status, moved_at }))
            })
            .collect();
        self.paths = merged.into_iter().collect();
    }
}

#[derive(Clone)]
struct LoopInfo {
    /// The states at the loop entry.
    entry: Vec<LocalState>,
    /// Locals that were already in scope when the loop started, in scope order.
    outer: Vec<LocalId>,
    /// States of the paths leaving through `break`.
    exits: Vec<Vec<LocalState>>,
    /// Moves of outer places made in the body, with the span to report E0104 at.
    moves: HashMap<(usize, Vec<usize>), Span>,
}

/// What an expression's value is used for. `Move` consumes it; `Use` only reads or borrows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Move,
    Use,
}

struct Checker<'a> {
    globals: &'a Globals,
    locals: &'a [tir::Local],
    diags: &'a mut Diagnostics,
    states: Vec<LocalState>,
    /// The locals in scope, innermost scope last.
    scopes: Vec<Vec<LocalId>>,
    loops: Vec<LoopInfo>,
    moved: Vec<tir::MovedPlace>,
}

/// Checks the body of `f` and returns the places it moves.
pub fn check_function(f: &tir::Function, globals: &Globals, diags: &mut Diagnostics) -> Vec<tir::MovedPlace> {
    let mut cx = Checker {
        globals,
        locals: &f.locals,
        diags,
        states: vec![LocalState::default(); f.locals.len()],
        scopes: vec![Vec::new()],
        loops: Vec::new(),
        moved: Vec::new(),
    };
    cx.block(&f.body, Mode::Move);
    cx.moved
}

impl<'a> Checker<'a> {
    fn is_copy(&self, ty: &Ty) -> bool {
        self.globals.is_copy(ty)
    }

    fn error(&mut self, code: Code, span: Span, message: impl Into<String>) {
        self.diags.error(code, span, message);
    }

    /// The local and field path of a place expression. Field selections on any base extend the path; indexing
    /// and `Heap` dereferences keep the base's path (memory-rules.md, place).
    fn place_of(&self, e: &tir::Expr) -> Option<(LocalId, Vec<usize>)> {
        match &e.kind {
            tir::ExprKind::Local(id) => Some((*id, Vec::new())),
            tir::ExprKind::Field(base, i) => {
                let (root, mut path) = self.place_of(base)?;
                path.push(*i);
                Some((root, path))
            }
            tir::ExprKind::Index(base, _) | tir::ExprKind::Deref(base) => self.place_of(base),
            _ => None,
        }
    }

    /// Reports E0100 unless the place at `path` is fully initialized (including its parts).
    fn read_place(&mut self, root: LocalId, path: &[usize], span: Span) {
        let status = self.states[root.0].get(path);
        let partial = self.states[root.0].has_moved_inside(path);
        let name = &self.locals[root.0].name;
        match status {
            Status::Moved => {
                let name = name.clone();
                self.error(Code::MOVED, span, format!("use of moved value `{name}`"));
            }
            Status::Maybe => {
                let name = name.clone();
                self.error(Code::MOVED, span, format!("use of possibly moved value `{name}`"));
            }
            Status::Init if partial => {
                let name = name.clone();
                self.error(Code::MOVED, span, format!("use of partially moved value `{name}`"));
            }
            Status::Init => {}
        }
    }

    /// Moves the place at `path` out of `root` (MV1, MV7, MV5). Reports the errors that make a move illegal.
    fn move_place(&mut self, root: LocalId, path: &[usize], span: Span) {
        self.read_place(root, path, span);
        let local = &self.locals[root.0];
        if matches!(local.kind, LocalKind::BorrowedParam | LocalKind::View) {
            let name = local.name.clone();
            self.error(Code::MOVE_FROM_BORROW, span, format!("cannot move out of `{name}`: it is only borrowed"));
            return;
        }
        self.check_partial_move(&local.ty.clone(), path, span);
        self.states[root.0].move_out(path, span);
        self.record_move(root, path, span);
    }

    /// A field move out of a type that implements `Drop` is E0103.
    fn check_partial_move(&mut self, ty: &Ty, path: &[usize], span: Span) {
        if path.is_empty() {
            return;
        }
        if self.globals.has_drop_impl(ty) {
            self.error(Code::PARTIAL_MOVE_FROM_DROP, span, "cannot move a field out of a type that implements `Drop`");
            return;
        }
        let (first, rest) = path.split_first().expect("path is not empty");
        match ty {
            Ty::Struct(a) => {
                if let Some(f) = self.globals.structs[a.id].fields.get(*first) {
                    let fty = f.ty.clone();
                    self.check_partial_move(&fty, rest, span);
                }
            }
            Ty::Tuple(ts) => {
                if let Some(t) = ts.get(*first) {
                    let t = t.clone();
                    self.check_partial_move(&t, rest, span);
                }
            }
            // The variant selector is not a value; deeper fields live inside the matched variant.
            _ => {}
        }
    }

    fn record_move(&mut self, root: LocalId, path: &[usize], span: Span) {
        let place = tir::MovedPlace { local: root, path: path.to_vec() };
        if !self.moved.contains(&place) {
            self.moved.push(place);
        }
        for lp in &mut self.loops {
            if lp.outer.contains(&root) {
                lp.moves.insert((root.0, path.to_vec()), span);
            }
        }
    }

    fn init_place(&mut self, root: LocalId, path: &[usize]) {
        self.states[root.0].reinit(path);
    }

    // ----- expressions -----------------------------------------------------

    fn expr(&mut self, e: &tir::Expr, mode: Mode) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::None | K::Const(_) => {}
            K::Local(id) => self.use_local(*id, &[], e.span, mode),
            K::Field(..) | K::Index(..) | K::Deref(_) => self.place_expr(e, mode),
            K::Call(f, args) => {
                let sig = &self.globals.sigs[f.0];
                for (a, p) in args.iter().zip(&sig.params) {
                    let m = if p.own { Mode::Move } else { Mode::Use };
                    self.expr(a, m);
                }
            }
            K::Builtin(b, args) => {
                let m = if *b == tir::Builtin::HeapNew { Mode::Move } else { Mode::Use };
                for a in args {
                    self.expr(a, m);
                }
            }
            K::Method(_, args) => {
                for a in args {
                    self.expr(a, Mode::Use);
                }
            }
            K::StringAsStr(inner) => self.expr(inner, Mode::Use),
            K::Block(b) => {
                self.block(b, mode);
            }
            K::If(cond, then, other) => self.if_expr(cond, then, other.as_deref(), mode),
            K::While(cond, body) => {
                self.expr(cond, Mode::Use);
                self.loop_body(body);
            }
            K::Loop(body) => self.loop_body(body),
            K::ForRange { var, lo, hi, body, .. } => {
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { entry, outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                if let Some(var) = var {
                    self.scopes.last_mut().expect("a scope is open").push(*var);
                }
                self.block(body, Mode::Use);
                self.scopes.pop();
                self.end_loop();
            }
            K::ForArray { pat, array, body } => {
                self.expr(array, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { entry, outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let Ty::Array(elem, _) = &array.ty else { unreachable!("a `for` array has an array type") };
                let elem = (**elem).clone();
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, array.span);
                self.block(body, Mode::Use);
                self.scopes.pop();
                self.end_loop();
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee, Mode::Use);
                let entry = self.states.clone();
                let root = self.place_of(scrutinee);
                let mut result: Option<Vec<LocalState>> = None;
                for arm in arms {
                    self.states.clone_from(&entry);
                    self.scopes.push(Vec::new());
                    self.bind_pattern(&arm.pat, &scrutinee.ty, &mut Vec::new(), root.as_ref(), scrutinee.span);
                    if let Some(g) = &arm.guard {
                        self.expr(g, Mode::Use);
                    }
                    self.expr(&arm.body, mode);
                    self.scopes.pop();
                    match &mut result {
                        Some(acc) => {
                            for (a, b) in acc.iter_mut().zip(&self.states) {
                                a.merge_with(b);
                            }
                        }
                        None => result = Some(self.states.clone()),
                    }
                }
                if let Some(r) = result {
                    self.states = r;
                }
            }
            K::Break => {
                let state = self.states.clone();
                if let Some(lp) = self.loops.last_mut() {
                    lp.exits.push(state);
                }
            }
            K::Continue => {
                self.check_loop_moves();
            }
            K::Return(value) => {
                if let Some(v) = value {
                    self.expr(v, Mode::Move);
                }
            }
            K::Assign(place, value) => {
                self.expr(value, Mode::Move);
                if let Some((root, path)) = self.place_of(place) {
                    if !path.is_empty() {
                        // Assigning to a field is a use of everything above it.
                        self.read_place(root, &path[..path.len() - 1], place.span);
                    }
                    self.expr_place_base(place);
                    self.init_place(root, &path);
                } else {
                    self.expr(place, Mode::Use);
                }
            }
            K::Binary(_, l, r) => {
                self.expr(l, Mode::Use);
                self.expr(r, Mode::Use);
            }
            K::Unary(_, operand) | K::Cast(operand) => self.expr(operand, Mode::Use),
            K::Array(elems) => {
                for el in elems {
                    self.expr(el, Mode::Move);
                }
            }
            K::ArrayRepeat(value, _) => self.expr(value, Mode::Move),
            K::Tuple(elems) => {
                for el in elems {
                    self.expr(el, Mode::Move);
                }
            }
            K::Struct(fields) | K::Variant(_, fields) => {
                for (_, v) in fields {
                    self.expr(v, Mode::Move);
                }
            }
            K::Some(inner) => self.expr(inner, Mode::Move),
        }
    }

    /// A place expression read in `mode`.
    fn place_expr(&mut self, e: &tir::Expr, mode: Mode) {
        use tir::ExprKind as K;
        // The subexpressions of the place (indices, the heap expression, the base of a non-place) have their
        // own effects; evaluate them in use position.
        match &e.kind {
            K::Field(base, _) => self.expr_place_base(base),
            K::Index(base, index) => {
                self.expr_place_base(base);
                self.expr(index, Mode::Use);
            }
            K::Deref(base) => self.expr_place_base(base),
            _ => {}
        }
        let Some((root, path)) = self.place_of(e) else { return };
        // The expression's own type is the place's type: an indexed element, a dereferenced pointee, or a field.
        let ty = e.ty.clone();
        if mode == Mode::Move && !self.is_copy(&ty) {
            self.move_place(root, &path, e.span);
        } else {
            self.read_place(root, &path, e.span);
        }
    }

    /// The base of a place: evaluate its non-place parts (a call or a constructed value) without reading it.
    fn expr_place_base(&mut self, e: &tir::Expr) {
        if self.place_of(e).is_some() {
            return;
        }
        self.expr(e, Mode::Use);
    }

    fn use_local(&mut self, id: LocalId, path: &[usize], span: Span, mode: Mode) {
        if mode == Mode::Move && !self.is_copy(&self.locals[id.0].ty.clone()) {
            self.move_place(id, path, span);
        } else {
            self.read_place(id, path, span);
        }
    }

    fn if_expr(&mut self, cond: &tir::Expr, then: &tir::Block, other: Option<&tir::Expr>, mode: Mode) {
        self.expr(cond, Mode::Use);
        let entry = self.states.clone();
        self.block(then, mode);
        let then_state = self.states.clone();
        self.states = entry;
        if let Some(other) = other {
            self.expr(other, mode);
        }
        for (a, b) in then_state.iter().zip(self.states.iter_mut()) {
            b.merge_with(a);
        }
    }

    fn loop_body(&mut self, body: &tir::Block) {
        let entry = self.states.clone();
        let outer = self.outer_locals();
        self.loops.push(LoopInfo { entry, outer, exits: Vec::new(), moves: HashMap::new() });
        self.block(body, Mode::Use);
        self.end_loop();
    }

    fn outer_locals(&self) -> Vec<LocalId> {
        self.scopes.iter().flatten().copied().collect()
    }

    /// Closes the innermost loop: reports E0104 for outer places still moved at the back edge, and merges the
    /// entry, exit, and body-end states.
    fn end_loop(&mut self) {
        self.check_loop_moves();
        let lp = self.loops.pop().expect("a loop is open");
        let mut after = lp.entry.clone();
        for exit in &lp.exits {
            for (a, b) in after.iter_mut().zip(exit) {
                a.merge_with(b);
            }
        }
        for (a, b) in after.iter_mut().zip(&self.states) {
            a.merge_with(b);
        }
        self.states = after;
    }

    fn check_loop_moves(&mut self) {
        let Some(lp) = self.loops.last() else { return };
        let mut reports: Vec<(Vec<usize>, Span)> = Vec::new();
        for ((local, path), span) in &lp.moves {
            if self.states[*local].get(path) != Status::Init {
                reports.push((path.clone(), *span));
            }
        }
        for (_, span) in reports {
            self.error(
                Code::MOVED_IN_LOOP,
                span,
                "this value is moved in a previous loop iteration; reinitialize it before the next one",
            );
        }
    }

    // ----- statements, patterns, and blocks ---------------------------------

    fn block(&mut self, b: &tir::Block, mode: Mode) {
        self.scopes.push(Vec::new());
        for s in &b.stmts {
            match s {
                tir::Stmt::Let(pat, init, else_block) => {
                    self.expr(init, Mode::Move);
                    if let Some(else_block) = else_block {
                        let after_init = self.states.clone();
                        self.block(else_block, Mode::Use);
                        self.states = after_init;
                    }
                    let ty = init.ty.clone();
                    self.bind_pattern(pat, &ty, &mut Vec::new(), None, init.span);
                    let mut bound = Vec::new();
                    collect_bindings(pat, &mut bound);
                    for id in bound {
                        self.init_place(id, &[]);
                        self.scopes.last_mut().expect("a scope is open").push(id);
                    }
                }
                tir::Stmt::Expr(e) => self.expr(e, Mode::Move),
            }
        }
        if let Some(tail) = &b.tail {
            self.expr(tail, mode);
        }
        self.scopes.pop();
    }

    /// Initializes the bindings of `pat` and marks the parts it moves out of `root` (MV8). `root` is the
    /// scrutinee place, when it is a place.
    fn bind_pattern(
        &mut self,
        pat: &Pat,
        ty: &Ty,
        path: &mut Vec<usize>,
        root: Option<&(LocalId, Vec<usize>)>,
        span: Span,
    ) {
        match pat {
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Range(..) | Pat::None => {}
            Pat::Binding(id, mode) => {
                self.init_binding(*id, *mode, path, root, ty, span);
            }
            Pat::At(id, mode, inner) => {
                self.init_binding(*id, *mode, path, root, ty, span);
                self.bind_pattern(inner, ty, path, root, span);
            }
            Pat::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.bind_pattern(first, ty, path, root, span);
                }
            }
            Pat::Some(inner) => {
                let Ty::Option(inner_ty) = ty else { return };
                path.push(0);
                self.bind_pattern(inner, inner_ty, path, root, span);
                path.pop();
            }
            Pat::Product(subs) => {
                let tys: Vec<Ty> = match ty {
                    Ty::Tuple(ts) => ts.clone(),
                    Ty::Struct(a) => self.globals.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
                    Ty::Unit => Vec::new(),
                    _ => return,
                };
                for (i, sub) in subs.iter().enumerate() {
                    let Some(sub_ty) = tys.get(i) else { continue };
                    path.push(i);
                    self.bind_pattern(sub, sub_ty, path, root, span);
                    path.pop();
                }
            }
            Pat::Variant(index, subs) => {
                let Ty::Enum(a) = ty else { return };
                let tys: Vec<Ty> =
                    self.globals.enums[a.id].variants[*index].fields.iter().map(|f| f.ty.clone()).collect();
                for (i, sub) in subs.iter().enumerate() {
                    let Some(sub_ty) = tys.get(i) else { continue };
                    path.push(*index);
                    path.push(i);
                    self.bind_pattern(sub, sub_ty, path, root, span);
                    path.pop();
                    path.pop();
                }
            }
        }
    }

    fn init_binding(
        &mut self,
        id: LocalId,
        mode: BindMode,
        path: &[usize],
        root: Option<&(LocalId, Vec<usize>)>,
        ty: &Ty,
        span: Span,
    ) {
        self.init_place(id, &[]);
        if mode == BindMode::Move
            && let Some((root, base)) = root
        {
            let mut full = base.clone();
            full.extend_from_slice(path);
            self.move_place(*root, &full, span);
        }
        let _ = ty;
    }
}

fn collect_bindings(pat: &Pat, out: &mut Vec<LocalId>) {
    match pat {
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Range(..) | Pat::None => {}
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            collect_bindings(inner, out);
        }
        Pat::Or(alts) => alts.iter().for_each(|a| collect_bindings(a, out)),
        Pat::Some(inner) => collect_bindings(inner, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().for_each(|s| collect_bindings(s, out)),
    }
}
