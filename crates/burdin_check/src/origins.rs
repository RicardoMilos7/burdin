//! Origin inference for returned views and for views stored in locals (memory-rules.md, RT).
//!
//! A function whose return type is second-class (`&T`, `&mut T`, `str`, `[T]`, or an aggregate containing one)
//! must return a view of exactly one of its parameters, or a static view. The origin set of an expression is
//! inferred by data flow through the body; a call with a second-class result maps the callee's signature origin
//! back to the argument. The check stays local: a body is analyzed against signatures only.
//!
//! The same walk enforces RT5: a second-class value stored in a local may only derive from static data, from a
//! parameter, or from a local of the same or an enclosing block. A temporary, or a local of an inner block, dies
//! too early, which is E0123.
//!
//! The signature summaries are computed first, to a fixpoint, because a call needs its callee's origin. The
//! errors are reported on a final pass over the stable summaries.

use burdin_diag::{Code, Diagnostics, Span};
use burdin_tir::{self as tir, BindMode, LocalId, LocalKind, Pat, Signature};
use std::collections::BTreeSet;

/// Where a value may borrow from, and whether it borrows from storage that does not live long enough for a
/// binding or a return: a temporary, or a local of this function.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Origin {
    /// The parameters whose storage the value may borrow from.
    params: BTreeSet<usize>,
    /// The locals whose storage the value may borrow from (an owned parameter included).
    locals: BTreeSet<usize>,
    /// The value borrows from a temporary, which dies at the end of its statement.
    temp: bool,
}

impl Origin {
    fn param(index: usize) -> Origin {
        Origin { params: BTreeSet::from([index]), ..Origin::default() }
    }

    /// A value that lives in the storage of a local, an owned parameter, or a temporary.
    fn local(id: LocalId) -> Origin {
        Origin { locals: BTreeSet::from([id.0]), ..Origin::default() }
    }

    /// A value computed by this statement, whose storage dies at its end.
    fn temp() -> Origin {
        Origin { temp: true, ..Origin::default() }
    }

    fn join(&mut self, other: &Origin) {
        self.params.extend(other.params.iter().copied());
        self.locals.extend(other.locals.iter().copied());
        self.temp |= other.temp;
    }

    /// The parameters a second-class result derives from, if the signature can say it: none for static data
    /// or for a value that borrows from the function's own storage (an error reported elsewhere).
    fn summary(&self) -> Vec<usize> {
        if self.temp || !self.locals.is_empty() {
            return Vec::new();
        }
        self.params.iter().copied().collect()
    }

    /// Whether returning this value would return storage of the function itself (RT2).
    fn escapes(&self) -> bool {
        self.temp || !self.locals.is_empty()
    }
}

/// Computes every signature's return origin and reports E0121, E0122, and E0123.
pub(crate) fn check(
    sigs: &mut [Signature],
    functions: &mut [tir::Function],
    enums: &[tir::EnumDef],
    diags: &mut Diagnostics,
) {
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
            if sigs[i].ret_origins != summary {
                sigs[i].ret_origins = summary;
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
        functions[i].ret_origins = sigs[i].ret_origins.clone();
        // A result that is itself a view derives from exactly one parameter (RT3); a struct, enum, or tuple of
        // views may derive from several, and its holder keeps every one of them borrowed (RT3a).
        // `Option` and `Result` are enums of `std`, but a view inside one is the result itself, not an aggregate
        // of views.
        let aggregate = match &functions[i].ret {
            tir::Ty::Enum(a) => enums[a.id].lang.is_none(),
            tir::Ty::Struct(_) | tir::Ty::Tuple(_) => true,
            _ => false,
        };
        if functions[i].ret.is_second_class() && !aggregate && origin.params.len() > 1 {
            let f = &functions[i];
            let names: Vec<String> = origin.params.iter().map(|p| f.locals[f.params[*p].0].name.clone()).collect();
            let error = diags.error(
                Code::RETURN_MULTIPLE_ORIGINS,
                functions[i].span,
                format!(
                    "this returns views of more than one parameter (`{}`); a second-class result must derive from \
                     exactly one parameter",
                    names.join("`, `")
                ),
            );
            for p in &origin.params {
                let local = &f.locals[f.params[*p].0];
                if local.span.lo < local.span.hi {
                    error.note(local.span, format!("`{}` is one of the sources of this result", local.name));
                }
            }
            error.help(crate::moves::TRACE_HELP);
        }
    }
}

