//! The memory checks of a type-checked program (spec/memory-rules.md): return origins, moves and drops,
//! mutability and exclusivity, and borrows. Each check looks at one function body at a time, from signatures only.

mod borrows;
mod moves;
mod origins;

use burdin_diag::Diagnostics;
use burdin_tir::{AnyIface, Block, Function, Program, Signature, TypeDefs};

/// What the checks need to know about the rest of the program: type definitions and callee signatures.
#[derive(Clone, Copy)]
struct Env<'a> {
    defs: TypeDefs<'a>,
    sigs: &'a [Signature],
    /// The vtable methods of every `any Interface`, whose receiver tells a dynamic call how it borrows the box.
    any_ifaces: &'a [AnyIface],
    /// Which functions are `Scope.spawn` (spec/tasks.md, TK7): the closure they are given borrows from the scope's
    /// surroundings until the scope body ends.
    scoped_spawns: &'a [bool],
    /// Every function's name, indexed by `FuncId`. The ownership events leave out the compiler's own generated
    /// forwarding calls (an interpolated string expands to them), whose names start with `__`.
    names: &'a [String],
}

/// Runs every memory check on `program`, filling in what later stages need: the return origin of each function and
/// the places it moves (for drop flags). Errors go to `diags`.
pub fn check(program: &mut Program, diags: &mut Diagnostics) {
    let Program { functions, structs, enums, signatures, any_ifaces, .. } = program;

    // The return origin of a signature is inferred from the body (RT): first to a fixpoint, then reported.
    origins::check(signatures, functions, diags);

    let scoped_spawns: Vec<bool> = functions.iter().map(|f| is_scope_spawn(&f.name)).collect();
    let names: Vec<String> = functions.iter().map(|f| f.name.clone()).collect();
    let env = Env {
        defs: TypeDefs { structs, enums },
        sigs: signatures,
        any_ifaces,
        scoped_spawns: &scoped_spawns,
        names: &names,
    };
    for func in functions.iter_mut().filter(|f| !f.symbolic) {
        let (moved, debug_kills, events) = moves::check_function(func, env, diags);
        func.moved = moved;
        func.debug_kills = debug_kills;
        func.events = events;
    }
    for closure in &mut *program.closures {
        let (moved, debug_kills, events) = moves::check_function(&closure.func, env, diags);
        closure.func.moved = moved;
        closure.func.debug_kills = debug_kills;
        closure.func.events = events;
    }
    for func in functions.iter_mut().filter(|f| !f.symbolic) {
        let events = borrows::check(func, env, &[], diags);
        func.events.extend(events);
        func.events.sort_by_key(|e| (e.span.file.0, e.span.lo, e.span.hi));
    }
    for closure in &mut *program.closures {
        let outer: Vec<_> = closure.captures.iter().map(|c| c.local).collect();
        let events = borrows::check(&closure.func, env, &outer, diags);
        closure.func.events.extend(events);
        closure.func.events.sort_by_key(|e| (e.span.file.0, e.span.lo, e.span.hi));
        origins::check_closure(&closure.func, signatures, diags);
    }
    // A constant's initializer is code the interpreter runs (`spec/const-eval.md`, CE2), so it follows the same
    // move and borrow rules as a function body that returns the value.
    for c in &program.consts {
        let func = Function {
            name: c.name.clone(),
            span: c.span,
            params: Vec::new(),
            ret: c.value.ty.clone(),
            locals: c.locals.clone(),
            body: Block { stmts: Vec::new(), tail: Some(Box::new(c.value.clone())), close: c.span },
            moved: Vec::new(),
            debug_kills: Vec::new(),
            ret_origins: Vec::new(),
            symbolic: false,
            exported: false,
            events: Vec::new(),
        };
        moves::check_function(&func, env, diags);
        borrows::check(&func, env, &[], diags);
    }
}

/// Whether a function instance is `std.task.Scope.spawn` (with or without its type arguments).
fn is_scope_spawn(name: &str) -> bool {
    name.split('<').next() == Some("std.task.Scope.spawn")
}
