//! The typed tree: the output of checking, the input of code generation. Names are resolved to ids and every
//! expression carries its type.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Unit,
    Bool,
    Str,
    Never,
    /// The type of an expression that already produced an error; it matches everything to avoid cascades.
    Error,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Ty::Unit => "()",
            Ty::Bool => "bool",
            Ty::Str => "str",
            Ty::Never => "!",
            Ty::Error => "{error}",
        };
        f.write_str(s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuncId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Print,
    Println,
    Eprint,
    Eprintln,
}

pub struct Program {
    pub functions: Vec<Function>,
    pub main: FuncId,
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
}

pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

pub enum Stmt {
    Let(LocalId, Expr),
    Expr(Expr),
}

pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
}

pub enum ExprKind {
    Unit,
    Bool(bool),
    Str(String),
    Local(LocalId),
    Call(FuncId, Vec<Expr>),
    Builtin(Builtin, Vec<Expr>),
    Block(Block),
}
