//! Function bodies: blocks, statements, expressions, and patterns.

use crate::ops::*;
use crate::program::ParamSig;
use crate::ty::*;
use burdin_diag::Span;

/// A constant. Its initializer is evaluated by the comptime engine (`spec/comptime.md`, CT7), which replaces
/// `value` with the materialized result; that value is expanded at every use, so it costs nothing at run time
/// beyond the expression itself.
#[derive(Clone, Debug)]
pub struct Const {
    pub name: String,
    /// Where the constant was declared, for diagnostics about its evaluation.
    pub span: Span,
    pub value: Expr,
    /// The locals the initializer declares (a block, a `let`, a closure capture). Empty once it is evaluated.
    pub locals: Vec<Local>,
}

#[derive(Clone, Debug)]
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
    /// Where a whole local is unconditionally moved out or dropped, so the debug information can end its
    /// location there (`spec/debug.md`, DBG4). A move the checker cannot prove unconditional — one inside a
    /// conditional branch, or inside a loop over a local that was already in scope there — is not listed, and
    /// the local keeps showing its (stale) bytes, which the drop flag is the honest answer for.
    pub debug_kills: Vec<DebugKill>,
    /// The parameters a second-class return value derives from (memory-rules.md, RT1, RT3a). Empty when the
    /// result is static (or the return type is not second-class). The borrow checker uses them to keep the
    /// arguments borrowed until the last use of the result (RF5).
    pub ret_origins: Vec<usize>,
    /// A generic function or method checked against its bounds, with type parameters still in its types. It has
    /// no code: every use calls an instantiation, which is an ordinary function.
    pub symbolic: bool,
    /// An `extern "C" fn`: the function is exported under its plain name with the C ABI (issue #489).
    pub exported: bool,
    /// Every ownership fact of the body, in source order (`spec/check.md`, CK10). The command line's
    /// `--explain-ownership`, the language server's ownership lens, and the REPL render these; no tool recomputes
    /// them.
    pub events: Vec<OwnershipEvent>,
}

/// A whole local whose location the debug information ends after `span`: the statement that moved it out
/// (`spec/debug.md`, DBG4). A debugger then shows `<optimized out>` instead of the stale bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugKill {
    pub local: LocalId,
    pub span: Span,
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

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    /// A parameter passed by address (a `mut self` receiver): the local is the caller's place.
    pub by_ref: bool,
    pub kind: LocalKind,
    /// The name where the binding is written (a `let`, a pattern, a parameter), for the linter's diagnostics. A
    /// local the compiler made up has an empty span (`lo == hi`).
    pub span: Span,
}

/// How a closure takes a captured variable (`spec/memory-rules.md` CL2-CL4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    /// A shared borrow (`&T`).
    Borrow,
    /// A mutable borrow (`&mut T`), because the body writes the captured place.
    BorrowMut,
    /// The value itself: a move capture, or a copy for a Copy type.
    Move,
}

/// One entry of a closure's capture record. In a [`Closure`] the local is the body's local for the capture; in an
/// [`ExprKind::Closure`] it is the enclosing function's local that is captured.
#[derive(Clone, Debug)]
pub struct Capture {
    pub local: LocalId,
    pub mode: CaptureMode,
    pub ty: Ty,
}

/// A closure body and the capture record it takes.
#[derive(Clone, Debug)]
pub struct Closure {
    /// The internal function name, `burdin.closure.N`.
    pub name: String,
    pub span: Span,
    pub kind: FnKind,
    pub sig: FnSig,
    /// The capture record layout: entry `i` is env field `i`.
    pub captures: Vec<Capture>,
    /// The body. Its locals include each capture (at `Capture::local`), then its parameters.
    pub func: Function,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    /// The closing `}` of the block, where the values its scope owns are dropped (DR1-DR2). A block the compiler
    /// synthesized without source text has an empty span at the expression it replaced.
    pub close: Span,
}

impl Block {
    /// A block with no source of its own, for compiler-generated bodies.
    pub fn synthetic(stmts: Vec<Stmt>, tail: Option<Box<Expr>>, close: Span) -> Block {
        Block { stmts, tail, close }
    }
}

/// How an argument is passed at a call (`spec/memory-rules.md`, CV1-CV3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallConvention {
    /// A read-only borrow: no marker.
    Read,
    /// `mut`: borrowed exclusively for the call.
    Mut,
    /// `own`: the value is moved into the callee.
    Own,
}

