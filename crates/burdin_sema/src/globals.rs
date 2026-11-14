//! What the checker knows about the program's declarations: names, types, signatures, and generics.

use crate::generics::{GenericAdt, GenericFn, ImplInfo, InterfaceInfo, Request};
use burdin_syntax::ast;
use burdin_tir::{self as tir, AdtRef, Builtin, ConstId, FuncId, Signature, Ty};
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) enum Callee {
    User(FuncId),
    Builtin(Builtin),
}

#[derive(Clone, Copy)]
pub(crate) enum Global {
    Func(FuncId),
    Const(ConstId),
    /// A generic function: an index into [`Globals::generic_fns`].
    Generic(usize),
}

#[derive(Clone, Copy)]
pub(crate) enum TypeName {
    Struct(usize),
    Enum(usize),
    Alias(usize),
    /// A generic struct or enum: an index into [`Globals::generic_adts`]. It is not a type until it has arguments.
    GenericAdt(usize),
}

#[derive(Default)]
pub(crate) struct Globals {
    pub(crate) names: HashMap<String, Global>,
    pub(crate) types: HashMap<String, TypeName>,
    pub(crate) aliases: Vec<Ty>,
    pub(crate) structs: Vec<tir::StructDef>,
    pub(crate) enums: Vec<tir::EnumDef>,
    pub(crate) struct_refs: Vec<AdtRef>,
    pub(crate) enum_refs: Vec<AdtRef>,
    /// Methods by the type they belong to (`(false, struct)` or `(true, enum)`) and name.
    pub(crate) methods: HashMap<((bool, usize), String), FuncId>,
    /// The `Drop::drop` method of a type that has a `Type: Drop` block.
    pub(crate) drop_methods: HashMap<(bool, usize), FuncId>,
    pub(crate) sigs: Vec<Signature>,
    pub(crate) const_tys: Vec<Ty>,
    // Interfaces and generics (see `generics.rs`).
    pub(crate) iface_names: HashMap<String, usize>,
    pub(crate) interfaces: Vec<InterfaceInfo>,
    pub(crate) generic_adts: Vec<GenericAdt>,
    pub(crate) generic_fns: Vec<GenericFn>,
    pub(crate) impls: Vec<ImplInfo>,
    /// The struct or enum for a generic type and its arguments.
    pub(crate) adt_instances: HashMap<(usize, Vec<Ty>), (bool, usize)>,
    /// The other way around: the generic type and arguments of an instance.
    pub(crate) adt_origin: HashMap<(bool, usize), (usize, Vec<Ty>)>,
    pub(crate) fn_instances: HashMap<(usize, Vec<Ty>), FuncId>,
    /// Instances asked for while resolving types and checking calls, to be allocated before the next pass.
    pub(crate) requests: RefCell<Vec<Request>>,
}

pub(crate) fn adt_key(ty: &Ty) -> Option<(bool, usize)> {
    match ty {
        Ty::Struct(a) => Some((false, a.id)),
        Ty::Enum(a) => Some((true, a.id)),
        _ => None,
    }
}

/// The name of the interface in a `Type: Interface { ... }` block, and whether it has generic arguments.
pub(crate) fn interface_name(ty: &ast::Type) -> (String, bool) {
    match &ty.kind {
        ast::TypeKind::Path(segments) if segments.len() == 1 => {
            (segments[0].name.name.clone(), !segments[0].args.is_empty())
        }
        _ => (String::new(), false),
    }
}
