//! The whole program, and the signatures the memory checks read.

use crate::defs::{EnumDef, StructDef};
use crate::ops::FuncId;
use crate::tree::{Const, Function};
use crate::ty::Ty;

/// A type-checked program: the input of the memory checks and then of code generation.
pub struct Program {
    pub functions: Vec<Function>,
    pub consts: Vec<Const>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    /// One signature per function, in the same order as [`Program::functions`].
    pub signatures: Vec<Signature>,
    /// `None` when the program has no `main` (already reported).
    pub main: Option<FuncId>,
}

#[derive(Clone)]
pub struct ParamSig {
    pub ty: Ty,
    /// The callee sees the caller's place: every parameter except an `own` one (a read-only or `mut` parameter,
    /// and every receiver except `own self`). Passed by address.
    pub by_ref: bool,
    /// A named `mut` parameter, which the call site writes as `f(mut x)`. (`mut self` needs no marker.)
    pub mut_param: bool,
    /// An `own` parameter (including `own self`): the callee takes ownership and drops the value.
    pub own: bool,
    /// Borrowed exclusively for the duration of the call (`mut` and `own` parameters, `mut self`), so no other
    /// argument of the same call may touch an overlapping place (EX1).
    pub exclusive: bool,
}

impl ParamSig {
    /// A plain read-only parameter: borrowed for the call, never dropped by the callee. Used for built-in
    /// functions and methods, whose arguments code generation evaluates directly.
    pub fn plain(ty: Ty) -> ParamSig {
        ParamSig { ty, by_ref: false, mut_param: false, own: false, exclusive: false }
    }
}

#[derive(Clone)]
pub struct Signature {
    pub params: Vec<ParamSig>,
    pub ret: Ty,
    /// The first parameter is `self`.
    pub has_self: bool,
    /// Which parameter a second-class return value derives from (memory-rules.md, RT1). `None` when the result
    /// is static. Inferred from the body by the return-origin check.
    pub ret_origin: Option<usize>,
}

impl Signature {
    /// What a signature is before its types are resolved.
    pub fn placeholder() -> Signature {
        Signature { params: Vec::new(), ret: Ty::Error, has_self: false, ret_origin: None }
    }
}
