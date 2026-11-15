//! Function bodies: blocks, statements, expressions, and patterns.

use crate::ops::*;
use crate::ty::*;
use burdin_diag::Span;

/// A constant. Its value is expanded at every use, so it costs nothing at run time beyond the expression itself.
pub struct Const {
    pub name: String,
    pub value: Expr,
}

pub struct Function {
    pub name: String,
    /// Where the function was declared, for diagnostics about its signature.
    pub span: Span,
    pub params: Vec<LocalId>,
    pub ret: Ty,
    pub locals: Vec<Local>,
    pub body: Block,
    /// Every place read in a move context, as a local and a field path from it. The code generator
    /// allocates a runtime drop flag for each of them.
    pub moved: Vec<MovedPlace>,
    /// The parameter a second-class return value derives from (memory-rules.md, RT1). `None` when the result is
    /// static (or the return type is not second-class). The borrow checker uses it to keep the argument borrowed
    /// until the last use of the result (RF5).
    pub ret_origin: Option<usize>,
    /// A generic function or method checked against its bounds, with type parameters still in its types. It has
    /// no code: every use calls an instantiation, which is an ordinary function.
    pub symbolic: bool,
    /// An `extern "C" fn`: the function is exported under its plain name with the C ABI (issue #489).
    pub exported: bool,
}

/// A path step that walks through the allocation of a `Heap<T>`: `(*h).field` is `[MOVED_DEREF, field]`.
/// Constant array and `List` indices are ordinary numeric steps.
pub const MOVED_DEREF: usize = usize::MAX;

/// A place that is moved somewhere in a function: `local` followed by struct or tuple field indices, constant
/// array or `List` indices, and [`MOVED_DEREF`] steps for the fields of a `Heap` pointee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedPlace {
    pub local: LocalId,
    pub path: Vec<usize>,
    /// The move checker proved that the whole local is uninitialized at every point where it could be dropped:
    /// every path that leaves its scope, or an early `return`/`break`/`continue`, happens after the move. Code
    /// generation then needs neither a drop flag nor a drop for this local (issue #427).
    pub always_dead: bool,
}

/// How a local came to be, which decides whether and when it is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalKind {
    /// A read-only or `mut` parameter: borrowed, never dropped by the callee.
    BorrowedParam,
    /// An `own` parameter: owned by the callee and dropped when it returns.
    OwnParam,
    /// A `let` or pattern binding that takes ownership: dropped at the end of its scope.
    Owned,
    /// A pattern binding that is a view of a borrowed scrutinee: not dropped.
    View,
    /// The compiler-made local a `match` scrutinee is moved into when an arm's pattern moves parts of it. It owns
    /// its value and is dropped like an ordinary local, but a view it holds may borrow from a temporary, because
    /// the local lives no longer than the `match` statement.
    Scrutinee,
}

pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    /// A parameter passed by address (a `mut self` receiver): the local is the caller's place.
    pub by_ref: bool,
    pub kind: LocalKind,
}

pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

pub enum Stmt {
    /// `let pattern = init`, with the `else` block of `let ... else` (which diverges).
    Let(Pat, Expr, Option<Block>),
    Expr(Expr),
}

pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