/// What one ownership event is (`spec/check.md`, CK10; `spec/memory-rules.md`). The list is stable data the command
/// line, the language server, and the REPL all render, so none of them has to re-derive the facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipEventKind {
    /// The local comes into being: a `let` or pattern binding, or a parameter.
    Declare,
    /// A non-Copy place is moved out. `target` names the destination when the move has one: the local a `let`
    /// binds, the parameter an `own` argument fills, or the place an assignment writes.
    Move { target: Option<String> },
    /// A Copy place is read in a move context.
    Copy,
    /// A shared borrow is taken (`&place`, a slice, `as_str`, a `for` loop, or an argument the callee borrows).
    BorrowRead,
    /// An exclusive borrow is taken (`&mut place`, a `mut` argument, or a mutating method's receiver).
    BorrowMut,
    /// The value is dropped when its scope ends. `conditional` is set when the compiler keeps a runtime drop flag
    /// because the value may already have been moved on some path (DR7).
    Drop { conditional: bool },
    /// A runtime drop flag is kept for this move, which happens on only some paths (DR7).
    DropFlag,
    /// The convention of one argument at a call (`spec/memory-rules.md`, CV).
    CallArg { parameter: String, convention: CallConvention },
    /// `.clone()` on a value whose `Clone` the compiler derives (`spec/derives.md`).
    Clone { derived: bool },
}

/// One fact about the ownership of one place in a function body (`spec/check.md`, CK10). Events are in source
/// order, one per place they concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnershipEvent {
    pub span: Span,
    /// The local the event is about; `None` for a `CallArg` event on a temporary argument.
    pub local: Option<LocalId>,
    /// Field indices from the local to the place; empty is the whole local. [`MOVED_DEREF`] steps walk through a
    /// `Heap<T>` pointee.
    pub path: Vec<usize>,
    pub kind: OwnershipEventKind,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let pattern = init`, with the `else` block of `let ... else` (which diverges).
    Let(Pat, Expr, Option<Block>),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug)]
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
    /// A closure expression: the index of its body in [`crate::Program::closures`], and the enclosing places it
    /// captures, in capture-record order. The expression's type is the closure interface it satisfies.
    Closure {
        closure: usize,
        captures: Vec<Capture>,
    },
    /// `f(args)` where `f` is a closure value. The kind decides how the callee is used (CL6).
    ClosureCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
        kind: FnKind,
    },
    /// `value` moved (or copied) into a fresh heap allocation, paired with `vtable` (an index into
    /// [`crate::Program::any_vtables`]). The expression's type is `any I` (spec/existentials.md, AN1/AN3).
    BoxAny {
        value: Box<Expr>,
        vtable: usize,
    },
    /// A method call through an `any` value's vtable: `iface` indexes [`crate::Program::any_ifaces`], `slot` its
    /// method list, and `params` are the method's parameter conventions without the receiver (AN5).
    AnyCall {
        recv: Box<Expr>,
        iface: usize,
        slot: usize,
        params: Vec<ParamSig>,
        args: Vec<Expr>,
    },
    /// `x.is<T>()` on an `any` value: whether the box holds exactly `target` (AN6). `type_id` indexes
    /// [`crate::Program::any_type_ids`].
    AnyIs {
        value: Box<Expr>,
        target: Ty,
        type_id: usize,
    },
    /// `x.downcast<T>()` on an `any` value: the value, or `None` (AN6). The receiver is consumed.
    AnyDowncast {
        value: Box<Expr>,
        target: Ty,
        type_id: usize,
    },
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
    /// A loop with a label (`'label: while ...`). The body is one of the loop expressions; `break 'label` and
    /// `continue 'label` inside it name this loop, and an unlabeled `break` or `continue` names the innermost loop.
    Labeled(String, Box<Expr>),
    /// `break`, `break 'label`, or `break value`: leave the loop with that label, giving it `value`. Only a
    /// `loop` takes a value (`spec/grammar.md`, rule 8.10).
    Break {
        label: Option<String>,
        value: Option<Box<Expr>>,
    },
    /// `continue`, or `continue 'label` to go on with the loop with that label.
    Continue(Option<String>),
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
    /// `base[lo..hi]` (or `base[lo..=hi]`): a view of bytes or elements of the base.
    Slice {
        base: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
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
    /// `comptime E` (`spec/comptime.md`, CT2): evaluated while compiling and replaced by its value. A `comptime
    /// if` is an [`ExprKind::If`] whose condition is this node; an argument for a `comptime` parameter is one too
    /// (CT4). No comptime node reaches code generation.
    Comptime(Box<Expr>),
    /// `comptime for pattern in iter { body }` (CT6): `iter` is a comptime expression and the body is repeated
    /// once per item with the pattern bound to it. The pattern binds owned items, like [`ExprKind::ForArray`].
    ComptimeFor {
        pat: Pat,
        iter: Box<Expr>,
        body: Block,
    },
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Binding(LocalId, BindMode),
    /// `name @ pattern`.
    At(LocalId, BindMode, Box<Pat>),
    Bool(bool),
    Int(i128),
    Char(char),
    /// A string literal, compared byte for byte against a `str` scrutinee.
    Str(String),
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
