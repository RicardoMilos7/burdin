//! The lints that need types: `eager_chain`, `redundant_clone`, `large_copy_in_loop`, `unused_mut`,
//! `loop_invariant_alloc`, `needless_to_list`, and `borrow_could_own` (`spec/lint.md`, LN20-LN26).
//!
//! They read the typed tree, one function at a time. A walk numbers every read of a local in evaluation order, so
//! "this is the last use" is a comparison of numbers, and it notes the numbers a loop spans, so a local that is read
//! again on the next iteration is never the last use. Branches are numbered one after the other, which only makes
//! "last use" harder to prove: a lint stays quiet when it cannot tell.
//!
//! A generic function is checked once against its bounds and once for each instantiation, and every copy has the same
//! spans. The findings are therefore collected by span and reported only when every copy agrees.

use crate::walk::{FnOrigin, Visit, for_each_fn, walk_pattern};
use crate::{Ctx, Finding, Lint};
use burdin_diag::{FileId, Span};
use burdin_syntax::ast::{self, Pattern, PatternKind};
use burdin_tir::place::is_place_expr;
use burdin_tir::{
    Builtin, CaptureMode, Expr, ExprKind, FuncId, Function, LocalId, LocalKind, Method, Pat, Program, Stmt, Ty,
    TypeDefs,
};
use std::collections::{HashMap, HashSet};

/// A `Copy` value at least this many bytes wide counts as large (`large_copy_in_loop`).
const LARGE_COPY_BYTES: u64 = 64;

/// A chain of at least this many eager `List` calls reports (`eager_chain`).
const EAGER_CHAIN_MIN: usize = 3;

type Key = (FileId, u32, u32);

fn key(span: Span) -> Key {
    (span.file, span.lo, span.hi)
}

/// What the passes over every copy of a function agree on, by span.
#[derive(Default)]
struct Shared {
    /// `unused_mut`: a `mut` binding by its name's span, and whether any copy mutates it.
    mut_bindings: HashMap<Key, MutState>,
    /// `redundant_clone`: a `.clone()` by span, and in how many copies it is redundant out of how many it appears in.
    clones: HashMap<Key, CloneState>,
    /// `borrow_could_own`: a parameter by its name's span.
    params: HashMap<Key, ParamState>,
    /// `borrow_could_own`: a function (by its declaration span) and the callers' verdict on each parameter index.
    calls: HashMap<Key, Vec<CallState>>,
    /// Findings that are decided in one copy and need no agreement.
    plain: Vec<Finding>,
}

struct MutState {
    name: String,
    mutated: bool,
}

struct CloneState {
    seen: usize,
    redundant: usize,
    call: Span,
    receiver: Span,
    local: String,
}

struct ParamState {
    name: String,
    function: String,
    decl: Key,
    index: usize,
    cloned: bool,
}

