//! Functions: the program, one function body, and the helpers every body uses.

use super::{Codegen, GenResult, Glue, Mode, llvm_err, undef};
use burdin_tir::{self as tir, LocalKind, Ty};
use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::module::Linkage;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{BasicValueEnum, FunctionValue, IntValue, PointerValue};

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(crate) fn program(&mut self) -> GenResult<()> {
        let program = self.program;
        for f in &program.functions {
            let ptr_ty = self.cx.ptr_type(AddressSpace::default());
            let params: Vec<BasicMetadataTypeEnum> = f
                .params
                .iter()
                .filter_map(|p| {
                    let local = &f.locals[p.0];
                    if local.by_ref { Some(ptr_ty.into()) } else { self.llvm_ty(&local.ty) }
                })
                .map(|t| t.into())
                .collect();
            let fn_ty = match self.llvm_ty(&f.ret) {
                Some(t) => t.fn_type(&params, false),
                None => self.cx.void_type().fn_type(&params, false),
            };
            let value = self.module.add_function(&format!("burdin.{}", f.name), fn_ty, Some(Linkage::Internal));
            self.functions.push(value);
        }
        for (i, f) in program.functions.iter().enumerate() {
            self.function(self.functions[i], f)?;
        }
        // Drop and clone glue may declare more glue for the types they contain; generate until there is none left.
        while let Some((kind, ty, value)) = self.pending_glue.pop() {
            match kind {
                Glue::Drop => self.drop_glue_body(value, &ty)?,
                Glue::Clone => self.clone_glue_body(value, &ty)?,
            }
        }

        crate::runtime::define_main(
            self.cx,
            self.module,
            self.functions[program.main.expect("the checker reports a missing `main`").0],
            self.windows,
        )
    }

    pub(super) fn function(&mut self, value: FunctionValue<'ctx>, f: &'a tir::Function) -> GenResult<()> {
        let entry = self.cx.append_basic_block(value, "entry");
        let body = self.cx.append_basic_block(value, "body");
        self.entry_builder.position_at_end(entry);
        self.builder.position_at_end(body);
        self.function = Some(value);
        self.locals = &f.locals;
        self.ret = &f.ret;
        self.loops.clear();
        self.temps = vec![Vec::new()];
        self.slots.clear();
        self.scopes = Vec::new();
        // The function scope holds the `own` parameters; the body block's locals drop before them (DR3).
        self.scopes.push(f.params.iter().copied().filter(|p| f.locals[p.0].kind == LocalKind::OwnParam).collect());
        self.flags.clear();
        for moved in &f.moved {
            let flag = self.entry_builder.build_alloca(self.cx.bool_type(), "moved").map_err(llvm_err)?;
            let one = self.cx.bool_type().const_int(1, false);
            self.entry_builder.build_store(flag, one).map_err(llvm_err)?;
            self.flags.push((moved.clone(), flag));
        }
        for local in &f.locals {
            let slot = match self.llvm_ty(&local.ty) {
                Some(t) if !local.by_ref => Some(self.entry_builder.build_alloca(t, &local.name).map_err(llvm_err)?),
                _ => None,
            };
            self.slots.push(slot);
        }
        let mut arg_index = 0;
        for p in &f.params {
            if f.locals[p.0].by_ref {
                let arg = value.get_nth_param(arg_index).expect("parameter count matches the signature");
                self.slots[p.0] = Some(arg.into_pointer_value());
                arg_index += 1;
            } else if let Some(slot) = self.slots[p.0] {
                let arg = value.get_nth_param(arg_index).expect("parameter count matches the signature");
                self.builder.build_store(slot, arg).map_err(llvm_err)?;
                arg_index += 1;
            }
        }
        let result = self.block(&f.body, Mode::Move)?;
        self.free_temps_from(0)?;
        // The body block dropped its own locals; only the parameters are left in their scope.
        self.drop_scope(0)?;
        match self.llvm_ty(&f.ret) {
            Some(_) => match result {
                Some(v) => self.builder.build_return(Some(&v)).map_err(llvm_err)?,
                // The body diverges: this point is never reached.
                None => self.builder.build_unreachable().map_err(llvm_err)?,
            },
            None => self.builder.build_return(None).map_err(llvm_err)?,
        };
        self.entry_builder.build_unconditional_branch(body).map_err(llvm_err)?;
        Ok(())
    }

    pub(super) fn current_fn(&self) -> FunctionValue<'ctx> {
        self.function.expect("inside a function")
    }

    pub(super) fn new_block(&self, name: &str) -> BasicBlock<'ctx> {
        self.cx.append_basic_block(self.current_fn(), name)
    }

    /// Continues in a fresh block with no predecessors, after a terminator.
    pub(super) fn continue_dead(&mut self) {
        let bb = self.new_block("dead");
        self.builder.position_at_end(bb);
    }

    pub(super) fn alloca(&self, ty: BasicTypeEnum<'ctx>, name: &str) -> GenResult<PointerValue<'ctx>> {
        self.entry_builder.build_alloca(ty, name).map_err(llvm_err)
    }

    /// `v`, or an undefined value of `ty` in unreachable code where the operand diverged.
    pub(super) fn need(&self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> BasicValueEnum<'ctx> {
        match v {
            Some(v) => v,
            None => {
                let t = self.llvm_ty(ty).unwrap_or_else(|| self.cx.bool_type().into());
                undef(t)
            }
        }
    }

    pub(super) fn need_int(&self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> IntValue<'ctx> {
        self.need(v, ty).into_int_value()
    }
}
