//! Strings, string literals, and slices.

use super::place::is_place;
use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_diag::Span;
use burdin_tir::{self as tir, Method, Ty};
use inkwell::AddressSpace;
use inkwell::IntPredicate;
use inkwell::module::Linkage;
use inkwell::values::{BasicValueEnum, IntValue, PointerValue, StructValue};

/// The three words of a `String` at a place: its fields and their loaded values.
struct Text<'ctx> {
    ptr_field: PointerValue<'ctx>,
    len_field: PointerValue<'ctx>,
    cap_field: PointerValue<'ctx>,
    data: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    cap: IntValue<'ctx>,
}

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

    /// `base[lo..hi]` or `base[lo..=hi]`: a view of bytes of a `str` or `String`, or of elements of a slice or
    /// array. An inclusive end is checked as `hi < len` and then advanced, so `hi + 1` cannot overflow.
    pub(super) fn slice(
        &mut self,
        base: &'a tir::Expr,
        lo: &'a tir::Expr,
        hi: &'a tir::Expr,
        inclusive: bool,
        span: Span,
    ) -> GenResult<StructValue<'ctx>> {
        if let Ty::Array(_, n) = &base.ty {
            let n = *n;
            let base_ptr = self.place_ptr(base)?;
            let base_ptr = base_ptr.unwrap_or_else(|| self.cx.ptr_type(AddressSpace::default()).get_undef());
            let lo_v = self.usize_value(lo)?;
            let hi_v = self.usize_value(hi)?;
            let hi_v = self.slice_end(lo_v, hi_v, self.usize_ty.const_int(n, false), inclusive, span)?;
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
        let hi_v = self.slice_end(lo_v, hi_v, len.into_int_value(), inclusive, span)?;
        self.bounds_guard_ule(lo_v, hi_v, span)?;
        // The element type of a slice or a `List` decides the stride; `str` and `String` are bytes. Slicing a
        // slice of `i32` by three elements must move three elements, not three bytes.
        let elem_ty = match &base.ty {
            Ty::Slice(elem) | Ty::List(elem, _) => self.llvm_ty(elem),
            _ => None,
        };
        let elem_ty = elem_ty.unwrap_or_else(|| self.cx.i8_type().into());
        // SAFETY (LLVM): lo <= hi <= the base length.
        let p = unsafe { self.builder.build_in_bounds_gep(elem_ty, ptr.into_pointer_value(), &[lo_v], "") }
            .map_err(llvm_err)?;
        let len = self.builder.build_int_sub(hi_v, lo_v, "").map_err(llvm_err)?;
        self.make_str(p.into(), len.into())
    }

    // ----- capacity and editing (`spec/stdlib.md`, "String capacity and editing") -----

    /// The three words of the `String` at `e`'s place. A temporary receiver is spilled first.
    fn text(&mut self, e: &'a tir::Expr) -> GenResult<Option<Text<'ctx>>> {
        let Some(addr) = self.place_ptr(e)? else { return Ok(None) };
        let string_t = self.string_ty;
        let ptr_field = self.builder.build_struct_gep(string_t, addr, 0, "s.ptr").map_err(llvm_err)?;
        let len_field = self.builder.build_struct_gep(string_t, addr, 1, "s.len").map_err(llvm_err)?;
        let cap_field = self.builder.build_struct_gep(string_t, addr, 2, "s.cap").map_err(llvm_err)?;
        let data = self.builder.build_load(self.ptr_ty(), ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        Ok(Some(Text { ptr_field, len_field, cap_field, data, len, cap }))
    }

    fn text_store(&mut self, t: &Text<'ctx>) -> GenResult<()> {
        self.builder.build_store(t.ptr_field, t.data).map_err(llvm_err)?;
        self.builder.build_store(t.len_field, t.len).map_err(llvm_err)?;
        self.builder.build_store(t.cap_field, t.cap).map_err(llvm_err)?;
        Ok(())
    }

    /// Grows the buffer so it holds `needed` bytes: the larger of the current capacity plus half (and eight) or
    /// exactly `needed`. A buffer that already fits is left alone.
    fn text_grow(&mut self, t: &mut Text<'ctx>, needed: IntValue<'ctx>, span: Span) -> GenResult<()> {
        let enough = self.builder.build_int_compare(IntPredicate::ULE, needed, t.cap, "").map_err(llvm_err)?;
        let grow = self.new_block("text.grow");
        let done = self.new_block("text.grow.end");
        self.builder.build_conditional_branch(enough, done, grow).map_err(llvm_err)?;
        self.builder.position_at_end(grow);
        let half =
            self.builder.build_right_shift(t.cap, self.usize_ty.const_int(1, false), false, "").map_err(llvm_err)?;
        let extra = self.builder.build_int_add(half, self.usize_ty.const_int(8, false), "").map_err(llvm_err)?;
        let grown = self.builder.build_int_add(t.cap, extra, "").map_err(llvm_err)?;
        let too_small = self.builder.build_int_compare(IntPredicate::ULT, grown, needed, "").map_err(llvm_err)?;
        let new_cap = self.builder.build_select(too_small, needed, grown, "").map_err(llvm_err)?.into_int_value();
        let data = self.global_realloc(t.data, new_cap, Some(span))?;
        self.builder.build_store(t.ptr_field, data).map_err(llvm_err)?;
        self.builder.build_store(t.cap_field, new_cap).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        // The two paths leave different words; read them back after the join.
        t.data = self.builder.build_load(self.ptr_ty(), t.ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        t.cap = self.builder.build_load(self.usize_ty, t.cap_field, "").map_err(llvm_err)?.into_int_value();
        Ok(())
    }

    /// Panics with `message` unless byte offset `at` is a character boundary of the text at `data` (0, the length,
    /// or a byte that does not continue a character).
    fn check_text_boundary(
        &mut self,
        data: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        at: IntValue<'ctx>,
        message: &str,
        span: Span,
    ) -> GenResult<()> {
        let at_zero =
            self.builder.build_int_compare(IntPredicate::EQ, at, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let at_len = self.builder.build_int_compare(IntPredicate::EQ, at, len, "").map_err(llvm_err)?;
        let edge = self.builder.build_or(at_zero, at_len, "").map_err(llvm_err)?;
        let inside = self.new_block("text.boundary");
        let done = self.new_block("text.boundary.end");
        self.builder.build_conditional_branch(edge, done, inside).map_err(llvm_err)?;
        self.builder.position_at_end(inside);
        // SAFETY (LLVM): `at` is inside the buffer (0 < at < len) when this block runs.
        let p = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), data, &[at], "boundary.byte") }
            .map_err(llvm_err)?;
        let byte = self.builder.build_load(self.cx.i8_type(), p, "").map_err(llvm_err)?.into_int_value();
        let masked = self.builder.build_and(byte, self.cx.i8_type().const_int(0xC0, false), "").map_err(llvm_err)?;
        let continuation = self
            .builder
            .build_int_compare(IntPredicate::EQ, masked, self.cx.i8_type().const_int(0x80, false), "")
            .map_err(llvm_err)?;
        let boundary = self.builder.build_not(continuation, "").map_err(llvm_err)?;
        self.check(boundary, message, span)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        Ok(())
    }

    /// The buffer methods of `String`. `None` when `m` is not one of them.
    pub(super) fn string_method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        _ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match m {
            Method::StringCapacity => {
                let Some(t) = self.text(&args[0])? else { return Ok(None) };
                Ok(Some(t.cap.into()))
            }
            Method::StringReserve => {
                let additional = self.expr_mode(&args[1], Mode::Use)?;
                let additional = self.need_int(additional, &Ty::Int(tir::IntTy::Usize));
                let Some(mut t) = self.text(&args[0])? else { return Ok(None) };
                let needed = self.builder.build_int_add(t.len, additional, "").map_err(llvm_err)?;
                let overflow =
                    self.builder.build_int_compare(IntPredicate::ULT, needed, t.len, "").map_err(llvm_err)?;
                let fits = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                self.check(fits, "String reserve: capacity overflow", span)?;
                self.text_grow(&mut t, needed, span)?;
                Ok(None)
            }
            Method::StringShrinkToFit => {
                let Some(t) = self.text(&args[0])? else { return Ok(None) };
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, t.len, self.usize_ty.const_zero(), "")
                    .map_err(llvm_err)?;
                let clear = self.new_block("text.shrink.clear");
                let resize = self.new_block("text.shrink.resize");
                let done = self.new_block("text.shrink.end");
                self.builder.build_conditional_branch(empty, clear, resize).map_err(llvm_err)?;
                self.builder.position_at_end(clear);
                self.builder.build_call(self.rt.string_free, &[t.data.into(), t.cap.into()], "").map_err(llvm_err)?;
                self.builder.build_store(t.ptr_field, self.ptr_ty().const_null()).map_err(llvm_err)?;
                self.builder.build_store(t.cap_field, self.usize_ty.const_zero()).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(resize);
                let smaller = self.builder.build_int_compare(IntPredicate::ULT, t.len, t.cap, "").map_err(llvm_err)?;
                let shrink = self.new_block("text.shrink.smaller");
                let keep = self.new_block("text.shrink.keep");
                self.builder.build_conditional_branch(smaller, shrink, keep).map_err(llvm_err)?;
                self.builder.position_at_end(shrink);
                // The buffer keeps its bytes; only the allocation shrinks.
                let data = self.global_realloc(t.data, t.len, Some(span))?;
                self.builder.build_store(t.ptr_field, data).map_err(llvm_err)?;
                self.builder.build_store(t.cap_field, t.len).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(keep);
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(done);
                Ok(None)
            }
            Method::StringInsertStr => {
                let at = self.expr_mode(&args[1], Mode::Use)?;
                let at = self.need_int(at, &Ty::Int(tir::IntTy::Usize));
                let text = self.expr_mode(&args[2], Mode::Use)?;
                let text = self.need(text, &Ty::Str).into_struct_value();
                let source = self.builder.build_extract_value(text, 0, "").map_err(llvm_err)?.into_pointer_value();
                let count = self.builder.build_extract_value(text, 1, "").map_err(llvm_err)?.into_int_value();
                let Some(mut t) = self.text(&args[0])? else { return Ok(None) };
                let in_bounds = self.builder.build_int_compare(IntPredicate::ULE, at, t.len, "").map_err(llvm_err)?;
                self.check(in_bounds, "insert_str: the index is out of bounds", span)?;
                self.check_text_boundary(t.data, t.len, at, "insert_str: the index is not a character boundary", span)?;
                let needed = self.builder.build_int_add(t.len, count, "").map_err(llvm_err)?;
                let overflow =
                    self.builder.build_int_compare(IntPredicate::ULT, needed, t.len, "").map_err(llvm_err)?;
                let fits = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                self.check(fits, "String capacity overflow", span)?;
                self.text_grow(&mut t, needed, span)?;
                let after = self.builder.build_int_add(at, count, "").map_err(llvm_err)?;
                let tail = self.builder.build_int_sub(t.len, at, "").map_err(llvm_err)?;
                // SAFETY (LLVM): `at <= len`, and the buffer was grown to hold the inserted text.
                let src = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[at], "") }
                    .map_err(llvm_err)?;
                let dst = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[after], "") }
                    .map_err(llvm_err)?;
                self.builder.build_memmove(dst, 1, src, 1, tail).map_err(llvm_err)?;
                let target = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[at], "") }
                    .map_err(llvm_err)?;
                self.builder.build_memcpy(target, 1, source, 1, count).map_err(llvm_err)?;
                t.len = needed;
                self.text_store(&t)?;
                Ok(None)
            }
            Method::StringReplaceRange => {
                let from = self.expr_mode(&args[1], Mode::Use)?;
                let from = self.need_int(from, &Ty::Int(tir::IntTy::Usize));
                let to = self.expr_mode(&args[2], Mode::Use)?;
                let to = self.need_int(to, &Ty::Int(tir::IntTy::Usize));
                let text = self.expr_mode(&args[3], Mode::Use)?;
                let text = self.need(text, &Ty::Str).into_struct_value();
                let source = self.builder.build_extract_value(text, 0, "").map_err(llvm_err)?.into_pointer_value();
                let count = self.builder.build_extract_value(text, 1, "").map_err(llvm_err)?.into_int_value();
                let Some(mut t) = self.text(&args[0])? else { return Ok(None) };
                let ordered = self.builder.build_int_compare(IntPredicate::ULE, from, to, "").map_err(llvm_err)?;
                let in_bounds = self.builder.build_int_compare(IntPredicate::ULE, to, t.len, "").map_err(llvm_err)?;
                let valid = self.builder.build_and(ordered, in_bounds, "").map_err(llvm_err)?;
                self.check(valid, "replace_range: the range is out of bounds", span)?;
                self.check_text_boundary(
                    t.data,
                    t.len,
                    from,
                    "replace_range: the start is not a character boundary",
                    span,
                )?;
                self.check_text_boundary(
                    t.data,
                    t.len,
                    to,
                    "replace_range: the end is not a character boundary",
                    span,
                )?;
                let old = self.builder.build_int_sub(to, from, "").map_err(llvm_err)?;
                // The new length is the text before the range, the replacement, and the text after it.
                let base = self.builder.build_int_sub(t.len, old, "").map_err(llvm_err)?;
                let needed = self.builder.build_int_add(base, count, "").map_err(llvm_err)?;
                let overflow = self.builder.build_int_compare(IntPredicate::ULT, needed, base, "").map_err(llvm_err)?;
                let fits = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                self.check(fits, "String capacity overflow", span)?;
                self.text_grow(&mut t, needed, span)?;
                let after = self.builder.build_int_add(from, count, "").map_err(llvm_err)?;
                let tail = self.builder.build_int_sub(t.len, to, "").map_err(llvm_err)?;
                // SAFETY (LLVM): `to <= len`, and the buffer was grown to hold the replacement text.
                let src = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[to], "") }
                    .map_err(llvm_err)?;
                let dst = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[after], "") }
                    .map_err(llvm_err)?;
                self.builder.build_memmove(dst, 1, src, 1, tail).map_err(llvm_err)?;
                let target = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), t.data, &[from], "") }
                    .map_err(llvm_err)?;
                self.builder.build_memcpy(target, 1, source, 1, count).map_err(llvm_err)?;
                t.len = needed;
                self.text_store(&t)?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// The exclusive end of a slice from its written end: `hi` itself, or `hi + 1` when the range is inclusive.
    /// An inclusive end must be strictly less than `len` and at least `lo`; checking both keeps `hi + 1` from
    /// wrapping and rejects a reversed inclusive range such as `2..=1`, which would otherwise look empty.
    fn slice_end(
        &mut self,
        lo: IntValue<'ctx>,
        hi: IntValue<'ctx>,
        len: IntValue<'ctx>,
        inclusive: bool,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        if !inclusive {
            self.bounds_guard_ule(hi, len, span)?;
            return Ok(hi);
        }
        self.bounds_check(hi, len, span)?;
        self.bounds_guard_ule(lo, hi, span)?;
        self.builder.build_int_add(hi, self.usize_ty.const_int(1, false), "").map_err(llvm_err)
    }
}
