//! Types as the checker and the code generator see them.

use std::fmt;
use std::rc::Rc;

/// A user-defined struct or enum: its index in `Program::structs` or `Program::enums`, and its name for messages.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdtRef {
    pub id: usize,
    pub name: Rc<str>,
    /// An instance of a generic type with a second-class type argument (`Wrapper<&i32>`) is second-class itself
    /// (SC3). A non-generic type is never second-class: a view cannot be stored in its fields.
    pub second_class: bool,
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

/// The capability family of a closure (`spec/grammar.md` section 4, `spec/memory-rules.md` CL5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FnKind {
    /// Can be called any number of times through a shared borrow.
    Fn,
    /// Can be called through an exclusive borrow: it mutates a capture.
    FnMut,
    /// Consumes the closure on the call: its body moves a captured value out.
    FnOnce,
}

impl FnKind {
    pub fn name(self) -> &'static str {
        match self {
            FnKind::Fn => "Fn",
            FnKind::FnMut => "FnMut",
            FnKind::FnOnce => "FnOnce",
        }
    }

    /// Whether a value of this kind may be used where `expected` is required (`Fn` as `FnMut` as `FnOnce`).
    pub fn at_least(self, expected: FnKind) -> bool {
        self <= expected
    }
}

/// The signature of a closure: its parameter and return types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Box<Ty>,
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
    /// A closure interface value: `Fn(A, B): R`, `FnMut(...)`, or `FnOnce(...)`. `borrowed` marks a closure with
    /// borrow captures, which is second-class (`spec/memory-rules.md` CL3).
    Fn {
        sig: Rc<FnSig>,
        kind: FnKind,
        borrowed: bool,
    },
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
    /// `Range<T>` (exclusive) or `RangeInclusive<T>` (inclusive): the two integer endpoints. The flag is true for
    /// the inclusive form.
    Range(Box<Ty>, bool),
    /// The owning iterator of a `List<T>`: the buffer, its length and capacity, and the next index.
    ListIntoIter(Box<Ty>),
    /// The owning iterator of an array `[T; N]`: the array value and the next index.
    ArrayIntoIter(Box<Ty>, u64),
    /// A borrowing iterator over a `Map<K, V>`: a copy of the map's three words and the next slot. It yields
    /// `(&K, &V)` and holds a loan on the map for its whole life.
    MapIter(Box<Ty>, Box<Ty>),
    /// The owning iterator of a `Map<K, V>`: the map's buffer, moved entry by entry.
    MapIntoIter(Box<Ty>, Box<Ty>),
    /// The iterator of a range: the two endpoints and whether the range is exhausted. The flag is true for the
    /// inclusive form.
    RangeIter(Box<Ty>, bool),
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
            // A closure with borrow captures points at the storage it captured (CL3).
            Ty::Fn { borrowed, .. } => *borrowed,
            Ty::Array(t, _) | Ty::Option(t) | Ty::List(t) => t.is_second_class(),
            // An owning iterator holds its elements; a borrowing one points at its source.
            Ty::MapIter(..) => true,
            Ty::ListIntoIter(t) | Ty::ArrayIntoIter(t, _) | Ty::RangeIter(t, _) => t.is_second_class(),
            Ty::Result(t, e) => t.is_second_class() || e.is_second_class(),
            Ty::Map(k, v) | Ty::MapIntoIter(k, v) => k.is_second_class() || v.is_second_class(),
            Ty::Tuple(ts) => ts.iter().any(Ty::is_second_class),
            Ty::Struct(a) | Ty::Enum(a) => a.second_class,
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
            Ty::Fn { sig, kind, .. } => {
                write!(f, "{}(", kind.name())?;
                for (i, p) in sig.params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, "): {}", sig.ret)
            }
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
            Ty::Range(t, false) => write!(f, "Range<{t}>"),
            Ty::Range(t, true) => write!(f, "RangeInclusive<{t}>"),
            Ty::ListIntoIter(t) => write!(f, "ListIntoIter<{t}>"),
            Ty::ArrayIntoIter(t, n) => write!(f, "ArrayIntoIter<{t}, {n}>"),
            Ty::MapIter(k, v) => write!(f, "MapIter<{k}, {v}>"),
            Ty::MapIntoIter(k, v) => write!(f, "MapIntoIter<{k}, {v}>"),
            Ty::RangeIter(t, false) => write!(f, "RangeIter<{t}>"),
            Ty::RangeIter(t, true) => write!(f, "RangeIterInclusive<{t}>"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_widths_and_ranges() {
        assert_eq!(IntTy::I8.bits(), 8);
        assert_eq!(IntTy::I8.min(), -128);
        assert_eq!(IntTy::I8.max(), 127);
        assert_eq!(IntTy::U8.min(), 0);
        assert_eq!(IntTy::U8.max(), 255);
        assert_eq!(IntTy::I64.min(), i64::MIN as i128);
        assert_eq!(IntTy::I64.max(), i64::MAX as i128);
        assert_eq!(IntTy::U64.max(), u64::MAX as i128);
        // Every supported target is 64-bit, so the pointer types are too.
        assert_eq!(IntTy::Isize.bits(), POINTER_BITS);
        assert_eq!(IntTy::Usize.bits(), 64);
        assert_eq!(IntTy::Usize.max(), u64::MAX as i128);
        assert!(IntTy::Isize.signed());
        assert!(!IntTy::Usize.signed());
        assert_eq!(IntTy::I32.name(), "i32");
        assert_eq!(IntTy::Usize.name(), "usize");
    }

    #[test]
    fn second_class_types_are_views() {
        let i32_ = Ty::Int(IntTy::I32);
        assert!(Ty::Str.is_second_class());
        assert!(Ty::Slice(Box::new(i32_.clone())).is_second_class());
        assert!(Ty::Ref { mutable: true, inner: Box::new(i32_.clone()) }.is_second_class());
        assert!(Ty::Tuple(vec![i32_.clone(), Ty::Str]).is_second_class());
        assert!(Ty::Option(Box::new(Ty::Str)).is_second_class());
        assert!(Ty::Array(Box::new(Ty::Str), 2).is_second_class());
        assert!(Ty::Result(Box::new(Ty::Bool), Box::new(Ty::Str)).is_second_class());
        assert!(Ty::Map(Box::new(Ty::Str), Box::new(Ty::Bool)).is_second_class());
        // Owned types are not views.
        assert!(!Ty::String.is_second_class());
        assert!(!Ty::Heap(Box::new(i32_.clone())).is_second_class());
        assert!(!Ty::List(Box::new(i32_.clone())).is_second_class());
        assert!(!i32_.is_second_class());
    }

    #[test]
    fn closure_capabilities_are_ordered() {
        assert!(FnKind::Fn.at_least(FnKind::Fn));
        assert!(FnKind::Fn.at_least(FnKind::FnMut));
        assert!(FnKind::Fn.at_least(FnKind::FnOnce));
        assert!(FnKind::FnMut.at_least(FnKind::FnMut));
        assert!(!FnKind::FnMut.at_least(FnKind::Fn));
        assert!(FnKind::FnOnce.at_least(FnKind::FnOnce));
        assert!(!FnKind::FnOnce.at_least(FnKind::FnMut));
    }

    #[test]
    fn closure_types_display_like_the_source() {
        let sig = |params: Vec<Ty>, ret: Ty| FnSig { params, ret: Box::new(ret) };
        let f = Ty::Fn {
            sig: Rc::new(sig(vec![Ty::Int(IntTy::I32)], Ty::Int(IntTy::I32))),
            kind: FnKind::Fn,
            borrowed: false,
        };
        assert_eq!(f.to_string(), "Fn(i32): i32");
        let g = Ty::Fn { sig: Rc::new(sig(vec![], Ty::Unit)), kind: FnKind::FnMut, borrowed: true };
        assert_eq!(g.to_string(), "FnMut(): ()");
        assert!(g.is_second_class(), "a borrowing closure is second-class");
        assert!(!f.is_second_class());
    }

    #[test]
    fn display_spells_types_like_the_source() {
        let i32_ = Ty::Int(IntTy::I32);
        let cases = [
            (Ty::Unit, "()"),
            (Ty::Bool, "bool"),
            (Ty::Char, "char"),
            (i32_.clone(), "i32"),
            (Ty::Float(FloatTy::F32), "f32"),
            (Ty::Float(FloatTy::F64), "f64"),
            (Ty::Str, "str"),
            (Ty::String, "String"),
            (Ty::Ref { mutable: false, inner: Box::new(i32_.clone()) }, "&i32"),
            (Ty::Ref { mutable: true, inner: Box::new(i32_.clone()) }, "&mut i32"),
            (Ty::Slice(Box::new(i32_.clone())), "[i32]"),
            (Ty::Array(Box::new(i32_.clone()), 4), "[i32; 4]"),
            (Ty::Option(Box::new(i32_.clone())), "Option<i32>"),
            (Ty::Result(Box::new(i32_.clone()), Box::new(Ty::Str)), "Result<i32, str>"),
            (Ty::List(Box::new(i32_.clone())), "List<i32>"),
            (Ty::Map(Box::new(Ty::Str), Box::new(i32_.clone())), "Map<str, i32>"),
            (Ty::Chars, "Chars"),
            (Ty::Heap(Box::new(i32_.clone())), "Heap<i32>"),
            (Ty::CPtr(Box::new(i32_.clone())), "CPtr<i32>"),
            (Ty::CString, "CString"),
            (Ty::Range(Box::new(i32_.clone()), false), "Range<i32>"),
            (Ty::Range(Box::new(i32_.clone()), true), "RangeInclusive<i32>"),
            (Ty::Tuple(vec![i32_.clone()]), "(i32,)"),
            (Ty::Tuple(vec![i32_.clone(), Ty::Bool]), "(i32, bool)"),
            (Ty::Struct(AdtRef { id: 0, name: "P".into(), second_class: false }), "P"),
            (Ty::Enum(AdtRef { id: 0, name: "E".into(), second_class: false }), "E"),
            (Ty::Never, "!"),
            (Ty::Param("T".into()), "T"),
            (Ty::Error, "{error}"),
        ];
        for (ty, want) in cases {
            assert_eq!(ty.to_string(), want);
        }
    }
}
