//! Types as the checker and the code generator see them.

use std::fmt;
use std::rc::Rc;

/// The allocator an allocator-aware type stores (`spec/allocators.md`, AL3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Alloc {
    /// The program's default allocator, `Global`. No value is stored: the container's buffer belongs to the
    /// C heap and is managed by the runtime.
    Global,
    /// A stored allocator value of this type, moved in when the container is created.
    Value(Box<Ty>),
}

impl Alloc {
    /// The stored allocator's type, or `None` for the default allocator.
    pub fn allocator_ty(&self) -> Option<&Ty> {
        match self {
            Alloc::Global => None,
            Alloc::Value(t) => Some(t),
        }
    }

    /// The allocator a `List`'s `alloc` field says: the zero-sized `std.alloc.Global` is the default one.
    pub fn of_field(ty: &Ty) -> Alloc {
        match ty {
            Ty::Struct(a) if a.name.ends_with("alloc.Global") => Alloc::Global,
            other => Alloc::Value(Box::new(other.clone())),
        }
    }

    /// Whether this is the default allocator.
    pub fn is_global(&self) -> bool {
        matches!(self, Alloc::Global)
    }
}

impl fmt::Display for Alloc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Alloc::Global => f.write_str("Global"),
            Alloc::Value(t) => write!(f, "{t}"),
        }
    }
}

/// A user-defined struct or enum: its index in `Program::structs` or `Program::enums`, and its name for messages.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AdtRef {
    pub id: usize,
    pub name: Rc<str>,
    /// An instance of a generic type with a second-class type argument (`Wrapper<&i32>`) is second-class itself
    /// (SC3). A non-generic type is never second-class: a view cannot be stored in its fields.
    pub second_class: bool,
}

/// `any I` (spec/existentials.md, AN1): an owned box of one value of a concrete type behind a vtable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnyRef {
    /// The index into `Program::any_ifaces`: the interface instance's vtable layout.
    pub id: usize,
    /// The display name, such as `Shape` or `Store<i32>`.
    pub name: Rc<str>,
    /// The interface the box was built as, and its type arguments. Substituting a generic signature maps these
    /// (`spec/existentials.md`, AN6).
    pub iface: usize,
    pub args: Vec<Ty>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

/// Every supported target is 64-bit, so `isize` and `usize` are 64 bits wide.
pub const POINTER_BITS: u32 = 64;

