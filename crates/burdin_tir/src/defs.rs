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
    /// Where the derive names were written, for diagnostics at the derive clause.
    pub copy_span: Option<Span>,
    pub clone_span: Option<Span>,
}

#[derive(Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub span: Span,
    /// Package-private: other packages can read and write it only through `Type`'s own code (E0150).
    pub is_priv: bool,
}

pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
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
    /// The `Drop::drop` method of `Type: Drop { ... }`, called by the drop glue.
    pub drop_method: Option<FuncId>,
}
