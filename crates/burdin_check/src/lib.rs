//! The memory checks of a type-checked program (spec/memory-rules.md): return origins, moves and drops,
//! mutability and exclusivity, and borrows. Each check looks at one function body at a time, from signatures only.

mod borrows;
mod moves;
mod origins;

use burdin_diag::Diagnostics;
use burdin_tir::{AnyIface, Program, Signature, TypeDefs};

/// What the checks need to know about the rest of the program: type definitions and callee signatures.
#[derive(Clone, Copy)]
struct Env<'a> {
    defs: TypeDefs<'a>,
    sigs: &'a [Signature],
    /// The vtable methods of every `any Interface`, whose receiver tells a dynamic call how it borrows the box.
    any_ifaces: &'a [AnyIface],
}

/// Runs every memory check on `program`, filling in what later stages need: the return origin of each function and
/// the places it moves (for drop flags). Errors go to `diags`.
pub fn check(program: &mut Program, diags: &mut Diagnostics) {
    let Program { functions, structs, enums, signatures, any_ifaces, .. } = program;

    // The return origin of a signature is inferred from the body (RT): first to a fixpoint, then reported.
    origins::check(signatures, functions, diags);

    let env = Env { defs: TypeDefs { structs, enums }, sigs: signatures, any_ifaces };
    for func in functions.iter_mut().filter(|f| !f.symbolic) {
        func.moved = moves::check_function(func, env, diags);
    }
    for closure in &mut *program.closures {
        closure.func.moved = moves::check_function(&closure.func, env, diags);
    }
    for func in functions.iter().filter(|f| !f.symbolic) {
        borrows::check(func, env, diags);
    }
    for closure in &program.closures {
        borrows::check(&closure.func, env, diags);
        origins::check_closure(&closure.func, signatures, diags);
    }
}