impl IntTy {
    pub fn signed(self) -> bool {
        matches!(self, IntTy::I8 | IntTy::I16 | IntTy::I32 | IntTy::I64 | IntTy::I128 | IntTy::Isize)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntTy::I8 | IntTy::U8 => 8,
            IntTy::I16 | IntTy::U16 => 16,
            IntTy::I32 | IntTy::U32 => 32,
            IntTy::I64 | IntTy::U64 => 64,
            IntTy::I128 | IntTy::U128 => 128,
            IntTy::Isize | IntTy::Usize => POINTER_BITS,
        }
    }

    /// The least value of the type, as a value (`spec/int128.md`, I128-1). `u128` values above `i128::MAX` are
    /// carried as their two's complement bit pattern, so `u128`'s least value is 0 and its greatest is `-1`.
    pub fn min(self) -> i128 {
        match self {
            IntTy::I128 => i128::MIN,
            _ if self.signed() => -(1i128 << (self.bits() - 1)),
            _ => 0,
        }
    }

    /// The greatest value of the type. For `u128` this is the bit pattern of `u128::MAX`, which is `-1` (see
    /// [`IntTy::min`]); callers that compare values of `u128` use [`IntTy::key`].
    pub fn max(self) -> i128 {
        match self {
            IntTy::I128 => i128::MAX,
            IntTy::U128 => -1,
            _ if self.signed() => (1i128 << (self.bits() - 1)) - 1,
            _ => (1i128 << self.bits()) - 1,
        }
    }

    /// A value's position in the type's order, as an `i128` that sorts the same way. It is the value itself for
    /// every type except `u128`, whose bit patterns sort wrongly as signed numbers: flipping the sign bit maps
    /// `0..=u128::MAX` onto `i128::MIN..=i128::MAX` in order. The mapping is its own inverse.
    pub fn key(self, v: i128) -> i128 {
        if self == IntTy::U128 { v ^ i128::MIN } else { v }
    }

    pub fn name(self) -> &'static str {
        match self {
            IntTy::I8 => "i8",
            IntTy::I16 => "i16",
            IntTy::I32 => "i32",
            IntTy::I64 => "i64",
            IntTy::I128 => "i128",
            IntTy::Isize => "isize",
            IntTy::U8 => "u8",
            IntTy::U16 => "u16",
            IntTy::U32 => "u32",
            IntTy::U64 => "u64",
            IntTy::U128 => "u128",
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

/// Whether a closure's captures are all `Send` (`spec/shared.md`, SH4). A closure type does not list its captures, so
/// the closure expression decides this when it is checked and the type carries the answer. It is not part of the
/// type's identity: two closure types with the same signature are the same type, and a closure written as a type
/// annotation (`f: Fn(): ()`) is not known to be `Send`.
#[derive(Clone, Debug)]
pub struct FnSend {
    /// Every capture may move to another task.
    pub ok: bool,
    /// When not, which capture is to blame and why, for the diagnostic.
    pub why: Option<Rc<str>>,
    /// Every capture is `Sync`, so a closure of kind `Fn` may be shared by several tasks (SH4). Only an `Fn` closure
    /// is ever `Sync`: the type's kind decides that, this says whether the captures allow it.
    pub sync_ok: bool,
    /// When not, which capture is to blame and why.
    pub sync_why: Option<Rc<str>>,
}

impl FnSend {
    /// A closure with nothing to block it: a function used as a value, or one whose captures are all `Send`.
    pub fn yes() -> FnSend {
        FnSend { ok: true, why: None, sync_ok: true, sync_why: None }
    }

    /// A closure whose captures are not known: a closure type written as an annotation or a bound.
    pub fn unknown() -> FnSend {
        FnSend { ok: false, why: None, sync_ok: false, sync_why: None }
    }

    /// The same closure with its `Sync` answer: `None` when every capture is `Sync`, else the reason one is not.
    pub fn with_sync(mut self, why: Option<String>) -> FnSend {
        self.sync_ok = why.is_none();
        self.sync_why = why.map(Into::into);
        self
    }

    /// A closure that captures something that is not `Send`.
    pub fn no(why: String) -> FnSend {
        FnSend { ok: false, why: Some(why.into()), sync_ok: true, sync_why: None }
    }
}

impl PartialEq for FnSend {
    fn eq(&self, _: &FnSend) -> bool {
        true
    }
}

impl Eq for FnSend {}

impl std::hash::Hash for FnSend {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
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
    /// borrow captures, which is second-class (`spec/memory-rules.md` CL3). `send` records that every capture of
    /// the closure the type came from may move to another task (`spec/shared.md`, SH4).
    Fn {
        sig: Rc<FnSig>,
        kind: FnKind,
        borrowed: bool,
        send: FnSend,
    },
    /// A shared (`&T`) or mutable (`&mut T`) reference. The value is the address of the referent.
    Ref {
        mutable: bool,
        inner: Box<Ty>,
    },
    /// `[T]`: a slice view, a pointer and a length.
    Slice(Box<Ty>),
    Array(Box<Ty>, u64),
    /// The iterator `str.chars()` returns, only usable in a `for` loop.
    Chars,
    /// `Heap<T>`: a single owned allocation of a value in the C heap.
    Heap(Box<Ty>, Alloc),
    /// `any I`: an owned value boxed behind the interface's vtable (spec/existentials.md, AN1).
    Any(AnyRef),
    /// `CPtr<T>`: a raw C pointer. It has no destructor and is copied like a number.
    CPtr(Box<Ty>),
    /// A borrowing iterator over a `Map<K, V>`: a copy of the map's three words and the next slot. It yields
    /// `(&K, &V)` and holds a loan on the map for its whole life.
    MapIter(Box<Ty>, Box<Ty>),
    /// A mutable iterator over the elements of a `List<T>` or an array `[T; N]` (`iter_mut()`): the address of the
    /// next element and the number left, like a slice. It yields `&mut T` and holds an exclusive loan on the
    /// collection for its whole life (spec/iterators.md, IT15).
    IterMut(Box<Ty>),
    /// A mutable iterator over a `Map<K, V>` (`iter_mut()`): the same state as [`Ty::MapIter`]. It yields
    /// `(&K, &mut V)` and holds an exclusive loan on the map.
    MapIterMut(Box<Ty>, Box<Ty>),
    /// The iterator `[T].chunks(n)` returns: the slice view, the chunk size, and the next index. The flag is
    /// `true` for `chunks_exact`, which stops before a short last chunk. It is second-class: it views its source.
    SliceChunks(Box<Ty>, bool),
    /// The iterator `[T].windows(n)` returns: the slice view, the window size, and the next index.
    SliceWindows(Box<Ty>),
    /// A tuple of two or more elements (or one, written with a trailing comma).
    Tuple(Vec<Ty>),
    Struct(AdtRef),
    Enum(AdtRef),
    Never,
    /// A type parameter of a generic function or type, only while its body is checked against its bounds. Such
    /// code never reaches code generation: instantiations replace the parameter with a concrete type.
    Param(Rc<str>),
    /// An unresolved associated type projection, `Self.Name` or `<T as Interface>.Name`, while a signature or a
    /// body is checked symbolically (`spec/associated-types.md`, AT3). It is opaque: resolution substitutes the
    /// implementation's binding as soon as the self type is known, so no projection reaches the typed tree of a
    /// concrete function or code generation.
    Projection {
        self_ty: Box<Ty>,
        /// The index of the interface in the program's interface table.
        iface: usize,
        /// The interface's display name, such as `Container` or `Container<i32>`.
        iface_name: Rc<str>,
        /// The interface's type arguments. Used for identity and display only.
        args: Vec<Ty>,
        /// The associated type's name.
        name: Rc<str>,
    },
    /// The error type of a conversion, `<T as TryFrom<S>>.Error` (`spec/stdlib.md`, TF9): the `E` of the `try_from`
    /// that converts an `S` into `T`. It is opaque like a projection, and resolves once `self_ty` is concrete, to
    /// `TryFromIntError` for an integer target or to the error of the target's `try_from` block.
    TryFromError {
        self_ty: Box<Ty>,
        /// The source type the conversion takes.
        source: Box<Ty>,
    },
    /// A const generic argument: the value `Matrix<f64, 2, 3>` gives `R` and `C`. Values from `1 << 100` up are
    /// expressions over const parameters that sema has not folded yet, only seen while a generic body is checked
    /// (`spec/const-generics.md`). It has no run-time representation.
    Const(i128),
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
            Ty::Array(t, _) => t.is_second_class(),
            // An owning iterator holds its elements; a borrowing one points at its source.
            Ty::MapIter(..) | Ty::IterMut(_) | Ty::MapIterMut(..) => true,
            // A chunks or windows iterator views the slice it was made from.
            Ty::SliceChunks(..) | Ty::SliceWindows(..) => true,
            // A `Shared<T>` owns its heaped value, like a `Heap`.
            Ty::Tuple(ts) => ts.iter().any(Ty::is_second_class),
            Ty::Struct(a) | Ty::Enum(a) => a.second_class,
            // A box owns its heaped value; an `any I` type argument is never a view of the box (AN1, AN3).
            Ty::Any(_) => false,
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
            Ty::Array(t, n) if *n >= 1 << 62 => write!(f, "[{t}; _]"),
            Ty::Array(t, n) => write!(f, "[{t}; {n}]"),
            Ty::Chars => f.write_str("Chars"),
            Ty::Heap(t, Alloc::Global) => write!(f, "Heap<{t}>"),
            Ty::Heap(t, a) => write!(f, "Heap<{t}, {a}>"),
            Ty::Any(a) => write!(f, "any {}", a.name),
            Ty::CPtr(t) => write!(f, "CPtr<{t}>"),
            Ty::MapIter(k, v) => write!(f, "MapIter<{k}, {v}>"),
            Ty::IterMut(t) => write!(f, "IterMut<{t}>"),
            Ty::MapIterMut(k, v) => write!(f, "MapIterMut<{k}, {v}>"),
            Ty::SliceChunks(t, false) => write!(f, "Chunks<{t}>"),
            Ty::SliceChunks(t, true) => write!(f, "ChunksExact<{t}>"),
            Ty::SliceWindows(t) => write!(f, "Windows<{t}>"),
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
            Ty::Projection { self_ty, iface_name, args, name, .. } => {
                write!(f, "<{self_ty} as {iface_name}")?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    f.write_str(">")?;
                }
                write!(f, ">.{name}")
            }
            Ty::TryFromError { self_ty, source } => write!(f, "<{self_ty} as TryFrom<{source}>>.Error"),
            Ty::Const(v) if *v >= 1 << 100 => f.write_str("_"),
            Ty::Const(v) => write!(f, "{v}"),
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
        // The 128-bit types: `u128::MAX` is the bit pattern `-1`, and `key` keeps the order of the values.
        assert_eq!(IntTy::I128.bits(), 128);
        assert_eq!(IntTy::I128.min(), i128::MIN);
        assert_eq!(IntTy::I128.max(), i128::MAX);
        assert_eq!(IntTy::U128.min(), 0);
        assert_eq!(IntTy::U128.max(), -1);
        assert_eq!(IntTy::U128.key(IntTy::U128.min()), i128::MIN);
        assert_eq!(IntTy::U128.key(IntTy::U128.max()), i128::MAX);
        assert!(IntTy::U128.key(1) < IntTy::U128.key(-1));
        assert_eq!(IntTy::U128.key(IntTy::U128.key(12345)), 12345);
        assert_eq!(IntTy::I128.key(-7), -7);
        assert_eq!(IntTy::I128.name(), "i128");
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
        assert!(Ty::Array(Box::new(Ty::Str), 2).is_second_class());
        // Owned types are not views.
        assert!(!Ty::String.is_second_class());
        assert!(!Ty::Heap(Box::new(i32_.clone()), Alloc::Global).is_second_class());
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
            send: FnSend::unknown(),
        };
        assert_eq!(f.to_string(), "Fn(i32): i32");
        let g = Ty::Fn {
            sig: Rc::new(sig(vec![], Ty::Unit)),
            kind: FnKind::FnMut,
            borrowed: true,
            send: FnSend::unknown(),
        };
        assert_eq!(g.to_string(), "FnMut(): ()");
        assert!(g.is_second_class(), "a borrowing closure is second-class");
        assert!(!f.is_second_class());
    }

    #[test]
    fn display_spells_types_like_the_source() {
        let i32_ = Ty::Int(IntTy::I32);
        let arena = || Ty::Struct(AdtRef { id: 0, name: "Arena".into(), second_class: false });
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
            (Ty::Chars, "Chars"),
            (Ty::Heap(Box::new(i32_.clone()), Alloc::Global), "Heap<i32>"),
            (Ty::Heap(Box::new(i32_.clone()), Alloc::Value(Box::new(arena()))), "Heap<i32, Arena>"),
            (Ty::Any(AnyRef { id: 0, name: "Shape".into(), iface: 0, args: Vec::new() }), "any Shape"),
            (Ty::CPtr(Box::new(i32_.clone())), "CPtr<i32>"),
            (Ty::Tuple(vec![i32_.clone()]), "(i32,)"),
            (Ty::Tuple(vec![i32_.clone(), Ty::Bool]), "(i32, bool)"),
            (Ty::Struct(AdtRef { id: 0, name: "P".into(), second_class: false }), "P"),
            (Ty::Enum(AdtRef { id: 0, name: "E".into(), second_class: false }), "E"),
            (Ty::Never, "!"),
            (Ty::Param("T".into()), "T"),
            (
                Ty::Projection {
                    self_ty: Box::new(Ty::Param("T".into())),
                    iface: 0,
                    iface_name: "Container".into(),
                    args: Vec::new(),
                    name: "Item".into(),
                },
                "<T as Container>.Item",
            ),
            (
                Ty::Projection {
                    self_ty: Box::new(i32_.clone()),
                    iface: 0,
                    iface_name: "Container".into(),
                    args: vec![i32_.clone()],
                    name: "Item".into(),
                },
                "<i32 as Container<i32>>.Item",
            ),
            (
                Ty::TryFromError { self_ty: Box::new(Ty::Param("T".into())), source: Box::new(i32_.clone()) },
                "<T as TryFrom<i32>>.Error",
            ),
            (Ty::Error, "{error}"),
        ];
        for (ty, want) in cases {
            assert_eq!(ty.to_string(), want);
        }
    }

    #[test]
    fn an_allocator_argument_is_global_or_a_stored_value() {
        assert!(Alloc::Global.is_global());
        assert!(Alloc::Global.allocator_ty().is_none());
        let pool = Ty::Struct(AdtRef { id: 0, name: "Pool".into(), second_class: false });
        let stored = Alloc::Value(Box::new(pool.clone()));
        assert!(!stored.is_global());
        assert_eq!(stored.allocator_ty(), Some(&pool));
        assert_eq!(stored.to_string(), "Pool");
    }
}
