//! Fallible allocation (`spec/allocators.md`, AL11): the `try_` forms of the allocating operations. A fallible
//! operation computes a status word rather than panicking, and [`Codegen::try_result`] turns the word into the
//! `Result<T, AllocError>` the program sees. The operations themselves live with their containers.

use super::{Codegen, GenResult, llvm_err};
use burdin_tir::Ty;
use inkwell::IntPredicate;
use inkwell::values::{BasicValue, BasicValueEnum, IntValue};

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
        let Some((ok_ty, err_ty)) = self.result_payload_tys(result) else {
            return Err("a fallible allocation returns a Result".into());
        };
        let Ty::Enum(err) = &err_ty else {
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
        let error: BasicValueEnum<'ctx> =
            self.builder.build_insert_value(et.const_zero(), variant, 0, "").map_err(llvm_err)?.as_basic_value_enum();
        // The `Ok` and `Err` payloads share storage, so each is built in its own branch.
        let rt = self.llvm_ty(result).expect("a Result has a type");
        let slot = self.alloca(rt, "try.result")?;
        let ok_bb = self.new_block("try.ok");
        let err_bb = self.new_block("try.err");
        let end = self.new_block("try.end");
        self.builder.build_conditional_branch(is_ok, ok_bb, err_bb).map_err(llvm_err)?;
        self.builder.position_at_end(ok_bb);
        let ok_payload = match ok {
            Some(v) if self.llvm_ty(&ok_ty).is_some() => Some(self.need(Some(v), &ok_ty)),
            _ => None,
        };
        let ok_value = self.make_result(result, true, ok_payload)?;
        self.builder.build_store(slot, ok_value).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(err_bb);
        let err_value = self.make_result(result, false, Some(error))?;
        self.builder.build_store(slot, err_value).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.builder.build_load(rt, slot, "").map_err(llvm_err)
    }
}
