//! LLVM code generation from the typed tree.
//!
//! Representation: integers are `iN` (`isize` and `usize` are pointer-sized), `char` is `i32`, `bool` is `i1`, `str`
//! is `{ ptr, usize }`, `String` is `{ ptr, usize len, usize cap }`, arrays are LLVM arrays, and `Option<T>` is
//! `{ i1 is_some, T }`. Tuples and structs are LLVM structs with one member per field, in declaration order (a field
//! without a value, such as `()`, is an empty struct, so member indices are field indices). An enum is
//! `{ i32 tag, [N x iA] payload }`, where the payload is sized and aligned for the largest variant and is read
//! through a pointer as the variant's own struct of fields. `()` and `!` have no value.
//!
//! A `mut self` receiver is passed by address: the callee's `self` local uses the caller's place as its storage. Functions are internal and named `burdin.<name>`. The entry point
//! `burdin_entry` calls the program's `main`; the C runtime (runtime/rt.c) defines the real `main`, output, string
//! conversions, and the panic path.
//!
//! Every function starts with an `entry` block that holds only stack slots and their initialization, and branches to
//! the body. After a terminator (`return`, `break`, a panic, ...) code generation continues in a fresh block without
//! predecessors, so the rest of the tree can be generated without special cases.

use burdin_diag::{SourceMap, Span};
use burdin_sema::tir::{self, BinOp, Builtin, FloatTy, IntTy, Method, Pat, Program, Ty, UnOp};
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::intrinsics::Intrinsic;
use inkwell::module::{Linkage, Module};
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetData, TargetMachine};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, IntType, StructType};
use inkwell::values::{BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue, StructValue};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate, OptimizationLevel};
use std::collections::HashMap;
use std::path::Path;

pub struct Options {
    pub release: bool,
    /// Also write the LLVM IR as text to this path.
    pub emit_ir: Option<std::path::PathBuf>,
}

/// Compiles the program to a native object file for the host. `sources` gives panic messages their file and line.
pub fn emit_object(
    program: &Program,
    sources: &SourceMap,
    module_name: &str,
    object: &Path,
    opts: &Options,
) -> Result<(), String> {
    Target::initialize_native(&InitializationConfig::default())?;
    let triple = TargetMachine::get_default_triple();
    let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
    let level = if opts.release { OptimizationLevel::Default } else { OptimizationLevel::None };
    let machine = target
        .create_target_machine(&triple, "generic", "", level, RelocMode::PIC, CodeModel::Default)
        .ok_or_else(|| format!("LLVM cannot create a target machine for {}", triple.as_str().to_string_lossy()))?;

    let context = Context::create();
    let module = context.create_module(module_name);
    module.set_triple(&triple);
    module.set_data_layout(&machine.get_target_data().get_data_layout());

    let mut cg = Codegen::new(&context, &module, &machine, program, sources, opts.release);
    cg.program()?;

    if let Some(path) = &opts.emit_ir {
        module.print_to_file(path).map_err(|e| e.to_string())?;
    }
    module.verify().map_err(|e| format!("LLVM module verification failed:\n{}", e.to_string_lossy()))?;
    if opts.release {
        module
            .run_passes("default<O2>", &machine, inkwell::passes::PassBuilderOptions::create())
            .map_err(|e| e.to_string_lossy().to_string())?;
    }
    machine.write_to_file(&module, FileType::Object, object).map_err(|e| e.to_string_lossy().to_string())
}

type GenResult<T> = Result<T, String>;

fn llvm_err(e: inkwell::builder::BuilderError) -> String {
    format!("LLVM builder error: {e}")
}

/// Functions of the C runtime.
struct Runtime<'ctx> {
    print: FunctionValue<'ctx>,
    panic: FunctionValue<'ctx>,
    panic_bounds: FunctionValue<'ctx>,
    string_free: FunctionValue<'ctx>,
    str_to_string: FunctionValue<'ctx>,
    i64_to_string: FunctionValue<'ctx>,
    u64_to_string: FunctionValue<'ctx>,
    char_to_string: FunctionValue<'ctx>,
}

struct LoopTargets<'ctx> {
    continue_bb: BasicBlock<'ctx>,
    break_bb: BasicBlock<'ctx>,
    /// The number of temporary frames open when the loop started; `break` and `continue` free the ones above.
    temp_depth: usize,
}

struct Codegen<'ctx, 'a> {
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
    str_literals: HashMap<Vec<u8>, StructValue<'ctx>>,
    // State of the function being generated.
    function: Option<FunctionValue<'ctx>>,
    locals: &'a [tir::Local],
    ret: &'a Ty,
    /// Storage of the locals (`None` for locals without a value).
    slots: Vec<Option<PointerValue<'ctx>>>,
    loops: Vec<LoopTargets<'ctx>>,
    /// One frame per statement being generated: `String` temporaries to free at the end of the statement. Each slot
    /// holds an empty string until the temporary is created, so freeing a slot that was not reached is harmless.
    temps: Vec<Vec<PointerValue<'ctx>>>,
}

