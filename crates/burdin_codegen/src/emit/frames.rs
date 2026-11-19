//! The `-g` frame table (`spec/debug.md`, DBG2): one private constant entry per function that can run, and the
//! lookup `burdin_rt_frame_info` the panic backtrace calls. Every function the program can reach is listed — with
//! its Burdin name and declaration line when it has them, with its symbol name when it is generated (drop and clone
//! glue, closure records, the task runtime) — so a frame inside generated code is attributed to that code and
//! never to the Burdin function that happens to precede it. The runtime's own functions are internal and are never
//! above a visible frame, so they stay out of the table and stay droppable by `globaldce`.

use super::{Codegen, GenResult, llvm_err};
use crate::emit::reach::Reach;
use inkwell::module::Linkage;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{AddressSpace, IntPredicate};

/// One entry of the table: the function's address, its name, the source file, the declaration line, and whether
/// the backtrace stops after this frame (the process entry point). This layout is the one the runtime reads;
/// `runtime.rs` builds the same struct and the two must stay in sync.
fn entry_ty<'ctx>(cg: &Codegen<'ctx, '_>) -> inkwell::types::StructType<'ctx> {
    let (ptr_ty, usize_ty, i32_ty, i1_ty) =
        (cg.cx.ptr_type(AddressSpace::default()), cg.usize_ty, cg.cx.i32_type(), cg.cx.bool_type());
    cg.cx.struct_type(
        &[ptr_ty.into(), ptr_ty.into(), usize_ty.into(), ptr_ty.into(), usize_ty.into(), i32_ty.into(), i1_ty.into()],
        false,
    )
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    /// A table entry as an initializer: the function `value`, called `name`, declared in `file` at `line`. An
    /// empty `file` is a null pointer, for generated code with no source. `stop` marks the process entry point,
    /// where a backtrace ends: above it are the C runtime and libc, whose addresses the table cannot name.
    fn frame_entry(
        &mut self,
        value: FunctionValue<'ctx>,
        name: &[u8],
        file: &[u8],
        line: u32,
        stop: bool,
    ) -> inkwell::values::StructValue<'ctx> {
        let ptr_ty = self.cx.ptr_type(AddressSpace::default());
        let name = self.str_literal(name);
        let (file_ptr, file_len): (BasicValueEnum<'ctx>, BasicValueEnum<'ctx>) = if file.is_empty() {
            (ptr_ty.const_null().into(), self.usize_ty.const_zero().into())
        } else {
            let file = self.str_literal(file);
            (
                file.get_field_at_index(0).expect("str has a pointer"),
                file.get_field_at_index(1).expect("str has a length"),
            )
        };
        let ty = entry_ty(self);
        ty.const_named_struct(&[
            value.as_global_value().as_pointer_value().into(),
            name.get_field_at_index(0).expect("str has a pointer"),
            name.get_field_at_index(1).expect("str has a length"),
            file_ptr,
            file_len,
            self.cx.i32_type().const_int(u64::from(line), false).into(),
            self.cx.bool_type().const_int(u64::from(stop), false).into(),
        ])
    }

    /// Emits `burdin_rt_frame_info`: with `-g` a lookup over the table of every reachable function, without it a
    /// stub that answers `null`, which is what keeps the panic trace's non-`-g` format unchanged. Runs after every
    /// body exists, so the sweep sees the whole module.
    pub(super) fn emit_frame_table(&mut self, reach: &Reach) -> GenResult<()> {
        let Some(lookup) = self.module.get_function("burdin_rt_frame_info") else {
            return Err("the runtime did not declare the frame lookup".into());
        };
        let stub = |cg: &mut Self| -> GenResult<()> {
            let ptr_ty = cg.cx.ptr_type(AddressSpace::default());
            let entry = cg.cx.append_basic_block(lookup, "entry");
            cg.builder.position_at_end(entry);
            cg.builder.build_return(Some(&ptr_ty.const_null())).map_err(llvm_err)?;
            Ok(())
        };
        if self.debug.is_none() {
            return stub(self);
        }
        // The entries, in module order: the program's functions and closures with their Burdin names and source
        // lines, the generated glue and the task runtime with their symbol names.
        let program = self.program;
        let mut entries: Vec<inkwell::values::StructValue<'ctx>> = Vec::new();
        for (i, f) in program.functions.iter().enumerate() {
            if !reach.functions[i] {
                continue;
            }
            let source = self.sources.get(f.span.file);
            let line = source.line_col(f.span.lo).0;
            entries.push(self.frame_entry(self.functions[i], f.name.as_bytes(), source.path.as_bytes(), line, false));
        }
        for (i, clos) in program.closures.iter().enumerate() {
            if !reach.closures[i] {
                continue;
            }
            let source = self.sources.get(clos.func.span.file);
            let line = source.line_col(clos.func.span.lo).0;
            let name = clos.func.name.as_bytes();
            let path = source.path.as_bytes().to_vec();
            entries.push(self.frame_entry(self.closure_funcs[i], name, &path, line, false));
        }
        let generated: Vec<FunctionValue<'ctx>> = self
            .glue
            .iter()
            .map(|(_, _, _, v)| *v)
            .chain(self.closure_drops.iter().copied())
            .chain(self.blocking_thunks.iter().flatten().copied())
            .chain(self.taskrt_fns.iter().copied())
            .collect();
        for value in generated {
            let name = value.get_name().to_bytes().to_vec();
            entries.push(self.frame_entry(value, &name, b"", 0, false));
        }
        // The sweep: every other externally visible function with a body (`main`, an exported `extern "C" fn`).
        // The unreachable functions have internal linkage (they are `dead_function` stubs) and so do the
        // runtime's own functions, so neither can appear here.
        let listed: std::collections::HashSet<_> = self
            .functions
            .iter()
            .chain(self.closure_funcs.iter())
            .chain(self.closure_drops.iter())
            .chain(self.glue.iter().map(|(_, _, _, v)| v))
            .chain(self.blocking_thunks.iter().flatten())
            .chain(self.taskrt_fns.iter())
            .map(|f| f.get_name().to_string_lossy().to_string())
            .collect();
        for value in self.module.get_functions() {
            let name = value.get_name().to_string_lossy().to_string();
            if listed.contains(&name)
                || value.get_linkage() != Linkage::External
                || value.count_basic_blocks() == 0
                || self.functions.iter().any(|f| f.get_name().to_string_lossy() == name)
            {
                continue;
            }
            let name = value.get_name().to_bytes().to_vec();
            let stop = name == b"main";
            entries.push(self.frame_entry(value, &name, b"", 0, stop));
        }
        if entries.is_empty() {
            return stub(self);
        }
        let ty = entry_ty(self);
        let table_ty = ty.array_type(entries.len() as u32);
        let table = self.module.add_global(table_ty, None, "burdin.frames");
        // inkwell's `const_array` only builds arrays of arrays and its value constructors are private, so the
        // constant array of entries is built and installed with the C API directly.
        // SAFETY: every entry is a constant of `ty`, the array is a constant of `table_ty`, and the global is
        // the one just added to this module.
        unsafe {
            use inkwell::llvm_sys::core::{LLVMConstArray2, LLVMGetElementType, LLVMSetInitializer};
            use inkwell::types::AsTypeRef;
            use inkwell::values::AsValueRef;
            let mut element_refs: Vec<_> = entries.iter().map(|e| e.as_value_ref()).collect();
            let elem = LLVMGetElementType(table_ty.as_type_ref());
            assert!(!elem.is_null());
            let array = LLVMConstArray2(elem, element_refs.as_mut_ptr(), element_refs.len() as u64);
            assert!(!array.is_null());
            LLVMSetInitializer(table.as_value_ref(), array);
        }
        table.set_linkage(Linkage::Private);
        table.set_constant(true);
        table.set_unnamed_addr(true);
        let count = self.cx.i32_type().const_int(entries.len() as u64, false);
        self.frame_lookup_body(lookup, table.as_pointer_value(), table_ty, count)
    }

    /// The lookup's body: the entry whose address is the greatest one not after the program counter. Addresses
    /// never repeat, so a single pass with a running maximum is exact.
    fn frame_lookup_body(
        &mut self,
        lookup: FunctionValue<'ctx>,
        table: PointerValue<'ctx>,
        table_ty: inkwell::types::ArrayType<'ctx>,
        count: inkwell::values::IntValue<'ctx>,
    ) -> GenResult<()> {
        let (ptr_ty, i64_ty, i32_ty) =
            (self.cx.ptr_type(AddressSpace::default()), self.cx.i64_type(), self.cx.i32_type());
        let pc = lookup.get_nth_param(0).expect("the lookup takes the program counter").into_int_value();
        let entry = self.cx.append_basic_block(lookup, "entry");
        self.builder.position_at_end(entry);
        // Allocas live in this function's own entry block, built with the plain builder (`self.alloca` would
        // position them at the end of the entry block of whatever function was generated last).
        let best = self.builder.build_alloca(ptr_ty, "best").map_err(llvm_err)?;
        self.builder.build_store(best, ptr_ty.const_null()).map_err(llvm_err)?;
        let best_addr = self.builder.build_alloca(i64_ty, "best.addr").map_err(llvm_err)?;
        self.builder.build_store(best_addr, i64_ty.const_zero()).map_err(llvm_err)?;
        let index = self.builder.build_alloca(i32_ty, "i").map_err(llvm_err)?;
        self.builder.build_store(index, i32_ty.const_zero()).map_err(llvm_err)?;
        let loop_bb = self.cx.append_basic_block(lookup, "loop");
        let body_bb = self.cx.append_basic_block(lookup, "body");
        let update_bb = self.cx.append_basic_block(lookup, "update");
        let next_bb = self.cx.append_basic_block(lookup, "next");
        let done_bb = self.cx.append_basic_block(lookup, "done");
        self.builder.build_unconditional_branch(loop_bb).map_err(llvm_err)?;
        self.builder.position_at_end(loop_bb);
        let i = self.builder.build_load(i32_ty, index, "i").map_err(llvm_err)?.into_int_value();
        let more = self.builder.build_int_compare(IntPredicate::SLT, i, count, "more").map_err(llvm_err)?;
        self.builder.build_conditional_branch(more, body_bb, done_bb).map_err(llvm_err)?;
        self.builder.position_at_end(body_bb);
        let zero = i32_ty.const_zero();
        // SAFETY (LLVM): 0 <= i < count.
        let e = unsafe { self.builder.build_in_bounds_gep(table_ty, table, &[zero, i], "e") }.map_err(llvm_err)?;
        let entry_struct = entry_ty(self);
        let addr_ptr = self.builder.build_struct_gep(entry_struct, e, 0, "addr").map_err(llvm_err)?;
        let addr_ptr = self.builder.build_load(ptr_ty, addr_ptr, "").map_err(llvm_err)?.into_pointer_value();
        let addr = self.builder.build_ptr_to_int(addr_ptr, i64_ty, "addr").map_err(llvm_err)?;
        let cur = self.builder.build_load(i64_ty, best_addr, "cur").map_err(llvm_err)?.into_int_value();
        let in_range = self.builder.build_int_compare(IntPredicate::ULE, addr, pc, "in.range").map_err(llvm_err)?;
        let better = self.builder.build_int_compare(IntPredicate::UGT, addr, cur, "better").map_err(llvm_err)?;
        let hit = self.builder.build_and(in_range, better, "hit").map_err(llvm_err)?;
        self.builder.build_conditional_branch(hit, update_bb, next_bb).map_err(llvm_err)?;
        self.builder.position_at_end(update_bb);
        self.builder.build_store(best, e).map_err(llvm_err)?;
        self.builder.build_store(best_addr, addr).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(next_bb).map_err(llvm_err)?;
        self.builder.position_at_end(next_bb);
        let next = self.builder.build_int_add(i, i32_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(index, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(loop_bb).map_err(llvm_err)?;
        self.builder.position_at_end(done_bb);
        let found = self.builder.build_load(ptr_ty, best, "found").map_err(llvm_err)?;
        self.builder.build_return(Some(&found)).map_err(llvm_err)?;
        Ok(())
    }
}
