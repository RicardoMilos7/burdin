//! Types as the checker and the code generator see them.

use std::fmt;
use std::rc::Rc;

/// A user-defined struct or enum: its index in `Program::structs` or `Program::enums`, and its name for messages.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdtRef {
    pub id: usize,
    pub name: Rc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatTy {
    F32,
    F64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    Unit,
    Bool,
    Char,
    Int(IntTy),
    Float(FloatTy),
    Str,
    String,
    /// A shared (`&T`) or mutable (`&mut T`) reference. The value is the address of the referent.
    Ref {
        mutable: bool,
        inner: Box<Ty>,
    },
    /// `[T]`: a slice view, a pointer and a length.
    Slice(Box<Ty>),
    Array(Box<Ty>, u64),
    /// The prelude's `Option<T>`.
    Option(Box<Ty>),
    /// The prelude's `Result<T, E>`: `Ok(T)` or `Err(E)`.
    Result(Box<Ty>, Box<Ty>),
    /// `List<T>`: a growable, owned array in the C heap.
    List(Box<Ty>),
    /// `Map<K, V>`: an owned collection of key/value pairs.
    Map(Box<Ty>, Box<Ty>),
    /// The iterator `str.chars()` returns, only usable in a `for` loop.
    Chars,
    /// `Heap<T>`: a single owned allocation of a value in the C heap.
    Heap(Box<Ty>),
    /// `CPtr<T>`: a raw C pointer. It has no destructor and is copied like a number.
    CPtr(Box<Ty>),
    /// `CString`: an owned, NUL-terminated C string in the C heap.
    CString,
    /// A tuple of two or more elements (or one, written with a trailing comma).
    Tuple(Vec<Ty>),
    Struct(AdtRef),
    Enum(AdtRef),
    Never,
    /// A type parameter of a generic function or type, only while its body is checked against its bounds. Such
    /// code never reaches code generation: instantiations replace the parameter with a concrete type.
    Param(Rc<str>),
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

    pub fn is_ref(&self) -> bool {
        matches!(self, Ty::Ref { .. })
    }

    /// Whether a value of this type is a view of other storage: a reference, a `str`, a slice, or an aggregate
    /// that contains one (memory-rules.md, second-class types).
    pub fn is_second_class(&self) -> bool {
        match self {
            Ty::Str | Ty::Slice(_) | Ty::Ref { .. } => true,
            Ty::Array(t, _) | Ty::Option(t) | Ty::List(t) => t.is_second_class(),
            Ty::Result(t, e) => t.is_second_class() || e.is_second_class(),
            Ty::Map(k, v) => k.is_second_class() || v.is_second_class(),
            Ty::Tuple(ts) => ts.iter().any(Ty::is_second_class),
            _ => false,
        }
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
            Ty::Ref { mutable: false, inner } => write!(f, "&{inner}"),
            Ty::Ref { mutable: true, inner } => write!(f, "&mut {inner}"),
            Ty::Slice(t) => write!(f, "[{t}]"),
            Ty::Array(t, n) => write!(f, "[{t}; {n}]"),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Result(t, e) => write!(f, "Result<{t}, {e}>"),
            Ty::List(t) => write!(f, "List<{t}>"),
            Ty::Map(k, v) => write!(f, "Map<{k}, {v}>"),
            Ty::Chars => f.write_str("Chars"),
            Ty::Heap(t) => write!(f, "Heap<{t}>"),
            Ty::CPtr(t) => write!(f, "CPtr<{t}>"),
            Ty::CString => f.write_str("CString"),
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
            Ty::Param(name) => f.write_str(name),
            Ty::Error => f.write_str("{error}"),
        }
    }
}
