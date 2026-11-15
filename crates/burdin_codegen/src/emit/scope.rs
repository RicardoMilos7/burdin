//! Statements, scopes, and temporaries: where values are dropped.

use super::pattern::collect_bindings;
use super::place::is_place;
use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, LocalKind, Pat, Ty};
use inkwell::basic_block::BasicBlock;
use inkwell::values::{BasicValueEnum, IntValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(super) fn block(&mut self, b: &'a tir::Block, tail_mode: Mode) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        self.scopes.push(Vec::new());
        for s in &b.stmts {
            self.temps.push(Vec::new());
            match s {
                tir::Stmt::Let(Pat::Binding(local, _), init, None) => {
                    let v = self.expr_mode(init, Mode::Move)?;
                    if let (Some(slot), Some(v)) = (self.slots[local.0], v) {
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                    }
                    self.set_initialized(*local, &[]);
                    self.scopes.last_mut().expect("a scope is open").push(*local);
                }
                tir::Stmt::Let(Pat::Wild, init, None) => {
                    // `let _ = e` binds nothing. A place in `e` is only read; an rvalue becomes a temporary (DR5).
                    let v = self.expr_mode(init, Mode::Use)?;
                    if !is_place(init) {
                        self.add_temp_value(v, &init.ty)?;
                    }
                }
                tir::Stmt::Let(pat, init, else_block) => {
                    let v = self.expr_mode(init, Mode::Move)?;
                    let place = self.spill(v, &init.ty)?;
                    if let Some(else_block) = else_block {
                        let ok = self.pat_test(pat, place, &init.ty)?;
                        let bind_bb = self.new_block("let.bind");
                        let else_bb = self.new_block("let.else");
                        self.builder.build_conditional_branch(ok, bind_bb, else_bb).map_err(llvm_err)?;
                        self.builder.position_at_end(else_bb);
                        // The checker proved the block diverges.
                        self.block(else_block, Mode::Use)?;
                        self.builder.build_unreachable().map_err(llvm_err)?;
                        self.builder.position_at_end(bind_bb);
                    }
                    self.pat_bind(pat, place, &init.ty, None, &mut Vec::new())?;
                    let mut bound = Vec::new();
                    collect_bindings(pat, &mut bound);
                    for id in bound {
                        self.set_initialized(id, &[]);
                        self.scopes.last_mut().expect("a scope is open").push(id);
                    }
                }
                tir::Stmt::Expr(e) => {
                    let v = self.expr_mode(e, Mode::Move)?;
                    // The value of an expression statement is a temporary of the statement (DR5).
                    if self.needs_drop(&e.ty) {
                        self.add_temp_value(v, &e.ty)?;
                    }
                }
            }
            let depth = self.temps.len() - 1;
            self.free_temps_from(depth)?;
            self.temps.pop();
        }
        // The tail expression's temporaries drop when it finishes, before the block's locals (DR5).
        self.temps.push(Vec::new());
        let result = match &b.tail {
            Some(e) => self.expr_mode(e, tail_mode)?,
            None => None,
        };
        let depth = self.temps.len() - 1;
        self.free_temps_from(depth)?;
        self.temps.pop();
        let index = self.scopes.len() - 1;
        self.drop_scope(index)?;
        self.scopes.pop();
        Ok(result)
    }

    /// Drops the locals of the scope at `index`, in reverse declaration order (DR2).
    pub(super) fn drop_scope(&mut self, index: usize) -> GenResult<()> {
        let locals = self.scopes[index].clone();
        for id in locals.into_iter().rev() {
            self.drop_local(id)?;
        }
        Ok(())
    }

    pub(super) fn drop_local(&mut self, id: tir::LocalId) -> GenResult<()> {
        // The move checker proved the whole local is moved before every way its scope can end.
        if self.dead_locals.contains(&id) {
            return Ok(());
        }
        let local = &self.locals[id.0];
        if local.by_ref || matches!(local.kind, LocalKind::BorrowedParam | LocalKind::View) {
            return Ok(());
        }
        let ty = local.ty.clone();
        if !self.needs_drop(&ty) {
            return Ok(());
        }
        self.drop_at(id, &[], &ty)
    }

    /// Drops the value at `local` + `path`, honoring the drop flags of every tracked path inside it (DR7).
    pub(super) fn drop_at(&mut self, id: tir::LocalId, path: &[usize], ty: &Ty) -> GenResult<()> {
        let Some(addr) = self.local_address(id, path)? else { return Ok(()) };
        self.drop_at_address(id, path, ty, addr)
    }

    /// Drops the value of type `ty` at `addr`, which is `local` + `path`; the flags of the tracked parts below
    /// the path decide which parts are still initialized.
    pub(super) fn drop_at_address(
        &mut self,
        id: tir::LocalId,
        path: &[usize],
        ty: &Ty,
        addr: PointerValue<'ctx>,
    ) -> GenResult<()> {
        let flag = self.flag_for(id, path);
        if !self.has_inner_flag(id, path) {
            return match flag {
                None => self.drop_value(addr, ty),
                Some(flag) => {
                    let alive = self.builder.build_load(self.cx.bool_type(), flag, "init").map_err(llvm_err)?;
                    self.if_then(alive.into_int_value(), "drop", move |cg| cg.drop_value(addr, ty))
                }
            };
        }
        let run = match flag {
            None => None,
            Some(flag) => {
                Some(self.builder.build_load(self.cx.bool_type(), flag, "init").map_err(llvm_err)?.into_int_value())
            }
        };
        let emit = |cg: &mut Self| -> GenResult<()> {
            match ty {
                Ty::Struct(_) | Ty::Tuple(_) => {
                    let t = cg.llvm_ty(ty).expect("an aggregate has a type").into_struct_type();
                    for (i, fty) in cg.field_tys(ty).iter().enumerate() {
                        let mut child = path.to_vec();
                        child.push(i);
                        if cg.needs_drop(fty) || cg.has_inner_flag(id, &child) || cg.flag_for(id, &child).is_some() {
                            let p = cg.builder.build_struct_gep(t, addr, i as u32, "").map_err(llvm_err)?;
                            cg.drop_at_address(id, &child, fty, p)?;
                        }
                    }
                    Ok(())
                }
                Ty::Enum(a) => {
                    let et = cg.enum_ty(a.id);
                    let tag_ptr = cg.builder.build_struct_gep(et, addr, 0, "tag").map_err(llvm_err)?;
                    let tag = cg.builder.build_load(cg.cx.i32_type(), tag_ptr, "").map_err(llvm_err)?.into_int_value();
                    let end = cg.new_block("enum.drop.end");
                    let mut cases = Vec::new();
                    for (vi, v) in cg.program.enums[a.id].variants.iter().enumerate() {
                        if v.fields.is_empty() {
                            continue;
                        }
                        cases.push((vi, cg.new_block("enum.drop")));
                    }
                    let switch_cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> =
                        cases.iter().map(|(vi, bb)| (cg.cx.i32_type().const_int(*vi as u64, false), *bb)).collect();
                    cg.builder.build_switch(tag, end, &switch_cases).map_err(llvm_err)?;
                    for (vi, case) in cases {
                        cg.builder.position_at_end(case);
                        let payload = cg.builder.build_struct_gep(et, addr, 1, "").map_err(llvm_err)?;
                        let vt = cg.variant_ty(a.id, vi);
                        for (fi, fty) in cg.variant_tys(a.id, vi).iter().enumerate() {
                            let mut child = path.to_vec();
                            child.push(vi);
                            child.push(fi);
                            if cg.needs_drop(fty) || cg.has_inner_flag(id, &child) || cg.flag_for(id, &child).is_some()
                            {
                                let p = cg.builder.build_struct_gep(vt, payload, fi as u32, "").map_err(llvm_err)?;
                                cg.drop_at_address(id, &child, fty, p)?;
                            }
                        }
                        cg.builder.build_unconditional_branch(end).map_err(llvm_err)?;
                    }
                    cg.builder.position_at_end(end);
                    Ok(())
                }
                Ty::Option(inner_ty) => {
                    let ot = cg.option_ty(inner_ty);
                    let tag_ptr = cg.builder.build_struct_gep(ot, addr, 0, "").map_err(llvm_err)?;
                    let tag = cg
                        .builder
                        .build_load(cg.cx.bool_type(), tag_ptr, "is_some")
                        .map_err(llvm_err)?
                        .into_int_value();
                    cg.if_then(tag, "option.drop", |cg| {
                        if cg.llvm_ty(inner_ty).is_some() {
                            let payload = cg.builder.build_struct_gep(ot, addr, 1, "").map_err(llvm_err)?;
                            let mut child = path.to_vec();
                            child.push(0);
                            cg.drop_at_address(id, &child, inner_ty, payload)?;
                        }
                        Ok(())
                    })
                }
                Ty::Result(ok, err) => {
                    let (ok, err) = ((**ok).clone(), (**err).clone());
                    let rt = cg.result_ty(&ok, &err);
                    let tag_ptr = cg.builder.build_struct_gep(rt, addr, 0, "").map_err(llvm_err)?;
                    let tag =
                        cg.builder.build_load(cg.cx.bool_type(), tag_ptr, "is_ok").map_err(llvm_err)?.into_int_value();
                    cg.if_then(tag, "result.drop.ok", |cg| {
                        if cg.llvm_ty(&ok).is_some() {
                            let payload = cg.builder.build_struct_gep(rt, addr, 1, "").map_err(llvm_err)?;
                            let mut child = path.to_vec();
                            child.push(0);
                            cg.drop_at_address(id, &child, &ok, payload)?;
                        }
                        Ok(())
                    })?;
                    let tag_ptr = cg.builder.build_struct_gep(rt, addr, 0, "").map_err(llvm_err)?;
                    let tag =
                        cg.builder.build_load(cg.cx.bool_type(), tag_ptr, "is_ok").map_err(llvm_err)?.into_int_value();
                    let is_err = cg.builder.build_not(tag, "").map_err(llvm_err)?;
                    cg.if_then(is_err, "result.drop.err", |cg| {
                        if cg.llvm_ty(&err).is_some() {
                            let payload = cg.builder.build_struct_gep(rt, addr, 2, "").map_err(llvm_err)?;
                            let mut child = path.to_vec();
                            child.push(1);
                            cg.drop_at_address(id, &child, &err, payload)?;
                        }
                        Ok(())
                    })
                }
                Ty::Array(elem, n) => {
                    let n = *n;
                    let elem = (**elem).clone();
                    let Some(array_t) = cg.llvm_ty(ty) else { return Ok(()) };
                    for i in 0..n {
                        let mut child = path.to_vec();
                        child.push(i as usize);
                        if cg.needs_drop(&elem) || cg.has_inner_flag(id, &child) || cg.flag_for(id, &child).is_some() {
                            let zero = cg.usize_ty.const_int(0, false);
                            let index = cg.usize_ty.const_int(i, false);
                            // SAFETY (LLVM): i < n.
                            let p = unsafe { cg.builder.build_in_bounds_gep(array_t, addr, &[zero, index], "") }
                                .map_err(llvm_err)?;
                            cg.drop_at_address(id, &child, &elem, p)?;
                        }
                    }
                    Ok(())
                }
                Ty::List(elem) => {
                    let elem = (**elem).clone();
                    let ptr_ty = cg.cx.ptr_type(AddressSpace::default());
                    let ptr_field = cg.builder.build_struct_gep(cg.string_ty, addr, 0, "").map_err(llvm_err)?;
                    let len_field = cg.builder.build_struct_gep(cg.string_ty, addr, 1, "").map_err(llvm_err)?;
                    let data = cg.builder.build_load(ptr_ty, ptr_field, "").map_err(llvm_err)?.into_pointer_value();
                    let len = cg.builder.build_load(cg.usize_ty, len_field, "").map_err(llvm_err)?.into_int_value();
                    if let Some(elem_t) = cg.llvm_ty(&elem) {
                        // The indices whose elements have a drop flag or a flagged field: the other elements
                        // are still wholly initialized and drop unconditionally.
                        let mut flagged: Vec<usize> = cg
                            .flags
                            .iter()
                            .filter(|(p, _)| p.local == id && p.path.len() > path.len() && p.path.starts_with(path))
                            .filter_map(|(p, _)| p.path.get(path.len()).copied())
                            .filter(|step| *step != tir::MOVED_DEREF)
                            .collect();
                        flagged.sort_unstable();
                        flagged.dedup();
                        let index = cg.alloca(cg.usize_ty.into(), "drop.i")?;
                        cg.builder.build_store(index, cg.usize_ty.const_zero()).map_err(llvm_err)?;
                        let header = cg.new_block("list.drop.cond");
                        let body = cg.new_block("list.drop.body");
                        let latch = cg.new_block("list.drop.next");
                        let end = cg.new_block("list.drop.end");
                        cg.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                        cg.builder.position_at_end(header);
                        let i = cg.builder.build_load(cg.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                        let go = cg.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
                        cg.builder.build_conditional_branch(go, body, end).map_err(llvm_err)?;
                        cg.builder.position_at_end(body);
                        // SAFETY (LLVM): i < len.
                        let p = unsafe { cg.builder.build_in_bounds_gep(elem_t, data, &[i], "") }.map_err(llvm_err)?;
                        // Keep index order: one branch per statically flagged element inside the loop.
                        let done = cg.new_block("list.drop.done");
                        for step in &flagged {
                            let hit = cg.new_block("list.drop.moved");
                            let next = cg.new_block("list.drop.check");
                            let want = cg.usize_ty.const_int(*step as u64, false);
                            let is = cg.builder.build_int_compare(IntPredicate::EQ, i, want, "").map_err(llvm_err)?;
                            cg.builder.build_conditional_branch(is, hit, next).map_err(llvm_err)?;
                            cg.builder.position_at_end(hit);
                            let mut child = path.to_vec();
                            child.push(*step);
                            cg.drop_at(id, &child, &elem)?;
                            cg.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                            cg.builder.position_at_end(next);
                        }
                        cg.drop_value(p, &elem)?;
                        cg.builder.build_unconditional_branch(done).map_err(llvm_err)?;
                        cg.builder.position_at_end(done);
                        cg.builder.build_unconditional_branch(latch).map_err(llvm_err)?;
                        cg.builder.position_at_end(latch);
                        let i = cg.builder.build_load(cg.usize_ty, index, "").map_err(llvm_err)?.into_int_value();
                        let next =
                            cg.builder.build_int_add(i, cg.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
                        cg.builder.build_store(index, next).map_err(llvm_err)?;
                        cg.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                        cg.builder.position_at_end(end);
                    }
                    cg.builder.build_call(cg.rt.free, &[data.into()], "").map_err(llvm_err)?;
                    Ok(())
                }
                Ty::Heap(inner) => {
                    let ptr_ty = cg.cx.ptr_type(AddressSpace::default());
                    let p = cg.builder.build_load(ptr_ty, addr, "heap").map_err(llvm_err)?.into_pointer_value();
                    let mut child = path.to_vec();
                    child.push(tir::MOVED_DEREF);
                    if cg.llvm_ty(inner).is_some() {
                        cg.drop_at_address(id, &child, inner, p)?;
                    }
                    cg.builder.build_call(cg.rt.free, &[p.into()], "").map_err(llvm_err)?;
                    Ok(())
                }
                _ => cg.drop_value(addr, ty),
            }
        };
        match run {
            None => emit(self),
            Some(cond) => self.if_then(cond, "drop", emit),
        }
    }

    /// Runs `f` in a block guarded by `cond`, continuing afterwards.
    pub(super) fn if_then(
        &mut self,
        cond: IntValue<'ctx>,
        name: &str,
        f: impl FnOnce(&mut Self) -> GenResult<()>,
    ) -> GenResult<()> {
        let then_bb = self.new_block(name);
        let end = self.new_block("drop.end");
        self.builder.build_conditional_branch(cond, then_bb, end).map_err(llvm_err)?;
        self.builder.position_at_end(then_bb);
        f(self)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;
        self.builder.position_at_end(end);
        Ok(())
    }

    /// Frees the temporaries of the frames from `depth` up, leaving their slots empty. Temporaries drop in
    /// reverse creation order (DR5), so its newest temporary goes first. A temporary is dropped only on a path
    /// that created it: its live flag guards the drop.
    pub(super) fn free_temps_from(&mut self, depth: usize) -> GenResult<()> {
        let units: Vec<(PointerValue<'ctx>, Ty, PointerValue<'ctx>)> =
            self.temps[depth..].iter().rev().flat_map(|frame| frame.iter().rev()).cloned().collect();
        for (slot, ty, live) in units {
            let Some(t) = self.llvm_ty(&ty) else { continue };
            let flag = self.builder.build_load(self.cx.bool_type(), live, "").map_err(llvm_err)?.into_int_value();
            self.if_then(flag, "temp.drop", |cg| {
                cg.drop_value(slot, &ty)?;
                cg.builder.build_store(slot, undef(t)).map_err(llvm_err)?;
                cg.builder.build_store(live, cg.cx.bool_type().const_zero()).map_err(llvm_err)?;
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Records a value as a temporary of the current statement.
    pub(super) fn add_temp_value(&mut self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> GenResult<()> {
        if !self.needs_drop(ty) {
            return Ok(());
        }
        let Some(t) = self.llvm_ty(ty) else { return Ok(()) };
        let slot = self.alloca(t, "tmp")?;
        let v = self.need(v, ty);
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        let live = self.mark_temp_live()?;
        self.temps.last_mut().expect("a temporary frame is open").push((slot, ty.clone(), live));
        Ok(())
    }

    /// Records the value at `slot` as a temporary of the current statement.
    pub(super) fn add_temp_slot(&mut self, slot: PointerValue<'ctx>, ty: &Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let live = self.mark_temp_live().expect("allocating a drop flag cannot fail");
        self.temps.last_mut().expect("a temporary frame is open").push((slot, ty.clone(), live));
    }

    /// The live flag of a new temporary, set on the current path.
    fn mark_temp_live(&mut self) -> GenResult<PointerValue<'ctx>> {
        let live = self.alloca(self.cx.bool_type().into(), "tmp.live")?;
        self.builder.build_store(live, self.cx.bool_type().const_int(1, false)).map_err(llvm_err)?;
        Ok(live)
    }
}
