//! Tokens produced by the lexer (spec/grammar.md, section 1).

use burdin_diag::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    /// A loop label, `'name` (spec/grammar.md, section 1). The name is kept without the quote.
    Label(String),
    /// Integer literal. The value is exact; whether it fits its type is checked later.
    Int(u128),
    /// Float literal, kept as written (without `_`).
    Float(String),
    Char(char),
    Str(String),
    /// A string literal with `{...}` placeholders (spec/format.md): its literal text and holes in order.
    Interp(Vec<InterpPiece>),

    // Keywords
    Any,
    As,
    Break,
    Comptime,
    Const,
    Continue,
    Else,
    Enum,
    Error,
    Extern,
    False,
    Fn,
    For,
    If,
    Impl,
    In,
    Interface,
    Let,
    Loop,
    Match,
    Mut,
    Orelse,
    Own,
    Priv,
    Return,
    SelfValue,
    SelfType,
    Struct,
    True,
    Type,
    Unsafe,
    Use,
    Where,
    While,
    /// A word reserved for a later version (`derive`).
    Reserved(String),
    /// `_`
    Underscore,

    // Punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDot,
    DotDotDot,
    DotDotEq,
    FatArrow,
    At,
    Question,
    Eq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Shl,
    Shr,
    Bang,
    AndAnd,
    OrOr,
    EqEq,
    NotEq,
    Lt,
    Gt,
    Le,
    Ge,

    /// End of input. When the lexer stopped at an unterminated literal or comment, the end is not a real one and
    /// the parser does not report errors at it.
    Eof,
}

impl TokenKind {
    pub fn keyword(word: &str) -> Option<TokenKind> {
        use TokenKind::*;
        Some(match word {
            "any" => Any,
            "as" => As,
            "break" => Break,
            "comptime" => Comptime,
            "const" => Const,
            "continue" => Continue,
            "else" => Else,
            "enum" => Enum,
            "error" => Error,
            "extern" => Extern,
            "false" => False,
            "fn" => Fn,
            "for" => For,
            "if" => If,
            "impl" => Impl,
            "in" => In,
            "interface" => Interface,
            "let" => Let,
            "loop" => Loop,
            "match" => Match,
            "mut" => Mut,
            "orelse" => Orelse,
            "own" => Own,
            "priv" => Priv,
            "return" => Return,
            "self" => SelfValue,
            "Self" => SelfType,
            "struct" => Struct,
            "true" => True,
            "type" => Type,
            "unsafe" => Unsafe,
            "use" => Use,
            "where" => Where,
            "while" => While,
            "derive" => Reserved(word.to_string()),
            _ => return None,
        })
    }

    /// How the token is shown in diagnostics.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Ident(name) => format!("identifier `{name}`"),
            Label(name) => format!("loop label `'{name}`"),
            Int(_) => "integer literal".into(),
            Float(_) => "float literal".into(),
            Char(_) => "character literal".into(),
            Str(_) | Interp(_) => "string literal".into(),
            Reserved(word) => format!("reserved word `{word}`"),
            Eof => "end of file".into(),
            other => format!("`{}`", other.text()),
        }
    }

    /// Source text of keywords and punctuation.
    pub fn text(&self) -> &'static str {
        use TokenKind::*;
        match self {
            Any => "any",
            As => "as",
            Break => "break",
            Comptime => "comptime",
            Const => "const",
            Continue => "continue",
            Else => "else",
            Enum => "enum",
            Error => "error",
            Extern => "extern",
            False => "false",
            Fn => "fn",
            For => "for",
            If => "if",
            Impl => "impl",
            In => "in",
            Interface => "interface",
            Let => "let",
            Loop => "loop",
            Match => "match",
            Mut => "mut",
            Orelse => "orelse",
            Own => "own",
            Priv => "priv",
            Return => "return",
            SelfValue => "self",
            SelfType => "Self",
            Struct => "struct",
            True => "true",
            Type => "type",
            Unsafe => "unsafe",
            Use => "use",
            Where => "where",
            While => "while",
            Underscore => "_",
            LParen => "(",
            RParen => ")",
            LBracket => "[",
            RBracket => "]",
            LBrace => "{",
            RBrace => "}",
            Comma => ",",
            Colon => ":",
            Semi => ";",
            Dot => ".",
            DotDot => "..",
            DotDotDot => "...",
            DotDotEq => "..=",
            FatArrow => "=>",
            At => "@",
            Question => "?",
            Eq => "=",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Amp => "&",
            Pipe => "|",
            Caret => "^",
            Shl => "<<",
            Shr => ">>",
            Bang => "!",
            AndAnd => "&&",
            OrOr => "||",
            EqEq => "==",
            NotEq => "!=",
            Lt => "<",
            Gt => ">",
            Le => "<=",
            Ge => ">=",
            Ident(_) | Label(_) | Int(_) | Float(_) | Char(_) | Str(_) | Interp(_) | Reserved(_) | Eof => "",
        }
    }
}

/// One piece of an interpolated string literal.
#[derive(Clone, Debug, PartialEq)]
pub enum InterpPiece {
    /// Literal text, with escapes and `{{`/`}}` already resolved.
    Lit(String),
    Hole(Box<HoleTokens>),
}

/// A `{expr:spec}` placeholder as the lexer saw it: the expression's tokens (ending in `Eof`; only `Eof` for a
/// positional `{}`), the raw specifier text after the first top-level `:`, and the span of the whole placeholder.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleTokens {
    pub tokens: Vec<Token>,
    pub spec: Option<(String, Span)>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// A line break comes between the previous token and this one.
    pub nl_before: bool,
}
