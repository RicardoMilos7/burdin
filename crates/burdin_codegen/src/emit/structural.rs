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
            Ty::Array(elem, n) => {
                let Some(elem_t) = self.llvm_ty(elem) else { return Ok(yes) };
                let head = self.new_block("eq.array.head");
                let body = self.new_block("eq.array.body");
                let end = self.new_block("eq.array.end");
                let before = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(head).map_err(llvm_err)?;
                self.builder.position_at_end(head);
                let i = self.builder.build_phi(self.usize_ty, "i").map_err(llvm_err)?;
                let index = i.as_basic_value().into_int_value();
                let n_v = self.usize_ty.const_int(*n, false);
                let more = self.builder.build_int_compare(IntPredicate::ULT, index, n_v, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                // SAFETY (LLVM): index < n.
                let a = unsafe { self.builder.build_in_bounds_gep(elem_t, l, &[index], "") }.map_err(llvm_err)?;
                let b = unsafe { self.builder.build_in_bounds_gep(elem_t, r, &[index], "") }.map_err(llvm_err)?;
                let same = self.eq_at(a, b, elem)?;
                let next =
                    self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let body_end = self.builder.get_insert_block().expect("positioned");
                self.builder.build_conditional_branch(same, head, end).map_err(llvm_err)?;
                i.add_incoming(&[(&self.usize_ty.const_zero(), before), (&next, body_end)]);
                self.builder.position_at_end(end);
                let result = self.builder.build_phi(bool_ty, "eq").map_err(llvm_err)?;
                result.add_incoming(&[(&yes, head), (&bool_ty.const_zero(), body_end)]);
                Ok(result.as_basic_value().into_int_value())
            }
            // A map compares by its entries, whatever the iteration order (`spec/stdlib.md`, "Map").
            Ty::Map(key, value, alloc) => self.map_eq(l, r, key, value, alloc),
            Ty::Range(inner, _) => {
                let st = t.into_struct_type();
                let a = self.builder.build_struct_gep(st, l, 0, "").map_err(llvm_err)?;
                let b = self.builder.build_struct_gep(st, r, 0, "").map_err(llvm_err)?;
                let first = self.eq_at(a, b, inner)?;
                let a = self.builder.build_struct_gep(st, l, 1, "").map_err(llvm_err)?;
                let b = self.builder.build_struct_gep(st, r, 1, "").map_err(llvm_err)?;
                let second = self.eq_at(a, b, inner)?;
                self.builder.build_and(first, second, "").map_err(llvm_err)
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
            Ty::Str | Ty::String => {
                let (a_ptr, a_len) = self.parts_at(l, ty)?;
                let (b_ptr, b_len) = self.parts_at(r, ty)?;
                let call = self
                    .builder
                    .build_call(self.rt.str_eq, &[a_ptr.into(), a_len.into(), b_ptr.into(), b_len.into()], "")
                    .map_err(llvm_err)?;
                Ok(call.try_as_basic_value().basic().expect("`str_eq` returns a flag").into_int_value())
            }
            Ty::Result(ok, err) => {
                let rt = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(rt, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(rt, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(bool_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(bool_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tags = self.builder.build_int_compare(IntPredicate::EQ, a_tag, b_tag, "").map_err(llvm_err)?;
                let mut payload = yes;
                if self.llvm_ty(ok).is_some() {
                    let a = self.builder.build_struct_gep(rt, l, 1, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(rt, r, 1, "").map_err(llvm_err)?;
                    let eq = self.eq_at(a, b, ok)?;
                    payload = self.builder.build_select(a_tag, eq, payload, "").map_err(llvm_err)?.into_int_value();
                }
                if self.llvm_ty(err).is_some() {
                    let a = self.builder.build_struct_gep(rt, l, 2, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(rt, r, 2, "").map_err(llvm_err)?;
                    let eq = self.eq_at(a, b, err)?;
                    let is_err = self.builder.build_not(a_tag, "").map_err(llvm_err)?;
                    payload = self.builder.build_select(is_err, eq, payload, "").map_err(llvm_err)?.into_int_value();
                }
                self.builder.build_and(tags, payload, "").map_err(llvm_err)
            }
            _ => Err(format!("unsupported `==` on `{ty}`")),
        }
    }

    /// The pointer and length of a `str` or `String` at `addr`.
    pub(super) fn parts_at(
        &mut self,
        addr: PointerValue<'ctx>,
        ty: &Ty,
    ) -> GenResult<(inkwell::values::BasicValueEnum<'ctx>, inkwell::values::BasicValueEnum<'ctx>)> {
        let t = self.llvm_ty(ty).expect("text has a type").into_struct_type();
        let ptr_ty = self.cx.ptr_type(inkwell::AddressSpace::default());
        let p = self.builder.build_struct_gep(t, addr, 0, "").map_err(llvm_err)?;
        let l = self.builder.build_struct_gep(t, addr, 1, "").map_err(llvm_err)?;
        let p = self.builder.build_load(ptr_ty, p, "").map_err(llvm_err)?;
        let l = self.builder.build_load(self.usize_ty, l, "").map_err(llvm_err)?;
        Ok((p, l))
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
            Ty::Array(elem, n) => {
                let Some(elem_t) = self.llvm_ty(elem) else { return Ok(i32_ty.const_zero()) };
                let head = self.new_block("cmp.array.head");
                let body = self.new_block("cmp.array.body");
                let end = self.new_block("cmp.array.end");
                let before = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(head).map_err(llvm_err)?;
                self.builder.position_at_end(head);
                let i = self.builder.build_phi(self.usize_ty, "i").map_err(llvm_err)?;
                let index = i.as_basic_value().into_int_value();
                let n_v = self.usize_ty.const_int(*n, false);
                let more = self.builder.build_int_compare(IntPredicate::ULT, index, n_v, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                // SAFETY (LLVM): index < n.
                let a = unsafe { self.builder.build_in_bounds_gep(elem_t, l, &[index], "") }.map_err(llvm_err)?;
                let b = unsafe { self.builder.build_in_bounds_gep(elem_t, r, &[index], "") }.map_err(llvm_err)?;
                let order = self.cmp_at(a, b, elem)?;
                let next =
                    self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let body_end = self.builder.get_insert_block().expect("positioned");
                let equal = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, order, i32_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(equal, head, end).map_err(llvm_err)?;
                i.add_incoming(&[(&self.usize_ty.const_zero(), before), (&next, body_end)]);
                self.builder.position_at_end(end);
                let result = self.builder.build_phi(i32_ty, "order").map_err(llvm_err)?;
                result.add_incoming(&[(&i32_ty.const_zero(), head), (&order, body_end)]);
                Ok(result.as_basic_value().into_int_value())
            }
            Ty::Range(inner, _) => {
                let st = t.into_struct_type();
                let mut orders = Vec::new();
                for i in 0..2u32 {
                    let a = self.builder.build_struct_gep(st, l, i, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(st, r, i, "").map_err(llvm_err)?;
                    orders.push(self.cmp_at(a, b, inner)?);
                }
                self.first_difference(&orders)
            }
            // `None < Some(_)`, and two `Some`s compare their payloads (`spec/stdlib.md`, "Interface coverage").
            Ty::Option(inner) => {
                let ot = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(ot, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(ot, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(self.cx.bool_type(), a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(self.cx.bool_type(), b_tag, "").map_err(llvm_err)?.into_int_value();
                let tag_order = self.cmp_ints(a_tag, b_tag, false)?;
                if self.llvm_ty(inner).is_none() {
                    return Ok(tag_order);
                }
                let a = self.builder.build_struct_gep(ot, l, 1, "").map_err(llvm_err)?;
                let b = self.builder.build_struct_gep(ot, r, 1, "").map_err(llvm_err)?;
                let payload = self.cmp_at(a, b, inner)?;
                let same_tag = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tag_order, i32_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                Ok(self.builder.build_select(same_tag, payload, tag_order, "").map_err(llvm_err)?.into_int_value())
            }
            // `Ok(_) < Err(_)` (the variant order written in `Result`), and two of a kind compare their payloads.
            Ty::Result(ok, err) => {
                let rt = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(rt, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(rt, r, 0, "").map_err(llvm_err)?;
                // The tag is `true` for `Ok`, so flipping it makes `Ok` compare as the first variant.
                let a_tag = self.builder.build_load(self.cx.bool_type(), a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(self.cx.bool_type(), b_tag, "").map_err(llvm_err)?.into_int_value();
                let a_tag = self.builder.build_not(a_tag, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_not(b_tag, "").map_err(llvm_err)?;
                let tag_order = self.cmp_ints(a_tag, b_tag, false)?;
                let is_err = a_tag;
                let is_ok = self.builder.build_not(a_tag, "").map_err(llvm_err)?;
                let mut payload = i32_ty.const_zero();
                if self.llvm_ty(ok).is_some() {
                    let a = self.builder.build_struct_gep(rt, l, 1, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(rt, r, 1, "").map_err(llvm_err)?;
                    let order = self.cmp_at(a, b, ok)?;
                    payload = self.builder.build_select(is_ok, order, payload, "").map_err(llvm_err)?.into_int_value();
                }
                if self.llvm_ty(err).is_some() {
                    let a = self.builder.build_struct_gep(rt, l, 2, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(rt, r, 2, "").map_err(llvm_err)?;
                    let order = self.cmp_at(a, b, err)?;
                    payload = self.builder.build_select(is_err, order, payload, "").map_err(llvm_err)?.into_int_value();
                }
                let same_tag = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, tag_order, i32_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                Ok(self.builder.build_select(same_tag, payload, tag_order, "").map_err(llvm_err)?.into_int_value())
            }
            Ty::Str | Ty::String => {
                let (a_ptr, a_len) = self.parts_at(l, ty)?;
                let (b_ptr, b_len) = self.parts_at(r, ty)?;
                let call = self
                    .builder
                    .build_call(self.rt.str_cmp, &[a_ptr.into(), a_len.into(), b_ptr.into(), b_len.into()], "")
                    .map_err(llvm_err)?;
                Ok(call.try_as_basic_value().basic().expect("`str_cmp` returns an order").into_int_value())
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
    ///
    /// A map hashes its keys with this function, and a key only has to support `==`, so it also covers `f32`/`f64`,
    /// `str`/`String`, and `Result`. The payload of an `Option`, `Result`, or enum is mixed in only when its tag says
    /// it is active: hashing (or reading) an inactive payload would touch a value that was never initialized.
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
            Ty::Float(_) => {
                let v = self.builder.build_load(t, ptr, "").map_err(llvm_err)?.into_float_value();
                let (zero, bits_ty) = match ty {
                    Ty::Float(burdin_tir::FloatTy::F32) => (self.cx.f32_type().const_zero(), self.cx.i32_type()),
                    _ => (self.cx.f64_type().const_zero(), self.cx.i64_type()),
                };
                // `0.0 == -0.0`, so they must hash alike; a NaN never equals anything and its bits are free.
                let is_zero = self.builder.build_float_compare(FloatPredicate::OEQ, v, zero, "").map_err(llvm_err)?;
                let bits = self.builder.build_bit_cast(v, bits_ty, "").map_err(llvm_err)?.into_int_value();
                let bits = if bits.get_type().get_bit_width() < 64 {
                    self.builder.build_int_z_extend(bits, self.cx.i64_type(), "").map_err(llvm_err)?
                } else {
                    bits
                };
                let bits = self
                    .builder
                    .build_select(is_zero, self.cx.i64_type().const_zero(), bits, "")
                    .map_err(llvm_err)?
                    .into_int_value();
                self.hash_mix(acc, bits)
            }
            Ty::Str | Ty::String => {
                let (text, len) = self.parts_at(ptr, ty)?;
                let call = self
                    .builder
                    .build_call(self.rt.str_hash, &[acc.into(), text.into(), len.into()], "hash")
                    .map_err(llvm_err)?;
                Ok(call.try_as_basic_value().basic().expect("`str_hash` returns a hash").into_int_value())
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
            Ty::Array(elem, n) => {
                let Some(elem_t) = self.llvm_ty(elem) else { return Ok(acc) };
                let head = self.new_block("hash.array.head");
                let body = self.new_block("hash.array.body");
                let end = self.new_block("hash.array.end");
                let before = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(head).map_err(llvm_err)?;
                self.builder.position_at_end(head);
                let i = self.builder.build_phi(self.usize_ty, "i").map_err(llvm_err)?;
                let running = self.builder.build_phi(self.cx.i64_type(), "hash").map_err(llvm_err)?;
                let index = i.as_basic_value().into_int_value();
                let n_v = self.usize_ty.const_int(*n, false);
                let more = self.builder.build_int_compare(IntPredicate::ULT, index, n_v, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                // SAFETY (LLVM): index < n.
                let element =
                    unsafe { self.builder.build_in_bounds_gep(elem_t, ptr, &[index], "") }.map_err(llvm_err)?;
                let mixed = self.hash_at(element, elem, running.as_basic_value().into_int_value())?;
                let next =
                    self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let body_end = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(head).map_err(llvm_err)?;
                i.add_incoming(&[(&self.usize_ty.const_zero(), before), (&next, body_end)]);
                running.add_incoming(&[(&acc, before), (&mixed, body_end)]);
                self.builder.position_at_end(end);
                Ok(running.as_basic_value().into_int_value())
            }
            Ty::Range(inner, _) => {
                let st = t.into_struct_type();
                let mut acc = acc;
                for i in 0..2u32 {
                    let f = self.builder.build_struct_gep(st, ptr, i, "").map_err(llvm_err)?;
                    acc = self.hash_at(f, inner, acc)?;
                }
                Ok(acc)
            }
            // A list hashes its length and then its elements in order, so equal lists hash alike whatever their
            // capacity (`spec/derives.md`, `Hash`).
            Ty::List(elem, alloc) => {
                let seq = self.seq_load_at(ptr, alloc)?;
                let len =
                    self.builder.build_int_z_extend_or_bit_cast(seq.len, self.cx.i64_type(), "").map_err(llvm_err)?;
                let with_len = self.hash_mix(acc, len)?;
                if self.llvm_ty(elem).is_none() {
                    return Ok(with_len);
                }
                let index = self.alloca(self.usize_ty.into(), "hash.list.i")?;
                self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
                let running = self.alloca(self.cx.i64_type().into(), "hash.list.acc")?;
                self.builder.build_store(running, with_len).map_err(llvm_err)?;
                let header = self.new_block("hash.list.cond");
                let body = self.new_block("hash.list.body");
                let end = self.new_block("hash.list.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let more = self.builder.build_int_compare(IntPredicate::ULT, i, seq.len, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                let element =
                    self.elem_ptr_at(seq.data, elem, i).ok_or_else(|| "a list element has no runtime type".to_string())?;
                let current =
                    self.builder.build_load(self.cx.i64_type(), running, "").map_err(llvm_err)?.into_int_value();
                let mixed = self.hash_at(element, elem, current)?;
                self.builder.build_store(running, mixed).map_err(llvm_err)?;
                let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next_i).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(self.builder.build_load(self.cx.i64_type(), running, "").map_err(llvm_err)?.into_int_value())
            }
            // A map hashes each entry from a fixed start and combines the results with `xor`, so two maps with the
            // same entries hash alike whatever their iteration order: `Eq` and `Hash` must agree.
            Ty::Map(key, value, alloc) => {
                let seq = self.seq_load_at(ptr, alloc)?;
                let entry = Self::map_entry_ty(key, value);
                let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
                let len =
                    self.builder.build_int_z_extend_or_bit_cast(seq.len, self.cx.i64_type(), "").map_err(llvm_err)?;
                let with_len = self.hash_mix(acc, len)?;
                if self.llvm_ty(key).is_none() && self.llvm_ty(value).is_none() {
                    return Ok(with_len);
                }
                let index = self.alloca(self.usize_ty.into(), "hash.map.i")?;
                self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
                let running = self.alloca(self.cx.i64_type().into(), "hash.map.acc")?;
                self.builder.build_store(running, with_len).map_err(llvm_err)?;
                let header = self.new_block("hash.map.cond");
                let body = self.new_block("hash.map.body");
                let live = self.new_block("hash.map.live");
                let next = self.new_block("hash.map.next");
                let end = self.new_block("hash.map.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let more = self.builder.build_int_compare(IntPredicate::ULT, i, seq.cap, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                let slot = self
                    .elem_ptr_at(seq.data, &entry, i)
                    .ok_or_else(|| "a map entry has no runtime type".to_string())?;
                let hash_field = self.builder.build_struct_gep(entry_t, slot, 2, "map.hash").map_err(llvm_err)?;
                let stored =
                    self.builder.build_load(self.cx.i64_type(), hash_field, "").map_err(llvm_err)?.into_int_value();
                let vacant = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, stored, self.cx.i64_type().const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(vacant, next, live).map_err(llvm_err)?;
                self.builder.position_at_end(live);
                let start = self.cx.i64_type().const_int(0xcbf2_9ce4_8422_2325, false);
                let key_field = self.builder.build_struct_gep(entry_t, slot, 0, "map.key").map_err(llvm_err)?;
                let entry_hash = self.hash_at(key_field, key, start)?;
                let value_field = self.builder.build_struct_gep(entry_t, slot, 1, "map.value").map_err(llvm_err)?;
                let entry_hash = self.hash_at(value_field, value, entry_hash)?;
                let current =
                    self.builder.build_load(self.cx.i64_type(), running, "").map_err(llvm_err)?.into_int_value();
                let combined = self.builder.build_xor(current, entry_hash, "").map_err(llvm_err)?;
                self.builder.build_store(running, combined).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(next).map_err(llvm_err)?;
                self.builder.position_at_end(next);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next_i).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(self.builder.build_load(self.cx.i64_type(), running, "").map_err(llvm_err)?.into_int_value())
            }
            Ty::Option(inner) => {
                let ot = t.into_struct_type();
                let tag_ptr = self.builder.build_struct_gep(ot, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let with_tag = self.hash_mix(acc, tag)?;
                if self.llvm_ty(inner).is_none() {
                    return Ok(with_tag);
                }
                let some_bb = self.new_block("hash.option.some");
                let none_bb = self.new_block("hash.option.none");
                let end = self.new_block("hash.option.end");
                self.builder.build_conditional_branch(tag, some_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(none_bb);
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(some_bb);
                let payload_ptr = self.builder.build_struct_gep(ot, ptr, 1, "").map_err(llvm_err)?;
                let some_hash = self.hash_at(payload_ptr, inner, with_tag)?;
                let some_from = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                let result = self.builder.build_phi(self.cx.i64_type(), "hash").map_err(llvm_err)?;
                result.add_incoming(&[(&with_tag, none_bb), (&some_hash, some_from)]);
                Ok(result.as_basic_value().into_int_value())
            }
            Ty::Result(ok, err) => {
                let rt = t.into_struct_type();
                let tag_ptr = self.builder.build_struct_gep(rt, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(self.cx.bool_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let with_tag = self.hash_mix(acc, tag)?;
                if self.llvm_ty(ok).is_none() && self.llvm_ty(err).is_none() {
                    return Ok(with_tag);
                }
                let ok_bb = self.new_block("hash.result.ok");
                let err_bb = self.new_block("hash.result.err");
                let end = self.new_block("hash.result.end");
                self.builder.build_conditional_branch(tag, ok_bb, err_bb).map_err(llvm_err)?;
                self.builder.position_at_end(ok_bb);
                let ok_hash = if self.llvm_ty(ok).is_some() {
                    let p = self.builder.build_struct_gep(rt, ptr, 1, "").map_err(llvm_err)?;
                    self.hash_at(p, ok, with_tag)?
                } else {
                    with_tag
                };
                let ok_from = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(err_bb);
                let err_hash = if self.llvm_ty(err).is_some() {
                    let p = self.builder.build_struct_gep(rt, ptr, 2, "").map_err(llvm_err)?;
                    self.hash_at(p, err, with_tag)?
                } else {
                    with_tag
                };
                let err_from = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                let result = self.builder.build_phi(self.cx.i64_type(), "hash").map_err(llvm_err)?;
                result.add_incoming(&[(&ok_hash, ok_from), (&err_hash, err_from)]);
                Ok(result.as_basic_value().into_int_value())
            }
            Ty::Enum(a) => {
                let et = t.into_struct_type();
                let i32_ty = self.cx.i32_type();
                let tag_ptr = self.builder.build_struct_gep(et, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(i32_ty, tag_ptr, "").map_err(llvm_err)?.into_int_value();
                let with_tag = self.hash_mix(acc, tag)?;
                let variants: Vec<Vec<Ty>> =
                    (0..self.program.enums[a.id].variants.len()).map(|vi| self.variant_tys(a.id, vi)).collect();
                if variants.iter().all(Vec::is_empty) {
                    return Ok(with_tag);
                }
                let payload = self.builder.build_struct_gep(et, ptr, 1, "").map_err(llvm_err)?;
                let end = self.new_block("hash.enum.end");
                let other = self.new_block("hash.enum.other");
                let mut cases = Vec::with_capacity(variants.len());
                let mut blocks = Vec::with_capacity(variants.len());
                for vi in 0..variants.len() {
                    let bb = self.new_block("hash.enum.case");
                    cases.push((i32_ty.const_int(vi as u64, false), bb));
                    blocks.push(bb);
                }
                self.builder.build_switch(tag, other, &cases).map_err(llvm_err)?;
                self.builder.position_at_end(other);
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                let mut incoming = vec![(with_tag, other)];
                for (vi, bb) in blocks.into_iter().enumerate() {
                    self.builder.position_at_end(bb);
                    let vt = self.variant_ty(a.id, vi);
                    let mut h = with_tag;
                    for (i, fty) in variants[vi].iter().enumerate() {
                        let f = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                        h = self.hash_at(f, fty, h)?;
                    }
                    let from = self.builder.get_insert_block().expect("positioned");
                    self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                    incoming.push((h, from));
                }
                self.builder.position_at_end(end);
                let result = self.builder.build_phi(self.cx.i64_type(), "hash").map_err(llvm_err)?;
                for (v, bb) in &incoming {
                    result.add_incoming(&[(v, *bb)]);
                }
                Ok(result.as_basic_value().into_int_value())
            }
            _ => Err(format!("unsupported hash of `{ty}`")),
        }
    }
}
