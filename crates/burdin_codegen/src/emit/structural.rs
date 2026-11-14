//! Structural comparison and hashing: the derived `Eq`, `Ord`, and `Hash`.

use super::{Codegen, GenResult, llvm_err};
use burdin_tir::Ty;
use inkwell::values::{IntValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// Whether the values at `l` and `r` (of type `ty`) are equal, field by field.
    pub(super) fn eq_at(&mut self, l: PointerValue<'ctx>, r: PointerValue<'ctx>, ty: &Ty) -> GenResult<IntValue<'ctx>> {
        let bool_ty = self.cx.bool_type();
        let yes = bool_ty.const_int(1, false);
        let Some(t) = self.llvm_ty(ty) else { return Ok(yes) };
        match ty {
            Ty::Int(_) | Ty::Bool | Ty::Char => {
                let a = self.builder.build_load(t, l, "").map_err(llvm_err)?.into_int_value();
                let b = self.builder.build_load(t, r, "").map_err(llvm_err)?.into_int_value();
                self.builder.build_int_compare(IntPredicate::EQ, a, b, "").map_err(llvm_err)
            }
            Ty::Float(_) => {
                let a = self.builder.build_load(t, l, "").map_err(llvm_err)?.into_float_value();
                let b = self.builder.build_load(t, r, "").map_err(llvm_err)?.into_float_value();
                self.builder.build_float_compare(FloatPredicate::OEQ, a, b, "").map_err(llvm_err)
            }
            Ty::Tuple(_) | Ty::Struct(_) => {
                let st = t.into_struct_type();
                let mut acc = yes;
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    let a = self.builder.build_struct_gep(st, l, i as u32, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(st, r, i as u32, "").map_err(llvm_err)?;
                    let eq = self.eq_at(a, b, fty)?;
                    acc = self.builder.build_and(acc, eq, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Ty::Option(inner) => {
                let ot = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(ot, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(ot, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(bool_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(bool_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tags = self.builder.build_int_compare(IntPredicate::EQ, a_tag, b_tag, "").map_err(llvm_err)?;
                if self.llvm_ty(inner).is_none() {
                    return Ok(tags);
                }
                let a = self.builder.build_struct_gep(ot, l, 1, "").map_err(llvm_err)?;
                let b = self.builder.build_struct_gep(ot, r, 1, "").map_err(llvm_err)?;
                let payload = self.eq_at(a, b, inner)?;
                let payload = self.builder.build_select(a_tag, payload, yes, "").map_err(llvm_err)?.into_int_value();
                self.builder.build_and(tags, payload, "").map_err(llvm_err)
            }
            Ty::Enum(a) => {
                let et = t.into_struct_type();
                let i32_ty = self.cx.i32_type();
                let a_tag = self.builder.build_struct_gep(et, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(et, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(i32_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(i32_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tags = self.builder.build_int_compare(IntPredicate::EQ, a_tag, b_tag, "").map_err(llvm_err)?;
                let mut payload = yes;
                for vi in 0..self.program.enums[a.id].variants.len() {
                    let tys = self.variant_tys(a.id, vi);
                    if tys.is_empty() {
                        continue;
                    }
                    let vt = self.variant_ty(a.id, vi);
                    let lp = self.builder.build_struct_gep(et, l, 1, "").map_err(llvm_err)?;
                    let rp = self.builder.build_struct_gep(et, r, 1, "").map_err(llvm_err)?;
                    let mut acc = yes;
                    for (i, fty) in tys.iter().enumerate() {
                        let a = self.builder.build_struct_gep(vt, lp, i as u32, "").map_err(llvm_err)?;
                        let b = self.builder.build_struct_gep(vt, rp, i as u32, "").map_err(llvm_err)?;
                        let eq = self.eq_at(a, b, fty)?;
                        acc = self.builder.build_and(acc, eq, "").map_err(llvm_err)?;
                    }
                    let is_this = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, a_tag, i32_ty.const_int(vi as u64, false), "")
                        .map_err(llvm_err)?;
                    payload = self.builder.build_select(is_this, acc, payload, "").map_err(llvm_err)?.into_int_value();
                }
                self.builder.build_and(tags, payload, "").map_err(llvm_err)
            }
            _ => Err(format!("unsupported `==` on `{ty}`")),
        }
    }

    /// `-1`, `0`, or `1` (an `i32`) for how the integers `a` and `b` compare.
    pub(super) fn cmp_ints(&mut self, a: IntValue<'ctx>, b: IntValue<'ctx>, signed: bool) -> GenResult<IntValue<'ctx>> {
        let (lt, gt) =
            if signed { (IntPredicate::SLT, IntPredicate::SGT) } else { (IntPredicate::ULT, IntPredicate::UGT) };
        let i32_ty = self.cx.i32_type();
        let less = self.builder.build_int_compare(lt, a, b, "").map_err(llvm_err)?;
        let greater = self.builder.build_int_compare(gt, a, b, "").map_err(llvm_err)?;
        let not_less = self
            .builder
            .build_select(greater, i32_ty.const_int(1, false), i32_ty.const_zero(), "")
            .map_err(llvm_err)?
            .into_int_value();
        let all_ones = i32_ty.const_all_ones();
        Ok(self.builder.build_select(less, all_ones, not_less, "").map_err(llvm_err)?.into_int_value())
    }

    /// The first non-zero ordering, or zero: how the fields of a struct or tuple compare in order.
    pub(super) fn first_difference(&mut self, orders: &[IntValue<'ctx>]) -> GenResult<IntValue<'ctx>> {
        let i32_ty = self.cx.i32_type();
        let mut acc = i32_ty.const_zero();
        for &o in orders.iter().rev() {
            let same =
                self.builder.build_int_compare(IntPredicate::EQ, o, i32_ty.const_zero(), "").map_err(llvm_err)?;
            acc = self.builder.build_select(same, acc, o, "").map_err(llvm_err)?.into_int_value();
        }
        Ok(acc)
    }

    /// How the values at `l` and `r` (of type `ty`, which derives `Ord`) compare: `-1`, `0`, or `1` as an `i32`.
    /// Fields compare in declaration order, enum variants by their position.
    pub(super) fn cmp_at(
        &mut self,
        l: PointerValue<'ctx>,
        r: PointerValue<'ctx>,
        ty: &Ty,
    ) -> GenResult<IntValue<'ctx>> {
        let i32_ty = self.cx.i32_type();
        let Some(t) = self.llvm_ty(ty) else { return Ok(i32_ty.const_zero()) };
        match ty {
            Ty::Int(_) | Ty::Bool | Ty::Char => {
                let signed = matches!(ty, Ty::Int(i) if i.signed());
                let a = self.builder.build_load(t, l, "").map_err(llvm_err)?.into_int_value();
                let b = self.builder.build_load(t, r, "").map_err(llvm_err)?.into_int_value();
                self.cmp_ints(a, b, signed)
            }
            Ty::Tuple(_) | Ty::Struct(_) => {
                let st = t.into_struct_type();
                let mut orders = Vec::new();
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    let a = self.builder.build_struct_gep(st, l, i as u32, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(st, r, i as u32, "").map_err(llvm_err)?;
                    orders.push(self.cmp_at(a, b, fty)?);
                }
                self.first_difference(&orders)
            }
            Ty::Enum(a) => {
                let et = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(et, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(et, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(i32_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(i32_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tag_order = self.cmp_ints(a_tag, b_tag, false)?;
                let mut payload = i32_ty.const_zero();
                for vi in 0..self.program.enums[a.id].variants.len() {
                    let tys = self.variant_tys(a.id, vi);
                    if tys.is_empty() {
                        continue;
                    }
                    let vt = self.variant_ty(a.id, vi);
                    let lp = self.builder.build_struct_gep(et, l, 1, "").map_err(llvm_err)?;
                    let rp = self.builder.build_struct_gep(et, r, 1, "").map_err(llvm_err)?;
                    let mut orders = Vec::new();
                    for (i, fty) in tys.iter().enumerate() {
                        let x = self.builder.build_struct_gep(vt, lp, i as u32, "").map_err(llvm_err)?;
                        let y = self.builder.build_struct_gep(vt, rp, i as u32, "").map_err(llvm_err)?;
                        orders.push(self.cmp_at(x, y, fty)?);
                    }
                    let variant_order = self.first_difference(&orders)?;
                    let is_this = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, a_tag, i32_ty.const_int(vi as u64, false), "")
                        .map_err(llvm_err)?;
                    payload = self
                        .builder
                        .build_select(is_this, variant_order, payload, "")
                        .map_err(llvm_err)?
                        .into_int_value();
                }
                let same_tag = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tag_order, i32_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                Ok(self.builder.build_select(same_tag, payload, tag_order, "").map_err(llvm_err)?.into_int_value())
            }
            _ => Err(format!("unsupported ordering on `{ty}`")),
        }
    }

    /// Mixes `x` into a running 64-bit hash (FNV-1a style).
    pub(super) fn hash_mix(&mut self, acc: IntValue<'ctx>, x: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let i64_ty = self.cx.i64_type();
        let x = if x.get_type().get_bit_width() < 64 {
            self.builder.build_int_z_extend(x, i64_ty, "").map_err(llvm_err)?
        } else {
            x
        };
        let mixed = self.builder.build_xor(acc, x, "").map_err(llvm_err)?;
        self.builder.build_int_mul(mixed, i64_ty.const_int(0x0000_0100_0000_01b3, false), "").map_err(llvm_err)
    }

    /// Folds the value at `ptr` (of type `ty`, which derives `Hash`) into the hash `acc`.
    pub(super) fn hash_at(
        &mut self,
        ptr: PointerValue<'ctx>,
        ty: &Ty,
        acc: IntValue<'ctx>,
    ) -> GenResult<IntValue<'ctx>> {
        let Some(t) = self.llvm_ty(ty) else { return Ok(acc) };
        match ty {
            Ty::Int(_) | Ty::Bool | Ty::Char => {
                let v = self.builder.build_load(t, ptr, "").map_err(llvm_err)?.into_int_value();
                let signed = matches!(ty, Ty::Int(i) if i.signed());
                let v = if signed && v.get_type().get_bit_width() < 64 {
                    self.builder.build_int_s_extend(v, self.cx.i64_type(), "").map_err(llvm_err)?
                } else {
                    v
                };
                self.hash_mix(acc, v)
            }
            Ty::Tuple(_) | Ty::Struct(_) => {
                let st = t.into_struct_type();
                let mut acc = acc;
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    let f = self.builder.build_struct_gep(st, ptr, i as u32, "").map_err(llvm_err)?;
                    acc = self.hash_at(f, fty, acc)?;
                }
                Ok(acc)
            }
            Ty::Option(inner) => {
                let ot = t.into_struct_type();
                let tag_ptr = self.builder.build_struct_gep(ot, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let with_tag = self.hash_mix(acc, tag)?;
                if self.llvm_ty(inner).is_none() {
                    return Ok(with_tag);
                }
                let payload_ptr = self.builder.build_struct_gep(ot, ptr, 1, "").map_err(llvm_err)?;
                let with_payload = self.hash_at(payload_ptr, inner, with_tag)?;
                Ok(self.builder.build_select(tag, with_payload, with_tag, "").map_err(llvm_err)?.into_int_value())
            }
            Ty::Enum(a) => {
                let et = t.into_struct_type();
                let i32_ty = self.cx.i32_type();
                let tag_ptr = self.builder.build_struct_gep(et, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(i32_ty, tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let with_tag = self.hash_mix(acc, tag)?;
                let mut result = with_tag;
                for vi in 0..self.program.enums[a.id].variants.len() {
                    let tys = self.variant_tys(a.id, vi);
                    if tys.is_empty() {
                        continue;
                    }
                    let vt = self.variant_ty(a.id, vi);
                    let payload = self.builder.build_struct_gep(et, ptr, 1, "").map_err(llvm_err)?;
                    let mut acc_v = with_tag;
                    for (i, fty) in tys.iter().enumerate() {
                        let f = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                        acc_v = self.hash_at(f, fty, acc_v)?;
                    }
                    let is_this = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, tag, i32_ty.const_int(vi as u64, false), "")
                        .map_err(llvm_err)?;
                    result = self.builder.build_select(is_this, acc_v, result, "").map_err(llvm_err)?.into_int_value();
                }
                Ok(result)
            }
            _ => Err(format!("unsupported hash of `{ty}`")),
        }
    }
}
