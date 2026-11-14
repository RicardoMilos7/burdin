//! The typed tree: the output of checking, the input of the memory checks and of code generation. Names are
//! resolved to ids and every expression carries its type and its source span (code generation uses the span for
//! panic locations).

pub mod defs;
pub mod ops;
pub mod place;
pub mod program;
pub mod query;
pub mod tree;
pub mod ty;

pub use defs::*;
pub use ops::*;
pub use program::*;
pub use query::TypeDefs;
pub use tree::*;
pub use ty::*;