/// Runs the RT5 check (a view stored in a binding) on a closure body. A closure's return type is never
/// second-class (sema rejects it), so the return-origin part reports nothing.
pub(crate) fn check_closure(func: &tir::Function, sigs: &[Signature], diags: &mut Diagnostics) {
    // A closure's return type is never second-class: sema already reported E0171, so the return-origin rules
    // would only add a second diagnostic for the same mistake.
    if func.symbolic || func.ret.is_second_class() {
        return;
    }
    let mut analyzer = Analyzer::new(sigs, func, Some(diags));
    analyzer.body();
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
    /// The block tree: `block_parent[id]` is the block that encloses block `id`, or `None` for the function body.
    block_parent: Vec<Option<usize>>,
    /// The block whose statements are being analyzed.
    current_block: usize,
    /// The block each local is declared in. Borrowed parameters have none (they live for the whole call).
    local_block: Vec<Option<usize>>,
    /// The enclosing loops, innermost last: the label a jump may name and the joined origin of their `break`
    /// values (only a `loop` gives one back).
    loops: Vec<LoopOrigin>,
    /// The label of the loop the next loop node is about to be checked with (`'a: loop ...`).
    pending_label: Option<String>,
}

/// One open loop for the origin of its `break` values.
struct LoopOrigin {
    label: Option<String>,
    origin: Origin,
}

impl<'a> Analyzer<'a> {
    fn new(sigs: &'a [Signature], f: &'a tir::Function, report: Option<&'a mut Diagnostics>) -> Self {
        let mut local_block = vec![None; f.locals.len()];
        for (i, local) in f.locals.iter().enumerate() {
            // An owned parameter lives until the function returns; a view of it kept in a local is fine, but
            // returning one is E0122 through the origin. A parameter whose type is itself a view is a
            // view-typed parameter (RT2): its value is a copy of the caller's view, so returning it is fine.
            if local.kind == LocalKind::OwnParam && !local.ty.is_second_class() {
                local_block[i] = Some(0);
            }
        }
        Analyzer {
            sigs,
            f,
            bindings: vec![None; f.locals.len()],
            returns: Origin::default(),
            report,
            block_parent: vec![None],
            current_block: 0,
            local_block,
            loops: Vec::new(),
            pending_label: None,
        }
    }
    /// Analyzes the body and returns the union of the origins of all returned expressions.
    fn body(&mut self) -> Origin {
        let tail = self.block_body(&self.f.body);
        let span = self.f.body.tail.as_ref().map(|e| e.span);
        if let Some(span) = span {
            self.record_return(&tail, span);
        } else if self.f.ret.is_second_class() {
            self.record_return(&tail, self.f.span);
        }
        self.returns.clone()
    }

    /// The origin of a local's storage: a parameter whose type is a view stands for its position in the
    /// signature, whether it is borrowed or passed by value. A `View` local is a binding onto the scrutinee's
    /// storage, so its storage is whatever the binding views.
    fn local_storage(&self, id: LocalId) -> Origin {
        let local = &self.f.locals[id.0];
        match local.kind {
            LocalKind::BorrowedParam => Origin::param(self.param_index(id)),
            LocalKind::OwnParam if local.ty.is_second_class() => Origin::param(self.param_index(id)),
            LocalKind::View => self.bindings[id.0].clone().unwrap_or_else(|| Origin::local(id)),
            _ => Origin::local(id),
        }
    }

