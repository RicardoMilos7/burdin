//! Expressions.

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, FloatTy, LocalKind, Ty, UnOp};
use inkwell::AddressSpace;
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// A call to a `blocking` extern (`spec/io.md`, IO4). The arguments are stored in an 8-byte-slot block, a
    /// generated thunk makes the C call from that block on the helper pool, and the result is read back from the
    /// slot after the argument slots. The calling task parks while the helper runs.
    fn blocking_extern_call(
        &mut self,
        id: usize,
        args: &'a [tir::Expr],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let symbol = self.program.externs[id].symbol.clone();
        let vararg = self.program.externs[id].vararg;
        let params = self.program.externs[id].params.clone();
        if vararg {
            return Err(format!(
                "`{symbol}` is a variadic `blocking` extern; a blocking declaration cannot take `...`"
            ));
        }
        let slots = args.len() + 1;
        let bytes = (slots as u64) * 8;
        let env = self
            .entry_builder
            .build_alloca(self.cx.i8_type().array_type(bytes as u32), "blocking.env")
            .map_err(llvm_err)?;
        for (i, arg) in args.iter().enumerate() {
            if self.llvm_ty(&params[i]).is_none() {
                continue;
            }
            let value = self.expr_mode(arg, Mode::Use)?;
            let value = self.need(value, &params[i]);
            let slot = self.blocking_slot(env, i)?;
            self.builder.build_store(slot, value).map_err(llvm_err)?;
        }
        let thunk = self.blocking_thunk(id)?;
        let shim = self.module.get_function("burdin.std.__offload_blocking").ok_or(
            "a `blocking` extern needs the standard library's `__offload_blocking` function (`spec/io.md`, IO4)",
        )?;
        let thunk_ptr = thunk.as_global_value().as_pointer_value();
        self.builder.build_call(shim, &[thunk_ptr.into(), env.into()], "").map_err(llvm_err)?;
        match self.llvm_ty(ty) {
            Some(result_ty) => {
                let slot = self.blocking_slot(env, args.len())?;
                Ok(Some(self.builder.build_load(result_ty, slot, "blocking.result").map_err(llvm_err)?))
            }
            None => Ok(None),
        }
    }

    /// The address of slot `i` in a blocking call's argument block.
    fn blocking_slot(
        &mut self,
        env: inkwell::values::PointerValue<'ctx>,
        i: usize,
    ) -> GenResult<inkwell::values::PointerValue<'ctx>> {
        let offset = self.cx.i64_type().const_int((i as u64) * 8, false);
        unsafe { self.builder.build_in_bounds_gep(self.cx.i8_type(), env, &[offset], "blocking.slot") }
            .map_err(llvm_err)
    }

    /// The thunk of one `blocking` extern: `void (ptr env, i1 owned)`, which loads the arguments from `env`, makes
    /// the C call, and stores the result after them. It is generated once per extern, in its own builder so the
    /// builder of the body being generated is not disturbed.
    fn blocking_thunk(&mut self, id: usize) -> GenResult<inkwell::values::FunctionValue<'ctx>> {
        if let Some(thunk) = self.blocking_thunks[id] {
            return Ok(thunk);
        }
        let params = self.program.externs[id].params.clone();
        let ret = self.program.externs[id].ret.clone();
        let declaration = self.externs[id];
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let void = self.cx.void_type();
        let thunk_ty = void.fn_type(&[ptr_ty.into(), self.cx.bool_type().into()], false);
        let name = format!("burdin.blocking.{id}");
        let thunk = self.module.add_function(&name, thunk_ty, Some(inkwell::module::Linkage::Internal));
        let builder = self.cx.create_builder();
        let entry = self.cx.append_basic_block(thunk, "entry");
        builder.position_at_end(entry);
        let env = thunk.get_nth_param(0).expect("the thunk has an argument block").into_pointer_value();
        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        for (i, param) in params.iter().enumerate() {
            let Some(llvm_ty) = self.llvm_ty(param) else { continue };
            let offset = self.cx.i64_type().const_int((i as u64) * 8, false);
            let slot =
                unsafe { builder.build_in_bounds_gep(self.cx.i8_type(), env, &[offset], "") }.map_err(llvm_err)?;
            let value = builder.build_load(llvm_ty, slot, "arg").map_err(llvm_err)?;
            values.push(value.into());
        }
        let call = builder.build_call(declaration, &values, "").map_err(llvm_err)?;
        let has_result = self.llvm_ty(&ret).is_some();
        if has_result && let Some(result) = call.try_as_basic_value().basic() {
            let offset = self.cx.i64_type().const_int((params.len() as u64) * 8, false);
            let slot =
                unsafe { builder.build_in_bounds_gep(self.cx.i8_type(), env, &[offset], "") }.map_err(llvm_err)?;
            builder.build_store(slot, result).map_err(llvm_err)?;
        }
        builder.build_return(None).map_err(llvm_err)?;
        self.blocking_thunks[id] = Some(thunk);
        Ok(thunk)
    }

    pub(super) fn expr_mode(&mut self, e: &'a tir::Expr, mode: Mode) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        use tir::ExprKind as K;
        self.debug_at(e.span);
        let v: Option<BasicValueEnum<'ctx>> = match &e.kind {
            K::Comptime(_) | K::ComptimeFor { .. } => {
                return Err("a comptime construct reached code generation; the comptime pass must replace it".into());
            }
            K::Unit => None,
            K::Bool(b) => Some(self.cx.bool_type().const_int(u64::from(*b), false).into()),
            K::Int(v) => match &e.ty {
                Ty::Int(i) => Some(super::types::int_const(self.int_ty(*i), *v).into()),
                _ => None,
            },
            K::Float(v) => match &e.ty {
                Ty::Float(FloatTy::F32) => Some(self.cx.f32_type().const_float(*v).into()),
                _ => Some(self.cx.f64_type().const_float(*v).into()),
            },
            K::Char(c) => Some(self.cx.i32_type().const_int(u64::from(*c as u32), false).into()),
            K::Str(s) => Some(self.str_literal(s.as_bytes()).into()),
            K::Local(id) => {
                let v = match (self.slots[id.0], self.llvm_ty(&e.ty)) {
                    (Some(slot), Some(t)) => {
                        Some(self.builder.build_load(t, slot, &self.locals[id.0].name).map_err(llvm_err)?)
                    }
                    _ => None,
                };
                if mode == Mode::Move && !self.is_copy(&e.ty) {
                    self.mark_moved(*id, &[]);
                }
                v
            }
            K::Const(id) => {
                let program = self.program;
                self.expr_mode(&program.consts[id.0].value, mode)?
            }
            K::Call(f, args) => {
                let program = self.program;
                let callee = &program.functions[f.0];
                let mut values: Vec<BasicMetadataValueEnum> = Vec::new();
                for (a, p) in args.iter().zip(&callee.params) {
                    if callee.locals[p.0].by_ref {
                        let ptr = match self.place_ptr(a)? {
                            Some(ptr) => ptr,
                            None => self.cx.ptr_type(AddressSpace::default()).get_undef(),
                        };
                        values.push(ptr.into());
                        continue;
                    }
                    let m = if callee.locals[p.0].kind == LocalKind::OwnParam { Mode::Move } else { Mode::Use };
                    let v = self.expr_mode(a, m)?;
                    if self.llvm_ty(&callee.locals[p.0].ty).is_some() {
                        values.push(self.need(v, &callee.locals[p.0].ty).into());
                    }
                }
                let call = self.builder.build_call(self.functions[f.0], &values, "").map_err(llvm_err)?;
                match self.llvm_ty(&e.ty) {
                    Some(_) => call.try_as_basic_value().basic(),
                    None => None,
                }
            }
            K::ExternCall(id, args) if self.program.externs[*id].blocking => {
                self.blocking_extern_call(*id, args, &e.ty)?
            }
            K::ExternCall(id, args) => {
                let ext = &self.program.externs[*id];
                let mut values: Vec<BasicMetadataValueEnum> = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let ty = ext.params.get(i).unwrap_or(&a.ty);
                    let v = self.expr_mode(a, Mode::Use)?;
                    if self.llvm_ty(ty).is_some() {
                        values.push(self.need(v, ty).into());
                    }
                }
                let call = self.builder.build_call(self.externs[*id], &values, "").map_err(llvm_err)?;
                match self.llvm_ty(&e.ty) {
                    Some(_) => call.try_as_basic_value().basic(),
                    None => None,
                }
            }
            K::Builtin(b, args) => self.builtin(*b, args, &e.ty, e.span)?,
            K::Method(m, args) => self.method(*m, args, &e.ty, e.span)?,
            K::Closure { closure, captures } => self.closure_value(*closure, captures, e.span)?,
            K::ClosureCall { callee, args, kind } => self.closure_call(callee, args, *kind)?,
            K::BoxAny { value, vtable } => self.box_any(value, *vtable)?,
            K::AnyCall { recv, iface, slot, params, args } => {
                self.any_call(recv, *iface, *slot, params, args, &e.ty)?
            }
            K::AnyIs { value, type_id, .. } => self.any_is(value, *type_id)?,
            K::AnyDowncast { value, target, type_id } => self.any_downcast(value, target, *type_id)?,
            K::StringAsStr(inner) => {
                let (ptr, len) = self.view_parts(inner)?;
                Some(self.make_str(ptr, len)?.into())
            }
            K::Ref { place, .. } => {
                // The address of the place; a value without storage still gets a well-formed pointer.
                let ptr = match self.place_ptr(place)? {
                    Some(ptr) => ptr,
                    None => self.cx.ptr_type(AddressSpace::default()).get_undef(),
                };
                Some(ptr.into())
            }
            K::Slice { base, lo, hi, inclusive } => Some(self.slice(base, lo, hi, *inclusive, e.span)?.into()),
            // The value of a block, `if`, or `match` is an owned temporary whatever uses it: its tail moves out
            // (spec/memory-rules.md, MV9), so a borrowed use does not leave a second owner behind.
            K::Block(b) => self.block(b, Mode::Move)?,
            K::If(cond, then, other) => self.if_expr(cond, then, other.as_deref(), &e.ty)?,
            K::While(..)
            | K::Loop(_)
            | K::ForRange { .. }
            | K::ForRangeValue { .. }
            | K::ForArray { .. }
            | K::ForSlice { .. }
            | K::ForList { .. }
            | K::ForMap { .. }
            | K::ForChars { .. } => {
                self.loop_expr(e, None)?;
                None
            }
            K::Labeled(label, inner) => {
                self.loop_expr(inner, Some(label.as_str()))?;
                None
            }
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, &e.ty)?,
            K::Break(label) | K::Continue(label) => {
                // An unlabeled jump names the innermost loop; a labeled one the loop with that label (sema checked it).
                let index = match label {
                    None => self.loops.len() - 1,
                    Some(name) => self
                        .loops
                        .iter()
                        .rposition(|lp| lp.label.as_deref() == Some(name.as_str()))
                        .expect("checked: a labeled loop encloses it"),
                };
                let target = &self.loops[index];
                let (bb, temp_depth, scope_depth) = match e.kind {
                    K::Break(_) => (target.break_bb, target.temp_depth, target.scope_depth),
                    _ => (target.continue_bb, target.temp_depth, target.scope_depth),
                };
                self.drop_scopes_above(scope_depth)?;
                self.free_temps_from(temp_depth)?;
                self.builder.build_unconditional_branch(bb).map_err(llvm_err)?;
                self.continue_dead();
                None
            }
            K::Return(value) => {
                let v = match value {
                    Some(v) => self.expr_mode(v, Mode::Move)?,
                    None => None,
                };
                self.drop_scopes_above(0)?;
                self.free_temps_from(0)?;
                // A `FnOnce` body owns its capture record; every return path must free it. A `Fn`/`FnMut` body
                // destroys it on an early return only when the call consumed the value (CL6/CL8).
                if let Some(env) = self.return_env {
                    self.builder.build_call(self.rt.free, &[env.into()], "").map_err(llvm_err)?;
                } else if let Some((env, drop, consumed)) = self.return_consume {
                    self.destroy_if_consumed(env, drop, consumed)?;
                }
                let ret = self.ret;
                if self.llvm_ty(ret).is_some() {
                    let v = self.need(v, ret);
                    self.builder.build_return(Some(&v)).map_err(llvm_err)?;
                } else {
                    self.builder.build_return(None).map_err(llvm_err)?;
                }
                self.continue_dead();
                None
            }
            K::Assign(place, value) => {
                let v = self.expr_mode(value, Mode::Move)?;
                self.assign(place, v)?;
                None
            }
            K::Binary(op, l, r) => self.binary(*op, l, r, e.span)?,
            K::Unary(op, operand) => {
                let v = self.expr_mode(operand, Mode::Use)?;
                let v = self.need(v, &operand.ty);
                match (op, &operand.ty) {
                    (UnOp::Not, _) => Some(self.builder.build_not(v.into_int_value(), "").map_err(llvm_err)?.into()),
                    (UnOp::Neg, Ty::Float(_)) => {
                        Some(self.builder.build_float_neg(v.into_float_value(), "").map_err(llvm_err)?.into())
                    }
                    (UnOp::Neg, Ty::Int(i)) => Some(self.negate(v.into_int_value(), *i, e.span)?.into()),
                    _ => None,
                }
            }
            K::Cast(operand) => {
                let v = self.expr_mode(operand, Mode::Use)?;
                match self.llvm_ty(&operand.ty) {
                    Some(_) => {
                        let v = self.need(v, &operand.ty);
                        Some(self.cast(v, &operand.ty, &e.ty)?)
                    }
                    None => None,
                }
            }
            K::Range { lo, hi, .. } => {
                let Some(t) = self.llvm_ty(&e.ty) else {
                    self.expr_mode(lo, Mode::Move)?;
                    self.expr_mode(hi, Mode::Move)?;
                    return Ok(None);
                };
                let lo_v = self.expr_mode(lo, Mode::Move)?;
                let lo_v = self.need(lo_v, &lo.ty);
                let hi_v = self.expr_mode(hi, Mode::Move)?;
                let hi_v = self.need(hi_v, &hi.ty);
                let mut agg = undef(t).into_struct_value();
                agg = self.builder.build_insert_value(agg, lo_v, 0, "").map_err(llvm_err)?.into_struct_value();
                agg = self.builder.build_insert_value(agg, hi_v, 1, "").map_err(llvm_err)?.into_struct_value();
                Some(agg.into())
            }
            K::Array(elems) => {
                let Some(t) = self.llvm_ty(&e.ty) else {
                    for el in elems {
                        self.expr_mode(el, Mode::Move)?;
                    }
                    return Ok(None);
                };
                let mut agg = undef(t).into_array_value();
                for (i, el) in elems.iter().enumerate() {
                    let v = self.expr_mode(el, Mode::Move)?;
                    let v = self.need(v, &el.ty);
                    agg = self.builder.build_insert_value(agg, v, i as u32, "").map_err(llvm_err)?.into_array_value();
                }
                Some(agg.into())
            }
            K::ArrayRepeat(value, n) => {
                let v = self.expr_mode(value, Mode::Move)?;
                match (self.llvm_ty(&e.ty), self.llvm_ty(&value.ty)) {
                    (Some(t), Some(elem_t)) => {
                        let v = self.need(v, &value.ty);
                        Some(self.repeat(v, elem_t, t, *n)?)
                    }
                    _ => None,
                }
            }
            K::Tuple(elems) => {
                let fields: Vec<(usize, &'a tir::Expr)> = elems.iter().enumerate().collect();
                self.product(&fields, &e.ty)?
            }
            K::Struct(fields) => {
                let fields: Vec<(usize, &'a tir::Expr)> = fields.iter().map(|(i, v)| (*i, v)).collect();
                self.product(&fields, &e.ty)?
            }
            K::Variant(index, fields) => self.variant(*index, fields, &e.ty)?,
            K::Some(inner) => {
                let v = self.expr_mode(inner, Mode::Move)?;
                let Ty::Option(t) = &e.ty else { unreachable!("`Some` builds an Option") };
                let opt_ty = self.option_ty(t);
                let yes = self.cx.bool_type().const_int(1, false);
                let agg = self
                    .builder
                    .build_insert_value(opt_ty.const_zero(), yes, 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                match self.llvm_ty(t) {
                    Some(_) => {
                        let v = self.need(v, t);
                        Some(
                            self.builder
                                .build_insert_value(agg, v, 1, "")
                                .map_err(llvm_err)?
                                .into_struct_value()
                                .into(),
                        )
                    }
                    None => Some(agg.into()),
                }
            }
            K::None => {
                let Ty::Option(t) = &e.ty else { unreachable!("`None` is an Option") };
                Some(self.option_ty(t).const_zero().into())
            }
            K::Ok(inner) | K::Err(inner) => {
                let v = self.expr_mode(inner, Mode::Move)?;
                let Ty::Result(ok, err) = &e.ty else { unreachable!("a Result constructor builds a Result") };
                let rt = self.result_ty(ok, err);
                let is_ok = matches!(e.kind, K::Ok(_));
                let tag = self.cx.bool_type().const_int(u64::from(is_ok), false);
                let mut agg = self
                    .builder
                    .build_insert_value(undef(rt.into()).into_struct_value(), tag, 0, "")
                    .map_err(llvm_err)?
                    .into_struct_value();
                let payload = if is_ok { ok } else { err };
                if self.llvm_ty(payload).is_some() {
                    let v = self.need(v, payload);
                    let index = if is_ok { 1 } else { 2 };
                    agg = self.builder.build_insert_value(agg, v, index, "").map_err(llvm_err)?.into_struct_value();
                }
                Some(agg.into())
            }
            K::Index(..) | K::Field(..) | K::Deref(..) | K::PtrDeref(..) | K::DerefRef(..) => {
                self.expr_place(e, mode)?
            }
            K::LayoutOf { ty, alignment } => {
                let size = self.layout_size(ty, *alignment);
                Some(self.usize_ty.const_int(size, false).into())
            }
        };
        if e.ty == Ty::Never {
            return Ok(None);
        }
        Ok(v)
    }
}
