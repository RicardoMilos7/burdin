//! The typed tree: the output of checking, the input of code generation. Names are resolved to ids and every
//! expression carries its type and its source span (code generation uses the span for panic locations).

use burdin_diag::Span;
use std::fmt;
use std::rc::Rc;

/// A user-defined struct or enum: its index in [`Program::structs`] or [`Program::enums`], and its name for messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdtRef {
    pub id: usize,
    pub name: Rc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
}

/// Every supported target is 64-bit, so `isize` and `usize` are 64 bits wide.
pub const POINTER_BITS: u32 = 64;

impl IntTy {
    pub fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64 | IntTy::Isize)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntTy::I8 | IntTy::U8 => 8,
            IntTy::I16 | IntTy::U16 => 16,
            IntTy::I32 | IntTy::U32 => 32,
            IntTy::I64 | IntTy::U64 => 64,
            IntTy::Isize | IntTy::Usize => POINTER_BITS,
        }
    }

    pub fn min(self) -> i128 {
        if self.signed() { -(1i128 << (self.bits() - 1)) } else { 0 }
    }

    pub fn max(self) -> i128 {
        if self.signed() { (1i128 << (self.bits() - 1)) - 1 } else { (1i128 << self.bits()) - 1 }
    }

    pub fn name(self) -> &'static str {
        match self {
            IntTy::I8 => "i8",
            IntTy::I16 => "i16",
            IntTy::I32 => "i32",
            IntTy::I64 => "i64",
            IntTy::Isize => "isize",
            IntTy::U8 => "u8",
            IntTy::U16 => "u16",
            IntTy::U32 => "u32",
            IntTy::U64 => "u64",
            IntTy::Usize => "usize",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatTy {
    F32,
    F64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Unit,
    Bool,
    Char,
    Int(IntTy),
    Float(FloatTy),
    Str,
    String,
    Array(Box<Ty>, u64),
    /// The prelude's `Option<T>`. Only matching on it is supported so far (milestone 2 needs `checked_add`).
    Option(Box<Ty>),
    /// A tuple of two or more elements (or one, written with a trailing comma).
    Tuple(Vec<Ty>),
    Struct(AdtRef),
    Enum(AdtRef),
    Never,
    /// The type of an expression that already produced an error; it matches everything to avoid cascades.
    Error,
}

impl Ty {
    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int(_))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int(_) | Ty::Float(_))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unit => f.write_str("()"),
            Ty::Bool => f.write_str("bool"),
            Ty::Char => f.write_str("char"),
            Ty::Int(i) => f.write_str(i.name()),
            Ty::Float(FloatTy::F32) => f.write_str("f32"),
            Ty::Float(FloatTy::F64) => f.write_str("f64"),
            Ty::Str => f.write_str("str"),
            Ty::String => f.write_str("String"),
            Ty::Array(t, n) => write!(f, "[{t}; {n}]"),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Tuple(elems) => {
                f.write_str("(")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{t}")?;
                }
                if elems.len() == 1 {
                    f.write_str(",")?;
                }
                f.write_str(")")
            }
            Ty::Struct(a) | Ty::Enum(a) => f.write_str(&a.name),
            Ty::Never => f.write_str("!"),
            Ty::Error => f.write_str("{error}"),
        }
    }
}

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

pub struct Program {
    pub functions: Vec<Function>,
    pub consts: Vec<Const>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub main: FuncId,
}

/// The built-in interfaces a type derives.
#[derive(Clone, Copy, Debug, Default)]
pub struct Derives {
    pub copy: bool,
    pub clone: bool,
    pub eq: bool,
    pub ord: bool,
    pub hash: bool,
    pub default: bool,
}

pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
}

pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub derives: Derives,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantShape {
    Unit,
    Tuple,
    Struct,
}

pub struct VariantDef {
    pub name: String,
    pub shape: VariantShape,
    /// The fields of a tuple variant are named `0`, `1`, ...
    pub fields: Vec<FieldDef>,
}

pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
    pub derives: Derives,
}

/// A constant. Its value is expanded at every use, so it costs nothing at run time beyond the expression itself.
pub struct Const {
    pub name: String,
    pub value: Expr,
}

pub struct Function {
    pub name: String,
    pub params: Vec<LocalId>,
    pub ret: Ty,
    pub locals: Vec<Local>,
    pub body: Block,
}

pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    /// A parameter passed by address (a `mut self` receiver): the local is the caller's place.
    pub by_ref: bool,
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
    Builtin(Builtin, Vec<Expr>),
    /// A built-in method: the receiver comes first in the arguments.
    Method(Method, Vec<Expr>),
    /// Borrows a `String` as a `str` (the one implicit coercion, tests/README.md assumption 2).
    StringAsStr(Box<Expr>),
    Block(Block),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    While(Box<Expr>, Block),
    Loop(Block),
    /// `for var in lo..hi` (or `lo..=hi`). `var` is `None` for the `_` pattern.
    ForRange {
        var: Option<LocalId>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
        body: Block,
    },
    /// `for pattern in array`: the array is evaluated once and its elements are bound in order.
    ForArray {
        pat: Pat,
        array: Box<Expr>,
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
    /// Field `n` of a struct or tuple.
    Field(Box<Expr>, usize),
}

pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

pub enum Pat {
    Wild,
    Binding(LocalId),
    /// `name @ pattern`.
    At(LocalId, Box<Pat>),
    Bool(bool),
    Int(i128),
    Char(char),
    /// An inclusive range of integers or characters (as their code points).
    Range(i128, i128),
    Or(Vec<Pat>),
    Some(Box<Pat>),
    None,
    /// A tuple or struct: one pattern per field, in declaration order.
    Product(Vec<Pat>),
    /// A variant of an enum: its index and one pattern per field, in declaration order.
    Variant(usize, Vec<Pat>),
}
