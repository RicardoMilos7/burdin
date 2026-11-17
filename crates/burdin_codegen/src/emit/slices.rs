//! The slice methods the compiler implements: the checked reads (`get`), the operations that write through a
//! slice (`swap`, `fill`, `copy_from_slice`, `clone_from_slice`), and the `chunks` and `windows` iterators
//! (`spec/stdlib.md`, "Slices"). The algorithms built on `swap` live in `std/src/slice.bd`.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_diag::Span;
use burdin_tir::{self as tir, Ty};
use inkwell::IntPredicate;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, IntValue, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The element pointer of a slice at `index`, or `None` for an element without a runtime value.
    fn slice_elem_ptr(
        &mut self,
        ptr: PointerValue<'ctx>,
        elem: &Ty,
        index: IntValue<'ctx>,
        name: &str,
    ) -> GenResult<Option<PointerValue<'ctx>>> {
        match self.llvm_ty(elem) {
            Some(t) => {
                // SAFETY (LLVM): the caller checked `index` against the length.
                Ok(Some(unsafe { self.builder.build_in_bounds_gep(t, ptr, &[index], name) }.map_err(llvm_err)?))
            }
            None => Ok(None),
        }
    }

    /// The receiver's pointer and length, with its value evaluated once.
    fn slice_parts(&mut self, recv: &'a tir::Expr) -> GenResult<(PointerValue<'ctx>, IntValue<'ctx>)> {
        let (ptr, len) = self.view_parts(recv)?;
        Ok((ptr.into_pointer_value(), len.into_int_value()))
    }

    /// The address holding the argument value, for the operations that clone it repeatedly: its place when it has
    /// one, otherwise a temporary slot of the statement.
    fn value_address(&mut self, e: &'a tir::Expr) -> GenResult<Option<PointerValue<'ctx>>> {
        if let Some(ptr) = self.place_ptr(e)? {
            return Ok(Some(ptr));
        }
        let Some(t) = self.llvm_ty(&e.ty) else { return Ok(None) };
        let v = self.expr_mode(e, Mode::Use)?;
        let slot = self.alloca(t, "slice.value")?;
        let v = self.need(v, &e.ty);
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        self.add_temp_slot(slot, &e.ty);
        Ok(Some(slot))
    }

    /// `xs.get(i)`, `xs.get(range)`, `xs.swap(a, b)`, `xs.fill(value)`, `xs.copy_from_slice(other)`,
    /// `xs.clone_from_slice(other)`, `xs.chunks(n)`, `xs.chunks_exact(n)`, `xs.windows(n)`, and
    /// `list.as_slice()`. `ty` is the call's result type.
    pub(super) fn slice_method(
        &mut self,
        m: tir::Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match m {
            tir::Method::AsSlice => {
                let (ptr, len) = self.view_parts(&args[0])?;
                Ok(Some(self.make_str(ptr, len)?.into()))
            }
            tir::Method::SliceGet => self.slice_get(args, ty),
            tir::Method::SliceLast => self.slice_last(args, ty),
            tir::Method::SliceToList => self.slice_to_list(args, ty, span),
            tir::Method::SliceGetRange => self.slice_get_range(args, ty),
            tir::Method::SliceSwap => self.slice_swap(args, span),
            tir::Method::SliceFill => self.slice_fill(args),
            tir::Method::SliceCopyFromSlice => self.slice_copy(args, span, false),
            tir::Method::SliceCloneFromSlice => self.slice_copy(args, span, true),
            tir::Method::SliceChunks | tir::Method::SliceWindows => self.slice_windows_value(m, args, ty, span),
            tir::Method::SliceSortBy => self.slice_sort_by(args),
            _ => Err(format!("unsupported slice method {m:?}")),
        }
    }

    /// `xs.get(i): Option<&T>`.
    fn slice_get(&mut self, args: &'a [tir::Expr], ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let index = self.usize_value(&args[1])?;
        let Ty::Option(inner) = ty else { return Err("`get` returns an `Option`".into()) };
        let item = inner.as_ref().clone();
        let out = self.alloca(self.option_ty(&item).into(), "get.out")?;
        let in_bounds = self.builder.build_int_compare(IntPredicate::ULT, index, len, "").map_err(llvm_err)?;
        let none_bb = self.new_block("get.none");
        let some_bb = self.new_block("get.some");
        let end = self.new_block("get.end");
        self.builder.build_conditional_branch(in_bounds, some_bb, none_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        self.builder.build_store(out, self.option_ty(&item).const_zero()).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let elem = match &item {
            Ty::Ref { inner, .. } => inner.as_ref().clone(),
            other => other.clone(),
        };
        let point = self.slice_elem_ptr(ptr, &elem, index, "get.ptr")?;
        let some = self.some_value(&item, point.map(BasicValueEnum::from));
        self.builder.build_store(out, some).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(self.option_ty(&item), out, "").map_err(llvm_err)?))
    }

    /// `xs.last(): Option<&T>`: a view of the last element, or `None` when the slice is empty.
    fn slice_last(&mut self, args: &'a [tir::Expr], ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let Ty::Option(inner) = ty else { return Err("`last` returns an `Option`".into()) };
        let item = inner.as_ref().clone();
        let elem = match &item {
            Ty::Ref { inner, .. } => inner.as_ref().clone(),
            other => other.clone(),
        };
        let out = self.alloca(self.option_ty(&item).into(), "last.out")?;
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, len, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        let none_bb = self.new_block("last.none");
        let some_bb = self.new_block("last.some");
        let end = self.new_block("last.end");
        self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        self.builder.build_store(out, self.option_ty(&item).const_zero()).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let index = self.builder.build_int_sub(len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let point = self.slice_elem_ptr(ptr, &elem, index, "last.ptr")?;
        let some = self.some_value(&item, point.map(BasicValueEnum::from));
        self.builder.build_store(out, some).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(self.option_ty(&item), out, "").map_err(llvm_err)?))
    }

    /// `xs.get(range): Option<[T]>`: checked slicing. An out-of-range or reversed range is `None`, never a panic.
    fn slice_get_range(&mut self, args: &'a [tir::Expr], ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let range_ty = args[1].ty.clone();
        let range = self.expr_mode(&args[1], Mode::Use)?;
        let range = self.need(range, &range_ty).into_struct_value();
        let lo = self.builder.build_extract_value(range, 0, "range.lo").map_err(llvm_err)?.into_int_value();
        let hi = self.builder.build_extract_value(range, 1, "range.hi").map_err(llvm_err)?.into_int_value();
        let inclusive = matches!(range_ty, Ty::Range(_, true));
        let Ty::Option(inner) = ty else { return Err("`get` returns an `Option`".into()) };
        let item = inner.as_ref().clone();
        let out = self.alloca(self.option_ty(&item).into(), "get.range.out")?;
        let hi_ok = if inclusive {
            self.builder.build_int_compare(IntPredicate::ULT, hi, len, "").map_err(llvm_err)?
        } else {
            self.builder.build_int_compare(IntPredicate::ULE, hi, len, "").map_err(llvm_err)?
        };
        let lo_ok = self.builder.build_int_compare(IntPredicate::ULE, lo, hi, "").map_err(llvm_err)?;
        let ok = self.builder.build_and(hi_ok, lo_ok, "").map_err(llvm_err)?;
        let none_bb = self.new_block("get.range.none");
        let some_bb = self.new_block("get.range.some");
        let end = self.new_block("get.range.end");
        self.builder.build_conditional_branch(ok, some_bb, none_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        self.builder.build_store(out, self.option_ty(&item).const_zero()).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let end_index = if inclusive {
            self.builder.build_int_add(hi, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?
        } else {
            hi
        };
        let elem = match &item {
            Ty::Slice(elem) => elem.as_ref().clone(),
            _ => return Err("`get(range)` returns an `Option<[T]>`".into()),
        };
        let point = self.slice_elem_ptr(ptr, &elem, lo, "get.range.ptr")?;
        let slice_len = self.builder.build_int_sub(end_index, lo, "").map_err(llvm_err)?;
        let value = match point {
            Some(point) => {
                let slice = self.make_str(point.into(), slice_len.into())?;
                self.some_value(&item, Some(slice.into()))
            }
            None => self.some_value(&item, None),
        };
        self.builder.build_store(out, value).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(self.option_ty(&item), out, "").map_err(llvm_err)?))
    }

    /// `xs.swap(a, b)`: exchanges two elements by moving their bytes.
    fn slice_swap(&mut self, args: &'a [tir::Expr], span: Span) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let a = self.usize_value(&args[1])?;
        let b = self.usize_value(&args[2])?;
        let a_ok = self.builder.build_int_compare(IntPredicate::ULT, a, len, "").map_err(llvm_err)?;
        let b_ok = self.builder.build_int_compare(IntPredicate::ULT, b, len, "").map_err(llvm_err)?;
        let ok = self.builder.build_and(a_ok, b_ok, "").map_err(llvm_err)?;
        self.check(ok, "swap: index out of bounds", span)?;
        let elem = match &args[0].ty {
            Ty::Slice(elem) => elem.as_ref().clone(),
            other => return Err(format!("`swap` on `{other}`")),
        };
        let (Some(pa), Some(pb)) =
            (self.slice_elem_ptr(ptr, &elem, a, "swap.a")?, self.slice_elem_ptr(ptr, &elem, b, "swap.b")?)
        else {
            return Ok(None);
        };
        let Some(elem_ty) = self.llvm_ty(&elem) else { return Ok(None) };
        let tmp = self.alloca(elem_ty, "swap.tmp")?;
        let size = self.elem_size(&elem);
        let bytes = self.usize_ty.const_int(size, false);
        self.builder.build_memcpy(tmp, 1, pa, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(pa, 1, pb, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(pb, 1, tmp, 1, bytes).map_err(llvm_err)?;
        Ok(None)
    }

    /// `xs.fill(value)`: drops each element and stores a clone of `value` in its place.
    fn slice_fill(&mut self, args: &'a [tir::Expr]) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let elem = match &args[0].ty {
            Ty::Slice(elem) => elem.as_ref().clone(),
            other => return Err(format!("`fill` on `{other}`")),
        };
        let Some(src) = self.value_address(&args[1])? else { return Ok(None) };
        let Some(elem_ty) = self.llvm_ty(&elem) else { return Ok(None) };
        let index = self.alloca(self.usize_ty.into(), "fill.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("fill.cond");
        let body = self.new_block("fill.body");
        let end = self.new_block("fill.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // SAFETY (LLVM): `i < len`.
        let p = unsafe { self.builder.build_in_bounds_gep(elem_ty, ptr, &[i], "fill.ptr") }.map_err(llvm_err)?;
        if self.needs_drop(&elem) {
            self.drop_value(p, &elem)?;
        }
        self.clone_value(p, src, &elem)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(None)
    }

    /// `xs.copy_from_slice(other)` and `xs.clone_from_slice(other)`: the lengths must match. The copy form moves
    /// the bytes; the clone form drops each old element and clones the new one.
    fn slice_copy(
        &mut self,
        args: &'a [tir::Expr],
        span: Span,
        clone: bool,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (dst, len) = self.slice_parts(&args[0])?;
        let (src, src_len) = self.slice_parts(&args[1])?;
        let same = self.builder.build_int_compare(IntPredicate::EQ, len, src_len, "").map_err(llvm_err)?;
        let message = if clone {
            "clone_from_slice: source and destination lengths differ"
        } else {
            "copy_from_slice: source and destination lengths differ"
        };
        self.check(same, message, span)?;
        let elem = match &args[0].ty {
            Ty::Slice(elem) => elem.as_ref().clone(),
            other => return Err(format!("a slice copy on `{other}`")),
        };
        let Some(elem_ty) = self.llvm_ty(&elem) else { return Ok(None) };
        if !clone {
            // The elements are `Copy`, so moving their bytes leaves the source valid.
            let size = self.elem_size(&elem);
            let bytes = self.builder.build_int_mul(len, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
            self.builder.build_memcpy(dst, 1, src, 1, bytes).map_err(llvm_err)?;
            return Ok(None);
        }
        let index = self.alloca(self.usize_ty.into(), "copy.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("copy.cond");
        let body = self.new_block("copy.body");
        let end = self.new_block("copy.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // SAFETY (LLVM): `i < len`, and both lengths are equal.
        let from = unsafe { self.builder.build_in_bounds_gep(elem_ty, src, &[i], "copy.src") }.map_err(llvm_err)?;
        let to = unsafe { self.builder.build_in_bounds_gep(elem_ty, dst, &[i], "copy.dst") }.map_err(llvm_err)?;
        if self.needs_drop(&elem) {
            self.drop_value(to, &elem)?;
        }
        self.clone_value(to, from, &elem)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(None)
    }

    /// `xs.chunks(n)`, `xs.chunks_exact(n)`, and `xs.windows(n)`: the iterator value `{ ptr, len, size, 0 }`.
    /// A size of zero panics here, when the iterator is made.
    fn slice_windows_value(
        &mut self,
        m: tir::Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let size = self.usize_value(&args[1])?;
        let zero = self.usize_ty.const_zero();
        let non_zero = self.builder.build_int_compare(IntPredicate::NE, size, zero, "").map_err(llvm_err)?;
        let message = if m == tir::Method::SliceWindows {
            "windows: window size must be non-zero"
        } else {
            "chunks: chunk size must be non-zero"
        };
        self.check(non_zero, message, span)?;
        let tt = match ty {
            Ty::SliceChunks(..) | Ty::SliceWindows(..) => self.slice_iter_ty(),
            other => return Err(format!("a chunks iterator of type `{other}`")),
        };
        let mut agg = undef(tt.into()).into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, BasicValueEnum::from(ptr), 0, "")
            .map_err(llvm_err)?
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, BasicValueEnum::from(len), 1, "")
            .map_err(llvm_err)?
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, BasicValueEnum::from(size), 2, "")
            .map_err(llvm_err)?
            .into_struct_value();
        agg = self
            .builder
            .build_insert_value(agg, BasicValueEnum::from(zero), 3, "")
            .map_err(llvm_err)?
            .into_struct_value();
        Ok(Some(agg.into()))
    }

    /// `xs.__sort_by(less)`: insertion sort with byte swaps. The comparator receives views of two elements and
    /// says whether the first is strictly less; swapping only when it is keeps the sort stable. The comparator
    /// value is dropped here, because the method takes ownership of it (`Method::owns_argument`).
    fn slice_sort_by(&mut self, args: &'a [tir::Expr]) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let (ptr, len) = self.slice_parts(&args[0])?;
        let elem = match &args[0].ty {
            Ty::Slice(elem) => elem.as_ref().clone(),
            other => return Err(format!("`sort_by` on `{other}`")),
        };
        let closure_ty = args[1].ty.clone();
        let closure_value = self.expr_mode(&args[1], Mode::Move)?;
        let Some(closure_slot) = self.spill(closure_value, &closure_ty)? else { return Ok(None) };
        let closure =
            self.builder.build_load(self.closure_ty, closure_slot, "sort.less").map_err(llvm_err)?.into_struct_value();
        if self.llvm_ty(&elem).is_none() {
            self.drop_value(closure_slot, &closure_ty)?;
            return Ok(None);
        }
        let elem_ty = self.llvm_ty(&elem).expect("checked above");
        let size = self.elem_size(&elem);
        let bytes = self.usize_ty.const_int(size, false);
        let temp = self.alloca(elem_ty, "sort.tmp")?;
        let outer = self.alloca(self.usize_ty.into(), "sort.i")?;
        self.builder.build_store(outer, self.usize_ty.const_int(1, false)).map_err(llvm_err)?;
        let outer_header = self.new_block("sort.cond");
        let outer_body = self.new_block("sort.body");
        let exit = self.new_block("sort.end");
        self.builder.build_unconditional_branch(outer_header).map_err(llvm_err)?;
        self.builder.position_at_end(outer_header);
        let i = self.builder.build_load(self.usize_ty, outer, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, outer_body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(outer_body);
        let inner = self.alloca(self.usize_ty.into(), "sort.j")?;
        self.builder.build_store(inner, i).map_err(llvm_err)?;
        let inner_header = self.new_block("sort.inner");
        let compare = self.new_block("sort.compare");
        let swap = self.new_block("sort.swap");
        let next_outer = self.new_block("sort.next");
        self.builder.build_unconditional_branch(inner_header).map_err(llvm_err)?;
        self.builder.position_at_end(inner_header);
        let j = self.builder.build_load(self.usize_ty, inner, "").map_err(llvm_err)?.into_int_value();
        let positive =
            self.builder.build_int_compare(IntPredicate::UGT, j, self.usize_ty.const_zero(), "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(positive, compare, next_outer).map_err(llvm_err)?;
        self.builder.position_at_end(compare);
        let prev = self.builder.build_int_sub(j, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        // SAFETY (LLVM): `j <= i < len`, so `j - 1` and `j` are both in range.
        let pa = unsafe { self.builder.build_in_bounds_gep(elem_ty, ptr, &[prev], "sort.prev") }.map_err(llvm_err)?;
        let pb = unsafe { self.builder.build_in_bounds_gep(elem_ty, ptr, &[j], "sort.cur") }.map_err(llvm_err)?;
        let less = self
            .call_closure_with(
                closure,
                &closure_ty,
                tir::FnKind::FnMut,
                &[
                    BasicMetadataValueEnum::from(BasicValueEnum::from(pb)),
                    BasicMetadataValueEnum::from(BasicValueEnum::from(pa)),
                ],
            )?
            .map(|v| v.into_int_value())
            .unwrap_or_else(|| self.cx.bool_type().const_zero());
        self.builder.build_conditional_branch(less, swap, next_outer).map_err(llvm_err)?;
        self.builder.position_at_end(swap);
        self.builder.build_memcpy(temp, 1, pa, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(pa, 1, pb, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(pb, 1, temp, 1, bytes).map_err(llvm_err)?;
        self.builder.build_store(inner, prev).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(inner_header).map_err(llvm_err)?;
        self.builder.position_at_end(next_outer);
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(outer, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(outer_header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        self.drop_value(closure_slot, &closure_ty)?;
        Ok(None)
    }
}