#[derive(Clone, Default)]
struct CallState {
    sites: usize,
    consumed: usize,
}

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    let program = cx.input.program;
    let defs = TypeDefs { structs: &program.structs, enums: &program.enums };
    let ast_info = AstInfo::collect(cx);
    // Whether each closure's body writes each of its captures. The capture mode in the typed tree is not enough (a
    // write through `IndexMut` leaves it a shared borrow), so the body is read; a closure inside a closure feeds the
    // outer one, so the pass repeats until nothing changes.
    let mut mutates: Vec<Vec<bool>> = program.closures.iter().map(|c| vec![false; c.captures.len()]).collect();
    loop {
        let mut changed = false;
        for (i, c) in program.closures.iter().enumerate() {
            let written = analyze(cx, program, &defs, &c.func, &mutates, &mut Shared::default());
            for (k, capture) in c.captures.iter().enumerate() {
                if written.contains(&capture.local) && !mutates[i][k] {
                    mutates[i][k] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut shared = Shared::default();
    for f in &program.functions {
        analyze(cx, program, &defs, f, &mutates, &mut shared);
    }
    for c in &program.closures {
        analyze(cx, program, &defs, &c.func, &mutates, &mut shared);
    }
    report(cx, &ast_info, shared, out);
}

// ----- facts from the syntax tree -----------------------------------------------------------------------------------

/// What the typed tree cannot say: where the `mut` keyword is, and how each function was declared.
struct AstInfo {
    /// The span of a whole `mut name` pattern, by the span of its name.
    mut_patterns: HashMap<Key, Span>,
    /// How each function was declared, by its declaration span and by its name's span.
    fns: HashMap<Key, (FnOrigin, bool)>,
}

impl AstInfo {
    fn collect(cx: &Ctx) -> AstInfo {
        let mut info = AstInfo { mut_patterns: HashMap::new(), fns: HashMap::new() };
        struct Patterns<'a>(&'a mut HashMap<Key, Span>);
        impl Visit for Patterns<'_> {
            fn pattern(&mut self, p: &Pattern) {
                if let PatternKind::Binding { name, mutable: true } = &p.kind {
                    self.0.insert(key(name.span), p.span);
                }
                walk_pattern(self, p);
            }
        }
        for source in &cx.input.files {
            for_each_fn(source.ast, &|span| cx.is_generated(span), &mut |origin, f: &ast::FnDecl| {
                info.fns.insert(key(f.span), (origin, f.is_priv));
                info.fns.insert(key(f.name.span), (origin, f.is_priv));
                Patterns(&mut info.mut_patterns).block(&f.body);
            });
        }
        info
    }
}

// ----- the walk over one function -----------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum ArgUse {
    /// A value computed for the call: moving it costs nothing.
    Temporary,
    /// A read of a local, numbered.
    Local(LocalId, u32),
    Other,
}

struct CallSite {
    callee: Key,
    index: usize,
    arg: ArgUse,
}

struct CloneSite {
    call: Span,
    receiver: Span,
    local: LocalId,
    seq: u32,
}

struct An<'a> {
    cx: &'a Ctx<'a>,
    program: &'a Program,
    defs: &'a TypeDefs<'a>,
    f: &'a Function,
    /// Per closure, whether its body writes each capture.
    closure_writes: &'a [Vec<bool>],
    seq: u32,
    uses: HashMap<LocalId, Vec<u32>>,
    decl: HashMap<LocalId, u32>,
    loops: Vec<(u32, u32)>,
    loop_depth: u32,
    mutated: HashSet<LocalId>,
    clones: Vec<CloneSite>,
    calls: Vec<CallSite>,
    /// Findings of this function that need no agreement with other copies.
    plain: Vec<Finding>,
}

/// Walks one function body, adds what it found to `shared`, and returns the locals the body writes.
fn analyze(
    cx: &Ctx,
    program: &Program,
    defs: &TypeDefs,
    f: &Function,
    closure_writes: &[Vec<bool>],
    shared: &mut Shared,
) -> HashSet<LocalId> {
    if !cx.in_package(f.span) {
        return HashSet::new();
    }
    let mut an = An {
        cx,
        program,
        defs,
        f,
        closure_writes,
        seq: 0,
        uses: HashMap::new(),
        decl: HashMap::new(),
        loops: Vec::new(),
        loop_depth: 0,
        mutated: HashSet::new(),
        clones: Vec::new(),
        calls: Vec::new(),
        plain: Vec::new(),
    };
    for p in &f.params {
        an.decl.insert(*p, 0);
    }
    an.block(&f.body);
    an.finish(shared)
}

