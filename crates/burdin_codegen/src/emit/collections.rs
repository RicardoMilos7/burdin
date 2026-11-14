//! `List<T>` and `Map<K, V>`: their layout, growth, indexing, and methods.
//!
//! Both have the three-word layout `{ ptr, len, cap }`, like a `String`. A list buffer holds `cap` elements of
//! `T`; a map buffer holds `cap` entry structs `{ K, V }`. A map is a linear sequence scanned with `==`: stage 0
//! only needs the observable API, and a hash table can replace it without changing the type.

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

    /// The address of element `index` in `seq` (no bounds check).
    fn elem_ptr(&mut self, seq: &Seq<'ctx>, elem: &Ty, index: IntValue<'ctx>) -> Option<PointerValue<'ctx>> {
        let t = self.llvm_ty(elem)?;
        // SAFETY (LLVM): the caller checked `index < len`.
        unsafe { self.builder.build_in_bounds_gep(t, seq.data, &[index], "elem") }.ok()
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
                let entry = Ty::Tuple(vec![key_ty.clone(), value_ty.clone()]);
                let k = self.expr_mode(&args[1], Mode::Move)?;
                let v = self.expr_mode(&args[2], Mode::Move)?;
                let k_slot = self.spill(k, &key_ty)?;
                let v_slot = self.spill(v, &value_ty)?;
                let at = self.map_find_index(&seq, &entry, &key_ty, k_slot)?;
                let found = self.builder.build_int_compare(IntPredicate::ULT, at, seq.len, "").map_err(llvm_err)?;
                let found_bb = self.new_block("map.found");
                let insert_bb = self.new_block("map.insert");
                let end = self.new_block("map.end");
                self.builder.build_conditional_branch(found, found_bb, insert_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                if let Some(p) = self.elem_ptr(&seq, &entry, at) {
                    let et = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                    let old = self.builder.build_struct_gep(et, p, 1, "").map_err(llvm_err)?;
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
                self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                self.builder.position_at_end(insert_bb);
                self.seq_grow(&mut seq, &entry)?;
                if let Some(p) = self.elem_ptr(&seq, &entry, seq.len) {
                    let et = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                    let kf = self.builder.build_struct_gep(et, p, 0, "").map_err(llvm_err)?;
                    let vf = self.builder.build_struct_gep(et, p, 1, "").map_err(llvm_err)?;
                    if let Some(k_slot) = k_slot {
                        let kt = self.llvm_ty(&key_ty).unwrap_or_else(|| self.cx.bool_type().into());
                        let kv = self.builder.build_load(kt, k_slot, "").map_err(llvm_err)?;
                        self.builder.build_store(kf, kv).map_err(llvm_err)?;
                    }
                    if let Some(v_slot) = v_slot {
                        let vt = self.llvm_ty(&value_ty).unwrap_or_else(|| self.cx.bool_type().into());
                        let vv = self.builder.build_load(vt, v_slot, "").map_err(llvm_err)?;
                        self.builder.build_store(vf, vv).map_err(llvm_err)?;
                    }
                }
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
                let entry = Ty::Tuple(vec![key_ty.clone(), value_ty.clone()]);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
                let at = self.map_find_index(&seq, &entry, &key_ty, k_slot)?;
                let found = self.builder.build_int_compare(IntPredicate::ULT, at, seq.len, "").map_err(llvm_err)?;
                let view = Ty::Ref { mutable: false, inner: Box::new(value_ty.clone()) };
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "get.out")?),
                    None => None,
                };
                let found_bb = self.new_block("map.get.found");
                let none_bb = self.new_block("map.get.none");
                let end = self.new_block("map.get.end");
                self.builder.build_conditional_branch(found, found_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let p = self.elem_ptr(&seq, &entry, at).and_then(|p| {
                    self.builder.build_struct_gep(self.llvm_ty(&entry)?.into_struct_type(), p, 1, "").ok()
                });
                if let (Some(slot), Some(_), Some(p)) = (slot, self.llvm_ty(ty), p) {
                    let some = self.some_value(&view, Some(p.into()));
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
                let entry = Ty::Tuple(vec![key_ty.clone(), value_ty.clone()]);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
                let at = self.map_find_index(&seq, &entry, &key_ty, k_slot)?;
                let found = self.builder.build_int_compare(IntPredicate::ULT, at, seq.len, "").map_err(llvm_err)?;
                let slot = match self.llvm_ty(ty) {
                    Some(t) => Some(self.alloca(t, "remove.out")?),
                    None => None,
                };
                let found_bb = self.new_block("map.remove.found");
                let none_bb = self.new_block("map.remove.none");
                let end = self.new_block("map.remove.end");
                self.builder.build_conditional_branch(found, found_bb, none_bb).map_err(llvm_err)?;
                self.builder.position_at_end(found_bb);
                let et = self.llvm_ty(&entry).expect("an entry has a type").into_struct_type();
                let p = self.elem_ptr(&seq, &entry, at);
                let mut value = None;
                if let Some(p) = p {
                    let vf = self.builder.build_struct_gep(et, p, 1, "").map_err(llvm_err)?;
                    if let Some(vt) = self.llvm_ty(&value_ty) {
                        value = Some(self.builder.build_load(vt, vf, "").map_err(llvm_err)?);
                    }
                    let kf = self.builder.build_struct_gep(et, p, 0, "").map_err(llvm_err)?;
                    self.drop_value(kf, &key_ty)?;
                    let after =
                        self.builder.build_int_add(at, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                    let tail = self.builder.build_int_sub(seq.len, after, "").map_err(llvm_err)?;
                    let bytes = self
                        .builder
                        .build_int_mul(tail, self.usize_ty.const_int(self.elem_size(&entry), false), "")
                        .map_err(llvm_err)?;
                    if let Some(src) = self.elem_ptr(&seq, &entry, after) {
                        self.builder.build_memmove(p, 1, src, 1, bytes).map_err(llvm_err)?;
                    }
                }
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
                let entry = Ty::Tuple(vec![key_ty.clone(), value_ty.clone()]);
                let k = self.expr_mode(&args[1], Mode::Use)?;
                let k_slot = self.spill(k, &key_ty)?;
                self.map_contains(&seq, &entry, &key_ty, k_slot)
            }
            (Ty::Map(key, value), Method::Clear) => {
                let (key_ty, value_ty) = ((**key).clone(), (**value).clone());
                let Some(mut seq) = self.seq_load(&args[0])? else { return Ok(None) };
                let entry = Ty::Tuple(vec![key_ty, value_ty]);
                self.drop_sequence_elements(&seq, &entry)?;
                seq.len = self.usize_zero();
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

    /// The index of the entry whose key equals the value at `key_ptr`, or `len` when there is none.
    fn map_find_index(
        &mut self,
        seq: &Seq<'ctx>,
        entry: &Ty,
        key_ty: &Ty,
        key_ptr: Option<PointerValue<'ctx>>,
    ) -> GenResult<IntValue<'ctx>> {
        let et = self.llvm_ty(entry).expect("an entry has a type").into_struct_type();
        let result = self.alloca(self.usize_ty.into(), "map.at")?;
        self.builder.build_store(result, seq.len).map_err(llvm_err)?;
        let Some(key_t) = self.llvm_ty(key_ty) else { return Ok(seq.len) };
        let Some(key_ptr) = key_ptr else { return Ok(seq.len) };
        let index = self.alloca(self.usize_ty.into(), "map.i")?;
        self.builder.build_store(index, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("map.find.cond");
        let body = self.new_block("map.find.body");
        let next = self.new_block("map.find.next");
        let exit = self.new_block("map.find.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, seq.len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        let p = unsafe { self.builder.build_in_bounds_gep(et, seq.data, &[i], "") }.map_err(llvm_err)?;
        let _ = key_t;
        let kf = self.builder.build_struct_gep(et, p, 0, "").map_err(llvm_err)?;
        let eq = self.eq_at(kf, key_ptr, key_ty)?;
        self.builder.build_conditional_branch(eq, exit, next).map_err(llvm_err)?;
        self.builder.position_at_end(next);
        let next_i = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next_i).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        let found = self.builder.build_load(self.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
        let at = self
            .builder
            .build_select(
                self.builder.build_int_compare(IntPredicate::ULT, found, seq.len, "").map_err(llvm_err)?,
                found,
                seq.len,
                "",
            )
            .map_err(llvm_err)?;
        self.builder.build_store(result, at).map_err(llvm_err)?;
        Ok(self.builder.build_load(self.usize_ty, result, "").map_err(llvm_err)?.into_int_value())
    }

    /// Whether some entry's key equals the value at `key_ptr`.
    fn map_find(
        &mut self,
        seq: &Seq<'ctx>,
        entry: &Ty,
        key_ty: &Ty,
        key_ptr: Option<PointerValue<'ctx>>,
    ) -> GenResult<IntValue<'ctx>> {
        let at = self.map_find_index(seq, entry, key_ty, key_ptr)?;
        self.builder.build_int_compare(IntPredicate::ULT, at, seq.len, "").map_err(llvm_err)
    }

    fn map_contains(
        &mut self,
        seq: &Seq<'ctx>,
        entry: &Ty,
        key_ty: &Ty,
        key_ptr: Option<PointerValue<'ctx>>,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let found = self.map_find(seq, entry, key_ty, key_ptr)?;
        Ok(Some(found.into()))
    }
}
