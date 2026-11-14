//! Building aggregates: repeated arrays, structs, tuples, and enum variants.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, Ty};
use inkwell::IntPredicate;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValueEnum, PointerValue};

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
        let Ty::Enum(a) = ty else { unreachable!("a variant has an enum type") };
        let mut values = Vec::new();
        for (i, e) in fields {
            let v = self.expr_mode(e, Mode::Move)?;
            values.push((*i, v, &e.ty));
        }
        let et = self.enum_ty(a.id);
        let slot = self.alloca(et.into(), "variant")?;
        let tag_ptr = self.builder.build_struct_gep(et, slot, 0, "").map_err(llvm_err)?;
        self.builder.build_store(tag_ptr, self.cx.i32_type().const_int(index as u64, false)).map_err(llvm_err)?;
        if !values.is_empty() {
            let payload = self.builder.build_struct_gep(et, slot, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(a.id, index);
            for (i, v, fty) in values {
                if self.llvm_ty(fty).is_some() {
                    let v = self.need(v, fty);
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    self.builder.build_store(p, v).map_err(llvm_err)?;
                }
            }
        }
        Ok(Some(self.builder.build_load(et, slot, "").map_err(llvm_err)?))
    }
}
