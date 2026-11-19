//! `any I`: boxing, method calls through the vtable, `is`, and `downcast` (`spec/existentials.md`).
//!
//! A box is `{ data: ptr, vtable: ptr }`. `data` is a `malloc`ed concrete value, and `vtable` points at a
//! private constant `{ i64 type_id, ptr drop, ptr method_0, ..., ptr method_n }` table, one per
//! (concrete type, interface) pair the program boxes (AN1, AN11). Dropping a box runs the table's `drop`
//! slot and frees `data`; a `downcast` that matches moves the value out and frees `data` without running the
//! drop, because the new owner drops it.

use super::place::is_place;
use super::{Codegen, GenResult, Mode, llvm_err, undef};
use burdin_tir::{self as tir, Ty};
use inkwell::module::Linkage;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, GlobalValue, StructValue};
use inkwell::{AddressSpace, IntPredicate};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// The shape of a vtable: `{ i64 type_id, ptr drop, ptr name, i64 name_len, ptr method_0, ..., ptr method_n }`.
    /// `name` is the concrete type's Burdin name, so a debugger's `any` printer can show what a box holds
    /// (`spec/debug.md`, DBG5).
    pub(super) fn vtable_struct_ty(&self, iface: usize) -> StructType<'ctx> {
        let ptr = self.cx.ptr_type(AddressSpace::default());
        let mut fields: Vec<BasicTypeEnum<'ctx>> =
            vec![self.cx.i64_type().into(), ptr.into(), ptr.into(), self.usize_ty.into()];
        for _ in &self.program.any_ifaces[iface].methods {
            fields.push(ptr.into());
        }
        self.cx.struct_type(&fields, false)
    }

    /// The private constant vtable of `Program::any_vtables[index]`, created on first use.
    pub(super) fn vtable_global(&mut self, index: usize) -> GlobalValue<'ctx> {
        if let Some(global) = self.vtables[index] {
            return global;
        }
        let program = self.program;
        let vtable = &program.any_vtables[index];
        debug_assert_eq!(
            vtable.entries.len(),
            program.any_ifaces[vtable.iface].methods.len(),
            "a vtable has one entry per interface method"
        );
        let concrete = vtable.concrete.clone();
        let ty = self.vtable_struct_ty(vtable.iface);
        let name = self.str_literal(format!("{concrete}").as_bytes());
        let mut fields: Vec<BasicValueEnum<'ctx>> = vec![
            self.cx.i64_type().const_int(vtable.type_id as u64, false).into(),
            self.drop_glue(&concrete).as_global_value().as_pointer_value().into(),
            name.get_field_at_index(0).expect("str has a pointer"),
            name.get_field_at_index(1).expect("str has a length"),
        ];
        for entry in &vtable.entries {
            fields.push(self.functions[entry.0].as_global_value().as_pointer_value().into());
        }
        let global = self.module.add_global(ty, None, &format!("burdin.vtable.{index}"));
        global.set_initializer(&ty.const_named_struct(&fields));
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        self.vtables[index] = Some(global);
        global
    }

    /// Boxes `value` into `any I` (AN3): allocates its storage, moves the value in, and pairs the address with
    /// the vtable at `Program::any_vtables[index]`.
    pub(super) fn box_any(&mut self, value: &'a tir::Expr, vtable: usize) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let v = self.expr_mode(value, Mode::Move)?;
        // A value-less type (the unit type) still gets a box: the allocation is what `data` points at, and the
        // vtable's drop does nothing.
        let size = self.llvm_ty(&value.ty).map(|t| self.target_data.get_abi_size(&t)).unwrap_or(0).max(1);
        let size = self.usize_ty.const_int(size, false);
        let data = self.global_alloc(size, None)?;
        if self.llvm_ty(&value.ty).is_some() {
            let v = self.need(v, &value.ty);
            self.builder.build_store(data, v).map_err(llvm_err)?;
        }
        // The box is the only place the concrete type's name survives (`spec/debug.md`, DBG5); naming its storage
        // keeps the type in the debug info for the `any` printer.
        self.debug_any_box(&value.ty, data);
        let vtable = self.vtable_global(vtable).as_pointer_value();
        let mut agg = undef(self.any_ty.into()).into_struct_value();
        agg = self.builder.build_insert_value(agg, data, 0, "any.data").map_err(llvm_err)?.into_struct_value();
        agg = self.builder.build_insert_value(agg, vtable, 1, "any.vtable").map_err(llvm_err)?.into_struct_value();
        Ok(Some(agg.into()))
    }

    /// A borrowed `any` value, as its two words. A box that is not a place is a temporary of the statement, so
    /// it is kept alive and dropped at the end of it (DR5), like a temporary `String` in `view_parts`.
    fn any_borrow(&mut self, e: &'a tir::Expr) -> GenResult<StructValue<'ctx>> {
        let v = self.expr_mode(e, Mode::Use)?;
        if !is_place(e) {
            self.add_temp_value(v, &e.ty)?;
        }
        Ok(self.need(v, &e.ty).into_struct_value())
    }

    /// A method call through the vtable (AN5): `data` is the receiver, then the interface's parameters as a
    /// direct call lowers them.
    pub(super) fn any_call(
        &mut self,
        recv: &'a tir::Expr,
        iface: usize,
        slot: usize,
        params: &'a [tir::ParamSig],
        args: &'a [tir::Expr],
        ret: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let boxed = self.any_borrow(recv)?;
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let data = self.builder.build_extract_value(boxed, 0, "any.data").map_err(llvm_err)?.into_pointer_value();
        let vtable = self.builder.build_extract_value(boxed, 1, "any.vtable").map_err(llvm_err)?.into_pointer_value();
        let vtable_ty = self.vtable_struct_ty(iface);
        let slot_ptr =
            self.builder.build_struct_gep(vtable_ty, vtable, (4 + slot) as u32, "any.slot").map_err(llvm_err)?;
        let method = self.builder.build_load(ptr_ty, slot_ptr, "any.method").map_err(llvm_err)?.into_pointer_value();
        let mut fn_params: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr_ty.into()];
        let mut values: Vec<BasicMetadataValueEnum<'ctx>> = vec![data.into()];
        for (a, p) in args.iter().zip(params) {
            if p.by_ref {
                fn_params.push(ptr_ty.into());
                let ptr = match self.place_ptr(a)? {
                    Some(ptr) => ptr,
                    None => ptr_ty.get_undef(),
                };
                values.push(ptr.into());
                continue;
            }
            let m = if p.own { Mode::Move } else { Mode::Use };
            let v = self.expr_mode(a, m)?;
            if let Some(t) = self.llvm_ty(&p.ty) {
                fn_params.push(t.into());
                values.push(self.need(v, &p.ty).into());
            }
        }
        let fn_ty = match self.llvm_ty(ret) {
            Some(t) => t.fn_type(&fn_params, false),
            None => self.cx.void_type().fn_type(&fn_params, false),
        };
        let call = self.builder.build_indirect_call(fn_ty, method, &values, "").map_err(llvm_err)?;
        match self.llvm_ty(ret) {
            Some(_) => Ok(call.try_as_basic_value().basic()),
            None => Ok(None),
        }
    }

    /// `x.is<T>()`: whether the box's type identity is `type_id` (AN6).
    pub(super) fn any_is(&mut self, value: &'a tir::Expr, type_id: usize) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let iface = any_iface(&value.ty);
        let boxed = self.any_borrow(value)?;
        let vtable = self.builder.build_extract_value(boxed, 1, "any.vtable").map_err(llvm_err)?.into_pointer_value();
        let vtable_ty = self.vtable_struct_ty(iface);
        let id_ptr = self.builder.build_struct_gep(vtable_ty, vtable, 0, "any.type_id").map_err(llvm_err)?;
        let id = self.builder.build_load(self.cx.i64_type(), id_ptr, "any.type_id").map_err(llvm_err)?.into_int_value();
        let want = self.cx.i64_type().const_int(type_id as u64, false);
        let eq = self.builder.build_int_compare(IntPredicate::EQ, id, want, "any.is").map_err(llvm_err)?;
        Ok(Some(eq.into()))
    }

    /// `x.downcast<T>()` (AN6): `Some` of the boxed value when it is exactly `T`, `None` otherwise. The receiver
    /// is consumed; a mismatch drops the boxed value through the vtable before freeing the allocation.
    pub(super) fn any_downcast(
        &mut self,
        value: &'a tir::Expr,
        target: &Ty,
        type_id: usize,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let iface = any_iface(&value.ty);
        let v = self.expr_mode(value, Mode::Move)?;
        let boxed = self.need(v, &value.ty).into_struct_value();
        let data = self.builder.build_extract_value(boxed, 0, "any.data").map_err(llvm_err)?.into_pointer_value();
        let vtable = self.builder.build_extract_value(boxed, 1, "any.vtable").map_err(llvm_err)?.into_pointer_value();
        let vtable_ty = self.vtable_struct_ty(iface);
        let id_ptr = self.builder.build_struct_gep(vtable_ty, vtable, 0, "any.type_id").map_err(llvm_err)?;
        let id = self.builder.build_load(self.cx.i64_type(), id_ptr, "any.type_id").map_err(llvm_err)?.into_int_value();
        let want = self.cx.i64_type().const_int(type_id as u64, false);
        let matches = self.builder.build_int_compare(IntPredicate::EQ, id, want, "any.matches").map_err(llvm_err)?;
        let opt_ty = self.option_ty(target);
        let out = self.alloca(opt_ty.into(), "downcast")?;
        let some_bb = self.new_block("downcast.some");
        let none_bb = self.new_block("downcast.none");
        let end = self.new_block("downcast.end");
        self.builder.build_conditional_branch(matches, some_bb, none_bb).map_err(llvm_err)?;

        self.builder.position_at_end(some_bb);
        let present = self.cx.bool_type().const_int(1, false);
        let mut some =
            self.builder.build_insert_value(opt_ty.const_zero(), present, 0, "").map_err(llvm_err)?.into_struct_value();
        if let Some(t) = self.llvm_ty(target) {
            let payload = self.builder.build_load(t, data, "downcast.value").map_err(llvm_err)?;
            some = self.builder.build_insert_value(some, payload, 1, "").map_err(llvm_err)?.into_struct_value();
        }
        self.builder.build_store(out, some).map_err(llvm_err)?;
        // The new owner drops the value: free the allocation without running the concrete type's drop.
        self.builder.build_call(self.rt.free, &[data.into()], "").map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

        self.builder.position_at_end(none_bb);
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let drop_ptr = self.builder.build_struct_gep(vtable_ty, vtable, 1, "any.drop").map_err(llvm_err)?;
        let drop = self.builder.build_load(ptr_ty, drop_ptr, "any.drop").map_err(llvm_err)?.into_pointer_value();
        let drop_ty = self.cx.void_type().fn_type(&[ptr_ty.into()], false);
        self.builder.build_indirect_call(drop_ty, drop, &[data.into()], "").map_err(llvm_err)?;
        self.builder.build_call(self.rt.free, &[data.into()], "").map_err(llvm_err)?;
        self.builder.build_store(out, opt_ty.const_zero()).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(end).map_err(llvm_err)?;

        self.builder.position_at_end(end);
        Ok(Some(self.builder.build_load(opt_ty, out, "downcast").map_err(llvm_err)?))
    }
}

/// The `Program::any_ifaces` index of an `any` value's type: the vtable layout every `any` operation reads.
/// (`AnyRef::iface` is the checker's interface index, not this one.)
fn any_iface(ty: &Ty) -> usize {
    match ty {
        Ty::Any(any) => any.id,
        _ => unreachable!("an `any` operation is only built on an `any` value"),
    }
}
