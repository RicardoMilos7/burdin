//! The move checker: enforces the move rules of `spec/memory-rules.md` (MV) on one function body and records
//! every place read in a move context, so that the code generator knows which drop flags to keep. It also observes
//! every point where a scope can end, and marks a whole-local move `always_dead` when every observation saw it moved;
//! the code generator then omits its flag and its drop (issue #427).
//!
//! The analysis walks the typed tree in source order and keeps an initialization state for every local, as a
//! map from field paths to [`Status`]. A path with no entry of its own inherits its parent's state. `if`,
//! `match`, and loops merge the states of their branches; a place that is moved on some paths becomes
//! `Maybe`, which is what a runtime drop flag resolves.

use crate::Env;
use burdin_diag::{Code, Diagnostic, Diagnostics, Span};
use burdin_tir::{self as tir, BindMode, LocalId, LocalKind, Pat, Ty};
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
        self.paths.retain(|k, _| !(k.starts_with(path) && k.len() >= path.len()));
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
    env: Env<'a>,
    locals: &'a [tir::Local],
    diags: &'a mut Diagnostics,
    states: Vec<LocalState>,
    /// The locals in scope, innermost scope last.
    scopes: Vec<Vec<LocalId>>,
    loops: Vec<LoopInfo>,
    moved: Vec<tir::MovedPlace>,
    /// Which locals have been initialized at least once (parameters start declared).
    declared: Vec<bool>,
    /// For every declared local: whether every observation at a drop site saw it moved. `None` before the first
    /// observation, `Some(false)` once any exit could drop it while it is initialized.
    dead_at_exits: Vec<Option<bool>>,
}

/// Checks the body of `f` and returns the places it moves.
pub(crate) fn check_function(f: &tir::Function, env: Env, diags: &mut Diagnostics) -> Vec<tir::MovedPlace> {
    let mut cx = Checker {
        env,
        locals: &f.locals,
        diags,
        states: vec![LocalState::default(); f.locals.len()],
        scopes: vec![Vec::new()],
        loops: Vec::new(),
        moved: Vec::new(),
        declared: f.locals.iter().map(|local| matches!(local.kind, LocalKind::OwnParam)).collect(),
        dead_at_exits: vec![None; f.locals.len()],
    };
    cx.block(&f.body, Mode::Move);
    cx.observe_exits();
    for place in &mut cx.moved {
        place.always_dead = place.path.is_empty() && cx.dead_at_exits[place.local.0] == Some(true);
    }
    cx.moved
}

impl<'a> Checker<'a> {
    fn is_copy(&self, ty: &Ty) -> bool {
        self.env.defs.is_copy(ty)
    }

    fn error(&mut self, code: Code, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.diags.error(code, span, message)
    }

