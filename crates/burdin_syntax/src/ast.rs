//! The syntax tree. It keeps spans for diagnostics and decides nothing about names or types.

use burdin_diag::Span;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct File {
    pub items: Vec<Item>,
    pub uses: Vec<UseDecl>,
    /// `test` items. They are kept apart from the other items: only `burdin test` declares and checks them, so a
    /// normal build strips them from the program.
    pub tests: Vec<TestDecl>,
}

/// `test "name" { ... }`: a test block. The checker compiles it as an ordinary function with no parameters that
/// returns `Result<(), AssertError>`, whose body is the block's statements followed by `Ok(())`.
#[derive(Debug)]
pub struct TestDecl {
    /// The text of the string after `test`.
    pub name: String,
    pub name_span: Span,
    /// The function the test is compiled as.
    pub func: FnDecl,
    pub span: Span,
}

#[derive(Debug)]
pub enum Item {
    Fn(FnDecl),
    Const(ConstDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    TypeAlias(TypeAliasDecl),
    Impl(ImplBlock),
    Interface(InterfaceDecl),
    Extern(ExternBlock),
}

/// `use a.b.c`, `use a.b as x`, or `use a.b.{c, d}`.
#[derive(Debug)]
pub struct UseDecl {
    /// The module or item path, without the imported names of a brace list.
    pub path: Vec<Ident>,
    /// The name after `as`.
    pub alias: Option<Ident>,
    /// The names of a `.{...}` list.
    pub items: Vec<Ident>,
    pub span: Span,
}

/// `extern "C" { fn ... }`: C functions the program calls, declared without a body.
#[derive(Debug)]
pub struct ExternBlock {
    pub abi: String,
    pub abi_span: Span,
    pub is_priv: bool,
    pub decls: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Debug)]
pub struct FieldDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub ty: Type,
}

#[derive(Debug)]
pub struct StructDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub derives: Vec<Type>,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug)]
pub enum VariantKind {
    Unit,
    Tuple(Vec<Type>),
    Struct(Vec<FieldDecl>),
}

#[derive(Debug)]
pub struct Variant {
    pub name: Ident,
    pub kind: VariantKind,
}

#[derive(Debug)]
pub struct EnumDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub derives: Vec<Type>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Debug)]
pub struct TypeAliasDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub ty: Type,
    pub span: Span,
}

/// `interface Name requires A + B { fn ...  fn ... { default body } }`.
#[derive(Debug)]
pub struct InterfaceDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub requires: Vec<Type>,
    pub methods: Vec<InterfaceFn>,
    pub span: Span,
}

/// A method of an interface. A method with a body is a default method; otherwise `decl.body` is empty and
/// `has_body` is false.
#[derive(Debug)]
pub struct InterfaceFn {
    pub decl: FnDecl,
    pub has_body: bool,
}

/// `Type { fn ... }` or `Type: Interface { fn ... }`.
#[derive(Debug)]
pub struct ImplBlock {
    pub ty: Type,
    pub interface: Option<Type>,
    pub where_clause: Vec<WherePred>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Debug)]
pub struct ConstDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub ty: Type,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convention {
    /// No marker: a read-only borrow.
    Read,
    Mut,
    Own,
}

#[derive(Debug)]
pub struct FnDecl {
    pub is_priv: bool,
    pub is_unsafe: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub where_clause: Vec<WherePred>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug)]
pub struct GenericParam {
    pub name: Ident,
    pub bounds: Vec<Type>,
}

#[derive(Debug)]
pub struct WherePred {
    pub ty: Type,
    pub bounds: Vec<Type>,
}

#[derive(Debug)]
pub enum ParamKind {
    SelfParam,
    Named { name: Ident, ty: Type },
}

#[derive(Debug)]
pub struct Param {
    pub convention: Convention,
    pub kind: ParamKind,
    pub span: Span,
}

#[derive(Debug)]
pub struct PathSegment {
    pub name: Ident,
    pub args: Vec<Type>,
}

#[derive(Debug)]
pub enum TypeKind {
    Path(Vec<PathSegment>),
    SelfType,
    Unit,
    Tuple(Vec<Type>),
    Array(Box<Type>, Box<Expr>),
    Slice(Box<Type>),
    Ref { mutable: bool, ty: Box<Type> },
    Never,
}

