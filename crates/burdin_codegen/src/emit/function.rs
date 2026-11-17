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
        // `extern "C"` declarations first: they are what the runtime and the program call by symbol.
        for e in &program.externs {
            let params: Vec<BasicMetadataTypeEnum> = e
                .params
                .iter()
                .map(|t| {
                    self.llvm_ty(t)
                        .map(|t| t.into())
                        .ok_or_else(|| format!("extern function `{}` has a parameter without a C value", e.symbol))
                })
                .collect::<GenResult<_>>()?;
            let fn_ty = match self.llvm_ty(&e.ret) {
                Some(t) => t.fn_type(&params, e.vararg),
                None => self.cx.void_type().fn_type(&params, e.vararg),
            };
            // The runtime already declares some libc functions (`free`, `malloc`, ...). Reuse the declaration when
            // it has the same type; a different one would silently link to the wrong ABI.
            let value = match self.module.get_function(&e.symbol) {
                Some(existing) if existing.get_type() == fn_ty => existing,
                Some(_) => {
                    return Err(format!(
                        "extern function `{}` conflicts with a runtime declaration of the same symbol",
                        e.symbol
                    ));
                }
                None => self.module.add_function(&e.symbol, fn_ty, Some(Linkage::External)),
            };
            self.externs.push(value);
        }
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
            // Burdin functions keep their symbol (`burdin.<name>`) in the executable's symbol table, so a panic
            // backtrace can name them. The `burdin.` prefix keeps them apart from C symbols. An exported function
            // takes its plain C symbol and reuses an `extern` declaration of the same symbol (issue #489).
            let name = if f.exported { f.name.clone() } else { format!("burdin.{}", f.name) };
            let value = match self.module.get_function(&name) {
                Some(existing) if existing.get_type() == fn_ty => existing,
                Some(_) => {
                    return Err(format!("exported function `{name}` conflicts with a declaration of another type"));
                }
                None => self.module.add_function(&name, fn_ty, Some(Linkage::External)),
            };
            self.functions.push(value);
        }
        // Closures: their functions are declared (and their record destructors generated) before any body that
        // may create a closure value.
        self.declare_closures()?;
        self.define_closure_drops()?;
        // A function nothing reaches is not emitted (see `reach.rs`): its declaration is enough.
        let reachable = self.reachable_functions();
        for (i, f) in program.functions.iter().enumerate() {
            if reachable[i] {
                self.function(self.functions[i], f)?;
            } else {
                self.dead_function(self.functions[i])?;
            }
        }
        for (i, clos) in program.closures.iter().enumerate() {
            self.closure_body(self.closure_funcs[i], i, clos)?;
        }
        // Drop and clone glue may declare more glue for the types they contain; generate until there is none left.
        while let Some((kind, ty, value)) = self.pending_glue.pop() {
            match kind {
                Glue::Drop => self.drop_glue_body(value, &ty)?,
                Glue::Clone => self.clone_glue_body(value, &ty)?,
            }
        }

        match program.main {
            Some(main) => crate::runtime::define_main(self.cx, self.module, self.functions[main.0], self.windows)?,
            // A test build: the entry point dispatches to one `test` block by its index on the command line.
            None => {
                let tests: Vec<_> = program.tests.iter().map(|t| self.functions[t.func.0]).collect();
                crate::runtime::define_test_main(self.cx, self.module, &tests, self.rt.leak_check, self.windows)?
            }
        }
        Ok(())
    }

    pub(super) fn function(&mut self, value: FunctionValue<'ctx>, f: &'a tir::Function) -> GenResult<()> {
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
        self.locals = &f.locals;
        self.ret = &f.ret;
        self.return_env = None;
        self.return_consume = None;
        self.loops.clear();
        self.temps = vec![Vec::new()];
        self.slots.clear();
        self.scopes = Vec::new();
        // The function scope holds the `own` parameters; the body block's locals drop before them (DR3).
        self.scopes.push(f.params.iter().copied().filter(|p| f.locals[p.0].kind == LocalKind::OwnParam).collect());
        self.flags.clear();
        self.dead_locals.clear();
        // A place the move checker proved uninitialized at every drop site never needs a flag, and its scope drop
        // is skipped (issue #427). Other places keep a runtime flag.
        for moved in &f.moved {
            if moved.always_dead {
                self.dead_locals.insert(moved.local);
            }
        }
        for moved in &f.moved {
            if self.dead_locals.contains(&moved.local) {
                continue;
            }
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
        self.debug_entry = Some(entry);
        self.debug_function(value, f);
        // A function with no expressions (a generic stub, for instance) would otherwise inherit the previous
        // function's location for its return.
        self.debug_at(f.span);
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

    /// The body of a function nothing reaches: `unreachable`, so the declaration stays defined and the IR of an
    /// unused standard-library function costs two lines.
    fn dead_function(&mut self, value: FunctionValue<'ctx>) -> GenResult<()> {
        // A function nothing reaches never runs, so nothing can see its name in a backtrace. Internal linkage
        // keeps the name in `-g` builds but leaves it out of the dynamic symbol table that `-rdynamic` exports
        // for backtraces on Linux, so the embedded standard library does not inflate every executable.
        value.set_linkage(Linkage::Internal);
        let block = self.cx.append_basic_block(value, "dead");
        self.builder.position_at_end(block);
        self.builder.build_unreachable().map_err(llvm_err)?;
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
