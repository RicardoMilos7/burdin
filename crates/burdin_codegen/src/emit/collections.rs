//! `List<T>` and `Map<K, V>`: their layout, growth, indexing, and methods.
//!
//! Both have the three-word layout `{ ptr, len, cap }`, like a `String`. A list buffer holds `cap` elements of
//! `T`. A map buffer is an open-addressing hash table: `cap` slots (a power of two) of `(key, value, hash)` entries,
//! where a zero hash marks an empty slot and `len` counts the live entries. A key is placed at `hash & (cap - 1)`
//! and probes forward; a removal shifts the rest of its probe cluster back, so there are no tombstones. The table
//! grows (and rehashes) when the next insert would pass a 3/4 load factor. Iteration scans the slots, so its order
//! is unspecified, as the standard library says.

use super::{Codegen, GenResult, Mode, llvm_err};
use burdin_diag::Span;
use burdin_tir::{self as tir, FnKind, Method, Ty};
use inkwell::types::BasicType;
use inkwell::values::{BasicValueEnum, IntValue, PointerValue, StructValue};
use inkwell::{AddressSpace, IntPredicate};

/// The three words of a list or map, loaded from its place, and the allocator value it stores.
pub(super) struct Seq<'ctx> {
    addr: PointerValue<'ctx>,
    pub(super) data: PointerValue<'ctx>,
    pub(super) len: IntValue<'ctx>,
    pub(super) cap: IntValue<'ctx>,
    alloc: burdin_tir::Alloc,
    /// The address of the stored allocator value, when the container has one.
    alloc_addr: Option<PointerValue<'ctx>>,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn ptr_ty(&self) -> inkwell::types::PointerType<'ctx> {
        self.cx.ptr_type(AddressSpace::default())
    }

    fn seq_field(
        &mut self,
        addr: PointerValue<'ctx>,
        alloc: &burdin_tir::Alloc,
        index: u32,
        name: &str,
    ) -> GenResult<PointerValue<'ctx>> {
        let ty = self.seq_ty(alloc);
        self.builder.build_struct_gep(ty, addr, index, name).map_err(llvm_err)
    }

    /// The allocator of a container value, from its type.
    fn seq_alloc(ty: &Ty) -> burdin_tir::Alloc {
        match ty {
            Ty::List(_, alloc) | Ty::Map(_, _, alloc) | Ty::ListIntoIter(_, alloc) | Ty::MapIntoIter(_, _, alloc) => {
                alloc.clone()
            }
            _ => burdin_tir::Alloc::Global,
        }
    }

    /// The `{ ptr, len, cap }` words of a list or map place.
    fn seq_load(&mut self, e: &'a tir::Expr) -> GenResult<Option<Seq<'ctx>>> {
        let Some(addr) = self.place_ptr(e)? else { return Ok(None) };
        let alloc = Self::seq_alloc(&e.ty);
        self.seq_load_at(addr, &alloc).map(Some)
    }

    /// The `{ ptr, len, cap }` words of a list or map at `addr`, whose allocator the type names.
    pub(super) fn seq_load_at(&mut self, addr: PointerValue<'ctx>, alloc: &burdin_tir::Alloc) -> GenResult<Seq<'ctx>> {
        let ptr_ty = self.ptr_ty();
        let f0 = self.seq_field(addr, alloc, 0, "seq.ptr")?;
        let f1 = self.seq_field(addr, alloc, 1, "seq.len")?;
        let f2 = self.seq_field(addr, alloc, 2, "seq.cap")?;
        let data = self.builder.build_load(ptr_ty, f0, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, f1, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, f2, "").map_err(llvm_err)?.into_int_value();
        let alloc_addr = match alloc.allocator_ty() {
            Some(_) => Some(self.seq_field(addr, alloc, 3, "seq.alloc")?),
            None => None,
        };
        Ok(Seq { addr, data, len, cap, alloc: alloc.clone(), alloc_addr })
    }

    pub(super) fn seq_store(&mut self, seq: &Seq<'ctx>) -> GenResult<()> {
        let f0 = self.seq_field(seq.addr, &seq.alloc, 0, "seq.ptr")?;
        let f1 = self.seq_field(seq.addr, &seq.alloc, 1, "seq.len")?;
        let f2 = self.seq_field(seq.addr, &seq.alloc, 2, "seq.cap")?;
        self.builder.build_store(f0, seq.data).map_err(llvm_err)?;
        self.builder.build_store(f1, seq.len).map_err(llvm_err)?;
        self.builder.build_store(f2, seq.cap).map_err(llvm_err)?;
        Ok(())
    }

    /// The byte size of one element, at least one so that growing always allocates.
    pub(super) fn elem_size(&self, elem: &Ty) -> u64 {
        self.llvm_ty(elem).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1)
    }

    fn usize_zero(&self) -> IntValue<'ctx> {
        self.usize_ty.const_zero()
    }

    /// Grows the buffer to hold at least one more element, through the container's allocator.
    fn seq_grow(&mut self, seq: &mut Seq<'ctx>, elem: &Ty, span: Span) -> GenResult<()> {
        let full = self.builder.build_int_compare(IntPredicate::EQ, seq.len, seq.cap, "").map_err(llvm_err)?;
        let grow = self.new_block("seq.grow");
        let done = self.new_block("seq.grow.end");
        let before = self.builder.get_insert_block().expect("positioned");
        self.builder.build_conditional_branch(full, grow, done).map_err(llvm_err)?;
        self.builder.position_at_end(grow);
        let doubled = self.builder.build_int_mul(seq.cap, self.usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let at_least_four = self
            .builder
            .build_select(
                self.builder
                    .build_int_compare(IntPredicate::ULT, doubled, self.usize_ty.const_int(4, false), "")
                    .map_err(llvm_err)?,
                self.usize_ty.const_int(4, false),
                doubled,
                "",
            )
            .map_err(llvm_err)?
            .into_int_value();
        let size = self.usize_ty.const_int(self.elem_size(elem), false);
        let old_bytes = self.builder.build_int_mul(seq.cap, size, "").map_err(llvm_err)?;
        let new_bytes = self.builder.build_int_mul(at_least_four, size, "").map_err(llvm_err)?;
        let align = self.value_align(elem);
        let alloc = seq.alloc.clone();
        let alloc_addr = seq.alloc_addr;
        let (data, cap, alloc2, alloc_addr2) = (seq.data, seq.cap, alloc.clone(), alloc_addr);
        let first = self.new_block("seq.alloc");
        let again = self.new_block("seq.realloc");
        let grown = self.new_block("seq.grown");
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(empty, first, again).map_err(llvm_err)?;
        self.builder.position_at_end(first);
        let fresh = self.alloc_in(&alloc, alloc_addr, new_bytes, align, Some(span))?;
        self.builder.build_unconditional_branch(grown).map_err(llvm_err)?;
        self.builder.position_at_end(again);
        let renewed = self.realloc_in(&alloc2, alloc_addr2, data, old_bytes, new_bytes, align, Some(span))?;
        self.builder.build_unconditional_branch(grown).map_err(llvm_err)?;
        self.builder.position_at_end(grown);
        let data_phi = self.builder.build_phi(self.ptr_ty(), "seq.data").map_err(llvm_err)?;
        data_phi.add_incoming(&[(&fresh, first), (&renewed, again)]);
        let cap_phi = self.builder.build_phi(self.usize_ty, "seq.cap").map_err(llvm_err)?;
        cap_phi.add_incoming(&[(&at_least_four, first), (&at_least_four, again)]);
        seq.data = data_phi.as_basic_value().into_pointer_value();
        seq.cap = cap_phi.as_basic_value().into_int_value();
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        let grown_data = seq.data;
        let grown_cap = seq.cap;
        let data_phi = self.builder.build_phi(self.ptr_ty(), "seq.data.end").map_err(llvm_err)?;
        data_phi.add_incoming(&[(&data, before), (&grown_data, grown)]);
        let cap_phi = self.builder.build_phi(self.usize_ty, "seq.cap.end").map_err(llvm_err)?;
        cap_phi.add_incoming(&[(&cap, before), (&grown_cap, grown)]);
        seq.data = data_phi.as_basic_value().into_pointer_value();
        seq.cap = cap_phi.as_basic_value().into_int_value();
        let _ = alloc2;
        Ok(())
    }

    /// The address of element `index` in a buffer of `elem` (no bounds check).
    pub(super) fn elem_ptr_at(
        &mut self,
        base: PointerValue<'ctx>,
        elem: &Ty,
        index: IntValue<'ctx>,
    ) -> Option<PointerValue<'ctx>> {
        let t = self.llvm_ty(elem)?;
        // SAFETY (LLVM): the caller checked `index < len`.
        unsafe { self.builder.build_in_bounds_gep(t, base, &[index], "elem") }.ok()
    }

    /// The address of element `index` in `seq` (no bounds check).
    fn elem_ptr(&mut self, seq: &Seq<'ctx>, elem: &Ty, index: IntValue<'ctx>) -> Option<PointerValue<'ctx>> {
        self.elem_ptr_at(seq.data, elem, index)
    }

    /// `Some(payload)` of an `Option<inner>`, or `None` when `payload` is `None`.
    pub(super) fn some_value(&mut self, inner: &Ty, payload: Option<BasicValueEnum<'ctx>>) -> StructValue<'ctx> {
        let ot = self.option_ty(inner);
        let yes = self.cx.bool_type().const_int(1, false);
        let agg = self
            .builder
            .build_insert_value(ot.const_zero(), yes, 0, "")
            .expect("inserting a tag into an Option")
            .into_struct_value();
        match (payload, self.llvm_ty(inner)) {
            (Some(p), Some(_)) => {
                self.builder.build_insert_value(agg, p, 1, "").expect("inserting a payload").into_struct_value()
            }
            _ => agg,
        }
    }

    /// The methods of `List<T>` and `Map<K, V>`.
    pub(super) fn collection_method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        match (&args[0].ty, m) {
            (Ty::List(elem, _), Method::Push) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.seq_grow(&mut seq, &elem, span)?;
                let v = self.expr_mode(&args[1], Mode::Move)?;
                if let Some(p) = self.elem_ptr(&seq, &elem, seq.len) {
                    let v = self.need(v, &elem);
                    self.builder.build_store(p, v).map_err(llvm_err)?;
                }
                seq.len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListPop) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.len, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                let none_bb = self.new_block("pop.none");
                let some_bb = self.new_block("pop.some");
                let end = self.new_block("pop.end");
                self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "pop.out")?),
                    None => None,
                };
                self.builder.position_at_end(none_bb);
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let none = self.option_ty(&elem).const_zero();
                    self.builder.build_store(slot, none).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(some_bb);
                let last =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let value = match self.elem_ptr(&seq, &elem, last) {
                    Some(p) if self.llvm_ty(&elem).is_some() => {
                        let t = self.llvm_ty(&elem).expect("checked above");
                        Some(self.builder.build_load(t, p, "pop.value").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                seq.len = last;
                self.seq_store(&seq)?;
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let some = self.some_value(&elem, value);
                    self.builder.build_store(slot, some).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(match (slot, self.llvm_ty(ty)) {
                    (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
                    _ => None,
                })
            }
            (Ty::List(elem, _), Method::ListLast) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.len, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                let none_bb = self.new_block("last.none");
                let some_bb = self.new_block("last.some");
                let end = self.new_block("last.end");
                self.builder.build_conditional_branch(empty, none_bb, some_bb).map_err(llvm_err)?;
                let view = Ty::Ref { mutable: false, inner: Box::new(elem.clone()) };
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "last.out")?),
                    None => None,
                };
                self.builder.position_at_end(none_bb);
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let none = self.option_ty(&view).const_zero();
                    self.builder.build_store(slot, none).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(some_bb);
                let last =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let p = self.elem_ptr(&seq, &elem, last);
                if let (Some(slot), Some(_), Some(p)) = (slot, self.llvm_ty(ty), p) {
                    let some = self.some_value(&view, Some(p.into()));
                    self.builder.build_store(slot, some).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(match (slot, self.llvm_ty(ty)) {
                    (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
                    _ => None,
                })
            }
            (Ty::List(elem, _), Method::ListInsert) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let index = self.expr_mode(&args[1], Mode::Use)?;
                let index = self.need_int(index, &Ty::Int(burdin_tir::IntTy::Usize));
                let limit =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.bounds_check(index, limit, span)?;
                // The value is evaluated before the shift, as an argument of a call is.
                let value = self.expr_mode(&args[2], Mode::Move)?;
                let value_slot = self.spill(value, &elem)?;
                self.seq_grow(&mut seq, &elem, span)?;
                let size = self.elem_size(&elem);
                let tail = self.builder.build_int_sub(seq.len, index, "").map_err(llvm_err)?;
                let bytes =
                    self.builder.build_int_mul(tail, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
                let src = self.elem_ptr(&seq, &elem, index);
                let next =
                    self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let dst = self.elem_ptr(&seq, &elem, next);
                if let (Some(src), Some(dst)) = (src, dst) {
                    self.builder.build_memmove(dst, 1, src, 1, bytes).map_err(llvm_err)?;
                }
                if let (Some(p), Some(value_slot)) = (self.elem_ptr(&seq, &elem, index), value_slot) {
                    let t = self.llvm_ty(&elem).expect("an element with a slot has a type");
                    let v = self.builder.build_load(t, value_slot, "").map_err(llvm_err)?;
                    self.builder.build_store(p, v).map_err(llvm_err)?;
                }
                seq.len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListRemove) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let index = self.expr_mode(&args[1], Mode::Use)?;
                let index = self.need_int(index, &Ty::Int(burdin_tir::IntTy::Usize));
                self.bounds_check(index, seq.len, span)?;
                let value = match self.elem_ptr(&seq, &elem, index) {
                    Some(p) if self.llvm_ty(&elem).is_some() => {
                        let t = self.llvm_ty(&elem).expect("checked above");
                        Some(self.builder.build_load(t, p, "remove.value").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                let size = self.elem_size(&elem);
                let after =
                    self.builder.build_int_add(index, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let tail = self.builder.build_int_sub(seq.len, after, "").map_err(llvm_err)?;
                let bytes =
                    self.builder.build_int_mul(tail, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
                let src = self.elem_ptr(&seq, &elem, after);
                let dst = self.elem_ptr(&seq, &elem, index);
                if let (Some(src), Some(dst)) = (src, dst) {
                    self.builder.build_memmove(dst, 1, src, 1, bytes).map_err(llvm_err)?;
                }
                seq.len =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                Ok(value)
            }
            (Ty::List(elem, _), Method::ListContains) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let key_ptr = self.place_ptr(&args[1])?;
                self.sequence_contains(seq.data, seq.len, &elem, key_ptr)
            }
            (Ty::List(elem, _), Method::ListSort) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.sort_sequence(&seq, &elem)
            }
            (Ty::List(elem, _), Method::Clear) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.drop_sequence_elements(&seq, &elem)?;
                seq.len = self.usize_zero();
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(_, _), Method::ListCapacity) => {
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                Ok(Some(seq.cap.into()))
            }
            (Ty::List(_, _), Method::ListIsEmpty) => {
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.len, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                Ok(Some(empty.into()))
            }
            (Ty::List(elem, _), Method::ListGet | Method::ListGetMut) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let index = self.expr_mode(&args[1], Mode::Use)?;
                let index = self.need_int(index, &Ty::Int(burdin_tir::IntTy::Usize));
                let view = Ty::Ref { mutable: m == Method::ListGetMut, inner: Box::new(elem.clone()) };
                self.sequence_element_view(&seq, &elem, &view, Some(index), ty)
            }
            (Ty::List(elem, _), Method::ListFirst | Method::ListFirstMut) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let view = Ty::Ref { mutable: m == Method::ListFirstMut, inner: Box::new(elem.clone()) };
                self.sequence_element_view(&seq, &elem, &view, Some(self.usize_zero()), ty)
            }
            (Ty::List(elem, _), Method::ListLastMut) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let view = Ty::Ref { mutable: true, inner: Box::new(elem.clone()) };
                self.sequence_element_view(&seq, &elem, &view, None, ty)
            }
            (Ty::List(elem, _), Method::ListReserve | Method::ListReserveExact) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let additional = self.expr_mode(&args[1], Mode::Use)?;
                let additional = self.need_int(additional, &Ty::Int(burdin_tir::IntTy::Usize));
                let want = self.builder.build_int_add(seq.len, additional, "reserve.want").map_err(llvm_err)?;
                let overflow =
                    self.builder.build_int_compare(IntPredicate::ULT, want, seq.len, "").map_err(llvm_err)?;
                let fits = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                self.check(fits, "reserve: capacity overflow", span)?;
                if m == Method::ListReserveExact {
                    self.seq_reserve_exact_to(&mut seq, &elem, want, span)?;
                } else {
                    self.seq_reserve_to(&mut seq, &elem, want, span)?;
                }
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListShrinkToFit) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let len = seq.len;
                self.seq_realloc_to(&mut seq, &elem, len, span)?;
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListSwap) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let a = self.expr_mode(&args[1], Mode::Use)?;
                let a = self.need_int(a, &Ty::Int(burdin_tir::IntTy::Usize));
                let b = self.expr_mode(&args[2], Mode::Use)?;
                let b = self.need_int(b, &Ty::Int(burdin_tir::IntTy::Usize));
                let a_ok = self.builder.build_int_compare(IntPredicate::ULT, a, seq.len, "").map_err(llvm_err)?;
                let b_ok = self.builder.build_int_compare(IntPredicate::ULT, b, seq.len, "").map_err(llvm_err)?;
                let ok = self.builder.build_and(a_ok, b_ok, "").map_err(llvm_err)?;
                self.check(ok, "swap: index out of bounds", span)?;
                let Some(elem_t) = self.llvm_ty(&elem) else { return Ok(None) };
                let (Some(pa), Some(pb)) = (self.elem_ptr(&seq, &elem, a), self.elem_ptr(&seq, &elem, b)) else {
                    return Ok(None);
                };
                let tmp = self.alloca(elem_t, "swap.value")?;
                let bytes = self.usize_ty.const_int(self.elem_size(&elem), false);
                self.builder.build_memcpy(tmp, 1, pa, 1, bytes).map_err(llvm_err)?;
                self.builder.build_memcpy(pa, 1, pb, 1, bytes).map_err(llvm_err)?;
                self.builder.build_memcpy(pb, 1, tmp, 1, bytes).map_err(llvm_err)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListSwapRemove) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let index = self.expr_mode(&args[1], Mode::Use)?;
                let index = self.need_int(index, &Ty::Int(burdin_tir::IntTy::Usize));
                let ok = self.builder.build_int_compare(IntPredicate::ULT, index, seq.len, "").map_err(llvm_err)?;
                self.check(ok, "swap_remove: index out of bounds", span)?;
                let value = match self.elem_ptr(&seq, &elem, index) {
                    Some(p) if self.llvm_ty(&elem).is_some() => {
                        let t = self.llvm_ty(&elem).expect("checked above");
                        Some(self.builder.build_load(t, p, "swap_remove.value").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                // The last element fills the hole; the slot past the new length is dead.
                let last =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let moved = self.builder.build_int_compare(IntPredicate::NE, index, last, "").map_err(llvm_err)?;
                let (Some(from), Some(to)) = (self.elem_ptr(&seq, &elem, last), self.elem_ptr(&seq, &elem, index))
                else {
                    return Ok(None);
                };
                let bytes = self.usize_ty.const_int(self.elem_size(&elem), false);
                self.if_then(moved, "swap_remove.move", move |cg| {
                    cg.builder.build_memcpy(to, 1, from, 1, bytes).map_err(llvm_err)?;
                    Ok(())
                })?;
                seq.len = last;
                self.seq_store(&seq)?;
                Ok(value)
            }
            (Ty::List(elem, _), Method::ListTruncate) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let len = self.expr_mode(&args[1], Mode::Use)?;
                let len = self.need_int(len, &Ty::Int(burdin_tir::IntTy::Usize));
                let shorter = self.builder.build_int_compare(IntPredicate::ULT, len, seq.len, "").map_err(llvm_err)?;
                let old_len = seq.len;
                self.if_then(shorter, "truncate", |cg| cg.drop_sequence_range(&seq, &elem, len, old_len))?;
                seq.len = self.builder.build_select(shorter, len, old_len, "").map_err(llvm_err)?.into_int_value();
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListSplitOff) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let index = self.expr_mode(&args[1], Mode::Use)?;
                let index = self.need_int(index, &Ty::Int(burdin_tir::IntTy::Usize));
                let ok = self.builder.build_int_compare(IntPredicate::ULE, index, seq.len, "").map_err(llvm_err)?;
                self.check(ok, "split_off: index out of bounds", span)?;
                let count = self.builder.build_int_sub(seq.len, index, "split.count").map_err(llvm_err)?;
                let empty = self.list_with_capacity(count, &elem, &burdin_tir::Alloc::Global, None, span)?;
                let tail = self
                    .builder
                    .build_insert_value(empty, count, 1, "split.len")
                    .map_err(llvm_err)?
                    .into_struct_value();
                let data = self.builder.build_extract_value(tail, 0, "split.data").map_err(llvm_err)?;
                let Some(elem_t) = self.llvm_ty(&elem) else {
                    seq.len = index;
                    self.seq_store(&seq)?;
                    return Ok(Some(tail.into()));
                };
                let source = seq.data;
                let bytes = self
                    .builder
                    .build_int_mul(count, self.usize_ty.const_int(self.elem_size(&elem), false), "")
                    .map_err(llvm_err)?;
                let nonzero =
                    self.builder.build_int_compare(IntPredicate::NE, count, self.usize_zero(), "").map_err(llvm_err)?;
                self.if_then(nonzero, "split.copy", move |cg| {
                    // SAFETY (LLVM): `index <= len` was checked, so `count` elements start at `index`.
                    let src =
                        unsafe { cg.builder.build_in_bounds_gep(elem_t, source, &[index], "") }.map_err(llvm_err)?;
                    cg.builder.build_memcpy(data.into_pointer_value(), 1, src, 1, bytes).map_err(llvm_err)?;
                    Ok(())
                })?;
                seq.len = index;
                self.seq_store(&seq)?;
                Ok(Some(tail.into()))
            }
            (Ty::List(elem, _), Method::ListAppend) => {
                let elem = (**elem).clone();
                let other_ty = args[1].ty.clone();
                let burdin_tir::Ty::List(_, other_alloc) = &other_ty else {
                    return Err("`append` takes another list".into());
                };
                let other_alloc = other_alloc.clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let value = self.expr_mode(&args[1], Mode::Move)?;
                let Some(other_slot) = self.spill(value, &other_ty)? else { return Ok(None) };
                let other = self.seq_load_at(other_slot, &other_alloc)?;
                let start = seq.len;
                let count = other.len;
                let want = self.builder.build_int_add(start, count, "append.len").map_err(llvm_err)?;
                self.seq_reserve_to(&mut seq, &elem, want, span)?;
                if self.llvm_ty(&elem).is_some() {
                    let target = self.elem_ptr(&seq, &elem, start);
                    let source: PointerValue<'ctx> = other.data;
                    let bytes = self
                        .builder
                        .build_int_mul(count, self.usize_ty.const_int(self.elem_size(&elem), false), "")
                        .map_err(llvm_err)?;
                    let nonzero = self
                        .builder
                        .build_int_compare(IntPredicate::NE, count, self.usize_zero(), "")
                        .map_err(llvm_err)?;
                    self.if_then(nonzero, "append.copy", move |cg| {
                        let Some(target) = target else { return Ok(()) };
                        cg.builder.build_memcpy(target, 1, source, 1, bytes).map_err(llvm_err)?;
                        Ok(())
                    })?;
                }
                seq.len = want;
                self.seq_store(&seq)?;
                // `other` was moved into the call: its elements now belong to `self`, so its buffer and its
                // stored allocator value are released here.
                self.free_sequence(&other, &elem)?;
                Ok(None)
            }
            (Ty::List(elem, _), Method::ListDrain) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let range = self.expr_mode(&args[1], Mode::Use)?;
                let range_ty = args[1].ty.clone();
                let range = self.need(range, &range_ty).into_struct_value();
                let lo = self.builder.build_extract_value(range, 0, "drain.lo").map_err(llvm_err)?.into_int_value();
                let hi = self.builder.build_extract_value(range, 1, "drain.hi").map_err(llvm_err)?.into_int_value();
                let inclusive = matches!(range_ty, Ty::Range(_, true));
                let len = seq.len;
                let end = if inclusive {
                    // An inclusive end must be strictly inside the list, so `hi + 1` cannot wrap.
                    self.check(
                        self.builder.build_int_compare(IntPredicate::ULT, hi, len, "").map_err(llvm_err)?,
                        "drain: range out of bounds",
                        span,
                    )?;
                    self.builder.build_int_add(hi, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?
                } else {
                    hi
                };
                self.check(
                    self.builder.build_int_compare(IntPredicate::ULE, end, len, "").map_err(llvm_err)?,
                    "drain: range out of bounds",
                    span,
                )?;
                self.check(
                    self.builder.build_int_compare(IntPredicate::ULE, lo, end, "").map_err(llvm_err)?,
                    "drain: range out of bounds",
                    span,
                )?;
                let count = self.builder.build_int_sub(end, lo, "drain.count").map_err(llvm_err)?;
                let removed = self.list_with_capacity(count, &elem, &burdin_tir::Alloc::Global, None, span)?;
                let data = self.builder.build_extract_value(removed, 0, "drain.data").map_err(llvm_err)?;
                let source = seq.data;
                let size = self.usize_ty.const_int(self.elem_size(&elem), false);
                let bytes = self.builder.build_int_mul(count, size, "").map_err(llvm_err)?;
                let nonzero =
                    self.builder.build_int_compare(IntPredicate::NE, count, self.usize_zero(), "").map_err(llvm_err)?;
                if let Some(elem_t) = self.llvm_ty(&elem) {
                    self.if_then(nonzero, "drain.copy", move |cg| {
                        // SAFETY (LLVM): `end <= len`, so the `count` elements from `lo` are in the buffer.
                        let src =
                            unsafe { cg.builder.build_in_bounds_gep(elem_t, source, &[lo], "") }.map_err(llvm_err)?;
                        cg.builder.build_memcpy(data.into_pointer_value(), 1, src, 1, bytes).map_err(llvm_err)?;
                        Ok(())
                    })?;
                }
                // The tail moves down over the hole.
                let tail_bytes = self
                    .builder
                    .build_int_mul(
                        self.builder.build_int_sub(len, end, "").map_err(llvm_err)?,
                        self.usize_ty.const_int(self.elem_size(&elem), false),
                        "",
                    )
                    .map_err(llvm_err)?;
                let has_tail = self.builder.build_int_compare(IntPredicate::ULT, end, len, "").map_err(llvm_err)?;
                if let Some(elem_t) = self.llvm_ty(&elem) {
                    let source = seq.data;
                    self.if_then(has_tail, "drain.shift", move |cg| {
                        // SAFETY (LLVM): `lo <= end <= len`; the source region starts at `end`.
                        let from =
                            unsafe { cg.builder.build_in_bounds_gep(elem_t, source, &[end], "") }.map_err(llvm_err)?;
                        let to =
                            unsafe { cg.builder.build_in_bounds_gep(elem_t, source, &[lo], "") }.map_err(llvm_err)?;
                        cg.builder.build_memmove(to, 1, from, 1, tail_bytes).map_err(llvm_err)?;
                        Ok(())
                    })?;
                }
                seq.len = self.builder.build_int_sub(len, count, "drain.len").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                // The removed elements travel in a new buffer with the default allocator, owned by the iterator.
                let zero = self.usize_zero();
                let iter = self.seq_value(
                    &burdin_tir::Alloc::Global,
                    &[(0, data), (1, count.into()), (2, count.into()), (3, zero.into())],
                )?;
                Ok(Some(iter.into()))
            }
            (Ty::List(elem, _), Method::ListExtendFromSlice) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let (source, count) = self.view_parts(&args[1])?;
                let source = source.into_pointer_value();
                let count = count.into_int_value();
                let start = seq.len;
                let want = self.builder.build_int_add(start, count, "extend.len").map_err(llvm_err)?;
                self.seq_reserve_to(&mut seq, &elem, want, span)?;
                if let Some(elem_t) = self.llvm_ty(&elem) {
                    let Some(target) = self.elem_ptr(&seq, &elem, start) else { return Ok(None) };
                    let elem2 = elem.clone();
                    let index = self.alloca(self.usize_ty.into(), "extend.i")?;
                    self.builder.build_store(index, self.usize_zero()).map_err(llvm_err)?;
                    let header = self.new_block("extend.cond");
                    let body = self.new_block("extend.body");
                    let exit = self.new_block("extend.end");
                    self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                    self.builder.position_at_end(header);
                    let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                    let go = self.builder.build_int_compare(IntPredicate::ULT, i, count, "").map_err(llvm_err)?;
                    self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
                    self.builder.position_at_end(body);
                    // SAFETY (LLVM): `i < count`, and the target has room for `count` elements.
                    let dst =
                        unsafe { self.builder.build_in_bounds_gep(elem_t, target, &[i], "") }.map_err(llvm_err)?;
                    let src =
                        unsafe { self.builder.build_in_bounds_gep(elem_t, source, &[i], "") }.map_err(llvm_err)?;
                    self.clone_value(dst, src, &elem2)?;
                    let next =
                        self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                    self.builder.build_store(index, next).map_err(llvm_err)?;
                    self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                    self.builder.position_at_end(exit);
                }
                seq.len = want;
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::Map(key, value, _), Method::MapInsert | Method::MapGetOrInsert | Method::MapGetOr) => {
                // `get_or_insert` and `get_or` keep an existing value (dropping the incoming key and value) and
                // yield a view of the value either way.
                let keep_existing = m != Method::MapInsert;
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k = self.expr_mode(&args[1], Mode::Move)?;
                let v = self.expr_mode(&args[2], Mode::Move)?;
                let k_slot = self.spill(k, &key_ty)?;
                let v_slot = self.spill(v, &value_ty)?;
                // Grow before probing: the empty map has no slots, and the table keeps a 3/4 load factor.
                let next_len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let needed =
                    self.builder.build_int_mul(next_len, self.usize_ty.const_int(4, false), "").map_err(llvm_err)?;
                let room =
                    self.builder.build_int_mul(seq.cap, self.usize_ty.const_int(3, false), "").map_err(llvm_err)?;
                let over = self.builder.build_int_compare(IntPredicate::UGT, needed, room, "").map_err(llvm_err)?;
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                let grow = self.builder.build_or(over, no_table, "").map_err(llvm_err)?;
                let grow_bb = self.new_block("map.grow");
                let ready_bb = self.new_block("map.ready");
                let before = self.builder.get_insert_block().expect("positioned");
                let (old_data, old_cap) = (seq.data, seq.cap);
                self.builder.build_conditional_branch(grow, grow_bb, ready_bb).map_err(llvm_err)?;
                self.builder.position_at_end(grow_bb);
                self.map_grow(&mut seq, &key_ty, &value_ty, span)?;
                let grown = self.builder.get_insert_block().expect("positioned");
                let (new_data, new_cap) = (seq.data, seq.cap);
                self.builder.build_unconditional_branch(ready_bb).map_err(llvm_err)?;
                self.builder.position_at_end(ready_bb);
                let data_phi = self.builder.build_phi(self.ptr_ty(), "map.data").map_err(llvm_err)?;
                data_phi.add_incoming(&[(&old_data, before), (&new_data, grown)]);
                let cap_phi = self.builder.build_phi(self.usize_ty, "map.cap").map_err(llvm_err)?;
                cap_phi.add_incoming(&[(&old_cap, before), (&new_cap, grown)]);
                seq.data = data_phi.as_basic_value().into_pointer_value();
                seq.cap = cap_phi.as_basic_value().into_int_value();

                let hash = self.map_key_hash(k_slot, &key_ty)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, &key_ty, hash, k_slot)?;
                let found_bb = self.new_block("map.found");
                let insert_bb = self.new_block("map.insert");
                let end = self.new_block("map.end");
                self.builder.build_conditional_branch(found, found_bb, insert_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                let old = self.map_field(entry_t, p, 1, "map.value")?;
                if keep_existing {
                    if let Some(v_slot) = v_slot {
                        self.drop_value(v_slot, &value_ty)?;
                    }
                } else {
                    self.drop_value(old, &value_ty)?;
                    if let Some(v_slot) = v_slot {
                        let vt = self.llvm_ty(&value_ty).unwrap_or_else(|| self.cx.bool_type().into());
                        let nv = self.builder.build_load(vt, v_slot, "").map_err(llvm_err)?;
                        self.builder.build_store(old, nv).map_err(llvm_err)?;
                    }
                }
                // The map already owns an equal key, so the incoming one is dropped.
                if let Some(k_slot) = k_slot {
                    self.drop_value(k_slot, &key_ty)?;
                }
                // Growing happens before the probe, so even a replacement must publish the new buffer.
                self.seq_store(&seq)?;
                let found_end = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(insert_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                if let Some(k_slot) = k_slot {
                    let kf = self.map_field(entry_t, p, 0, "map.key")?;
                    let kt = self.llvm_ty(&key_ty).unwrap_or_else(|| self.cx.bool_type().into());
                    let kv = self.builder.build_load(kt, k_slot, "").map_err(llvm_err)?;
                    self.builder.build_store(kf, kv).map_err(llvm_err)?;
                }
                let vf = self.map_field(entry_t, p, 1, "map.value")?;
                if let Some(v_slot) = v_slot {
                    let vt = self.llvm_ty(&value_ty).unwrap_or_else(|| self.cx.bool_type().into());
                    let vv = self.builder.build_load(vt, v_slot, "").map_err(llvm_err)?;
                    self.builder.build_store(vf, vv).map_err(llvm_err)?;
                }
                let hf = self.map_field(entry_t, p, 2, "map.hash")?;
                self.builder.build_store(hf, hash).map_err(llvm_err)?;
                seq.len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                let insert_end = self.builder.get_insert_block().expect("positioned");
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                if !keep_existing {
                    return Ok(None);
                }
                let at_value = self.builder.build_phi(self.ptr_ty(), "map.value").map_err(llvm_err)?;
                at_value.add_incoming(&[(&old, found_end), (&vf, insert_end)]);
                Ok(Some(at_value.as_basic_value()))
            }
            (Ty::Map(key, value, _), Method::MapGet | Method::MapGetMut) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let view = Ty::Ref { mutable: m == Method::MapGetMut, inner: Box::new(value_ty.clone()) };
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "get.out")?),
                    None => None,
                };
                let found_bb = self.new_block("map.get.found");
                let none_bb = self.new_block("map.get.none");
                let probe_bb = self.new_block("map.get.probe");
                let end = self.new_block("map.get.end");
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(no_table, none_bb, probe_bb).map_err(llvm_err)?;
                self.builder.position_at_end(probe_bb);
                let key_arg = args[1].ty.clone();
                let hash = self.map_key_hash(k_slot, &key_arg)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, &key_arg, hash, k_slot)?;
                self.builder.build_conditional_branch(found, found_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                let value_field = self.map_field(entry_t, p, 1, "map.value")?;
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let some = self.some_value(&view, Some(value_field.into()));
                    self.builder.build_store(slot, some).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(none_bb);
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    self.builder.build_store(slot, self.option_ty(&view).const_zero()).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(match (slot, self.llvm_ty(ty)) {
                    (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
                    _ => None,
                })
            }
            (Ty::Map(key, value, _), Method::MapRemove) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "remove.out")?),
                    None => None,
                };
                let found_bb = self.new_block("map.remove.found");
                let none_bb = self.new_block("map.remove.none");
                let probe_bb = self.new_block("map.remove.probe");
                let end = self.new_block("map.remove.end");
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(no_table, none_bb, probe_bb).map_err(llvm_err)?;
                self.builder.position_at_end(probe_bb);
                let key_arg = args[1].ty.clone();
                let hash = self.map_key_hash(k_slot, &key_arg)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, &key_arg, hash, k_slot)?;
                self.builder.build_conditional_branch(found, found_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                let value_field = self.map_field(entry_t, p, 1, "map.value")?;
                let mut value = None;
                if let Some(vt) = self.llvm_ty(&value_ty) {
                    value = Some(self.builder.build_load(vt, value_field, "").map_err(llvm_err)?);
                }
                let key_field = self.map_field(entry_t, p, 0, "map.key")?;
                self.drop_value(key_field, &key_ty)?;
                self.map_shift_delete(&seq, &entry, at)?;
                seq.len =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let some = self.some_value(&value_ty, value);
                    self.builder.build_store(slot, some).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(none_bb);
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    self.builder.build_store(slot, self.option_ty(&value_ty).const_zero()).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(match (slot, self.llvm_ty(ty)) {
                    (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
                    _ => None,
                })
            }
            (Ty::Map(key, value, _), Method::MapContainsKey) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let result = self.alloca(self.cx.bool_type().into(), "map.contains")?;
                self.builder.build_store(result, self.cx.bool_type().const_zero()).map_err(llvm_err)?;
                let probe_bb = self.new_block("map.contains.probe");
                let end = self.new_block("map.contains.end");
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(no_table, end, probe_bb).map_err(llvm_err)?;
                self.builder.position_at_end(probe_bb);
                let key_arg = args[1].ty.clone();
                let hash = self.map_key_hash(k_slot, &key_arg)?;
                let (found, _) = self.map_probe(&seq, &entry, &key_ty, &key_arg, hash, k_slot)?;
                self.builder.build_store(result, found).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(Some(self.builder.build_load(self.cx.bool_type(), result, "").map_err(llvm_err)?))
            }
            // `remove_entry(key)` moves the stored key and value out and shifts the probe cluster back.
            (Ty::Map(key, value, _), Method::MapRemoveEntry) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "remove_entry.out")?),
                    None => None,
                };
                let found_bb = self.new_block("remove_entry.found");
                let none_bb = self.new_block("remove_entry.none");
                let probe_bb = self.new_block("remove_entry.probe");
                let end = self.new_block("remove_entry.end");
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(no_table, none_bb, probe_bb).map_err(llvm_err)?;
                self.builder.position_at_end(probe_bb);
                let key_arg = args[1].ty.clone();
                let hash = self.map_key_hash(k_slot, &key_arg)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, &key_arg, hash, k_slot)?;
                self.builder.build_conditional_branch(found, found_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                let pair_ty = Ty::Tuple(vec![key_ty.clone(), value_ty.clone()]);
                let pair = match self.llvm_ty(&pair_ty) {
                    Some(pair_t) => {
                        let pair_t = pair_t.into_struct_type();
                        let mut pair = pair_t.const_zero();
                        let kf = self.map_field(entry_t, p, 0, "map.key")?;
                        if let Some(kt) = self.llvm_ty(&key_ty) {
                            let kv = self.builder.build_load(kt, kf, "").map_err(llvm_err)?;
                            pair =
                                self.builder.build_insert_value(pair, kv, 0, "").map_err(llvm_err)?.into_struct_value();
                        }
                        let vf = self.map_field(entry_t, p, 1, "map.value")?;
                        if let Some(vt) = self.llvm_ty(&value_ty) {
                            let vv = self.builder.build_load(vt, vf, "").map_err(llvm_err)?;
                            pair =
                                self.builder.build_insert_value(pair, vv, 1, "").map_err(llvm_err)?.into_struct_value();
                        }
                        Some(pair)
                    }
                    None => None,
                };
                self.map_shift_delete(&seq, &entry, at)?;
                seq.len =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    let some = self.some_value(&pair_ty, pair.map(BasicValueEnum::from));
                    self.builder.build_store(slot, some).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(none_bb);
                if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
                    self.builder.build_store(slot, self.option_ty(&pair_ty).const_zero()).map_err(llvm_err)?;
                }
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(match (slot, self.llvm_ty(ty)) {
                    (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
                    _ => None,
                })
            }
            // `retain(f)`: walk the slots and drop every entry `f` rejects. A removal shifts the rest of the
            // probe cluster back, so the same slot is examined again.
            (Ty::Map(key, value, _), Method::MapRetain) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let closure_ty = args[1].ty.clone();
                let closure = self.expr_mode(&args[1], Mode::Move)?;
                let Some(closure_slot) = self.spill(closure, &closure_ty)? else { return Ok(None) };
                let index = self.alloca(self.usize_ty.into(), "retain.i")?;
                self.builder.build_store(index, self.usize_zero()).map_err(llvm_err)?;
                // The live count changes on every removal, so it lives in a slot that each removal updates.
                let len_slot = self.alloca(self.usize_ty.into(), "retain.len")?;
                self.builder.build_store(len_slot, seq.len).map_err(llvm_err)?;
                let header = self.new_block("retain.cond");
                let body = self.new_block("retain.body");
                let live = self.new_block("retain.live");
                let keep = self.new_block("retain.keep");
                let drop = self.new_block("retain.drop");
                let next = self.new_block("retain.next");
                let end = self.new_block("retain.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let more = self.builder.build_int_compare(IntPredicate::ULT, i, seq.cap, "").map_err(llvm_err)?;
                self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
                self.builder.position_at_end(body);
                let slot = self.map_slot(&seq, &entry, i)?;
                let hash = self.map_slot_hash(entry_t, slot)?;
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, hash, self.cx.i64_type().const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(empty, next, live).map_err(llvm_err)?;
                self.builder.position_at_end(live);
                let key_field = self.map_field(entry_t, slot, 0, "map.key")?;
                let value_field = self.map_field(entry_t, slot, 1, "map.value")?;
                let closure_value =
                    self.builder.build_load(self.closure_ty, closure_slot, "").map_err(llvm_err)?.into_struct_value();
                let accepted = self.call_closure_with(
                    closure_value,
                    &closure_ty,
                    FnKind::FnMut,
                    &[key_field.into(), value_field.into()],
                )?;
                let accepted = accepted.map(|v| v.into_int_value()).unwrap_or_else(|| self.cx.bool_type().const_zero());
                self.builder.build_conditional_branch(accepted, keep, drop).map_err(llvm_err)?;
                self.builder.position_at_end(drop);
                self.drop_value(key_field, &key_ty)?;
                self.drop_value(value_field, &value_ty)?;
                self.map_shift_delete(&seq, &entry, i)?;
                let current = self.builder.build_load(self.usize_ty, len_slot, "").map_err(llvm_err)?.into_int_value();
                let remaining =
                    self.builder.build_int_sub(current, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(len_slot, remaining).map_err(llvm_err)?;
                // The shifted-in entry (if any) is examined at the same index.
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(keep);
                self.builder.build_unconditional_branch(next).map_err(llvm_err)?;
                self.builder.position_at_end(next);
                let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.builder.build_store(index, next_i).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                // The method owns the closure; `FnMut` calls keep its capture record, so dispose of it here.
                self.drop_value(closure_slot, &closure_ty)?;
                seq.len = self.builder.build_load(self.usize_ty, len_slot, "").map_err(llvm_err)?.into_int_value();
                self.seq_store(&seq)?;
                Ok(None)
            }
            // `reserve(additional)`: grow the slot table so it holds the entries plus `additional` more without
            // passing the 3/4 load factor.
            (Ty::Map(key, value, _), Method::MapReserve) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let additional = self.expr_mode(&args[1], Mode::Use)?;
                let additional = self.need_int(additional, &Ty::Int(tir::IntTy::Usize));
                let max = self.usize_ty.const_int(u64::MAX, false);
                let room = self.builder.build_int_sub(max, seq.len, "").map_err(llvm_err)?;
                let fits = self.builder.build_int_compare(IntPredicate::ULE, additional, room, "").map_err(llvm_err)?;
                self.check(fits, "Map reserve: capacity overflow", span)?;
                let needed = self.builder.build_int_add(seq.len, additional, "").map_err(llvm_err)?;
                let cap = self.map_capacity_for(needed, span)?;
                let over = self.builder.build_int_compare(IntPredicate::UGT, cap, seq.cap, "").map_err(llvm_err)?;
                let grow = self.new_block("reserve.grow");
                let done = self.new_block("reserve.end");
                let before = self.builder.get_insert_block().expect("positioned");
                self.builder.build_conditional_branch(over, grow, done).map_err(llvm_err)?;
                self.builder.position_at_end(grow);
                self.map_rehash_to(&mut seq, &key_ty, &value_ty, cap, span)?;
                self.seq_store(&seq)?;
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(done);
                let _ = before;
                Ok(None)
            }
            // `shrink_to_fit()`: rehash into the smallest table that holds the entries, and free the buffer of an
            // empty map.
            (Ty::Map(key, value, _), Method::MapShrinkToFit) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let empty = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.len, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                let clear = self.new_block("shrink.clear");
                let rehash = self.new_block("shrink.rehash");
                let done = self.new_block("shrink.end");
                let before = self.builder.get_insert_block().expect("positioned");
                self.builder.build_conditional_branch(empty, clear, rehash).map_err(llvm_err)?;
                self.builder.position_at_end(clear);
                let old_bytes = self.builder.build_int_mul(
                    seq.cap,
                    self.usize_ty.const_int(self.map_entry_size(&key_ty, &value_ty), false),
                    "",
                );
                let old_bytes = old_bytes.map_err(llvm_err)?;
                let align = self.value_align(&Self::map_entry_ty(&key_ty, &value_ty));
                let alloc = seq.alloc.clone();
                let alloc_addr = seq.alloc_addr;
                self.dealloc_in(&alloc, alloc_addr, seq.data, old_bytes, align)?;
                seq.data = self.ptr_ty().const_null();
                seq.cap = self.usize_zero();
                self.seq_store(&seq)?;
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(rehash);
                let cap = self.map_capacity_for(seq.len, span)?;
                let smaller = self.builder.build_int_compare(IntPredicate::ULT, cap, seq.cap, "").map_err(llvm_err)?;
                let shrink = self.new_block("shrink.smaller");
                let keep_bb = self.new_block("shrink.keep");
                self.builder.build_conditional_branch(smaller, shrink, keep_bb).map_err(llvm_err)?;
                self.builder.position_at_end(shrink);
                self.map_rehash_to(&mut seq, &key_ty, &value_ty, cap, span)?;
                self.seq_store(&seq)?;
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(keep_bb);
                self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                self.builder.position_at_end(done);
                let _ = before;
                Ok(None)
            }
            (Ty::Map(key, value, _), Method::Clear) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.map_drop_entries(&seq, &key_ty, &value_ty)?;
                self.builder.build_call(self.rt.free, &[seq.data.into()], "").map_err(llvm_err)?;
                seq.data = self.ptr_ty().const_null();
                seq.len = self.usize_zero();
                seq.cap = self.usize_zero();
                self.seq_store(&seq)?;
                Ok(None)
            }
            // `map.entry(key)` probes once and keeps the slot; the entry operations insert into it. The probe is
            // recorded here so a fill does not look the key up a second time.
            (Ty::Map(key, value, _), Method::MapEntry) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let map_addr = seq.addr;
                let k = self.expr_mode(&args[1], Mode::Move)?;
                let k_slot = self.spill(k, &key_ty)?;
                let hash = self.map_key_hash(k_slot, &key_ty)?;
                let entry_t = self.map_entry_value_ty(&key_ty);
                let out = self.alloca(entry_t.into(), "entry")?;
                let f = self.map_field(entry_t, out, 0, "entry.map")?;
                self.builder.build_store(f, map_addr).map_err(llvm_err)?;
                if let Some(k_slot) = k_slot {
                    let kt = self.llvm_ty(&key_ty).unwrap_or_else(|| self.cx.bool_type().into());
                    let kv = self.builder.build_load(kt, k_slot, "").map_err(llvm_err)?;
                    let f = self.map_field(entry_t, out, 1, "entry.key")?;
                    self.builder.build_store(f, kv).map_err(llvm_err)?;
                }
                let f = self.map_field(entry_t, out, 2, "entry.hash")?;
                self.builder.build_store(f, hash).map_err(llvm_err)?;
                let f = self.map_field(entry_t, out, 3, "entry.slot")?;
                self.builder.build_store(f, self.usize_zero()).map_err(llvm_err)?;
                let f = self.map_field(entry_t, out, 4, "entry.found")?;
                self.builder.build_store(f, self.cx.bool_type().const_zero()).map_err(llvm_err)?;
                let probe_bb = self.new_block("entry.probe");
                let end = self.new_block("entry.end");
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_conditional_branch(no_table, end, probe_bb).map_err(llvm_err)?;
                self.builder.position_at_end(probe_bb);
                let slot_ty = Self::map_entry_ty(&key_ty, &value_ty);
                let (found, at) = self.map_probe(&seq, &slot_ty, &key_ty, &key_ty, hash, k_slot)?;
                let f = self.map_field(entry_t, out, 3, "entry.slot")?;
                self.builder.build_store(f, at).map_err(llvm_err)?;
                let f = self.map_field(entry_t, out, 4, "entry.found")?;
                self.builder.build_store(f, found).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(Some(self.builder.build_load(entry_t, out, "entry.value").map_err(llvm_err)?))
            }
            (Ty::MapEntry(key, value, alloc), Method::MapEntryOrInsert | Method::MapEntryOrInsertWith) => {
                let with_make = m == Method::MapEntryOrInsertWith;
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let entry_ty = args[0].ty.clone();
                let entry_t = self.llvm_ty(&entry_ty).expect("an entry has a type").into_struct_type();
                let entry_v = self.expr_mode(&args[0], Mode::Move)?;
                let Some(entry_slot) = self.spill(entry_v, &entry_ty)? else { return Ok(None) };
                // Arguments are evaluated before the operation, whatever branch it takes.
                let arg_v = self.expr_mode(&args[1], Mode::Move)?;
                let arg_slot = self.spill(arg_v, &args[1].ty)?;
                let f = self.map_field(entry_t, entry_slot, 0, "entry.map")?;
                let map_addr = self.builder.build_load(self.ptr_ty(), f, "").map_err(llvm_err)?.into_pointer_value();
                let f = self.map_field(entry_t, entry_slot, 2, "entry.hash")?;
                let hash = self.builder.build_load(self.cx.i64_type(), f, "").map_err(llvm_err)?.into_int_value();
                let f = self.map_field(entry_t, entry_slot, 3, "entry.slot")?;
                let at = self.builder.build_load(self.usize_ty, f, "").map_err(llvm_err)?.into_int_value();
                let f = self.map_field(entry_t, entry_slot, 4, "entry.found")?;
                let found = self.builder.build_load(self.cx.bool_type(), f, "").map_err(llvm_err)?.into_int_value();
                let key_ptr = match self.llvm_ty(&key_ty) {
                    Some(_) => Some(self.map_field(entry_t, entry_slot, 1, "entry.key")?),
                    None => None,
                };
                let mut seq = self.seq_load_at(map_addr, alloc)?;
                let slot_ty = Self::map_entry_ty(&key_ty, &value_ty);
                let slot_t = self.llvm_ty(&slot_ty).expect("a map entry has a type").into_struct_type();
                let result = self.alloca(self.ptr_ty().into(), "entry.result")?;
                let found_bb = self.new_block("entry.found");
                let vacant_bb = self.new_block("entry.vacant");
                let end = self.new_block("entry.done");
                self.builder.build_conditional_branch(found, found_bb, vacant_bb).map_err(llvm_err)?;

                // The key is already there: keep its value, drop the incoming one, and drop the entry's key.
                self.builder.position_at_end(found_bb);
                if let Some(arg_slot) = arg_slot {
                    self.drop_value(arg_slot, &args[1].ty)?;
                }
                if let Some(key_ptr) = key_ptr {
                    self.drop_value(key_ptr, &key_ty)?;
                }
                let old = self.map_slot(&seq, &slot_ty, at)?;
                let value_ptr = self.map_field(slot_t, old, 1, "map.value")?;
                self.builder.build_store(result, value_ptr).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

                // The key is absent: make room first (growing moves every slot, so the recorded one is redone),
                // then store the key, the value, and the hash.
                self.builder.position_at_end(vacant_bb);
                let next_len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let needed =
                    self.builder.build_int_mul(next_len, self.usize_ty.const_int(4, false), "").map_err(llvm_err)?;
                let room =
                    self.builder.build_int_mul(seq.cap, self.usize_ty.const_int(3, false), "").map_err(llvm_err)?;
                let over = self.builder.build_int_compare(IntPredicate::UGT, needed, room, "").map_err(llvm_err)?;
                let no_table = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, seq.cap, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                let grow = self.builder.build_or(over, no_table, "").map_err(llvm_err)?;
                let grow_bb = self.new_block("entry.grow");
                let slot_bb = self.new_block("entry.slot");
                let fill_bb = self.new_block("entry.fill");
                let (old_data, old_cap) = (seq.data, seq.cap);
                self.builder.build_conditional_branch(grow, grow_bb, slot_bb).map_err(llvm_err)?;
                self.builder.position_at_end(grow_bb);
                self.map_grow(&mut seq, &key_ty, &value_ty, span)?;
                let (_, grown_at) = self.map_probe(&seq, &slot_ty, &key_ty, &key_ty, hash, key_ptr)?;
                let grown_end = self.builder.get_insert_block().expect("positioned");
                let (new_data, new_cap) = (seq.data, seq.cap);
                self.builder.build_unconditional_branch(fill_bb).map_err(llvm_err)?;
                self.builder.position_at_end(slot_bb);
                self.builder.build_unconditional_branch(fill_bb).map_err(llvm_err)?;
                self.builder.position_at_end(fill_bb);
                let data_phi = self.builder.build_phi(self.ptr_ty(), "entry.data").map_err(llvm_err)?;
                data_phi.add_incoming(&[(&old_data, slot_bb), (&new_data, grown_end)]);
                let cap_phi = self.builder.build_phi(self.usize_ty, "entry.cap").map_err(llvm_err)?;
                cap_phi.add_incoming(&[(&old_cap, slot_bb), (&new_cap, grown_end)]);
                let at_phi = self.builder.build_phi(self.usize_ty, "entry.at").map_err(llvm_err)?;
                at_phi.add_incoming(&[(&at, slot_bb), (&grown_at, grown_end)]);
                seq.data = data_phi.as_basic_value().into_pointer_value();
                seq.cap = cap_phi.as_basic_value().into_int_value();
                let at = at_phi.as_basic_value().into_int_value();
                let value = if with_make {
                    let Some(arg_slot) = arg_slot else {
                        return Err("internal error: `or_insert_with` lost its closure".into());
                    };
                    let closure =
                        self.builder.build_load(self.closure_ty, arg_slot, "").map_err(llvm_err)?.into_struct_value();
                    self.call_closure_value(closure, &args[1].ty, FnKind::FnOnce, &[])?
                } else {
                    match (arg_slot, self.llvm_ty(&value_ty)) {
                        (Some(arg_slot), Some(vt)) => {
                            Some(self.builder.build_load(vt, arg_slot, "").map_err(llvm_err)?)
                        }
                        _ => None,
                    }
                };
                let slot = self.map_slot(&seq, &slot_ty, at)?;
                if let Some(key_ptr) = key_ptr {
                    let kt = self.llvm_ty(&key_ty).expect("checked above");
                    let kv = self.builder.build_load(kt, key_ptr, "").map_err(llvm_err)?;
                    let key_field = self.map_field(slot_t, slot, 0, "map.key")?;
                    self.builder.build_store(key_field, kv).map_err(llvm_err)?;
                }
                if let Some(value) = value {
                    let value_field = self.map_field(slot_t, slot, 1, "map.value")?;
                    self.builder.build_store(value_field, value).map_err(llvm_err)?;
                }
                let hash_field = self.map_field(slot_t, slot, 2, "map.hash")?;
                self.builder.build_store(hash_field, hash).map_err(llvm_err)?;
                seq.len = next_len;
                self.seq_store(&seq)?;
                let value_ptr = self.map_field(slot_t, slot, 1, "map.value")?;
                self.builder.build_store(result, value_ptr).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

                self.builder.position_at_end(end);
                Ok(Some(self.builder.build_load(self.ptr_ty(), result, "").map_err(llvm_err)?))
            }
            _ => Err(format!("unsupported collection method {m:?} on `{}`", args[0].ty)),
        }
    }

    /// Whether `seq` holds an element equal to the value at `key_ptr`; sets a zero when the buffer is empty.
    fn sequence_contains(
        &mut self,
        data: PointerValue<'ctx>,
        len: IntValue<'ctx>,
        elem: &Ty,
        key_ptr: Option<PointerValue<'ctx>>,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let bool_ty = self.cx.bool_type();
        let Some(key_ptr) = key_ptr else { return Ok(Some(bool_ty.const_zero().into())) };
        let Some(elem_t) = self.llvm_ty(elem) else { return Ok(Some(bool_ty.const_int(1, false).into())) };
        let index = self.alloca(self.usize_ty.into(), "find.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let acc = self.alloca(bool_ty.into(), "find.found")?;
        self.builder.build_store(acc, bool_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("find.cond");
        let body = self.new_block("find.body");
        let next = self.new_block("find.next");
        let exit = self.new_block("find.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let p = unsafe { self.builder.build_in_bounds_gep(elem_t, data, &[i], "") }.map_err(llvm_err)?;
        let eq = self.eq_at(p, key_ptr, elem)?;
        self.builder.build_store(acc, eq).map_err(llvm_err)?;
        self.builder.build_conditional_branch(eq, exit, next).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(Some(self.builder.build_load(bool_ty, acc, "").map_err(llvm_err)?))
    }

    /// Insertion sort of a list buffer by `Ord`.
    fn sort_sequence(&mut self, seq: &Seq<'ctx>, elem: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some(elem_t) = self.llvm_ty(elem) else { return Ok(None) };
        let size = self.elem_size(elem);
        let temp = self.alloca(elem_t, "sort.tmp")?;
        let i = self.alloca(self.usize_ty.into(), "sort.i")?;
        self.builder.build_store(i, self.usize_ty.const_int(1, false)).map_err(llvm_err)?;
        let header = self.new_block("sort.cond");
        let body = self.new_block("sort.body");
        let exit = self.new_block("sort.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let iv = self.builder.build_load(self.usize_ty, i, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, iv, seq.len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let j = self.alloca(self.usize_ty.into(), "sort.j")?;
        self.builder.build_store(j, iv).map_err(llvm_err)?;
        let inner = self.new_block("sort.inner");
        let compare = self.new_block("sort.compare");
        let swap = self.new_block("sort.swap");
        let next_i = self.new_block("sort.next");
        self.builder.build_unconditional_branch(inner).map_err(llvm_err)?;
        self.builder.position_at_end(inner);
        let jv = self.builder.build_load(self.usize_ty, j, "").map_err(llvm_err)?.into_int_value();
        let positive =
            self.builder.build_int_compare(IntPredicate::UGT, jv, self.usize_zero(), "").map_err(llvm_err)?;
        // Only look at `j - 1` when `j > 0`.
        self.builder.build_conditional_branch(positive, compare, next_i).map_err(llvm_err)?;
        self.builder.position_at_end(compare);
        let prev = self.builder.build_int_sub(jv, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        let less = self.elem_ptr(seq, elem, prev);
        let cur = self.elem_ptr(seq, elem, jv);
        let (Some(less), Some(cur)) = (less, cur) else {
            self.builder.build_unconditional_branch(next_i).map_err(llvm_err)?;
            self.builder.position_at_end(next_i);
            let n = self.builder.build_int_add(iv, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.builder.build_store(i, n).map_err(llvm_err)?;
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(exit);
            return Ok(None);
        };
        let order = self.cmp_at(less, cur, elem)?;
        let swap_needed = self
            .builder
            .build_int_compare(IntPredicate::SGT, order, self.cx.i32_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(swap_needed, swap, next_i).map_err(llvm_err)?;
        self.builder.position_at_end(swap);
        let bytes = self.usize_ty.const_int(size, false);
        self.builder.build_memcpy(temp, 1, less, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(less, 1, cur, 1, bytes).map_err(llvm_err)?;
        self.builder.build_memcpy(cur, 1, temp, 1, bytes).map_err(llvm_err)?;
        let back = self.builder.build_int_sub(jv, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(j, back).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(inner).map_err(llvm_err)?;
        self.builder.position_at_end(next_i);
        let n = self.builder.build_int_add(iv, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(i, n).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(None)
    }

    /// Drops the elements of `seq` (no flags) in order.
    fn drop_sequence_elements(&mut self, seq: &Seq<'ctx>, elem: &Ty) -> GenResult<()> {
        let zero = self.usize_zero();
        let len = seq.len;
        self.drop_sequence_range(seq, elem, zero, len)
    }

    /// Drops the elements of `seq` in `from..to` (no flags), in index order.
    fn drop_sequence_range(
        &mut self,
        seq: &Seq<'ctx>,
        elem: &Ty,
        from: IntValue<'ctx>,
        to: IntValue<'ctx>,
    ) -> GenResult<()> {
        if !self.needs_drop(elem) {
            return Ok(());
        }
        let Some(elem_t) = self.llvm_ty(elem) else { return Ok(()) };
        let index = self.alloca(self.usize_ty.into(), "clear.i")?;
        self.builder.build_store(index, from).map_err(llvm_err)?;
        let header = self.new_block("clear.cond");
        let body = self.new_block("clear.body");
        let exit = self.new_block("clear.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, to, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let p = unsafe { self.builder.build_in_bounds_gep(elem_t, seq.data, &[i], "") }.map_err(llvm_err)?;
        self.drop_value(p, elem)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// `Some(view of element `index`)`, or of the last element when `index` is `None`; `None` when the list is
    /// empty or the index is out of bounds.
    fn sequence_element_view(
        &mut self,
        seq: &Seq<'ctx>,
        elem: &Ty,
        view: &Ty,
        index: Option<IntValue<'ctx>>,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let slot = match self.llvm_ty(ty) {
            Some(t) => Some(self.alloca(t, "element.out")?),
            None => None,
        };
        let (at, valid) = match index {
            Some(i) => (i, self.builder.build_int_compare(IntPredicate::ULT, i, seq.len, "").map_err(llvm_err)?),
            None => {
                let at =
                    self.builder.build_int_sub(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                let valid = self
                    .builder
                    .build_int_compare(IntPredicate::NE, seq.len, self.usize_zero(), "")
                    .map_err(llvm_err)?;
                (at, valid)
            }
        };
        let some_bb = self.new_block("element.some");
        let none_bb = self.new_block("element.none");
        let end = self.new_block("element.end");
        self.builder.build_conditional_branch(valid, some_bb, none_bb).map_err(llvm_err)?;
        self.builder.position_at_end(none_bb);
        if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
            self.builder.build_store(slot, self.option_ty(view).const_zero()).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(some_bb);
        let pointer = self.elem_ptr(seq, elem, at);
        if let (Some(slot), Some(_)) = (slot, self.llvm_ty(ty)) {
            let some = self.some_value(view, pointer.map(BasicValueEnum::from));
            self.builder.build_store(slot, some).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(match (slot, self.llvm_ty(ty)) {
            (Some(slot), Some(t)) => Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?),
            _ => None,
        })
    }

    /// Grows the buffer to hold at least `want` elements, doubling the capacity when that is enough, through the
    /// container's allocator.
    fn seq_reserve_to(&mut self, seq: &mut Seq<'ctx>, elem: &Ty, want: IntValue<'ctx>, span: Span) -> GenResult<()> {
        let (old_data, old_cap) = (seq.data, seq.cap);
        let enough = self.builder.build_int_compare(IntPredicate::ULE, want, old_cap, "").map_err(llvm_err)?;
        let done = self.new_block("reserve.enough");
        let grow = self.new_block("reserve.grow");
        let before = self.builder.get_insert_block().expect("positioned");
        self.builder.build_conditional_branch(enough, done, grow).map_err(llvm_err)?;
        self.builder.position_at_end(grow);
        let doubled = self.builder.build_int_mul(old_cap, self.usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let short = self.builder.build_int_compare(IntPredicate::ULT, doubled, want, "").map_err(llvm_err)?;
        let new_cap = self.builder.build_select(short, want, doubled, "").map_err(llvm_err)?.into_int_value();
        self.seq_realloc_to(seq, elem, new_cap, span)?;
        let (new_data, grown_cap) = (seq.data, seq.cap);
        let grown = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        let data = self.builder.build_phi(self.ptr_ty(), "reserve.data").map_err(llvm_err)?;
        data.add_incoming(&[(&old_data, before), (&new_data, grown)]);
        let cap = self.builder.build_phi(self.usize_ty, "reserve.cap").map_err(llvm_err)?;
        cap.add_incoming(&[(&old_cap, before), (&grown_cap, grown)]);
        seq.data = data.as_basic_value().into_pointer_value();
        seq.cap = cap.as_basic_value().into_int_value();
        Ok(())
    }

    /// Grows the buffer to exactly `want` elements when it is smaller, through the container's allocator.
    fn seq_reserve_exact_to(
        &mut self,
        seq: &mut Seq<'ctx>,
        elem: &Ty,
        want: IntValue<'ctx>,
        span: Span,
    ) -> GenResult<()> {
        let (old_data, old_cap) = (seq.data, seq.cap);
        let enough = self.builder.build_int_compare(IntPredicate::ULE, want, old_cap, "").map_err(llvm_err)?;
        let done = self.new_block("reserve.enough");
        let grow = self.new_block("reserve.grow");
        let before = self.builder.get_insert_block().expect("positioned");
        self.builder.build_conditional_branch(enough, done, grow).map_err(llvm_err)?;
        self.builder.position_at_end(grow);
        self.seq_realloc_to(seq, elem, want, span)?;
        let (new_data, new_cap) = (seq.data, seq.cap);
        let grown = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        let data = self.builder.build_phi(self.ptr_ty(), "reserve.data").map_err(llvm_err)?;
        data.add_incoming(&[(&old_data, before), (&new_data, grown)]);
        let cap = self.builder.build_phi(self.usize_ty, "reserve.cap").map_err(llvm_err)?;
        cap.add_incoming(&[(&old_cap, before), (&new_cap, grown)]);
        seq.data = data.as_basic_value().into_pointer_value();
        seq.cap = cap.as_basic_value().into_int_value();
        Ok(())
    }

    /// Changes the buffer's capacity to exactly `new_cap` elements. A zero capacity frees the buffer; a buffer that
    /// had none is allocated only for a positive capacity. The live elements are copied.
    fn seq_realloc_to(&mut self, seq: &mut Seq<'ctx>, elem: &Ty, new_cap: IntValue<'ctx>, span: Span) -> GenResult<()> {
        let zero = self.usize_zero();
        let same = self.builder.build_int_compare(IntPredicate::EQ, new_cap, seq.cap, "").map_err(llvm_err)?;
        let unchanged = self.new_block("seq.capacity.same");
        let work = self.new_block("seq.capacity");
        let end = self.new_block("seq.capacity.end");
        let (old_data, old_cap) = (seq.data, seq.cap);
        self.builder.build_conditional_branch(same, unchanged, work).map_err(llvm_err)?;
        self.builder.position_at_end(unchanged);
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(work);
        let size = self.usize_ty.const_int(self.elem_size(elem), false);
        let old_bytes = self.builder.build_int_mul(old_cap, size, "").map_err(llvm_err)?;
        let new_bytes = self.builder.build_int_mul(new_cap, size, "").map_err(llvm_err)?;
        let align = self.value_align(elem);
        let is_zero = self.builder.build_int_compare(IntPredicate::EQ, new_cap, zero, "").map_err(llvm_err)?;
        let free_bb = self.new_block("seq.capacity.free");
        let grow_bb = self.new_block("seq.capacity.grow");
        let done = self.new_block("seq.capacity.done");
        self.builder.build_conditional_branch(is_zero, free_bb, grow_bb).map_err(llvm_err)?;
        self.builder.position_at_end(free_bb);
        let alloc = seq.alloc.clone();
        let alloc_addr = seq.alloc_addr;
        self.dealloc_in(&alloc, alloc_addr, old_data, old_bytes, align)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(grow_bb);
        let had = self.builder.build_int_compare(IntPredicate::NE, old_cap, zero, "").map_err(llvm_err)?;
        let fresh_bb = self.new_block("seq.capacity.alloc");
        let renew_bb = self.new_block("seq.capacity.realloc");
        let grew = self.new_block("seq.capacity.grew");
        self.builder.build_conditional_branch(had, renew_bb, fresh_bb).map_err(llvm_err)?;
        self.builder.position_at_end(fresh_bb);
        let fresh = self.alloc_in(&alloc, alloc_addr, new_bytes, align, Some(span))?;
        self.builder.build_unconditional_branch(grew).map_err(llvm_err)?;
        self.builder.position_at_end(renew_bb);
        let renewed = self.realloc_in(&alloc, alloc_addr, old_data, old_bytes, new_bytes, align, Some(span))?;
        self.builder.build_unconditional_branch(grew).map_err(llvm_err)?;
        self.builder.position_at_end(grew);
        let data = self.builder.build_phi(self.ptr_ty(), "seq.capacity.data").map_err(llvm_err)?;
        data.add_incoming(&[(&fresh, fresh_bb), (&renewed, renew_bb)]);
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        let null = self.ptr_ty().const_null();
        let zero_value = self.usize_zero();
        let data_done = self.builder.build_phi(self.ptr_ty(), "seq.capacity.data.done").map_err(llvm_err)?;
        data_done.add_incoming(&[(&null, free_bb), (&data.as_basic_value(), grew)]);
        let cap_done = self.builder.build_phi(self.usize_ty, "seq.capacity.cap.done").map_err(llvm_err)?;
        cap_done.add_incoming(&[(&zero_value, free_bb), (&new_cap, grew)]);
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        let data_end = self.builder.build_phi(self.ptr_ty(), "seq.capacity.data.end").map_err(llvm_err)?;
        data_end.add_incoming(&[(&old_data, unchanged), (&data_done.as_basic_value(), done)]);
        let cap_end = self.builder.build_phi(self.usize_ty, "seq.capacity.cap.end").map_err(llvm_err)?;
        cap_end.add_incoming(&[(&old_cap, unchanged), (&cap_done.as_basic_value(), done)]);
        seq.data = data_end.as_basic_value().into_pointer_value();
        seq.cap = cap_end.as_basic_value().into_int_value();
        Ok(())
    }

    /// Releases the buffer and the stored allocator of a sequence whose elements were moved out (`append`).
    fn free_sequence(&mut self, seq: &Seq<'ctx>, elem: &Ty) -> GenResult<()> {
        let bytes = self
            .builder
            .build_int_mul(seq.cap, self.usize_ty.const_int(self.elem_size(elem), false), "")
            .map_err(llvm_err)?;
        let align = self.value_align(elem);
        let alloc = seq.alloc.clone();
        self.dealloc_in(&alloc, seq.alloc_addr, seq.data, bytes, align)?;
        self.drop_stored_allocator(&alloc, seq.alloc_addr)
    }

    /// An empty list value with room for `capacity` elements, allocated through `alloc`. When the list stores an
    /// allocator, its value is read back from `alloc_addr` after the allocation: allocating may change the
    /// allocator in place (an arena advances its cursor), so the stored value must be the updated one.
    pub(super) fn list_with_capacity(
        &mut self,
        capacity: IntValue<'ctx>,
        elem: &Ty,
        alloc: &burdin_tir::Alloc,
        alloc_addr: Option<PointerValue<'ctx>>,
        span: Span,
    ) -> GenResult<StructValue<'ctx>> {
        let seq_ty = self.seq_ty(alloc);
        let ptr_ty = self.ptr_ty();
        let zero = self.usize_zero();
        let bytes = self
            .builder
            .build_int_mul(capacity, self.usize_ty.const_int(self.elem_size(elem), false), "")
            .map_err(llvm_err)?;
        let empty = self.builder.build_int_compare(IntPredicate::EQ, capacity, zero, "").map_err(llvm_err)?;
        let allocate = self.new_block("list.capacity.alloc");
        let end = self.new_block("list.capacity.end");
        let before = self.builder.get_insert_block().expect("positioned");
        self.builder.build_conditional_branch(empty, end, allocate).map_err(llvm_err)?;
        self.builder.position_at_end(allocate);
        let align = self.value_align(elem);
        let fresh = self.alloc_in(alloc, alloc_addr, bytes, align, Some(span))?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        let data = self.builder.build_phi(ptr_ty, "list.capacity.data").map_err(llvm_err)?;
        let null = ptr_ty.const_null();
        data.add_incoming(&[(&null, before), (&fresh, allocate)]);
        let mut agg = seq_ty.const_zero();
        agg = self.builder.build_insert_value(agg, data.as_basic_value(), 0, "").map_err(llvm_err)?.into_struct_value();
        agg = self.builder.build_insert_value(agg, capacity, 2, "").map_err(llvm_err)?.into_struct_value();
        if let (Some(addr), Some(ty)) = (alloc_addr, alloc.allocator_ty()) {
            let t = self.storage_ty(ty);
            let value = self.builder.build_load(t, addr, "list.alloc").map_err(llvm_err)?;
            agg = self.builder.build_insert_value(agg, value, 3, "").map_err(llvm_err)?.into_struct_value();
        }
        Ok(agg)
    }

    /// A map entry is `(key, value, hash)`. A zero hash marks an empty slot; a live entry stores a nonzero hash.
    pub(super) fn map_entry_ty(key: &Ty, value: &Ty) -> Ty {
        Ty::Tuple(vec![key.clone(), value.clone(), Ty::Int(tir::IntTy::U64)])
    }

    fn map_entry_size(&self, key: &Ty, value: &Ty) -> u64 {
        self.elem_size(&Self::map_entry_ty(key, value))
    }

    /// A field of a map entry slot.
    fn map_field(
        &mut self,
        entry_t: inkwell::types::StructType<'ctx>,
        slot: PointerValue<'ctx>,
        index: u32,
        name: &str,
    ) -> GenResult<PointerValue<'ctx>> {
        self.builder.build_struct_gep(entry_t, slot, index, name).map_err(llvm_err)
    }

    /// The hash word of an entry slot; zero means empty.
    fn map_slot_hash(
        &mut self,
        entry_t: inkwell::types::StructType<'ctx>,
        slot: PointerValue<'ctx>,
    ) -> GenResult<IntValue<'ctx>> {
        let p = self.map_field(entry_t, slot, 2, "map.hash")?;
        Ok(self.builder.build_load(self.cx.i64_type(), p, "").map_err(llvm_err)?.into_int_value())
    }

    /// The address of slot `index` of a map.
    fn map_slot(&mut self, seq: &Seq<'ctx>, entry: &Ty, index: IntValue<'ctx>) -> GenResult<PointerValue<'ctx>> {
        self.elem_ptr(seq, entry, index).ok_or_else(|| "a map entry has no runtime type".to_string())
    }

    /// The hash a map stores for the key at `key_ptr`: FNV-1a over the key's parts, remapped so that zero (the
    /// empty-slot marker) is never a live hash.
    fn map_key_hash(&mut self, key_ptr: Option<PointerValue<'ctx>>, key_ty: &Ty) -> GenResult<IntValue<'ctx>> {
        let start = self.cx.i64_type().const_int(0xcbf2_9ce4_8422_2325, false);
        let hash = match key_ptr {
            Some(ptr) => self.hash_at(ptr, key_ty, start)?,
            None => start,
        };
        let empty = self
            .builder
            .build_int_compare(IntPredicate::EQ, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        let one = self.cx.i64_type().const_int(1, false);
        Ok(self.builder.build_select(empty, one, hash, "").map_err(llvm_err)?.into_int_value())
    }

    /// Structural equality of two maps (`spec/stdlib.md`, "Map"): the same number of entries, and every entry of
    /// the left map found in the right one with an equal value. The iteration order does not matter.
    pub(super) fn map_eq(
        &mut self,
        l: PointerValue<'ctx>,
        r: PointerValue<'ctx>,
        key: &Ty,
        value: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<IntValue<'ctx>> {
        let left = self.seq_load_at(l, alloc)?;
        let right = self.seq_load_at(r, alloc)?;
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let bool_ty = self.cx.bool_type();
        let result = self.alloca(bool_ty.into(), "map.eq.result")?;
        let same_len = self.builder.build_int_compare(IntPredicate::EQ, left.len, right.len, "").map_err(llvm_err)?;
        self.builder.build_store(result, same_len).map_err(llvm_err)?;
        let check = self.new_block("map.eq.check");
        let scan = self.new_block("map.eq.scan");
        let end = self.new_block("map.eq.end");
        self.builder.build_conditional_branch(same_len, check, end).map_err(llvm_err)?;
        self.builder.position_at_end(check);
        let empty =
            self.builder.build_int_compare(IntPredicate::EQ, left.len, self.usize_zero(), "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(empty, end, scan).map_err(llvm_err)?;
        self.builder.position_at_end(scan);
        let index = self.alloca(self.usize_ty.into(), "map.eq.i")?;
        self.builder.build_store(index, self.usize_zero()).map_err(llvm_err)?;
        let header = self.new_block("map.eq.cond");
        let body = self.new_block("map.eq.body");
        let live = self.new_block("map.eq.live");
        let compare = self.new_block("map.eq.compare");
        let fail = self.new_block("map.eq.fail");
        let next = self.new_block("map.eq.next");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, left.cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let slot = self.map_slot(&left, &entry, i)?;
        let hash = self.map_slot_hash(entry_t, slot)?;
        let vacant = self
            .builder
            .build_int_compare(IntPredicate::EQ, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(vacant, next, live).map_err(llvm_err)?;
        self.builder.position_at_end(live);
        let left_key = self.map_field(entry_t, slot, 0, "map.key")?;
        let left_value = self.map_field(entry_t, slot, 1, "map.value")?;
        let (found, at) = self.map_probe(&right, &entry, key, key, hash, Some(left_key))?;
        self.builder.build_conditional_branch(found, compare, fail).map_err(llvm_err)?;
        self.builder.position_at_end(compare);
        let at_slot = self.map_slot(&right, &entry, at)?;
        let right_value = self.map_field(entry_t, at_slot, 1, "map.value")?;
        let equal = self.eq_at(right_value, left_value, value)?;
        self.builder.build_conditional_branch(equal, next, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        self.builder.build_store(result, bool_ty.const_zero()).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(self.builder.build_load(bool_ty, result, "").map_err(llvm_err)?.into_int_value())
    }

    /// Whether the key stored in a map slot (of type `key_ty`) equals the lookup key at `key_ptr` (of type
    /// `key_arg`). A `str` argument looks up a `String` key by bytes, which is the borrowed-key rule
    /// (`spec/stdlib.md`, "Borrowed-key lookup").
    fn map_key_eq(
        &mut self,
        slot_key: PointerValue<'ctx>,
        key_ptr: PointerValue<'ctx>,
        key_ty: &Ty,
        key_arg: &Ty,
    ) -> GenResult<IntValue<'ctx>> {
        if key_ty == key_arg {
            return self.eq_at(slot_key, key_ptr, key_ty);
        }
        if matches!(key_ty, Ty::String) && matches!(key_arg, Ty::Str) {
            let (a, alen) = self.parts_at(slot_key, key_ty)?;
            let (b, blen) = self.parts_at(key_ptr, key_arg)?;
            let call = self
                .builder
                .build_call(self.rt.str_eq, &[a.into(), alen.into(), b.into(), blen.into()], "")
                .map_err(llvm_err)?;
            return Ok(call.try_as_basic_value().basic().expect("`str_eq` returns a bool").into_int_value());
        }
        self.eq_at(slot_key, key_ptr, key_ty)
    }

    /// Finds the entry whose key equals the value at `key_ptr`: the slot and whether it was found. When the key is
    /// absent, the slot is the empty one where it belongs. `seq.cap` must be a nonzero power of two.
    fn map_probe(
        &mut self,
        seq: &Seq<'ctx>,
        entry: &Ty,
        key_ty: &Ty,
        key_arg: &Ty,
        hash: IntValue<'ctx>,
        key_ptr: Option<PointerValue<'ctx>>,
    ) -> GenResult<(IntValue<'ctx>, IntValue<'ctx>)> {
        let entry_t = self.llvm_ty(entry).expect("a map entry has a type").into_struct_type();
        let one = self.usize_ty.const_int(1, false);
        let mask = self.builder.build_int_sub(seq.cap, one, "").map_err(llvm_err)?;
        let index = self.alloca(self.usize_ty.into(), "map.probe.i")?;
        let start = self.builder.build_and(hash, mask, "").map_err(llvm_err)?;
        self.builder.build_store(index, start).map_err(llvm_err)?;
        let found = self.alloca(self.cx.bool_type().into(), "map.probe.found")?;
        self.builder.build_store(found, self.cx.bool_type().const_zero()).map_err(llvm_err)?;
        let at = self.alloca(self.usize_ty.into(), "map.probe.at")?;
        let header = self.new_block("map.probe.cond");
        let occupied = self.new_block("map.probe.occupied");
        let compare = self.new_block("map.probe.compare");
        let next = self.new_block("map.probe.next");
        let hit = self.new_block("map.probe.hit");
        let miss = self.new_block("map.probe.miss");
        let end = self.new_block("map.probe.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let slot = self.map_slot(seq, entry, i)?;
        let slot_hash = self.map_slot_hash(entry_t, slot)?;
        let empty = self
            .builder
            .build_int_compare(IntPredicate::EQ, slot_hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(empty, miss, occupied).map_err(llvm_err)?;

        self.builder.position_at_end(occupied);
        let same_hash = self.builder.build_int_compare(IntPredicate::EQ, slot_hash, hash, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(same_hash, compare, next).map_err(llvm_err)?;

        self.builder.position_at_end(compare);
        let key_field = self.map_field(entry_t, slot, 0, "map.key")?;
        let equal = match key_ptr {
            Some(key_ptr) => self.map_key_eq(key_field, key_ptr, key_ty, key_arg)?,
            None => self.cx.bool_type().const_int(1, false),
        };
        self.builder.build_conditional_branch(equal, hit, next).map_err(llvm_err)?;

        self.builder.position_at_end(hit);
        self.builder.build_store(found, self.cx.bool_type().const_int(1, false)).map_err(llvm_err)?;
        self.builder.build_store(at, i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

        self.builder.position_at_end(miss);
        self.builder.build_store(at, i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

        self.builder.position_at_end(next);
        let after = self.builder.build_int_add(i, one, "").map_err(llvm_err)?;
        let next_i = self.builder.build_and(after, mask, "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(end);
        let was_found = self.builder.build_load(self.cx.bool_type(), found, "").map_err(llvm_err)?.into_int_value();
        let slot_index = self.builder.build_load(self.usize_ty, at, "").map_err(llvm_err)?.into_int_value();
        Ok((was_found, slot_index))
    }

    /// The first empty slot at or after `hash & (cap - 1)` in a map buffer. The caller guarantees there is one.
    fn map_empty_slot(
        &mut self,
        base: PointerValue<'ctx>,
        cap: IntValue<'ctx>,
        entry: &Ty,
        hash: IntValue<'ctx>,
    ) -> GenResult<PointerValue<'ctx>> {
        let entry_t = self.llvm_ty(entry).expect("a map entry has a type").into_struct_type();
        let one = self.usize_ty.const_int(1, false);
        let mask = self.builder.build_int_sub(cap, one, "").map_err(llvm_err)?;
        let index = self.alloca(self.usize_ty.into(), "map.empty.i")?;
        let start = self.builder.build_and(hash, mask, "").map_err(llvm_err)?;
        self.builder.build_store(index, start).map_err(llvm_err)?;
        let header = self.new_block("map.empty.cond");
        let next = self.new_block("map.empty.next");
        let end = self.new_block("map.empty.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let slot = self.elem_ptr_at(base, entry, i).ok_or_else(|| "a map entry has no runtime type".to_string())?;
        let slot_hash = self.map_slot_hash(entry_t, slot)?;
        let empty = self
            .builder
            .build_int_compare(IntPredicate::EQ, slot_hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(empty, end, next).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let after = self.builder.build_int_add(i, one, "").map_err(llvm_err)?;
        let next_i = self.builder.build_and(after, mask, "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(slot)
    }

    /// The smallest power-of-two slot count (at least eight) that holds `needed` entries without passing the 3/4
    /// load factor. The limit is 2^62 entries, far past any allocation; a larger request panics.
    pub(super) fn map_capacity_for(&mut self, needed: IntValue<'ctx>, span: Span) -> GenResult<IntValue<'ctx>> {
        let limit = self.usize_ty.const_int(1u64 << 62, false);
        let fits = self.builder.build_int_compare(IntPredicate::ULE, needed, limit, "").map_err(llvm_err)?;
        self.check(fits, "Map capacity overflow", span)?;
        let cap = self.alloca(self.usize_ty.into(), "map.cap")?;
        self.builder.build_store(cap, self.usize_ty.const_int(8, false)).map_err(llvm_err)?;
        let header = self.new_block("map.cap.cond");
        let body = self.new_block("map.cap.grow");
        let done = self.new_block("map.cap.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let current = self.builder.build_load(self.usize_ty, cap, "").map_err(llvm_err)?.into_int_value();
        let quarter =
            self.builder.build_int_unsigned_div(current, self.usize_ty.const_int(4, false), "").map_err(llvm_err)?;
        let room = self.builder.build_int_sub(current, quarter, "").map_err(llvm_err)?;
        let enough = self.builder.build_int_compare(IntPredicate::ULE, needed, room, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(enough, done, body).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let doubled = self.builder.build_int_mul(current, self.usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        self.builder.build_store(cap, doubled).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        Ok(self.builder.build_load(self.usize_ty, cap, "").map_err(llvm_err)?.into_int_value())
    }

    /// Doubles a map's slot table and rehashes its entries (an empty map gets eight slots); `seq` then holds the new
    /// buffer and capacity. The old buffer is freed without dropping its entries: ownership moved into the new table.
    fn map_grow(&mut self, seq: &mut Seq<'ctx>, key: &Ty, value: &Ty, span: Span) -> GenResult<()> {
        let zero = self.usize_ty.const_zero();
        let doubled = self.builder.build_int_mul(seq.cap, self.usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let no_table = self.builder.build_int_compare(IntPredicate::EQ, seq.cap, zero, "").map_err(llvm_err)?;
        let new_cap = self
            .builder
            .build_select(no_table, self.usize_ty.const_int(8, false), doubled, "")
            .map_err(llvm_err)?
            .into_int_value();
        self.map_rehash_to(seq, key, value, new_cap, span)
    }

    /// Rehashes a map's entries into a fresh table of `new_cap` slots; `seq` then holds the new buffer and
    /// capacity. The old buffer is freed without dropping its entries: ownership moved into the new table.
    pub(super) fn map_rehash_to(
        &mut self,
        seq: &mut Seq<'ctx>,
        key: &Ty,
        value: &Ty,
        new_cap: IntValue<'ctx>,
        span: Span,
    ) -> GenResult<()> {
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let size = self.map_entry_size(key, value);
        let one = self.usize_ty.const_int(1, false);
        let zero = self.usize_ty.const_zero();
        let bytes = self.builder.build_int_mul(new_cap, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
        let align = self.value_align(&entry);
        let base = self.alloc_in(&seq.alloc, seq.alloc_addr, bytes, align, Some(span))?;
        let entry_bytes = self.usize_ty.const_int(size, false);

        // Zero every hash word: every slot of the new table starts empty.
        let index = self.alloca(self.usize_ty.into(), "map.zero.i")?;
        self.builder.build_store(index, zero).map_err(llvm_err)?;
        let zero_cond = self.new_block("map.zero.cond");
        let zero_body = self.new_block("map.zero.body");
        let zero_end = self.new_block("map.zero.end");
        self.builder.build_unconditional_branch(zero_cond).map_err(llvm_err)?;
        self.builder.position_at_end(zero_cond);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, new_cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, zero_body, zero_end).map_err(llvm_err)?;
        self.builder.position_at_end(zero_body);
        let slot = self.elem_ptr_at(base, &entry, i).ok_or_else(|| "a map entry has no runtime type".to_string())?;
        let hash_field = self.map_field(entry_t, slot, 2, "map.hash")?;
        self.builder.build_store(hash_field, self.cx.i64_type().const_zero()).map_err(llvm_err)?;
        let next = self.builder.build_int_add(i, one, "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(zero_cond).map_err(llvm_err)?;

        // Move the live entries into their new slots by hash; a move never compares keys.
        self.builder.position_at_end(zero_end);
        self.builder.build_store(index, zero).map_err(llvm_err)?;
        let move_cond = self.new_block("map.move.cond");
        let move_body = self.new_block("map.move.body");
        let place = self.new_block("map.move.place");
        let move_next = self.new_block("map.move.next");
        let move_end = self.new_block("map.move.end");
        self.builder.build_unconditional_branch(move_cond).map_err(llvm_err)?;
        self.builder.position_at_end(move_cond);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, seq.cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, move_body, move_end).map_err(llvm_err)?;
        self.builder.position_at_end(move_body);
        let old = self.map_slot(seq, &entry, i)?;
        let old_hash = self.map_slot_hash(entry_t, old)?;
        let live = self
            .builder
            .build_int_compare(IntPredicate::NE, old_hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(live, place, move_next).map_err(llvm_err)?;
        self.builder.position_at_end(place);
        let dest = self.map_empty_slot(base, new_cap, &entry, old_hash)?;
        self.builder.build_memcpy(dest, 1, old, 1, entry_bytes).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(move_next).map_err(llvm_err)?;
        self.builder.position_at_end(move_next);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, one, "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(move_cond).map_err(llvm_err)?;
        self.builder.position_at_end(move_end);

        let old_bytes = self.builder.build_int_mul(seq.cap, entry_bytes, "").map_err(llvm_err)?;
        let alloc = seq.alloc.clone();
        let alloc_addr = seq.alloc_addr;
        self.dealloc_in(&alloc, alloc_addr, seq.data, old_bytes, align)?;
        seq.data = base;
        seq.cap = new_cap;
        Ok(())
    }

    /// Removes the entry at `hole`, shifting the rest of its probe cluster back so every remaining key stays
    /// reachable (linear-probing deletion without tombstones).
    fn map_shift_delete(&mut self, seq: &Seq<'ctx>, entry: &Ty, hole: IntValue<'ctx>) -> GenResult<()> {
        let entry_t = self.llvm_ty(entry).expect("a map entry has a type").into_struct_type();
        let size = self.elem_size(entry);
        let entry_bytes = self.usize_ty.const_int(size, false);
        let one = self.usize_ty.const_int(1, false);
        let mask = self.builder.build_int_sub(seq.cap, one, "").map_err(llvm_err)?;
        let hole_slot = self.alloca(self.usize_ty.into(), "map.hole")?;
        self.builder.build_store(hole_slot, hole).map_err(llvm_err)?;
        let j_slot = self.alloca(self.usize_ty.into(), "map.j")?;
        let after = self.builder.build_int_add(hole, one, "").map_err(llvm_err)?;
        let first = self.builder.build_and(after, mask, "").map_err(llvm_err)?;
        self.builder.build_store(j_slot, first).map_err(llvm_err)?;
        let header = self.new_block("map.delete.cond");
        let scan = self.new_block("map.delete.scan");
        let relocate = self.new_block("map.delete.move");
        let advance = self.new_block("map.delete.next");
        let clear = self.new_block("map.delete.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let hole_v = self.builder.build_load(self.usize_ty, hole_slot, "").map_err(llvm_err)?.into_int_value();
        let j = self.builder.build_load(self.usize_ty, j_slot, "").map_err(llvm_err)?.into_int_value();
        let slot = self.map_slot(seq, entry, j)?;
        let hash = self.map_slot_hash(entry_t, slot)?;
        let empty = self
            .builder
            .build_int_compare(IntPredicate::EQ, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(empty, clear, scan).map_err(llvm_err)?;

        self.builder.position_at_end(scan);
        let home = self.builder.build_and(hash, mask, "").map_err(llvm_err)?;
        // The entry stays when its home is in the cyclic interval (hole, j]; otherwise the hole moved past it.
        let before = self.builder.build_int_compare(IntPredicate::ULT, hole_v, j, "").map_err(llvm_err)?;
        let after_hole = self.builder.build_int_compare(IntPredicate::UGT, home, hole_v, "").map_err(llvm_err)?;
        let up_to_j = self.builder.build_int_compare(IntPredicate::ULE, home, j, "").map_err(llvm_err)?;
        let inside = self.builder.build_and(after_hole, up_to_j, "").map_err(llvm_err)?;
        let wrapped = self.builder.build_or(after_hole, up_to_j, "").map_err(llvm_err)?;
        let stays = self.builder.build_select(before, inside, wrapped, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(stays.into_int_value(), advance, relocate).map_err(llvm_err)?;

        self.builder.position_at_end(relocate);
        let dest = self.map_slot(seq, entry, hole_v)?;
        self.builder.build_memcpy(dest, 1, slot, 1, entry_bytes).map_err(llvm_err)?;
        self.builder.build_store(hole_slot, j).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(advance).map_err(llvm_err)?;

        self.builder.position_at_end(advance);
        let j = self.builder.build_load(self.usize_ty, j_slot, "").map_err(llvm_err)?.into_int_value();
        let after = self.builder.build_int_add(j, one, "").map_err(llvm_err)?;
        let next = self.builder.build_and(after, mask, "").map_err(llvm_err)?;
        self.builder.build_store(j_slot, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(clear);
        let hole_v = self.builder.build_load(self.usize_ty, hole_slot, "").map_err(llvm_err)?.into_int_value();
        let slot = self.map_slot(seq, entry, hole_v)?;
        let hash_field = self.map_field(entry_t, slot, 2, "map.hash")?;
        self.builder.build_store(hash_field, self.cx.i64_type().const_zero()).map_err(llvm_err)?;
        Ok(())
    }

    /// Drops every live entry of a map's slot table; the buffer itself is not freed.
    fn map_drop_entries(&mut self, seq: &Seq<'ctx>, key: &Ty, value: &Ty) -> GenResult<()> {
        if !self.needs_drop(key) && !self.needs_drop(value) {
            return Ok(());
        }
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let index = self.alloca(self.usize_ty.into(), "map.drop.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("map.drop.cond");
        let body = self.new_block("map.drop.body");
        let live = self.new_block("map.drop.live");
        let next = self.new_block("map.drop.next");
        let end = self.new_block("map.drop.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, seq.cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let slot = self.map_slot(seq, &entry, i)?;
        let hash = self.map_slot_hash(entry_t, slot)?;
        let occupied = self
            .builder
            .build_int_compare(IntPredicate::NE, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(occupied, live, next).map_err(llvm_err)?;
        self.builder.position_at_end(live);
        let key_field = self.map_field(entry_t, slot, 0, "map.key")?;
        self.drop_value(key_field, key)?;
        let value_field = self.map_field(entry_t, slot, 1, "map.value")?;
        self.drop_value(value_field, value)?;
        self.builder.build_unconditional_branch(next).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(())
    }

    /// Drops a `ListIntoIter<T>`: the elements it has not handed out, then the buffer, through the allocator the
    /// iterator owns. Its layout is `{ ptr, len, cap, index, allocator? }`.
    pub(super) fn drop_list_into_iter(
        &mut self,
        addr: PointerValue<'ctx>,
        elem: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let seq_ty = self.iter_seq_ty_for(alloc);
        let field = |cg: &mut Self, i: u32, name: &str| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(seq_ty, addr, i, name).map_err(llvm_err)
        };
        let data = {
            let p = field(self, 0, "iter.ptr")?;
            self.builder.build_load(ptr_ty, p, "").map_err(llvm_err)?.into_pointer_value()
        };
        let len = {
            let p = field(self, 1, "iter.len")?;
            self.builder.build_load(self.usize_ty, p, "").map_err(llvm_err)?.into_int_value()
        };
        let cap = {
            let p = field(self, 2, "iter.cap")?;
            self.builder.build_load(self.usize_ty, p, "").map_err(llvm_err)?.into_int_value()
        };
        let index = {
            let p = field(self, 3, "iter.index")?;
            self.builder.build_load(self.usize_ty, p, "").map_err(llvm_err)?.into_int_value()
        };
        if self.needs_drop(elem) && self.llvm_ty(elem).is_some() {
            let elem = elem.clone();
            let elem_t = self.llvm_ty(&elem).expect("checked above");
            let i_slot = self.alloca(self.usize_ty.into(), "iter.drop.i")?;
            self.builder.build_store(i_slot, index).map_err(llvm_err)?;
            let header = self.new_block("iter.drop.cond");
            let body = self.new_block("iter.drop.body");
            let end = self.new_block("iter.drop.end");
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(header);
            let i = self.builder.build_load(self.usize_ty, i_slot, "").map_err(llvm_err)?.into_int_value();
            let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
            self.builder.build_conditional_branch(go, body, end).map_err(llvm_err)?;
            self.builder.position_at_end(body);
            // SAFETY (LLVM): index <= i < len.
            let p = unsafe { self.builder.build_in_bounds_gep(elem_t, data, &[i], "") }.map_err(llvm_err)?;
            self.drop_value(p, &elem)?;
            let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
            self.builder.build_store(i_slot, next).map_err(llvm_err)?;
            self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
            self.builder.position_at_end(end);
        }
        let alloc_addr = match alloc.allocator_ty() {
            Some(_) => Some(field(self, 4, "iter.alloc")?),
            None => None,
        };
        let size = self.usize_ty.const_int(self.elem_size(elem), false);
        let bytes = self.builder.build_int_mul(cap, size, "").map_err(llvm_err)?;
        let align = self.value_align(elem);
        self.dealloc_in(alloc, alloc_addr, data, bytes, align)?;
        self.drop_stored_allocator(alloc, alloc_addr)
    }

    /// Drops a `MapIntoIter<K, V>`: the entries it has not handed out (a handed-out entry has a zeroed hash
    /// word), then the slot table.
    pub(super) fn drop_map_into_iter(
        &mut self,
        addr: PointerValue<'ctx>,
        key: &Ty,
        value: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let seq_ty = self.iter_seq_ty_for(alloc);
        let field = |cg: &mut Self, i: u32, name: &str| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(seq_ty, addr, i, name).map_err(llvm_err)
        };
        let data = {
            let p = field(self, 0, "iter.ptr")?;
            self.builder.build_load(ptr_ty, p, "").map_err(llvm_err)?.into_pointer_value()
        };
        let len = {
            let p = field(self, 1, "iter.len")?;
            self.builder.build_load(self.usize_ty, p, "").map_err(llvm_err)?.into_int_value()
        };
        let cap = {
            let p = field(self, 2, "iter.cap")?;
            self.builder.build_load(self.usize_ty, p, "").map_err(llvm_err)?.into_int_value()
        };
        let alloc_addr = match alloc.allocator_ty() {
            Some(_) => Some(field(self, 4, "iter.alloc")?),
            None => None,
        };
        let seq = Seq { addr, data, len, cap, alloc: alloc.clone(), alloc_addr };
        self.map_drop_entries(&seq, key, value)?;
        let size = self.usize_ty.const_int(self.map_entry_size(key, value), false);
        let bytes = self.builder.build_int_mul(cap, size, "").map_err(llvm_err)?;
        let align = self.value_align(&Self::map_entry_ty(key, value));
        self.dealloc_in(alloc, alloc_addr, data, bytes, align)?;
        self.drop_stored_allocator(alloc, alloc_addr)
    }

    /// The four-word layout of the list and map iterators without a stored allocator.
    pub(super) fn iter_seq_ty(&self) -> inkwell::types::StructType<'ctx> {
        let ptr = self.ptr_ty();
        self.cx.struct_type(&[ptr.into(), self.usize_ty.into(), self.usize_ty.into(), self.usize_ty.into()], false)
    }

    /// The layout of an `ArrayIntoIter<T, N>`: the array itself and the next index.
    pub(super) fn iter_array_ty(&self, elem: &Ty, n: u64) -> inkwell::types::StructType<'ctx> {
        let array = self.storage_ty(elem).array_type(n as u32);
        self.cx.struct_type(&[array.into(), self.usize_ty.into()], false)
    }

    /// Drops the allocator value stored at `addr`, after its buffer is released.
    pub(super) fn drop_stored_allocator(
        &mut self,
        alloc: &burdin_tir::Alloc,
        addr: Option<PointerValue<'ctx>>,
    ) -> GenResult<()> {
        if let (Some(ty), Some(addr)) = (alloc.allocator_ty(), addr)
            && self.needs_drop(ty)
        {
            self.drop_value(addr, ty)?;
        }
        Ok(())
    }

    /// Drops a map: every live entry, then the slot table, then the stored allocator.
    pub(super) fn drop_map(
        &mut self,
        addr: PointerValue<'ctx>,
        key: &Ty,
        value: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let seq_ty = self.seq_ty(alloc);
        let field = |cg: &mut Self, i: u32, name: &str| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(seq_ty, addr, i, name).map_err(llvm_err)
        };
        let data_field = field(self, 0, "map.ptr")?;
        let data = self.builder.build_load(ptr_ty, data_field, "").map_err(llvm_err)?.into_pointer_value();
        let len_field = field(self, 1, "map.len")?;
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let cap_field = field(self, 2, "map.cap")?;
        let cap = self.builder.build_load(self.usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        let alloc_addr = match alloc.allocator_ty() {
            Some(_) => Some(field(self, 3, "map.alloc")?),
            None => None,
        };
        let seq = Seq { addr, data, len, cap, alloc: alloc.clone(), alloc_addr };
        self.map_drop_entries(&seq, key, value)?;
        let size = self.usize_ty.const_int(self.map_entry_size(key, value), false);
        let bytes = self.builder.build_int_mul(cap, size, "").map_err(llvm_err)?;
        let align = self.value_align(&Self::map_entry_ty(key, value));
        self.dealloc_in(alloc, alloc_addr, data, bytes, align)?;
        self.drop_stored_allocator(alloc, alloc_addr)
    }

    /// Deep-copies a map: clones the allocator value first, then the live entries, and preserves the slot table's
    /// shape (and hashes).
    pub(super) fn clone_map(
        &mut self,
        dst: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        key: &Ty,
        value: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let seq_ty = self.seq_ty(alloc);
        let field_src = |cg: &mut Self, base: PointerValue<'ctx>, i: u32| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(seq_ty, base, i, "").map_err(llvm_err)
        };
        let src_ptr_field = field_src(self, src, 0)?;
        let src_len_field = field_src(self, src, 1)?;
        let src_cap_field = field_src(self, src, 2)?;
        let base = self.builder.build_load(ptr_ty, src_ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, src_len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, src_cap_field, "").map_err(llvm_err)?.into_int_value();
        let mut dst_alloc_addr = None;
        if let Some(ty) = alloc.allocator_ty() {
            let s = field_src(self, src, 3)?;
            let d = field_src(self, dst, 3)?;
            self.clone_value(d, s, ty)?;
            dst_alloc_addr = Some(d);
        }
        let bytes = self
            .builder
            .build_int_mul(cap, self.usize_ty.const_int(self.map_entry_size(key, value), false), "")
            .map_err(llvm_err)?;
        let align = self.value_align(&entry);
        let new_base = self.alloc_in(alloc, dst_alloc_addr, bytes, align, None)?;
        let index = self.alloca(self.usize_ty.into(), "map.clone.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("map.clone.cond");
        let body = self.new_block("map.clone.body");
        let live = self.new_block("map.clone.live");
        let next = self.new_block("map.clone.next");
        let end = self.new_block("map.clone.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let s = self.elem_ptr_at(base, &entry, i).ok_or_else(|| "a map entry has no runtime type".to_string())?;
        let d = self.elem_ptr_at(new_base, &entry, i).ok_or_else(|| "a map entry has no runtime type".to_string())?;
        let hash = self.map_slot_hash(entry_t, s)?;
        let d_hash = self.map_field(entry_t, d, 2, "map.hash")?;
        self.builder.build_store(d_hash, hash).map_err(llvm_err)?;
        let occupied = self
            .builder
            .build_int_compare(IntPredicate::NE, hash, self.cx.i64_type().const_zero(), "")
            .map_err(llvm_err)?;
        self.builder.build_conditional_branch(occupied, live, next).map_err(llvm_err)?;
        self.builder.position_at_end(live);
        let s_key = self.map_field(entry_t, s, 0, "map.key")?;
        let d_key = self.map_field(entry_t, d, 0, "map.key")?;
        self.clone_value(d_key, s_key, key)?;
        let s_value = self.map_field(entry_t, s, 1, "map.value")?;
        let d_value = self.map_field(entry_t, d, 1, "map.value")?;
        self.clone_value(d_value, s_value, value)?;
        self.builder.build_unconditional_branch(next).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        let dst_ptr_field = field_src(self, dst, 0)?;
        let dst_len_field = field_src(self, dst, 1)?;
        let dst_cap_field = field_src(self, dst, 2)?;
        self.builder.build_store(dst_ptr_field, new_base).map_err(llvm_err)?;
        self.builder.build_store(dst_len_field, len).map_err(llvm_err)?;
        self.builder.build_store(dst_cap_field, cap).map_err(llvm_err)?;
        Ok(())
    }
}
