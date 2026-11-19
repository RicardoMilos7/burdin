//! The query-based compiler library (`spec/query.md`): file texts in, diagnostics and typed programs out, with
//! everything an edit cannot affect reused. The command line and a language server share it.
//!
//! ```text
//! Db::open / Db::from_files      the inputs: a package on disk, or files in memory
//! db.set_file(path, text)        an edit
//! db.analyze()                   parse -> declare -> check bodies -> memory checks -> compile-time evaluation
//! ```
//!
//! The crate sits beside the compiler stages, not inside them: it depends on `burdin_syntax`, `burdin_sema`,
//! `burdin_check`, and `burdin_comptime`, never on code generation, so a tool that wants the checker does not link
//! LLVM.

pub mod api;
mod db;
mod memory;
pub mod navigate;
pub mod package;
mod reparse;
mod stats;

pub use api::{
    Convention, DefinitionKind, ExprFacts, ItemSignature, LocalUse, Location, Member, MoveEvent, OwnedLocal, Ownership,
    ParamInfo, Resolution, StmtInfo, StmtKind, SyntaxTree, Trivia, TriviaKind, Typed,
};
pub use burdin_sema::EntryMode;
pub use db::{Analysis, Budget, CancelHandle, Cancelled, Db, Options, Parsed, env_jobs};
pub use navigate::{CallSite, FunctionInfo, LocalInfo, ModuleInfo, Occurrence, PackageInfo, TestInfo};
pub use stats::{Counters, RevisionStats};