impl An<'_> {
    fn finish(self, shared: &mut Shared) -> HashSet<LocalId> {
        let f = self.f;
        shared.plain.extend(self.plain.iter().cloned());
        // Which locals are `mut` bindings of the user's.
        for (i, local) in f.locals.iter().enumerate() {
            let id = LocalId(i);
            if !local.mutable || local.span.lo == local.span.hi || f.params.contains(&id) || local.name.starts_with('_')
            {
                continue;
            }
            // A local the body never binds was declared inside a `comptime` block, which is already evaluated.
            if !self.cx.in_package(local.span) || !self.decl.contains_key(&id) {
                continue;
            }
            let state = shared
                .mut_bindings
                .entry(key(local.span))
                .or_insert(MutState { name: local.name.clone(), mutated: false });
            state.mutated |= self.mutated.contains(&id);
        }
        for site in &self.clones {
            let local = &f.locals[site.local.0];
            let redundant = self.clone_is_redundant(site);
            let state = shared.clones.entry(key(site.call)).or_insert(CloneState {
                seen: 0,
                redundant: 0,
                call: site.call,
                receiver: site.receiver,
                local: local.name.clone(),
            });
            state.seen += 1;
            state.redundant += usize::from(redundant);
        }
        // Parameters that are cloned, for `borrow_could_own`.
        let cloned: HashSet<LocalId> = self.clones.iter().map(|s| s.local).collect();
        for (index, p) in f.params.iter().enumerate() {
            let local = &f.locals[p.0];
            if local.span.lo == local.span.hi || !self.cx.in_package(local.span) {
                continue;
            }
            let entry = shared.params.entry(key(local.span)).or_insert(ParamState {
                name: local.name.clone(),
                function: f.name.clone(),
                decl: key(f.span),
                index,
                cloned: true,
            });
            entry.cloned &= cloned.contains(p) && local.kind == LocalKind::BorrowedParam;
        }
        for site in &self.calls {
            let consumed = match site.arg {
                ArgUse::Temporary => true,
                ArgUse::Local(id, seq) => self.is_last_use(id, seq),
                ArgUse::Other => false,
            };
            let states = shared.calls.entry(site.callee).or_default();
            if states.len() <= site.index {
                states.resize(site.index + 1, CallState::default());
            }
            states[site.index].sites += 1;
            states[site.index].consumed += usize::from(consumed);
        }
        self.mutated
    }

    /// Whether the read of `id` numbered `seq` is the last one: nothing reads it later, and no loop that started after
    /// it was declared brings the read around again.
    fn is_last_use(&self, id: LocalId, seq: u32) -> bool {
        let local = &self.f.locals[id.0];
        if !matches!(local.kind, LocalKind::Owned | LocalKind::OwnParam) {
            return false;
        }
        if self.uses.get(&id).is_some_and(|u| u.iter().any(|&s| s > seq)) {
            return false;
        }
        let declared = self.decl.get(&id).copied().unwrap_or(0);
        !self.loops.iter().any(|&(a, b)| a <= seq && seq <= b && declared <= a)
    }

    fn clone_is_redundant(&self, site: &CloneSite) -> bool {
        let local = &self.f.locals[site.local.0];
        let ty = &local.ty;
        self.is_last_use(site.local, site.seq)
            && !self.defs.is_copy(ty)
            && self.defs.is_clone(ty)
            && !ty.is_second_class()
            && !has_user_drop(ty, self.defs, 0)
    }

    fn bump(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }

    fn use_local(&mut self, id: LocalId) -> u32 {
        let seq = self.bump();
        self.uses.entry(id).or_default().push(seq);
        seq
    }

    fn bind(&mut self, p: &Pat) {
        match p {
            Pat::Binding(id, _) => {
                self.decl.insert(*id, self.seq);
            }
            Pat::At(id, _, inner) => {
                self.decl.insert(*id, self.seq);
                self.bind(inner);
            }
            Pat::Or(ps) | Pat::Product(ps) | Pat::Variant(_, ps) => ps.iter().for_each(|p| self.bind(p)),
            Pat::Some(p) | Pat::Ok(p) | Pat::Err(p) => self.bind(p),
            _ => {}
        }
    }

    fn mark_mut(&mut self, place: &Expr) {
        if let Some(id) = root_of(place) {
            self.mutated.insert(id);
        }
    }

    fn block(&mut self, b: &burdin_tir::Block) {
        for s in &b.stmts {
            match s {
                Stmt::Let(pat, init, other) => {
                    self.expr(init, false);
                    self.check_loop_invariant(pat, init);
                    self.bump();
                    self.bind(pat);
                    if let Some(other) = other {
                        self.block(other);
                    }
                }
                Stmt::Expr(e) => self.expr(e, false),
            }
        }
        if let Some(t) = &b.tail {
            self.expr(t, false);
        }
    }

    fn each(&mut self, es: &[Expr]) {
        for e in es {
            self.expr(e, false);
        }
    }

    /// A loop: the iterable (evaluated once, outside), then the body, which runs again and again.
    fn looped(&mut self, before: &[&Expr], bind: &[&Pat], var: Option<LocalId>, body: &burdin_tir::Block) {
        for e in before {
            self.expr(e, false);
        }
        let start = self.bump();
        for p in bind {
            self.bind(p);
        }
        if let Some(v) = var {
            self.decl.insert(v, self.seq);
        }
        self.loop_depth += 1;
        self.block(body);
        self.loop_depth -= 1;
        self.loops.push((start, self.seq));
    }

    /// `receiver_of_eager`: this expression is the list a later eager call works on, so the chain is reported from
    /// the outermost call only.
    fn expr(&mut self, e: &Expr, receiver_of_eager: bool) {
        use ExprKind as K;
        match &e.kind {
            K::Unit
            | K::Bool(_)
            | K::Int(_)
            | K::Float(_)
            | K::Char(_)
            | K::Str(_)
            | K::Const(_)
            | K::None
            | K::Continue(_)
            | K::Return(None)
            | K::LayoutOf { .. } => {}
            K::Break { value, .. } => {
                if let Some(value) = value {
                    self.expr(value, false);
                }
            }
            K::Local(id) => {
                self.use_local(*id);
            }
            K::Call(fid, args) => {
                self.call(e, *fid, args, receiver_of_eager);
            }
            K::ExternCall(_, args) => self.each(args),
            K::Builtin(_, args) => {
                self.each(args);
                // What a built-in does to its arguments is not tracked, so none of them counts as unmutated.
                for a in args {
                    self.mark_mut(a);
                }
            }
            K::Method(m, args) => {
                self.each(args);
                if m.mutates_receiver()
                    && let Some(recv) = args.first()
                {
                    self.mark_mut(recv);
                }
                if *m == Method::Clone
                    && let [recv] = args.as_slice()
                    && let K::Local(id) = &recv.kind
                    && recv.ty == e.ty
                {
                    let seq = self.seq;
                    self.clones.push(CloneSite { call: e.span, receiver: recv.span, local: *id, seq });
                }
                if matches!(m, Method::Iter | Method::IntoIter)
                    && let Some(recv) = args.first()
                {
                    self.check_to_list(recv, "iterated right away");
                }
            }
            K::Closure { closure, captures } => {
                let mutating = self.program.closures[*closure].kind != burdin_tir::FnKind::Fn;
                for (k, c) in captures.iter().enumerate() {
                    self.use_local(c.local);
                    if mutating || self.closure_writes[*closure].get(k).copied().unwrap_or(false) {
                        self.mutated.insert(c.local);
                    }
                    // A closure that owns the capture (`Move`) may assign to its own copy, and the checker wants the
                    // original declared `mut` for that, so only a shared borrow proves the binding is not mutated.
                    if c.mode != CaptureMode::Borrow {
                        self.mutated.insert(c.local);
                    }
                }
            }
            K::ClosureCall { callee, args, kind } => {
                self.expr(callee, false);
                self.each(args);
                if *kind != burdin_tir::FnKind::Fn {
                    self.mark_mut(callee);
                }
            }
            K::AnyCall { recv, args, .. } => {
                self.expr(recv, false);
                self.each(args);
                self.mark_mut(recv);
            }
            K::BoxAny { value: inner, .. }
            | K::AnyIs { value: inner, .. }
            | K::AnyDowncast { value: inner, .. }
            | K::StringAsStr(inner)
            | K::Unary(_, inner)
            | K::Cast(inner)
            | K::Deref(inner)
            | K::PtrDeref(inner)
            | K::DerefRef(inner)
            | K::Field(inner, _)
            | K::Some(inner)
            | K::Ok(inner)
            | K::Err(inner)
            | K::Return(Some(inner))
            | K::ArrayRepeat(inner, _)
            | K::Comptime(inner) => self.expr(inner, false),
            K::Ref { place, mutable } => {
                self.expr(place, false);
                if *mutable {
                    self.mark_mut(place);
                }
            }
            K::Assign(target, value) => {
                self.expr(value, false);
                self.expr(target, false);
                self.mark_mut(target);
            }
            K::Binary(_, a, b) | K::Index(a, b) => {
                self.expr(a, false);
                self.expr(b, false);
            }
            K::Range { lo, hi, .. } => {
                self.expr(lo, false);
                self.expr(hi, false);
            }
            K::Slice { base, lo, hi, .. } => {
                self.expr(base, false);
                // The view may be written through (`xs.swap`, `xs[0] = ..`), which needs the base declared `mut`.
                self.mark_mut(base);
                self.expr(lo, false);
                self.expr(hi, false);
            }
            K::Array(es) | K::Tuple(es) => self.each(es),
            K::Struct(fields) | K::Variant(_, fields) => fields.iter().for_each(|(_, v)| self.expr(v, false)),
            K::Block(b) => self.block(b),
            K::If(c, then, other) => {
                self.expr(c, false);
                self.block(then);
                if let Some(o) = other {
                    self.expr(o, false);
                }
            }
            K::Labeled(_, inner) => self.expr(inner, receiver_of_eager),
            K::While(c, body) => {
                // The condition is evaluated on every iteration, so it belongs to the loop.
                let start = self.bump();
                self.loop_depth += 1;
                self.expr(c, false);
                self.block(body);
                self.loop_depth -= 1;
                self.loops.push((start, self.seq));
            }
            K::Loop(body) => {
                let start = self.bump();
                self.loop_depth += 1;
                self.block(body);
                self.loop_depth -= 1;
                self.loops.push((start, self.seq));
            }
            K::ForRange { var, lo, hi, body, .. } => self.looped(&[&**lo, &**hi], &[], *var, body),
            K::ForRangeValue { var, range, body } => self.looped(&[&**range], &[], *var, body),
            K::ForArray { pat, array: iter, body }
            | K::ForSlice { pat, slice: iter, body }
            | K::ForMap { pat, map: iter, body }
            | K::ForChars { pat, string: iter, body }
            | K::ComptimeFor { pat, iter, body } => self.looped(&[&**iter], &[pat], None, body),
            K::ForList { pat, list, body } => {
                self.check_to_list(list, "iterated right away by this loop");
                self.looped(&[&**list], &[pat], None, body);
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee, false);
                for arm in arms {
                    self.bump();
                    self.bind(&arm.pat);
                    if let Some(g) = &arm.guard {
                        self.expr(g, false);
                    }
                    self.expr(&arm.body, false);
                }
            }
        }
    }

    fn call(&mut self, e: &Expr, fid: FuncId, args: &[Expr], receiver_of_eager: bool) {
        let callee = &self.program.functions[fid.0];
        let sig = &self.program.signatures[fid.0];
        // `eager_chain`: report once, from the outermost call of a chain.
        let eager = eager_name(&callee.name).is_some();
        if eager && !receiver_of_eager {
            let len = chain_len(self.program, e);
            if len >= EAGER_CHAIN_MIN && self.cx.in_package(e.span) {
                self.plain.push(
                    Finding::new(
                        Lint::EagerChain,
                        e.span,
                        format!("a chain of {len} eager list methods builds a new list at every step"),
                    )
                    .note("start the chain with `.iter()` so it runs lazily without the intermediate lists")
                    .note("the closures of the lazy form take the items themselves, where the eager `List` methods pass views"),
                );
            }
        }
        for (i, a) in args.iter().enumerate() {
            // Only the first argument of an eager call is the chain's list.
            self.expr(a, eager && i == 0);
        }
        for (i, a) in args.iter().enumerate() {
            let Some(p) = sig.params.get(i) else { continue };
            if p.exclusive && !p.own {
                self.mark_mut(a);
            }
            if self.loop_depth > 0 && p.own && self.defs.is_copy(&a.ty) && self.cx.in_package(a.span) {
                let bytes = size_of(&a.ty, self.defs, 0).0;
                if bytes >= LARGE_COPY_BYTES {
                    self.plain.push(
                        Finding::new(
                            Lint::LargeCopyInLoop,
                            a.span,
                            format!(
                                "`{}` is {bytes} bytes and is copied into `{}` on every iteration",
                                a.ty,
                                short_name(&callee.name)
                            ),
                        )
                        .note("take the parameter by borrow, or hoist the call out of the loop"),
                    );
                }
            }
            // `borrow_could_own`: how this caller treats the argument. A receiver (`self`) is not a candidate.
            if !(sig.has_self && i == 0) && !p.own && !p.exclusive {
                let arg = match &a.kind {
                    ExprKind::Local(id) => ArgUse::Local(*id, self.last_seq_of(*id)),
                    _ if !is_place_expr(a) => ArgUse::Temporary,
                    _ => ArgUse::Other,
                };
                self.calls.push(CallSite { callee: key(callee.span), index: i, arg });
            }
        }
        self.bump();
    }

    /// The number of the most recent read of `id`: the one just made for the argument.
    fn last_seq_of(&self, id: LocalId) -> u32 {
        self.uses.get(&id).and_then(|u| u.last()).copied().unwrap_or(0)
    }

    fn check_to_list(&mut self, e: &Expr, why: &str) {
        let Some(source) = to_list_source(self.program, e) else { return };
        if !self.cx.in_package(e.span) {
            return;
        }
        let _ = source;
        self.plain.push(
            Finding::new(Lint::NeedlessToList, e.span, format!("`to_list()` builds a list that is {why}"))
                .note("iterate the source directly: the list is never needed, only its items"),
        );
    }

    /// `loop_invariant_alloc`: an immutable local initialized inside a loop with an allocation that never changes.
    fn check_loop_invariant(&mut self, pat: &Pat, init: &Expr) {
        if self.loop_depth == 0 {
            return;
        }
        let Pat::Binding(id, _) = pat else { return };
        let local = &self.f.locals[id.0];
        if local.mutable || local.name.starts_with('_') || !self.cx.in_package(init.span) {
            return;
        }
        if !matches!(init.ty, Ty::String | Ty::List(..) | Ty::Map(..) | Ty::Heap(..) | Ty::CString) {
            return;
        }
        let mut allocates = false;
        if invariant(init, &mut allocates) && allocates {
            self.plain.push(
                Finding::new(
                    Lint::LoopInvariantAlloc,
                    init.span,
                    format!("`{}` is allocated again on every iteration but never changes", local.name),
                )
                .note("build it once before the loop"),
            );
        }
    }
}

