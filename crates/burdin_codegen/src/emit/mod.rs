//! The code generator: one `Codegen` holds the LLVM state, and its methods are split by what they generate, one
//! file per theme.

mod aggregate;
mod alloc;
mod any;
mod atomic;
mod builtins;
mod closure;
pub(crate) use debug::DebugInfo;
mod collections;
mod control;
mod debug;
mod expr;
mod frames;
mod function;
mod glue;
mod iterate;
mod locals;
mod ops;
mod panics;
mod pattern;
mod place;
mod reach;
mod results;
mod scope;
mod scripts;
mod slices;
mod strings;
mod structural;
mod tasks;
mod types;

use crate::runtime::{self, Runtime};
use burdin_diag::SourceMap;
use burdin_diag::Span;
use burdin_tir::{self as tir, Program, Ty};
use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{TargetData, TargetMachine};
use inkwell::types::{BasicTypeEnum, IntType, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, GlobalValue, IntValue, PointerValue, StructValue};
use std::collections::{HashMap, HashSet};

pub(crate) type GenResult<T> = Result<T, String>;

pub(crate) fn llvm_err(e: inkwell::builder::BuilderError) -> String {
    format!("LLVM builder error: {e}")
}

/// How an expression's value is used: `Move` consumes it, `Use` only reads or borrows it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Move,
    Use,
}

/// Which glue function a pending declaration stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Glue {
    Drop,
    Clone,
}

