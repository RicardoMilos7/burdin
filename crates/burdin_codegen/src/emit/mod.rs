//! The code generator: one `Codegen` holds the LLVM state, and its methods are split by what they generate, one
//! file per theme.

mod aggregate;
mod builtins;
mod collections;
mod control;
mod expr;
mod function;
mod glue;
mod locals;
mod ops;
mod panics;
mod pattern;
mod place;
mod results;
mod scope;
mod strings;
mod structural;
mod types;

use crate::runtime::{self, Runtime};
use burdin_diag::SourceMap;
use burdin_tir::{self as tir, Program, Ty};
use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{TargetData, TargetMachine};
use inkwell::types::{BasicTypeEnum, IntType, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue, StructValue};
use std::collections::HashMap;

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
    temps: Vec<Vec<(PointerValue<'ctx>, Ty)>>,
    flags: Vec<(tir::MovedPlace, PointerValue<'ctx>)>,
    loops: Vec<LoopTargets<'ctx>>,
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
    rt: Runtime<'ctx>,
    functions: Vec<FunctionValue<'ctx>>,
    /// The declarations of `Program::externs`, in the same order.
    externs: Vec<FunctionValue<'ctx>>,
    str_literals: HashMap<Vec<u8>, StructValue<'ctx>>,
    /// Drop and clone glue functions generated so far, and the ones still to generate.
    glue: Vec<(Glue, Ty, FunctionValue<'ctx>)>,
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
    /// value of its type; the slot is zeroed once it is dropped, so dropping it twice is harmless.
    temps: Vec<Vec<(PointerValue<'ctx>, Ty)>>,
    /// The target is Windows (the runtime uses the Windows CRT).
    windows: bool,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    pub(crate) fn new(
        cx: &'ctx Context,
        module: &'a Module<'ctx>,
        machine: &TargetMachine,
        program: &'a Program,
        sources: &'a SourceMap,
        release: bool,
    ) -> GenResult<Self> {
        let ptr_ty = cx.ptr_type(AddressSpace::default());
        let usize_ty = cx.ptr_sized_int_type(&machine.get_target_data(), None);
        let str_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into()], false);
        let string_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into()], false);
        let windows = machine.get_triple().as_str().to_string_lossy().contains("windows");
        let rt = runtime::generate(cx, module, usize_ty, string_ty, windows)?;
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
            rt,
            functions: Vec::new(),
            externs: Vec::new(),
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
