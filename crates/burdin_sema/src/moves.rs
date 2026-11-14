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
    /// and `Heap` dereferences keep the base's path (memory-rules.md, place). A field selection beyond one of them
    /// does not extend the path either: `a[i].f` and `(*h).f` are not places, so their tracked place is `a` and `h`.
    fn place_of(&self, e: &tir::Expr) -> Option<(LocalId, Vec<usize>)> {
        match &e.kind {
            tir::ExprKind::Local(id) => Some((*id, Vec::new())),
            tir::ExprKind::Field(base, i) => {
                let (root, mut path) = self.place_of(base)?;
                if is_direct_place(base) {
                    path.push(*i);
                }
                Some((root, path))
            }
            tir::ExprKind::Index(base, _) | tir::ExprKind::Deref(base) | tir::ExprKind::DerefRef(base) => {
                self.place_of(base)
            }
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

    /// Reports a use of a moved value for every proper prefix of `path`. Unlike [`Self::read_place`], a move
    /// inside a sibling field does not count: MV5 keeps the other fields usable.
    fn read_ancestors(&mut self, root: LocalId, path: &[usize], span: Span) {
        for len in 0..path.len() {
            let status = self.states[root.0].get(&path[..len]);
            let name = self.locals[root.0].name.clone();
            match status {
                Status::Moved => {
                    self.error(Code::MOVED, span, format!("use of moved value `{name}`"));
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
            // `[0]` is the payload of an `Option`; `[variant, field]` selects an enum field. The variant selector
            // itself is not a value, but the field's type can still implement `Drop`.
            Ty::Option(inner) => {
                if *first == 0 {
                    let inner = (**inner).clone();
                    self.check_partial_move(&inner, rest, span);
                }
            }
            Ty::Enum(a) => {
                let Some((field, rest)) = rest.split_first() else { return };
                if let Some(f) = self.globals.enums[a.id].variants.get(*first).and_then(|v| v.fields.get(*field)) {
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
            Pat::Product(subs) => {
                let tys: Vec<Ty> = match ty {
                    Ty::Tuple(ts) => ts.clone(),
                    Ty::Struct(a) => self.globals.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
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
                        self.globals.enums[a.id].variants[*index].fields.iter().map(|f| f.ty.clone()).collect();
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
            K::Field(..) | K::Index(..) | K::Deref(_) | K::DerefRef(_) => self.place_expr(e, mode),
            K::Ref { place, .. } => self.expr(place, Mode::Use),
            K::Slice { base, lo, hi } => {
                self.expr(base, Mode::Use);
                self.expr(lo, Mode::Use);
                self.expr(hi, Mode::Use);
            }
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
                let Ty::Array(elem, _) = &array.ty else { unreachable!("a `for` array has an array type") };
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
                let Ty::Slice(elem) = &slice.ty else { unreachable!("a `for` slice has a slice type") };
                let elem = (**elem).clone();
                self.bind_pattern(pat, &elem, &mut Vec::new(), None, slice.span);
                let body_falls = self.block(body, Mode::Use);
                self.scopes.pop();
                self.finish_loop(entry, body_falls);
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee, Mode::Use);
                let entry = self.states.clone();
                let root = self.place_of(scrutinee);
                // Indexing and `Heap` dereferences are not places, so matching through them cannot track the
                // parts an arm moves. Matching on the whole of `*h` would also have to free the allocation.
                let crossing = root.is_some() && !is_direct_place(scrutinee) && !is_whole_deref(scrutinee);
                let whole_deref = root.is_some() && is_whole_deref(scrutinee);
                let mut reported = false;
                let mut result: Option<Vec<LocalState>> = None;
                for arm in arms {
                    let moves = pattern_moves(&arm.pat);
                    if moves && (crossing || whole_deref) && !reported {
                        let what = if crossing {
                            "matching through indexing or a `Heap` dereference"
                        } else {
                            "moving the value behind a `Heap` out of a `match`"
                        };
                        self.error(
                            Code::UNSUPPORTED,
                            scrutinee.span,
                            format!("{what} is not implemented in this compiler yet"),
                        );
                        reported = true;
                    }
                    if moves
                        && root.is_none()
                        && let Some(guard) = &arm.guard
                    {
                        self.error(
                            Code::UNSUPPORTED,
                            guard.span,
                            "a guard on an arm that moves out of a temporary is not implemented in this compiler yet",
                        );
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
                    self.place_subexpr(place);
                    if is_direct_place(place) {
                        // Assigning to a field uses the values above it, but not its sibling fields (MV5).
                        self.read_ancestors(root, &path, place.span);
                        self.init_place(root, &path);
                    } else {
                        // Assigning through an index or a `Heap` dereference uses the base place (MV2); the
                        // element or pointee itself is not a tracked place.
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
        self.place_subexpr(e);
        let Some((root, path)) = self.place_of(e) else { return };
        // The expression's own type is the place's type: an indexed element, a dereferenced pointee, or a field.
        let ty = e.ty.clone();
        if mode == Mode::Move && !self.is_copy(&ty) {
            // Only a local/field place or a whole `*h` can be moved: indexing and `Heap` dereferences are not
            // places, so a field behind them has no drop flag.
            if !is_direct_place(e) && !is_whole_deref(e) {
                if is_view_place(e) {
                    self.error(Code::MOVE_FROM_BORROW, e.span, "cannot move out of borrowed content");
                } else {
                    self.error(
                        Code::UNSUPPORTED,
                        e.span,
                        "moving a non-Copy value through indexing or a `Heap` dereference is not implemented in this compiler yet",
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
            K::Field(base, _) | K::Deref(base) | K::DerefRef(base) => self.place_subexpr(base),
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
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Range(..) | Pat::None => {}
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

/// A place made only of a local and field selections: the analysis can track it as a path in the local's type.
/// Indexing and `Heap` dereferences are not places, so a field selection beyond one does not extend the path.
fn is_direct_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) => true,
        tir::ExprKind::Field(base, _) => is_direct_place(base),
        _ => false,
    }
}

/// Whether `e` is the whole-pointee move of MV6: `*h` on a direct place.
fn is_whole_deref(e: &tir::Expr) -> bool {
    matches!(&e.kind, tir::ExprKind::Deref(base) if is_direct_place(base))
}

/// Whether a place expression goes through a reference or a slice, so its storage belongs to someone else.
fn is_view_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::DerefRef(_) => true,
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
        Pat::Product(subs) | Pat::Variant(_, subs) => subs.iter().any(pattern_moves),
        _ => false,
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
