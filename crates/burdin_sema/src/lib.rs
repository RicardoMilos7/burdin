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

mod aliases;
mod any;
mod bodies;
mod builtins;
mod cache;
mod comptime;
mod consteval;
mod consts;
mod declare;
mod derives;
mod exhaust;
mod extensions;
mod fncx;
mod generics;
mod globals;
mod impl_match;
#[cfg(test)]
mod impl_tests;
mod impls;
mod imports;
mod instances;
mod iterators;
mod markers;
mod prelude;
mod resolve;
mod session;
mod types;
mod user_derives;
mod validate;

pub use cache::{CheckCache, CheckStats, Edit};

use burdin_diag::{Diagnostics, FileId, SourceMap};
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

/// A language edition (`docs/releasing.md`, "Editions and migrations"; `spec/fix.md`, FX10): the meaning of the
/// language a package is written against. Each package names its own in `burdin.toml`; packages of different
/// editions link together, and every rule an edition changes is checked with the edition of the package whose
/// source is being checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edition {
    /// The first edition, and the default of a package whose manifest names none (and of a one-file target).
    #[default]
    E2024,
    /// Removes the forms edition 2024 deprecates (`spec/fix.md`, FX11).
    E2026,
}

impl Edition {
    /// Every edition this compiler knows, oldest first.
    pub const ALL: [Edition; 2] = [Edition::E2024, Edition::E2026];
    /// The newest edition: what `burdin fix --migrate` moves a package to.
    pub const LATEST: Edition = Edition::E2026;

    pub fn as_str(self) -> &'static str {
        match self {
            Edition::E2024 => "2024",
            Edition::E2026 => "2026",
        }
    }

    pub fn parse(text: &str) -> Option<Edition> {
        Edition::ALL.into_iter().find(|e| e.as_str() == text)
    }

    /// The edition after this one, if there is one.
    pub fn next(self) -> Option<Edition> {
        Edition::ALL.into_iter().find(|e| *e > self)
    }
}

/// One package of the compilation. The package being built is marked `is_root`; its dependencies are the rest.
pub struct Package<'a> {
    pub name: String,
    pub is_root: bool,
    /// The edition the package's source is written against.
    pub edition: Edition,
    /// The packages this one may import from, by the name its `use` items write.
    pub deps: Vec<(String, usize)>,
    /// The module other packages reach as `use <package>` (an index into `modules`).
    pub root_module: Option<usize>,
    pub modules: Vec<Module<'a>>,
}

/// What the entry point of this build is: the program's `main`, its `test` items, or its `bench` items. `burdin
/// build` and `burdin run` check a program; `burdin test` checks the entry package's tests and needs no `main`;
/// `burdin bench` checks its benches the same way (`spec/bench.md`, BN2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EntryMode {
    /// The program's `main` function.
    #[default]
    Build,
    /// The `test` items of the entry package (`burdin test`).
    Test,
    /// The `bench` items of the entry package (`burdin bench`).
    Bench,
    /// The `fuzz` targets of the entry package (`burdin fuzz`, `spec/fuzz.md`).
    Fuzz,
}

impl EntryMode {
    /// Whether `main` is required and called. Test and bench builds replace the entry point with their own
    /// dispatcher, so a package whose items all live in other files needs none.
    pub fn wants_main(self) -> bool {
        self == EntryMode::Build
    }
}

/// Type-checks a parsed program. `entry_package` and `entry_module` name the module whose `main` is the program's
/// entry point (the module index is relative to that package). With [`EntryMode::Test`], the `test` items of the
/// entry package are declared and checked too, and `main` is not required; that is what `burdin test` builds
/// ([`EntryMode::Bench`] is `burdin bench` over `bench` items, `spec/bench.md`). `traces` turns on the source
/// locations the dynamic `Error` records in a debug build (`spec/errors.md`, ERR7), and `sources` is the map those
/// locations (and the typed test assertions) are read from.
/// Errors go to `diags`; the program is returned either way, because the memory checks still run on it to report
/// every error in the file. Do not generate code when `diags` has errors.
pub fn check<'a>(
    packages: &'a [Package<'a>],
    entry_package: usize,
    entry_module: usize,
    mode: EntryMode,
    traces: bool,
    sources: &'a SourceMap,
    diags: &mut Diagnostics,
) -> Program {
    let mut cache = CheckCache::disabled();
    check_cached(packages, entry_package, entry_module, mode, traces, sources, diags, &mut cache)
        .expect("a check with no cancel flag finishes")
}

/// [`check`] through a cache of checked bodies: a body whose text, declarations, and registry state are unchanged
/// since a run the cache saw is not checked again. The result is the same program and the same diagnostics as
/// [`check`] gives (`spec/query.md`, QL4). Returns `None` when the cache's cancel flag was set before the run
/// finished; `diags` then holds nothing useful and the cache stays valid.
#[allow(clippy::too_many_arguments)]
pub fn check_cached<'a>(
    packages: &'a [Package<'a>],
    entry_package: usize,
    entry_module: usize,
    mode: EntryMode,
    traces: bool,
    sources: &'a SourceMap,
    diags: &mut Diagnostics,
    cache: &mut CheckCache,
) -> Option<Program> {
    let declarations = cache::declaration_fingerprint(packages, (entry_package, entry_module), mode, traces, sources);
    cache.begin(declarations);
    let mut sema = Sema::new(packages, entry_package, entry_module, mode, traces, sources);
    sema.declare_types(diags);
    sema.declare_values(diags);
    sema.resolve_imports(diags);
    sema.install_prelude();
    sema.declare_bounds(diags);
    sema.register_impls(diags);
    sema.register_user_derives(diags);
    sema.declare_identities(diags);
    sema.register_extensions(diags);
    let (functions, consts, closures) = sema.settle(diags, cache)?;
    let r = sema.finish(functions, consts, closures, diags);
    Some(r)
}