    fn param_index(&self, id: LocalId) -> usize {
        self.f.params.iter().position(|p| *p == id).unwrap_or(0)
    }

    /// The origin of a local's value: a view binding stands for the storage it views.
    fn local_value(&self, id: LocalId) -> Origin {
        match &self.bindings[id.0] {
            Some(origin) => origin.clone(),
            None => self.local_storage(id),
        }
    }

    fn record_return(&mut self, origin: &Origin, span: Span) {
        self.returns.join(origin);
        if !self.f.ret.is_second_class() || !origin.escapes() {
            return;
        }
        let notes: Vec<(Span, String)> = origin
            .locals
            .iter()
            .filter_map(|id| {
                let local = self.f.locals.get(*id)?;
                (local.span.lo < local.span.hi)
                    .then(|| (local.span, format!("`{}` is dropped when the function returns", local.name)))
            })
            .collect();
        let temporary = origin.temp;
        if let Some(diags) = self.report.as_deref_mut() {
            let error = diags.error(
                Code::RETURN_LOCAL_ORIGIN,
                span,
                "this view borrows from a local or an owned parameter, which does not live past the return",
            );
            for (span, message) in notes {
                error.note(span, message);
            }
            if temporary {
                error.note_without_span(
                    "the value comes from a temporary, which is dropped at the end of its statement",
                );
            }
            error.help(crate::moves::TRACE_HELP);
        }
    }

    /// Whether `source` lives at least as long as a binding in `target_block`: it is declared in the same block,
    /// or in one that encloses it.
    fn outlives(&self, source: LocalId, target_block: usize) -> bool {
        let Some(declared) = self.local_block[source.0] else {
            // A borrowed parameter lives for the whole call, and a synthetic local with no recorded block
            // (a loop binding the analysis introduced) is treated the same way.
            return true;
        };
        let mut block = Some(target_block);
        while let Some(b) = block {
            if b == declared {
                return true;
            }
            block = self.block_parent[b];
        }
        false
    }

    /// Enforces RT5 on a value of a second-class type stored in a binding of `target_block`.
    fn check_stored_view(&mut self, origin: &Origin, target_block: usize, allow_temp: bool, span: Span) {
        let local_dies_early = origin.locals.iter().any(|id| !self.outlives(LocalId(*id), target_block));
        let temporary_dies_early = origin.temp && !allow_temp;
        if !local_dies_early && !temporary_dies_early {
            return;
        }
        let notes: Vec<(Span, String)> = origin
            .locals
            .iter()
            .filter_map(|id| {
                let local = self.f.locals.get(*id)?;
                (local.span.lo < local.span.hi && !self.outlives(LocalId(*id), target_block)).then(|| {
                    (
                        local.span,
                        format!("`{}` is declared in an inner block and dropped before the binding", local.name),
                    )
                })
            })
            .collect();
        let from_temporary = temporary_dies_early;
        if let Some(diags) = self.report.as_deref_mut() {
            let error = diags.error(
                Code::VIEW_OUTLIVES_SOURCE,
                span,
                if local_dies_early {
                    "this view borrows from a local of an inner block, which does not live as long as the binding"
                } else {
                    "this view borrows from a temporary, which is dropped at the end of this statement"
                },
            );
            for (span, message) in notes {
                error.note(span, message);
            }
            if from_temporary {
                error.note_without_span("the source is a temporary, dropped at the end of the statement that made it");
            }
            error.help(crate::moves::TRACE_HELP);
        }
    }

    fn set_binding(&mut self, id: LocalId, mode: BindMode, origin: &Origin, allow_temp: bool, span: Span) {
        self.local_block[id.0] = Some(self.current_block);
        let local = &self.f.locals[id.0];
        let ty = &local.ty;
        // The `match` scrutinee local lives only until the end of the statement that evaluates the match, so a
        // view of a temporary in it is valid (RT5's arm-binding exception).
        let allow_temp = allow_temp || local.kind == LocalKind::Scrutinee;
        // A view binding, or any value of a view type, borrows from the same storage as the scrutinee.
        if mode == BindMode::View || ty.is_second_class() {
            self.check_stored_view(origin, self.current_block, allow_temp, span);
            self.bindings[id.0] = Some(origin.clone());
        }
    }