/// Whether an expression computes the same value every time (literals and constants only) and whether it allocates.
fn invariant(e: &Expr, allocates: &mut bool) -> bool {
    use ExprKind as K;
    match &e.kind {
        K::Unit | K::Bool(_) | K::Int(_) | K::Float(_) | K::Char(_) | K::Str(_) | K::Const(_) | K::None => true,
        K::Array(es) | K::Tuple(es) => es.iter().all(|x| invariant(x, allocates)),
        K::Struct(fs) | K::Variant(_, fs) => fs.iter().all(|(_, x)| invariant(x, allocates)),
        K::Some(x) | K::Ok(x) | K::Err(x) | K::Cast(x) | K::Unary(_, x) | K::StringAsStr(x) => invariant(x, allocates),
        K::Binary(_, a, b) => invariant(a, allocates) && invariant(b, allocates),
        K::Method(Method::ToString, args) => {
            *allocates = true;
            args.iter().all(|x| invariant(x, allocates))
        }
        K::Builtin(b, args) => {
            let kind = match b {
                Builtin::StringNew | Builtin::ListNew | Builtin::MapNew => Some(false),
                Builtin::StringFrom
                | Builtin::HeapNew
                | Builtin::ListWithCapacity
                | Builtin::MapWithCapacity
                | Builtin::CStringFrom => Some(true),
                _ => None,
            };
            match kind {
                Some(allocating) => {
                    *allocates |= allocating;
                    args.iter().all(|x| invariant(x, allocates))
                }
                None => false,
            }
        }
        _ => false,
    }
}