#[derive(Debug)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Stmt {
    Let { pattern: Pattern, ty: Option<Type>, init: Expr, else_block: Option<Block>, span: Span },
    Expr(Expr),
}

#[derive(Debug)]
pub enum PatternKind {
    Wild,
    /// A plain identifier: a binding, or a unit variant or constant in scope (decided by name resolution).
    Binding {
        name: Ident,
        mutable: bool,
    },
    Tuple(Vec<Pattern>),
    Lit(LitPat),
    /// An inclusive range `lo..=hi`.
    Range(LitPat, LitPat),
    /// A path with more than one segment, such as `Shape.Circle`.
    Path(Vec<Ident>),
    /// `Path(p, ...)`, such as `Some(v)`.
    TupleStruct(Vec<Ident>, Vec<Pattern>),
    /// `Path { field, field: p, ... }`.
    Struct(Vec<Ident>, Vec<FieldPat>),
    Or(Vec<Pattern>),
    At(Ident, Box<Pattern>),
}

#[derive(Clone, Debug)]
pub enum LitPat {
    Int { value: u128, negative: bool },
    Char(char),
    Str(String),
    Bool(bool),
}

#[derive(Debug)]
pub struct FieldPat {
    pub name: Ident,
    /// `None` for the shorthand `name` (meaning `name: name`).
    pub pattern: Option<Pattern>,
}

#[derive(Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    BitOr,
    BitXor,
    BitAnd,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Deref,
    Ref,
    RefMut,
}

#[derive(Debug)]
pub struct Arg {
    /// The call-site `mut` marker.
    pub mut_marker: bool,
    /// The `mut` token, when the marker is written; diagnostics suggest removing it (E0115).
    pub mut_span: Option<Span>,
    pub expr: Expr,
}

#[derive(Debug)]
pub enum ExprKind {
    Int(u128),
    Float(String),
    Char(char),
    Str(String),
    Bool(bool),
    Unit,
    /// `name`, or a type path with generic arguments (`List<i32>`), which only means something as the left side
    /// of an associated call or a variant path (grammar rule 8.2).
    Name {
        name: Ident,
        args: Vec<Type>,
    },
    SelfValue,
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    ArrayRepeat(Box<Expr>, Box<Expr>),
    Block(Block),
    UnsafeBlock(Block),
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_expr: Option<Box<Expr>>,
    },
    While {
        cond: Box<Expr>,
        body: Block,
    },
    Loop(Block),
    For {
        pattern: Pattern,
        iter: Box<Expr>,
        body: Block,
    },
    Return(Option<Box<Expr>>),
    Break,
    Continue,
    Assign(Box<Expr>, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Cast(Box<Expr>, Type),
    Range {
        lo: Box<Expr>,
        hi: Box<Expr>,
        inclusive: bool,
    },
    OrElse(Box<Expr>, Box<Expr>),
    Try(Box<Expr>),
    Field(Box<Expr>, Ident),
    TupleField(Box<Expr>, u32, Span),
    Call(Box<Expr>, Vec<Arg>),
    Index(Box<Expr>, Box<Expr>),
    Match(Box<Expr>, Vec<MatchArm>),
    /// `Path { field: value, ... }`; the path is a type name or `Enum.Variant`.
    StructLit(Vec<Ident>, Vec<FieldInit>),
    IfLet {
        pattern: Pattern,
        scrutinee: Box<Expr>,
        then_block: Block,
        else_expr: Option<Box<Expr>>,
    },
}

#[derive(Debug)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` for the shorthand `name` (meaning `name: name`).
    pub value: Option<Expr>,
}

#[derive(Debug)]
pub struct MatchArm {
    /// `None` for the catch-all `else` arm.
    pub pattern: Option<Pattern>,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl Expr {
    /// Expressions that end with a block and can stand as a statement without a value (`if`, `while`, ...).
    pub fn is_block_like(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Block(_)
                | ExprKind::UnsafeBlock(_)
                | ExprKind::If { .. }
                | ExprKind::IfLet { .. }
                | ExprKind::While { .. }
                | ExprKind::Loop(_)
                | ExprKind::For { .. }
                | ExprKind::Match(..)
        )
    }
}
