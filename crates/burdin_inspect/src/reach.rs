//! Which functions code generation emits (the same roots and the same walk as `burdin_codegen`'s reachability pass),
//! so the inspection commands agree with the binary about what is in it.

use burdin_tir::visit::{Visit, walk_expr};
use burdin_tir::{Expr, ExprKind, FuncId, Program};
use std::collections::HashSet;

/// The functions code generation starts from: `main`, the `test` and `bench` blocks, the exported functions, every
/// `Drop` implementation, the vtable entries, and the allocator implementations of the containers the program stores.
pub fn roots(program: &Program) -> Vec<usize> {
    let mut roots: Vec<usize> = Vec::new();
    roots.extend(program.main.map(|id| id.0));
    roots.extend(program.tests.iter().map(|t| t.func.0));
    roots.extend(program.benches.iter().map(|b| b.func.0));
    roots.extend(program.functions.iter().enumerate().filter(|(_, f)| f.exported).map(|(i, _)| i));
    roots.extend(program.structs.iter().filter_map(|s| s.drop_method).map(|id| id.0));
    roots.extend(program.enums.iter().filter_map(|e| e.drop_method).map(|id| id.0));
    roots.extend(program.any_vtables.iter().flat_map(|v| v.entries.iter().map(|id| id.0)));
    for d in program.structs.iter().map(|s| &s.derives).chain(program.enums.iter().map(|e| &e.derives)) {
        roots.extend([d.eq_method, d.lt_method, d.clone_method].into_iter().flatten().map(|id| id.0));
    }
    roots.extend(program.allocator_fns.iter().flat_map(|f| [f.allocate.0, f.reallocate.0, f.deallocate.0]));
    roots.retain(|&i| i < program.functions.len());
    roots
}

/// Every function a body calls and every closure it creates, in body order. A box into an `any` counts as a call of
/// the vtable's methods, and a closure's body belongs to the function that creates it.
pub struct Refs<'p> {
    program: &'p Program,
    /// The calls, with the span of the call expression.
    pub calls: Vec<(FuncId, burdin_diag::Span)>,
    seen_closures: HashSet<usize>,
}

impl<'p> Refs<'p> {
    pub fn new(program: &'p Program) -> Refs<'p> {
        Refs { program, calls: Vec::new(), seen_closures: HashSet::new() }
    }

    /// Collects the calls of one function into `calls` (cleared first).
    pub fn of_function(&mut self, f: &burdin_tir::Function) {
        self.calls.clear();
        self.seen_closures.clear();
        self.block(&f.body);
    }
}

impl Visit for Refs<'_> {
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call(id, _) if id.0 < self.program.functions.len() => self.calls.push((*id, e.span)),
            ExprKind::BoxAny { vtable, .. } => {
                for entry in &self.program.any_vtables[*vtable].entries {
                    if entry.0 < self.program.functions.len() {
                        self.calls.push((*entry, e.span));
                    }
                }
            }
            ExprKind::Closure { closure, .. } if self.seen_closures.insert(*closure) => {
                let body = &self.program.closures[*closure].func.body;
                self.block(body);
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

/// Flags indexed like `program.functions`: whether code generation emits the function's body.
pub fn functions(program: &Program) -> Vec<bool> {
    let mut reached = vec![false; program.functions.len()];
    let mut pending: Vec<usize> = Vec::new();
    for root in roots(program) {
        if !reached[root] {
            reached[root] = true;
            pending.push(root);
        }
    }
    let mut refs = Refs::new(program);
    while let Some(i) = pending.pop() {
        refs.of_function(&program.functions[i]);
        for (callee, _) in &refs.calls {
            if !reached[callee.0] && !program.functions[callee.0].symbolic {
                reached[callee.0] = true;
                pending.push(callee.0);
            }
        }
    }
    reached
}
