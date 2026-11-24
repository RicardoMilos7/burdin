//! Reachability: which functions a program can actually run.
//!
//! The program carries every function the checker saw, including the whole embedded standard library. A function
//! nothing can reach never runs, so its body is not emitted: the module keeps a declaration (so a call from another
//! unreachable body still type-checks) and a body that is just `unreachable`. That keeps a program's IR — and its
//! binary — proportional to what the program uses, which the code-generation checks rely on when they count drop
//! flags and bounds checks.
//!
//! The roots are `main`, the `test` and `bench` blocks, the exported (`extern "C" fn`) functions, every `Drop`
//! implementation,
//! and every vtable entry; a vtable entry is a root because it is reached only through an indirect call the walker
//! cannot see. Everything they call is reachable as well, and so is every closure body a reachable body creates: a
//! closure inside a function nothing reaches is dead code too, which keeps the scheduler of `std.task` out of a program
//! that starts no task.

use crate::emit::Codegen;
use burdin_tir as tir;

/// What the program can reach: flags indexed like `program.functions` and `program.closures`.
pub(super) struct Reach {
    pub(super) functions: Vec<bool>,
    pub(super) closures: Vec<bool>,
}

impl Codegen<'_, '_> {
    /// The functions and closure bodies the program can reach.
    pub(super) fn reachable(&self) -> Reach {
        let program = self.program;
        let mut reachable = vec![false; program.functions.len()];
        let mut closures_reached = vec![false; program.closures.len()];
        let mut pending: Vec<usize> = Vec::new();
        if let Some(main) = program.main {
            mark(main, &mut reachable, &mut pending);
        }
        // The static initializer runs before `main` (`spec/globals.md`, GL1), so it is reachable with it.
        if let Some(init) = program.static_init {
            mark(init, &mut reachable, &mut pending);
        }
        // A library's artifact holds none of its test, bench, or fuzz blocks (`spec/library.md`, LB6).
        if !self.library {
            for test in &program.tests {
                mark(test.func, &mut reachable, &mut pending);
            }
            // The bench dispatcher calls each bench the same way (`spec/bench.md`, BN4).
            for bench in &program.benches {
                mark(bench.func, &mut reachable, &mut pending);
            }
            // The test dispatcher replays every fuzz target; a fuzz build's entry point calls its own.
            for fuzz in &program.fuzzes {
                mark(fuzz.func, &mut reachable, &mut pending);
            }
        }
        for (i, f) in program.functions.iter().enumerate() {
            if f.exported {
                mark(tir::FuncId(i), &mut reachable, &mut pending);
            }
        }
        let drops = program
            .structs
            .iter()
            .filter_map(|s| s.drop_method)
            .chain(program.enums.iter().filter_map(|e| e.drop_method));
        for drop in drops {
            mark(drop, &mut reachable, &mut pending);
        }
        // A vtable entry is called indirectly, through the vtable's slot.
        for vtable in &program.any_vtables {
            for entry in &vtable.entries {
                mark(*entry, &mut reachable, &mut pending);
            }
        }
        // A `blocking` extern call goes through the standard library's offload shim, which no expression names
        // (`spec/io.md`, IO4). It is only rooted when the program actually declares one.
        if program.externs.iter().any(|e| e.blocking)
            && let Some(shim) = program.functions.iter().position(|f| f.name == "std.__offload_blocking")
        {
            mark(tir::FuncId(shim), &mut reachable, &mut pending);
        }
        // A container's buffer is allocated and freed through its allocator implementation, which no expression
        // calls directly (`spec/allocators.md`, AL4).
        for d in program.structs.iter().map(|s| &s.derives).chain(program.enums.iter().map(|e| &e.derives)) {
            for f in [d.eq_method, d.lt_method].into_iter().flatten() {
                mark(f, &mut reachable, &mut pending);
            }
        }
        for fns in &program.allocator_fns {
            mark(fns.allocate, &mut reachable, &mut pending);
            mark(fns.reallocate, &mut reachable, &mut pending);
            mark(fns.deallocate, &mut reachable, &mut pending);
        }
        let mut calls: Vec<tir::FuncId> = Vec::new();
        let mut closures: Vec<usize> = Vec::new();
        let mut pending_closures: Vec<usize> = Vec::new();
        loop {
            if let Some(i) = pending.pop() {
                refs_in(&program.functions[i].body, &mut calls, &mut closures);
            } else if let Some(c) = pending_closures.pop() {
                refs_in(&program.closures[c].func.body, &mut calls, &mut closures);
            } else {
                break;
            }
            for id in calls.drain(..) {
                mark(id, &mut reachable, &mut pending);
            }
            for c in closures.drain(..) {
                if !closures_reached[c] {
                    closures_reached[c] = true;
                    pending_closures.push(c);
                }
            }
        }
        Reach { functions: reachable, closures: closures_reached }
    }
}

