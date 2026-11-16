//! Operators, arithmetic with overflow checks, and casts.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_diag::Span;
use burdin_tir::{self as tir, BinOp, IntTy, Ty};
use inkwell::intrinsics::Intrinsic;
use inkwell::values::{BasicValueEnum, IntValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn binary(
        &mut self,
        op: BinOp,
        l: &'a tir::Expr,
        r: &'a tir::Expr,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        if matches!(op, BinOp::And | BinOp::Or) {
            return self.short_circuit(op, l, r).map(Some);
        }
        let operand_ty = if l.ty == Ty::Never { &r.ty } else { &l.ty };
        let lv = self.expr_mode(l, Mode::Use)?;
        let rv = self.expr_mode(r, Mode::Use)?;
        if let Ty::CPtr(_) = operand_ty {
            return self.pointer_binary(op, lv, rv, l, r);
        }
        if matches!(op, BinOp::Eq | BinOp::Ne)
            && matches!(
                operand_ty,
                Ty::Unit
                    | Ty::Tuple(_)
                    | Ty::Array(..)
                    | Ty::Struct(_)
                    | Ty::Enum(_)
                    | Ty::Option(_)
                    | Ty::Result(..)
                    | Ty::Range(..)
            )
        {
            let lp = self.spill(lv, operand_ty)?;
            let rp = self.spill(rv, operand_ty)?;
            let eq = match (lp, rp) {
                (Some(lp), Some(rp)) => self.eq_at(lp, rp, operand_ty)?,
                _ => self.cx.bool_type().const_int(1, false),
            };
            let v = if op == BinOp::Ne { self.builder.build_not(eq, "").map_err(llvm_err)? } else { eq };
            return Ok(Some(v.into()));
        }
        if matches!(op, BinOp::Eq | BinOp::Ne) && matches!(operand_ty, Ty::Str | Ty::String) {
            let lp = self.spill(lv, operand_ty)?;
            let rp = self.spill(rv, operand_ty)?;
            let eq = match (lp, rp) {
                (Some(lp), Some(rp)) => self.eq_at(lp, rp, operand_ty)?,
                _ => self.cx.bool_type().const_int(1, false),
            };
            let v = if op == BinOp::Ne { self.builder.build_not(eq, "").map_err(llvm_err)? } else { eq };
            return Ok(Some(v.into()));
        }
        if matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge)
            && matches!(
                operand_ty,
                Ty::Unit
                    | Ty::Tuple(_)
                    | Ty::Array(..)
                    | Ty::Struct(_)
                    | Ty::Enum(_)
                    | Ty::Str
                    | Ty::String
                    | Ty::Range(..)
            )
        {
            let lp = self.spill(lv, operand_ty)?;
            let rp = self.spill(rv, operand_ty)?;
            let order = match (lp, rp) {
                (Some(lp), Some(rp)) => self.cmp_at(lp, rp, operand_ty)?,
                _ => self.cx.i32_type().const_zero(),
            };
            let pred = match op {
                BinOp::Lt => IntPredicate::SLT,
                BinOp::Gt => IntPredicate::SGT,
                BinOp::Le => IntPredicate::SLE,
                _ => IntPredicate::SGE,
            };
            let zero = self.cx.i32_type().const_zero();
            let v = self.builder.build_int_compare(pred, order, zero, "").map_err(llvm_err)?;
            return Ok(Some(v.into()));
        }
        if self.llvm_ty(operand_ty).is_none() {
            return Ok(None);
        }
        let lv = self.need(lv, operand_ty);
        let rv = self.need(rv, &r.ty);
        let b = &self.builder;
        let v: BasicValueEnum<'ctx> = match operand_ty {
            Ty::Float(_) => {
                let (x, y) = (lv.into_float_value(), rv.into_float_value());
                let pred = match op {
                    BinOp::Add => return Ok(Some(b.build_float_add(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Sub => return Ok(Some(b.build_float_sub(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Mul => return Ok(Some(b.build_float_mul(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Div => return Ok(Some(b.build_float_div(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Rem => return Ok(Some(b.build_float_rem(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Eq => FloatPredicate::OEQ,
                    BinOp::Ne => FloatPredicate::UNE,
                    BinOp::Lt => FloatPredicate::OLT,
                    BinOp::Gt => FloatPredicate::OGT,
                    BinOp::Le => FloatPredicate::OLE,
                    BinOp::Ge => FloatPredicate::OGE,
                    _ => unreachable!("checked: no bit operators on floats"),
                };
                b.build_float_compare(pred, x, y, "").map_err(llvm_err)?.into()
            }
            _ => {
                let (x, y) = (lv.into_int_value(), rv.into_int_value());
                let int = match operand_ty {
                    Ty::Int(i) => Some(*i),
                    _ => None,
                };
                let signed = int.is_some_and(|i| i.signed());
                let cmp = |pred_s, pred_u| if signed { pred_s } else { pred_u };
                let pred = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let i = int.expect("checked: arithmetic on integers");
                        return Ok(Some(self.arith(op, x, y, i, span)?.into()));
                    }
                    BinOp::Div | BinOp::Rem => {
                        let i = int.expect("checked: arithmetic on integers");
                        return Ok(Some(self.div_rem(op, x, y, i, span)?.into()));
                    }
                    BinOp::Shl | BinOp::Shr => {
                        let i = int.expect("checked: shifts on integers");
                        return Ok(Some(self.shift(op, x, y, i, span)?.into()));
                    }
                    BinOp::BitAnd => return Ok(Some(b.build_and(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::BitOr => return Ok(Some(b.build_or(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::BitXor => return Ok(Some(b.build_xor(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Eq => IntPredicate::EQ,
                    BinOp::Ne => IntPredicate::NE,
                    BinOp::Lt => cmp(IntPredicate::SLT, IntPredicate::ULT),
                    BinOp::Gt => cmp(IntPredicate::SGT, IntPredicate::UGT),
                    BinOp::Le => cmp(IntPredicate::SLE, IntPredicate::ULE),
                    BinOp::Ge => cmp(IntPredicate::SGE, IntPredicate::UGE),
                    BinOp::And | BinOp::Or => unreachable!("handled above"),
                };
                b.build_int_compare(pred, x, y, "").map_err(llvm_err)?.into()
            }
        };
        Ok(Some(v))
    }

    /// Pointer equality, pointer difference, and element offsets (`p + n`, `p - n`). None of these are checked:
    /// the type checker keeps them inside `unsafe`.
    fn pointer_binary(
        &mut self,
        op: BinOp,
        lv: Option<BasicValueEnum<'ctx>>,
        rv: Option<BasicValueEnum<'ctx>>,
        l: &'a tir::Expr,
        r: &'a tir::Expr,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Ty::CPtr(inner) = &l.ty else { return Ok(None) };
        let lhs = self.need(lv, &l.ty).into_pointer_value();
        match op {
            BinOp::Eq | BinOp::Ne => {
                let rhs = self.need(rv, &r.ty).into_pointer_value();
                let a = self.builder.build_ptr_to_int(lhs, self.usize_ty, "").map_err(llvm_err)?;
                let b = self.builder.build_ptr_to_int(rhs, self.usize_ty, "").map_err(llvm_err)?;
                let pred = if op == BinOp::Eq { IntPredicate::EQ } else { IntPredicate::NE };
                let v = self.builder.build_int_compare(pred, a, b, "").map_err(llvm_err)?;
                Ok(Some(v.into()))
            }
            BinOp::Add | BinOp::Sub if matches!(r.ty, Ty::Int(_)) => {
                let index = match op {
                    BinOp::Add => self.need_int(rv, &r.ty),
                    _ => {
                        let v = self.need_int(rv, &r.ty);
                        self.builder.build_int_neg(v, "").map_err(llvm_err)?
                    }
                };
                let index = if let Ty::Int(i) = r.ty {
                    if i.bits() < 64 {
                        if i.signed() {
                            self.builder.build_int_s_extend(index, self.usize_ty, "").map_err(llvm_err)?
                        } else {
                            self.builder.build_int_z_extend(index, self.usize_ty, "").map_err(llvm_err)?
                        }
                    } else {
                        index
                    }
                } else {
                    index
                };
                let elem = self.llvm_ty(inner).unwrap_or_else(|| self.cx.i8_type().into());
                // SAFETY (LLVM): the result may point outside the allocation; computing the address is allowed.
                let p = unsafe { self.builder.build_gep(elem, lhs, &[index], "") }.map_err(llvm_err)?;
                Ok(Some(p.into()))
            }
            BinOp::Sub if matches!(r.ty, Ty::CPtr(_)) => {
                let rhs = self.need(rv, &r.ty).into_pointer_value();
                let int_ty = self.int_ty(IntTy::Isize);
                let a = self.builder.build_ptr_to_int(lhs, int_ty, "").map_err(llvm_err)?;
                let b = self.builder.build_ptr_to_int(rhs, int_ty, "").map_err(llvm_err)?;
                let bytes = self.builder.build_int_sub(a, b, "").map_err(llvm_err)?;
                // `p - q` counts elements, like C and `offset_from`; a zero-sized pointee counts as one byte.
                let size = int_ty.const_int(self.layout_size(inner, false).max(1), false);
                let v = self.builder.build_int_signed_div(bytes, size, "").map_err(llvm_err)?;
                Ok(Some(v.into()))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn short_circuit(
        &mut self,
        op: BinOp,
        l: &'a tir::Expr,
        r: &'a tir::Expr,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let lv = self.expr_mode(l, Mode::Use)?;
        let lv = self.need_int(lv, &Ty::Bool);
        let left_end = self.builder.get_insert_block().expect("positioned");
        let rhs_bb = self.new_block("logic.rhs");
        let merge = self.new_block("logic.end");
        match op {
            BinOp::And => self.builder.build_conditional_branch(lv, rhs_bb, merge),
            _ => self.builder.build_conditional_branch(lv, merge, rhs_bb),
        }
        .map_err(llvm_err)?;
        self.builder.position_at_end(rhs_bb);
        let rv = self.expr_mode(r, Mode::Use)?;
        let rv = self.need_int(rv, &Ty::Bool);
        let right_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;
        self.builder.position_at_end(merge);
        let phi = self.builder.build_phi(self.cx.bool_type(), "").map_err(llvm_err)?;
        let short = self.cx.bool_type().const_int(u64::from(op == BinOp::Or), false);
        phi.add_incoming(&[(&short, left_end), (&rv, right_end)]);
        Ok(phi.as_basic_value())
    }

    /// `+`, `-`, `*`: panics on overflow in debug builds, wraps in release builds.
    pub(super) fn arith(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        if self.release {
            return self.wrapping(op, x, y);
        }
        let (v, overflow) = self.with_overflow(op, x, y, i)?;
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            _ => "multiply",
        };
        let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
        self.check(ok, &format!("attempt to {verb} with overflow"), span)?;
        Ok(v)
    }

    pub(super) fn wrapping(&mut self, op: BinOp, x: IntValue<'ctx>, y: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let b = &self.builder;
        match op {
            BinOp::Add => b.build_int_add(x, y, ""),
            BinOp::Sub => b.build_int_sub(x, y, ""),
            _ => b.build_int_mul(x, y, ""),
        }
        .map_err(llvm_err)
    }

    /// The wrapped result and whether the operation overflowed.
    pub(super) fn with_overflow(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
    ) -> GenResult<(IntValue<'ctx>, IntValue<'ctx>)> {
        let prefix = if i.signed() { "s" } else { "u" };
        let name = match op {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            _ => "mul",
        };
        let intrinsic =
            Intrinsic::find(&format!("llvm.{prefix}{name}.with.overflow")).ok_or("missing LLVM overflow intrinsic")?;
        let decl = intrinsic
            .get_declaration(self.module, &[x.get_type().into()])
            .ok_or("cannot declare LLVM overflow intrinsic")?;
        let call = self.builder.build_call(decl, &[x.into(), y.into()], "").map_err(llvm_err)?;
        let pair = call.try_as_basic_value().basic().ok_or("overflow intrinsic returns a value")?.into_struct_value();
        let v = self.builder.build_extract_value(pair, 0, "").map_err(llvm_err)?.into_int_value();
        let overflow = self.builder.build_extract_value(pair, 1, "").map_err(llvm_err)?.into_int_value();
        Ok((v, overflow))
    }

    /// `/` and `%`: truncate toward zero; division by zero always panics; `MIN / -1` overflows.
    pub(super) fn div_rem(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        let t = x.get_type();
        let nonzero = self.builder.build_int_compare(IntPredicate::NE, y, t.const_zero(), "").map_err(llvm_err)?;
        self.check(nonzero, "division by zero", span)?;
        let mut divisor = y;
        if i.signed() {
            let min = t.const_int(i.min() as u64, false);
            let minus_one = t.const_all_ones();
            let is_min = self.builder.build_int_compare(IntPredicate::EQ, x, min, "").map_err(llvm_err)?;
            let is_m1 = self.builder.build_int_compare(IntPredicate::EQ, y, minus_one, "").map_err(llvm_err)?;
            let overflow = self.builder.build_and(is_min, is_m1, "").map_err(llvm_err)?;
            if self.release {
                // MIN / 1 = MIN and MIN % 1 = 0 are the wrapped results of MIN / -1 and MIN % -1.
                divisor = self
                    .builder
                    .build_select(overflow, t.const_int(1, false), y, "")
                    .map_err(llvm_err)?
                    .into_int_value();
            } else {
                let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                let what = if op == BinOp::Div { "divide" } else { "calculate the remainder" };
                self.check(ok, &format!("attempt to {what} with overflow"), span)?;
            }
        }
        let b = &self.builder;
        match (op, i.signed()) {
            (BinOp::Div, true) => b.build_int_signed_div(x, divisor, ""),
            (BinOp::Div, false) => b.build_int_unsigned_div(x, divisor, ""),
            (_, true) => b.build_int_signed_rem(x, divisor, ""),
            (_, false) => b.build_int_unsigned_rem(x, divisor, ""),
        }
        .map_err(llvm_err)
    }

    /// `<<` and `>>`: a shift by the bit width or more always panics. `>>` is arithmetic for signed types.
    pub(super) fn shift(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        let limit = y.get_type().const_int(u64::from(i.bits()), false);
        // Unsigned comparison: a negative amount is huge and fails too.
        let ok = self.builder.build_int_compare(IntPredicate::ULT, y, limit, "").map_err(llvm_err)?;
        self.check(ok, "attempt to shift with overflow", span)?;
        let amount = self.builder.build_int_cast_sign_flag(y, x.get_type(), false, "").map_err(llvm_err)?;
        match op {
            BinOp::Shl => self.builder.build_left_shift(x, amount, ""),
            _ => self.builder.build_right_shift(x, amount, i.signed(), ""),
        }
        .map_err(llvm_err)
    }

    pub(super) fn negate(&mut self, v: IntValue<'ctx>, i: IntTy, span: Span) -> GenResult<IntValue<'ctx>> {
        let zero = v.get_type().const_zero();
        if self.release {
            return self.builder.build_int_sub(zero, v, "").map_err(llvm_err);
        }
        let (r, overflow) = self.with_overflow(BinOp::Sub, zero, v, i)?;
        let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
        self.check(ok, "attempt to negate with overflow", span)?;
        Ok(r)
    }

    pub(super) fn cast(&mut self, v: BasicValueEnum<'ctx>, from: &Ty, to: &Ty) -> GenResult<BasicValueEnum<'ctx>> {
        if from == to {
            return Ok(v);
        }
        let target = self.llvm_ty(to).expect("cast targets are scalars");
        let b = &self.builder;
        let signed_from = matches!(from, Ty::Int(i) if i.signed());
        let r: BasicValueEnum<'ctx> = match (from, to) {
            (Ty::Int(_) | Ty::Bool | Ty::Char, Ty::Int(_) | Ty::Char) => b
                .build_int_cast_sign_flag(v.into_int_value(), target.into_int_type(), signed_from, "")
                .map_err(llvm_err)?
                .into(),
            (Ty::Int(_), Ty::Float(_)) => {
                let (x, t) = (v.into_int_value(), target.into_float_type());
                if signed_from {
                    b.build_signed_int_to_float(x, t, "").map_err(llvm_err)?.into()
                } else {
                    b.build_unsigned_int_to_float(x, t, "").map_err(llvm_err)?.into()
                }
            }
            (Ty::Float(_), Ty::Int(i)) => {
                // Saturating, and NaN becomes 0, so the conversion is never undefined.
                let name = if i.signed() { "llvm.fptosi.sat" } else { "llvm.fptoui.sat" };
                let intrinsic = Intrinsic::find(name).ok_or("missing LLVM fpto*i.sat intrinsic")?;
                let decl = intrinsic
                    .get_declaration(self.module, &[target, v.get_type()])
                    .ok_or("cannot declare LLVM fpto*i.sat intrinsic")?;
                let call = self.builder.build_call(decl, &[v.into()], "").map_err(llvm_err)?;
                call.try_as_basic_value().basic().ok_or("fpto*i.sat returns a value")?
            }
            (Ty::Float(_), Ty::Float(_)) => {
                b.build_float_cast(v.into_float_value(), target.into_float_type(), "").map_err(llvm_err)?.into()
            }
            (Ty::CPtr(_), Ty::CPtr(_)) | (Ty::Ref { .. }, Ty::CPtr(_)) => v,
            (Ty::CPtr(_), Ty::Int(i)) => {
                let int_ty = self.int_ty(*i);
                b.build_ptr_to_int(v.into_pointer_value(), int_ty, "").map_err(llvm_err)?.into()
            }
            (Ty::Int(_), Ty::CPtr(_)) => {
                let ptr_ty = self.cx.ptr_type(AddressSpace::default());
                b.build_int_to_ptr(v.into_int_value(), ptr_ty, "").map_err(llvm_err)?.into()
            }
            _ => return Err(format!("unsupported cast from `{from}` to `{to}`")),
        };
        Ok(r)
    }
}
