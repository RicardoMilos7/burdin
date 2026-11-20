//! Closure bodies and values: the boxed capture record, the indirect call, and the record's destructor.
//!
//! A closure value is `{ code: ptr, env: ptr, drop: ptr }`. `code` points at the body function, whose first
//! parameter is the capture record; `drop` takes the record, drops the moved-in captures, and frees it. The body
//! borrows borrowed captures through pointers stored in the record (`spec/memory-rules.md` CL1-CL9).

use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, CaptureMode, FnKind, LocalKind, Ty};
use inkwell::AddressSpace;
use inkwell::module::Linkage;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, StructValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The capture record's LLVM type: a pointer for each borrowed capture, the value itself for each moved one.
    pub(super) fn closure_env_ty(&self, clos: &tir::Closure) -> StructType<'ctx> {
        let ptr = self.cx.ptr_type(AddressSpace::default());
        let fields: Vec<BasicTypeEnum<'ctx>> = clos
            .captures
            .iter()
            .map(|c| match c.mode {
                CaptureMode::Move => self.storage_ty(&c.ty),
                CaptureMode::Borrow | CaptureMode::BorrowMut => ptr.into(),
            })
            .collect();
        self.cx.struct_type(&fields, false)
    }

    /// Declares the body and destructor function of every closure.
    pub(super) fn declare_closures(&mut self) -> GenResult<()> {
        let ptr = self.cx.ptr_type(AddressSpace::default());
        let program = self.program;
        for clos in &program.closures {
            // The record pointer, then the call's consumed flag (an `FnOnce` call destroys the record), then the
            // closure's own parameters.
            let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr.into(), self.cx.bool_type().into()];
            for p in &clos.sig.params {
                if let Some(t) = self.llvm_ty(p) {
                    params.push(t.into());
                }
            }
            let ret = self.llvm_ty(&clos.sig.ret);
            let fn_ty = match ret {
                Some(t) => t.fn_type(&params, false),
                None => self.cx.void_type().fn_type(&params, false),
            };
            let name = format!("burdin.{}", clos.name);
            let value = self.module.add_function(&name, fn_ty, Some(Linkage::Internal));
            self.closure_funcs.push(value);
            let drop_ty = self.cx.void_type().fn_type(&[ptr.into()], false);
            let drop = self.module.add_function(&format!("{name}.drop"), drop_ty, Some(Linkage::Internal));
            self.closure_drops.push(drop);
        }
        Ok(())
    }

    /// Generates every capture-record destructor: it drops the moved-in captures and frees the record. A closure
    /// with no dropped captures still frees the record.
    pub(super) fn define_closure_drops(&mut self) -> GenResult<()> {
        let program = self.program;
        for (i, clos) in program.closures.iter().enumerate() {
            let value = self.closure_drops[i];
            self.start_glue(value)?;
            let env = value.get_nth_param(0).expect("the destructor takes a record").into_pointer_value();
            let env_ty = self.closure_env_ty(clos);
            for (k, c) in clos.captures.iter().enumerate() {
                if c.mode != CaptureMode::Move || !self.needs_drop(&c.ty) || self.llvm_ty(&c.ty).is_none() {
                    continue;
                }
                let field = self.builder.build_struct_gep(env_ty, env, k as u32, "").map_err(llvm_err)?;
                self.drop_value(field, &c.ty)?;
            }
            self.builder.build_call(self.rt.free, &[env.into()], "").map_err(llvm_err)?;
            self.finish_glue()?;
        }
        Ok(())
    }

    /// Generates one closure body. The body's capture locals read from the record; a `FnOnce` body owns the
    /// record and frees it after dropping whatever captures remain. A `Fn`/`FnMut` body keeps the record for
    /// repeated calls, but destroys it when the call consumed a value weakened to `FnOnce`.
    pub(super) fn closure_body(
        &mut self,
        value: FunctionValue<'ctx>,
        index: usize,
        clos: &'a tir::Closure,
    ) -> GenResult<()> {
        let entry = self.cx.append_basic_block(value, "entry");
        let body = self.cx.append_basic_block(value, "body");
        self.entry_builder.position_at_end(entry);
        self.builder.position_at_end(body);
        // The prologue stores the parameters before the subprogram exists; without this, those stores would carry
        // the previous function's source location, which LLVM's verifier rejects.
        if self.debug.is_some() {
            self.builder.unset_current_debug_location();
        }
        self.function = Some(value);
        self.locals = &clos.func.locals;
        self.ret = &clos.func.ret;
        self.return_env = None;
        self.return_consume = None;
        self.loops.clear();
        self.temps = vec![Vec::new()];
        self.slots.clear();
        self.scopes = Vec::new();
        self.flags.clear();
        self.dead_locals.clear();
        self.bounds_facts.clear();
        for moved in &clos.func.moved {
            if moved.always_dead {
                self.dead_locals.insert(moved.local);
            }
        }
        for moved in &clos.func.moved {
            if self.dead_locals.contains(&moved.local) {
                continue;
            }
            let flag = self.entry_builder.build_alloca(self.cx.bool_type(), "moved").map_err(llvm_err)?;
            let one = self.cx.bool_type().const_int(1, false);
            self.entry_builder.build_store(flag, one).map_err(llvm_err)?;
            self.flags.push((moved.clone(), flag));
        }
        for local in &clos.func.locals {
            let slot = match self.llvm_ty(&local.ty) {
                Some(t) if !local.by_ref => Some(self.entry_builder.build_alloca(t, &local.name).map_err(llvm_err)?),
                _ => None,
            };
            self.slots.push(slot);
        }
        // The record is the first parameter and the consumed flag the second; capture i is field i.
        let env = value.get_nth_param(0).expect("a closure body takes its record").into_pointer_value();
        let consumed = value.get_nth_param(1).expect("a closure body takes its consumed flag").into_int_value();
        if clos.kind == FnKind::FnOnce {
            // The call transferred the record to this body; an early `return` frees it too.
            self.return_env = Some(env);
        } else {
            // The same body serves `Fn`/`FnMut` calls, which keep the record, and an `FnOnce` call through a
            // weakened type, which consumes it. The destructor runs on both the early and the normal exit.
            self.return_consume = Some((env, self.closure_drops[index], consumed));
        }
        let env_ty = self.closure_env_ty(clos);
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        for (i, c) in clos.captures.iter().enumerate() {
            let field = self.builder.build_struct_gep(env_ty, env, i as u32, "").map_err(llvm_err)?;
            let slot = match c.mode {
                CaptureMode::Borrow | CaptureMode::BorrowMut => {
                    self.builder.build_load(ptr_ty, field, "").map_err(llvm_err)?.into_pointer_value()
                }
                CaptureMode::Move => field,
            };
            self.slots[c.local.0] = Some(slot);
        }
        // The explicit parameters follow the record and the flag.
        let mut arg_index = 2;
        for p in &clos.func.params {
            if self.llvm_ty(&clos.func.locals[p.0].ty).is_none() {
                continue;
            }
            let arg = value.get_nth_param(arg_index).expect("parameter count matches the signature");
            arg_index += 1;
            if let Some(slot) = self.slots[p.0] {
                self.builder.build_store(slot, arg).map_err(llvm_err)?;
            }
        }
        // The closure's own parameters and the captures a `FnOnce` body owns are dropped at the end.
        let owned: Vec<tir::LocalId> = clos
            .func
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind == LocalKind::OwnParam)
            .map(|(i, _)| tir::LocalId(i))
            .collect();
        self.scopes.push(owned);
        self.debug_entry = Some(entry);
        self.debug_function(value, &clos.func);
        // The captures and the parameters were stored above, so their debug locations start here (DBG4).
        for c in &clos.captures {
            self.debug_reopen(c.local);
        }
        for p in &clos.func.params {
            self.debug_reopen(*p);
        }
        self.debug_at(clos.func.span);
        let result = self.block(&clos.func.body, Mode::Move)?;
        self.free_temps_from(0)?;
        self.drop_scope(0)?;
        if clos.kind == FnKind::FnOnce {
            self.builder.build_call(self.rt.free, &[env.into()], "").map_err(llvm_err)?;
        } else {
            self.destroy_if_consumed(env, self.closure_drops[index], consumed)?;
        }
        match self.llvm_ty(&clos.func.ret) {
            Some(_) => match result {
                Some(v) => self.builder.build_return(Some(&v)).map_err(llvm_err)?,
                None => self.builder.build_unreachable().map_err(llvm_err)?,
            },
            None => self.builder.build_return(None).map_err(llvm_err)?,
        };
        self.entry_builder.build_unconditional_branch(body).map_err(llvm_err)?;
        Ok(())
    }

    /// Builds a closure value: allocates its capture record, fills it, and pairs it with the body and its
    /// destructor.
    pub(super) fn closure_value(
        &mut self,
        index: usize,
        captures: &[tir::Capture],
        span: burdin_diag::Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let program = self.program;
        let clos = &program.closures[index];
        let env_ty = self.closure_env_ty(clos);
        let size = self.target_data.get_abi_size(&env_ty).max(1);
        let size = self.usize_ty.const_int(size, false);
        let env = self.global_alloc(size, None)?;
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        for (i, c) in captures.iter().enumerate() {
            let field = self.builder.build_struct_gep(env_ty, env, i as u32, "").map_err(llvm_err)?;
            match c.mode {
                CaptureMode::Borrow | CaptureMode::BorrowMut => {
                    let ptr = self.slots[c.local.0].unwrap_or_else(|| ptr_ty.const_null());
                    self.builder.build_store(field, ptr).map_err(llvm_err)?;
                }
                CaptureMode::Move => {
                    if let Some(t) = self.llvm_ty(&c.ty) {
                        let Some(slot) = self.slots[c.local.0] else {
                            // A value-less capture (a unit local) stores nothing.
                            continue;
                        };
                        let v = self.builder.build_load(t, slot, "").map_err(llvm_err)?;
                        if !self.is_copy(&c.ty) {
                            self.mark_moved(c.local, &[]);
                        }
                        self.builder.build_store(field, v).map_err(llvm_err)?;
                    }
                }
            }
        }
        let mut value = undef(self.closure_ty.into()).into_struct_value();
        let code = self.closure_funcs[index].as_global_value().as_pointer_value();
        let drop = self.closure_drops[index].as_global_value().as_pointer_value();
        value = self.builder.build_insert_value(value, code, 0, "").map_err(llvm_err)?.into_struct_value();
        value = self.builder.build_insert_value(value, env, 1, "").map_err(llvm_err)?.into_struct_value();
        value = self.builder.build_insert_value(value, drop, 2, "").map_err(llvm_err)?.into_struct_value();
        let _ = span;
        Ok(Some(value.into()))
    }

    /// Calls a closure through its value: `code(env, args...)`. `FnOnce` consumes the closure; the body frees
    /// the record.
    pub(super) fn closure_call(
        &mut self,
        callee: &'a tir::Expr,
        args: &'a [tir::Expr],
        kind: FnKind,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mode = if kind == FnKind::FnOnce { Mode::Move } else { Mode::Use };
        let callee_v = self.expr_mode(callee, mode)?;
        // An `Fn` or `FnMut` closure that is not a place (`make(4)(4)`) is borrowed by the call and owned by no
        // one, so it is a temporary of the statement and its capture record is freed at the end (DR5).
        if kind != FnKind::FnOnce && !super::place::is_place(callee) {
            self.add_temp_value(callee_v, &callee.ty)?;
        }
        let callee_v = self.need(callee_v, &callee.ty).into_struct_value();
        self.call_closure_value(callee_v, &callee.ty, kind, args)
    }

    /// Calls a closure value that has already been evaluated. A built-in operation that owns the closure calls it
    /// at most once, so the call consumes the value and the body frees the capture record.
    pub(super) fn call_closure_value(
        &mut self,
        value: StructValue<'ctx>,
        closure_ty: &Ty,
        kind: FnKind,
        args: &'a [tir::Expr],
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = Vec::new();
        for a in args {
            let v = self.expr_mode(a, Mode::Move)?;
            if self.llvm_ty(&a.ty).is_some() {
                values.push(self.need(v, &a.ty).into());
            }
        }
        self.call_closure_with(value, closure_ty, kind, &values)
    }

    /// Calls a closure value with arguments that are already LLVM values. A built-in operation that takes a
    /// closure and hands it borrowed places (`Map.retain`) passes the addresses itself.
    pub(super) fn call_closure_with(
        &mut self,
        value: StructValue<'ctx>,
        closure_ty: &Ty,
        kind: FnKind,
        args: &[BasicMetadataValueEnum<'ctx>],
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let code = self.builder.build_extract_value(value, 0, "closure.code").map_err(llvm_err)?;
        let env = self.builder.build_extract_value(value, 1, "closure.env").map_err(llvm_err)?;
        let Ty::Fn { sig, .. } = closure_ty else {
            return Err("internal error: a closure call on a non-closure type".into());
        };
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let mut params: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr_ty.into(), self.cx.bool_type().into()];
        for p in &sig.params {
            if let Some(t) = self.llvm_ty(p) {
                params.push(t.into());
            }
        }
        let ret = self.llvm_ty(&sig.ret);
        let fn_ty = match ret {
            Some(t) => t.fn_type(&params, false),
            None => self.cx.void_type().fn_type(&params, false),
        };
        let consumed = self.cx.bool_type().const_int(u64::from(kind == FnKind::FnOnce), false);
        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = vec![env.into(), consumed.into()];
        values.extend(args.iter().copied());
        let call = self.builder.build_indirect_call(fn_ty, code.into_pointer_value(), &values, "").map_err(llvm_err)?;
        if ret.is_some() { Ok(call.try_as_basic_value().basic()) } else { Ok(None) }
    }

    /// Destroys a `Fn`/`FnMut` body's capture record when the call that entered the body consumed the value
    /// (an `FnOnce` call through a weakened type). The record is kept for ordinary `Fn`/`FnMut` calls.
    pub(super) fn destroy_if_consumed(
        &mut self,
        env: inkwell::values::PointerValue<'ctx>,
        drop: FunctionValue<'ctx>,
        consumed: IntValue<'ctx>,
    ) -> GenResult<()> {
        let current = self.current_fn();
        let destroy = self.cx.append_basic_block(current, "closure.consume");
        let after = self.cx.append_basic_block(current, "closure.keep");
        self.builder.build_conditional_branch(consumed, destroy, after).map_err(llvm_err)?;
        self.builder.position_at_end(destroy);
        self.builder.build_call(drop, &[env.into()], "").map_err(llvm_err)?;
        self.builder.build_unconditional_branch(after).map_err(llvm_err)?;
        self.builder.position_at_end(after);
        Ok(())
    }
}