fn mark(id: tir::FuncId, reachable: &mut [bool], pending: &mut Vec<usize>) {
    if let Some(slot) = reachable.get_mut(id.0)
        && !*slot
    {
        *slot = true;
        pending.push(id.0);
    }
}

/// Every function `b` calls and every closure it creates, in any expression it contains.
fn refs_in(b: &tir::Block, calls: &mut Vec<tir::FuncId>, closures: &mut Vec<usize>) {
    let mut walk = Walker { out: calls, closures };
    walk.block(b);
}

struct Walker<'a> {
    out: &'a mut Vec<tir::FuncId>,
    closures: &'a mut Vec<usize>,
}

impl Walker<'_> {
    fn block(&mut self, b: &tir::Block) {
        for stmt in &b.stmts {
            match stmt {
                tir::Stmt::Let(_, init, else_block) => {
                    self.expr(init);
                    if let Some(b) = else_block {
                        self.block(b);
                    }
                }
                tir::Stmt::Expr(e) => self.expr(e),
            }
        }
        if let Some(tail) = &b.tail {
            self.expr(tail);
        }
    }

    fn all(&mut self, es: &[tir::Expr]) {
        for e in es {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &tir::Expr) {
        use tir::ExprKind as K;
        match &e.kind {
            K::Call(f, args) => {
                self.out.push(*f);
                self.all(args);
            }
            K::Closure { closure, .. } => self.closures.push(*closure),
            K::Block(b) => self.block(b),
            K::If(cond, then_block, else_expr) => {
                self.expr(cond);
                self.block(then_block);
                if let Some(e) = else_expr {
                    self.expr(e);
                }
            }
            K::While(cond, body) => {
                self.expr(cond);
                self.block(body);
            }
            K::Loop(body) => self.block(body),
            K::Labeled(_, inner) => self.expr(inner),
            K::ForRange { lo, hi, body, .. } => {
                self.expr(lo);
                self.expr(hi);
                self.block(body);
            }
            K::ForRangeValue { range, body, .. } => {
                self.expr(range);
                self.block(body);
            }
            K::ForArray { array, body, .. }
            | K::ForSlice { slice: array, body, .. }
            | K::ForList { list: array, body, .. }
            | K::ForMap { map: array, body, .. }
            | K::ForChars { string: array, body, .. } => {
                self.expr(array);
                self.block(body);
            }
            K::Match(scrutinee, arms) => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&arm.body);
                }
            }
            K::Return(Some(e)) => self.expr(e),
            K::Builtin(_, args) | K::Method(_, args) | K::ExternCall(_, args) => self.all(args),
            K::ClosureCall { callee, args, .. } => {
                self.expr(callee);
                self.all(args);
            }
            K::AnyCall { recv, args, .. } => {
                self.expr(recv);
                self.all(args);
            }
            K::BoxAny { value, .. } | K::AnyIs { value, .. } | K::AnyDowncast { value, .. } => self.expr(value),
            // The comptime pass replaces these before code generation (spec/comptime.md, CT13); what they call is
            // not reachable from the generated program.
            K::Comptime(_) | K::ComptimeFor { .. } => {}
            K::StringAsStr(inner)
            | K::Unary(_, inner)
            | K::Cast(inner)
            | K::Deref(inner)
            | K::PtrDeref(inner)
            | K::DerefRef(inner)
            | K::Field(inner, _)
            | K::Some(inner)
            | K::Ok(inner)
            | K::Err(inner) => self.expr(inner),
            K::Assign(a, b) | K::CompoundAssign(_, a, b) | K::Binary(_, a, b) | K::Index(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            K::Range { lo, hi, .. } => {
                self.expr(lo);
                self.expr(hi);
            }
            K::Ref { place, .. } => self.expr(place),
            K::Slice { base, lo, hi, .. } => {
                self.expr(base);
                self.expr(lo);
                self.expr(hi);
            }
            K::Array(es) | K::Tuple(es) => self.all(es),
            K::ArrayRepeat(a, _) => self.expr(a),
            K::Struct(fields) | K::Variant(_, fields) => {
                for (_, e) in fields {
                    self.expr(e);
                }
            }
            K::Unit
            | K::Bool(_)
            | K::Int(_)
            | K::Float(_)
            | K::Char(_)
            | K::Str(_)
            | K::Bytes(_)
            | K::Local(_)
            | K::Const(_)
            | K::Static(_)
            | K::Continue(_)
            | K::None
            | K::Return(None)
            | K::LayoutOf { .. } => {}
            K::Break { value, .. } => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
        }
    }
}
