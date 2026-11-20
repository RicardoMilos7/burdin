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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantShape {
    Unit,
    Tuple,
    Struct,
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
}