    fn bind_pattern(&mut self, pat: &Pat, origin: &Origin, allow_temp: bool, span: Span) {
        match pat {
            Pat::Wild | Pat::Bool(_) | Pat::Int(_) | Pat::Char(_) | Pat::Str(_) | Pat::Range(..) => {}
            Pat::Binding(id, mode) => self.set_binding(*id, *mode, origin, allow_temp, span),
            Pat::At(id, mode, inner) => {
                self.set_binding(*id, *mode, origin, allow_temp, span);
                self.bind_pattern(inner, origin, allow_temp, span);
            }
            Pat::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.bind_pattern(first, origin, allow_temp, span);
                }
            }
            Pat::Product(subs) | Pat::Variant(_, subs) => {
                for sub in subs {
                    self.bind_pattern(sub, origin, allow_temp, span);
                }
            }
            Pat::Slice { prefix, rest, suffix } => {
                for sub in prefix.iter().chain(rest.as_deref()).chain(suffix) {
                    self.bind_pattern(sub, origin, allow_temp, span);
                }
            }
        }
    }

    /// Opens a loop for the origin of its `break` values, with the label the enclosing `Labeled` gave it.
    fn open_loop(&mut self) {
        self.loops.push(LoopOrigin { label: self.pending_label.take(), origin: Origin::default() });
    }

    /// Analyzes a block in a new scope.
    fn block(&mut self, b: &tir::Block) -> Origin {
        let parent = self.current_block;
        let id = self.block_parent.len();
        self.block_parent.push(Some(parent));
        self.current_block = id;
        let origin = self.block_body(b);
        self.current_block = parent;
        origin
    }

    /// Analyzes a block's statements and value in the current scope.
    fn block_body(&mut self, b: &tir::Block) -> Origin {
        for stmt in &b.stmts {
            match stmt {
                tir::Stmt::Let(pat, init, else_block) => {
                    let origin = self.expr(init);
                    if let Some(else_block) = else_block {
                        self.block(else_block);
                    }
                    // The binding outlives this statement, so a temporary source is too short-lived (RT5);
                    // the binding dies with the block, so a local of an inner block is fine.
                    self.bind_pattern(pat, &origin, false, init.span);
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

    /// The origin of the storage a place expression names. A reference to a local borrows the local's slot, not
    /// whatever view that local holds, so this is separate from the value's origin.
    fn place_origin(&mut self, e: &tir::Expr) -> Origin {
        use tir::ExprKind as K;
        match &e.kind {
            K::Local(id) => self.local_storage(*id),
            // A static is storage that lives for the whole program, so a view of it is static data (RT4).
            K::Static(_) => Origin::default(),
            K::Field(base, _) => self.place_origin(base),
            // Indexing a slice looks through to whatever the slice views; indexing an array or a list names the
            // collection's own storage.
            K::Index(base, index) => {
                self.expr(index);
                if matches!(base.ty, tir::Ty::Slice(_)) { self.expr(base) } else { self.place_origin(base) }
            }
            // A `Heap` owns its pointee; a reference points at someone else's storage.
            K::Deref(base) => self.place_origin(base),
            // A raw pointer's pointee belongs to no local. A view of it derives from the storage that holds the
            // pointer, so a method of a handle type (`self.ptr`) can return a view that lives as long as the
            // handle is borrowed; `unsafe` is the promise that the pointee lives that long. A pointer held in a
            // local or a temporary gives a local or temporary origin, which cannot be returned (RT2).
            K::PtrDeref(base) => self.place_origin(base),
            // An element offset of a raw pointer points into the same allocation, so it keeps the pointer's origin.
            K::Binary(tir::BinOp::Add | tir::BinOp::Sub, base, offset) if matches!(base.ty, tir::Ty::CPtr(_)) => {
                self.expr(offset);
                self.place_origin(base)
            }
            K::DerefRef(base) => self.expr(base),
            _ => {
                self.expr(e);
                Origin::temp()
            }
        }
    }

    /// The origin of the storage an argument hands to a call. A value of a second-class type carries a view, so
    /// its own origin is what matters; a `View` binding stands for the storage it views. Any other argument's
    /// storage belongs to the argument's place or temporary.
    fn argument_storage(&mut self, e: &tir::Expr) -> Origin {
        let view_local = matches!(&e.kind, tir::ExprKind::Local(id) if self.f.locals[id.0].kind == LocalKind::View);
        if e.ty.is_second_class() || view_local { self.expr(e) } else { self.place_origin(e) }
    }

    fn expr(&mut self, e: &tir::Expr) -> Origin {
        let origin = self.expr_origin(e);
        // An expression that never produces a value (a call to `panic`) hands out no view.
        if e.ty == tir::Ty::Never { Origin::default() } else { origin }
    }

    fn expr_origin(&mut self, e: &tir::Expr) -> Origin {
        use tir::ExprKind as K;
        match &e.kind {
            K::Unit
            | K::Bool(_)
            | K::Int(_)
            | K::Float(_)
            | K::Char(_)
            | K::Str(_)
            | K::Bytes(_)
            | K::Const(_)
            | K::Static(_)
            | K::LayoutOf { .. } => Origin::default(),
            // A plain value carries no view, even when its storage is a parameter's.
            K::Local(id) => {
                if e.ty.is_second_class() {
                    self.local_value(*id)
                } else {
                    Origin::default()
                }
            }
            // A field, an element, a pointee, or a referent derives from the storage of its base.
            K::Field(base, _) | K::DerefRef(base) => self.expr(base),
            // A `Heap` owns its pointee, so a view of it derives from the heap's own storage.
            K::Deref(base) => {
                if base.ty.is_second_class() {
                    self.expr(base)
                } else {
                    self.place_origin(base)
                }
            }
            K::PtrDeref(base) => {
                self.expr(base);
                Origin::temp()
            }
            K::Index(base, index) => {
                self.expr(index);
                self.expr(base)
            }
            K::Ref { place, .. } => self.place_origin(place),
            K::Slice { base, lo, hi, .. } => {
                self.expr(lo);
                self.expr(hi);
                // Slicing a `String` views the string's own storage; slicing a view looks through it.
                if base.ty.is_second_class() { self.expr(base) } else { self.place_origin(base) }
            }
            // A `str` view of a `String` views the string's storage: the string's own place when it is owned,
            // or whatever a view of a string already views.
            K::StringAsStr(inner) => {
                if inner.ty.is_second_class() {
                    self.expr(inner)
                } else {
                    self.place_origin(inner)
                }
            }
            // A compile-time condition is a `bool`: it carries no view.
            K::ConstCond(inner) => {
                self.expr(inner);
                Origin::default()
            }
            // A box, a type test, and a downcast are owned values; they carry no view out of the value.
            K::BoxAny { value, .. }
            | K::AnyIs { value, .. }
            | K::AnyDowncast { value, .. }
            | K::AnyUpcast { value, .. } => {
                self.expr(value);
                Origin::default()
            }
            // A dynamic call's result is an owned type (object safety), so it derives from no argument.
            K::AnyCall { recv, args, .. } => {
                self.expr(recv);
                for a in args {
                    self.expr(a);
                }
                Origin::default()
            }
            K::ArrayRepeat(inner, _) | K::Unary(_, inner) | K::Cast(inner) => self.expr(inner),
            K::Array(elems) | K::Tuple(elems) => {
                let mut origin = Origin::default();
                for x in elems {
                    if x.ty.is_second_class() {
                        origin.join(&self.expr(x));
                    } else {
                        self.expr(x);
                    }
                }
                origin
            }
            // A closure derives from what its captures derive from: a borrowed capture from the captured
            // place's storage, a moved capture from the value that moved in (CL3, CL4).
            K::Closure { captures, .. } => {
                let mut origin = Origin::default();
                for c in captures {
                    match c.mode {
                        tir::CaptureMode::Move => origin.join(&self.local_value(c.local)),
                        _ => origin.join(&self.local_storage(c.local)),
                    }
                }
                origin
            }
            // A closure call's second-class result derives from the closure value it was called through.
            K::ClosureCall { callee, args, .. } => {
                let origin = self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
                if e.ty.is_second_class() { origin } else { Origin::temp() }
            }
            K::Struct(fields) | K::Variant(_, fields) => {
                let mut origin = Origin::default();
                for (_, x) in fields {
                    // Only a field that carries a view contributes an origin; an owned field that happens to
                    // name a parameter (`TakeIter { remaining: count }`) must not (RT2).
                    if x.ty.is_second_class() {
                        origin.join(&self.expr(x));
                    } else {
                        self.expr(x);
                    }
                }
                origin
            }
            // A C function returns a scalar or a raw pointer, never a view of Burdin storage.
            K::ExternCall(_, args) => {
                for arg in args {
                    self.expr(arg);
                }
                Origin::temp()
            }
            K::Call(f, args) => {
                let mut storages = Vec::with_capacity(args.len());
                for arg in args {
                    storages.push(self.argument_storage(arg));
                }
                let sig = &self.sigs[f.0];
                if !sig.ret.is_second_class() {
                    return Origin::temp();
                }
                // Static (no origin), or the union of the storage of every argument the result derives from.
                let mut origin = Origin::default();
                for &index in &sig.ret_origins {
                    if let Some(storage) = storages.get(index) {
                        origin.join(storage);
                    }
                }
                origin
            }
            K::Builtin(b, args) => {
                let mut origins = Vec::with_capacity(args.len());
                for arg in args {
                    origins.push(self.expr(arg));
                }
                match b {
                    // The old value comes out of the place, so a view result derives from it.
                    tir::Builtin::Replace => origins.into_iter().next().unwrap_or_default(),
                    // The text views the bytes it was made from.
                    tir::Builtin::ViewAsText => origins.into_iter().next().unwrap_or_default(),
                    // The view derives from the storage that holds the pointer, like a dereference of it.
                    tir::Builtin::SliceRaw | tir::Builtin::IterMutRaw => self.place_origin(&args[0]),
                    // An empty container owns its (empty) buffer; storing it borrows nothing. The same holds for
                    // a container with a stored allocator.
                    // A value read out of raw memory is whatever was stored there; unsafe code answers for it.
                    tir::Builtin::RawRead => Origin::default(),
                    tir::Builtin::ListNew
                    | tir::Builtin::MapNew
                    | tir::Builtin::ListNewIn
                    | tir::Builtin::MapNewIn
                    | tir::Builtin::HeapNewIn => Origin::default(),
                    _ => Origin::temp(),
                }
            }
            K::Method(m, args) => {
                // A method whose result is a view of its receiver hands out the receiver's storage, whether the
                // receiver is itself a view (`[T]`) or an owned container (`String`, `List`, a map).
                let receiver_view = matches!(
                    m,
                    tir::Method::AsStr
                        | tir::Method::AsSlice
                        | tir::Method::Clone
                        | tir::Method::ListLast
                        | tir::Method::ListFirst
                        | tir::Method::ListFirstMut
                        | tir::Method::ListLastMut
                        | tir::Method::ListGet
                        | tir::Method::ListGetMut
                        | tir::Method::SliceGet
                        | tir::Method::SliceGetRange
                        | tir::Method::SliceLast
                        | tir::Method::SliceChunks
                        | tir::Method::SliceWindows
                        | tir::Method::MapGet
                        | tir::Method::MapGetMut
                        | tir::Method::MapGetOrInsert
                        | tir::Method::MapGetOr
                        | tir::Method::AsBytes
                        | tir::Method::Iter
                        | tir::Method::IterMut
                        | tir::Method::IterNext
                        | tir::Method::IterNextBack
                );
                let mut origins = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    origins.push(if i == 0 && receiver_view { self.argument_storage(arg) } else { self.expr(arg) });
                }
                match m {
                    // A view of the receiver, or a copy of it.
                    tir::Method::AsStr
                    | tir::Method::AsSlice
                    | tir::Method::Clone
                    | tir::Method::ListLast
                    | tir::Method::ListFirst
                    | tir::Method::ListFirstMut
                    | tir::Method::ListLastMut
                    | tir::Method::ListGet
                    | tir::Method::ListGetMut
                    | tir::Method::SliceGet
                    | tir::Method::SliceGetRange
                    | tir::Method::SliceLast
                    | tir::Method::SliceChunks
                    | tir::Method::SliceWindows
                    | tir::Method::MapGet
                    | tir::Method::MapGetMut
                    | tir::Method::MapGetOrInsert
                    | tir::Method::MapGetOr
                    | tir::Method::AsBytes
                    // An iterator borrows the collection it walks; the items it yields point into it.
                    | tir::Method::Iter
                    | tir::Method::IterMut
                    | tir::Method::IterNext
                    | tir::Method::IterNextBack => origins.into_iter().next().unwrap_or_default(),
                    _ => Origin::temp(),
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
            K::Labeled(label, inner) => {
                // The label belongs to the loop node right below it; the loop pushes it and its break values
                // join into that loop's origin.
                let saved = self.pending_label.replace(label.clone());
                let origin = self.expr(inner);
                self.pending_label = saved;
                origin
            }
            K::While(cond, body) => {
                self.open_loop();
                self.expr(cond);
                self.block(body);
                self.loops.pop();
                Origin::default()
            }
            K::Loop(body) => {
                self.open_loop();
                self.block(body);
                let origin = self.loops.pop().expect("opened above").origin;
                // A `loop` gives back the joined origin of its break values; `while` and `for` give `()`.
                origin
            }
            K::Range { lo, hi, .. } => {
                let mut origin = self.expr(lo);
                origin.join(&self.expr(hi));
                origin
            }
            K::ForRangeValue { var, range, body } => {
                self.open_loop();
                self.expr(range);
                let id = self.block_parent.len();
                self.block_parent.push(Some(self.current_block));
                self.current_block = id;
                if let Some(var) = var {
                    self.local_block[var.0] = Some(id);
                }
                self.block_body(body);
                self.current_block = self.block_parent[id].expect("a nested block has a parent");
                self.loops.pop();
                Origin::default()
            }
            K::ForRange { var, lo, hi, body, .. } => {
                self.open_loop();
                self.expr(lo);
                self.expr(hi);
                let id = self.block_parent.len();
                self.block_parent.push(Some(self.current_block));
                self.current_block = id;
                if let Some(var) = var {
                    // The loop binding lives for one iteration of the body.
                    self.local_block[var.0] = Some(id);
                }
                self.block_body(body);
                self.current_block = self.block_parent[id].expect("a nested block has a parent");
                self.loops.pop();
                Origin::default()
            }
            K::ForArray { pat, array, body }
            | K::ForSlice { pat, slice: array, body }
            | K::ForList { pat, list: array, body }
            | K::ForMap { pat, map: array, body } => {
                self.open_loop();
                let origin = self.expr(array);
                let id = self.block_parent.len();
                self.block_parent.push(Some(self.current_block));
                self.current_block = id;
                // The scrutinee's temporaries live until the end of the loop statement, so a view bound by the
                // pattern is valid for the body even when the scrutinee was a temporary.
                self.bind_pattern(pat, &origin, true, array.span);
                self.block_body(body);
                self.current_block = self.block_parent[id].expect("a nested block has a parent");
                self.loops.pop();
                Origin::default()
            }
            K::Match(scrutinee, arms) => {
                // Pattern bindings view sub-places of the scrutinee, so a plain value contributes its storage;
                // a value that is itself a view contributes whatever it views.
                let origin =
                    if scrutinee.ty.is_second_class() { self.expr(scrutinee) } else { self.place_origin(scrutinee) };
                let mut result = Origin::default();
                for arm in arms {
                    let parent = self.current_block;
                    let id = self.block_parent.len();
                    self.block_parent.push(Some(parent));
                    self.current_block = id;
                    // An arm binding lives no longer than the match statement, whose temporaries are still
                    // alive, so a temporary scrutinee is fine here.
                    self.bind_pattern(&arm.pat, &origin, true, scrutinee.span);
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    result.join(&self.expr(&arm.body));
                    self.current_block = parent;
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
                let origin = self.expr(value);
                if let K::Local(id) = &place.kind
                    && self.f.locals[id.0].ty.is_second_class()
                {
                    // The target outlives this statement; its block decides how long its view may borrow.
                    let target = self.local_block[id.0].unwrap_or(self.current_block);
                    self.check_stored_view(&origin, target, false, value.span);
                    self.bindings[id.0] = Some(origin);
                }
                self.place_origin(place);
                Origin::default()
            }
            K::CompoundAssign(_, place, value) => {
                // A built-in value holds no view, so nothing is stored; the operands are still walked.
                self.expr(value);
                self.place_origin(place);
                Origin::default()
            }
            K::Binary(_, l, r) => {
                self.expr(l);
                self.expr(r);
                Origin::temp()
            }
            K::Break { label, value } => {
                // A break value leaves the loop with the jump, so its origin joins the loop's result and the
                // return and binding checks (RT2, RT5) see where it came from.
                if let Some(value) = value {
                    let origin = self.expr(value);
                    let target = match label {
                        None => self.loops.len().checked_sub(1),
                        Some(name) => self.loops.iter().rposition(|lp| lp.label.as_deref() == Some(name.as_str())),
                    };
                    if let Some(target) = target
                        && let Some(open) = self.loops.get_mut(target)
                    {
                        open.origin.join(&origin);
                    }
                }
                Origin::default()
            }
            K::Continue(_) => Origin::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_lists_the_parameters_of_a_result() {
        assert_eq!(Origin::param(0).summary(), vec![0]);
        assert_eq!(Origin::param(2).summary(), vec![2]);
        assert!(Origin::local(LocalId(3)).summary().is_empty());
        assert!(Origin::temp().summary().is_empty());
        assert!(Origin::default().summary().is_empty());
        let mut two = Origin::param(0);
        two.join(&Origin::param(1));
        assert_eq!(two.summary(), vec![0, 1], "an aggregate result may derive from two parameters (RT3a)");
        let mut mixed = Origin::param(0);
        mixed.join(&Origin::local(LocalId(1)));
        assert!(mixed.summary().is_empty(), "a local cannot escape in a view");
    }

    #[test]
    fn joining_unions_parameters_locals_and_temporaries() {
        let mut a = Origin::param(0);
        a.join(&Origin::param(1));
        assert_eq!(a.params.iter().copied().collect::<Vec<_>>(), vec![0, 1]);
        assert!(!a.escapes());
        a.join(&Origin::local(LocalId(4)));
        assert_eq!(a.locals.iter().copied().collect::<Vec<_>>(), vec![4]);
        assert!(a.escapes());
        let mut b = Origin::temp();
        b.join(&Origin::temp());
        assert!(b.temp);
        assert!(!Origin::param(0).summary().is_empty());
    }

    #[test]
    fn only_locals_and_temporaries_escape_a_return() {
        assert!(!Origin::default().escapes(), "static data is fine");
        assert!(!Origin::param(0).escapes());
        assert!(Origin::local(LocalId(0)).escapes());
        assert!(Origin::temp().escapes());
    }
}