impl<'ctx, 'a> Codegen<'ctx, 'a> {
    fn new(
        cx: &'ctx Context,
        module: &'a Module<'ctx>,
        machine: &TargetMachine,
        program: &'a Program,
        sources: &'a SourceMap,
        release: bool,
    ) -> Self {
        let ptr_ty = cx.ptr_type(AddressSpace::default());
        let usize_ty = cx.ptr_sized_int_type(&machine.get_target_data(), None);
        let str_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into()], false);
        let string_ty = cx.struct_type(&[ptr_ty.into(), usize_ty.into(), usize_ty.into()], false);
        let void = cx.void_type();
        let i32_ty = cx.i32_type();
        let i64_ty = cx.i64_type();
        let declare = |name: &str, ty| module.add_function(name, ty, Some(Linkage::External));
        let noreturn = cx.create_enum_attribute(Attribute::get_named_enum_kind_id("noreturn"), 0);
        let panic_tail = [ptr_ty.into(), usize_ty.into(), i32_ty.into(), i32_ty.into()];
        let rt = Runtime {
            print: declare(
                "burdin_rt_print",
                void.fn_type(&[i32_ty.into(), ptr_ty.into(), usize_ty.into(), i32_ty.into()], false),
            ),
            panic: declare(
                "burdin_rt_panic",
                void.fn_type(&[&[ptr_ty.into(), usize_ty.into()][..], &panic_tail[..]].concat(), false),
            ),
            panic_bounds: declare(
                "burdin_rt_panic_bounds",
                void.fn_type(&[&[i64_ty.into(), i64_ty.into()][..], &panic_tail[..]].concat(), false),
            ),
            string_free: declare("burdin_rt_string_free", void.fn_type(&[ptr_ty.into(), usize_ty.into()], false)),
            str_to_string: declare(
                "burdin_rt_str_to_string",
                void.fn_type(&[ptr_ty.into(), usize_ty.into(), ptr_ty.into()], false),
            ),
            i64_to_string: declare("burdin_rt_i64_to_string", void.fn_type(&[i64_ty.into(), ptr_ty.into()], false)),
            u64_to_string: declare("burdin_rt_u64_to_string", void.fn_type(&[i64_ty.into(), ptr_ty.into()], false)),
            char_to_string: declare("burdin_rt_char_to_string", void.fn_type(&[i32_ty.into(), ptr_ty.into()], false)),
        };
        rt.panic.add_attribute(AttributeLoc::Function, noreturn);
        rt.panic_bounds.add_attribute(AttributeLoc::Function, noreturn);
        Codegen {
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
            str_literals: HashMap::new(),
            function: None,
            locals: &[],
            ret: &Ty::Unit,
            slots: Vec::new(),
            loops: Vec::new(),
            temps: Vec::new(),
        }
    }

    // ----- types -------------------------------------------------------------

    fn int_ty(&self, i: IntTy) -> IntType<'ctx> {
        match i {
            IntTy::Isize | IntTy::Usize => self.usize_ty,
            IntTy::I8 | IntTy::U8 => self.cx.i8_type(),
            IntTy::I16 | IntTy::U16 => self.cx.i16_type(),
            IntTy::I32 | IntTy::U32 => self.cx.i32_type(),
            IntTy::I64 | IntTy::U64 => self.cx.i64_type(),
        }
    }

    /// The LLVM type of a value of `ty`, or `None` for types without a runtime value.
    fn llvm_ty(&self, ty: &Ty) -> Option<BasicTypeEnum<'ctx>> {
        Some(match ty {
            Ty::Unit | Ty::Never | Ty::Error => return None,
            Ty::Bool => self.cx.bool_type().into(),
            Ty::Char => self.cx.i32_type().into(),
            Ty::Int(i) => self.int_ty(*i).into(),
            Ty::Float(FloatTy::F32) => self.cx.f32_type().into(),
            Ty::Float(FloatTy::F64) => self.cx.f64_type().into(),
            Ty::Str => self.str_ty.into(),
            Ty::String => self.string_ty.into(),
            Ty::Array(elem, n) => self.llvm_ty(elem)?.array_type(*n as u32).into(),
            Ty::Option(inner) => self.option_ty(inner).into(),
            Ty::Tuple(_) | Ty::Struct(_) => self.product_ty(&self.field_tys(ty)).into(),
            Ty::Enum(a) => self.enum_ty(a.id).into(),
        })
    }

    /// The type of a member of an aggregate: like `llvm_ty`, but a value-less type is an empty struct.
    fn storage_ty(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        self.llvm_ty(ty).unwrap_or_else(|| self.cx.struct_type(&[], false).into())
    }

    fn product_ty(&self, fields: &[Ty]) -> StructType<'ctx> {
        let members: Vec<BasicTypeEnum<'ctx>> = fields.iter().map(|t| self.storage_ty(t)).collect();
        self.cx.struct_type(&members, false)
    }

    /// The field types of a tuple or struct.
    fn field_tys(&self, ty: &Ty) -> Vec<Ty> {
        match ty {
            Ty::Tuple(elems) => elems.clone(),
            Ty::Struct(a) => self.program.structs[a.id].fields.iter().map(|f| f.ty.clone()).collect(),
            _ => Vec::new(),
        }
    }

    fn variant_tys(&self, enum_id: usize, variant: usize) -> Vec<Ty> {
        self.program.enums[enum_id].variants[variant].fields.iter().map(|f| f.ty.clone()).collect()
    }

    fn variant_ty(&self, enum_id: usize, variant: usize) -> StructType<'ctx> {
        self.product_ty(&self.variant_tys(enum_id, variant))
    }

    fn enum_ty(&self, enum_id: usize) -> StructType<'ctx> {
        let tag = self.cx.i32_type().into();
        let mut size = 0u64;
        let mut align = 1u32;
        for (i, v) in self.program.enums[enum_id].variants.iter().enumerate() {
            if v.fields.is_empty() {
                continue;
            }
            let vt = self.variant_ty(enum_id, i);
            size = size.max(self.target_data.get_abi_size(&vt));
            align = align.max(self.target_data.get_abi_alignment(&vt));
        }
        if size == 0 {
            return self.cx.struct_type(&[tag], false);
        }
        let align = align.min(8);
        let unit = match align {
            1 => self.cx.i8_type(),
            2 => self.cx.i16_type(),
            4 => self.cx.i32_type(),
            _ => self.cx.i64_type(),
        };
        let count = size.div_ceil(u64::from(align));
        self.cx.struct_type(&[tag, unit.array_type(count as u32).into()], false)
    }

    fn option_ty(&self, inner: &Ty) -> StructType<'ctx> {
        match self.llvm_ty(inner) {
            Some(t) => self.cx.struct_type(&[self.cx.bool_type().into(), t], false),
            None => self.cx.struct_type(&[self.cx.bool_type().into()], false),
        }
    }

    // ----- functions -----------------------------------------------------------

    fn program(&mut self) -> GenResult<()> {
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

        let entry = self.module.add_function("burdin_entry", self.cx.void_type().fn_type(&[], false), None);
        let bb = self.cx.append_basic_block(entry, "entry");
        self.builder.position_at_end(bb);
        self.builder.build_call(self.functions[program.main.0], &[], "").map_err(llvm_err)?;
        self.builder.build_return(None).map_err(llvm_err)?;
        Ok(())
    }

    fn function(&mut self, value: FunctionValue<'ctx>, f: &'a tir::Function) -> GenResult<()> {
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
        let result = self.block(&f.body)?;
        self.free_temps_from(0)?;
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

    fn current_fn(&self) -> FunctionValue<'ctx> {
        self.function.expect("inside a function")
    }

    fn new_block(&self, name: &str) -> BasicBlock<'ctx> {
        self.cx.append_basic_block(self.current_fn(), name)
    }

    /// Continues in a fresh block with no predecessors, after a terminator.
    fn continue_dead(&mut self) {
        let bb = self.new_block("dead");
        self.builder.position_at_end(bb);
    }

    fn alloca(&self, ty: BasicTypeEnum<'ctx>, name: &str) -> GenResult<PointerValue<'ctx>> {
        self.entry_builder.build_alloca(ty, name).map_err(llvm_err)
    }

    /// `v`, or an undefined value of `ty` in unreachable code where the operand diverged.
    fn need(&self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> BasicValueEnum<'ctx> {
        match v {
            Some(v) => v,
            None => {
                let t = self.llvm_ty(ty).unwrap_or_else(|| self.cx.bool_type().into());
                undef(t)
            }
        }
    }

    fn need_int(&self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> IntValue<'ctx> {
        self.need(v, ty).into_int_value()
    }

    // ----- statements and temporaries ------------------------------------------

    fn block(&mut self, b: &'a tir::Block) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        for s in &b.stmts {
            self.temps.push(Vec::new());
            match s {
                tir::Stmt::Let(Pat::Binding(local), init, None) => {
                    let v = self.expr(init)?;
                    if let (Some(slot), Some(v)) = (self.slots[local.0], v) {
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                    }
                }
                tir::Stmt::Let(pat, init, else_block) => {
                    let v = self.expr(init)?;
                    let place = self.spill(v, &init.ty)?;
                    if let Some(else_block) = else_block {
                        let ok = self.pat_test(pat, place, &init.ty)?;
                        let bind_bb = self.new_block("let.bind");
                        let else_bb = self.new_block("let.else");
                        self.builder.build_conditional_branch(ok, bind_bb, else_bb).map_err(llvm_err)?;
                        self.builder.position_at_end(else_bb);
                        // The checker proved the block diverges.
                        self.block(else_block)?;
                        self.builder.build_unreachable().map_err(llvm_err)?;
                        self.builder.position_at_end(bind_bb);
                    }
                    self.pat_bind(pat, place, &init.ty)?;
                }
                tir::Stmt::Expr(e) => {
                    self.expr(e)?;
                }
            }
            let depth = self.temps.len() - 1;
            self.free_temps_from(depth)?;
            self.temps.pop();
        }
        match &b.tail {
            Some(e) => self.expr(e),
            None => Ok(None),
        }
    }

    /// Frees the temporaries of the frames from `depth` up, leaving their slots empty.
    fn free_temps_from(&mut self, depth: usize) -> GenResult<()> {
        let slots: Vec<PointerValue<'ctx>> = self.temps[depth..].iter().flatten().copied().collect();
        for slot in slots {
            let s = self.builder.build_load(self.string_ty, slot, "tmp").map_err(llvm_err)?.into_struct_value();
            let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
            let cap = self.builder.build_extract_value(s, 2, "").map_err(llvm_err)?;
            self.builder.build_call(self.rt.string_free, &[ptr.into(), cap.into()], "").map_err(llvm_err)?;
            self.builder.build_store(slot, self.string_ty.const_zero()).map_err(llvm_err)?;
        }
        Ok(())
    }

    /// Records a `String` temporary to free at the end of the current statement.
    fn add_temp(&mut self, v: BasicValueEnum<'ctx>) -> GenResult<()> {
        let slot = self.alloca(self.string_ty.into(), "tmp")?;
        self.entry_builder.build_store(slot, self.string_ty.const_zero()).map_err(llvm_err)?;
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        self.temps.last_mut().expect("a temporary frame is open").push(slot);
        Ok(())
    }

    // ----- expressions ---------------------------------------------------------

    fn expr(&mut self, e: &'a tir::Expr) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        use tir::ExprKind as K;
        let v: Option<BasicValueEnum<'ctx>> = match &e.kind {
            K::Unit => None,
            K::Bool(b) => Some(self.cx.bool_type().const_int(u64::from(*b), false).into()),
            K::Int(v) => match &e.ty {
                // Truncating to 64 bits keeps the two's complement bits of a negative value.
                Ty::Int(i) => Some(self.int_ty(*i).const_int(*v as u64, false).into()),
                _ => None,
            },
            K::Float(v) => match &e.ty {
                Ty::Float(FloatTy::F32) => Some(self.cx.f32_type().const_float(*v).into()),
                _ => Some(self.cx.f64_type().const_float(*v).into()),
            },
            K::Char(c) => Some(self.cx.i32_type().const_int(u64::from(*c as u32), false).into()),
            K::Str(s) => Some(self.str_literal(s.as_bytes()).into()),
            K::Local(id) => match (self.slots[id.0], self.llvm_ty(&e.ty)) {
                (Some(slot), Some(t)) => {
                    Some(self.builder.build_load(t, slot, &self.locals[id.0].name).map_err(llvm_err)?)
                }
                _ => None,
            },
            K::Const(id) => {
                let program = self.program;
                self.expr(&program.consts[id.0].value)?
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
                    let v = self.expr(a)?;
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
            K::Builtin(b, args) => self.builtin(*b, args, e.span)?,
            K::Method(m, args) => self.method(*m, args, &e.ty, e.span)?,
            K::StringAsStr(inner) => {
                let is_place = is_place(inner);
                let s = self.expr(inner)?;
                let s = self.need(s, &Ty::String);
                if !is_place {
                    self.add_temp(s)?;
                }
                let s = s.into_struct_value();
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                Some(self.make_str(ptr, len)?.into())
            }
            K::Block(b) => self.block(b)?,
            K::If(cond, then, other) => self.if_expr(cond, then, other.as_deref(), &e.ty)?,
            K::While(cond, body) => {
                let header = self.new_block("while.cond");
                let body_bb = self.new_block("while.body");
                let exit = self.new_block("while.end");
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(header);
                let c = self.expr(cond)?;
                let c = self.need_int(c, &Ty::Bool);
                self.builder.build_conditional_branch(c, body_bb, exit).map_err(llvm_err)?;
                self.builder.position_at_end(body_bb);
                self.loop_body(body, header, exit)?;
                self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
                self.builder.position_at_end(exit);
                None
            }
            K::Loop(body) => {
                let body_bb = self.new_block("loop.body");
                let exit = self.new_block("loop.end");
                self.builder.build_unconditional_branch(body_bb).map_err(llvm_err)?;
                self.builder.position_at_end(body_bb);
                self.loop_body(body, body_bb, exit)?;
                self.builder.build_unconditional_branch(body_bb).map_err(llvm_err)?;
                self.builder.position_at_end(exit);
                None
            }
            K::ForRange { var, lo, hi, inclusive, body } => {
                self.for_range(*var, lo, hi, *inclusive, body)?;
                None
            }
            K::ForArray { pat, array, body } => {
                self.for_array(pat, array, body)?;
                None
            }
            K::Match(scrutinee, arms) => self.match_expr(scrutinee, arms, &e.ty)?,
            K::Break | K::Continue => {
                let target = self.loops.last().expect("checked: inside a loop");
                let (bb, depth) = match e.kind {
                    K::Break => (target.break_bb, target.temp_depth),
                    _ => (target.continue_bb, target.temp_depth),
                };
                self.free_temps_from(depth)?;
                self.builder.build_unconditional_branch(bb).map_err(llvm_err)?;
                self.continue_dead();
                None
            }
            K::Return(value) => {
                let v = match value {
                    Some(v) => self.expr(v)?,
                    None => None,
                };
                self.free_temps_from(0)?;
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
                let v = self.expr(value)?;
                if let Some(ptr) = self.place_ptr(place)?
                    && self.llvm_ty(&place.ty).is_some()
                {
                    let v = self.need(v, &place.ty);
                    self.builder.build_store(ptr, v).map_err(llvm_err)?;
                }
                None
            }
            K::Binary(op, l, r) => self.binary(*op, l, r, e.span)?,
            K::Unary(op, operand) => {
                let v = self.expr(operand)?;
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
                let v = self.expr(operand)?;
                match self.llvm_ty(&operand.ty) {
                    Some(_) => {
                        let v = self.need(v, &operand.ty);
                        Some(self.cast(v, &operand.ty, &e.ty)?)
                    }
                    None => None,
                }
            }
            K::Array(elems) => {
                let Some(t) = self.llvm_ty(&e.ty) else {
                    for el in elems {
                        self.expr(el)?;
                    }
                    return Ok(None);
                };
                let mut agg = undef(t).into_array_value();
                for (i, el) in elems.iter().enumerate() {
                    let v = self.expr(el)?;
                    let v = self.need(v, &el.ty);
                    agg = self.builder.build_insert_value(agg, v, i as u32, "").map_err(llvm_err)?.into_array_value();
                }
                Some(agg.into())
            }
            K::ArrayRepeat(value, n) => {
                let v = self.expr(value)?;
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
                let v = self.expr(inner)?;
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
            K::Index(..) | K::Field(..) => {
                let ptr = self.place_ptr(e)?;
                match (ptr, self.llvm_ty(&e.ty)) {
                    (Some(ptr), Some(t)) => Some(self.builder.build_load(t, ptr, "").map_err(llvm_err)?),
                    _ => None,
                }
            }
        };
        if e.ty == Ty::Never {
            return Ok(None);
        }
        Ok(v)
    }

    fn loop_body(
        &mut self,
        body: &'a tir::Block,
        continue_bb: BasicBlock<'ctx>,
        break_bb: BasicBlock<'ctx>,
    ) -> GenResult<()> {
        self.loops.push(LoopTargets { continue_bb, break_bb, temp_depth: self.temps.len() });
        self.block(body)?;
        self.loops.pop();
        Ok(())
    }

    fn for_range(
        &mut self,
        var: Option<tir::LocalId>,
        lo: &'a tir::Expr,
        hi: &'a tir::Expr,
        inclusive: bool,
        body: &'a tir::Block,
    ) -> GenResult<()> {
        let Ty::Int(ity) = lo.ty.clone() else {
            // The range diverges (for example `return..x`); nothing after it runs.
            self.expr(lo)?;
            self.expr(hi)?;
            return Ok(());
        };
        let signed = ity.signed();
        let lo_v = self.expr(lo)?;
        let lo_v = self.need_int(lo_v, &lo.ty);
        let hi_v = self.expr(hi)?;
        let hi_v = self.need_int(hi_v, &lo.ty);
        let counter = self.alloca(self.int_ty(ity).into(), "for.counter")?;
        self.builder.build_store(counter, lo_v).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let cur = self.builder.build_load(self.int_ty(ity), counter, "i").map_err(llvm_err)?.into_int_value();
        let pred = match (inclusive, signed) {
            (true, true) => IntPredicate::SLE,
            (true, false) => IntPredicate::ULE,
            (false, true) => IntPredicate::SLT,
            (false, false) => IntPredicate::ULT,
        };
        let go = self.builder.build_int_compare(pred, cur, hi_v, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        if let Some(var) = var
            && let Some(slot) = self.slots[var.0]
        {
            self.builder.build_store(slot, cur).map_err(llvm_err)?;
        }
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        // `cur < hi` (or `cur != hi` for an inclusive range) holds here, so the increment cannot overflow.
        self.builder.position_at_end(latch);
        let cur = self.builder.build_load(self.int_ty(ity), counter, "i").map_err(llvm_err)?.into_int_value();
        let inc = self.new_block("for.inc");
        if inclusive {
            let last = self.builder.build_int_compare(IntPredicate::EQ, cur, hi_v, "").map_err(llvm_err)?;
            self.builder.build_conditional_branch(last, exit, inc).map_err(llvm_err)?;
        } else {
            self.builder.build_unconditional_branch(inc).map_err(llvm_err)?;
        }
        self.builder.position_at_end(inc);
        let one = self.int_ty(ity).const_int(1, false);
        let next = self.builder.build_int_add(cur, one, "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(exit);
        Ok(())
    }

    fn if_expr(
        &mut self,
        cond: &'a tir::Expr,
        then: &'a tir::Block,
        other: Option<&'a tir::Expr>,
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let c = self.expr(cond)?;
        let c = self.need_int(c, &Ty::Bool);
        let result = match self.llvm_ty(ty) {
            Some(t) => Some(self.alloca(t, "if.result")?),
            None => None,
        };
        let then_bb = self.new_block("if.then");
        let else_bb = self.new_block("if.else");
        let merge = self.new_block("if.end");
        self.builder.build_conditional_branch(c, then_bb, else_bb).map_err(llvm_err)?;

        self.builder.position_at_end(then_bb);
        let v = self.block(then)?;
        if let (Some(slot), Some(v)) = (result, v) {
            self.builder.build_store(slot, v).map_err(llvm_err)?;
        }
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;

        self.builder.position_at_end(else_bb);
        if let Some(other) = other {
            let v = self.expr(other)?;
            if let (Some(slot), Some(v)) = (result, v) {
                self.builder.build_store(slot, v).map_err(llvm_err)?;
            }
        }
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;

        self.builder.position_at_end(merge);
        match (result, self.llvm_ty(ty)) {
            (Some(slot), Some(t)) => Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?)),
            _ => Ok(None),
        }
    }

    fn match_expr(
        &mut self,
        scrutinee: &'a tir::Expr,
        arms: &'a [tir::Arm],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let v = self.expr(scrutinee)?;
        let place = match self.llvm_ty(&scrutinee.ty) {
            Some(t) => {
                let slot = self.alloca(t, "match.value")?;
                let v = self.need(v, &scrutinee.ty);
                self.builder.build_store(slot, v).map_err(llvm_err)?;
                Some(slot)
            }
            None => None,
        };
        let result = match self.llvm_ty(ty) {
            Some(t) => Some(self.alloca(t, "match.result")?),
            None => None,
        };
        let merge = self.new_block("match.end");
        for arm in arms {
            let arm_bb = self.new_block("match.arm");
            let next = self.new_block("match.next");
            let ok = self.pat_test(&arm.pat, place, &scrutinee.ty)?;
            self.builder.build_conditional_branch(ok, arm_bb, next).map_err(llvm_err)?;
            self.builder.position_at_end(arm_bb);
            self.pat_bind(&arm.pat, place, &scrutinee.ty)?;
            if let Some(guard) = &arm.guard {
                let g = self.expr(guard)?;
                let g = self.need_int(g, &Ty::Bool);
                let body_bb = self.new_block("match.body");
                self.builder.build_conditional_branch(g, body_bb, next).map_err(llvm_err)?;
                self.builder.position_at_end(body_bb);
            }
            let v = self.expr(&arm.body)?;
            if let (Some(slot), Some(v)) = (result, v) {
                self.builder.build_store(slot, v).map_err(llvm_err)?;
            }
            self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;
            self.builder.position_at_end(next);
        }
        // The checker proved the arms exhaustive.
        self.builder.build_unreachable().map_err(llvm_err)?;
        self.builder.position_at_end(merge);
        match (result, self.llvm_ty(ty)) {
            (Some(slot), Some(t)) => Ok(Some(self.builder.build_load(t, slot, "").map_err(llvm_err)?)),
            _ => Ok(None),
        }
    }

    /// Whether the value at `place` (of type `ty`) matches `pat`. Reads memory only; never has side effects.
    fn pat_test(&mut self, pat: &Pat, place: Option<PointerValue<'ctx>>, ty: &Ty) -> GenResult<IntValue<'ctx>> {
        let bool_ty = self.cx.bool_type();
        let yes = bool_ty.const_int(1, false);
        let load_int = |cg: &mut Self| -> GenResult<IntValue<'ctx>> {
            let t = cg.llvm_ty(ty).expect("a literal pattern has a scalar type");
            let ptr = place.expect("a scalar has storage");
            Ok(cg.builder.build_load(t, ptr, "").map_err(llvm_err)?.into_int_value())
        };
        match pat {
            Pat::Wild | Pat::Binding(_) => Ok(yes),
            Pat::At(_, inner) => self.pat_test(inner, place, ty),
            Pat::Bool(b) => {
                let v = load_int(self)?;
                let c = bool_ty.const_int(u64::from(*b), false);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Int(n) => {
                let v = load_int(self)?;
                let c = v.get_type().const_int(*n as u64, false);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Char(ch) => {
                let v = load_int(self)?;
                let c = v.get_type().const_int(u64::from(*ch as u32), false);
                self.builder.build_int_compare(IntPredicate::EQ, v, c, "").map_err(llvm_err)
            }
            Pat::Range(lo, hi) => {
                let v = load_int(self)?;
                let signed = matches!(ty, Ty::Int(i) if i.signed());
                let (ge, le) = if signed {
                    (IntPredicate::SGE, IntPredicate::SLE)
                } else {
                    (IntPredicate::UGE, IntPredicate::ULE)
                };
                let lo_c = v.get_type().const_int(*lo as u64, false);
                let hi_c = v.get_type().const_int(*hi as u64, false);
                let a = self.builder.build_int_compare(ge, v, lo_c, "").map_err(llvm_err)?;
                let b = self.builder.build_int_compare(le, v, hi_c, "").map_err(llvm_err)?;
                self.builder.build_and(a, b, "").map_err(llvm_err)
            }
            Pat::Or(alts) => {
                let mut acc = bool_ty.const_int(0, false);
                for alt in alts {
                    let t = self.pat_test(alt, place, ty)?;
                    acc = self.builder.build_or(acc, t, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Pat::Product(subs) => {
                let (Some(ptr), Some(t)) = (place, self.llvm_ty(ty)) else { return Ok(yes) };
                let tys = self.field_tys(ty);
                let mut acc = yes;
                for (i, sub) in subs.iter().enumerate() {
                    if matches!(sub, Pat::Wild | Pat::Binding(_)) {
                        continue;
                    }
                    let p = self.builder.build_struct_gep(t.into_struct_type(), ptr, i as u32, "").map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &tys[i])?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Pat::Variant(index, subs) => {
                let Ty::Enum(a) = ty else { unreachable!("checked: a variant pattern on an enum") };
                let et = self.enum_ty(a.id);
                let ptr = place.expect("an enum has storage");
                let tag_ptr = self.builder.build_struct_gep(et, ptr, 0, "").map_err(llvm_err)?;
                let i32_ty = self.cx.i32_type();
                let tag = self.builder.build_load(i32_ty, tag_ptr, "tag").map_err(llvm_err)?.into_int_value();
                let want = i32_ty.const_int(*index as u64, false);
                let tag_ok = self.builder.build_int_compare(IntPredicate::EQ, tag, want, "").map_err(llvm_err)?;
                if subs.iter().all(|s| matches!(s, Pat::Wild | Pat::Binding(_))) {
                    return Ok(tag_ok);
                }
                let vt = self.variant_ty(a.id, *index);
                let tys = self.variant_tys(a.id, *index);
                let payload = self.builder.build_struct_gep(et, ptr, 1, "").map_err(llvm_err)?;
                let mut acc = yes;
                for (i, sub) in subs.iter().enumerate() {
                    if matches!(sub, Pat::Wild | Pat::Binding(_)) {
                        continue;
                    }
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    let ok = self.pat_test(sub, Some(p), &tys[i])?;
                    acc = self.builder.build_and(acc, ok, "").map_err(llvm_err)?;
                }
                // `select`, not `and`: the payload belongs to another variant when the tag differs.
                let no = bool_ty.const_int(0, false);
                Ok(self.builder.build_select(tag_ok, acc, no, "").map_err(llvm_err)?.into_int_value())
            }
            Pat::None | Pat::Some(_) => {
                let Ty::Option(inner) = ty else { unreachable!("checked: an Option pattern on an Option") };
                let opt_ty = self.option_ty(inner);
                let ptr = place.expect("an Option has storage");
                let tag_ptr = self.builder.build_struct_gep(opt_ty, ptr, 0, "").map_err(llvm_err)?;
                let tag = self.builder.build_load(bool_ty, tag_ptr, "is_some").map_err(llvm_err)?.into_int_value();
                match pat {
                    Pat::None => self.builder.build_not(tag, "").map_err(llvm_err),
                    Pat::Some(sub) => {
                        let payload = if self.llvm_ty(inner).is_some() {
                            Some(self.builder.build_struct_gep(opt_ty, ptr, 1, "").map_err(llvm_err)?)
                        } else {
                            None
                        };
                        let sub_ok = self.pat_test(sub, payload, inner)?;
                        // `select`, not `and`: the payload of `None` is uninitialized.
                        let no = bool_ty.const_int(0, false);
                        Ok(self.builder.build_select(tag, sub_ok, no, "").map_err(llvm_err)?.into_int_value())
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Stores the parts of the value at `place` into the bindings of `pat` (which matched).
    fn pat_bind(&mut self, pat: &Pat, place: Option<PointerValue<'ctx>>, ty: &Ty) -> GenResult<()> {
        match pat {
            Pat::Binding(id) => self.bind(*id, place, ty),
            Pat::At(id, inner) => {
                self.bind(*id, place, ty)?;
                self.pat_bind(inner, place, ty)
            }
            Pat::Some(sub) => {
                let Ty::Option(inner) = ty else { unreachable!() };
                let payload = match (place, self.llvm_ty(inner)) {
                    (Some(ptr), Some(_)) => {
                        Some(self.builder.build_struct_gep(self.option_ty(inner), ptr, 1, "").map_err(llvm_err)?)
                    }
                    _ => None,
                };
                self.pat_bind(sub, payload, inner)
            }
            Pat::Product(subs) => {
                let (Some(ptr), Some(t)) = (place, self.llvm_ty(ty)) else { return Ok(()) };
                let tys = self.field_tys(ty);
                for (i, sub) in subs.iter().enumerate() {
                    let p = self.builder.build_struct_gep(t.into_struct_type(), ptr, i as u32, "").map_err(llvm_err)?;
                    self.pat_bind(sub, Some(p), &tys[i])?;
                }
                Ok(())
            }
            Pat::Variant(index, subs) => {
                let Ty::Enum(a) = ty else { unreachable!() };
                let Some(ptr) = place else { return Ok(()) };
                if subs.is_empty() {
                    return Ok(());
                }
                let payload = self.builder.build_struct_gep(self.enum_ty(a.id), ptr, 1, "").map_err(llvm_err)?;
                let vt = self.variant_ty(a.id, *index);
                let tys = self.variant_tys(a.id, *index);
                for (i, sub) in subs.iter().enumerate() {
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    self.pat_bind(sub, Some(p), &tys[i])?;
                }
                Ok(())
            }
            // Or-patterns have no bindings (checked).
            _ => Ok(()),
        }
    }

    fn bind(&mut self, id: tir::LocalId, place: Option<PointerValue<'ctx>>, ty: &Ty) -> GenResult<()> {
        if let (Some(slot), Some(ptr), Some(t)) = (self.slots[id.0], place, self.llvm_ty(ty)) {
            let v = self.builder.build_load(t, ptr, "").map_err(llvm_err)?;
            self.builder.build_store(slot, v).map_err(llvm_err)?;
        }
        Ok(())
    }

    /// The address of a place expression (a local or an element of one). Other expressions are evaluated into a
    /// temporary slot. `None` for values without storage.
    fn place_ptr(&mut self, e: &'a tir::Expr) -> GenResult<Option<PointerValue<'ctx>>> {
        match &e.kind {
            tir::ExprKind::Local(id) => Ok(self.slots[id.0]),
            tir::ExprKind::Field(base, index) => {
                let base_ptr = self.place_ptr(base)?;
                let (Some(ptr), Some(t)) = (base_ptr, self.llvm_ty(&base.ty)) else { return Ok(None) };
                let field =
                    self.builder.build_struct_gep(t.into_struct_type(), ptr, *index as u32, "").map_err(llvm_err)?;
                Ok(Some(field))
            }
            tir::ExprKind::Index(base, index) => {
                let Ty::Array(_, len) = &base.ty else {
                    // The base diverged; this code is unreachable.
                    self.expr(base)?;
                    self.expr(index)?;
                    return Ok(None);
                };
                let base_ptr = self.place_ptr(base)?;
                let i = self.expr(index)?;
                let i = self.need_int(i, &Ty::Int(IntTy::Usize));
                let n = self.usize_ty.const_int(*len, false);
                let in_bounds = self.builder.build_int_compare(IntPredicate::ULT, i, n, "").map_err(llvm_err)?;
                let ok = self.new_block("index.ok");
                let fail = self.new_block("index.fail");
                self.builder.build_conditional_branch(in_bounds, ok, fail).map_err(llvm_err)?;
                self.builder.position_at_end(fail);
                let (file, line, col) = self.location(e.span);
                let i64_ty = self.cx.i64_type();
                let i_wide = self.builder.build_int_z_extend_or_bit_cast(i, i64_ty, "").map_err(llvm_err)?;
                let args = [
                    i_wide.into(),
                    i64_ty.const_int(*len, false).into(),
                    file.0.into(),
                    file.1.into(),
                    line.into(),
                    col.into(),
                ];
                self.builder.build_call(self.rt.panic_bounds, &args, "").map_err(llvm_err)?;
                self.builder.build_unreachable().map_err(llvm_err)?;
                self.builder.position_at_end(ok);
                let (Some(base_ptr), Some(array_t)) = (base_ptr, self.llvm_ty(&base.ty)) else { return Ok(None) };
                let zero = self.usize_ty.const_int(0, false);
                // SAFETY (LLVM): the index was checked against the array length above.
                let ptr =
                    unsafe { self.builder.build_in_bounds_gep(array_t, base_ptr, &[zero, i], "") }.map_err(llvm_err)?;
                Ok(Some(ptr))
            }
            _ => {
                let v = self.expr(e)?;
                match self.llvm_ty(&e.ty) {
                    Some(t) => {
                        let slot = self.alloca(t, "tmp")?;
                        let v = self.need(v, &e.ty);
                        self.builder.build_store(slot, v).map_err(llvm_err)?;
                        Ok(Some(slot))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    fn repeat(
        &mut self,
        v: BasicValueEnum<'ctx>,
        elem_t: BasicTypeEnum<'ctx>,
        array_t: BasicTypeEnum<'ctx>,
        n: u64,
    ) -> GenResult<BasicValueEnum<'ctx>> {
        let slot = self.alloca(array_t, "repeat")?;
        let counter = self.alloca(self.usize_ty.into(), "repeat.i")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("repeat.cond");
        let body = self.new_block("repeat.body");
        let exit = self.new_block("repeat.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "").map_err(llvm_err)?.into_int_value();
        let n_v = self.usize_ty.const_int(n, false);
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, n_v, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body, exit).map_err(llvm_err)?;
        self.builder.position_at_end(body);
        // SAFETY (LLVM): i < n.
        let ptr = unsafe { self.builder.build_in_bounds_gep(elem_t, slot, &[i], "") }.map_err(llvm_err)?;
        self.builder.build_store(ptr, v).map_err(llvm_err)?;
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        self.builder.build_load(array_t, slot, "").map_err(llvm_err)
    }

    /// Stores a value in a fresh stack slot and returns its address (`None` for types without a value).
    fn spill(&mut self, v: Option<BasicValueEnum<'ctx>>, ty: &Ty) -> GenResult<Option<PointerValue<'ctx>>> {
        let Some(t) = self.llvm_ty(ty) else { return Ok(None) };
        let slot = self.alloca(t, "tmp")?;
        let v = self.need(v, ty);
        self.builder.build_store(slot, v).map_err(llvm_err)?;
        Ok(Some(slot))
    }

    /// A tuple or struct value from field indices and values (in evaluation order).
    fn product(&mut self, fields: &[(usize, &'a tir::Expr)], ty: &Ty) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let mut values = Vec::new();
        for (i, e) in fields {
            let v = self.expr(e)?;
            values.push((*i, v, &e.ty));
        }
        let Some(t) = self.llvm_ty(ty) else { return Ok(None) };
        let mut agg = undef(t).into_struct_value();
        for (i, v, fty) in values {
            if self.llvm_ty(fty).is_some() {
                let v = self.need(v, fty);
                agg = self.builder.build_insert_value(agg, v, i as u32, "").map_err(llvm_err)?.into_struct_value();
            }
        }
        Ok(Some(agg.into()))
    }

    fn variant(
        &mut self,
        index: usize,
        fields: &'a [(usize, tir::Expr)],
        ty: &Ty,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let Ty::Enum(a) = ty else { unreachable!("a variant has an enum type") };
        let mut values = Vec::new();
        for (i, e) in fields {
            let v = self.expr(e)?;
            values.push((*i, v, &e.ty));
        }
        let et = self.enum_ty(a.id);
        let slot = self.alloca(et.into(), "variant")?;
        let tag_ptr = self.builder.build_struct_gep(et, slot, 0, "").map_err(llvm_err)?;
        self.builder.build_store(tag_ptr, self.cx.i32_type().const_int(index as u64, false)).map_err(llvm_err)?;
        if !values.is_empty() {
            let payload = self.builder.build_struct_gep(et, slot, 1, "").map_err(llvm_err)?;
            let vt = self.variant_ty(a.id, index);
            for (i, v, fty) in values {
                if self.llvm_ty(fty).is_some() {
                    let v = self.need(v, fty);
                    let p = self.builder.build_struct_gep(vt, payload, i as u32, "").map_err(llvm_err)?;
                    self.builder.build_store(p, v).map_err(llvm_err)?;
                }
            }
        }
        Ok(Some(self.builder.build_load(et, slot, "").map_err(llvm_err)?))
    }

    /// `for pattern in array`: binds each element in order.
    fn for_array(&mut self, pat: &'a Pat, array: &'a tir::Expr, body: &'a tir::Block) -> GenResult<()> {
        let Ty::Array(elem, n) = &array.ty else {
            // The array expression diverges; nothing after it runs.
            self.expr(array)?;
            return Ok(());
        };
        let base = self.place_ptr(array)?;
        let array_t = self.llvm_ty(&array.ty);
        let counter = self.alloca(self.usize_ty.into(), "for.index")?;
        self.builder.build_store(counter, self.usize_ty.const_zero()).map_err(llvm_err)?;
        let header = self.new_block("for.cond");
        let body_bb = self.new_block("for.body");
        let latch = self.new_block("for.next");
        let exit = self.new_block("for.end");
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;

        self.builder.position_at_end(header);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let len = self.usize_ty.const_int(*n, false);
        let go = self.builder.build_int_compare(IntPredicate::ULT, i, len, "").map_err(llvm_err)?;
        self.builder.build_conditional_branch(go, body_bb, exit).map_err(llvm_err)?;

        self.builder.position_at_end(body_bb);
        let elem_ptr = match (base, array_t) {
            (Some(base), Some(t)) => {
                let zero = self.usize_ty.const_zero();
                // SAFETY (LLVM): i < n.
                Some(unsafe { self.builder.build_in_bounds_gep(t, base, &[zero, i], "") }.map_err(llvm_err)?)
            }
            _ => None,
        };
        self.pat_bind(pat, elem_ptr, elem)?;
        self.loop_body(body, latch, exit)?;
        self.builder.build_unconditional_branch(latch).map_err(llvm_err)?;

        self.builder.position_at_end(latch);
        let i = self.builder.build_load(self.usize_ty, counter, "i").map_err(llvm_err)?.into_int_value();
        let next = self.builder.build_int_add(i, self.usize_ty.const_int(1, false), "").map_err(llvm_err)?;
        self.builder.build_store(counter, next).map_err(llvm_err)?;
        self.builder.build_unconditional_branch(header).map_err(llvm_err)?;
        self.builder.position_at_end(exit);
        Ok(())
    }

    /// Whether the values at `l` and `r` (of type `ty`) are equal, field by field.
    fn eq_at(&mut self, l: PointerValue<'ctx>, r: PointerValue<'ctx>, ty: &Ty) -> GenResult<IntValue<'ctx>> {
        let bool_ty = self.cx.bool_type();
        let yes = bool_ty.const_int(1, false);
        let Some(t) = self.llvm_ty(ty) else { return Ok(yes) };
        match ty {
            Ty::Int(_) | Ty::Bool | Ty::Char => {
                let a = self.builder.build_load(t, l, "").map_err(llvm_err)?.into_int_value();
                let b = self.builder.build_load(t, r, "").map_err(llvm_err)?.into_int_value();
                self.builder.build_int_compare(IntPredicate::EQ, a, b, "").map_err(llvm_err)
            }
            Ty::Float(_) => {
                let a = self.builder.build_load(t, l, "").map_err(llvm_err)?.into_float_value();
                let b = self.builder.build_load(t, r, "").map_err(llvm_err)?.into_float_value();
                self.builder.build_float_compare(FloatPredicate::OEQ, a, b, "").map_err(llvm_err)
            }
            Ty::Tuple(_) | Ty::Struct(_) => {
                let st = t.into_struct_type();
                let mut acc = yes;
                for (i, fty) in self.field_tys(ty).iter().enumerate() {
                    let a = self.builder.build_struct_gep(st, l, i as u32, "").map_err(llvm_err)?;
                    let b = self.builder.build_struct_gep(st, r, i as u32, "").map_err(llvm_err)?;
                    let eq = self.eq_at(a, b, fty)?;
                    acc = self.builder.build_and(acc, eq, "").map_err(llvm_err)?;
                }
                Ok(acc)
            }
            Ty::Option(inner) => {
                let ot = t.into_struct_type();
                let a_tag = self.builder.build_struct_gep(ot, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(ot, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(bool_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(bool_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tags = self.builder.build_int_compare(IntPredicate::EQ, a_tag, b_tag, "").map_err(llvm_err)?;
                if self.llvm_ty(inner).is_none() {
                    return Ok(tags);
                }
                let a = self.builder.build_struct_gep(ot, l, 1, "").map_err(llvm_err)?;
                let b = self.builder.build_struct_gep(ot, r, 1, "").map_err(llvm_err)?;
                let payload = self.eq_at(a, b, inner)?;
                let payload = self.builder.build_select(a_tag, payload, yes, "").map_err(llvm_err)?.into_int_value();
                self.builder.build_and(tags, payload, "").map_err(llvm_err)
            }
            Ty::Enum(a) => {
                let et = t.into_struct_type();
                let i32_ty = self.cx.i32_type();
                let a_tag = self.builder.build_struct_gep(et, l, 0, "").map_err(llvm_err)?;
                let b_tag = self.builder.build_struct_gep(et, r, 0, "").map_err(llvm_err)?;
                let a_tag = self.builder.build_load(i32_ty, a_tag, "").map_err(llvm_err)?.into_int_value();
                let b_tag = self.builder.build_load(i32_ty, b_tag, "").map_err(llvm_err)?.into_int_value();
                let tags = self.builder.build_int_compare(IntPredicate::EQ, a_tag, b_tag, "").map_err(llvm_err)?;
                let mut payload = yes;
                for vi in 0..self.program.enums[a.id].variants.len() {
                    let tys = self.variant_tys(a.id, vi);
                    if tys.is_empty() {
                        continue;
                    }
                    let vt = self.variant_ty(a.id, vi);
                    let lp = self.builder.build_struct_gep(et, l, 1, "").map_err(llvm_err)?;
                    let rp = self.builder.build_struct_gep(et, r, 1, "").map_err(llvm_err)?;
                    let mut acc = yes;
                    for (i, fty) in tys.iter().enumerate() {
                        let a = self.builder.build_struct_gep(vt, lp, i as u32, "").map_err(llvm_err)?;
                        let b = self.builder.build_struct_gep(vt, rp, i as u32, "").map_err(llvm_err)?;
                        let eq = self.eq_at(a, b, fty)?;
                        acc = self.builder.build_and(acc, eq, "").map_err(llvm_err)?;
                    }
                    let is_this = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, a_tag, i32_ty.const_int(vi as u64, false), "")
                        .map_err(llvm_err)?;
                    payload = self.builder.build_select(is_this, acc, payload, "").map_err(llvm_err)?.into_int_value();
                }
                self.builder.build_and(tags, payload, "").map_err(llvm_err)
            }
            _ => Err(format!("unsupported `==` on `{ty}`")),
        }
    }

    // ----- operators -------------------------------------------------------------

    fn binary(
        &mut self,
        op: BinOp,
        l: &'a tir::Expr,
        r: &'a tir::Expr,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        if matches!(op, BinOp::And | BinOp::Or) {
            return self.short_circuit(op, l, r).map(Some);
        }
        let operand_ty = if l.ty == Ty::Never { &r.ty } else { &l.ty };
        let lv = self.expr(l)?;
        let rv = self.expr(r)?;
        if matches!(op, BinOp::Eq | BinOp::Ne)
            && matches!(operand_ty, Ty::Unit | Ty::Tuple(_) | Ty::Struct(_) | Ty::Enum(_) | Ty::Option(_))
        {
            let lp = self.spill(lv, operand_ty)?;
            let rp = self.spill(rv, operand_ty)?;
            let eq = match (lp, rp) {
                (Some(lp), Some(rp)) => self.eq_at(lp, rp, operand_ty)?,
                _ => self.cx.bool_type().const_int(1, false),
            };
            let v = if op == BinOp::Ne { self.builder.build_not(eq, "").map_err(llvm_err)? } else { eq };
            return Ok(Some(v.into()));
        }
        if self.llvm_ty(operand_ty).is_none() {
            return Ok(None);
        }
        let lv = self.need(lv, operand_ty);
        let rv = self.need(rv, &r.ty);
        let b = &self.builder;
        let v: BasicValueEnum<'ctx> = match operand_ty {
            Ty::Float(_) => {
                let (x, y) = (lv.into_float_value(), rv.into_float_value());
                let pred = match op {
                    BinOp::Add => return Ok(Some(b.build_float_add(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Sub => return Ok(Some(b.build_float_sub(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Mul => return Ok(Some(b.build_float_mul(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Div => return Ok(Some(b.build_float_div(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Rem => return Ok(Some(b.build_float_rem(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Eq => FloatPredicate::OEQ,
                    BinOp::Ne => FloatPredicate::UNE,
                    BinOp::Lt => FloatPredicate::OLT,
                    BinOp::Gt => FloatPredicate::OGT,
                    BinOp::Le => FloatPredicate::OLE,
                    BinOp::Ge => FloatPredicate::OGE,
                    _ => unreachable!("checked: no bit operators on floats"),
                };
                b.build_float_compare(pred, x, y, "").map_err(llvm_err)?.into()
            }
            _ => {
                let (x, y) = (lv.into_int_value(), rv.into_int_value());
                let int = match operand_ty {
                    Ty::Int(i) => Some(*i),
                    _ => None,
                };
                let signed = int.is_some_and(|i| i.signed());
                let cmp = |pred_s, pred_u| if signed { pred_s } else { pred_u };
                let pred = match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul => {
                        let i = int.expect("checked: arithmetic on integers");
                        return Ok(Some(self.arith(op, x, y, i, span)?.into()));
                    }
                    BinOp::Div | BinOp::Rem => {
                        let i = int.expect("checked: arithmetic on integers");
                        return Ok(Some(self.div_rem(op, x, y, i, span)?.into()));
                    }
                    BinOp::Shl | BinOp::Shr => {
                        let i = int.expect("checked: shifts on integers");
                        return Ok(Some(self.shift(op, x, y, i, span)?.into()));
                    }
                    BinOp::BitAnd => return Ok(Some(b.build_and(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::BitOr => return Ok(Some(b.build_or(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::BitXor => return Ok(Some(b.build_xor(x, y, "").map_err(llvm_err)?.into())),
                    BinOp::Eq => IntPredicate::EQ,
                    BinOp::Ne => IntPredicate::NE,
                    BinOp::Lt => cmp(IntPredicate::SLT, IntPredicate::ULT),
                    BinOp::Gt => cmp(IntPredicate::SGT, IntPredicate::UGT),
                    BinOp::Le => cmp(IntPredicate::SLE, IntPredicate::ULE),
                    BinOp::Ge => cmp(IntPredicate::SGE, IntPredicate::UGE),
                    BinOp::And | BinOp::Or => unreachable!("handled above"),
                };
                b.build_int_compare(pred, x, y, "").map_err(llvm_err)?.into()
            }
        };
        Ok(Some(v))
    }

    fn short_circuit(&mut self, op: BinOp, l: &'a tir::Expr, r: &'a tir::Expr) -> GenResult<BasicValueEnum<'ctx>> {
        let lv = self.expr(l)?;
        let lv = self.need_int(lv, &Ty::Bool);
        let left_end = self.builder.get_insert_block().expect("positioned");
        let rhs_bb = self.new_block("logic.rhs");
        let merge = self.new_block("logic.end");
        match op {
            BinOp::And => self.builder.build_conditional_branch(lv, rhs_bb, merge),
            _ => self.builder.build_conditional_branch(lv, merge, rhs_bb),
        }
        .map_err(llvm_err)?;
        self.builder.position_at_end(rhs_bb);
        let rv = self.expr(r)?;
        let rv = self.need_int(rv, &Ty::Bool);
        let right_end = self.builder.get_insert_block().expect("positioned");
        self.builder.build_unconditional_branch(merge).map_err(llvm_err)?;
        self.builder.position_at_end(merge);
        let phi = self.builder.build_phi(self.cx.bool_type(), "").map_err(llvm_err)?;
        let short = self.cx.bool_type().const_int(u64::from(op == BinOp::Or), false);
        phi.add_incoming(&[(&short, left_end), (&rv, right_end)]);
        Ok(phi.as_basic_value())
    }

    /// `+`, `-`, `*`: panics on overflow in debug builds, wraps in release builds.
    fn arith(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        if self.release {
            return self.wrapping(op, x, y);
        }
        let (v, overflow) = self.with_overflow(op, x, y, i)?;
        let verb = match op {
            BinOp::Add => "add",
            BinOp::Sub => "subtract",
            _ => "multiply",
        };
        let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
        self.check(ok, &format!("attempt to {verb} with overflow"), span)?;
        Ok(v)
    }

    fn wrapping(&mut self, op: BinOp, x: IntValue<'ctx>, y: IntValue<'ctx>) -> GenResult<IntValue<'ctx>> {
        let b = &self.builder;
        match op {
            BinOp::Add => b.build_int_add(x, y, ""),
            BinOp::Sub => b.build_int_sub(x, y, ""),
            _ => b.build_int_mul(x, y, ""),
        }
        .map_err(llvm_err)
    }

    /// The wrapped result and whether the operation overflowed.
    fn with_overflow(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
    ) -> GenResult<(IntValue<'ctx>, IntValue<'ctx>)> {
        let prefix = if i.signed() { "s" } else { "u" };
        let name = match op {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            _ => "mul",
        };
        let intrinsic =
            Intrinsic::find(&format!("llvm.{prefix}{name}.with.overflow")).ok_or("missing LLVM overflow intrinsic")?;
        let decl = intrinsic
            .get_declaration(self.module, &[x.get_type().into()])
            .ok_or("cannot declare LLVM overflow intrinsic")?;
        let call = self.builder.build_call(decl, &[x.into(), y.into()], "").map_err(llvm_err)?;
        let pair = call.try_as_basic_value().basic().ok_or("overflow intrinsic returns a value")?.into_struct_value();
        let v = self.builder.build_extract_value(pair, 0, "").map_err(llvm_err)?.into_int_value();
        let overflow = self.builder.build_extract_value(pair, 1, "").map_err(llvm_err)?.into_int_value();
        Ok((v, overflow))
    }

    /// `/` and `%`: truncate toward zero; division by zero always panics; `MIN / -1` overflows.
    fn div_rem(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        let t = x.get_type();
        let nonzero = self.builder.build_int_compare(IntPredicate::NE, y, t.const_zero(), "").map_err(llvm_err)?;
        self.check(nonzero, "division by zero", span)?;
        let mut divisor = y;
        if i.signed() {
            let min = t.const_int(i.min() as u64, false);
            let minus_one = t.const_all_ones();
            let is_min = self.builder.build_int_compare(IntPredicate::EQ, x, min, "").map_err(llvm_err)?;
            let is_m1 = self.builder.build_int_compare(IntPredicate::EQ, y, minus_one, "").map_err(llvm_err)?;
            let overflow = self.builder.build_and(is_min, is_m1, "").map_err(llvm_err)?;
            if self.release {
                // MIN / 1 = MIN and MIN % 1 = 0 are the wrapped results of MIN / -1 and MIN % -1.
                divisor = self
                    .builder
                    .build_select(overflow, t.const_int(1, false), y, "")
                    .map_err(llvm_err)?
                    .into_int_value();
            } else {
                let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                let what = if op == BinOp::Div { "divide" } else { "calculate the remainder" };
                self.check(ok, &format!("attempt to {what} with overflow"), span)?;
            }
        }
        let b = &self.builder;
        match (op, i.signed()) {
            (BinOp::Div, true) => b.build_int_signed_div(x, divisor, ""),
            (BinOp::Div, false) => b.build_int_unsigned_div(x, divisor, ""),
            (_, true) => b.build_int_signed_rem(x, divisor, ""),
            (_, false) => b.build_int_unsigned_rem(x, divisor, ""),
        }
        .map_err(llvm_err)
    }

    /// `<<` and `>>`: a shift by the bit width or more always panics. `>>` is arithmetic for signed types.
    fn shift(
        &mut self,
        op: BinOp,
        x: IntValue<'ctx>,
        y: IntValue<'ctx>,
        i: IntTy,
        span: Span,
    ) -> GenResult<IntValue<'ctx>> {
        let limit = y.get_type().const_int(u64::from(i.bits()), false);
        // Unsigned comparison: a negative amount is huge and fails too.
        let ok = self.builder.build_int_compare(IntPredicate::ULT, y, limit, "").map_err(llvm_err)?;
        self.check(ok, "attempt to shift with overflow", span)?;
        let amount = self.builder.build_int_cast_sign_flag(y, x.get_type(), false, "").map_err(llvm_err)?;
        match op {
            BinOp::Shl => self.builder.build_left_shift(x, amount, ""),
            _ => self.builder.build_right_shift(x, amount, i.signed(), ""),
        }
        .map_err(llvm_err)
    }

    fn negate(&mut self, v: IntValue<'ctx>, i: IntTy, span: Span) -> GenResult<IntValue<'ctx>> {
        let zero = v.get_type().const_zero();
        if self.release {
            return self.builder.build_int_sub(zero, v, "").map_err(llvm_err);
        }
        let (r, overflow) = self.with_overflow(BinOp::Sub, zero, v, i)?;
        let ok = self.builder.build_not(overflow, "").map_err(llvm_err)?;
        self.check(ok, "attempt to negate with overflow", span)?;
        Ok(r)
    }

    fn cast(&mut self, v: BasicValueEnum<'ctx>, from: &Ty, to: &Ty) -> GenResult<BasicValueEnum<'ctx>> {
        if from == to {
            return Ok(v);
        }
        let target = self.llvm_ty(to).expect("cast targets are scalars");
        let b = &self.builder;
        let signed_from = matches!(from, Ty::Int(i) if i.signed());
        let r: BasicValueEnum<'ctx> = match (from, to) {
            (Ty::Int(_) | Ty::Bool | Ty::Char, Ty::Int(_) | Ty::Char) => b
                .build_int_cast_sign_flag(v.into_int_value(), target.into_int_type(), signed_from, "")
                .map_err(llvm_err)?
                .into(),
            (Ty::Int(_), Ty::Float(_)) => {
                let (x, t) = (v.into_int_value(), target.into_float_type());
                if signed_from {
                    b.build_signed_int_to_float(x, t, "").map_err(llvm_err)?.into()
                } else {
                    b.build_unsigned_int_to_float(x, t, "").map_err(llvm_err)?.into()
                }
            }
            (Ty::Float(_), Ty::Int(i)) => {
                // Saturating, and NaN becomes 0, so the conversion is never undefined.
                let name = if i.signed() { "llvm.fptosi.sat" } else { "llvm.fptoui.sat" };
                let intrinsic = Intrinsic::find(name).ok_or("missing LLVM fpto*i.sat intrinsic")?;
                let decl = intrinsic
                    .get_declaration(self.module, &[target, v.get_type()])
                    .ok_or("cannot declare LLVM fpto*i.sat intrinsic")?;
                let call = self.builder.build_call(decl, &[v.into()], "").map_err(llvm_err)?;
                call.try_as_basic_value().basic().ok_or("fpto*i.sat returns a value")?
            }
            (Ty::Float(_), Ty::Float(_)) => {
                b.build_float_cast(v.into_float_value(), target.into_float_type(), "").map_err(llvm_err)?.into()
            }
            _ => return Err(format!("unsupported cast from `{from}` to `{to}`")),
        };
        Ok(r)
    }

    // ----- built-ins ---------------------------------------------------------------

    fn builtin(&mut self, b: Builtin, args: &'a [tir::Expr], span: Span) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let s = self.expr(&args[0])?;
        let s = self.need(s, &Ty::Str).into_struct_value();
        let ptr = self.builder.build_extract_value(s, 0, "ptr").map_err(llvm_err)?;
        let len = self.builder.build_extract_value(s, 1, "len").map_err(llvm_err)?;
        let (fd, newline) = match b {
            Builtin::Print => (1, 0),
            Builtin::Println => (1, 1),
            Builtin::Eprint => (2, 0),
            Builtin::Eprintln => (2, 1),
            Builtin::Panic => {
                self.emit_panic(ptr, len, span)?;
                self.continue_dead();
                return Ok(None);
            }
        };
        let i32_ty = self.cx.i32_type();
        let args: [BasicMetadataValueEnum; 4] =
            [i32_ty.const_int(fd, false).into(), ptr.into(), len.into(), i32_ty.const_int(newline, false).into()];
        self.builder.build_call(self.rt.print, &args, "").map_err(llvm_err)?;
        Ok(None)
    }

    fn method(
        &mut self,
        m: Method,
        args: &'a [tir::Expr],
        ty: &Ty,
        span: Span,
    ) -> GenResult<Option<BasicValueEnum<'ctx>>> {
        let recv_ty = &args[0].ty;
        let mut values = Vec::new();
        for a in args {
            let v = self.expr(a)?;
            values.push(self.need(v, &a.ty));
        }
        let recv = values[0];
        let v: BasicValueEnum<'ctx> = match (m, recv_ty) {
            (Method::ToString, _) => self.string_of(recv, recv_ty)?,
            (Method::WrappingAdd | Method::WrappingSub | Method::WrappingMul, _) => {
                let op = wrapping_op(m);
                self.wrapping(op, recv.into_int_value(), values[1].into_int_value())?.into()
            }
            (Method::CheckedAdd | Method::CheckedSub | Method::CheckedMul, Ty::Int(i)) => {
                let op = wrapping_op(m);
                let (r, overflow) = self.with_overflow(op, recv.into_int_value(), values[1].into_int_value(), *i)?;
                let Ty::Option(inner) = ty else { unreachable!("checked_* returns an Option") };
                let opt_ty = self.option_ty(inner);
                let is_some = self.builder.build_not(overflow, "").map_err(llvm_err)?;
                let agg = self
                    .builder
                    .build_insert_value(undef(opt_ty.into()).into_struct_value(), is_some, 0, "")
                    .map_err(llvm_err)?;
                self.builder.build_insert_value(agg, r, 1, "").map_err(llvm_err)?.into_struct_value().into()
            }
            (Method::Min | Method::Max, Ty::Int(i)) => {
                let (x, y) = (recv.into_int_value(), values[1].into_int_value());
                let pred = match (m, i.signed()) {
                    (Method::Min, true) => IntPredicate::SLT,
                    (Method::Min, false) => IntPredicate::ULT,
                    (_, true) => IntPredicate::SGT,
                    (_, false) => IntPredicate::UGT,
                };
                let c = self.builder.build_int_compare(pred, x, y, "").map_err(llvm_err)?;
                self.builder.build_select(c, x, y, "").map_err(llvm_err)?
            }
            (Method::Abs, Ty::Int(i)) => {
                let x = recv.into_int_value();
                let neg = self.negate(x, *i, span)?;
                let is_neg = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, x, x.get_type().const_zero(), "")
                    .map_err(llvm_err)?;
                self.builder.build_select(is_neg, neg, x, "").map_err(llvm_err)?
            }
            _ => return Err(format!("unsupported method {m:?} on `{recv_ty}`")),
        };
        Ok(Some(v))
    }

    fn string_of(&mut self, v: BasicValueEnum<'ctx>, ty: &Ty) -> GenResult<BasicValueEnum<'ctx>> {
        let out = self.alloca(self.string_ty.into(), "string")?;
        let i64_ty = self.cx.i64_type();
        match ty {
            Ty::Int(i) => {
                let wide = self
                    .builder
                    .build_int_cast_sign_flag(v.into_int_value(), i64_ty, i.signed(), "")
                    .map_err(llvm_err)?;
                let f = if i.signed() { self.rt.i64_to_string } else { self.rt.u64_to_string };
                self.builder.build_call(f, &[wide.into(), out.into()], "").map_err(llvm_err)?;
            }
            Ty::Char => {
                self.builder.build_call(self.rt.char_to_string, &[v.into(), out.into()], "").map_err(llvm_err)?;
            }
            Ty::Bool | Ty::Str => {
                let s = if *ty == Ty::Bool {
                    let t = self.str_literal(b"true");
                    let f = self.str_literal(b"false");
                    self.builder.build_select(v.into_int_value(), t, f, "").map_err(llvm_err)?.into_struct_value()
                } else {
                    v.into_struct_value()
                };
                let ptr = self.builder.build_extract_value(s, 0, "").map_err(llvm_err)?;
                let len = self.builder.build_extract_value(s, 1, "").map_err(llvm_err)?;
                self.builder
                    .build_call(self.rt.str_to_string, &[ptr.into(), len.into(), out.into()], "")
                    .map_err(llvm_err)?;
            }
            _ => return Err(format!("unsupported to_string on `{ty}`")),
        }
        self.builder.build_load(self.string_ty, out, "").map_err(llvm_err)
    }

    // ----- panics --------------------------------------------------------------------

    /// The file path (as a `str` pointer and length), line, and column of `span`, for the panic functions.
    fn location(
        &mut self,
        span: Span,
    ) -> ((BasicValueEnum<'ctx>, BasicValueEnum<'ctx>), IntValue<'ctx>, IntValue<'ctx>) {
        let file = self.sources.get(span.file);
        let (line, col) = file.line_col(span.lo);
        let path = self.str_literal(file.path.clone().as_bytes());
        let ptr = path.get_field_at_index(0).expect("str has a pointer");
        let len = path.get_field_at_index(1).expect("str has a length");
        let i32_ty = self.cx.i32_type();
        ((ptr, len), i32_ty.const_int(u64::from(line), false), i32_ty.const_int(u64::from(col), false))
    }

    /// Calls the runtime's panic function and terminates the current block.
    fn emit_panic(
        &mut self,
        msg_ptr: BasicValueEnum<'ctx>,
        msg_len: BasicValueEnum<'ctx>,
        span: Span,
    ) -> GenResult<()> {
        let (file, line, col) = self.location(span);
        let args = [msg_ptr.into(), msg_len.into(), file.0.into(), file.1.into(), line.into(), col.into()];
        self.builder.build_call(self.rt.panic, &args, "").map_err(llvm_err)?;
        self.builder.build_unreachable().map_err(llvm_err)?;
        Ok(())
    }

    /// Panics with `msg` unless `ok` holds.
    fn check(&mut self, ok: IntValue<'ctx>, msg: &str, span: Span) -> GenResult<()> {
        let pass = self.new_block("check.ok");
        let fail = self.new_block("check.fail");
        self.builder.build_conditional_branch(ok, pass, fail).map_err(llvm_err)?;
        self.builder.position_at_end(fail);
        let m = self.str_literal(msg.as_bytes());
        let ptr = m.get_field_at_index(0).expect("str has a pointer");
        let len = m.get_field_at_index(1).expect("str has a length");
        self.emit_panic(ptr, len, span)?;
        self.builder.position_at_end(pass);
        Ok(())
    }

    // ----- strings -------------------------------------------------------------------

    fn make_str(&mut self, ptr: BasicValueEnum<'ctx>, len: BasicValueEnum<'ctx>) -> GenResult<StructValue<'ctx>> {
        let s = undef(self.str_ty.into()).into_struct_value();
        let s = self.builder.build_insert_value(s, ptr, 0, "").map_err(llvm_err)?;
        Ok(self.builder.build_insert_value(s, len, 1, "").map_err(llvm_err)?.into_struct_value())
    }

    /// A `str` constant pointing to a private global holding the bytes (no terminating NUL).
    fn str_literal(&mut self, bytes: &[u8]) -> StructValue<'ctx> {
        if let Some(s) = self.str_literals.get(bytes) {
            return *s;
        }
        let data = self.cx.const_string(bytes, false);
        let global = self.module.add_global(data.get_type(), None, &format!("str.{}", self.str_literals.len()));
        global.set_initializer(&data);
        global.set_constant(true);
        global.set_linkage(Linkage::Private);
        global.set_unnamed_addr(true);
        let len = self.usize_ty.const_int(bytes.len() as u64, false);
        let s = self.str_ty.const_named_struct(&[global.as_pointer_value().into(), len.into()]);
        self.str_literals.insert(bytes.to_vec(), s);
        s
    }
}

fn wrapping_op(m: Method) -> BinOp {
    match m {
        Method::WrappingAdd | Method::CheckedAdd => BinOp::Add,
        Method::WrappingSub | Method::CheckedSub => BinOp::Sub,
        _ => BinOp::Mul,
    }
}

/// A local, possibly followed by fields and indexing: its storage outlives the current statement.
fn is_place(e: &tir::Expr) -> bool {
    match &e.kind {
        tir::ExprKind::Local(_) => true,
        tir::ExprKind::Field(base, _) | tir::ExprKind::Index(base, _) => is_place(base),
        _ => false,
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
