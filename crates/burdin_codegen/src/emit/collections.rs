//! `List<T>` and `Map<K, V>`: their layout, growth, indexing, and methods.
//!
//! Both have the three-word layout `{ ptr, len, cap }`, like a `String`. A list buffer holds `cap` elements of
//! `T`. A map buffer is an open-addressing hash table: `cap` slots (a power of two) of `(key, value, hash)` entries,
//! where a zero hash marks an empty slot and `len` counts the live entries. A key is placed at `hash & (cap - 1)`
//! and probes forward; a removal shifts the rest of its probe cluster back, so there are no tombstones. The table
//! grows (and rehashes) when the next insert would pass a 3/4 load factor. Iteration scans the slots, so its order
//! is unspecified, as the standard library says.

use super::{Codegen, GenResult, llvm_err};
use burdin_diag::Span;
use burdin_tir::{self as tir, Ty};
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
        let first_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(grown).map_err(llvm_err)?;
        self.builder.position_at_end(again);
        let renewed = self.realloc_in(&alloc2, alloc_addr2, data, old_bytes, new_bytes, align, Some(span))?;
        let again_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(grown).map_err(llvm_err)?;
        self.builder.position_at_end(grown);
        let data_phi = self.builder.build_phi(self.ptr_ty(), "seq.data").map_err(llvm_err)?;
        data_phi.add_incoming(&[(&fresh, first_end), (&renewed, again_end)]);
        let cap_phi = self.builder.build_phi(self.usize_ty, "seq.cap").map_err(llvm_err)?;
        cap_phi.add_incoming(&[(&at_least_four, first_end), (&at_least_four, again_end)]);
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

    /// `[T].to_list(): List<T>`: a new list holding a clone of every element of the slice.
    pub(super) fn slice_to_list(
        &mut self,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Some((elem, alloc)) = self.list_info(ty) else { return Err("`to_list` returns a `List`".into()) };
        let elem = (*elem).clone();
        let (ptr, len) = self.view_parts(&args[0])?;
        let (ptr, len) = (ptr.into_pointer_value(), len.into_int_value());
        let seq_ty = self.seq_ty(&alloc);
        let slot = self.alloca(seq_ty.into(), "to_list")?;
        self.builder.build_store(slot, seq_ty.const_zero()).map_err(llvm_err)?;
        let elem_ty = self.llvm_ty(&elem);
        let index = self.alloca(self.usize_ty.into(), "to_list.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("to_list.cond");
        let body = self.new_block("to_list.body");
        let end = self.new_block("to_list.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // The list is reloaded each iteration: growing replaces the buffer along one path only, and the loop
        // header must not see a value from that path.
        let mut seq = self.seq_load_at(slot, &alloc)?;
        self.seq_grow(&mut seq, &elem, span)?;
        if let Some(elem_t) = elem_ty {
            // SAFETY (LLVM): `i < len`.
            let src =
                unsafe { self.builder.build_in_bounds_gep(elem_t, ptr, &[i], "to_list.src") }.map_err(llvm_err)?;
            let dst = self.elem_ptr(&seq, &elem, seq.len).ok_or("`to_list` lost its element pointer")?;
            self.clone_value(dst, src, &elem)?;
        }
        seq.len = self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.seq_store(&seq)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(seq_ty, slot, "to_list.value").map_err(llvm_err)?))
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

    /// Structural equality of two lists (`spec/stdlib.md`, "List"): the same length, and equal elements in order.
    pub(super) fn list_eq(
        &mut self,
        l: PointerValue<'ctx>,
        r: PointerValue<'ctx>,
        elem: &Ty,
        alloc: &burdin_tir::Alloc,
    ) -> GenResult<IntValue<'ctx>> {
        let left = self.seq_load_at(l, alloc)?;
        let right = self.seq_load_at(r, alloc)?;
        let bool_ty = self.cx.bool_type();
        let same_len = self.builder.build_int_compare(IntPredicate::EQ, left.len, right.len, "").map_err(llvm_err)?;
        // A zero-sized element has nothing to compare, so equal lengths are equal lists.
        if self.llvm_ty(elem).is_none() {
            return Ok(same_len);
        }
        let result = self.alloca(bool_ty.into(), "list.eq.result")?;
        self.builder.build_store(result, same_len).map_err(llvm_err)?;
        let scan = self.new_block("list.eq.scan");
        let end = self.new_block("list.eq.end");
        self.builder.build_conditional_branch(same_len, scan, end).map_err(llvm_err)?;
        self.builder.position_at_end(scan);
        let index = self.alloca(self.usize_ty.into(), "list.eq.i")?;
        self.builder.build_store(index, self.usize_zero()).map_err(llvm_err)?;
        let header = self.new_block("list.eq.cond");
        let body = self.new_block("list.eq.body");
        let next = self.new_block("list.eq.next");
        let fail = self.new_block("list.eq.fail");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::ULT, i, left.len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body, end).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let a = self.elem_ptr_at(left.data, elem, i).expect("a list element has a type");
        let b = self.elem_ptr_at(right.data, elem, i).expect("a list element has a type");
        let same = self.eq_at(a, b, elem)?;
        self.builder.build_conditional_branch(same, next, fail).map_err(llvm_err)?;
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