    /// The local and field path of a place expression. Field selections on any base extend the path; indexing
    /// and `Heap` dereferences keep the base's path (memory-rules.md, place). A field selection beyond one of them
    /// does not extend the path either: `a[i].f` and `(*h).f` are not places, so their tracked place is `a` and `h`.
    fn place_of(&self, e: &tir::Expr) -> Option<(LocalId, Vec<usize>)> {
        match &e.kind {
            tir::ExprKind::Local(id) => Some((*id, Vec::new())),
            tir::ExprKind::Field(base, i) => {
                let (root, mut path) = self.place_of(base)?;
                if is_tracked_place(base) {
                    path.push(*i);
                }
                Some((root, path))
            }
            tir::ExprKind::Index(base, index) => {
                let (root, mut path) = self.place_of(base)?;
                if is_tracked_place(base)
                    && matches!(base.ty, Ty::Array(..) | Ty::List(_, _))
                    && let Some(index) = constant_index(index)
                {
                    path.push(index);
                }
                Some((root, path))
            }
            tir::ExprKind::Deref(base) => {
                let (root, mut path) = self.place_of(base)?;
                if is_tracked_place(base) && matches!(base.ty, Ty::Heap(_, _)) {
                    path.push(tir::MOVED_DEREF);
                }
                Some((root, path))
            }
            tir::ExprKind::DerefRef(base) => self.place_of(base),
            // A raw pointer's pointee belongs to C memory, not to the local's value, so `*p` is a view of the
            // pointer itself and adds no path step.
            tir::ExprKind::PtrDeref(base) => self.place_of(base),
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
                let moved_at = self.states[root.0].moved_span(path);
                let error = self.error(Code::MOVED, span, format!("use of moved value `{name}`"));
                if let Some(moved_at) = moved_at {
                    error.note(moved_at, "value moved here");
                }
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

    /// Reports a use of a moved value for every proper prefix of `path`. Unlike [`Self::read_place`], a move
    /// inside a sibling field does not count: MV5 keeps the other fields usable.
    fn read_ancestors(&mut self, root: LocalId, path: &[usize], span: Span) {
        for len in 0..path.len() {
            let status = self.states[root.0].get(&path[..len]);
            let name = self.locals[root.0].name.clone();
            match status {
                Status::Moved => {
                    let moved_at = self.states[root.0].moved_span(&path[..len]);
                    let error = self.error(Code::MOVED, span, format!("use of moved value `{name}`"));
                    if let Some(moved_at) = moved_at {
                        error.note(moved_at, "value moved here");
                    }
                    return;
                }
                Status::Maybe => {
                    self.error(Code::MOVED, span, format!("use of possibly moved value `{name}`"));
                    return;
                }
                Status::Init => {}
            }
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
        if self.env.defs.has_drop_impl(ty) {
            self.error(Code::PARTIAL_MOVE_FROM_DROP, span, "cannot move a field out of a type that implements `Drop`");
            return;
        }
        let (first, rest) = path.split_first().expect("path is not empty");
        match ty {
            Ty::Struct(a) => {
                if let Some(f) = self.env.defs.structs[a.id].fields.get(*first) {
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
            // A constant array or `List` index selects one element.
            Ty::Array(elem, _) | Ty::List(elem, _) => {
                let elem = (**elem).clone();
                self.check_partial_move(&elem, rest, span);
            }
            Ty::Heap(inner, _) => {
                if *first == tir::MOVED_DEREF {
                    let inner = (**inner).clone();
                    self.check_partial_move(&inner, rest, span);
                }
            }
            // `[0]` is the payload of an `Option`; `[variant, field]` selects an enum field. The variant selector
            // itself is not a value, but the field's type can still implement `Drop`.
            Ty::Option(inner) => {
                if *first == 0 {
                    let inner = (**inner).clone();
                    self.check_partial_move(&inner, rest, span);
                }
            }
            Ty::Result(ok, err) => {
                let payload = if *first == 0 { (**ok).clone() } else { (**err).clone() };
                self.check_partial_move(&payload, rest, span);
            }
            Ty::Enum(a) => {
                let Some((field, rest)) = rest.split_first() else { return };
                if let Some(f) = self.env.defs.enums[a.id].variants.get(*first).and_then(|v| v.fields.get(*field)) {
                    let fty = f.ty.clone();
                    self.check_partial_move(&fty, rest, span);
                }
            }
            _ => {}
        }
    }

    /// Checks the move bindings of a pattern that is matched against a value without a tracked place (a
    /// temporary scrutinee or a `let` initializer), where [`Self::move_place`] is not called: moving a field out
    /// of a type that implements `Drop` is still E0103.
    fn check_pattern_drop_moves(&mut self, pat: &Pat, root_ty: &Ty, ty: &Ty, path: &mut Vec<usize>, span: Span) {
        match pat {
            Pat::Binding(_, BindMode::Move) | Pat::At(_, BindMode::Move, _) => {
                self.check_partial_move(root_ty, path, span);
            }
            Pat::At(_, _, inner) => self.check_pattern_drop_moves(inner, root_ty, ty, path, span),
            Pat::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.check_pattern_drop_moves(first, root_ty, ty, path, span);
                }
            }
            Pat::Some(inner) => {
                if let Ty::Option(inner_ty) = ty {
                    let inner_ty = (**inner_ty).clone();
                    path.push(0);
                    self.check_pattern_drop_moves(inner, root_ty, &inner_ty, path, span);
                    path.pop();
                }
            }
            Pat::Ok(inner) => {
                if let Ty::Result(ok, _) = ty {
                    let ok = (**ok).clone();
                    path.push(0);
                    self.check_pattern_drop_moves(inner, root_ty, &ok, path, span);
                    path.pop();
                }
            }
            Pat::Err(inner) => {
                if let Ty::Result(_, err) = ty {
                    let err = (**err).clone();
                    path.push(1);
                    self.check_pattern_drop_moves(inner, root_ty, &err, path, span);
                    path.pop();
                }
            }
            Pat::Product(subs) => {
                let tys: Vec<Ty> = match ty {
                    Ty::Tuple(ts) => ts.clone(),
                    Ty::Struct(a) => self.env.defs.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
                    _ => return,
                };
                for (i, sub) in subs.iter().enumerate() {
                    let Some(sub_ty) = tys.get(i) else { continue };
                    path.push(i);
                    self.check_pattern_drop_moves(sub, root_ty, sub_ty, path, span);
                    path.pop();
                }
            }
            Pat::Variant(index, subs) => {
                if let Ty::Enum(a) = ty {
                    let tys: Vec<Ty> =
                        self.env.defs.enums[a.id].variants[*index].fields.iter().map(|f| f.ty.clone()).collect();
                    for (i, sub) in subs.iter().enumerate() {
                        let Some(sub_ty) = tys.get(i) else { continue };
                        path.push(*index);
                        path.push(i);
                        self.check_pattern_drop_moves(sub, root_ty, sub_ty, path, span);
                        path.pop();
                        path.pop();
                    }
                }
            }
            _ => {}
        }
    }

    fn record_move(&mut self, root: LocalId, path: &[usize], span: Span) {
        let place = tir::MovedPlace { local: root, path: path.to_vec(), always_dead: false };
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
        if path.is_empty() {
            self.declared[root.0] = true;
        }
    }

    /// Records whether every declared local is moved at a point where its scope can end, which is what makes a
    /// whole-local move statically dead and saves its drop flag (issue #427).
    fn observe_exits(&mut self) {
        for (i, declared) in self.declared.iter().enumerate() {
            if !declared {
                continue;
            }
            let dead = self.states[i].get(&[]) == Status::Moved;
            self.dead_at_exits[i] = Some(self.dead_at_exits[i].unwrap_or(true) && dead);
        }
    }

    // ----- expressions -----------------------------------------------------

    fn expr(&mut self, e: &tir::Expr, mode: Mode) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::None | K::Const(_) => {}
            K::LayoutOf { .. } => {}
            K::Local(id) => self.use_local(*id, &[], e.span, mode),
            K::Field(..) | K::Index(..) | K::Deref(_) | K::PtrDeref(_) | K::DerefRef(_) => self.place_expr(e, mode),
            K::Ref { place, .. } => self.expr(place, Mode::Use),
            K::Slice { base, lo, hi, .. } => {
                self.expr(base, Mode::Use);
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
            }
            K::Call(f, args) => {
                let sig = &self.env.sigs[f.0];
                for (a, p) in args.iter().zip(&sig.params) {
                    let m = if p.own { Mode::Move } else { Mode::Use };
                    self.expr(a, m);
                }
            }
            // Every `extern` parameter is passed by value and is a C scalar or a `CPtr`, so all are Copy and the
            // arguments are only read.
            K::ExternCall(_, args) => {
                for a in args {
                    self.expr(a, Mode::Use);
                }
            }
            K::Builtin(b, args) => match b {
                tir::Builtin::HeapNew | tir::Builtin::SharedNew => self.expr(&args[0], Mode::Move),
                tir::Builtin::ListNewIn | tir::Builtin::MapNewIn => self.expr(&args[0], Mode::Move),
                // `with_capacity_in(n, alloc)` stores the allocator value.
                tir::Builtin::ListWithCapacityIn => {
                    self.expr(&args[0], Mode::Use);
                    self.expr(&args[1], Mode::Move);
                }
                tir::Builtin::HeapNewIn => {
                    self.expr(&args[0], Mode::Move);
                    self.expr(&args[1], Mode::Move);
                }
                tir::Builtin::Replace => {
                    self.expr(&args[0], Mode::Use);
                    self.expr(&args[1], Mode::Move);
                }
                _ => {
                    for a in args {
                        self.expr(a, Mode::Use);
                    }
                }
            },
            K::Method(m, args) => {
                for (i, a) in args.iter().enumerate() {
                    self.expr(a, if m.owns_argument(i) { Mode::Move } else { Mode::Use });
                }
            }
            // Boxing moves (or copies) the value into the heap allocation; `downcast` consumes the box.
            K::BoxAny { value, .. } => self.expr(value, Mode::Move),
            K::AnyIs { value, .. } => self.expr(value, Mode::Use),
            K::AnyDowncast { value, .. } => self.expr(value, Mode::Move),
            // A dynamic call borrows the box (it can never take `own self`); its arguments follow the interface
            // method's conventions.
            K::AnyCall { recv, params, args, .. } => {
                self.expr(recv, Mode::Use);
                for (a, p) in args.iter().zip(params) {
                    self.expr(a, if p.own { Mode::Move } else { Mode::Use });
                }
            }
            // Building a closure moves each moved-in capture and reads each borrowed one (CL2-CL4).
            K::Closure { captures, .. } => {
                for c in captures {
                    match c.mode {
                        tir::CaptureMode::Move => {
                            if self.is_copy(&c.ty) {
                                self.read_place(c.local, &[], e.span);
                            } else {
                                self.move_place(c.local, &[], e.span);
                            }
                        }
                        _ => self.read_place(c.local, &[], e.span),
                    }
                }
            }
            // Calling an `FnOnce` closure consumes it; an `Fn`/`FnMut` call only reads it (CL6). A closure
            // parameter is owned, so each argument is moved.
            K::ClosureCall { callee, args, kind } => {
                self.expr(callee, if *kind == tir::FnKind::FnOnce { Mode::Move } else { Mode::Use });
                for a in args {
                    self.expr(a, Mode::Move);
                }
            }
            K::StringAsStr(inner) => self.expr(inner, Mode::Use),
            K::Block(b) => {
                self.block(b, mode);
            }
            // A comptime expression uses only its own locals (spec/comptime.md, CT2), checked like any code.
            K::Comptime(inner) => self.expr(inner, mode),
            // The unrolled body is checked as a loop body over owned items (CT6), so a move of an outer value in
            // it is reported as a move in a loop, like any loop.
            K::ComptimeFor { pat, iter, body } => {
                self.expr(iter, Mode::Move);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let elem = match &iter.ty {
                    Ty::Range(t, _) | Ty::Array(t, _) | Ty::List(t, _) => (**t).clone(),
                    _ => Ty::Error,
                };
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, iter.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::If(cond, then, other) => self.if_expr(cond, then, other.as_deref(), mode),
            K::While(cond, body) => {
                // The condition runs before the first iteration and again after every iteration; analyzing it
                // inside the loop makes a move in it visible at the back edge (MV3).
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.expr(cond, Mode::Use);
                let after_cond = self.states.clone();
                let body_falls = self.block(body, Mode::Use);
                self.finish_loop(after_cond, body_falls);
            }
            K::Loop(body) => {
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                let body_falls = self.block(body, Mode::Use);
                self.finish_loop(entry, body_falls);
            }
            K::ForRangeValue { var, range, body } => {
                self.expr(range, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                if let Some(var) = var {
                    self.scopes.last_mut().expect("a scope is open").push(*var);
                }
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForRange { var, lo, hi, body, .. } => {
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                if let Some(var) = var {
                    self.scopes.last_mut().expect("a scope is open").push(*var);
                }
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForArray { pat, array, body } => {
                self.expr(array, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let Ty::Array(elem, _) = &array.ty else {
                    // A type error already reported; the body still gets checked.
                    self.bind_pattern(pat, &Ty::Error, &mut Vec::new(), None, array.span);
                    let body_falls = self.block(body, Mode::Use);
                    self.scopes.pop();
                    self.finish_loop(entry, body_falls);
                    return;
                };
                let elem = (**elem).clone();
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, array.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForSlice { pat, slice, body } => {
                self.expr(slice, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let Ty::Slice(elem) = &slice.ty else {
                    // A type error already reported; the body still gets checked.
                    self.bind_pattern(pat, &Ty::Error, &mut Vec::new(), None, slice.span);
                    let body_falls = self.block(body, Mode::Use);
                    self.scopes.pop();
                    self.finish_loop(entry, body_falls);
                    return;
                };
                let elem = (**elem).clone();
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, slice.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForList { pat, list, body } => {
                self.expr(list, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let elem = match &list.ty {
                    Ty::List(elem, _) => (**elem).clone(),
                    _ => Ty::Error,
                };
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, list.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForMap { pat, map, body } => {
                self.expr(map, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                let pair = match &map.ty {
                    Ty::Map(k, v, _) => Ty::Tuple(vec![(**k).clone(), (**v).clone()]),
                    _ => Ty::Error,
                };
                self.bind_pattern(pat, &pair, &mut Vec::new(), None, map.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::ForChars { pat, string, body } => {
                self.expr(string, Mode::Use);
                let entry = self.states.clone();
                let outer = self.outer_locals();
                self.loops.push(LoopInfo { outer, exits: Vec::new(), moves: HashMap::new() });
                self.scopes.push(Vec::new());
                self.bind_pattern(pat, &Ty::Char, &mut Vec::new(), None, string.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee, Mode::Use);
                let entry = self.states.clone();
                let root = self.place_of(scrutinee);
                // A pattern can move parts of a tracked place: fields, constant array or `List` indices, and the
                // fields of a `Heap` pointee. A runtime index or a slice element is not tracked.
                let crossing = root.is_some() && !is_tracked_place(scrutinee);
                let mut reported = false;
                let mut result: Option<Vec<LocalState>> = None;
                for arm in arms {
                    let moves = pattern_moves(&arm.pat);
                    if moves && crossing && !reported {
                        let (code, what) = if is_view_place(scrutinee) {
                            (Code::MOVE_FROM_BORROW, "cannot move out of a slice element")
                        } else {
                            (
                                Code::UNSUPPORTED, // e9000: move-runtime-index-match
                                "matching through a runtime index is not implemented in this compiler yet",
                            )
                        };
                        self.error(code, scrutinee.span, what);
                        reported = true;
                    }
                    self.states.clone_from(&entry);
                    self.scopes.push(Vec::new());
                    if moves && root.is_none() {
                        self.check_pattern_drop_moves(
                            &arm.pat,
                            &scrutinee.ty,
                            &scrutinee.ty,
                            &mut Vec::new(),
                            scrutinee.span,
                        );
                    }
                    if !(moves && crossing) {
                        self.bind_pattern(&arm.pat, &scrutinee.ty, &mut Vec::new(), root.as_ref(), scrutinee.span);
                    }
                    if let Some(g) = &arm.guard {
                        self.expr(g, Mode::Use);
                    }
                    self.expr(&arm.body, mode);
                    self.scopes.pop();
                    // An arm that does not fall through contributes no state to the code after the match.
                    if arm.body.ty != Ty::Never {
                        match &mut result {
                            Some(acc) => {
                                for (a, b) in acc.iter_mut().zip(&self.states) {
                                    a.merge_with(b);
                                }
                            }
                            None => result = Some(self.states.clone()),
                        }
                    }
                }
                self.states = result.unwrap_or(entry);
            }
            K::Break => {
                let state = self.states.clone();
                if let Some(lp) = self.loops.last_mut() {
                    lp.exits.push(state);
                }
                self.observe_exits();
            }
            K::Continue => {
                self.check_loop_moves();
                self.observe_exits();
            }
            K::Return(value) => {
                if let Some(v) = value {
                    self.expr(v, Mode::Move);
                }
                self.observe_exits();
            }
            K::Assign(place, value) => {
                self.expr(value, Mode::Move);
                if let Some((root, path)) = self.place_of(place) {
                    self.place_subexpr(place);
                    if is_tracked_place(place) {
                        // Assigning to a field uses the values above it, but not its sibling fields (MV5).
                        self.read_ancestors(root, &path, place.span);
                        self.init_place(root, &path);
                    } else {
                        // Assigning through a runtime index is not tracked; using the place only reads the base.
                        self.read_place(root, &path, place.span);
                    }
                } else {
                    self.expr(place, Mode::Use);
                }
            }
            K::Binary(_, l, r) => {
                self.expr(l, Mode::Use);
                self.expr(r, Mode::Use);
            }
            K::Range { lo, hi, .. } => {
                self.expr(lo, Mode::Move);
                self.expr(hi, Mode::Move);
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
            K::Some(inner) | K::Ok(inner) | K::Err(inner) => self.expr(inner, Mode::Move),
        }
    }

    /// A place expression read in `mode`.
    fn place_expr(&mut self, e: &tir::Expr, mode: Mode) {
        self.place_subexpr(e);
        // A place behind a call (the referent of a user `Index` implementation's reference) has no local root
        // the checker can follow. Reading a non-Copy value from it would move out of borrowed content (MV7,
        // `spec/operators.md`, OP10).
        if mode == Mode::Move && !self.is_copy(&e.ty) && is_view_place(e) && self.place_of(e).is_none() {
            self.error(Code::MOVE_FROM_BORROW, e.span, "cannot move out of borrowed content");
            return;
        }
        let Some((root, path)) = self.place_of(e) else { return };
        // The expression's own type is the place's type: an indexed element, a dereferenced pointee, or a field.
        let ty = e.ty.clone();
        if mode == Mode::Move && !self.is_copy(&ty) {
            // A move is tracked through fields, constant array and `List` indices, and `Heap` pointee fields.
            if !is_tracked_place(e) {
                if is_view_place(e) {
                    self.error(Code::MOVE_FROM_BORROW, e.span, "cannot move out of borrowed content");
                } else {
                    self.error(
                        Code::UNSUPPORTED, // e9000: move-runtime-index-place
                        e.span,
                        "moving a non-Copy value through a runtime index is not implemented in this compiler yet",
                    );
                }
                return;
            }
            self.move_place(root, &path, e.span);
        } else {
            self.read_place(root, &path, e.span);
        }
    }

    /// Evaluates the parts of a place expression that are not places themselves (indices, a call under a field).
    fn place_subexpr(&mut self, e: &tir::Expr) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Field(base, _) | K::Deref(base) | K::PtrDeref(base) | K::DerefRef(base) => {
                self.place_subexpr(base);
            }
            K::Index(base, index) => {
                self.place_subexpr(base);
                self.expr(index, Mode::Use);
            }
            _ => {
                if self.place_of(e).is_none() {
                    self.expr(e, Mode::Use);
                }
            }
        }
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
        let then_falls = self.block(then, mode);
        let then_state = self.states.clone();
        self.states = entry.clone();
        let else_falls = match other {
            Some(other) => {
                self.expr(other, mode);
                other.ty != Ty::Never
            }
            None => true,
        };
        // Only the branches that can reach the code after the `if` contribute to its state.
        self.states = match (then_falls, else_falls) {
            (true, true) => {
                let mut after = self.states.clone();
                for (a, b) in then_state.iter().zip(after.iter_mut()) {
                    b.merge_with(a);
                }
                after
            }
            (true, false) => then_state,
            (false, true) => self.states.clone(),
            (false, false) => entry,
        };
    }

    fn outer_locals(&self) -> Vec<LocalId> {
        self.scopes.iter().flatten().copied().collect()
    }

    /// Closes the innermost loop. `base` is the state of a path that runs the loop zero times (the state after
    /// the bounds of a `for`, or after the first evaluation of a `while` condition). `body_falls` is false when
    /// the body always leaves through `break`, `continue`, or `return`, so there is no back edge to check.
    fn finish_loop(&mut self, base: Vec<LocalState>, body_falls: bool) {
        if body_falls {
            self.check_loop_moves();
        }
        let lp = self.loops.pop().expect("a loop is open");
        let mut after = base;
        for exit in &lp.exits {
            for (a, b) in after.iter_mut().zip(exit) {
                a.merge_with(b);
            }
        }
        if body_falls {
            for (a, b) in after.iter_mut().zip(&self.states) {
                a.merge_with(b);
            }
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

    /// Checks a block and reports whether it can finish normally. Statements after one that diverges are
    /// unreachable, so they are not checked and the tail is ignored.
    fn block(&mut self, b: &tir::Block, mode: Mode) -> bool {
        self.scopes.push(Vec::new());
        let mut falls = true;
        for s in &b.stmts {
            match s {
                tir::Stmt::Let(pat, init, else_block) => {
                    // `let _ = e` does not bind, so a place in `e` is only read (DR5); an rvalue is a temporary.
                    let init_mode = if matches!(pat, Pat::Wild) { Mode::Use } else { Mode::Move };
                    self.expr(init, init_mode);
                    if init.ty == Ty::Never {
                        falls = false;
                        break;
                    }
                    if let Some(else_block) = else_block {
                        let after_init = self.states.clone();
                        self.block(else_block, Mode::Use);
                        self.states = after_init;
                    }
                    let ty = init.ty.clone();
                    if pattern_moves(pat) {
                        self.check_pattern_drop_moves(pat, &ty, &ty, &mut Vec::new(), init.span);
                    }
                    self.bind_pattern(pat, &ty, &mut Vec::new(), None, init.span);
                    let mut bound = Vec::new();
                    collect_bindings(pat, &mut bound);
                    for id in bound {
                        self.init_place(id, &[]);
                        self.scopes.last_mut().expect("a scope is open").push(id);
                    }
                }
                tir::Stmt::Expr(e) => {
                    self.expr(e, Mode::Move);
                    if e.ty == Ty::Never {
                        falls = false;
                        break;
                    }
                }
            }
        }
        if falls && let Some(tail) = &b.tail {
            self.expr(tail, mode);
            if tail.ty == Ty::Never {
                falls = false;
            }
        }
        if falls {
            self.observe_exits();
        }
        self.scopes.pop();
        falls
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
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
            Pat::Binding(id, mode) => {
                self.init_binding(*id, *mode, path, root, span);
            }
            Pat::At(id, mode, inner) => {
                self.init_binding(*id, *mode, path, root, span);
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
            Pat::Ok(inner) => {
                let Ty::Result(ok, _) = ty else { return };
                path.push(0);
                self.bind_pattern(inner, ok, path, root, span);
                path.pop();
            }
            Pat::Err(inner) => {
                let Ty::Result(_, err) = ty else { return };
                path.push(1);
                self.bind_pattern(inner, err, path, root, span);
                path.pop();
            }
            Pat::Product(subs) => {
                let tys: Vec<Ty> = match ty {
                    Ty::Tuple(ts) => ts.clone(),
                    Ty::Struct(a) => self.env.defs.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
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
                    self.env.defs.enums[a.id].variants[*index].fields.iter().map(|f| f.ty.clone()).collect();
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
    }
}

/// A place the analysis can follow as a path in the local's type: a local, its fields, constant array and
/// `List` indices, and the fields of a `Heap` pointee.
fn is_tracked_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) => true,
        tir::ExprKind::Field(base, _) => is_tracked_place(base),
        tir::ExprKind::Index(base, index) => {
            is_tracked_place(base)
                && matches!(base.ty, Ty::Array(..) | Ty::List(_, _))
                && constant_index(index).is_some()
        }
        tir::ExprKind::Deref(base) => is_tracked_place(base) && matches!(base.ty, Ty::Heap(_, _)),
        _ => false,
    }
}

/// An index written as a non-negative integer literal.
fn constant_index(e: &tir::Expr) -> Option<usize> {
    match e.kind {
        tir::ExprKind::Int(v) if v >= 0 => usize::try_from(v).ok(),
        _ => None,
    }
}

/// Whether a place expression goes through a reference or a slice, so its storage belongs to someone else.
fn is_view_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::DerefRef(_) | tir::ExprKind::PtrDeref(_) => true,
        tir::ExprKind::Index(base, _) => is_view_place(base) || matches!(base.ty, Ty::Slice(_)),
        tir::ExprKind::Field(base, _) => is_view_place(base),
        _ => false,
    }
}

/// Whether any binding in `pat` takes its value by moving it out of the scrutinee.
fn pattern_moves(pat: &Pat) -> bool {
    match pat {
        Pat::Binding(_, BindMode::Move) => true,
        Pat::At(_, BindMode::Move, _) => true,
        Pat::At(_, _, inner) => pattern_moves(inner),
        Pat::Or(alts) => alts.iter().any(pattern_moves),
        Pat::Some(inner) => pattern_moves(inner),
        Pat::Ok(inner) | Pat::Err(inner) => pattern_moves(inner),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().any(pattern_moves),
        _ => false,
    }
}

fn collect_bindings(pat: &Pat, out: &mut Vec<LocalId>) {
    match pat {
        Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) | Pat::None => {}
        Pat::Binding(id, _) => out.push(*id),
        Pat::At(id, _, inner) => {
            out.push(*id);
            collect_bindings(inner, out);
        }
        Pat::Or(alts) => alts.iter().for_each(|a| collect_bindings(a, out)),
        Pat::Some(inner) => collect_bindings(inner, out),
        Pat::Ok(inner) | Pat::Err(inner) => collect_bindings(inner, out),
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().for_each(|s| collect_bindings(s, out)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use burdin_diag::FileId;

    fn span(lo: u32) -> Span {
        Span::new(FileId(0), lo, lo + 1)
    }

    #[test]
    fn statuses_merge_to_maybe_only_when_they_differ() {
        assert_eq!(Status::Init.merge(Status::Init), Status::Init);
        assert_eq!(Status::Moved.merge(Status::Moved), Status::Moved);
        assert_eq!(Status::Maybe.merge(Status::Maybe), Status::Maybe);
        assert_eq!(Status::Init.merge(Status::Moved), Status::Maybe);
        assert_eq!(Status::Moved.merge(Status::Init), Status::Maybe);
        assert_eq!(Status::Moved.merge(Status::Maybe), Status::Maybe);
    }

    #[test]
    fn paths_inherit_the_state_of_their_parent() {
        let mut state = LocalState::default();
        assert_eq!(state.get(&[]), Status::Init);
        assert_eq!(state.get(&[0, 1]), Status::Init);
        state.move_out(&[0], span(4));
        assert_eq!(state.get(&[0]), Status::Moved);
        assert_eq!(state.get(&[0, 1]), Status::Moved, "a path inherits from its parent");
        assert_eq!(state.get(&[1]), Status::Init);
        assert_eq!(state.moved_span(&[0, 7]), Some(span(4)));
        assert_eq!(state.moved_span(&[1]), None);
        assert!(state.has_moved_inside(&[]));
        assert!(!state.has_moved_inside(&[0]));
        assert!(!state.has_moved_inside(&[1]));
    }

    #[test]
    fn moving_marks_tracked_descendants_and_reinit_clears_them() {
        let mut state = LocalState::default();
        state.move_out(&[1, 2], span(1));
        state.move_out(&[1, 3], span(2));
        assert_eq!(state.get(&[1, 2]), Status::Moved);
        assert_eq!(state.get(&[1, 3]), Status::Moved);
        state.reinit(&[1]);
        assert_eq!(state.get(&[1]), Status::Init);
        assert_eq!(state.get(&[1, 2]), Status::Init);
        assert_eq!(state.get(&[1, 3]), Status::Init);
    }

    #[test]
    fn merging_a_moved_path_with_an_initialized_one_makes_it_maybe() {
        let mut moved = LocalState::default();
        moved.move_out(&[0], span(5));
        moved.merge_with(&LocalState::default());
        assert_eq!(moved.get(&[0]), Status::Maybe);
        assert_eq!(moved.moved_span(&[0]), None, "a maybe-moved path has no single move site");
    }

    #[test]
    fn merging_two_moves_keeps_the_first_move_site() {
        let mut a = LocalState::default();
        a.move_out(&[0], span(1));
        let mut b = LocalState::default();
        b.move_out(&[0], span(9));
        a.merge_with(&b);
        assert_eq!(a.get(&[0]), Status::Moved);
        assert_eq!(a.moved_span(&[0]), Some(span(1)));
    }

    #[test]
    fn merging_initialized_paths_drops_their_entries() {
        let mut a = LocalState::default();
        a.move_out(&[0], span(1));
        a.reinit(&[0]);
        let mut b = LocalState::default();
        b.move_out(&[0], span(2));
        b.reinit(&[0]);
        a.merge_with(&b);
        assert!(a.paths.is_empty(), "an initialized place needs no entry");
    }
}