/// The local at the root of a place, through fields, indexing, dereferences, and slicing.
fn root_of(e: &Expr) -> Option<LocalId> {
    use ExprKind as K;
    match &e.kind {
        K::Local(id) => Some(*id),
        K::Index(b, _)
        | K::Field(b, _)
        | K::Deref(b)
        | K::PtrDeref(b)
        | K::DerefRef(b)
        | K::StringAsStr(b)
        | K::Ref { place: b, .. } => root_of(b),
        K::Slice { base, .. } => root_of(base),
        _ => None,
    }
}

/// `__list_map<...>` is the eager `map`; the name of the method is what follows `__list_`.
fn eager_name(name: &str) -> Option<&str> {
    // The name is `std.iter.__list_map<i32, ...>`: drop the type arguments, then the module path.
    let path = name.split('<').next().unwrap_or(name);
    let last = path.rsplit('.').next().unwrap_or(path);
    let method = last.strip_prefix("__list_")?;
    ["map", "filter", "fold", "any", "all", "find"].into_iter().find(|m| *m == method)
}

/// How many eager calls are chained: a call whose list argument is itself an eager call.
fn chain_len(program: &Program, e: &Expr) -> usize {
    match &e.kind {
        ExprKind::Call(fid, args) if eager_name(&program.functions[fid.0].name).is_some() => {
            1 + args.first().map_or(0, |a| chain_len(program, a))
        }
        _ => 0,
    }
}

