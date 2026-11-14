//! Name resolution and type checking. Produces the typed tree in [`burdin_tir`].
//!
//! The checker covers the stage 0 language up to milestone 7: functions, constants, the primitive types, arrays,
//! tuples, structs, enums, type aliases, method blocks, ownership, conventions, references, and generics with
//! interfaces (see `generics.rs`). Everything else is reported with `Code::UNSUPPORTED` until its milestone.
//!
//! Generics are monomorphized here, so the memory checks (`burdin_check`) and code generation only ever see
//! concrete types. The work is organized in phases over one `Sema` (`session.rs`):
//!
//! 1. `declare`: names of types, interfaces, functions, and constants, and the impl blocks (`impls`).
//! 2. `resolve` and `bodies`: written types and function bodies (`fncx` checks one body), repeated with
//!    `instances` until no generic instance is missing.
//! 3. `validate` and `derives`: checks that need the whole program.

mod bodies;
mod builtins;
mod consts;
mod declare;
mod derives;
mod exhaust;
mod fncx;
mod generics;
mod globals;
mod impls;
mod instances;
mod resolve;
mod session;
mod types;
mod validate;

use burdin_diag::{Diagnostics, FileId};
use burdin_syntax::ast;
use burdin_tir::Program;
use session::Sema;

/// Type-checks a parsed file. Errors go to `diags`; the program is returned either way, because the memory checks
/// still run on it to report every error in the file. Do not generate code when `diags` has errors.
pub fn check(file: FileId, ast: &ast::File, diags: &mut Diagnostics) -> Program {
    let mut sema = Sema::new();
    sema.declare_types(ast, diags);
    sema.declare_bounds(diags);
    sema.declare_values(ast, diags);
    sema.declare_identities(diags);
    let (functions, consts) = sema.settle(file, diags);
    sema.finish(file, functions, consts, diags)
}
