//! Name resolution and type checking. Produces the typed tree in [`burdin_tir`].
//!
//! The checker covers the stage 0 language up to milestone 9: every file is a module, packages import each other
//! with `use`, `priv` items are package-private, and `extern "C"` declarations plus `unsafe` blocks expose the C
//! ABI (see `modules.rs` and `unsafe.rs`). Everything else is reported with `Code::UNSUPPORTED` until its milestone;
//! `spec/e9000-audit.md` lists every remaining site with the milestone and issue that remove it.
//!
//! Generics are monomorphized here, so the memory checks (`burdin_check`) and code generation only ever see
//! concrete types. The work is organized in phases over one `Sema` (`session.rs`):
//!
//! 1. `declare`: names of types, interfaces, functions, constants, and extern functions.
//! 2. `imports`: `use` items, copied into each module's namespace; `impls` registers the impl blocks.
//! 3. `resolve` and `bodies`: written types and function bodies (`fncx` checks one body), repeated with
//!    `instances` until no generic instance is missing.
//! 4. `validate` and `derives`: checks that need the whole program.

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
mod imports;
mod instances;
mod prelude;
mod resolve;
mod session;
mod types;
mod validate;

use burdin_diag::{Diagnostics, FileId};
use burdin_syntax::ast;
use burdin_tir::Program;
use session::Sema;

/// One module of a package: a source file and the path it is known by. A single-file target is one module with an
/// empty path.
pub struct Module<'a> {
    pub file: FileId,
    /// The path inside the package's `src`, for example `["geometry", "shapes"]`.
    pub path: Vec<String>,
    pub ast: &'a ast::File,
}

/// One package of the compilation. The package being built is marked `is_root`; its dependencies are the rest.
pub struct Package<'a> {
    pub name: String,
    pub is_root: bool,
    /// The packages this one may import from, by the name its `use` items write.
    pub deps: Vec<(String, usize)>,
    /// The module other packages reach as `use <package>` (an index into `modules`).
    pub root_module: Option<usize>,
    pub modules: Vec<Module<'a>>,
}

/// Type-checks a parsed program. `entry_package` and `entry_module` name the module whose `main` is the program's
/// entry point (the module index is relative to that package). With `test_mode`, the `test` items of the entry
/// package are declared and checked too, and `main` is not required; that is what `burdin test` builds.
/// Errors go to `diags`; the program is returned either way, because the memory checks still run on it to report
/// every error in the file. Do not generate code when `diags` has errors.
pub fn check<'a>(
    packages: &'a [Package<'a>],
    entry_package: usize,
    entry_module: usize,
    test_mode: bool,
    diags: &mut Diagnostics,
) -> Program {
    let mut sema = Sema::new(packages, entry_package, entry_module, test_mode);
    sema.declare_types(diags);
    sema.declare_values(diags);
    sema.resolve_imports(diags);
    sema.install_prelude();
    sema.declare_bounds(diags);
    sema.register_impls(diags);
    sema.declare_identities(diags);
    let (functions, consts) = sema.settle(diags);
    sema.finish(functions, consts, diags)
}
