//! Ids, built-in functions and methods, and operators.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuncId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprint,
    Eprintln,
    Panic,
    /// `String.new()`: an empty string.
    StringNew,
    /// `String.from(str)`: a new string with a copy of the bytes.
    StringFrom,
    /// `Heap.new(value)`: allocates and moves `value` into a fresh heap allocation.
    HeapNew,
}

/// Built-in methods of the primitive types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ToString,
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    CheckedAdd,
    CheckedSub,
    CheckedMul,
    Min,
    Max,
    Abs,
    /// `String.push_str(str)`: appends bytes, growing the buffer.
    PushStr,
    /// `String.as_str()`: a `str` view of the bytes.
    AsStr,
    /// `x.len()` on `str`, `[T]`, `String`, and arrays.
    Len,
    /// `s.starts_with(prefix)` on `str` (and `String`).
    StartsWith,
    /// `s.find(c): Option<usize>` on `str` (and `String`); a byte index.
    Find,
    /// `x.clone()`: a deep copy derived by the compiler.
    Clone,
    /// `x.hash(): u64` of a type that derives `Hash`.
    Hash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}