pub enum ExprKind {
    Unit,
    Bool(bool),
    Int(i128),
    Float(f64),
    Char(char),
    Str(String),
    Local(LocalId),
    Const(ConstId),
    Call(FuncId, Vec<Expr>),
    /// A call to the C function `n` of [`crate::Program::externs`].
    ExternCall(usize, Vec<Expr>),
    Builtin(Builtin, Vec<Expr>),
    /// A built-in method: the receiver comes first in the arguments.
    Method(Method, Vec<Expr>),
    /// Borrows a `String` as a `str` (the one implicit coercion, tests/README.md assumption 2).
    StringAsStr(Box<Expr>),
    Block(Block),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    While(Box<Expr>, Block),
    Loop(Block),
    /// `for var in lo..hi` (or `lo..=hi`), with the endpoints written directly in the loop header. `var` is
    /// `None` for the `_` pattern.
    ForRange {
        var: Option<LocalId>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
        body: Block,
    },
    /// `for var in range`, where `range` is an ordinary `Range<T>` or `RangeInclusive<T>` value, evaluated once.
    /// The inclusive flag comes from the range's type.
    ForRangeValue {
        var: Option<LocalId>,
        range: Box<Expr>,
        body: Block,
    },
    /// `for pattern in array`: the array is evaluated once and its elements are bound in order.
    ForArray {
        pat: Pat,
        array: Box<Expr>,
        body: Block,
    },
    /// `for pattern in slice`: iterate a `[T]` view, whose length is known only at run time.
    ForSlice {
        pat: Pat,
        slice: Box<Expr>,
        body: Block,
    },
    /// `for pattern in list`: iterate a `List<T>`, borrowing it.
    ForList {
        pat: Pat,
        list: Box<Expr>,
        body: Block,
    },
    /// `for pattern in map`: iterate a `Map<K, V>` as `(key, value)` pairs, borrowing it.
    ForMap {
        pat: Pat,
        map: Box<Expr>,
        body: Block,
    },
    /// `for pattern in string.chars()`: iterate the characters of a `str`.
    ForChars {
        pat: Pat,
        string: Box<Expr>,
        body: Block,
    },
    Match(Box<Expr>, Vec<Arm>),
    Break,
    Continue,
    Return(Option<Box<Expr>>),
    /// The left side is a place: a local, or an index into a place.
    Assign(Box<Expr>, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    /// Converts the operand to this expression's type.
    Cast(Box<Expr>),
    /// `Type.size()` or `Type.align()`: the C layout size or alignment of the type in bytes.
    LayoutOf {
        ty: Ty,
        alignment: bool,
    },
    /// `lo..hi` or `lo..=hi`: a `Range<T>` or `RangeInclusive<T>` value.
    Range {
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
    },
    /// `*h` on a `Heap<T>`: the pointee.
    Deref(Box<Expr>),
    /// `*p` on a `CPtr<T>`: the value at the raw address. It is a place, so `*p = v` stores through it.
    PtrDeref(Box<Expr>),
    /// `&place` or `&mut place`: the address of the place (a temporary gets a slot of its own).
    Ref {
        place: Box<Expr>,
        mutable: bool,
    },
    /// `*r` on a `&T` or `&mut T`: the referent.
    DerefRef(Box<Expr>),
    /// `base[lo..hi]`: a view of bytes or elements of the base.
    Slice {
        base: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    Array(Vec<Expr>),
    ArrayRepeat(Box<Expr>, u64),
    Index(Box<Expr>, Box<Expr>),
    Tuple(Vec<Expr>),
    /// A struct literal of this expression's type: field indices with their values, in source order.
    Struct(Vec<(usize, Expr)>),
    /// A variant of this expression's enum type, with field indices and values in source order.
    Variant(usize, Vec<(usize, Expr)>),
    Some(Box<Expr>),
    None,
    /// `Ok(value)` of a `Result<T, E>`.
    Ok(Box<Expr>),
    /// `Err(value)` of a `Result<T, E>`.
    Err(Box<Expr>),
    /// Field `n` of a struct or tuple.
    Field(Box<Expr>, usize),
}

pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

/// How a pattern binding takes its value from the scrutinee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindMode {
    /// The binding takes ownership of the value, which is moved out of an owned scrutinee.
    Move,
    /// The value is a Copy type, so it is copied and the scrutinee is untouched.
    Copy,
    /// A read-only view of a borrowed scrutinee; nothing is moved and the binding is not dropped.
    View,
}

pub enum Pat {
    Wild,
    Binding(LocalId, BindMode),
    /// `name @ pattern`.
    At(LocalId, BindMode, Box<Pat>),
    Bool(bool),
    Int(i128),
    Char(char),
    /// An inclusive range of integers or characters (as their code points).
    Range(i128, i128),
    Or(Vec<Pat>),
    Some(Box<Pat>),
    None,
    /// `Ok(pattern)` of a `Result`.
    Ok(Box<Pat>),
    /// `Err(pattern)` of a `Result`.
    Err(Box<Pat>),
    /// A tuple or struct: one pattern per field, in declaration order.
    Product(Vec<Pat>),
    /// A variant of an enum: its index and one pattern per field, in declaration order.
    Variant(usize, Vec<Pat>),
}
