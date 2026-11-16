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

/// `test "name" [attrs] { ... }`: a test block. The checker compiles it as an ordinary function with no parameters
/// that returns `Result<(), AssertError>`, whose body is the block's statements followed by `Ok(())`.
#[derive(Debug)]
pub struct TestDecl {
    /// The text of the string after `test`.
    pub name: String,
    pub name_span: Span,
    /// The attributes in `[...]`, as written. The checker validates the names, values, and combinations.
    pub attrs: Vec<TestAttr>,
    /// The function the test is compiled as.
    pub func: FnDecl,
    /// The test block itself, renamed `__burdin_test_N_body`. The wrapper in `func` calls it and prints the error
    /// before returning it (spec/errors.md, ERR9). `burdin test` declares both functions.
    pub body_func: FnDecl,
    pub span: Span,
}

/// One attribute of a test item: `ignore`, `timeout(500)`, `should_panic`, `should_panic("text")`, or
/// `should_fail`. The parser accepts any identifier with an optional integer or string value; names,
/// values, duplicates, and combinations are checked in `burdin_sema` (E0170).
#[derive(Debug)]
pub struct TestAttr {
    pub name: Ident,
    pub value: Option<TestAttrValue>,
}

#[derive(Debug, Clone)]
pub enum TestAttrValue {
    Int(u128),
    Str(String),
}