/// The generator state to restore after a glue function body, which is generated with its own function, locals,
/// scopes, and flags.
struct SavedContext<'ctx, 'a> {
    position: Option<BasicBlock<'ctx>>,
    entry: Option<BasicBlock<'ctx>>,
    function: Option<FunctionValue<'ctx>>,
    locals: &'a [tir::Local],
    ret: &'a Ty,
    scopes: Vec<Vec<tir::LocalId>>,
    temps: Vec<Vec<(PointerValue<'ctx>, Ty, PointerValue<'ctx>)>>,
    flags: Vec<(tir::MovedPlace, PointerValue<'ctx>)>,
    loops: Vec<LoopTargets<'ctx>>,
    debug_scope: Option<inkwell::debug_info::DISubprogram<'ctx>>,
    debug_file: Option<inkwell::debug_info::DIFile<'ctx>>,
    debug_entry: Option<BasicBlock<'ctx>>,
    /// The per-function debug state of the function the glue interrupts: its location record, the debug variables
    /// of its locals, and its kill sites (`spec/debug.md`, DBG4). Glue has no variables of its own.
    debug_fn_location: Option<inkwell::debug_info::DILocation<'ctx>>,
    debug_vars: Vec<Option<inkwell::debug_info::DILocalVariable<'ctx>>>,
    debug_kills: Vec<(burdin_tir::LocalId, Span)>,
    dead_locals: HashSet<tir::LocalId>,
    bounds_facts: Vec<(tir::LocalId, tir::LocalId)>,
    /// The capture record a `FnOnce` body owns and must free on an early `return` too.
    return_env: Option<PointerValue<'ctx>>,
    /// A `Fn`/`FnMut` body's record, its destructor, and the call's consumed flag. The body keeps the record for
    /// `Fn`/`FnMut` calls; an `FnOnce` call through a weakened type must destroy it after the body returns.
    return_consume: Option<(PointerValue<'ctx>, FunctionValue<'ctx>, IntValue<'ctx>)>,
}

struct LoopTargets<'ctx> {
    continue_bb: BasicBlock<'ctx>,
    break_bb: BasicBlock<'ctx>,
    /// The number of temporary frames open when the loop started; `break` and `continue` free the ones above.
    temp_depth: usize,
    /// The number of scopes open when the loop started; `break` and `continue` drop the ones above.
    scope_depth: usize,
}

pub(crate) struct Codegen<'ctx, 'a> {
    cx: &'ctx Context,
    module: &'a Module<'ctx>,
    program: &'a Program,
    sources: &'a SourceMap,
    release: bool,
    builder: Builder<'ctx>,
    /// Positioned at the end of the current function's `entry` block, for stack slots.
    entry_builder: Builder<'ctx>,
    usize_ty: IntType<'ctx>,
    target_data: TargetData,
    str_ty: StructType<'ctx>,
    string_ty: StructType<'ctx>,
    /// `{ code: ptr, env: ptr, drop: ptr }`: every closure value's layout.
    closure_ty: StructType<'ctx>,
    /// `{ data: ptr, vtable: ptr }`: every `any I` value's layout.
    any_ty: StructType<'ctx>,
    rt: Runtime<'ctx>,
    functions: Vec<FunctionValue<'ctx>>,
    /// The body function of each entry of `Program::closures`, in the same order.
    closure_funcs: Vec<FunctionValue<'ctx>>,
    /// The capture-record destructor of each closure, in the same order.
    closure_drops: Vec<FunctionValue<'ctx>>,
    /// The declarations of `Program::externs`, in the same order.
    externs: Vec<FunctionValue<'ctx>>,
    /// The thunk of each `blocking` extern that has been called, created on first use (`spec/io.md`, IO4).
    blocking_thunks: Vec<Option<FunctionValue<'ctx>>>,
    /// The private constant vtable of each `Program::any_vtables` entry, created on first use.
    vtables: Vec<Option<GlobalValue<'ctx>>>,
    str_literals: HashMap<Vec<u8>, StructValue<'ctx>>,
    /// Drop and clone glue functions generated so far, and the ones still to generate.
    /// Drop and clone glue functions generated so far: the kind, the structural key that decides sharing, the
    /// representative type its body is generated for, and the function.
    glue: Vec<(Glue, String, Ty, FunctionValue<'ctx>)>,
    pending_glue: Vec<(Glue, Ty, FunctionValue<'ctx>)>,
    /// Set while generating a glue body; restored when the body is done.
    context: Option<SavedContext<'ctx, 'a>>,
    // State of the function being generated.
    function: Option<FunctionValue<'ctx>>,
    locals: &'a [tir::Local],
    ret: &'a Ty,
    /// Storage of the locals (`None` for locals without a value).
    slots: Vec<Option<PointerValue<'ctx>>>,
    loops: Vec<LoopTargets<'ctx>>,
    /// One entry per block: the locals declared in it, in declaration order.
    scopes: Vec<Vec<tir::LocalId>>,
    /// The drop flags of the places `tir::Function::moved` lists, innermost path last.
    flags: Vec<(tir::MovedPlace, PointerValue<'ctx>)>,
    /// One frame per statement being generated: values to drop at the end of the statement. Each slot holds a
    /// value of its type and a live flag; the flag is set where the temporary is created and cleared once it is
    /// dropped, so a temporary created on only one branch of a `match` is not dropped on the other (DR5).
    temps: Vec<Vec<(PointerValue<'ctx>, Ty, PointerValue<'ctx>)>>,
    /// The target is Windows (the runtime uses the Windows CRT).
    windows: bool,
    /// The target triple, for the parts of the task runtime that differ per architecture.
    triple: String,
    /// The task runtime (`taskrt.rs`) has been linked into the module.
    tasks_linked: bool,
    /// The functions the task runtime's IR defined, for the `-g` frame table (`spec/debug.md`, DBG2).
    taskrt_fns: Vec<FunctionValue<'ctx>>,
    /// The operating system whose I/O error codes the program reports (`spec/errors.md`, ERR11).
    platform: tir::ioerror::Platform,
    /// The span of the expression being generated, for the allocation site a leak report names.
    current_span: Option<Span>,
    /// `-g`: the module's debug metadata, or `None`.
    debug: Option<DebugInfo<'ctx>>,
    /// The entry block of the function being generated, where debug variables are declared.
    debug_entry: Option<BasicBlock<'ctx>>,
    /// Locals the move checker proved are uninitialized at every drop site: no flag and no drop is generated
    /// for them (issue #427).
    dead_locals: HashSet<tir::LocalId>,
    /// `(loop index, collection)` pairs where the index is known to be in `0..collection.len()`: indexing that
    /// collection with that local needs no bounds check (issue #428).
    bounds_facts: Vec<(tir::LocalId, tir::LocalId)>,
    /// The capture record owned by the `FnOnce` closure body being generated, freed on every return path.
    return_env: Option<PointerValue<'ctx>>,
    /// The capture record of the `Fn`/`FnMut` closure body being generated, destroyed when a call consumes it.
    return_consume: Option<(PointerValue<'ctx>, FunctionValue<'ctx>, IntValue<'ctx>)>,
    /// The opaque inline-asm call `black_box` passes a value's stack slot through (`spec/bench.md`, BN9), created
    /// on first use.
    blackbox_asm: Option<PointerValue<'ctx>>,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(crate) fn new(
        cx: &'ctx Context,
        module: &'a Module<'ctx>,
        machine: &TargetMachine,
        program: &'a Program,
        sources: &'a SourceMap,
        release: bool,
        debug: Option<DebugInfo<'ctx>>,
    ) -> GenResult<Self> {
        let ptr_ty = cx.ptr_type(AddressSpace::default());
        let usize_ty = cx.ptr_sized_int_type(&machine.get_target_data(), None);
        let str_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into()], false);
        let string_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into()], false);
        let closure_ty = cx.struct_type(&[ptr_ty.into(), ptr_ty.into(), ptr_ty.into()], false);
        let any_ty = cx.struct_type(&[ptr_ty.into(), ptr_ty.into()], false);
        let triple = machine.get_triple().as_str().to_string_lossy().to_string();
        let windows = triple.contains("windows");
        let darwin = triple.contains("darwin") || triple.contains("apple");
        let platform = tir::ioerror::platform(windows, darwin);
        // The checking allocator is a `burdin test` build's (`spec/allocators.md`, AL9): a bench build measures the
        // plain C heap, so its numbers are not the allocator's bookkeeping.
        let checking = program.main.is_none() && program.benches.is_empty();
        let rt = runtime::generate(cx, module, usize_ty, string_ty, windows, darwin, checking)?;
        Ok(Codegen {
            cx,
            module,
            program,
            sources,
            release,
            builder: cx.create_builder(),
            entry_builder: cx.create_builder(),
            usize_ty,
            target_data: machine.get_target_data(),
            str_ty,
            string_ty,
            closure_ty,
            any_ty,
            rt,
            functions: Vec::new(),
            closure_funcs: Vec::new(),
            closure_drops: Vec::new(),
            externs: Vec::new(),
            vtables: vec![None; program.any_vtables.len()],
            str_literals: HashMap::new(),
            glue: Vec::new(),
            pending_glue: Vec::new(),
            context: None,
            function: None,
            locals: &[],
            ret: &Ty::Unit,
            slots: Vec::new(),
            loops: Vec::new(),
            scopes: Vec::new(),
            flags: Vec::new(),
            temps: Vec::new(),
            windows,
            triple,
            tasks_linked: false,
            taskrt_fns: Vec::new(),
            blocking_thunks: vec![None; program.externs.len()],
            platform,
            current_span: None,
            debug,
            debug_entry: None,
            dead_locals: HashSet::new(),
            bounds_facts: Vec::new(),
            return_env: None,
            return_consume: None,
            blackbox_asm: None,
        })
    }
}

fn undef(t: BasicTypeEnum<'_>) -> BasicValueEnum<'_> {
    match t {
        BasicTypeEnum::ArrayType(t) => t.get_undef().into(),
        BasicTypeEnum::FloatType(t) => t.get_undef().into(),
        BasicTypeEnum::IntType(t) => t.get_undef().into(),
        BasicTypeEnum::PointerType(t) => t.get_undef().into(),
        BasicTypeEnum::StructType(t) => t.get_undef().into(),
        BasicTypeEnum::VectorType(t) => t.get_undef().into(),
        BasicTypeEnum::ScalableVectorType(t) => t.get_undef().into(),
    }
}
