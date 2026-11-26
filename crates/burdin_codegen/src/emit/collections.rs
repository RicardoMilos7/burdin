//! `List<T>` and `Map<K, V>`: their layout, growth, indexing, and methods.
//!
//! Both have the three-word layout `{ ptr, len, cap }`, like a `String`. A list buffer holds `cap` elements of
//! `T`. A map buffer is an open-addressing hash table: `cap` slots (a power of two) of `(key, value, hash)` entries,
//! where a zero hash marks an empty slot and `len` counts the live entries. A key is placed at `hash & (cap - 1)`
//! and probes forward; a removal shifts the rest of its probe cluster back, so there are no tombstones. The table
//! grows (and rehashes) when the next insert would pass a 3/4 load factor. Iteration scans the slots, so its order
//! is unspecified, as the standard library says.

use super::fallible::{TRY_OK, TRY_OUT_OF_MEMORY, TRY_OVERFLOW};
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
    fn seq_alloc(&self, ty: &Ty) -> burdin_tir::Alloc {
        if let Some((_, _, alloc)) = self.map_info(ty) {
            return alloc;
        }
        match ty {
            Ty::MapIntoIter(_, _, alloc) => alloc.clone(),
            _ => burdin_tir::Alloc::Global,
        }
    }

    /// The `{ ptr, len, cap }` words of a list or map place.
    fn seq_load(&mut self, e: &'a tir::Expr) -> GenResult<Option<Seq<'ctx>>> {
        let Some(addr) = self.place_ptr(e)? else { return Ok(None) };
        let alloc = self.seq_alloc(&e.ty);
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

    /// The methods of `List<T>` and `Map<K, V>`.
    pub(super) fn collection_method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let map = self.map_info(&args[0].ty);
        match (&args[0].ty, &map, m) {
            (_, Some((key, value, _)), Method::MapInsert | Method::MapGetOrInsert | Method::MapGetOr) => {
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
            (_, Some((key, value, _)), Method::MapGet | Method::MapGetMut) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let view = Ty::Ref { mutable: m == Method::MapGetMut, inner: Box::new(value_ty.clone()) };
                let slot = self.option_slot(ty, "get.out")?;
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
                self.load_option(slot, ty)
            }
            (_, Some((key, value, _)), Method::MapRemove) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let slot = self.option_slot(ty, "remove.out")?;
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
                self.load_option(slot, ty)
            }
            (_, Some((key, value, _)), Method::MapContainsKey) => {
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
            (_, Some((key, value, _)), Method::MapRemoveEntry) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k_slot = self.place_ptr(&args[1])?;
                let slot = self.option_slot(ty, "remove_entry.out")?;
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
                self.load_option(slot, ty)
            }
            // `retain(f)`: walk the slots and drop every entry `f` rejects. A removal shifts the rest of the
            // probe cluster back, so the same slot is examined again.
            (_, Some((key, value, _)), Method::MapRetain) => {
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
            (_, Some((key, value, _)), Method::MapReserve) => {
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
            // `try_reserve(additional)`: the fallible `reserve`, which leaves the map unchanged on failure.
            (_, Some((key, value, alloc)), Method::MapTryReserve) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let alloc = alloc.clone();
                let Some(addr) = self.place_ptr(&args[0])? else { return Ok(None) };
                let additional = self.expr_mode(&args[1], Mode::Use)?;
                let additional = self.need_int(additional, &Ty::Int(tir::IntTy::Usize));
                let status = self.map_try_reserve_at(addr, &alloc, &key_ty, &value_ty, additional, span)?;
                Ok(Some(self.try_result(status, None, ty)?))
            }
            (_, Some((key, value, _)), Method::MapShrinkToFit) => {
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
            (_, Some((key, value, _)), Method::Clear) => {
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
            _ => Err(format!("unsupported collection method {m:?} on `{}`", args[0].ty)),
        }
    }

    /// `try_reserve(additional)` on a map and the fallible `with_capacity` (AL11): makes the slot table hold
    /// `additional` more entries without passing the load factor, and returns the status word. The map at `addr`
    /// changes only when the status is `TRY_OK`.
    pub(super) fn map_try_reserve_at(
        &mut self,
        addr: PointerValue<'ctx>,
        alloc: &burdin_tir::Alloc,
        key: &Ty,
        value: &Ty,
        additional: IntValue<'ctx>,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        let usize_ty = self.usize_ty;
        let i32_ty = self.cx.i32_type();
        let status = self.alloca(i32_ty.into(), "try.status")?;
        self.builder.build_store(status, i32_ty.const_int(TRY_OK, false)).map_err(llvm_err)?;
        let mut seq = self.seq_load_at(addr, alloc)?;
        let done = self.new_block("try.map.end");
        let overflow = self.new_block("try.map.overflow");
        let fits = self.new_block("try.map.fits");
        let grow = self.new_block("try.map.grow");
        let oom = self.new_block("try.map.oom");
        let took = self.new_block("try.map.took");

        // `len + additional` must not wrap, and the slot count it needs must stay within the map limit.
        let room = self.builder.build_int_sub(usize_ty.const_all_ones(), seq.len, "").map_err(llvm_err)?;
        let too_many = self.builder.build_int_compare(IntPredicate::UGT, additional, room, "").map_err(llvm_err)?;
        let needed = self.builder.build_int_add(seq.len, additional, "").map_err(llvm_err)?;
        let limit = usize_ty.const_int(1u64 << 62, false);
        let over_limit = self.builder.build_int_compare(IntPredicate::UGT, needed, limit, "").map_err(llvm_err)?;
        let not_fit = self.builder.build_or(too_many, over_limit, "").map_err(llvm_err)?;
        let check_cap = self.new_block("try.map.cap");
        self.builder.build_conditional_branch(not_fit, overflow, check_cap).map_err(llvm_err)?;
        self.builder.position_at_end(check_cap);
        let cap = self.map_capacity_loop(needed)?;
        let over = self.builder.build_int_compare(IntPredicate::UGT, cap, seq.cap, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(over, grow, fits).map_err(llvm_err)?;
        self.builder.position_at_end(fits);
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;

        // The table's bytes must fit in a `usize`; then the new table is allocated before anything moves.
        self.builder.position_at_end(grow);
        let entry_size = self.map_entry_size(key, value);
        let entry_limit = usize_ty.const_int(u64::MAX / entry_size, false);
        let wide = self.builder.build_int_compare(IntPredicate::UGT, cap, entry_limit, "").map_err(llvm_err)?;
        let fit_bytes = self.new_block("try.map.bytes");
        self.builder.build_conditional_branch(wide, overflow, fit_bytes).map_err(llvm_err)?;
        self.builder.position_at_end(fit_bytes);
        let bytes = self.builder.build_int_mul(cap, usize_ty.const_int(entry_size, false), "").map_err(llvm_err)?;
        let entry = Self::map_entry_ty(key, value);
        let align = self.value_align(&entry);
        let base = self.try_alloc_in(alloc, seq.alloc_addr, bytes, align, Some(span))?;
        let refused = self.builder.build_is_null(base, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(refused, oom, took).map_err(llvm_err)?;
        self.builder.position_at_end(took);
        self.map_rehash_fill(&mut seq, key, value, base, cap)?;
        self.seq_store(&seq)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(oom);
        self.builder.build_store(status, i32_ty.const_int(TRY_OUT_OF_MEMORY, false)).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(overflow);
        self.builder.build_store(status, i32_ty.const_int(TRY_OVERFLOW, false)).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;

        self.builder.position_at_end(done);
        Ok(self.builder.build_load(i32_ty, status, "").map_err(llvm_err)?.into_int_value())
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
        self.map_capacity_loop(needed)
    }

    /// The slot count [`Codegen::map_capacity_for`] computes, for a `needed` the caller has checked against the map
    /// limit of 2^62 entries.
    fn map_capacity_loop(&mut self, needed: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
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
        let size = self.map_entry_size(key, value);
        let bytes = self.builder.build_int_mul(new_cap, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
        let align = self.value_align(&entry);
        let base = self.alloc_in(&seq.alloc, seq.alloc_addr, bytes, align, Some(span))?;
        self.map_rehash_fill(seq, key, value, base, new_cap)
    }

    /// Moves a map's live entries into `base`, a fresh slot table of `new_cap` slots that the caller allocated for
    /// them, and frees the old table. `seq` then holds the new buffer and capacity.
    pub(super) fn map_rehash_fill(
        &mut self,
        seq: &mut Seq<'ctx>,
        key: &Ty,
        value: &Ty,
        base: PointerValue<'ctx>,
        new_cap: IntValue<'ctx>,
    ) -> GenResult<()> {
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let size = self.map_entry_size(key, value);
        let one = self.usize_ty.const_int(1, false);
        let zero = self.usize_ty.const_zero();
        let align = self.value_align(&entry);
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
