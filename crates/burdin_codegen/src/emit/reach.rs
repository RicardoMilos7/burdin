//! Reachability: which functions a program can actually run.
//!
//! The program carries every function the checker saw, including the whole embedded standard library. A function
//! nothing can reach never runs, so its body is not emitted: the module keeps a declaration (so a call from another
//! unreachable body still type-checks) and a body that is just `unreachable`. That keeps a program's IR — and its
//! binary — proportional to what the program uses, which the code-generation checks rely on when they count drop
//! flags and bounds checks.
//!
//! The roots are `main`, the `test` blocks, the exported (`extern "C" fn`) functions, every `Drop` implementation,
//! every closure body, and every vtable entry. A closure body is a root because closures are emitted for the whole
//! program; a vtable entry is a root because it is reached only through an indirect call the walker cannot see.
//! Everything they call is reachable as well.

use crate::emit::Codegen;
use burdin_tir as tir;

impl Codegen<'_, '_> {
    /// The functions the program can reach, as indices into `program.functions`.
    pub(super) fn reachable_functions(&self) -> Vec<bool> {
        let program = self.program;
        let mut reachable = vec![false; program.functions.len()];
        let mut pending: Vec<usize> = Vec::new();
        if let Some(main) = program.main {
            mark(main, &mut reachable, &mut pending);
        }
        for test in &program.tests {
            mark(test.func, &mut reachable, &mut pending);
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
        // Every closure body is emitted, so everything it calls is reachable too.
        let mut closure_calls: Vec<tir::FuncId> = Vec::new();
        for clos in &program.closures {
            calls_in(&clos.func.body, &mut closure_calls);
        }
        for id in closure_calls {
            mark(id, &mut reachable, &mut pending);
        }
        // A vtable entry is called indirectly, through the vtable's slot.
        for vtable in &program.any_vtables {
            for entry in &vtable.entries {
                mark(*entry, &mut reachable, &mut pending);
            }
        }
        // A container's buffer is allocated and freed through its allocator implementation, which no expression
        // calls directly (`spec/allocators.md`, AL4).
        for fns in &program.allocator_fns {
            mark(fns.allocate, &mut reachable, &mut pending);
            mark(fns.reallocate, &mut reachable, &mut pending);
            mark(fns.deallocate, &mut reachable, &mut pending);
        }
        let mut calls: Vec<tir::FuncId> = Vec::new();
        while let Some(i) = pending.pop() {
            calls.clear();
            calls_in(&program.functions[i].body, &mut calls);
            for id in calls.drain(..) {
                mark(id, &mut reachable, &mut pending);
            }
        }
        reachable
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

/// Every function `b` calls, in any expression it contains.
pub(super) fn calls_in(b: &tir::Block, out: &mut Vec<tir::FuncId>) {
    let mut walk = Walker { out };
    walk.block(b);
}

struct Walker<'a> {
    out: &'a mut Vec<tir::FuncId>,
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
            K::Assign(a, b) | K::Binary(_, a, b) | K::Index(a, b) => {
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
            | K::Local(_)
            | K::Const(_)
            | K::Closure { .. }
            | K::Break
            | K::Continue
            | K::None
            | K::Return(None)
            | K::LayoutOf { .. } => {}
        }
    }
}
