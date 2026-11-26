//! LLVM types for Burdin types, and the questions about them that code generation asks.

use super::Codegen;
use burdin_tir::{FloatTy, IntTy, Ty, TypeDefs};
use inkwell::AddressSpace;
use inkwell::types::{BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::IntValue;

/// The constant of integer type `t` with value `v`. A value is carried as an `i128`, and a `u128` above
/// `i128::MAX` as its bit pattern (`spec/int128.md`, I128-1), so both 128-bit types take both words.
pub(super) fn int_const<'ctx>(t: IntType<'ctx>, v: i128) -> IntValue<'ctx> {
    if t.get_bit_width() == 128 {
        t.const_int_arbitrary_precision(&[v as u64, (v >> 64) as u64])
    } else {
        // Truncating to 64 bits keeps the two's complement bits of a negative value.
        t.const_int(v as u64, false)
    }
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn int_ty(&self, i: IntTy) -> IntType<'ctx> {
        match i {
            IntTy::Isize | IntTy::Usize => self.usize_ty,
            IntTy::I8 | IntTy::U8 => self.cx.i8_type(),
            IntTy::I16 | IntTy::U16 => self.cx.i16_type(),
            IntTy::I32 | IntTy::U32 => self.cx.i32_type(),
            IntTy::I64 | IntTy::U64 => self.cx.i64_type(),
            IntTy::I128 | IntTy::U128 => self.cx.i128_type(),
        }
    }

    /// The LLVM type of a value of `ty`, or `None` for types without a runtime value.
    pub(super) fn llvm_ty(&self, ty: &Ty) -> Option<BasicTypeEnum<'ctx>> {
        Some(match ty {
            Ty::Unit
            | Ty::Never
            | Ty::Error
            | Ty::Param(_)
            | Ty::Const(_)
            | Ty::Projection { .. }
            | Ty::TryFromError { .. } => return None,
            Ty::Bool => self.cx.bool_type().into(),
            Ty::Char => self.cx.i32_type().into(),
            Ty::Int(i) => self.int_ty(*i).into(),
            Ty::Float(FloatTy::F32) => self.cx.f32_type().into(),
            Ty::Float(FloatTy::F64) => self.cx.f64_type().into(),
            Ty::Str => self.str_ty.into(),
            // A closure value is a code pointer, a capture-record pointer, and its destructor.
            Ty::Fn { .. } => self.closure_ty.into(),
            // An `any I` box is the data pointer and the vtable pointer.
            Ty::Any(_) => self.any_ty.into(),
            Ty::Chars => self.str_ty.into(),
            // A list and a map have the same three-word shape as a `String`: pointer, length, capacity. A stored
            // allocator value follows them (`spec/allocators.md`, AL3), and the default one stores nothing.
            Ty::String => self.string_ty.into(),
            Ty::Map(_, _, alloc) => self.seq_ty(alloc).into(),
            // A map entry is the map's address, the key it owns, the key's hash, the probe slot, and whether the
            // key was found.
            Ty::MapEntry(key, _, _) => self.map_entry_value_ty(key).into(),
            // A `CPtr` is a raw address.
            Ty::CPtr(_) => self.cx.ptr_type(AddressSpace::default()).into(),
            Ty::Heap(_, alloc) => self.heap_ty(alloc),
            // A reference is the address of the referent; a slice is a pointer and a length, like a `str`.
            Ty::Ref { .. } => self.cx.ptr_type(AddressSpace::default()).into(),
            Ty::Slice(_) => self.str_ty.into(),
            Ty::Array(elem, n) => self.llvm_ty(elem)?.array_type(*n as u32).into(),
            Ty::Tuple(_) | Ty::Struct(_) => self.product_ty(&self.field_tys(ty)).into(),
            // A range is two copied endpoints.
            Ty::Range(inner, _) => self.product_ty(&[(**inner).clone(), (**inner).clone()]).into(),
            // An iterator over a list or a map keeps the collection's three words and the next index.
            Ty::MapIntoIter(_, _, alloc) => self.iter_seq_ty_for(alloc).into(),
            Ty::MapIter(..) | Ty::MapIterMut(..) => self.iter_seq_ty().into(),
            // A mutable list or array iterator is the next element's address and the number left, like a slice.
            Ty::IterMut(_) => self.str_ty.into(),
            // The range iterator keeps both endpoints and whether the range is exhausted.
            Ty::RangeIter(inner, _) => self.product_ty(&[(**inner).clone(), (**inner).clone(), Ty::Bool]).into(),
            // The chunks and windows iterators keep the slice view (pointer and length), the chunk or window
            // size, and the next index.
            Ty::SliceChunks(..) | Ty::SliceWindows(..) => self.slice_iter_ty().into(),
            Ty::Enum(a) => self.enum_ty(a.id).into(),
        })
    }

    /// The type of a member of an aggregate: like `llvm_ty`, but a value-less type is an empty struct.
    pub(super) fn storage_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        self.llvm_ty(ty).unwrap_or_else(|| self.cx.struct_type(&[], false).into())
    }

    /// The layout of a pending map lookup: `{ ptr map, K key, u64 hash, usize slot, bool found }`. The key is owned
    /// by the entry until `or_insert` stores it or drops it. The value does not store the map's allocator: the entry
    /// reaches it through the map it points at, which the entry type names.
    pub(super) fn map_entry_value_ty(&self, key: &Ty) -> StructType<'ctx> {
        let ptr = self.cx.ptr_type(AddressSpace::default());
        self.cx.struct_type(
            &[
                ptr.into(),
                self.storage_ty(key),
                self.cx.i64_type().into(),
                self.usize_ty.into(),
                self.cx.bool_type().into(),
            ],
            false,
        )
    }

    /// The `{ ptr, len, cap }` layout of a list or map, with its stored allocator value appended.
    pub(super) fn seq_ty(&self, alloc: &burdin_tir::Alloc) -> StructType<'ctx> {
        match alloc.allocator_ty() {
            None => self.string_ty,
            Some(a) => {
                let ptr = self.cx.ptr_type(AddressSpace::default());
                let a = self.storage_ty(a);
                self.cx.struct_type(&[ptr.into(), self.usize_ty.into(), self.usize_ty.into(), a], false)
            }
        }
    }

    /// The `{ ptr, len, cap, index }` layout of the owning list and map iterators, with the stored allocator.
    pub(super) fn iter_seq_ty_for(&self, alloc: &burdin_tir::Alloc) -> StructType<'ctx> {
        match alloc.allocator_ty() {
            None => self.iter_seq_ty(),
            Some(a) => {
                let ptr = self.cx.ptr_type(AddressSpace::default());
                let a = self.storage_ty(a);
                self.cx.struct_type(
                    &[ptr.into(), self.usize_ty.into(), self.usize_ty.into(), self.usize_ty.into(), a],
                    false,
                )
            }
        }
    }

    /// The `{ ptr, len, size, index }` layout of the chunks and windows iterators: the slice view, the chunk or
    /// window size, and the next index.
    pub(super) fn slice_iter_ty(&self) -> StructType<'ctx> {
        let ptr = self.cx.ptr_type(AddressSpace::default());
        self.cx.struct_type(&[ptr.into(), self.usize_ty.into(), self.usize_ty.into(), self.usize_ty.into()], false)
    }

    /// A `Heap<T>` is its pointer, or `{ ptr, allocator }` when the allocator is stored.
    pub(super) fn heap_ty(&self, alloc: &burdin_tir::Alloc) -> BasicTypeEnum<'ctx> {
        let ptr = self.cx.ptr_type(AddressSpace::default()).into();
        match alloc.allocator_ty() {
            None => ptr,
            Some(a) => {
                let a = self.storage_ty(a);
                self.cx.struct_type(&[ptr, a], false).into()
            }
        }
    }

    pub(super) fn product_ty(&self, fields: &[Ty]) -> StructType<'ctx> {
        let members: Vec<BasicTypeEnum<'ctx>> = fields.iter().map(|t| self.storage_ty(t)).collect();
        self.cx.struct_type(&members, false)
    }

    /// The field types of a tuple or struct.
    pub(super) fn field_tys(&self, ty: &Ty) -> Vec<Ty> {
        match ty {
            Ty::Tuple(elems) => elems.clone(),
            Ty::Struct(a) => self.program.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn variant_tys(&self, enum_id: usize, variant: usize) -> Vec<Ty> {
        self.program.enums[enum_id].variants[variant].fields.iter().map(|f| f.ty.clone()).collect()
    }

    pub(super) fn variant_ty(&self, enum_id: usize, variant: usize) -> StructType<'ctx> {
        self.product_ty(&self.variant_tys(enum_id, variant))
    }

    pub(super) fn enum_ty(&self, enum_id: usize) -> StructType<'ctx> {
        let tag = self.cx.i32_type().into();
        let mut size = 0u64;
        let mut align = 1u32;
        for (i, v) in self.program.enums[enum_id].variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            let vt = self.variant_ty(enum_id, i);
            size = size.max(self.target_data.get_abi_size(&vt));
            align = align.max(self.target_data.get_abi_alignment(&vt));
        }
        if size == 0 {
            // A variant whose fields all have no value (`Ok(())`) still has a payload slot, so the code that
            // addresses the payload of a variant need not know that.
            let has_fields = self.program.enums[enum_id].variants.iter().any(|v| !v.fields.is_empty());
            if has_fields {
                return self.cx.struct_type(&[tag, self.cx.i8_type().array_type(0).into()], false);
            }
            return self.cx.struct_type(&[tag], false);
        }
        let align = align.min(8);
        let unit = match align {
            1 => self.cx.i8_type(),
            2 => self.cx.i16_type(),
            4 => self.cx.i32_type(),
            _ => self.cx.i64_type(),
        };
        let count = size.div_ceil(u64::from(align));
        self.cx.struct_type(&[tag, unit.array_type(count as u32).into()], false)
    }

    /// The scratch layout a runtime operation fills in for an `Option<inner>`: `{ i1 is_some, T }`. It is not the
    /// layout of the type (that is `std`'s enum, `enum_ty`): [`Codegen::option_from_scratch`] converts a finished one.
    pub(super) fn option_ty(&self, inner: &Ty) -> StructType<'ctx> {
        match self.llvm_ty(inner) {
            Some(t) => self.cx.struct_type(&[self.cx.bool_type().into(), t], false),
            None => self.cx.struct_type(&[self.cx.bool_type().into()], false),
        }
    }

    /// The scratch layout of a `Result<ok, err>` an operation fills in: `{ i1 is_ok, T, E }`, converted by
    /// [`Codegen::result_from_scratch`].
    pub(super) fn result_ty(&self, ok: &Ty, err: &Ty) -> StructType<'ctx> {
        self.cx.struct_type(&[self.cx.bool_type().into(), self.storage_ty(ok), self.storage_ty(err)], false)
    }

    /// The payload type of an `Option` instance (`std`'s enum), `None` for any other type.
    pub(super) fn option_payload_ty(&self, ty: &Ty) -> Option<Ty> {
        self.defs().option_payload(ty).cloned()
    }

    /// The `Ok` and `Err` types of a `Result` instance (`std`'s enum), `None` for any other type.
    pub(super) fn result_payload_tys(&self, ty: &Ty) -> Option<(Ty, Ty)> {
        self.defs().result_parts(ty).map(|(ok, err)| (ok.clone(), err.clone()))
    }

    /// The element type and allocator of a `List` (the compiler's list struct in `std`), `None` for any other type.
    /// The element is boxed so a caller can bind it the way the old `Ty::List` pattern did.
    pub(super) fn list_info(&self, ty: &Ty) -> Option<(Box<Ty>, burdin_tir::Alloc)> {
        self.defs().list_parts(ty).map(|(e, a)| (Box::new(e.clone()), a))
    }

    /// The same for a `ListIntoIter`.
    pub(super) fn list_iter_info(&self, ty: &Ty) -> Option<(Box<Ty>, burdin_tir::Alloc)> {
        self.defs().list_into_iter_parts(ty).map(|(e, a)| (Box::new(e.clone()), a))
    }

    /// Whether `ty` is a `List`, a `ListIntoIter`, or an `ArrayIntoIter`: types whose values the compiler also builds
    /// in a plain layout of its own (the `{ ptr, len, cap }` words of the runtime helpers, or an array and an index).
    pub(super) fn is_seq_struct(&self, ty: &Ty) -> bool {
        self.list_info(ty).is_some()
            || self.list_iter_info(ty).is_some()
            || self.defs().array_into_iter_elem(ty).is_some()
    }

    pub(super) fn defs(&self) -> TypeDefs<'a> {
        TypeDefs { structs: &self.program.structs, enums: &self.program.enums }
    }

    /// The C layout size or alignment of `ty` in bytes, for `Type.size()` and `Type.align()`.
    pub(super) fn layout_size(&self, ty: &Ty, alignment: bool) -> u64 {
        match self.llvm_ty(ty) {
            Some(t) if alignment => u64::from(self.target_data.get_abi_alignment(&t)),
            Some(t) => self.target_data.get_abi_size(&t),
            None => u64::from(alignment),
        }
    }

    /// Whether values of `ty` are copied instead of moved.
    pub(super) fn is_copy(&self, ty: &Ty) -> bool {
        self.defs().is_copy(ty)
    }

    /// Whether dropping a value of `ty` runs any code.
    pub(super) fn needs_drop(&self, ty: &Ty) -> bool {
        self.defs().needs_drop(ty)
    }
}