/// The receiver of `x.to_list()` when `e` is one.
fn to_list_source<'e>(program: &Program, e: &'e Expr) -> Option<&'e Expr> {
    match &e.kind {
        ExprKind::Call(fid, args) if program.functions[fid.0].name.ends_with(".to_list") => args.first(),
        ExprKind::Method(Method::SliceToList, args) => args.first(),
        _ => None,
    }
}

fn short_name(name: &str) -> &str {
    name.split('<').next().unwrap_or(name)
}

/// Whether dropping a value of the type runs a user's `Drop`, which cloning and then dropping both copies would show.
fn has_user_drop(ty: &Ty, defs: &TypeDefs, depth: u32) -> bool {
    if depth > 6 {
        return true;
    }
    let d = depth + 1;
    match ty {
        Ty::Struct(a) => {
            let s = &defs.structs[a.id];
            s.drop_method.is_some() || s.fields.iter().any(|f| has_user_drop(&f.ty, defs, d))
        }
        Ty::Enum(a) => {
            let e = &defs.enums[a.id];
            e.drop_method.is_some() || e.variants.iter().any(|v| v.fields.iter().any(|f| has_user_drop(&f.ty, defs, d)))
        }
        Ty::Option(t) | Ty::List(t, _) | Ty::Heap(t, _) | Ty::Shared(t) | Ty::Array(t, _) => has_user_drop(t, defs, d),
        Ty::Result(a, b) | Ty::Map(a, b, _) => has_user_drop(a, defs, d) || has_user_drop(b, defs, d),
        Ty::Tuple(ts) => ts.iter().any(|t| has_user_drop(t, defs, d)),
        Ty::Any(_) | Ty::Fn { .. } => true,
        _ => false,
    }
}

