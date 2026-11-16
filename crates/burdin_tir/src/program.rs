//! The whole program, and the signatures the memory checks read.

use crate::defs::{EnumDef, StructDef};
use crate::ops::FuncId;
use crate::tree::{Closure, Const, Expr, Function};
use crate::ty::Ty;

/// A type-checked program: the input of the memory checks and then of code generation.
pub struct Program {
    pub functions: Vec<Function>,
    /// The closure bodies, indexed by [`crate::tree::ExprKind::Closure`].
    pub closures: Vec<Closure>,
    pub consts: Vec<Const>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    /// One signature per function, in the same order as [`Program::functions`].
    pub signatures: Vec<Signature>,
    /// The C functions the program calls, in the order of its `extern "C"` declarations.
    pub externs: Vec<ExternFn>,
    /// `None` when the program has no `main` (already reported).
    pub main: Option<FuncId>,
    /// The program's `test` blocks, in declaration order. Only `burdin test` fills this in; a normal build has
    /// none, because test items are stripped before checking.
    pub tests: Vec<TestEntry>,
    /// The interface instances an `any` type names, indexed by [`AnyRef::id`].
    pub any_ifaces: Vec<AnyIface>,
    /// One vtable per (concrete type, interface instance) the program boxes, indexed by `ExprKind::BoxAny`.
    pub any_vtables: Vec<AnyVTable>,
    /// The types with a runtime identity (boxed concrete types and downcast targets), indexed by their id.
    pub any_type_ids: Vec<Ty>,
}

/// The vtable layout of one interface instance (`any I`), in the order of `spec/existentials.md`, AN2/AN11.
pub struct AnyIface {
    /// The display name, such as `Shape` or `Store<i32>`.
    pub name: String,
    /// One slot per method, in vtable order. The receiver is `sig.params[0]`.
    pub methods: Vec<AnyMethod>,
}

/// One vtable slot: the interface method's substituted signature. Every object-safe implementation matches it, so
/// one signature describes the slot for every boxed type.
#[derive(Clone)]
pub struct AnyMethod {
    pub name: String,
    pub sig: Signature,
    pub is_unsafe: bool,
}

/// One `(T, I)` vtable: the concrete type, its identity, and one function per interface method.
#[derive(Clone)]
pub struct AnyVTable {
    pub concrete: Ty,
    /// The concrete type's identity: an index into [`Program::any_type_ids`], shared by every interface.
    pub type_id: usize,
    /// The interface instance (an index into [`Program::any_ifaces`]).
    pub iface: usize,
    /// The concrete implementation of each slot of `iface`, in order.
    pub entries: Vec<FuncId>,
}

/// One `test "name" [attrs] { ... }` block, compiled as the function `func`.
pub struct TestEntry {
    pub name: String,
    pub func: FuncId,
    /// The validated attributes of the test item (spec/test-runner.md).
    pub attrs: TestAttrs,
}

/// The runner-visible attributes of a test item. `burdin_sema` validates them; the driver reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestAttrs {
    /// `ignore`: do not run the test unless `--ignored` or `--include-ignored` is passed.
    pub ignore: bool,
    /// `timeout(ms)`: override the runner's per-test timeout; `0` means no limit.
    pub timeout_ms: Option<u64>,
    /// `should_panic[(message)]`: the test must abort with a panic, and `message` must appear on stderr.
    pub should_panic: bool,
    pub should_panic_message: Option<String>,
    /// `should_fail`: the test must return an error instead of passing.
    pub should_fail: bool,
}

/// One `extern "C"` declaration. Its parameters are passed by value, as the C ABI expects.
pub struct ExternFn {
    /// The symbol to link against: the name as written in the `extern` block.
    pub symbol: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// A C variadic function (`...` after the named parameters): a call may pass extra C-ABI values.
    pub vararg: bool,
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
    /// The parameter's name as written, for named arguments (`spec/arguments.md`, AG4). `None` for `self` and
    /// for built-in functions and methods, which take no named arguments.
    pub name: Option<String>,
    /// The checked default value of a parameter written `name: Type = expr` (`spec/arguments.md`, AG1-AG3).
    /// `None` when the parameter is required. A default is a constant expression, so a call may insert it
    /// directly, and it may not mention a type parameter.
    pub default: Option<Expr>,
}

impl ParamSig {
    /// A plain read-only parameter: borrowed for the call, never dropped by the callee. Used for built-in
    /// functions and methods, whose arguments code generation evaluates directly.
    pub fn plain(ty: Ty) -> ParamSig {
        ParamSig { ty, by_ref: false, mut_param: false, own: false, exclusive: false, name: None, default: None }
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
