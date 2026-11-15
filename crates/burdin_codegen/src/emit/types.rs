//! LLVM types for Burdin types, and the questions about them that code generation asks.

use super::Codegen;
use burdin_tir::{FloatTy, IntTy, Ty, TypeDefs};
use inkwell::AddressSpace;
use inkwell::types::{BasicType, BasicTypeEnum, IntType, StructType};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn int_ty(&self, i: IntTy) -> IntType<'ctx> {
        match i {
            IntTy::Isize | IntTy::Usize => self.usize_ty,
            IntTy::I8 | IntTy::U8 => self.cx.i8_type(),
            IntTy::I16 | IntTy::U16 => self.cx.i16_type(),
            IntTy::I32 | IntTy::U32 => self.cx.i32_type(),
            IntTy::I64 | IntTy::U64 => self.cx.i64_type(),
        }
    }

    /// The LLVM type of a value of `ty`, or `None` for types without a runtime value.
    pub(super) fn llvm_ty(&self, ty: &Ty) -> Option<BasicTypeEnum<'ctx>> {
        Some(match ty {
            Ty::Unit | Ty::Never | Ty::Error | Ty::Param(_) => return None,
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
            // A list and a map have the same three-word shape as a `String`: pointer, length, capacity.
            Ty::String | Ty::List(_) | Ty::Map(..) => self.string_ty.into(),
            Ty::Result(ok, err) => self.result_ty(ok, err).into(),
            // A `CString` is the address of its NUL-terminated buffer; a `CPtr` is a raw address.
            Ty::CString | Ty::CPtr(_) => self.cx.ptr_type(AddressSpace::default()).into(),
            Ty::Heap(_) => self.cx.ptr_type(AddressSpace::default()).into(),
            // A reference is the address of the referent; a slice is a pointer and a length, like a `str`.
            Ty::Ref { .. } => self.cx.ptr_type(AddressSpace::default()).into(),
            Ty::Slice(_) => self.str_ty.into(),
            Ty::Array(elem, n) => self.llvm_ty(elem)?.array_type(*n as u32).into(),
            Ty::Option(inner) => self.option_ty(inner).into(),
            Ty::Tuple(_) | Ty::Struct(_) => self.product_ty(&self.field_tys(ty)).into(),
            // A range is two copied endpoints.
            Ty::Range(inner, _) => self.product_ty(&[(**inner).clone(), (**inner).clone()]).into(),
            // An iterator over a list or a map keeps the collection's three words and the next index.
            Ty::ListIntoIter(_) | Ty::MapIter(..) | Ty::MapIntoIter(..) => {
                let ptr = self.cx.ptr_type(AddressSpace::default());
                self.cx
                    .struct_type(&[ptr.into(), self.usize_ty.into(), self.usize_ty.into(), self.usize_ty.into()], false)
                    .into()
            }
            // The array owns its elements inline; the index says which one comes next.
            Ty::ArrayIntoIter(inner, n) => self.iter_array_ty(inner, *n).into(),
            // The range iterator keeps both endpoints and whether the range is exhausted.
            Ty::RangeIter(inner, _) => self.product_ty(&[(**inner).clone(), (**inner).clone(), Ty::Bool]).into(),
            Ty::Enum(a) => self.enum_ty(a.id).into(),
        })
    }

    /// The type of a member of an aggregate: like `llvm_ty`, but a value-less type is an empty struct.
    pub(super) fn storage_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        self.llvm_ty(ty).unwrap_or_else(|| self.cx.struct_type(&[], false).into())
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

    pub(super) fn option_ty(&self, inner: &Ty) -> StructType<'ctx> {
        match self.llvm_ty(inner) {
            Some(t) => self.cx.struct_type(&[self.cx.bool_type().into(), t], false),
            None => self.cx.struct_type(&[self.cx.bool_type().into()], false),
        }
    }

    /// `Result<T, E>`: a tag and both payloads. The inactive payload is undefined and never dropped.
    pub(super) fn result_ty(&self, ok: &Ty, err: &Ty) -> StructType<'ctx> {
        self.cx.struct_type(&[self.cx.bool_type().into(), self.storage_ty(ok), self.storage_ty(err)], false)
    }

    fn defs(&self) -> TypeDefs<'a> {
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