/// The size and alignment of a type in bytes, as the C layout the compiler uses would place it. Types without a fixed
/// layout of their own count as a pointer.
fn size_of(ty: &Ty, defs: &TypeDefs, depth: u32) -> (u64, u64) {
    if depth > 8 {
        return (8, 8);
    }
    let d = depth + 1;
    let record = |fields: &mut dyn Iterator<Item = &Ty>| -> (u64, u64) {
        let (mut size, mut align) = (0u64, 1u64);
        for t in fields {
            let (s, a) = size_of(t, defs, d);
            size = size.next_multiple_of(a) + s;
            align = align.max(a);
        }
        (size.next_multiple_of(align), align)
    };
    match ty {
        Ty::Unit | Ty::Never => (0, 1),
        Ty::Bool => (1, 1),
        Ty::Char => (4, 4),
        Ty::Int(i) => {
            let b = u64::from(i.bits() / 8);
            (b, b)
        }
        Ty::Float(burdin_tir::FloatTy::F32) => (4, 4),
        Ty::Float(_) => (8, 8),
        Ty::Str | Ty::Slice(_) => (16, 8),
        Ty::Array(t, n) => {
            let (s, a) = size_of(t, defs, d);
            (s.saturating_mul(*n), a)
        }
        Ty::Tuple(ts) => record(&mut ts.iter()),
        Ty::Option(t) => {
            let (s, a) = size_of(t, defs, d);
            ((s + a).next_multiple_of(a), a)
        }
        Ty::Result(t, e) => {
            let ((s1, a1), (s2, a2)) = (size_of(t, defs, d), size_of(e, defs, d));
            let a = a1.max(a2).max(1);
            ((s1.max(s2) + a).next_multiple_of(a), a)
        }
        Ty::Struct(a) => record(&mut defs.structs[a.id].fields.iter().map(|f| &f.ty)),
        Ty::Enum(a) => {
            let e = &defs.enums[a.id];
            let (mut size, mut align) = (0u64, 4u64);
            for v in &e.variants {
                let (s, a) = record(&mut v.fields.iter().map(|f| &f.ty));
                size = size.max(s);
                align = align.max(a);
            }
            ((size + align).next_multiple_of(align), align)
        }
        Ty::Range(t, _) => {
            let (s, a) = size_of(t, defs, d);
            ((2 * s).next_multiple_of(a), a)
        }
        _ => (8, 8),
    }
}

