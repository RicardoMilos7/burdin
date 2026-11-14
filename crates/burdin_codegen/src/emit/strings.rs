//! Strings, string literals, and slices.

use super::place::is_place;
use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_diag::Span;
use burdin_tir::{self as tir, Ty};
use inkwell::AddressSpace;
use inkwell::module::Linkage;
use inkwell::values::{BasicValueEnum, IntValue, StructValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn make_str(
        &mut self,
        ptr: BasicValueEnum<'ctx>,
        len: BasicValueEnum<'ctx>,
    ) -> GenResult<StructValue<'ctx>> {
        let s = undef(self.str_ty.into()).into_struct_value();
        let s = self.builder.build_insert_value(s, ptr, 0, "").map_err(llvm_err)?;
        Ok(self.builder.build_insert_value(s, len, 1, "").map_err(llvm_err)?.into_struct_value())
    }

    /// A `str` constant pointing to a private global holding the bytes (no terminating NUL).
    pub(super) fn str_literal(&mut self, bytes: &[u8]) -> StructValue<'ctx> {
        if let Some(s) = self.str_literals.get(bytes) {
            return *s;
        }
        let data = self.cx.const_string(bytes, false);
        let global = self.module.add_global(data.get_type(), None, &format!("str.{}", self.str_literals.len()));
        global.set_initializer(&data);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        let len = self.usize_ty.const_int(bytes.len() as u64, false);
        let s = self.str_ty.const_named_struct(&[global.as_pointer_value().into(), len.into()]);
        self.str_literals.insert(bytes.to_vec(), s);
        s
    }

    /// The pointer and length of a `str`, `String`, or `[T]` value. A `String` that is not a place is recorded
    /// as a temporary of the statement, so its buffer outlives the view until the statement ends (DR5).
    pub(super) fn view_parts(&mut self, e: &'a tir::Expr) -> GenResult<(BasicValueEnum<'ctx>, BasicValueEnum<'ctx>)> {
        let ty = e.ty.clone();
        let place = is_place(e);
        let v = self.expr_mode(e, Mode::Use)?;
        if !place && ty == Ty::String {
            self.add_temp_value(v, &ty)?;
        }
        let v = self.need(v, &ty);
        let s = v.into_struct_value();
        let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
        let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
        Ok((ptr, len))
    }

    /// An index expression's value, as a `usize`.
    pub(super) fn usize_value(&mut self, e: &'a tir::Expr) -> GenResult<IntValue<'ctx>> {
        let v = self.expr_mode(e, Mode::Use)?;
        Ok(self.need_int(v, &e.ty))
    }

    /// `base[lo..hi]`: a view of bytes of a `str` or `String`, or of elements of a slice or array.
    pub(super) fn slice(
        &mut self,
        base: &'a tir::Expr,
        lo: &'a tir::Expr,
        hi: &'a tir::Expr,
        span: Span,
    ) -> GenResult<StructValue<'ctx>> {
        if let Ty::Array(_, n) = &base.ty {
            let n = *n;
            let base_ptr = self.place_ptr(base)?;
            let base_ptr = base_ptr.unwrap_or_else(|| self.cx.ptr_type(AddressSpace::default()).get_undef());
            let lo_v = self.usize_value(lo)?;
            let hi_v = self.usize_value(hi)?;
            self.bounds_guard_ule(hi_v, self.usize_ty.const_int(n, false), span)?;
            self.bounds_guard_ule(lo_v, hi_v, span)?;
            let array_t = self.llvm_ty(&base.ty).expect("an array has a type");
            let zero = self.usize_ty.const_zero();
            // SAFETY (LLVM): lo <= hi <= n.
            let p =
                unsafe { self.builder.build_in_bounds_gep(array_t, base_ptr, &[zero, lo_v], "") }.map_err(llvm_err)?;
            let len = self.builder.build_int_sub(hi_v, lo_v, "").map_err(llvm_err)?;
            return self.make_str(p.into(), len.into());
        }
        let (ptr, len) = self.view_parts(base)?;
        let lo_v = self.usize_value(lo)?;
        let hi_v = self.usize_value(hi)?;
        self.bounds_guard_ule(hi_v, len.into_int_value(), span)?;
        self.bounds_guard_ule(lo_v, hi_v, span)?;
        // SAFETY (LLVM): lo <= hi <= the base length.
        let p = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), ptr.into_pointer_value(), &[lo_v], "") }
            .map_err(llvm_err)?;
        let len = self.builder.build_int_sub(hi_v, lo_v, "").map_err(llvm_err)?;
        self.make_str(p.into(), len.into())
    }
}