#[derive(Debug)]
pub enum Item {
    Fn(FnDecl),
    /// `fn Type.name(self, ...) { ... }`: an extension method (`spec/extensions.md`). The receiver type is
    /// resolved in the declaring module; the function itself is an ordinary one whose first parameter is
    /// `self`.
    Extension(ExtensionDecl),
    Const(ConstDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    /// `error enum ...`: lowered to an ordinary enum plus generated impls by `crate::error_item`, so later stages
    /// never see this variant (spec/errors.md).
    ErrorEnum(ErrorEnumDecl),
    TypeAlias(TypeAliasDecl),
    Impl(ImplBlock),
    Interface(InterfaceDecl),
    Extern(ExternBlock),
    /// `extern "C" fn ... { ... }`: a Burdin function exported with the C ABI (issue #489).
    ExternFn(ExternFnDecl),
}

/// `fn Type.name(self, ...)`: an extension method. `func` holds the signature and body exactly as an ordinary
/// function's; `recv` is the receiver type written before the method name, including the type parameters the
/// receiver declares (`fn List<T>.sum(self)`).
#[derive(Debug)]
pub struct ExtensionDecl {
    pub recv: Type,
    pub func: FnDecl,
}

#[derive(Debug)]
pub struct ExternFnDecl {
    pub abi: String,
    pub abi_span: Span,
    pub decl: FnDecl,
}

/// `use a.b.c`, `use a.b as x`, or `use a.b.{c, d}`. A `pub use` also re-exports what it imports.
#[derive(Debug)]
pub struct UseDecl {
    pub is_pub: bool,
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
    /// `repr(C)`: the struct has the C layout and crosses the C ABI by value.
    pub repr_c: bool,
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

/// `error enum Name { Variant(payload) from "message", ... }`: an enum whose variants carry a required message
/// template and an optional `from` conversion (spec/errors.md, ERR1).
#[derive(Debug)]
pub struct ErrorEnumDecl {
    pub is_priv: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub derives: Vec<Type>,
    pub variants: Vec<ErrorVariant>,
    pub span: Span,
}

#[derive(Debug)]
pub struct ErrorVariant {
    pub name: Ident,
    pub kind: VariantKind,
    /// The `from` marker, when written. Its span is where a diagnostic about the conversion is reported.
    pub from: Option<Span>,
    /// The message template: a string literal with `{...}` placeholders that name the payload fields.
    pub message: Vec<InterpPart>,
    /// The span of the whole message string literal.
    pub message_span: Span,
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
    /// `...` after the named parameters of an `extern` declaration.
    pub is_vararg: bool,
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
    /// `= expr` after the type: the value a call may omit (`spec/arguments.md`, AG1). The checker validates it;
    /// a `self` parameter cannot have one.
    pub default: Option<Expr>,
    /// `comptime name: Type`: the argument is a compile-time value and the function is specialized for it
    /// (`spec/comptime.md`, CT4). The span is the `comptime` keyword.
    pub comptime: Option<Span>,
    pub span: Span,
}

#[derive(Debug)]
pub struct PathSegment {
    pub name: Ident,
    pub args: Vec<Type>,
}

/// The capability family of a closure type (`Fn`, `FnMut`, `FnOnce`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKind {
    Fn,
    FnMut,
    FnOnce,
}

#[derive(Debug)]
pub enum TypeKind {
    Path(Vec<PathSegment>),
    /// `any Interface`: the owned boxed existential (spec/existentials.md).
    Any(Box<Type>),
    /// `impl Interface`. Only a parameter list accepts it; the parser turns it into a hidden generic parameter,
    /// so anywhere else it reaches the checker as written and is rejected (spec/existentials.md, AN4).
    Impl(Box<Type>),
    /// `Fn(A, B): R`, `FnMut(A): R`, or `FnOnce(): R`.
    Fn {
        kind: FnKind,
        params: Vec<Type>,
        ret: Option<Box<Type>>,
    },
    SelfType,
    Unit,
    Tuple(Vec<Type>),
    Array(Box<Type>, Box<Expr>),
    Slice(Box<Type>),
    Ref {
        mutable: bool,
        ty: Box<Type>,
    },
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
    /// The parameter name of a named argument (`f(name: value)`), or `None` for a positional one
    /// (`spec/arguments.md`, AG4).
    pub name: Option<Ident>,
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
    /// A string literal with `{...}` placeholders (spec/format.md).
    Interp(Vec<InterpPart>),
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
    /// `recv.name<T>`: type arguments on a member, as in `x.downcast<Circle>()` (spec/existentials.md, AN6).
    TypeArgs(Box<Expr>, Vec<Type>),
    TupleField(Box<Expr>, u32, Span),
    Call(Box<Expr>, Vec<Arg>),
    Index(Box<Expr>, Box<Expr>),
    Match(Box<Expr>, Vec<MatchArm>),
    /// `|a, b| body`, `|a: i32|: bool body`, or `|| body`.
    Closure {
        params: Vec<ClosureParam>,
        /// The return type after the closing `|`, if written.
        ret: Option<Type>,
        body: Box<Expr>,
    },
    /// `Path { field: value, ... }`; the path is a type name or `Enum.Variant`.
    StructLit(Vec<Ident>, Vec<FieldInit>),
    IfLet {
        pattern: Pattern,
        scrutinee: Box<Expr>,
        then_block: Block,
        else_expr: Option<Box<Expr>>,
    },
    /// `comptime E`: evaluated while compiling (`spec/comptime.md`, CT2). `comptime if` (CT5) and `comptime for`
    /// (CT6) are this node around an `If` or a `For`.
    Comptime(Box<Expr>),
    /// `comptime assert(condition)` or `comptime assert(condition, "message")` (CT8).
    ComptimeAssert(Box<Expr>, Option<Box<Expr>>),
}

/// One piece of an interpolated string literal.
#[derive(Debug)]
pub enum InterpPart {
    Lit(String),
    Hole(Hole),
}

/// A `{arg:spec}` placeholder.
#[derive(Debug)]
pub struct Hole {
    pub arg: HoleArg,
    pub spec: FormatSpec,
    /// The placeholder including its braces.
    pub span: Span,
}

#[derive(Debug)]
pub enum HoleArg {
    /// `{}`: the next positional argument of a format call.
    Next,
    /// `{0}`: the positional argument at this index.
    Index(usize),
    /// `{name}`, `{a + b}`: any expression, evaluated where the string is.
    Expr(Box<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Not written: left for text, right for numbers.
    Default,
    Left,
    Center,
    Right,
}

/// The type letter of a specifier: which interface formats the value, and the radix for integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecKind {
    Display,
    Debug,
    LowerHex,
    UpperHex,
    Binary,
    Octal,
}

/// The parsed specifier after the `:` of a placeholder: `[[fill]align][+][#][0][width][.precision][type]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatSpec {
    pub fill: char,
    pub align: Align,
    pub plus: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: Option<usize>,
    pub precision: Option<usize>,
    pub kind: SpecKind,
    pub span: Span,
}

impl FormatSpec {
    pub fn plain(span: Span) -> FormatSpec {
        FormatSpec {
            fill: ' ',
            align: Align::Default,
            plus: false,
            alt: false,
            zero: false,
            width: None,
            precision: None,
            kind: SpecKind::Display,
            span,
        }
    }

    /// Whether nothing but the interface is chosen, so the formatter keeps its default settings.
    pub fn is_default(&self) -> bool {
        self.fill == ' '
            && self.align == Align::Default
            && !self.plus
            && !self.alt
            && !self.zero
            && self.width.is_none()
            && self.precision.is_none()
            && matches!(self.kind, SpecKind::Display | SpecKind::Debug)
    }
}

/// One parameter of a closure: a name and an optional type (`|x: i32, y|`).
#[derive(Debug)]
pub struct ClosureParam {
    pub name: Ident,
    pub ty: Option<Type>,
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