// ----- the verdicts -----------------------------------------------------------------------------------------------

fn report(cx: &Ctx, info: &AstInfo, shared: Shared, out: &mut Vec<Finding>) {
    out.extend(shared.plain);
    let text = |span: Span| cx.slice(span).to_string();
    for (k, state) in &shared.mut_bindings {
        if state.mutated {
            continue;
        }
        let name_span = Span::new(k.0, k.1, k.2);
        let mut finding = Finding::new(
            Lint::UnusedMut,
            name_span,
            format!("variable `{}` is declared `mut` but never mutated", state.name),
        );
        if let Some(pattern) = info.mut_patterns.get(k) {
            let keyword = Span::new(pattern.file, pattern.lo, name_span.lo);
            if text(keyword).trim() == "mut" {
                finding = finding.fix(keyword, "remove the unneeded `mut`", "");
            }
        }
        out.push(finding);
    }
    for state in shared.clones.values() {
        if state.seen == 0 || state.redundant != state.seen {
            continue;
        }
        out.push(
            Finding::new(
                Lint::RedundantClone,
                state.call,
                format!("`{}` is not used after this `.clone()`; move it instead", state.local),
            )
            .fix(state.call, "move the value instead of cloning it", text(state.receiver)),
        );
    }
    for (k, state) in &shared.params {
        let Some((origin, is_priv)) = info.fns.get(&state.decl) else { continue };
        if !is_priv || !matches!(origin, FnOrigin::Free | FnOrigin::Method) || !state.cloned {
            continue;
        }
        let Some(verdicts) = shared.calls.get(&state.decl) else { continue };
        let Some(v) = verdicts.get(state.index) else { continue };
        if v.sites == 0 || v.consumed != v.sites {
            continue;
        }
        out.push(
            Finding::new(
                Lint::BorrowCouldOwn,
                Span::new(k.0, k.1, k.2),
                format!(
                    "parameter `{}` of `{}` is cloned inside, but no caller uses its argument afterwards",
                    state.name,
                    short_name(&state.function)
                ),
            )
            .note(format!("take it with `own {}` and use the value directly instead of cloning it", state.name)),
        );
    }
}
