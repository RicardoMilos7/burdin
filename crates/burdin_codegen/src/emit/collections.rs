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
use burdin_tir::{self as tir, Method, Ty};
use inkwell::values::{BasicValueEnum, IntValue, PointerValue, StructValue};
use inkwell::{AddressSpace, IntPredicate};

/// The three words of a list or map, loaded from its place.
struct Seq<'ctx> {
    addr: PointerValue<'ctx>,
    data: PointerValue<'ctx>,
    len: IntValue<'ctx>,
    cap: IntValue<'ctx>,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    fn ptr_ty(&self) -> inkwell::types::PointerType<'ctx> {
        self.cx.ptr_type(AddressSpace::default())
    }

    fn seq_field(&mut self, addr: PointerValue<'ctx>, index: u32, name: &str) -> GenResult<PointerValue<'ctx>> {
        self.builder.build_struct_gep(self.string_ty, addr, index, name).map_err(llvm_err)
    }

    /// The `{ ptr, len, cap }` words of a list or map place.
    fn seq_load(&mut self, e: &'a tir::Expr) -> GenResult<Option<Seq<'ctx>>> {
        let Some(addr) = self.place_ptr(e)? else { return Ok(None) };
        let ptr_ty = self.ptr_ty();
        let f0 = self.seq_field(addr, 0, "seq.ptr")?;
        let f1 = self.seq_field(addr, 1, "seq.len")?;
        let f2 = self.seq_field(addr, 2, "seq.cap")?;
        let data = self.builder.build_load(ptr_ty, f0, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, f1, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, f2, "").map_err(llvm_err)?.into_int_value();
        Ok(Some(Seq { addr, data, len, cap }))
    }

    fn seq_store(&mut self, seq: &Seq<'ctx>) -> GenResult<()> {
        let f0 = self.seq_field(seq.addr, 0, "seq.ptr")?;
        let f1 = self.seq_field(seq.addr, 1, "seq.len")?;
        let f2 = self.seq_field(seq.addr, 2, "seq.cap")?;
        self.builder.build_store(f0, seq.data).map_err(llvm_err)?;
        self.builder.build_store(f1, seq.len).map_err(llvm_err)?;
        self.builder.build_store(f2, seq.cap).map_err(llvm_err)?;
        Ok(())
    }

    /// The byte size of one element, at least one so that growing always allocates.
    fn elem_size(&self, elem: &Ty) -> u64 {
        self.llvm_ty(elem).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1)
    }

    fn usize_zero(&self) -> IntValue<'ctx> {
        self.usize_ty.const_zero()
    }

    /// Grows the buffer to hold at least one more element.
    fn seq_grow(&mut self, seq: &mut Seq<'ctx>, elem: &Ty) -> GenResult<()> {
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
        let bytes = self
            .builder
            .build_int_mul(at_least_four, self.usize_ty.const_int(self.elem_size(elem), false), "")
            .map_err(llvm_err)?;
        let call = self
            .builder
            .build_call(self.rt.realloc, &[seq.data.into(), bytes.into()], "seq.buffer")
            .map_err(llvm_err)?;
        let data = call.try_as_basic_value().basic().expect("realloc returns a pointer").into_pointer_value();
        self.builder.build_unconditional_branch(done).map_err(llvm_err)?;
        self.builder.position_at_end(done);
        let data_phi = self.builder.build_phi(self.ptr_ty(), "seq.data").map_err(llvm_err)?;
        data_phi.add_incoming(&[(&seq.data, before), (&data, grow)]);
        let cap_phi = self.builder.build_phi(self.usize_ty, "seq.cap").map_err(llvm_err)?;
        cap_phi.add_incoming(&[(&seq.cap, before), (&at_least_four, grow)]);
        seq.data = data_phi.as_basic_value().into_pointer_value();
        seq.cap = cap_phi.as_basic_value().into_int_value();
        Ok(())
    }

    /// The address of element `index` in a buffer of `elem` (no bounds check).
    fn elem_ptr_at(
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
            (Ty::List(elem), Method::Push) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.seq_grow(&mut seq, &elem)?;
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
            (Ty::List(elem), Method::ListPop) => {
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
            (Ty::List(elem), Method::ListLast) => {
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
            (Ty::List(elem), Method::ListInsert) => {
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
                self.seq_grow(&mut seq, &elem)?;
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
            (Ty::List(elem), Method::ListRemove) => {
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
            (Ty::List(elem), Method::ListContains) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let key = self.expr_mode(&args[1], Mode::Use)?;
                let key_ptr = self.spill(key, &elem)?;
                self.sequence_contains(seq.data, seq.len, &elem, key_ptr)
            }
            (Ty::List(elem), Method::ListSort) => {
                let elem = (**elem).clone();
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.sort_sequence(&seq, &elem)
            }
            (Ty::List(elem), Method::Clear) => {
                let elem = (**elem).clone();
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                self.drop_sequence_elements(&seq, &elem)?;
                seq.len = self.usize_zero();
                self.seq_store(&seq)?;
                Ok(None)
            }
            (Ty::Map(key, value), Method::MapInsert) => {
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
                self.map_grow(&mut seq, &key_ty, &value_ty)?;
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
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, hash, k_slot)?;
                let found_bb = self.new_block("map.found");
                let insert_bb = self.new_block("map.insert");
                let end = self.new_block("map.end");
                self.builder.build_conditional_branch(found, found_bb, insert_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let entry_t = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.map_slot(&seq, &entry, at)?;
                let old = self.map_field(entry_t, p, 1, "map.value")?;
                self.drop_value(old, &value_ty)?;
                if let Some(v_slot) = v_slot {
                    let vt = self.llvm_ty(&value_ty).unwrap_or_else(|| self.cx.bool_type().into());
                    let nv = self.builder.build_load(vt, v_slot, "").map_err(llvm_err)?;
                    self.builder.build_store(old, nv).map_err(llvm_err)?;
                }
                // The map already owns an equal key, so the incoming one is dropped.
                if let Some(k_slot) = k_slot {
                    self.drop_value(k_slot, &key_ty)?;
                }
                // Growing happens before the probe, so even a replacement must publish the new buffer.
                self.seq_store(&seq)?;
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
                if let Some(v_slot) = v_slot {
                    let vf = self.map_field(entry_t, p, 1, "map.value")?;
                    let vt = self.llvm_ty(&value_ty).unwrap_or_else(|| self.cx.bool_type().into());
                    let vv = self.builder.build_load(vt, v_slot, "").map_err(llvm_err)?;
                    self.builder.build_store(vf, vv).map_err(llvm_err)?;
                }
                let hf = self.map_field(entry_t, p, 2, "map.hash")?;
                self.builder.build_store(hf, hash).map_err(llvm_err)?;
                seq.len =
                    self.builder.build_int_add(seq.len, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                self.seq_store(&seq)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(None)
            }
            (Ty::Map(key, value), Method::MapGet) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
                let view = Ty::Ref { mutable: false, inner: Box::new(value_ty.clone()) };
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
                let hash = self.map_key_hash(k_slot, &key_ty)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, hash, k_slot)?;
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
            (Ty::Map(key, value), Method::MapRemove) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
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
                let hash = self.map_key_hash(k_slot, &key_ty)?;
                let (found, at) = self.map_probe(&seq, &entry, &key_ty, hash, k_slot)?;
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
            (Ty::Map(key, value), Method::MapContainsKey) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Self::map_entry_ty(&key_ty, &value_ty);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
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
                let hash = self.map_key_hash(k_slot, &key_ty)?;
                let (found, _) = self.map_probe(&seq, &entry, &key_ty, hash, k_slot)?;
                self.builder.build_store(result, found).map_err(llvm_err)?;
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(end);
                Ok(Some(self.builder.build_load(self.cx.bool_type(), result, "").map_err(llvm_err)?))
            }
            (Ty::Map(key, value), Method::Clear) => {
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
        if !self.needs_drop(elem) {
            return Ok(());
        }
        let Some(elem_t) = self.llvm_ty(elem) else { return Ok(()) };
        let index = self.alloca(self.usize_ty.into(), "clear.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("clear.cond");
        let body = self.new_block("clear.body");
        let exit = self.new_block("clear.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, seq.len, "").map_err(llvm_err)?;
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

    /// Finds the entry whose key equals the value at `key_ptr`: the slot and whether it was found. When the key is
    /// absent, the slot is the empty one where it belongs. `seq.cap` must be a nonzero power of two.
    fn map_probe(
        &mut self,
        seq: &Seq<'ctx>,
        entry: &Ty,
        key_ty: &Ty,
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
            Some(key_ptr) => self.eq_at(key_field, key_ptr, key_ty)?,
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

    /// Doubles a map's slot table and rehashes its entries (an empty map gets eight slots); `seq` then holds the new
    /// buffer and capacity. The old buffer is freed without dropping its entries: ownership moved into the new table.
    fn map_grow(&mut self, seq: &mut Seq<'ctx>, key: &Ty, value: &Ty) -> GenResult<()> {
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let size = self.map_entry_size(key, value);
        let one = self.usize_ty.const_int(1, false);
        let zero = self.usize_ty.const_zero();
        let doubled = self.builder.build_int_mul(seq.cap, self.usize_ty.const_int(2, false), "").map_err(llvm_err)?;
        let no_table = self.builder.build_int_compare(IntPredicate::EQ, seq.cap, zero, "").map_err(llvm_err)?;
        let new_cap = self
            .builder
            .build_select(no_table, self.usize_ty.const_int(8, false), doubled, "")
            .map_err(llvm_err)?
            .into_int_value();
        let bytes = self.builder.build_int_mul(new_cap, self.usize_ty.const_int(size, false), "").map_err(llvm_err)?;
        let call = self.builder.build_call(self.rt.alloc, &[bytes.into()], "map.slots").map_err(llvm_err)?;
        let base = call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value();
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

        self.builder.build_call(self.rt.free, &[seq.data.into()], "").map_err(llvm_err)?;
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

    /// Drops a map: every live entry, then the slot table.
    pub(super) fn drop_map(&mut self, addr: PointerValue<'ctx>, key: &Ty, value: &Ty) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let data_field = self.seq_field(addr, 0, "map.ptr")?;
        let len_field = self.seq_field(addr, 1, "map.len")?;
        let cap_field = self.seq_field(addr, 2, "map.cap")?;
        let data = self.builder.build_load(ptr_ty, data_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, cap_field, "").map_err(llvm_err)?.into_int_value();
        let seq = Seq { addr, data, len, cap };
        self.map_drop_entries(&seq, key, value)?;
        self.builder.build_call(self.rt.free, &[data.into()], "").map_err(llvm_err)?;
        Ok(())
    }

    /// Deep-copies a map: clones the live entries and preserves the slot table's shape (and hashes).
    pub(super) fn clone_map(
        &mut self,
        dst: PointerValue<'ctx>,
        src: PointerValue<'ctx>,
        key: &Ty,
        value: &Ty,
    ) -> GenResult<()> {
        let ptr_ty = self.ptr_ty();
        let entry = Self::map_entry_ty(key, value);
        let entry_t = self.llvm_ty(&entry).expect("a map entry has a type").into_struct_type();
        let field = |cg: &mut Self, base: PointerValue<'ctx>, i: u32| -> GenResult<PointerValue<'ctx>> {
            cg.builder.build_struct_gep(cg.string_ty, base, i, "").map_err(llvm_err)
        };
        let src_ptr_field = field(self, src, 0)?;
        let src_len_field = field(self, src, 1)?;
        let src_cap_field = field(self, src, 2)?;
        let base = self.builder.build_load(ptr_ty, src_ptr_field, "").map_err(llvm_err)?.into_pointer_value();
        let len = self.builder.build_load(self.usize_ty, src_len_field, "").map_err(llvm_err)?.into_int_value();
        let cap = self.builder.build_load(self.usize_ty, src_cap_field, "").map_err(llvm_err)?.into_int_value();
        let bytes = self
            .builder
            .build_int_mul(cap, self.usize_ty.const_int(self.map_entry_size(key, value), false), "")
            .map_err(llvm_err)?;
        let call = self.builder.build_call(self.rt.alloc, &[bytes.into()], "map.copy").map_err(llvm_err)?;
        let new_base = call.try_as_basic_value().basic().expect("alloc returns a pointer").into_pointer_value();
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
        let dst_ptr_field = field(self, dst, 0)?;
        let dst_len_field = field(self, dst, 1)?;
        let dst_cap_field = field(self, dst, 2)?;
        self.builder.build_store(dst_ptr_field, new_base).map_err(llvm_err)?;
        self.builder.build_store(dst_len_field, len).map_err(llvm_err)?;
        self.builder.build_store(dst_cap_field, cap).map_err(llvm_err)?;
        Ok(())
    }
}
