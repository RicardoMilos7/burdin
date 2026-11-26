//! Building aggregates: repeated arrays, structs, tuples, and enum variants.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, Ty};
use inkwell::IntPredicate;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValue, BasicValueEnum, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn repeat(
        &mut self,
        v: BasicValueEnum<'ctx>,
        elem_t: BasicTypeEnum<'ctx>,
        array_t: BasicTypeEnum<'ctx>,
        n: u64,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let slot = self.alloca(array_t, "repeat")?;
        let counter = self.alloca(self.usize_ty.into(), "repeat.i")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("repeat.cond");
        let body = self.new_block("repeat.body");
        let exit = self.new_block("repeat.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "").map_err(llvm_err)?.into_int_value();
        let n_v = self.usize_ty.const_int(n, false);
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, n_v, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // SAFETY (LLVM): i < n.
        let ptr = unsafe { self.builder.build_in_bounds_gep(elem_t, slot, &[i], "") }.map_err(llvm_err)?;
        self.builder.build_store(ptr, v).map_err(llvm_err)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        self.builder.build_load(array_t, slot, "").map_err(llvm_err)
    }

    /// Stores a value in a fresh stack slot and returns its address (`None` for types without a value).
    pub(super) fn spill(&mut self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> GenResult<Option<PointerValue<'ctx>>> {
        let Some(t) = self.llvm_ty(ty) else { return Ok(None) };
        let slot = self.alloca(t, "tmp")?;
        let v = self.need(v, ty);
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        Ok(Some(slot))
    }

    /// A tuple or struct value from field indices and values (in evaluation order).
    pub(super) fn product(
        &mut self,
        fields: &[(usize, &'a tir::Expr)],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mut values = Vec::new();
        for (i, e) in fields {
            let v = self.expr_mode(e, Mode::Move)?;
            values.push((*i, v, &e.ty));
        }
        let Some(t) = self.llvm_ty(ty) else { return Ok(None) };
        let mut agg = undef(t).into_struct_value();
        for (i, v, fty) in values {
            if self.llvm_ty(fty).is_some() {
                let v = self.need(v, fty);
                agg = self.builder.build_insert_value(agg, v, i as u32, "").map_err(llvm_err)?.into_struct_value();
            }
        }
        Ok(Some(agg.into()))
    }

    pub(super) fn variant(
        &mut self,
        index: usize,
        fields: &'a [(usize, tir::Expr)],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mut values = Vec::new();
        for (i, e) in fields {
            let v = self.expr_mode(e, Mode::Move)?;
            values.push((*i, v, e.ty.clone()));
        }
        Ok(Some(self.build_variant(ty, index, values)?))
    }

    /// A value of the enum type `ty`: variant `index`, with its fields' values. A field whose type has no runtime
    /// value is skipped, and the payload bytes a variant does not use are left as they were.
    pub(super) fn build_variant(
        &mut self,
        ty: &Ty,
        index: usize,
        values: Vec<(usize, Option<BasicValueEnum<'ctx>>, Ty)>,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let Ty::Enum(a) = ty else { unreachable!("a variant has an enum type") };
        let et = self.enum_ty(a.id);
        let slot = self.alloca(et.into(), "variant")?;
        // The slot starts zeroed, so a variant without fields (and its padding) is deterministic.
        self.builder.build_store(slot, et.const_zero()).map_err(llvm_err)?;
        let tag_ptr = self.builder.build_struct_gep(et, slot, 0, "").map_err(llvm_err)?;
        self.builder.build_store(tag_ptr, self.cx.i32_type().const_int(index as u64, false)).map_err(llvm_err)?;
        if values.iter().any(|(_, _, fty)| self.llvm_ty(fty).is_some()) {
            let payload = self.builder.build_struct_gep(et, slot, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(a.id, index);
            for (i, v, fty) in values {
                if self.llvm_ty(&fty).is_some() {
                    let v = self.need(v, &fty);
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    self.builder.build_store(p, v).map_err(llvm_err)?;
                }
            }
        }
        self.builder.build_load(et, slot, "").map_err(llvm_err)
    }

    /// Converts a finished scratch `Option` (`option_ty`: `{ i1 is_some, T }`) into a value of the `Option` type `ty`.
    pub(super) fn option_from_scratch(
        &mut self,
        scratch: BasicValueEnum<'ctx>,
        ty: &Ty,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let inner = self.option_payload_ty(ty).ok_or("an `Option` value is built for an `Option` type")?;
        let Ty::Enum(a) = ty else { unreachable!("an Option is an enum") };
        let scratch = scratch.into_struct_value();
        let tag = self.builder.build_extract_value(scratch, 0, "is_some").map_err(llvm_err)?.into_int_value();
        let et = self.enum_ty(a.id);
        let slot = self.alloca(et.into(), "option.out")?;
        self.builder.build_store(slot, et.const_zero()).map_err(llvm_err)?;
        let tag_ptr = self.builder.build_struct_gep(et, slot, 0, "").map_err(llvm_err)?;
        let wide = self.builder.build_int_z_extend(tag, self.cx.i32_type(), "").map_err(llvm_err)?;
        self.builder.build_store(tag_ptr, wide).map_err(llvm_err)?;
        if self.llvm_ty(&inner).is_some() {
            // The payload of a `None` is whatever the scratch held and is never read.
            let payload = self.builder.build_extract_value(scratch, 1, "payload").map_err(llvm_err)?;
            let pl = self.builder.build_struct_gep(et, slot, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(a.id, 1);
            let p = self.builder.build_struct_gep(vt, pl, 0, "").map_err(llvm_err)?;
            self.builder.build_store(p, payload).map_err(llvm_err)?;
        }
        self.builder.build_load(et, slot, "").map_err(llvm_err)
    }

    /// Converts a finished scratch `Result` (`result_ty`: `{ i1 is_ok, T, E }`) into a value of the `Result` type `ty`.
    pub(super) fn result_from_scratch(
        &mut self,
        scratch: BasicValueEnum<'ctx>,
        ty: &Ty,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let (ok_ty, err_ty) = self.result_payload_tys(ty).ok_or("a `Result` value is built for a `Result` type")?;
        let scratch = scratch.into_struct_value();
        let is_ok = self.builder.build_extract_value(scratch, 0, "is_ok").map_err(llvm_err)?.into_int_value();
        let et = self.llvm_ty(ty).expect("a Result has a type");
        let slot = self.alloca(et, "result.out")?;
        let ok_bb = self.new_block("result.scratch.ok");
        let err_bb = self.new_block("result.scratch.err");
        let end = self.new_block("result.scratch.end");
        self.builder.build_conditional_branch(is_ok, ok_bb, err_bb).map_err(llvm_err)?;
        self.builder.position_at_end(ok_bb);
        let payload = if self.llvm_ty(&ok_ty).is_some() {
            Some(self.builder.build_extract_value(scratch, 1, "ok").map_err(llvm_err)?.as_basic_value_enum())
        } else {
            None
        };
        let v = self.make_result(ty, true, payload)?;
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(err_bb);
        let payload = if self.llvm_ty(&err_ty).is_some() {
            Some(self.builder.build_extract_value(scratch, 2, "err").map_err(llvm_err)?.as_basic_value_enum())
        } else {
            None
        };
        let v = self.make_result(ty, false, payload)?;
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.builder.build_load(et, slot, "").map_err(llvm_err)
    }

    /// Loads the scratch `Result` in `slot` and converts it into a value of the `Result` type `ty`.
    pub(super) fn finish_result(
        &mut self,
        scratch_ty: inkwell::types::StructType<'ctx>,
        slot: PointerValue<'ctx>,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let scratch = self.builder.build_load(scratch_ty, slot, "").map_err(llvm_err)?;
        Ok(Some(self.result_from_scratch(scratch, ty)?))
    }

    /// Loads the scratch `Option` in `slot` and converts it into a value of the `Option` type `ty`.
    pub(super) fn finish_option(
        &mut self,
        scratch_ty: inkwell::types::StructType<'ctx>,
        slot: PointerValue<'ctx>,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let scratch = self.builder.build_load(scratch_ty, slot, "").map_err(llvm_err)?;
        Ok(Some(self.option_from_scratch(scratch, ty)?))
    }

    /// `Ok(payload)` or `Err(payload)` of the `Result` type `ty`.
    pub(super) fn make_result(
        &mut self,
        ty: &Ty,
        ok: bool,
        payload: Option<BasicValueEnum<'ctx>>,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let (ok_ty, err_ty) = self.result_payload_tys(ty).ok_or("`Ok` and `Err` build a `Result`")?;
        let payload_ty = if ok { ok_ty } else { err_ty };
        self.build_variant(ty, usize::from(!ok), vec![(0, payload, payload_ty)])
    }
}
