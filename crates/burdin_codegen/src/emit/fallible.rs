//! Fallible allocation (`spec/allocators.md`, AL11): the `try_` forms of the allocating operations. A fallible
//! operation computes a status word rather than panicking, and [`Codegen::try_result`] turns the word into the
//! `Result<T, AllocError>` the program sees. The operations themselves live with their containers.

use super::{Codegen, GenResult, llvm_err};
use burdin_tir::Ty;
use inkwell::IntPredicate;
use inkwell::values::{BasicValueEnum, IntValue};

/// The status word of a fallible operation that succeeded.
pub(super) const TRY_OK: u64 = 0;
/// The requested capacity cannot be addressed: `AllocError.CapacityOverflow`.
pub(super) const TRY_OVERFLOW: u64 = 1;
/// The allocator had no block to give: `AllocError.OutOfMemory`.
pub(super) const TRY_OUT_OF_MEMORY: u64 = 2;

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The `Result` a fallible operation returns, for its status word `status` (an `i32`). `Ok` carries `ok` when
    /// the `Ok` type has a runtime value; otherwise the error is the `AllocError` variant the status names. `result`
    /// is the `Result<T, AllocError>` type of the expression. `AllocError` declares `CapacityOverflow` first and
    /// `OutOfMemory` second (`std.bd`), which is the order this function relies on.
    pub(super) fn try_result(
        &mut self,
        status: IntValue<'ctx>,
        ok: Option<BasicValueEnum<'ctx>>,
        result: &Ty,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let Ty::Result(ok_ty, err_ty) = result else {
            return Err("a fallible allocation returns a Result".into());
        };
        let Ty::Enum(err) = &**err_ty else {
            return Err("`AllocError` is an enum".into());
        };
        let i32_ty = self.cx.i32_type();
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, status, i32_ty.const_int(TRY_OK, false), "try.ok")
            .map_err(llvm_err)?;
        let is_overflow = self
            .builder
            .build_int_compare(IntPredicate::EQ, status, i32_ty.const_int(TRY_OVERFLOW, false), "try.overflow")
            .map_err(llvm_err)?;
        let variant = self
            .builder
            .build_select(is_overflow, i32_ty.const_zero(), i32_ty.const_int(1, false), "try.variant")
            .map_err(llvm_err)?
            .into_int_value();
        let et = self.enum_ty(err.id);
        let error =
            self.builder.build_insert_value(et.const_zero(), variant, 0, "").map_err(llvm_err)?.into_struct_value();
        let rt = self.result_ty(ok_ty, err_ty);
        let mut agg =
            self.builder.build_insert_value(rt.get_undef(), is_ok, 0, "").map_err(llvm_err)?.into_struct_value();
        if let Some(v) = ok
            && self.llvm_ty(ok_ty).is_some()
        {
            let v = self.need(Some(v), ok_ty);
            agg = self.builder.build_insert_value(agg, v, 1, "").map_err(llvm_err)?.into_struct_value();
        }
        agg = self.builder.build_insert_value(agg, error, 2, "").map_err(llvm_err)?.into_struct_value();
        Ok(agg.into())
    }
}
