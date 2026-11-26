//! The iteration protocol's built-in methods: `iter`, `into_iter`, and `next` on the compiler-known iterators.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::Ty;
use inkwell::IntPredicate;
use inkwell::values::{BasicValueEnum, IntValue, PointerValue, StructValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// `xs.iter()`, `xs.iter_mut()`, and `xs.into_iter()`: builds the iterator value of the receiver's type.
    pub(super) fn iter_value(
        &mut self,
        args: &'a [burdin_tir::Expr],
        into: bool,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let recv = &args[0];
        let recv_ty = recv.ty.clone();
        let seq_alloc = match self.map_info(&recv_ty) {
            Some((_, _, alloc)) => Some(alloc),
            None => self.list_info(&recv_ty).map(|(_, alloc)| alloc),
        };
        match (&recv_ty, into) {
            // A slice view is its own iterator; `Chars` is a `str` view.
            (Ty::Slice(_) | Ty::Chars, _) => self.expr_mode(recv, Mode::Use),
            (Ty::Array(_, n), false) => {
                let n = *n;
                match self.place_ptr(recv)? {
                    Some(ptr) => Ok(Some(self.make_str(ptr.into(), self.usize_ty.const_int(n, false).into())?.into())),
                    None => Ok(None),
                }
            }
            (Ty::Array(elem, n), true) => {
                let elem = (**elem).clone();
                let n = *n;
                let v = self.expr_mode(recv, Mode::Move)?;
                let array = self.need(v, &recv_ty);
                let tt = self.iter_array_ty(&elem, n);
                let mut agg = undef(tt.into()).into_struct_value();
                agg = self.builder.build_insert_value(agg, array, 0, "").map_err(llvm_err)?.into_struct_value();
                agg = self
                    .builder
                    .build_insert_value(agg, self.usize_ty.const_zero(), 1, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                Ok(Some(agg.into()))
            }
            (_, _) if seq_alloc.is_some() => {
                // `into_iter` takes the collection, so the collection (and a stored allocator) is moved and its
                // drop flag cleared; `iter` only borrows it.
                let alloc = seq_alloc.clone().expect("checked by the guard");
                let v = self.expr_mode(recv, if into { Mode::Move } else { Mode::Use })?;
                let st = self.llvm_ty(&recv_ty).expect("a container has a type").into_struct_type();
                let s = self.need(v, &recv_ty).into_struct_value();
                let base = self.builder.build_extract_value(s, 0, "iter.ptr").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "iter.len").map_err(llvm_err)?;
                if into {
                    let cap = self.builder.build_extract_value(s, 2, "iter.cap").map_err(llvm_err)?;
                    let zero = self.usize_ty.const_zero().into();
                    let mut fields = vec![(0, base), (1, len), (2, cap), (3, zero)];
                    if alloc.allocator_ty().is_some() {
                        let a = self.builder.build_extract_value(s, 3, "iter.alloc").map_err(llvm_err)?;
                        fields.push((4, a));
                    }
                    Ok(Some(self.seq_value(&alloc, &fields)?.into()))
                } else if self.map_info(&recv_ty).is_some() {
                    let cap = self.builder.build_extract_value(s, 2, "iter.cap").map_err(llvm_err)?;
                    let zero = self.usize_ty.const_zero().into();
                    Ok(Some(
                        self.seq_value(&burdin_tir::Alloc::Global, &[(0, base), (1, len), (2, cap), (3, zero)])?.into(),
                    ))
                } else {
                    let _ = st;
                    Ok(Some(self.make_str(base, len)?.into()))
                }
            }
            (other, _) => Err(format!("unsupported iterator source `{other}`")),
        }
    }

    /// A `{ ptr, len, cap, index }` value of the list and map iterators.
    pub(super) fn seq_value(
        &mut self,
        alloc: &burdin_tir::Alloc,
        fields: &[(u32, BasicValueEnum<'ctx>)],
    ) -> GenResult<StructValue<'ctx>> {
        let mut agg = undef(self.iter_seq_ty_for(alloc).into()).into_struct_value();
        for (i, v) in fields {
            agg = self.builder.build_insert_value(agg, *v, *i, "").map_err(llvm_err)?.into_struct_value();
        }
        Ok(agg)
    }

    /// `it.next()` on a compiler-known iterator. `ty` is the call's `Option<Item>` type.
    pub(super) fn iter_next(
        &mut self,
        args: &'a [burdin_tir::Expr],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(item) = self.option_payload_ty(ty) else {
            return Err("an iterator's `next` returns an `Option`".into());
        };
        let item = &item;
        let item = item.clone();
        let recv_ty = args[0].ty.clone();
        match &recv_ty {
            _ if self.list_iter_info(&recv_ty).is_some() => {
                let (elem, alloc) = self.list_iter_info(&recv_ty).expect("checked by the guard");
                self.next_list_into(&args[0], &elem, &alloc, ty)
            }
            Ty::Slice(elem) => {
                let elem = (**elem).clone();
                self.next_slice(&args[0], &elem, &item, ty)
            }
            // A mutable list or array iterator walks the same `{ptr, len}` state as a slice; its item is always the
            // element's address.
            Ty::IterMut(elem) => {
                let elem = (**elem).clone();
                self.next_slice(&args[0], &elem, &item, ty)
            }
            Ty::Chars => self.next_chars(&args[0], ty),
            Ty::MapIter(k, v) | Ty::MapIterMut(k, v) => {
                let (k, v) = ((**k).clone(), (**v).clone());
                self.next_map(&args[0], &k, &v, &item, ty, false, &burdin_tir::Alloc::Global)
            }
            Ty::MapIntoIter(k, v, alloc) => {
                let (k, v, alloc) = ((**k).clone(), (**v).clone(), alloc.clone());
                self.next_map(&args[0], &k, &v, &item, ty, true, &alloc)
            }
            Ty::SliceChunks(elem, exact) => {
                let (elem, exact) = ((**elem).clone(), *exact);
                self.next_chunks(&args[0], &elem, exact, false, ty)
            }
            Ty::SliceWindows(elem) => {
                let elem = (**elem).clone();
                self.next_chunks(&args[0], &elem, false, true, ty)
            }
            other => Err(format!("`next` on `{other}` has no built-in implementation")),
        }
    }

    /// `it.next_back()` on a slice, `Chars`, or a range iterator (spec/iterators.md, IT14).
    pub(super) fn iter_next_back(
        &mut self,
        args: &'a [burdin_tir::Expr],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(item) = self.option_payload_ty(ty) else {
            return Err("an iterator's `next_back` returns an `Option`".into());
        };
        let item = &item;
        let item = item.clone();
        let recv_ty = args[0].ty.clone();
        match &recv_ty {
            Ty::Slice(elem) => {
                let elem = (**elem).clone();
                self.next_back_slice(&args[0], &elem, &item, ty)
            }
            Ty::Chars => self.next_back_chars(&args[0], ty),
            other => Err(format!("`next_back` on `{other}` has no built-in implementation")),
        }
    }

    /// The scratch slot (`option_ty`) a `next` implementation fills in its branches; `load_option` converts it.
    pub(super) fn option_slot(&mut self, ty: &Ty, name: &str) -> GenResult<Option<PointerValue<'ctx>>> {
        let inner = self.option_payload_ty(ty).ok_or("an iterator's `next` returns an `Option`")?;
        let scratch = self.option_ty(&inner);
        Ok(Some(self.alloca(scratch.into(), name)?))
    }

    pub(super) fn load_option(
        &mut self,
        slot: Option<PointerValue<'ctx>>,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = slot else { return Ok(None) };
        let inner = self.option_payload_ty(ty).ok_or("an iterator's `next` returns an `Option`")?;
        let scratch = self.option_ty(&inner);
        self.finish_option(scratch, slot, ty)
    }

    /// `[T].next()`: yields the first element (a copy for a `Copy` element, a view otherwise) and advances.
    fn next_slice(
        &mut self,
        recv: &'a burdin_tir::Expr,
        elem: &Ty,
        item: &Ty,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let s = self.builder.build_load(self.str_ty, slot, "slice.iter").map_err(llvm_err)?.into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "slice.len").map_err(llvm_err)?.into_int_value();
        let out = self.option_slot(ty, "next.out")?;
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, len, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let none_bb = self.new_block("next.none");
        let some_bb = self.new_block("next.some");
        let end = self.new_block("next.end");
        self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let none = self.option_ty(item).const_zero();
            self.builder.build_store(out, none).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        // A non-`Copy` element is yielded as a shared view of the buffer.
        let payload = if matches!(item, Ty::Ref { .. }) {
            Some(base.into())
        } else {
            match self.llvm_ty(elem) {
                Some(t) => Some(self.builder.build_load(t, base, "item").map_err(llvm_err)?),
                None => None,
            }
        };
        let one = self.usize_ty.const_int(1, false);
        let next_ptr = match self.llvm_ty(elem) {
            Some(t) => {
                // SAFETY (LLVM): one element was available.
                unsafe { self.builder.build_in_bounds_gep(t, base, &[one], "iter.ptr") }.map_err(llvm_err)?.into()
            }
            None => base.into(),
        };
        let next_len = self.builder.build_int_sub(len, one, "").map_err(llvm_err)?;
        let advanced = self.make_str(next_ptr, next_len.into())?;
        self.builder.build_store(slot, advanced).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(item, payload);
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// `[T].next_back()`: yields the last element (a copy for a `Copy` element, a view otherwise) and shortens
    /// the slice from the back.
    fn next_back_slice(
        &mut self,
        recv: &'a burdin_tir::Expr,
        elem: &Ty,
        item: &Ty,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let s = self.builder.build_load(self.str_ty, slot, "slice.iter").map_err(llvm_err)?.into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "slice.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "slice.len").map_err(llvm_err)?.into_int_value();
        let out = self.option_slot(ty, "next.out")?;
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, len, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let none_bb = self.new_block("back.none");
        let some_bb = self.new_block("back.some");
        let end = self.new_block("back.end");
        self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let none = self.option_ty(item).const_zero();
            self.builder.build_store(out, none).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let one = self.usize_ty.const_int(1, false);
        let last = self.builder.build_int_sub(len, one, "").map_err(llvm_err)?;
        // The last element, as a view of the buffer or a copy of it.
        let last_ptr = match self.llvm_ty(elem) {
            Some(t) => {
                // SAFETY (LLVM): `last < len`, because the slice is not empty.
                unsafe { self.builder.build_in_bounds_gep(t, base, &[last], "back.ptr") }.map_err(llvm_err)?
            }
            None => base,
        };
        let payload = if matches!(item, Ty::Ref { .. }) {
            Some(last_ptr.into())
        } else {
            match self.llvm_ty(elem) {
                Some(t) => Some(self.builder.build_load(t, last_ptr, "item").map_err(llvm_err)?),
                None => None,
            }
        };
        let shortened = self.make_str(base.into(), last.into())?;
        self.builder.build_store(slot, shortened).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(item, payload);
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// `Chars.next_back()`: decodes the last character and drops its bytes from the back.
    fn next_back_chars(&mut self, recv: &'a burdin_tir::Expr, ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let s = self.builder.build_load(self.str_ty, slot, "chars.iter").map_err(llvm_err)?.into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "chars.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "chars.len").map_err(llvm_err)?.into_int_value();
        let out = self.option_slot(ty, "char.out")?;
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, len, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let none_bb = self.new_block("back.none");
        let some_bb = self.new_block("back.some");
        let end = self.new_block("back.end");
        self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            self.builder.build_store(out, self.option_ty(&Ty::Char).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        // Walk back over at most three continuation bytes (`10xxxxxx`) to the first byte of the character.
        let i8_ty = self.cx.i8_type();
        let one = self.usize_ty.const_int(1, false);
        let mut start = self.builder.build_int_sub(len, one, "").map_err(llvm_err)?;
        for _ in 0..3 {
            // SAFETY (LLVM): `start < len`, because it only ever decreases from `len - 1`.
            let p = unsafe { self.builder.build_in_bounds_gep(i8_ty, base, &[start], "") }.map_err(llvm_err)?;
            let b = self.builder.build_load(i8_ty, p, "b").map_err(llvm_err)?.into_int_value();
            let top = self.builder.build_and(b, i8_ty.const_int(0xC0, false), "").map_err(llvm_err)?;
            let continuation = self
                .builder
                .build_int_compare(IntPredicate::EQ, top, i8_ty.const_int(0x80, false), "")
                .map_err(llvm_err)?;
            let not_first = self
                .builder
                .build_int_compare(IntPredicate::UGT, start, self.usize_ty.const_zero(), "")
                .map_err(llvm_err)?;
            let step = self.builder.build_and(continuation, not_first, "").map_err(llvm_err)?;
            let before = self.builder.build_int_sub(start, one, "").map_err(llvm_err)?;
            start = self.builder.build_select(step, before, start, "").map_err(llvm_err)?.into_int_value();
        }
        let (cp, _) = self.decode_char_at(base, len, start)?;
        let shortened = self.make_str(base.into(), start.into())?;
        self.builder.build_store(slot, shortened).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(&Ty::Char, Some(cp.into()));
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// The scalar value of the character that starts at byte `at`, and the number of bytes it takes.
    pub(super) fn decode_char_at(
        &mut self,
        base: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        at: IntValue<'ctx>,
    ) -> GenResult<(IntValue<'ctx>, IntValue<'ctx>)> {
        let i32_ty = self.cx.i32_type();
        // SAFETY (LLVM): `at < len`, checked by the caller.
        let b0_ptr =
            unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), base, &[at], "") }.map_err(llvm_err)?;
        let b0 = self.builder.build_load(self.cx.i8_type(), b0_ptr, "b0").map_err(llvm_err)?.into_int_value();
        let b0_wide = self.builder.build_int_z_extend(b0, i32_ty, "").map_err(llvm_err)?;
        let cont = |cg: &mut Self, index: u64| -> GenResult<IntValue<'ctx>> {
            let off = cg.usize_ty.const_int(index, false);
            let at_k = cg.builder.build_int_add(at, off, "").map_err(llvm_err)?;
            let avail = cg.builder.build_int_compare(IntPredicate::ULT, at_k, len, "").map_err(llvm_err)?;
            let more = cg.new_block("char.more");
            let done = cg.new_block("char.done");
            let slot = cg.alloca(i32_ty.into(), "char.byte")?;
            cg.builder.build_store(slot, i32_ty.const_zero()).map_err(llvm_err)?;
            cg.builder.build_conditional_branch(avail, more, done).map_err(llvm_err)?;
            cg.builder.position_at_end(more);
            // SAFETY (LLVM): `at + index < len`.
            let p = unsafe { cg.builder.build_in_bounds_gep(cg.cx.i8_type(), base, &[at_k], "") }.map_err(llvm_err)?;
            let b = cg.builder.build_load(cg.cx.i8_type(), p, "").map_err(llvm_err)?.into_int_value();
            let wide = cg.builder.build_int_z_extend(b, i32_ty, "").map_err(llvm_err)?;
            cg.builder.build_store(slot, wide).map_err(llvm_err)?;
            cg.builder.build_unconditional_branch(done).map_err(llvm_err)?;
            cg.builder.position_at_end(done);
            Ok(cg.builder.build_load(i32_ty, slot, "").map_err(llvm_err)?.into_int_value())
        };
        let cont1 = cont(self, 1)?;
        let cont2 = cont(self, 2)?;
        let cont3 = cont(self, 3)?;
        let mask = |cg: &mut Self, v: IntValue<'ctx>, m: u64, want: u64| -> GenResult<IntValue<'ctx>> {
            let a = cg.builder.build_and(v, i32_ty.const_int(m, false), "").map_err(llvm_err)?;
            cg.builder.build_int_compare(IntPredicate::EQ, a, i32_ty.const_int(want, false), "").map_err(llvm_err)
        };
        let is_1 = self
            .builder
            .build_int_compare(IntPredicate::ULT, b0_wide, i32_ty.const_int(0x80, false), "")
            .map_err(llvm_err)?;
        let is_2 = mask(self, b0_wide, 0xE0, 0xC0)?;
        let is_3 = mask(self, b0_wide, 0xF0, 0xE0)?;
        let is_4 = mask(self, b0_wide, 0xF8, 0xF0)?;
        let low = |cg: &mut Self, v: IntValue<'ctx>, m: u64, shift: u64| -> GenResult<IntValue<'ctx>> {
            let a = cg.builder.build_and(v, i32_ty.const_int(m, false), "").map_err(llvm_err)?;
            cg.builder.build_left_shift(a, i32_ty.const_int(shift, false), "").map_err(llvm_err)
        };
        let cont_mask =
            |cg: &mut Self, v: IntValue<'ctx>, shift: u64| -> GenResult<IntValue<'ctx>> { low(cg, v, 0x3F, shift) };
        let four = low(self, b0_wide, 0x07, 18)?;
        let three = low(self, b0_wide, 0x0F, 12)?;
        let two = low(self, b0_wide, 0x1F, 6)?;
        let m1_12 = cont_mask(self, cont1, 12)?;
        let m1_6 = cont_mask(self, cont1, 6)?;
        let m1_0 = cont_mask(self, cont1, 0)?;
        let m2_6 = cont_mask(self, cont2, 6)?;
        let m2_0 = cont_mask(self, cont2, 0)?;
        let m3_0 = cont_mask(self, cont3, 0)?;
        let cp4 = self.builder.build_or(four, m1_12, "").map_err(llvm_err)?;
        let cp4 = self.builder.build_or(cp4, m2_6, "").map_err(llvm_err)?;
        let cp4 = self.builder.build_or(cp4, m3_0, "").map_err(llvm_err)?;
        let cp3 = self.builder.build_or(three, m1_6, "").map_err(llvm_err)?;
        let cp3 = self.builder.build_or(cp3, m2_0, "").map_err(llvm_err)?;
        let cp2 = self.builder.build_or(two, m1_0, "").map_err(llvm_err)?;
        let cp = self.builder.build_select(is_2, cp2, cp4, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_select(is_3, cp3, cp, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_select(is_1, b0_wide, cp, "").map_err(llvm_err)?.into_int_value();
        let cp = self.builder.build_int_truncate(cp, i32_ty, "").map_err(llvm_err)?;
        let w2 = self
            .builder
            .build_select(is_2, self.usize_ty.const_int(2, false), self.usize_ty.const_int(1, false), "")
            .map_err(llvm_err)?
            .into_int_value();
        let w3 = self
            .builder
            .build_select(is_3, self.usize_ty.const_int(3, false), w2, "")
            .map_err(llvm_err)?
            .into_int_value();
        let w4 = self
            .builder
            .build_select(is_4, self.usize_ty.const_int(4, false), w3, "")
            .map_err(llvm_err)?
            .into_int_value();
        let width = self
            .builder
            .build_select(is_1, self.usize_ty.const_int(1, false), w4, "")
            .map_err(llvm_err)?
            .into_int_value();
        Ok((cp, width))
    }

    /// `Chars.next()`: decodes the first character and advances by its width.
    fn next_chars(&mut self, recv: &'a burdin_tir::Expr, ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let s = self.builder.build_load(self.str_ty, slot, "chars.iter").map_err(llvm_err)?.into_struct_value();
        let base = self.builder.build_extract_value(s, 0, "chars.ptr").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_extract_value(s, 1, "chars.len").map_err(llvm_err)?.into_int_value();
        let out = self.option_slot(ty, "char.out")?;
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, len, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let none_bb = self.new_block("char.none");
        let some_bb = self.new_block("char.some");
        let end = self.new_block("char.end");
        self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            self.builder.build_store(out, self.option_ty(&Ty::Char).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let (cp, width) = self.decode_char_at(base, len, self.usize_ty.const_zero())?;
        let next_ptr = unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), base, &[width], "") }
            .map_err(llvm_err)?
            .into();
        let next_len = self.builder.build_int_sub(len, width, "").map_err(llvm_err)?;
        let advanced = self.make_str(next_ptr, next_len.into())?;
        self.builder.build_store(slot, advanced).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(&Ty::Char, Some(cp.into()));
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// `ListIntoIter<T>.next()`: moves the element at the cursor out of the buffer.
    fn next_list_into(
        &mut self,
        recv: &'a burdin_tir::Expr,
        elem: &Ty,
        alloc: &burdin_tir::Alloc,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let seq_ty = self.iter_seq_ty_for(alloc);
        let data_field = self.builder.build_struct_gep(seq_ty, slot, 0, "iter.ptr").map_err(llvm_err)?;
        let base = self.builder.build_load(self.ptr_ty(), data_field, "").map_err(llvm_err)?.into_pointer_value();
        let len_field = self.builder.build_struct_gep(seq_ty, slot, 1, "iter.len").map_err(llvm_err)?;
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let index_ptr = self.builder.build_struct_gep(seq_ty, slot, 3, "iter.index").map_err(llvm_err)?;
        let index = self.builder.build_load(self.usize_ty, index_ptr, "").map_err(llvm_err)?.into_int_value();
        let out = self.option_slot(ty, "next.out")?;
        let done = self.builder.build_int_compare(IntPredicate::UGE, index, len, "").map_err(llvm_err)?;
        let none_bb = self.new_block("next.none");
        let some_bb = self.new_block("next.some");
        let end = self.new_block("next.end");
        self.builder.build_conditional_branch(done, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            self.builder.build_store(out, self.option_ty(elem).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let value = match self.llvm_ty(elem) {
            Some(t) => {
                // SAFETY (LLVM): index < len.
                let p = unsafe { self.builder.build_in_bounds_gep(t, base, &[index], "item") }.map_err(llvm_err)?;
                Some(self.builder.build_load(t, p, "item.value").map_err(llvm_err)?)
            }
            None => None,
        };
        let next = self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index_ptr, next).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(elem, value);
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// `MapIter` and `MapIntoIter`: scans for the next live slot. The borrowing form yields `(&K, &V)`; the
    /// owning form moves the key and value out and zeroes the slot's hash word.
    #[allow(clippy::too_many_arguments)]
    fn next_map(
        &mut self,
        recv: &'a burdin_tir::Expr,
        key: &Ty,
        value: &Ty,
        item: &Ty,
        ty: &Ty,
        owning: bool,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let seq_ty = self.iter_seq_ty_for(alloc);
        let data_field = self.builder.build_struct_gep(seq_ty, slot, 0, "iter.ptr").map_err(llvm_err)?;
        let base = self.builder.build_load(self.ptr_ty(), data_field, "").map_err(llvm_err)?.into_pointer_value();
        let cap_field = self.builder.build_struct_gep(seq_ty, slot, 2, "iter.cap").map_err(llvm_err)?;
        let cap = self.builder.build_load(self.usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        let index_ptr = self.builder.build_struct_gep(seq_ty, slot, 3, "iter.index").map_err(llvm_err)?;
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let out = self.option_slot(ty, "next.out")?;
        let header = self.new_block("map.next.cond");
        let body = self.new_block("map.next.body");
        let found = self.new_block("map.next.found");
        let advance = self.new_block("map.next.advance");
        let none_bb = self.new_block("map.next.none");
        let end = self.new_block("map.next.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let index = self.builder.build_load(self.usize_ty, index_ptr, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, index, cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body, none_bb).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // SAFETY (LLVM): index < cap.
        let ent = unsafe { self.builder.build_in_bounds_gep(entry_t, base, &[index], "map.slot") }.map_err(llvm_err)?;
        let hash_field = self.builder.build_struct_gep(entry_t, ent, 2, "map.hash").map_err(llvm_err)?;
        let hash = self.builder.build_load(self.cx.i64_type(), hash_field, "").map_err(llvm_err)?.into_int_value();
        let live = self
            .builder
            .build_int_compare(IntPredicate::NE, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(live, found, advance).map_err(llvm_err)?;
        self.builder.position_at_end(advance);
        let next = self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index_ptr, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(found);
        let key_ptr = self.builder.build_struct_gep(entry_t, ent, 0, "map.key").map_err(llvm_err)?;
        let value_ptr = self.builder.build_struct_gep(entry_t, ent, 1, "map.value").map_err(llvm_err)?;
        let mut payload = Vec::new();
        if owning {
            let kt = self.llvm_ty(key).expect("a map key has a type");
            let vt = self.llvm_ty(value).expect("a map value has a type");
            payload.push(self.builder.build_load(kt, key_ptr, "key").map_err(llvm_err)?);
            payload.push(self.builder.build_load(vt, value_ptr, "value").map_err(llvm_err)?);
            // A moved entry is no longer live: its drop glue must skip it.
            self.builder.build_store(hash_field, self.cx.i64_type().const_zero()).map_err(llvm_err)?;
        } else {
            payload.push(BasicValueEnum::from(key_ptr));
            payload.push(BasicValueEnum::from(value_ptr));
        }
        let next = self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index_ptr, next).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let mut tuple = undef(self.llvm_ty(item).expect("a map item has a type")).into_struct_value();
            for (i, v) in payload.iter().enumerate() {
                tuple = self.builder.build_insert_value(tuple, *v, i as u32, "").map_err(llvm_err)?.into_struct_value();
            }
            let some = self.some_value(item, Some(tuple.into()));
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            self.builder.build_store(out, self.option_ty(item).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }

    /// `chunks`, `chunks_exact`, and `windows`: yields a slice view of the next chunk or window and advances.
    /// A chunk advances by its length (the last one may be shorter); a window advances by one element.
    fn next_chunks(
        &mut self,
        recv: &'a burdin_tir::Expr,
        elem: &Ty,
        exact: bool,
        windows: bool,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(slot) = self.place_ptr(recv)? else {
            self.expr_mode(recv, Mode::Use)?;
            return Ok(None);
        };
        let tt = self.slice_iter_ty();
        let ptr_field = self.builder.build_struct_gep(tt, slot, 0, "windows.ptr").map_err(llvm_err)?;
        let base = self.builder.build_load(self.ptr_ty(), ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len_field = self.builder.build_struct_gep(tt, slot, 1, "windows.len").map_err(llvm_err)?;
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let size_field = self.builder.build_struct_gep(tt, slot, 2, "windows.size").map_err(llvm_err)?;
        let size = self.builder.build_load(self.usize_ty, size_field, "").map_err(llvm_err)?.into_int_value();
        let index_ptr = self.builder.build_struct_gep(tt, slot, 3, "windows.index").map_err(llvm_err)?;
        let index = self.builder.build_load(self.usize_ty, index_ptr, "").map_err(llvm_err)?.into_int_value();
        let Some(item) = self.option_payload_ty(ty) else {
            return Err("an iterator's `next` returns an `Option`".into());
        };
        let item = &item;
        let item = item.clone();
        let out = self.option_slot(ty, "windows.out")?;
        // `index` never passes `len`: a chunk advances by what it yielded, and a window stops before the end.
        let remaining = self.builder.build_int_sub(len, index, "").map_err(llvm_err)?;
        let done = if windows || exact {
            self.builder.build_int_compare(IntPredicate::ULT, remaining, size, "").map_err(llvm_err)?
        } else {
            self.builder
                .build_int_compare(IntPredicate::EQ, remaining, self.usize_ty.const_zero(), "")
                .map_err(llvm_err)?
        };
        let none_bb = self.new_block("windows.none");
        let some_bb = self.new_block("windows.some");
        let end = self.new_block("windows.end");
        self.builder.build_conditional_branch(done, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            self.builder.build_store(out, self.option_ty(&item).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let take = if windows || exact {
            size
        } else {
            // The last chunk of `chunks` is shorter when the length is not a multiple of the size.
            let short = self.builder.build_int_compare(IntPredicate::ULT, remaining, size, "").map_err(llvm_err)?;
            self.builder.build_select(short, remaining, size, "").map_err(llvm_err)?.into_int_value()
        };
        let value = match self.llvm_ty(elem) {
            Some(elem_t) => {
                // SAFETY (LLVM): `index + take <= len`.
                let p = unsafe { self.builder.build_in_bounds_gep(elem_t, base, &[index], "window.ptr") }
                    .map_err(llvm_err)?;
                Some(self.make_str(p.into(), take.into())?.into())
            }
            None => None,
        };
        let advance = if windows { self.usize_ty.const_int(1, false) } else { take };
        let next = self.builder.build_int_add(index, advance, "").map_err(llvm_err)?;
        self.builder.build_store(index_ptr, next).map_err(llvm_err)?;
        if let (Some(out), Some(_)) = (out, self.llvm_ty(ty)) {
            let some = self.some_value(&item, value);
            self.builder.build_store(out, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        self.load_option(out, ty)
    }
}
