//! The typed tree: the output of checking, the input of the memory checks and of code generation. Names are
//! resolved to ids and every expression carries its type and its source span (code generation uses the span for
//! panic locations).

pub mod defs;
pub mod ioerror;
pub mod ops;
pub mod place;
pub mod program;
pub mod query;
pub mod target;
pub mod tree;
pub mod ty;
pub mod visit;

pub use defs::*;
pub use ops::*;
pub use program::*;
pub use query::TypeDefs;
pub use tree::*;
pub use ty::*;

/// The operating system programs are compiled for, as `target.os()` reports it: the host's unless `--target` named
/// another platform (`spec/comptime.md`, CT5; `spec/cross-compilation.md`).
pub fn target_os() -> &'static str {
    target::current().os_name()
}

/// The processor architecture programs are compiled for, as `target.arch()` reports it.
pub fn target_arch() -> &'static str {
    target::current().arch_name()
}
