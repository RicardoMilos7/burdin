//! Struct and enum definitions.

use crate::ops::FuncId;
use crate::ty::Ty;
use burdin_diag::Span;

/// The built-in interfaces a type derives.
#[derive(Clone, Copy, Debug, Default)]
pub struct Derives {
    pub copy: bool,
    pub clone: bool,
    pub eq: bool,
    pub ord: bool,
    pub hash: bool,
    pub default: bool,
    /// `Debug`: the compiler generates the impl block, and this records that the fields must support it.
    pub debug: bool,
    /// `Serialize` and `Deserialize`: the compiler generates the impl blocks (spec/serialization.md, SER12). Their
    /// fields' requirements are not checked yet; the generated body calls the per-shape helpers directly.
    pub serialize: bool,
    pub deserialize: bool,
    /// Where the derive names were written, for diagnostics at the derive clause.
    pub copy_span: Option<Span>,
    pub clone_span: Option<Span>,
    /// The hand-written `PartialEq.eq` and `PartialOrd.lt` of the type, when it implements the interface instead of
    /// deriving `Eq`/`Ord`. The built-in comparisons (`Map` keys, `List.contains`, `sort`, `==` on a tuple or list
    /// of the type) call them (`spec/derives.md`, issue #918). Filled in when the program is finished.
    pub eq_method: Option<FuncId>,
    pub lt_method: Option<FuncId>,
    /// The `clone(self): Self` method a `std` type writes by hand instead of deriving `Clone` (`Shared`, `CString`):
    /// the type is `Clone` and the clone glue calls it.
    pub clone_method: Option<FuncId>,
}

#[derive(Clone, Debug)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
    /// Package-private: other packages can read and write it only through `Type`'s own code (E0150).
    pub is_priv: bool,
}

#[derive(Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// `repr(C)`: the fields keep declaration order and the C ABI's natural padding and alignment, and the struct
    /// may cross an `extern "C"` boundary by value (issues #73 and #490).
    pub repr_c: bool,
    pub derives: Derives,
    /// The `Drop::drop` method of `Type: Drop { ... }`, called by the drop glue.
    pub drop_method: Option<FuncId>,
    /// Which language-known struct this instance is (`std`'s `List` or its owning iterator), if either.
    pub lang: Option<LangItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantShape {
    Unit,
    Tuple,
    Struct,
}

/// The enums the language itself knows, declared in `std` (`spec/stdlib.md`, "Compiler-known types").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangItem {
    /// `Option<T>`: the variants `None` and `Some(T)`, in that order.
    Option,
    /// `Result<T, E>`: the variants `Ok(T)` and `Err(E)`, in that order.
    Result,
    /// `List<T, A>`: the fields `ptr`, `len`, `cap`, and `alloc`, in that order.
    List,
    /// `ListIntoIter<T, A>`: the fields `ptr`, `len`, `cap`, `pos`, and `alloc`, in that order.
    ListIntoIter,
    /// `ManuallyDrop<T>`: the one field `value`, which drop glue never drops.
    ManuallyDrop,
    /// `ArrayIntoIter<T, N>`: the fields `data` (a `ManuallyDrop<[T; N]>`) and `pos`, in that order.
    ArrayIntoIter,
    /// `MapIntoIter<K, V, A>`: the fields `ptr`, `len`, `cap`, `pos`, and `alloc`, in that order.
    MapIntoIter,
    /// `Map<K, V, A>`: the fields `ptr` (a `CPtr<(K, V, u64)>`), `len`, `cap`, and `alloc`, in that order.
    Map,
    /// `Range<T>`: the fields `start` and `end`.
    Range,
    /// `RangeInclusive<T>`: the fields `start` and `end`.
    RangeInclusive,
    /// `RangeIter<T>`: the iterator of a `Range<T>`; the fields `cur`, `end`, and `done`.
    RangeIter,
    /// `RangeIterInclusive<T>`: the iterator of a `RangeInclusive<T>`; the fields `cur`, `end`, and `done`.
    RangeIterInclusive,
}

#[derive(Debug)]
pub struct VariantDef {
    pub name: String,
    pub shape: VariantShape,
    /// The fields of a tuple variant are named `0`, `1`, ...
    pub fields: Vec<FieldDef>,
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<VariantDef>,
    pub derives: Derives,
    /// The `Drop::drop` method of `Type: Drop { ... }`, called by the drop glue.
    pub drop_method: Option<FuncId>,
    /// Which language-known enum this instance is (`std`'s `Option` or `Result`), if either.
    pub lang: Option<LangItem>,
}
